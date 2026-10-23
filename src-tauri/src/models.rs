use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub provider: String,
    pub provider_id: String,
    pub source_url: String,
    pub artist: String,
    pub title: String,
    pub version: Option<String>,
    pub album: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub recording_date: Option<String>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub track_total: Option<u32>,
    #[serde(default)]
    pub disc_number: Option<u32>,
    #[serde(default)]
    pub disc_total: Option<u32>,
    #[serde(default)]
    pub remixers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_range: Option<PlaybackRange>,
    pub label: Option<String>,
    pub genre: Option<String>,
    pub normalized_genre: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub release_date: Option<String>,
    /// Preserve the raw legacy value: old listings may use non-string JSON here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_added: Option<serde_json::Value>,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
    pub catalog_number: Option<String>,
    pub rank: Option<u32>,
    pub preview_url: Option<String>,
    pub favorite: bool,
    pub disposition: Option<String>,
    pub local_path: Option<String>,
    #[serde(default = "no_local_availability")]
    pub local_availability: String,
    #[serde(default)]
    pub asset_id: Option<String>,
    #[serde(default)]
    pub provenance: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackRange {
    pub parent_track_id: String,
    pub chapter_id: String,
    pub source_url: String,
    pub start_time: f64,
    pub end_time: f64,
    pub parent_title: String,
    #[serde(default)]
    pub parent_duration_seconds: Option<f64>,
}

fn no_local_availability() -> String {
    "none".into()
}

#[cfg(test)]
mod date_added_compatibility_tests {
    use super::Track;
    use serde_json::{json, Value};

    #[test]
    fn date_added_round_trips_valid_and_invalid_legacy_json_without_string_errors() {
        let mut base = Track {
            id: "fixture:date-added".into(),
            provider: "fixture".into(),
            provider_id: "date-added".into(),
            source_url: String::new(),
            artist: "Artist".into(),
            title: "Title".into(),
            provenance: json!({"dateAdded":"2020-01-01"}),
            ..Track::default()
        };

        for value in [
            json!("2024-01-02T03:04:05Z"),
            json!(42),
            json!(false),
            json!({"legacy":"object"}),
            json!(["legacy", 1]),
        ] {
            base.date_added = Some(value.clone());
            let encoded = serde_json::to_vec(&base).unwrap();
            let decoded: Track = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded.date_added, Some(value.clone()));
            let reencoded = serde_json::to_value(decoded).unwrap();
            assert_eq!(reencoded.get("dateAdded"), Some(&value));
        }
    }

    #[test]
    fn null_or_absent_date_added_stays_omitted_like_the_unknown_field_behavior() {
        let base = Track {
            id: "fixture:date-added-null".into(),
            provider: "fixture".into(),
            provider_id: "date-added-null".into(),
            source_url: String::new(),
            artist: "Artist".into(),
            title: "Title".into(),
            provenance: Value::Null,
            ..Track::default()
        };
        let mut raw = serde_json::to_value(&base).unwrap();
        raw["dateAdded"] = Value::Null;
        let decoded: Track = serde_json::from_value(raw).unwrap();
        assert_eq!(decoded.date_added, None);
        assert!(serde_json::to_value(decoded)
            .unwrap()
            .get("dateAdded")
            .is_none());
    }

    #[test]
    fn manual_date_added_override_survives_the_hydration_serialize_merge_deserialize_shape() {
        let provider = Track {
            id: "fixture:date-added-manual".into(),
            provider: "fixture".into(),
            provider_id: "date-added-manual".into(),
            source_url: String::new(),
            artist: "Artist".into(),
            title: "Title".into(),
            date_added: Some(json!("2020-01-01")),
            provenance: json!({"dateAdded":"2019-01-01"}),
            ..Track::default()
        };

        // hydrate serializes its decoded Track, merges manual top-level fields,
        // then deserializes. The additive field must survive all three steps.
        let mut merged = serde_json::to_value(&provider).unwrap();
        merged["dateAdded"] = json!(12345);
        let hydrated: Track = serde_json::from_value(merged).unwrap();
        assert_eq!(hydrated.date_added, Some(json!(12345)));
        let api_value = serde_json::to_value(hydrated).unwrap();
        assert_eq!(api_value["dateAdded"], 12345);
    }
}

#[cfg(test)]
mod download_layout_compatibility_tests {
    use super::{DownloadJob, DownloadLayout, Settings};

