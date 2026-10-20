#![recursion_limit = "256"]

// Tauri embeds its manifest in binaries, but not the library unit-test runner.
// Use the same generated resource so Windows selects Common Controls v6 before
// resolving TaskDialogIndirect. This does not affect production library builds.
#[cfg(all(test, target_os = "windows", target_env = "msvc"))]
#[link(name = "resource", kind = "static", modifiers = "-bundle")]
extern "C" {}

pub mod acquisition;
pub mod analysis;
mod artist_alias_discovery;
mod artist_alias_lookup;
mod artist_credit_aliases;
pub mod artists;
mod artwork_display;
mod artwork_protocol_trace;
mod artwork_resource_probe;
pub mod automation;
pub mod autotag;
pub mod bandcamp;
mod bandcamp_fallback;
pub mod browser_bridge;
pub mod candidate_preview;
pub mod collection_sources;
pub mod connections;
mod directory_browser;
pub mod discovery;
pub mod external_player;
pub mod imports;
pub mod install_integration;
#[cfg(target_os = "linux")]
pub mod linux_webview_output;
pub mod local_audio;
mod main_ui_capture;
pub mod managed_provider_session;
pub mod matching;
pub mod media;
pub mod metadata;
pub mod metadata_normalization;
pub mod models;
pub mod monochrome;
#[cfg(target_os = "linux")]
pub mod mpris;
pub mod native_output;
#[cfg(feature = "experimental-opus")]
mod opus_experiment;
pub mod pcm_tap;
pub mod playback;
pub mod playback_failure;
pub mod playback_history;
pub mod playback_radio;
pub mod playback_sources;
mod playing_track_identification;
pub mod playlist_files;
pub mod playlist_preferences;
pub mod playlist_sync;
pub mod playlist_watch;
pub mod provider_read_requests;
pub mod providers;
mod queued_playback;
pub mod radio;
pub mod recording_identity;
pub mod recording_match;
mod recording_match_aliases;
mod recording_match_context;
#[cfg(test)]
mod recording_match_corpus;
pub mod remote_playlists;
mod shutdown_signal;
pub mod sidebar_layout;
pub mod slskd;
pub mod source_fallback;
mod source_preparation;
mod spotify_mb_crosswalk;
pub mod storage;
pub mod system_media;
pub mod tools;
pub mod track_sort_sql;
pub mod track_sort_text;
mod webview_audio_transcode;
mod webview_audio_transport;
pub mod webview_identification;
pub mod webview_radio_pcm;
mod youtube_direct_experiment;
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
                }),
            };
            let convert = webview_audio_transcode::needs_live_ogg_conversion(
                mime.as_deref(),
                codec.as_deref(),
                webview_capabilities.as_ref(),
            );
            if convert || webview_audio_transport::needs_relay(&source) {
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
                true,
                None,
            );
        }
        if detailed.unwrap_or(false) {
            Ok(
                serde_json::json!({"url":url,"audioMimeType":mime,"audioCodec":codec,"sourceTransport":transport}),
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
    tokio::task::spawn_blocking(move || playlist_sync::sync_playlists(&store, &store.settings()?))
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
    use sha2::{Digest, Sha256};
    let stem = format!("{:x}", Sha256::digest(key.as_bytes()));
    for extension in ["jpg", "png", "webp"] {
        let path = directory.join(format!("{stem}.{extension}"));
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn cache_artwork_bytes(app: &tauri::AppHandle, key: &str, bytes: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let extension = media::artwork_extension(bytes)?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("artwork");
    let target = directory.join(format!("{:x}.{extension}", Sha256::digest(key.as_bytes())));
    if !target.is_file() {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        if let Err(error) = std::fs::rename(&temporary, &target) {
            let _ = std::fs::remove_file(&temporary);
            if !target.is_file() {
                return Err(error.to_string());
            }
        }
    }
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

#[derive(Default)]
struct PlaybackResolutionGate {
    generation: u64,
    active: Option<(u64, tokio::sync::watch::Sender<bool>)>,
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
        self.generation = generation;
        let (sender, receiver) = tokio::sync::watch::channel(false);
        self.active = Some((generation, sender.clone()));
        Ok((sender, receiver))
    }

    fn cancel_older_than(&mut self, generation: u64) {
        if generation < self.generation {
            return;
        }
        self.generation = generation;
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
        // Return the relay fence without consuming the frontend's next request.
        self.generation.saturating_add(1)
    }

    fn has_current_resolution(&self) -> bool {
        self.active.is_some()
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
    history: Arc<playback_history::PlaybackHistory>,
    playlist_watcher: std::sync::Mutex<Option<playlist_watch::PlaylistWatcher>>,
    provider_reads: Arc<provider_read_requests::ProviderReadRequests>,
    sort_availability_backfill: Arc<SortAvailabilityBackfill>,
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
) {
    state
        .automation
        .complete_ui_navigation_request(&request_id, &destination, &status);
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
#[tauri::command]
async fn import_collection_url(
    state: State<'_, AppState>,
    url: String,
) -> Result<browser_bridge::BrowserImportResult> {
    let provider = browser_bridge::provider_from_url(&url)?;
    let snapshot = if matches!(provider, "beatport" | "traxsource") {
        let source = providers::provider(provider)?.get_chart(&url).await?;
        let entries = source
            .tracks
            .into_iter()
            .enumerate()
            .map(|(index, t)| browser_bridge::BrowserEntry {
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
        collection_sources::import_source(&url, &state.connections).await?
    };
    let mut chart = browser_bridge::validate_snapshot(&snapshot)?;
    for track in &mut chart.tracks {
        track.provenance["browserImport"] = serde_json::json!(false);
        track.provenance["captureMethod"] = serde_json::json!("direct");
        track.provenance["identity"] = serde_json::json!("unverified source metadata");
    }
    state.store.import_browser_chart(&chart, &snapshot)
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
    let preview = if let Some(path) = &track.local_path {
        if let Some(asset) = state.store.get_asset_by_path(path)? {
            Preview {
                kind: "local".into(),
                url: state.local_audio.url_for(&asset.id)?,
                can_seek: true,
                can_volume: true,
                can_autoplay: true,
                is_excerpt: false,
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
            if let Some(path) = track.local_path.as_deref() {
                let asset = state
                    .store
                    .get_asset_by_path(path)?
                    .ok_or("Local audio is not registered in the library")?;
                let settings = state.store.settings()?;
                Some(crate::native_output::AudioSource {
                    input: asset.path,
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: vec![],
                    decoder_hint: None,
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
    attempt_limit: Option<usize>,
    webview_capabilities: Option<webview_audio_transcode::WebviewAudioCapabilities>,
) -> Result<serde_json::Value> {
    cancel_preview_warmup(&state.preview_warmup_cancel);
    let failed_candidates = failed_candidates.unwrap_or_default();
    playback_sources::validate_failed_candidates(&failed_candidates)?;
    let settings = state.store.settings()?;
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
            )
            .await
        } else {
            let limit = playback_sources::validate_candidate_attempt_limit(attempt_limit)?;
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
            let source = if let Some(path) = track.local_path.as_deref() {
                let asset = state
                    .store
                    .get_asset_by_path(path)?
                    .ok_or("Local audio is not registered in the library")?;
                let settings = state.store.settings()?;
                crate::native_output::AudioSource {
                    input: asset.path,
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: vec![],
                    decoder_hint: None,
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
    let local_source = track.local_path.is_some();
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
    let mut url = if let Some(path) = track.local_path.as_deref() {
        let asset = state
            .store
            .get_asset_by_path(path)?
            .ok_or("Local audio is not registered in the library")?;
        state.local_audio.url_for(&asset.id)?
    } else if convert || webview_audio_transport::needs_relay(&resolved.source) {
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
    let preview = serde_json::json!({
        "kind": if local_source { "local" } else { "audio" },
        "url": url,
        "canSeek": can_seek,
        "canVolume": true,
        "canAutoplay": true,
        "isExcerpt": track.playback_range.is_some(),
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
        "sourceGeneration": request_generation
    });
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
        identify_webview_track_for(&*state, generation, track_id, position).await
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
    identify_webview_track_for(&*state, generation, &track_id, position_seconds).await
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
    let result = state
        .webview_identification
        .identify(generation, track_id, absolute_position)
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
    Ok(result)
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
    state.store.delete_collection(&collection_id)
}
#[tauri::command]
fn restore_collection(state: State<'_, AppState>, collection_id: String) -> Result<Collection> {
    state.store.restore_collection(&collection_id)
}
#[tauri::command]
fn create_collection(state: State<'_, AppState>, name: String) -> Result<Collection> {
    state.store.create_collection(&name)
}
#[tauri::command]
fn rename_collection(
    state: State<'_, AppState>,
    collection_id: String,
    name: String,
) -> Result<Collection> {
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
    budget_ms: Option<u64>,
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
    let mut renderer_url = audio.url.clone();
    let mut hls_transport = false;
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
            }),
        };
        hls_transport = webview_audio_transport::is_hls(&source);
        if webview_audio_transport::needs_relay(&source) {
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
        "title": candidate.title, "durationSeconds": audio.duration_seconds, "audioMimeType":if hls_transport { "application/vnd.apple.mpegurl".to_owned() } else { audio.mime_type }, "audioCodec":audio.codec,"audioBitrateKbps":audio.bitrate_kbps
    });
    if let Some((generation, track_id, stop, cancel)) = registration {
        let mut gate = state
            .playback_resolution
            .lock()
            .map_err(|_| "Playback resolver unavailable")?;
        gate.finish(generation, &stop);
        if gate.generation == generation && !*cancel.borrow() {
            state.webview_identification.register(
                generation,
                track_id,
                crate::native_output::AudioSource {
                    input: audio.url.clone(),
                    ffmpeg: tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                    headers: audio.headers.clone(),
                    decoder_hint: None,
                },
                false,
                None,
            );
            result["sourceGeneration"] = serde_json::json!(generation);
        }
    }
    Ok(result)
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
        "youtube" | "soundcloud" | "monochrome" => provider,
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

// Candidate search only: no inspection, matching promotion, audio URL resolution,
// probing, mapping writes or changes to the foreground playback deadline.
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
    let alternate_artists =
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(&state.store, &track)
            .search_artist_candidates();
    let search = state
        .acquisition
        .warm_playback_candidates_cancelled(
            &track,
            &alternate_artists,
            std::slice::from_ref(&provider),
            &settings,
            cancel,
            deadline,
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
fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    state.store.settings()
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
    state.store.save_settings(&settings)
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
async fn import_local_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Track> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err("Choose an audio file".into());
    }
    let mut t = media::inspect_local_file(&path, &state.store.settings()?).await?;
    let hash = hash_path(&path)?;
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
    let a = media::export_mp3(&a, &state.store.settings()?).await?;
    state.store.save_asset(&a)?;
    media::acknowledge_publication(&state.store.settings()?, &a.id)?;
    Ok(a)
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
    tauri::Builder::default()
        .append_invoke_initialization_script(webview_media_event_trace_script())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let startup_trace = std::env::var("BURRATA_STARTUP_TRACE").is_ok_and(|value| value == "1");
            let startup_started = std::time::Instant::now();
            let startup_mark = |phase: &str| {
                if startup_trace {
                    eprintln!("[startup] phase={} elapsed_ms={}", phase, startup_started.elapsed().as_millis());
                }
            };
            startup_mark("setup.begin");
            app.manage(shutdown_signal::install(app.handle().clone())?);
            let resources=release_resource_root(app.handle()).map_err(std::io::Error::other)?;
            tools::initialize(resources.clone());

            let dir = std::env::var_os("BURRATA_APP_DATA").map(PathBuf::from).unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&dir)?;
            // Asset protocol grants are process-local. Restored playback can already
            // contain asset:// artwork URLs and skip the HTTPS cache command, so
            // regrant only our own flat artwork cache before the frontend mounts.
            app.asset_protocol_scope().allow_directory(
                app.path().app_data_dir()?.join("artwork"),
                false,
            )?;
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
            let history = Arc::new(playback_history::PlaybackHistory::open(&dir.join("playback-history.sqlite")).map_err(std::io::Error::other)?);
            let mut diagnostics=Vec::new();
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
                        } else { store.save_analyzed_asset(&asset, &previous.hash, analysis::VERSION, &payload)?; }
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
            automation.set_playback_service(playback.clone());
            let system_media = if playback.native_enabled() { match system_media::SystemMediaService::start(app.handle().clone(), playback.clone()) { Ok(service)=>Some(service),Err(error)=>{diagnostics.push(format!("System media controls unavailable: {error}"));None} } } else { None };
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
            let playlist_watcher = match playlist_watch::PlaylistWatcher::start(app.handle().clone(), store.clone()) { Ok(watcher) => Some(watcher), Err(error) => { diagnostics.push(error); None } };
            let startup_analysis = analysis.clone();
            let startup_diagnostics = Arc::new(std::sync::Mutex::new(diagnostics));
            let analysis_diagnostics = startup_diagnostics.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = startup_analysis.queue(None, false) {
                    if let Ok(mut diagnostics) = analysis_diagnostics.lock() {
                        diagnostics.push(format!("Analysis preparation failed: {error}"));
                    }
                }
            });
            let playback_resolution = Arc::new(std::sync::Mutex::new(PlaybackResolutionGate::default()));
            let sort_availability_backfill = Arc::new(SortAvailabilityBackfill::new(store.clone(), playback.clone(), playback_resolution.clone(), app.handle().clone()));
            let playback_provider_session_status = std::sync::Mutex::new(managed_provider_session.status());
            source_preparation::schedule_saved(acquisition.clone());
            app.manage(AppState { discovery, autotag, store, directory_browser, automation, playback, _system_media: system_media, #[cfg(target_os = "linux")] _mpris: mpris, local_audio, webview_audio_transport, webview_audio_transcoder, acquisition, diagnostics: startup_diagnostics, browser, managed_provider_session, playback_provider_session_status, remote_playlists, candidate_preview, preview_warmup_cancel:std::sync::Mutex::new(None), playback_resolution, webview_identification: webview_identification::WebViewIdentification::default(), connections, artists, analysis, history, playlist_watcher: std::sync::Mutex::new(playlist_watcher), provider_reads: Arc::new(provider_read_requests::ProviderReadRequests::default()), sort_availability_backfill });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_track_files,set_preferred_track_file,set_remote_playlist_write_sync_enabled,remote_playlist_connection_status,connect_remote_playlist_provider,disconnect_remote_playlist_provider,list_remote_playlists,browse_remote_playlist,browse_remote_artist,browse_remote_artist_tracks,browse_remote_album_tracks,prepare_spotify_recording_crosswalk,search_remote_playlists,cancel_remote_read,repair_spotify_durations_public,cancel_public_spotify_duration_repair,public_spotify_duration_repair_running,list_remote_playlist_bindings,import_remote_playlists,get_remote_playlist_import_job,cancel_remote_playlist_import,refresh_remote_playlist_binding,get_remote_playlist_sync_preview,search_remote_playlist_writeback_candidates,apply_remote_playlist_sync_preview,get_managed_provider_session_status,start_managed_provider_login,start_managed_provider_background,stop_managed_provider_session,confirm_managed_provider_account,create_verification_playlist,create_youtube_handoff_pairing_code,
            get_release_setup, complete_release_setup, get_radio_recognition, set_radio_recognition, choose_music_directory,
            get_playback_snapshot, playback_command,
            get_browser_install_status, register_browser_host, unregister_browser_host,
            youtube_support_status,youtube_support_check,youtube_support_update,youtube_support_rollback,youtube_support_diagnose,
            get_radio_stations, rename_radio_station, resolve_radio_station_playback_url, resolve_radio_station_artwork, get_station_details, poll_radio_station, identify_radio_station, get_mpv_status, open_in_mpv,
            search_radio_stations, add_radio_station, add_custom_radio_station, remove_radio_station,
            get_radio_metadata,
            get_sidebar_layout,
            save_sidebar_layout,
            list_search_sources, register_chapter_track, search_source, poll_search_source, inspect_youtube_source, enrich_search_result, get_autotag_jobs, get_autotag_settings, save_autotag_settings, retry_autotag, review_autotag_candidate, apply_autotag_review,
            get_browser_status, get_automation_status, set_automation_permissions, automation_playback_state, automation_playback_complete, automation_queued_playback_accept, automation_queued_playback_update, automation_queued_playback_status, automation_identify_complete, set_automation_frontend_ready, automation_layout_complete, automation_search_ui_complete, automation_search_controls_complete, automation_player_panel_complete, automation_ui_navigation_complete,
            request_browser_import,
            import_collection_url,
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
            identify_webview_playing_pcm,
            identify_webview_playing_output,
            cancel_webview_identification,
            set_favorite,
            set_disposition,
            update_track,
            list_collections,
            list_deleted_collections,
            delete_collection,
            restore_collection,
            create_collection,
            rename_collection,
            remove_from_collection,
            add_to_collection,
            append_playlist_entries,
            import_tracks,
        merge_recordings,
            search_tracks,
            search_audio_tracks,
            save_audio_match,
            find_candidates,
            resolve_candidate_audio,
            warm_track_previews,
            warm_playback_candidate_search,
            inspect_candidate,
            enqueue_download,
            cancel_download,
            retry_download,
            list_jobs,
            get_settings,
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
            import_local_file,
            export_mp3,
            export_playlist, export_mixxx_playlist, resume_playlist_mirror, copy_playlist, move_playlist_entry,
            open_in_file_manager,
            open_external_url
            ,get_connections, save_connections, test_connections, get_slskd_shares, rescan_slskd_shares, add_managed_slskd_share, remove_managed_slskd_share,
            start_slskd_search, get_slskd_search, cancel_slskd_search,
            search_artists, search_albums, get_artist_profile, get_artist_releases, get_release_tracks, save_library_tracks,
            get_artist_tracks, get_artist_collaborations, resolve_recording_metadata,
            list_directory_browser_roots, list_directory_browser_children, choose_directory_browser_root, add_directory_browser_root, remove_directory_browser_root, browse_directory_browser, start_directory_browser_sort_availability, get_directory_browser_sort_availability, cancel_directory_browser_sort_availability, start_directory_browser_scan, get_directory_browser_scan, cancel_directory_browser_scan,
            import_onetagger_discogs, list_local_playlists, import_local_playlist,
            analyze_local_assets, get_analysis_status, scan_local_playlists, sync_playlists, playlist_watcher_running,
            get_radio_song_track,set_playlist_shortcut_binding,get_playlist_preferences,set_playlist_shortcut,set_favorites_playlist,set_favorite_membership,get_radio_history_setting,set_radio_history_setting,record_radio_history,
            record_playback, list_playback_history, search_playback_history, clear_playback_history, inspect_chapters,
            enrich_chapters, cache_artwork, cache_asset_artwork, artwork_display::read_cached_artwork_display, get_song_radio
        ])
        .build(tauri::generate_context!())
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
