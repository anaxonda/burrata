//! A recording's explicit MusicBrainz streaming relationship. This is an
//! operation-local locator, independent of artist/title search and saved maps.
use crate::{
    acquisition,
    artists::ArtistService,
    matching,
    models::{Candidate, Track},
};
use serde_json::Value;

const RECORDING_FREE_STREAMING: &str = "7e41ef12-a124-4324-afdb-fdbae687a89c";

pub(super) fn eligible(track: &Track) -> bool {
    track.provider == "musicbrainz"
        && crate::spotify_mb_crosswalk::valid_mbid(&track.provider_id)
        && track.id == format!("musicbrainz:{}", track.provider_id)
        && track.source_url == format!("https://musicbrainz.org/recording/{}", track.provider_id)
        && track.provenance["catalog"]["video"] != false
        && track.provenance["manualFields"]
            .as_object()
            .is_none_or(|fields| {
                [
                    "artist",
                    "title",
                    "version",
                    "durationSeconds",
                    "isrc",
                    "playbackMode",
                ]
                .iter()
                .all(|field| fields.get(*field).is_none_or(|value| value != true))
            })
        && track.duration_seconds.is_some_and(valid_duration)
}

#[derive(Debug)]
pub(super) struct VideoLink {
    video_id: String,
    title: String,
    duration: f64,
}

impl VideoLink {
    pub(super) async fn load(
        store: std::sync::Arc<crate::storage::Store>,
        track: &Track,
    ) -> Option<Self> {
        let service = ArtistService::new(store).ok()?;
        let details = service
            .musicbrainz_recording_video_details(&track.provider_id)
            .await
            .ok()?;
        Self::from_recording(track, &details)
    }

    fn from_recording(track: &Track, details: &Value) -> Option<Self> {
        if !eligible(track)
            || details["id"].as_str() != Some(track.provider_id.as_str())
            || details["video"] != true
        {
            return None;
        }
        let title = matching::normalize(details["title"].as_str()?);
        let artist =
            matching::normalize(&crate::artists::mb_credit_name(&details["artist-credit"]));
        let version = details["disambiguation"]
            .as_str()
            .unwrap_or_default()
            .trim();
        let duration = details["length"].as_u64()? as f64 / 1000.0;
        if title.is_empty()
            || title != matching::normalize(&track.title)
            || artist.is_empty()
            || artist != matching::normalize(&track.artist)
            || version != track.version.as_deref().unwrap_or_default().trim()
            || !valid_duration(duration)
            || (track.duration_seconds? - duration).abs() > 1.0
        {
            return None;
        }
        let mut video_id = None;
        for relation in details["relations"].as_array()?.iter() {
            if relation["type-id"] != RECORDING_FREE_STREAMING
                || relation["target-type"] != "url"
                || relation["direction"] != "forward"
                || relation["ended"] == true
                || relation["end"].as_str().is_some_and(|end| !end.is_empty())
            {
                continue;
            }
            let Some(id) = relation["url"]["resource"]
                .as_str()
                .and_then(watch_video_id)
            else {
                continue;
            };
            if video_id.as_ref().is_some_and(|known| known != &id) {
                return None;
            }
            video_id = Some(id);
        }
        Some(Self {
            video_id: video_id?,
            title,
            duration,
        })
    }

    pub(super) fn candidate(&self, track: &Track) -> Candidate {
        Candidate {
            id: format!("youtube:{}", self.video_id),
            provider: "youtube".into(),
            url: format!("https://www.youtube.com/watch?v={}", self.video_id),
            title: track.title.clone(),
            duration_seconds: Some(self.duration),
            confidence: "review".into(),
            permission: "unknown".into(),
            ..Default::default()
        }
    }

    pub(super) fn matches_resolved(&self, candidate: &Candidate, quality: &Value) -> bool {
        candidate.provider == "youtube"
            && candidate.id.strip_prefix("youtube:") == Some(self.video_id.as_str())
            && watch_video_id(&candidate.url).as_deref() == Some(self.video_id.as_str())
            && matching::normalize(&candidate.title) == self.title
            && quality["resolvedProviderId"].as_str() == Some(self.video_id.as_str())
            && quality["extractedDurationSeconds"]
                .as_f64()
                .is_some_and(|seconds| {
                    valid_duration(seconds)
                        && (seconds - self.duration).abs() <= 5.0_f64.min(self.duration * 0.02)
                })
    }
}

fn valid_duration(seconds: f64) -> bool {
    seconds.is_finite() && (5.0..=86_400.0).contains(&seconds)
}

