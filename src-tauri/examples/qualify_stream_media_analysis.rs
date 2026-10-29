//! Finite private-profile driver for the production full-stream media job.
//!
//! Persistent cache qualification uses one new direct child of `/tmp`, for
//! example: `/tmp/burrata-stream-cache-qualification-runtime31`. Run once with
//! `BURRATA_STREAM_CACHE_PROFILE=... BURRATA_STREAM_CACHE_PHASE=populate`, then
//! in a new process with `...PHASE=restart`, then optionally `...PHASE=variant-mismatch`.
//!
//! The caller supplies stable provider identity and metadata only. No URL is
//! accepted or printed. The default mode uses a temporary Store. Optional
//! persistent phases are restricted to a marked direct child of `/tmp` and
//! support a fresh populate, a new-process restart lookup, and a deliberate
//! stale-variant metadata rejection against the production resolver.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use burrata_lib::{
    acquisition::AcquisitionManager,
    analysis_stream_selection::{
        SelectedStreamAnalysisJobs, SelectedStreamMediaAnalysisRequest, StreamMediaTrackContext,
    },
    connections::ConnectionManager,
    observed_analysis::ObservedAnalysis,
    storage::Store,
};
use serde_json::{json, Value};

const PROFILE_ENV: &str = "BURRATA_STREAM_CACHE_PROFILE";
const PHASE_ENV: &str = "BURRATA_STREAM_CACHE_PHASE";
const PROFILE_PREFIX: &str = "burrata-stream-cache-qualification-";
const PROFILE_MARKER: &str = "qualification-profile.json";
const CACHE_PREFERENCE: &str = "streamAnalysisCache.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QualificationPhase {
    Ephemeral,
    Populate,
    Restart,
    VariantMismatch,
}

impl QualificationPhase {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "populate" => Ok(Self::Populate),
            "restart" => Ok(Self::Restart),
            "variant-mismatch" => Ok(Self::VariantMismatch),
            _ => Err(
                "Stream cache qualification phase must be populate, restart, or variant-mismatch"
                    .into(),
            ),
        }
    }
}

struct ProfileLock(PathBuf);

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn validate_marked_profile_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new("/tmp")) {
        return Err("Persistent qualification profile must be a direct child of /tmp".into());
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Persistent qualification profile name is invalid")?;
    let suffix = name
        .strip_prefix(PROFILE_PREFIX)
        .ok_or("Persistent qualification profile must use the marked burrata-stream-cache-qualification- prefix")?;
    if suffix.is_empty()
        || suffix.len() > 64
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("Persistent qualification profile name is invalid".into());
    }
    Ok(())
}

fn prepare_persistent_profile(
    path: &Path,
    phase: QualificationPhase,
    provider: &str,
    source_id: &str,
) -> Result<(PathBuf, ProfileLock), String> {
    validate_marked_profile_path(path)?;
    let expected_marker = json!({
        "format": "org.burrata.stream-cache-qualification",
        "schemaVersion": 1,
        "provider": provider,
        "sourceIdentitySha256": burrata_lib::stream_analysis::stream_source_identity_digest(provider, source_id)
            .map_err(|error| error.to_string())?,
    });

    match phase {
        QualificationPhase::Populate => {
            if path.exists() {
                return Err(
                    "Populate phase requires a new, unused marked profile directory".into(),
                );
            }
            fs::create_dir(path).map_err(|_| "Could not create marked qualification profile")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Could not restrict qualification profile permissions")?;
            }
            let marker_path = path.join(PROFILE_MARKER);
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker_path)
                .map_err(|_| "Could not create qualification profile marker")?;
            marker
                .write_all(
                    &serde_json::to_vec(&expected_marker)
                        .map_err(|_| "Could not encode qualification profile marker")?,
                )
                .map_err(|_| "Could not write qualification profile marker")?;
        }
        QualificationPhase::Restart | QualificationPhase::VariantMismatch => {
            let metadata =
                fs::symlink_metadata(path).map_err(|_| "Qualification profile does not exist")?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Qualification profile must be a real directory".into());
            }
            let marker_path = path.join(PROFILE_MARKER);
            let marker_metadata = fs::symlink_metadata(&marker_path)
                .map_err(|_| "Qualification profile marker is missing")?;
            if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
                return Err("Qualification profile marker is invalid".into());
            }
            let bytes = fs::read(&marker_path)
                .map_err(|_| "Could not read qualification profile marker")?;
            if bytes.len() > 4096
                || serde_json::from_slice::<Value>(&bytes).ok().as_ref() != Some(&expected_marker)
            {
                return Err("Qualification profile marker does not match this source".into());
            }
            let database_path = path.join("stream-analysis.sqlite");
            let database_metadata = fs::symlink_metadata(&database_path)
                .map_err(|_| "Persistent phase requires the populated qualification Store")?;
            if !database_metadata.is_file() || database_metadata.file_type().is_symlink() {
                return Err("Qualification Store must be a regular file".into());
            }
        }
        QualificationPhase::Ephemeral => {
            return Err("Persistent profile cannot use ephemeral phase".into());
        }
    }

    let lock_path = path.join("qualification.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Qualification profile is already in use or has a stale lock")?;
    drop(lock);
    Ok((path.join("stream-analysis.sqlite"), ProfileLock(lock_path)))
}

