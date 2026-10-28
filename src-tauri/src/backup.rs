//! Versioned backup archive contract and validation primitives.
//!
//! Archive I/O and live profile coordination are intentionally kept outside
//! this module until the app has a backup coordinator. Nothing here opens or
//! mutates the user's profile.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{self, Read};

pub use crate::backup_auxiliary::{
    auxiliary_component_id, auxiliary_component_path, snapshot_lyrics_sidecar,
    validate_auxiliary_attachments, AuxiliaryAssetKind, AuxiliaryAttachment, AuxiliaryDiscovery,
    AuxiliaryInput,
};

pub const BACKUP_FORMAT: &str = "org.burrata.backup";
pub const BACKUP_SCHEMA_VERSION: u32 = 2;
pub const OLDEST_SUPPORTED_BACKUP_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_COMPONENTS: usize = 20_032;
pub const MAX_DATA_COMPONENTS: usize = 32;
pub const MAX_AUXILIARY_ATTACHMENTS: usize = 10_000;
pub const MAX_COMPONENT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024 * 1024;
pub const MAX_LYRICS_SIDECAR_BYTES: u64 = 64 * 1024;

/// Archive component declarations are deliberately closed: the manifest may
/// not direct restore code to read arbitrary paths or execute directives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BackupComponent {
    pub id: String,
    pub path: String,
    pub encoding: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BackupMediaSummary {
    pub included: bool,
    pub assets: u64,
    pub missing: u64,
    pub changed: u64,
    pub omitted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format: String,
    pub schema_version: u32,
    pub app_version: String,
    pub created_at: String,
    pub snapshot_id: String,
    pub components: Vec<BackupComponent>,
    pub media: BackupMediaSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auxiliary_assets: Vec<AuxiliaryAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    TooLarge,
    InvalidJson(String),
    UnsupportedFormat,
    UnsupportedSchema(u32),
    InvalidField(&'static str),
    TooManyComponents,
    DuplicateComponent(String),
    DuplicatePath(String),
    UnsafePath(String),
    UnexpectedComponent(String),
    ComponentPathMismatch(String),
    UnsupportedEncoding(String),
    InvalidChecksum(String),
    ComponentTooLarge(String),
    TotalTooLarge,
    InvalidMediaSummary,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "backup manifest exceeds the size limit"),
            Self::InvalidJson(e) => write!(f, "invalid backup manifest JSON: {e}"),
            Self::UnsupportedFormat => write!(f, "unsupported backup format"),
            Self::UnsupportedSchema(v) => write!(f, "unsupported backup schema version {v}"),
            Self::InvalidField(name) => write!(f, "invalid backup manifest field {name}"),
            Self::TooManyComponents => write!(f, "backup contains too many components"),
            Self::DuplicateComponent(id) => write!(f, "duplicate backup component {id}"),
            Self::DuplicatePath(path) => write!(f, "duplicate backup path {path}"),
            Self::UnsafePath(path) => write!(f, "unsafe backup path {path}"),
            Self::UnexpectedComponent(id) => write!(f, "unsupported backup component {id}"),
            Self::ComponentPathMismatch(id) => {
                write!(f, "backup component {id} has an invalid path")
            }
            Self::UnsupportedEncoding(enc) => write!(f, "unsupported backup encoding {enc}"),
            Self::InvalidChecksum(id) => write!(f, "invalid SHA-256 for backup component {id}"),
            Self::ComponentTooLarge(id) => {
                write!(f, "backup component {id} exceeds the size limit")
            }
            Self::TotalTooLarge => write!(f, "backup exceeds the total uncompressed size limit"),
            Self::InvalidMediaSummary => write!(f, "invalid backup media summary"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl BackupManifest {
    /// Parse a manifest with a strict byte limit and validate its supported contract.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.format != BACKUP_FORMAT {
            return Err(ManifestError::UnsupportedFormat);
        }
        if !(OLDEST_SUPPORTED_BACKUP_SCHEMA_VERSION..=BACKUP_SCHEMA_VERSION)
            .contains(&self.schema_version)
        {
            return Err(ManifestError::UnsupportedSchema(self.schema_version));
        }
        if self.schema_version == OLDEST_SUPPORTED_BACKUP_SCHEMA_VERSION
            && !self.auxiliary_assets.is_empty()
        {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        }
        if self.app_version.is_empty() || self.app_version.len() > 128 {
            return Err(ManifestError::InvalidField("appVersion"));
        }
        if self.created_at.is_empty() || self.created_at.len() > 64 {
            return Err(ManifestError::InvalidField("createdAt"));
        }
        if chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|timestamp| timestamp.offset().local_minus_utc() != 0)
            .unwrap_or(true)
        {
            return Err(ManifestError::InvalidField("createdAt"));
        }
        if uuid::Uuid::parse_str(&self.snapshot_id)
            .map(|snapshot| snapshot.to_string() != self.snapshot_id)
            .unwrap_or(true)
        {
            return Err(ManifestError::InvalidField("snapshotId"));
        }
        let data_components = self
            .components
            .iter()
            .filter(|component| {
                !component.id.starts_with("media:") && !component.id.starts_with("aux-lyrics:")
            })
            .count();
        if data_components == 0
            || data_components > MAX_DATA_COMPONENTS
            || self.components.len() > MAX_COMPONENTS
        {
            return Err(ManifestError::TooManyComponents);
        }

        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        let mut total = 0u64;
        for component in &self.components {
            if !ids.insert(component.id.as_str()) {
                return Err(ManifestError::DuplicateComponent(component.id.clone()));
            }
            validate_archive_path(&component.path)?;
            if !paths.insert(component.path.as_str()) {
                return Err(ManifestError::DuplicatePath(component.path.clone()));
            }
            validate_component(component)?;
            if component.bytes > MAX_COMPONENT_BYTES {
                return Err(ManifestError::ComponentTooLarge(component.id.clone()));
            }
            total = total
                .checked_add(component.bytes)
                .ok_or(ManifestError::TotalTooLarge)?;
            if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
                return Err(ManifestError::TotalTooLarge);
            }
        }
        validate_auxiliary_attachments(&self.auxiliary_assets, &self.components)?;
        for required in ["library", "history", "settings", "playback"] {
            if !ids.contains(required) {
                return Err(ManifestError::InvalidField("components"));
            }
        }
        let media_entries = self
            .components
            .iter()
            .filter(|component| component.id.starts_with("media:"))
            .count() as u64;
        if media_entries != self.media.assets || self.media.included != (media_entries > 0) {
            return Err(ManifestError::InvalidMediaSummary);
        }
        if self
            .media
            .missing
            .checked_add(self.media.changed)
            .and_then(|n| n.checked_add(self.media.omitted))
            .is_none()
        {
            return Err(ManifestError::InvalidMediaSummary);
        }
        Ok(())
    }
}

