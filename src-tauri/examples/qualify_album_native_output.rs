//! Actual native-output qualification for an explicitly ordered synthetic album.
//!
//! This runs the production selected-album analyzer and cache writer, reloads
//! the exact typed programme record, checks its local seals, applies the shared
//! album policy, then sends both real fixture files through NativeOutput using
//! the same gain on the initial Load and gapless Preload. A profile-local
//! ALSA_CONFIG_PATH must route the process to the harness-owned Pulse sink.
//!
//! Run only in a disposable first-four synthetic profile through
//! scripts/performance/run_album_native_output_case.py.

use burrata_lib::{
    loudness::{LoudnessResourceIdentity, NormalizationMode},
    loudness_manager::{self, LocalVerificationMemo, LoudnessStatusKind},
    native_output::{AudioSource, NativeOutput, OutputCommand, OutputEvent},
    tools, Store,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    profile_root: PathBuf,
    app_data_directory: PathBuf,
    synthetic_only: bool,
    external_audio_used: bool,
    tracks: Vec<InventoryTrack>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InventoryTrack {
    id: String,
    asset_id: String,
    audio_path: PathBuf,
    sha256: String,
}

fn fail(message: impl AsRef<str>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, message.as_ref().to_owned())
}

fn synthetic_profile(path: &Path) -> Result<(PathBuf, Inventory), Box<dyn std::error::Error>> {
    let profile = path.canonicalize()?;
    if profile.parent() != Some(Path::new("/tmp"))
        || !profile
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("burrata-first-four-profile-"))
    {
        return Err(
            fail("profile must be a dedicated first-four fixture profile under /tmp").into(),
        );
    }
    if fs::read_to_string(profile.join(".burrata-qualification-profile"))?.trim()
        != "synthetic fixture only"
    {
        return Err(fail("synthetic qualification marker is missing").into());
    }
    let inventory: Inventory =
        serde_json::from_slice(&fs::read(profile.join("qualification-inventory.json"))?)?;
    if !inventory.synthetic_only
        || inventory.external_audio_used
        || inventory.profile_root.canonicalize()? != profile
        || inventory.app_data_directory.canonicalize()? != profile.join("data").canonicalize()?
    {
        return Err(
            fail("profile inventory is not synthetic-only or does not match this profile").into(),
        );
    }
    if inventory.tracks.len() != 2 {
        return Err(fail("album qualification requires exactly two ordered fixture tracks").into());
    }
    let media_root = profile.join("media").canonicalize()?;
    let mut ids = std::collections::HashSet::new();
    for track in &inventory.tracks {
        let path = track.audio_path.canonicalize()?;
        if !path.starts_with(&media_root)
            || !track.id.starts_with("local:")
            || !ids.insert(track.asset_id.clone())
        {
            return Err(fail("fixture order contains a repeated or out-of-profile asset").into());
        }
    }
    Ok((profile, inventory))
}

