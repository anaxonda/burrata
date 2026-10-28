//! Actual two-format stream-analysis/cache qualification.
//!
//! This driver resolves the same exact YouTube recording through the normal
//! production resolver, then asks the production same-recording recovery path
//! for its alternate audio preference. It remuxes each resolved audio stream
//! into a short-lived Matroska file (256 MiB hard FFmpeg output cap plus a
//! post-write size check), hashes copied audio-packet payloads with FFmpeg's
//! hash muxer (not nondeterministic container bytes), probes actual codec and
//! container metadata, decodes that file to bounded PCM, and feeds the production
//! WholeTrackCapture analyzer. No stream URL/header is printed or persisted.
//!
//! Populate and restart are separate processes. The persistent Store is
//! restricted to a marked direct child of /tmp. Populate seals resolver
//! metadata and packet-payload hashes into a small receipt; restart rejects
//! any drift before accepting the persisted cache hits. Neither phase inserts
//! tracks.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use burrata_lib::{
    acquisition::AcquisitionManager,
    connections::ConnectionManager,
    models::{Candidate, Track},
    observed_analysis::ObservedAnalysis,
    playback_sources::{self, ResolvedSource},
    storage::Store,
    stream_analysis::{
        stream_source_identity_digest, AnalysisResource, PcmBatch, StreamAudioVariant,
        WholeTrackCapture, KEY_PCM_RATE, MAX_TRACK_SECONDS,
    },
};
use serde_json::{json, Value};
use tokio::{io::AsyncReadExt, process::Command};

const PROFILE_ENV: &str = "BURRATA_STREAM_VARIANT_PROFILE";
const PHASE_ENV: &str = "BURRATA_STREAM_VARIANT_PHASE";
const PROFILE_PREFIX: &str = "burrata-stream-variant-qualification-";
const MARKER: &str = "qualification-profile.json";
const RECEIPT: &str = "encoded-variants-receipt.json";
const CACHE_PREF: &str = "streamAnalysisCache.v1";
const MAX_REMUX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PCM_BYTES: u64 = KEY_PCM_RATE as u64 * MAX_TRACK_SECONDS as u64 * 4;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const FULL_DECODE_TIMEOUT: Duration = Duration::from_secs(21 * 60);
const REMUX_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Populate,
    Restart,
}

impl Phase {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "populate" => Ok(Self::Populate),
            "restart" => Ok(Self::Restart),
            _ => Err("Phase must be populate or restart".into()),
        }
    }
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn validate_profile(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new("/tmp")) {
        return Err("Profile must be a direct child of /tmp".into());
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Invalid profile name")?;
    let suffix = name
        .strip_prefix(PROFILE_PREFIX)
        .ok_or("Profile name must use the marked qualification prefix")?;
    if suffix.is_empty()
        || suffix.len() > 64
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Invalid marked profile suffix".into());
    }
    Ok(())
}

fn open_profile(
    path: &Path,
    phase: Phase,
    marker_value: &Value,
) -> Result<(PathBuf, Lock), String> {
    validate_profile(path)?;
    match phase {
        Phase::Populate => {
            if path.exists() {
                return Err("Populate requires a new unused profile".into());
            }
            fs::create_dir(path).map_err(|_| "Could not create marked profile")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Could not restrict profile permissions")?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.join(MARKER))
                .map_err(|_| "Could not create profile marker")?;
            file.write_all(
                &serde_json::to_vec(marker_value).map_err(|_| "Could not encode profile marker")?,
            )
            .map_err(|_| "Could not write profile marker")?;
        }
        Phase::Restart => {
            let meta = fs::symlink_metadata(path).map_err(|_| "Profile does not exist")?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("Profile must be a real directory".into());
            }
            let marker_path = path.join(MARKER);
            let meta =
                fs::symlink_metadata(&marker_path).map_err(|_| "Profile marker is missing")?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Profile marker is invalid".into());
            }
            let bytes = fs::read(marker_path).map_err(|_| "Could not read profile marker")?;
            if bytes.len() > 4096
                || serde_json::from_slice::<Value>(&bytes).ok().as_ref() != Some(marker_value)
            {
                return Err("Profile marker does not match this recording".into());
            }
            let db = path.join("stream-analysis.sqlite");
            let meta =
                fs::symlink_metadata(&db).map_err(|_| "Restart requires the populated Store")?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Store must be a regular file".into());
            }
        }
    }
    let lock_path = path.join("qualification.lock");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Profile is in use or has a stale lock")?;
    Ok((path.join("stream-analysis.sqlite"), Lock(lock_path)))
}

