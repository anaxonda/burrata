//! Read-only public-network resolver diagnostic in a disposable Store.
//! cargo run --example playback_reliability -- --track PUBLIC.json --settings PUBLIC_SETTINGS.json --output REPORT.json
//! Default: 20 measured warm attempts after three warmups. Use --smoke --repetitions 1 --warmups 0 for one check.
//! Audio scope: production resolver + FFmpeg five-second PCM decode; no mixer/device/GUI.
use burrata_lib::{
    acquisition::AcquisitionManager,
    connections::ConnectionManager,
    models::{Candidate, Settings, Track},
    native_output::AudioSource,
    playback_sources,
    recording_match::MatchLevel,
    tools,
    youtube_support::YouTubeSupportManager,
    Store,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};
use tokio::{io::AsyncReadExt, sync::watch};

#[derive(Debug)]
struct Options {
    tracks: Vec<PathBuf>,
    settings: PathBuf,
    output: PathBuf,
    resources: Option<PathBuf>,
    repetitions: usize,
    warmups: usize,
    smoke: bool,
    scenario: String,
    expected_video_id: Option<String>,
    position: f64,
    prewarm_provider: bool,
}
impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, &'static str> {
        let mut args = args.into_iter();
        let (mut tracks, mut settings, mut output, mut resources) = (Vec::new(), None, None, None);
        let (mut repetitions, mut warmups, mut smoke) = (20, 3, false);
        let mut scenario = "warm".to_owned();
        let mut expected_video_id = None;
        let mut position: f64 = 0.;
        let mut prewarm_provider = false;
        while let Some(argument) = args.next() {
            if argument == "--prewarm-provider" {
                prewarm_provider = true;
                continue;
            }
            if argument == "--smoke" {
                smoke = true;
                continue;
            }
            let value = args.next().ok_or("MISSING_ARGUMENT_VALUE")?;
            match argument.as_str() {
                "--track" => tracks.push(PathBuf::from(value)),
                "--settings" => settings = Some(PathBuf::from(value)),
                "--output" => output = Some(PathBuf::from(value)),
                "--resources" => resources = Some(PathBuf::from(value)),
                "--repetitions" => {
                    repetitions = value.parse().map_err(|_| "INVALID_REPETITIONS")?
                }
                "--warmups" => warmups = value.parse().map_err(|_| "INVALID_WARMUPS")?,
                "--scenario" => scenario = value,
                "--position" => position = value.parse().map_err(|_| "INVALID_POSITION")?,
                "--expected-video-id" => expected_video_id = Some(value),
                _ => return Err("UNKNOWN_ARGUMENT"),
            }
        }
        if repetitions == 0 || repetitions > 1000 || (!smoke && repetitions < 20) {
            return Err("INVALID_REPETITIONS");
        }
        if warmups > 10 || (!smoke && warmups < 3) {
            return Err("INVALID_WARMUPS");
        }
        if !matches!(scenario.as_str(), "cold" | "warm" | "both" | "paired") {
            return Err("INVALID_SCENARIO");
        }
        if !position.is_finite() || position < 0. {
            return Err("INVALID_POSITION");
        }
        if expected_video_id.as_ref().is_some_and(|id: &String| {
            id.len() != 11
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }) {
            return Err("INVALID_EXPECTED_VIDEO_ID");
        }
        if tracks.len() > 200 {
            return Err("TOO_MANY_TRACKS");
        }
        if tracks.len() > 1 && expected_video_id.is_some() {
            return Err("EXPECTED_VIDEO_ID_REQUIRES_SINGLE_TRACK");
        }
        Ok(Self {
            tracks: if tracks.is_empty() {
                return Err("TRACK_REQUIRED");
            } else {
                tracks
            },
            settings: settings.ok_or("SETTINGS_REQUIRED")?,
            output: output.ok_or("OUTPUT_REQUIRED")?,
            resources,
            repetitions,
            warmups,
            smoke,
            scenario,
            expected_video_id,
            position,
            prewarm_provider,
        })
    }
}

