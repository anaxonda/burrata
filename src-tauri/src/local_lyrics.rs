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