fn watch_video_id(raw: &str) -> Option<String> {
    let url = acquisition::validate_source_url(raw, "youtube").ok()?;
    if url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let valid_shape = match host {
        "youtube.com" | "www.youtube.com" | "music.youtube.com" => {
            url.path() == "/watch" && url.query_pairs().filter(|(key, _)| key == "v").count() == 1
        }
        "youtu.be" => url.path_segments()?.count() == 1,
        _ => false,
    };
    if !valid_shape
        || url
            .query_pairs()
            .any(|(key, _)| matches!(key.as_ref(), "t" | "start" | "end" | "list"))
    {
        return None;
    }
    acquisition::youtube_video_id(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn linked_recording() -> (Track, Value) {
        let id = "10e142de-3f49-498a-b813-f6266021be94";
        let title = "Tschaikowsky: Violinkonzert ∙ hr-Sinfonieorchester ∙ Hilary Hahn ∙ Andrés Orozco-Estrada";
        let track = Track {
            id: format!("musicbrainz:{id}"),
            provider_id: id.into(),
            provider: "musicbrainz".into(),
            source_url: format!("https://musicbrainz.org/recording/{id}"),
            title: title.into(),
            artist: "Hilary Hahn, Frankfurt Radio Symphony, Andrés Orozco‐Estrada".into(),
            duration_seconds: Some(2491.0),
            provenance: json!({"catalog":{"video":true}}),
            ..Default::default()
        };
        let recording = json!({
            "id":id, "title":title, "video":true, "length":2491000, "disambiguation":"",
            "artist-credit":[
                {"name":"Hilary Hahn","joinphrase":", "},
                {"name":"Frankfurt Radio Symphony","joinphrase":","},
                {"name":"Andrés Orozco‐Estrada","joinphrase":""}
            ],
            "relations":[{
                "type-id":RECORDING_FREE_STREAMING, "target-type":"url", "direction":"forward",
                "ended":false, "url":{"resource":"https://www.youtube.com/watch?v=2Q_DzWUvcL8"}
            }]
        });
        (track, recording)
    }

    #[test]
    fn musicbrainz_video_link_requires_the_exact_recording_and_one_full_upload() {
        let (mut track, recording) = linked_recording();
        let link = VideoLink::from_recording(&track, &recording).unwrap();
        assert_eq!(link.candidate(&track).id, "youtube:2Q_DzWUvcL8");
        for (field, value) in [
            ("id", json!("different")),
            ("video", json!(false)),
            ("length", json!(2451000)),
        ] {
            let mut wrong = recording.clone();
            wrong[field] = value;
            assert!(
                VideoLink::from_recording(&track, &wrong).is_none(),
                "{field}"
            );
        }
        for resource in [
            "https://www.youtube.com/playlist?v=2Q_DzWUvcL8",
            "https://www.youtube.com/watch?v=2Q_DzWUvcL8&t=90",
            "https://www.youtube.com/watch?v=2Q_DzWUvcL8&v=Zrds6B32PBM",
        ] {
            let mut wrong = recording.clone();
            wrong["relations"][0]["url"]["resource"] = json!(resource);
            assert!(
                VideoLink::from_recording(&track, &wrong).is_none(),
                "{resource}"
            );
        }
        let mut ambiguous = recording.clone();
        let mut extra_link = recording["relations"][0].clone();
        extra_link["url"]["resource"] = json!("https://youtu.be/Zrds6B32PBM");
        ambiguous["relations"]
            .as_array_mut()
            .unwrap()
            .push(extra_link);
        assert!(VideoLink::from_recording(&track, &ambiguous).is_none());
        track.provenance["manualFields"] = json!({"artist":true});
        assert!(VideoLink::from_recording(&track, &recording).is_none());
    }

    #[test]
    fn musicbrainz_video_link_rejects_conflicting_fresh_source_metadata() {
        let (track, recording) = linked_recording();
        let link = VideoLink::from_recording(&track, &recording).unwrap();
        let mut candidate = link.candidate(&track);
        let mut quality =
            json!({"resolvedProviderId":"2Q_DzWUvcL8","extractedDurationSeconds":2492.0});
        assert!(link.matches_resolved(&candidate, &quality));
        quality["extractedDurationSeconds"] = json!(2485);
        assert!(!link.matches_resolved(&candidate, &quality));
        quality["extractedDurationSeconds"] = json!(2491);
        quality["resolvedProviderId"] = json!("Zrds6B32PBM");
        assert!(!link.matches_resolved(&candidate, &quality));
        quality["resolvedProviderId"] = json!("2Q_DzWUvcL8");
        candidate.title = "Tchaikovsky violin concerto highlights".into();
        assert!(!link.matches_resolved(&candidate, &quality));
    }
}
