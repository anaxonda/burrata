//! Versioned portable M3U8 playlists.
//!
//! This module is deliberately side-effect free: playlist locations are inert
//! strings, and parsing never resolves URLs, opens paths, mutates history, or
//! changes favorites. See `docs/portable-playlist-format.md` for the wire contract.

use crate::models::Track;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_RECORD_BYTES: usize = 32 * 1024;
const TOKEN_MAX: usize = 64;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistPurpose {
    #[default]
    Share,
    #[serde(rename = "mirror")]
    LocalMirror,
    History,
}

impl PlaylistPurpose {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Share => "share",
            Self::LocalMirror => "mirror",
            Self::History => "history",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistExportOptions {
    #[serde(default)]
    pub purpose: PlaylistPurpose,
    #[serde(default)]
    pub include_ytdlp_search: bool,
    /// Optional managed-mirror resource policy. `None` retains the existing
    /// purpose-specific behavior for older callers and saved settings.
    #[serde(default)]
    pub include_media_links: Option<bool>,
    #[serde(default)]
    pub include_notes: bool,
    #[serde(default)]
    pub include_tags: bool,
    #[serde(default)]
    pub include_ratings: bool,
    #[serde(default)]
    pub include_history_dates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NamedItem {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCredit {
    pub name: String,
    #[serde(default = "default_artist_role")]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_phrase: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

fn default_artist_role() -> String {
    "primary".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub value: Value,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReference {
    pub namespace: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Portable identity fields only. This intentionally has no `Track.provenance`
/// member so exporters cannot accidentally leak an arbitrary provenance blob.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortableTrack {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<ArtistCredit>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub rating: Option<u8>,
    #[serde(default)]
    pub catalog_references: Vec<CatalogReference>,
    #[serde(default)]
    pub bpm: Option<Measurement>,
    #[serde(default)]
    pub key: Option<Measurement>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl PortableTrack {
    /// Copy only the public, intentionally portable subset of a library Track.
    pub fn from_track(track: &Track) -> Self {
        let artists = track
            .provenance
            .get("artistCredits")
            .and_then(Value::as_array)
            .map(|credits| {
                credits
                    .iter()
                    .filter_map(|credit| {
                        let name = credit
                            .get("creditedAs")
                            .and_then(Value::as_str)
                            .or_else(|| credit.get("name").and_then(Value::as_str))
                            .map(str::trim)
                            .filter(|name| !name.is_empty())?;
                        let role = credit
                            .get("role")
                            .and_then(Value::as_str)
                            .filter(|role| !role.trim().is_empty())
                            .unwrap_or("primary");
                        let join_phrase = credit
                            .get("joinPhrase")
                            .and_then(Value::as_str)
                            .filter(|phrase| !phrase.is_empty())
                            .map(str::to_owned);
                        Some(ArtistCredit {
                            name: name.to_owned(),
                            role: role.to_owned(),
                            join_phrase,
                            extensions: BTreeMap::new(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let artists = if artists.is_empty() && !track.artist.trim().is_empty() {
            vec![ArtistCredit {
                name: track.artist.trim().to_owned(),
                role: "primary".into(),
                ..ArtistCredit::default()
            }]
        } else {
            artists
        };
        let bpm = track
            .bpm
            .filter(|value| value.is_finite() && *value > 0.0)
            .and_then(|value| {
                known_measurement_source(track, "bpm").map(|source| Measurement {
                    value: serde_json::json!(value),
                    source,
                })
            });
        let key = track
            .key
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| {
                known_measurement_source(track, "key").map(|source| Measurement {
                    value: Value::String(value.clone()),
                    source,
                })
            });
        Self {
            id: String::new(),
            title: track.title.clone(),
            artists,
            album: track.album.clone(),
            album_artist: track.album_artist.clone(),
            version: track.version.clone(),
            duration_seconds: track
                .duration_seconds
                .filter(|value| value.is_finite() && *value > 0.0),
            isrc: track
                .isrc
                .as_deref()
                .and_then(crate::recording_identity::normalize_isrc),
            rating: track
                .provenance
                .get("rating")
                .and_then(Value::as_u64)
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| *value <= 5),
            catalog_references: public_catalog_references(track),
            bpm,
            key,
            tags: track
                .provenance
                .get("tags")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
                        .take(128)
                        .collect()
                })
                .unwrap_or_default(),
            extensions: BTreeMap::new(),
        }
    }

    pub fn to_track(&self) -> Track {
        let artist = format_artist_credit(&self.artists);
        let identity = format!(
            "{}\n{}\n{}",
            artist.to_lowercase(),
            self.title.to_lowercase(),
            self.version.as_deref().unwrap_or_default().to_lowercase()
        );
        let digest = sha256_hex(identity.as_bytes());
        Track {
            id: format!("import:{digest}"),
            provider: "import".into(),
            provider_id: digest,
            source_url: String::new(),
            artist,
            title: self.title.clone(),
            version: self.version.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            duration_seconds: self.duration_seconds,
            bpm: self
                .bpm
                .as_ref()
                .and_then(|item| item.value.as_f64())
                .filter(|value| value.is_finite()),
            key: self
                .key
                .as_ref()
                .and_then(|item| item.value.as_str())
                .map(str::to_owned),
            isrc: self.isrc.clone(),
            provenance: serde_json::json!({
                "identity":"unverified portable playlist metadata",
                "catalogReferences":self.catalog_references,
                "artistCredits":self.artists.iter().map(|credit| serde_json::json!({
                    "creditedAs":credit.name,
                    "role":credit.role,
                    "joinPhrase":credit.join_phrase
                })).collect::<Vec<_>>(),
                "tags":self.tags,
                "rating":self.rating,
                "bpmSource":self.bpm.as_ref().map(|item| &item.source),
                "keySource":self.key.as_ref().map(|item| &item.source)
            }),
            ..Track::default()
        }
    }

    fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() || self.title.len() > 4096 {
            return Err("Portable track title is missing or too long".into());
        }
        if self.artists.len() > 64
            || self
                .artists
                .iter()
                .any(|credit| credit.name.trim().is_empty() || credit.name.len() > 1024)
        {
            return Err("Portable artist credits are missing or too large".into());
        }
        if self.duration_seconds.is_some_and(|value| {
            !value.is_finite() || value <= 0.0 || value > 7.0 * 24.0 * 60.0 * 60.0
        }) {
            return Err("Portable track duration is outside supported bounds".into());
        }
        if self.isrc.as_ref().is_some_and(|value| {
            crate::recording_identity::normalize_isrc(value).as_deref() != Some(value.as_str())
        }) {
            return Err("Portable ISRC must be normalized uppercase".into());
        }
        if self.catalog_references.len() > 32 || self.tags.len() > 128 {
            return Err("Portable track has too many catalog references or tags".into());
        }
        if self.rating.is_some_and(|rating| rating > 5) {
            return Err("Portable rating must be between 0 and 5".into());
        }
        for measurement in self.bpm.iter().chain(self.key.iter()) {
            if !matches!(
                measurement.source.as_str(),
                "manual" | "tag" | "catalog" | "analysis"
            ) {
                return Err("Portable measurement has an unsupported provenance".into());
            }
        }
        if self.bpm.as_ref().is_some_and(|measurement| {
            measurement
                .value
                .as_f64()
                .is_none_or(|value| !value.is_finite() || value <= 0.0 || value > 1000.0)
        }) {
            return Err("Portable BPM measurement is invalid".into());
        }
        if self.key.as_ref().is_some_and(|measurement| {
            measurement
                .value
                .as_str()
                .is_none_or(|value| value.trim().is_empty() || value.len() > 32)
        }) {
            return Err("Portable key measurement is invalid".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackRange {
    pub start_seconds: f64,
    pub end_seconds: f64,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOccurrence {
    #[serde(default)]
    pub occurrence_id: Option<String>,
    #[serde(default)]
    pub played_at: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortableEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub cue_points: Vec<f64>,
    #[serde(default)]
    pub playback_range: Option<PlaybackRange>,
    #[serde(default)]
    pub history: Option<HistoryOccurrence>,
    /// Raw location from an imported file. It is never opened or executed.
    #[serde(skip)]
    pub resource: Option<String>,
    /// Caller-validated resource for local-mirror serialization only.
    #[serde(skip)]
    pub resolved_resource: Option<String>,
    #[serde(default)]
    pub track: PortableTrack,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInput {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub folder_ancestry: Vec<NamedItem>,
    #[serde(default)]
    pub sections: Vec<NamedItem>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub entries: Vec<PortableEntry>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistExportPreview {
    pub purpose: PlaylistPurpose,
    pub entry_count: usize,
    pub local_entries: usize,
    pub identity_placeholders: usize,
    pub search_links: usize,
    pub requires_burrata: bool,
    pub excluded_private_fields: Vec<String>,
}

impl PlaylistInput {
    pub fn from_tracks(name: impl Into<String>, tracks: &[Track]) -> Self {
        let mut entries = Vec::with_capacity(tracks.len());
        for track in tracks {
            entries.push(PortableEntry {
                track: PortableTrack::from_track(track),
                ..PortableEntry::default()
            });
        }
        Self {
            name: name.into(),
            entries,
            ..Self::default()
        }
    }
}

/// Describe the output behavior and privacy filters before writing or copying.
pub fn preview_export(
    input: &PlaylistInput,
    options: &PlaylistExportOptions,
) -> Result<PlaylistExportPreview> {
    validate_playlist(input)?;
    if input.entries.len() > MAX_ENTRIES {
        return Err("Portable playlist exceeds the 10,000 occurrence limit".into());
    }
    let mut preview = PlaylistExportPreview {
        purpose: options.purpose,
        entry_count: input.entries.len(),
        ..PlaylistExportPreview::default()
    };
    for entry in &input.entries {
        entry.track.validate()?;
        match options.purpose {
            PlaylistPurpose::LocalMirror
                if options.include_media_links != Some(false)
                    && entry
                        .resolved_resource
                        .as_deref()
                        .is_some_and(safe_resource_line) =>
            {
                preview.local_entries += 1;
            }
            PlaylistPurpose::History
                if options.include_media_links == Some(true)
                    && entry
                        .resolved_resource
                        .as_deref()
                        .is_some_and(safe_resource_line) =>
            {
                preview.local_entries += 1;
            }
            PlaylistPurpose::Share | PlaylistPurpose::LocalMirror | PlaylistPurpose::History
                if options.include_ytdlp_search =>
            {
                preview.search_links += 1;
            }
            _ => preview.identity_placeholders += 1,
        }
    }
    preview.requires_burrata = preview.identity_placeholders > 0;
    preview.excluded_private_fields = if options.purpose == PlaylistPurpose::LocalMirror {
        // Local mirrors intentionally serialize resolved resource lines. Do
        // not claim those paths or URLs are redacted in the preview.
        vec!["local asset, database, and sender playlist identifiers".into()]
    } else {
        vec![
            "filesystem paths and local asset identifiers".into(),
            "database and sender playlist identifiers".into(),
            "private provider URLs, signed media URLs, and credentials".into(),
        ]
    };
    if !options.include_tags {
        preview.excluded_private_fields.push("personal tags".into());
    }
    if !options.include_ratings {
        preview
            .excluded_private_fields
            .push("personal ratings".into());
    }
    if !options.include_notes {
        preview
            .excluded_private_fields
            .push("playlist notes and cue points".into());
    }
    if options.purpose != PlaylistPurpose::History || !options.include_history_dates {
        preview
            .excluded_private_fields
            .push("listening timestamps and session identifiers".into());
    }
    Ok(preview)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Serialize a portable playlist. Share output never reuses input resource lines.
/// Typed fields in this schema round-trip in both purposes; arbitrary unknown
/// extension fields are retained only for local forms because they cannot be
/// classified as safe to share without understanding their semantics.
pub fn serialize(input: &PlaylistInput, options: &PlaylistExportOptions) -> Result<String> {
    validate_playlist(input)?;
    if input.entries.len() > MAX_ENTRIES {
        return Err("Portable playlist exceeds the 10,000 occurrence limit".into());
    }
    let purpose = options.purpose;
    let section_map = input
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| (section.id.clone(), format!("s{}", index + 1)))
        .collect::<HashMap<_, _>>();
    let folders = input.folder_ancestry.iter().enumerate().map(|(index, folder)| {
        let mut value = serde_json::json!({"id":format!("f{}", index + 1), "name":clean_text(&folder.name)});
        if purpose != PlaylistPurpose::Share { merge_extensions(&mut value, &folder.extensions, &["id", "name"]); }
        value
    }).collect::<Vec<_>>();
    let sections = input.sections.iter().enumerate().map(|(index, section)| {
        let mut value = serde_json::json!({"id":format!("s{}", index + 1), "name":clean_text(&section.name)});
        if purpose != PlaylistPurpose::Share { merge_extensions(&mut value, &section.extensions, &["id", "name"]); }
        value
    }).collect::<Vec<_>>();
    let mut playlist_record = serde_json::json!({
        "schemaVersion":1,
        "id":"p1",
        "purpose":purpose.as_wire(),
        "name":clean_text(&input.name),
        "description":input.description.as_deref().map(clean_text),
        "folderAncestry":folders,
        "sections":sections,
        "createdAt":if purpose == PlaylistPurpose::Share { None::<String> } else { input.created_at.clone() },
        "updatedAt":if purpose == PlaylistPurpose::Share { None::<String> } else { input.updated_at.clone() },
    });
    if purpose != PlaylistPurpose::Share {
        merge_extensions(
            &mut playlist_record,
            &input.extensions,
            &[
                "schemaVersion",
                "id",
                "purpose",
                "name",
                "description",
                "folderAncestry",
                "sections",
                "createdAt",
                "updatedAt",
            ],
        );
    }
    let mut out = String::from("#EXTM3U\n");
    write_record(&mut out, "#BURRATA-PLAYLIST:", &playlist_record)?;
    let mut identity_ids: HashMap<String, String> = HashMap::new();
    for (index, entry) in input.entries.iter().enumerate() {
        let mut portable = entry.track.clone();
        portable.validate()?;
        portable.id.clear();
        if purpose == PlaylistPurpose::Share {
            // Flattened unknown fields are already present after converting
            // the typed record to JSON; Share must exclude them unless this
            // version understands and explicitly allowlists their meaning.
            portable.extensions.clear();
        }
        if !options.include_tags {
            portable.tags.clear();
        }
        if !options.include_ratings {
            portable.rating = None;
        }
        portable
            .catalog_references
            .retain(|reference| public_reference(reference));
        let fingerprint = serde_json::to_string(&portable).map_err(|error| error.to_string())?;
        let track_id = if let Some(id) = identity_ids.get(&fingerprint) {
            id.clone()
        } else {
            let id = format!("t{}", identity_ids.len() + 1);
            identity_ids.insert(fingerprint.clone(), id.clone());
            id
        };
        portable.id = track_id.clone();
        let mut track_value = serde_json::to_value(&portable).map_err(|error| error.to_string())?;
        track_value["schemaVersion"] = Value::from(1);
        if purpose != PlaylistPurpose::Share {
            merge_extensions(
                &mut track_value,
                &entry.track.extensions,
                &[
                    "schemaVersion",
                    "id",
                    "title",
                    "artists",
                    "album",
                    "albumArtist",
                    "version",
                    "durationSeconds",
                    "isrc",
                    "rating",
                    "catalogReferences",
                    "bpm",
                    "key",
                    "tags",
                ],
            );
        }
        let mut entry_value = serde_json::json!({
            "schemaVersion":1,
            "id":format!("e{}", index + 1),
            "track":track_id,
            "order":index + 1,
            "sectionId":entry.section_id.as_ref().and_then(|id| section_map.get(id)).cloned(),
            "note":if options.include_notes { entry.note.as_deref().map(clean_text) } else { None::<String> },
            "cuePoints":if options.include_notes { entry.cue_points.clone() } else { Vec::new() },
            "playbackRange":if options.include_notes { entry.playback_range.clone() } else { None },
            "history":history_value(entry, index, options, purpose),
        });
        if purpose != PlaylistPurpose::Share {
            merge_extensions(
                &mut entry_value,
                &entry.extensions,
                &[
                    "schemaVersion",
                    "id",
                    "track",
                    "order",
                    "sectionId",
                    "note",
                    "cuePoints",
                    "playbackRange",
                    "history",
                ],
            );
        }
        write_record(&mut out, "#BURRATA-TRACK:", &track_value)?;
        write_record(&mut out, "#BURRATA-ENTRY:", &entry_value)?;
        let duration = portable
            .duration_seconds
            .map(|value| value.round() as i64)
            .filter(|value| *value >= 0)
            .unwrap_or(-1);
        let display = display_name(&portable);
        out.push_str(&format!("#EXTINF:{duration},{display}\n"));
        let permits_media_links = match purpose {
            PlaylistPurpose::LocalMirror => options.include_media_links != Some(false),
            PlaylistPurpose::History => options.include_media_links == Some(true),
            PlaylistPurpose::Share => false,
        };
        let local_resource = if permits_media_links {
            entry
                .resolved_resource
                .as_deref()
                .filter(|value| safe_resource_line(value))
                .map(str::to_owned)
        } else {
            None
        };
        let resource = if let Some(local) = local_resource {
            local
        } else if options.include_ytdlp_search {
            search_link(&portable)
        } else {
            format!("burrata://track/{track_id}")
        };
        out.push_str(&resource);
        out.push('\n');
    }
    if out.len() > MAX_BYTES {
        return Err("Portable playlist exceeds the 2 MB format limit".into());
    }
    Ok(out)
}

/// Parse a standard or Burrata v1 M3U8 file without touching its resource lines.
pub fn parse(text: &str) -> std::result::Result<PlaylistInput, ParseError> {
    if text.len() > MAX_BYTES {
        return Err(ParseError {
            line: 1,
            message: "Playlist exceeds the 2 MB limit".into(),
        });
    }
    let text = text.trim_start_matches('\u{feff}');
    let mut result = PlaylistInput::default();
    let mut saw_header = false;
    let mut saw_burrata = false;
    let mut saw_playlist_record = false;
    let mut pending_track: Option<(PortableTrack, BTreeMap<String, Value>, usize)> = None;
    let mut pending_entry: Option<(Value, usize)> = None;
    let mut pending_extinf: Option<(i64, String, usize)> = None;
    let mut pending_resource: Option<String> = None;
    let mut tracks_by_id: HashMap<String, PortableTrack> = HashMap::new();
    let mut entries_by_id = std::collections::HashSet::new();
    let mut physical_position = 0usize;
    let mut line_no = 0usize;

    fn flush(
        result: &mut PlaylistInput,
        pending_track: &mut Option<(PortableTrack, BTreeMap<String, Value>, usize)>,
        pending_entry: &mut Option<(Value, usize)>,
        pending_extinf: &mut Option<(i64, String, usize)>,
        pending_resource: &mut Option<String>,
        tracks_by_id: &mut HashMap<String, PortableTrack>,
        entries_by_id: &mut std::collections::HashSet<String>,
        physical_position: &mut usize,
        saw_burrata: bool,
    ) -> std::result::Result<(), ParseError> {
        let Some((duration, label, info_line)) = pending_extinf.take() else {
            return Ok(());
        };
        *physical_position += 1;
        if *physical_position > MAX_ENTRIES {
            return Err(ParseError {
                line: info_line,
                message: "Playlist exceeds the 10,000 occurrence limit".into(),
            });
        }
        let resource = pending_resource.take();
        let track = if let Some((mut track, ext, track_line)) = pending_track.take() {
            let track_id = std::mem::take(&mut track.id);
            if !valid_token(&track_id) {
                return Err(ParseError {
                    line: track_line,
                    message: "Track id is not a valid export token".into(),
                });
            }
            track.extensions = ext;
            track.catalog_references.retain(public_reference);
            track.validate().map_err(|message| ParseError {
                line: track_line,
                message,
            })?;
            if let Some(existing) = tracks_by_id.get(&track_id) {
                if existing != &track {
                    return Err(ParseError {
                        line: track_line,
                        message: "Repeated track id has conflicting metadata".into(),
                    });
                }
            } else {
                tracks_by_id.insert(track_id.clone(), track.clone());
            }
            if let Some(resource_id) = resource.as_deref().and_then(identity_placeholder) {
                if resource_id != track_id {
                    return Err(ParseError {
                        line: info_line,
                        message: "Identity placeholder does not match the track metadata".into(),
                    });
                }
            }
            track.id = track_id;
            track
        } else {
            let (artist, title, version) = parse_display_name(&label);
            let mut track = PortableTrack {
                title,
                version,
                ..PortableTrack::default()
            };
            if !artist.is_empty() {
                track.artists.push(ArtistCredit {
                    name: artist,
                    ..ArtistCredit::default()
                });
            }
            if duration >= 0 {
                track.duration_seconds = Some(duration as f64);
            }
            track
        };
        let mut parsed_entry = PortableEntry {
            track,
            resource,
            ..PortableEntry::default()
        };
        if let Some((entry_value, entry_line)) = pending_entry.take() {
            let entry_obj = entry_value.as_object().ok_or_else(|| ParseError {
                line: entry_line,
                message: "Entry metadata must be a JSON object".into(),
            })?;
            let schema = entry_obj.get("schemaVersion").and_then(Value::as_u64);
            if schema != Some(1) {
                return Err(ParseError {
                    line: entry_line,
                    message: "Unsupported Burrata entry schema version".into(),
                });
            }
            let entry_id = entry_obj
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !valid_token(entry_id) || !entries_by_id.insert(entry_id.to_owned()) {
                return Err(ParseError {
                    line: entry_line,
                    message: "Entry id is invalid or duplicated".into(),
                });
            }
            let track_ref = entry_obj
                .get("track")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if track_ref != parsed_entry.track.id {
                return Err(ParseError {
                    line: entry_line,
                    message: "Entry track reference does not match its track record".into(),
                });
            }
            parsed_entry.id = entry_id.to_owned();
            parsed_entry.section_id = match entry_obj.get("sectionId") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if valid_token(value) => Some(value.clone()),
                _ => {
                    return Err(ParseError {
                        line: entry_line,
                        message: "Entry section reference is invalid".into(),
                    })
                }
            };
            parsed_entry.note = match entry_obj.get("note") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if value.len() <= 16 * 1024 => Some(value.clone()),
                _ => {
                    return Err(ParseError {
                        line: entry_line,
                        message: "Entry note is invalid or too large".into(),
                    })
                }
            };
            parsed_entry.cue_points = match entry_obj.get("cuePoints") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_f64().ok_or_else(|| ParseError {
                            line: entry_line,
                            message: "Cue point must be a number".into(),
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                _ => {
                    return Err(ParseError {
                        line: entry_line,
                        message: "Cue points must be an array".into(),
                    })
                }
            };
            parsed_entry.playback_range =
                match entry_obj.get("playbackRange") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(serde_json::from_value(value.clone()).map_err(
                        |error| ParseError {
                            line: entry_line,
                            message: format!("Invalid playback range: {error}"),
                        },
                    )?),
                };
            parsed_entry.history =
                match entry_obj.get("history") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(serde_json::from_value(value.clone()).map_err(
                        |error| ParseError {
                            line: entry_line,
                            message: format!("Invalid history occurrence: {error}"),
                        },
                    )?),
                };
            parsed_entry.extensions = unknown_fields(
                entry_obj,
                &[
                    "schemaVersion",
                    "id",
                    "track",
                    "order",
                    "sectionId",
                    "note",
                    "cuePoints",
                    "playbackRange",
                    "history",
                ],
            );
            let declared_order =
                entry_obj.get("order").and_then(Value::as_u64).unwrap_or(0) as usize;
            if declared_order != *physical_position {
                parsed_entry
                    .extensions
                    .insert("declaredOrder".into(), Value::from(declared_order as u64));
            }
        } else if saw_burrata {
            return Err(ParseError {
                line: info_line,
                message: "Burrata entry is missing metadata".into(),
            });
        }
        result.entries.push(parsed_entry);
        *pending_track = None;
        Ok(())
    }

    for raw in text.lines() {
        line_no += 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if line == "#EXTM3U" {
            saw_header = true;
            continue;
        }
        if let Some(json) = line.strip_prefix("#BURRATA-PLAYLIST:") {
            saw_burrata = true;
            if saw_playlist_record {
                return Err(ParseError {
                    line: line_no,
                    message: "Multiple playlist metadata records".into(),
                });
            }
            saw_playlist_record = true;
            if json.len() > MAX_RECORD_BYTES {
                return Err(ParseError {
                    line: line_no,
                    message: "Playlist metadata exceeds 32 KiB".into(),
                });
            }
            let value: Value = serde_json::from_str(json).map_err(|error| ParseError {
                line: line_no,
                message: format!("Invalid playlist JSON: {error}"),
            })?;
            let object = value.as_object().ok_or_else(|| ParseError {
                line: line_no,
                message: "Playlist metadata must be a JSON object".into(),
            })?;
            if object.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
                return Err(ParseError {
                    line: line_no,
                    message: "Unsupported Burrata playlist schema version".into(),
                });
            }
            let playlist_id = object.get("id").and_then(Value::as_str).unwrap_or_default();
            if !valid_token(playlist_id) {
                return Err(ParseError {
                    line: line_no,
                    message: "Playlist id is not a valid export token".into(),
                });
            }
            result.id = playlist_id.to_owned();
            result.name = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            result.description = object
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            result.folder_ancestry = serde_json::from_value(
                object
                    .get("folderAncestry")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            )
            .map_err(|error| ParseError {
                line: line_no,
                message: format!("Invalid folder ancestry: {error}"),
            })?;
            result.sections = serde_json::from_value(
                object
                    .get("sections")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            )
            .map_err(|error| ParseError {
                line: line_no,
                message: format!("Invalid sections: {error}"),
            })?;
            result.created_at = object
                .get("createdAt")
                .and_then(Value::as_str)
                .map(str::to_owned);
            result.updated_at = object
                .get("updatedAt")
                .and_then(Value::as_str)
                .map(str::to_owned);
            result.extensions = unknown_fields(
                object,
                &[
                    "schemaVersion",
                    "id",
                    "purpose",
                    "name",
                    "description",
                    "folderAncestry",
                    "sections",
                    "createdAt",
                    "updatedAt",
                ],
            );
            continue;
        }
        if let Some(name) = line.strip_prefix("#PLAYLIST:") {
            if result.name.is_empty() && !name.trim().is_empty() {
                result.name = clean_text(name);
            }
            continue;
        }
        if let Some(json) = line.strip_prefix("#BURRATA-TRACK:") {
            saw_burrata = true;
            flush(
                &mut result,
                &mut pending_track,
                &mut pending_entry,
                &mut pending_extinf,
                &mut pending_resource,
                &mut tracks_by_id,
                &mut entries_by_id,
                &mut physical_position,
                saw_burrata,
            )?;
            if pending_track.is_some() || pending_entry.is_some() {
                return Err(ParseError {
                    line: line_no,
                    message: "Previous Burrata entry metadata is incomplete".into(),
                });
            }
            if json.len() > MAX_RECORD_BYTES {
                return Err(ParseError {
                    line: line_no,
                    message: "Track metadata exceeds 32 KiB".into(),
                });
            }
            let value: Value = serde_json::from_str(json).map_err(|error| ParseError {
                line: line_no,
                message: format!("Invalid track JSON: {error}"),
            })?;
            let object = value.as_object().ok_or_else(|| ParseError {
                line: line_no,
                message: "Track metadata must be a JSON object".into(),
            })?;
            if object.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
                return Err(ParseError {
                    line: line_no,
                    message: "Unsupported Burrata track schema version".into(),
                });
            }
            let track: PortableTrack =
                serde_json::from_value(value.clone()).map_err(|error| ParseError {
                    line: line_no,
                    message: format!("Invalid track record: {error}"),
                })?;
            pending_track = Some((
                track,
                unknown_fields(
                    object,
                    &[
                        "schemaVersion",
                        "id",
                        "title",
                        "artists",
                        "album",
                        "albumArtist",
                        "version",
                        "durationSeconds",
                        "isrc",
                        "rating",
                        "catalogReferences",
                        "bpm",
                        "key",
                        "tags",
                    ],
                ),
                line_no,
            ));
            continue;
        }
        if let Some(json) = line.strip_prefix("#BURRATA-ENTRY:") {
            saw_burrata = true;
            if json.len() > MAX_RECORD_BYTES {
                return Err(ParseError {
                    line: line_no,
                    message: "Entry metadata exceeds 32 KiB".into(),
                });
            }
            flush(
                &mut result,
                &mut pending_track,
                &mut pending_entry,
                &mut pending_extinf,
                &mut pending_resource,
                &mut tracks_by_id,
                &mut entries_by_id,
                &mut physical_position,
                saw_burrata,
            )?;
            if pending_track.is_none() || pending_entry.is_some() {
                return Err(ParseError {
                    line: line_no,
                    message: "Entry metadata is out of order or incomplete".into(),
                });
            }
            let value: Value = serde_json::from_str(json).map_err(|error| ParseError {
                line: line_no,
                message: format!("Invalid entry JSON: {error}"),
            })?;
            pending_entry = Some((value, line_no));
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            flush(
                &mut result,
                &mut pending_track,
                &mut pending_entry,
                &mut pending_extinf,
                &mut pending_resource,
                &mut tracks_by_id,
                &mut entries_by_id,
                &mut physical_position,
                saw_burrata,
            )?;
            let (duration, label) = info.split_once(',').ok_or_else(|| ParseError {
                line: line_no,
                message: "EXTINF has no comma or display name".into(),
            })?;
            let duration = duration.parse::<i64>().map_err(|_| ParseError {
                line: line_no,
                message: "EXTINF duration must be integer seconds or -1".into(),
            })?;
            if duration < -1 {
                return Err(ParseError {
                    line: line_no,
                    message: "EXTINF duration must be -1 or nonnegative".into(),
                });
            }
            pending_extinf = Some((duration, label.to_owned(), line_no));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if pending_extinf.is_some() {
            pending_resource = Some(line.to_owned());
            flush(
                &mut result,
                &mut pending_track,
                &mut pending_entry,
                &mut pending_extinf,
                &mut pending_resource,
                &mut tracks_by_id,
                &mut entries_by_id,
                &mut physical_position,
                saw_burrata,
            )?;
        }
    }
    flush(
        &mut result,
        &mut pending_track,
        &mut pending_entry,
        &mut pending_extinf,
        &mut pending_resource,
        &mut tracks_by_id,
        &mut entries_by_id,
        &mut physical_position,
        saw_burrata,
    )?;
    if pending_track.is_some() || pending_entry.is_some() {
        return Err(ParseError {
            line: line_no.max(1),
            message: "Burrata entry metadata is incomplete".into(),
        });
    }
    if !saw_header {
        return Err(ParseError {
            line: 1,
            message: "Playlist is missing #EXTM3U".into(),
        });
    }
    if saw_burrata && result.name.is_empty() {
        return Err(ParseError {
            line: 1,
            message: "Burrata playlist metadata is missing".into(),
        });
    }
    if result.name.is_empty() {
        result.name = "Imported playlist".into();
    }
    for (index, entry) in result.entries.iter_mut().enumerate() {
        if entry.id.is_empty() {
            entry.id = format!("e{}", index + 1);
        }
        if entry.track.id.is_empty() {
            entry.track.id = format!("t{}", index + 1);
        }
    }
    validate_playlist(&result).map_err(|message| ParseError {
        line: line_no.max(1),
        message,
    })?;
    Ok(result)
}

/// Fast format discriminator for migration-safe callers. It only checks for a
/// known metadata marker; callers still need to parse and validate the file.
pub fn is_portable_playlist(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("#BURRATA-PLAYLIST:")
            || line.starts_with("#BURRATA-TRACK:")
            || line.starts_with("#BURRATA-ENTRY:")
    })
}

fn validate_playlist(input: &PlaylistInput) -> Result<()> {
    if input.name.trim().is_empty()
        || input.name.len() > 512
        || input.name.chars().any(char::is_control)
    {
        return Err("Playlist name is missing or too long".into());
    }
    if input
        .description
        .as_ref()
        .is_some_and(|value| value.len() > 4 * 1024)
    {
        return Err("Playlist description exceeds 4 KiB".into());
    }
    if input.folder_ancestry.len() > 32 || input.sections.len() > 256 {
        return Err("Playlist has too many folders or sections".into());
    }
    for item in input.folder_ancestry.iter().chain(input.sections.iter()) {
        if !valid_token(&item.id)
            || item.name.trim().is_empty()
            || item.name.len() > 512
            || item.name.chars().any(char::is_control)
        {
            return Err("Folder or section metadata is invalid".into());
        }
    }
    let section_ids = input
        .sections
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for entry in &input.entries {
        if entry
            .section_id
            .as_ref()
            .is_some_and(|section| !section_ids.contains(section.as_str()))
        {
            return Err("Playlist entry references a missing section".into());
        }
        if entry
            .note
            .as_ref()
            .is_some_and(|note| note.len() > 16 * 1024)
            || entry.cue_points.len() > 128
            || entry
                .cue_points
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("Playlist note or cue points are invalid".into());
        }
        if entry.playback_range.as_ref().is_some_and(|range| {
            !range.start_seconds.is_finite()
                || !range.end_seconds.is_finite()
                || range.start_seconds < 0.0
                || range.end_seconds <= range.start_seconds
        }) {
            return Err("Playlist playback range is invalid".into());
        }
    }
    Ok(())
}

fn history_value(
    entry: &PortableEntry,
    index: usize,
    options: &PlaylistExportOptions,
    purpose: PlaylistPurpose,
) -> Value {
    if purpose != PlaylistPurpose::History {
        return Value::Null;
    }
    let played_at = if options.include_history_dates {
        entry
            .history
            .as_ref()
            .and_then(|history| history.played_at.clone())
    } else {
        None
    };
    let occurrence_id = entry
        .history
        .as_ref()
        .and_then(|history| history.occurrence_id.clone())
        .unwrap_or_else(|| format!("h{}", index + 1));
    serde_json::json!({"occurrenceId":occurrence_id, "playedAt":played_at})
}

fn public_catalog_references(track: &Track) -> Vec<CatalogReference> {
    let Some(url) = public_page_url(&track.source_url) else {
        return Vec::new();
    };
    let Ok(parsed) = url::Url::parse(&url) else {
        return Vec::new();
    };
    let namespace = match parsed.host_str().unwrap_or_default() {
        "open.spotify.com" => match parsed.path_segments().and_then(|mut parts| parts.next()) {
            Some("track") => "spotify.track",
            Some("album") => "spotify.album",
            Some("playlist") => "spotify.playlist",
            _ => return Vec::new(),
        },
        "musicbrainz.org" => match parsed.path_segments().and_then(|mut parts| parts.next()) {
            Some("recording") => "musicbrainz.recording",
            Some("release") => "musicbrainz.release",
            _ => return Vec::new(),
        },
        "www.discogs.com" | "discogs.com" => {
            match parsed.path_segments().and_then(|mut parts| parts.next()) {
                Some("release") => "discogs.release",
                Some("master") => "discogs.master",
                _ => return Vec::new(),
            }
        }
        host if host.ends_with(".bandcamp.com") => {
            match parsed.path_segments().and_then(|mut parts| parts.next()) {
                Some("track") => "bandcamp.track",
                Some("album") => "bandcamp.album",
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };
    let id = parsed
        .path_segments()
        .and_then(|parts| parts.filter(|part| !part.is_empty()).last())
        .unwrap_or_default();
    if id.is_empty() {
        return Vec::new();
    }
    vec![CatalogReference {
        namespace: namespace.into(),
        id: id.into(),
        url: Some(url),
        extensions: BTreeMap::new(),
    }]
}

fn public_page_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let parts = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }
    let namespace = match (host, parts[0]) {
        ("open.spotify.com", "track") => "spotify.track",
        ("open.spotify.com", "album") => "spotify.album",
        ("open.spotify.com", "playlist") => "spotify.playlist",
        ("musicbrainz.org", "recording") => "musicbrainz.recording",
        ("musicbrainz.org", "release") => "musicbrainz.release",
        ("www.discogs.com" | "discogs.com", "release") => "discogs.release",
        ("www.discogs.com" | "discogs.com", "master") => "discogs.master",
        (host, "track") if host.ends_with(".bandcamp.com") => "bandcamp.track",
        (host, "album") if host.ends_with(".bandcamp.com") => "bandcamp.album",
        _ => return None,
    };
    valid_catalog_id(namespace, parts[1]).then(|| url.to_string().trim_end_matches('/').to_owned())
}

fn valid_catalog_id(namespace: &str, id: &str) -> bool {
    match namespace {
        "spotify.track" | "spotify.album" | "spotify.playlist" => {
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }
        "musicbrainz.recording" | "musicbrainz.release" => {
            id.len() == 36
                && id.bytes().enumerate().all(|(index, byte)| {
                    if matches!(index, 8 | 13 | 18 | 23) {
                        byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
        }
        "discogs.release" | "discogs.master" | "apple_music.song" | "apple_music.album" => {
            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
        }
        "bandcamp.track" | "bandcamp.album" => {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }
        _ => false,
    }
}

fn public_reference(reference: &CatalogReference) -> bool {
    let allowed_namespace = matches!(
        reference.namespace.as_str(),
        "spotify.track"
            | "spotify.album"
            | "spotify.playlist"
            | "musicbrainz.recording"
            | "musicbrainz.release"
            | "discogs.release"
            | "discogs.master"
            | "bandcamp.track"
            | "bandcamp.album"
            | "apple_music.song"
            | "apple_music.album"
    );
    if !allowed_namespace || !valid_catalog_id(&reference.namespace, reference.id.trim()) {
        return false;
    }
    reference.url.as_deref().is_none_or(|value| {
        let Some(public_url) = public_page_url(value) else {
            return false;
        };
        let Ok(parsed) = url::Url::parse(&public_url) else {
            return false;
        };
        let Some(mut parts) = parsed.path_segments() else {
            return false;
        };
        let kind = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        let expected_namespace = match (parsed.host_str().unwrap_or_default(), kind) {
            ("open.spotify.com", "track") => "spotify.track",
            ("open.spotify.com", "album") => "spotify.album",
            ("open.spotify.com", "playlist") => "spotify.playlist",
            ("musicbrainz.org", "recording") => "musicbrainz.recording",
            ("musicbrainz.org", "release") => "musicbrainz.release",
            ("discogs.com" | "www.discogs.com", "release") => "discogs.release",
            ("discogs.com" | "www.discogs.com", "master") => "discogs.master",
            (host, "track") if host.ends_with(".bandcamp.com") => "bandcamp.track",
            (host, "album") if host.ends_with(".bandcamp.com") => "bandcamp.album",
            _ => return false,
        };
        expected_namespace == reference.namespace && id == reference.id
    })
}

fn known_measurement_source(track: &Track, field: &str) -> Option<String> {
    if track.provenance["manualFields"][field] == true {
        return Some("manual".into());
    }
    let analysis_key = format!("{field}Analysis");
    if track.provenance.get(&analysis_key).is_some() {
        return Some("analysis".into());
    }
    let source_key = format!("{field}Source");
    track
        .provenance
        .get(&source_key)
        .and_then(Value::as_str)
        .or_else(|| {
            track
                .provenance
                .get("portableMeasurements")
                .and_then(|value| value.get(field))
                .and_then(Value::as_str)
        })
        .filter(|value| matches!(*value, "manual" | "tag" | "catalog" | "analysis"))
        .map(str::to_owned)
}

fn display_name(track: &PortableTrack) -> String {
    let mut out = format!(
        "{} - {}",
        format_artist_credit(&track.artists),
        clean_text(&track.title)
    );
    if let Some(version) = track
        .version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        out.push_str(" (");
        out.push_str(&clean_text(version));
        out.push(')');
    }
    out
}

fn format_artist_credit(artists: &[ArtistCredit]) -> String {
    let mut out = String::new();
    for (index, credit) in artists.iter().enumerate() {
        out.push_str(&clean_text(&credit.name));
        if let Some(join) = credit.join_phrase.as_deref() {
            out.push_str(&clean_text(join));
        } else if index + 1 < artists.len() {
            out.push_str(", ");
        }
    }
    out
}

fn parse_display_name(label: &str) -> (String, String, Option<String>) {
    let (artist, mut title) = label
        .split_once(" - ")
        .or_else(|| label.split_once(" — "))
        .or_else(|| label.split_once(" – "))
        .unwrap_or(("", label));
    let mut version = None;
    if let Some((prefix, suffix)) = title
        .strip_suffix(')')
        .and_then(|value| value.rsplit_once(" ("))
    {
        if !suffix.trim().is_empty() {
            title = prefix;
            version = Some(suffix.to_owned());
        }
    }
    (artist.trim().to_owned(), title.trim().to_owned(), version)
}

fn search_link(track: &PortableTrack) -> String {
    let mut query = format_artist_credit(&track.artists);
    if !query.is_empty() {
        query.push(' ');
    }
    query.push_str(track.title.trim());
    if let Some(version) = track
        .version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        query.push(' ');
        query.push_str(version.trim());
    }
    format!("ytdl://ytsearch1:{}", clean_text(&query))
}

fn clean_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn safe_resource_line(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16 * 1024
        && !value.chars().any(|character| character.is_control())
}

fn identity_placeholder(value: &str) -> Option<&str> {
    let id = value.strip_prefix("burrata://track/")?;
    valid_token(id).then_some(id)
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= TOKEN_MAX
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn write_record(out: &mut String, prefix: &str, value: &Value) -> Result<()> {
    let json = serde_json::to_string(value).map_err(|error| error.to_string())?;
    if json.len() > MAX_RECORD_BYTES {
        return Err("Portable metadata record exceeds 32 KiB".into());
    }
    out.push_str(prefix);
    out.push_str(&json);
    out.push('\n');
    Ok(())
}

fn merge_extensions(value: &mut Value, extensions: &BTreeMap<String, Value>, reserved: &[&str]) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for (key, extension) in extensions {
        if !reserved.contains(&key.as_str()) && !key.starts_with("_") {
            object.insert(key.clone(), extension.clone());
        }
    }
}

fn unknown_fields(object: &Map<String, Value>, known: &[&str]) -> BTreeMap<String, Value> {
    object
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/portable_playlist_v1.m3u8");

    #[test]
    fn v1_fixture_preserves_unicode_hierarchy_sections_and_duplicate_occurrences() {
        let playlist = parse(FIXTURE).unwrap();
        assert_eq!(playlist.name, "夜の道");
        assert_eq!(playlist.folder_ancestry[0].name, "Sets");
        assert_eq!(playlist.sections[0].name, "Warm up");
        assert_eq!(playlist.entries.len(), 2);
        assert_eq!(playlist.entries[0].track.id, "t1");
        assert_eq!(playlist.entries[1].track.id, "t1");
        assert_eq!(playlist.entries[0].id, "e1");
        assert_eq!(playlist.entries[1].id, "e2");
        assert_eq!(playlist.entries[0].track.artists[0].name, "Élodie");
        assert_eq!(
            playlist.entries[0].track.version.as_deref(),
            Some("Extended Mix")
        );
        assert_eq!(playlist.entries[0].section_id.as_deref(), Some("s1"));
    }

    #[test]
    fn legacy_playlist_title_before_burrata_metadata_is_not_a_duplicate_record() {
        let with_legacy_title =
            FIXTURE.replacen("#EXTM3U\n", "#EXTM3U\n#PLAYLIST:Legacy title\n", 1);
        assert_eq!(parse(&with_legacy_title).unwrap().name, "夜の道");
        let record = FIXTURE
            .lines()
            .find(|line| line.starts_with("#BURRATA-PLAYLIST:"))
            .unwrap();
        let duplicate = format!("{FIXTURE}{record}\n");
        assert!(parse(&duplicate)
            .unwrap_err()
            .message
            .contains("Multiple playlist metadata records"));
    }

    #[test]
    fn share_serializer_redacts_private_fields_and_uses_identity_placeholders_by_default() {
        let track = Track {
            id: "spotify:private-listing-id".into(),
            provider: "spotify".into(),
            provider_id: "private-listing-id".into(),
            source_url: "https://cdn.example.test/audio?token=secret".into(),
            artist: "Artist One & Guest".into(),
            title: "Track".into(),
            version: Some("Club Edit".into()),
            local_path: Some("/home/person/Music/private.flac".into()),
            provenance: serde_json::json!({"apiToken":"never-share","localPath":"/home/person/Music/private.flac","artistCredits":[{"creditedAs":"Artist One","role":"primary","joinPhrase":" & ","sourceUrl":"https://private.test/artist"},{"creditedAs":"Guest","role":"featured"}]}),
            ..Track::default()
        };
        let mut playlist = PlaylistInput::from_tracks("Set", &[track]);
        playlist.id = "sender-private-playlist-id".into();
        playlist.entries[0].id = "sender-occurrence-id".into();
        playlist.entries[0].note =
            Some("Reference https://notes.invalid/score\nFile /vault/playlist/track.flac".into());
        playlist.entries[0].track.tags = vec!["private-tag".into()];
        playlist.entries[0].resource = Some("/home/person/private.flac".into());
        playlist.entries[0].resolved_resource = Some("/home/person/private.flac".into());

        let output = serialize(&playlist, &PlaylistExportOptions::default()).unwrap();
        assert!(output.contains("burrata://track/t1"));
        assert!(output.contains("\"id\":\"p1\""));
        assert!(output.contains("\"id\":\"e1\""));
        assert!(output.contains("\"role\":\"featured\""));
        assert!(!output.contains("home/person"));
        assert!(!output.contains("private-listing-id"));
        assert!(!output.contains("apiToken"));
        assert!(!output.contains("notes.invalid"));
        assert!(!output.contains("/vault/playlist"));
        assert!(!output.contains("private-tag"));
        assert!(!output.contains("token=secret"));
    }

    #[test]
    fn public_catalog_references_require_provider_valid_ids() {
        let valid = Track {
            source_url: "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC".into(),
            ..Track::default()
        };
        let portable = PortableTrack::from_track(&valid);
        assert_eq!(portable.catalog_references.len(), 1);
        assert_eq!(portable.catalog_references[0].id, "4uLU6hMCjMI75M1A2tKUQC");

        let malformed = Track {
            source_url: "https://open.spotify.com/track/private-provider-id".into(),
            ..Track::default()
        };
        assert!(PortableTrack::from_track(&malformed)
            .catalog_references
            .is_empty());
        assert!(!public_reference(&CatalogReference {
            namespace: "spotify.track".into(),
            id: "private-provider-id".into(),
            ..CatalogReference::default()
        }));
    }

    #[test]
    fn explicit_search_link_uses_artist_title_and_version_without_generic_suffix() {
        let track = Track {
            artist: "Artist".into(),
            title: "Song".into(),
            version: Some("Extended Mix".into()),
            ..Track::default()
        };
        let playlist = PlaylistInput::from_tracks("Set", &[track]);
        let output = serialize(
            &playlist,
            &PlaylistExportOptions {
                include_ytdlp_search: true,
                ..PlaylistExportOptions::default()
            },
        )
        .unwrap();
        assert!(output.contains("ytdl://ytsearch1:Artist Song Extended Mix\n"));
        assert!(!output.contains("Extended Mix mix\n"));
        let preview = preview_export(
            &playlist,
            &PlaylistExportOptions {
                include_ytdlp_search: true,
                ..PlaylistExportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(preview.search_links, 1);
        assert_eq!(preview.identity_placeholders, 0);
        assert!(!preview.requires_burrata);
    }

    #[test]
    fn local_mirror_preview_does_not_claim_resource_redaction_while_share_does() {
        let mut playlist = PlaylistInput::from_tracks(
            "Local fixture",
            &[Track {
                id: "local:fixture-asset-id".into(),
                provider: "local".into(),
                provider_id: "fixture-asset-id".into(),
                artist: "Fixture Artist".into(),
                title: "Fixture Track".into(),
                ..Track::default()
            }],
        );
        let path = "/tmp/synthetic-private-library/fixture-track.wav";
        playlist.entries[0].resolved_resource = Some(path.into());

        let mirror_options = PlaylistExportOptions {
            purpose: PlaylistPurpose::LocalMirror,
            ..PlaylistExportOptions::default()
        };
        let mirror_preview = preview_export(&playlist, &mirror_options).unwrap();
        let mirror_output = serialize(&playlist, &mirror_options).unwrap();
        assert!(mirror_output.contains(path));
        assert!(mirror_preview
            .excluded_private_fields
            .contains(&"local asset, database, and sender playlist identifiers".into()));
        assert!(!mirror_preview
            .excluded_private_fields
            .iter()
            .any(|field| field.contains("filesystem paths") || field.contains("provider URLs")));

        let share_options = PlaylistExportOptions::default();
        let share_preview = preview_export(&playlist, &share_options).unwrap();
        let share_output = serialize(&playlist, &share_options).unwrap();
        assert!(!share_output.contains(path));
        assert!(share_preview
            .excluded_private_fields
            .contains(&"filesystem paths and local asset identifiers".into()));
        assert!(share_preview
            .excluded_private_fields
            .contains(&"private provider URLs, signed media URLs, and credentials".into()));
    }

    #[test]
    fn history_search_links_are_opt_in_and_share_the_same_preview_contract() {
        let mut playlist = PlaylistInput::from_tracks(
            "Listening History",
            &[Track {
                artist: "Artist Credit".into(),
                title: "Song".into(),
                ..Track::default()
            }],
        );
        playlist.entries[0].history = Some(HistoryOccurrence {
            occurrence_id: Some("h1".into()),
            played_at: Some("2026-01-02T03:04:05Z".into()),
            ..Default::default()
        });
        let off = PlaylistExportOptions {
            purpose: PlaylistPurpose::History,
            ..Default::default()
        };
        let off_preview = preview_export(&playlist, &off).unwrap();
        assert_eq!(off_preview.search_links, 0);
        assert_eq!(off_preview.identity_placeholders, 1);
        let on = PlaylistExportOptions {
            purpose: PlaylistPurpose::History,
            include_ytdlp_search: true,
            include_history_dates: true,
            ..Default::default()
        };
        let on_preview = preview_export(&playlist, &on).unwrap();
        let output = serialize(&playlist, &on).unwrap();
        assert_eq!(on_preview.search_links, 1);
        assert_eq!(on_preview.identity_placeholders, 0);
        assert!(output.contains("ytdl://ytsearch1:Artist Credit Song\n"));
        assert!(output.contains("playedAt\":\"2026-01-02T03:04:05Z\""));
        assert!(!output.contains("sessionId"));
    }

    #[test]
    fn managed_mirror_media_and_search_policies_are_independent_and_safe() {
        let path = "../Audio/Fixture.flac";
        let mut playlist = PlaylistInput::from_tracks(
            "Managed playlist",
            &[Track {
                artist: "Fixture Artist".into(),
                title: "Fixture Song".into(),
                ..Track::default()
            }],
        );
        playlist.entries[0].resolved_resource = Some(path.into());

        let metadata_only = PlaylistExportOptions {
            purpose: PlaylistPurpose::LocalMirror,
            include_media_links: Some(false),
            include_ytdlp_search: false,
            ..Default::default()
        };
        let output = serialize(&playlist, &metadata_only).unwrap();
        assert!(!output.contains(path));
        assert!(output.contains("burrata://track/"));

        let search_only = PlaylistExportOptions {
            include_ytdlp_search: true,
            ..metadata_only.clone()
        };
        let output = serialize(&playlist, &search_only).unwrap();
        assert!(!output.contains(path));
        assert!(output.contains("ytdl://ytsearch1:Fixture Artist Fixture Song"));

        let media_only = PlaylistExportOptions {
            include_media_links: Some(true),
            ..metadata_only.clone()
        };
        let output = serialize(&playlist, &media_only).unwrap();
        assert!(output.contains(path));
        assert!(!output.contains("ytdl://"));

        let share = PlaylistExportOptions {
            purpose: PlaylistPurpose::Share,
            include_media_links: Some(true),
            ..Default::default()
        };
        assert!(!serialize(&playlist, &share).unwrap().contains(path));

        let history = PlaylistExportOptions {
            purpose: PlaylistPurpose::History,
            include_media_links: Some(true),
            ..Default::default()
        };
        assert!(serialize(&playlist, &history).unwrap().contains(path));
    }

    #[test]
    fn measurements_need_an_allowlisted_source_and_keep_manual_or_analysis_origin() {
        let unlabelled = Track {
            bpm: Some(124.0),
            key: Some("8A".into()),
            ..Track::default()
        };
        let portable = PortableTrack::from_track(&unlabelled);
        assert!(portable.bpm.is_none());
        assert!(portable.key.is_none());

        let labelled = Track {
            bpm: Some(124.0),
            key: Some("8A".into()),
            provenance: serde_json::json!({"bpmSource":"catalog","keyAnalysis":{"source":"decodedFilePcm"},"manualFields":{"bpm":true}}),
            ..Track::default()
        };
        let portable = PortableTrack::from_track(&labelled);
        assert_eq!(portable.bpm.unwrap().source, "manual");
        assert_eq!(portable.key.unwrap().source, "analysis");
    }

    #[test]
    fn history_dates_are_opt_in_and_session_identifiers_never_serialize() {
        let track = Track {
            artist: "A".into(),
            title: "History item".into(),
            source_url: "https://private.example/signed?token=secret".into(),
            ..Track::default()
        };
        let mut input = PlaylistInput::from_tracks("History", &[track]);
        input.entries[0].history = Some(HistoryOccurrence {
            occurrence_id: Some("h-event-sha256".into()),
            played_at: Some("2026-10-25T12:00:00Z".into()),
            extensions: BTreeMap::from([(
                "sessionId".into(),
                Value::String("private-session-id".into()),
            )]),
        });
        let hidden = serialize(
            &input,
            &PlaylistExportOptions {
                purpose: PlaylistPurpose::History,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hidden.contains("\"playedAt\":null"));
        assert!(!hidden.contains("2026-10-25T12:00:00Z"));
        assert!(!hidden.contains("private-session-id"));
        assert!(!hidden.contains("private.example"));

        let visible = serialize(
            &input,
            &PlaylistExportOptions {
                purpose: PlaylistPurpose::History,
                include_history_dates: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(visible.contains("2026-10-25T12:00:00Z"));
        assert!(visible.contains("occurrenceId\":\"h-event-sha256\""));
        assert!(!visible.contains("private-session-id"));
    }

    #[test]
    fn local_mirror_accepts_only_explicit_resolved_resource_and_preserves_extensions() {
        let mut playlist = parse(FIXTURE).unwrap();
        playlist.extensions.insert(
            "x-format-note".into(),
            Value::String("kept for local round trip".into()),
        );
        playlist.entries[0].resolved_resource = Some("../Audio/Nuit.flac".into());
        let output = serialize(
            &playlist,
            &PlaylistExportOptions {
                purpose: PlaylistPurpose::LocalMirror,
                ..PlaylistExportOptions::default()
            },
        )
        .unwrap();
        assert!(output.contains("../Audio/Nuit.flac"));
        assert!(output.contains("x-format-note"));
        let parsed = parse(&output).unwrap();
        assert_eq!(
            parsed.extensions["x-format-note"],
            "kept for local round trip"
        );
    }

    #[test]
    fn extinf_only_and_location_lines_remain_separate_untrusted_occurrences() {
        let playlist = parse("#EXTM3U\n#EXTINF:180,Artist - Song (Club Mix)\n#EXTINF:-1,Another Artist — Unavailable\n").unwrap();
        assert_eq!(playlist.entries.len(), 2);
        assert_eq!(playlist.entries[0].track.title, "Song");
        assert_eq!(
            playlist.entries[0].track.version.as_deref(),
            Some("Club Mix")
        );
        assert_eq!(playlist.entries[0].resource, None);
        assert_eq!(playlist.entries[1].track.title, "Unavailable");

        let with_location = parse("#EXTM3U\n#EXTINF:90,A - B\nfile:///etc/passwd\n").unwrap();
        assert_eq!(
            with_location.entries[0].resource.as_deref(),
            Some("file:///etc/passwd")
        );
        assert!(with_location.entries[0]
            .track
            .to_track()
            .source_url
            .is_empty());
    }

    #[test]
    fn unknown_optional_metadata_round_trips_but_future_schema_is_rejected() {
        let mut playlist = parse(FIXTURE).unwrap();
        playlist.entries[0]
            .track
            .extensions
            .insert("x-observed-by".into(), Value::String("v1-reader".into()));
        let output = serialize(
            &playlist,
            &PlaylistExportOptions {
                purpose: PlaylistPurpose::LocalMirror,
                ..PlaylistExportOptions::default()
            },
        )
        .unwrap();
        assert!(parse(&output).unwrap().entries[0]
            .track
            .extensions
            .contains_key("x-observed-by"));

        // Unknown extension payloads can contain data this version cannot
        // classify as safe for sharing. Local mirrors preserve them for
        // round-trip compatibility; Share exports omit arbitrary extensions.
        let shared = serialize(
            &playlist,
            &PlaylistExportOptions {
                purpose: PlaylistPurpose::Share,
                ..PlaylistExportOptions::default()
            },
        )
        .unwrap();
        assert!(!shared.contains("x-observed-by"));
        assert!(!parse(&shared).unwrap().entries[0]
            .track
            .extensions
            .contains_key("x-observed-by"));

        let future = FIXTURE.replacen("\"schemaVersion\":1", "\"schemaVersion\":2", 1);
        assert!(parse(&future)
            .unwrap_err()
            .message
            .contains("Unsupported Burrata playlist schema version"));
    }

    #[test]
    fn mismatched_identity_placeholder_is_a_parse_conflict() {
        let conflict = FIXTURE.replacen("burrata://track/t1", "burrata://track/t2", 1);
        assert!(parse(&conflict)
            .unwrap_err()
            .message
            .contains("Identity placeholder does not match"));
    }
}
