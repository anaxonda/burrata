//! Shared, demand-only alias work. No scheduler and no automatic playback.
use crate::{
    acquisition::AcquisitionManager,
    models::Track,
    playback_sources::{wait_for_cancellation, PlaybackDeadline, SourceTrace},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

const NEGATIVE_CACHE_NAMESPACE: &str = "artists.alias-discovery.negative.v4";

struct Flight {
    id: uuid::Uuid,
    users: AtomicUsize,
    stop: watch::Sender<bool>,
    result: watch::Sender<Option<Result<(), String>>>,
}
struct Lease(Arc<Flight>);
impl Drop for Lease {
    fn drop(&mut self) {
        if self.0.users.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.stop.send_replace(true);
        }
    }
}
fn flights() -> &'static Mutex<HashMap<String, Arc<Flight>>> {
    static FLIGHTS: OnceLock<Mutex<HashMap<String, Arc<Flight>>>> = OnceLock::new();
    FLIGHTS.get_or_init(Default::default)
}
fn fingerprint(track: &Track) -> String {
    crate::remote_playlists::alias_hydration_key(track)
        .unwrap_or_else(|_| format!("{:x}", Sha256::digest(track.id.as_bytes())))
}

fn negative_cache_key(original: &Track, hydrated: &Track) -> Option<String> {
    if original.provider == "spotify" {
        // A Spotify no-match is meaningful only for a complete, verified
        // catalog identity. Bind it to the hydrated track fingerprint so a
        // later artist-ID/ISRC enrichment cannot inherit an older negative.
        let identity = crate::artist_credit_aliases::spotify_artist_credit_fingerprint(hydrated)?;
        let bytes = serde_json::to_vec(&identity).ok()?;
        Some(format!("spotify:{:x}", Sha256::digest(bytes)))
    } else {
        Some(fingerprint(original))
    }
}

pub(crate) fn enabled() -> bool {
    std::env::var("BURRATA_ARTIST_ALIAS_DISCOVERY").as_deref() != Ok("0")
}

pub(crate) async fn lookup(
    acquisition: &AcquisitionManager,
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
    trace: &SourceTrace,
) -> Result<(), String> {
    let store = acquisition.alias_store();
    let hash = fingerprint(track);
    // Non-Spotify sources already have complete source identity at this point.
    // Spotify must hydrate before consulting a negative cache entry.
    if track.provider != "spotify"
        && store
            .get_provider_cache(NEGATIVE_CACHE_NAMESPACE, &hash)?
            .is_some()
    {
        trace.event(
            "alias_lookup_skipped",
            &track.provider,
            json!({"reason":"negative_cache"}),
        );
        return Ok(());
    }
    let key = format!("{:p}:{hash}", Arc::as_ptr(&store));
    let acquisition = acquisition.clone();
    let track = track.clone();
    let worker_trace = trace.clone();
    shared(key,cancel,deadline,trace,move |stop, shared_id| async move {
        let worker_trace=worker_trace.with_shared_lookup(shared_id);
        let started=Instant::now();
        let worker_deadline=PlaybackDeadline::after(Duration::from_secs(20));
        worker_trace.event("alias_discovery_start",&track.provider,json!({}));
        let result=tokio::select! {
            biased;
            _=wait_for_cancellation(stop.clone())=>Err("Cancelled".into()),
            _=tokio::time::sleep_until(worker_deadline.instant())=>Err("Alias discovery deadline exceeded".into()),
            result=async {
                let hydrated = if track.provider == "spotify" {
                    if crate::artist_credit_aliases::spotify_artist_credit_fingerprint(&track).is_some() {
                        track.clone()
                    } else if let Some(cached) = crate::remote_playlists::read_alias_hydrated_track(&store, &track) {
                        cached
                    } else {
                        acquisition.prepare_alias_track(&track,stop.clone(),worker_deadline.instant(),&worker_trace).await?
                    }
                } else {
                    track.clone()
                };
                let negative_key = negative_cache_key(&track, &hydrated);
                if let Some(key) = negative_key.as_deref() {
                    if store.get_provider_cache(NEGATIVE_CACHE_NAMESPACE, key)?.is_some() {
                        worker_trace.event("alias_lookup_skipped", &track.provider, json!({"reason":"negative_cache","identity":"complete"}));
                        return Ok(());
                    }
                }
                let report=crate::artist_alias_discovery::discover_verified_artist_aliases_traced(&store,&hydrated,worker_trace.clone()).await;
                worker_trace.event("alias_discovery_work",&track.provider,json!({"metadataRequests":report.stats.requests,"bytes":report.stats.bytes,"queueMs":report.stats.queue_ms,"httpMs":report.stats.http_ms,"lastStatus":report.stats.last_status,"ok":report.result.is_ok()}));
                report.result?;
                if crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&store,&track).has_verified_credit_sets() == false {
                    if let Some(key) = negative_key {
                        store.set_provider_cache(NEGATIVE_CACHE_NAMESPACE,&key,&json!({"outcome":"no_verified_evidence","identity":"complete"}),24*60*60)?;
                    }
                }
                Ok(())
            }=>result,
        };
        worker_trace.event("alias_discovery_end",&track.provider,json!({"durationMs":started.elapsed().as_secs_f64()*1000.0,"ok":result.is_ok(),"cancelled":result.as_ref().err().is_some_and(|e|e=="Cancelled")}));
        result
    }).await
}

