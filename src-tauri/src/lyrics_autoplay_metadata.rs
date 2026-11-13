//! Lyrics-only metadata from the exact saved autoplay video; no queue or source edits.
use crate::models::Track;
use serde_json::Value;
use url::Url;

fn same_video_identity(provider: &str, id: &str, provider_id: &str, source_url: &str) -> bool {
    if provider != "youtube"
        || provider_id.len() != 11
        || !provider_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        || id != format!("youtube:{provider_id}")
    {
        return false;
    }
    let Ok(url) = Url::parse(source_url) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.host_str() {
        Some("youtube.com" | "www.youtube.com" | "music.youtube.com") => url
            .query_pairs()
            .any(|(key, value)| key == "v" && value == provider_id),
        Some("youtu.be") => url.path().strip_prefix('/') == Some(provider_id),
        _ => false,
    }
}

pub(crate) fn lookup_id(snapshot: &Value) -> Option<&str> {
    let track = &snapshot["track"];
    if snapshot["engine"] != "native"
        || !matches!(
            snapshot["status"].as_str(),
            Some("playing" | "paused" | "buffering" | "ended")
        )
        || track["provenance"]["sourceKind"] != "youtubeRadio"
        || track["provenance"]["manualFields"]["artist"] == true
        || !track["artist"].as_str()?.trim().is_empty()
        || track.get("playbackRange").is_some_and(|v| !v.is_null())
        || snapshot.get("chapter").is_some_and(|v| !v.is_null())
    {
        return None;
    }
    let id = track["id"].as_str()?;
    same_video_identity(
        track["provider"].as_str()?,
        id,
        track["providerId"].as_str()?,
        track["sourceUrl"].as_str()?,
    )
    .then_some(id)
}

pub(crate) fn with_saved_metadata(mut snapshot: Value, saved: Option<&Track>) -> Value {
    let Some(id) = lookup_id(&snapshot) else {
        return snapshot;
    };
    let Some(saved) = saved.filter(|saved| {
        saved.id == id
            && saved.playback_range.is_none()
            && !saved.artist.trim().is_empty()
            && same_video_identity(
                &saved.provider,
                &saved.id,
                &saved.provider_id,
                &saved.source_url,
            )
    }) else {
        return snapshot;
    };
    snapshot["track"]["artist"] = Value::String(saved.artist.clone());
    for (field, value) in [
        ("album", saved.album.as_ref()),
        ("albumArtist", saved.album_artist.as_ref()),
    ] {
        if snapshot["track"]["provenance"]["manualFields"][field] != true
            && snapshot["track"][field]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
        {
            if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
                snapshot["track"][field] = Value::String(value.clone());
            }
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Track) {
        let saved = Track {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            provider_id: "abcdefghijk".into(),
            source_url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Qué Es Lo Que Pasa".into(),
            artist: "Macha Y El Bloque Depresivo, Vicente Cifuentes".into(),
            album: Some("Qué Es Lo Que Pasa".into()),
            duration_seconds: Some(232.),
            ..Default::default()
        };
        let mut raw = saved.clone();
        raw.artist.clear();
        raw.album = None;
        raw.duration_seconds = Some(233.);
        raw.provenance = serde_json::json!({"sourceKind":"youtubeRadio"});
        (
            serde_json::json!({"engine":"native","status":"paused","track":raw,"duration":233.,"candidate":{"id":"youtube:abcdefghijk"}}),
            saved,
        )
    }
    #[test]
    fn exact_saved_video_fills_lyrics_identity_without_changing_playback_binding_or_duration() {
        let (raw, saved) = fixture();
        let enriched = with_saved_metadata(raw.clone(), Some(&saved));
        assert_eq!(enriched["track"]["artist"], saved.artist);
        assert_eq!(enriched["track"]["album"], saved.album.as_deref().unwrap());
        for field in [
            "id",
            "provider",
            "providerId",
            "sourceUrl",
            "title",
            "durationSeconds",
            "provenance",
            "playbackRange",
        ] {
            assert_eq!(enriched["track"][field], raw["track"][field], "{field}");
        }
        assert_eq!(enriched["candidate"], raw["candidate"]);
        assert_eq!(enriched["duration"], raw["duration"]);
        assert_eq!(raw["track"]["artist"], "");
    }
    #[test]
    fn unrelated_malformed_explicit_or_chapter_metadata_is_never_replaced() {
        let (raw, saved) = fixture();
        for (pointer, value) in [
            ("/engine", serde_json::json!("webview")),
            ("/status", serde_json::json!("idle")),
            ("/track/artist", serde_json::json!("Explicit artist")),
            (
                "/track/provenance/sourceKind",
                serde_json::json!("audioSearch"),
            ),
            (
                "/track/provenance/manualFields",
                serde_json::json!({"artist":true}),
            ),
            ("/track/providerId", serde_json::json!("different01")),
            ("/track/id", serde_json::json!("youtube:different01")),
            (
                "/track/sourceUrl",
                serde_json::json!("https://example.test/watch?v=abcdefghijk"),
            ),
            (
                "/track/playbackRange",
                serde_json::json!({"startTime":1,"endTime":20}),
            ),
            ("/chapter", serde_json::json!({"title":"Another song"})),
        ] {
            let mut changed = raw.clone();
            if let Some(target) = changed.pointer_mut(pointer) {
                *target = value;
            } else {
                match pointer {
                    "/chapter" => changed["chapter"] = value,
                    "/track/playbackRange" => changed["track"]["playbackRange"] = value,
                    "/track/provenance/manualFields" => {
                        changed["track"]["provenance"]["manualFields"] = value
                    }
                    _ => panic!("missing fixture field"),
                }
            }
            assert_eq!(
                with_saved_metadata(changed.clone(), Some(&saved)),
                changed,
                "{pointer}"
            );
        }
        let mut different = saved.clone();
        different.source_url = "https://youtu.be/different01".into();
        assert_eq!(with_saved_metadata(raw.clone(), Some(&different)), raw);
        assert_eq!(with_saved_metadata(raw.clone(), None), raw);
    }
    #[test]
    fn manual_album_and_existing_artist_remain_owned_by_the_current_track() {
        let (mut raw, saved) = fixture();
        raw["track"]["provenance"]["manualFields"] = serde_json::json!({"album":true});
        let enriched = with_saved_metadata(raw.clone(), Some(&saved));
        assert_eq!(enriched["track"]["artist"], saved.artist);
        assert!(enriched["track"]["album"].is_null());
        raw["track"]["artist"] = serde_json::json!("Already known");
        assert_eq!(with_saved_metadata(raw.clone(), Some(&saved)), raw);
    }
}
