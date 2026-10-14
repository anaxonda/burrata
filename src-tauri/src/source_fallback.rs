use crate::models::{Candidate, DownloadJob, Settings, Track};

pub fn provider_order(
    primary: &str,
    enabled: bool,
    configured: &[String],
    download: bool,
) -> Vec<String> {
    if !enabled {
        return vec![];
    }
    let allowed: &[&str] = if download {
        &["monochrome", "youtube", "soundcloud", "slskd"]
    } else {
        &["monochrome", "youtube", "soundcloud"]
    };
    let mut result = vec![];
    for id in configured {
        if id != primary && allowed.contains(&id.as_str()) && !result.contains(id) {
            result.push(id.clone());
        }
    }
    result
}
pub fn validate(settings: &Settings) -> Result<(), String> {
    for (sources, download) in [
        (&settings.streaming_fallback_sources, false),
        (&settings.download_fallback_sources, true),
    ] {
        let allowed: &[&str] = if download {
            &["monochrome", "youtube", "soundcloud", "slskd"]
        } else {
            &["monochrome", "youtube", "soundcloud"]
        };
        if sources.len() > allowed.len()
            || sources.iter().any(|s| !allowed.contains(&s.as_str()))
            || sources
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != sources.len()
        {
            return Err("Fallback sources must be supported and listed once".into());
        }
    }
    Ok(())
}
pub fn may_switch(job: &DownloadJob, error: &str, settings: &Settings, cancelled: bool) -> bool {
    settings.download_fallback_enabled
        && !cancelled
        && error != "Cancelled"
        && job.chapter_ids.as_ref().is_none_or(|ids| ids.is_empty())
        && ["downloading", "waiting_for_peer", "stalled"].contains(&job.state.as_str())
        && ![
            "Cannot create staging",
            "outside its staging",
            "Permission denied",
            "No space left",
            "Selected audio format",
        ]
        .iter()
        .any(|s| error.contains(s))
}
fn clean_title(raw: &str) -> String {
    let normalized = crate::matching::normalize(raw);
    for suffix in [
        " official audio",
        " official video",
        " official music video",
    ] {
        if let Some(title) = normalized.strip_suffix(suffix) {
            return title.trim().into();
        }
    }
    normalized
}
pub fn recording_matches(track: &Track, candidate: &Candidate) -> bool {
    if candidate.permission == "restricted" || candidate.confidence == "low" {
        return false;
    }
    if candidate.confidence == "high" {
        return true;
    }
    let Some((a, b)) = track.duration_seconds.zip(candidate.duration_seconds) else {
        return false;
    };
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 || (a - b).abs() > 5.0 {
        return false;
    }
    let artist = crate::matching::normalize(&track.artist);
    let title = clean_title(&track.title);
    let actual = clean_title(&candidate.title);
    if artist.is_empty() || title.is_empty() {
        return false;
    }
    let exact = actual == format!("{artist} {title}")
        || (actual == title
            && artist == crate::matching::normalize(candidate.uploader.as_deref().unwrap_or("")));
    let version = crate::matching::normalize_version(track.version.as_deref().unwrap_or(""));
    let version_ok = version.is_empty()
        || matches!(version.as_str(), "original" | "original mix")
        || format!(" {actual} ").contains(&format!(" {version} "));
    exact
        && version_ok
        && !candidate
            .reasons
            .iter()
            .any(|r| r.contains("version conflicts"))
}
pub fn requires_lossless(job: &DownloadJob) -> bool {
    job.output_profile != "mp3"
        && if let Some(id) = &job.format_id {
            job.candidate
                .formats
                .iter()
                .any(|f| &f.id == id && f.lossless)
        } else {
            job.candidate.formats.iter().any(|f| f.lossless)
        }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_order_is_bounded_and_disabled_means_none() {
        let list = vec![
            "youtube".into(),
            "monochrome".into(),
            "slskd".into(),
            "youtube".into(),
            "soundcloud".into(),
        ];
        assert_eq!(
            provider_order("monochrome", true, &list, true),
            vec!["youtube", "slskd", "soundcloud"]
        );
        assert_eq!(
            provider_order("monochrome", true, &list, false),
            vec!["youtube", "soundcloud"]
        );
        assert!(provider_order("monochrome", false, &list, true).is_empty());
    }
    fn job() -> DownloadJob {
        DownloadJob {
            id: "test".into(),
            track: Track::default(),
            candidate: Candidate::default(),
            state: "downloading".into(),
            progress: 0.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
        }
    }
    #[test]
    fn switches_only_source_failures_and_never_cancelled_or_processing_jobs() {
        let mut settings = Settings::default();
        settings.download_fallback_enabled = true;
        let mut job = job();
        assert!(may_switch(&job, "HTTP 404", &settings, false));
        assert!(!may_switch(&job, "HTTP 404", &settings, true));
        assert!(!may_switch(&job, "Cancelled", &settings, false));
        for state in ["validating", "tagging", "publishing", "completed"] {
            job.state = state.into();
            assert!(!may_switch(&job, "failure", &settings, false));
        }
        job.state = "downloading".into();
        job.chapter_ids = Some(vec!["chapter".into()]);
        assert!(!may_switch(&job, "HTTP 404", &settings, false));
        job.chapter_ids = None;
        settings.download_fallback_enabled = false;
        assert!(!may_switch(&job, "HTTP 404", &settings, false));
    }
    #[test]
    fn source_quality_is_preserved_unless_mp3_is_explicitly_requested() {
        let mut job = job();
        job.candidate.formats = vec![crate::models::AudioFormat {
            id: "flac".into(),
            codec: "flac".into(),
            extension: "flac".into(),
            lossless: true,
            bitrate: None,
            sample_rate: Some(44100),
        }];
        assert!(requires_lossless(&job));
        job.output_profile = "mp3".into();
        assert!(!requires_lossless(&job));
    }
    #[tokio::test]
    async fn lossless_guard_checks_decoded_file_codec_not_filename_claims() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        for (name, codec, expected) in
            [("lossless.flac", "flac", true), ("lossy.m4a", "aac", false)]
        {
            let path = temp.path().join(name);
            let result = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=44100:cl=stereo",
                    "-t",
                    "0.1",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(result.status.success());
            assert_eq!(
                crate::media::verify_lossless_fallback(&path, &settings)
                    .await
                    .is_ok(),
                expected
            );
        }
    }
    #[test]
    fn fallback_does_not_substitute_live_versions_or_covers() {
        let t = Track {
            artist: "Sui Generis".into(),
            title: "Quizás, Porque".into(),
            duration_seconds: Some(132.973),
            ..Track::default()
        };
        let c = Candidate {
            title: "Sui Generis - Quizás, Porque (Official Audio)".into(),
            duration_seconds: Some(135.0),
            confidence: "review".into(),
            ..Candidate::default()
        };
        assert!(recording_matches(&t, &c));
        for title in [
            "Sui Generis - Quizás, Porque (Live)",
            "Sui Generis - Quizás, Porque Cover",
        ] {
            assert!(!recording_matches(
                &t,
                &Candidate {
                    title: title.into(),
                    ..c.clone()
                }
            ));
        }
        assert!(!recording_matches(
            &t,
            &Candidate {
                duration_seconds: Some(200.0),
                ..c
            }
        ));
    }
}