async fn shared<F, Fut>(
    key: String,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
    trace: &SourceTrace,
    work: F,
) -> Result<(), String>
where
    F: FnOnce(watch::Receiver<bool>, uuid::Uuid) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Alias lookup deadline exceeded".into());
    }
    let (flight, joined) = {
        let mut map = flights()
            .lock()
            .map_err(|_| "Alias lookup state unavailable")?;
        if let Some(flight) = map.get(&key).filter(|f| !*f.stop.borrow()) {
            flight.users.fetch_add(1, Ordering::AcqRel);
            (flight.clone(), true)
        } else {
            let (stop, _) = watch::channel(false);
            let (result, _) = watch::channel(None);
            let flight = Arc::new(Flight {
                id: uuid::Uuid::new_v4(),
                users: AtomicUsize::new(1),
                stop,
                result,
            });
            map.insert(key.clone(), flight.clone());
            (flight, false)
        }
    };
    let _lease = Lease(flight.clone());
    let trace = trace.clone().with_shared_lookup(flight.id);
    trace.event("alias_lookup_start", "other", json!({"joined":joined}));
    if !joined {
        let task = flight.clone();
        tokio::spawn(async move {
            let result = work(task.stop.subscribe(), task.id).await;
            task.result.send_replace(Some(result));
            if let Ok(mut map) = flights().lock() {
                if map
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &task))
                {
                    map.remove(&key);
                }
            }
        });
    }
    let started = Instant::now();
    let mut receiver = flight.result.subscribe();
    let result = tokio::select! {
        biased;
        _=wait_for_cancellation(cancel)=>Err("Cancelled".into()),
        _=tokio::time::sleep_until(deadline.instant())=>Err("Alias lookup deadline exceeded".into()),
        result=async { loop { if let Some(result)=receiver.borrow().clone(){return result;} if receiver.changed().await.is_err(){return Err("Alias lookup stopped".into());} } }=>result,
    };
    trace.event("alias_lookup_end","other",json!({"durationMs":started.elapsed().as_secs_f64()*1000.0,"ok":result.is_ok(),"outcome":if result.is_ok(){"completed"}else if result.as_ref().err().is_some_and(|e|e=="Cancelled"){"cancelled"}else{"failed"}}));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spotify_track_with_artist_identity() -> Track {
        let track_id = "0123456789012345678901";
        let artist_id = "abcdefghijklmnopqrstuv";
        let mut track = Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: format!("https://open.spotify.com/track/{track_id}"),
            title: "Example song".into(),
            artist: "Example artist".into(),
            duration_seconds: Some(180.0),
            ..Track::default()
        };
        track.provenance = json!({
            "catalog":{"verified":true,"provider":"spotify","providerId":track_id,
                "artistCount":1,"artists":[{"provider":"spotify","id":artist_id,"name":"Example artist"}]},
            "remoteCatalog":{"trackId":track_id,"title":"Example song","artist":"Example artist",
                "durationSeconds":180.0,"artistCount":1,
                "artistRefs":[{"provider":"spotify","id":artist_id,"name":"Example artist"}]}
        });
        track
    }

    #[test]
    fn spotify_negative_cache_requires_complete_hydrated_identity() {
        let mut incomplete = spotify_track_with_artist_identity();
        incomplete.provenance = json!({});
        assert!(negative_cache_key(&incomplete, &incomplete).is_none());

        let complete = spotify_track_with_artist_identity();
        assert!(negative_cache_key(&complete, &complete).is_some());
    }

    #[test]
    fn spotify_negative_cache_is_bound_to_ordered_artist_identity() {
        let original = spotify_track_with_artist_identity();
        let mut changed = original.clone();
        changed.provenance["catalog"]["artists"][0]["id"] = json!("zyxwvutsrqponmlkjihgfe");
        changed.provenance["remoteCatalog"]["artistRefs"][0]["id"] =
            json!("zyxwvutsrqponmlkjihgfe");

        assert_ne!(
            negative_cache_key(&original, &original),
            negative_cache_key(&original, &changed)
        );
    }

    #[test]
    fn non_spotify_negative_cache_keeps_source_identity_key() {
        let original = Track {
            id: "radio_song:123".into(),
            provider: "radio_song".into(),
            title: "Example song".into(),
            artist: "Example artist".into(),
            ..Track::default()
        };
        assert_eq!(
            negative_cache_key(&original, &original),
            Some(fingerprint(&original))
        );
    }

    #[tokio::test]
    async fn shared_callers_cancel_independently_and_execute_once() {
        let key = uuid::Uuid::new_v4().to_string();
        let trace = SourceTrace::new(None, None, None);
        let (a, ca) = watch::channel(false);
        let (_b, cb) = watch::channel(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let one = calls.clone();
        let first = shared(
            key.clone(),
            ca,
            PlaybackDeadline::after(Duration::from_secs(2)),
            &trace,
            move |_, _| async move {
                one.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            },
        );
        let two = calls.clone();
        let second = shared(
            key,
            cb,
            PlaybackDeadline::after(Duration::from_secs(2)),
            &trace,
            move |_, _| async move {
                two.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let cancellation = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            a.send_replace(true);
        };
        let (r1, r2, _) = tokio::join!(first, second, cancellation);
        assert_eq!(r1, Err("Cancelled".into()));
        assert!(r2.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn expired_caller_starts_no_work() {
        let (_tx, cancel) = watch::channel(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let result = shared(
            uuid::Uuid::new_v4().to_string(),
            cancel,
            PlaybackDeadline::after(Duration::ZERO),
            &SourceTrace::new(None, None, None),
            move |_, _| async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
