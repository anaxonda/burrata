//! Owned staged-file recognition. No tags, assets or library membership are written.
use crate::{models::Track, recording_match::MatchLevel, Store};
use std::{future::Future, path::Path, time::Duration};
use tokio::sync::watch;

fn check(
    store: &Store,
    queued_consent: bool,
    level: MatchLevel,
    cancel: &watch::Receiver<bool>,
) -> Result<(), String> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err("Cancelled".into());
    }
    let settings = store.settings().map_err(|error| error.to_string())?;
    if !queued_consent || !settings.download_audio_verification_enabled {
        return Err("Audio verification is disabled or consent was revoked".into());
    }
    if settings.matching_level != level {
        return Err("Matching policy changed during audio verification".into());
    }
    Ok(())
}

async fn checked_wait<T>(
    store: &Store,
    queued: bool,
    level: MatchLevel,
    cancel: &watch::Receiver<bool>,
    future: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::pin!(future);
    let mut changed = cancel.clone();
    loop {
        check(store, queued, level, cancel)?;
        tokio::select! {
            biased;
            result = changed.changed() => {
                result.map_err(|_| "Cancelled".to_owned())?;
            }
            result = &mut future => {
                check(store, queued, level, cancel)?;
                return result;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
}

#[cfg(test)]
struct SyntheticRecognizer {
    callback: Box<
        dyn Fn(usize) -> Result<Option<crate::radio::RadioTrackMetadata>, String> + Send + Sync,
    >,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(test)]
tokio::task_local! { static SYNTHETIC_RECOGNIZER: SyntheticRecognizer; }

/// Test-only task-scoped backend: production builds cannot substitute responses.
#[cfg(test)]
pub(crate) async fn with_synthetic_recognition<F: Future>(
    callback: impl Fn(usize) -> Result<Option<crate::radio::RadioTrackMetadata>, String>
        + Send
        + Sync
        + 'static,
    future: F,
) -> (F::Output, usize) {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let result = SYNTHETIC_RECOGNIZER
        .scope(
            SyntheticRecognizer {
                callback: Box::new(callback),
                calls: calls.clone(),
            },
            future,
        )
        .await;
    (result, calls.load(std::sync::atomic::Ordering::SeqCst))
}

async fn recognize_excerpt(
    path: &str,
    start: f64,
    ffmpeg: &str,
    check: impl Fn() -> Result<(), String>,
) -> Result<Option<crate::radio::RadioTrackMetadata>, String> {
    check()?;
    #[cfg(test)]
    if let Ok(result) = SYNTHETIC_RECOGNIZER.try_with(|backend| {
        let index = backend
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (backend.callback)(index)
    }) {
        return result;
    }
    crate::autotag::songrec_excerpt_guarded(path, start, ffmpeg, check).await
}

fn excerpt_spacing() -> Duration {
    #[cfg(test)]
    if SYNTHETIC_RECOGNIZER.try_with(|_| ()).is_ok() {
        return Duration::ZERO;
    }
    Duration::from_secs(10)
}

pub(crate) struct StagedRecognition {
    binding: crate::recognition_input::RecognitionInputBinding,
    recognition: crate::autotag::AudioRecognition,
    duration: f64,
    level: MatchLevel,
}

impl StagedRecognition {
    /// Must run before the acquisition cancellation/publication boundary.
    pub(crate) async fn recheck(
        &self,
        store: &Store,
        requested: &Track,
        source: &Track,
        path: &Path,
        queued_consent: bool,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), String> {
        check(store, queued_consent, self.level, cancel)?;
        self.binding
            .recheck(requested, source, path, self.level)
            .await?;
        crate::download_verification::verify_recognized_recording(
            store,
            requested,
            &self.recognition,
            self.duration,
            self.level,
        )
        .ok_or("Recognized recording no longer meets verification policy")?;
        check(store, queued_consent, self.level, cancel)
    }
}

pub(crate) async fn verify_staged_file(
    store: &Store,
    requested: &Track,
    source: &Track,
    owned: &tempfile::TempDir,
    path: &Path,
    queued_consent: bool,
    level: MatchLevel,
    cancel: &watch::Receiver<bool>,
) -> Result<StagedRecognition, String> {
    check(store, queued_consent, level, cancel)?;
    let root = owned
        .path()
        .canonicalize()
        .map_err(|_| "Verification staging directory unavailable")?;
    let canonical = path
        .canonicalize()
        .map_err(|_| "Staged audio unavailable")?;
    if !canonical.starts_with(&root) || canonical == root {
        return Err("Verification input is outside its owned staging directory".into());
    }
    let path_text = path.to_str().ok_or("Verification path is not valid text")?;
    let settings = store.settings().map_err(|error| error.to_string())?;
    let operation = async {
        let binding = crate::recognition_input::RecognitionInputBinding::capture(
            requested, source, path, level, None,
        )
        .await?;
        let duration = crate::media::inspect_quality_facts(path, &settings)
            .await?
            .duration_seconds
            .filter(|duration| duration.is_finite() && (50.0..=86_400.0).contains(duration))
            .ok_or("Staged audio cannot provide two non-overlapping recognition excerpts")?;
        let first = recognize_excerpt(path_text, 5.0, &settings.ffmpeg_path, || {
            check(store, queued_consent, level, cancel)
        })
        .await?;
        if first.is_none() {
            return Err("First audio excerpt was not identified".into());
        }
        tokio::time::sleep(excerpt_spacing()).await;
        binding.recheck(requested, source, path, level).await?;
        let second = recognize_excerpt(path_text, duration - 25.0, &settings.ffmpeg_path, || {
            check(store, queued_consent, level, cancel)
        })
        .await?;
        let recognition = crate::autotag::consistent_songrec_excerpts(first, second)
            .ok_or("Audio excerpts did not identify one consistent recording")?;
        crate::download_verification::verify_recognized_recording(
            store,
            requested,
            &recognition,
            duration,
            level,
        )
        .ok_or("Recognized audio did not establish the requested recording")?;
        let result = StagedRecognition {
            binding,
            recognition,
            duration,
            level,
        };
        result
            .recheck(store, requested, source, path, queued_consent, cancel)
            .await?;
        Ok(result)
    };
    tokio::time::timeout(
        Duration::from_secs(180),
        checked_wait(store, queued_consent, level, cancel, Box::pin(operation)),
    )
    .await
    .map_err(|_| "Audio verification timed out".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("staging.sqlite")).unwrap();
        let mut settings = store.settings().unwrap();
        settings.download_audio_verification_enabled = true;
        store.save_settings(&settings).unwrap();
        (root, store)
    }
    #[tokio::test]
    async fn consent_revocation_and_cancellation_drop_inflight_work() {
        let (_root, store) = store();
        let (sender, cancel) = watch::channel(false);
        let revoked = async {
            let mut settings = store.settings().unwrap();
            settings.download_audio_verification_enabled = false;
            store.save_settings(&settings).unwrap();
            std::future::pending::<Result<(), String>>().await
        };
        assert!(tokio::time::timeout(
            Duration::from_secs(2),
            checked_wait(&store, true, MatchLevel::Balanced, &cancel, revoked)
        )
        .await
        .unwrap()
        .is_err());
        let mut settings = store.settings().unwrap();
        settings.download_audio_verification_enabled = true;
        store.save_settings(&settings).unwrap();
        sender.send(true).unwrap();
        assert!(
            checked_wait(&store, true, MatchLevel::Balanced, &cancel, async {
                panic!("cancelled work must not be polled");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn submission_guard_runs_after_extraction_before_any_recognition() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let extractor = root.path().join("extractor.sh");
        std::fs::write(
            &extractor,
            "#!/bin/sh\nfor output do :; done\nprintf synthetic > \"$output\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&extractor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let checks = AtomicUsize::new(0);
        let result = crate::autotag::songrec_excerpt_guarded(
            "synthetic-input",
            5.0,
            extractor.to_str().unwrap(),
            || {
                if checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err("synthetic consent revoked after extraction".into())
                }
            },
        )
        .await;
        assert_eq!(checks.load(Ordering::SeqCst), 2);
        assert_eq!(
            result.unwrap_err(),
            "synthetic consent revoked after extraction"
        );
    }

    #[tokio::test]
    async fn staged_entry_refuses_unowned_input_and_missing_queue_consent_before_extraction() {
        let (_root, store) = store();
        let owned = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = other.path().join("synthetic.flac");
        std::fs::write(&path, b"synthetic").unwrap();
        let (_sender, cancel) = watch::channel(false);
        let track = Track::default();
        assert!(verify_staged_file(
            &store,
            &track,
            &track,
            &owned,
            &path,
            true,
            MatchLevel::Balanced,
            &cancel
        )
        .await
        .is_err());
        assert!(verify_staged_file(
            &store,
            &track,
            &track,
            &owned,
            &path,
            false,
            MatchLevel::Balanced,
            &cancel
        )
        .await
        .is_err());
    }
}