fn headers_arg(source: &burrata_lib::native_output::AudioSource) -> Result<Option<String>, String> {
    if source.headers.is_empty() {
        return Ok(None);
    }
    let mut lines = String::new();
    for (name, value) in &source.headers {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.len() > 4096
            || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        {
            return Err("Resolved request headers failed bounded validation".into());
        }
        lines.push_str(name);
        lines.push_str(": ");
        lines.push_str(value);
        lines.push_str("\r\n");
        if lines.len() > MAX_HEADER_BYTES {
            return Err("Resolved request headers exceed the cap".into());
        }
    }
    Ok(Some(lines))
}

fn check_resolved(source: &ResolvedSource) -> Result<(), String> {
    let url =
        url::Url::parse(&source.source.input).map_err(|_| "Resolver returned invalid audio URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Only resolver-provided HTTPS audio resources are accepted".into());
    }
    if source.station.is_some() || source.source.cenc.is_some() {
        return Err("Radio or encrypted audio is outside this qualification".into());
    }
    Ok(())
}

async fn remux_and_hash(
    source: &ResolvedSource,
) -> Result<(tempfile::NamedTempFile, String, Value), String> {
    check_resolved(source)?;
    let file = tempfile::NamedTempFile::new()
        .map_err(|_| "Could not create temporary encoded-media file")?;
    let headers = headers_arg(&source.source)?;
    let mut command = Command::new(&source.source.ffmpeg);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-nostdin",
            "-v",
            "error",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-max_alloc",
            "134217728",
            "-protocol_whitelist",
            "http,https,tls,tcp,crypto,data",
            "-rw_timeout",
            "10000000",
        ]);
    if let Some(headers) = headers.as_deref() {
        command.args(["-headers", headers]);
    }
    command
        .arg("-i")
        .arg(&source.source.input)
        .args([
            "-map",
            "0:a:0",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-vn",
            "-c:a",
            "copy",
            "-f",
            "matroska",
            "-fs",
        ])
        .arg((MAX_REMUX_BYTES + 1).to_string())
        .arg("-y")
        .arg(file.path());
    let status = tokio::time::timeout(REMUX_TIMEOUT, command.status())
        .await
        .map_err(|_| "Encoded stream remux exceeded its 5 minute bound")?
        .map_err(|_| "Could not start encoded stream remux")?;
    if !status.success() {
        return Err("Resolver media could not be remuxed as encoded audio packets".into());
    }
    let size = fs::metadata(file.path())
        .map_err(|_| "Could not stat remuxed media")?
        .len();
    if size == 0 || size > MAX_REMUX_BYTES {
        return Err("Remuxed media size is outside the qualification bound".into());
    }
    // Hash copied audio packets from the demuxed temporary media, not the
    // Matroska container bytes (container IDs/timestamps can vary per run).
    let digest = hash_audio_packets(&source.source.ffmpeg, file.path()).await?;
    let output = tokio::time::timeout(
        PROCESS_TIMEOUT,
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-show_entries",
                "format=format_name,duration:stream=codec_type,codec_name,sample_rate,channels",
                "-of",
                "json",
            ])
            .arg(file.path())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Encoded media probe exceeded its bound")?
    .map_err(|_| "Could not start encoded media probe")?;
    if !output.status.success() || output.stdout.len() > 16 * 1024 {
        return Err("Encoded media metadata could not be parsed within the bound".into());
    }
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Encoded media probe returned invalid metadata")?;
    if parsed["streams"]
        .as_array()
        .is_none_or(|streams| streams.len() != 1 || streams[0]["codec_type"] != "audio")
    {
        return Err("Remuxed resource does not contain exactly one audio stream".into());
    }
    if parsed["streams"][0]["codec_name"].as_str().is_none()
        || parsed["format"]["format_name"].as_str().is_none()
    {
        return Err("Encoded stream probe omitted codec or container metadata".into());
    }
    Ok((file, digest, parsed))
}