fn qualification_phase_from_env() -> Result<(QualificationPhase, Option<PathBuf>), String> {
    match (std::env::var_os(PROFILE_ENV), std::env::var(PHASE_ENV).ok()) {
        (None, None) => Ok((QualificationPhase::Ephemeral, None)),
        (Some(path), Some(phase)) => Ok((
            QualificationPhase::parse(&phase)?,
            Some(PathBuf::from(path)),
        )),
        (Some(_), None) => Err("Set BURRATA_STREAM_CACHE_PHASE with the persistent profile".into()),
        (None, Some(_)) => {
            Err("Persistent qualification phase requires BURRATA_STREAM_CACHE_PROFILE".into())
        }
    }
}

fn has_stable_cache_for_source(cache: &Value, provider: &str, source_id: &str) -> bool {
    let Ok(source_digest) =
        burrata_lib::stream_analysis::stream_source_identity_digest(provider, source_id)
    else {
        return false;
    };
    cache
        .get("records")
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records.iter().any(|record| {
                record.get("resource").is_some_and(|resource| {
                    resource.get("provider").and_then(Value::as_str) == Some(provider)
                        && resource.get("sourceIdentitySha256").and_then(Value::as_str)
                            == Some(source_digest.as_str())
                        && resource
                            .get("audioVariantSha256")
                            .and_then(Value::as_str)
                            .is_some_and(|digest| digest.len() == 64)
                }) && record
                    .get("waveform")
                    .and_then(Value::as_object)
                    .is_some_and(|waveform| {
                        waveform.get("complete").and_then(Value::as_bool) == Some(true)
                            && waveform.get("binCount").and_then(Value::as_u64) == Some(512)
                    })
            })
        })
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err("Usage: qualify_stream_media_analysis <provider> <source-id> <duration-seconds> <title> <artist>".into());
    }
    let provider = args[0].clone();
    let source_id = args[1].clone();
    let duration_seconds = args[2].parse::<f64>().map_err(|error| error.to_string())?;
    let title = args[3].clone();
    let artist = args[4].clone();
    let track_id = format!("qualification:stream:{provider}:{source_id}");

    let (phase, profile_path) = qualification_phase_from_env()?;
    let mut ephemeral_profile = None;
    let (database_path, _profile_lock) = if let Some(profile_path) = profile_path {
        let (database_path, lock) =
            prepare_persistent_profile(&profile_path, phase, &provider, &source_id)?;
        (database_path, Some(lock))
    } else {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let database_path = temp.path().join("qualification.sqlite");
        ephemeral_profile = Some(temp);
        (database_path, None)
    };
    let store = Arc::new(Store::open(&database_path).map_err(|error| error.to_string())?);
    let connections = Arc::new(ConnectionManager::new(store.clone()));
    let acquisition = Arc::new(AcquisitionManager::new_for_diagnostics(
        store.clone(),
        connections,
    ));
    let observed = ObservedAnalysis::new(store.clone());
    let jobs = SelectedStreamAnalysisJobs::new();
    let track = StreamMediaTrackContext {
        id: track_id.clone(),
        provider: provider.clone(),
        provider_id: if provider == "youtube" {
            source_id
                .strip_prefix("youtube:")
                .unwrap_or(&source_id)
                .to_owned()
        } else {
            source_id.clone()
        },
        artist,
        title,
        source_page_url: std::env::var("BURRATA_QUALIFICATION_SOURCE_PAGE_URL").ok(),
        version: None,
        album: None,
        album_artist: None,
        duration_seconds: Some(duration_seconds),
        isrc: None,
        catalog_number: None,
        release_date: None,
        recording_date: None,
        has_playback_range: false,
    };
    let before_count = store
        .list_tracks("library", None)
        .map_err(|error| error.to_string())?
        .len();
    if before_count != 0 {
        return Err("Qualification Store must not contain library tracks".into());
    }
    let started = std::time::Instant::now();
    let cache_before_mismatch = if phase == QualificationPhase::VariantMismatch {
        let cache = store
            .get_json_preference(CACHE_PREFERENCE)?
            .unwrap_or(Value::Null);
        if !has_stable_cache_for_source(&cache, &provider, &source_id) {
            return Err(
                "Variant-mismatch phase requires a complete cached variant for this exact source"
                    .into(),
            );
        }
        Some(cache)
    } else {
        None
    };
    let request = SelectedStreamMediaAnalysisRequest {
        track,
        provider: provider.clone(),
        source_id: source_id.clone(),
        duration_seconds,
        audio_variant: (phase == QualificationPhase::VariantMismatch).then(|| {
            burrata_lib::stream_analysis::StreamAudioVariant {
                format_id: Some("qualification-mismatch".into()),
                codec: Some("qualification-mismatch".into()),
                mime_type: Some("audio/qualification-mismatch".into()),
                contains_video: Some(false),
                ..burrata_lib::stream_analysis::StreamAudioVariant::default()
            }
        }),
        // This standalone qualification driver has no active WebView playback
        // generation, so it deliberately exercises the native resolver path.
        webview_generation: None,
    };
    let job = jobs.start_media_analysis(
        store.clone(),
        acquisition.clone(),
        observed.clone(),
        request.clone(),
        None,
    )?;
    let finished = loop {
        let current = jobs.get(&job.job_id)?;
        if matches!(current.state.as_str(), "completed" | "cancelled" | "failed") {
            break current;
        }
        if started.elapsed() > Duration::from_secs(21 * 60) {
            jobs.cancel(&job.job_id)?;
            return Err("Qualification wait exceeded its 21 minute bound".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let result = finished
        .results
        .first()
        .ok_or_else(|| "Analysis job returned no result".to_owned())?;
    if phase == QualificationPhase::VariantMismatch {
        let cache_after = store
            .get_json_preference(CACHE_PREFERENCE)?
            .unwrap_or(Value::Null);
        let mismatch_after_count = store
            .list_tracks("library", None)
            .map_err(|error| error.to_string())?
            .len();
        if finished.state != "completed"
            || result.status != "failed"
            || result.error.as_deref()
                != Some("Resolved audio format changed; no analysis was cached")
            || cache_before_mismatch.as_ref() != Some(&cache_after)
            || mismatch_after_count != before_count
        {
            return Err("Production resolver did not reject the mismatched expected audio variant without changing cache or library".into());
        }
        let summary = json!({
            "phase": "variant-mismatch",
            "state": finished.state,
            "status": result.status,
            "rejection": "stale-expected-audio-variant",
            "cacheUnchanged": true,
            "libraryTrackCountBefore": before_count,
            "libraryTrackCountAfter": mismatch_after_count,
            "libraryCountUnchanged": before_count == mismatch_after_count,
            "meaning": "stale expected variant metadata was rejected after exact source re-resolution; this is not an alternate encoded playback comparison",
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?
        );
        drop(store);
        drop(ephemeral_profile);
        return Ok(());
    }
    if finished.state != "completed" || result.status != "completed" {
        return Err(format!(
            "Analysis did not complete successfully: job={}, result={}, errorPresent={}",
            finished.state,
            result.status,
            result.error.is_some()
        ));
    }
    let analysis = result
        .analysis
        .as_ref()
        .ok_or_else(|| "Completed analysis has no record".to_owned())?;
    if phase != QualificationPhase::Ephemeral && !analysis.resource.has_stable_audio_variant() {
        return Err(
            "Persistent qualification requires resolver-provided stable audio-variant metadata"
                .into(),
        );
    }
    analysis.validate().map_err(|error| error.to_string())?;
    let waveform = analysis
        .waveform
        .as_ref()
        .ok_or_else(|| "Completed full-media analysis has no waveform".to_owned())?;
    waveform
        .validate(analysis.duration_seconds)
        .map_err(|error| error.to_string())?;
    let after_count = store
        .list_tracks("library", None)
        .map_err(|error| error.to_string())?
        .len();
    if after_count != before_count {
        return Err("Qualification unexpectedly changed the library track count".into());
    }
    if phase == QualificationPhase::Restart && !result.cache_hit {
        return Err("Restart phase did not reuse the exact encoded-variant cache record".into());
    }
    if phase == QualificationPhase::Populate && result.cache_hit {
        return Err("Populate phase unexpectedly hit a preexisting cache record".into());
    }
    let summary = json!({
        "phase": match phase {
            QualificationPhase::Ephemeral => "ephemeral",
            QualificationPhase::Populate => "populate",
            QualificationPhase::Restart => "restart",
            QualificationPhase::VariantMismatch => "variant-mismatch",
        },
        "state": finished.state,
        "status": result.status,
        "cacheHit": result.cache_hit,
        "elapsedMs": result.elapsed_ms,
        "wallMs": started.elapsed().as_millis(),
        "mediaDurationSeconds": analysis.duration_seconds,
        "speedup": (!result.cache_hit).then(|| result.elapsed_ms.map(|ms| {
            if ms == 0 { 0.0 } else { analysis.duration_seconds * 1000.0 / ms as f64 }
        })).flatten(),
        "resourceIdentityHash": match &analysis.resource {
            burrata_lib::stream_analysis::AnalysisResource::Stream { source_identity_sha256, .. } => Some(source_identity_sha256),
            _ => None,
        },
        "audioVariantCacheable": analysis.resource.has_stable_audio_variant(),
        "audioVariantSha256": match &analysis.resource {
            burrata_lib::stream_analysis::AnalysisResource::Stream { audio_variant_sha256, .. } => audio_variant_sha256.as_deref(),
            _ => None,
        },
        "bpm": analysis.bpm.as_ref().map(|value| json!({
            "bpm": value.bpm,
            "confidence": value.confidence,
            "confidenceMeaning": "normalized onset autocorrelation; not calibrated"
        })),
        "key": analysis.key.as_ref().map(|value| json!({
            "key": value.key,
            "confidence": value.confidence,
            "confidenceMeaning": "fraction of five windows agreeing; not calibrated"
        })),
        "waveformBinCount": waveform.bin_count,
        "waveformComplete": waveform.complete,
        "libraryTrackCountBefore": before_count,
        "libraryTrackCountAfter": after_count,
        "libraryCountUnchanged": before_count == after_count,
        // Resolver errors can contain signed media URLs; qualification output
        // records presence only and never serializes backend error text.
        "errorPresent": result.error.is_some(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?
    );

    if result.status == "completed" && analysis.resource.has_stable_audio_variant() {
        let cached = observed.cached_resource(&analysis.resource);
        let cached_record = cached.ok_or_else(|| {
            "Exact full-waveform cache lookup missed after successful analysis".to_owned()
        })?;
        cached_record
            .waveform
            .as_ref()
            .ok_or_else(|| "Exact cache record has no waveform".to_owned())?
            .validate(cached_record.duration_seconds)
            .map_err(|error| error.to_string())?;
        println!("cacheVerification={}", "hit");
    } else if observed.cached_resource(&analysis.resource).is_some() {
        return Err("Variant-unknown analysis unexpectedly entered the persistent cache".into());
    }

    if phase == QualificationPhase::Ephemeral
        && result.status == "completed"
        && analysis.resource.has_stable_audio_variant()
    {
        let second_started = std::time::Instant::now();
        let second =
            jobs.start_media_analysis(store.clone(), acquisition, observed, request, None)?;
        let second_result = loop {
            let current = jobs.get(&second.job_id)?;
            if matches!(current.state.as_str(), "completed" | "cancelled" | "failed") {
                break current;
            }
            if second_started.elapsed() > Duration::from_secs(30) {
                jobs.cancel(&second.job_id)?;
                return Err("Cache-hit verification exceeded its 30 second bound".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let second_row = second_result
            .results
            .first()
            .ok_or_else(|| "Cache verification returned no result".to_owned())?;
        let repeat_record = second_row
            .analysis
            .as_ref()
            .ok_or_else(|| "Cache verification returned no analysis record".to_owned())?;
        if second_result.state != "completed"
            || second_row.status != "completed"
            || !second_row.cache_hit
        {
            return Err("Repeat job was not a successful exact-resource cache hit".into());
        }
        repeat_record
            .validate()
            .map_err(|error| error.to_string())?;
        repeat_record
            .waveform
            .as_ref()
            .ok_or_else(|| "Repeat cache record has no waveform".to_owned())?
            .validate(repeat_record.duration_seconds)
            .map_err(|error| error.to_string())?;
        println!(
            "repeatCacheHit={}",
            serde_json::json!({
                "status": second_row.status,
                "cacheHit": second_row.cache_hit,
                "elapsedMs": second_row.elapsed_ms,
                "waveformBinCount": second_row.analysis.as_ref().and_then(|row| row.waveform.as_ref().map(|waveform| waveform.bin_count)),
            })
        );
    }

    // Persistent mode leaves only its explicitly marked private qualification
    // profile; ephemeral mode removes its temporary database here.
    drop(store);
    drop(ephemeral_profile);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_profile_path_requires_a_marked_direct_tmp_child() {
        assert!(validate_marked_profile_path(Path::new(
            "/tmp/burrata-stream-cache-qualification-runtime31"
        ))
        .is_ok());
        assert!(validate_marked_profile_path(Path::new("/tmp/ordinary-profile")).is_err());
        assert!(validate_marked_profile_path(Path::new(
            "/home/example/burrata-stream-cache-qualification-runtime31"
        ))
        .is_err());
        assert!(validate_marked_profile_path(Path::new(
            "/tmp/burrata-stream-cache-qualification-../outside"
        ))
        .is_err());
    }

    #[test]
    fn mismatch_phase_requires_complete_variant_cache_for_exact_source() {
        let source_digest = burrata_lib::stream_analysis::stream_source_identity_digest(
            "youtube",
            "youtube:video12345",
        )
        .unwrap();
        let cache = json!({
            "records": [{
                "resource": {
                    "kind": "stream",
                    "provider": "youtube",
                    "sourceIdentitySha256": source_digest,
                    "audioVariantSha256": "a".repeat(64)
                },
                "waveform": {"complete": true, "binCount": 512}
            }]
        });
        assert!(has_stable_cache_for_source(
            &cache,
            "youtube",
            "youtube:video12345"
        ));
        assert!(!has_stable_cache_for_source(
            &cache,
            "soundcloud",
            "youtube:video12345"
        ));
        assert!(!has_stable_cache_for_source(
            &json!({"records": []}),
            "youtube",
            "youtube:video12345"
        ));
    }
}
