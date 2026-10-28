//! Versioned, path-free descriptors for user-owned files associated with
//! registered media. The first supported attachment is an exact sibling LRC.

use crate::{
    backup::{BackupComponent, ManifestError, MAX_LYRICS_SIDECAR_BYTES},
    models::LocalAsset,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct AuxiliaryAttachment {
    pub schema_version: u32,
    pub kind: AuxiliaryAssetKind,
    pub asset_id: String,
    pub audio_sha256: String,
    pub component_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuxiliaryAssetKind {
    LyricsLrc,
}

#[derive(Debug, Clone)]
pub struct AuxiliaryInput {
    pub attachment: AuxiliaryAttachment,
    pub source_path: PathBuf,
    pub expected_sha256: String,
}

#[derive(Debug, Clone)]
pub enum AuxiliaryDiscovery {
    Missing,
    Invalid,
    Included(AuxiliaryInput),
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn auxiliary_component_id(asset_id: &str) -> String {
    format!("aux-lyrics:{:x}", Sha256::digest(asset_id.as_bytes()))
}

pub fn auxiliary_component_path(asset_id: &str) -> String {
    format!("aux/lyrics/{:x}.lrc", Sha256::digest(asset_id.as_bytes()))
}

/// Validate the closed auxiliary attachment contract and its relation to the
/// exact included media component. Archive paths are derived, never accepted
/// from descriptor content.
pub fn validate_auxiliary_attachments(
    attachments: &[AuxiliaryAttachment],
    components: &[BackupComponent],
) -> Result<(), ManifestError> {
    if attachments.len() > crate::backup::MAX_AUXILIARY_ATTACHMENTS {
        return Err(ManifestError::TooManyComponents);
    }
    let by_id = components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let mut referenced = HashSet::new();
    for attachment in attachments {
        if attachment.schema_version != 1
            || attachment.asset_id.is_empty()
            || attachment.asset_id.len() > 512
            || attachment.asset_id.chars().any(char::is_control)
            || !valid_sha256(&attachment.audio_sha256)
            || attachment.component_id != auxiliary_component_id(&attachment.asset_id)
            || !referenced.insert(attachment.component_id.as_str())
        {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        }
        let media_id = format!("media:{}", attachment.asset_id);
        let Some(media) = by_id.get(media_id.as_str()).copied() else {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        };
        if !media.sha256.eq_ignore_ascii_case(&attachment.audio_sha256) {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        }
        let Some(auxiliary) = by_id.get(attachment.component_id.as_str()).copied() else {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        };
        if auxiliary.path != auxiliary_component_path(&attachment.asset_id)
            || auxiliary.bytes == 0
            || auxiliary.bytes > MAX_LYRICS_SIDECAR_BYTES
            || !valid_sha256(&auxiliary.sha256)
        {
            return Err(ManifestError::InvalidField("auxiliaryAssets"));
        }
    }
    let declared = components
        .iter()
        .filter(|component| component.id.starts_with("aux-lyrics:"))
        .map(|component| component.id.as_str())
        .collect::<HashSet<_>>();
    if declared.len() != attachments.len() || declared != referenced {
        return Err(ManifestError::InvalidField("auxiliaryAssets"));
    }
    Ok(())
}

/// Discover only the exact sibling LRC file supported by local lyrics lookup.
/// The caller supplies a registered audio hash it already verified; the
/// archive writer must recheck `expected_sha256` while copying the candidate.
pub fn snapshot_lyrics_sidecar(
    asset: &LocalAsset,
    verified_audio_sha256: &str,
) -> AuxiliaryDiscovery {
    let audio_path = Path::new(&asset.path);
    if !valid_sha256(verified_audio_sha256)
        || !asset.hash.eq_ignore_ascii_case(verified_audio_sha256)
        || !audio_path.is_absolute()
        || !fs::symlink_metadata(audio_path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        || !fs::canonicalize(audio_path).is_ok_and(|canonical| canonical == audio_path)
    {
        return AuxiliaryDiscovery::Invalid;
    }
    let sidecar_path = audio_path.with_extension("lrc");
    let metadata = match fs::symlink_metadata(&sidecar_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AuxiliaryDiscovery::Missing;
        }
        _ => return AuxiliaryDiscovery::Invalid,
    };
    if metadata.len() == 0 || metadata.len() > MAX_LYRICS_SIDECAR_BYTES {
        return AuxiliaryDiscovery::Invalid;
    }
    if !fs::canonicalize(&sidecar_path).is_ok_and(|canonical| canonical == sidecar_path) {
        return AuxiliaryDiscovery::Invalid;
    }
    let Ok(mut file) = File::open(&sidecar_path) else {
        return AuxiliaryDiscovery::Invalid;
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .by_ref()
        .take(MAX_LYRICS_SIDECAR_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return AuxiliaryDiscovery::Invalid;
    }
    if bytes.len() as u64 != metadata.len()
        || bytes.len() as u64 > MAX_LYRICS_SIDECAR_BYTES
        || !std::str::from_utf8(&bytes).is_ok_and(|text| {
            let text = text.trim_start_matches('\u{feff}');
            !text.trim().is_empty() && !text.contains('\0')
        })
    {
        return AuxiliaryDiscovery::Invalid;
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    AuxiliaryDiscovery::Included(AuxiliaryInput {
        attachment: AuxiliaryAttachment {
            schema_version: 1,
            kind: AuxiliaryAssetKind::LyricsLrc,
            asset_id: asset.id.clone(),
            audio_sha256: verified_audio_sha256.to_ascii_lowercase(),
            component_id: auxiliary_component_id(&asset.id),
        },
        source_path: sidecar_path,
        expected_sha256: digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_only_a_valid_exact_sibling_bound_to_registered_audio_hash() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path().canonicalize().unwrap();
        let audio_path = directory.join("track.flac");
        let sidecar_path = directory.join("track.lrc");
        fs::write(&audio_path, b"audio fixture").unwrap();
        fs::write(&sidecar_path, "[00:01.00] lyric line").unwrap();
        let audio_hash = format!("{:x}", Sha256::digest(b"audio fixture"));
        let asset = LocalAsset {
            id: "asset:id/with unsafe path chars".into(),
            recording_id: "recording:fixture".into(),
            path: audio_path.to_string_lossy().into_owned(),
            hash: audio_hash.clone(),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: crate::models::Track::default(),
        };

        let AuxiliaryDiscovery::Included(input) = snapshot_lyrics_sidecar(&asset, &audio_hash)
        else {
            panic!("expected exact sibling LRC to be included");
        };
        assert_eq!(input.source_path, sidecar_path);
        assert_eq!(input.attachment.audio_sha256, audio_hash);
        assert_eq!(
            input.attachment.component_id,
            auxiliary_component_id(&asset.id)
        );
        assert!(!auxiliary_component_path(&asset.id).contains("asset:id"));
        assert_eq!(
            input.expected_sha256,
            format!("{:x}", Sha256::digest(b"[00:01.00] lyric line"))
        );

        assert!(matches!(
            snapshot_lyrics_sidecar(&asset, &"0".repeat(64)),
            AuxiliaryDiscovery::Invalid
        ));
        fs::write(directory.join("other.lrc"), "[00:01] unrelated").unwrap();
        fs::remove_file(&sidecar_path).unwrap();
        assert!(matches!(
            snapshot_lyrics_sidecar(&asset, &audio_hash),
            AuxiliaryDiscovery::Missing
        ));
    }

    #[test]
    fn auxiliary_validation_requires_exact_media_hash_and_deterministic_component() {
        let asset_id = "asset:id/with unsafe path chars";
        let audio_hash = format!("{:x}", Sha256::digest(b"audio"));
        let lyrics_hash = format!("{:x}", Sha256::digest(b"lrc"));
        let attachment = AuxiliaryAttachment {
            schema_version: 1,
            kind: AuxiliaryAssetKind::LyricsLrc,
            asset_id: asset_id.into(),
            audio_sha256: audio_hash.clone(),
            component_id: auxiliary_component_id(asset_id),
        };
        let components = vec![
            BackupComponent {
                id: format!("media:{asset_id}"),
                path: format!("media/assets/{asset_id}"),
                encoding: "binary".into(),
                bytes: 5,
                sha256: audio_hash,
                schema_version: None,
            },
            BackupComponent {
                id: attachment.component_id.clone(),
                path: auxiliary_component_path(asset_id),
                encoding: "binary".into(),
                bytes: 3,
                sha256: lyrics_hash,
                schema_version: None,
            },
        ];
        assert!(validate_auxiliary_attachments(&[attachment.clone()], &components).is_ok());
        let mut mismatched = attachment;
        mismatched.audio_sha256 = "0".repeat(64);
        assert!(validate_auxiliary_attachments(&[mismatched], &components).is_err());
    }
}