/// Deliberately excludes music/library paths, connection credentials and profile settings.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct PublicSettings {
    resources: Option<PathBuf>,
    matching_level: Option<MatchLevel>,
    preferred_streaming_source: Option<String>,
    streaming_fallback_enabled: Option<bool>,
    streaming_fallback_sources: Option<Vec<String>>,
    streaming_fallback_delay_ms: Option<u32>,
    playback_startup_timeout_seconds: Option<u32>,
    yt_dlp_path: Option<String>,
    ffmpeg_path: Option<String>,
    ffprobe_path: Option<String>,
    js_runtime_path: Option<String>,
}
impl PublicSettings {
    fn apply(self) -> Result<(Settings, Option<PathBuf>), &'static str> {
        let mut settings = Settings::default();
        if let Some(value) = self.matching_level {
            settings.matching_level = value;
        }
        if let Some(value) = self.preferred_streaming_source {
            settings.preferred_streaming_source = value;
        }
        if let Some(value) = self.streaming_fallback_enabled {
            settings.streaming_fallback_enabled = value;
        }
        if let Some(value) = self.streaming_fallback_sources {
            settings.streaming_fallback_sources = value;
        }
        if let Some(value) = self.streaming_fallback_delay_ms {
            settings.streaming_fallback_delay_ms = value;
        }
        if let Some(value) = self.playback_startup_timeout_seconds {
            settings.playback_startup_timeout_seconds = value;
        }
        if let Some(value) = self.yt_dlp_path {
            settings.yt_dlp_path = value;
        }
        if let Some(value) = self.ffmpeg_path {
            settings.ffmpeg_path = value;
        }
        if let Some(value) = self.ffprobe_path {
            settings.ffprobe_path = value;
        }
        if let Some(value) = self.js_runtime_path {
            settings.js_runtime_path = value;
        }
        settings
            .validate_playback_startup_timeout()
            .map_err(|_| "INVALID_STARTUP_TIMEOUT")?;
        if ![settings.preferred_streaming_source.as_str()]
            .into_iter()
            .chain(
                settings
                    .streaming_fallback_sources
                    .iter()
                    .map(String::as_str),
            )
            .all(|provider| {
                matches!(
                    provider,
                    "youtube" | "soundcloud" | "monochrome" | "bandcamp"
                )
            })
        {
            return Err("INVALID_PUBLIC_PROVIDER");
        }
        Ok((settings, self.resources))
    }
}

struct Context {
    acquisition: AcquisitionManager,
    store: Arc<Store>,
    _directory: tempfile::TempDir,
}
fn context(settings: &Settings, track: &Track) -> Result<Context, &'static str> {
    let directory = tempfile::tempdir().map_err(|_| "SCRATCH_UNAVAILABLE")?;
    let store = Arc::new(
        Store::open(&directory.path().join("isolated.sqlite")).map_err(|_| "STORE_UNAVAILABLE")?,
    );
    store
        .save_settings(settings)
        .map_err(|_| "SETTINGS_UNAVAILABLE")?;
    // credential() checks this list before touching OS keyring. No keyring write/delete occurs.
    store
        .set_json_preference(
            "connections_session_only",
            &json!(["slskd", "discogs", "lastfm", "acoustid"]),
        )
        .map_err(|_| "CONNECTION_ISOLATION_FAILED")?;
    store.set_json_preference("connections", &json!({"slskd":{"enabled":false},"discogsEnabled":false,"lastfmEnabled":false,"acoustidEnabled":false}))
        .map_err(|_| "CONNECTION_ISOLATION_FAILED")?;
    store.save_track(track).map_err(|_| "TRACK_UNAVAILABLE")?;
    let connections = Arc::new(ConnectionManager::new(store.clone()));
    let acquisition = AcquisitionManager::new_for_diagnostics(store.clone(), connections);
    Ok(Context {
        acquisition,
        store,
        _directory: directory,
    })
}
fn validate_public_track(track: &Track) -> Result<(), &'static str> {
    if track.local_path.is_some()
        || track.asset_id.is_some()
        || track.playback_range.is_some()
        || track.preview_url.is_some()
    {
        return Err("PUBLIC_CATALOG_FIXTURE_REQUIRED");
    }
    let parsed =
        url::Url::parse(&track.source_url).map_err(|_| "PUBLIC_CATALOG_FIXTURE_REQUIRED")?;
    let host = parsed.host_str().unwrap_or("");
    if !matches!(parsed.scheme(), "https" | "http")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || ![
            "spotify.com",
            "youtube.com",
            "youtu.be",
            "soundcloud.com",
            "bandcamp.com",
            "tidal.com",
            "discogs.com",
            "musicbrainz.org",
            "beatport.com",
            "traxsource.com",
        ]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return Err("PUBLIC_CATALOG_FIXTURE_REQUIRED");
    }
    Ok(())
}
fn identity(candidate: &Candidate) -> Value {
    let metadata = candidate.source_metadata.as_ref();
    json!({"provider":candidate.provider,"providerId":candidate.id,"title":candidate.title,"uploader":candidate.uploader,
        "durationSeconds":candidate.duration_seconds,"official":candidate.official,"confidence":candidate.confidence,
        "channelId":metadata.and_then(|m| m["channelId"].as_str()),
        "channel":metadata.and_then(|m| m["channel"].as_str()),
        "track":metadata.and_then(|m| m["track"].as_str()),
        "artists":metadata.and_then(|m| m["artists"].as_array()).map(|values| values.iter().filter_map(Value::as_str).take(16).collect::<Vec<_>>())})
}
fn expected_video_matches(candidate: &Candidate, expected: &str) -> bool {
    candidate.provider == "youtube"
        && candidate
            .id
            .strip_prefix("youtube:")
            .unwrap_or(&candidate.id)
            == expected
}
fn error_category(error: &str) -> &'static str {
    let value = error.to_ascii_lowercase();
    if value.contains("429")
        || value.contains("rate limit")
        || value.contains("rate-limit")
        || value.contains("cooldown")
    {
        "RATE_LIMITED"
    } else if value.contains("cancel") {
        "CANCELLED"
    } else if ["deadline", "budget", "timed out", "timeout"]
        .iter()
        .any(|word| value.contains(word))
    {
        "TIMEOUT"
    } else if value.contains("match") || value.contains("identity") {
        "RECORDING_IDENTITY_UNVERIFIED"
    } else if value.contains("403") || value.contains("410") {
        "STREAM_DENIED"
    } else if ["runtime", "executable", "spawn"]
        .iter()
        .any(|word| value.contains(word))
    {
        "TOOL_UNAVAILABLE"
    } else {
        "SOURCE_UNAVAILABLE"
    }
}