fn validate_component(component: &BackupComponent) -> Result<(), ManifestError> {
    if let Some(token) = component.id.strip_prefix("aux-lyrics:") {
        let expected_path = format!("aux/lyrics/{token}.lrc");
        if !valid_sha256(token)
            || component.path != expected_path
            || component.encoding != "binary"
            || component.schema_version.is_some()
            || component.bytes == 0
            || component.bytes > MAX_LYRICS_SIDECAR_BYTES
            || !valid_sha256(&component.sha256)
        {
            return Err(ManifestError::ComponentPathMismatch(component.id.clone()));
        }
        return Ok(());
    }
    if let Some(asset_id) = component.id.strip_prefix("media:") {
        let valid_token = !asset_id.is_empty()
            && asset_id.len() <= 128
            && asset_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        let expected_path = format!("media/assets/{asset_id}");
        if !valid_token
            || component.path != expected_path
            || component.encoding != "binary"
            || component.schema_version.is_some()
        {
            return Err(ManifestError::ComponentPathMismatch(component.id.clone()));
        }
        if component.sha256.len() != 64
            || !component
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ManifestError::InvalidChecksum(component.id.clone()));
        }
        return Ok(());
    }
    let (expected_path, expected_encoding) = match component.id.as_str() {
        "library" => ("data/library.sqlite", "sqlite"),
        "history" => ("data/playback-history.sqlite", "sqlite"),
        "settings" => ("data/settings.json", "json"),
        "playback" => ("data/playback.json", "json"),
        _ => return Err(ManifestError::UnexpectedComponent(component.id.clone())),
    };
    if component.path != expected_path {
        return Err(ManifestError::ComponentPathMismatch(component.id.clone()));
    }
    if component.encoding != expected_encoding {
        return Err(ManifestError::UnsupportedEncoding(
            component.encoding.clone(),
        ));
    }
    if component.sha256.len() != 64
        || !component
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ManifestError::InvalidChecksum(component.id.clone()));
    }
    if component.schema_version.is_some() {
        Ok(())
    } else {
        Err(ManifestError::InvalidField("schemaVersion"))
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Reject path forms that can escape an extraction root or alias another name.
/// The v1 format uses forward-slash-separated ASCII paths and no path aliases.
pub fn validate_archive_path(path: &str) -> Result<(), ManifestError> {
    if path.is_empty()
        || path.len() > 240
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains(':')
        || path.bytes().any(|b| b == 0 || b < 0x20)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ManifestError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

/// Hash a bounded stream and require the declared byte count and digest.
/// Callers must also enforce the ZIP entry's declared and actual size bounds.
pub fn verify_component_reader<R: Read>(
    mut reader: R,
    component: &BackupComponent,
) -> io::Result<()> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > component.bytes || total > MAX_COMPONENT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "component exceeds declared size",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    let digest = format!("{:x}", hasher.finalize());
    if total != component.bytes || !digest.eq_ignore_ascii_case(&component.sha256) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "component size or SHA-256 mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> BackupManifest {
        let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        BackupManifest {
            format: BACKUP_FORMAT.into(),
            schema_version: BACKUP_SCHEMA_VERSION,
            app_version: "0.1.8".into(),
            created_at: "2026-10-25T12:00:00Z".into(),
            snapshot_id: "00000000-0000-4000-8000-000000000001".into(),
            components: [
                ("library", "data/library.sqlite", "sqlite", 10),
                ("history", "data/playback-history.sqlite", "sqlite", 1),
                ("settings", "data/settings.json", "json", 1),
                ("playback", "data/playback.json", "json", 1),
            ]
            .into_iter()
            .map(|(id, path, encoding, schema_version)| BackupComponent {
                id: id.into(),
                path: path.into(),
                encoding: encoding.into(),
                bytes: 0,
                sha256: empty_hash.into(),
                schema_version: Some(schema_version),
            })
            .collect(),
            media: BackupMediaSummary {
                included: false,
                assets: 0,
                missing: 0,
                changed: 0,
                omitted: 0,
            },
            auxiliary_assets: vec![],
        }
    }

    #[test]
    fn parses_versioned_manifest_fixture() {
        let manifest = fixture();
        let encoded = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(BackupManifest::parse(&encoded).unwrap(), manifest);
        let fixture_bytes = include_bytes!("../tests/fixtures/backup-v1-manifest.json");
        let parsed_fixture = BackupManifest::parse(fixture_bytes).unwrap();
        assert_eq!(parsed_fixture.schema_version, 1);
    }

    #[test]
    fn rejects_unsafe_archive_paths() {
        for path in [
            "../escape",
            "data/../../escape",
            "/absolute",
            "C:/drive",
            "data\\file",
        ] {
            assert!(matches!(
                validate_archive_path(path),
                Err(ManifestError::UnsafePath(_))
            ));
        }
        assert!(validate_archive_path("data/library.sqlite").is_ok());
    }

    #[test]
    fn rejects_future_schema_and_duplicate_components() {
        let mut manifest = fixture();
        manifest.schema_version = BACKUP_SCHEMA_VERSION + 1;
        assert_eq!(
            manifest.validate(),
            Err(ManifestError::UnsupportedSchema(BACKUP_SCHEMA_VERSION + 1))
        );
        let mut manifest = fixture();
        manifest.components.push(manifest.components[0].clone());
        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::DuplicateComponent(_))
        ));
    }

    #[test]
    fn version_two_manifest_round_trips_a_hash_bound_lyrics_attachment() {
        let asset_id = "asset-1";
        let audio_sha256 = format!("{:x}", Sha256::digest(b"audio"));
        let lyrics_sha256 = format!("{:x}", Sha256::digest(b"lyrics"));
        let mut manifest = fixture();
        manifest.media = BackupMediaSummary {
            included: true,
            assets: 1,
            missing: 0,
            changed: 0,
            omitted: 0,
        };
        manifest.components.push(BackupComponent {
            id: format!("media:{asset_id}"),
            path: format!("media/assets/{asset_id}"),
            encoding: "binary".into(),
            bytes: 5,
            sha256: audio_sha256.clone(),
            schema_version: None,
        });
        let attachment = AuxiliaryAttachment {
            schema_version: 1,
            kind: AuxiliaryAssetKind::LyricsLrc,
            asset_id: asset_id.into(),
            audio_sha256,
            component_id: auxiliary_component_id(asset_id),
        };
        manifest.components.push(BackupComponent {
            id: attachment.component_id.clone(),
            path: auxiliary_component_path(asset_id),
            encoding: "binary".into(),
            bytes: 6,
            sha256: lyrics_sha256,
            schema_version: None,
        });
        manifest.auxiliary_assets.push(attachment);

        let encoded = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(BackupManifest::parse(&encoded).unwrap(), manifest);
        manifest.auxiliary_assets[0].audio_sha256 = "0".repeat(64);
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn verifies_component_digest_and_declared_size() {
        let component = BackupComponent {
            id: "settings".into(),
            path: "data/settings.json".into(),
            encoding: "json".into(),
            bytes: 2,
            sha256: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a".into(),
            schema_version: Some(1),
        };
        assert!(verify_component_reader(&b"{}"[..], &component).is_ok());
        assert!(verify_component_reader(&b"{ }"[..], &component).is_err());
    }
}