async fn hash_audio_packets(ffmpeg: &Path, path: &Path) -> Result<String, String> {
    let mut child = Command::new(ffmpeg)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args([
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-map", "0:a:0", "-vn", "-c:a", "copy", "-f", "hash", "-hash", "sha256", "pipe:1",
        ])
        .spawn()
        .map_err(|_| "Could not start encoded-packet hash")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Packet hash stdout is unavailable")?;
    let hash_process = async {
        let mut bytes = Vec::with_capacity(128);
        let mut limited = (&mut stdout).take(1025);
        limited
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Could not read encoded-packet hash")?;
        if bytes.len() > 1024 {
            let _ = child.kill().await;
            return Err("Encoded-packet hash output exceeded 1 KiB".into());
        }
        drop(limited);
        drop(stdout);
        let status = child
            .wait()
            .await
            .map_err(|_| "Could not collect encoded-packet hash status")?;
        if !status.success() {
            return Err("Could not hash the copied encoded audio packets".into());
        }
        let line = std::str::from_utf8(&bytes)
            .map_err(|_| "Encoded-packet hash was not UTF-8")?
            .trim();
        let digest = line
            .strip_prefix("SHA256=")
            .ok_or("Encoded-packet hash output had an unexpected format")?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Encoded-packet hash output was invalid".into());
        }
        Ok(digest.to_ascii_lowercase())
    };
    tokio::time::timeout(REMUX_TIMEOUT, hash_process)
        .await
        .map_err(|_| "Encoded-packet hashing exceeded its 5 minute bound")?
}