#[derive(Clone, Copy)]
struct Usage {
    cpu_ms: f64,
    max_rss_bytes: u64,
}
#[cfg(unix)]
fn usage(children: bool) -> Option<Usage> {
    let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
    if unsafe {
        libc::getrusage(
            if children {
                libc::RUSAGE_CHILDREN
            } else {
                libc::RUSAGE_SELF
            },
            value.as_mut_ptr(),
        )
    } != 0
    {
        return None;
    }
    let value = unsafe { value.assume_init() };
    let scale = if cfg!(target_os = "macos") { 1 } else { 1024 };
    Some(Usage {
        cpu_ms: (value.ru_utime.tv_sec + value.ru_stime.tv_sec) as f64 * 1000.
            + (value.ru_utime.tv_usec + value.ru_stime.tv_usec) as f64 / 1000.,
        max_rss_bytes: value.ru_maxrss as u64 * scale,
    })
}
#[cfg(not(unix))]
fn usage(_children: bool) -> Option<Usage> {
    None
}
fn cpu_delta(before: Option<Usage>, after: Option<Usage>) -> Option<f64> {
    before
        .zip(after)
        .map(|(a, b)| (b.cpu_ms - a.cpu_ms).max(0.))
}

async fn decode(
    source: AudioSource,
    position: f64,
    deadline: tokio::time::Instant,
    mut cancel: watch::Receiver<bool>,
) -> Value {
    let started = Instant::now();
    let mut command = tokio::process::Command::new(source.ffmpeg);
    command.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-rw_timeout",
        "10000000",
    ]);
    if !source.headers.is_empty() {
        let headers = source
            .headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}\r\n"))
            .collect::<String>();
        command.arg("-headers").arg(headers);
    }
    if position > 0. {
        command.arg("-ss").arg(position.to_string());
    }
    command
        .arg("-i")
        .arg(source.input)
        .args([
            "-t",
            "5",
            "-vn",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return json!({"ok":false,"failure":"DECODER_UNAVAILABLE","stage":"decode"}),
    };
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped FFmpeg stdout");
    let mut bytes = 0usize;
    let mut first_pcm_ms = None;
    let work = async {
        let mut buffer = [0u8; 65536];
        loop {
            let count = stdout
                .read(&mut buffer)
                .await
                .map_err(|_| "DECODE_IO_FAILED")?;
            if count == 0 {
                break;
            }
            first_pcm_ms.get_or_insert_with(|| started.elapsed().as_secs_f64() * 1000.);
            bytes += count;
        }
        let status = child.wait().await.map_err(|_| "DECODE_WAIT_FAILED")?;
        if !status.success() {
            return Err("DECODE_FAILED");
        }
        if bytes < 240_000 * 8 {
            return Err("SHORT_AUDIO");
        }
        Ok(())
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, work) => result.unwrap_or(Err("TIMEOUT")),
        _ = cancel.changed() => Err("CANCELLED"),
    };
    if result.is_err() {
        #[cfg(unix)]
        if let Some(pid) = pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    json!({"ok":result.is_ok(),"failure":result.err(),"stage":"decode","firstPcmAfterResolveMs":first_pcm_ms,
        "decodedFrames":bytes/8,"decodedAudioMs":bytes as f64/384.,"decodeWallMs":started.elapsed().as_secs_f64()*1000.,
        "scope":"FFmpeg decoder only; no native mixer, audio device or GUI"})
}

