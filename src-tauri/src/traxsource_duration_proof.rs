//! Request-scoped duration evidence for a malformed Traxsource capture.
use std::{collections::HashSet, future::Future, time::Duration};

use serde_json::json;
use tokio::{sync::watch, time::Instant};

use crate::{
    acquisition::AcquisitionManager,
    models::Track,
    providers::traxsource::{Traxsource, TraxsourceReleaseEvidence},
    recording_identity::{
        artist_search_fold, canonical_version_key, normalize_isrc, parse_recording_text,
        title_search_fold,
    },
    remote_playlists::spotify_release_evidence::IndependentAlbumEvidence,
    storage::Store,
};

#[path = "traxsource_deezer_duration.rs"]
mod deezer;

const MAX_TRACKS: usize = 40;
const MAX_CANDIDATES: usize = 6;
const SIBLING_DURATION_TOLERANCE: f64 = 2.0;

fn same_text(left: &str, right: &str) -> bool {
    let left = title_search_fold(&crate::matching::normalize(left));
    !left.is_empty() && left == title_search_fold(&crate::matching::normalize(right))
}

fn recording_key(title: &str, version: Option<&str>) -> Option<(String, String)> {
    let title = parse_recording_text(title);
    if title.parsing_incomplete {
        return None;
    }
    let mut versions: HashSet<String> = title
        .semantic_versions()
        .map(canonical_version_key)
        .collect();
    if let Some(version) = version.filter(|value| !value.trim().is_empty()) {
        versions.insert(canonical_version_key(version));
    }
    if versions.len() > 1 {
        return None;
    }
    let title = title_search_fold(&title.normalized_title_key?);
    let version = versions
        .into_iter()
        .next()
        .filter(|name| name != "original mix" && name != "original version")
        .unwrap_or_default();
    (!title.is_empty()).then_some((title, version))
}

fn artist_keys(artists: &[String]) -> Option<HashSet<String>> {
    if artists.is_empty() || artists.len() > 24 {
        return None;
    }
    let mut keys = HashSet::new();
    for artist in artists {
        let key = artist_search_fold(artist);
        if key.is_empty() || !keys.insert(key) {
            return None;
        }
    }
    Some(keys)
}

