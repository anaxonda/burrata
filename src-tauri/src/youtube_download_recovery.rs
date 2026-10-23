//! Bounded retries of a failed YouTube download using freshly inspected audio
//! formats from the same exact video. This path never searches for a new source.

use crate::models::{AudioFormat, Candidate, DownloadJob};
use std::{future::Future, pin::Pin};
use tokio::sync::watch;

const MAX_ALTERNATE_FORMATS: usize = 2;

pub(crate) type RecoveryFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) async fn download_with_403_recovery<'a, Refresh, Clear, Attempt>(
    candidate: &'a Candidate,
    selected_format_id: Option<&'a str>,
    job: &mut DownloadJob,
    initial_cancel: watch::Receiver<bool>,
    mut refresh: Refresh,
    mut clear_staging: Clear,
    mut attempt: Attempt,
) -> Result<(String, Option<String>), String>
where
    Refresh: FnMut(watch::Receiver<bool>) -> RecoveryFuture<'a, Result<Candidate, String>>,
    Clear: FnMut() -> RecoveryFuture<'a, Result<(), String>>,
    Attempt: for<'b> FnMut(
            &'b mut DownloadJob,
            Option<String>,
            watch::Receiver<bool>,
        ) -> RecoveryFuture<'b, Result<String, String>>
        + 'a,
{
    let initial = attempt(
        job,
        selected_format_id.map(str::to_owned),
        initial_cancel.clone(),
    )
    .await;
    let first_error = match initial {
        Ok(output) => return Ok((output, selected_format_id.map(str::to_owned))),
        Err(error) => error,
    };
    if selected_format_id.is_some()
        || candidate_video_id(candidate).is_none()
        || !is_http_403(&first_error)
    {
        return Err(first_error);
    }
    if *initial_cancel.borrow() {
        clear_staging().await?;
        return Err("Cancelled".into());
    }

    let fresh = match refresh(initial_cancel.clone()).await {
        Ok(fresh) => fresh,
        Err(error) => {
            clear_staging().await?;
            return Err(error);
        }
    };
    if fresh.permission == "restricted" {
        clear_staging().await?;
        return Err("Fresh download inspection reports restricted access".into());
    }
    if candidate_video_id(&fresh) != candidate_video_id(candidate) {
        clear_staging().await?;
        return Err("Fresh download inspection changed the selected YouTube video".into());
    }
    if *initial_cancel.borrow() {
        clear_staging().await?;
        return Err("Cancelled".into());
    }

    let mut last_error = first_error;
    let formats = alternate_audio_formats(&fresh.formats);
    for format_id in formats {
        if *initial_cancel.borrow() {
            clear_staging().await?;
            return Err("Cancelled".into());
        }
        clear_staging().await?;
        let attempted_format_id = format_id.clone();
        match attempt(job, Some(format_id), initial_cancel.clone()).await {
            Ok(output) => return Ok((output, Some(attempted_format_id))),
            Err(error) if error == "Cancelled" => {
                clear_staging().await?;
                return Err(error);
            }
            Err(error) if is_http_403(&error) => last_error = error,
            Err(error) => {
                clear_staging().await?;
                return Err(error);
            }
        }
    }
    clear_staging().await?;
    Err(last_error)
}

fn candidate_video_id(candidate: &Candidate) -> Option<&str> {
    if candidate.provider != "youtube" {
        return None;
    }
    let expected_id = candidate.id.strip_prefix("youtube:")?;
    if expected_id.is_empty() {
        return None;
    }
    let url = url::Url::parse(&candidate.url).ok()?;
    if !matches!(
        url.host_str()?,
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    ) {
        return None;
    }
    let url_id = if url.path() == "/watch" {
        url.query_pairs()
            .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))?
    } else {
        return None;
    };
    (url_id == expected_id).then_some(expected_id)
}

