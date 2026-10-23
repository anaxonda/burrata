//! File and request coherence for audio recognition. A binding is not a
//! recording match or permission to publish an asset.
use crate::{models::Track, recording_match::MatchLevel};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tokio::sync::Semaphore;

pub(crate) struct RecognitionInputBinding {
    path: PathBuf,
    hash: String,
    request: String,
    source: String,
    level: MatchLevel,
}

fn source_fingerprint(track: &Track) -> String {
    let acquisition = &track.provenance["acquisition"];
    let source = &acquisition["source"];
    let raw_url = acquisition["url"].as_str().unwrap_or(&track.source_url);
    let url = url::Url::parse(raw_url)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| raw_url.to_owned());
    let locator = serde_json::json!({
        "provider":acquisition["provider"].as_str().unwrap_or(&track.provider),
        "id":acquisition["id"].as_str().unwrap_or(&track.provider_id),
        "url":url,
        "format":acquisition["resolvedFormatId"].as_str().or(acquisition["formatId"].as_str()),
        "peer":source["username"],"filename":source["filename"],
        "size":source["size_bytes"].as_u64().or(source["sizeBytes"].as_u64()),
    });
    format!("{:x}", Sha256::digest(locator.to_string().as_bytes()))
}

async fn file_snapshot(path: &Path) -> Result<(PathBuf, String), String> {
    static WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = WORKERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "Recognition input verification unavailable")?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        // Keep the permit inside the blocking operation even if its caller is
        // cancelled; detached filesystem work must not grow without a bound.
        let _permit = permit;
        if !path.is_absolute() {
            return Err("Recognition input must have an absolute path".into());
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| "Recognition input is unavailable")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Recognition input must be a regular non-symlink file".into());
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "Recognition input is unavailable")?;
        let hash = crate::media::hash_file(&canonical)
            .map_err(|_| "Could not verify recognition input bytes")?;
        Ok((canonical, hash))
    })
    .await
    .map_err(|_| "Recognition input verification failed".to_owned())?
}

impl RecognitionInputBinding {
    pub(crate) async fn capture(
        track: &Track,
        selected_source: &Track,
        path: &Path,
        level: MatchLevel,
        expected_hash: Option<&str>,
    ) -> Result<Self, String> {
        let request = crate::playback_sources::recognition_request_fingerprint(track, level)
            .ok_or("Recognition request identity is invalid")?;
        let source = source_fingerprint(selected_source);
        let (path, hash) = file_snapshot(path).await?;
        if expected_hash.is_some_and(|expected| expected != hash) {
            return Err(
                "Registered audio changed before recognition; review or reimport it first".into(),
            );
        }
        Ok(Self {
            path,
            hash,
            request,
            source,
            level,
        })
    }

    pub(crate) async fn recheck(
        &self,
        track: &Track,
        selected_source: &Track,
        path: &Path,
        level: MatchLevel,
    ) -> Result<(), String> {
        if self.level != level
            || Some(&self.request)
                != crate::playback_sources::recognition_request_fingerprint(track, level).as_ref()
            || self.source != source_fingerprint(selected_source)
        {
            return Err("Recording request or selected source changed during recognition".into());
        }
        let (path, hash) = file_snapshot(path).await?;
        if path != self.path || hash != self.hash {
            return Err(
                "Audio input changed during recognition; retry from the current file".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            id: "local:binding".into(),
            provider: "local".into(),
            provider_id: "binding".into(),
            source_url: "https://example.test/source".into(),
            artist: "Synthetic artist".into(),
            title: "Synthetic recording".into(),
            duration_seconds: Some(180.0),
            provenance: serde_json::json!({"acquisition":{"provider":"slskd","id":"slskd:source","source":{"username":"peer-a","filename":"Album\\Track.flac","size_bytes":20,"transfer_id":"first"}}}),
            ..Track::default()
        }
    }
    #[tokio::test]
    async fn binding_rejects_changed_bytes_or_another_path_with_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("audio.flac");
        std::fs::write(&path, b"synthetic source bytes").unwrap();
        let t = track();
        let bound = RecognitionInputBinding::capture(&t, &t, &path, MatchLevel::Balanced, None)
            .await
            .unwrap();
        bound
            .recheck(&t, &t, &path, MatchLevel::Balanced)
            .await
            .unwrap();
        let other = path.with_file_name("other.flac");
        std::fs::copy(&path, &other).unwrap();
        assert!(bound
            .recheck(&t, &t, &other, MatchLevel::Balanced)
            .await
            .is_err());
        std::fs::write(&path, b"different source bytes").unwrap();
        assert!(bound
            .recheck(&t, &t, &path, MatchLevel::Balanced)
            .await
            .is_err());
        assert!(RecognitionInputBinding::capture(
            &t,
            &t,
            &path,
            MatchLevel::Balanced,
            Some("stale-registered-hash")
        )
        .await
        .is_err());
    }
    #[tokio::test]
    async fn binding_preserves_source_identity_and_matching_scope_but_ignores_transfer_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("audio.flac");
        std::fs::write(&path, b"synthetic bytes").unwrap();
        let t = track();
        let bound = RecognitionInputBinding::capture(&t, &t, &path, MatchLevel::Balanced, None)
            .await
            .unwrap();
        let mut operational = t.clone();
        operational.provenance["acquisition"]["source"]["transfer_id"] =
            serde_json::json!("second");
        bound
            .recheck(&t, &operational, &path, MatchLevel::Balanced)
            .await
            .unwrap();
        assert!(bound
            .recheck(&t, &t, &path, MatchLevel::Exact)
            .await
            .is_err());
        let mut metadata = t.clone();
        metadata.title = "Another recording".into();
        assert!(bound
            .recheck(&metadata, &t, &path, MatchLevel::Balanced)
            .await
            .is_err());
        for key in ["username", "filename", "size_bytes"] {
            let mut changed = t.clone();
            changed.provenance["acquisition"]["source"][key] = if key == "size_bytes" {
                serde_json::json!(21)
            } else {
                serde_json::json!("changed")
            };
            assert!(
                bound
                    .recheck(&t, &changed, &path, MatchLevel::Balanced)
                    .await
                    .is_err(),
                "{key}"
            );
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn binding_refuses_a_symlinked_input() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("real.flac");
        std::fs::write(&path, b"synthetic bytes").unwrap();
        let link = path.with_file_name("link.flac");
        std::os::unix::fs::symlink(path, &link).unwrap();
        assert!(RecognitionInputBinding::capture(
            &track(),
            &track(),
            &link,
            MatchLevel::Balanced,
            None
        )
        .await
        .is_err());
    }
}