fn track_artist_credits(track: &Track) -> Option<Vec<String>> {
    track.provenance["artistCredits"]
        .as_array()?
        .iter()
        .map(|credit| {
            (credit["role"] == "artist")
                .then(|| credit["name"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect()
}

fn malformed_clock(track: &Track) -> Option<&str> {
    if track.provider != "traxsource"
        || track.id != format!("traxsource:{}", track.provider_id)
        || track.provider_id.is_empty()
        || !track.provider_id.bytes().all(|byte| byte.is_ascii_digit())
        || track.provenance["sourceKind"] != "chart"
        || track.local_path.is_some()
        || track.asset_id.is_some()
        || track.playback_range.is_some()
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
        || [
            "title",
            "artist",
            "artistCredits",
            "version",
            "durationSeconds",
            "album",
            "label",
            "releaseDate",
            "isrc",
        ]
        .iter()
        .any(|field| track.provenance["manualFields"][field] == true)
    {
        return None;
    }
    let url = url::Url::parse(&track.source_url).ok()?;
    let mut segments = url.path_segments()?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.traxsource.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || segments.next() != Some("track")
        || segments.next() != Some(track.provider_id.as_str())
    {
        return None;
    }
    let version = track.version.as_deref()?;
    let clock = version.trim().strip_prefix('(')?.strip_suffix(')')?;
    let (minutes, seconds) = clock.split_once(':')?;
    if minutes.is_empty()
        || minutes.len() > 2
        || seconds.len() != 2
        || !minutes
            .bytes()
            .chain(seconds.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds = seconds.parse::<u32>().ok()?;
    let stored = track.duration_seconds;
    // Older captures converted an invalid clock arithmetically; current
    // companions retain the original clock but leave duration unknown.
    if seconds < 60
        || stored.is_some_and(|seconds_value| {
            !seconds_value.is_finite()
                || (seconds_value - f64::from(minutes * 60 + seconds)).abs() >= 0.5
        })
    {
        return None;
    }
    let (_, edition) = recording_key(&track.title, None)?;
    if !edition.ends_with(" remix") {
        return None;
    }
    let captured = track.provenance["rawCaptures"]
        .as_array()?
        .iter()
        .any(|row| {
            row["providerId"].as_str() == Some(track.provider_id.as_str())
                && row["sourceUrl"].as_str() == Some(track.source_url.as_str())
                && row["title"].as_str() == Some(track.title.as_str())
                && row["artist"].as_str() == Some(track.artist.as_str())
                && row["version"].as_str() == Some(version)
                && row.get("durationSeconds").is_some_and(|raw| match stored {
                    Some(seconds) => raw.as_f64() == Some(seconds),
                    None => raw.is_null(),
                })
                && row["artistCredits"] == track.provenance["artistCredits"]
        });
    captured.then_some(clock)
}

pub(crate) fn is_eligible(track: &Track) -> bool {
    malformed_clock(track).is_some()
}

fn valid_duration(seconds: f64) -> bool {
    seconds.is_finite() && (1.0..=86_400.0).contains(&seconds)
}

fn target_row<'a>(
    track: &Track,
    release: &'a TraxsourceReleaseEvidence,
) -> Option<&'a crate::providers::traxsource::TraxsourceReleaseTrackEvidence> {
    let clock = malformed_clock(track)?;
    if !(3..=MAX_TRACKS).contains(&release.rows.len())
        || track
            .label
            .as_deref()
            .is_some_and(|label| !same_text(label, &release.label))
        || track
            .release_date
            .as_deref()
            .is_some_and(|date| date != release.release_date)
        || track
            .album
            .as_deref()
            .is_some_and(|album| !same_text(album, &release.title))
    {
        return None;
    }
    let mut rows = release
        .rows
        .iter()
        .filter(|row| row.track_id == track.provider_id);
    let row = rows.next()?;
    let captured_credits = artist_keys(&track_artist_credits(track)?)?;
    (rows.next().is_none()
        && row.duration_seconds.is_none()
        && row.malformed_duration.as_deref() == Some(clock)
        && recording_key(&row.title, row.version.as_deref())? == recording_key(&track.title, None)?
        && artist_keys(&row.artist_credits)? == captured_credits
        && track
            .track_number
            .is_none_or(|position| position == row.track_number)
        && track.disc_number.is_none_or(|disc| disc == row.disc_number))
    .then_some(row)
}

fn compare_release<'a>(
    track: &Track,
    source: &TraxsourceReleaseEvidence,
    candidate: &'a IndependentAlbumEvidence,
) -> Option<&'a crate::remote_playlists::spotify_release_evidence::IndependentAlbumTrackEvidence> {
    if !same_text(&source.title, &candidate.title)
        || !same_text(&source.label, &candidate.label)
        || source.release_date != candidate.release_date
        || source.rows.len() != candidate.tracks.len()
    {
        return None;
    }
    // A merchant catalogue number need not be a barcode. Only compare the
    // numeric product-code namespace when both releases declare one.
    let comparable_code = |code: &str| {
        matches!(code.len(), 8 | 12 | 13 | 14) && code.bytes().all(|byte| byte.is_ascii_digit())
    };
    if comparable_code(&source.catalog_number)
        && candidate
            .product_codes
            .iter()
            .any(|code| comparable_code(code))
        && !candidate.product_codes.iter().any(|code| {
            comparable_code(code)
                && code.trim_start_matches('0') == source.catalog_number.trim_start_matches('0')
        })
    {
        return None;
    }
    let mut seen_source = HashSet::new();
    let mut seen_candidate = HashSet::new();
    let mut target = None;
    let mut previous = (1, 0);
    for (left, right) in source.rows.iter().zip(&candidate.tracks) {
        let position = (left.disc_number, left.track_number);
        if (position != (previous.0, previous.1 + 1) && position != (previous.0 + 1, 1))
            || (previous.1 == 0 && position != (1, 1))
            || position != (right.disc_number, right.track_number)
            || !seen_source.insert(&left.track_id)
            || !seen_candidate.insert(&right.id)
            || recording_key(&left.title, left.version.as_deref())?
                != recording_key(&right.title, right.version.as_deref())?
            || artist_keys(&left.artist_credits)? != artist_keys(&right.artist_credits)?
            || !valid_duration(right.duration_seconds)
        {
            return None;
        }
        previous = position;
        if left.track_id == track.provider_id {
            target = Some(right);
        } else {
            let seconds = left.duration_seconds?;
            if !valid_duration(seconds)
                || left.malformed_duration.is_some()
                || (seconds - right.duration_seconds).abs() > SIBLING_DURATION_TOLERANCE
            {
                return None;
            }
        }
    }
    target
}

fn verified_track(
    track: &Track,
    release: &TraxsourceReleaseEvidence,
    candidates: &[IndependentAlbumEvidence],
) -> Option<Track> {
    let target = target_row(track, release)?;
    if candidates.is_empty() || candidates.len() > MAX_CANDIDATES {
        return None;
    }
    let mut matches = candidates.iter().filter_map(|candidate| {
        compare_release(track, release, candidate).map(|matched| (candidate, matched))
    });
    let (album, independent) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let isrc = independent.isrc.as_deref().and_then(normalize_isrc);
    if track
        .isrc
        .as_deref()
        .is_some_and(|current| normalize_isrc(current) != isrc)
    {
        return None;
    }
    let mut verified = track.clone();
    verified.title = independent.title.clone();
    verified.version = independent.version.clone();
    verified.album = Some(album.title.clone());
    verified.artist = independent.artist_credits.join(", ");
    verified.duration_seconds = Some(independent.duration_seconds);
    verified.isrc = isrc;
    verified.provenance["artistCredits"] = json!(independent
        .artist_credits
        .iter()
        .map(|name| json!({"name":name,"role":"artist"}))
        .collect::<Vec<_>>());
    verified.provenance["playbackCatalogDurationProof"] = json!({
        "provider":"traxsourceSpotifyCompleteRelease",
        "savedFingerprint":crate::remote_playlists::alias_hydration_key(track).ok()?,
        "sourceReleaseId":release.release_id,
        "sourceReleaseUrl":release.source_url,
        "sourceTrackUrl":target.source_url,
        "candidateReleaseId":album.id,
        "candidateTrackId": independent.id,
        "discNumber":target.disc_number,
        "trackNumber":target.track_number,
        "releaseTrackCount":release.rows.len(),
    });
    Some(verified)
}

enum ReleaseProof {
    Verified(Track),
    Contradictory,
    Unavailable,
}

async fn select_duration_proof(
    release: impl Future<Output = Result<ReleaseProof, String>>,
    fallback: impl Future<Output = Result<Option<Track>, String>>,
    is_current: impl Fn() -> Result<bool, String>,
    cancel: &watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Option<Track>, String> {
    let can_publish = || -> Result<bool, String> {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        Ok(is_current()? && Instant::now() < deadline)
    };
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => Err("Cancelled".into()),
        _ = tokio::time::sleep_until(deadline) => Ok(None),
        result = async {
            let verified = {
                // Overlap public metadata reads under the same deadline. An
                // early fallback result cannot outrank the ordered release.
                let mut release = std::pin::pin!(release);
                let mut fallback = std::pin::pin!(fallback);
                let (proof, buffered_fallback) = tokio::select! {
                    biased;
                    proof = &mut release => (proof, None),
                    result = &mut fallback => (release.await, Some(result)),
                };
                match proof {
                    Ok(ReleaseProof::Verified(proven)) => Some(proven),
                    Ok(ReleaseProof::Contradictory) => None,
                    Ok(ReleaseProof::Unavailable) | Err(_) => {
                        if !can_publish()? {
                            return Ok(None);
                        }
                        match buffered_fallback {
                            Some(result) => result?,
                            None => fallback.await?,
                        }
                    }
                }
            };
            Ok(if can_publish()? { verified } else { None })
        } => result,
    }
}

pub(crate) async fn prepare(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Option<Track>, String> {
    if !is_eligible(track) {
        return Ok(None);
    }
    let deadline = deadline.min(Instant::now() + Duration::from_secs(4));
    let saved_at_start = store.get_track(&track.id)?;
    if !saved_at_start.as_ref().is_some_and(|saved| {
        crate::remote_playlists::alias_hydration_identity(saved)
            == crate::remote_playlists::alias_hydration_identity(track)
            && saved.provenance["rawCaptures"] == track.provenance["rawCaptures"]
            && saved.provenance["manualFields"] == track.provenance["manualFields"]
            && is_eligible(saved)
    }) {
        return Ok(None);
    }
    let release_proof = async {
        let release = Traxsource::new()
            .get_exact_release_for_malformed_track(track)
            .await?;
        if target_row(track, &release).is_none() {
            return Ok(ReleaseProof::Contradictory);
        }
        let candidates = acquisition
            .find_spotify_release_evidence(
                &release.title,
                &release.release_date,
                &release.label,
                cancel.clone(),
                deadline,
            )
            .await?;
        let Some(candidates) = candidates.filter(|candidates| !candidates.is_empty()) else {
            return Ok(ReleaseProof::Unavailable);
        };
        Ok(match verified_track(track, &release, &candidates) {
            Some(track) => ReleaseProof::Verified(track),
            None => ReleaseProof::Contradictory,
        })
    };
    Box::pin(select_duration_proof(
        release_proof,
        deezer::prepare(track),
        || Ok(store.get_track(&track.id)? == saved_at_start),
        &cancel,
        deadline,
    ))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::traxsource::TraxsourceReleaseTrackEvidence,
        remote_playlists::spotify_release_evidence::IndependentAlbumTrackEvidence,
    };

    #[tokio::test]
    async fn overlapping_fallback_keeps_release_priority_and_store_fence() {
        use std::cell::Cell;
        use tokio::sync::oneshot;

        for (release_proof, current, expected) in [
            (
                Ok(ReleaseProof::Verified(Track {
                    id: "release".into(),
                    ..Track::default()
                })),
                true,
                Some("release"),
            ),
            (Ok(ReleaseProof::Contradictory), true, None),
            (Ok(ReleaseProof::Unavailable), true, Some("fallback")),
            (Err("Release request failed".into()), true, Some("fallback")),
            (Ok(ReleaseProof::Unavailable), false, None),
        ] {
            let (_cancel_sender, cancel) = watch::channel(false);
            let (started, fallback_started) = oneshot::channel();
            let still_current = Cell::new(true);
            let release = async {
                fallback_started.await.unwrap();
                still_current.set(current);
                release_proof
            };
            let fallback = async {
                started.send(()).unwrap();
                Ok(Some(Track {
                    id: "fallback".into(),
                    ..Track::default()
                }))
            };
            let resolved = select_duration_proof(
                release,
                fallback,
                || Ok(still_current.get()),
                &cancel,
                Instant::now() + Duration::from_secs(1),
            )
            .await
            .unwrap();
            assert_eq!(resolved.as_ref().map(|track| track.id.as_str()), expected);
            assert_eq!(still_current.get(), current);
        }
    }

    #[tokio::test]
    async fn quick_contradiction_never_polls_fallback() {
        let (_cancel_sender, cancel) = watch::channel(false);
        let resolved = select_duration_proof(
            std::future::ready(Ok(ReleaseProof::Contradictory)),
            async { panic!("contradictory release must stop before polling fallback") },
            || Ok(true),
            &cancel,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn cancellation_drops_a_pending_public_fallback() {
        struct MarkDrop<'a>(&'a std::cell::Cell<bool>);
        impl Drop for MarkDrop<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let (cancel_sender, cancel) = watch::channel(false);
        let (started, fallback_started) = tokio::sync::oneshot::channel();
        let dropped = std::cell::Cell::new(false);
        let outcome = select_duration_proof(
            std::future::pending(),
            async {
                let _mark_drop = MarkDrop(&dropped);
                started.send(()).unwrap();
                std::future::pending().await
            },
            || Ok(true),
            &cancel,
            Instant::now() + Duration::from_secs(1),
        );
        let cancel_after_start = async {
            fallback_started.await.unwrap();
            cancel_sender.send(true).unwrap();
        };
        let (outcome, ()) = tokio::join!(outcome, cancel_after_start);
        assert_eq!(outcome.unwrap_err(), "Cancelled");
        assert!(dropped.get());
    }

    fn fixture() -> (Track, TraxsourceReleaseEvidence, IndependentAlbumEvidence) {
        let title = "Plastico (Bugz In The Attic Remix)";
        let source_url =
            "https://www.traxsource.com/track/1494176/plastico-bugz-in-the-attic-remix";
        let artist = "Willie Colon, Ruben Blades";
        let credits = json!([
            {"name":"Willie Colon","role":"artist"},
            {"name":"Ruben Blades","role":"artist"},
        ]);
        let track = Track {
            id: "traxsource:1494176".into(),
            provider: "traxsource".into(),
            provider_id: "1494176".into(),
            source_url: source_url.into(),
            title: title.into(),
            version: Some("(6:90)".into()),
            artist: artist.into(),
            duration_seconds: Some(450.0),
            label: Some("Fania".into()),
            release_date: Some("2012-02-28".into()),
            provenance: json!({
                "sourceKind":"chart",
                "artistCredits":credits,
                "rawCaptures":[{"providerId":"1494176","sourceUrl":source_url,"title":title,
                    "version":"(6:90)","artist":artist,"durationSeconds":450.0,"artistCredits":credits}],
            }),
            ..Track::default()
        };
        let source = TraxsourceReleaseEvidence {
            release_id: "275301".into(),
            source_url: "https://www.traxsource.com/title/275301/i-like-it-like-that-fania-remixed"
                .into(),
            title: "I Like It Like That Fania Remixed".into(),
            label: "Fania".into(),
            release_date: "2012-02-28".into(),
            catalog_number: "G010002782582X".into(),
            rows: [
                ("100", "Intro", vec!["Unknown Artist"], Some(25.0), None),
                (
                    "1494176",
                    title,
                    vec!["Willie Colon", "Ruben Blades"],
                    None,
                    Some("6:90"),
                ),
                (
                    "102",
                    "Ponte Duro (Remix)",
                    vec!["Fania All Stars"],
                    Some(292.0),
                    None,
                ),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (id, title, artists, duration, malformed))| {
                TraxsourceReleaseTrackEvidence {
                    track_id: id.into(),
                    source_url: format!("https://www.traxsource.com/track/{id}/fixture"),
                    title: title.into(),
                    artist_credits: artists.into_iter().map(str::to_owned).collect(),
                    version: None,
                    disc_number: 1,
                    track_number: index as u32 + 1,
                    duration_seconds: duration,
                    malformed_duration: malformed.map(str::to_owned),
                }
            })
            .collect(),
        };
        let mut independent = IndependentAlbumEvidence {
            id: "aaaaaaaaaaaaaaaaaaaaaa".into(),
            title: "I Like It Like That: Fania Remixed".into(),
            label: "Fania".into(),
            release_date: "2012-02-28".into(),
            product_codes: Vec::new(),
            tracks: source
                .rows
                .iter()
                .map(|row| IndependentAlbumTrackEvidence {
                    id: format!("{:022}", row.track_number),
                    title: row.title.clone(),
                    version: None,
                    artist_credits: row.artist_credits.clone(),
                    disc_number: row.disc_number,
                    track_number: row.track_number,
                    duration_seconds: row.duration_seconds.unwrap_or(369.36),
                    isrc: None,
                })
                .collect(),
        };
        independent.tracks[1].title = "Plástico (Bugz In The Attic Remix)".into();
        independent.tracks[1].artist_credits = vec!["Willie Colón".into(), "Rubén Blades".into()];
        independent.tracks[1].isrc = Some("USDBB0610599".into());
        (track, source, independent)
    }

    #[test]
    fn malformed_chart_duration_requires_a_unique_complete_ordered_release() {
        let (saved, release, independent) = fixture();
        let accepted =
            verified_track(&saved, &release, std::slice::from_ref(&independent)).unwrap();
        assert_eq!(accepted.duration_seconds, Some(369.36));
        assert_eq!(accepted.title, "Plástico (Bugz In The Attic Remix)");
        assert_eq!(accepted.isrc.as_deref(), Some("USDBB0610599"));
        assert_eq!(saved.duration_seconds, Some(450.0));
        assert_eq!(saved.version.as_deref(), Some("(6:90)"));
        assert_eq!(
            accepted.provenance["rawCaptures"],
            saved.provenance["rawCaptures"]
        );

        let mut ambiguous = independent.clone();
        ambiguous.id = "bbbbbbbbbbbbbbbbbbbbbb".into();
        assert!(verified_track(&saved, &release, &[independent.clone(), ambiguous]).is_none());

        for (position, change) in [
            (0, "wrong sibling time"),
            (1, "wrong edition"),
            (1, "wrong artist"),
            (2, "missing sibling"),
            (2, "wrong position"),
        ] {
            let mut alternative = independent.clone();
            match change {
                "wrong sibling time" => alternative.tracks[position].duration_seconds += 15.0,
                "wrong edition" => {
                    alternative.tracks[position].title = "Plástico (Original Mix)".into()
                }
                "wrong artist" => alternative.tracks[position]
                    .artist_credits
                    .push("Another performer".into()),
                "missing sibling" => {
                    alternative.tracks.remove(position);
                }
                "wrong position" => alternative.tracks[position].track_number = 1,
                _ => unreachable!(),
            }
            assert!(
                verified_track(&saved, &release, &[alternative]).is_none(),
                "{change}"
            );
        }
    }

    #[test]
    fn a_new_capture_with_unknown_duration_still_requires_full_release_proof() {
        let (mut saved, release, independent) = fixture();
        saved.duration_seconds = None;
        let raw = saved.provenance["rawCaptures"][0].as_object_mut().unwrap();
        raw.remove("durationSeconds");
        let candidates = std::slice::from_ref(&independent);
        assert!(verified_track(&saved, &release, candidates).is_none());

        saved.provenance["rawCaptures"][0]["durationSeconds"] = json!(null);
        let enriched = verified_track(&saved, &release, candidates).unwrap();
        assert_eq!(enriched.duration_seconds, Some(369.36));
        assert_eq!(saved.duration_seconds, None);

        saved.provenance["rawCaptures"][0]["durationSeconds"] = json!("unknown");
        assert!(verified_track(&saved, &release, candidates).is_none());
    }

    #[test]
    fn a_manual_or_changed_capture_cannot_be_repaired_from_release_metadata() {
        let (mut saved, release, independent) = fixture();
        saved.provenance["manualFields"]["durationSeconds"] = json!(true);
        assert!(verified_track(&saved, &release, std::slice::from_ref(&independent)).is_none());
        let (mut saved, _, _) = fixture();
        saved.provenance["rawCaptures"][0]["durationSeconds"] = json!(369.36);
        assert!(verified_track(&saved, &release, &[independent]).is_none());
    }
}