async fn attempt(
    ctx: &Context,
    track: &Track,
    settings: &Settings,
    options: &Options,
    manager: &YouTubeSupportManager,
    cancel: watch::Receiver<bool>,
) -> Value {
    let started = Instant::now();
    let diagnostic_metrics_before = burrata_lib::diagnostic_metrics::snapshot();
    let deadline = tokio::time::Instant::now() + settings.playback_startup_timeout();
    let (own_before, children_before) = (usage(false), usage(true));
    // Playback-only experiment: overlap local helper loading with the resolver's
    // search. It does not mint a token or run for a standalone search request.
    let prewarm = if options.prewarm_provider {
        let manager = manager.clone();
        Some(tokio::spawn(async move {
            let started = Instant::now();
            let ready = tokio::time::timeout_at(deadline, manager.ensure_provider()).await;
            json!({"completed":true,"ok":matches!(ready,Ok(Ok(_))),"readyAfterMs":started.elapsed().as_secs_f64()*1000.})
        }))
    } else {
        None
    };

    let mut row = json!({"ok":false,"resolved":false,"positionSeconds":options.position,"budgetMs":settings.playback_startup_timeout().as_millis()});
    let native_operation_id = format!("diagnostic:{}", uuid::Uuid::new_v4());
    row["nativeOperationId"] = json!(native_operation_id);
    let result = tokio::time::timeout_at(
        deadline,
        playback_sources::resolve_with_trace_context(
            &ctx.store,
            &ctx.acquisition,
            track,
            None,
            cancel.clone(),
            Some(native_operation_id.clone()),
            None,
            None,
        ),
    )
    .await;
    let resolve_ms = started.elapsed().as_secs_f64() * 1000.;
    row["resolveMs"] = json!(resolve_ms);
    match result {
        Err(_) => {
            row["failure"] = json!("TIMEOUT");
            row["stage"] = json!("resolver");
        }
        Ok(Err(error)) => {
            row["failure"] = json!(error_category(&error));
            row["stage"] = json!("resolver");
        }
        Ok(Ok(resolved)) => {
            row["resolved"] = json!(true);
            row["resolvedRecordingIdentity"] = resolved
                .candidate
                .as_ref()
                .map(identity)
                .unwrap_or(Value::Null);
            let invariant = options.expected_video_id.as_ref().map(|expected| {
                resolved
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| expected_video_matches(candidate, expected))
            });
            row["explicitVideoIdInvariant"] = json!(invariant);
            let pcm = decode(resolved.source, options.position, deadline, cancel).await;
            row["firstPcmFromRequestMs"] = json!(pcm["firstPcmAfterResolveMs"]
                .as_f64()
                .map(|time| time + resolve_ms));
            row["ok"] = json!(pcm["ok"] == true && invariant != Some(false));
            if invariant == Some(false) {
                row["failure"] = json!("UNEXPECTED_SOURCE_ID");
                row["stage"] = json!("identity");
            } else if pcm["ok"] != true {
                row["failure"] = pcm["failure"].clone();
                row["stage"] = json!("decode");
            }
            row["decoder"] = pcm;
        }
    }
    if let Some(prewarm) = prewarm {
        if !prewarm.is_finished() {
            prewarm.abort();
        }
        row["providerPrewarm"] = prewarm.await.unwrap_or_else(|_|json!({"completed":false,"ok":false,"failure":"ABORTED_AFTER_FOREGROUND_COMPLETION"}));
    }
    row["wallMs"] = json!(started.elapsed().as_secs_f64() * 1000.);
    let (own_after, children_after) = (usage(false), usage(true));
    row["helperCpuMs"] = json!(cpu_delta(own_before, own_after));
    row["reapedChildCpuMs"] = json!(cpu_delta(children_before, children_after));
    row["helperMaxRssBytes"] = json!(own_after.map(|usage| usage.max_rss_bytes));
    row["reapedChildMaxRssBytes"] = json!(children_after.map(|usage| usage.max_rss_bytes));
    let diagnostic_metrics =
        burrata_lib::diagnostic_metrics::snapshot().delta_since(diagnostic_metrics_before);
    row["instrumentedMetrics"] = json!({
        "rustHttpSendAttempts":diagnostic_metrics.http_send_attempts,
        "successfulYtDlpSpawns":diagnostic_metrics.yt_dlp_successful_spawns,
        "matchPrepareElapsedMs":diagnostic_metrics.match_prepare_elapsed_ns as f64 / 1_000_000.,
        "matchAssessElapsedMs":diagnostic_metrics.match_assess_elapsed_ns as f64 / 1_000_000.,
        "matchPrepareCpuElapsedNs":diagnostic_metrics.match_prepare_cpu_elapsed_ns,
        "matchPrepareCpuObservations":diagnostic_metrics.match_prepare_cpu_observations,
        "matchAssessCpuElapsedNs":diagnostic_metrics.match_assess_cpu_elapsed_ns,
        "matchAssessCpuObservations":diagnostic_metrics.match_assess_cpu_observations,
        "scope":"process-window deltas around this serial diagnostic attempt; only enumerated native send sites, yt-dlp starts, and synchronous matching preparation/assessment are instrumented"
    });
    row["accountingScope"] = json!(
        "main helper CPU and reaped child CPU; persistent BgUtils CPU not yet reaped is excluded per attempt; lifetime RSS maxima are not additive"
    );
    row
}
fn summarize(rows: &[Value]) -> Vec<Value> {
    ["cold", "warm"].into_iter().filter_map(|scenario| {
        let group = rows.iter().filter(|row| row["scenario"] == scenario && row["warmup"] == false).collect::<Vec<_>>();
        if group.is_empty() { return None; }
        let successes=group.iter().filter(|row|row["ok"]==true).count();
        let mut failures=serde_json::Map::new();
        for row in group.iter().filter(|row|row["ok"]!=true) {
            let code=row["failure"].as_str().unwrap_or("UNKNOWN");
            let count=failures.get(code).and_then(Value::as_u64).unwrap_or(0)+1;
            failures.insert(code.to_owned(),json!(count));
        }
        let mut summary = json!({"scenario":scenario,"attempts":group.len(),"successes":successes,"failureCount":group.len()-successes,"failures":failures});
        for key in ["resolveMs", "firstPcmFromRequestMs", "wallMs", "helperCpuMs", "reapedChildCpuMs"] {
            let successful_pcm=key=="firstPcmFromRequestMs";
            let mut values=group.iter().filter(|row| !successful_pcm || row["ok"]==true)
                .filter_map(|row|row[key].as_f64()).filter(|value|value.is_finite()).collect::<Vec<_>>();
            values.sort_by(f64::total_cmp);
            let n=values.len();
            if n>0 {
                summary[key]=json!({"n":n,"cohort":if successful_pcm{"successful_attempts_only"}else{"all_measured_attempts"},
                    "p50":if n%2==0{(values[n/2-1]+values[n/2])/2.}else{values[n/2]},
                    "p95":if n>=20{Some(values[((n as f64*0.95).ceil()as usize)-1])}else{None},
                    "p95UnavailableReason":if n<20{Some("fewer_than_20_observations")}else{None},"worst":values[n-1]});
            }
        }
        Some(summary)
    }).collect()
}