async fn analyze_remux(
    file: &Path,
    ffmpeg: &Path,
    track: &Track,
    source_id: &str,
    variant: &StreamAudioVariant,
    store: &Store,
    observed: &ObservedAnalysis,
) -> Result<(String, f64), String> {
    let duration = track
        .duration_seconds
        .ok_or("Missing whole-second source duration")?;
    if !duration.is_finite()
        || duration.fract().abs() > 1e-6
        || !(8.0..=MAX_TRACK_SECONDS).contains(&duration)
    {
        return Err(
            "Qualification requires an integer duration from 8 seconds through 900 seconds".into(),
        );
    }
    let provider = &track.provider;
    let source_identity_sha256 = stream_source_identity_digest(provider, source_id)
        .map_err(|_| "Invalid provider recording identity")?;
    let audio_variant_sha256 = variant
        .identity_sha256()
        .ok_or("Resolved encoded variant lacks stable format identity")?;
    let resource = AnalysisResource::Stream {
        recording_id: track.id.clone(),
        provider: provider.clone(),
        source_identity_sha256,
        audio_variant_sha256: Some(audio_variant_sha256.clone()),
    };
    let cached = observed.cached_stream_media(track, provider, source_id, duration, Some(variant));
    if cached.is_some() {
        return Err("Populate expected an empty exact-variant cache entry".into());
    }
    let capture = WholeTrackCapture::new_with_rounded_duration(1, resource, duration, 0.0, true)
        .map_err(|_| "Could not initialize bounded whole-track analyzer")?;
    let mut child = Command::new(ffmpeg);
    child
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args([
            "-nostdin",
            "-v",
            "error",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-max_alloc",
            "134217728",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(file)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "1",
            "-ar",
            &KEY_PCM_RATE.to_string(),
            "-threads",
            "1",
            "-f",
            "f32le",
            "pipe:1",
        ]);
    let decode = async {
        let mut process = child
            .spawn()
            .map_err(|_| "Could not start decoded-pcm extractor")?;
        let mut stdout = process
            .stdout
            .take()
            .ok_or("FFmpeg stdout is unavailable")?;
        let mut bytes = Vec::with_capacity(64 * 1024);
        let mut buf = vec![0u8; 64 * 1024];
        let mut frames = 0u64;
        let mut capture = capture;
        loop {
            let n = stdout
                .read(&mut buf)
                .await
                .map_err(|_| "Could not read decoded PCM")?;
            if n == 0 {
                break;
            }
            if bytes.len().saturating_add(n) > 64 * 1024 + 3 {
                return Err("PCM decoder chunk exceeded its bound".to_owned());
            }
            bytes.extend_from_slice(&buf[..n]);
            let aligned = bytes.len() / 4 * 4;
            if aligned == 0 {
                continue;
            }
            let samples = bytes[..aligned]
                .chunks_exact(4)
                .map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]))
                .collect::<Vec<_>>();
            if (frames + samples.len() as u64).saturating_mul(4) > MAX_PCM_BYTES {
                return Err("Decoded PCM exceeded the 15 minute bound".to_owned());
            }
            frames += samples.len() as u64;
            let end = frames as f64 / f64::from(KEY_PCM_RATE);
            capture
                .push(PcmBatch {
                    generation: 1,
                    sample_rate: KEY_PCM_RATE,
                    channels: 1,
                    end_position_seconds: end,
                    interleaved: &samples,
                })
                .map_err(|_| "Decoded PCM failed whole-track continuity validation")?;
            bytes.drain(..aligned);
        }
        if !bytes.is_empty() || frames == 0 {
            return Err("Decoded PCM ended on an invalid sample boundary".to_owned());
        }
        let status = process
            .wait()
            .await
            .map_err(|_| "Could not collect decoder status")?;
        if !status.success() {
            return Err("Decoded stream ended with an FFmpeg error".to_owned());
        }
        let actual_duration = frames as f64 / f64::from(KEY_PCM_RATE);
        let record = capture
            .finish_with_rounded_duration(actual_duration, chrono::Utc::now().to_rfc3339())
            .map_err(|_| "Decoded audio did not reach natural whole-track EOF")?;
        record
            .validate()
            .map_err(|_| "Whole-track analysis validation failed")?;
        observed
            .cache_record(record)
            .map_err(|_| "Could not save exact encoded-variant analysis")?;
        let resource = AnalysisResource::Stream {
            recording_id: track.id.clone(),
            provider: provider.clone(),
            source_identity_sha256: stream_source_identity_digest(provider, source_id)
                .map_err(|_| "Invalid source identity")?,
            audio_variant_sha256: Some(audio_variant_sha256.clone()),
        };
        if observed.cached_resource(&resource).is_none() {
            return Err("Persisted exact variant could not be read back".into());
        }
        let cache = store
            .get_json_preference(CACHE_PREF)
            .map_err(|_| "Could not inspect stream cache")?
            .unwrap_or(Value::Null);
        let exact_records = cache["records"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .filter(|row| {
                        row["resource"]["audioVariantSha256"].as_str()
                            == resource_audio_variant(&resource)
                    })
                    .count()
            })
            .unwrap_or(0);
        if exact_records == 0 {
            return Err("Persisted cache did not contain the exact variant key".into());
        }
        Ok((audio_variant_sha256, actual_duration))
    };
    tokio::time::timeout(FULL_DECODE_TIMEOUT, decode)
        .await
        .map_err(|_| "Full-media decode exceeded 21 minutes")?
}

fn resource_audio_variant(resource: &AnalysisResource) -> Option<&str> {
    match resource {
        AnalysisResource::Stream {
            audio_variant_sha256,
            ..
        } => audio_variant_sha256.as_deref(),
        _ => None,
    }
}

fn variant_receipt(
    label: &str,
    variant: &StreamAudioVariant,
    packet_sha256: &str,
) -> Result<Value, String> {
    Ok(json!({
        "label": label,
        "formatId": variant.format_id,
        "codec": variant.codec,
        "mimeType": variant.mime_type,
        "expectedContentLength": variant.expected_content_length,
        "audioVariantSha256": variant.identity_sha256().ok_or("Variant is not stable")?,
        "encodedPacketSha256": packet_sha256,
    }))
}

fn receipt_value(
    source_identity_sha256: &str,
    primary: &StreamAudioVariant,
    primary_packet_sha256: &str,
    alternate: &StreamAudioVariant,
    alternate_packet_sha256: &str,
) -> Result<Value, String> {
    Ok(json!({
        "format": "org.burrata.stream-variant-receipt",
        "schemaVersion": 1,
        "sourceIdentitySha256": source_identity_sha256,
        "variants": [
            variant_receipt("primary", primary, primary_packet_sha256)?,
            variant_receipt("alternate", alternate, alternate_packet_sha256)?,
        ]
    }))
}