fn alternate_audio_formats(formats: &[AudioFormat]) -> Vec<String> {
    let mut formats = formats
        .iter()
        .filter(|format| safe_format_id(&format.id) && is_audio_format(format))
        .collect::<Vec<_>>();
    formats.sort_by(|left, right| {
        right
            .bitrate
            .unwrap_or_default()
            .total_cmp(&left.bitrate.unwrap_or_default())
            .then_with(|| right.sample_rate.cmp(&left.sample_rate))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut ids = Vec::new();
    for format in formats {
        if !ids.contains(&format.id) {
            ids.push(format.id.clone());
        }
        if ids.len() == MAX_ALTERNATE_FORMATS {
            break;
        }
    }
    ids
}

fn safe_format_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_audio_format(format: &AudioFormat) -> bool {
    let codec = format.codec.to_ascii_lowercase();
    let extension = format.extension.to_ascii_lowercase();
    [
        "aac", "mp4a", "opus", "vorbis", "mp3", "flac", "alac", "ac3", "eac3", "pcm", "dts",
    ]
    .iter()
    .any(|prefix| codec.starts_with(prefix))
        && matches!(
            extension.as_str(),
            "aac" | "flac" | "m4a" | "mp3" | "oga" | "ogg" | "opus" | "wav" | "webm"
        )
}

fn is_http_403(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("http error 403")
        || error.contains("http 403")
        || error.contains("403 forbidden")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn candidate() -> Candidate {
        Candidate {
            id: "youtube:WthU1IjzZxo".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=WthU1IjzZxo".into(),
            title: "LA LUNA MORENA SON TEKAMELI".into(),
            formats: vec![],
            ..Candidate::default()
        }
    }

    fn format(id: &str, codec: &str, extension: &str, bitrate: f64) -> AudioFormat {
        AudioFormat {
            id: id.into(),
            codec: codec.into(),
            extension: extension.into(),
            bitrate: Some(bitrate),
            sample_rate: Some(44_100),
            lossless: false,
            ..AudioFormat::default()
        }
    }

    fn job(candidate: Candidate) -> DownloadJob {
        DownloadJob {
            id: "download-1".into(),
            track: crate::models::Track::default(),
            candidate,
            state: "downloading".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            disposition: Default::default(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
            download_layout: Default::default(),
            playlist_context: None,
        }
    }

    fn boxed<T: Send + 'static>(
        future: impl Future<Output = T> + Send + 'static,
    ) -> RecoveryFuture<'static, T> {
        Box::pin(future)
    }

    #[tokio::test]
    async fn retries_same_video_with_fresh_alternate_format_after_403() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let fresh = Candidate {
            formats: vec![
                format("139", "mp4a.40.5", "m4a", 48.8),
                format("140", "mp4a.40.2", "m4a", 129.5),
                format("251", "opus", "webm", 114.8),
                format("18", "avc1.64001F", "mp4", 1000.0),
            ],
            ..original.clone()
        };
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let cleanups = Arc::new(Mutex::new(0usize));
        let refresh_count = Arc::new(Mutex::new(0usize));
        let attempts_for_closure = attempted.clone();
        let cleanup_count = cleanups.clone();
        let refresh_counter = refresh_count.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let fresh = fresh.clone();
                let count = refresh_counter.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(fresh)
                })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts_for_closure.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id.clone());
                    if format_id.is_none() {
                        Err("yt-dlp failed: HTTP Error 403: Forbidden".into())
                    } else {
                        Ok(format!("selected format {}", format_id.unwrap()))
                    }
                })
            },
        )
        .await;
        assert_eq!(
            result.unwrap(),
            ("selected format 140".into(), Some("140".into()))
        );
        assert_eq!(*attempted.lock().unwrap(), vec![None, Some("140".into())]);
        assert_eq!(*refresh_count.lock().unwrap(), 1);
        assert_eq!(*cleanups.lock().unwrap(), 1);
        assert_eq!(download_job.format_id, None);
    }

    #[tokio::test]
    async fn exhausts_bounded_same_video_alternates_without_switching_source() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let fresh = Candidate {
            formats: vec![
                format("140", "mp4a.40.2", "m4a", 129.5),
                format("251", "opus", "webm", 114.8),
                format("139", "mp4a.40.5", "m4a", 48.8),
            ],
            ..original.clone()
        };
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let cleanups = Arc::new(Mutex::new(0usize));
        let attempts_for_closure = attempted.clone();
        let cleanup_count = cleanups.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let fresh = fresh.clone();
                boxed(async move { Ok(fresh) })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts_for_closure.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id);
                    Err("HTTP 403 Forbidden".into())
                })
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "HTTP 403 Forbidden");
        assert_eq!(
            *attempted.lock().unwrap(),
            vec![None, Some("140".into()), Some("251".into())]
        );
        assert_eq!(*cleanups.lock().unwrap(), 3);
        assert!(attempted
            .lock()
            .unwrap()
            .iter()
            .all(|format_id| format_id.is_none()
                || *format_id == Some("140".into())
                || *format_id == Some("251".into())));
    }

    #[tokio::test]
    async fn explicit_format_selection_is_never_overridden_after_403() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let attempts_for_closure = attempted.clone();
        let refresh_count = Arc::new(Mutex::new(0usize));
        let cleanups = Arc::new(Mutex::new(0usize));
        let refresh_counter = refresh_count.clone();
        let cleanup_count = cleanups.clone();
        let result = download_with_403_recovery(
            &original,
            Some("251"),
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let count = refresh_counter.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(candidate())
                })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts_for_closure.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id);
                    Err("HTTP Error 403: Forbidden".into())
                })
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "HTTP Error 403: Forbidden");
        assert_eq!(*attempted.lock().unwrap(), vec![Some("251".into())]);
        assert_eq!(*refresh_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn cancellation_after_first_403_prevents_refresh_and_alternate_attempt() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let attempts_for_closure = attempted.clone();
        let refresh_count = Arc::new(Mutex::new(0usize));
        let refresh_counter = refresh_count.clone();
        let cleanups = Arc::new(Mutex::new(0usize));
        let cleanup_count = cleanups.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let count = refresh_counter.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(candidate())
                })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts_for_closure.clone();
                let cancel_tx = cancel_tx.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id);
                    cancel_tx.send(true).unwrap();
                    Err("HTTP Error 403: Forbidden".into())
                })
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert_eq!(*attempted.lock().unwrap(), vec![None]);
        assert_eq!(*refresh_count.lock().unwrap(), 0);
        assert_eq!(*cleanups.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn alternate_attempt_cancellation_cleans_partial_staging() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let fresh = Candidate {
            formats: vec![format("140", "mp4a.40.2", "m4a", 129.5)],
            ..original.clone()
        };
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let cleanups = Arc::new(Mutex::new(0usize));
        let cleanup_count = cleanups.clone();
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let attempts = attempted.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let fresh = fresh.clone();
                boxed(async move { Ok(fresh) })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id.clone());
                    if format_id.is_none() {
                        Err("HTTP Error 403: Forbidden".into())
                    } else {
                        Err("Cancelled".into())
                    }
                })
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert_eq!(*attempted.lock().unwrap(), vec![None, Some("140".into())]);
        assert_eq!(*cleanups.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn refreshed_candidate_must_be_the_same_video() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let mut other = candidate();
        other.id = "youtube:different".into();
        other.url = "https://www.youtube.com/watch?v=different".into();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let attempts_for_closure = attempted.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let other = other.clone();
                boxed(async move { Ok(other) })
            },
            || boxed(async { Ok(()) }),
            move |_job, format_id, _cancel| {
                let attempts = attempts_for_closure.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id);
                    Err("HTTP Error 403: Forbidden".into())
                })
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Fresh download inspection changed the selected YouTube video"
        );
        assert_eq!(*attempted.lock().unwrap(), vec![None]);
    }

    #[tokio::test]
    async fn newly_restricted_video_does_not_start_an_alternate_download() {
        let original = candidate();
        let mut download_job = job(original.clone());
        let fresh = Candidate {
            permission: "restricted".into(),
            formats: vec![format("140", "mp4a.40.2", "m4a", 129.5)],
            ..original.clone()
        };
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let attempted = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let attempts = attempted.clone();
        let cleanups = Arc::new(Mutex::new(0usize));
        let cleanup_count = cleanups.clone();
        let result = download_with_403_recovery(
            &original,
            None,
            &mut download_job,
            cancel_rx,
            move |_cancel| {
                let fresh = fresh.clone();
                boxed(async move { Ok(fresh) })
            },
            move || {
                let count = cleanup_count.clone();
                boxed(async move {
                    *count.lock().unwrap() += 1;
                    Ok(())
                })
            },
            move |_job, format_id, _cancel| {
                let attempts = attempts.clone();
                boxed(async move {
                    attempts.lock().unwrap().push(format_id);
                    Err("HTTP Error 403: Forbidden".into())
                })
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Fresh download inspection reports restricted access"
        );
        assert_eq!(*attempted.lock().unwrap(), vec![None]);
        assert_eq!(*cleanups.lock().unwrap(), 1);
    }

    #[test]
    fn invalid_url_provider_and_video_formats_cannot_enter_same_video_recovery() {
        let mut source = candidate();
        assert_eq!(candidate_video_id(&source), Some("WthU1IjzZxo"));
        source.url = "https://youtube.com/watch?v=other".into();
        assert_eq!(candidate_video_id(&source), None);
        source.url = "https://example.invalid/watch?v=WthU1IjzZxo".into();
        assert_eq!(candidate_video_id(&source), None);
        assert_eq!(
            is_audio_format(&format("18", "avc1.64001F", "mp4", 1000.0)),
            false
        );
        assert!(safe_format_id("140"));
        assert!(!safe_format_id("140?token=private"));
    }
}
