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
    pub chapter_ids: Option<Vec<String>>,
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
    pub format_id: Option<String>,
    #[serde(default)]
    pub chapter_ids: Option<Vec<String>>,
    #[serde(default)]
    pub soulseek: Option<SoulseekProgress>,
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
    pub auto_preview: bool,
    pub preview_delay_ms: u32,
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
    #[serde(default = "default_streaming_fallbacks")]
    pub streaming_fallback_sources: Vec<String>,
    #[serde(default)]
    pub download_fallback_enabled: bool,
    #[serde(default = "default_download_fallbacks")]
    pub download_fallback_sources: Vec<String>,
    #[serde(default)]
    pub download_fallback_allow_lossy: bool,
    #[serde(default)]
    pub playlist_directories: Vec<String>,
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
            auto_preview: false,
            preview_delay_ms: 250,
            theme: "dark".into(),
            yt_dlp_path: "yt-dlp".into(),
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            js_runtime_path: "node".into(),
            preferred_download_source: default_download_source(),
            preferred_streaming_source: default_stream_source(),
            override_playlist_track_urls: false,
            streaming_fallback_enabled: true,
            streaming_fallback_sources: default_streaming_fallbacks(),
            download_fallback_enabled: false,
            download_fallback_sources: default_download_fallbacks(),
            download_fallback_allow_lossy: false,
            playlist_directories: Vec::new(),
        }
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
    pub processing: String,
    pub track: Track,
}