fn save(path: &Path, report: &Value) -> Result<(), &'static str> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|_| "OUTPUT_UNAVAILABLE")?;
    }
    let bytes = serde_json::to_vec_pretty(report).map_err(|_| "REPORT_SERIALIZATION_FAILED")?;
    std::fs::write(path, bytes).map_err(|_| "OUTPUT_UNAVAILABLE")
}
async fn run(options: Options) -> Result<bool, &'static str> {
    let tracks = options
        .tracks
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|_| "TRACK_UNAVAILABLE")?;
            let track: Track = serde_json::from_slice(&bytes).map_err(|_| "INVALID_TRACK_JSON")?;
            validate_public_track(&track)?;
            Ok::<_, &'static str>((track, bytes))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let track = &tracks[0].0;
    let settings_bytes = std::fs::read(&options.settings).map_err(|_| "SETTINGS_UNAVAILABLE")?;
    let public: PublicSettings =
        serde_json::from_slice(&settings_bytes).map_err(|_| "INVALID_PUBLIC_SETTINGS_JSON")?;
    let (settings, from_settings) = public.apply()?;
    let resources = options
        .resources
        .clone()
        .or(from_settings)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
    let resources = resources
        .canonicalize()
        .map_err(|_| "RESOURCES_UNAVAILABLE")?;
    tools::initialize(resources.clone());
    let isolated = tempfile::tempdir().map_err(|_| "SCRATCH_UNAVAILABLE")?;
    let yt = tools::bundled_executable("yt-dlp")
        .unwrap_or_else(|| tools::executable(&settings.yt_dlp_path, "yt-dlp"));
    let node = tools::bundled_executable("node").or_else(|| {
        let path = tools::executable(&settings.js_runtime_path, "node");
        path.is_file().then_some(path)
    });
    let manager = YouTubeSupportManager::new(
        isolated.path().join("youtube-support"),
        yt,
        node,
        Some(resources.join("youtube-support/bgutil/server/build/main.js")),
        Some(resources.join("youtube-support/bgutil/plugin")),
    );
    burrata_lib::youtube_support::initialize_global(manager.clone())
        .map_err(|_| "PROVIDER_INITIALIZATION_FAILED")?;
    burrata_lib::diagnostic_metrics::enable();
    let (stop, cancel) = watch::channel(false);
    let interrupt = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop.send_replace(true);
    });
    let mut report = json!({"schema":"burrata.production-resolver-diagnostic/v1","qualification":false,"runPassed":false,
        "scope":"production resolver and FFmpeg five-second PCM only; no mixer, audio device, GUI, normal profile or credentials",
        "environmentIsolation":"process-only disposable XDG cache/config/data roots; HOME unchanged; all connection credentials session-only and empty",
        "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"build":if cfg!(debug_assertions){"debug"}else{"release"},
        "requestedRecordingIdentity":{"id":track.id,"provider":track.provider,"providerId":track.provider_id,"artist":track.artist,"title":track.title,"durationSeconds":track.duration_seconds},
        "requestedRecordingIdentities":tracks.iter().map(|(track,_)|json!({"id":track.id,"provider":track.provider,"providerId":track.provider_id,"artist":track.artist,"title":track.title,"durationSeconds":track.duration_seconds})).collect::<Vec<_>>(),
        "inputHashes":{"trackSha256":format!("{:x}",Sha256::digest(&tracks[0].1)),"trackSha256s":tracks.iter().map(|(_,bytes)|format!("{:x}",Sha256::digest(bytes))).collect::<Vec<_>>(),"publicSettingsSha256":format!("{:x}",Sha256::digest(&settings_bytes)),"exampleSourceSha256":format!("{:x}",Sha256::digest(include_bytes!("playback_reliability.rs")))},
        "batchScope":if tracks.len()>1{"sequential shared-process/store multi-track diagnostic; not concurrent playback"}else{"single-track diagnostic"},
        "externalRequestMeasurement":{"status":"partial_instrumentation","physicalOutboundRequestCount":null,"note":"instrumented Rust send invocations are listed in instrumentedMetricsCoverage; redirects, yt-dlp internal network activity and other uninstrumented modules remain opaque"},
        "instrumentedMetricsCoverage":burrata_lib::diagnostic_metrics::coverage(),
        "expectedVideoId":options.expected_video_id,"providerPrewarmEnabled":options.prewarm_provider,"smoke":options.smoke,"repetitions":options.repetitions,"warmupsPerScenario":options.warmups,
        "resolverBudgetMs":settings.playback_startup_timeout().as_millis(),"commandBudgetScope":"one shared absolute budget for resolver and five seconds PCM decode",
        "streamingPolicy":{"matchingLevel":settings.matching_level,"preferred":settings.preferred_streaming_source,"fallbackEnabled":settings.streaming_fallback_enabled,"fallbackSources":settings.streaming_fallback_sources},
        "coldScope":"new disposable Store/AcquisitionManager and stopped BgUtils helper; OS and packaged tool caches are not flushed",
        "warmScope":if options.scenario=="paired"{"same Store/AcquisitionManager immediately after its paired cold attempt"}else{"one reused Store/AcquisitionManager per warm scenario; warmups excluded"},
        "identityScope":"production resolver evidence gate; explicit video ID invariant only when supplied; no acoustic verification",
        "attempts":[],"batches":[],"unexpectedSourceIds":[],"summary":[]});
    let before_children = usage(true);
    let before_helper = usage(false);
    let mut rows = Vec::new();
    let operation: Result<(), &'static str> = async {
        let paired = options.scenario == "paired";
        let total = options.warmups + options.repetitions;
        let schedule: Vec<(&str, usize)> = if paired {
            (0..total)
                .flat_map(|index| [("cold", index), ("warm", index)])
                .collect()
        } else {
            ["cold", "warm"]
                .into_iter()
                .filter(|scenario| options.scenario == "both" || options.scenario == *scenario)
                .flat_map(|scenario| (0..total).map(move |index| (scenario, index)))
                .collect()
        };
        let mut retained: Option<Context> = None;
        for (scenario, index) in schedule {
            if *cancel.borrow() {
                report["cancelled"] = json!(true);
                break;
            }
            if paired && scenario == "cold" || !paired && scenario == "warm" && retained.is_none() {
                if paired {
                    manager
                        .shutdown()
                        .await
                        .map_err(|_| "PROVIDER_RESET_FAILED")?;
                }
                retained = Some(context(&settings, track)?);
            }
            let cold;
            let ctx = if paired || scenario == "warm" {
                retained.as_ref().ok_or("CONTEXT_UNAVAILABLE")?
            } else {
                manager
                    .shutdown()
                    .await
                    .map_err(|_| "PROVIDER_RESET_FAILED")?;
                cold = context(&settings, track)?;
                &cold
            };
            let batch_id = format!("{scenario}-{index}");
            let batch_started = Instant::now();
            let (batch_helper_before, batch_children_before) = (usage(false), usage(true));
            let batch_metrics_before = burrata_lib::diagnostic_metrics::snapshot();
            let batch_row_start = rows.len();
            for (entry_index, (entry, _)) in tracks.iter().enumerate() {
                ctx.store.save_track(entry).map_err(|_| "TRACK_UNAVAILABLE")?;
                let mut row = attempt(ctx, entry, &settings, &options, &manager, cancel.clone()).await;
                row["scenario"] = json!(scenario);
                row["matchingLevel"] = json!(settings.matching_level);
                row["batchId"] = json!(batch_id);
                row["batchEntryIndex"] = json!(entry_index);
                row["batchEntryCount"] = json!(tracks.len());
                if paired { row["pairIndex"] = json!(index); }
                row["warmup"] = json!(index < options.warmups);
                row["iteration"] = json!(index.saturating_sub(options.warmups));
                if row["failure"] == "UNEXPECTED_SOURCE_ID" {
                    report["unexpectedSourceIds"].as_array_mut().unwrap().push(row.clone());
                }
                rows.push(row);
            }
            let batch_metrics = burrata_lib::diagnostic_metrics::snapshot()
                .delta_since(batch_metrics_before);
            report["batches"].as_array_mut().unwrap().push(json!({
                "batchId":batch_id,"scenario":scenario,"pairIndex":if paired{Some(index)}else{None},
                "warmup":index<options.warmups,"entryCount":tracks.len(),"wallMs":batch_started.elapsed().as_secs_f64()*1000.,
                "successCount":rows[batch_row_start..].iter().filter(|row|row["ok"]==true).count(),
                "failureCount":rows[batch_row_start..].iter().filter(|row|row["ok"]!=true).count(),
                "helperCpuMs":cpu_delta(batch_helper_before,usage(false)),"reapedChildCpuMs":cpu_delta(batch_children_before,usage(true)),
                "helperMaxRssBytes":usage(false).map(|usage|usage.max_rss_bytes),"reapedChildMaxRssBytes":usage(true).map(|usage|usage.max_rss_bytes),
                "instrumentedMetrics":json!({"rustHttpSendAttempts":batch_metrics.http_send_attempts,"successfulYtDlpSpawns":batch_metrics.yt_dlp_successful_spawns,
                        "matchPrepareElapsedMs":batch_metrics.match_prepare_elapsed_ns as f64/1_000_000.,
                        "matchAssessElapsedMs":batch_metrics.match_assess_elapsed_ns as f64/1_000_000.,
                        "matchPrepareCpuElapsedNs":batch_metrics.match_prepare_cpu_elapsed_ns,
                        "matchPrepareCpuObservations":batch_metrics.match_prepare_cpu_observations,
                        "matchAssessCpuElapsedNs":batch_metrics.match_assess_cpu_elapsed_ns,
                        "matchAssessCpuObservations":batch_metrics.match_assess_cpu_observations,
                        "scope":"process-window delta for serial batch; counters cover only enumerated sites and are not physical HTTP request totals"}),
                "resourceScope":"batch cumulative CPU deltas; maxRSS is process lifetime high-water observed at batch end, not per-entry or additive"
            }));
            report["attempts"] = json!(rows);
            report["summary"] = json!(summarize(&rows));
            save(&options.output, &report)?;
            if rows
                .last()
                .is_some_and(|row| row["failure"] == "RATE_LIMITED")
            {
                report["stoppedEarly"] = json!("RATE_LIMITED");
                break;
            }
        }
        Ok(())
    }
    .await;
    interrupt.abort();
    let shutdown = manager.shutdown_for_exit().await;
    report["providerShutdownOk"] = json!(shutdown.is_ok());
    report["fullRunHelperCpuMs"] = json!(cpu_delta(before_helper, usage(false)));
    report["fullRunHelperMaxRssBytes"] = json!(usage(false).map(|usage| usage.max_rss_bytes));
    report["reapedChildrenIncludingProviderCpuMs"] = json!(cpu_delta(before_children, usage(true)));
    report["fullRunReapedChildMaxRssBytes"] = json!(usage(true).map(|usage| usage.max_rss_bytes));
    report["resourceAccountingNote"] = json!(
        "full-run reaped child CPU includes stopped BgUtils helpers, extractor runtimes and FFmpeg; lifetime maxRSS is a maximum rather than a simultaneous sum"
    );
    if let Err(code) = operation {
        report["harnessFailure"] = json!(code);
    }
    let expected = options.repetitions
        * if matches!(options.scenario.as_str(), "both" | "paired") {
            2
        } else {
            1
        };
    let measured = rows
        .iter()
        .filter(|r| r["warmup"] == false)
        .collect::<Vec<_>>();
    let passed = operation.is_ok()
        && shutdown.is_ok()
        && measured.len() == expected * tracks.len()
        && measured.iter().all(|r| r["ok"] == true);
    report["runPassed"] = json!(passed);
    save(&options.output, &report)?;
    println!(
        "{}",
        json!({"ok":passed,"qualification":false,"attempts":rows.len(),"schema":"burrata.production-resolver-diagnostic/v1"})
    );
    Ok(passed)
}
fn main() {
    if std::env::args().any(|arg| arg == "--help") {
        println!(
            "playback_reliability --track PUBLIC_TRACK.json --settings PUBLIC_SETTINGS.json --output REPORT.json [--resources DIR] [--repetitions 20] [--warmups 3] [--scenario cold|warm|both|paired] [--expected-video-id ID] [--position SECONDS] [--prewarm-provider] [--smoke]"
        );
        return;
    }
    // Set child-runtime cache/config/data roots before starting any async threads.
    // HOME and persistent system/account settings remain untouched.
    let result = (|| -> Result<bool, &'static str> {
        let options = Options::parse(std::env::args().skip(1))?;
        let diagnostic_environment = tempfile::tempdir().map_err(|_| "SCRATCH_UNAVAILABLE")?;
        for (variable, relative) in [
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
        ] {
            let path = diagnostic_environment.path().join(relative);
            std::fs::create_dir_all(&path).map_err(|_| "SCRATCH_UNAVAILABLE")?;
            std::env::set_var(variable, path);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| "RUNTIME_UNAVAILABLE")?;
        runtime.block_on(run(options))
    })();
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(code) => {
            eprintln!("{}", json!({"ok":false,"failure":code,"stage":"setup"}));
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(extra: &[&str]) -> Vec<String> {
        [
            "--track",
            "public.json",
            "--settings",
            "settings.json",
            "--output",
            "report.json",
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .map(String::from)
        .collect()
    }
    #[test]
    fn protocol_defaults_to_twenty_and_three_warmups() {
        let options = Options::parse(args(&[])).unwrap();
        assert_eq!(options.repetitions, 20);
        assert_eq!(options.warmups, 3);
        assert!(!options.smoke);
        assert!(!options.prewarm_provider);
        assert!(
            Options::parse(args(&["--prewarm-provider"]))
                .unwrap()
                .prewarm_provider
        );
    }
    #[test]
    fn repeated_track_arguments_select_shared_process_batch_mode() {
        let mut input = args(&[]);
        input.extend(["--track", "second.json"].into_iter().map(String::from));
        let options = Options::parse(input).unwrap();
        assert_eq!(
            options.tracks,
            [PathBuf::from("public.json"), PathBuf::from("second.json")]
        );
    }
    #[test]
    fn batch_limits_and_single_track_identity_expectation_are_enforced() {
        let mut with_expected_id = args(&["--expected-video-id", "abcdefghijk"]);
        with_expected_id.extend(["--track", "second.json"].into_iter().map(String::from));
        assert_eq!(
            Options::parse(with_expected_id).unwrap_err(),
            "EXPECTED_VIDEO_ID_REQUIRES_SINGLE_TRACK"
        );

        let mut too_many = args(&[]);
        for index in 0..200 {
            too_many.extend(["--track".to_owned(), format!("{index}.json")]);
        }
        assert_eq!(Options::parse(too_many).unwrap_err(), "TOO_MANY_TRACKS");
    }
    #[test]
    fn one_attempt_requires_explicit_smoke() {
        assert!(Options::parse(args(&["--repetitions", "1"])).is_err());
        assert!(Options::parse(args(&["--smoke", "--repetitions", "1", "--warmups", "0"])).is_ok());
    }
    #[test]
    fn settings_reject_profile_and_credential_fields() {
        assert!(serde_json::from_value::<PublicSettings>(
            json!({"musicDirectory":"/normal/profile"})
        )
        .is_err());
        assert!(serde_json::from_value::<PublicSettings>(json!({"token":"secret"})).is_err());
    }
    #[test]
    fn identity_retains_public_fields_without_urls_tokens_or_metadata_payloads() {
        let mut candidate:Candidate=serde_json::from_value(json!({"id":"abcdefghijk","provider":"youtube","url":"https://private.invalid/?token=secret","title":"Matthew","uploader":"John Denver - Topic","durationSeconds":222,"artworkUrl":null,"formats":[],"permission":"unknown","confidence":"high","score":1,"reasons":["secret"],"official":true,"sourceMetadata":{"poToken":"secret"}})).unwrap();
        let output = identity(&candidate).to_string();
        assert!(output.contains("Matthew"));
        assert!(!output.contains("secret"));
        assert!(!output.contains("private.invalid"));
        candidate.id = "youtube:abcdefghijk".into();
        assert!(expected_video_matches(&candidate, "abcdefghijk"));
        assert!(!expected_video_matches(&candidate, "zzzzzzzzzzz"));
    }
    #[test]
    fn summaries_exclude_warmups_and_preserve_failed_attempt_count() {
        let rows = vec![
            json!({"scenario":"warm","warmup":true,"ok":true,"resolveMs":999}),
            json!({"scenario":"warm","warmup":false,"ok":true,"resolveMs":2}),
            json!({"scenario":"warm","warmup":false,"ok":false,"resolveMs":8}),
        ];
        let output = summarize(&rows);
        assert_eq!(output[0]["attempts"], 2);
        assert_eq!(output[0]["successes"], 1);
        assert_eq!(output[0]["resolveMs"]["worst"], 8.);
        assert_eq!(output[0]["resolveMs"]["p50"], 5.);
        assert_eq!(output[0]["resolveMs"]["n"], 2);
        assert_eq!(output[0]["failureCount"], 1);
        assert!(output[0]["resolveMs"]["p95"].is_null());
    }
    #[test]
    fn all_matching_modes_are_explicit_and_unknown_settings_stay_rejected() {
        for (name, level) in [
            ("exact", MatchLevel::Exact),
            ("balanced", MatchLevel::Balanced),
            ("flexible", MatchLevel::Flexible),
        ] {
            let settings = serde_json::from_value::<PublicSettings>(json!({"matchingLevel":name}))
                .unwrap()
                .apply()
                .unwrap()
                .0;
            assert_eq!(settings.matching_level, level);
        }
        assert!(
            serde_json::from_value::<PublicSettings>(json!({"matchingLevel":"automatic"})).is_err()
        );
        assert!(serde_json::from_value::<PublicSettings>(
            json!({"matchingLevel":"exact","credentials":{}})
        )
        .is_err());
    }
    #[test]
    fn successful_pcm_has_its_own_denominator_and_small_samples_have_no_p95() {
        let rows = vec![
            json!({"scenario":"cold","warmup":false,"ok":true,"resolveMs":2,"wallMs":4,"firstPcmFromRequestMs":3}),
            json!({"scenario":"cold","warmup":false,"ok":false,"resolveMs":20,"wallMs":21,"firstPcmFromRequestMs":19,"failure":"DECODE_FAILED"}),
        ];
        let summary = summarize(&rows);
        assert_eq!(summary[0]["wallMs"]["p50"], 12.5);
        assert_eq!(summary[0]["firstPcmFromRequestMs"]["n"], 1);
        assert_eq!(summary[0]["firstPcmFromRequestMs"]["p50"], 3.);
        assert!(summary[0]["firstPcmFromRequestMs"]["p95"].is_null());
        let large=(0..20).map(|index|json!({"scenario":"cold","warmup":false,"ok":false,"resolveMs":index,"failure":"TIMEOUT"})).collect::<Vec<_>>();
        assert_eq!(summarize(&large)[0]["resolveMs"]["p95"], 18.);
    }
}