fn local_source(path: &Path, ffmpeg: &Path) -> AudioSource {
    AudioSource {
        input: path.to_string_lossy().into_owned(),
        ffmpeg: ffmpeg.to_path_buf(),
        headers: Vec::new(),
        decoder_hint: None,
        cenc: None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let profile_arg = env::args_os()
        .nth(1)
        .ok_or_else(|| fail("usage: qualify_album_native_output PROFILE"))?;
    let (profile, inventory) = synthetic_profile(Path::new(&profile_arg))?;
    let qualification_case = env::var("BURRATA_QUAL_CASE")
        .map_err(|_| fail("BURRATA_QUAL_CASE must be off, normalized, or analyze-only"))?;
    let analyze_only = qualification_case == "analyze-only";
    let mode = match qualification_case.as_str() {
        "off" => NormalizationMode::Off,
        "normalized" | "analyze-only" => NormalizationMode::Album,
        _ => return Err(fail("BURRATA_QUAL_CASE must be off, normalized, or analyze-only").into()),
    };

    let store = Arc::new(Store::open(&profile.join("data/burrata.sqlite")).map_err(fail)?);
    let mut assets_by_id = HashMap::new();
    for asset in store
        .list_assets_by_ids(
            &inventory
                .tracks
                .iter()
                .map(|track| track.asset_id.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(fail)?
    {
        assets_by_id.insert(asset.id.clone(), asset);
    }
    let mut ordered_assets = Vec::with_capacity(2);
    for item in &inventory.tracks {
        let asset = assets_by_id
            .remove(&item.asset_id)
            .ok_or_else(|| fail("ordered synthetic asset is no longer registered"))?;
        if asset.hash != item.sha256
            || Path::new(&asset.path).canonicalize()? != item.audio_path.canonicalize()?
        {
            return Err(
                fail("registered synthetic asset no longer matches its inventory seal").into(),
            );
        }
        ordered_assets.push(asset);
    }

    let asset_ids = ordered_assets
        .iter()
        .map(|asset| asset.id.clone())
        .collect();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let store_for_analysis = store.clone();
    let ffmpeg_settings = store.settings().map_err(fail)?;
    let ffmpeg = tools::executable(&ffmpeg_settings.ffmpeg_path, "ffmpeg");
    let ffprobe = tools::executable(&ffmpeg_settings.ffprobe_path, "ffprobe");
    let analysis = loudness_manager::analyze_selected_album(
        store_for_analysis,
        asset_ids,
        vec![0.0],
        ffmpeg.clone(),
        ffprobe,
        cancel_rx,
    )
    .await
    .map_err(fail)?;
    drop(cancel_tx);

    let resource: LoudnessResourceIdentity = analysis.resource.clone();
    resource
        .validate()
        .map_err(|_| fail("analyzer returned an invalid album resource"))?;
    if resource.scope != burrata_lib::loudness::ResourceScope::Album
        || resource.inter_track_gaps_seconds != [0.0]
        || resource.members.len() != 2
        || resource
            .members
            .iter()
            .zip(&ordered_assets)
            .any(|(member, asset)| member.asset_id != asset.id || member.file_hash != asset.hash)
    {
        return Err(
            fail("analyzer record does not bind the exact ordered gapless fixtures").into(),
        );
    }
    let cached = loudness_manager::cached_record(&store, &resource, None)
        .map_err(fail)?
        .ok_or_else(|| fail("album record was not written to the production analysis cache"))?;
    if cached.resource != resource {
        return Err(fail("cached album record changed its ordered resource identity").into());
    }
    let verification = LocalVerificationMemo::default();
    if !verification
        .is_currently_verified(&store, &resource, Some(&cached))
        .map_err(fail)?
    {
        return Err(fail("current local file seals do not verify the analyzed album").into());
    }

    let mut settings = loudness_manager::load_settings(&store).map_err(fail)?;
    settings.mode = mode;
    loudness_manager::save_settings(&store, &settings).map_err(fail)?;
    let decision = loudness_manager::decide_for_measurements(
        &settings,
        None,
        Some(&cached),
        Some(resource.resource_key.clone()),
    );
    if (mode == NormalizationMode::Album && decision.status != LoudnessStatusKind::Applied)
        || (mode == NormalizationMode::Off && decision.status != LoudnessStatusKind::Off)
    {
        return Err(
            fail("shared loudness policy did not produce the expected Album/Off decision").into(),
        );
    }

    if analyze_only {
        println!(
            "{}",
            json!({
                "status": "analyzed",
                "mode": "album",
                "resource": resource.clone(),
                "analysis": {
                    "integratedLufs": cached.integrated_lufs,
                    "truePeakDbtp": cached.true_peak_dbtp,
                    "analyzerVersion": cached.analyzer_version,
                    "provenance": cached.provenance,
                    "localSealsVerified": true
                },
                "decision": decision,
                "orderedAssetIds": resource.members.iter().map(|member| &member.asset_id).collect::<Vec<_>>(),
                "unchangedAudioFiles": true
            })
        );
        return Ok(());
    }

    let sink = env::var("BURRATA_QUAL_SINK")
        .map_err(|_| fail("qualification harness did not supply its private sink"))?;
    let user_volume = env::var("BURRATA_QUAL_USER_VOLUME")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.36);
    if !user_volume.is_finite() || !(0.0..=1.0).contains(&user_volume) {
        return Err(fail("qualification volume must be finite and between zero and one").into());
    }
    let alsa_config = env::var_os("ALSA_CONFIG_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| fail("profile-local ALSA_CONFIG_PATH is required"))?;
    if !alsa_config.starts_with(&profile) || !alsa_config.is_file() {
        return Err(fail("ALSA_CONFIG_PATH must be a file inside the synthetic profile").into());
    }

    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let output = NativeOutput::new(events_tx).map_err(fail)?;
    let gain_db = decision.applied_gain_db as f32;
    output.send(OutputCommand::Volume(0.0)).map_err(fail)?;
    let started_at = Instant::now();
    output
        .send(OutputCommand::LoadWithNormalization {
            generation: 1,
            startup_deadline: Instant::now() + Duration::from_secs(20),
            allow_startup_reserve: false,
            source: local_source(Path::new(&ordered_assets[0].path), &ffmpeg),
            position: 0.0,
            end: None,
            initial_gain_db: gain_db,
        })
        .map_err(fail)?;
    output
        .send(OutputCommand::PreloadWithNormalization {
            generation: 1,
            next_generation: 2,
            source: local_source(Path::new(&ordered_assets[1].path), &ffmpeg),
            position: 0.0,
            end: None,
            initial_gain_db: gain_db,
        })
        .map_err(fail)?;
    output.send(OutputCommand::Pause).map_err(fail)?;

    #[cfg(target_os = "linux")]
    {
        println!(
            "{}",
            json!({
                "status": "waiting_for_route_guard",
                "pid": std::process::id(),
                "executable": env::current_exe()?.display().to_string(),
                "profile": profile,
                "alsaConfig": alsa_config,
                "privatePulseSink": sink,
                "mode": if mode == NormalizationMode::Off { "off" } else { "album" },
                "resource": resource,
                "decision": decision,
                "sourcePositionSeconds": 0.0,
                "initialVolume": 0.0
            })
        );
        io::stdout().flush()?;
        let mut route_guard_signal = signal(SignalKind::user_defined1())?;
        route_guard_signal
            .recv()
            .await
            .ok_or_else(|| fail("qualification route-guard signal was lost"))?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        return Err(
            fail("this native output qualification controller currently requires Linux").into(),
        );
    }

    // The controller sends SIGUSR1 only after verifying this exact process and
    // the owned sink. Restart from source position zero while still silent,
    // then set app volume and resume with the gain already present from Load.
    output
        .send(OutputCommand::Seek {
            generation: 1,
            position: 0.0,
        })
        .map_err(fail)?;
    output
        .send(OutputCommand::PreloadWithNormalization {
            generation: 1,
            next_generation: 2,
            source: local_source(Path::new(&ordered_assets[1].path), &ffmpeg),
            position: 0.0,
            end: None,
            initial_gain_db: gain_db,
        })
        .map_err(fail)?;
    output
        .send(OutputCommand::Volume(user_volume))
        .map_err(fail)?;
    output.send(OutputCommand::Resume).map_err(fail)?;
    let resume_command_sent_ms = started_at.elapsed().as_millis();

    let mut first_started_ms = None;
    let mut second_started_ms = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(70);
    loop {
        let event = tokio::time::timeout_at(deadline, events_rx.recv())
            .await
            .map_err(|_| fail("native album output exceeded the 70-second bound"))?
            .ok_or_else(|| fail("native output event stream stopped"))?;
        match event {
            OutputEvent::Started { generation: 1, .. } => {
                first_started_ms.get_or_insert(started_at.elapsed().as_millis());
            }
            OutputEvent::Started { generation: 2, .. } => {
                second_started_ms.get_or_insert(started_at.elapsed().as_millis());
            }
            OutputEvent::Failed {
                generation,
                message,
            } => {
                return Err(fail(format!(
                    "native output generation {generation} failed: {message}"
                ))
                .into());
            }
            OutputEvent::LocalStorageFailed { generation, .. } => {
                return Err(fail(format!(
                    "native output generation {generation} lost local storage"
                ))
                .into());
            }
            OutputEvent::DeviceLost { reason, .. } => {
                return Err(fail(format!("native output device was lost: {reason}")).into());
            }
            OutputEvent::Ended { generation: 2 } => break,
            _ => {}
        }
    }
    let second_started_ms =
        second_started_ms.ok_or_else(|| fail("gapless preload did not activate generation 2"))?;
    if first_started_ms.is_none() {
        return Err(fail("first album track did not start").into());
    }
    println!(
        "{}",
        json!({
            "status": "completed",
            "scope": "synthetic two-track ordered album, production FFmpeg EBU analysis and native Load/Preload output",
            "mode": if mode == NormalizationMode::Off { "off" } else { "album" },
            "privatePulseSink": sink,
            "appVolume": user_volume,
            "resource": resource.clone(),
            "analysis": {
                "integratedLufs": cached.integrated_lufs,
                "truePeakDbtp": cached.true_peak_dbtp,
                "analyzerVersion": cached.analyzer_version,
                "provenance": cached.provenance,
                "localSealsVerified": true
            },
            "decision": decision,
            "orderedTracks": inventory.tracks.iter().map(|item| &item.id).collect::<Vec<_>>(),
            "nativeOutput": {
                "generation1StartedMs": first_started_ms,
                "generation2StartedMs": second_started_ms,
                "resumeCommandSentMs": resume_command_sent_ms,
                "completedElapsedMs": started_at.elapsed().as_millis(),
                "initialGainDb": gain_db,
                "preloadGainDb": gain_db,
                "gaplessResourceBoundarySeconds": resource.inter_track_gaps_seconds.clone(),
                "queueTransition": "preloaded next local source adopted by NativeOutput"
            },
            "unchangedAudioFiles": true
        })
    );
    io::stdout().flush()?;
    Ok(())
}
