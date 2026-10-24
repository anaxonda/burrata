//! Recording-bound playback from a MusicBrainz release's explicit Bandcamp link.
//! A title suffix may be omitted only when it names that exact album, and the
//! entire ordered track list, full artist credits and runtimes agree.
use super::*;
use std::collections::HashSet;

pub(super) fn release_id(track: &Track) -> Option<String> {
    if track.provenance["catalogProvider"] != "musicbrainz" {
        return None;
    }
    let id = track.provenance["releaseId"].as_str()?;
    uuid::Uuid::parse_str(id).ok()?;
    (track.source_url == format!("https://musicbrainz.org/release/{id}")
        && track.album.as_ref().is_some_and(|a| !a.trim().is_empty()))
    .then(|| id.to_owned())
}
fn credit(raw: &Value) -> Option<String> {
    raw.as_array()?
        .iter()
        .map(|row| {
            Some(format!(
                "{}{}",
                row["name"]
                    .as_str()
                    .or_else(|| row["artist"]["name"].as_str())?,
                row["joinphrase"].as_str().unwrap_or_default()
            ))
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join(""))
}
fn title_in_album(title: &str, album: &str) -> String {
    let base = title
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(_, suffix)| {
            crate::matching::normalize(suffix) == crate::matching::normalize(album)
        })
        .map_or(title, |(base, _)| base);
    compact(&crate::recording_identity::search_fold(base))
}
fn close_runtime(milliseconds: f64, seconds: f64) -> bool {
    if !milliseconds.is_finite() || !seconds.is_finite() || milliseconds <= 0.0 || seconds <= 0.0 {
        return false;
    }
    // MusicBrainz can report only whole seconds. Preserve the tighter check
    // when the reference actually has millisecond precision.
    let tolerance = if milliseconds % 1000.0 == 0.0 {
        0.51
    } else {
        0.025
    };
    (milliseconds / 1000.0 - seconds).abs() <= tolerance
}
fn verify(
    track: &Track,
    release: &Value,
    album: &Value,
    url: &reqwest::Url,
) -> Option<(Candidate, Value)> {
    let release_id = release_id(track)?;
    let expected_recording = track.provenance["recordingId"].as_str()?;
    uuid::Uuid::parse_str(expected_recording).ok()?;
    let name = track.album.as_deref()?;
    if release["id"] != release_id
        || crate::matching::normalize(release["title"].as_str()?)
            != crate::matching::normalize(name)
        || !same_credit(&track.artist, &credit(&release["artist-credit"])?)
        || !same_credit(&track.artist, album["artist"].as_str()?)
        || album["current"]["type"] != "album"
        || crate::matching::normalize(album["current"]["title"].as_str()?)
            != crate::matching::normalize(name)
        || !release["relations"]
            .as_array()?
            .iter()
            .any(|relation| relation["url"]["resource"].as_str() == Some(url.as_str()))
    {
        return None;
    }
    let mut left = Vec::new();
    for (medium_index, medium) in release["media"].as_array()?.iter().enumerate() {
        let rows = medium["tracks"].as_array()?;
        if medium["position"].as_u64() != Some((medium_index + 1) as u64)
            || medium["track-count"].as_u64() != Some(rows.len() as u64)
        {
            return None;
        }
        for (index, row) in rows.iter().enumerate() {
            if row["position"].as_u64() != Some((index + 1) as u64) {
                return None;
            }
            left.push(row);
        }
    }
    let right = album["trackinfo"].as_array()?;
    if !(2..=500).contains(&left.len()) || left.len() != right.len() {
        return None;
    }
    let album_id = album["current"]["id"].as_u64().filter(|id| *id > 0)?;
    let mut seen = HashSet::new();
    let mut matches = Vec::new();
    let mut vector = Vec::new();
    for (index, (a, b)) in left.iter().zip(right).enumerate() {
        let catalog_title = a["title"].as_str()?;
        let source_title = b["title"].as_str()?;
        let duration = a["length"]
            .as_f64()
            .or_else(|| a["recording"]["length"].as_f64())?;
        let actual = b["duration"].as_f64()?;
        let id = b["track_id"].as_u64().filter(|id| *id > 0)?;
        if !seen.insert(id)
            || b["track_num"].as_u64() != Some((index + 1) as u64)
            || title_in_album(catalog_title, name) != title_in_album(source_title, name)
            || !close_runtime(duration, actual)
            || !same_credit(&track.artist, &credit(&a["artist-credit"])?)
            || b["artist"]
                .as_str()
                .is_some_and(|artist| !same_credit(&track.artist, artist))
        {
            return None;
        }
        vector.push(json!({"recordingId":a["recording"]["id"],"catalogTitle":catalog_title,"catalogMs":duration,"sourceId":id,"sourceTitle":source_title,"sourceSeconds":actual}));
        if a["recording"]["id"] == expected_recording
            && track.provenance["trackGuid"]
                .as_str()
                .is_none_or(|guid| a["id"].as_str() == Some(guid))
        {
            if title_in_album(&track.title, name) != title_in_album(catalog_title, name)
                || track
                    .duration_seconds
                    .is_none_or(|d| (d - duration / 1000.0).abs() > 0.025)
            {
                return None;
            }
            matches.push((id, source_title, actual, b));
        }
    }
    if matches.len() != 1 {
        return None;
    }
    let (id, title, duration, row) = matches[0];
    let target = url.join(row["title_link"].as_str()?).ok()?;
    let target = crate::bandcamp::validate_track_page_url(target.as_str()).ok()?;
    if target.host_str() != url.host_str() {
        return None;
    }
    let candidate = Candidate {
        id: format!("bandcamp:{id}"),
        provider: "bandcamp".into(),
        url: target.to_string(),
        title: title.into(),
        uploader: Some(track.artist.clone()),
        duration_seconds: Some(duration),
        permission: "stream_only".into(),
        ..Candidate::default()
    };
    let proof = json!({"method":"catalog-linked-album-v1","referenceId":expected_recording,"releaseId":release_id,"catalogTitle":track.title,"catalogArtist":track.artist,"linkedAlbumUrl":url.as_str(),"bandcampAlbumId":album_id,"albumTrackCount":right.len(),"albumFingerprint":format!("{:x}",Sha256::digest(json!(vector).to_string().as_bytes()))});
    Some((candidate, proof))
}
pub(super) async fn discover(
    store: &Store,
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: PlaybackDeadline,
) -> Result<Option<(Candidate, Value)>, String> {
    let Some(id) = release_id(track) else {
        return Ok(None);
    };
    bounded(cancel, deadline, async {
        let namespace = "playback.bandcamp-linked-release.v1";
        let release = if let Some(raw) = store.get_provider_cache(namespace, &id)? {
            raw
        } else {
            let response = client()?
                .get(format!("https://musicbrainz.org/ws/2/release/{id}"))
                .query(&[
                    ("inc", "recordings+artist-credits+url-rels"),
                    ("fmt", "json"),
                ])
                .send()
                .await
                .map_err(|_| "Release reference unavailable")?;
            let raw: Value = serde_json::from_slice(&body(response, 2 * 1024 * 1024).await?)
                .map_err(|_| "Invalid release reference")?;
            if raw["id"] != id {
                return Ok(None);
            }
            store.set_provider_cache(namespace, &id, &raw, 86400)?;
            raw
        };
        let mut seen = HashSet::new();
        let urls = release["relations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["url"]["resource"].as_str())
            .filter_map(|raw| crate::bandcamp::validate_album_page_url(raw).ok())
            .filter(|url| seen.insert(url.to_string()))
            .collect::<Vec<_>>();
        // Ambiguous editions need a user choice; never pick an arbitrary link.
        if urls.len() != 1 {
            return Ok(None);
        }
        let url = &urls[0];
        let addresses = crate::acquisition::public_addresses(url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Burrata/0.1 (public recording playback)")
            .resolve_to_addrs(url.host_str().ok_or("Album host missing")?, &addresses)
            .build()
            .map_err(|_| "Album client unavailable")?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| "Linked album unavailable")?;
        let html = body(response, 2 * 1024 * 1024).await?;
        let Some(album) =
            crate::collection_sources::extract_tralbum_data(&String::from_utf8_lossy(&html))
        else {
            return Ok(None);
        };
        Ok(verify(track, &release, &album, url))
    })
    .await
}

pub(super) fn bind_audio(
    proof: &mut Value,
    candidate: &Candidate,
    audio: &crate::acquisition::PreviewAudio,
) -> bool {
    if proof["method"] != "catalog-linked-album-v1" {
        return false;
    }
    let Some(metadata) = audio.source_metadata.as_ref() else {
        return false;
    };
    let artist = metadata["channel"].as_str().unwrap_or_default();
    let title = audio
        .resolved_title
        .as_deref()
        .unwrap_or_default()
        .strip_prefix(artist)
        .unwrap_or(audio.resolved_title.as_deref().unwrap_or_default())
        .trim()
        .trim_start_matches('-')
        .trim();
    if compact(title) != compact(&candidate.title)
        || metadata["albumUrl"] != proof["linkedAlbumUrl"]
        || metadata["albumId"] != proof["bandcampAlbumId"]
        || candidate
            .duration_seconds
            .zip(audio.duration_seconds)
            .is_none_or(|(a, b)| (a - b).abs() > 0.025)
        || candidate.id.strip_prefix("bandcamp:") != audio.resolved_provider_id.as_deref()
        || metadata["channel"].as_str().is_none_or(|artist| {
            !same_credit(artist, proof["catalogArtist"].as_str().unwrap_or_default())
        })
    {
        return false;
    }
    proof["sourceId"] = json!(candidate.id);
    proof["sourceUrl"] = json!(candidate.url);
    proof["resolvedTitle"] = json!(audio.resolved_title);
    proof["resolvedArtist"] = metadata["channel"].clone();
    proof["duration"] = json!(audio.duration_seconds);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Track, Value, Value, reqwest::Url) {
        let release = "11111111-1111-4111-8111-111111111111";
        let recording = "22222222-2222-4222-8222-222222222222";
        let url =
            reqwest::Url::parse("https://fixture.bandcamp.com/album/live-at-fixture-26").unwrap();
        let credit = json!([{"name":"Synthetic Band","joinphrase":""}]);
        let track = Track {
            id: "catalog:linked-fixture".into(),
            provider: "catalog".into(),
            provider_id: "linked-fixture".into(),
            source_url: format!("https://musicbrainz.org/release/{release}"),
            title: "First Song".into(),
            artist: "Synthetic Band".into(),
            album: Some("Live at Fixture '26".into()),
            duration_seconds: Some(470.0),
            provenance: json!({"catalogProvider":"musicbrainz","releaseId":release,"recordingId":recording,"trackGuid":"first"}),
            ..Track::default()
        };
        let songs = json!([{"id":"first","position":1,"title":"First Song","length":470000,"recording":{"id":recording},"artist-credit":credit},{"id":"second","position":2,"title":"Second Song","length":120000,"recording":{"id":"33333333-3333-4333-8333-333333333333"},"artist-credit":credit}]);
        let source = json!({"artist":"Synthetic Band","current":{"id":99,"type":"album","title":"Live at Fixture '26"},"trackinfo":[{"track_num":1,"track_id":51,"title":"First Song (Live at Fixture '26)","title_link":"/track/first-live","duration":469.647},{"track_num":2,"track_id":52,"title":"Second Song (Live at Fixture '26)","title_link":"/track/second-live","duration":120.2}]});
        let reference = json!({"id":release,"title":"Live at Fixture '26","artist-credit":credit,"relations":[{"url":{"resource":url.as_str()}}],"media":[{"position":1,"track-count":2,"tracks":songs}]});
        (track, reference, source, url)
    }
    #[test]
    fn exact_linked_release_accepts_rounded_live_runtime_but_rejects_wrong_performance() {
        let (track, reference, source, url) = fixture();
        let (candidate, proof) = verify(&track, &reference, &source, &url).unwrap();
        assert_eq!(candidate.id, "bandcamp:51");
        assert_eq!(candidate.duration_seconds, Some(469.647));
        assert_eq!(proof["method"], "catalog-linked-album-v1");
        for field in ["artist", "current", "trackinfo"] {
            let mut changed = source.clone();
            match field {
                "artist" => changed[field] = json!("Cover Band"),
                "current" => changed[field]["title"] = json!("Live at Another Show"),
                _ => changed[field][0]["duration"] = json!(298.76),
            };
            assert!(verify(&track, &reference, &changed, &url).is_none());
        }
        let mut remix = source.clone();
        remix["trackinfo"][1]["title"] = json!("Second Song (Remix)");
        assert!(verify(&track, &reference, &remix, &url).is_none());
        let mut missing = source.clone();
        missing["trackinfo"].as_array_mut().unwrap().pop();
        assert!(verify(&track, &reference, &missing, &url).is_none());
        let mut reordered = source.clone();
        reordered["trackinfo"].as_array_mut().unwrap().swap(0, 1);
        assert!(verify(&track, &reference, &reordered, &url).is_none());
        let mut unlinked = reference.clone();
        unlinked["relations"] = json!([]);
        assert!(verify(&track, &unlinked, &source, &url).is_none());
        let other =
            reqwest::Url::parse("https://another.bandcamp.com/album/live-at-fixture-26").unwrap();
        assert!(verify(&track, &reference, &source, &other).is_none());
    }
    #[test]
    fn proof_requires_fresh_album_track_identity_and_shared_matcher_acceptance() {
        let (mut track, mut reference, mut source, url) = fixture();
        let band = "Synthetic Band & The Ensemble";
        track.artist = band.into();
        track.provenance["artistCredits"] = json!([{"name":band,"joinPhrase":""}]);
        let full_credit = json!([{"name":band,"joinphrase":""}]);
        reference["artist-credit"] = full_credit.clone();
        for row in reference["media"][0]["tracks"].as_array_mut().unwrap() {
            row["artist-credit"] = full_credit.clone();
        }
        source["artist"] = json!(band);
        let (candidate, mut proof) = verify(&track, &reference, &source, &url).unwrap();
        let mut audio = crate::acquisition::PreviewAudio {
            provider_session: None,
            url: "https://audio.example/unused".into(),
            mime_type: "audio/mpeg".into(),
            codec: Some("mp3".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(469.647),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: Some(json!({"channel":band,"albumId":99,"albumUrl":url.as_str()})),
            resolved_title: Some(format!("{band} - {}", candidate.title)),
            resolved_provider_id: Some("51".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        };
        assert!(bind_audio(&mut proof, &candidate, &audio));
        let binding = strong_binding_id(&proof, &candidate, &audio).unwrap();
        for level in [
            crate::recording_match::MatchLevel::Exact,
            crate::recording_match::MatchLevel::Balanced,
            crate::recording_match::MatchLevel::Flexible,
        ] {
            assert!(shared_policy_assessment(
                &track,
                &candidate,
                &track.title,
                &track.artist,
                audio.duration_seconds,
                Some(&binding),
                level
            )
            .is_some());
        }
        audio.source_metadata.as_mut().unwrap()["albumId"] = json!(100);
        assert!(!cached_binding_matches(&proof, &candidate, &audio));
        assert!(!bind_audio(&mut proof, &candidate, &audio));
        let mut long = track;
        long.duration_seconds = Some(1500.0);
        assert!(eligible_with_policy(
            &long,
            &Settings::default(),
            false,
            2,
            true
        ));
    }
    #[tokio::test]
    #[ignore = "Explicit public-network recording-specific playback qualification; local decoder only"]
    async fn live_gizzard_linked_release_resolves_and_decodes_correct_long_live_recording() {
        assert_eq!(std::env::var("BURRATA_GIZZARD_LIVE").as_deref(), Ok("1"));
        let path = std::env::var("BURRATA_GIZZARD_TRACK_FIXTURE").unwrap();
        let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let track: Track =
            serde_json::from_value(raw["structuredContent"]["result"]["track"].clone()).unwrap();
        assert_eq!(
            track.provenance["releaseId"],
            "de4c4c4f-5605-4c94-bf2f-2c0136a96a61"
        );
        assert_eq!(track.duration_seconds, Some(470.0));
        let directory = tempfile::tempdir().unwrap();
        let store =
            std::sync::Arc::new(Store::open(&directory.path().join("store.sqlite")).unwrap());
        store.cache_track(&track).unwrap();
        let connections =
            std::sync::Arc::new(crate::connections::ConnectionManager::new(store.clone()));
        let acquisition = AcquisitionManager::new_for_diagnostics(store.clone(), connections);
        let settings = Settings::default();
        let (_stop, cancel) = watch::channel(false);
        let started = std::time::Instant::now();
        let source = super::super::resolve(
            &store,
            &acquisition,
            &track,
            &settings,
            cancel.clone(),
            SourceTrace::new(None, None, None),
            PlaybackDeadline::after(Duration::from_secs(20)),
            &[],
            false,
        )
        .await
        .unwrap();
        let cold_ms = started.elapsed().as_millis();
        assert_eq!(source.candidate.as_ref().unwrap().id, "bandcamp:4270074567");
        assert!((source.duration.unwrap() - 469.647).abs() < 0.025);
        let proof = store
            .get_provider_cache(CACHE, &fingerprint(&track))
            .unwrap()
            .unwrap();
        assert_eq!(proof["method"], "catalog-linked-album-v1");
        assert_eq!(proof["albumTrackCount"], 64);
        assert!(!proof.to_string().contains("mp3-128"));
        let pcm = crate::native_output::direct_pcm_experiment::collect_until(
            source.source,
            0.0,
            3.0,
            (std::time::Instant::now() + Duration::from_secs(15)),
            cancel.clone(),
        )
        .await;
        let pcm = serde_json::to_value(pcm).unwrap();
        assert_eq!(pcm["ok"], true, "{pcm}");
        let start = std::time::Instant::now();
        let refreshed = super::super::resolve(
            &store,
            &acquisition,
            &track,
            &settings,
            cancel,
            SourceTrace::new(None, None, None),
            PlaybackDeadline::after(Duration::from_secs(20)),
            &[],
            false,
        )
        .await
        .unwrap();
        assert_eq!(refreshed.candidate.unwrap().id, "bandcamp:4270074567");
        eprintln!("Gizzard exact linked release: 64 tracks, source 4270074567, 469.647 seconds, native PCM decoded; cold {cold_ms} ms, cached {} ms",start.elapsed().as_millis());
    }
}