    #[test]
    fn fresh_settings_are_organized_but_legacy_settings_remain_flat() {
        assert_eq!(
            Settings::default().download_layout,
            DownloadLayout::ArtistAlbum
        );
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("downloadLayout");
        let legacy: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.download_layout, DownloadLayout::Flat);
        for (wire, expected) in [
            ("flat", DownloadLayout::Flat),
            ("artistAlbum", DownloadLayout::ArtistAlbum),
            ("playlist", DownloadLayout::Playlist),
        ] {
            assert_eq!(
                serde_json::from_str::<DownloadLayout>(&format!("\"{wire}\"")).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn pre_layout_download_jobs_recover_with_flat_snapshot() {
        let mut value = serde_json::json!({
            "id":"legacy", "track":{}, "candidate":{}, "state":"queued",
            "progress":0.0, "error":null, "outputPath":null, "createdAt":"",
            "outputProfile":"source"
        });
        value["track"] = serde_json::to_value(super::Track::default()).unwrap();
        value["candidate"] = serde_json::to_value(super::Candidate::default()).unwrap();
        let job: DownloadJob = serde_json::from_value(value).unwrap();
        assert_eq!(job.download_layout, DownloadLayout::Flat);
        assert!(job.playlist_context.is_none());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub id: String,
    pub name: String,
    pub chart_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub source_url: String,
    pub fetched_at: String,
    pub tracks: Vec<Track>,
    pub stale: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartMembership {
    pub provider: String,
    pub title: String,
    pub source_url: String,
    pub rank: Option<u32>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub kind: String,
    pub url: String,
    pub can_seek: bool,
    pub can_volume: bool,
    pub can_autoplay: bool,
    pub is_excerpt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormat {
    pub id: String,
    pub codec: String,
    pub extension: String,
    pub bitrate: Option<f64>,
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub channels: Option<u32>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    pub lossless: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_metadata: Option<serde_json::Value>,
    pub id: String,
    pub provider: String,
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub artwork_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<Chapter>>,
    pub formats: Vec<AudioFormat>,
    pub permission: String,
    pub confidence: String,
    pub score: f64,
    pub reasons: Vec<String>,
    pub official: bool,
    /// Typed origin metadata for sources that do not have a public HTTP URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CandidateSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub id: String,
    pub title: String,
    #[serde(rename = "startTime")]
    pub start_seconds: f64,
    #[serde(rename = "endTime")]
    pub end_seconds: f64,
    pub confidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_track: Option<Box<Track>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion_confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CandidateSource {
    Soulseek {
        username: String,
        filename: String,
        size_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bitrate_kbps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        search_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub track: Track,
    pub candidate: Candidate,
    pub format_id: Option<String>,
    pub output_profile: String,
    #[serde(default)]
    pub disposition: ReplacementDisposition,
    #[serde(default)]
    pub chapter_ids: Option<Vec<String>>,
    #[serde(default)]
    pub playlist_context: Option<DownloadPlaylistContext>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DownloadLayout {
    Flat,
    ArtistAlbum,
    Playlist,
}

impl Default for DownloadLayout {
    fn default() -> Self {
        Self::Flat
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPlaylistContext {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SoulseekProgress {
    pub bytes_received: u64,
    #[serde(default)]
    pub last_progress_at: Option<String>,
    pub speed_bytes_per_second: f64,
    pub queue_position: Option<u64>,
    pub queue_checked_at: Option<String>,
    pub queue_error: Option<String>,
    pub attempts: u64,
    pub next_attempt_at: Option<String>,
    pub daemon_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJob {
    pub id: String,
    pub track: Track,
    pub candidate: Candidate,
    pub state: String,
    pub progress: f64,
    pub error: Option<String>,
    pub output_path: Option<String>,
    pub created_at: String,
    pub output_profile: String,
    #[serde(default)]
    pub disposition: ReplacementDisposition,
    #[serde(default)]
    pub format_id: Option<String>,
    #[serde(default)]
    pub chapter_ids: Option<Vec<String>>,
    #[serde(default)]
    pub soulseek: Option<SoulseekProgress>,
    #[serde(default)]
    pub download_layout: DownloadLayout,
    #[serde(default)]
    pub playlist_context: Option<DownloadPlaylistContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub music_directory: String,
    #[serde(default)]
    pub download_layout: DownloadLayout,
    pub auto_preview: bool,
    #[serde(default)]
    pub playback_diagnostics_enabled: bool,
    #[serde(default)]
    pub live_bpm_enabled: bool,
    #[serde(default = "default_matching_level")]
    pub matching_level: crate::recording_match::MatchLevel,
    /// Separate consent for staged weak-metadata audio verification.
    #[serde(default)]
    pub download_audio_verification_enabled: bool,
    #[serde(default)]
    pub pause_on_source_open: bool,
    #[serde(default)]
    pub source_open_at_position: bool,
    pub preview_delay_ms: u32,
    #[serde(default = "default_playback_startup_timeout_seconds")]
    pub playback_startup_timeout_seconds: u32,
    pub theme: String,
    pub yt_dlp_path: String,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub js_runtime_path: String,
    #[serde(default = "default_download_source")]
    pub preferred_download_source: String,
    #[serde(default = "default_stream_source")]
    pub preferred_streaming_source: String,
    #[serde(default)]
    pub override_playlist_track_urls: bool,
    #[serde(default = "default_true")]
    pub streaming_fallback_enabled: bool,
    #[serde(default = "default_streaming_fallback_delay")]
    pub streaming_fallback_delay_ms: u32,
    #[serde(default = "default_streaming_fallbacks")]
    pub streaming_fallback_sources: Vec<String>,
    #[serde(default = "default_true")]
    pub bandcamp_fallback_enabled: bool,
    #[serde(default = "default_true")]
    pub background_source_preparation: bool,
    #[serde(default)]
    pub download_fallback_enabled: bool,
    #[serde(default = "default_download_fallbacks")]
    pub download_fallback_sources: Vec<String>,
    #[serde(default)]
    pub download_fallback_allow_lossy: bool,
    #[serde(default)]
    pub playlist_directories: Vec<String>,
}
fn default_playback_startup_timeout_seconds() -> u32 {
    20
}
fn default_matching_level() -> crate::recording_match::MatchLevel {
    crate::recording_match::MatchLevel::Balanced
}
impl Settings {
    /// Old or manually edited settings must never create an unbounded deadline.
    pub fn playback_startup_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(match self.playback_startup_timeout_seconds {
            20 | 30 | 45 => self.playback_startup_timeout_seconds as u64,
            _ => default_playback_startup_timeout_seconds() as u64,
        })
    }

    pub fn validate_playback_startup_timeout(&self) -> Result<(), String> {
        if matches!(self.playback_startup_timeout_seconds, 20 | 30 | 45) {
            Ok(())
        } else {
            Err("Playback startup timeout must be 20, 30 or 45 seconds".into())
        }
    }
}
fn default_streaming_fallback_delay() -> u32 {
    1500
}
fn default_true() -> bool {
    true
}
fn default_streaming_fallbacks() -> Vec<String> {
    vec!["youtube".into(), "soundcloud".into()]
}
fn default_download_fallbacks() -> Vec<String> {
    vec![
        "monochrome".into(),
        "slskd".into(),
        "youtube".into(),
        "soundcloud".into(),
    ]
}
fn default_download_source() -> String {
    "youtube".into()
}
fn default_stream_source() -> String {
    "youtube".into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            music_directory: String::new(),
            download_layout: DownloadLayout::ArtistAlbum,
            auto_preview: false,
            playback_diagnostics_enabled: false,
            live_bpm_enabled: false,
            matching_level: default_matching_level(),
            download_audio_verification_enabled: false,
            pause_on_source_open: false,
            source_open_at_position: false,
            preview_delay_ms: 250,
            playback_startup_timeout_seconds: default_playback_startup_timeout_seconds(),
            theme: "dark".into(),
            yt_dlp_path: "yt-dlp".into(),
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            js_runtime_path: "node".into(),
            preferred_download_source: default_download_source(),
            preferred_streaming_source: default_stream_source(),
            override_playlist_track_urls: false,
            streaming_fallback_enabled: true,
            streaming_fallback_delay_ms: default_streaming_fallback_delay(),
            streaming_fallback_sources: default_streaming_fallbacks(),
            bandcamp_fallback_enabled: true,
            background_source_preparation: true,
            download_fallback_enabled: false,
            download_fallback_sources: default_download_fallbacks(),
            download_fallback_allow_lossy: false,
            playlist_directories: Vec::new(),
        }
    }
}

#[cfg(test)]
mod playback_startup_settings_tests {
    use super::Settings;
    use std::time::Duration;

    #[test]
    fn legacy_settings_default_to_twenty_seconds() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("playbackStartupTimeoutSeconds");
        let settings: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(settings.playback_startup_timeout_seconds, 20);
        assert_eq!(settings.playback_startup_timeout(), Duration::from_secs(20));
    }

    #[test]
    fn supported_timeouts_round_trip_and_invalid_values_are_bounded() {
        for seconds in [20, 30, 45] {
            let settings = Settings {
                playback_startup_timeout_seconds: seconds,
                ..Settings::default()
            };
            let encoded = serde_json::to_value(&settings).unwrap();
            assert_eq!(encoded["playbackStartupTimeoutSeconds"], seconds);
            let restored: Settings = serde_json::from_value(encoded).unwrap();
            assert_eq!(
                restored.playback_startup_timeout(),
                Duration::from_secs(seconds as u64)
            );
            assert!(restored.validate_playback_startup_timeout().is_ok());
        }
        for seconds in [0, 1, 19, 21, 46, u32::MAX] {
            let settings = Settings {
                playback_startup_timeout_seconds: seconds,
                ..Settings::default()
            };
            assert_eq!(settings.playback_startup_timeout(), Duration::from_secs(20));
            assert!(settings.validate_playback_startup_timeout().is_err());
        }
    }
}

#[cfg(test)]
mod matching_level_settings_tests {
    use super::Settings;
    use crate::recording_match::MatchLevel;
    use serde_json::json;

    #[test]
    fn legacy_settings_default_to_balanced_matching() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("matchingLevel");

        let settings: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(settings.matching_level, MatchLevel::Balanced);
        assert_eq!(Settings::default().matching_level, MatchLevel::Balanced);
    }

    #[test]
    fn staged_audio_verification_requires_explicit_separate_opt_in() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("downloadAudioVerificationEnabled");
        let restored: Settings = serde_json::from_value(legacy.clone()).unwrap();
        assert!(!restored.download_audio_verification_enabled);
        legacy["downloadAudioVerificationEnabled"] = json!(true);
        let opted_in: Settings = serde_json::from_value(legacy).unwrap();
        assert!(opted_in.download_audio_verification_enabled);
        assert_eq!(
            serde_json::to_value(opted_in).unwrap()["downloadAudioVerificationEnabled"],
            json!(true)
        );
    }

    #[test]
    fn matching_level_serializes_and_deserializes_all_choices() {
        for (level, serialized) in [
            (MatchLevel::Exact, "exact"),
            (MatchLevel::Balanced, "balanced"),
            (MatchLevel::Flexible, "flexible"),
        ] {
            let mut settings = Settings::default();
            settings.matching_level = level.clone();
            let mut value = serde_json::to_value(settings).unwrap();
            assert_eq!(value["matchingLevel"], json!(serialized));
            value["matchingLevel"] = json!(serialized);
            let restored: Settings = serde_json::from_value(value).unwrap();
            assert_eq!(restored.matching_level, level);
        }
    }
}

#[cfg(test)]
mod live_bpm_settings_tests {
    use super::Settings;

    #[test]
    fn live_bpm_defaults_off_for_legacy_settings_and_round_trips() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("liveBpmEnabled");
        let restored: Settings = serde_json::from_value(legacy).unwrap();
        assert!(!restored.live_bpm_enabled);

        let enabled = Settings {
            live_bpm_enabled: true,
            ..Settings::default()
        };
        let encoded = serde_json::to_value(&enabled).unwrap();
        assert_eq!(encoded["liveBpmEnabled"], true);
        let round_trip: Settings = serde_json::from_value(encoded).unwrap();
        assert!(round_trip.live_bpm_enabled);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAsset {
    pub id: String,
    pub recording_id: String,
    pub path: String,
    pub hash: String,
    pub source_codec: String,
    pub final_codec: String,
    pub source_bitrate: Option<f64>,
    pub final_bitrate: Option<f64>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channels: Option<u32>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// `stream`, `format`, or `file-size-estimate`; estimated values are not codec metadata.
    #[serde(default)]
    pub bitrate_origin: Option<String>,
    pub processing: String,
    pub track: Track,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioFacts {
    pub codec: String,
    pub bitrate_kbps: Option<f64>,
    pub bitrate_origin: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bit_depth: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReplacementDisposition {
    Keep,
    Replace {
        target_asset_id: String,
        expected_hash: String,
    },
}

impl Default for ReplacementDisposition {
    fn default() -> Self {
        Self::Keep
    }
}
