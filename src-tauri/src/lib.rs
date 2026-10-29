#![recursion_limit = "256"]

// Tauri embeds its manifest in binaries, but not the library unit-test runner.
// Use the same generated resource so Windows selects Common Controls v6 before
// resolving TaskDialogIndirect. This does not affect production library builds.
#[cfg(all(test, target_os = "windows", target_env = "msvc"))]
#[link(name = "resource", kind = "static", modifiers = "-bundle")]
extern "C" {}

pub mod acquisition;
mod album_excerpt_audio;
pub mod analysis;
pub mod analysis_selection;
pub mod analysis_stream_selection;
mod artist_alias_discovery;
mod artist_alias_lookup;
mod artist_credit_aliases;
pub mod artists;
mod artwork_cache;
mod artwork_display;
mod artwork_protocol_trace;
mod artwork_resource_probe;
pub mod automation;
pub mod autotag;
pub mod backup;
pub mod backup_archive;
pub mod backup_auxiliary;
pub(crate) mod backup_frontend_preferences;
pub mod backup_manager;
pub mod backup_staging;
pub mod backup_state;
pub mod bandcamp;
mod bandcamp_fallback;
pub mod browser_bridge;
pub mod candidate_preview;
mod collection_imports;
pub mod collection_sources;
mod compilation_excerpt;
pub mod connections;
pub mod diagnostic_metrics;
mod directory_browser;
pub mod discovery;
mod download_verification;
pub mod external_player;
mod import_drop;
mod import_metadata;
pub mod imports;
pub mod install_integration;
#[cfg(target_os = "linux")]
pub mod linux_webview_output;
pub(crate) mod live_analysis_gate;
pub mod live_bpm;
pub mod live_key;
pub mod local_audio;
mod local_lyrics;
pub mod loudness;
pub mod loudness_analysis;
pub mod loudness_manager;
pub mod lyrics;
mod lyrics_aliases;
mod lyrics_discovery;
pub mod lyrics_translation;
mod main_ui_capture;
pub mod managed_provider_session;
mod managed_slskd_settings;
pub mod matching;
pub mod media;
pub mod metadata;
pub mod metadata_normalization;
pub mod models;
pub mod monochrome;
pub(crate) mod monochrome_playback;
pub mod monochrome_resolver;
#[cfg(target_os = "linux")]
pub mod mpris;
pub mod native_output;
pub mod observed_analysis;
mod opus_decoder_capability;
#[cfg(feature = "experimental-opus")]
mod opus_experiment;
pub mod pcm_tap;
pub mod playback;
pub mod playback_diagnostics;
mod playback_diagnostics_context;
mod playback_engine_settings;
pub mod playback_failure;
pub mod playback_history;
pub mod playback_radio;
pub mod playback_sources;
pub mod playing_track_detections;
mod playing_track_identification;
pub mod playlist_files;
pub mod playlist_hierarchy;
mod playlist_incremental;
pub mod playlist_mirror_options;
pub mod playlist_preferences;
pub mod playlist_sync;
pub mod playlist_watch;
pub mod portable_playlist;
pub mod provider_read_requests;
pub mod providers;
mod queued_playback;
pub mod radio;
mod recognition_input;
pub mod recording_filename;
pub mod recording_identity;
pub mod recording_match;
mod recording_match_aliases;
mod recording_match_context;
#[cfg(test)]
mod recording_match_corpus;
pub mod remote_playlists;
pub mod restore_work_policy;
mod shutdown_signal;
pub mod sidebar_layout;
pub mod slskd;
pub mod source_fallback;
mod source_preparation;
mod spotify_mb_crosswalk;
mod staged_recognition;
pub mod storage;
pub mod stream_analysis;
pub mod system_media;
pub mod tools;
pub mod track_sort_sql;
pub mod track_sort_text;
mod verification_staging;
pub mod waveform_analysis;
mod webview_audio_transcode;
mod webview_audio_transport;
pub mod webview_identification;
pub mod webview_live_bpm;
pub mod webview_radio_pcm;
mod youtube_direct_experiment;
mod youtube_download_recovery;
pub mod youtube_music;
pub mod youtube_support;
mod youtube_web_search;
use acquisition::{AcquisitionManager, AcquisitionProvider};
use models::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub use storage::Store;
use tauri::{Emitter, Manager, State};
type Result<T> = std::result::Result<T, String>;

fn release_resource_root(app: &tauri::AppHandle) -> Result<PathBuf> {
    let root = app.path().resource_dir().map_err(|e| e.to_string())?;
    Ok(if root.join("resources").is_dir() {
        root.join("resources")
    } else {
        root
    })
}
#[tauri::command]
fn get_release_setup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value> {
    let settings = state.store.settings()?;
    let paths = [
        ("yt-dlp", settings.yt_dlp_path.as_str()),
        ("ffmpeg", settings.ffmpeg_path.as_str()),
        ("ffprobe", settings.ffprobe_path.as_str()),
        ("node", settings.js_runtime_path.as_str()),
    ]
    .map(|(name, configured)| {
        let path = tools::executable(configured, name);
        let origin = if configured.starts_with("system:") {
            "system"
        } else if Path::new(configured).is_absolute() {
            "custom"
        } else if tools::bundled_executable(name).as_ref() == Some(&path) {
            "bundled"
        } else {
            "system"
        };
        serde_json::json!({"name":name,"path":path,"origin":origin})
    });
    let mcp = tools::bundled_executable("burrata-mcp").unwrap_or(
        release_resource_root(&app)?
            .join("tools")
            .join(tools::target_triple())
            .join(if cfg!(windows) {
                "burrata-mcp.exe"
            } else {
                "burrata-mcp"
            }),
    );
    Ok(
        serde_json::json!({"setupComplete":state.store.get_json_preference("releaseSetupComplete.v1")?.unwrap_or(serde_json::json!(false)),"musicDirectory":settings.music_directory,"radioRecognition":state.store.get_json_preference("radioRecognitionEnabled.v1")?.unwrap_or(serde_json::json!(true)),"mcpCommand":[mcp],"browser":install_integration::status(),"toolPaths":paths}),
    )
}
#[tauri::command]
fn complete_release_setup(state: State<'_, AppState>, radio_recognition: bool) -> Result<()> {
    state.store.set_json_preference(
        "radioRecognitionEnabled.v1",
        &serde_json::json!(radio_recognition),
    )?;
    state
        .store
        .set_json_preference("releaseSetupComplete.v1", &serde_json::json!(true))
}
#[tauri::command]
fn get_radio_recognition(state: State<'_, AppState>) -> Result<bool> {
    Ok(state
        .store
        .get_json_preference("radioRecognitionEnabled.v1")?
        .and_then(|value| value.as_bool())
        .unwrap_or(true))
}
#[tauri::command]
fn set_radio_recognition(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    state
        .store
        .set_json_preference("radioRecognitionEnabled.v1", &serde_json::json!(enabled))
}
#[tauri::command]
async fn choose_music_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Choose your music folder")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let probe = path.join(format!(".burrata-write-check-{}", uuid::Uuid::new_v4()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("This folder is not writable: {e}"))?;
    std::fs::remove_file(&probe).map_err(|e| e.to_string())?;
    let mut settings = state.store.settings()?;
    let directory = path
        .to_str()
        .ok_or("Folder path is not valid UTF-8")?
        .to_owned();
    settings.music_directory = directory.clone();
    save_settings(app, state, settings)?;
    Ok(Some(directory))
}
#[tauri::command]
fn get_browser_install_status() -> install_integration::InstallStatus {
    install_integration::status()
}
#[tauri::command]
fn register_browser_host(app: tauri::AppHandle) -> Result<install_integration::InstallStatus> {
    install_integration::register(&release_resource_root(&app)?)
}
#[tauri::command]
fn unregister_browser_host() -> Result<install_integration::InstallStatus> {
    install_integration::unregister()
}
fn youtube_manager() -> Result<&'static youtube_support::YouTubeSupportManager> {
    youtube_support::global().ok_or("YouTube support is not initialized".into())
}
#[tauri::command]
async fn youtube_support_status() -> Result<youtube_support::SupportStatus> {
    Ok(youtube_manager()?.status().await)
}
#[tauri::command]
async fn youtube_support_check(channel: Option<String>) -> Result<youtube_support::UpdateCheck> {
    match channel.as_deref().unwrap_or("recommended") {
        "recommended" => Ok(youtube_manager()?.check_recommended_update().await),
        "nightly" => youtube_manager()?.check_advanced_update().await,
        _ => Err("Choose recommended or nightly".into()),
    }
}
#[tauri::command]
async fn youtube_support_update(
    state: State<'_, AppState>,
    channel: Option<String>,
) -> Result<youtube_support::SupportStatus> {
    if state
        .store
        .list_jobs()?
        .iter()
        .any(|j| !matches!(j.state.as_str(), "completed" | "failed" | "cancelled"))
    {
        return Err("Finish or cancel active downloads before changing YouTube support".into());
    }
    match channel.as_deref().unwrap_or("recommended") {
        "recommended" => youtube_manager()?.update_recommended().await,
        "nightly" => youtube_manager()?.update_advanced().await,
        _ => Err("Choose recommended or nightly".into()),
    }
}
#[tauri::command]
async fn youtube_support_rollback(
    state: State<'_, AppState>,
) -> Result<youtube_support::SupportStatus> {
    if state
        .store
        .list_jobs()?
        .iter()
        .any(|j| !matches!(j.state.as_str(), "completed" | "failed" | "cancelled"))
    {
        return Err("Finish or cancel active downloads before changing YouTube support".into());
    }
    youtube_manager()?.rollback().await
}
#[tauri::command]
async fn youtube_support_diagnose(
    url: Option<String>,
) -> Result<youtube_support::SupportDiagnosis> {
    Ok(youtube_manager()?.diagnose_url(url.as_deref()).await)
}

#[tauri::command]
fn get_radio_stations(state: State<'_, AppState>) -> Result<Vec<radio::RadioStation>> {
    radio::get_radio_stations_saved(&state.store)
}
#[tauri::command]
async fn search_radio_stations(
    query: String,
    country: Option<String>,
    tag: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<radio::RadioStation>> {
    radio::search_radio_stations(query, country, tag, limit).await
}
#[tauri::command]
fn add_radio_station(
    state: State<'_, AppState>,
    station: radio::RadioStation,
) -> Result<Vec<radio::RadioStation>> {
    radio::add_radio_station(station, &state.store)
}
#[tauri::command]
async fn add_custom_radio_station(
    state: State<'_, AppState>,
    name: String,
    url: String,
) -> Result<Vec<radio::RadioStation>> {
    radio::add_custom_radio_station(name, url, &state.store).await
}
#[tauri::command]
async fn edit_radio_station_url(
    state: State<'_, AppState>,
    station_id: String,
    url: String,
) -> Result<Vec<radio::RadioStation>> {
    radio::edit_radio_station_url(station_id, url, &state.store).await
}
#[tauri::command]
fn list_radio_playlist_targets(state: State<'_, AppState>) -> Result<Vec<Collection>> {
    radio::list_radio_playlist_targets(&state.store)
}
#[tauri::command]
fn get_radio_playlist_option(
    state: State<'_, AppState>,
    station_id: String,
) -> Result<radio::RadioPlaylistOption> {
    radio::get_radio_playlist_option(&state.store, &station_id)
}
#[tauri::command]
fn set_radio_playlist_option(
    state: State<'_, AppState>,
    station_id: String,
    enabled: bool,
    collection_id: Option<String>,
) -> Result<radio::RadioPlaylistOption> {
    radio::set_radio_playlist_option(&state.store, &station_id, enabled, collection_id.as_deref())
}
#[tauri::command]
fn remove_radio_station(
    state: State<'_, AppState>,
    station_id: String,
) -> Result<Vec<radio::RadioStation>> {
    radio::remove_radio_station(station_id, &state.store)
}
#[tauri::command]
fn rename_radio_station(
    state: State<'_, AppState>,
    station_id: String,
    name: String,
) -> Result<Vec<radio::RadioStation>> {
    radio::rename_radio_station(station_id, name, &state.store)
}
#[tauri::command]
async fn resolve_radio_station_artwork(
    state: State<'_, AppState>,
    station_id: String,
) -> Result<Option<String>> {
    radio::resolve_station_artwork(station_id, &state.store).await
}

#[tauri::command]
async fn resolve_radio_station_playback_url(
    state: State<'_, AppState>,
    station_id: String,
    request_generation: Option<u64>,
    webview_capabilities: Option<webview_audio_transcode::WebviewAudioCapabilities>,
    detailed: Option<bool>,
    normalization_route_required: Option<bool>,
) -> Result<serde_json::Value> {
    let settings = state.store.settings()?;
    let deadline = playback_sources::PlaybackDeadline::after(settings.playback_startup_timeout());
    let registration = if let Some(generation) = request_generation {
        state.webview_audio_transport.begin(generation)?;
        state.webview_audio_transcoder.cancel_older_than(generation);
        Some((
            generation,
            state
                .playback_resolution
                .lock()
                .map_err(|_| "Playback resolver unavailable")?
                .begin(generation)?,
        ))
    } else {
        None
    };
    let (_idle, idle_cancel) = tokio::sync::watch::channel(false);
    let cancel = registration
        .as_ref()
        .map(|(_, (_, cancel))| cancel.clone())
        .unwrap_or(idle_cancel);
    let operation = async {
        let resolved = radio::resolve_station_playback_until(
            station_id.clone(),
            &state.store,
            deadline.instant(),
            cancel.clone(),
        )
        .await?;
        let mut url = resolved.url;
        let mut mime = resolved.mime_type;
        let mut codec = resolved.codec;
        let mut transport = "direct";
        if let Some((generation, _)) = registration.as_ref() {
            let source = native_output::AudioSource {
                input: url.clone(),
                ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                headers: vec![],
                decoder_hint: Some(native_output::AudioSourceDecoderHint {
                    mime_type: mime.clone(),
                    codec: codec.clone(),
                    duration_seconds: None,
                    content_length: None,
                    has_video: false,
                    decode_policy: crate::native_output::AudioSourceDecodePolicy::Automatic,
                    decoder_fingerprint: None,
                }),

                cenc: None,
            };
            let convert = webview_audio_transcode::needs_live_ogg_conversion(
                mime.as_deref(),
                codec.as_deref(),
                webview_capabilities.as_ref(),
            );
            if convert
                || webview_audio_transport::needs_relay_for_normalization(
                    &source,
                    normalization_route_required.unwrap_or(false),
                )
            {
                url = state
                    .webview_audio_transport
                    .register(
                        *generation,
                        &format!("radio:{station_id}"),
                        &source,
                        true,
                        deadline,
                        cancel.clone(),
                    )
                    .await?;
                transport = "loopback-relay";
            }
            if convert {
                url = state.webview_audio_transcoder.register(
                    *generation,
                    url,
                    source.ffmpeg.clone(),
                    deadline,
                )?;
                mime = Some("audio/mpeg".into());
                codec = Some("mp3".into());
                transport = "loopback-transcode";
            }
            state.webview_identification.register(
                *generation,
                format!("radio:{station_id}"),
                source,
                None,
                true,
                None,
            );
        }
        if detailed.unwrap_or(false) {
            Ok(
                serde_json::json!({"url":url,"audioMimeType":mime,"audioCodec":codec,"sourceTransport":transport,"normalizationRoute":webview_audio_transport::normalization_route_capability(transport!="direct")}),
            )
        } else {
            Ok(serde_json::Value::String(url))
        }
    };
    let result = operation.await;
    if let Some((generation, (stop, cancel))) = registration {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(generation, &stop);
        if gate.generation != generation || *cancel.borrow() {
            return Err("Playback resolution was superseded".into());
        }
    }
    result
}
#[tauri::command]
async fn get_station_details(
    state: State<'_, AppState>,
    station_id: String,
) -> Result<radio::RadioStationDetails> {
    radio::get_station_details(station_id, &state.store).await
}
#[tauri::command]
async fn poll_radio_station(
    state: State<'_, AppState>,
    station_id: String,
    recognize: bool,
) -> Result<radio::RadioMetadata> {
    let enabled = state
        .store
        .get_json_preference("radioRecognitionEnabled.v1")?
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let before = if state.playback.native_enabled() {
        state.playback.snapshot()
    } else {
        state.automation.playback_tracking_snapshot()
    };
    let mut result =
        radio::poll_radio_station(station_id.clone(), recognize && enabled, &state.store).await?;
    let snapshot = if state.playback.native_enabled() {
        state.playback.snapshot()
    } else {
        state.automation.playback_tracking_snapshot()
    };
    let generation_field = if state.playback.native_enabled() {
        "generation"
    } else {
        "sourceGeneration"
    };
    if before[generation_field] != snapshot[generation_field]
        || before["sessionId"] != snapshot["sessionId"]
    {
        return Err("Playback changed during station metadata lookup".into());
    }
    let active_id = snapshot["trackId"]
        .as_str()
        .or_else(|| snapshot["track"]["id"].as_str());
    if active_id == Some(format!("radio:{station_id}").as_str()) {
        if let Some(generation) = snapshot[if state.playback.native_enabled() {
            "generation"
        } else {
            "sourceGeneration"
        }]
        .as_u64()
        {
            result.now_playing =
                radio::reconcile_played_radio_metadata(&station_id, generation, result.now_playing);
        }
    }
    Ok(result)
}
#[tauri::command]
async fn identify_radio_station(
    state: State<'_, AppState>,
    station_id: String,
    capture_id: Option<String>,
) -> Result<radio::RadioMetadata> {
    if state.playback.native_enabled() {
        state
            .playback
            .identify_radio_for(&station_id, capture_id)
            .await
    } else {
        radio::identify_radio_station(station_id, &state.store).await
    }
}
#[tauri::command]
fn get_mpv_status() -> external_player::MpvStatus {
    external_player::status()
}
#[tauri::command]
async fn open_in_mpv(
    state: State<'_, AppState>,
    source: String,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    video: bool,
) -> Result<()> {
    external_player::open_in_mpv_with_provider_support(
        &source,
        start_seconds,
        end_seconds,
        video,
        &state.store.settings()?.yt_dlp_path,
    )
    .await
}
#[tauri::command]
async fn get_radio_metadata(station_id: String) -> Result<radio::RadioMetadata> {
    radio::get_radio_metadata(station_id).await
}

#[tauri::command]
fn get_sidebar_layout(state: State<'_, AppState>) -> Result<sidebar_layout::SidebarLayout> {
    sidebar_layout::load(&state.store)
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CreateBackupOptions {
    include_media: bool,
}

#[tauri::command]
async fn inspect_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<backup_manager::BackupPreview>> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .add_filter("Burrata backup", &["burrata-backup"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|e| e.to_string())?;
    let manager = state.backup_manager.clone();
    tauri::async_runtime::spawn_blocking(move || manager.inspect_path(&path))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

#[tauri::command]
async fn consume_import_drop(
    state: State<'_, AppState>,
    token: String,
) -> Result<import_drop::ConsumedImportDrop> {
    let claim = state.import_drops.consume(&token)?;
    match claim {
        import_drop::ImportDropClaim::Multiple(count) => Err(format!(
            "Drop one supported file at a time; {count} files were dropped."
        )),
        import_drop::ImportDropClaim::Empty => {
            Err("Drop one supported playlist or Burrata backup file.".into())
        }
        import_drop::ImportDropClaim::One(path) => match import_drop::classify_path(&path)? {
            import_drop::ImportDropKind::Playlist => {
                let (name, text) = tauri::async_runtime::spawn_blocking(move || {
                    import_drop::read_playlist_file(&path)
                })
                .await
                .map_err(|error| error.to_string())??;
                Ok(import_drop::ConsumedImportDrop::Playlist { name, text })
            }
            import_drop::ImportDropKind::Backup => {
                let manager = state.backup_manager.clone();
                let preview = tauri::async_runtime::spawn_blocking(move || {
                    let canonical = import_drop::prepare_backup_path(&path)?;
                    manager.inspect_path(&canonical)
                })
                .await
                .map_err(|error| error.to_string())??;
                Ok(import_drop::ConsumedImportDrop::Backup { preview })
            }
        },
    }
}

#[tauri::command]
async fn create_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    options: CreateBackupOptions,
) -> Result<serde_json::Value> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .add_filter("Burrata backup", &["burrata-backup"])
            .set_file_name("Burrata-backup.burrata-backup")
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(selected) = selected else {
        return Ok(serde_json::json!({"jobId":null}));
    };
    let mut destination = selected.into_path().map_err(|e| e.to_string())?;
    if destination.extension().and_then(|v| v.to_str()) != Some("burrata-backup") {
        let mut name = destination.file_name().unwrap_or_default().to_os_string();
        name.push(".burrata-backup");
        destination.set_file_name(name);
    }
    let job_id =
        state
            .backup_manager
            .start_create(app, destination, options.include_media, false)?;
    Ok(serde_json::json!({"jobId":job_id}))
}

#[tauri::command]
fn submit_backup_checkpoint(
    state: State<'_, AppState>,
    response: backup_manager::FrontendCheckpointResponse,
) -> Result<()> {
    state.backup_manager.submit_frontend_checkpoint(response)
}

#[tauri::command]
async fn estimate_backup(
    state: State<'_, AppState>,
    include_media: bool,
) -> Result<backup_manager::BackupEstimate> {
    let manager = state.backup_manager.clone();
    tauri::async_runtime::spawn_blocking(move || manager.estimate(include_media))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_backup_job(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<backup_manager::BackupJobStatus> {
    state.backup_manager.status(&job_id)
}

#[tauri::command]
fn get_active_backup_job(
    state: State<'_, AppState>,
) -> Result<Option<backup_manager::BackupJobStatus>> {
    state.backup_manager.active_job_status()
}

#[tauri::command]
fn cancel_backup_job(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.backup_manager.cancel(&job_id)
}

#[tauri::command]
fn restore_backup(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    preview_id: String,
    options: backup_manager::RestoreRequest,
) -> Result<serde_json::Value> {
    let job_id = state
        .backup_manager
        .start_restore(app, preview_id, options)?;
    Ok(serde_json::json!({"jobId":job_id}))
}

#[tauri::command]
fn get_restore_work_policy(
    state: State<'_, AppState>,
) -> Result<restore_work_policy::RestoreWorkPolicyState> {
    restore_work_policy::state(&state.store)
}

#[tauri::command]
async fn review_restored_automatic_work(
    state: State<'_, AppState>,
    resume: restore_work_policy::ResumeAutomaticWork,
) -> Result<restore_work_policy::RestoreWorkPolicyState> {
    let store = state.store.clone();
    let analysis = state.analysis.clone();
    let autotag = state.autotag.clone();
    let acquisition = state.acquisition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reviewed = restore_work_policy::review(&store, resume.clone())?;
        if resume.metadata_retag_analysis {
            analysis.queue(None, false)?;
        }
        if resume.asset_autotag {
            autotag.resume_restored_work()?;
        }
        if resume.background_source_preparation {
            source_preparation::schedule_saved(acquisition);
        }
        Ok(reviewed)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn preview_backup_restore(
    state: State<'_, AppState>,
    preview_id: String,
    options: backup_manager::RestorePreviewOptions,
) -> Result<backup_state::RestoreStateSummary> {
    state
        .backup_manager
        .preview_restore_selection(&preview_id, options)
}

#[tauri::command]
async fn choose_backup_restore_root(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    preview_id: String,
    root_key: String,
) -> Result<Option<backup_manager::BackupMediaRootPreview>> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Choose audio restore folder")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let destination = selected.into_path().map_err(|e| e.to_string())?;
    state
        .backup_manager
        .register_restore_root(&preview_id, &root_key, &destination)
        .map(Some)
}

#[tauri::command]
fn get_backup_schedule(state: State<'_, AppState>) -> Result<backup_manager::BackupSchedule> {
    state.backup_manager.get_schedule()
}

#[tauri::command]
fn set_backup_schedule(
    state: State<'_, AppState>,
    schedule: backup_manager::BackupSchedule,
) -> Result<backup_manager::BackupSchedule> {
    state.backup_manager.set_schedule(schedule)
}

#[tauri::command]
async fn choose_backup_schedule_destination(app: tauri::AppHandle) -> Result<Option<PathBuf>> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Choose automatic backup folder")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    selected
        .map(|path| path.into_path().map_err(|e| e.to_string()))
        .transpose()
}

#[tauri::command]
async fn export_backup_settings(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<bool> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .add_filter("Burrata settings", &["json"])
            .set_file_name("Burrata-settings.json")
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(selected) = selected else {
        return Ok(false);
    };
    let mut destination = selected.into_path().map_err(|error| error.to_string())?;
    if destination
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("json")
    {
        destination.set_extension("json");
    }
    let manager = state.backup_manager.clone();
    let checkpoint_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.export_settings(&checkpoint_app, &destination)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(true)
}

#[tauri::command]
async fn inspect_backup_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<backup_state::SettingsImportPreview>> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .add_filter("Burrata settings", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|error| error.to_string())?;
    let manager = state.backup_manager.clone();
    tauri::async_runtime::spawn_blocking(move || manager.inspect_settings_path(&path))
        .await
        .map_err(|error| error.to_string())?
        .map(Some)
}

#[tauri::command]
fn apply_backup_settings(
    state: State<'_, AppState>,
    preview_id: String,
    selection: backup_state::SettingsImportSelection,
) -> Result<backup_manager::SettingsImportResult> {
    state
        .backup_manager
        .apply_settings_import(&preview_id, selection)
}

#[tauri::command]
fn get_backup_frontend_preferences(
    state: State<'_, AppState>,
) -> Result<Option<backup_frontend_preferences::ReadResponse>> {
    backup_frontend_preferences::read_or_migrate(
        &state.store,
        &state.backup_frontend_preference_session_id,
    )
}

#[tauri::command]
fn ack_backup_frontend_preferences(
    state: State<'_, AppState>,
    nonce: String,
    receipt_session_id: String,
) -> Result<backup_frontend_preferences::AckResult> {
    backup_frontend_preferences::acknowledge(
        &state.store,
        &state.backup_frontend_preference_session_id,
        &nonce,
        &receipt_session_id,
    )
}

#[tauri::command]
fn get_backup_playback_restore(state: State<'_, AppState>) -> Result<Option<serde_json::Value>> {
    let Some(mut value) = state
        .store
        .get_json_preference(backup_state::BACKUP_PLAYBACK_HANDOFF_KEY)?
    else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    if object.is_empty() {
        return Ok(None);
    }
    // The backend session is process-local. It augments the legacy v1 payload
    // at read time and is never written to the handoff or portable archive.
    object.insert(
        "currentSessionId".into(),
        serde_json::Value::String(state.backup_frontend_preference_session_id.clone()),
    );
    Ok(Some(value))
}

#[tauri::command]
fn ack_backup_playback_restore(
    state: State<'_, AppState>,
    nonce: String,
    receipt_session_id: String,
) -> Result<backup_frontend_preferences::AckResult> {
    backup_frontend_preferences::acknowledge_key(
        &state.store,
        &state.backup_frontend_preference_session_id,
        backup_state::BACKUP_PLAYBACK_HANDOFF_KEY,
        &nonce,
        &receipt_session_id,
    )
}
#[tauri::command]
fn save_sidebar_layout(
    state: State<'_, AppState>,
    layout: sidebar_layout::SidebarLayout,
) -> Result<sidebar_layout::SidebarLayout> {
    sidebar_layout::save(&state.store, layout)
}

#[tauri::command]
async fn get_connections(state: State<'_, AppState>) -> Result<connections::ConnectionStatus> {
    let manager = state.connections.clone();
    tokio::task::spawn_blocking(move || manager.status())
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn save_connections(
    state: State<'_, AppState>,
    update: connections::ConnectionUpdate,
) -> Result<connections::ConnectionStatus> {
    let manager = state.connections.clone();
    tokio::task::spawn_blocking(move || manager.update(update))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn import_onetagger_discogs(
    state: State<'_, AppState>,
    session_only: Option<bool>,
) -> Result<connections::ConnectionStatus> {
    let manager = state.connections.clone();
    tokio::task::spawn_blocking(move || {
        connections::import_onetagger_discogs(&manager, session_only.unwrap_or(false))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_directory_browser_roots(
    state: State<'_, AppState>,
) -> Result<Vec<directory_browser::DirectoryRoot>> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.roots())
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_directory_browser_children(
    state: State<'_, AppState>,
    root_id: String,
    relative_path: String,
) -> Result<directory_browser::DirectoryChildren> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.children(&root_id, &relative_path))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn choose_directory_browser_root(app: tauri::AppHandle) -> Result<Option<serde_json::Value>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose a directory to browse")
            .blocking_pick_folder()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|_| "Chosen directory path is unavailable".to_owned())?;
    let path = path.to_str().ok_or("Chosen directory path is not UTF-8")?;
    // Selection alone never registers a root or changes Music/playlist settings.
    Ok(Some(serde_json::json!({"path":path})))
}
#[tauri::command]
async fn add_directory_browser_root(
    state: State<'_, AppState>,
    path: String,
) -> Result<directory_browser::DirectoryRoot> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.add_root(&path))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn remove_directory_browser_root(state: State<'_, AppState>, root_id: String) -> Result<()> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.remove_root(&root_id))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn start_directory_browser_sort_availability(
    state: State<'_, AppState>,
    root_id: String,
    relative_path: String,
    query: Option<String>,
    duration_min: Option<f64>,
    duration_max: Option<f64>,
) -> Result<directory_browser::DirectorySortAvailabilityJob> {
    let manager = state.directory_browser.clone();
    tauri::async_runtime::spawn(async move {
        manager.start_sort_availability(
            &root_id,
            &relative_path,
            directory_browser::DirectoryBrowseOptions {
                query,
                duration_min,
                duration_max,
                ..Default::default()
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn get_directory_browser_sort_availability(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<directory_browser::DirectorySortAvailabilityJob> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.get_sort_availability(&job_id))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn cancel_directory_browser_sort_availability(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<directory_browser::DirectorySortAvailabilityJob> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.cancel_sort_availability(&job_id))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn browse_directory_browser(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    root_id: String,
    relative_path: String,
    offset: usize,
    limit: usize,
    query: Option<String>,
    sort_field: Option<String>,
    sort_direction: Option<String>,
    duration_min: Option<f64>,
    duration_max: Option<f64>,
) -> Result<directory_browser::DirectoryPage> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || -> Result<directory_browser::DirectoryPage> {
        let options = directory_browser::DirectoryBrowseOptions {
            query,
            sort_field,
            sort_direction,
            duration_min,
            duration_max,
        };
        let page =
            manager.browse_with_options(&root_id, &relative_path, offset, limit, &options)?;
        for file in &page.files {
            let Some(track) = file.track.as_ref() else {
                continue;
            };
            // Only fingerprint-validated, indexed files returned by this registered
            // directory browse receive exact-file scope. Never grant a root/glob.
            if track.local_path.as_deref() != Some(file.path.as_str())
                || file.asset_id.is_none()
                || track.asset_id != file.asset_id
            {
                return Err("Directory file asset binding is invalid".into());
            }
            app.asset_protocol_scope()
                .allow_file(Path::new(&file.path))
                .map_err(|_| "Directory file playback scope is unavailable".to_owned())?;
        }
        Ok(page)
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn start_directory_browser_scan(
    state: State<'_, AppState>,
    root_id: String,
) -> Result<directory_browser::DirectoryScan> {
    let manager = state.directory_browser.clone();
    // start_scan creates the async indexing worker and must retain runtime context.
    tauri::async_runtime::spawn(async move { manager.start_scan(&root_id) })
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn get_directory_browser_scan(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<directory_browser::DirectoryScan> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.scan_status(&job_id))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn cancel_directory_browser_scan(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<directory_browser::DirectoryScan> {
    let manager = state.directory_browser.clone();
    tokio::task::spawn_blocking(move || manager.cancel_scan(&job_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_local_playlists(
    state: State<'_, AppState>,
) -> Result<Vec<playlist_files::LocalPlaylistFile>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || playlist_files::list_local_playlists(&store.settings()?))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn scan_local_playlists(
    state: State<'_, AppState>,
) -> Result<playlist_files::LocalPlaylistScan> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || playlist_files::scan_local_playlists(&store.settings()?))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn playlist_watcher_running(state: State<'_, AppState>) -> bool {
    let running = state
        .playlist_watcher
        .lock()
        .map(|watcher| watcher.as_ref().is_some_and(|watcher| watcher.is_running()))
        .unwrap_or(false);
    if std::env::var_os("BURRATA_PERF_TRACE").is_some() {
        eprintln!("[perf] playlist_sync.watcher_health running={running}");
    }
    running
}

#[tauri::command]
async fn sync_playlists(state: State<'_, AppState>) -> Result<playlist_sync::PlaylistSyncStatus> {
    let store = state.store.clone();
    let history = state.history.clone();
    tokio::task::spawn_blocking(move || {
        playlist_sync::force_sync_playlists_with_history(&store, &store.settings()?, &history)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn import_local_playlist(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    refresh: Option<bool>,
) -> Result<playlist_files::LocalPlaylistImport> {
    let _library_write = state.store.try_library_write()?;
    let settings = state.store.settings()?;
    let existing = state
        .store
        .list_asset_paths()?
        .into_iter()
        .map(PathBuf::from)
        .collect::<std::collections::HashSet<_>>();
    for file in playlist_files::referenced_local_files(&settings, &path)? {
        app.asset_protocol_scope()
            .allow_file(&file)
            .map_err(|e| e.to_string())?;
        if existing.contains(&file) {
            continue;
        }
        let Ok(mut track) = media::inspect_local_file(&file, &settings).await else {
            continue;
        };
        track.provenance["readOnly"] = serde_json::json!(true);
        let hash = track
            .id
            .strip_prefix("local:")
            .unwrap_or(&track.id)
            .to_owned();
        let asset = LocalAsset {
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: None,
            size_bytes: None,
            bitrate_origin: None,
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: track.id.clone(),
            path: file.to_string_lossy().into_owned(),
            hash,
            source_codec: track.provenance["codec"]
                .as_str()
                .unwrap_or("unknown")
                .into(),
            final_codec: track.provenance["codec"]
                .as_str()
                .unwrap_or("unknown")
                .into(),
            source_bitrate: track.provenance["bitrateKbps"].as_f64(),
            final_bitrate: track.provenance["bitrateKbps"].as_f64(),
            processing: "external-readonly".into(),
            track,
        };
        state.store.save_asset(&asset)?;
    }
    if refresh.unwrap_or(false) {
        playlist_sync::refresh_discovered_playlist(&state.store, &settings, &path)
    } else {
        playlist_sync::import_discovered_playlist(&state.store, &settings, &path)
    }
}
#[tauri::command]
fn get_playlist_preferences(
    state: State<'_, AppState>,
) -> Result<playlist_preferences::PlaylistPreferences> {
    playlist_preferences::load(&state.store)
}
#[tauri::command]
fn set_playlist_shortcut(
    state: State<'_, AppState>,
    slot: usize,
    collection_id: Option<String>,
) -> Result<playlist_preferences::PlaylistPreferences> {
    playlist_preferences::set_shortcut(&state.store, slot, collection_id)
}
#[tauri::command]
fn set_favorites_playlist(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<playlist_preferences::PlaylistPreferences> {
    playlist_preferences::set_favorites_playlist(&state.store, &collection_id)
}
#[tauri::command]
fn set_favorite_membership(state: State<'_, AppState>, track: Track, liked: bool) -> Result<()> {
    playlist_preferences::set_favorite_membership(&state.store, &track, liked)?;
    if liked {
        source_preparation::schedule(state.acquisition.clone(), vec![track]);
    }
    Ok(())
}
#[tauri::command]
fn set_playlist_shortcut_binding(
    state: State<'_, AppState>,
    slot: usize,
    binding: String,
) -> Result<playlist_preferences::PlaylistPreferences> {
    playlist_preferences::set_shortcut_binding(&state.store, slot, &binding)
}
#[tauri::command]
fn get_playlist_mirror_options(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<playlist_mirror_options::PlaylistMirrorOptionsView> {
    playlist_mirror_options::get_playlist_mirror_options(&state.store, &collection_id)
}
#[tauri::command]
fn set_playlist_mirror_options(
    state: State<'_, AppState>,
    collection_id: String,
    options: playlist_mirror_options::PlaylistMirrorOptions,
) -> Result<playlist_mirror_options::PlaylistMirrorOptionsView> {
    playlist_mirror_options::set_playlist_mirror_options(&state.store, &collection_id, options)
}
#[tauri::command]
fn get_radio_song_track(
    state: State<'_, AppState>,
    station_id: String,
    metadata: radio::RadioTrackMetadata,
) -> Result<Track> {
    let station = radio::get_radio_stations_saved(&state.store)?
        .into_iter()
        .find(|s| s.id == station_id)
        .ok_or("Radio station not found")?;
    if metadata.station_id != station.id {
        return Err("Station metadata does not match".into());
    }
    let mut track = playback_history::radio_song_track(&station, &metadata);
    if let Some(saved) = state.store.get_track(&track.id)? {
        track.favorite = saved.favorite;
        track.local_path = saved.local_path;
        track.local_availability = saved.local_availability;
        track.asset_id = saved.asset_id;
        track.duration_seconds = saved.duration_seconds;
        if track.isrc.is_none() {
            track.isrc = saved.isrc;
        }
        track.bpm = saved.bpm;
        track.key = saved.key;
        if track.artwork_url.is_none() {
            track.artwork_url = saved.artwork_url;
        }
    }
    Ok(track)
}
#[tauri::command]
fn get_radio_history_setting(state: State<'_, AppState>) -> Result<bool> {
    playback_history::include_radio_songs_in_history(&state.store)
}
#[tauri::command]
fn set_radio_history_setting(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    playback_history::set_include_radio_songs_in_history(&state.store, enabled)
}
#[tauri::command]
fn record_radio_history(
    state: State<'_, AppState>,
    listening_session_id: String,
    occurrence_key: String,
    listened_at: String,
    station_id: String,
    metadata: radio::RadioTrackMetadata,
) -> Result<bool> {
    let station = radio::get_radio_stations_saved(&state.store)?
        .into_iter()
        .find(|s| s.id == station_id)
        .ok_or("Radio station not found")?;
    state.history.record_radio_occurrence(
        &state.store,
        &listening_session_id,
        &occurrence_key,
        &listened_at,
        &station,
        &metadata,
    )
}
#[tauri::command]
fn record_playback(
    state: State<'_, AppState>,
    session_id: String,
    provider: String,
    track: Track,
    chapter: Option<serde_json::Value>,
) -> Result<bool> {
    if state.store.get_track(&track.id)?.is_none() {
        state.store.cache_track(&track)?;
    }
    state
        .history
        .record_enriched(&state.store, &session_id, &provider, &track, chapter)
}
#[tauri::command]
fn list_playback_history(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<playback_history::PlaybackHistoryEntry>> {
    state
        .history
        .list_enriched(&state.store, limit.unwrap_or(100), offset.unwrap_or(0))
}
#[tauri::command]
fn search_playback_history(
    state: State<'_, AppState>,
    options: playback_history::PlaybackHistorySearchOptions,
) -> Result<playback_history::PlaybackHistoryPage> {
    state.history.search(&state.store, options)
}
#[tauri::command]
fn get_track_library_summary(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<playback_history::TrackLibrarySummary> {
    state.history.track_library_summary(&state.store, &track_id)
}
#[tauri::command]
fn clear_playback_history(state: State<'_, AppState>) -> Result<()> {
    state.history.clear()
}
#[tauri::command]
fn save_audio_match(
    state: State<'_, AppState>,
    mut track: Track,
    mut candidate: Candidate,
) -> Result<Track> {
    if !matches!(candidate.provider.as_str(), "youtube" | "soundcloud") {
        return Err("Only public audio sources can be remembered".into());
    }
    acquisition::validate_source_url(&candidate.url, &candidate.provider)?;
    let mut url = url::Url::parse(&candidate.url).map_err(|_| "Invalid source URL")?;
    if candidate.provider == "youtube" {
        let id = if url.host_str() == Some("youtu.be") {
            url.path().trim_start_matches('/').to_owned()
        } else {
            url.query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default()
        };
        if id.is_empty() {
            return Err("Expected a YouTube video URL".into());
        }
        candidate.url = format!("https://www.youtube.com/watch?v={id}");
    } else {
        url.set_query(None);
        url.set_fragment(None);
        candidate.url = url.to_string();
    }
    if track.provenance["sourceKind"] == "audioSearch" {
        track.id = format!("{}:{}", track.provider, track.provider_id);
    }
    state.store.save_audio_match(&track, &candidate)
}
#[tauri::command]
async fn inspect_chapters(
    state: State<'_, AppState>,
    track: Track,
    candidate: Candidate,
) -> Result<Vec<Chapter>> {
    acquisition::inspect_chapters(&track, &candidate, &state.store.settings()?).await
}
#[tauri::command]
async fn enrich_chapters(state: State<'_, AppState>, candidate: Candidate) -> Result<Candidate> {
    Ok(state
        .artists
        .enrich_candidate_chapters(
            &candidate,
            &artist_credentials(state.connections.clone()).await?,
        )
        .await)
}
fn cached_artwork_path(directory: &Path, key: &str) -> Result<Option<PathBuf>> {
    Ok(artwork_cache::lookup_or_migrate(directory, key))
}

fn cache_artwork_bytes(app: &tauri::AppHandle, key: &str, bytes: &[u8]) -> Result<String> {
    let extension = media::artwork_extension(bytes)?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("artwork");
    let target = artwork_cache::write(&directory, key, bytes, extension)?;
    app.asset_protocol_scope()
        .allow_file(&target)
        .map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().into_owned())
}

#[tauri::command]
async fn cache_artwork(app: tauri::AppHandle, url: String) -> Result<String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("artwork");
    if let Some(path) = cached_artwork_path(&directory, &url)? {
        app.asset_protocol_scope()
            .allow_file(&path)
            .map_err(|e| e.to_string())?;
        return Ok(path.to_string_lossy().into_owned());
    }
    let bytes = media::artwork(&Track {
        artwork_url: Some(url.clone()),
        ..Track::default()
    })
    .await?
    .ok_or("No artwork")?;
    cache_artwork_bytes(&app, &url, &bytes)
}

#[tauri::command]
async fn cache_asset_artwork(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    asset_id: String,
) -> Result<Option<String>> {
    let asset = state
        .store
        .get_asset_by_id(&asset_id)?
        .ok_or("Registered audio asset not found")?;
    let Some(bytes) = media::embedded_artwork_for_asset(&asset).await? else {
        return Ok(None);
    };
    cache_artwork_bytes(&app, &format!("asset:{}:{}", asset.id, asset.hash), &bytes).map(Some)
}
fn slskd_connection(state: &AppState) -> Result<connections::SlskdConnection> {
    state
        .connections
        .slskd()?
        .ok_or_else(|| "Enable and configure Soulseek in Settings first".into())
}
#[tauri::command]
async fn get_slskd_shares(state: State<'_, AppState>) -> Result<serde_json::Value> {
    let config = slskd_connection(&state)?;
    let managed = slskd::is_managed_slskd_connection(&config.base_url, &config.downloads_path);
    let client = slskd::SlskdClient::new(&config.base_url, &config.api_key)?;
    let remote = client.shared_folders().await;
    let mut shares = remote.as_ref().cloned().unwrap_or_default();
    if managed {
        let managed_folders = slskd::list_managed_share_folders()?;
        slskd::associate_managed_share_ids(&mut shares, &managed_folders);
        for item in managed_folders {
            let remote_path = format!("/burrata-shares/{}", item.id);
            if let Some(share) = shares.iter_mut().find(|share| {
                share.id == item.id
                    || share.local_path == remote_path
                    || share.remote_path == remote_path
            }) {
                share.id = item.id;
                share.local_path = item.directory_path;
            } else {
                shares.push(slskd::SharedFolder {
                    id: item.id,
                    alias: item.alias,
                    local_path: item.directory_path,
                    remote_path,
                    files: 0,
                    directories: 0,
                    excluded: false,
                });
            }
        }
    }
    let guidance = if managed {
        "Folder changes restart the local Soulseek service and may interrupt transfers. Folders are mounted read-only."
    } else {
        "This service is managed outside Burrata. Add or remove shared folders in its slskd configuration, then restart it and rescan."
    };
    Ok(
        serde_json::json!({"managed":managed,"canEdit":managed && slskd::managed_share_controls_available(),"shares":shares,"guidance":guidance,"warning":remote.err()}),
    )
}
#[tauri::command]
async fn rescan_slskd_shares(state: State<'_, AppState>) -> Result<serde_json::Value> {
    let config = slskd_connection(&state)?;
    slskd::SlskdClient::new(&config.base_url, &config.api_key)?
        .rescan_shared_folders()
        .await?;
    get_slskd_shares(state).await
}
#[tauri::command]
async fn add_managed_slskd_share(
    state: State<'_, AppState>,
    directory_path: String,
) -> Result<serde_json::Value> {
    let config = slskd_connection(&state)?;
    if !slskd::is_managed_slskd_connection(&config.base_url, &config.downloads_path) {
        return Err(
            "Folder controls are available only for Burrata’s managed local Soulseek service"
                .into(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || {
        slskd::add_managed_share_folder(&directory_path).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())??;
    get_slskd_shares(state).await
}
#[tauri::command]
async fn remove_managed_slskd_share(
    state: State<'_, AppState>,
    share_id: String,
) -> Result<serde_json::Value> {
    let config = slskd_connection(&state)?;
    if !slskd::is_managed_slskd_connection(&config.base_url, &config.downloads_path) {
        return Err(
            "Folder controls are available only for Burrata’s managed local Soulseek service"
                .into(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || {
        slskd::remove_managed_share_folder(&share_id).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())??;
    get_slskd_shares(state).await
}

#[tauri::command]
async fn get_managed_slskd_download_settings(
    state: State<'_, AppState>,
) -> Result<managed_slskd_settings::ManagedDownloadSettingsState> {
    let settings = state.connections.settings()?.slskd;
    tauri::async_runtime::spawn_blocking(move || {
        managed_slskd_settings::state(&settings.base_url, &settings.downloads_path)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_managed_slskd_download_settings(
    state: State<'_, AppState>,
    settings: managed_slskd_settings::ManagedDownloadSettings,
) -> Result<managed_slskd_settings::ManagedDownloadSettingsState> {
    let connection = state.connections.settings()?.slskd;
    tauri::async_runtime::spawn_blocking(move || {
        managed_slskd_settings::update(&connection.base_url, &connection.downloads_path, settings)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn test_connections(state: State<'_, AppState>) -> Result<serde_json::Value> {
    let mut results = Vec::new();
    let settings = state.connections.settings()?;
    if settings.slskd.enabled {
        let result = match state.connections.slskd() {
            Ok(Some(config)) => match slskd::SlskdClient::new(&config.base_url, &config.api_key) {
                Ok(client) => client.check_connection().await,
                Err(error) => Err(error),
            },
            Ok(None) => Err("slskd is not configured".into()),
            Err(error) => Err(error),
        };
        results.push(serde_json::json!({"provider":"slskd","available":result.is_ok(),"message":result.err().unwrap_or_else(|| "Connected; completed files will use the configured local folder.".into())}));
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("Burrata/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    for (provider, enabled) in [
        ("discogs", settings.discogs_enabled),
        ("lastfm", settings.lastfm_enabled),
    ] {
        if !enabled {
            continue;
        }
        let result: Result<()> = async {
            let credential = state
                .connections
                .credential(provider)?
                .ok_or_else(|| format!("Configure a {provider} credential first"))?;
            let request = if provider == "discogs" {
                http.get("https://api.discogs.com/oauth/identity")
                    .header("Authorization", format!("Discogs token={credential}"))
            } else {
                http.get("https://ws.audioscrobbler.com/2.0/").query(&[
                    ("method", "artist.getInfo"),
                    ("artist", "Daft Punk"),
                    ("format", "json"),
                    ("api_key", credential.as_str()),
                ])
            };
            let response = request
                .send()
                .await
                .map_err(|_| format!("Could not reach {provider}; check the connection"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "{provider} rejected the request (HTTP {})",
                    response.status().as_u16()
                ));
            }
            let body: serde_json::Value = response
                .json()
                .await
                .map_err(|_| format!("{provider} returned an invalid response"))?;
            if body.get("error").is_some() {
                return Err(format!("{provider} rejected the API key"));
            }
            Ok(())
        }
        .await;
        results.push(serde_json::json!({"provider":provider,"available":result.is_ok(),"message":result.err().unwrap_or_else(|| "Connected".into())}));
    }
    if results.is_empty() {
        results.push(serde_json::json!({"provider":"Sources","available":false,"message":"Enable and save a source connection first."}));
    }
    Ok(serde_json::Value::Array(results))
}

#[tauri::command]
async fn start_slskd_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<serde_json::Value> {
    let id = state.acquisition.start_slskd_search(&query).await?;
    Ok(serde_json::json!({"id":id}))
}
#[tauri::command]
async fn get_slskd_search(
    state: State<'_, AppState>,
    search_id: String,
) -> Result<acquisition::SlskdSearchPoll> {
    state.acquisition.poll_slskd_search(&search_id).await
}
#[tauri::command]
async fn cancel_slskd_search(state: State<'_, AppState>, search_id: String) -> Result<()> {
    state.acquisition.cancel_slskd_search(&search_id).await
}

async fn artist_credentials(
    manager: Arc<connections::ConnectionManager>,
) -> Result<artists::ArtistCredentials> {
    tokio::task::spawn_blocking(move || {
        let settings = manager.settings()?;
        let mut warnings = Vec::new();
        let mut credential = |provider: &str, enabled: bool| {
            if !enabled {
                return None;
            }
            match manager.credential(provider) {
                Ok(value) => value,
                Err(_) => {
                    warnings.push(format!(
                        "{provider} credentials unavailable; check Connections settings."
                    ));
                    None
                }
            }
        };
        let discogs_token = credential("discogs", settings.discogs_enabled);
        let lastfm_api_key = credential("lastfm", settings.lastfm_enabled);
        Ok(artists::ArtistCredentials {
            discogs_token,
            lastfm_api_key,
            warnings,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn search_artists(
    state: State<'_, AppState>,
    query: String,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<artists::ArtistSearchPage> {
    let credentials = artist_credentials(state.connections.clone()).await?;
    search_artist_request(&state.artists, &credentials, &query, page, page_size).await
}

async fn search_artist_request(
    service: &artists::ArtistService,
    credentials: &artists::ArtistCredentials,
    query: &str,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<artists::ArtistSearchPage> {
    // ArtistPage and ArtistService both use one-based pages. Incrementing here
    // skipped exact matches on page 1 and every other subsequent result page.
    service
        .search_artists(
            query,
            page.unwrap_or(1),
            page_size.unwrap_or(25),
            credentials,
        )
        .await
}
#[tauri::command]
async fn search_albums(
    state: State<'_, AppState>,
    query: String,
    page: Option<u32>,
) -> Result<artists::AlbumSearchPage> {
    state
        .artists
        .search_albums(
            &query,
            page.unwrap_or(1),
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
async fn get_artist_artwork(
    state: State<'_, AppState>,
    artist: artists::ArtistRef,
) -> Result<serde_json::Value> {
    let image = state
        .artists
        .get_artist_artwork(
            &artist,
            &artist_credentials(state.connections.clone()).await?,
        )
        .await?;
    Ok(serde_json::json!({"imageUrl":image}))
}
#[tauri::command]
async fn get_artist_profile(
    state: State<'_, AppState>,
    artist: artists::ArtistRef,
) -> Result<artists::ArtistProfile> {
    state
        .artists
        .get_artist_profile(
            &artist,
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
async fn get_artist_releases(
    state: State<'_, AppState>,
    artist: artists::ArtistRef,
    cursor: Option<String>,
) -> Result<artists::ArtistReleasePage> {
    state
        .artists
        .get_artist_releases(
            &artist,
            cursor.as_deref(),
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
async fn get_release_tracks(
    state: State<'_, AppState>,
    release: artists::ArtistReleaseRef,
    artist: artists::ArtistRef,
) -> Result<Vec<Track>> {
    let tracks = state
        .artists
        .get_release_tracks(
            &release,
            &artist,
            &artist_credentials(state.connections.clone()).await?,
        )
        .await?;
    tracks
        .iter()
        .map(|track| state.store.cache_track(track))
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveLibraryTracksResult {
    saved_count: usize,
    saved_track_ids: Vec<String>,
}

#[tauri::command]
async fn save_library_tracks(
    state: State<'_, AppState>,
    tracks: Vec<Track>,
) -> Result<SaveLibraryTracksResult> {
    let store = state.store.clone();
    let to_prepare = tracks.clone();
    let saved_track_ids = tokio::task::spawn_blocking(move || store.save_library_tracks(&tracks))
        .await
        .map_err(|error| error.to_string())??;
    source_preparation::schedule(state.acquisition.clone(), to_prepare);
    Ok(SaveLibraryTracksResult {
        saved_count: saved_track_ids.len(),
        saved_track_ids,
    })
}

#[tauri::command]
async fn get_artist_tracks(
    state: State<'_, AppState>,
    artist: artists::ArtistRef,
    cursor: Option<String>,
) -> Result<artists::ArtistTrackPage> {
    state
        .artists
        .get_artist_tracks(
            &artist,
            cursor.as_deref(),
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
async fn get_artist_collaborations(
    state: State<'_, AppState>,
    artist: artists::ArtistRef,
    cursor: Option<String>,
) -> Result<artists::ArtistCollaborationPage> {
    state
        .artists
        .get_artist_collaborations(
            &artist,
            cursor.as_deref(),
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
async fn resolve_recording_metadata(
    state: State<'_, AppState>,
    track: Track,
    release_ref: Option<artists::ArtistReleaseRef>,
    recording_ref: Option<artists::ArtistRef>,
) -> Result<metadata::MetadataReview> {
    state
        .artists
        .resolve_recording_metadata(
            &track,
            release_ref.as_ref(),
            recording_ref.as_ref(),
            &artist_credentials(state.connections.clone()).await?,
        )
        .await
}
#[tauri::command]
fn analyze_local_assets(
    state: State<'_, AppState>,
    asset_ids: Option<Vec<String>>,
    force: Option<bool>,
) -> Result<serde_json::Value> {
    Ok(serde_json::json!({"queued":state.analysis.queue(asset_ids, force.unwrap_or(false))?}))
}
#[tauri::command]
fn get_analysis_status(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>> {
    state.analysis.status()
}

#[tauri::command]
fn start_selected_asset_analysis(
    state: State<'_, AppState>,
    asset_ids: Vec<String>,
    fields: std::collections::BTreeSet<stream_analysis::AnalysisMetric>,
) -> Result<analysis_selection::SelectedAssetAnalysisJob> {
    state
        .analysis_selection_jobs
        .start(state.store.clone(), asset_ids, fields)
}

#[tauri::command]
fn get_selected_asset_analysis(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<analysis_selection::SelectedAssetAnalysisJob> {
    state.analysis_selection_jobs.get(&job_id)
}

#[tauri::command]
fn cancel_selected_asset_analysis(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.analysis_selection_jobs.cancel(&job_id)
}

#[tauri::command]
async fn apply_selected_asset_analysis(
    state: State<'_, AppState>,
    job_id: String,
    asset_id: String,
    selection: stream_analysis::ApplyAnalysisSelection,
) -> Result<Track> {
    let _writer_lease = state.store.try_library_write()?;
    state
        .analysis_selection_jobs
        .apply(&state.store, &job_id, &asset_id, &selection)
        .await
}

#[tauri::command]
fn start_selected_stream_analysis(
    state: State<'_, AppState>,
    track_ids: Vec<String>,
    fields: std::collections::BTreeSet<stream_analysis::AnalysisMetric>,
) -> Result<analysis_stream_selection::SelectedStreamAnalysisJob> {
    state.stream_analysis_selection_jobs.start(
        state.store.clone(),
        state.acquisition.clone(),
        state.playback.observed_analysis_manager(),
        track_ids,
        fields,
    )
}

#[tauri::command]
fn start_stream_media_analysis(
    state: State<'_, AppState>,
    track: analysis_stream_selection::StreamMediaTrackContext,
    provider: String,
    source_id: String,
    duration_seconds: f64,
    audio_variant: Option<stream_analysis::StreamAudioVariant>,
    automatic: Option<bool>,
    webview_generation: Option<u64>,
) -> Result<analysis_stream_selection::SelectedStreamAnalysisJob> {
    restore_work_policy::ensure_stream_media_analysis_allowed(
        &state.store,
        automatic.unwrap_or(false),
    )?;
    let selected_candidate = if let Some(generation) = webview_generation {
        let gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        active_stream_candidate_for_context(
            &gate,
            generation,
            &track,
            duration_seconds,
            &provider,
            &source_id,
        )?
    } else {
        None
    };
    state.stream_analysis_selection_jobs.start_media_analysis(
        state.store.clone(),
        state.acquisition.clone(),
        state.playback.observed_analysis_manager(),
        analysis_stream_selection::SelectedStreamMediaAnalysisRequest {
            track,
            provider,
            source_id,
            duration_seconds,
            audio_variant,
            webview_generation,
        },
        selected_candidate,
    )
}

#[tauri::command]
fn get_cached_stream_media_analysis(
    state: State<'_, AppState>,
    track: analysis_stream_selection::StreamMediaTrackContext,
    provider: String,
    source_id: String,
    duration_seconds: f64,
    audio_variant: Option<stream_analysis::StreamAudioVariant>,
) -> Result<Option<stream_analysis::WholeTrackAnalysis>> {
    let track = {
        let gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        stream_media_cache_lookup_track(&gate, &track, duration_seconds, &provider, &source_id)?
    };
    Ok(state
        .playback
        .observed_analysis_manager()
        .cached_stream_media(
            &track,
            &provider,
            &source_id,
            duration_seconds,
            audio_variant.as_ref(),
        ))
}

#[tauri::command]
fn get_selected_stream_analysis(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<analysis_stream_selection::SelectedStreamAnalysisJob> {
    state.stream_analysis_selection_jobs.get(&job_id)
}

#[tauri::command]
fn cancel_selected_stream_analysis(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.stream_analysis_selection_jobs.cancel(&job_id)
}

#[tauri::command]
async fn apply_selected_stream_analysis(
    state: State<'_, AppState>,
    job_id: String,
    track_id: String,
    selection: stream_analysis::ApplyAnalysisSelection,
) -> Result<Track> {
    let _writer_lease = state.store.try_library_write()?;
    state
        .stream_analysis_selection_jobs
        .apply(&state.store, &job_id, &track_id, &selection)
        .await
}

#[derive(Default)]
struct PlaybackResolutionGate {
    generation: u64,
    active: Option<(u64, tokio::sync::watch::Sender<bool>)>,
    resolved_stream_source: Option<ResolvedStreamSourceBinding>,
    stream_source_cancelled: bool,
}

#[derive(Clone)]
struct ResolvedStreamSourceBinding {
    generation: u64,
    track_id: String,
    recording_identity: String,
    candidate: crate::models::Candidate,
}

impl PlaybackResolutionGate {
    fn begin(
        &mut self,
        generation: u64,
    ) -> Result<(
        tokio::sync::watch::Sender<bool>,
        tokio::sync::watch::Receiver<bool>,
    )> {
        if generation < self.generation {
            return Err("Playback request was superseded".into());
        }
        if let Some((_, previous)) = self.active.take() {
            previous.send_replace(true);
        }
        self.resolved_stream_source = None;
        self.stream_source_cancelled = false;
        self.generation = generation;
        let (sender, receiver) = tokio::sync::watch::channel(false);
        self.active = Some((generation, sender.clone()));
        Ok((sender, receiver))
    }

    fn cancel_older_than(&mut self, generation: u64) {
        if generation <= self.generation {
            return;
        }
        self.generation = generation;
        self.resolved_stream_source = None;
        self.stream_source_cancelled = true;
        if self
            .active
            .as_ref()
            .is_some_and(|(active_generation, _)| *active_generation < generation)
        {
            if let Some((_, sender)) = self.active.take() {
                sender.send_replace(true);
            }
        }
    }

    fn cancel_for_provider_session_change(&mut self) -> u64 {
        if let Some((_, stop)) = self.active.take() {
            stop.send_replace(true);
        }
        self.resolved_stream_source = None;
        self.stream_source_cancelled = true;
        // Return the relay fence without consuming the frontend's next request.
        self.generation.saturating_add(1)
    }

    fn has_current_resolution(&self) -> bool {
        self.active.is_some()
    }

    fn stream_candidate_for(
        &self,
        generation: u64,
        track_id: &str,
        recording_identity: Option<&str>,
        provider: &str,
        source_id: &str,
    ) -> Option<crate::models::Candidate> {
        let binding = self.resolved_stream_source.as_ref()?;
        (!self.stream_source_cancelled
            && binding.generation == generation
            && binding.track_id == track_id
            && recording_identity == Some(binding.recording_identity.as_str())
            && binding.candidate.provider == provider
            && binding.candidate.id == source_id)
            .then(|| binding.candidate.clone())
    }

    fn stream_candidate_rejection_reason(
        &self,
        generation: u64,
        track_id: &str,
        recording_identity: Option<&str>,
        provider: &str,
        source_id: &str,
    ) -> &'static str {
        if self.stream_source_cancelled {
            return "active YouTube source lease was cancelled";
        }
        let Some(binding) = self.resolved_stream_source.as_ref() else {
            return "no active YouTube source binding";
        };
        if binding.generation != generation || self.generation != generation {
            return "active YouTube source generation does not match";
        }
        if binding.track_id != track_id {
            return "active YouTube source track ID does not match";
        }
        if recording_identity != Some(binding.recording_identity.as_str()) {
            return "active YouTube recording context does not match";
        }
        if binding.candidate.provider != provider {
            return "active YouTube source provider does not match";
        }
        if binding.candidate.id != source_id {
            return "active YouTube source ID does not match";
        }
        "active YouTube source binding is unavailable"
    }

    fn finish(&mut self, generation: u64, sender: &tokio::sync::watch::Sender<bool>) {
        if self
            .active
            .as_ref()
            .is_some_and(|(active_generation, active_sender)| {
                *active_generation == generation && active_sender.same_channel(sender)
            })
        {
            self.active.take();
        }
    }
}

async fn resolve_with_playback_budget<T>(
    budget: std::time::Duration,
    stop: &tokio::sync::watch::Sender<bool>,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => result,
        _ = tokio::time::sleep(budget) => {
            // Notify nested searches/extractors before their future is dropped.
            stop.send_replace(true);
            Err("Audio lookup exceeded the remaining playback startup budget".into())
        }
    }
}

async fn resolve_with_shared_playback_deadline<T>(
    deadline: playback_sources::PlaybackDeadline,
    stop: &tokio::sync::watch::Sender<bool>,
    cancel: &tokio::sync::watch::Receiver<bool>,
    mut future: std::pin::Pin<Box<impl std::future::Future<Output = Result<T>>>>,
) -> Result<T> {
    let (result, deadline_branch) = tokio::select! {
        biased;
        _ = playback_sources::wait_for_cancellation(cancel.clone()) => {
            (Err("Cancelled".into()), false)
        },
        result = &mut future => (result, false),
        _ = tokio::time::sleep_until(deadline.instant()) => {
            (Err("Audio lookup exceeded the remaining playback startup budget".into()), true)
        }
    };
    // Drop the owned resolver before sending timer-generated cancellation.
    // Resolution trace Drop can then distinguish deadline expiry from a real
    // caller cancellation. Resource Drop cleanup runs before we return too.
    drop(future);
    if deadline_branch {
        stop.send_replace(true);
        result
    } else if *cancel.borrow() {
        Err("Cancelled".into())
    } else if deadline.remaining().is_zero() {
        stop.send_replace(true);
        Err("Audio lookup exceeded the remaining playback startup budget".into())
    } else {
        result
    }
}

#[derive(Default)]
struct SortAvailabilityBackfillControl {
    running: bool,
    requested: bool,
    stopped: bool,
}

impl SortAvailabilityBackfillControl {
    fn request(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        self.requested = true;
        if self.running {
            false
        } else {
            self.running = true;
            true
        }
    }

    fn begin_batch(&mut self) {
        // Requests made before this batch are represented by its pending check.
        // A request arriving during the batch remains set for the zero-work race.
        self.requested = false;
    }

    fn finish_batch(&mut self, pending_rows_may_remain: bool) -> bool {
        if self.stopped {
            self.running = false;
            return false;
        }
        if pending_rows_may_remain || self.requested {
            self.requested = false;
            return true;
        }
        self.running = false;
        false
    }

    fn fail(&mut self) -> bool {
        if self.stopped {
            self.running = false;
            return false;
        }
        if self.requested {
            self.requested = false;
            return true;
        }
        self.running = false;
        false
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.running = false;
        self.requested = false;
    }
}

struct SortAvailabilityBackfill {
    store: Arc<Store>,
    playback: Arc<playback::PlaybackService>,
    app: tauri::AppHandle,
    control: std::sync::Mutex<SortAvailabilityBackfillControl>,
    playback_resolution: Arc<std::sync::Mutex<PlaybackResolutionGate>>,
    stopped: std::sync::atomic::AtomicBool,
}

impl SortAvailabilityBackfill {
    fn new(
        store: Arc<Store>,
        playback: Arc<playback::PlaybackService>,
        playback_resolution: Arc<std::sync::Mutex<PlaybackResolutionGate>>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            store,
            playback,
            app,
            control: std::sync::Mutex::new(SortAvailabilityBackfillControl::default()),
            playback_resolution,
            stopped: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn request(self: &Arc<Self>) {
        let should_start = self
            .control
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .request();
        if should_start {
            let worker = self.clone();
            tauri::async_runtime::spawn(async move { worker.run().await });
        }
    }

    fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        self.control
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop();
    }

    async fn run(self: Arc<Self>) {
        loop {
            if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                self.control
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .fail();
                return;
            }

            let webview_resolution_active = self
                .playback_resolution
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .has_current_resolution();
            let native_startup_pending = {
                let playback_updates = self.playback.subscribe();
                let playback_snapshot = playback_updates.borrow();
                let playback_status = playback_snapshot["status"].as_str().unwrap_or_default();
                playback_status == "resolving"
                    || (playback_status == "buffering"
                        && playback_snapshot["diagnostics"]["firstAudioMs"].is_null())
            };
            if webview_resolution_active || native_startup_pending {
                // Keep library work out of native or WebView playback startup.
                // Read live state again after a short yield instead of delaying
                // unrelated steady-state buffering or playback. The subscribed
                // playback value omits queue rows, unlike snapshot().
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }

            self.control
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .begin_batch();
            if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                self.control
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .fail();
                return;
            }

            let store = self.store.clone();
            let result =
                tokio::task::spawn_blocking(move || store.backfill_sort_availability_batch(250))
                    .await;
            let updated = match result {
                Ok(Ok(updated)) => updated,
                // A failed batch is left pending. A later explicit first-page
                // request can retry it; never spin on a persistent Store error.
                _ => {
                    let retry_requested = self
                        .control
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .fail();
                    if retry_requested && !self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                        // Retry only because a caller explicitly requested while
                        // the failed batch was active; failures alone never spin.
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                    return;
                }
            };

            if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                self.control
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .fail();
                return;
            }

            let more = self
                .control
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .finish_batch(updated != 0);
            if !more {
                if !self.stopped.load(std::sync::atomic::Ordering::Acquire) {
                    let _ = self.app.emit(
                        "sort-availability-updated",
                        serde_json::json!({"status": "ready"}),
                    );
                }
                return;
            }

            // Each transaction releases the Store lock before yielding, so
            // interactive reads and playback setup get a chance between batches.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

pub(crate) struct AppState {
    discovery: Arc<discovery::DiscoveryService>,
    autotag: Arc<autotag::AutoTagger>,
    store: Arc<Store>,
    _profile_lock: backup_manager::ProfileLock,
    backup_manager: Arc<backup_manager::BackupManager>,
    import_drops: Arc<import_drop::ImportDropRegistry>,
    directory_browser: directory_browser::DirectoryBrowser,
    automation: Arc<automation::AutomationService>,
    playback: Arc<playback::PlaybackService>,
    _system_media: Option<system_media::SystemMediaService>,
    #[cfg(target_os = "linux")]
    _mpris: Option<mpris::MprisService>,
    local_audio: Arc<local_audio::LocalAudioManager>,
    webview_audio_transport: Arc<webview_audio_transport::WebviewAudioTransport>,
    webview_audio_transcoder: Arc<webview_audio_transcode::WebviewAudioTranscoder>,
    acquisition: Arc<AcquisitionManager>,
    diagnostics: Arc<std::sync::Mutex<Vec<String>>>,
    browser: Arc<browser_bridge::BrowserBridge>,
    pub(crate) managed_provider_session: Arc<managed_provider_session::ManagedProviderSession>,
    playback_provider_session_status:
        std::sync::Mutex<managed_provider_session::ManagedProviderSessionStatus>,
    remote_playlists: Arc<remote_playlists::RemotePlaylistService>,
    candidate_preview: Arc<candidate_preview::CandidatePreviewManager>,
    preview_warmup_cancel: std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    playback_resolution: Arc<std::sync::Mutex<PlaybackResolutionGate>>,
    webview_identification: webview_identification::WebViewIdentification,
    connections: Arc<connections::ConnectionManager>,
    artists: Arc<artists::ArtistService>,
    analysis: Arc<analysis::AnalysisManager>,
    analysis_selection_jobs: Arc<analysis_selection::AnalysisSelectionJobs>,
    stream_analysis_selection_jobs: Arc<analysis_stream_selection::SelectedStreamAnalysisJobs>,
    loudness_analysis_jobs: Arc<loudness_manager::LoudnessAnalysisJobs>,
    waveform_analysis_jobs: Arc<waveform_analysis::WaveformAnalysisJobs>,
    history: Arc<playback_history::PlaybackHistory>,
    playlist_watcher: std::sync::Mutex<Option<playlist_watch::PlaylistWatcher>>,
    provider_reads: Arc<provider_read_requests::ProviderReadRequests>,
    sort_availability_backfill: Arc<SortAvailabilityBackfill>,
    backup_frontend_preference_session_id: String,
}

#[cfg(test)]
mod sort_availability_backfill_control_tests {
    use super::SortAvailabilityBackfillControl;

    #[test]
    fn overlapping_requests_coalesce_to_one_worker() {
        let mut control = SortAvailabilityBackfillControl::default();
        assert!(control.request());
        assert!(!control.request());
        control.begin_batch();
        assert!(!control.finish_batch(false));
    }

    #[test]
    fn request_during_zero_result_batch_is_not_lost() {
        let mut control = SortAvailabilityBackfillControl::default();
        assert!(control.request());
        control.begin_batch();
        assert!(!control.request());
        assert!(control.finish_batch(false));
        control.begin_batch();
        assert!(!control.finish_batch(false));
    }

    #[test]
    fn failed_batch_retries_only_after_an_explicit_overlapping_request() {
        let mut control = SortAvailabilityBackfillControl::default();
        assert!(control.request());
        control.begin_batch();
        assert!(!control.fail());
        assert!(control.request());
        control.begin_batch();
        assert!(!control.fail());

        assert!(control.request());
        control.begin_batch();
        assert!(!control.request());
        assert!(control.fail());
        control.begin_batch();
        assert!(!control.fail());
    }

    #[test]
    fn bounded_batches_continue_and_stop_prevents_restart() {
        let mut control = SortAvailabilityBackfillControl::default();
        assert!(control.request());
        control.begin_batch();
        assert!(control.finish_batch(true));
        control.stop();
        assert!(!control.request());
        assert!(!control.finish_batch(true));
    }
}

#[tauri::command]
async fn search_source(
    state: State<'_, AppState>,
    query: String,
    source_ids: Vec<String>,
    kinds: Vec<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    isrc: Option<String>,
    search_id: Option<String>,
    request_id: Option<String>,
) -> Result<discovery::DiscoveryPage> {
    state
        .provider_reads
        .run(request_id, || async {
            let mut page = discovery::search_source(
                &state.discovery,
                query,
                source_ids,
                kinds,
                cursor,
                limit,
                isrc,
                search_id,
            )
            .await?;
            for result in &mut page.results {
                if let Some(track) = &result.track {
                    result.track = Some(state.store.cache_track(track)?);
                }
            }
            Ok(page)
        })
        .await
}
#[tauri::command]
async fn list_search_sources(
    state: State<'_, AppState>,
) -> Result<Vec<discovery::SourceCapability>> {
    discovery::source_registry_with_remote(&state.connections, Some(&state.remote_playlists)).await
}
#[tauri::command]
async fn register_chapter_track(state: State<'_, AppState>, mut track: Track) -> Result<Track> {
    let range = track
        .playback_range
        .clone()
        .ok_or("Chapter playback range is missing")?;
    if !range.start_time.is_finite()
        || !range.end_time.is_finite()
        || range.start_time < 0.0
        || range.end_time <= range.start_time
        || range
            .parent_duration_seconds
            .is_some_and(|d| !d.is_finite() || d < range.end_time)
        || track.id != format!("chapter:{}:{}", range.parent_track_id, range.chapter_id)
        || track.provider != "chapter"
    {
        return Err("Invalid chapter identity or range".into());
    }
    let candidate: Candidate = serde_json::from_value(track.provenance["audioCandidate"].clone())
        .map_err(|_| "Chapter source is missing")?;
    acquisition::validate_source_url(&range.source_url, &candidate.provider)?;
    if candidate.url != range.source_url
        || !candidate.chapters.as_ref().is_some_and(|chapters| {
            chapters.iter().any(|c| {
                c.id == range.chapter_id
                    && c.start_seconds == range.start_time
                    && c.end_seconds == range.end_time
            })
        })
    {
        return Err("Chapter boundaries do not match the inspected source".into());
    }
    let parent: Track = serde_json::from_value(track.provenance["parentTrack"].clone())
        .map_err(|_| "Chapter parent track is missing")?;
    if parent.id != range.parent_track_id {
        return Err("Chapter parent identity does not match".into());
    }
    let refreshed = inspect_youtube_source(state.clone(), range.source_url.clone()).await?;
    if !refreshed.chapters.as_ref().is_some_and(|chapters| {
        chapters.iter().any(|c| {
            c.id == range.chapter_id
                && c.start_seconds == range.start_time
                && c.end_seconds == range.end_time
        })
    }) {
        return Err(
            "Chapter boundaries changed. Refresh this source before playing the chapter.".into(),
        );
    }
    track.provenance["audioCandidate"] =
        serde_json::to_value(refreshed).map_err(|e| e.to_string())?;
    state.store.cache_track(&parent)?;
    state.store.cache_track(&track)
}
#[tauri::command]
async fn poll_search_source(
    state: State<'_, AppState>,
    search_id: String,
    source_id: String,
) -> Result<discovery::DiscoveryPage> {
    discovery::poll_search_source(&state.discovery, &search_id, &source_id).await
}
#[tauri::command]
async fn inspect_youtube_source(state: State<'_, AppState>, url: String) -> Result<Candidate> {
    let settings = state.store.settings()?;
    let provider = if url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| h == "soundcloud.com" || h.ends_with(".soundcloud.com"))
    {
        "soundcloud"
    } else {
        "youtube"
    };
    acquisition::validate_source_url(&url, provider)?;
    let source = youtube_music::inspect_url_with_runtime(
        &url,
        &settings.yt_dlp_path,
        &settings.js_runtime_path,
    )
    .await?;
    let chapters = source
        .chapters
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let start = c["start_time"].as_f64()?;
            let end = c["end_time"].as_f64()?;
            let title = c["title"].as_str()?.trim();
            if !start.is_finite()
                || !end.is_finite()
                || start < 0.0
                || end <= start
                || title.is_empty()
                || source.duration_seconds.is_some_and(|d| end > d)
            {
                return None;
            }
            Some(Chapter {
                id: format!("chapter-{index}-{}", start.to_bits()),
                title: title.into(),
                start_seconds: start,
                end_seconds: end,
                confidence: "source_metadata_unverified".into(),
                suggested_track: None,
                suggestion_confidence: None,
            })
        })
        .collect::<Vec<_>>();
    let mut inspected = Candidate {
        id: format!("{provider}:{}", source.id),
        provider: provider.into(),
        url: url.clone(),
        title: source.title.clone(),
        uploader: source.channel.clone(),
        artwork_url: source.thumbnail_url.clone(),
        chapters: Some(chapters),
        duration_seconds: source.duration_seconds,
        permission: "unknown".into(),
        confidence: "review".into(),
        ..Candidate::default()
    };
    inspected.source_metadata = Some(serde_json::to_value(source).map_err(|e| e.to_string())?);
    Ok(inspected)
}
#[tauri::command]
async fn enrich_search_result(state: State<'_, AppState>, mut track: Track) -> Result<Track> {
    if !matches!(track.provider.as_str(), "youtube" | "soundcloud") {
        return Ok(track);
    }
    let candidate = inspect_youtube_source(state.clone(), track.source_url.clone()).await?;
    let source = candidate
        .source_metadata
        .as_ref()
        .unwrap_or(&serde_json::Value::Null)
        .clone();
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    let inferred = matches!(
        track.provenance["artistSource"].as_str(),
        Some("unknown" | "inferred_from_title")
    );
    if (track.artist.is_empty() || inferred)
        && source["artist"].as_str().is_some_and(|s| !s.is_empty())
    {
        track.artist = source["artist"].as_str().unwrap().into();
        track.provenance["artistSource"] = serde_json::json!("source_metadata");
    }
    if let Some(song) = source["song"].as_str().filter(|s| !s.is_empty()) {
        track.title = song.into();
    }
    if track.album.is_none() {
        track.album = source["album"].as_str().map(str::to_owned);
    }
    if track.genre.is_none() {
        track.genre = source["sourceGenres"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }
    track.duration_seconds = candidate.duration_seconds.or(track.duration_seconds);
    track.artwork_url = candidate.artwork_url.clone().or(track.artwork_url);
    track.provenance["sourceMetadata"] = source;
    track.provenance["audioCandidate"] =
        serde_json::to_value(candidate).map_err(|e| e.to_string())?;
    state.store.cache_track(&track)
}
#[tauri::command]
fn get_autotag_jobs(state: State<'_, AppState>) -> Result<Vec<autotag::AutoTagJob>> {
    state.autotag.records()
}
#[tauri::command]
fn get_autotag_settings(state: State<'_, AppState>) -> Result<autotag::AutoTagSettings> {
    state.autotag.settings()
}
#[tauri::command]
fn save_autotag_settings(
    state: State<'_, AppState>,
    enabled: bool,
    shazam_fallback: bool,
) -> Result<autotag::AutoTagSettings> {
    state.autotag.save_settings(enabled, shazam_fallback)
}
#[tauri::command]
fn retry_autotag(state: State<'_, AppState>, asset_id: String) -> Result<bool> {
    state.autotag.retry(&asset_id)
}
#[tauri::command]
async fn repair_download_source_tags(
    state: State<'_, AppState>,
    asset_id: String,
) -> Result<autotag::DownloadSourceTagRepairResult> {
    state.autotag.repair_download_source_tags(&asset_id).await
}
#[tauri::command]
fn review_autotag_candidate(
    state: State<'_, AppState>,
    asset_id: String,
    provider_id: String,
) -> Result<autotag::AutoTagJob> {
    state.autotag.review(&asset_id, &provider_id)
}
#[tauri::command]
async fn apply_autotag_review(
    state: State<'_, AppState>,
    asset_id: String,
    field_keys: Option<Vec<String>>,
) -> Result<LocalAsset> {
    state
        .autotag
        .apply_review(&asset_id, &field_keys.unwrap_or_default())
        .await
}

#[tauri::command]
async fn apply_recognition_review(
    state: State<'_, AppState>,
    asset_id: String,
    field_keys: Vec<String>,
) -> Result<LocalAsset> {
    state
        .autotag
        .apply_recognition_review(&asset_id, &field_keys)
        .await
}

#[tauri::command]
fn get_automation_status(state: State<'_, AppState>) -> serde_json::Value {
    state.automation.status()
}
#[tauri::command]
fn set_automation_permissions(
    state: State<'_, AppState>,
    control: bool,
    manage: bool,
) -> Result<serde_json::Value> {
    state.automation.set_permission_values(control, manage)
}
#[tauri::command]
fn get_playback_engine_settings(
    state: State<'_, AppState>,
) -> Result<playback_engine_settings::EngineSettings> {
    playback_engine_settings::status(
        &state.store,
        state.playback.native_enabled(),
        std::env::var("BURRATA_PLAYBACK_ENGINE").is_ok(),
    )
}
#[tauri::command]
async fn restart_with_playback_engine(
    state: State<'_, AppState>,
    engine: playback_engine_settings::Engine,
    checkpoint: serde_json::Value,
) -> Result<()> {
    state.playback.restart_with_engine(engine, checkpoint).await
}
#[tauri::command]
fn get_playback_snapshot(state: State<'_, AppState>) -> serde_json::Value {
    state.playback.snapshot()
}
#[tauri::command]
async fn playback_command(
    state: State<'_, AppState>,
    command: serde_json::Value,
) -> Result<serde_json::Value> {
    state.playback.execute(command).await
}

// Queue projections deliberately contain only occurrence display identity.
// Compare source metadata against the active-track snapshot, where the WebView
// publisher actually sends it, while retaining the compact queue ID check.
fn webview_handoff_track_matches(
    active_track: &serde_json::Value,
    queue_track: Option<&serde_json::Value>,
    expected: &Track,
) -> bool {
    queue_track.and_then(|track| track["id"].as_str()) == Some(expected.id.as_str())
        && active_track["id"].as_str() == Some(expected.id.as_str())
        && active_track["sourceUrl"].as_str() == Some(expected.source_url.as_str())
        && active_track["durationSeconds"].as_f64() == expected.duration_seconds
}

/// CENC sources are native-decoder inputs, never WebView media URLs. Keep this
/// check ahead of transport registration and preview serialization so the
/// lease-bearing loopback URL cannot cross the Tauri command boundary.
fn ensure_webview_compatible_source(
    source: &native_output::AudioSource,
) -> std::result::Result<(), &'static str> {
    if source.cenc.is_some() {
        Err("Encrypted lossless audio requires native playback")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod webview_handoff_snapshot_tests {
    use super::*;
    #[test]
    fn compact_queue_uses_active_track_metadata_and_rejects_changed_sources() {
        let expected = Track {
            id: "youtube:6MAzUT1YhWE".into(),
            source_url: "https://www.youtube.com/watch?v=6MAzUT1YhWE".into(),
            duration_seconds: Some(6128.0),
            ..Track::default()
        };
        let queue =
            serde_json::json!({"id":expected.id,"artist":"Fred again..","title":"Rooftop Live"});
        let active = serde_json::json!({"id":expected.id,"sourceUrl":expected.source_url,"durationSeconds":6128.0});
        assert!(webview_handoff_track_matches(
            &active,
            Some(&queue),
            &expected
        ));
        let mut changed = active.clone();
        changed["sourceUrl"] = serde_json::json!("https://www.youtube.com/watch?v=otherSource");
        assert!(!webview_handoff_track_matches(
            &changed,
            Some(&queue),
            &expected
        ));
        changed = active.clone();
        changed["durationSeconds"] = serde_json::json!(42.0);
        assert!(!webview_handoff_track_matches(
            &changed,
            Some(&queue),
            &expected
        ));
        assert!(!webview_handoff_track_matches(&active, None, &expected));
        assert!(!webview_handoff_track_matches(
            &active,
            Some(&serde_json::json!({"id":"different-occurrence-track"})),
            &expected
        ));
    }
}

#[cfg(test)]
mod webview_cenc_boundary_tests {
    use super::*;
    use std::path::PathBuf;

    fn source(cenc: bool) -> native_output::AudioSource {
        native_output::AudioSource {
            input: "http://127.0.0.1/webview-audio/test?token=redacted".into(),
            ffmpeg: PathBuf::new(),
            headers: vec![],
            decoder_hint: None,
            cenc: cenc.then(|| {
                native_output::EncryptedMp4Audio::new(
                    native_output::CencDecryptionKey::from_hex("0123456789abcdef0123456789abcdef")
                        .unwrap(),
                )
            }),
        }
    }

    #[test]
    fn encrypted_sources_are_rejected_before_webview_preview_registration() {
        assert_eq!(
            ensure_webview_compatible_source(&source(true)),
            Err("Encrypted lossless audio requires native playback")
        );
        assert!(ensure_webview_compatible_source(&source(false)).is_ok());
    }
}

#[tauri::command]
async fn handoff_resolved_playback_source(
    state: State<'_, AppState>,
    handoff: playback::ResolvedSourceHandoff,
) -> Result<serde_json::Value> {
    let automation = state.automation.clone();
    let resolution = state.playback_resolution.clone();
    state
        .playback
        .handoff_webview_resolved_source(handoff, move |request, track_id| {
            let Ok(gate) = resolution.lock() else {
                return false;
            };
            if gate.generation != request.source_generation {
                return false;
            }
            let current = automation.playback_tracking_snapshot();
            if current["engine"].as_str() == Some("native")
                || current["status"].as_str() != Some("buffering")
                || current["trackId"].as_str() != Some(track_id)
                || current["sourceGeneration"].as_u64() != Some(request.source_generation)
                || current["seekSerial"].as_u64() != Some(request.seek_serial)
                || current["sessionId"].as_str() != Some(request.session_id.as_str())
                || current["entryId"].as_str() != Some(request.entry_id.as_str())
                || current["currentIndex"].as_u64() != Some(request.index as u64)
            {
                return false;
            }
            let expected = &request.tracks[request.index];
            let mut revision = None;
            for offset in (0..request.tracks.len()).step_by(200) {
                let page = automation.playback_snapshot_page(offset, 200);
                if page["queueTotal"].as_u64() != Some(request.tracks.len() as u64) {
                    return false;
                }
                if let Some(previous) = &revision {
                    if previous != &page["queueRevision"] {
                        return false;
                    }
                } else {
                    revision = Some(page["queueRevision"].clone());
                }
                let Some(rows) = page["queue"].as_array() else {
                    return false;
                };
                let Some(ids) = page["queueEntryIds"].as_array() else {
                    return false;
                };
                let length = (request.tracks.len() - offset).min(200);
                if rows.len() != length || ids.len() != length {
                    return false;
                }
                for index in 0..length {
                    if rows[index]["id"].as_str()
                        != Some(request.tracks[offset + index].id.as_str())
                        || ids[index].as_str() != Some(request.entry_ids[offset + index].as_str())
                    {
                        return false;
                    }
                }
                if (offset..offset + length).contains(&request.index)
                    && !webview_handoff_track_matches(
                        &current["track"],
                        rows.get(request.index - offset),
                        expected,
                    )
                {
                    return false;
                }
            }
            true
        })
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn handoff_webview_playback(
    state: State<'_, AppState>,
    handoff: playback::WebViewPlaybackHandoff,
) -> Result<serde_json::Value> {
    if state.playback.native_enabled() {
        return Err("Playback ownership has already transferred".into());
    }
    let idx = handoff.index;
    let expected_track_id = handoff
        .tracks
        .get(idx)
        .map(|track| track.id.clone())
        .ok_or_else(|| "Playback queue position is out of range".to_owned())?;
    let expected_track = handoff.tracks[idx].clone();
    let automation = state.automation.clone();
    let identification = state.webview_identification.clone();
    let expected_source_generation = handoff.source_generation;
    let expected_seek_serial = handoff.seek_serial;
    let expected_session = handoff.session_id.clone();
    let expected_entry = handoff.entry_id.clone();
    state
        .playback
        .handoff_webview(handoff, move |generation, track_id| {
            let current = automation.playback_tracking_snapshot();
            let queue = automation.playback_snapshot_page(idx, 1);
            let queue_track = queue["queue"].as_array().and_then(|tracks| tracks.first());
            let current_status = current["status"].as_str();
            let active_webview_audio = if current_status == Some("paused") {
                current["paused"].as_bool() == Some(true)
            } else {
                matches!(current_status, Some("playing" | "buffering"))
                    && current["paused"].as_bool() == Some(false)
            };
            current["engine"].as_str() != Some("native")
                && active_webview_audio
                && current["trackId"]
                    .as_str()
                    .or_else(|| current["track"]["id"].as_str())
                    == Some(expected_track_id.as_str())
                && current["sourceGeneration"].as_u64() == Some(expected_source_generation)
                && current["seekSerial"].as_u64() == Some(expected_seek_serial)
                && current["sessionId"].as_str() == Some(expected_session.as_str())
                && current["entryId"].as_str() == Some(expected_entry.as_str())
                && current["currentIndex"].as_u64() == Some(idx as u64)
                && webview_handoff_track_matches(&current["track"], queue_track, &expected_track)
                && queue["queueEntryIds"]
                    .as_array()
                    .and_then(|ids| ids.first())
                    .and_then(serde_json::Value::as_str)
                    == Some(expected_entry.as_str())
                && identification.matches_registered_youtube_source(generation, track_id)
        })
        .await
        .map_err(Into::into)
}

#[tauri::command]
fn automation_playback_state(
    state: State<'_, AppState>,
    playback: serde_json::Value,
) -> serde_json::Value {
    if state.playback.native_enabled() {
        state.webview_identification.cancel_capture();
        return state.automation.update_playback_state(playback);
    }
    let previous = state.automation.playback_tracking_snapshot();
    let ack = state.automation.update_playback_state(playback.clone());
    if ack["accepted"].as_bool() == Some(true)
        && (playback["status"].as_str() != Some("playing")
            || playback_snapshot_track_id(&previous) != playback_snapshot_track_id(&playback)
            || previous["sessionId"] != playback["sessionId"]
            || previous["seekSerial"] != playback["seekSerial"]
            || previous["sourceGeneration"] != playback["sourceGeneration"])
    {
        state.webview_identification.cancel_capture();
    }
    ack
}

fn playback_snapshot_track_id(snapshot: &serde_json::Value) -> Option<String> {
    snapshot["trackId"]
        .as_str()
        .or_else(|| snapshot["track"]["id"].as_str())
        .map(str::to_owned)
}
#[tauri::command]
fn set_automation_frontend_ready(state: State<'_, AppState>, ready: bool) {
    state.automation.set_frontend_ready(ready);
}
#[tauri::command]
fn automation_layout_complete(
    state: State<'_, AppState>,
    request_id: String,
    layout: serde_json::Value,
) {
    state
        .automation
        .complete_layout_request(&request_id, layout);
}

#[tauri::command]
fn automation_stereo_diagnostic_complete(
    state: State<'_, AppState>,
    request_id: String,
    track_id: String,
    session_id: String,
    source_generation: u64,
    seek_serial: u64,
    current_index: i64,
    queue_total: u64,
    entry_id: String,
    queue_track_id: String,
    capture: serde_json::Value,
) -> Result<()> {
    state.automation.complete_stereo_diagnostic_request(
        &request_id,
        serde_json::json!({
            "trackId":track_id,"sessionId":session_id,"sourceGeneration":source_generation,
            "seekSerial":seek_serial,"currentIndex":current_index,"queueTotal":queue_total,
            "entryId":entry_id,"queueTrackId":queue_track_id,"capture":capture
        }),
    )
}

#[tauri::command]
fn automation_player_panel_complete(
    state: State<'_, AppState>,
    request_id: String,
    panel: String,
    status: String,
) {
    state
        .automation
        .complete_player_panel_request(&request_id, &panel, &status);
}

#[tauri::command]
fn automation_ui_navigation_complete(
    state: State<'_, AppState>,
    request_id: String,
    destination: String,
    status: String,
    release_revision: Option<String>,
    track_count: Option<usize>,
    collection_revision: Option<String>,
) {
    state.automation.complete_ui_navigation_request(
        &request_id,
        &destination,
        &status,
        release_revision.as_deref(),
        track_count,
        collection_revision.as_deref(),
    );
}

#[tauri::command]
fn automation_search_ui_complete(
    state: State<'_, AppState>,
    request_id: String,
    status: String,
    source_id: String,
    kind: String,
    result_counts: serde_json::Value,
    source_coverage: Option<serde_json::Value>,
    error_code: Option<String>,
) {
    state.automation.complete_search_ui_request(
        &request_id,
        &status,
        &source_id,
        &kind,
        &result_counts,
        source_coverage.as_ref(),
        error_code.as_deref(),
    );
}

#[tauri::command]
fn automation_search_controls_complete(
    state: State<'_, AppState>,
    request_id: String,
    result: serde_json::Value,
) {
    state
        .automation
        .complete_search_control_request(&request_id, &result);
}

#[tauri::command]
fn automation_identify_complete(
    state: State<'_, AppState>,
    capture_id: String,
    result: serde_json::Value,
    error: Option<String>,
) {
    state
        .automation
        .complete_identify_request(&capture_id, result, error);
}

#[tauri::command]
fn automation_queued_playback_accept(
    state: State<'_, AppState>,
    operation_id: String,
    request_id: String,
    receipt: serde_json::Value,
    playback: serde_json::Value,
) -> serde_json::Value {
    state
        .automation
        .accept_queued_playback(&operation_id, &request_id, receipt, playback)
}
#[tauri::command]
fn automation_queued_playback_update(
    state: State<'_, AppState>,
    operation_id: String,
    request_id: String,
    status: String,
    detail: serde_json::Value,
) -> serde_json::Value {
    state
        .automation
        .update_queued_playback(&operation_id, &request_id, &status, detail)
}
#[tauri::command]
fn automation_queued_playback_status(
    state: State<'_, AppState>,
    operation_id: String,
) -> serde_json::Value {
    state.automation.queued_playback_status(&operation_id)
}
#[tauri::command]
fn automation_playback_complete(
    state: State<'_, AppState>,
    request_id: String,
    playback: serde_json::Value,
    error: Option<String>,
) -> serde_json::Value {
    let ack = state.automation.update_playback_state(playback.clone());
    state
        .automation
        .complete_playback_request(&request_id, playback, error);
    ack
}
#[tauri::command]
fn get_browser_status(state: State<'_, AppState>) -> browser_bridge::BrowserStatus {
    state.browser.status()
}

/// Invalidate cached and in-flight playback work when provider credentials change.
/// Callers must not hold the managed-session state mutex while obtaining a status.
pub(crate) fn invalidate_managed_playback_sources(state: &AppState, providers: &[&str]) {
    if providers.is_empty() {
        return;
    }
    for provider in providers {
        state
            .acquisition
            .invalidate_playback_provider_session(provider);
    }
    cancel_preview_warmup(&state.preview_warmup_cancel);
    if let Ok(mut gate) = state.playback_resolution.lock() {
        // The frontend owns generation allocation. Repeated session events must
        // cancel existing work without consuming future frontend generations.
        let generation = gate.cancel_for_provider_session_change();
        state.webview_audio_transport.cancel_older_than(generation);
        state.webview_audio_transcoder.cancel_older_than(generation);
    }
    state.playback.cancel_all_pending_native_sources();
    state.webview_identification.clear();
}

fn changed_managed_playback_providers(
    previous: &managed_provider_session::ManagedProviderSessionStatus,
    current: &managed_provider_session::ManagedProviderSessionStatus,
) -> Vec<&'static str> {
    let stopped_changed = (previous.mode == "stopped") != (current.mode == "stopped");
    let mut changed = Vec::new();
    for (provider, previous_id, current_id) in [
        (
            "spotify",
            &previous.spotify_account_id,
            &current.spotify_account_id,
        ),
        (
            "youtube",
            &previous.youtube_account_id,
            &current.youtube_account_id,
        ),
    ] {
        if stopped_changed
            || previous_id != current_id
            || previous.account_revisions.get(provider) != current.account_revisions.get(provider)
            || (provider == "youtube"
                && previous.youtube_reconnect_required != current.youtube_reconnect_required)
        {
            changed.push(provider);
        }
    }
    changed
}

fn observe_managed_playback_session(
    state: &AppState,
    current: managed_provider_session::ManagedProviderSessionStatus,
    explicitly_changed: &[&str],
) -> managed_provider_session::ManagedProviderSessionStatus {
    let mut changed: Vec<&str> =
        if let Ok(mut previous) = state.playback_provider_session_status.lock() {
            let changed = changed_managed_playback_providers(&previous, &current);
            *previous = current.clone();
            changed
        } else {
            vec!["spotify", "youtube"]
        };
    for provider in explicitly_changed {
        if matches!(*provider, "spotify" | "youtube") && !changed.contains(provider) {
            changed.push(provider);
        }
    }
    invalidate_managed_playback_sources(state, &changed);
    current
}

#[tauri::command]
fn get_managed_provider_session_status(
    state: State<'_, AppState>,
) -> managed_provider_session::ManagedProviderSessionStatus {
    observe_managed_playback_session(&state, state.managed_provider_session.status(), &[])
}

#[tauri::command]
fn start_managed_provider_login(
    state: State<'_, AppState>,
    provider: String,
) -> Result<managed_provider_session::ManagedProviderSessionStatus> {
    let providers = if matches!(provider.as_str(), "spotify" | "youtube") {
        vec![provider.as_str()]
    } else {
        vec![]
    };
    // Fence work already in flight before the login window can change credentials.
    invalidate_managed_playback_sources(&state, &providers);
    let result = state.managed_provider_session.start_login(&provider);
    observe_managed_playback_session(&state, state.managed_provider_session.status(), &providers);
    result
}

#[tauri::command]
fn start_managed_provider_background(
    state: State<'_, AppState>,
) -> Result<managed_provider_session::ManagedProviderSessionStatus> {
    let result = state.managed_provider_session.start_background();
    observe_managed_playback_session(&state, state.managed_provider_session.status(), &[]);
    result
}

#[tauri::command]
fn stop_managed_provider_session(
    state: State<'_, AppState>,
) -> Result<managed_provider_session::ManagedProviderSessionStatus> {
    invalidate_managed_playback_sources(&state, &["spotify", "youtube"]);
    let result = state.managed_provider_session.stop();
    observe_managed_playback_session(
        &state,
        state.managed_provider_session.status(),
        &["spotify", "youtube"],
    );
    result
}

#[tauri::command]
async fn create_verification_playlist(
    state: State<'_, AppState>,
    provider: String,
) -> Result<serde_json::Value> {
    state
        .managed_provider_session
        .create_verification_playlist(&provider)
        .await
}

#[tauri::command]
async fn confirm_managed_provider_account(
    state: State<'_, AppState>,
    provider: String,
) -> Result<managed_provider_session::ManagedProviderSessionStatus> {
    let providers = if matches!(provider.as_str(), "spotify" | "youtube") {
        vec![provider.as_str()]
    } else {
        vec![]
    };
    invalidate_managed_playback_sources(&state, &providers);
    let result = state
        .managed_provider_session
        .confirm_account(&provider)
        .await;
    observe_managed_playback_session(&state, state.managed_provider_session.status(), &providers);
    result
}

#[tauri::command]
fn create_youtube_handoff_pairing_code(
    state: State<'_, AppState>,
) -> Result<managed_provider_session::YoutubeHandoffPairingCode> {
    state
        .managed_provider_session
        .create_youtube_handoff_pairing_code()
}
#[tauri::command]
fn request_browser_import(state: State<'_, AppState>, url: String) -> Result<String> {
    state.browser.request_import(&url)
}
pub(crate) async fn read_collection_snapshot(
    url: &str,
    connections: &connections::ConnectionManager,
) -> Result<browser_bridge::BrowserSnapshot> {
    let provider = browser_bridge::provider_from_url(&url)?;
    let snapshot = if matches!(provider, "beatport" | "traxsource") {
        let source = providers::provider(provider)?.get_chart(&url).await?;
        let entries = source
            .tracks
            .into_iter()
            .enumerate()
            .map(|(index, t)| browser_bridge::BrowserEntry {
                metadata: serde_json::json!({"remixers":t.remixers.clone().map(serde_json::to_value).and_then(std::result::Result::ok).unwrap_or_else(||t.provenance["remixers"].clone())}),
                provider_id: Some(t.provider_id),
                source_url: Some(t.source_url),
                artist: t.artist,
                title: t.title,
                version: t.version,
                position: (index + 1) as u32,
                rank: t.rank,
                duration_seconds: t.duration_seconds,
                bpm: t.bpm,
                key: t.key,
                album: t.album,
                label: t.label,
                genre: t.genre,
                release_date: t.release_date,
                artwork_url: t.artwork_url,
                isrc: t.isrc,
                catalog_number: t.catalog_number,
            })
            .collect::<Vec<_>>();
        let top100 = url.ends_with("/top-100") || url.ends_with("/top");
        browser_bridge::BrowserSnapshot {
            failed_sources: vec![],
            album_urls: vec![],
            schema_version: 1,
            provider: provider.into(),
            kind: "chart".into(),
            title: source.title,
            raw_page_title: None,
            source_url: source.source_url,
            captured_at: chrono::Utc::now().to_rfc3339(),
            complete: top100 && entries.len() == 100,
            expected_total: top100.then_some(100),
            warnings: if top100 {
                vec![]
            } else {
                vec![
                    "Direct page capture has no verified total; retained as a partial import."
                        .into(),
                ]
            },
            entries,
        }
    } else {
        collection_sources::import_source(&url, connections).await?
    };
    Ok(snapshot)
}

#[tauri::command]
async fn import_collection_url(
    state: State<'_, AppState>,
    url: String,
) -> Result<browser_bridge::BrowserImportResult> {
    let snapshot = read_collection_snapshot(&url, &state.connections).await?;
    let mut chart = browser_bridge::validate_snapshot(&snapshot)?;
    for track in &mut chart.tracks {
        track.provenance["browserImport"] = serde_json::json!(false);
        track.provenance["captureMethod"] = serde_json::json!("direct");
        track.provenance["identity"] = serde_json::json!("unverified source metadata");
    }
    state.store.import_browser_chart(&chart, &snapshot)
}
#[tauri::command]
async fn preview_collection_import(
    state: State<'_, AppState>,
    url: String,
) -> Result<serde_json::Value> {
    let snapshot = read_collection_snapshot(&url, &state.connections).await?;
    collection_imports::preview(&state.store, snapshot)
}
#[tauri::command]
fn apply_collection_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    preview_id: String,
    destination_collection_id: Option<String>,
    duplicate_policy: Option<String>,
    folder_path: Option<String>,
    section_name: Option<String>,
) -> Result<browser_bridge::BrowserImportResult> {
    let result = collection_imports::apply_with_destination(
        &state.store,
        &preview_id,
        destination_collection_id.as_deref(),
        duplicate_policy.as_deref(),
        folder_path.as_deref(),
        section_name.as_deref(),
    )?;
    let _ = app.emit("browser-imported", &result);
    Ok(result)
}
#[tauri::command]
async fn retry_import_failures(
    state: State<'_, AppState>,
    import_id: String,
) -> Result<serde_json::Value> {
    collection_imports::retry(&state.store, &import_id).await
}
#[tauri::command]
async fn enrich_import_track(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<serde_json::Value> {
    collection_imports::enrich(&state.store, &track_id).await
}
#[tauri::command]
fn set_import_playback_mode(
    state: State<'_, AppState>,
    track_id: String,
    mode: String,
) -> Result<Track> {
    if !matches!(mode.as_str(), "source" | "lookup") {
        return Err("Choose source or lookup".into());
    }
    let mut track = state
        .store
        .get_track(&track_id)?
        .ok_or("Saved track not found")?;
    if track.playback_range.is_some() || track.provider == "radio" {
        return Err("Chapters and radio keep their exact source".into());
    }
    if !track.provenance.is_object() {
        track.provenance = serde_json::json!({});
    }
    track.provenance["playbackMode"] = serde_json::json!(mode);
    if !track.provenance["manualFields"].is_object() {
        track.provenance["manualFields"] = serde_json::json!({});
    }
    track.provenance["manualFields"]["playbackMode"] = serde_json::json!(true);
    state.store.cache_track(&track)
}

#[tauri::command]
fn list_browser_imports(
    state: State<'_, AppState>,
) -> Result<Vec<browser_bridge::BrowserImportResult>> {
    state.store.list_browser_imports()
}
#[tauri::command]
fn load_browser_import(state: State<'_, AppState>, id: String) -> Result<Chart> {
    state.store.load_browser_import(&id)
}
#[tauri::command]
fn import_browser_snapshot(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    snapshot: browser_bridge::BrowserSnapshot,
) -> Result<browser_bridge::BrowserImportResult> {
    let chart = browser_bridge::validate_snapshot(&snapshot)?;
    let result = state.store.import_browser_chart(&chart, &snapshot)?;
    let _ = app.emit("browser-imported", &result);
    Ok(result)
}
#[tauri::command]
async fn start_candidate_preview(
    state: State<'_, AppState>,
    candidate: Candidate,
    request_generation: Option<u64>,
) -> Result<serde_json::Value> {
    state
        .candidate_preview
        .start(&candidate, request_generation)
        .await
}
#[tauri::command]
fn candidate_preview_control(
    state: State<'_, AppState>,
    action: String,
    value: Option<f64>,
) -> Result<()> {
    state.candidate_preview.control(&action, value)
}
#[tauri::command]
fn stop_candidate_preview(state: State<'_, AppState>, session_id: Option<String>) -> Result<()> {
    state.candidate_preview.stop_session(session_id.as_deref())
}

#[tauri::command]
fn list_genres(provider_id: String) -> Result<Vec<Genre>> {
    Ok(providers::provider(&provider_id)?.list_genres())
}
#[tauri::command]
async fn get_chart(
    state: State<'_, AppState>,
    provider_id: String,
    url: String,
    refresh: bool,
) -> Result<Chart> {
    let adapter = providers::provider(&provider_id)?;
    let cached = state.store.latest_snapshot(&provider_id, &url)?;
    if !refresh {
        if let Some(c) = &cached {
            if chrono::DateTime::parse_from_rfc3339(&c.fetched_at)
                .is_ok_and(|t| chrono::Utc::now().signed_duration_since(t).num_hours() < 6)
            {
                return state.store.load_snapshot(&c.id);
            }
        }
    }
    match adapter.get_chart(&url).await {
        Ok(chart) => state.store.save_chart(&chart),
        Err(e) => match cached {
            Some(c) => {
                let mut c = state.store.load_snapshot(&c.id)?;
                c.stale = true;
                c.warning = Some(format!("Showing cached chart: {e}"));
                Ok(c)
            }
            None => Err(e),
        },
    }
}
#[tauri::command]
fn list_snapshots(
    state: State<'_, AppState>,
    provider_id: String,
    url: String,
) -> Result<Vec<Chart>> {
    state.store.list_snapshots(&provider_id, &url)
}
#[tauri::command]
fn get_chart_memberships(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<Vec<ChartMembership>> {
    state.store.chart_memberships(&track_id)
}
#[tauri::command]
fn load_snapshot(state: State<'_, AppState>, id: String) -> Result<Chart> {
    state.store.load_snapshot(&id)
}
#[tauri::command]
async fn get_track_details(state: State<'_, AppState>, track: Track) -> Result<Track> {
    if track
        .provenance
        .get("browserImport")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return state.store.save_track(&track);
    }
    if let Some(mut cached) = state.store.get_track(&track.id)? {
        if cached
            .provenance
            .get("detailsFetchedAt")
            .and_then(|v| v.as_str())
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .is_some_and(|date| chrono::Utc::now().signed_duration_since(date).num_days() < 7)
        {
            cached.rank = track.rank;
            return Ok(cached);
        }
    }
    let mut details = providers::provider(&track.provider)?
        .get_track_details(&track)
        .await?;
    if !details.provenance.is_object() {
        details.provenance = serde_json::json!({});
    }
    details.provenance["detailsFetchedAt"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
    state.store.cache_track(&details)
}
#[tauri::command]
async fn resolve_preview(
    state: State<'_, AppState>,
    track: Track,
    request_generation: Option<u64>,
) -> Result<serde_json::Value> {
    let mut approximate_excerpt = None;
    let mut local_asset_id = None;
    let preview = if let Some(path) = &track.local_path {
        if let Some(asset) = state.store.get_asset_by_path(path)? {
            let settings = state.store.settings()?;
            if !crate::compilation_excerpt::asset_proof_is_eligible(
                &track,
                &asset.track.provenance,
                settings.matching_level,
            ) || !crate::compilation_excerpt::asset_file_hash_matches(&asset)
            {
                metadata_preview(&track)
            } else {
                local_asset_id = Some(asset.id.clone());
                if asset.track.provenance["automaticExcerpt"].is_object() {
                    let proof = &asset.track.provenance["automaticExcerpt"];
                    approximate_excerpt = Some(serde_json::json!({
                        "label":"Approximate album excerpt",
                        "source":{"provider":proof["source"]["provider"],"id":proof["source"]["id"]}
                    }));
                }
                Preview {
                    kind: "local".into(),
                    url: state.local_audio.url_for(&asset.id)?,
                    can_seek: true,
                    can_volume: true,
                    can_autoplay: true,
                    is_excerpt: approximate_excerpt.is_some(),
                }
            }
        } else {
            metadata_preview(&track)
        }
    } else {
        // Playback must not wait on a storefront metadata request. Missing previews
        // are resolved by the player's YouTube/SoundCloud discovery flow instead.
        metadata_preview(&track)
    };
    let mut value = serde_json::to_value(&preview).map_err(|error| error.to_string())?;
    if let Some(marker) = approximate_excerpt {
        if let Some(object) = value.as_object_mut() {
            object.insert("approximateExcerpt".into(), marker);
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "assetId".into(),
            local_asset_id
                .as_ref()
                .map_or(serde_json::Value::Null, |asset_id| {
                    serde_json::json!(asset_id)
                }),
        );
        object.insert(
            "normalizationRoute".into(),
            serde_json::json!(webview_audio_transport::normalization_route_capability(
                preview.kind == "local"
            )),
        );
    }
    if let Some(generation) = request_generation {
        let (stop, cancel) = {
            let mut gate = state
                .playback_resolution
                .lock()
                .map_err(|_| "Playback resolver unavailable")?;
            let started = gate.begin(generation)?;
            state.webview_identification.clear();
            started
        };
        let source = if preview.kind == "local" {
            if let Some(asset_id) = local_asset_id.as_deref() {
                let asset = state
                    .store
                    .get_asset_by_id(asset_id)?
                    .ok_or("Local audio is not registered in the library")?;
                let settings = state.store.settings()?;
                Some(crate::native_output::AudioSource {
                    input: asset.path,
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: vec![],
                    decoder_hint: None,

                    cenc: None,
                })
            } else {
                None
            }
        } else if preview.kind == "audio" {
            let trusted_preview = state
                .store
                .get_track(&track.id)?
                .is_some_and(|stored| stored.preview_url.as_deref() == Some(preview.url.as_str()));
            if trusted_preview && acquisition::validate_source_url(&preview.url, "direct").is_ok() {
                Some(crate::native_output::AudioSource {
                    input: preview.url.clone(),
                    ffmpeg: tools::executable(&state.store.settings()?.ffmpeg_path, "ffmpeg"),
                    headers: vec![],
                    decoder_hint: None,

                    cenc: None,
                })
            } else {
                None
            }
        } else {
            None
        };
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(generation, &stop);
        if gate.generation == generation && !*cancel.borrow() {
            if let Some(source) = source {
                let bounds = track
                    .playback_range
                    .as_ref()
                    .map(|r| (r.start_time, r.end_time));
                state.webview_identification.register(
                    generation,
                    track.id.clone(),
                    source,
                    if preview.kind == "audio" {
                        playing_track_detections::SourceIdentity::from_validated_url(
                            "preview",
                            &preview.url,
                            None,
                        )
                    } else if preview.kind == "local" {
                        local_asset_id.as_deref().and_then(|asset_id| {
                            playing_track_detections::SourceIdentity::new("local", asset_id)
                        })
                    } else {
                        None
                    },
                    false,
                    bounds,
                );
                if let Some(object) = value.as_object_mut() {
                    object.insert("sourceGeneration".into(), serde_json::json!(generation));
                }
            }
        }
    }
    Ok(value)
}

/// Collect exact paths that the current Tauri scope does not already allow.
/// Call this before adding per-file grants so the scope's allowed-pattern set
/// remains stable throughout classification.
fn asset_paths_needing_explicit_grants(
    scope: &tauri::scope::fs::Scope,
    paths: impl IntoIterator<Item = String>,
    music_root: Option<&Path>,
) -> (Vec<String>, usize) {
    let mut needs_grant = Vec::new();
    let mut already_allowed = 0usize;
    for path in paths {
        // This lexical check only limits which paths need an exact Tauri
        // scope check. It never authorizes a path; all other paths retain the
        // explicit allow_file behavior.
        let could_use_music_root =
            music_root.is_some_and(|root| Path::new(&path).starts_with(root));
        if could_use_music_root && scope.is_allowed(&path) {
            already_allowed += 1;
        } else {
            needs_grant.push(path);
        }
    }
    (needs_grant, already_allowed)
}

/// Resolve the complete safe playback path for the WebView player. This uses the
/// same bounded source search and full-metadata identity checks as native output,
/// and only then returns the short-lived stream URL to the renderer.
fn webview_header_policy(headers: &[(String, String)]) -> serde_json::Value {
    let mut authorization = false;
    let mut cookie = false;
    let mut referer = false;
    let mut origin = false;
    let mut user_agent = false;
    let mut other = false;
    for (name, _) in headers {
        match name.to_ascii_lowercase().as_str() {
            "authorization" => authorization = true,
            "cookie" => cookie = true,
            "referer" => referer = true,
            "origin" => origin = true,
            "user-agent" => user_agent = true,
            _ => other = true,
        }
    }
    serde_json::json!({"headerCount":headers.len().min(10000),"authorization":authorization,
        "cookie":cookie,"referer":referer,"origin":origin,"userAgent":user_agent,"other":other})
}

fn cancel_preview_warmup(slot: &std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>) {
    if let Ok(mut previous) = slot.lock() {
        if let Some(cancel) = previous.take() {
            cancel.send_replace(true);
        }
    }
}

/// The client may shorten the remaining attempt budget but may not extend the
/// configured startup deadline. Legacy settings resolve to twenty seconds.
fn playback_request_budget(settings: &Settings, budget_ms: Option<u64>) -> std::time::Duration {
    let maximum_ms = settings.playback_startup_timeout().as_millis() as u64;
    std::time::Duration::from_millis(budget_ms.unwrap_or(maximum_ms).clamp(1, maximum_ms))
}

fn should_relay_seek_recovery_source(
    seek_recovery: bool,
    track: &Track,
    candidate: Option<&Candidate>,
    source_url: &str,
    local_source: bool,
    is_station: bool,
) -> bool {
    if !seek_recovery || local_source || is_station || track.provider != "youtube" {
        return false;
    }
    let Some(candidate) = candidate else {
        return false;
    };
    let Some(video_id) = candidate.id.strip_prefix("youtube:") else {
        return false;
    };
    if candidate.provider != "youtube"
        || video_id != track.provider_id
        || candidate.source.is_some()
        || candidate
            .chapters
            .as_ref()
            .is_some_and(|chapters| !chapters.is_empty())
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|chapters| !chapters.is_empty())
        || track.local_path.is_some()
        || track.asset_id.is_some()
        || track.playback_range.is_some()
        || track
            .version
            .as_deref()
            .is_some_and(|version| !version.trim().is_empty())
        || !track
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && (5.0..=86_400.0).contains(&duration))
        || acquisition::validate_source_url(&candidate.url, "youtube").is_err()
        || acquisition::youtube_video_id(&candidate.url).as_deref() != Some(video_id)
        || acquisition::validate_source_url(&track.source_url, "youtube").is_err()
        || acquisition::youtube_video_id(&track.source_url).as_deref() != Some(video_id)
    {
        return false;
    }
    let Ok(url) = acquisition::validate_source_url(source_url, "direct") else {
        return false;
    };
    url.host_str()
        .is_some_and(|host| host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
}

fn playback_normalization_route(
    local_source: bool,
    relay_registered: bool,
    transcode_registered: bool,
) -> &'static str {
    webview_audio_transport::normalization_route_capability(
        local_source || relay_registered || transcode_registered,
    )
}

fn playback_relay_required(
    source: &crate::native_output::AudioSource,
    transcode_required: bool,
    seek_recovery_relay: bool,
    normalization_route_required: bool,
) -> bool {
    transcode_required
        || seek_recovery_relay
        || webview_audio_transport::needs_relay_for_normalization(
            source,
            normalization_route_required,
        )
}

#[tauri::command]
async fn resolve_playback_track(
    state: State<'_, AppState>,
    track: Track,
    request_generation: u64,
    selected_candidate: Option<Candidate>,
    budget_ms: Option<u64>,
    failed_candidates: Option<Vec<playback_sources::FailedPlaybackCandidate>>,
    refresh_candidate_fresh: Option<bool>,
    refresh_candidate: Option<Candidate>,
    seek_recovery: Option<bool>,
    attempt_limit: Option<usize>,
    webview_capabilities: Option<webview_audio_transcode::WebviewAudioCapabilities>,
    normalization_route_required: Option<bool>,
) -> Result<serde_json::Value> {
    cancel_preview_warmup(&state.preview_warmup_cancel);
    let failed_candidates = failed_candidates.unwrap_or_default();
    playback_sources::validate_failed_candidates(&failed_candidates)?;
    let settings = state.store.settings()?;
    let explicit_source = playback_sources::source_resolution_is_pinned(
        &track,
        selected_candidate.is_some(),
        &settings,
        true,
    );
    let budget = playback_request_budget(&settings, budget_ms);
    let (stop, cancel) = {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        let started = gate.begin(request_generation)?;
        state.webview_identification.clear();
        started
    };

    state.webview_audio_transport.begin(request_generation)?;
    state
        .webview_audio_transcoder
        .cancel_older_than(request_generation);
    let deadline = playback_sources::PlaybackDeadline::after(budget);
    // Keep the large resolver state machine off the Tauri command future's
    // inline layout; this command is polled on the UI-facing invoke runtime.
    let resolver = Box::pin(async {
        if refresh_candidate_fresh.unwrap_or(false) {
            let candidate =
                refresh_candidate.ok_or("Fresh playback refresh requires the current candidate")?;
            playback_sources::resolve_candidate_fresh_until(
                &state.store,
                &state.acquisition,
                &track,
                candidate,
                selected_candidate.is_some(),
                cancel.clone(),
                request_generation,
                deadline,
                seek_recovery.unwrap_or(false),
            )
            .await
        } else {
            let limit = playback_sources::validate_candidate_attempt_limit(attempt_limit)?;
            let selected_candidate = if selected_candidate.is_some() {
                selected_candidate
            } else {
                playlist_sync::resolve_portable_playback_candidate(
                    &state.store,
                    &state.acquisition,
                    &track,
                    &settings,
                    cancel.clone(),
                )
                .await?
            };
            playback_sources::resolve_with_trace_context_excluding_until_attempt_limit(
                &state.store,
                &state.acquisition,
                &track,
                selected_candidate,
                cancel.clone(),
                None,
                None,
                Some(request_generation),
                &failed_candidates,
                deadline,
                limit,
            )
            .await
        }
    });
    let resolved = resolve_with_shared_playback_deadline(deadline, &stop, &cancel, resolver).await;

    let mut resolved = resolved?;
    if resolved.source.cenc.is_some() {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(request_generation, &stop);
        if gate.generation != request_generation || gate.stream_source_cancelled || *cancel.borrow()
        {
            return Err("Playback resolution was superseded".into());
        }
        // Retain the encrypted input and its lease entirely in Rust. The
        // WebView receives only a short-lived one-use handoff capability.
        let source_handle = state.playback.register_native_resolved_source(
            request_generation,
            &track,
            explicit_source,
            resolved,
        )?;
        return Ok(serde_json::json!({
            "transport":"native", "sourceHandle":source_handle,
            "sourceGeneration":request_generation
        }));
    }
    ensure_webview_compatible_source(&resolved.source)?;
    if let Some(station) = resolved.station.as_ref() {
        let transport = radio::resolve_station_playback_until(
            station.id.clone(),
            &state.store,
            deadline.instant(),
            cancel.clone(),
        )
        .await?;
        resolved.source.input = transport.url;
        resolved.source.decoder_hint = Some(native_output::AudioSourceDecoderHint {
            mime_type: transport.mime_type.clone(),
            codec: transport.codec.clone(),
            duration_seconds: None,
            content_length: None,
            has_video: false,
            decode_policy: crate::native_output::AudioSourceDecodePolicy::Automatic,
            decoder_fingerprint: None,
        });
        resolved.quality["mimeType"] = serde_json::json!(transport.mime_type);
        resolved.quality["codec"] = serde_json::json!(transport.codec);
    }
    let current_request = {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(request_generation, &stop);
        let current = gate.generation == request_generation && !*cancel.borrow();
        if current {
            let source = if let Some(asset_id) = resolved.quality["assetId"].as_str() {
                let asset = state
                    .store
                    .get_asset_by_id(asset_id)?
                    .ok_or("Resolved local asset is unavailable")?;
                if asset.path != resolved.source.input {
                    return Err("Resolved local asset changed during playback startup".into());
                }
                if !crate::compilation_excerpt::asset_file_hash_matches(&asset) {
                    return Err(
                        "Resolved excerpt contents no longer match their verified hash".into(),
                    );
                }
                if asset.track.provenance["automaticExcerpt"].is_object()
                    && !crate::compilation_excerpt::asset_proof_is_eligible(
                        &track,
                        &asset.track.provenance,
                        state.store.settings()?.matching_level,
                    )
                {
                    return Err("Matching policy changed during excerpt playback startup".into());
                }
                let settings = state.store.settings()?;
                crate::native_output::AudioSource {
                    input: asset.path,
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: vec![],
                    decoder_hint: None,

                    cenc: None,
                }
            } else {
                resolved.source.clone()
            };
            let bounds = track
                .playback_range
                .as_ref()
                .map(|range| (range.start_time, range.end_time));
            state.webview_identification.register(
                request_generation,
                track.id.clone(),
                source,
                resolved.candidate.as_ref().and_then(|candidate| {
                    playing_track_detections::SourceIdentity::new(
                        &candidate.provider,
                        &candidate.id,
                    )
                    .map(|identity| identity.with_duration_seconds(candidate.duration_seconds))
                }),
                resolved.station.is_some(),
                bounds,
            );
        }
        current
    };
    if !current_request {
        return Err("Playback resolution was superseded".into());
    }

    if webview_audio_transport::is_hls(&resolved.source) {
        resolved.quality["mimeType"] = serde_json::json!("application/vnd.apple.mpegurl");
    }
    let resolved_asset_id = resolved.quality["assetId"].as_str();
    let local_source = resolved_asset_id.is_some();
    let seek_recovery_relay = should_relay_seek_recovery_source(
        seek_recovery.unwrap_or(false),
        &track,
        resolved.candidate.as_ref(),
        &resolved.source.input,
        local_source,
        resolved.station.is_some(),
    );
    let mut relayed = false;
    let convert = resolved.station.is_some()
        && webview_audio_transcode::needs_live_ogg_conversion(
            resolved
                .quality
                .get("mimeType")
                .and_then(serde_json::Value::as_str),
            resolved
                .quality
                .get("codec")
                .and_then(serde_json::Value::as_str),
            webview_capabilities.as_ref(),
        );
    let mut url = if let Some(asset_id) = resolved_asset_id {
        let asset = state
            .store
            .get_asset_by_id(asset_id)?
            .ok_or("Verified excerpt asset is unavailable")?;
        if asset.path != resolved.source.input {
            return Err("Resolved local asset changed during playback startup".into());
        }
        if !crate::compilation_excerpt::asset_file_hash_matches(&asset) {
            return Err("Resolved excerpt contents no longer match their verified hash".into());
        }
        if asset.track.provenance["automaticExcerpt"].is_object()
            && !crate::compilation_excerpt::asset_proof_is_eligible(
                &track,
                &asset.track.provenance,
                state.store.settings()?.matching_level,
            )
        {
            return Err("Matching policy changed during excerpt playback startup".into());
        }
        if asset.hash != resolved.quality["hash"].as_str().unwrap_or_default() {
            return Err("Verified excerpt asset changed after proof validation".into());
        }
        state.local_audio.url_for(&asset.id)?
    } else if playback_relay_required(
        &resolved.source,
        convert,
        seek_recovery_relay,
        normalization_route_required.unwrap_or(false),
    ) {
        relayed = true;
        state
            .webview_audio_transport
            .register(
                request_generation,
                &track.id,
                &resolved.source,
                resolved.station.is_some(),
                deadline,
                cancel.clone(),
            )
            .await?
    } else {
        resolved.source.input.clone()
    };
    if convert {
        url = state.webview_audio_transcoder.register(
            request_generation,
            url,
            resolved.source.ffmpeg.clone(),
            deadline,
        )?;
        resolved.quality["mimeType"] = serde_json::json!("audio/mpeg");
        resolved.quality["codec"] = serde_json::json!("mp3");
        resolved.quality["bitrateKbps"] = serde_json::json!(192);
    }
    let duration = resolved.duration;
    let can_seek = resolved.station.is_none()
        && duration.is_some_and(|value| value.is_finite() && value > 0.0);
    let provider = resolved
        .candidate
        .as_ref()
        .map(|candidate| candidate.provider.as_str())
        .unwrap_or(track.provider.as_str());
    let title = resolved
        .candidate
        .as_ref()
        .map(|candidate| candidate.title.as_str())
        .unwrap_or(track.title.as_str());
    let approximate_excerpt =
        if resolved.quality["sourceKind"].as_str() == Some("approximateAlbumExcerpt") {
            let proof = &resolved.quality["approximateExcerpt"];
            Some(serde_json::json!({
                "label": "Approximate album excerpt",
                "source": {"provider":proof["source"]["provider"],"id":proof["source"]["id"]}
            }))
        } else {
            None
        };
    let preview = serde_json::json!({
        "kind": if local_source { "local" } else { "audio" },
        "url": url,
        "canSeek": can_seek,
        "canVolume": true,
        "canAutoplay": true,
        "isExcerpt": track.playback_range.is_some() || approximate_excerpt.is_some(),
        "approximateExcerpt": approximate_excerpt,
        "provider": provider,
        "title": title,
        "durationSeconds": duration,
        "matching": resolved.candidate.as_ref().and_then(|candidate| candidate.source_metadata.as_ref()).and_then(|metadata| metadata.get("matching")),
        "audioMimeType": resolved.quality.get("mimeType").and_then(serde_json::Value::as_str),
        "audioCodec": resolved.quality.get("codec").and_then(serde_json::Value::as_str),
        "audioBitrateKbps": resolved.quality.get("bitrateKbps").and_then(serde_json::Value::as_f64),
        "audioFormatId": resolved.quality.get("formatId").and_then(serde_json::Value::as_str),
        "audioFormatHasVideo": resolved.quality.get("formatHasVideo").and_then(serde_json::Value::as_bool),
        "audioExpectedContentLength": resolved.quality.get("expectedContentLength").and_then(serde_json::Value::as_u64),
        "sourceHeaderPolicy": webview_header_policy(&resolved.source.headers),
        "sourceTransport": if convert { "loopback-transcode" } else if relayed { "loopback-relay" } else { "direct" },
        "normalizationRoute": playback_normalization_route(local_source, relayed, convert),
        "seekRecoveryRelay": seek_recovery_relay,
        "sourceGeneration": request_generation
    });
    {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        if gate.generation != request_generation || gate.stream_source_cancelled || *cancel.borrow()
        {
            return Err("Playback resolution was superseded".into());
        }
        gate.resolved_stream_source = if resolved.source.cenc.is_none()
            && resolved.station.is_none()
            && track.playback_range.is_none()
            && track.local_path.is_none()
        {
            resolved.candidate.as_ref().and_then(|candidate| {
                let recording_identity =
                    analysis_stream_selection::stream_media_identity_fingerprint(&track)?;
                Some(ResolvedStreamSourceBinding {
                    generation: request_generation,
                    track_id: track.id.clone(),
                    recording_identity,
                    candidate: candidate.clone(),
                })
            })
        } else {
            None
        };
    }
    Ok(serde_json::json!({
        "preview": preview,
        "candidate": resolved.candidate,
        "quality": resolved.quality,
        "durationSeconds": duration
    }))
}

#[tauri::command]
fn cancel_playback_resolution(state: State<'_, AppState>, request_generation: u64) -> Result<()> {
    cancel_preview_warmup(&state.preview_warmup_cancel);
    state
        .webview_audio_transport
        .cancel_older_than(request_generation);
    state
        .webview_audio_transcoder
        .cancel_older_than(request_generation);
    state
        .playback_resolution
        .lock()
        .map_err(|_| "Playback resolver unavailable")?
        .cancel_older_than(request_generation);
    state
        .playback
        .cancel_pending_native_sources_older_than(request_generation);
    Ok(())
}

fn metadata_preview(track: &Track) -> Preview {
    if let Some(url) = track
        .preview_url
        .as_ref()
        .filter(|raw| acquisition::validate_source_url(raw, "direct").is_ok())
    {
        return Preview {
            kind: "audio".into(),
            url: url.clone(),
            can_seek: true,
            can_volume: true,
            can_autoplay: true,
            is_excerpt: true,
        };
    }
    Preview {
        kind: "external".into(),
        url: track.source_url.clone(),
        can_seek: false,
        can_volume: false,
        can_autoplay: false,
        is_excerpt: false,
    }
}

#[cfg(test)]
mod playback_metadata_tests {
    use super::*;
    #[test]
    fn playback_resolution_generation_cancels_only_older_requests() {
        let mut gate = PlaybackResolutionGate::default();
        let (_first, first_rx) = gate.begin(1).unwrap();
        gate.cancel_older_than(1);
        assert!(!*first_rx.borrow(), "same-generation cancel is a no-op");
        gate.cancel_older_than(2);
        assert!(
            *first_rx.borrow(),
            "a newer generation cancels the old resolver"
        );
        assert!(
            gate.begin(1).is_err(),
            "stale resolver requests are rejected"
        );

        let (second, second_rx) = gate.begin(2).unwrap();
        gate.cancel_older_than(2);
        assert!(
            !*second_rx.borrow(),
            "late same-generation cancel cannot cancel current work"
        );
        gate.finish(2, &second);
    }

    #[test]
    fn delayed_same_generation_fence_keeps_new_source_lease() {
        let mut gate = PlaybackResolutionGate::default();
        let (second, second_rx) = gate.begin(2).unwrap();
        let candidate = crate::models::Candidate {
            id: "youtube:lSmAz2rIBnE".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=lSmAz2rIBnE".into(),
            title: "Tengo tengo".into(),
            ..crate::models::Candidate::default()
        };
        gate.resolved_stream_source = Some(ResolvedStreamSourceBinding {
            generation: 2,
            track_id: "search:recording-1".into(),
            recording_identity: "identity".into(),
            candidate: candidate.clone(),
        });
        gate.cancel_older_than(2);
        assert!(!*second_rx.borrow());
        assert_eq!(
            gate.stream_candidate_for(
                2,
                "search:recording-1",
                Some("identity"),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .map(|value| value.id),
            Some(candidate.id)
        );
        gate.cancel_older_than(3);
        assert!(gate.resolved_stream_source.is_none());
        assert!(gate.stream_source_cancelled);
        assert!(*second_rx.borrow());
    }

    #[test]
    fn main_playback_normalization_route_matches_registered_transport() {
        let direct_source = crate::native_output::AudioSource {
            input: "https://cdn.example/audio.mp3".into(),
            ffmpeg: std::path::PathBuf::from("ffmpeg"),
            headers: vec![],
            decoder_hint: None,
            cenc: None,
        };
        assert!(!playback_relay_required(
            &direct_source,
            false,
            false,
            false,
        ));
        assert_eq!(
            playback_normalization_route(false, false, false),
            "unsupported",
            "the direct source is never advertised as Web Audio capable"
        );
        assert!(playback_relay_required(&direct_source, false, false, true,));
        assert_eq!(
            playback_normalization_route(false, true, false),
            "same-origin-relay",
            "a registered loopback relay is exposed as the capable source"
        );
        assert_eq!(
            playback_normalization_route(true, false, false),
            "same-origin-relay",
            "registered local audio is also an app-owned CORS route"
        );
        assert_eq!(
            playback_normalization_route(false, false, true),
            "same-origin-relay",
            "the registered transcode route is app-owned"
        );
    }

    #[test]
    fn active_candidate_lease_binds_unsaved_catalog_recording_and_exact_source() {
        let context = analysis_stream_selection::StreamMediaTrackContext {
            id: "search:musicbrainz-123".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz-123".into(),
            has_playback_range: false,
            artist: "Chico and The Gypsies".into(),
            title: "Tengo tengo".into(),
            source_page_url: None,
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(155.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let analysis_track = context
            .clone()
            .into_sanitized_track(155.364_716_553, "youtube", "youtube:lSmAz2rIBnE")
            .unwrap();
        let mut playback_track = analysis_track.clone();
        playback_track.duration_seconds = Some(155.0);
        let recording_identity =
            analysis_stream_selection::stream_media_identity_fingerprint(&playback_track).unwrap();
        let candidate = crate::models::Candidate {
            id: "youtube:lSmAz2rIBnE".into(),
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=lSmAz2rIBnE".into(),
            title: "Tengo tengo".into(),
            ..crate::models::Candidate::default()
        };
        let mut gate = PlaybackResolutionGate::default();
        let _ = gate.begin(41).unwrap();
        gate.resolved_stream_source = Some(ResolvedStreamSourceBinding {
            generation: 41,
            track_id: analysis_track.id.clone(),
            recording_identity: recording_identity.clone(),
            candidate: candidate.clone(),
        });
        assert_eq!(
            gate.stream_candidate_for(
                41,
                &analysis_track.id,
                analysis_stream_selection::stream_media_identity_fingerprint(&analysis_track)
                    .as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .map(|candidate| candidate.id),
            Some(candidate.id.clone()),
            "the same unsigned catalog context can use the resolver-created active source"
        );
        assert!(gate
            .stream_candidate_for(
                41,
                &analysis_track.id,
                Some("foreign-recording-fingerprint"),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none());
        assert_eq!(
            gate.stream_candidate_rejection_reason(
                41,
                &analysis_track.id,
                Some("foreign-recording-fingerprint"),
                "youtube",
                "youtube:lSmAz2rIBnE",
            ),
            "active YouTube recording context does not match"
        );
        assert!(gate
            .stream_candidate_for(
                40,
                &analysis_track.id,
                Some(&recording_identity),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none());
        assert!(gate
            .stream_candidate_for(
                41,
                &analysis_track.id,
                Some(&recording_identity),
                "youtube",
                "youtube:aaaaaaaaaaa",
            )
            .is_none());
        let mut changed_track = analysis_track;
        changed_track.title.push_str(" live");
        assert!(gate
            .stream_candidate_for(
                41,
                &changed_track.id,
                analysis_stream_selection::stream_media_identity_fingerprint(&changed_track)
                    .as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none());
        gate.cancel_older_than(42);
        assert!(gate.resolved_stream_source.is_none());
        assert_eq!(
            gate.stream_candidate_rejection_reason(
                41,
                "search:recording-1",
                Some("identity"),
                "youtube",
                "youtube:lSmAz2rIBnE",
            ),
            "active YouTube source lease was cancelled"
        );
    }

    #[test]
    fn explicit_youtube_audio_handoff_needs_exact_track_and_extractor_id() {
        let context = analysis_stream_selection::StreamMediaTrackContext {
            id: "search:musicbrainz-123".into(),
            provider: "catalog".into(),
            provider_id: "musicbrainz-123".into(),
            has_playback_range: false,
            artist: String::new(),
            title: "Tengo tengo".into(),
            source_page_url: None,
            version: None,
            album: None,
            album_artist: None,
            duration_seconds: Some(155.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let candidate = crate::models::Candidate {
            id: "youtube:lSmAz2rIBnE".into(),
            provider: "youtube".into(),
            url: "https://attacker.invalid/untrusted".into(),
            title: "client supplied title".into(),
            ..crate::models::Candidate::default()
        };
        let audio = crate::acquisition::PreviewAudio {
            provider_session: None,
            url: "https://signed.invalid/private?token=secret".into(),
            mime_type: "audio/mp4".into(),
            codec: Some("aac".into()),
            bitrate_kbps: Some(128.0),
            duration_seconds: Some(155.364_716_553),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: Some("Tengo tengo".into()),
            resolved_provider_id: Some("lSmAz2rIBnE".into()),
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        };
        let binding = candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&context),
            &candidate,
            &audio,
        )
        .expect("a validated exact-source handoff may lease an unsaved catalog track");
        assert_eq!(binding.generation, 52);
        assert_eq!(binding.track_id, context.id);
        assert_eq!(binding.candidate.id, "youtube:lSmAz2rIBnE");
        assert_eq!(binding.candidate.title, "Tengo tengo");
        assert_eq!(
            binding.candidate.url,
            "https://www.youtube.com/watch?v=lSmAz2rIBnE"
        );
        let projected = context
            .clone()
            .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
            .unwrap();
        let fingerprint = analysis_stream_selection::stream_media_identity_fingerprint(&projected);
        let mut gate = PlaybackResolutionGate::default();
        gate.generation = 52;
        gate.resolved_stream_source = Some(binding.clone());
        assert!(
            stream_media_cache_lookup_track(
                &gate,
                &context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_ok(),
            "artistless cache lookup uses the exact current source lease"
        );
        assert!(
            stream_media_cache_lookup_track(
                &PlaybackResolutionGate::default(),
                &context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_err(),
            "artistless lookup without a lease remains rejected"
        );
        assert!(stream_media_cache_lookup_track(
            &gate,
            &context,
            155.364_716_553,
            "youtube",
            "youtube:aaaaaaaaaaa",
        )
        .is_err());
        assert!(stream_media_cache_lookup_track(
            &gate,
            &context,
            155.364_716_553,
            "soundcloud",
            "youtube:lSmAz2rIBnE",
        )
        .is_err());
        let changed_cache_context = analysis_stream_selection::StreamMediaTrackContext {
            title: "Changed title".into(),
            ..context.clone()
        };
        assert!(
            stream_media_cache_lookup_track(
                &gate,
                &changed_cache_context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_err(),
            "recording identity changes cannot read the leased cache entry"
        );
        assert!(
            active_stream_candidate_for_context(
                &gate,
                52,
                &context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .unwrap()
            .is_some(),
            "blank artist is admitted only through a matching active lease"
        );
        assert!(
            active_stream_candidate_for_context(
                &PlaybackResolutionGate::default(),
                52,
                &context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_err(),
            "no lease keeps the strict missing-artist rejection"
        );
        assert_eq!(
            PlaybackResolutionGate::default().stream_candidate_rejection_reason(
                52,
                &context.id,
                None,
                "youtube",
                "youtube:lSmAz2rIBnE",
            ),
            "no active YouTube source binding"
        );
        assert!(
            active_stream_candidate_for_context(
                &gate,
                52,
                &context,
                155.364_716_553,
                "youtube",
                "youtube:aaaaaaaaaaa",
            )
            .is_err(),
            "a foreign source ID cannot use the active lease"
        );
        assert!(
            context
                .clone()
                .into_sanitized_track(155.364_716_553, "youtube", "youtube:lSmAz2rIBnE")
                .is_err(),
            "requests without generation lease stay on the strict projection"
        );
        assert!(
            gate.stream_candidate_for(
                52,
                &context.id,
                fingerprint.as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_some(),
            "only the exact request projection should retrieve the active source lease"
        );
        assert!(gate
            .stream_candidate_for(
                51,
                &context.id,
                fingerprint.as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none());
        assert!(gate
            .stream_candidate_for(
                52,
                &context.id,
                fingerprint.as_deref(),
                "youtube",
                "youtube:aaaaaaaaaaa",
            )
            .is_none());
        assert_eq!(
            gate.stream_candidate_rejection_reason(
                52,
                &context.id,
                fingerprint.as_deref(),
                "youtube",
                "youtube:aaaaaaaaaaa",
            ),
            "active YouTube source ID does not match"
        );
        let changed = analysis_stream_selection::StreamMediaTrackContext {
            title: "A different catalog title".into(),
            ..context.clone()
        }
        .into_active_youtube_handoff_track(155.364_716_553, "youtube:lSmAz2rIBnE")
        .unwrap();
        assert!(gate
            .stream_candidate_for(
                52,
                &context.id,
                analysis_stream_selection::stream_media_identity_fingerprint(&changed).as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none());
        gate.cancel_older_than(53);
        assert!(
            gate.stream_candidate_for(
                52,
                &context.id,
                fingerprint.as_deref(),
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_none(),
            "a cancelled/stale source lease cannot authorize analysis"
        );
        assert!(
            stream_media_cache_lookup_track(
                &gate,
                &context,
                155.364_716_553,
                "youtube",
                "youtube:lSmAz2rIBnE",
            )
            .is_err(),
            "stale generation cannot authorize artistless cache lookup"
        );
        assert!(candidate_audio_stream_binding(
            52,
            "search:other-track",
            Some(&context),
            &candidate,
            &audio,
        )
        .is_none());
        let mut malformed_candidate = candidate.clone();
        malformed_candidate.id = "youtube:aaaaaaaaaaa".into();
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&context),
            &malformed_candidate,
            &audio,
        )
        .is_none());
        let mut wrong_audio = audio.clone();
        wrong_audio.resolved_provider_id = Some("aaaaaaaaaaa".into());
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&context),
            &candidate,
            &wrong_audio,
        )
        .is_none());
        let mut missing_resolved_duration = audio.clone();
        missing_resolved_duration.duration_seconds = None;
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&context),
            &candidate,
            &missing_resolved_duration,
        )
        .is_none());
        let mut malformed_resolved_title = audio.clone();
        malformed_resolved_title.resolved_title = Some("Tengo\ntengo".into());
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&context),
            &candidate,
            &malformed_resolved_title,
        )
        .is_none());
        let excerpt = analysis_stream_selection::StreamMediaTrackContext {
            has_playback_range: true,
            ..context.clone()
        };
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&excerpt),
            &candidate,
            &audio,
        )
        .is_none());
        let wrong_page = analysis_stream_selection::StreamMediaTrackContext {
            source_page_url: Some("https://www.youtube.com/watch?v=foreignvideo1".into()),
            ..context.clone()
        };
        assert!(candidate_audio_stream_binding(
            52,
            "search:musicbrainz-123",
            Some(&wrong_page),
            &candidate,
            &audio,
        )
        .is_none());
        let current_gate = PlaybackResolutionGate {
            generation: 52,
            resolved_stream_source: Some(binding),
            ..PlaybackResolutionGate::default()
        };
        assert!(stream_media_cache_lookup_track(
            &current_gate,
            &wrong_page,
            155.364_716_553,
            "youtube",
            "youtube:lSmAz2rIBnE",
        )
        .is_err());
    }

    #[test]
    fn active_monochrome_audio_handoff_binds_only_exact_finite_canonical_resource() {
        let context = analysis_stream_selection::StreamMediaTrackContext {
            id: "catalog:recording-42".into(),
            provider: "catalog".into(),
            provider_id: "recording-42".into(),
            has_playback_range: false,
            artist: "Example Artist".into(),
            title: "Example Recording".into(),
            source_page_url: None,
            version: None,
            album: Some("Example Album".into()),
            album_artist: None,
            duration_seconds: Some(180.0),
            isrc: None,
            catalog_number: None,
            release_date: None,
            recording_date: None,
        };
        let candidate = crate::models::Candidate {
            id: "monochrome:154030613014253568".into(),
            provider: "monochrome".into(),
            url: "https://tracks.monochrome.st/track/154030613014253568".into(),
            title: "Untrusted display label".into(),
            ..crate::models::Candidate::default()
        };
        let audio = crate::acquisition::PreviewAudio {
            provider_session: None,
            url: candidate.url.clone(),
            mime_type: "audio/flac".into(),
            codec: Some("flac".into()),
            bitrate_kbps: Some(900.0),
            duration_seconds: Some(180.0),
            selected_format_id: None,
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: None,
            chapters: Some(vec![]),
            expected_content_length: None,
            headers: vec![],
        };
        let binding =
            candidate_audio_stream_binding(93, &context.id, Some(&context), &candidate, &audio)
                .expect("the exact finite Monochrome URL was accepted for current playback");
        assert_eq!(binding.generation, 93);
        assert_eq!(binding.track_id, context.id);
        assert_eq!(binding.candidate.provider, "monochrome");
        assert_eq!(binding.candidate.id, candidate.id);
        assert_eq!(binding.candidate.url, candidate.url);
        assert_eq!(binding.candidate.title, context.title);
        let fingerprinted_track = context
            .clone()
            .into_sanitized_track(180.0, "monochrome", &candidate.id)
            .unwrap();
        let fingerprint =
            analysis_stream_selection::stream_media_identity_fingerprint(&fingerprinted_track);
        let gate = PlaybackResolutionGate {
            generation: 93,
            resolved_stream_source: Some(binding),
            ..PlaybackResolutionGate::default()
        };
        assert!(gate
            .stream_candidate_for(
                93,
                &context.id,
                fingerprint.as_deref(),
                "monochrome",
                &candidate.id,
            )
            .is_some());
        assert!(gate
            .stream_candidate_for(
                92,
                &context.id,
                fingerprint.as_deref(),
                "monochrome",
                &candidate.id,
            )
            .is_none());
        assert!(gate
            .stream_candidate_for(
                93,
                &context.id,
                fingerprint.as_deref(),
                "monochrome",
                "monochrome:154030613014253569",
            )
            .is_none());

        let mut mismatched_audio = audio.clone();
        mismatched_audio.url = "https://tracks.monochrome.st/track/154030613014253569".into();
        assert!(candidate_audio_stream_binding(
            93,
            &context.id,
            Some(&context),
            &candidate,
            &mismatched_audio,
        )
        .is_none());
        let mut malformed_url = candidate.clone();
        malformed_url.url = "https://attacker.invalid/track/154030613014253568".into();
        assert!(candidate_audio_stream_binding(
            93,
            &context.id,
            Some(&context),
            &malformed_url,
            &audio,
        )
        .is_none());
        let mut malformed_id = candidate.clone();
        malformed_id.id = "monochrome:154030613014253569".into();
        assert!(candidate_audio_stream_binding(
            93,
            &context.id,
            Some(&context),
            &malformed_id,
            &audio,
        )
        .is_none());
        let mut excerpt = context.clone();
        excerpt.has_playback_range = true;
        assert!(candidate_audio_stream_binding(
            93,
            &context.id,
            Some(&excerpt),
            &candidate,
            &audio,
        )
        .is_none());
        let mut unknown_duration = audio.clone();
        unknown_duration.duration_seconds = None;
        assert!(candidate_audio_stream_binding(
            93,
            &context.id,
            Some(&context),
            &candidate,
            &unknown_duration,
        )
        .is_none());
        assert!(candidate_audio_stream_binding(
            93,
            "another-track",
            Some(&context),
            &candidate,
            &audio,
        )
        .is_none());
    }

    #[tokio::test]
    async fn playback_resolution_budget_signals_cancellation_before_returning() {
        let (stop, mut cancelled) = tokio::sync::watch::channel(false);
        let observer = cancelled.clone();
        let result =
            resolve_with_playback_budget(std::time::Duration::from_millis(1), &stop, async move {
                cancelled
                    .changed()
                    .await
                    .map_err(|_| "cancellation channel closed".to_string())?;
                Err::<(), String>("Cancelled".into())
            })
            .await;
        assert!(result
            .unwrap_err()
            .contains("remaining playback startup budget"));
        assert!(*observer.borrow());
    }

    fn terminal_trace_fixture() -> (
        playback_sources::SourceTrace,
        Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = events.clone();
        let trace =
            playback_sources::SourceTrace::new(Some("fixture-operation".into()), None, Some(52))
                .with_test_sink(Arc::new(move |event| output.lock().unwrap().push(event)));
        (trace, events)
    }

    fn assert_one_resolution_end(events: &[serde_json::Value], cancelled: bool, deadline: bool) {
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["phase"], "resolution_start");
        assert_eq!(events[1]["phase"], "resolution_end");
        assert_eq!(events[1]["resolutionId"], events[0]["resolutionId"]);
        assert_eq!(events[1]["nativeOperationId"], "fixture-operation");
        assert_eq!(events[1]["webviewGeneration"], 52);
        assert_eq!(events[1]["details"]["ok"], false);
        assert_eq!(events[1]["details"]["cancelled"], cancelled);
        assert_eq!(events[1]["details"]["deadline"], deadline);
        assert_eq!(events[1]["details"]["abandoned"], false);
    }

    #[tokio::test]
    async fn outer_deadline_drop_emits_resolution_end_before_timer_signals_cancel() {
        let (trace, events) = terminal_trace_fixture();
        let deadline =
            playback_sources::PlaybackDeadline::after(std::time::Duration::from_millis(10));
        let (stop, cancel) = tokio::sync::watch::channel(false);
        let observer = cancel.clone();
        let inner_cancel = cancel.clone();
        let result = resolve_with_shared_playback_deadline(
            deadline,
            &stop,
            &cancel,
            Box::pin(async move {
                let _completion = playback_sources::ResolutionTraceGuard::begin(
                    trace,
                    "youtube",
                    false,
                    inner_cancel,
                    deadline,
                );
                std::future::pending::<Result<()>>().await
            }),
        )
        .await;
        assert!(result
            .unwrap_err()
            .contains("remaining playback startup budget"));
        assert!(*observer.borrow(), "timer still signals stop before return");
        assert_one_resolution_end(&events.lock().unwrap(), false, true);
    }

    #[tokio::test]
    async fn outer_cancel_drop_emits_one_cancelled_resolution_end() {
        let (trace, events) = terminal_trace_fixture();
        let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(1));
        let (stop, cancel) = tokio::sync::watch::channel(false);
        let inner_stop = stop.clone();
        let inner_cancel = cancel.clone();
        let result = resolve_with_shared_playback_deadline(
            deadline,
            &stop,
            &cancel,
            Box::pin(async move {
                let _completion = playback_sources::ResolutionTraceGuard::begin(
                    trace,
                    "youtube",
                    false,
                    inner_cancel,
                    deadline,
                );
                inner_stop.send_replace(true);
                std::future::pending::<Result<()>>().await
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert_one_resolution_end(&events.lock().unwrap(), true, false);
    }

    #[tokio::test]
    async fn external_cancel_and_expired_deadline_keep_cancel_priority_on_drop() {
        use std::future::Future;
        let (trace, events) = terminal_trace_fixture();
        let deadline = playback_sources::PlaybackDeadline::at(tokio::time::Instant::now());
        let (stop, cancel) = tokio::sync::watch::channel(false);
        let inner_cancel = cancel.clone();
        let mut inner = Box::pin(async move {
            let _completion = playback_sources::ResolutionTraceGuard::begin(
                trace,
                "youtube",
                false,
                inner_cancel,
                deadline,
            );
            std::future::pending::<Result<()>>().await
        });
        // Start the resolution before the caller observes both ready outcomes.
        std::future::poll_fn(|cx| {
            assert!(inner.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        stop.send_replace(true);
        let result = resolve_with_shared_playback_deadline(deadline, &stop, &cancel, inner).await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert_one_resolution_end(&events.lock().unwrap(), true, false);
    }

    #[tokio::test]
    async fn shared_deadline_normalizes_nested_subbudget_timeout_to_public_error() {
        let deadline = playback_sources::PlaybackDeadline::at(tokio::time::Instant::now());
        let (stop, observer) = tokio::sync::watch::channel(false);
        let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let result = resolve_with_shared_playback_deadline(
            deadline,
            &stop,
            &cancel,
            Box::pin(async { Err::<(), _>("Audio stream resolution timed out".into()) }),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Audio lookup exceeded the remaining playback startup budget"
        );
        assert!(
            *observer.borrow(),
            "deadline exhaustion signals nested work"
        );
    }

    #[tokio::test]
    async fn already_expired_deadline_overrides_ready_success_and_signals_stop() {
        let deadline = playback_sources::PlaybackDeadline::at(
            tokio::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        let (stop, observer) = tokio::sync::watch::channel(false);
        let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let result = resolve_with_shared_playback_deadline(
            deadline,
            &stop,
            &cancel,
            Box::pin(async { Ok::<_, String>(()) }),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Audio lookup exceeded the remaining playback startup budget"
        );
        assert!(*observer.borrow(), "expired deadline signals nested work");
    }

    #[tokio::test]
    async fn artist_command_keeps_first_match_and_does_not_skip_following_pages() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("artist-pages.sqlite")).unwrap());
        for (page, name, id) in [
            (1, "Gillian Welch", "2ff49515-1701-4bac-a53a-04345893f3f3"),
            (2, "Gillian Philip", "aab1e845-ba00-41f0-a717-46fd4ba24498"),
        ] {
            store
                .set_provider_cache(
                    "artists.search.musicbrainz",
                    &format!("gillian welch:{page}:20"),
                    &serde_json::json!({"count":40,"artists":[{"id":id,"name":name,"score":100}]}),
                    3600,
                )
                .unwrap();
        }
        let service = artists::ArtistService::new(store).unwrap();
        let credentials = artists::ArtistCredentials::default();
        for requested in [None, Some(1)] {
            let first =
                search_artist_request(&service, &credentials, "Gillian Welch", requested, Some(20))
                    .await
                    .unwrap();
            assert_eq!(first.page, 1);
            assert_eq!(first.results[0].name, "Gillian Welch");
            assert!(first.has_more);
        }
        let second =
            search_artist_request(&service, &credentials, "Gillian Welch", Some(2), Some(20))
                .await
                .unwrap();
        assert_eq!(second.page, 2);
        assert_eq!(second.results[0].name, "Gillian Philip");
        assert!(!second.has_more);
    }
    #[test]
    fn missing_storefront_preview_returns_discovery_fallback_without_accessing_provider() {
        for provider in ["traxsource", "beatport", "spotify", "import"] {
            let track = Track {
                provider: provider.into(),
                ..Default::default()
            };
            assert_eq!(metadata_preview(&track).kind, "external");
        }
    }
    #[test]
    fn public_cached_audio_preview_is_playable_and_unsafe_urls_are_not() {
        let mut track = Track {
            preview_url: Some("https://audio.example.com/preview.mp3".into()),
            ..Default::default()
        };
        assert_eq!(metadata_preview(&track).kind, "audio");
        for url in [
            "file:///tmp/audio.mp3",
            "https://localhost/audio",
            "https://user:pass@audio.example.com/preview.mp3",
        ] {
            track.preview_url = Some(url.into());
            assert_eq!(metadata_preview(&track).kind, "external");
        }
    }
}
#[tauri::command]
fn list_favorite_track_ids(state: State<'_, AppState>) -> Result<Vec<String>> {
    state.store.list_favorite_track_ids()
}
#[tauri::command]
fn list_tracks(
    state: State<'_, AppState>,
    view: String,
    collection_id: Option<String>,
) -> Result<Vec<Track>> {
    state.store.list_tracks(
        if view == "collections" {
            "collection"
        } else {
            &view
        },
        collection_id.as_deref(),
    )
}
#[tauri::command]
async fn list_tracks_page(
    state: State<'_, AppState>,
    view: String,
    collection_id: Option<String>,
    limit: usize,
    offset: usize,
    query: Option<String>,
    sort_field: Option<String>,
    direction: Option<String>,
    min_duration_seconds: Option<u32>,
    max_duration_seconds: Option<u32>,
    availability: Option<String>,
    provider: Option<String>,
) -> Result<serde_json::Value> {
    let limit = limit.clamp(1, 1000);
    let store = state.store.clone();
    let result = tokio::task::spawn_blocking(move || {
        store.list_tracks_page_with_sort_state(
            if view == "collections" {
                "collection"
            } else {
                &view
            },
            collection_id.as_deref(),
            limit,
            offset,
            query.as_deref(),
            sort_field.as_deref(),
            direction.as_deref(),
            min_duration_seconds,
            max_duration_seconds,
            availability.as_deref(),
            provider.as_deref(),
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    let (tracks, total, availability_state, sort_pending) = result;
    let pending = matches!(
        &availability_state,
        Some(storage::SortAvailabilityState::Pending)
    );
    let sort_availability = match availability_state {
        Some(storage::SortAvailabilityState::Ready(fields)) => Some(fields),
        Some(storage::SortAvailabilityState::Pending) | None => None,
    };
    if sort_pending || (pending && offset == 0) {
        state.sort_availability_backfill.request();
    }
    Ok(serde_json::json!({
        "tracks": tracks, "total": total, "offset": offset, "limit": limit,
        "sortAvailability": sort_availability, "sortAvailabilityPending": pending,
        "sortPending": sort_pending
    }))
}
#[tauri::command]
fn list_track_prefix(
    state: State<'_, AppState>,
    view: String,
    collection_id: Option<String>,
    limit: usize,
    query: Option<String>,
    sort_field: Option<String>,
    direction: Option<String>,
    min_duration_seconds: Option<u32>,
    max_duration_seconds: Option<u32>,
    availability: Option<String>,
    provider: Option<String>,
) -> Result<serde_json::Value> {
    let (rows, total) = state.store.list_track_prefix(
        if view == "collections" {
            "collection"
        } else {
            &view
        },
        collection_id.as_deref(),
        limit,
        query.as_deref(),
        sort_field.as_deref(),
        direction.as_deref(),
        min_duration_seconds,
        max_duration_seconds,
        availability.as_deref(),
        provider.as_deref(),
    )?;
    Ok(serde_json::json!({"rows":rows,"total":total,"limit":limit}))
}
#[tauri::command]
fn get_tracks(state: State<'_, AppState>, ids: Vec<String>) -> Result<Vec<Track>> {
    state.store.get_tracks(&ids)
}
#[tauri::command]
async fn identify_playing_track(
    state: State<'_, AppState>,
    capture_id: Option<String>,
) -> Result<playback::PlayingTrackIdentification> {
    if state.playback.native_enabled() {
        state.playback.identify_playing_track_for(capture_id).await
    } else {
        let snapshot = state.automation.playback_snapshot();
        let generation = snapshot["sourceGeneration"]
            .as_u64()
            .ok_or("Playback source is unavailable for identification")?;
        let track_id = snapshot["trackId"]
            .as_str()
            .or_else(|| snapshot["track"]["id"].as_str())
            .ok_or("There is no current playback track")?;
        let position = snapshot["absolutePosition"]
            .as_f64()
            .or_else(|| snapshot["position"].as_f64())
            .ok_or("Current playback position is unavailable")?;
        identify_webview_track_for(&*state, generation, track_id, position, None, None, None).await
    }
}

#[tauri::command]
fn cancel_playing_track_identification(state: State<'_, AppState>, capture_id: String) -> bool {
    state
        .playback
        .cancel_playing_track_identification(&capture_id)
}

#[tauri::command]
async fn identify_webview_track(
    state: State<'_, AppState>,
    generation: u64,
    track_id: String,
    position_seconds: f64,
) -> Result<playback::PlayingTrackIdentification> {
    identify_webview_track_for(
        &*state,
        generation,
        &track_id,
        position_seconds,
        None,
        None,
        None,
    )
    .await
}

#[tauri::command]
async fn identify_webview_playing_pcm(
    state: State<'_, AppState>,
    capture_id: String,
    track_id: String,
    station_id: Option<String>,
    session_id: String,
    generation: u64,
    seek_serial: u64,
    chapter_id: Option<String>,
    wav_base64: String,
) -> Result<serde_json::Value> {
    if track_id.is_empty()
        || track_id.len() > 512
        || session_id.is_empty()
        || session_id.len() > 128
        || chapter_id
            .as_ref()
            .is_some_and(|chapter| chapter.is_empty() || chapter.len() > 256)
    {
        return Err("Playback identity is invalid".into());
    }
    if station_id.as_ref().is_some_and(|station| {
        station.is_empty()
            || station.len() > 120
            || track_id.as_str() != format!("radio:{station}").as_str()
    }) {
        return Err("Radio station identity does not match the current playback track".into());
    }
    let capture_job = state
        .webview_identification
        .begin_pcm_capture(capture_id.clone())?;
    let cancelled = capture_job.cancellation();
    let matches = || {
        webview_radio_pcm::snapshot_matches(
            &state.automation.playback_tracking_snapshot(),
            &track_id,
            &session_id,
            generation,
            seek_serial,
            chapter_id.as_deref(),
        )
    };
    if state.playback.native_enabled() || !matches() {
        return Err("Playback changed before captured audio could be identified".into());
    }
    let bytes = webview_radio_pcm::decode_bounded_wav(&wav_base64)?;
    if !matches() {
        return Err("Playback changed before identification started".into());
    }
    let station = if let Some(station_id) = station_id.as_deref() {
        radio::get_radio_stations_saved(&state.store)?
            .into_iter()
            .find(|station| station.id == station_id)
            .ok_or("This station is no longer available")?
    } else {
        radio::RadioStation {
            id: format!("webview:{generation}:{track_id}"),
            name: "Currently playing audio".into(),
            stream_url: String::new(),
            source_url: String::new(),
            provider: Some("webview-audio".into()),
            country: None,
            tags: None,
            homepage_url: None,
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        }
    };
    let mut wav = tempfile::Builder::new()
        .prefix("burrata-webview-audio-")
        .suffix(".wav")
        .tempfile()
        .map_err(|error| format!("Could not prepare captured audio for identification: {error}"))?;
    use std::io::Write;
    wav.write_all(&bytes)
        .and_then(|_| wav.flush())
        .map_err(|error| format!("Could not write captured audio for identification: {error}"))?;
    if *cancelled.borrow() {
        return Err("Audio identification was cancelled".into());
    }
    let now_playing = recognize_webview_pcm(
        state.webview_identification.clone(),
        capture_id.clone(),
        capture_job,
        wav,
        std::time::Duration::from_secs(60),
        "Audio identification timed out",
        move |path, cancellation| async move {
            radio::recognize_captured_sample_cancellable(&station, &path, cancellation).await
        },
    )
    .await?;
    if *cancelled.borrow() {
        return Err("Audio identification was cancelled".into());
    }
    if !matches() {
        return Err("Playback changed during identification".into());
    }
    let note: String = if now_playing.is_some() {
        "SongRec identified audio captured from current WebView playback; catalog identity is unverified.".into()
    } else if station_id.is_some() {
        "SongRec did not identify a track in the captured audio excerpt.".into()
    } else {
        "SongRec did not identify a track in the captured audio excerpt.".into()
    };
    if let Some(station_id) = station_id {
        radio::record_played_radio_identification(&station_id, generation, now_playing.as_ref());
        Ok(serde_json::json!({"nowPlaying":now_playing,"note":note}))
    } else {
        let identified = now_playing;
        Ok(serde_json::to_value(playback::PlayingTrackIdentification {
            track_id,
            title: identified.as_ref().map(|item| item.title.clone()),
            artist: identified.as_ref().map(|item| item.artist.clone()),
            artwork_url: identified
                .as_ref()
                .and_then(|item| item.artwork_url.clone()),
            note,
            timing_anchor: None,
            detection_metadata: None,
            captured_audio_interval: None,
            detection_source_identity: None,
            detection_source_epoch: None,
        })
        .map_err(|error| error.to_string())?)
    }
}

/// Keep the private WAV and registry job inside the worker until SongRec has
/// reaped its child. Dropping a timed-out JoinHandle detaches that worker; it
/// does not cancel it or release the file early.
async fn recognize_webview_pcm<T, F, Fut>(
    registry: webview_identification::WebViewIdentification,
    capture_id: String,
    job: webview_identification::PcmCaptureJob,
    wav: tempfile::NamedTempFile,
    timeout: std::time::Duration,
    timeout_message: &'static str,
    recognize: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(std::path::PathBuf, tokio::sync::watch::Receiver<bool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T>> + Send + 'static,
{
    let mut cancelled = job.cancellation();
    let worker_cancelled = cancelled.clone();
    let wav_path = wav.path().to_path_buf();
    let mut worker = tokio::spawn(async move {
        let _job = job;
        let _wav = wav;
        recognize(wav_path, worker_cancelled).await
    });
    let terminal = tokio::select! {
        result = &mut worker => return result.map_err(|_| "Audio identification worker failed")?,
        _ = async {
            loop {
                if *cancelled.borrow() { return; }
                if cancelled.changed().await.is_err() {
                    // The worker drops its registry sender just before its
                    // JoinHandle becomes ready. Closure is completion, not Cancel.
                    std::future::pending::<()>().await;
                }
            }
        } => "Audio identification was cancelled",
        _ = tokio::time::sleep(timeout) => timeout_message,
    };

    registry.cancel_pcm_capture(&capture_id);
    // The worker owns both RAII guards and can outlive this bounded wait. Never
    // abort it: SongRec may still be reading the WAV until its child is reaped.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), &mut worker).await;
    Err(terminal.into())
}

#[cfg(test)]
mod webview_pcm_recognition_tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn successful_worker_sender_closure_is_not_cancellation() {
        for index in 0..32 {
            let registry = webview_identification::WebViewIdentification::default();
            let id = format!("successful-{index}");
            let job = registry.begin_pcm_capture(id.clone()).unwrap();
            let wav = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
            let result = recognize_webview_pcm(
                registry,
                id,
                job,
                wav,
                std::time::Duration::from_secs(1),
                "timed out",
                |_, _| async { Ok::<_, String>(42) },
            )
            .await;
            assert_eq!(result.unwrap(), 42);
        }
    }

    #[tokio::test]
    async fn cancelled_worker_keeps_private_wav_until_recognition_exits() {
        let registry = webview_identification::WebViewIdentification::default();
        let job = registry.begin_pcm_capture("owned-wav-test".into()).unwrap();
        let mut wav = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        wav.write_all(b"test wav").unwrap();
        let path = wav.path().to_path_buf();
        let worker_registry = registry.clone();
        let worker = tokio::spawn(async move {
            recognize_webview_pcm(
                worker_registry,
                "owned-wav-test".into(),
                job,
                wav,
                std::time::Duration::from_secs(10),
                "recognition timed out",
                move |path, mut cancelled| async move {
                    cancelled
                        .changed()
                        .await
                        .map_err(|_| "cancel channel closed")?;
                    assert!(
                        path.exists(),
                        "the WAV must remain available until recognition exits"
                    );
                    Ok::<(), String>(())
                },
            )
            .await
        });
        for _ in 0..50 {
            if registry.cancel_pcm_capture("owned-wav-test") {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(worker.await.unwrap().is_err());
        assert!(
            !path.exists(),
            "the worker-owned temporary WAV is cleaned after exit"
        );
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
async fn identify_webview_playing_output(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    capture_id: String,
    track_id: String,
    station_id: Option<String>,
    session_id: String,
    generation: u64,
    seek_serial: u64,
    chapter_id: Option<String>,
    capture_seconds: Option<usize>,
    web_audio_graph_expected: Option<bool>,
) -> Result<serde_json::Value> {
    if track_id.is_empty()
        || track_id.len() > 512
        || session_id.is_empty()
        || session_id.len() > 128
        || chapter_id
            .as_ref()
            .is_some_and(|chapter| chapter.is_empty() || chapter.len() > 256)
    {
        return Err("Playback identity is invalid".into());
    }
    if station_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 120 || track_id != format!("radio:{id}"))
    {
        return Err("Radio station identity does not match the current playback track".into());
    }
    let started = std::time::Instant::now();
    let job = state
        .webview_identification
        .begin_pcm_capture(capture_id.clone())?;
    let cancelled = job.cancellation();
    let matches = || {
        !state.playback.native_enabled()
            && webview_radio_pcm::snapshot_matches(
                &state.automation.playback_tracking_snapshot(),
                &track_id,
                &session_id,
                generation,
                seek_serial,
                chapter_id.as_deref(),
            )
    };
    if state.playback.native_enabled() || !matches() {
        return Err("Playback changed before audio capture".into());
    }
    let app_root_pid = std::process::id();
    let initial_status = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_burrata_webview_audio_status(&app),
    )
    .await
    .map_err(|_| "Timed out checking Burrata playback windows")??;
    let (status_tx, status_rx) =
        tokio::sync::watch::channel(Some((initial_status, std::time::Instant::now())));
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let status_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let value = read_burrata_webview_audio_status(&status_app)
                .await
                .ok()
                .map(|s| (s, std::time::Instant::now()));
            status_tx.send_replace(value);
            tokio::select! {_=&mut stop_rx=>break,_=tokio::time::sleep(std::time::Duration::from_millis(300))=>{}}
        }
    });
    let status = move || {
        status_rx
            .borrow()
            .as_ref()
            .filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(1))
            .map(|(s, _)| *s)
            .unwrap_or(linux_webview_output::ViewAudio {
                main: false,
                others: 0,
                known: false,
            })
    };
    let capture_budget = std::time::Duration::from_secs(60).saturating_sub(started.elapsed());
    // Keep this command and its registry guard alive until the recorder child
    // has been explicitly killed and reaped on cancellation or deadline.
    let capture = linux_webview_output::capture_wav_seconds_cancellable(
        app_root_pid,
        capture_budget,
        capture_seconds.unwrap_or(3),
        status,
        matches,
        cancelled.clone(),
        web_audio_graph_expected.unwrap_or(false),
    )
    .await;
    let _ = stop_tx.send(());
    let wav = capture?;
    if *cancelled.borrow() || !matches() {
        return Err("Playback changed after audio capture".into());
    }
    let station = if let Some(id) = station_id.as_deref() {
        radio::get_radio_stations_saved(&state.store)?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("This station is no longer available")?
    } else {
        radio::RadioStation {
            id: format!("webview:{generation}:{track_id}"),
            name: "Currently playing audio".into(),
            stream_url: String::new(),
            source_url: String::new(),
            provider: Some("webview-audio".into()),
            country: None,
            tags: None,
            homepage_url: None,
            favicon_url: None,
            bitrate_kbps: None,
            codec: None,
            language: None,
            is_custom: true,
        }
    };
    let mut tempfile = tempfile::Builder::new()
        .prefix("burrata-webview-output-")
        .suffix(".wav")
        .tempfile()
        .map_err(|e| format!("Could not prepare captured audio: {e}"))?;
    use std::io::Write;
    tempfile
        .write_all(&wav)
        .and_then(|_| tempfile.flush())
        .map_err(|e| format!("Could not save captured audio: {e}"))?;
    if *cancelled.borrow() || !matches() {
        return Err("Playback changed before recognition".into());
    }
    let recognition_timeout = std::time::Duration::from_secs(60).saturating_sub(started.elapsed());
    if recognition_timeout.is_zero() {
        return Err("Audio identification exceeded its 60-second limit".into());
    }
    let identified = recognize_webview_pcm(
        state.webview_identification.clone(),
        capture_id.clone(),
        job,
        tempfile,
        recognition_timeout,
        "Audio identification exceeded its 60-second limit",
        move |path, cancellation| async move {
            radio::recognize_captured_sample_cancellable(&station, &path, cancellation).await
        },
    )
    .await?;
    if *cancelled.borrow() || !matches() {
        return Err("Playback changed during identification".into());
    }
    let note = if identified.is_some() {
        "SongRec identified the captured audio; catalog identity is unverified."
    } else {
        "No song was identified in the captured audio. Try again during a clear section of music."
    };
    if let Some(station_id) = station_id {
        radio::record_played_radio_identification(&station_id, generation, identified.as_ref());
        Ok(serde_json::json!({"nowPlaying":identified,"note":note}))
    } else {
        Ok(serde_json::to_value(playback::PlayingTrackIdentification {
            track_id,
            title: identified.as_ref().map(|m| m.title.clone()),
            artist: identified.as_ref().map(|m| m.artist.clone()),
            artwork_url: identified.as_ref().and_then(|m| m.artwork_url.clone()),
            note: note.into(),
            timing_anchor: None,
            detection_metadata: None,
            captured_audio_interval: None,
            detection_source_identity: None,
            detection_source_epoch: None,
        })
        .map_err(|e| e.to_string())?)
    }
}

#[cfg(target_os = "linux")]
async fn read_burrata_webview_audio_status(
    app: &tauri::AppHandle,
) -> Result<linux_webview_output::ViewAudio> {
    use tauri::Manager;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    // Every app WebView is created through WebviewWindowBuilder; child WebViews
    // are unavailable without Tauri's disabled unstable feature.
    let views = app.webview_windows();
    if views.is_empty() {
        return Err("No Burrata WebViews are available".into());
    }
    let labels = views
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut main = None;
    let mut others = 0usize;
    for (label, view) in views {
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |webview| {
            use webkit2gtk::WebViewExt;
            let _ = tx.send(webview.inner().is_playing_audio());
        })
        .map_err(|_| "Could not inspect Burrata WebView audio state")?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timed out checking Burrata playback windows".into());
        }
        let playing = tokio::time::timeout(remaining, rx)
            .await
            .map_err(|_| "Timed out checking Burrata playback windows")?
            .map_err(|_| "Could not inspect Burrata WebView audio state")?;
        if label == "main" {
            main = Some(playing)
        } else if playing {
            others += 1
        }
    }
    if app
        .webview_windows()
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        != labels
    {
        return Err("Burrata WebViews changed while checking audio state".into());
    }
    Ok(linux_webview_output::ViewAudio {
        main: main.ok_or("Main Burrata WebView is unavailable")?,
        others,
        known: true,
    })
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
async fn identify_webview_playing_output(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    capture_id: String,
    track_id: String,
    station_id: Option<String>,
    session_id: String,
    generation: u64,
    seek_serial: u64,
    chapter_id: Option<String>,
    web_audio_graph_expected: Option<bool>,
) -> Result<serde_json::Value> {
    let _ = (
        app,
        state,
        capture_id,
        track_id,
        station_id,
        session_id,
        generation,
        seek_serial,
        chapter_id,
        web_audio_graph_expected,
    );
    Err("System output capture for WebView audio is available only on Linux".into())
}

#[tauri::command]
fn cancel_webview_identification(state: State<'_, AppState>, capture_id: String) -> bool {
    state.webview_identification.cancel_pcm_capture(&capture_id)
}

async fn identify_webview_track_for(
    state: &AppState,
    generation: u64,
    track_id: &str,
    position_seconds: f64,
    expected_seek_serial: Option<u64>,
    expected_resource: Option<&playing_track_detections::DetectionResourceKey>,
    capture_id: Option<String>,
) -> Result<playback::PlayingTrackIdentification> {
    let current_snapshot = || {
        let native_enabled = state.playback.native_enabled();
        let snapshot = state.automation.playback_snapshot();
        (snapshot, native_enabled)
    };
    let (snapshot, native_enabled) = current_snapshot();
    let chapter_id = snapshot
        .get("chapterId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let playing = webview_identification::snapshot_matches_playback(
        &snapshot,
        native_enabled,
        track_id,
        generation,
        None,
        chapter_id.as_deref(),
    );
    if !playing {
        return Err("Play the current WebView track before identifying it".into());
    }
    let absolute_position = snapshot["absolutePosition"]
        .as_f64()
        .or_else(|| snapshot["position"].as_f64())
        .ok_or("Current playback position is unavailable")?;
    if !position_seconds.is_finite() || (position_seconds - absolute_position).abs() > 5.0 {
        return Err("Playback changed before identification started".into());
    }
    let seek_serial = snapshot["seekSerial"]
        .as_u64()
        .ok_or("Playback seek state is unavailable")?;
    if expected_seek_serial.is_some_and(|expected| expected != seek_serial) {
        return Err("Playback changed before track detection started".into());
    }
    let initial_source_identity = state
        .webview_identification
        .registered_resource_identity(generation, track_id);
    let initial_source_epoch = expected_resource.map(|_| {
        state
            .webview_identification
            .registered_finite_source_epoch(generation, track_id)
    });
    if matches!(initial_source_epoch, Some(None)) {
        return Err("The current finite audio source is not registered".into());
    }
    if let Some(expected_resource) = expected_resource {
        let track = detection_resource_track_from_snapshot(
            track_id,
            &snapshot,
            initial_source_identity.as_ref(),
            None,
        )
        .ok_or("The registered WebView source has no stable resource identity")?;
        let current_resource = detection_resource_key_blocking(
            state.store.clone(),
            track,
            initial_source_identity.clone(),
            None,
        )
        .await?
        .ok_or("The current source has no stable identity for track detection")?;
        if &current_resource != expected_resource {
            return Err("Playback source changed before track detection started".into());
        }
    }
    let result = state
        .webview_identification
        .identify_with_capture(generation, track_id, absolute_position, capture_id)
        .await?;
    let (snapshot_after, native_after) = current_snapshot();
    if !webview_identification::snapshot_matches_playback(
        &snapshot_after,
        native_after,
        track_id,
        generation,
        Some(seek_serial),
        chapter_id.as_deref(),
    ) {
        return Err("Playback changed during identification".into());
    }
    if result.detection_source_identity != initial_source_identity
        || initial_source_epoch.is_some_and(|expected| {
            result.detection_source_epoch != expected
                || state
                    .webview_identification
                    .registered_finite_source_epoch(generation, track_id)
                    != expected
        })
    {
        return Err("Playback source changed during track detection".into());
    }
    if let (Some(metadata), Some((start, duration)), Some(_)) = (
        result.detection_metadata.as_ref(),
        result.captured_audio_interval,
        snapshot_after.get("track"),
    ) {
        if state
            .webview_identification
            .matches_registered_finite_source(generation, track_id)
        {
            if let Some(track) = detection_resource_track_from_snapshot(
                track_id,
                &snapshot_after,
                result.detection_source_identity.as_ref(),
                None,
            ) {
                let current_source_identity = state
                    .webview_identification
                    .registered_resource_identity(generation, track_id);
                let source_owner_matches = current_source_identity
                    == result.detection_source_identity
                    && result.detection_source_epoch
                        == state
                            .webview_identification
                            .registered_finite_source_epoch(generation, track_id);
                if let Some(resource) = detection_resource_key_blocking(
                    state.store.clone(),
                    track,
                    result.detection_source_identity.clone(),
                    None,
                )
                .await?
                {
                    if source_owner_matches
                        && expected_resource.is_none_or(|expected| expected == &resource)
                    {
                        let _ = playing_track_detections::record_if_enabled(
                            &state.store,
                            &resource,
                            metadata,
                            Some(track_id),
                            start,
                            duration,
                        );
                    }
                }
            }
        }
    }
    Ok(result)
}

fn detection_resource_key(
    store: &Store,
    track: &models::Track,
    source_identity: Option<&playing_track_detections::SourceIdentity>,
    active_asset_id: Option<&str>,
) -> Option<playing_track_detections::DetectionResourceKey> {
    let local_hash = playing_track_detections::requires_verified_local_hash(
        track,
        source_identity,
        active_asset_id.is_some(),
    )
    .then(|| {
        playing_track_detections::local_asset_id_for_source(track, source_identity, active_asset_id)
            .and_then(|asset_id| {
                playing_track_detections::verified_local_asset_hash(store, asset_id)
            })
    })
    .flatten();
    playing_track_detections::DetectionResourceKey::for_playback_source(
        track,
        source_identity,
        local_hash.as_deref(),
    )
}

fn saved_track_detection_key(
    store: &Store,
    track: &models::Track,
) -> Option<playing_track_detections::DetectionResourceKey> {
    let provenance = track.provenance.as_object();
    let candidate = provenance
        .and_then(|value| value.get("audioCandidate"))
        .and_then(serde_json::Value::as_object);
    let candidate_identity = candidate.and_then(|candidate| {
        let provider = candidate.get("provider")?.as_str()?;
        let id = candidate.get("id")?.as_str()?;
        playing_track_detections::SourceIdentity::new(provider, id).map(|identity| {
            identity.with_duration_seconds(
                candidate
                    .get("durationSeconds")
                    .and_then(serde_json::Value::as_f64),
            )
        })
    });
    let source_identity = candidate_identity
        .or_else(|| {
            track.asset_id.as_deref().and_then(|asset_id| {
                playing_track_detections::SourceIdentity::new("local", asset_id)
                    .map(|identity| identity.with_duration_seconds(track.duration_seconds))
            })
        })
        .or_else(|| {
            playing_track_detections::SourceIdentity::new(&track.provider, &track.provider_id)
                .map(|identity| identity.with_duration_seconds(track.duration_seconds))
        });
    let asset_id = track.asset_id.as_deref().or_else(|| {
        source_identity
            .as_ref()
            .filter(|source| source.provider_source_id().0 == "local")
            .map(|source| source.provider_source_id().1)
    });
    detection_resource_key(store, track, source_identity.as_ref(), asset_id)
}

/// Construct only the track fields required for a resource key from the
/// playback owner and a source identity registered by the server. WebView
/// playback snapshots intentionally expose a thin track DTO, not a full
/// library Track.
fn detection_resource_track_from_snapshot(
    track_id: &str,
    snapshot: &serde_json::Value,
    source_identity: Option<&playing_track_detections::SourceIdentity>,
    active_asset_id: Option<&str>,
) -> Option<models::Track> {
    let thin = snapshot.get("track")?;
    if thin["id"]
        .as_str()
        .is_some_and(|snapshot_track_id| snapshot_track_id != track_id)
    {
        return None;
    }
    let identity = source_identity.map(|source| source.provider_source_id());
    let asset_identity = active_asset_id.or_else(|| {
        identity
            .filter(|(provider, _)| *provider == "local")
            .map(|(_, source_id)| source_id)
    });
    let provider = identity
        .map(|(provider, _)| provider.to_owned())
        .or_else(|| asset_identity.map(|_| "local".to_owned()))?;
    let provider_id = identity
        .map(|(_, source_id)| source_id.to_owned())
        .or_else(|| asset_identity.map(str::to_owned))?;
    Some(models::Track {
        id: track_id.to_owned(),
        provider,
        provider_id,
        title: thin["title"].as_str().unwrap_or_default().to_owned(),
        artist: thin["artist"].as_str().unwrap_or_default().to_owned(),
        duration_seconds: thin["durationSeconds"].as_f64(),
        asset_id: asset_identity.map(str::to_owned),
        ..models::Track::default()
    })
}

fn playback_snapshot_is_live_radio(snapshot: &serde_json::Value) -> bool {
    let track = &snapshot["track"];
    let source_kind = snapshot["sourceKind"]
        .as_str()
        .or_else(|| snapshot["quality"]["sourceKind"].as_str())
        .or_else(|| track["sourceKind"].as_str())
        .or_else(|| track["provenance"]["sourceKind"].as_str());
    let radio_provider = track["provenance"]["radioProvider"]
        .as_str()
        .or_else(|| track["radioProvider"].as_str())
        .or_else(|| snapshot["radioProvider"].as_str())
        .or_else(|| track["provider"].as_str())
        .or_else(|| snapshot["provider"].as_str())
        .or_else(|| snapshot["candidate"]["provider"].as_str())
        .map(|provider| provider.to_ascii_lowercase().replace('_', "-"));
    snapshot
        .get("radioNowPlaying")
        .is_some_and(|value| !value.is_null())
        || snapshot["isLive"].as_bool() == Some(true)
        || track["isLive"].as_bool() == Some(true)
        || source_kind == Some("radio")
        || radio_provider.is_some_and(|provider| {
            matches!(
                provider.as_str(),
                "internet-radio" | "radioparadise" | "radio-paradise"
            )
        })
}

#[cfg(test)]
mod detection_snapshot_tests {
    use super::*;

    #[test]
    fn saved_detection_identity_prefers_persisted_audio_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("saved-detection.sqlite")).unwrap();
        let track = models::Track {
            id: "library:mix".into(),
            provider: "bandcamp".into(),
            provider_id: "mix-track".into(),
            duration_seconds: Some(900.0),
            provenance: serde_json::json!({"audioCandidate": {
                "provider":"youtube", "id":"abcdefghijk", "durationSeconds":901.0
            }}),
            ..models::Track::default()
        };
        let saved_key = saved_track_detection_key(&store, &track).unwrap();
        let candidate_track = models::Track {
            provider: "youtube".into(),
            provider_id: "abcdefghijk".into(),
            duration_seconds: Some(901.0),
            ..track.clone()
        };
        let candidate_key = saved_track_detection_key(&store, &candidate_track).unwrap();
        assert_eq!(saved_key, candidate_key);
    }

    #[test]
    fn saved_detection_identity_rejects_unstable_catalog_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("saved-detection.sqlite")).unwrap();
        let track = models::Track {
            id: "catalog:unresolved".into(),
            provider: "catalog".into(),
            provider_id: "recording-id".into(),
            ..models::Track::default()
        };
        assert!(saved_track_detection_key(&store, &track).is_none());
    }

    #[test]
    fn thin_webview_track_uses_registered_local_or_remote_resource_identity() {
        let snapshot = serde_json::json!({
            "trackId":"musicbrainz:recording-a",
            "track":{
                "id":"musicbrainz:recording-a",
                "artist":"Artist",
                "title":"Long Mix",
                "album":"Album",
                "artworkUrl":"https://art.example/image",
                "durationSeconds":1200.0,
                "sourceUrl":"https://example.invalid/mix",
                "localPath":"/private/library/mix.flac"
            }
        });
        assert!(serde_json::from_value::<models::Track>(snapshot["track"].clone()).is_err());

        let local =
            playing_track_detections::SourceIdentity::new("local", "asset:resolved-a").unwrap();
        let local_before = detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            Some(&local),
            None,
        )
        .unwrap();
        let local_after = detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            Some(&local),
            None,
        )
        .unwrap();
        assert_eq!(local_before.asset_id.as_deref(), Some("asset:resolved-a"));
        let verified_hash = "a".repeat(64);
        assert_eq!(
            playing_track_detections::DetectionResourceKey::for_playback_source(
                &local_before,
                Some(&local),
                Some(&verified_hash),
            ),
            playing_track_detections::DetectionResourceKey::for_playback_source(
                &local_after,
                Some(&local),
                Some(&verified_hash),
            )
        );

        let remote =
            playing_track_detections::SourceIdentity::new("youtube", "unsaved-video").unwrap();
        let mut remote_snapshot = snapshot.clone();
        remote_snapshot["trackId"] = serde_json::json!("musicbrainz:uninserted-recording");
        remote_snapshot["track"]["id"] = serde_json::json!("musicbrainz:uninserted-recording");
        let remote_before = detection_resource_track_from_snapshot(
            "musicbrainz:uninserted-recording",
            &remote_snapshot,
            Some(&remote),
            None,
        )
        .unwrap();
        let remote_after = detection_resource_track_from_snapshot(
            "musicbrainz:uninserted-recording",
            &remote_snapshot,
            Some(&remote),
            None,
        )
        .unwrap();
        let before_key = playing_track_detections::DetectionResourceKey::for_playback_source(
            &remote_before,
            Some(&remote),
            None,
        );
        let after_key = playing_track_detections::DetectionResourceKey::for_playback_source(
            &remote_after,
            Some(&remote),
            None,
        );
        assert!(before_key.is_some());
        assert_eq!(before_key, after_key);
        assert!(detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            None,
            None,
        )
        .is_none());
    }

    #[test]
    fn native_sanitized_track_uses_active_asset_or_candidate_not_stripped_url() {
        let mut snapshot = serde_json::json!({
            "trackId":"musicbrainz:recording-a",
            "track":{
                "id":"musicbrainz:recording-a",
                "provider":"musicbrainz",
                "providerId":"recording-a",
                "sourceUrl":"local:/private/audio.flac",
                "localPath":"/private/audio.flac",
                "artist":"Artist",
                "title":"Mix",
                "durationSeconds":900.0,
                "favorite":false,
                "provenance":{"sourceKind":"library"}
            },
            "quality":{"sourceKind":"localAsset","assetId":"asset:active"},
            "candidate":{"provider":"youtube","id":"candidate-that-is-not-active"}
        });
        snapshot["track"]
            .as_object_mut()
            .unwrap()
            .remove("sourceUrl");
        snapshot["track"]
            .as_object_mut()
            .unwrap()
            .remove("localPath");
        assert!(serde_json::from_value::<models::Track>(snapshot["track"].clone()).is_err());

        let active_local =
            playing_track_detections::SourceIdentity::new("local", "asset:active").unwrap();
        let local_before = detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            Some(&active_local),
            Some("asset:active"),
        )
        .unwrap();
        let verified_hash = "d".repeat(64);
        let local_key_before = playing_track_detections::DetectionResourceKey::for_playback_source(
            &local_before,
            Some(&active_local),
            Some(&verified_hash),
        );
        let local_after = detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            Some(&active_local),
            Some("asset:active"),
        )
        .unwrap();
        assert_eq!(
            local_key_before,
            playing_track_detections::DetectionResourceKey::for_playback_source(
                &local_after,
                Some(&active_local),
                Some(&verified_hash),
            )
        );
        assert!(local_key_before.is_some());

        snapshot["quality"] = serde_json::Value::Null;
        let remote =
            playing_track_detections::SourceIdentity::new("youtube", "candidate-that-is-active")
                .unwrap();
        let remote_track = detection_resource_track_from_snapshot(
            "musicbrainz:recording-a",
            &snapshot,
            Some(&remote),
            None,
        )
        .unwrap();
        assert!(
            playing_track_detections::DetectionResourceKey::for_playback_source(
                &remote_track,
                Some(&remote),
                None,
            )
            .is_some()
        );
    }

    #[test]
    fn radio_source_filter_keeps_finite_radio_song_and_rejects_stations() {
        assert!(!playback_snapshot_is_live_radio(&serde_json::json!({
            "track":{"provider":"radio","provenance":{"sourceKind":"radioSong"}}
        })));
        assert!(playback_snapshot_is_live_radio(&serde_json::json!({
            "track":{"provenance":{"sourceKind":"radio"}}
        })));
        assert!(playback_snapshot_is_live_radio(&serde_json::json!({
            "track":{"provenance":{"radioProvider":"radio_paradise"}}
        })));
    }
}

async fn detection_resource_key_blocking(
    store: Arc<Store>,
    track: models::Track,
    source_identity: Option<playing_track_detections::SourceIdentity>,
    active_asset_id: Option<String>,
) -> Result<Option<playing_track_detections::DetectionResourceKey>> {
    tauri::async_runtime::spawn_blocking(move || {
        detection_resource_key(
            &store,
            &track,
            source_identity.as_ref(),
            active_asset_id.as_deref(),
        )
    })
    .await
    .map_err(|error| error.to_string())
}

fn current_detection_resource_key(
    state: &AppState,
    generation: u64,
    seek_serial: u64,
    track_id: &str,
) -> Result<playing_track_detections::DetectionResourceKey> {
    let snapshot = state.automation.playback_tracking_snapshot();
    if !matches!(snapshot["status"].as_str(), Some("playing" | "paused"))
        || snapshot["trackId"].as_str() != Some(track_id)
    {
        return Err("Play the current finite recording before changing detection options".into());
    }
    let native = state.playback.native_enabled();
    let current_generation = if native {
        snapshot["generation"].as_u64()
    } else {
        snapshot["sourceGeneration"].as_u64()
    };
    if current_generation != Some(generation) {
        return Err("Playback changed before detection options were read".into());
    }
    if snapshot["seekSerial"].as_u64() != Some(seek_serial) {
        return Err("Playback position changed before detection options were read".into());
    }
    if !native
        && !state
            .webview_identification
            .matches_registered_finite_source(generation, track_id)
    {
        return Err("The current finite audio source is not registered".into());
    }
    if playback_snapshot_is_live_radio(&snapshot) {
        return Err("Track detection is available for finite recordings, not live radio".into());
    }
    let local_asset_source =
        native && snapshot["quality"]["sourceKind"].as_str() == Some("localAsset");
    let active_asset_id = local_asset_source
        .then(|| snapshot["quality"]["assetId"].as_str())
        .flatten();
    let source_identity = if local_asset_source {
        active_asset_id
            .and_then(|asset_id| playing_track_detections::SourceIdentity::new("local", asset_id))
    } else if native {
        snapshot["candidate"]["provider"]
            .as_str()
            .zip(snapshot["candidate"]["id"].as_str())
            .and_then(|(provider, source_id)| {
                playing_track_detections::SourceIdentity::new(provider, source_id).map(|identity| {
                    identity
                        .with_duration_seconds(snapshot["candidate"]["durationSeconds"].as_f64())
                })
            })
    } else {
        state
            .webview_identification
            .registered_resource_identity(generation, track_id)
    };
    let track = detection_resource_track_from_snapshot(
        track_id,
        &snapshot,
        source_identity.as_ref(),
        active_asset_id,
    )
    .ok_or("The active playback source has no stable resource identity")?;
    if track.id != track_id {
        return Err("Playback track identity changed".into());
    }
    detection_resource_key(
        &state.store,
        &track,
        source_identity.as_ref(),
        active_asset_id,
    )
    .ok_or_else(|| "The current source has no stable identity for track detection".into())
}

async fn current_detection_resource_key_blocking(
    app: tauri::AppHandle,
    generation: u64,
    seek_serial: u64,
    track_id: String,
) -> Result<playing_track_detections::DetectionResourceKey> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        current_detection_resource_key(&state, generation, seek_serial, &track_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_playing_track_detections(
    app: tauri::AppHandle,
    track_id: String,
    generation: u64,
    seek_serial: u64,
) -> Result<playing_track_detections::DetectionResourceView> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let key = current_detection_resource_key(&state, generation, seek_serial, &track_id)?;
        let view = playing_track_detections::read(&state.store, &key)?;
        if current_detection_resource_key(&state, generation, seek_serial, &track_id)? != key {
            return Err("Playback source changed while detection options were read".into());
        }
        Ok(view)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Read detections for a saved or currently displayed track without requiring
/// it to own playback. The key is derived only from persisted source identity;
/// callers cannot select an arbitrary cache key.
#[tauri::command]
async fn get_saved_track_detections(
    app: tauri::AppHandle,
    track: models::Track,
) -> Result<playing_track_detections::DetectionResourceView> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let key = saved_track_detection_key(&state.store, &track)
            .ok_or("This track has no stable saved source identity for track detections")?;
        playing_track_detections::read(&state.store, &key)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn set_playing_track_detection_enabled(
    app: tauri::AppHandle,
    track_id: String,
    generation: u64,
    seek_serial: u64,
    enabled: bool,
) -> Result<playing_track_detections::DetectionResourceView> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let key = current_detection_resource_key(&state, generation, seek_serial, &track_id)?;
        if current_detection_resource_key(&state, generation, seek_serial, &track_id)? != key {
            return Err("Playback source changed while detection options were updated".into());
        }
        let view =
            playing_track_detections::set_enabled(&state.store, &key, enabled, Some(&track_id))?;
        if current_detection_resource_key(&state, generation, seek_serial, &track_id)? != key {
            return Err("Playback source changed while detection options were updated".into());
        }
        Ok(view)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn detect_playing_track_segment(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    track_id: String,
    generation: u64,
    seek_serial: u64,
    capture_id: String,
) -> Result<playing_track_detections::DetectionResourceView> {
    let current = state.automation.playback_tracking_snapshot();
    if current["status"].as_str() != Some("playing") {
        return Err("Resume this recording to detect tracks in it".into());
    }
    let key = current_detection_resource_key_blocking(
        app.clone(),
        generation,
        seek_serial,
        track_id.clone(),
    )
    .await?;
    if !playing_track_detections::read(&state.store, &key)
        .map_err(|error| error.to_string())?
        .enabled
    {
        return Err("Track detection is turned off for this recording".into());
    }
    if state.playback.native_enabled() {
        let owner_snapshot = state.automation.playback_tracking_snapshot();
        let candidate_identity = owner_snapshot["candidate"]["provider"]
            .as_str()
            .zip(owner_snapshot["candidate"]["id"].as_str());
        if current_detection_resource_key_blocking(
            app.clone(),
            generation,
            seek_serial,
            track_id.clone(),
        )
        .await?
            != key
        {
            return Err("Playback source changed before track detection started".into());
        }
        state
            .playback
            .identify_playing_track_for_owner(
                Some(capture_id.clone()),
                &track_id,
                generation,
                seek_serial,
                candidate_identity,
            )
            .await?;
    } else {
        let snapshot = state.automation.playback_tracking_snapshot();
        let position = snapshot["absolutePosition"]
            .as_f64()
            .or_else(|| snapshot["position"].as_f64())
            .ok_or("Current playback position is unavailable")?;
        identify_webview_track_for(
            &*state,
            generation,
            &track_id,
            position,
            Some(seek_serial),
            Some(&key),
            Some(capture_id),
        )
        .await?;
    }
    if current_detection_resource_key_blocking(app, generation, seek_serial, track_id.clone())
        .await?
        != key
    {
        return Err("Playback source changed during track detection".into());
    }
    playing_track_detections::read(&state.store, &key).map_err(Into::into)
}
#[tauri::command]
fn set_favorite(state: State<'_, AppState>, track: Track, value: bool) -> Result<()> {
    playlist_preferences::set_favorite_membership(&state.store, &track, value)?;
    if value {
        source_preparation::schedule(state.acquisition.clone(), vec![track]);
    }
    Ok(())
}
#[tauri::command]
fn set_disposition(state: State<'_, AppState>, track: Track, value: Option<String>) -> Result<()> {
    state.store.set_disposition(&track, value.as_deref())
}
#[tauri::command]
async fn preview_unsorted_library_entries(
    state: State<'_, AppState>,
    limit: usize,
    offset: usize,
) -> Result<storage::UnsortedLibraryPreview> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.preview_unsorted_library_entries(limit, offset)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn remove_unsorted_library_entries(
    state: State<'_, AppState>,
    selections: Vec<storage::LibraryEntrySelection>,
) -> Result<storage::UnsortedLibraryRemovalResult> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.remove_unsorted_library_entries(&selections))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn update_track(state: State<'_, AppState>, track: Track) -> Result<Track> {
    let updated = state.store.update_track(&track)?;
    let recording = state.store.recording_id(&updated.id)?;
    let ids = state
        .store
        .list_track_files(&updated.id)?
        .into_iter()
        .filter(|asset| asset.recording_id == recording && asset.processing != "external-readonly")
        .map(|asset| asset.id)
        .collect::<Vec<_>>();
    if !ids.is_empty() {
        state.analysis.queue(Some(ids), true)?;
    }
    Ok(updated)
}
#[tauri::command]
fn list_collections(state: State<'_, AppState>) -> Result<Vec<Collection>> {
    state.store.list_collections()
}
#[tauri::command]
fn list_deleted_collections(state: State<'_, AppState>) -> Result<Vec<Collection>> {
    state.store.list_deleted_collections()
}
#[tauri::command]
fn delete_collection(state: State<'_, AppState>, collection_id: String) -> Result<Collection> {
    let _library_write = state.store.try_library_write()?;
    state.store.delete_collection(&collection_id)
}
#[tauri::command]
fn restore_collection(state: State<'_, AppState>, collection_id: String) -> Result<Collection> {
    let _library_write = state.store.try_library_write()?;
    state.store.restore_collection(&collection_id)
}
#[tauri::command]
fn create_collection(state: State<'_, AppState>, name: String) -> Result<Collection> {
    let _library_write = state.store.try_library_write()?;
    state.store.create_collection(&name)
}
#[tauri::command]
fn rename_collection(
    state: State<'_, AppState>,
    collection_id: String,
    name: String,
) -> Result<Collection> {
    let _library_write = state.store.try_library_write()?;
    state.store.rename_collection(
        &playlist_sync::ensure_editable_collection(&state.store, &collection_id)?,
        &name,
    )
}
#[tauri::command]
fn remove_from_collection(
    state: State<'_, AppState>,
    collection_id: String,
    track_ids: Vec<String>,
) -> Result<String> {
    let _library_write = state.store.try_library_write()?;
    let target_id = playlist_sync::ensure_editable_collection(&state.store, &collection_id)?;
    state.store.remove_from_collection(&target_id, &track_ids)?;
    playlist_preferences::resync_favorite_flags_after_collection_change(&state.store, &target_id)?;
    Ok(target_id)
}
#[tauri::command]
fn add_to_collection(
    state: State<'_, AppState>,
    collection_id: String,
    tracks: Vec<Track>,
) -> Result<()> {
    let _library_write = state.store.try_library_write()?;
    let target_id = playlist_sync::ensure_editable_collection(&state.store, &collection_id)?;
    state.store.add_to_collection(&target_id, &tracks)?;
    playlist_preferences::resync_favorite_flags_after_collection_change(&state.store, &target_id)
}
#[tauri::command]
async fn append_playlist_entries(
    state: State<'_, AppState>,
    collection_id: String,
    tracks: Vec<Track>,
) -> Result<()> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        let _library_write = store.try_library_write()?;
        let target_id = playlist_sync::ensure_editable_collection(&store, &collection_id)?;
        store.append_playlist_entries(&target_id, &tracks)
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
fn merge_recordings(
    state: State<'_, AppState>,
    primary_id: String,
    other_ids: Vec<String>,
) -> Result<()> {
    state.store.merge_recordings(&primary_id, &other_ids)
}
#[tauri::command]
fn import_tracks(state: State<'_, AppState>, text: String, kind: String) -> Result<Vec<Track>> {
    imports::parse(&text, &kind)?
        .into_iter()
        .map(|t| state.store.save_track(&t))
        .collect()
}
#[tauri::command]
fn preview_import_tracks(text: String, kind: String) -> Result<Vec<Track>> {
    imports::parse(&text, &kind)
}
#[tauri::command]
async fn pick_import_audio_sources(
    app: tauri::AppHandle,
    source_kind: String,
) -> Result<Vec<String>> {
    use tauri_plugin_dialog::DialogExt;
    if !matches!(source_kind.as_str(), "files" | "folder") {
        return Err("Choose files or a folder".into());
    }
    let choose_folder = source_kind == "folder";
    let picker = app.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        let dialog = picker.dialog().file().set_title("Choose local audio");
        if choose_folder {
            dialog.blocking_pick_folder().map(|path| vec![path])
        } else {
            dialog
                .add_filter(
                    "Audio files",
                    &[
                        "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "aiff", "wma",
                    ],
                )
                .blocking_pick_files()
        }
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(selected) = selected else {
        return Ok(Vec::new());
    };
    let paths = selected
        .into_iter()
        .map(|file| file.into_path().map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let mut files = Vec::new();
    for path in paths {
        if !choose_folder {
            if path.is_file() && is_supported_import_audio(&path) {
                files.push(std::fs::canonicalize(path).map_err(|error| error.to_string())?);
            }
        } else {
            collect_import_audio_files(&path, 0, &mut files)?;
        }
        if files.len() > 1000 {
            return Err("A single import is limited to 1,000 audio files".into());
        }
    }
    files.sort();
    files.dedup();
    Ok(files
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}
fn is_supported_import_audio(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp3" | "flac" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "aiff" | "wma"
            )
        })
}
fn collect_import_audio_files(
    directory: &std::path::Path,
    depth: usize,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    if depth > 8 {
        return Ok(());
    }
    let entries = std::fs::read_dir(directory).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_import_audio_files(&entry.path(), depth + 1, files)?;
        } else if kind.is_file() && is_supported_import_audio(&entry.path()) {
            files.push(std::fs::canonicalize(entry.path()).map_err(|error| error.to_string())?);
            if files.len() > 1000 {
                return Err("A single import is limited to 1,000 audio files".into());
            }
        }
    }
    Ok(())
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LocalAudioFileSeal {
    path: String,
    size_bytes: String,
    modified_ns: Option<String>,
    file_identity: String,
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalAudioImportPreview {
    tracks: Vec<Track>,
    seals: Vec<LocalAudioFileSeal>,
}
fn local_audio_file_seal(path: &std::path::Path) -> Result<LocalAudioFileSeal> {
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Choose an audio file".into());
    }
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string());
    #[cfg(unix)]
    let file_identity = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    };
    #[cfg(not(unix))]
    let file_identity = canonical.to_string_lossy().into_owned();
    Ok(LocalAudioFileSeal {
        path: canonical.to_string_lossy().into_owned(),
        size_bytes: metadata.len().to_string(),
        modified_ns,
        file_identity,
    })
}
#[tauri::command]
async fn preview_local_audio_import(
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<LocalAudioImportPreview> {
    if paths.is_empty() || paths.len() > 1000 {
        return Err("Select between 1 and 1,000 audio files".into());
    }
    let settings = state.store.settings()?;
    let mut tracks = Vec::with_capacity(paths.len());
    let mut seals = Vec::with_capacity(paths.len());
    let mut seen = std::collections::HashSet::new();
    for selected in paths {
        let seal = local_audio_file_seal(std::path::Path::new(&selected))?;
        let path = std::path::PathBuf::from(&seal.path);
        if !is_supported_import_audio(&path) {
            continue;
        }
        let path_identity = format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(seal.path.as_bytes())
        );
        if !seen.insert(path_identity.clone()) {
            continue;
        }
        let mut track = media::inspect_local_file(&path, &settings).await?;
        track.provider = "local".into();
        track.provider_id = path_identity.clone();
        track.id = format!("local-preview:{path_identity}");
        track.local_path = Some(path.to_string_lossy().into_owned());
        track.source_url.clear();
        tracks.push(track);
        seals.push(seal);
    }
    Ok(LocalAudioImportPreview { tracks, seals })
}
#[tauri::command]
async fn apply_local_audio_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    seals: Vec<LocalAudioFileSeal>,
) -> Result<Vec<Track>> {
    if seals.is_empty() || seals.len() > 1000 {
        return Err("Select between 1 and 1,000 audio files".into());
    }
    for seal in &seals {
        let current = local_audio_file_seal(std::path::Path::new(&seal.path))?;
        if current != *seal {
            return Err("A selected audio file changed after preview. Choose it again and review a fresh preview.".into());
        }
    }
    let mut tracks = Vec::with_capacity(seals.len());
    for seal in &seals {
        tracks.push(import_local_file_inner(&app, &state, &seal.path, Some(seal)).await?);
    }
    Ok(tracks)
}
#[tauri::command]
async fn search_tracks(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    query: String,
    provider_ids: Vec<String>,
) -> Result<Vec<Track>> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 300 {
        return Err("Search must contain 1–300 characters".into());
    }
    let mut result = Vec::new();
    let mut errors = Vec::new();
    for id in provider_ids {
        match providers::provider(&id) {
            Ok(p) => match p.search_tracks(query).await {
                Ok(tracks) => {
                    for t in tracks {
                        result.push(state.store.cache_track(&t)?);
                    }
                }
                Err(e) => errors.push(e),
            },
            Err(e) => errors.push(e),
        }
    }
    let terms: Vec<_> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    for t in state.store.list_tracks("all", None)? {
        let haystack = format!(
            "{} {} {} {}",
            t.artist,
            t.title,
            t.version.as_deref().unwrap_or(""),
            t.label.as_deref().unwrap_or("")
        )
        .to_lowercase();
        if terms.iter().all(|v| haystack.contains(v))
            && !result.iter().any(|v: &Track| v.id == t.id)
        {
            result.push(t)
        }
    }
    if !errors.is_empty() {
        let _ = app.emit("provider-warning", errors.join("; "));
    }
    if result.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(result)
}
#[tauri::command]
async fn find_candidates(
    state: State<'_, AppState>,
    track: Track,
    provider_ids: Vec<String>,
    request_generation: Option<u64>,
) -> Result<Vec<Candidate>> {
    if let Some(generation) = request_generation {
        cancel_preview_warmup(&state.preview_warmup_cancel);
        return state
            .acquisition
            .find_preview_candidates(&track, &provider_ids, &state.store.settings()?, generation)
            .await;
    }
    state
        .acquisition
        .find_candidates(&track, &provider_ids, &state.store.settings()?)
        .await
}

#[tauri::command]
async fn browse_soulseek_directory(
    state: State<'_, AppState>,
    username: String,
    directory: String,
    request_id: String,
) -> Result<acquisition::SoulseekDirectoryBrowse> {
    let guard = state.provider_reads.begin(request_id)?;
    state
        .acquisition
        .browse_soulseek_directory(&username, &directory, guard.cancellation())
        .await
}

#[tauri::command]
async fn find_soulseek_album_candidates(
    state: State<'_, AppState>,
    tracks: Vec<Track>,
    request_id: String,
) -> Result<Vec<acquisition::SoulseekAlbumAssignment>> {
    let guard = state.provider_reads.begin(request_id)?;
    state
        .acquisition
        .find_soulseek_album_candidates(&tracks, guard.cancellation())
        .await
}

#[tauri::command]
async fn prepare_download_match_identity(
    state: State<'_, AppState>,
    track: Track,
    request_id: String,
) -> Result<()> {
    let guard = state.provider_reads.begin(request_id.clone())?;
    let cancel = guard.cancellation();
    if !crate::artist_alias_lookup::enabled()
        || !(track.provider == "spotify"
            || track.provider == "catalog"
                && matches!(
                    track.provenance["catalogProvider"].as_str(),
                    Some("musicbrainz" | "discogs")
                )
            || matches!(
                track.provider.as_str(),
                "beatport" | "traxsource" | "local" | "radio_song"
            ) && track.isrc.is_some())
    {
        return Ok(());
    }
    let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(8));
    let trace = playback_sources::SourceTrace::new(Some(request_id), None, None);
    let _ = crate::artist_alias_lookup::lookup(
        &state.acquisition,
        &track,
        cancel.clone(),
        deadline,
        &trace,
    )
    .await;
    if *cancel.borrow() {
        return Err("Download identity preparation cancelled".into());
    }
    Ok(())
}

#[tauri::command]
async fn find_download_candidates(
    state: State<'_, AppState>,
    track: Track,
    provider_id: String,
    request_id: String,
) -> Result<Vec<Candidate>> {
    let guard = state.provider_reads.begin(request_id)?;
    let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(22));
    state
        .acquisition
        .find_download_candidates_with_cached_aliases(
            &track,
            &provider_id,
            &state.store.settings()?,
            guard.cancellation(),
            deadline,
        )
        .await
}

#[tauri::command]
async fn search_audio_tracks(
    state: State<'_, AppState>,
    query: String,
    provider_ids: Vec<String>,
) -> Result<Vec<Track>> {
    state
        .acquisition
        .search_audio_tracks(&query, &provider_ids, &state.store.settings()?)
        .await
}
async fn resolve_with_candidate_audio_deadline<T>(
    deadline: playback_sources::PlaybackDeadline,
    stop: &tokio::sync::watch::Sender<bool>,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout_at(deadline.instant(), future).await {
        Ok(result) if !deadline.remaining().is_zero() => result,
        _ => {
            stop.send_replace(true);
            Err("Audio lookup exceeded the remaining playback startup budget".into())
        }
    }
}

#[tauri::command]
async fn resolve_candidate_audio(
    state: State<'_, AppState>,
    candidate: Candidate,
    request_generation: Option<u64>,
    track_id: Option<String>,
    track_context: Option<analysis_stream_selection::StreamMediaTrackContext>,
    budget_ms: Option<u64>,
    normalization_route_required: Option<bool>,
) -> Result<serde_json::Value> {
    let settings = state.store.settings()?;
    let transport_deadline =
        playback_sources::PlaybackDeadline::after(playback_request_budget(&settings, budget_ms));
    let registration =
        if let (Some(generation), Some(track_id)) = (request_generation, track_id.as_ref()) {
            state.webview_audio_transport.begin(generation)?;
            state.webview_audio_transcoder.cancel_older_than(generation);
            let mut gate = state
                .playback_resolution
                .lock()
                .map_err(|_| "Playback resolver unavailable")?;
            let started = gate.begin(generation)?;
            state.webview_identification.clear();
            Some((generation, track_id.clone(), started.0, started.1))
        } else {
            None
        };
    let (idle_stop, idle_cancel) = tokio::sync::watch::channel(false);
    let (stop, cancel) = registration
        .as_ref()
        .map(|(_, _, stop, cancel)| (stop, cancel.clone()))
        .unwrap_or((&idle_stop, idle_cancel));
    // Use the caller's remaining absolute deadline through extraction and relay
    // setup. Timing out this subscriber must not invalidate other cache users.
    let audio_result = resolve_with_candidate_audio_deadline(
        transport_deadline,
        stop,
        state
            .acquisition
            .resolve_preview_audio_cancelled(&candidate, &settings, cancel),
    )
    .await;
    let audio = match audio_result {
        Ok(audio) => audio,
        Err(error) => {
            if let Some((generation, _, stop, _)) = registration.as_ref() {
                if let Ok(mut gate) = state.playback_resolution.lock() {
                    gate.finish(*generation, stop);
                }
            }
            return Err(error);
        }
    };
    let verified_candidate_binding =
        registration
            .as_ref()
            .and_then(|(generation, track_id, _, _)| {
                candidate_audio_stream_binding(
                    *generation,
                    track_id,
                    track_context.as_ref(),
                    &candidate,
                    &audio,
                )
            });
    let detection_source_identity = verified_candidate_binding
        .as_ref()
        .and_then(|binding| {
            playing_track_detections::SourceIdentity::new(
                &binding.candidate.provider,
                &binding.candidate.id,
            )
            .map(|identity| identity.with_duration_seconds(binding.candidate.duration_seconds))
        })
        .or_else(|| {
            audio.resolved_provider_id.as_deref().and_then(|source_id| {
                playing_track_detections::SourceIdentity::new(&candidate.provider, source_id)
                    .map(|identity| identity.with_duration_seconds(audio.duration_seconds))
            })
        });
    let mut renderer_url = audio.url.clone();
    let mut hls_transport = false;
    let mut normalization_relay = false;
    if let Some((generation, track_id, _, cancel)) = registration.as_ref() {
        let source = native_output::AudioSource {
            input: audio.url.clone(),
            ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
            headers: audio.headers.clone(),
            decoder_hint: Some(native_output::AudioSourceDecoderHint {
                mime_type: Some(audio.mime_type.clone()),
                codec: audio.codec.clone(),
                duration_seconds: audio.duration_seconds,
                content_length: audio.expected_content_length,
                has_video: audio.selected_format_has_video,
                decode_policy: crate::native_output::AudioSourceDecodePolicy::Automatic,
                decoder_fingerprint: None,
            }),

            cenc: None,
        };
        hls_transport = webview_audio_transport::is_hls(&source);
        if webview_audio_transport::needs_relay_for_normalization(
            &source,
            normalization_route_required.unwrap_or(false),
        ) {
            normalization_relay = true;
            renderer_url = match state
                .webview_audio_transport
                .register(
                    *generation,
                    track_id,
                    &source,
                    false,
                    transport_deadline,
                    cancel.clone(),
                )
                .await
            {
                Ok(url) => url,
                Err(error) => {
                    if let Some((generation, _, stop, _)) = registration.as_ref() {
                        if let Ok(mut gate) = state.playback_resolution.lock() {
                            gate.finish(*generation, stop);
                        }
                    }
                    return Err(error);
                }
            };
        }
    }
    let mut result = serde_json::json!({
        "kind": "audio", "url": renderer_url, "canSeek": true, "canVolume": true,
        "canAutoplay": true, "isExcerpt": false, "provider": candidate.provider,
        "title": candidate.title, "durationSeconds": audio.duration_seconds, "audioMimeType":if hls_transport { "application/vnd.apple.mpegurl".to_owned() } else { audio.mime_type }, "audioCodec":audio.codec,"audioBitrateKbps":audio.bitrate_kbps,
        "normalizationRoute":webview_audio_transport::normalization_route_capability(normalization_relay)
    });
    if let Some((generation, track_id, stop, cancel)) = registration {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(generation, &stop);
        if gate.generation == generation && !gate.stream_source_cancelled && !*cancel.borrow() {
            gate.resolved_stream_source = verified_candidate_binding;
            state.webview_identification.register(
                generation,
                track_id,
                crate::native_output::AudioSource {
                    input: audio.url.clone(),
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: audio.headers.clone(),
                    decoder_hint: None,

                    cenc: None,
                },
                detection_source_identity,
                false,
                None,
            );
            result["sourceGeneration"] = serde_json::json!(generation);
        }
    }
    Ok(result)
}

fn candidate_audio_stream_binding(
    generation: u64,
    track_id: &str,
    track_context: Option<&analysis_stream_selection::StreamMediaTrackContext>,
    candidate: &Candidate,
    audio: &acquisition::PreviewAudio,
) -> Option<ResolvedStreamSourceBinding> {
    let context = track_context?;
    if context.id != track_id || context.has_playback_range {
        return None;
    }
    if candidate.provider == "monochrome" {
        // Monochrome's public track URL is itself the stable source identity;
        // its playback probe intentionally has no extractor session/title/id.
        // Bind only the exact finite resource that was accepted for playback.
        let source_id = candidate.id.strip_prefix("monochrome:")?;
        if !(17..=20).contains(&source_id.len())
            || !source_id.bytes().all(|byte| byte.is_ascii_digit())
            || crate::monochrome::validate_url(&candidate.url).is_err()
            || candidate.url != format!("https://tracks.monochrome.st/track/{source_id}")
            || audio.url != candidate.url
        {
            return None;
        }
        let duration = audio
            .duration_seconds
            .filter(|duration| duration.is_finite() && (8.0..=900.0).contains(duration))?;
        let track = context
            .clone()
            .into_sanitized_track(duration, "monochrome", &candidate.id)
            .ok()?;
        let recording_identity =
            analysis_stream_selection::stream_media_identity_fingerprint(&track)?;
        let candidate = Candidate {
            id: format!("monochrome:{source_id}"),
            provider: "monochrome".into(),
            url: format!("https://tracks.monochrome.st/track/{source_id}"),
            title: context.title.clone(),
            duration_seconds: Some(duration),
            permission: "unknown".into(),
            confidence: "review".into(),
            ..Candidate::default()
        };
        return Some(ResolvedStreamSourceBinding {
            generation,
            track_id: track_id.to_owned(),
            recording_identity,
            candidate,
        });
    }
    if candidate.provider != "youtube" {
        return None;
    }
    let video_id = candidate.id.strip_prefix("youtube:")?;
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || audio.resolved_provider_id.as_deref() != Some(video_id)
        || !audio.provider_session_current()
    {
        return None;
    }
    let title = audio.resolved_title.as_deref().filter(|title| {
        !title.trim().is_empty() && title.len() <= 512 && !title.chars().any(char::is_control)
    })?;
    let duration = audio
        .duration_seconds
        .filter(|duration| duration.is_finite() && (8.0..=900.0).contains(duration))?;
    let track = match context
        .clone()
        .into_sanitized_track(duration, "youtube", &candidate.id)
    {
        Ok(track) => track,
        Err(_) if context.artist.is_empty() => context
            .clone()
            .into_active_youtube_handoff_track(duration, &candidate.id)
            .ok()?,
        Err(_) => return None,
    };
    let recording_identity = analysis_stream_selection::stream_media_identity_fingerprint(&track)?;
    let candidate = Candidate {
        id: format!("youtube:{video_id}"),
        provider: "youtube".into(),
        url: format!("https://www.youtube.com/watch?v={video_id}"),
        title: title.to_owned(),
        duration_seconds: Some(duration),
        permission: "unknown".into(),
        confidence: "review".into(),
        ..Candidate::default()
    };
    Some(ResolvedStreamSourceBinding {
        generation,
        track_id: track_id.to_owned(),
        recording_identity,
        candidate,
    })
}

fn active_stream_candidate_for_context(
    gate: &PlaybackResolutionGate,
    generation: u64,
    track: &analysis_stream_selection::StreamMediaTrackContext,
    duration_seconds: f64,
    provider: &str,
    source_id: &str,
) -> Result<Option<Candidate>> {
    match track
        .clone()
        .into_sanitized_track(duration_seconds, provider, source_id)
    {
        Ok(sanitized) => {
            let identity = analysis_stream_selection::stream_media_identity_fingerprint(&sanitized);
            Ok(gate.stream_candidate_for(
                generation,
                &sanitized.id,
                identity.as_deref(),
                provider,
                source_id,
            ))
        }
        Err(strict_error) if provider == "youtube" => {
            let sanitized = track
                .clone()
                .into_active_youtube_handoff_track(duration_seconds, source_id)
                .map_err(|handoff_error| {
                    format!("Active YouTube handoff projection rejected: {handoff_error}")
                })?;
            let identity = analysis_stream_selection::stream_media_identity_fingerprint(&sanitized);
            let candidate = gate.stream_candidate_for(
                generation,
                &sanitized.id,
                identity.as_deref(),
                provider,
                source_id,
            );
            match candidate {
                Some(candidate) => Ok(Some(candidate)),
                None => Err(format!(
                    "Active YouTube source rejected: {}",
                    gate.stream_candidate_rejection_reason(
                        generation,
                        &sanitized.id,
                        identity.as_deref(),
                        provider,
                        source_id,
                    )
                )),
            }
        }
        Err(error) => Err(error),
    }
}

fn stream_media_cache_lookup_track(
    gate: &PlaybackResolutionGate,
    context: &analysis_stream_selection::StreamMediaTrackContext,
    duration_seconds: f64,
    provider: &str,
    source_id: &str,
) -> Result<Track> {
    match context
        .clone()
        .into_sanitized_track(duration_seconds, provider, source_id)
    {
        Ok(track) => Ok(track),
        Err(strict_error) if provider == "youtube" && gate.generation > 0 => {
            let leased_candidate = active_stream_candidate_for_context(
                gate,
                gate.generation,
                context,
                duration_seconds,
                provider,
                source_id,
            )
            .map_err(|_| strict_error.clone())?;
            if leased_candidate.is_none() {
                return Err(strict_error);
            }
            context
                .clone()
                .into_active_youtube_handoff_track(duration_seconds, source_id)
                .map_err(|_| strict_error)
        }
        Err(error) => Err(error.into()),
    }
}

fn playback_candidate_search_needed(native_enabled: bool, track: &Track) -> bool {
    !native_enabled
        && !matches!(track.provider.as_str(), "radio" | "youtube" | "soundcloud")
        && track.local_path.is_none()
        && track.playback_range.is_none()
}
fn check_playback_candidate_search_budget(
    cancelled: bool,
    deadline: playback_sources::PlaybackDeadline,
) -> Result<()> {
    if cancelled {
        return Err("Playback candidate preparation cancelled".into());
    }
    if deadline.remaining().is_zero() {
        return Err("Playback candidate preparation deadline expired".into());
    }
    Ok(())
}

fn playback_trace_is_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn upcoming_candidate_search_trace_payload(
    phase: &'static str,
    request_id: &str,
    provider: &str,
    elapsed_ms: u128,
    details: serde_json::Value,
) -> serde_json::Value {
    let provider = match provider {
        "youtube" | "soundcloud" | "monochrome" | "bandcamp" => provider,
        _ => "other",
    };
    serde_json::json!({
        "resolutionId":request_id,
        "nativeOperationId":null,
        "nativeSeekOperationId":null,
        "webviewGeneration":null,
        "sharedLookupId":null,
        "atUnixMs":chrono::Utc::now().timestamp_millis(),
        "elapsedMs":elapsed_ms,
        "phase":phase,
        "provider":provider,
        "details":details
    })
}

fn emit_upcoming_candidate_search_trace(
    enabled: bool,
    phase: &'static str,
    request_id: &str,
    provider: &str,
    elapsed_ms: u128,
    details: impl FnOnce() -> serde_json::Value,
) {
    if !enabled {
        return;
    }
    let payload =
        upcoming_candidate_search_trace_payload(phase, request_id, provider, elapsed_ms, details());
    eprintln!("burrata_playback_phase {}", payload);
}

// Queue-owned identity facts and raw candidate search only: no audio extraction,
// expiring URL resolution, probing or foreground deadline changes.
#[tauri::command]
async fn warm_playback_candidate_search(
    state: State<'_, AppState>,
    track: Track,
    request_id: String,
) -> Result<()> {
    let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(8));
    let trace_enabled =
        playback_trace_is_enabled(std::env::var("BURRATA_PLAYBACK_TRACE").ok().as_deref());
    let trace_request_id = trace_enabled.then(|| request_id.clone());
    let guard = state.provider_reads.begin(request_id)?;
    let cancel = guard.cancellation();
    if *cancel.borrow() {
        return Err("Playback candidate preparation cancelled".into());
    }
    if !playback_candidate_search_needed(state.playback.native_enabled(), &track) {
        return Ok(());
    }
    let settings = state.store.settings()?;
    check_playback_candidate_search_budget(*cancel.borrow(), deadline)?;
    if playback_sources::source_resolution_is_pinned(&track, false, &settings, true) {
        return Ok(());
    }
    let provider = match settings.preferred_streaming_source.as_str() {
        "youtube" | "soundcloud" | "monochrome" => settings.preferred_streaming_source.clone(),
        _ => "youtube".to_owned(),
    };
    let started = std::time::Instant::now();
    if let Some(trace_request_id) = trace_request_id.as_deref() {
        emit_upcoming_candidate_search_trace(
            trace_enabled,
            "upcoming_candidate_search_start",
            trace_request_id,
            &provider,
            0,
            || serde_json::json!({"requestId":trace_request_id}),
        );
    }
    let trace = playback_sources::SourceTrace::new(trace_request_id.clone(), None, None);
    let search = crate::playback::warm_upcoming_playback_identity(
        &state.acquisition,
        &state.store,
        &track,
        &settings,
        std::slice::from_ref(&provider),
        cancel.clone(),
        deadline,
        &trace,
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis();
    let (ok, result_count, cancelled, deadline_expired) = match &search {
        Ok(found) => (true, Some(*found), false, false),
        Err(error) => (
            false,
            None,
            error == "Cancelled",
            deadline.remaining().is_zero(),
        ),
    };
    if let Some(trace_request_id) = trace_request_id.as_deref() {
        emit_upcoming_candidate_search_trace(
            trace_enabled,
            "upcoming_candidate_search_end",
            trace_request_id,
            &provider,
            elapsed_ms,
            || {
                serde_json::json!({
                    "requestId":trace_request_id,
                    "ok":ok,
                    "resultCount":result_count,
                    "cancelled":cancelled,
                    "deadline":deadline_expired
                })
            },
        );
    }
    search?;
    // Keep the guard alive until the search settles so cancellation reaches the
    // loader, releases its cache gate and never drops an independent waiter.
    drop(guard);
    Ok(())
}

// Resolve and validate the next WebView track while the current track is
// playing. AcquisitionManager retains the playable stream in its bounded,
// expiry-aware cache; playback resolution still obtains a fresh URL at handoff
// whenever the signed URL has expired or its TTL is unknown.
#[tauri::command]
async fn prepare_upcoming_playback_source(
    state: State<'_, AppState>,
    track: Track,
    request_id: String,
    handoff_in_ms: u64,
) -> Result<()> {
    let trace_enabled =
        playback_trace_is_enabled(std::env::var("BURRATA_PLAYBACK_TRACE").ok().as_deref());
    let trace_request_id = trace_enabled.then(|| request_id.clone());
    let operation_id = format!("preparation:webview:{request_id}");
    let guard = state.provider_reads.begin(request_id)?;
    let cancel = guard.cancellation();
    if *cancel.borrow() {
        return Err("Upcoming playback preparation cancelled".into());
    }
    if !playback_candidate_search_needed(state.playback.native_enabled(), &track) {
        return Ok(());
    }
    let settings = state.store.settings()?;
    if playback_sources::source_resolution_is_pinned(&track, false, &settings, true) {
        return Ok(());
    }
    let budget = handoff_in_ms.clamp(
        50,
        playback_sources::NEXT_SOURCE_DISCOVERY_BUDGET.as_millis() as u64,
    );
    let handoff = tokio::time::Instant::now() + std::time::Duration::from_millis(budget);
    let started = std::time::Instant::now();
    let preferred = settings.preferred_streaming_source.as_str();
    if let Some(trace_request_id) = trace_request_id.as_deref() {
        emit_upcoming_candidate_search_trace(
            trace_enabled,
            "upcoming_source_preparation_start",
            trace_request_id,
            preferred,
            0,
            || {
                serde_json::json!({
                    "requestId":trace_request_id,
                    "preparationKind":"validated_playable_source",
                    "budgetMs":budget,
                })
            },
        );
    }
    let resolution = playback_sources::resolve_for_preload_until(
        &state.store,
        &state.acquisition,
        &track,
        cancel.clone(),
        operation_id,
        handoff,
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis();
    let prepared_provider = resolution
        .as_ref()
        .ok()
        .and_then(|resolved| resolved.candidate.as_ref())
        .map(|candidate| candidate.provider.as_str())
        .unwrap_or(preferred);
    if let Some(trace_request_id) = trace_request_id.as_deref() {
        emit_upcoming_candidate_search_trace(
            trace_enabled,
            "upcoming_source_preparation_end",
            trace_request_id,
            prepared_provider,
            elapsed_ms,
            || {
                serde_json::json!({
                    "requestId":trace_request_id,
                    "preparationKind":"validated_playable_source",
                    "ok":resolution.is_ok(),
                    "cancelled":resolution.as_ref().is_err_and(|error| error == "Cancelled"),
                    "deadline":resolution.as_ref().is_err_and(|error| error.contains("deadline")),
                })
            },
        );
    }
    resolution.map(|_| ())
}

#[tauri::command]
async fn warm_track_previews(state: State<'_, AppState>, tracks: Vec<Track>) -> Result<()> {
    // Kept for older frontend clients. Browsing or selecting a row must not
    // launch speculative extraction/token/probe work beside actual playback.
    cancel_preview_warmup(&state.preview_warmup_cancel);
    if playback_trace_is_enabled(std::env::var("BURRATA_PLAYBACK_TRACE").ok().as_deref()) {
        eprintln!(
            "burrata_playback_phase {}",
            serde_json::json!({
                "phase":"browse_preview_warmup_skipped", "provider":"none",
                "details":{"reason":"eager_browse_preparation_disabled","trackCount":tracks.len().min(3)}
            })
        );
    }
    Ok(())
}

#[tauri::command]
async fn inspect_candidate(
    state: State<'_, AppState>,
    track: Track,
    candidate: Candidate,
) -> Result<Candidate> {
    state
        .acquisition
        .inspect_formats(&track, &candidate, &state.store.settings()?)
        .await
}
#[tauri::command]
fn assess_download_candidate(
    state: State<'_, AppState>,
    track: Track,
    candidate: Candidate,
) -> Result<download_verification::DownloadAssessment> {
    let level = state.store.settings()?.matching_level;
    let assessment = recording_match_context::assess(
        &state.store,
        &track,
        &candidate,
        level,
        recording_match::MatchStage::Resolved,
    );
    Ok(download_verification::DownloadAssessment::new(
        assessment, &track, &candidate,
    ))
}
#[tauri::command]
async fn enqueue_download(
    state: State<'_, AppState>,
    request: DownloadRequest,
) -> Result<DownloadJob> {
    state
        .acquisition
        .download(request, state.store.settings()?)
        .await
}
#[tauri::command]
async fn cancel_download(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.acquisition.cancel(&job_id).await
}
#[tauri::command]
async fn retry_download(state: State<'_, AppState>, job_id: String) -> Result<DownloadJob> {
    state
        .acquisition
        .retry(&job_id, state.store.settings()?)
        .await
}
#[tauri::command]
fn list_jobs(state: State<'_, AppState>) -> Result<Vec<DownloadJob>> {
    state.store.list_jobs()
}
#[tauri::command]
fn get_diagnostics(state: State<'_, AppState>) -> Vec<String> {
    state
        .diagnostics
        .lock()
        .map(|diagnostics| diagnostics.clone())
        .unwrap_or_default()
}
#[tauri::command]
fn get_playback_diagnostics_status() -> playback_diagnostics::DiagnosticsStatus {
    playback_diagnostics::status()
}
#[tauri::command]
fn set_playback_diagnostics_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<playback_diagnostics::DiagnosticsStatus> {
    let mut settings = state.store.settings()?;
    settings.playback_diagnostics_enabled = enabled;
    state.store.save_settings(&settings)?;
    playback_diagnostics::set_enabled(enabled);
    Ok(playback_diagnostics::status())
}
#[tauri::command]
fn clear_playback_diagnostics() -> playback_diagnostics::DiagnosticsStatus {
    playback_diagnostics::clear()
}

/// A detected song is a lyrics identity hint only. Revalidate its owner and
/// observation against the active resource cache on every command invocation.
fn authorize_detected_lyrics_input(
    state: &AppState,
    snapshot: &serde_json::Value,
    mut visible: Option<lyrics::LyricsRequestInput>,
) -> Result<Option<lyrics::LyricsRequestInput>> {
    let Some(input) = visible.as_mut() else {
        return Ok(visible);
    };
    let Some(detection) = input.detection.as_ref() else {
        return Ok(visible);
    };
    if input.radio || detection.track_id.is_empty() || !detection.detected_at_seconds.is_finite() {
        return Err("Detected lyrics identity is invalid".into());
    }
    let key = current_detection_resource_key(
        state,
        detection.generation,
        detection.seek_serial,
        &detection.track_id,
    )?;
    if key.as_str() != detection.resource_key {
        return Err("Detected lyrics source changed".into());
    }
    let view = playing_track_detections::read(&state.store, &key)
        .map_err(|_| "Detected lyrics are no longer available")?;
    if !view.enabled || view.resource_key != detection.resource_key {
        return Err("Track detection is not active for this source".into());
    }
    let position =
        lyrics::detection_playback_position(&state.automation.playback_tracking_snapshot())
            .ok_or("Current playback position is unavailable")?;
    if !lyrics::active_detection_matches(
        &view.observations,
        position,
        &input.track_name,
        &input.artist_name,
        detection.detected_at_seconds,
    ) {
        return Err("Detected song is no longer current".into());
    }
    // Native chapter snapshots keep the set as the playback track; the
    // verified visible identity deliberately supplies the song metadata.
    let _ = snapshot;
    input.detected_track_verified = true;
    input.duration_seconds = 0.0;
    input.playback_range = None;
    Ok(visible)
}

#[tauri::command]
async fn get_current_track_lyrics(
    state: State<'_, AppState>,
    visible_track: Option<lyrics::LyricsRequestInput>,
) -> Result<lyrics_discovery::Document> {
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    Ok(lyrics_discovery::current(&snapshot, visible_track, &state.store, &state.artists).await)
}
#[tauri::command]
async fn search_current_track_lyric_candidates(
    state: State<'_, AppState>,
    visible_track: Option<lyrics::LyricsRequestInput>,
) -> Result<lyrics_discovery::Page> {
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    Ok(lyrics_discovery::search(&snapshot, visible_track, &state.store, &state.artists).await)
}
#[tauri::command]
fn select_current_track_lyric_candidate(
    state: State<'_, AppState>,
    visible_track: Option<lyrics::LyricsRequestInput>,
    scope_id: String,
    candidate_id: String,
) -> Result<lyrics_discovery::Document> {
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    let result = lyrics_discovery::choose(&snapshot, visible_track, &scope_id, &candidate_id)?;
    lyrics_discovery::remember(&state.store, &scope_id, &candidate_id)?;
    Ok(result)
}
#[tauri::command]
async fn export_current_track_lyrics_text(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    visible_track: Option<lyrics::LyricsRequestInput>,
    source_hash: String,
    text: String,
) -> Result<serde_json::Value> {
    use tauri_plugin_dialog::DialogExt;
    if text.trim().is_empty() || text.len() > 131072 || text.contains('\0') {
        return Err("Lyrics text is empty or too large".into());
    }
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    let current =
        lyrics_discovery::current(&snapshot, visible_track, &state.store, &state.artists).await;
    if current.document.source_hash.as_deref() != Some(source_hash.as_str()) {
        return Err("Displayed lyrics changed; reload before exporting".into());
    }
    tokio::task::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Text", &["txt"])
            .set_file_name("lyrics.txt")
            .blocking_save_file()
        else {
            return Ok(serde_json::json!({"cancelled":true}));
        };
        let path = file
            .into_path()
            .map_err(|_| "Selected text path is unavailable".to_owned())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"cancelled":false}))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn save_current_track_lyrics_to_file(
    state: State<'_, AppState>,
    visible_track: Option<lyrics::LyricsRequestInput>,
    source_hash: String,
) -> Result<serde_json::Value> {
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    if lyrics::detected_request(visible_track.as_ref()).is_some() {
        return Err(
            "Export detected song lyrics as text; they do not belong to the full recording file"
                .into(),
        );
    }
    let (scope, _, id) = lyrics_discovery::context(&snapshot, visible_track.clone())
        .ok_or("No current lyrics scope")?;
    let visible_path = visible_track.as_ref().and_then(|t| t.local_path.clone());
    let current = lyrics_discovery::current(
        &snapshot,
        visible_track.clone(),
        &state.store,
        &state.artists,
    )
    .await;
    if lyrics_discovery::context(&state.playback.snapshot_page(0, 1), visible_track)
        .map(|v| v.0)
        .as_deref()
        != Some(scope.as_str())
    {
        return Err("Playback changed before saving lyrics".into());
    }
    if current.document.source_hash.as_deref() != Some(source_hash.as_str())
        || !matches!(current.document.status.as_str(), "synced" | "unsynced")
    {
        return Err("Displayed lyrics changed; reload before saving".into());
    }
    let id = id.ok_or("No current track")?;
    let track = state
        .store
        .get_track(&id)?
        .ok_or("Current track unavailable")?;
    let path = snapshot["track"]["localPath"]
        .as_str()
        .filter(|_| {
            snapshot["engine"] == "native"
                && matches!(
                    snapshot["status"].as_str(),
                    Some("playing" | "paused" | "buffering")
                )
        })
        .or(visible_path.as_deref())
        .or(track.local_path.as_deref())
        .ok_or("Download this song before saving lyrics")?;
    let asset = state
        .store
        .get_asset_by_path(path)?
        .ok_or("Registered local file unavailable")?;
    if asset.recording_id != id || track.playback_range.is_some() {
        return Err("Lyrics must belong to the full selected audio file".into());
    }
    let text =
        lyrics::embedding_text(&current.document).ok_or("No source lyrics available to embed")?;
    let settings = state.store.settings()?;
    let _writer_lease = state.store.try_library_write()?;
    let updated=media::embed_lyrics(&asset,&settings,&text,serde_json::json!({"sourceHash":source_hash,"provider":current.document.provider,"credit":current.document.matched,"selectionNotice":current.selection_notice})).await?;
    state.store.save_lyric_asset(&updated, &asset.hash)?;
    if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
        media::acknowledge_retag_journal(&settings, id)?;
    }
    Ok(serde_json::json!({"saved":true,"assetId":updated.id}))
}
#[tauri::command]
async fn translate_current_track_lyrics(
    state: State<'_, AppState>,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
    visible_track: Option<lyrics::LyricsRequestInput>,
    line_start: Option<usize>,
    line_end: Option<usize>,
    use_fallback: Option<bool>,
) -> Result<lyrics_translation::LyricsTranslationResponse> {
    let snapshot = state.playback.snapshot_page(0, 1);
    let visible_track = authorize_detected_lyrics_input(&state, &snapshot, visible_track)?;
    let selected = lyrics_discovery::selected(&snapshot, visible_track.clone())
        .or_else(|| lyrics_discovery::remembered(&state.store, &snapshot, visible_track.clone()));
    let current = if let Some(selected) = selected {
        selected.document
    } else {
        lyrics::get_current_track_lyrics_with_catalog(
            &snapshot,
            visible_track,
            &state.store,
            &state.artists,
        )
        .await
    };
    if !matches!(current.status.as_str(), "synced" | "unsynced") {
        return Err("Current playback does not have available lyrics to translate".into());
    }
    if !lyrics::displayed_document_matches(&current, &source_hash, &lines) {
        return Err("Current lyrics changed; reload them before translating".into());
    }
    let current_lines = current
        .lines
        .ok_or("Current playback does not have translatable lyrics")?;
    let manager = state.connections.clone();
    let connection = tokio::task::spawn_blocking(move || {
        if use_fallback.unwrap_or(false) {
            manager.lyrics_translation_using_fallback()
        } else {
            manager.lyrics_translation()
        }
    })
    .await
    .map_err(|error| error.to_string())??
    .ok_or("Lyrics translation is disabled in Connections")?;
    match (line_start, line_end) {
        (None, None) => {
            lyrics_translation::translate_current_document(connection, source_hash, current_lines)
                .await
        }
        (Some(start), Some(end)) => {
            lyrics_translation::translate_current_chunk(
                connection,
                source_hash,
                current_lines,
                start,
                end,
            )
            .await
        }
        _ => Err("Incomplete lyric translation batch".into()),
    }
}
#[tauri::command]
async fn export_playback_diagnostics(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    webview_context: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    use tauri_plugin_dialog::DialogExt;
    let settings = state.store.settings()?;
    let diagnostics = playback_diagnostics::export();
    let mut playback_context = playback_diagnostics_context::project(
        &state.playback.snapshot_page(0, 1),
        tools::target_triple(),
    );
    if let Some(webview) = playback_diagnostics_context::project_webview(webview_context.as_ref()) {
        if let Some(context) = playback_context.as_object_mut() {
            context.insert("engine".into(), serde_json::json!("webview"));
            if let Some(status) = webview.get("status") {
                context.insert("status".into(), status.clone());
            }
            context.insert("webview".into(), webview);
        }
    }
    let count = diagnostics.status.event_count;
    #[cfg(target_os = "macos")]
    let os_version = {
        let mut command = tokio::process::Command::new("/usr/bin/sw_vers");
        command.arg("-productVersion").kill_on_drop(true);
        tokio::time::timeout(std::time::Duration::from_secs(2), command.output())
            .await
            .ok()
            .and_then(|output| output.ok())
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|value| playback_diagnostics_context::safe_os_version(&value))
    };
    #[cfg(not(target_os = "macos"))]
    let os_version: Option<String> = None;
    let helpers = acquisition::check_dependencies(&settings).await.into_iter().map(|dependency| {
        let version = dependency.version.as_deref().and_then(playback_diagnostics::numeric_helper_version);
        serde_json::json!({"availabilityState":if dependency.name == "yt-dlp-ejs" { "unknown" } else if dependency.available { "available" } else { "unavailable" },"name":dependency.name,"available":if dependency.name == "yt-dlp-ejs" { None } else { Some(dependency.available) },"version":version})
    }).collect::<Vec<_>>();
    let configured = |names: &[&str]| {
        names
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    };
    let network_environment = serde_json::json!({
        "httpProxyConfigured":configured(&["HTTP_PROXY", "http_proxy"]),
        "httpsProxyConfigured":configured(&["HTTPS_PROXY", "https_proxy"]),
        "allProxyConfigured":configured(&["ALL_PROXY", "all_proxy"]),
        "noProxyConfigured":configured(&["NO_PROXY", "no_proxy"]),
        "customCaConfigured":configured(&["SSL_CERT_FILE", "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"]),
        "systemProxyChecked":false, "streamProbesUseProxy":false
    });
    let report = serde_json::json!({"diagnosticRevision":3,"networkEnvironment":network_environment,"diagnosticCoverage":{"publicSourcePages":true,"redactedExtractorErrors":true,"streamAuthorizationIndicators":true,"externalComparisons":"not_run"},"appVersion":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"osVersion":os_version,"architecture":std::env::consts::ARCH,"targetTriple":tools::target_triple(),"runtimePlayback":playback_context,"helpers":helpers,"settings":{"matchingLevel":settings.matching_level,"primaryStreamingSource":(["youtube","soundcloud","monochrome"].into_iter().find(|provider| *provider == settings.preferred_streaming_source)),"fallbackEnabled":settings.streaming_fallback_enabled,"fallbackSources":settings.streaming_fallback_sources.iter().filter(|provider| matches!(provider.as_str(),"youtube"|"soundcloud"|"monochrome")).take(3).collect::<Vec<_>>(),"fallbackDelayMs":settings.streaming_fallback_delay_ms,"startupTimeoutSeconds":settings.playback_startup_timeout_seconds,"bandcampFallbackEnabled":settings.bandcamp_fallback_enabled,"overridePlaylistTrackUrls":settings.override_playlist_track_urls},"playback":diagnostics});
    tauri::async_runtime::spawn_blocking(move || {
        let picked = app
            .dialog()
            .file()
            .set_title("Export playback diagnostic report")
            .add_filter("JSON report", &["json"])
            .set_file_name("burrata-playback-diagnostics.json")
            .blocking_save_file();
        let Some(picked) = picked else {
            return Ok(serde_json::json!({"cancelled":true}));
        };
        let path = picked
            .into_path()
            .map_err(|_| "Chosen report path is unavailable".to_owned())?;
        let bytes = serde_json::to_vec_pretty(&report)
            .map_err(|_| "Could not serialize diagnostic report".to_owned())?;
        std::fs::write(&path, bytes).map_err(|_| "Could not write diagnostic report".to_owned())?;
        Ok(serde_json::json!({"cancelled":false,"path":path.to_string_lossy(),"eventCount":count}))
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    state.store.settings()
}
#[tauri::command]
async fn get_live_bpm(
    state: State<'_, AppState>,
) -> Result<Option<playback::LiveBpmPlaybackEstimate>> {
    state.playback.live_bpm().await
}
#[tauri::command]
async fn get_live_playback_estimates(
    state: State<'_, AppState>,
) -> Result<Option<playback::LivePlaybackEstimates>> {
    state.playback.live_playback_estimates().await
}
#[tauri::command]
async fn estimate_webview_live_bpm(
    state: State<'_, AppState>,
    track_id: String,
    generation: u64,
    seek_serial: u64,
    sample_rate: u32,
    samples_base64: String,
) -> Result<webview_live_bpm::Outcome> {
    use webview_live_bpm::{matches_playing_owner, Outcome, OutcomeStatus, Owner};
    let owner = Owner {
        track_id,
        generation,
        seek_serial,
    };
    if owner.track_id.trim().is_empty() || owner.track_id.len() > 512 {
        return Err("WebView live BPM track identity is invalid".into());
    }
    if !state.store.settings()?.live_bpm_enabled {
        return Ok(Outcome::status(OutcomeStatus::Disabled));
    }
    if state.playback.native_enabled() {
        return Ok(Outcome::status(OutcomeStatus::NativeActive));
    }
    let before = state.automation.playback_tracking_snapshot();
    if !matches_playing_owner(&before, &owner) {
        return Ok(Outcome::status(OutcomeStatus::OwnerMismatchBefore));
    }
    let samples = webview_live_bpm::decode_pcm16(sample_rate, &samples_base64)?;
    let Some(worker_permit) = live_analysis_gate::try_acquire() else {
        return Ok(Outcome::status(OutcomeStatus::NoEstimate));
    };
    let sample_window_seconds = samples.len() as f64 / sample_rate as f64;
    let analysis_started = std::time::Instant::now();
    let (estimate, key_estimate) = tauri::async_runtime::spawn_blocking(move || {
        let _worker_permit = worker_permit;
        (
            live_bpm::estimate_interleaved_pcm(&samples, sample_rate, 1),
            live_key::estimate_interleaved_pcm(&samples, sample_rate, 1),
        )
    })
    .await
    .map_err(|error| format!("WebView live BPM analysis worker stopped: {error}"))?;
    let after = state.automation.playback_tracking_snapshot();
    if state.playback.native_enabled()
        || !state.store.settings()?.live_bpm_enabled
        || !matches_playing_owner(&after, &owner)
        || before.get("sessionId") != after.get("sessionId")
    {
        return Ok(Outcome::status(OutcomeStatus::OwnerChangedAfter));
    }
    let analysis_ms = analysis_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if estimate.is_none() && key_estimate.is_none() {
        let mut outcome = Outcome::status(OutcomeStatus::NoEstimate);
        outcome.sample_window_seconds = Some(sample_window_seconds);
        outcome.analysis_ms = Some(analysis_ms);
        return Ok(outcome);
    }
    Ok(Outcome {
        status: OutcomeStatus::Success,
        estimate: estimate.map(|estimate| {
            serde_json::json!({
                "trackId": owner.track_id.clone(),
                "generation": owner.generation,
                "seekSerial": owner.seek_serial,
                "bpm": estimate.bpm,
                "confidence": estimate.confidence,
                "relatedTempoAlternativeBpm": estimate.related_tempo_alternative_bpm,
                "windowSeconds": estimate.window_seconds,
            })
        }),
        key_estimate: key_estimate.map(|estimate| {
            serde_json::json!({
                "trackId": owner.track_id.clone(),
                "generation": owner.generation,
                "seekSerial": owner.seek_serial,
                "key": estimate.key,
                "profileFit": estimate.profile_fit,
                "profileMargin": estimate.profile_margin,
                "windowSeconds": estimate.window_seconds,
            })
        }),
        sample_window_seconds: Some(sample_window_seconds),
        analysis_ms: Some(analysis_ms),
    })
}
#[tauri::command]
async fn get_playback_waveform(
    state: State<'_, AppState>,
    start_seconds: Option<f64>,
    duration_seconds: Option<f64>,
) -> Result<pcm_tap::ObservedWaveform> {
    let playback = state.playback.clone();
    tauri::async_runtime::spawn_blocking(move || {
        playback.waveform_snapshot(
            start_seconds.unwrap_or(0.0),
            duration_seconds.unwrap_or(0.0),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
fn get_cached_stream_analysis(
    state: State<'_, AppState>,
    track_id: String,
    generation: u64,
) -> Option<stream_analysis::WholeTrackAnalysis> {
    state.playback.cached_stream_analysis(&track_id, generation)
}

#[tauri::command]
fn begin_webview_stream_analysis(
    state: State<'_, AppState>,
    session_id: String,
    generation: u64,
    recording_id: String,
    provider: String,
    source_id: String,
    duration_seconds: f64,
    initial_position_seconds: f64,
    audio_variant: Option<stream_analysis::StreamAudioVariant>,
) -> Result<()> {
    state
        .playback
        .observed_analysis_manager()
        .begin_webview_capture(
            &session_id,
            generation,
            &recording_id,
            &provider,
            &source_id,
            duration_seconds,
            initial_position_seconds,
            audio_variant.as_ref(),
        )
}

#[tauri::command]
fn push_webview_stream_analysis(
    state: State<'_, AppState>,
    session_id: String,
    generation: u64,
    end_position_seconds: f64,
    sample_rate: u32,
    samples_base64: String,
) -> Result<()> {
    use base64::Engine as _;
    if samples_base64.len() > ((stream_analysis::KEY_PCM_RATE as usize * 2 + 2) / 3) * 4 {
        return Err("WebView analysis PCM chunk exceeds its transport bound".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(samples_base64)
        .map_err(|_| "WebView analysis PCM chunk is malformed")?;
    if bytes.is_empty()
        || bytes.len() % 2 != 0
        || bytes.len() > stream_analysis::KEY_PCM_RATE as usize * 2
    {
        return Err("WebView analysis PCM chunk exceeds its format or size bound".into());
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    state
        .playback
        .observed_analysis_manager()
        .push_webview_capture(
            &session_id,
            generation,
            end_position_seconds,
            sample_rate,
            samples,
        )
}

#[tauri::command]
async fn finish_webview_stream_analysis(
    state: State<'_, AppState>,
    session_id: String,
    generation: u64,
    natural_end_position_seconds: f64,
) -> Result<stream_analysis::WholeTrackAnalysis> {
    let manager = state.playback.observed_analysis_manager();
    tauri::async_runtime::spawn_blocking(move || {
        manager.finish_webview_capture(&session_id, generation, natural_end_position_seconds)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn cancel_webview_stream_analysis(state: State<'_, AppState>, session_id: String) {
    state
        .playback
        .observed_analysis_manager()
        .cancel_webview_capture(&session_id);
}
#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<()> {
    settings.validate_playback_startup_timeout()?;
    if !matches!(settings.theme.as_str(), "dark" | "light" | "system") {
        return Err("Invalid theme".into());
    }
    if !matches!(
        settings.preferred_streaming_source.as_str(),
        "youtube" | "soundcloud" | "monochrome"
    ) {
        return Err("Choose Monochrome, YouTube or SoundCloud as the streaming source".into());
    }
    if !matches!(
        settings.preferred_download_source.as_str(),
        "youtube" | "soundcloud" | "slskd" | "monochrome"
    ) {
        return Err(
            "Choose YouTube, SoundCloud, Soulseek or Monochrome as the download source".into(),
        );
    }
    source_fallback::validate(&settings)?;
    if settings.playlist_directories.len() > 32
        || settings
            .playlist_directories
            .iter()
            .any(|root| !Path::new(root).is_absolute())
    {
        return Err("Playlist folders must be absolute paths (at most 32 folders)".into());
    }
    if settings.preview_delay_ms > 5000 {
        return Err("Preview delay must be at most 5000 ms".into());
    }
    let previous = state.store.settings()?;
    if previous.music_directory != settings.music_directory
        && state.store.list_jobs()?.iter().any(|j| {
            matches!(
                j.state.as_str(),
                "queued"
                    | "waiting_for_peer"
                    | "downloading"
                    | "validating"
                    | "tagging"
                    | "publishing"
                    | "finalizing"
            )
        })
    {
        return Err("Wait for active downloads to finish or cancel them before changing the music directory".into());
    }
    if !settings.music_directory.is_empty() {
        let path = Path::new(&settings.music_directory);
        if !path.is_absolute() {
            return Err("Music directory must be an absolute path".into());
        }
        std::fs::create_dir_all(path).map_err(|e| format!("Cannot create music directory: {e}"))?;
        app.asset_protocol_scope()
            .allow_directory(path, true)
            .map_err(|e| e.to_string())?;
    }
    if !settings.background_source_preparation {
        source_preparation::yield_to_playback();
    }
    state.store.save_settings(&settings)?;
    playback_diagnostics::set_enabled(settings.playback_diagnostics_enabled);
    Ok(())
}
#[tauri::command]
async fn get_song_radio(
    state: State<'_, AppState>,
    track: Track,
    candidate: Candidate,
    limit: Option<usize>,
    persist: Option<bool>,
) -> Result<radio::SongRadioResult> {
    let mut result = radio::start_song_radio(
        &track,
        &candidate,
        &state.store.settings()?,
        limit.unwrap_or(25),
    )
    .await?;
    if persist.unwrap_or(true) {
        result.tracks = result
            .tracks
            .iter()
            .map(|track| state.store.cache_track(track))
            .collect::<Result<Vec<_>>>()?;
    }
    Ok(result)
}
#[tauri::command]
async fn check_dependencies(state: State<'_, AppState>) -> Result<Vec<Dependency>> {
    Ok(acquisition::check_dependencies(&state.store.settings()?).await)
}
#[tauri::command]
fn list_track_files(state: State<'_, AppState>, track_id: String) -> Result<Vec<LocalAsset>> {
    state.store.list_track_files(&track_id)
}
#[tauri::command]
fn set_preferred_track_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    track_id: String,
    asset_id: String,
) -> Result<Track> {
    let track = state.store.set_preferred_track_file(&track_id, &asset_id)?;
    let _ = app.emit("library-updated", &track);
    Ok(track)
}
#[tauri::command]
fn set_remote_playlist_write_sync_enabled(
    state: State<'_, AppState>,
    provider: String,
    enabled: bool,
) -> Result<bool> {
    state
        .remote_playlists
        .set_write_sync_enabled(&provider, enabled)
}
#[tauri::command]
async fn remote_playlist_connection_status(
    state: State<'_, AppState>,
    provider: String,
) -> Result<remote_playlists::RemoteProviderStatus> {
    state.remote_playlists.connection_status(&provider).await
}
#[tauri::command]
async fn connect_remote_playlist_provider(
    state: State<'_, AppState>,
    provider: String,
) -> Result<remote_playlists::RemoteProviderStatus> {
    state.remote_playlists.connect(&provider).await
}
#[tauri::command]
fn disconnect_remote_playlist_provider(
    state: State<'_, AppState>,
    provider: String,
) -> Result<remote_playlists::RemoteProviderStatus> {
    state.remote_playlists.disconnect(&provider)
}
#[tauri::command]
async fn list_remote_playlists(
    state: State<'_, AppState>,
    provider: String,
    owner_id: Option<String>,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemotePlaylistPage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .list_playlists(&provider, owner_id.as_deref(), cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
async fn browse_remote_playlist(
    state: State<'_, AppState>,
    provider: String,
    playlist_id: String,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemotePlaylistBrowsePage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .browse_playlist(&provider, &playlist_id, cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
async fn browse_remote_artist(
    state: State<'_, AppState>,
    artist_id: String,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemoteArtistBrowsePage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .browse_artist(&artist_id, cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
async fn browse_remote_artist_tracks(
    state: State<'_, AppState>,
    artist_id: String,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemoteArtistTracksPage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .browse_artist_tracks(&artist_id, cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
async fn browse_remote_album_tracks(
    state: State<'_, AppState>,
    album_id: String,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemoteAlbumTracksPage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .browse_album_tracks(&album_id, cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
async fn prepare_spotify_recording_crosswalk(
    state: State<'_, AppState>,
    track_id: String,
    album_id_hint: Option<String>,
    music_brainz_recording_id_hint: Option<String>,
    request_id: String,
) -> Result<serde_json::Value> {
    if track_id.len() != 22 || !track_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("A Spotify track ID must contain exactly 22 letters or digits".into());
    }
    if request_id.len() != 36 || uuid::Uuid::parse_str(&request_id).is_err() {
        return Err("Remote read request ID must be a UUID".into());
    }
    if album_id_hint
        .as_deref()
        .is_some_and(|id| id.len() != 22 || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    {
        return Err("A Spotify album hint must contain exactly 22 letters or digits".into());
    }
    if album_id_hint.is_some() && music_brainz_recording_id_hint.is_some() {
        return Err("Choose either an album hint or a MusicBrainz recording hint".into());
    }
    if music_brainz_recording_id_hint
        .as_deref()
        .is_some_and(|id| !artist_credit_aliases::valid_musicbrainz_recording_id(id))
    {
        return Err("A MusicBrainz recording hint must be a valid recording ID".into());
    }

    let guard = state.provider_reads.begin(request_id)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    if let Some(recording_id) = music_brainz_recording_id_hint.as_deref() {
        let status = tokio::time::timeout_at(
            deadline,
            state
                .remote_playlists
                .prepare_spotify_artist_credit_crosswalk(
                    &state.artists,
                    &track_id,
                    recording_id,
                    deadline,
                    guard.cancellation(),
                ),
        )
        .await
        .map_err(|_| "Spotify recording verification exceeded its 30-second limit".to_owned())??;
        drop(guard);
        return serde_json::to_value(status)
            .map_err(|_| "Recording credit crosswalk serialization failed".to_owned());
    }
    let occurrence = tokio::time::timeout_at(
        deadline,
        state.remote_playlists.prepare_spotify_recording_crosswalk(
            &state.artists,
            &track_id,
            album_id_hint.as_deref(),
            deadline,
            guard.cancellation(),
        ),
    )
    .await
    .map_err(|_| "Spotify recording verification exceeded its 30-second limit".to_owned())??;
    drop(guard);

    Ok(serde_json::json!({
        "verified": true,
        "spotifyTrackId": occurrence.spotify_track_id,
        "spotifyAlbumId": occurrence.spotify_album_id,
        "spotifyPosition": occurrence.spotify_position,
        "musicbrainzReleaseId": occurrence.musicbrainz_release_id,
        "musicbrainzRecordingId": occurrence.musicbrainz_recording_id,
        "musicbrainzTrackId": occurrence.musicbrainz_track_id,
        "mediumPosition": occurrence.medium_position,
        "mediumTrackPosition": occurrence.medium_track_position,
    }))
}
#[tauri::command]
async fn search_remote_playlists(
    state: State<'_, AppState>,
    provider: String,
    query: String,
    kind: Option<String>,
    cursor: Option<String>,
    request_id: Option<String>,
) -> Result<remote_playlists::RemotePlaylistPage> {
    state
        .provider_reads
        .run(request_id, || async {
            state
                .remote_playlists
                .search(&provider, &query, kind.as_deref(), cursor.as_deref())
                .await
        })
        .await
}
#[tauri::command]
fn cancel_remote_read(state: State<'_, AppState>, request_id: String) -> Result<bool> {
    Ok(state.provider_reads.cancel(&request_id))
}
#[tauri::command]
async fn repair_spotify_durations_public(
    state: State<'_, AppState>,
) -> Result<remote_playlists::SpotifyDurationRepairResult> {
    state
        .remote_playlists
        .repair_spotify_durations_public()
        .await
}
#[tauri::command]
fn cancel_public_spotify_duration_repair(state: State<'_, AppState>) -> Result<bool> {
    state
        .remote_playlists
        .cancel_public_spotify_duration_repair()
}
#[tauri::command]
fn public_spotify_duration_repair_running(state: State<'_, AppState>) -> bool {
    state
        .remote_playlists
        .public_spotify_duration_repair_running()
}
#[tauri::command]
fn list_remote_playlist_bindings(
    state: State<'_, AppState>,
    provider: Option<String>,
) -> Result<Vec<remote_playlists::RemotePlaylistBindingStatus>> {
    state.remote_playlists.list_bindings(provider.as_deref())
}
#[tauri::command]
fn import_remote_playlists(
    state: State<'_, AppState>,
    provider: String,
    playlist_ids: Vec<String>,
) -> Result<remote_playlists::RemotePlaylistJobStatus> {
    state
        .remote_playlists
        .import_playlists(&provider, playlist_ids)
}
#[tauri::command]
fn get_remote_playlist_import_job(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<remote_playlists::RemotePlaylistJobStatus> {
    state.remote_playlists.import_job_status(&job_id)
}
#[tauri::command]
fn cancel_remote_playlist_import(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<remote_playlists::RemotePlaylistJobStatus> {
    state.remote_playlists.cancel_import(&job_id)
}
#[tauri::command]
async fn refresh_remote_playlist_binding(
    state: State<'_, AppState>,
    binding_id: String,
) -> Result<remote_playlists::RemotePlaylistImportItem> {
    state.remote_playlists.refresh(&binding_id).await
}
#[tauri::command]
async fn get_remote_playlist_sync_preview(
    state: State<'_, AppState>,
    binding_id: String,
    reviewed_matches: Option<Vec<remote_playlists::ReviewedRemoteTrackMatch>>,
) -> Result<remote_playlists::RemotePlaylistSyncPreview> {
    state
        .remote_playlists
        .get_sync_preview(&binding_id, reviewed_matches)
        .await
}
#[tauri::command]
async fn search_remote_playlist_writeback_candidates(
    state: State<'_, AppState>,
    binding_id: String,
    local_track_id: String,
) -> Result<remote_playlists::RemotePlaylistTrackMatches> {
    state
        .remote_playlists
        .search_remote_playlist_writeback_candidates(&binding_id, &local_track_id)
        .await
}
#[tauri::command]
async fn apply_remote_playlist_sync_preview(
    state: State<'_, AppState>,
    preview_id: String,
    changes: Option<remote_playlists::RemotePlaylistSyncChanges>,
) -> Result<remote_playlists::RemotePlaylistSyncPreview> {
    state
        .remote_playlists
        .apply_sync_preview(&preview_id, changes)
        .await
}
#[tauri::command]
fn list_assets(state: State<'_, AppState>) -> Result<Vec<LocalAsset>> {
    state.store.list_assets()
}
#[tauri::command]
async fn get_asset_by_id(
    state: State<'_, AppState>,
    asset_id: String,
) -> Result<Option<LocalAsset>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.get_asset_by_id(&asset_id))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_assets_for_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<LocalAsset>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.list_assets_for_paths(&paths))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_assets_by_ids(
    state: State<'_, AppState>,
    asset_ids: Vec<String>,
) -> Result<Vec<LocalAsset>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.list_assets_by_ids(&asset_ids))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_asset_artist_matches_page(
    state: State<'_, AppState>,
    names: Vec<String>,
    limit: usize,
    offset: usize,
) -> Result<serde_json::Value> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        let (items, total) = store.list_asset_artist_matches_page(&names, limit, offset)?;
        Ok(serde_json::json!({
            "items": items,
            "total": total,
            "limit": limit.clamp(1, 200),
            "offset": offset,
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_artist_local_assets_page(
    state: State<'_, AppState>,
    names: Vec<String>,
    limit: usize,
    offset: usize,
    query: Option<String>,
    sort_field: Option<String>,
    direction: Option<String>,
    min_duration_seconds: Option<u32>,
    max_duration_seconds: Option<u32>,
    availability: Option<String>,
) -> Result<serde_json::Value> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        let (items, total, sort_availability) = store.list_artist_local_assets_page(
            &names,
            limit,
            offset,
            query.as_deref(),
            sort_field.as_deref(),
            direction.as_deref(),
            min_duration_seconds,
            max_duration_seconds,
            availability.as_deref(),
        )?;
        Ok(serde_json::json!({
            "items": items,
            "total": total,
            "limit": limit.clamp(1, 200),
            "offset": offset,
            "sortAvailability": sort_availability,
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn match_artist_playback_sources(
    state: State<'_, AppState>,
    artist_names: Vec<String>,
    source_tracks: Vec<Track>,
) -> Result<Vec<Option<LocalAsset>>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        store.match_artist_playback_sources(&artist_names, &source_tracks)
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn list_track_files_for_recordings(
    state: State<'_, AppState>,
    recording_ids: Vec<String>,
) -> Result<Vec<storage::TrackFilesForRecording>> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.list_track_files_for_recordings(&recording_ids))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
fn count_assets(state: State<'_, AppState>) -> Result<usize> {
    state.store.count_assets()
}
#[tauri::command]
fn get_analysis_status_for_assets(
    state: State<'_, AppState>,
    asset_ids: Vec<String>,
) -> Result<Vec<serde_json::Value>> {
    state
        .analysis
        .status_for_assets(&asset_ids)
        .map_err(Into::into)
}

#[tauri::command]
fn get_loudness_settings(state: State<'_, AppState>) -> Result<loudness_manager::LoudnessSettings> {
    state.playback.loudness_settings()
}

#[tauri::command]
fn set_loudness_settings(
    state: State<'_, AppState>,
    settings: loudness_manager::LoudnessSettings,
) -> Result<loudness_manager::LoudnessStatus> {
    state.playback.set_loudness_settings(settings)
}

#[tauri::command]
fn get_loudness_decision_for_asset(
    state: State<'_, AppState>,
    asset_id: String,
    range: loudness::AudioRange,
    album_resource: Option<loudness::LoudnessResourceIdentity>,
) -> Result<loudness_manager::LoudnessStatus> {
    state
        .playback
        .loudness_decision_for_registered_asset(&asset_id, range, album_resource)
}

#[tauri::command]
fn set_loudness_album_context(
    state: State<'_, AppState>,
    resource: Option<loudness::LoudnessResourceIdentity>,
) -> Result<()> {
    state.playback.set_loudness_album_context(resource)
}

#[tauri::command]
fn start_loudness_analysis(
    state: State<'_, AppState>,
    asset_ids: Vec<String>,
    inter_track_gaps_seconds: Option<Vec<f64>>,
) -> Result<String> {
    let is_album = inter_track_gaps_seconds.is_some();
    let valid_count = if is_album {
        (2..=200).contains(&asset_ids.len())
    } else {
        (1..=50).contains(&asset_ids.len())
    };
    if !valid_count {
        return Err(if is_album {
            "Select 2–200 local tracks for an album programme analysis".into()
        } else {
            "Select 1–50 local tracks for loudness analysis".into()
        });
    }
    if asset_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != asset_ids.len()
    {
        return Err("Selected loudness assets must be unique".into());
    }
    if inter_track_gaps_seconds.as_ref().is_some_and(|gaps| {
        gaps.len() + 1 != asset_ids.len() || gaps.iter().any(|gap| !gap.is_finite() || *gap != 0.0)
    }) {
        return Err("Album analysis currently requires zero-gap queue boundaries".into());
    }
    let store = state.store.clone();
    let analysis_settings = store.settings()?;
    let ffmpeg = tools::executable(&analysis_settings.ffmpeg_path, "ffmpeg");
    let ffprobe = tools::executable(&analysis_settings.ffprobe_path, "ffprobe");
    let job_id = uuid::Uuid::new_v4().to_string();
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    state.loudness_analysis_jobs.insert(
        loudness_manager::LoudnessAnalysisJobStatus {
            job_id: job_id.clone(),
            status: "queued".into(),
            total: asset_ids.len(),
            completed: 0,
            error: None,
            resource: None,
        },
        cancel,
    )?;
    let jobs = state.loudness_analysis_jobs.clone();
    let accepted_job_id = job_id.clone();
    spawn_loudness_analysis_task(async move {
        let _ = jobs.update(&job_id, |status| status.status = "running".into());
        if let Some(gaps) = inter_track_gaps_seconds {
            let result = loudness_manager::analyze_selected_album(
                store,
                asset_ids.clone(),
                gaps,
                ffmpeg,
                ffprobe,
                cancel_rx,
            )
            .await;
            let _ = jobs.update(&job_id, |status| match result {
                Ok(record) => {
                    status.completed = status.total;
                    status.resource = Some(record.resource);
                    status.status = "completed".into();
                }
                Err(error) if error.contains("cancelled") => status.status = "cancelled".into(),
                Err(error) => {
                    status.status = "failed".into();
                    status.error = Some(error);
                }
            });
        } else {
            for asset_id in asset_ids {
                let was_cancelled = { *cancel_rx.borrow() };
                if was_cancelled {
                    let _ = jobs.update(&job_id, |status| status.status = "cancelled".into());
                    return;
                }
                let result = loudness_manager::analyze_selected_assets(
                    store.clone(),
                    vec![asset_id],
                    ffmpeg.clone(),
                    ffprobe.clone(),
                    cancel_rx.clone(),
                )
                .await;
                match result {
                    Ok(_) => {
                        let _ = jobs.update(&job_id, |status| status.completed += 1);
                    }
                    Err(error) if error.contains("cancelled") => {
                        let _ = jobs.update(&job_id, |status| status.status = "cancelled".into());
                        return;
                    }
                    Err(error) => {
                        let _ = jobs.update(&job_id, |status| {
                            status.status = "failed".into();
                            status.error = Some(error);
                        });
                        return;
                    }
                }
            }
            let _ = jobs.update(&job_id, |status| status.status = "completed".into());
        }
    });
    Ok(accepted_job_id)
}

fn spawn_loudness_analysis_task(task: impl std::future::Future<Output = ()> + Send + 'static) {
    // This command is synchronous and may run on a Tauri command thread without
    // a Tokio thread-local context. Tauri owns the application runtime.
    tauri::async_runtime::spawn(task);
}

#[cfg(test)]
mod loudness_task_launcher_tests {
    use super::spawn_loudness_analysis_task;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn synchronous_command_launcher_works_without_tokio_context() {
        let (finished, wait) = mpsc::channel();
        std::thread::spawn(move || {
            spawn_loudness_analysis_task(async move {
                let _ = finished.send(());
            });
        })
        .join()
        .expect("synchronous launcher thread should not panic");

        wait.recv_timeout(Duration::from_secs(5))
            .expect("Tauri runtime should execute task without a Tokio caller context");
    }
}

#[tauri::command]
fn get_loudness_analysis_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<loudness_manager::LoudnessAnalysisJobStatus> {
    state.loudness_analysis_jobs.status(&job_id)
}

#[tauri::command]
fn cancel_loudness_analysis(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.loudness_analysis_jobs.cancel(&job_id)
}
#[tauri::command]
fn start_local_waveform_analysis(
    state: State<'_, AppState>,
    asset_id: String,
    expected_hash: String,
    bin_count: usize,
) -> Result<String> {
    let settings = state.store.settings()?;
    state.waveform_analysis_jobs.start(
        state.store.clone(),
        asset_id,
        expected_hash,
        bin_count,
        tools::executable(&settings.ffmpeg_path, "ffmpeg"),
    )
}
#[tauri::command]
fn get_local_waveform_analysis_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<waveform_analysis::WaveformJobStatus> {
    state.waveform_analysis_jobs.status(&job_id)
}
#[tauri::command]
fn cancel_local_waveform_analysis(state: State<'_, AppState>, job_id: String) -> Result<()> {
    state.waveform_analysis_jobs.cancel(&job_id)
}
#[tauri::command]
async fn get_local_waveform_analysis(
    state: State<'_, AppState>,
    asset_id: String,
    expected_hash: String,
    bin_count: usize,
) -> Result<Option<waveform_analysis::LocalWaveform>> {
    waveform_analysis::cached_waveform(state.store.clone(), asset_id, expected_hash, bin_count)
        .await
}
#[tauri::command]
fn read_analysis_cache_record(
    state: State<'_, AppState>,
    namespace: String,
    key: String,
) -> Result<Option<serde_json::Value>> {
    state.store.read_analysis_cache_record(&namespace, &key)
}
#[tauri::command]
fn write_analysis_cache_record(
    state: State<'_, AppState>,
    namespace: String,
    key: String,
    payload: serde_json::Value,
    recording_id: Option<String>,
) -> Result<()> {
    state
        .store
        .write_analysis_cache_record(&namespace, &key, &payload, recording_id.as_deref())
}
#[tauri::command]
async fn import_local_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Track> {
    import_local_file_inner(&app, &state, &path, None).await
}
async fn import_local_file_inner(
    app: &tauri::AppHandle,
    state: &AppState,
    selected_path: &str,
    expected_seal: Option<&LocalAudioFileSeal>,
) -> Result<Track> {
    let path = std::fs::canonicalize(selected_path).map_err(|e| e.to_string())?;
    if !path.is_file() || !is_supported_import_audio(&path) {
        return Err("Choose a supported audio file".into());
    }
    if let Some(expected) = expected_seal {
        if local_audio_file_seal(&path)? != *expected {
            return Err("A selected audio file changed after preview. Choose it again and review a fresh preview.".into());
        }
    }
    let settings = state.store.settings()?;
    let mut t = media::inspect_local_file(&path, &settings).await?;
    let hash = hash_path(&path)?;
    if let Some(expected) = expected_seal {
        if local_audio_file_seal(&path)? != *expected {
            return Err("A selected audio file changed while it was being imported. Review a fresh preview.".into());
        }
    }
    t.provider = "local".into();
    t.provider_id = hash.clone();
    t.id = format!("local:{hash}");
    t.local_path = Some(path.to_string_lossy().into_owned());
    t.source_url = format!("local:{hash}");
    let a = LocalAsset {
        sample_rate: None,
        channels: None,
        bit_depth: None,
        duration_seconds: None,
        size_bytes: None,
        bitrate_origin: None,
        id: uuid::Uuid::new_v4().to_string(),
        recording_id: t.id.clone(),
        path: path.to_string_lossy().into_owned(),
        hash,
        source_codec: t.provenance["codec"].as_str().unwrap_or("unknown").into(),
        final_codec: t.provenance["codec"].as_str().unwrap_or("unknown").into(),
        source_bitrate: t.provenance["bitrateKbps"].as_f64(),
        final_bitrate: t.provenance["bitrateKbps"].as_f64(),
        processing: "existing-local-file".into(),
        track: t.clone(),
    };
    if let Some(existing) = state.store.find_asset_by_hash(&a.hash)? {
        return Ok(existing.track);
    }
    state.store.save_asset(&a)?;
    app.asset_protocol_scope()
        .allow_file(path)
        .map_err(|e| e.to_string())?;
    state
        .store
        .get_track(&t.id)?
        .ok_or_else(|| "Imported track disappeared".into())
}
#[tauri::command]
async fn export_mp3(state: State<'_, AppState>, asset_id: String) -> Result<LocalAsset> {
    let a = state
        .store
        .get_asset_by_id(&asset_id)?
        .ok_or("Asset not found")?;
    let _writer_lease = state.store.try_library_write()?;
    let a = media::export_mp3(&a, &state.store.settings()?).await?;
    state.store.save_asset(&a)?;
    media::acknowledge_publication(&state.store.settings()?, &a.id)?;
    Ok(a)
}
#[tauri::command]
fn get_playlist_hierarchy(
    state: State<'_, AppState>,
) -> Result<playlist_hierarchy::PlaylistHierarchy> {
    playlist_hierarchy::load(&state.store)
}
#[tauri::command]
fn save_playlist_hierarchy(
    state: State<'_, AppState>,
    hierarchy: playlist_hierarchy::PlaylistHierarchy,
) -> Result<playlist_hierarchy::PlaylistHierarchy> {
    let _library_write = state.store.try_library_write()?;
    playlist_hierarchy::save(&state.store, &hierarchy)
}
#[tauri::command]
fn place_imported_playlist_items(
    state: State<'_, AppState>,
    collection_id: String,
    folder_path: String,
    section_name: String,
    occurrence_count: usize,
) -> Result<playlist_hierarchy::PlaylistHierarchy> {
    let _library_write = state.store.try_library_write()?;
    playlist_hierarchy::place_imported_items(
        &state.store,
        &collection_id,
        &folder_path,
        &section_name,
        occurrence_count,
    )
}
#[tauri::command]
fn preview_portable_playlist_export(
    state: State<'_, AppState>,
    collection_id: String,
    options: portable_playlist::PlaylistExportOptions,
) -> Result<portable_playlist::PlaylistExportPreview> {
    playlist_sync::preview_portable_playlist_export(&state.store, &collection_id, &options)
}
#[tauri::command]
fn render_portable_playlist_export(
    state: State<'_, AppState>,
    collection_id: String,
    options: portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    playlist_sync::render_portable_playlist_export(&state.store, &collection_id, &options)
}
#[tauri::command]
fn export_portable_playlist(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    collection_id: String,
    options: portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    let fallback_download_dir = (options.purpose == portable_playlist::PlaylistPurpose::Share)
        .then(|| app.path().download_dir().ok())
        .flatten();
    playlist_sync::export_portable_playlist_path_with_fallback(
        &state.store,
        &collection_id,
        &options,
        fallback_download_dir.as_deref(),
    )
}
#[tauri::command]
async fn preview_portable_playlist_import(
    state: State<'_, AppState>,
    text: String,
    options: playlist_sync::PortablePlaylistImportOptions,
) -> Result<playlist_sync::PortablePlaylistImportPreview> {
    let settings = state.store.settings()?;
    playlist_sync::preview_portable_playlist_import_with_options(
        &state.store,
        &state.acquisition,
        &settings,
        &text,
        options,
    )
    .await
}
#[tauri::command]
async fn apply_portable_playlist_import(
    state: State<'_, AppState>,
    text: String,
    preview_id: String,
    expected_source_hash: String,
    destination_name: Option<String>,
    destination_folder_path: Option<String>,
    provider_selections: Vec<playlist_sync::PortablePlaylistProviderSelection>,
) -> Result<playlist_files::LocalPlaylistImport> {
    let settings = state.store.settings()?;
    playlist_sync::apply_portable_playlist_import_with_providers(
        &state.store,
        &state.acquisition,
        &settings,
        &text,
        &preview_id,
        &expected_source_hash,
        destination_name.as_deref(),
        destination_folder_path.as_deref(),
        &provider_selections,
    )
    .await
}
#[tauri::command]
async fn resolve_portable_playback_candidate(
    state: State<'_, AppState>,
    track: Track,
) -> Result<Option<Candidate>> {
    let settings = state.store.settings()?;
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    playlist_sync::resolve_portable_playback_candidate(
        &state.store,
        &state.acquisition,
        &track,
        &settings,
        receiver,
    )
    .await
}
#[tauri::command]
fn preview_playlist_mirror_migration(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<playlist_sync::PlaylistMirrorMigrationPreview> {
    playlist_sync::preview_playlist_mirror_migration(
        &state.store,
        &state.store.settings()?,
        &collection_id,
    )
}
#[tauri::command]
fn migrate_playlist_mirror_v1(
    state: State<'_, AppState>,
    collection_id: String,
    expected_source_hash: String,
) -> Result<String> {
    playlist_sync::migrate_playlist_mirror_v1(&state.store, &collection_id, &expected_source_hash)
}
#[tauri::command]
fn preview_history_playlist_export(
    state: State<'_, AppState>,
    options: portable_playlist::PlaylistExportOptions,
) -> Result<portable_playlist::PlaylistExportPreview> {
    playlist_sync::preview_history_export(&state.history, &options)
}
#[tauri::command]
fn render_history_playlist_export(
    state: State<'_, AppState>,
    options: portable_playlist::PlaylistExportOptions,
) -> Result<String> {
    playlist_sync::render_history_export(&state.history, &options)
}
#[tauri::command]
fn get_history_mirror_options(
    state: State<'_, AppState>,
) -> Result<playlist_sync::HistoryMirrorOptions> {
    playlist_sync::get_history_mirror_options(&state.store)
}
#[tauri::command]
fn set_history_mirror_options(
    state: State<'_, AppState>,
    options: playlist_sync::HistoryMirrorOptions,
) -> Result<playlist_sync::HistoryMirrorOptions> {
    playlist_sync::set_history_mirror_options(&state.store, options)
}
#[tauri::command]
fn preview_restored_playlist_sync(
    state: State<'_, AppState>,
) -> Result<playlist_sync::RestoredSyncResumePreview> {
    playlist_sync::preview_restored_playlist_sync(&state.store, &state.store.settings()?)
}
#[tauri::command]
fn resume_reviewed_restored_playlist_sync(
    state: State<'_, AppState>,
    review_token: String,
) -> Result<()> {
    playlist_sync::resume_reviewed_restored_playlist_sync(
        &state.store,
        &state.store.settings()?,
        &review_token,
    )
}
#[tauri::command]
fn resume_history_playlist_mirror(state: State<'_, AppState>) -> Result<String> {
    playlist_sync::resume_history_mirror(&state.store, &state.history, &state.store.settings()?)
}
#[tauri::command]
fn export_playlist(state: State<'_, AppState>, collection_id: String) -> Result<String> {
    playlist_sync::export_playlist_path(&state.store, &state.store.settings()?, &collection_id)
}
#[tauri::command]
fn resume_playlist_mirror(state: State<'_, AppState>, collection_id: String) -> Result<String> {
    playlist_sync::resume_playlist_mirror(&state.store, &state.store.settings()?, &collection_id)
}
#[tauri::command]
fn copy_playlist(state: State<'_, AppState>, collection_id: String) -> Result<Collection> {
    let _library_write = state.store.try_library_write()?;
    let id = playlist_sync::ensure_editable_collection(&state.store, &collection_id)?;
    state
        .store
        .list_collections()?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or("Playlist not found".into())
}
#[tauri::command]
fn export_mixxx_playlist(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<playlist_sync::MixxxPlaylistExport> {
    playlist_sync::export_mixxx_playlist(&state.store, &state.store.settings()?, &collection_id)
}
#[tauri::command]
fn move_playlist_entry(
    state: State<'_, AppState>,
    collection_id: String,
    from: usize,
    to: usize,
) -> Result<String> {
    let _library_write = state.store.try_library_write()?;
    let id = playlist_sync::ensure_editable_collection(&state.store, &collection_id)?;
    let mut tracks = state.store.list_tracks("collection", Some(&id))?;
    if from >= tracks.len() || to >= tracks.len() {
        return Err("Playlist position is out of range".into());
    }
    let track = tracks.remove(from);
    tracks.insert(to, track);
    state.store.replace_playlist_entries(&id, &tracks)?;
    Ok(id)
}
#[tauri::command]
async fn open_in_file_manager(state: State<'_, AppState>, path: String) -> Result<()> {
    let settings = state.store.settings()?;
    let registered = state.store.get_asset_by_path(&path)?.is_some()
        || (!settings.music_directory.is_empty()
            && Path::new(&path).starts_with(&settings.music_directory));
    if !registered {
        return Err("This path is not in the Burrata music library".into());
    }
    media::open_in_file_manager(Path::new(&path)).await
}
#[tauri::command]
async fn open_external_url(url: String) -> Result<()> {
    let u = url::Url::parse(&url).map_err(|e| e.to_string())?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() || !u.username().is_empty()
    {
        return Err("Only HTTP(S) provider pages can be opened".into());
    }
    #[cfg(target_os = "linux")]
    let mut c = tokio::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut c = tokio::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut c = {
        let mut c = tokio::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    let status = c
        .arg(u.as_str())
        .status()
        .await
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Cannot open provider page".into())
    }
}
fn hash_path(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn webview_media_event_trace_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn webview_media_event_trace_script() -> &'static str {
    if webview_media_event_trace_enabled(
        std::env::var("BURRATA_WEBVIEW_MEDIA_EVENT_TRACE")
            .ok()
            .as_deref(),
    ) {
        "try{sessionStorage.setItem('burrata.webviewMediaEventDiagnostics','1')}catch{}"
    } else {
        ""
    }
}

pub fn run() {
    // Opt-in diagnostic; default builds install no logger and alter no protocol behavior.
    artwork_protocol_trace::initialize();
    let mut context = tauri::generate_context!();
    if let Some(main_window) = context
        .config_mut()
        .app
        .windows
        .iter_mut()
        .find(|window| window.label == "main")
    {
        // Windows keeps native drag/drop disabled so HTML file reordering remains available.
        main_window.drag_drop_enabled = !cfg!(target_os = "windows");
    }
    tauri::Builder::default()
        .append_invoke_initialization_script(webview_media_event_trace_script())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let backup_frontend_preference_session_id = uuid::Uuid::new_v4().to_string();
            let startup_trace = std::env::var("BURRATA_STARTUP_TRACE").is_ok_and(|value| value == "1");
            let startup_started = std::time::Instant::now();
            let startup_mark = |phase: &str| {
                if startup_trace {
                    eprintln!("[startup] phase={} elapsed_ms={}", phase, startup_started.elapsed().as_millis());
                }
            };
            startup_mark("setup.begin");
            let import_drops = Arc::new(import_drop::ImportDropRegistry::default());
            if let Some(main_window) = app.get_webview_window("main") {
                let registry = import_drops.clone();
                let event_window = main_window.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {
                        paths,
                        position,
                        ..
                    }) = event
                    {
                        let token = registry.register(paths);
                        let payload = import_drop::ImportFileDropEvent {
                            token: token.clone(),
                            kind: import_drop::event_kind_hint(paths),
                            coordinate_space: import_drop::native_drop_coordinate_space(),
                            position: import_drop::DropPoint {
                                // The event declares whether native units are logical or physical.
                                x: position.x,
                                y: position.y,
                            },
                        };
                        if event_window.emit("import-file-drop", payload).is_err() {
                            registry.discard(&token);
                        }
                    }
                });
            }
            app.manage(shutdown_signal::install(app.handle().clone())?);
            let resources=release_resource_root(app.handle()).map_err(std::io::Error::other)?;
            tools::initialize(resources.clone());

            let dir = std::env::var_os("BURRATA_APP_DATA").map(PathBuf::from).unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&dir)?;
            // Tauri restart starts its replacement process before this process
            // exits. Keep the profile locked until process teardown so a
            // restored database pair is never swapped under open writers.
            let profile_lock = backup_manager::acquire_profile_lock(&dir)
                .map_err(std::io::Error::other)?;
            let backup_staging = backup_staging::BackupStaging::open(&dir)?;
            backup_staging.cleanup_abandoned()?;
            let restored_profile = backup_manager::apply_pending_restore_with_staging(&dir, &backup_staging)
                .map_err(std::io::Error::other)?;
            // Asset protocol grants are process-local. Restored playback can already
            // contain asset:// artwork URLs and skip the HTTPS cache command, so
            // regrant only our own flat artwork cache before the frontend mounts.
            for path in artwork_cache::cached_image_files(&app.path().app_data_dir()?.join("artwork")) {
                app.asset_protocol_scope().allow_file(&path)?;
            }
            let yt=tools::bundled_executable("yt-dlp").unwrap_or_else(||tools::executable("system:yt-dlp","yt-dlp"));
            let node=tools::bundled_executable("node").or_else(||Some(tools::executable("system:node","node")));
            let helper=resources.join("youtube-support/bgutil/server/build/main.js");
            let plugin=resources.join("youtube-support/bgutil/plugin");
            let manager=youtube_support::YouTubeSupportManager::new(dir.join("youtube-support"),yt,node,helper.is_file().then_some(helper),plugin.is_dir().then_some(plugin));
            youtube_support::initialize_global(manager.clone()).map_err(std::io::Error::other)?;
            tauri::async_runtime::spawn(async move{loop{tokio::time::sleep(std::time::Duration::from_secs(30)).await;let _=manager.stop_helper_if_idle().await;}});

            startup_mark("store.open.begin");
            let store =
                Arc::new(Store::open(&dir.join("burrata.sqlite")).map_err(std::io::Error::other)?);
            startup_mark("store.open.end");
            // The staged database carries the authoritative restore marker.
            // Repair older committed restores before any automatic workers
            // are constructed below.
            let restore_pause_incomplete = [
                restore_work_policy::AutomaticWorkKind::MetadataRetagAnalysis,
                restore_work_policy::AutomaticWorkKind::AssetAutotag,
                restore_work_policy::AutomaticWorkKind::BackgroundSourcePreparation,
            ]
            .into_iter()
            .any(|kind| !restore_work_policy::is_paused(&store, kind));
            if restored_profile && restore_pause_incomplete {
                restore_work_policy::pause_all_after_restore(
                    &store,
                    "startup-recovered-restore",
                )
                .map_err(std::io::Error::other)?;
            }
            let directory_browser = directory_browser::DirectoryBrowser::new(store.clone());
            startup_mark("store.downloads_backfill.begin");
            store.ensure_downloads_collection().map_err(std::io::Error::other)?;
            store.backfill_managed_downloads().map_err(std::io::Error::other)?;
            startup_mark("store.downloads_backfill.end");
            #[cfg(target_os = "linux")]
            if let Some(window) = app.get_webview_window("main") {
                candidate_preview::enable_candidate_autoplay(&window).map_err(std::io::Error::other)?;
            }
            let settings = store.settings().map_err(std::io::Error::other)?;
            playback_diagnostics::set_enabled(settings.playback_diagnostics_enabled);
            let history = Arc::new(playback_history::PlaybackHistory::open_with_barrier(
                &dir.join("playback-history.sqlite"),
                store.library_write_barrier(),
            ).map_err(std::io::Error::other)?);
            let mut diagnostics=Vec::new();
            if let Err(error) = backup_manager::finalize_pending_restore(&dir) {
                diagnostics.push(format!("Restore cleanup needs review: {error}"));
            }
            startup_mark("store.recovery.begin");
            match media::recover_publications(&settings) {
                Ok(recovered)=>for (job_id,asset) in recovered {
                    let commit: Result<()>=(||{
                        store.save_asset(&asset)?;
                        if let Ok(mut job)=store.get_job(&job_id) {
                            job.output_path=Some(asset.path.clone());job.progress=100.0;
                            if job.output_profile=="mp3" && asset.final_codec!="mp3" {
                                job.state="failed".into();job.error=Some("Source publication recovered. Create an MP3 compatibility export from Library.".into());
                            }else{job.state="completed".into();job.error=None;}
                            store.save_job(&job)?;
                        }
                        media::acknowledge_publication(&settings,&job_id)
                    })();
                    if let Err(e)=commit{diagnostics.push(format!("Publication recovery needs review: {e}"));}
                },
                Err(e)=>diagnostics.push(format!("Publication recovery needs review: {e}")),
            }
            match media::recover_retag_journals(&settings) {
                Ok(recovered) => for (id, asset) in recovered {
                    let result: Result<()> = (|| {
                        let previous = store.get_asset_by_id(&asset.id)?.ok_or("Retag recovery asset is missing from the library")?;
                        let payload = serde_json::json!({"bpm":asset.track.bpm,"key":asset.track.key,"bpmAnalysis":asset.track.provenance.get("bpmAnalysis"),"keyAnalysis":asset.track.provenance.get("keyAnalysis")});
                        if asset.track.provenance["retagKind"] == "autotag" {
                            let mut recovered_job=store.list_autotag_records()?.into_iter().find(|row|row["assetId"]==asset.id && row["inputHash"]==previous.hash).map(|row|row["payload"].clone()).unwrap_or_else(||serde_json::json!({"assetId":asset.id,"version":autotag::VERSION,"attempts":0,"nextAttemptAt":null,"message":"Recovered tag write","candidates":[],"chosen":null,"approvedFields":[],"excerptOnly":false}));
                            recovered_job["inputHash"]=serde_json::json!(asset.hash);recovered_job["status"]=serde_json::json!("applied");recovered_job["updatedAt"]=serde_json::json!(chrono::Utc::now().to_rfc3339());
                            store.save_autotagged_asset(&asset, &previous.hash, autotag::VERSION, &recovered_job)?;
                        } else if asset.track.provenance["retagKind"] == "lyrics" {store.save_lyric_asset(&asset,&previous.hash)?;} else { store.save_analyzed_asset(&asset, &previous.hash, analysis::VERSION, &payload)?; }
                        media::acknowledge_retag_journal(&settings, &id)
                    })();
                    if let Err(error) = result { diagnostics.push(format!("Tag recovery needs review: {error}")); }
                },
                Err(error) => diagnostics.push(format!("Tag recovery needs review: {error}")),
            }
            startup_mark("store.recovery.end");
            if !settings.music_directory.is_empty() {
                app.asset_protocol_scope()
                    .allow_directory(PathBuf::from(&settings.music_directory), true)?;
            }
            let asset_scope = app.asset_protocol_scope();
            let asset_grants_started = std::time::Instant::now();
            let mut asset_grant_count = 0usize;
            let mut initial_scope_allowed_count = 0usize;
            let mut classification_pages = 0usize;
            let mut allow_file_calls = 0usize;
            if settings.music_directory.is_empty() {
                // With no recursive music-root grant, keep the original
                // one-pass paged behavior: there is no existing scope to reuse.
                let mut offset = 0usize;
                loop {
                    let paths = store.list_asset_paths_page(500, offset).map_err(std::io::Error::other)?;
                    if paths.is_empty() { break; }
                    let page_len = paths.len();
                    asset_grant_count += page_len;
                    for path in &paths { asset_scope.allow_file(path)?; }
                    allow_file_calls += page_len;
                    offset += page_len;
                    if page_len < 500 { break; }
                }
            } else {
                let music_root = Path::new(&settings.music_directory);
                let mut offset = 0usize;
                let mut needs_explicit_grant = Vec::new();
                // Classify against the unchanged initial scope. Only paths
                // lexically beneath the configured root need exact scope checks;
                // external paths retain the old explicit grant behavior.
                // Retain only grant-required path strings in original order.
                loop {
                    let paths = store.list_asset_paths_page(500, offset).map_err(std::io::Error::other)?;
                    if paths.is_empty() { break; }
                    let page_len = paths.len();
                    asset_grant_count += page_len;
                    let (page_needs_grant, page_already_allowed) =
                        asset_paths_needing_explicit_grants(&asset_scope, paths, Some(music_root));
                    needs_explicit_grant.extend(page_needs_grant);
                    initial_scope_allowed_count += page_already_allowed;
                    classification_pages += 1;
                    offset += page_len;
                    if page_len < 500 { break; }
                }
                allow_file_calls = needs_explicit_grant.len();
                for path in &needs_explicit_grant {
                    asset_scope.allow_file(path)?;
                }
            }
            if startup_trace {
                eprintln!("[startup] phase=asset_protocol_grants count={} classification_pages={} allow_file_calls={} reused_existing={} elapsed_ms={} total_elapsed_ms={}",
                    asset_grant_count, classification_pages, allow_file_calls, initial_scope_allowed_count,
                    asset_grants_started.elapsed().as_millis(), startup_started.elapsed().as_millis());
            }
            let connections = Arc::new(connections::ConnectionManager::new(store.clone()));
            let acquisition =
                Arc::new(AcquisitionManager::new(app.handle().clone(), store.clone(), connections.clone()));
            let artists = Arc::new(artists::ArtistService::new(store.clone()).map_err(std::io::Error::other)?);
            let analysis = Arc::new(analysis::AnalysisManager::new(app.handle().clone(), store.clone()));
            acquisition.set_analysis_manager(analysis.clone());
            let discovery = Arc::new(discovery::DiscoveryService::new(artists.clone(), acquisition.clone(), store.clone(), connections.clone()));
            let autotag = autotag::AutoTagger::new(app.handle().clone(), store.clone(), artists.clone(), connections.clone());
            acquisition.set_autotag_manager(autotag.clone());
            let recovery_acquisition = acquisition.clone();
            let recovery_settings = settings.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = recovery_acquisition.resume_interrupted_soulseek(recovery_settings).await {
                    eprintln!("Soulseek job recovery failed: {error}");
                }
            });
            let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
            let candidate_preview = candidate_preview::CandidatePreviewManager::new(app.handle().clone(), format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()));
            let automation = automation::AutomationService::new(app.handle().clone(), store.clone(), acquisition.clone(), artists.clone(), connections.clone(), analysis.clone(), history.clone(), dir.clone());
            automation.set_autotag_manager(autotag.clone());
            let playback = playback::PlaybackService::new(app.handle().clone(), store.clone(), acquisition.clone(), history.clone()).map_err(std::io::Error::other)?;
            let backup_manager = Arc::new(backup_manager::BackupManager::with_playback_and_staging(
                dir.clone(),
                store.clone(),
                history.clone(),
                playback.clone(),
                backup_staging,
            ));
            backup_manager.start_scheduler(app.handle().clone());
            automation.set_playback_service(playback.clone());
            let system_media = match system_media::SystemMediaService::start(app.handle().clone(), playback.clone()) { Ok(service)=>Some(service),Err(error)=>{diagnostics.push(format!("System media controls unavailable: {error}"));None} };
            let local_audio = local_audio::LocalAudioManager::new(store.clone());
            let webview_audio_transport = webview_audio_transport::WebviewAudioTransport::new();
            let webview_audio_transcoder = webview_audio_transcode::WebviewAudioTranscoder::new();
            #[cfg(target_os = "linux")]
            let mpris = match mpris::MprisService::start(app.handle().clone(), automation.clone(), automation.subscribe_playback_state()) {
                Ok(service) => Some(service),
                Err(error) => { diagnostics.push(format!("Desktop media controls unavailable: {error}")); None }
            };
            let router = candidate_preview.router().merge(automation.router()).merge(local_audio.router()).merge(webview_audio_transport.router()).merge(webview_audio_transcoder.router());
            startup_mark("browser_bridge.begin");
            let browser = browser_bridge::BrowserBridge::start(app.handle().clone(), store.clone(), dir.clone(), token, router).map_err(std::io::Error::other)?;
            startup_mark("browser_bridge.end");
            let managed_provider_session = managed_provider_session::ManagedProviderSession::new(app.handle().clone(), browser.clone(), dir);
            {
                let facts_store = store.clone();
                let facts_app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
                    let worker_started = std::time::Instant::now();
                    let Ok(settings) = facts_store.settings() else { return };
                    let select_started = std::time::Instant::now();
                    let assets = facts_store
                        .list_assets_needing_quality_facts(100)
                        .unwrap_or_default();
                    if perf_trace {
                        eprintln!(
                            "[perf] quality_facts.select assets={} elapsed_ms={}",
                            assets.len(),
                            select_started.elapsed().as_millis()
                        );
                    }
                    for (index, asset) in assets.into_iter().enumerate() {
                        let item = index + 1;
                        let path = Path::new(&asset.path);
                        if !path.is_file() {
                            if perf_trace {
                                eprintln!("[perf] quality_facts.item item={} outcome=missing", item);
                            }
                            continue;
                        }
                        let hash_started = std::time::Instant::now();
                        let hash_matches =
                            media::hash_file(path).ok().as_deref() == Some(asset.hash.as_str());
                        if perf_trace {
                            eprintln!(
                                "[perf] quality_facts.hash item={} matches={} elapsed_ms={}",
                                item,
                                hash_matches,
                                hash_started.elapsed().as_millis()
                            );
                        }
                        if !hash_matches {
                            continue;
                        }
                        let inspect_started = std::time::Instant::now();
                        if let Ok(facts) = media::inspect_quality_facts(path, &settings).await {
                            let saved = facts_store
                                .save_asset_quality_facts(&asset.id, &asset.hash, &facts)
                                .is_ok();
                            if saved {
                                let _ = facts_app.emit("asset-facts-updated", &asset.id);
                            }
                            if perf_trace {
                                eprintln!(
                                    "[perf] quality_facts.inspect item={} saved={} elapsed_ms={}",
                                    item,
                                    saved,
                                    inspect_started.elapsed().as_millis()
                                );
                            }
                        } else if perf_trace {
                            eprintln!(
                                "[perf] quality_facts.inspect item={} outcome=error elapsed_ms={}",
                                item,
                                inspect_started.elapsed().as_millis()
                            );
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    if perf_trace {
                        eprintln!(
                            "[perf] quality_facts.total elapsed_ms={}",
                            worker_started.elapsed().as_millis()
                        );
                    }
                });
            }
            let remote_transport: Arc<dyn browser_bridge::ProviderTransport> = managed_provider_session.clone();
            let remote_playlists=Arc::new(remote_playlists::RemotePlaylistService::new(store.clone(),remote_transport));
            { let handle = app.handle().clone(); remote_playlists.set_change_notifier(Arc::new(move || { let _ = handle.emit("automation-library-changed", serde_json::json!({"at":chrono::Utc::now().to_rfc3339()})); })); }
            acquisition.set_remote_playlist_service(remote_playlists.clone());
            discovery.set_remote_service(remote_playlists.clone());
            automation.set_remote_service(remote_playlists.clone());
            startup_mark("automation_endpoint.begin");
            automation.set_base_url(&browser.base_url()).map_err(std::io::Error::other)?;
            startup_mark("automation_endpoint.registered");
            candidate_preview.set_base_url(browser.base_url());
            local_audio.set_base_url(browser.base_url());
            webview_audio_transport.set_base_url(browser.base_url());
            webview_audio_transcoder.set_base_url(browser.base_url());
            let playlist_watcher = match playlist_watch::PlaylistWatcher::start(app.handle().clone(), store.clone(), history.clone()) { Ok(watcher) => Some(watcher), Err(error) => { diagnostics.push(error); None } };
            // Opening the app must not enqueue a bulk file-retag pass. Current
            // playback analysis uses resource-bound caches; explicit tagging
            // and newly acquired-file processing keep their separate actions.
            let startup_diagnostics = Arc::new(std::sync::Mutex::new(diagnostics));
            let playback_resolution = Arc::new(std::sync::Mutex::new(PlaybackResolutionGate::default()));
            let sort_availability_backfill = Arc::new(SortAvailabilityBackfill::new(store.clone(), playback.clone(), playback_resolution.clone(), app.handle().clone()));
            let playback_provider_session_status = std::sync::Mutex::new(managed_provider_session.status());
            source_preparation::schedule_saved(acquisition.clone());
            app.manage(AppState { discovery, autotag, store, _profile_lock: profile_lock, backup_manager, import_drops, directory_browser, automation, playback, _system_media: system_media, #[cfg(target_os = "linux")] _mpris: mpris, local_audio, webview_audio_transport, webview_audio_transcoder, acquisition, diagnostics: startup_diagnostics, browser, managed_provider_session, playback_provider_session_status, remote_playlists, candidate_preview, preview_warmup_cancel:std::sync::Mutex::new(None), playback_resolution, webview_identification: webview_identification::WebViewIdentification::default(), connections, artists, analysis, analysis_selection_jobs: analysis_selection::AnalysisSelectionJobs::new(), stream_analysis_selection_jobs: analysis_stream_selection::SelectedStreamAnalysisJobs::new(), loudness_analysis_jobs: Arc::default(), waveform_analysis_jobs: Arc::default(), history, playlist_watcher: std::sync::Mutex::new(playlist_watcher), provider_reads: Arc::new(provider_read_requests::ProviderReadRequests::default()), sort_availability_backfill, backup_frontend_preference_session_id });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            export_backup_settings,
            preview_backup_restore,
            submit_backup_checkpoint,
            inspect_backup_settings,
            apply_backup_settings,
            list_track_files,set_preferred_track_file,set_remote_playlist_write_sync_enabled,remote_playlist_connection_status,connect_remote_playlist_provider,disconnect_remote_playlist_provider,list_remote_playlists,browse_remote_playlist,browse_remote_artist,browse_remote_artist_tracks,browse_remote_album_tracks,prepare_spotify_recording_crosswalk,search_remote_playlists,cancel_remote_read,repair_spotify_durations_public,cancel_public_spotify_duration_repair,public_spotify_duration_repair_running,list_remote_playlist_bindings,import_remote_playlists,get_remote_playlist_import_job,cancel_remote_playlist_import,refresh_remote_playlist_binding,get_remote_playlist_sync_preview,search_remote_playlist_writeback_candidates,apply_remote_playlist_sync_preview,get_managed_provider_session_status,start_managed_provider_login,start_managed_provider_background,stop_managed_provider_session,confirm_managed_provider_account,create_verification_playlist,create_youtube_handoff_pairing_code,
            inspect_backup, consume_import_drop, create_backup, estimate_backup, get_backup_job, get_active_backup_job, cancel_backup_job, restore_backup, get_restore_work_policy, review_restored_automatic_work, choose_backup_restore_root, get_backup_schedule, set_backup_schedule, choose_backup_schedule_destination, get_backup_frontend_preferences, ack_backup_frontend_preferences, get_backup_playback_restore, ack_backup_playback_restore,
            get_release_setup, complete_release_setup, get_radio_recognition, set_radio_recognition, choose_music_directory,
            get_playback_engine_settings, restart_with_playback_engine, get_playback_snapshot, playback_command, handoff_webview_playback, handoff_resolved_playback_source,
            get_browser_install_status, register_browser_host, unregister_browser_host,
            youtube_support_status,youtube_support_check,youtube_support_update,youtube_support_rollback,youtube_support_diagnose,
            get_radio_stations, rename_radio_station, resolve_radio_station_playback_url, resolve_radio_station_artwork, get_station_details, poll_radio_station, identify_radio_station, get_mpv_status, open_in_mpv,
            search_radio_stations, add_radio_station, add_custom_radio_station, edit_radio_station_url, get_radio_playlist_option, set_radio_playlist_option, list_radio_playlist_targets, remove_radio_station,
            get_radio_metadata,
            get_sidebar_layout,
            save_sidebar_layout,
            list_search_sources, register_chapter_track, search_source, poll_search_source, inspect_youtube_source, enrich_search_result, get_autotag_jobs, get_autotag_settings, save_autotag_settings, retry_autotag, repair_download_source_tags, review_autotag_candidate, apply_autotag_review, apply_recognition_review,
            get_browser_status, get_automation_status, set_automation_permissions, automation_playback_state, automation_playback_complete, automation_queued_playback_accept, automation_queued_playback_update, automation_queued_playback_status, automation_identify_complete, set_automation_frontend_ready, automation_layout_complete, automation_stereo_diagnostic_complete, automation_search_ui_complete, automation_search_controls_complete, automation_player_panel_complete, automation_ui_navigation_complete,
            request_browser_import,
            import_collection_url,
            preview_collection_import,
            apply_collection_import,
            retry_import_failures,
            enrich_import_track,
            set_import_playback_mode,
            list_browser_imports,
            load_browser_import,
            import_browser_snapshot,
            start_candidate_preview,
            candidate_preview_control,
            stop_candidate_preview,
            list_genres,
            get_chart,
            list_snapshots,
            load_snapshot,
            get_chart_memberships,
            get_track_details,
            resolve_preview,
            resolve_playback_track,
            cancel_playback_resolution,
            list_tracks,
            list_favorite_track_ids,
            list_tracks_page,
            list_track_prefix,
            get_tracks,
            identify_playing_track,
            cancel_playing_track_identification,
            identify_webview_track,
            get_playing_track_detections,
            get_saved_track_detections,
            set_playing_track_detection_enabled,
            detect_playing_track_segment,
            identify_webview_playing_pcm,
            identify_webview_playing_output,
            cancel_webview_identification,
            set_favorite,
            set_disposition,
            update_track,
            preview_unsorted_library_entries,
            remove_unsorted_library_entries,
            list_collections,
            list_deleted_collections,
            delete_collection,
            restore_collection,
            create_collection,
            rename_collection,
            remove_from_collection,
            add_to_collection,
            append_playlist_entries,
            import_tracks, preview_import_tracks, pick_import_audio_sources, preview_local_audio_import,
        merge_recordings,
            search_tracks,
            search_audio_tracks,
            save_audio_match,
            find_candidates,
            find_soulseek_album_candidates,
            browse_soulseek_directory,
            prepare_download_match_identity,
            find_download_candidates,
            resolve_candidate_audio,
            warm_track_previews,
            warm_playback_candidate_search,
            prepare_upcoming_playback_source,
            inspect_candidate,
            assess_download_candidate,
            enqueue_download,
            cancel_download,
            retry_download,
            list_jobs,
            get_settings,
            get_live_bpm,
            get_live_playback_estimates,
            estimate_webview_live_bpm,
            get_playback_waveform,
            get_cached_stream_analysis,
            begin_webview_stream_analysis,
            push_webview_stream_analysis,
            finish_webview_stream_analysis,
            cancel_webview_stream_analysis,
            get_playback_diagnostics_status,
            set_playback_diagnostics_enabled,
            clear_playback_diagnostics,
            get_current_track_lyrics, search_current_track_lyric_candidates, select_current_track_lyric_candidate, save_current_track_lyrics_to_file, export_current_track_lyrics_text,
            export_playback_diagnostics,

            get_diagnostics,
            save_settings,
            check_dependencies,
            list_assets,
            get_asset_by_id,
            list_assets_for_paths,
            list_assets_by_ids,
            list_artist_local_assets_page,
            match_artist_playback_sources,
            list_asset_artist_matches_page,
            list_track_files_for_recordings,
            count_assets,
            get_analysis_status_for_assets,
            get_loudness_settings, set_loudness_settings, get_loudness_decision_for_asset,
            set_loudness_album_context,
            start_loudness_analysis, get_loudness_analysis_status, cancel_loudness_analysis,
            start_local_waveform_analysis, get_local_waveform_analysis_status,
            cancel_local_waveform_analysis, get_local_waveform_analysis,
            read_analysis_cache_record, write_analysis_cache_record,
            import_local_file, apply_local_audio_import,
            export_mp3,
            export_playlist, export_mixxx_playlist, resume_playlist_mirror, copy_playlist, move_playlist_entry,
            get_playlist_hierarchy, save_playlist_hierarchy, place_imported_playlist_items, preview_portable_playlist_export,
            render_portable_playlist_export, export_portable_playlist, preview_portable_playlist_import,
            apply_portable_playlist_import, resolve_portable_playback_candidate, preview_playlist_mirror_migration, migrate_playlist_mirror_v1,
            preview_history_playlist_export, render_history_playlist_export, get_history_mirror_options, set_history_mirror_options, preview_restored_playlist_sync, resume_reviewed_restored_playlist_sync, resume_history_playlist_mirror,
            get_playlist_mirror_options, set_playlist_mirror_options,
            open_in_file_manager,
            open_external_url
            ,get_connections, save_connections, test_connections, get_managed_slskd_download_settings, save_managed_slskd_download_settings, get_slskd_shares, rescan_slskd_shares, add_managed_slskd_share, remove_managed_slskd_share, translate_current_track_lyrics,
            start_slskd_search, get_slskd_search, cancel_slskd_search,
            search_artists, search_albums, get_artist_artwork, get_artist_profile, get_artist_releases, get_release_tracks, save_library_tracks,
            get_artist_tracks, get_artist_collaborations, resolve_recording_metadata,
            list_directory_browser_roots, list_directory_browser_children, choose_directory_browser_root, add_directory_browser_root, remove_directory_browser_root, browse_directory_browser, start_directory_browser_sort_availability, get_directory_browser_sort_availability, cancel_directory_browser_sort_availability, start_directory_browser_scan, get_directory_browser_scan, cancel_directory_browser_scan,
            import_onetagger_discogs, list_local_playlists, import_local_playlist,
            analyze_local_assets, get_analysis_status, start_selected_asset_analysis, get_selected_asset_analysis, cancel_selected_asset_analysis, apply_selected_asset_analysis, start_selected_stream_analysis, start_stream_media_analysis, get_cached_stream_media_analysis, get_selected_stream_analysis, cancel_selected_stream_analysis, apply_selected_stream_analysis, scan_local_playlists, sync_playlists, playlist_watcher_running,
            get_radio_song_track,set_playlist_shortcut_binding,get_playlist_preferences,set_playlist_shortcut,set_favorites_playlist,set_favorite_membership,get_radio_history_setting,set_radio_history_setting,record_radio_history,
            record_playback, list_playback_history, search_playback_history, get_track_library_summary, clear_playback_history, inspect_chapters,
            enrich_chapters, cache_artwork, cache_asset_artwork, artwork_display::read_cached_artwork_display, get_song_radio
        ])
        .build(context)
        .expect("Cannot start Burrata")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let state = app.state::<AppState>();
                state.sort_availability_backfill.stop();
                if let Err(error) = tauri::async_runtime::block_on(state.playback.shutdown_identification_for_exit()) { eprintln!("Recognition shutdown: {error}"); }
                if let Err(error) = tauri::async_runtime::block_on(state.webview_identification.shutdown_for_exit()) { eprintln!("WebView recognition shutdown: {error}"); }
                state.managed_provider_session.shutdown();
                if let Ok(mut watcher) = state.playlist_watcher.lock() { watcher.take(); }
                state.browser.shutdown();
                if let Some(manager)=youtube_support::global(){tauri::async_runtime::block_on(manager.shutdown_for_exit()).ok();}
            }
        });
}

#[cfg(test)]
mod webview_media_event_trace_flag_tests {
    use super::webview_media_event_trace_enabled;

    #[test]
    fn event_trace_requires_exact_opt_in_value() {
        assert!(webview_media_event_trace_enabled(Some("1")));
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some(" 1"),
            Some("1 "),
        ] {
            assert!(!webview_media_event_trace_enabled(value));
        }
    }
}

#[cfg(test)]
mod asset_protocol_startup_grant_tests {
    use super::asset_paths_needing_explicit_grants;
    use tauri::Manager;

    #[test]
    fn initial_scope_classification_preserves_exact_tauri_permission_behavior() {
        let app = tauri::test::mock_app();
        let scope = app.asset_protocol_scope();
        let root = tempfile::tempdir().unwrap();
        let in_root = root.path().join("inside.flac");
        let hidden = root.path().join(".hidden.flac");
        std::fs::write(&in_root, b"fixture").unwrap();
        std::fs::write(&hidden, b"fixture").unwrap();
        let external_dir = tempfile::tempdir().unwrap();
        let external = external_dir.path().join("outside.flac");
        std::fs::write(&external, b"fixture").unwrap();
        let missing = external_dir.path().join("later.flac");
        let missing_in_root = root.path().join("later-inside.flac");

        scope.allow_directory(root.path(), true).unwrap();
        // Recursive directory access permits a not-yet-created child path.
        assert!(scope.is_allowed(&missing_in_root));
        std::fs::write(&missing_in_root, b"fixture").unwrap();
        assert!(scope.is_allowed(&missing_in_root));
        let (pending, already_allowed) = asset_paths_needing_explicit_grants(
            &scope,
            vec![
                in_root.to_string_lossy().into_owned(),
                external.to_string_lossy().into_owned(),
                external.to_string_lossy().into_owned(),
                hidden.to_string_lossy().into_owned(),
                missing.to_string_lossy().into_owned(),
            ],
            Some(root.path()),
        );
        // Tauri excludes leading-dot paths by default on Unix, but a leading
        // dot is an ordinary filename on Windows. Preserve each platform's
        // actual scope policy rather than imposing the Unix fixture outcome.
        let hidden_initially_allowed = cfg!(windows);
        assert_eq!(already_allowed, 1 + usize::from(hidden_initially_allowed));
        let mut expected_pending = vec![
            external.to_string_lossy().into_owned(),
            external.to_string_lossy().into_owned(),
        ];
        if !hidden_initially_allowed {
            expected_pending.push(hidden.to_string_lossy().into_owned());
        }
        expected_pending.push(missing.to_string_lossy().into_owned());
        assert_eq!(pending, expected_pending);
        assert!(scope.is_allowed(&in_root));
        assert!(!scope.is_allowed(&external));
        assert_eq!(scope.is_allowed(&hidden), hidden_initially_allowed);
        assert!(!scope.is_allowed(&missing));

        // Explicit grants retain the previous behavior for every initially
        // denied path; no directory pattern is added by this optimization.
        for path in &pending {
            scope.allow_file(path).unwrap();
        }
        assert!(scope.is_allowed(&external));
        assert!(scope.is_allowed(&hidden));
        // Tauri preserves the exact path pattern before the file exists.
        assert!(scope.is_allowed(&missing));
        std::fs::write(&missing, b"fixture").unwrap();
        assert!(scope.is_allowed(&missing));
        assert!(!scope.is_allowed(external_dir.path().join("unrelated.flac")));

        let forbidden = external_dir.path().join("forbidden.flac");
        std::fs::write(&forbidden, b"fixture").unwrap();
        scope.forbid_file(&forbidden).unwrap();
        let (pending, already_allowed) = asset_paths_needing_explicit_grants(
            &scope,
            vec![forbidden.to_string_lossy().into_owned()],
            Some(root.path()),
        );
        assert_eq!(already_allowed, 0);
        assert_eq!(pending, vec![forbidden.to_string_lossy().into_owned()]);
        scope.allow_file(&forbidden).unwrap();
        assert!(!scope.is_allowed(&forbidden));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let broken = root.path().join("broken-link.flac");
            symlink(external_dir.path().join("not-created.flac"), &broken).unwrap();
            let (pending, already_allowed) = asset_paths_needing_explicit_grants(
                &scope,
                vec![broken.to_string_lossy().into_owned()],
                Some(root.path()),
            );
            assert_eq!(already_allowed, 0);
            assert_eq!(pending, vec![broken.to_string_lossy().into_owned()]);
            scope.allow_file(&broken).unwrap();
            assert!(!scope.is_allowed(&broken));
        }
    }

    #[test]
    fn no_music_root_keeps_every_asset_on_explicit_grant_path() {
        let app = tauri::test::mock_app();
        let scope = app.asset_protocol_scope();
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("inside.flac");
        std::fs::write(&existing, b"fixture").unwrap();
        scope.allow_directory(root.path(), true).unwrap();

        // The caller's no-root branch avoids classification entirely; this
        // helper result documents that no root means no reusable grant.
        let (pending, already_allowed) = asset_paths_needing_explicit_grants(
            &scope,
            vec![existing.to_string_lossy().into_owned()],
            None,
        );
        assert_eq!(already_allowed, 0);
        assert_eq!(pending, vec![existing.to_string_lossy().into_owned()]);
    }
}

#[cfg(test)]
#[test]
fn webview_header_policy_never_exposes_names_values_or_urls() {
    let value = webview_header_policy(&[
        ("Authorization".into(), "Bearer private".into()),
        ("x-private-name".into(), "https://private.example".into()),
    ]);
    assert_eq!(value["authorization"], true);
    assert_eq!(value["other"], true);
    assert_eq!(value["headerCount"], 2);
    assert!(!value.to_string().contains("private"));
    assert!(!value.to_string().contains("Bearer"));
}

#[cfg(test)]
mod playback_candidate_prefetch_tests {
    use super::*;
    #[test]
    fn foreground_cancels_only_legacy_browse_warmup_not_independent_queue_work() {
        let (warm_stop, warm_cancel) = tokio::sync::watch::channel(false);
        let (queue_stop, queue_cancel) = tokio::sync::watch::channel(false);
        let slot = std::sync::Mutex::new(Some(warm_stop));
        cancel_preview_warmup(&slot);
        assert!(*warm_cancel.borrow());
        assert!(!*queue_cancel.borrow());
        assert!(slot.lock().unwrap().is_none());
        cancel_preview_warmup(&slot);
        queue_stop.send_replace(true);
        assert!(*queue_cancel.borrow());
    }

    #[test]
    fn upcoming_trace_opt_in_is_exact_and_payload_is_redacted() {
        assert!(playback_trace_is_enabled(Some("1")));
        for value in [None, Some("0"), Some("true"), Some("")] {
            assert!(!playback_trace_is_enabled(value));
        }
        let payload = upcoming_candidate_search_trace_payload(
            "upcoming_candidate_search_end",
            "9c7d5b83-b3a5-46ca-b3da-d5be420d8472",
            "youtube",
            24,
            serde_json::json!({"requestId":"9c7d5b83-b3a5-46ca-b3da-d5be420d8472","ok":true,"resultCount":3,"cancelled":false,"deadline":false}),
        );
        assert_eq!(payload["provider"], "youtube");
        assert_eq!(payload["details"]["resultCount"], 3);
        for forbidden in ["title", "query", "url", "candidateId", "cacheKey"] {
            assert!(payload.get(forbidden).is_none());
            assert!(payload["details"].get(forbidden).is_none());
        }
        assert!(!payload.to_string().contains("Magalenha"));
        assert!(!payload.to_string().contains("qySa2Xtq0kU"));
    }

    #[test]
    fn native_and_direct_local_radio_paths_do_not_start_speculative_search() {
        let track = Track {
            provider: "spotify".into(),
            ..Track::default()
        };
        assert!(playback_candidate_search_needed(false, &track));
        assert!(!playback_candidate_search_needed(true, &track));
        let local = Track {
            local_path: Some("/music/a.flac".into()),
            ..track.clone()
        };
        assert!(!playback_candidate_search_needed(false, &local));
        for provider in ["youtube", "soundcloud"] {
            let direct = Track {
                provider: provider.into(),
                ..track.clone()
            };
            assert!(!playback_candidate_search_needed(false, &direct));
        }
        let radio = Track {
            provider: "radio".into(),
            ..track
        };
        assert!(!playback_candidate_search_needed(false, &radio));
    }
    #[tokio::test]
    async fn settings_elapsed_time_and_early_cancellation_do_not_restart_prefetch_budget() {
        let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(8));
        assert!(check_playback_candidate_search_budget(false, deadline).is_ok());
        assert!(check_playback_candidate_search_budget(true, deadline).is_err());
        let expired = playback_sources::PlaybackDeadline::at(
            tokio::time::Instant::now() - std::time::Duration::from_millis(1),
        );
        assert!(check_playback_candidate_search_budget(false, expired).is_err());
        assert!(check_playback_candidate_search_budget(false, expired)
            .unwrap_err()
            .contains("deadline"));
    }
}

#[cfg(test)]
mod managed_playback_session_invalidation_tests {
    use super::*;
    fn healthy() -> managed_provider_session::ManagedProviderSessionStatus {
        managed_provider_session::ManagedProviderSessionStatus {
            enabled: true,
            running: true,
            mode: "native-ready".into(),
            spotify_account_id: Some("spotify-a".into()),
            youtube_account_id: Some("youtube-a".into()),
            pending_youtube_account_id: None,
            pending_youtube_display_name: None,
            account_revisions: [("spotify".into(), 1), ("youtube".into(), 1)]
                .into_iter()
                .collect(),
            connection_seen: true,
            message: String::new(),
            transport: String::new(),
            credential_storage: "keyring".into(),
            youtube_login_page: None,
            youtube_reconnect_required: false,
        }
    }
    #[test]
    fn healthy_poll_and_idle_transport_do_not_invalidate_sources() {
        let previous = healthy();
        let mut current = previous.clone();
        assert!(changed_managed_playback_providers(&previous, &current).is_empty());
        current.running = false;
        current.connection_seen = false;
        current.mode = "idle".into();
        current.message = "Transport is idle".into();
        assert!(changed_managed_playback_providers(&previous, &current).is_empty());
    }
    #[test]
    fn switch_revision_expiry_and_stop_invalidate_only_affected_accounts() {
        let previous = healthy();
        let mut current = previous.clone();
        current.youtube_account_id = Some("youtube-b".into());
        assert_eq!(
            changed_managed_playback_providers(&previous, &current),
            ["youtube"]
        );
        current = previous.clone();
        current.account_revisions.insert("spotify".into(), 2);
        assert_eq!(
            changed_managed_playback_providers(&previous, &current),
            ["spotify"]
        );
        current = previous.clone();
        current.youtube_reconnect_required = true;
        assert_eq!(
            changed_managed_playback_providers(&previous, &current),
            ["youtube"]
        );
        current = previous.clone();
        current.mode = "stopped".into();
        assert_eq!(
            changed_managed_playback_providers(&previous, &current),
            ["spotify", "youtube"]
        );
        assert_eq!(
            changed_managed_playback_providers(&current, &previous),
            ["spotify", "youtube"]
        );
    }
    #[test]
    fn account_session_fence_cancels_old_resolution_and_preserves_next_generation() {
        let mut gate = PlaybackResolutionGate::default();
        let (_, cancelled) = gate.begin(12).unwrap();
        assert_eq!(gate.cancel_for_provider_session_change(), 13);
        assert!(*cancelled.borrow());
        assert!(!gate.has_current_resolution());
        for _ in 0..5 {
            assert_eq!(gate.cancel_for_provider_session_change(), 13);
        }
        assert_eq!(gate.generation, 12);
        assert!(gate.begin(13).is_ok());
    }
}

#[cfg(test)]
mod playback_request_budget_tests {
    use super::*;
    #[test]
    fn omitted_legacy_setting_keeps_twenty_second_startup_budget() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("playbackStartupTimeoutSeconds");
        let settings: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            playback_request_budget(&settings, None),
            std::time::Duration::from_secs(20)
        );
    }
    #[test]
    fn configured_budget_accepts_remaining_time_and_rejects_client_extensions() {
        for seconds in [20, 30, 45] {
            let settings = Settings {
                playback_startup_timeout_seconds: seconds,
                ..Settings::default()
            };
            assert_eq!(
                playback_request_budget(&settings, None),
                std::time::Duration::from_secs(seconds.into())
            );
            assert_eq!(
                playback_request_budget(&settings, Some(12_345)),
                std::time::Duration::from_millis(12_345)
            );
            assert_eq!(
                playback_request_budget(&settings, Some(0)),
                std::time::Duration::from_millis(1)
            );
            for requested in [60_000, u64::MAX] {
                assert_eq!(
                    playback_request_budget(&settings, Some(requested)),
                    std::time::Duration::from_secs(seconds.into())
                );
            }
        }
    }
    #[test]
    fn invalid_persisted_budget_falls_back_to_bounded_legacy_default() {
        let settings = Settings {
            playback_startup_timeout_seconds: u32::MAX,
            ..Settings::default()
        };
        assert_eq!(
            playback_request_budget(&settings, Some(u64::MAX)),
            std::time::Duration::from_secs(20)
        );
    }
}

#[cfg(test)]
mod seek_recovery_relay_tests {
    use super::*;

    fn fixture() -> (Track, Candidate, String) {
        let source = "https://rr1.googlevideo.com/videoplayback?sig=private".to_owned();
        let track = Track {
            id: "youtube:abcdefghijk".into(),
            provider: "youtube".into(),
            provider_id: "abcdefghijk".into(),
            source_url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Exact video".into(),
            duration_seconds: Some(300.0),
            ..Track::default()
        };
        let candidate = Candidate {
            id: track.id.clone(),
            provider: "youtube".into(),
            url: track.source_url.clone(),
            title: track.title.clone(),
            duration_seconds: track.duration_seconds,
            ..Candidate::default()
        };
        (track, candidate, source)
    }

    #[test]
    fn only_relays_the_exact_remote_youtube_source_during_seek_recovery() {
        let (track, candidate, source) = fixture();
        assert!(should_relay_seek_recovery_source(
            true,
            &track,
            Some(&candidate),
            &source,
            false,
            false,
        ));
        assert!(!should_relay_seek_recovery_source(
            false,
            &track,
            Some(&candidate),
            &source,
            false,
            false,
        ));
        assert!(!should_relay_seek_recovery_source(
            true,
            &track,
            Some(&candidate),
            &source,
            true,
            false,
        ));
        assert!(!should_relay_seek_recovery_source(
            true,
            &track,
            Some(&candidate),
            &source,
            false,
            true,
        ));
        assert!(!should_relay_seek_recovery_source(
            true,
            &track,
            Some(&candidate),
            "https://unrelated.example/audio.mp4",
            false,
            false,
        ));
        let mut wrong_video = candidate;
        wrong_video.id = "youtube:other-video".into();
        assert!(!should_relay_seek_recovery_source(
            true,
            &track,
            Some(&wrong_video),
            &source,
            false,
            false,
        ));
    }
}

#[cfg(test)]
mod candidate_audio_deadline_tests {
    use super::*;
    #[tokio::test]
    async fn spent_budget_rejects_even_immediately_ready_audio_without_resetting_deadline() {
        let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::ZERO);
        let (stop, observer) = tokio::sync::watch::channel(false);
        let result =
            resolve_with_candidate_audio_deadline(deadline, &stop, async { Ok("audio") }).await;
        assert!(result
            .unwrap_err()
            .contains("remaining playback startup budget"));
        assert!(*observer.borrow());
    }
    #[tokio::test]
    async fn timed_out_subscriber_leaves_independent_caller_cancellation_unchanged() {
        let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::ZERO);
        let (stop, observer) = tokio::sync::watch::channel(false);
        let (_other_stop, other_observer) = tokio::sync::watch::channel(false);
        let result = resolve_with_candidate_audio_deadline(
            deadline,
            &stop,
            std::future::pending::<Result<()>>(),
        )
        .await;
        assert!(result
            .unwrap_err()
            .contains("remaining playback startup budget"));
        assert!(*observer.borrow());
        assert!(!*other_observer.borrow());
    }
    #[tokio::test]
    async fn ready_audio_with_remaining_budget_does_not_cancel_its_registration() {
        let deadline = playback_sources::PlaybackDeadline::after(std::time::Duration::from_secs(1));
        let (stop, observer) = tokio::sync::watch::channel(false);
        assert_eq!(
            resolve_with_candidate_audio_deadline(deadline, &stop, async { Ok("audio") })
                .await
                .unwrap(),
            "audio"
        );
        assert!(!*observer.borrow());
    }
}
