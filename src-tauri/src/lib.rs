pub mod acquisition;
pub mod analysis;
pub mod artists;
pub mod automation;
pub mod autotag;
pub mod browser_bridge;
pub mod candidate_preview;
pub mod collection_sources;
pub mod connections;
pub mod discovery;
pub mod imports;
pub mod install_integration;
pub mod local_audio;
pub mod matching;
pub mod media;
pub mod metadata;
pub mod metadata_normalization;
pub mod models;
pub mod monochrome;
#[cfg(target_os = "linux")]
pub mod mpris;
pub mod playback_history;
pub mod playlist_files;
pub mod playlist_sync;
pub mod playlist_watch;
pub mod providers;
pub mod radio;
pub mod sidebar_layout;
pub mod slskd;
pub mod source_fallback;
pub mod storage;
pub mod tools;
pub mod youtube_music;
pub mod youtube_support;
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
    radio::poll_radio_station(station_id, recognize && enabled, &state.store).await
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
fn list_local_playlists(
    state: State<'_, AppState>,
) -> Result<Vec<playlist_files::LocalPlaylistFile>> {
    playlist_files::list_local_playlists(&state.store.settings()?)
}
#[tauri::command]
fn scan_local_playlists(state: State<'_, AppState>) -> Result<playlist_files::LocalPlaylistScan> {
    playlist_files::scan_local_playlists(&state.store.settings()?)
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
    let existing = state.store.list_assets()?;
    for file in playlist_files::referenced_local_files(&settings, &path)? {
        app.asset_protocol_scope()
            .allow_file(&file)
            .map_err(|e| e.to_string())?;
        if existing.iter().any(|a| Path::new(&a.path) == file) {
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
fn record_playback(
    state: State<'_, AppState>,
    session_id: String,
    provider: String,
    track: Track,
    chapter: Option<serde_json::Value>,
) -> Result<bool> {
    state
        .history
        .record(&session_id, &provider, &track, chapter)
}
#[tauri::command]
fn list_playback_history(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<playback_history::PlaybackHistoryEntry>> {
    state
        .history
        .list(limit.unwrap_or(100), offset.unwrap_or(0))
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
        .list_assets()?
        .into_iter()
        .find(|asset| asset.id == asset_id)
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
        .map(|track| state.store.save_track(track))
        .collect()
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

struct AppState {
    discovery: Arc<discovery::DiscoveryService>,
    autotag: Arc<autotag::AutoTagger>,
    store: Arc<Store>,
    automation: Arc<automation::AutomationService>,
    #[cfg(target_os = "linux")]
    _mpris: Option<mpris::MprisService>,
    local_audio: Arc<local_audio::LocalAudioManager>,
    acquisition: Arc<AcquisitionManager>,
    diagnostics: Vec<String>,
    browser: Arc<browser_bridge::BrowserBridge>,
    candidate_preview: Arc<candidate_preview::CandidatePreviewManager>,
    preview_warmup: tokio::sync::Semaphore,
    preview_warmup_cancel: std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    connections: Arc<connections::ConnectionManager>,
    artists: Arc<artists::ArtistService>,
    analysis: Arc<analysis::AnalysisManager>,
    history: Arc<playback_history::PlaybackHistory>,
    playlist_watcher: std::sync::Mutex<Option<playlist_watch::PlaylistWatcher>>,
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
) -> Result<discovery::DiscoveryPage> {
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
            result.track = Some(state.store.save_track(track)?);
        }
    }
    Ok(page)
}
#[tauri::command]
fn list_search_sources(state: State<'_, AppState>) -> Result<Vec<discovery::SourceCapability>> {
    discovery::source_registry(&state.connections)
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
    state.store.save_track(&parent)?;
    state.store.save_track(&track)
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
    state.store.save_track(&track)
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
fn automation_playback_state(state: State<'_, AppState>, playback: serde_json::Value) {
    state.automation.update_playback_state(playback);
}
#[tauri::command]
fn automation_playback_complete(
    state: State<'_, AppState>,
    request_id: String,
    playback: serde_json::Value,
    error: Option<String>,
) {
    state.automation.update_playback_state(playback.clone());
    state
        .automation
        .complete_playback_request(&request_id, playback, error);
}
#[tauri::command]
fn get_browser_status(state: State<'_, AppState>) -> browser_bridge::BrowserStatus {
    state.browser.status()
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
    if matches!(track.provider.as_str(), "local" | "import" | "spotify")
        || track
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
    state.store.save_track(&details)
}
#[tauri::command]
async fn resolve_preview(state: State<'_, AppState>, track: Track) -> Result<Preview> {
    if let Some(path) = &track.local_path {
        if let Some(asset) = state.store.get_asset_by_path(path)? {
            return Ok(Preview {
                kind: "local".into(),
                url: state.local_audio.url_for(&asset.id)?,
                can_seek: true,
                can_volume: true,
                can_autoplay: true,
                is_excerpt: false,
            });
        }
    }
    // Playback must not wait on a storefront metadata request. Missing previews
    // are resolved by the player's YouTube/SoundCloud discovery flow instead.
    Ok(metadata_preview(&track))
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
fn set_favorite(state: State<'_, AppState>, track: Track, value: bool) -> Result<()> {
    state.store.set_favorite(&track, value)
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
        .list_assets()?
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
    Ok(target_id)
}
#[tauri::command]
fn add_to_collection(
    state: State<'_, AppState>,
    collection_id: String,
    tracks: Vec<Track>,
) -> Result<()> {
    state.store.add_to_collection(
        &playlist_sync::ensure_editable_collection(&state.store, &collection_id)?,
        &tracks,
    )
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
                        result.push(state.store.save_track(&t)?);
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
        if let Ok(mut previous) = state.preview_warmup_cancel.lock() {
            if let Some(cancel) = previous.take() {
                let _ = cancel.send(true);
            }
        }
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
#[tauri::command]
async fn resolve_candidate_audio(
    state: State<'_, AppState>,
    candidate: Candidate,
) -> Result<serde_json::Value> {
    let audio = state
        .acquisition
        .resolve_preview_audio(&candidate, &state.store.settings()?)
        .await?;
    Ok(serde_json::json!({
        "kind": "audio", "url": audio.url, "canSeek": true, "canVolume": true,
        "canAutoplay": true, "isExcerpt": false, "provider": candidate.provider,
        "title": candidate.title, "durationSeconds": audio.duration_seconds, "audioCodec":audio.codec,"audioBitrateKbps":audio.bitrate_kbps
    }))
}

#[tauri::command]
async fn warm_track_previews(state: State<'_, AppState>, tracks: Vec<Track>) -> Result<()> {
    use futures_util::{stream, StreamExt};
    let (sender, cancel) = tokio::sync::watch::channel(false);
    if let Ok(mut previous) = state.preview_warmup_cancel.lock() {
        if let Some(old) = previous.replace(sender) {
            let _ = old.send(true);
        }
    }
    let _permit = state
        .preview_warmup
        .acquire()
        .await
        .map_err(|_| "Preview preparation stopped")?;
    if *cancel.borrow() {
        return Err("Preview preparation superseded".into());
    }
    let settings = state.store.settings()?;
    let acquisition = state.acquisition.clone();
    stream::iter(tracks.into_iter().take(3))
        .for_each_concurrent(2, |track| {
            let settings = &settings;
            let acquisition = &acquisition;
            let cancel = cancel.clone();
            async move {
                if *cancel.borrow()
                    || track.local_path.is_some()
                    || track
                        .preview_url
                        .as_ref()
                        .is_some_and(|url| acquisition::validate_source_url(url, "direct").is_ok())
                {
                    return;
                }
                for provider in ["youtube", "soundcloud"] {
                    if *cancel.borrow() {
                        return;
                    }
                    let Ok(candidates) = acquisition
                        .prefetch_preview_candidates_cancelled(
                            &track,
                            &[provider.into()],
                            settings,
                            cancel.clone(),
                        )
                        .await
                    else {
                        continue;
                    };
                    let candidate = candidates
                        .into_iter()
                        .filter(|c| c.confidence != "low" && c.score.is_finite() && c.score >= 0.55)
                        .max_by(|a, b| a.score.total_cmp(&b.score));
                    if let Some(candidate) = candidate {
                        if acquisition
                            .resolve_preview_audio_cancelled(&candidate, settings, cancel.clone())
                            .await
                            .is_ok()
                        {
                            break;
                        }
                    }
                }
            }
        })
        .await;
    if *cancel.borrow() {
        Err("Preview preparation superseded".into())
    } else {
        Ok(())
    }
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
    state.diagnostics.clone()
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
    state.store.save_settings(&settings)
}
#[tauri::command]
async fn get_song_radio(
    state: State<'_, AppState>,
    track: Track,
    candidate: Candidate,
    limit: Option<usize>,
) -> Result<radio::SongRadioResult> {
    let mut result = radio::start_song_radio(
        &track,
        &candidate,
        &state.store.settings()?,
        limit.unwrap_or(25),
    )
    .await?;
    result.tracks = result
        .tracks
        .iter()
        .map(|track| state.store.save_track(track))
        .collect::<Result<Vec<_>>>()?;
    Ok(result)
}
#[tauri::command]
async fn check_dependencies(state: State<'_, AppState>) -> Result<Vec<Dependency>> {
    Ok(acquisition::check_dependencies(&state.store.settings()?).await)
}
#[tauri::command]
fn list_assets(state: State<'_, AppState>) -> Result<Vec<LocalAsset>> {
    state.store.list_assets()
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
        .list_assets()?
        .into_iter()
        .find(|a| a.id == asset_id)
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
    let registered = state.store.list_assets()?.iter().any(|a| a.path == path)
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let resources=release_resource_root(app.handle()).map_err(std::io::Error::other)?;
            tools::initialize(resources.clone());

            let dir = std::env::var_os("BURRATA_APP_DATA").map(PathBuf::from).unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&dir)?;
            let yt=tools::bundled_executable("yt-dlp").unwrap_or_else(||tools::executable("system:yt-dlp","yt-dlp"));
            let node=tools::bundled_executable("node").or_else(||Some(tools::executable("system:node","node")));
            let helper=resources.join("youtube-support/bgutil/server/build/main.js");
            let plugin=resources.join("youtube-support/bgutil/plugin");
            let manager=youtube_support::YouTubeSupportManager::new(dir.join("youtube-support"),yt,node,helper.is_file().then_some(helper),plugin.is_dir().then_some(plugin));
            youtube_support::initialize_global(manager.clone()).map_err(std::io::Error::other)?;
            tauri::async_runtime::spawn(async move{loop{tokio::time::sleep(std::time::Duration::from_secs(30)).await;let _=manager.stop_helper_if_idle().await;}});

            let store =
                Arc::new(Store::open(&dir.join("burrata.sqlite")).map_err(std::io::Error::other)?);
            store.ensure_downloads_collection().map_err(std::io::Error::other)?;
            store.backfill_managed_downloads().map_err(std::io::Error::other)?;
            #[cfg(target_os = "linux")]
            if let Some(window) = app.get_webview_window("main") {
                candidate_preview::enable_candidate_autoplay(&window).map_err(std::io::Error::other)?;
            }
            let settings = store.settings().map_err(std::io::Error::other)?;
            let history = Arc::new(playback_history::PlaybackHistory::open(&dir.join("playback-history.sqlite")).map_err(std::io::Error::other)?);
            let mut diagnostics=Vec::new();
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
                        let previous = store.list_assets()?.into_iter().find(|saved| saved.id == asset.id).ok_or("Retag recovery asset is missing from the library")?;
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
            if !settings.music_directory.is_empty() {
                app.asset_protocol_scope()
                    .allow_directory(PathBuf::from(&settings.music_directory), true)?;
            }
            for a in store.list_assets().map_err(std::io::Error::other)? {
                app.asset_protocol_scope().allow_file(&a.path)?;
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
            if let Err(error) = analysis.queue(None, false) { diagnostics.push(format!("Analysis preparation failed: {error}")); }
            let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
            let candidate_preview = candidate_preview::CandidatePreviewManager::new(app.handle().clone(), format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()));
            let automation = automation::AutomationService::new(app.handle().clone(), store.clone(), acquisition.clone(), artists.clone(), connections.clone(), analysis.clone(), dir.clone());
            automation.set_autotag_manager(autotag.clone());
            let local_audio = local_audio::LocalAudioManager::new(store.clone());
            #[cfg(target_os = "linux")]
            let mpris = match mpris::MprisService::start(app.handle().clone(), automation.clone(), automation.subscribe_playback_state()) {
                Ok(service) => Some(service),
                Err(error) => { diagnostics.push(format!("Desktop media controls unavailable: {error}")); None }
            };
            let router = candidate_preview.router().merge(automation.router()).merge(local_audio.router());
            let browser = browser_bridge::BrowserBridge::start(app.handle().clone(), store.clone(), dir, token, router).map_err(std::io::Error::other)?;
            automation.set_base_url(&browser.base_url()).map_err(std::io::Error::other)?;
            candidate_preview.set_base_url(browser.base_url());
            local_audio.set_base_url(browser.base_url());
            let playlist_watcher = match playlist_watch::PlaylistWatcher::start(app.handle().clone(), store.clone()) { Ok(watcher) => Some(watcher), Err(error) => { diagnostics.push(error); None } };
            app.manage(AppState { discovery, autotag, store, automation, #[cfg(target_os = "linux")] _mpris: mpris, local_audio, acquisition, diagnostics, browser, candidate_preview, preview_warmup: tokio::sync::Semaphore::new(1), preview_warmup_cancel:std::sync::Mutex::new(None), connections, artists, analysis, history, playlist_watcher: std::sync::Mutex::new(playlist_watcher) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_release_setup, complete_release_setup, set_radio_recognition, choose_music_directory,
            get_browser_install_status, register_browser_host, unregister_browser_host,
            youtube_support_status,youtube_support_check,youtube_support_update,youtube_support_rollback,youtube_support_diagnose,
            get_radio_stations, rename_radio_station, get_station_details, poll_radio_station,
            search_radio_stations, add_radio_station, add_custom_radio_station, remove_radio_station,
            get_radio_metadata,
            get_sidebar_layout,
            save_sidebar_layout,
            list_search_sources, register_chapter_track, search_source, poll_search_source, inspect_youtube_source, enrich_search_result, get_autotag_jobs, get_autotag_settings, save_autotag_settings, retry_autotag, review_autotag_candidate, apply_autotag_review,
            get_browser_status, get_automation_status, set_automation_permissions, automation_playback_state, automation_playback_complete,
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
            list_tracks,
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
            import_tracks,
        merge_recordings,
            search_tracks,
            search_audio_tracks,
            save_audio_match,
            find_candidates,
            resolve_candidate_audio,
            warm_track_previews,
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
            import_local_file,
            export_mp3,
            export_playlist, export_mixxx_playlist, resume_playlist_mirror, copy_playlist, move_playlist_entry,
            open_in_file_manager,
            open_external_url
            ,get_connections, save_connections, test_connections, get_slskd_shares, rescan_slskd_shares, add_managed_slskd_share, remove_managed_slskd_share,
            start_slskd_search, get_slskd_search, cancel_slskd_search,
            search_artists, search_albums, get_artist_profile, get_artist_releases, get_release_tracks,
            get_artist_tracks, get_artist_collaborations, resolve_recording_metadata,
            import_onetagger_discogs, list_local_playlists, import_local_playlist,
            analyze_local_assets, get_analysis_status, scan_local_playlists, sync_playlists,
            record_playback, list_playback_history, clear_playback_history, inspect_chapters,
            enrich_chapters, cache_artwork, cache_asset_artwork, get_song_radio
        ])
        .build(tauri::generate_context!())
        .expect("Cannot start Burrata")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let state = app.state::<AppState>();
                if let Ok(mut watcher) = state.playlist_watcher.lock() { watcher.take(); }
                state.browser.shutdown();
                if let Some(manager)=youtube_support::global(){tauri::async_runtime::block_on(manager.shutdown()).ok();}
            }
        });
}
