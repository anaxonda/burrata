//! Local-only lyric reads scoped to the selected, registered audio asset.
use crate::models::LocalAsset;
use lofty::{file::TaggedFileExt, probe::Probe, tag::ItemKey};
use std::{io::Read, path::Path};
const MAX_BYTES: usize = 64 * 1024;

pub(crate) struct LocalLyrics {
    pub text: String,
    pub sidecar: bool,
}

fn regular_canonical(path: &Path) -> bool {
    path.is_absolute()
        && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
        && std::fs::canonicalize(path).is_ok_and(|canonical| canonical == path)
}

fn sidecar(audio: &Path) -> Option<String> {
    let path = audio.with_extension("lrc");
    if !regular_canonical(&path) {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let text = text.trim_start_matches('\u{feff}');
    (!text.trim().is_empty() && !text.contains('\0')).then(|| text.to_owned())
}

pub(crate) async fn read(asset: &LocalAsset) -> Option<LocalLyrics> {
    if !regular_canonical(Path::new(&asset.path)) {
        return None;
    }
    crate::media::verify_asset(asset).await.ok()?;
    let path = asset.path.clone();
    tokio::task::spawn_blocking(move || {
        let audio = Path::new(&path);
        if let Some(text) = sidecar(audio) {
            return Some(LocalLyrics {
                text,
                sidecar: true,
            });
        }
        let tagged = Probe::open(audio)
            .ok()?
            .guess_file_type()
            .ok()?
            .read()
            .ok()?;
        let text = tagged
            .tags()
            .iter()
            .find_map(|tag| tag.get_string(&ItemKey::Lyrics))?;
        if text.trim().is_empty() || text.len() > MAX_BYTES || text.contains('\0') {
            return None;
        }
        Some(LocalLyrics {
            text: text.to_owned(),
            sidecar: false,
        })
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Explicit fixture setup, never run by the ordinary test suite.
    #[test]
    #[ignore = "prepares a marked disposable Linux runtime profile; requires explicit directory"]
    fn prepare_local_lyrics_runtime_fixture() {
        use crate::models::{LocalAsset, Settings, Track};
        use lofty::{
            config::WriteOptions,
            file::AudioFile,
            tag::{Tag, TagType},
        };
        let root = std::path::PathBuf::from(
            std::env::var("BURRATA_LYRICS_RUNTIME_PROFILE").expect("explicit fixture root"),
        )
        .canonicalize()
        .unwrap();
        assert_eq!(root.parent(), Some(Path::new("/tmp")));
        assert!(root.join(".burrata-isolated-profile").is_file());
        let data = root.join("data/org.burrata.discovery");
        std::fs::create_dir_all(&data).unwrap();
        assert!(
            !data.join("burrata.sqlite").exists(),
            "Only prepare a new disposable profile"
        );
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        let store = crate::Store::open(&data.join("burrata.sqlite")).unwrap();
        let mut settings = Settings::default();
        settings.music_directory = assets.to_string_lossy().into_owned();
        store.save_settings(&settings).unwrap();
        store
            .set_json_preference("releaseSetupComplete.v1", &serde_json::json!(true))
            .unwrap();
        store
            .set_json_preference("radioRecognitionEnabled.v1", &serde_json::json!(false))
            .unwrap();
        store
            .set_json_preference(
                "autotag.settings",
                &serde_json::json!({"enabled":false,"shazamFallback":false}),
            )
            .unwrap();
        store
            .set_json_preference(
                "automation.permissions.v1",
                &serde_json::json!({"read":true,"control":true,"manage":false}),
            )
            .unwrap();
        let mut records = Vec::new();
        for (kind, title) in [
            ("sidecar", "Local LRC fixture"),
            ("embedded", "Local embedded fixture"),
        ] {
            let path = assets.join(format!("{kind}.flac"));
            assert!(std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=44100:cl=mono",
                    "-t",
                    "12"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success());
            if kind == "sidecar" {
                std::fs::write(path.with_extension("lrc"), "[ar:Synthetic Choir]\n[ti:Local LRC fixture]\n[offset:+500]\n[00:00.00] Synthetic first line\n[00:06.00] Synthetic second line\n").unwrap();
            } else {
                let mut tagged = Probe::open(&path).unwrap().read().unwrap();
                let mut tag = Tag::new(TagType::VorbisComments);
                tag.insert_text(
                    ItemKey::Lyrics,
                    "Synthetic embedded paragraph one\n\nSynthetic embedded paragraph two".into(),
                );
                tagged.insert_tag(tag);
                tagged.save_to_path(&path, WriteOptions::default()).unwrap();
            }
            let id = format!("local:lyrics-{kind}");
            let asset_id = format!("lyrics-runtime-{kind}");
            let track = Track {
                id: id.clone(),
                provider: "local".into(),
                provider_id: format!("lyrics-{kind}"),
                source_url: path.to_string_lossy().into_owned(),
                artist: "Synthetic Choir".into(),
                title: title.into(),
                album: Some("Runtime validation".into()),
                duration_seconds: Some(12.0),
                local_path: Some(path.to_string_lossy().into_owned()),
                local_availability: "available".into(),
                asset_id: Some(asset_id.clone()),
                ..Track::default()
            };
            let asset = LocalAsset {
                id: asset_id,
                recording_id: id.clone(),
                path: path.to_string_lossy().into_owned(),
                hash: crate::media::hash_file(&path).unwrap(),
                source_codec: "flac".into(),
                final_codec: "flac".into(),
                source_bitrate: None,
                final_bitrate: None,
                sample_rate: Some(44100),
                channels: Some(1),
                bit_depth: Some(16),
                duration_seconds: Some(12.0),
                size_bytes: Some(path.metadata().unwrap().len()),
                bitrate_origin: None,
                processing: "external-readonly".into(),
                track,
            };
            store.save_asset(&asset).unwrap();
            records.push(serde_json::json!({"trackId":id,"kind":kind}));
        }
        if let Ok(import) = std::env::var("BURRATA_LYRICS_RUNTIME_IMPORT_ASSET") {
            let mut asset: LocalAsset =
                serde_json::from_slice(&std::fs::read(import).unwrap()).unwrap();
            let path = Path::new(&asset.path);
            assert!(path.is_absolute() && path.is_file());
            assert_eq!(crate::media::hash_file(path).unwrap(), asset.hash);
            asset.processing = "external-readonly".into();
            store.save_asset(&asset).unwrap();
            records.push(serde_json::json!({"trackId":asset.track.id,"kind":"imported-readonly","hash":asset.hash}));
        }
        std::fs::write(
            root.join("lyrics-runtime-fixtures.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn sidecars_are_exact_name_bounded_utf8_and_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let audio = dir.join("selected.flac");
        std::fs::write(dir.join("other.lrc"), "[00:01] Wrong file").unwrap();
        assert!(sidecar(&audio).is_none());
        std::fs::write(
            audio.with_extension("lrc"),
            "\u{feff}[00:01] Synthetic line",
        )
        .unwrap();
        assert_eq!(sidecar(&audio).as_deref(), Some("[00:01] Synthetic line"));
        std::fs::write(audio.with_extension("lrc"), vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert!(sidecar(&audio).is_none());
        std::fs::write(audio.with_extension("lrc"), [0xff, 0xfe]).unwrap();
        assert!(sidecar(&audio).is_none());
    }
    #[tokio::test]
    async fn reads_embedded_lyrics_from_a_real_tagged_audio_file() {
        use lofty::{
            config::WriteOptions,
            file::AudioFile,
            tag::{Tag, TagType},
        };
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("synthetic.flac");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=mono",
                "-t",
                "0.1",
            ])
            .arg(&path)
            .status()
            .expect("FFmpeg fixture dependency");
        assert!(status.success());
        let mut tagged = Probe::open(&path).unwrap().read().unwrap();
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.insert_text(
            ItemKey::Lyrics,
            "Synthetic paragraph one\n\nSynthetic paragraph two".into(),
        );
        tagged.insert_tag(tag);
        tagged.save_to_path(&path, WriteOptions::default()).unwrap();
        let asset = LocalAsset {
            id: "fixture".into(),
            recording_id: "local:fixture".into(),
            path: path.to_string_lossy().into_owned(),
            hash: format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
            source_codec: "flac".into(),
            final_codec: "flac".into(),
            source_bitrate: None,
            final_bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: Some(0.1),
            size_bytes: None,
            bitrate_origin: None,
            processing: "fixture".into(),
            track: crate::models::Track::default(),
        };
        let lyrics = read(&asset).await.unwrap();
        assert!(!lyrics.sidecar);
        assert_eq!(
            lyrics.text,
            "Synthetic paragraph one\n\nSynthetic paragraph two"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        std::fs::write(dir.join("elsewhere"), "[00:01] Synthetic line").unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("selected.lrc")).unwrap();
        assert!(sidecar(&dir.join("selected.flac")).is_none());
    }
}