fn write_receipt(profile: &Path, value: &Value) -> Result<(), String> {
    let path = profile.join(RECEIPT);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Could not create encoded-variant receipt")?;
    file.write_all(
        &serde_json::to_vec(value).map_err(|_| "Could not encode encoded-variant receipt")?,
    )
    .map_err(|_| "Could not write encoded-variant receipt")?;
    file.sync_all()
        .map_err(|_| "Could not sync encoded-variant receipt".to_string())
}

fn read_receipt(profile: &Path) -> Result<Value, String> {
    let path = profile.join(RECEIPT);
    let metadata = fs::symlink_metadata(&path).map_err(|_| "Populate receipt is missing")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 8192 {
        return Err("Populate receipt is invalid or exceeds its 8 KiB bound".into());
    }
    let bytes = fs::read(path).map_err(|_| "Could not read populate receipt")?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Populate receipt JSON is invalid")?;
    if value["format"] != "org.burrata.stream-variant-receipt"
        || value["schemaVersion"] != 1
        || value["variants"]
            .as_array()
            .is_none_or(|items| items.len() != 2)
    {
        return Err("Populate receipt schema is invalid".into());
    }
    Ok(value)
}

async fn resolve_pair(
    store: &Store,
    acquisition: &AcquisitionManager,
    track: &Track,
    candidate: Candidate,
) -> Result<(ResolvedSource, ResolvedSource), String> {
    let cancel = tokio::sync::watch::channel(false).1;
    let primary = playback_sources::resolve(
        store,
        acquisition,
        track,
        Some(candidate.clone()),
        cancel.clone(),
    )
    .await
    .map_err(|_| "Could not resolve primary stream format")?;
    if primary
        .candidate
        .as_ref()
        .is_none_or(|v| v.provider != track.provider || v.id != candidate.id)
    {
        return Err("Primary resolver changed recording identity".into());
    }
    let alternate = playback_sources::recover(
        store,
        acquisition,
        track,
        primary
            .candidate
            .as_ref()
            .ok_or("Primary candidate missing")?,
        true,
        cancel,
    )
    .await
    .map_err(|_| "Production alternate-format recovery was unavailable")?;
    if alternate
        .candidate
        .as_ref()
        .is_none_or(|v| v.provider != track.provider || v.id != candidate.id)
    {
        return Err("Alternate resolver changed recording identity".into());
    }
    let a = StreamAudioVariant::from_resolver_quality(&primary.quality)
        .ok_or("Primary resolver did not provide a stable encoded variant")?;
    let b = StreamAudioVariant::from_resolver_quality(&alternate.quality)
        .ok_or("Alternate resolver did not provide a stable encoded variant")?;
    if a.identity_sha256() == b.identity_sha256() || a.format_id == b.format_id {
        return Err(
            "This source did not resolve to distinct primary and alternate encoded formats".into(),
        );
    }
    Ok((primary, alternate))
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err("Usage: qualify_stream_media_variants <youtube-video-id> <duration-seconds> <title> <artist>".into());
    }
    let video_id = args[0]
        .strip_prefix("youtube:")
        .unwrap_or(&args[0])
        .to_owned();
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("A canonical 11-character YouTube video ID is required".into());
    }
    let duration = args[1].parse::<f64>().map_err(|_| "Invalid duration")?;
    let title = args[2].clone();
    let artist = args[3].clone();
    if title.trim().is_empty()
        || title.len() > 512
        || artist.trim().is_empty()
        || artist.len() > 512
        || title.chars().chain(artist.chars()).any(char::is_control)
    {
        return Err("Title and artist must be bounded plain text".into());
    }
    let provider = "youtube".to_owned();
    let source_id = format!("youtube:{video_id}");
    let track = Track {
        id: format!("qualification:stream:{source_id}"),
        provider: provider.clone(),
        provider_id: video_id.clone(),
        source_url: format!("https://www.youtube.com/watch?v={video_id}"),
        artist,
        title: title.clone(),
        duration_seconds: Some(duration),
        ..Track::default()
    };
    let candidate = Candidate {
        id: source_id.clone(),
        provider: provider.clone(),
        url: track.source_url.clone(),
        title,
        uploader: Some(track.artist.clone()),
        duration_seconds: Some(duration),
        permission: "allowed".into(),
        confidence: "high".into(),
        score: 1.0,
        ..Candidate::default()
    };
    let digest = stream_source_identity_digest(&provider, &source_id)
        .map_err(|_| "Invalid source identity")?;
    let marker = json!({"format":"org.burrata.stream-variant-qualification","schemaVersion":1,"sourceIdentitySha256":digest});
    let phase =
        Phase::parse(&std::env::var(PHASE_ENV).map_err(|_| "Set BURRATA_STREAM_VARIANT_PHASE")?)?;
    let profile =
        PathBuf::from(std::env::var_os(PROFILE_ENV).ok_or("Set BURRATA_STREAM_VARIANT_PROFILE")?);
    let (database_path, _lock) = open_profile(&profile, phase, &marker)?;
    let store = Arc::new(Store::open(&database_path).map_err(|_| "Could not open marked Store")?);
    let connections = Arc::new(ConnectionManager::new(store.clone()));
    let acquisition = Arc::new(AcquisitionManager::new_for_diagnostics(
        store.clone(),
        connections,
    ));
    let observed = ObservedAnalysis::new(store.clone());
    let before_count = store
        .list_tracks("library", None)
        .map_err(|_| "Could not inspect private library")?
        .len();
    if before_count != 0 {
        return Err("Marked Store must have no library tracks".into());
    }
    let (primary, alternate) = resolve_pair(&store, &acquisition, &track, candidate).await?;
    let primary_variant = StreamAudioVariant::from_resolver_quality(&primary.quality)
        .ok_or("Primary format metadata unavailable")?;
    let alternate_variant = StreamAudioVariant::from_resolver_quality(&alternate.quality)
        .ok_or("Alternate format metadata unavailable")?;
    let (primary_file, primary_packet_sha, primary_probe) = remux_and_hash(&primary).await?;
    let (alternate_file, alternate_packet_sha, alternate_probe) =
        remux_and_hash(&alternate).await?;
    if primary_packet_sha == alternate_packet_sha {
        return Err("Primary and alternate resolver outputs had identical packetized bytes".into());
    }
    let primary_resource = AnalysisResource::Stream {
        recording_id: track.id.clone(),
        provider: provider.clone(),
        source_identity_sha256: digest.clone(),
        audio_variant_sha256: Some(
            primary_variant
                .identity_sha256()
                .ok_or("Primary variant is not stable")?,
        ),
    };
    let alternate_resource = AnalysisResource::Stream {
        recording_id: track.id.clone(),
        provider: provider.clone(),
        source_identity_sha256: digest,
        audio_variant_sha256: Some(
            alternate_variant
                .identity_sha256()
                .ok_or("Alternate variant is not stable")?,
        ),
    };
    let source_identity_digest = match &primary_resource {
        AnalysisResource::Stream {
            source_identity_sha256,
            ..
        } => source_identity_sha256.clone(),
        _ => unreachable!(),
    };
    match phase {
        Phase::Populate => {
            if observed.cached_resource(&primary_resource).is_some()
                || observed.cached_resource(&alternate_resource).is_some()
            {
                return Err("Populate profile unexpectedly contains either encoded variant".into());
            }
            let (primary_key, primary_duration) = analyze_remux(
                primary_file.path(),
                &primary.source.ffmpeg,
                &track,
                &source_id,
                &primary_variant,
                &store,
                &observed,
            )
            .await?;
            let (alternate_key, alternate_duration) = analyze_remux(
                alternate_file.path(),
                &alternate.source.ffmpeg,
                &track,
                &source_id,
                &alternate_variant,
                &store,
                &observed,
            )
            .await?;
            if primary_key == alternate_key
                || observed.cached_resource(&primary_resource).is_none()
                || observed.cached_resource(&alternate_resource).is_none()
            {
                return Err("Two distinct actual encoded variants did not persist as separate exact cache entries".into());
            }
            let after_count = store
                .list_tracks("library", None)
                .map_err(|_| "Could not inspect private library")?
                .len();
            if after_count != 0 {
                return Err("Qualification changed the private library".into());
            }
            let receipt = receipt_value(
                &source_identity_digest,
                &primary_variant,
                &primary_packet_sha,
                &alternate_variant,
                &alternate_packet_sha,
            )?;
            write_receipt(&profile, &receipt)?;
            println!("{}", serde_json::to_string_pretty(&json!({"phase":"populate","sourceIdentitySha256":source_identity_digest,"variants":[variant_summary("primary",&primary_variant,&primary_key,&primary_packet_sha,&primary_probe,primary_duration),variant_summary("alternate",&alternate_variant,&alternate_key,&alternate_packet_sha,&alternate_probe,alternate_duration)],"distinctVariantKeys":true,"distinctPacketHashes":true,"complete512BinAnalyses":true,"libraryTrackCountBefore":before_count,"libraryTrackCountAfter":after_count,"libraryUnchanged":true,"qualification":"same public recording; primary and production alternate format were each fetched, packetized, hashed, probed, decoded to natural EOF, analyzed, and stored under separate resolver-derived variant keys; no library row was inserted"})).map_err(|_| "Could not encode summary")?);
        }
        Phase::Restart => {
            let expected_receipt = receipt_value(
                &source_identity_digest,
                &primary_variant,
                &primary_packet_sha,
                &alternate_variant,
                &alternate_packet_sha,
            )?;
            if read_receipt(&profile)? != expected_receipt {
                return Err("Restart media packet hashes or resolver variant metadata changed since populate".into());
            }
            let primary_cached = observed
                .cached_resource(&primary_resource)
                .ok_or("Fresh process missed primary exact-format cache record")?;
            let alternate_cached = observed
                .cached_resource(&alternate_resource)
                .ok_or("Fresh process missed alternate exact-format cache record")?;
            for item in [&primary_cached, &alternate_cached] {
                item.validate()
                    .map_err(|_| "Restart cache record failed validation")?;
                item.waveform
                    .as_ref()
                    .ok_or("Restart cache record has no full waveform")?
                    .validate(item.duration_seconds)
                    .map_err(|_| "Restart waveform failed validation")?;
            }
            if primary_cached.resource == alternate_cached.resource {
                return Err("Different encoded formats aliased to one cache resource".into());
            }
            let after_count = store
                .list_tracks("library", None)
                .map_err(|_| "Could not inspect private library")?
                .len();
            if after_count != 0 {
                return Err("Qualification changed the private library".into());
            }
            println!("{}", serde_json::to_string_pretty(&json!({"phase":"restart","variants":[variant_summary("primary",&primary_variant,resource_audio_variant(&primary_resource).unwrap_or_default(),&primary_packet_sha,&primary_probe,primary_cached.duration_seconds),variant_summary("alternate",&alternate_variant,resource_audio_variant(&alternate_resource).unwrap_or_default(),&alternate_packet_sha,&alternate_probe,alternate_cached.duration_seconds)],"freshProcessCacheHitForBothVariants":true,"cacheEntriesRemainDistinct":true,"packetHashesAndResolverMetadataMatchPopulateReceipt":true,"libraryTrackCountBefore":before_count,"libraryTrackCountAfter":after_count,"libraryUnchanged":true,"qualification":"after restart, both exact production resolver variants matched their populate packet-payload hashes and variant metadata; both separate cache records were read and validated"})).map_err(|_| "Could not encode summary")?);
        }
    }
    Ok(())
}

fn variant_summary(
    label: &str,
    variant: &StreamAudioVariant,
    key: &str,
    packet_sha256: &str,
    probe: &Value,
    duration: f64,
) -> Value {
    json!({"label":label,"formatId":variant.format_id,"codecMetadata":variant.codec,"mimeType":variant.mime_type,"expectedContentLength":variant.expected_content_length,"audioVariantSha256":key,"encodedPacketSha256":packet_sha256,"probedCodec":probe["streams"][0]["codec_name"],"probedContainer":probe["format"]["format_name"],"containerDurationSeconds":probe["format"]["duration"],"decodedDurationSeconds":duration,"sampleRate":probe["streams"][0]["sample_rate"],"channels":probe["streams"][0]["channels"],"waveformBins":512,"waveformComplete":true})
}
