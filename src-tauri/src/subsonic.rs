//! Burrata's read-only, account-scoped OpenSubsonic library and audio bridge.
use crate::{
    models::{Settings, Track},
    native_output::{AudioSource, AudioSourceDecodePolicy, AudioSourceDecoderHint},
    playback_sources::{ResolvedSource, ResolvedSourceOrigin},
    storage::Store,
    subsonic_cache,
    subsonic_client::{canonical_endpoint, canonical_username, Client, Song},
};
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode},
    routing::get,
    Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use futures_util::{stream, StreamExt, TryStreamExt};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const PREF: &str = "subsonic.connection.v1";
const KEYRING_SERVICE: &str = "org.burrata.discovery.subsonic";
const ALBUM_PAGE: usize = 250;
const SONG_PAGE: usize = 250;
const MAX_ALBUMS: usize = 40_000;
const AUDIO_LEASE_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_LEASES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    Subsonic,
    Jellyfin,
}

impl Protocol {
    fn provider(self) -> &'static str {
        match self {
            Self::Subsonic => "subsonic",
            Self::Jellyfin => "jellyfin",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Subsonic => "Subsonic",
            Self::Jellyfin => "Jellyfin",
        }
    }
    fn preference(self) -> &'static str {
        match self {
            Self::Subsonic => PREF,
            Self::Jellyfin => "jellyfin.connection.v1",
        }
    }
    fn keyring_service(self) -> &'static str {
        match self {
            Self::Subsonic => KEYRING_SERVICE,
            Self::Jellyfin => "org.burrata.discovery.jellyfin",
        }
    }
    fn bridge_route(self) -> &'static str {
        match self {
            Self::Subsonic => "subsonic-audio",
            Self::Jellyfin => "jellyfin-audio",
        }
    }
}

#[derive(Clone)]
enum ServerClient {
    Subsonic(Client),
    Jellyfin(crate::jellyfin_client::Client),
}

impl ServerClient {
    async fn connect(
        protocol: Protocol,
        endpoint: &str,
        username: &str,
        password: String,
        profile: &str,
    ) -> Result<Self, String> {
        match protocol {
            Protocol::Subsonic => Client::connect(endpoint, username, password)
                .await
                .map(Self::Subsonic),
            Protocol::Jellyfin => crate::jellyfin_client::Client::connect_with_device_id(
                endpoint, username, password, profile,
            )
            .await
            .map(Self::Jellyfin),
        }
    }
    async fn song_catalog_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Song>, usize), String> {
        match self {
            Self::Subsonic(c) => c.song_catalog_page(offset, limit).await,
            Self::Jellyfin(c) => c.song_catalog_page(offset, limit).await,
        }
    }
    async fn song(&self, id: &str) -> Result<Song, String> {
        match self {
            Self::Subsonic(c) => c.song(id).await,
            Self::Jellyfin(c) => c.song(id).await,
        }
    }
    async fn album_cover_id(&self, id: &str) -> Result<Option<String>, String> {
        match self {
            Self::Subsonic(c) => c.album_cover_id(id).await,
            Self::Jellyfin(c) => c.album_cover_id(id).await,
        }
    }
    async fn cover_art(&self, id: &str) -> Result<(&'static str, Vec<u8>), String> {
        match self {
            Self::Subsonic(c) => c.cover_art(id).await,
            Self::Jellyfin(c) => c.cover_art(id).await,
        }
    }
    async fn stream(
        &self,
        id: &str,
        range: Option<HeaderValue>,
    ) -> Result<reqwest::Response, String> {
        match self {
            Self::Subsonic(c) => c.stream(id, range).await,
            Self::Jellyfin(c) => c.stream(id, range).await,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Configuration {
    endpoint: String,
    username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authenticated_account: Option<String>,
    visible: bool,
    allow_fallback: bool,
}

impl Configuration {
    fn account(&self) -> String {
        scope_hash(
            self.endpoint.as_bytes(),
            self.authenticated_account
                .as_deref()
                .unwrap_or(&self.username)
                .as_bytes(),
        )
    }
}

fn read_configuration(store: &Store, protocol: Protocol) -> Result<Option<Configuration>, String> {
    store
        .get_json_preference(protocol.preference())?
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|_| format!("Invalid {} preferences", protocol.label()))
        })
        .transpose()
}

pub(crate) fn suppress_automatic_login(store: &Store) -> Result<(), String> {
    store.set_json_preference(PREF, &serde_json::Value::Null)
}

/// Library restores retain the existing device connection. The backup itself
/// contains only saved track references and never carries a connection.
pub(crate) fn preserve_current_connection(current: &Store, staged: &Store) -> Result<(), String> {
    for protocol in [Protocol::Subsonic, Protocol::Jellyfin] {
        if current
            .get_json_preference(protocol.preference())?
            .is_some()
        {
            staged.set_json_preference(
                protocol.preference(),
                &json!(read_configuration(current, protocol)?),
            )?;
        }
    }
    Ok(())
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    endpoint: Option<String>,
    username: Option<String>,
    visible: bool,
    allow_fallback: bool,
    track_count: usize,
    can_seed: bool,
    strawberry_found: bool,
    refresh_after_login: bool,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    tracks: Vec<Track>,
    total: usize,
    snapshot_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "trackId", rename_all = "camelCase")]
pub(crate) enum BrowseScope {
    Artist(String),
    Album(String),
}

#[derive(Clone)]
struct Lease {
    issued: Instant,
    account: String,
    id: String,
}

pub(crate) struct Service {
    store: Arc<Store>,
    protocol: Protocol,
    cache_path: PathBuf,
    profile: String,
    cache: OnceLock<Mutex<Connection>>,
    configured_client: tokio::sync::Mutex<Option<(String, ServerClient)>>,
    automatic_login_attempted: AtomicBool,
    updates: tokio::sync::Mutex<()>,
    bridge_base: OnceLock<String>,
    leases: Mutex<HashMap<String, Lease>>,
}

impl Service {
    pub(crate) fn new(store: Arc<Store>, profile_dir: &Path) -> Arc<Self> {
        Self::new_for(store, profile_dir, Protocol::Subsonic)
    }

    pub(crate) fn new_for(store: Arc<Store>, profile_dir: &Path, protocol: Protocol) -> Arc<Self> {
        Arc::new(Self {
            store,
            protocol,
            cache_path: profile_dir.join(protocol.provider()).join("catalog.sqlite"),
            profile: profile_scope(profile_dir),
            cache: OnceLock::new(),
            configured_client: tokio::sync::Mutex::new(None),
            automatic_login_attempted: AtomicBool::new(false),
            updates: tokio::sync::Mutex::new(()),
            bridge_base: OnceLock::new(),
            leases: Mutex::default(),
        })
    }

    pub(crate) fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(
                &format!("/{}/{{token}}", self.protocol.bridge_route()),
                get(audio).head(audio),
            )
            .with_state(self.clone())
    }

    pub(crate) fn set_base_url(&self, url: String) {
        let _ = self.bridge_base.set(url.trim_end_matches('/').to_owned());
    }

    fn cache(&self) -> Result<&Mutex<Connection>, String> {
        if let Some(cache) = self.cache.get() {
            return Ok(cache);
        }
        let dir = self
            .cache_path
            .parent()
            .ok_or("Invalid music server cache path")?;
        std::fs::create_dir_all(dir)
            .map_err(|_| "Could not prepare the music server cache directory")?;
        let connection = subsonic_cache::open(&self.cache_path)?;
        Ok(self.cache.get_or_init(|| Mutex::new(connection)))
    }

    fn with_cache<T>(
        &self,
        read: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut conn = self
            .cache()?
            .lock()
            .map_err(|_| format!("{} catalog unavailable", self.protocol.label()))?;
        read(&mut conn)
    }

    fn configuration(&self) -> Result<Option<Configuration>, String> {
        read_configuration(&self.store, self.protocol)
    }

    fn account_config(&self) -> Result<Configuration, String> {
        self.configuration()?.ok_or_else(|| {
            format!(
                "Connect a {} server in Settings first",
                self.protocol.label()
            )
        })
    }

    fn keyring_id(&self, account: &str) -> String {
        format!("{}-{account}", self.profile)
    }

    async fn client(&self, expected: &Configuration) -> Result<ServerClient, String> {
        let mut slot = self.configured_client.lock().await;
        let account = expected.account();
        self.check_current_account(&account)?;
        if let Some((active, client)) = slot.as_ref() {
            if active == &account {
                return Ok(client.clone());
            }
        }
        let key = self.keyring_id(&account);
        let protocol = self.protocol;
        let password = tokio::task::spawn_blocking(move || {
            keyring::Entry::new(protocol.keyring_service(), &key)
                .and_then(|entry| entry.get_password())
                .map_err(|_| {
                    format!(
                        "{} password unavailable in the system keyring",
                        protocol.label()
                    )
                })
        })
        .await
        .map_err(|_| format!("Could not read the {} password", protocol.label()))??;
        let client = ServerClient::connect(
            protocol,
            &expected.endpoint,
            &expected.username,
            password,
            &self.profile,
        )
        .await?;
        if let ServerClient::Jellyfin(jellyfin) = &client {
            if expected.authenticated_account.as_deref() != Some(jellyfin.account_identity()) {
                return Err("The Jellyfin account changed. Reconnect it in Settings.".into());
            }
        }
        self.check_current_account(&account)?;
        *slot = Some((account, client.clone()));
        Ok(client)
    }

    fn check_current_account(&self, expected: &str) -> Result<(), String> {
        if self
            .configuration()?
            .is_some_and(|config| config.account() == expected)
        {
            Ok(())
        } else {
            Err(format!("The {} connection changed", self.protocol.label()))
        }
    }

    pub(crate) async fn status_with_existing_login(self: &Arc<Self>) -> Result<Status, String> {
        // A saved connection or explicit disconnect (JSON null) always wins.
        if self.protocol == Protocol::Jellyfin
            || self
                .store
                .get_json_preference(self.protocol.preference())?
                .is_some()
            || self.automatic_login_attempted.swap(true, Ordering::AcqRel)
        {
            if let Ok(_update) = self.updates.try_lock() {
                self.sync_library_catalog(self.configuration()?).await?;
            }
            return self.status();
        }
        let Some(login) = crate::subsonic_login::discover_strawberry().await else {
            return self.status();
        };
        let _update = self.updates.lock().await;
        if self
            .store
            .get_json_preference(self.protocol.preference())?
            .is_some()
        {
            return self.status();
        }
        match self
            .connect_locked(&login.endpoint, &login.username, login.password)
            .await
        {
            Ok(mut status) => {
                status.refresh_after_login = true;
                Ok(status)
            }
            Err(_) => Ok(Status {
                warning: Some(
                    "Could not use Strawberry’s saved Subsonic login. You can connect manually."
                        .into(),
                ),
                ..self.status()?
            }),
        }
    }

    pub(crate) async fn connect(
        self: &Arc<Self>,
        endpoint: &str,
        username: &str,
        password: String,
    ) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        self.connect_locked(endpoint, username, password).await
    }

    async fn connect_locked(
        self: &Arc<Self>,
        endpoint: &str,
        username: &str,
        password: String,
    ) -> Result<Status, String> {
        if (self.protocol == Protocol::Subsonic && password.is_empty()) || password.len() > 4096 {
            return Err(format!("Enter a valid {} password", self.protocol.label()));
        }
        let mut config = Configuration {
            endpoint: canonical_endpoint(endpoint)?,
            username: canonical_username(username)?,
            authenticated_account: None,
            visible: true,
            allow_fallback: false,
        };
        let protocol = self.protocol;
        let client = ServerClient::connect(
            protocol,
            &config.endpoint,
            &config.username,
            password.clone(),
            &self.profile,
        )
        .await?;
        if let ServerClient::Jellyfin(jellyfin) = &client {
            config.authenticated_account = Some(jellyfin.account_identity().to_owned());
        }
        let account = config.account();
        let previous = self.configuration()?;
        if let Some(previous) = &previous {
            if previous.account() == account {
                config.visible = previous.visible;
                config.allow_fallback = previous.allow_fallback;
            }
        }
        // A usable cache is required before persisting a connection or password.
        self.cache()?;
        let key = self.keyring_id(&account);
        tokio::task::spawn_blocking(move || {
            keyring::Entry::new(protocol.keyring_service(), &key)
                .and_then(|entry| entry.set_password(&password))
                .map_err(|_| {
                    format!(
                        "Could not save the {} password in the system keyring",
                        protocol.label()
                    )
                })
        })
        .await
        .map_err(|_| format!("Could not open the {} keyring", protocol.label()))??;
        if let Err(error) = self.store.save_server_library_connection(
            protocol.provider(),
            protocol.preference(),
            Some(&account),
            config.visible,
            &json!(&config),
        ) {
            if previous
                .as_ref()
                .is_none_or(|previous| previous.account() != account)
            {
                remove_password(protocol, self.keyring_id(&account)).await;
            }
            return Err(error);
        }
        *self.configured_client.lock().await = Some((account.clone(), client));
        self.clear_leases();
        self.sync_library_catalog(Some(config)).await?;
        let mut status = self.status()?;
        status.refresh_after_login = status.track_count == 0;
        if let Some(previous) = previous.filter(|previous| previous.account() != account) {
            status.warning = remove_password(protocol, self.keyring_id(&previous.account())).await;
        }
        Ok(status)
    }

    pub(crate) async fn disconnect(&self) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        if let Some(config) = self.configuration()? {
            self.store.save_server_library_connection(
                self.protocol.provider(),
                self.protocol.preference(),
                None,
                false,
                &serde_json::Value::Null,
            )?;
            *self.configured_client.lock().await = None;
            self.clear_leases();
            let key = self.keyring_id(&config.account());
            return Ok(Status {
                warning: remove_password(self.protocol, key).await,
                ..Status::default()
            });
        }
        self.status()
    }

    fn clear_leases(&self) {
        if let Ok(mut leases) = self.leases.lock() {
            leases.clear();
        }
    }

    pub(crate) async fn preferences(
        self: &Arc<Self>,
        visible: bool,
        allow_fallback: bool,
    ) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        let mut config = self.account_config()?;
        config.visible = visible;
        config.allow_fallback = allow_fallback;
        self.store.save_server_library_connection(
            self.protocol.provider(),
            self.protocol.preference(),
            Some(&config.account()),
            config.visible,
            &json!(config),
        )?;
        self.sync_library_catalog(Some(config)).await?;
        self.status()
    }

    async fn sync_library_catalog(
        self: &Arc<Self>,
        configuration: Option<Configuration>,
    ) -> Result<(), String> {
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            let provider = service.protocol.provider();
            let Some(config) = configuration.filter(|config| config.visible) else {
                return service.store.show_server_library_catalog(provider, None);
            };
            let account = config.account();
            let stored_revision = service
                .store
                .server_library_catalog_revision(provider, &account)?;
            let snapshot = service.with_cache(|conn| {
                let Some(revision) = subsonic_cache::revision(conn, &account)? else {
                    return Ok(None);
                };
                if stored_revision.as_deref() == Some(revision.as_str()) {
                    return Ok(Some((revision, None)));
                }
                Ok(Some((
                    revision,
                    Some(subsonic_cache::catalog_entries(conn, &account)?),
                )))
            })?;
            match snapshot {
                Some((revision, Some(songs))) => {
                    let tracks = songs
                        .iter()
                        .map(|song| server_track(provider, &account, song))
                        .collect::<Vec<_>>();
                    service
                        .store
                        .replace_server_library_catalog(provider, &account, &revision, &tracks)
                }
                Some((_, None)) => service
                    .store
                    .show_server_library_catalog(provider, Some(&account)),
                None => service.store.show_server_library_catalog(provider, None),
            }
        })
        .await
        .map_err(|_| {
            format!(
                "Could not synchronize the {} Library catalog",
                self.protocol.label()
            )
        })?
    }

    pub(crate) fn status(&self) -> Result<Status, String> {
        let Some(config) = self.configuration()? else {
            return Ok(Status::default());
        };
        let account = config.account();
        let (track_count, can_seed) = self.with_cache(|conn| {
            Ok((
                subsonic_cache::count(conn, &account)?,
                subsonic_cache::can_seed(conn, &account)?,
            ))
        })?;
        Ok(Status {
            endpoint: Some(config.endpoint),
            username: Some(config.username),
            visible: config.visible,
            allow_fallback: config.allow_fallback,
            track_count,
            can_seed,
            strawberry_found: self.protocol == Protocol::Subsonic
                && strawberry_default_path().is_some(),
            refresh_after_login: false,
            warning: None,
        })
    }

    pub(crate) fn page(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        scope: Option<BrowseScope>,
    ) -> Result<Page, String> {
        let Some(config) = self.configuration()?.filter(|config| config.visible) else {
            return Ok(Page {
                tracks: Vec::new(),
                total: 0,
                snapshot_id: None,
            });
        };
        let account = config.account();
        let (songs, total, snapshot_id) = self.with_cache(|conn| {
            let representative = match scope.as_ref() {
                Some(BrowseScope::Artist(track_id) | BrowseScope::Album(track_id)) => {
                    let id = track_id
                        .strip_prefix(&format!("{}:", self.protocol.provider()))
                        .and_then(reference_id)
                        .filter(|(owner, _)| *owner == account)
                        .ok_or_else(|| {
                            format!(
                                "This track belongs to a different {} account",
                                self.protocol.label()
                            )
                        })?
                        .1;
                    Some(
                        subsonic_cache::catalog_song(conn, &account, &id)?.ok_or_else(|| {
                            format!(
                                "This track is no longer in the {} catalog",
                                self.protocol.label()
                            )
                        })?,
                    )
                }
                None => None,
            };
            let filter = match (scope.as_ref(), representative.as_ref()) {
                (Some(BrowseScope::Album(_)), Some(song)) => Some(subsonic_cache::Scope::Album(
                    song.album_id
                        .as_deref()
                        .ok_or("This server did not provide an album ID")?,
                )),
                (Some(BrowseScope::Artist(_)), Some(song)) => {
                    Some(match song.artist_id.as_deref() {
                        Some(id) => subsonic_cache::Scope::ArtistId(id),
                        None => subsonic_cache::Scope::ArtistName(
                            song.artist
                                .as_deref()
                                .ok_or("This server did not provide an artist")?,
                        ),
                    })
                }
                _ => None,
            };
            let (songs, total) =
                subsonic_cache::page(conn, &account, query, offset, limit, filter)?;
            Ok((songs, total, subsonic_cache::revision(conn, &account)?))
        })?;
        self.check_current_account(&account)?;
        Ok(Page {
            tracks: songs
                .iter()
                .map(|song| server_track(self.protocol.provider(), &account, song))
                .collect(),
            total,
            snapshot_id,
        })
    }

    pub(crate) async fn track_artwork(&self, track_id: &str) -> Result<Option<String>, String> {
        let Some((account, id)) = track_id
            .strip_prefix(&format!("{}:", self.protocol.provider()))
            .and_then(reference_id)
        else {
            return Ok(None);
        };
        let Some(config) = self
            .configuration()?
            .filter(|config| config.account() == account)
        else {
            return Ok(None);
        };
        let Some(cached) =
            self.with_cache(|conn| subsonic_cache::catalog_song(conn, account, &id))?
        else {
            return Ok(None);
        };
        let client = self.client(&config).await?;
        let mut attempted = HashSet::new();
        let mut album_id = cached.album_id;
        let mut cover = cached.cover_art;
        for refresh_song in [false, true] {
            self.check_current_account(account)?;
            if refresh_song {
                if let Ok(song) = client.song(&id).await {
                    cover = song.cover_art;
                    album_id = song.album_id;
                }
            }
            if let Some(cover) = cover.take().filter(|cover| attempted.insert(cover.clone())) {
                if let Ok((mime, bytes)) = client.cover_art(&cover).await {
                    self.check_current_account(account)?;
                    return Ok(Some(format!(
                        "data:{mime};base64,{}",
                        STANDARD.encode(bytes)
                    )));
                }
            }
        }
        let Some(album_id) = album_id else {
            return Ok(None);
        };
        self.check_current_account(account)?;
        let Some(cover) = client.album_cover_id(&album_id).await? else {
            return Ok(None);
        };
        if attempted.contains(&cover) {
            return Ok(None);
        }
        self.check_current_account(account)?;
        let (mime, bytes) = client.cover_art(&cover).await?;
        self.check_current_account(account)?;
        Ok(Some(format!(
            "data:{mime};base64,{}",
            STANDARD.encode(bytes)
        )))
    }

    pub(crate) async fn refresh(self: &Arc<Self>) -> Result<Status, String> {
        let _update = self
            .updates
            .try_lock()
            .map_err(|_| format!("A {} update is already running", self.protocol.label()))?;
        let config = self.account_config()?;
        self.refresh_configured(config).await
    }

    async fn refresh_configured(self: &Arc<Self>, config: Configuration) -> Result<Status, String> {
        let account = config.account();
        let client = self.client(&config).await?;
        let songs = match &client {
            ServerClient::Subsonic(client) if !client.supports_song_catalog() => {
                Self::catalog_from_albums(client).await?
            }
            _ => Self::catalog_from_songs(&client).await?,
        };
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            service.check_current_account(&account)?;
            service.with_cache(|conn| subsonic_cache::replace(conn, &account, &songs, false))
        })
        .await
        .map_err(|_| format!("Could not save the {} catalog", self.protocol.label()))??;
        self.sync_library_catalog(Some(config)).await?;
        self.status()
    }

    async fn catalog_from_songs(client: &ServerClient) -> Result<Vec<Song>, String> {
        let mut songs = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        loop {
            let (page, consumed) = client.song_catalog_page(offset, SONG_PAGE).await?;
            if consumed == 0 {
                return Ok(songs);
            }
            offset += consumed;
            if offset > subsonic_cache::MAX_SONGS {
                return Err("The server song count exceeds the supported catalog size".into());
            }
            if page.iter().any(|song| !seen.insert(song.id.clone())) {
                return Err(
                    "The server song list repeated an earlier page; previous catalog retained"
                        .into(),
                );
            }
            songs.extend(page);
        }
    }

    async fn catalog_from_albums(client: &Client) -> Result<Vec<Song>, String> {
        let mut songs = Vec::new();
        let mut seen_albums = HashSet::new();
        let mut seen_songs = HashSet::new();
        let mut offset = 0;
        loop {
            let albums = client.album_page(offset, ALBUM_PAGE).await?;
            if albums.len() > ALBUM_PAGE || offset + albums.len() > MAX_ALBUMS {
                return Err("The Subsonic album count exceeds the supported catalog size".into());
            }
            if albums
                .iter()
                .any(|album| !seen_albums.insert(album.id.clone()))
            {
                return Err(
                    "The Subsonic album list repeated an earlier page; previous catalog retained"
                        .into(),
                );
            }
            let length = albums.len();
            let pages = stream::iter(albums.into_iter().map(|album| {
                let client = client.clone();
                async move {
                    let mut tracks = client.album(&album.id).await?;
                    for track in &mut tracks {
                        if track.album_id.as_ref().is_some_and(|id| id != &album.id) {
                            return Err("Subsonic returned a track from a different album".into());
                        }
                        track.album_id = Some(album.id.clone());
                        if track.album.is_none() {
                            track.album = Some(album.title.clone());
                        }
                        if track.album_artist.is_none() {
                            track.album_artist = album.artist.clone();
                        }
                    }
                    Ok::<_, String>(tracks)
                }
            }))
            .buffered(6);
            futures_util::pin_mut!(pages);
            while let Some(mut tracks) = pages.try_next().await? {
                if tracks
                    .iter()
                    .any(|track| !seen_songs.insert(track.id.clone()))
                {
                    return Err("The Subsonic snapshot has conflicting song identities; previous catalog retained".into());
                }
                if songs.len() + tracks.len() > subsonic_cache::MAX_SONGS {
                    return Err("The Subsonic song count exceeds the supported catalog size".into());
                }
                songs.append(&mut tracks);
            }
            offset += length;
            if length < ALBUM_PAGE {
                break;
            }
        }
        Ok(songs)
    }

    pub(crate) async fn import_strawberry(
        self: &Arc<Self>,
        path: Option<PathBuf>,
    ) -> Result<Status, String> {
        let _update = self
            .updates
            .try_lock()
            .map_err(|_| "A Subsonic update is already running")?;
        let config = self.account_config()?;
        let account = config.account();
        if !self.with_cache(|conn| subsonic_cache::can_seed(conn, &account))? {
            return Err("This account already has a Burrata Subsonic catalog".into());
        }
        let path = path
            .or_else(strawberry_default_path)
            .ok_or("Choose the Strawberry database first")?;
        let songs = tokio::task::spawn_blocking(move || subsonic_cache::strawberry(&path))
            .await
            .map_err(|_| "Could not read the Strawberry cache")??;
        let client = self.client(&config).await?;
        // Strawberry has no trustworthy account field per row. Sample IDs across
        // the cache and check exact catalog metadata against the authenticated server.
        let stride = (songs.len() / 8).max(1);
        let samples = songs
            .iter()
            .step_by(stride)
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        stream::iter(samples)
            .map(|expected| {
                let client = client.clone();
                async move {
                    let found = client.song(&expected.id).await?;
                    if !seed_identity_agrees(&expected, &found) {
                        return Err(
                            "The Strawberry cache does not match the connected Subsonic account"
                                .into(),
                        );
                    }
                    Ok::<_, String>(())
                }
            })
            .buffer_unordered(4)
            .try_collect::<Vec<_>>()
            .await?;
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            service.check_current_account(&account)?;
            service.with_cache(|conn| subsonic_cache::replace(conn, &account, &songs, true))
        })
        .await
        .map_err(|_| "Could not save the Strawberry seed")??;
        self.sync_library_catalog(Some(config.clone())).await?;
        match self.refresh_configured(config).await {
            Ok(status) => Ok(status),
            Err(error) => {
                let mut status = self.status()?;
                status.warning = Some(format!(
                    "The Strawberry catalog is ready. Live reconciliation did not finish: {error}"
                ));
                Ok(status)
            }
        }
    }

    pub(crate) fn fallback_track(&self, track: &Track) -> Option<Track> {
        let (account, id) = server_track_reference(self.protocol.provider(), track)?;
        let config = self.configuration().ok()??;
        if !config.allow_fallback
            || config.account() != account
            || !self
                .store
                .has_saved_server_track(&track.id)
                .unwrap_or(false)
        {
            return None;
        }
        // Only a reconciled server snapshot can supply fallback recording
        // evidence. A reference restored from a backup does not certify tags.
        let song = self
            .with_cache(|conn| subsonic_cache::song(conn, account, &id))
            .ok()??;
        let cached = server_track(self.protocol.provider(), account, &song);
        (cached.title == track.title
            && cached.artist == track.artist
            && cached.version == track.version
            && cached.duration_seconds == track.duration_seconds)
            .then_some(cached)
    }

    pub(crate) async fn resolve(
        &self,
        track: &Track,
        settings: &Settings,
    ) -> Result<ResolvedSource, String> {
        let (account, id) = server_track_reference(self.protocol.provider(), track)
            .ok_or_else(|| format!("Invalid {} track reference", self.protocol.label()))?;
        let config = self.account_config()?;
        let client = self.client(&config).await?;
        self.check_current_account(account)?;
        let song = client.song(&id).await?;
        if song.id != id {
            return Err(format!(
                "The server did not return the requested {} song",
                self.protocol.label()
            ));
        }
        let mut probe = client
            .stream(&id, Some(HeaderValue::from_static("bytes=0-1")))
            .await?;
        if !matches!(probe.status(), StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
            return Err(format!(
                "{} did not return playable audio for the requested song",
                self.protocol.label()
            ));
        }
        self.check_current_account(account)?;
        let mime = probe
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = probe
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit_once('/'))
            .and_then(|(_, length)| length.parse().ok())
            .or_else(|| {
                if probe.status() == StatusCode::OK {
                    probe.content_length()
                } else {
                    None
                }
            });
        if !probe
            .chunk()
            .await
            .ok()
            .flatten()
            .is_some_and(|chunk| !chunk.is_empty())
        {
            return Err(format!(
                "The {} server returned no audio for the requested song",
                self.protocol.label()
            ));
        }
        self.check_current_account(account)?;
        let base = self
            .bridge_base
            .get()
            .ok_or_else(|| format!("The {} audio bridge is not ready", self.protocol.label()))?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut leases = self
            .leases
            .lock()
            .map_err(|_| format!("The {} audio bridge is unavailable", self.protocol.label()))?;
        leases.retain(|_, lease| lease.issued.elapsed() < AUDIO_LEASE_TTL);
        if leases.len() >= MAX_LEASES {
            if let Some(oldest) = leases
                .iter()
                .min_by_key(|(_, lease)| lease.issued)
                .map(|(key, _)| key.clone())
            {
                leases.remove(&oldest);
            }
        }
        leases.insert(
            token.clone(),
            Lease {
                issued: Instant::now(),
                account: account.into(),
                id,
            },
        );
        Ok(ResolvedSource {
            origin: ResolvedSourceOrigin::Existing,
            source: AudioSource {
                input: format!("{base}/{}/{token}", self.protocol.bridge_route()),
                ffmpeg: crate::tools::executable(&settings.ffmpeg_path, "ffmpeg"),
                headers: Vec::new(),
                decoder_hint: Some(AudioSourceDecoderHint {
                    mime_type: mime,
                    codec: None,
                    duration_seconds: song.duration,
                    content_length,
                    has_video: false,
                    decode_policy: AudioSourceDecodePolicy::Automatic,
                    decoder_fingerprint: None,
                }),
                cenc: None,
            },
            quality: json!({"sourceKind":"stream","provider":self.protocol.provider()}),
            candidate: None,
            station: None,
            duration: song.duration,
        })
    }
}

fn strawberry_default_path() -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("strawberry")
            .join("strawberry")
            .join("strawberry.db"),
    )
    .filter(|path| path.is_file())
}

fn scope_hash(first: &[u8], second: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update((first.len() as u64).to_be_bytes());
    hash.update(first);
    hash.update(second);
    format!("{:x}", hash.finalize())
}

fn profile_scope(profile_dir: &Path) -> String {
    scope_hash(b"profile", profile_dir.as_os_str().as_encoded_bytes())
}

fn delete_password(protocol: Protocol, key: &str) -> Option<String> {
    match keyring::Entry::new(protocol.keyring_service(), key)
        .and_then(|entry| entry.delete_credential())
    {
        Ok(()) | Err(keyring::Error::NoEntry) => None,
        Err(_) => Some(format!(
            "The system keyring could not remove the previous {} password.",
            protocol.label()
        )),
    }
}

async fn remove_password(protocol: Protocol, key: String) -> Option<String> {
    tokio::task::spawn_blocking(move || delete_password(protocol, &key))
        .await
        .unwrap_or_else(|_| Some(format!("Could not open the {} keyring.", protocol.label())))
}

pub(crate) fn reset_profile_credentials(profile_dir: &Path, quarantine: &Path) -> Vec<String> {
    [Protocol::Subsonic, Protocol::Jellyfin]
        .into_iter()
        .filter_map(|protocol| reset_connection_credentials(profile_dir, quarantine, protocol))
        .collect()
}

fn reset_connection_credentials(
    profile_dir: &Path,
    quarantine: &Path,
    protocol: Protocol,
) -> Option<String> {
    let database = quarantine.join("burrata.sqlite");
    if !database.is_file() {
        return None;
    }
    let saved = (|| {
        let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
            .ok()?;
        conn.query_row(
            "SELECT value FROM settings WHERE key=?1",
            [protocol.preference()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
    })();
    let config = match saved {
        None => {
            return Some(format!(
                "Could not check for a saved {} password during reset.",
                protocol.label()
            ))
        }
        Some(None) => return None,
        Some(Some(raw)) => match serde_json::from_str::<Option<Configuration>>(&raw) {
            Ok(config) => config?,
            Err(_) => {
                return Some(format!(
                    "Could not read {} connection metadata during reset.",
                    protocol.label()
                ))
            }
        },
    };
    delete_password(
        protocol,
        &format!("{}-{}", profile_scope(profile_dir), config.account()),
    )
}

fn server_track_reference<'a>(provider: &str, track: &'a Track) -> Option<(&'a str, String)> {
    if track.provider != provider || track.id != format!("{provider}:{}", track.provider_id) {
        return None;
    }
    reference_id(&track.provider_id)
}

fn reference_id(provider_id: &str) -> Option<(&str, String)> {
    let (account, encoded) = provider_id.split_once('.')?;
    if account.len() != 64
        || !account
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || encoded.is_empty()
        || encoded.len() > 1400
    {
        return None;
    }
    let id = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?;
    if id.is_empty()
        || id.len() > 512
        || id.chars().any(char::is_control)
        || URL_SAFE_NO_PAD.encode(id.as_bytes()) != encoded
    {
        return None;
    }
    Some((account, id))
}

fn server_track(provider: &str, account: &str, song: &Song) -> Track {
    let provider_id = format!("{account}.{}", URL_SAFE_NO_PAD.encode(song.id.as_bytes()));
    let mut title = song.title.clone();
    if let Some(movement) = &song.movement {
        if !title.to_lowercase().contains(&movement.to_lowercase()) {
            title.push_str(": ");
            title.push_str(movement);
        }
    }
    Track {
        id: format!("{provider}:{provider_id}"),
        provider: provider.into(),
        provider_id,
        title,
        artist: song.artist.clone().unwrap_or_default(),
        album: song.album.clone(),
        album_artist: song.album_artist.clone(),
        version: song.version.clone(),
        duration_seconds: song.duration,
        track_number: song.track,
        disc_number: song.disc,
        genre: song.genre.clone(),
        release_date: song.year.map(|year| year.to_string()),
        isrc: song.isrc.clone(),
        provenance: json!({
            "serverAlbumId": song.album_id,
            "serverArtistId": song.artist_id,
            "serverCoverArtId": song.cover_art,
            "subsonicAlbumId": song.album_id,
            "subsonicArtistId": song.artist_id,
            "subsonicCoverArtId": song.cover_art,
            "recordingId": song.recording_mbid,
            "artistCredits": song.performers.iter().map(|name| json!({"name":name,"role":"artist"})).collect::<Vec<_>>(),
            "workIds": song.work_mbid.iter().collect::<Vec<_>>(),
            "subsonicWork": song.work,
            "subsonicWorkMbid": song.work_mbid,
            "subsonicMovement": song.movement,
            "subsonicMovementNumber": song.movement_number,
        }),
        ..Track::default()
    }
}

fn seed_identity_agrees(expected: &Song, found: &Song) -> bool {
    let normal = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    expected.id == found.id
        && expected.album_id.is_some()
        && expected.album_id == found.album_id
        && normal(&expected.title) == normal(&found.title)
        && expected.artist.as_deref().map(normal) == found.artist.as_deref().map(normal)
        && match (expected.duration, found.duration) {
            (Some(left), Some(right)) => (left - right).abs() <= 2.0,
            _ => true,
        }
}

pub(crate) fn verified_alternate(source: &ResolvedSource) -> bool {
    use crate::recording_match::{
        Comparison, MatchAction, MatchAssessment, MatchRelationship, ProofStrength,
    };
    let Some(metadata) = source
        .candidate
        .as_ref()
        .and_then(|candidate| candidate.source_metadata.as_ref())
    else {
        return false;
    };
    let Ok(assessment) =
        serde_json::from_value::<MatchAssessment>(metadata["matching"]["assessment"].clone())
    else {
        return false;
    };
    assessment.relationship == MatchRelationship::Exact
        && assessment.action == MatchAction::Accept
        && assessment.proof_strength != ProofStrength::MetadataOnly
        && assessment.dimensions.title == Comparison::Agree
        && assessment.dimensions.artists == Comparison::Agree
        && assessment.dimensions.version == Comparison::Agree
        && assessment.dimensions.runtime == Comparison::Agree
        && assessment.dimensions.guests != Comparison::Conflict
        && assessment.dimensions.work != Comparison::Conflict
}

fn empty(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

async fn audio(
    State(service): State<Arc<Service>>,
    AxumPath(token): AxumPath<String>,
    headers: HeaderMap,
    method: Method,
) -> Response<Body> {
    if token.len() != 32
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || headers.get(header::ORIGIN).is_some()
    {
        return empty(StatusCode::NOT_FOUND);
    }
    let lease = service
        .leases
        .lock()
        .ok()
        .and_then(|leases| leases.get(&token).cloned())
        .filter(|lease| lease.issued.elapsed() < AUDIO_LEASE_TTL);
    let Some(lease) = lease else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(config) = service.account_config() else {
        return empty(StatusCode::NOT_FOUND);
    };
    if config.account() != lease.account {
        return empty(StatusCode::NOT_FOUND);
    }
    let Ok(client) = service.client(&config).await else {
        return empty(StatusCode::BAD_GATEWAY);
    };
    let range = headers.get(header::RANGE).cloned();
    let Ok(upstream) = client.stream(&lease.id, range).await else {
        return empty(StatusCode::BAD_GATEWAY);
    };
    if service.check_current_account(&lease.account).is_err()
        || !service
            .leases
            .lock()
            .is_ok_and(|leases| leases.contains_key(&token))
    {
        return empty(StatusCode::NOT_FOUND);
    }
    let status = upstream.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let mut response = empty(status);
        if let Some(range) = upstream.headers().get(header::CONTENT_RANGE) {
            response
                .headers_mut()
                .insert(header::CONTENT_RANGE, range.clone());
        }
        return response;
    }
    let forwarded = [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ]
    .into_iter()
    .filter_map(|name| {
        upstream
            .headers()
            .get(&name)
            .cloned()
            .map(|value| (name, value))
    })
    .collect::<Vec<_>>();
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(
            upstream
                .bytes_stream()
                .map_err(|_| std::io::Error::other("Music server stream interrupted")),
        )
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().extend(forwarded);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
#[path = "subsonic_jellyfin_tests.rs"]
mod jellyfin_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_subsonic_catalog_advances_by_wire_rows_across_short_pages() {
        use axum::{extract::Query, Json};

        async fn search(Query(args): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
            assert_eq!(args.get("query").map(String::as_str), Some(""));
            assert_eq!(args.get("artistCount").map(String::as_str), Some("0"));
            assert_eq!(args.get("albumCount").map(String::as_str), Some("0"));
            let songs = match args.get("songOffset").map(String::as_str) {
                Some("0") => json!([
                    {"id":"first","title":"First","albumId":"album","type":"music"},
                    {"id":"video","title":"Video","isVideo":true}
                ]),
                Some("2") => json!([{"id":"last","title":"Last","type":"music"}]),
                Some("3") => json!([]),
                offset => panic!("Unexpected search offset: {offset:?}"),
            };
            Json(json!({"subsonic-response":{"status":"ok","searchResult3":{"song":songs}}}))
        }

        let app = Router::new()
            .route(
                "/rest/ping.view",
                get(|| async {
                    Json(json!({"subsonic-response":{"status":"ok","openSubsonic":true}}))
                }),
            )
            .route("/rest/search3.view", get(search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::connect(&endpoint, "listener", "test".into())
            .await
            .unwrap();
        assert!(client.supports_song_catalog());
        let songs = Service::catalog_from_songs(&ServerClient::Subsonic(client))
            .await
            .unwrap();
        assert_eq!(
            songs
                .iter()
                .map(|song| song.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "last"]
        );
        assert_eq!(songs[0].album_id.as_deref(), Some("album"));
        server.abort();
    }

    #[tokio::test]
    async fn artwork_reads_the_album_cover_from_the_connected_server_for_an_old_catalog() {
        use axum::{extract::Query, Json};

        let app = Router::new()
            .route(
                "/rest/ping.view",
                get(|| async {
                    Json(json!({"subsonic-response":{"status":"ok","openSubsonic":true}}))
                }),
            )
            .route(
                "/rest/getSong.view",
                get(|Query(args): Query<HashMap<String, String>>| async move {
                    assert_eq!(args.get("id").unwrap(), "track");
                    Json(json!({"subsonic-response":{"status":"ok","song":{
                        "id":"track","title":"Title","albumId":"album"
                    }}}))
                }),
            )
            .route(
                "/rest/getAlbum.view",
                get(|Query(args): Query<HashMap<String, String>>| async move {
                    assert_eq!(args.get("id").unwrap(), "album");
                    Json(json!({"subsonic-response":{"status":"ok","album":{
                        "id":"album","name":"Album","coverArt":"cover","song":[]
                    }}}))
                }),
            )
            .route(
                "/rest/getCoverArt.view",
                get(|Query(args): Query<HashMap<String, String>>| async move {
                    assert_eq!(args.get("id").unwrap(), "cover");
                    assert_eq!(args.get("size").unwrap(), "256");
                    assert!(args.contains_key("t"));
                    (
                        [(header::CONTENT_TYPE, "image/png")],
                        b"\x89PNG\r\n\x1a\n".as_slice(),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("library.sqlite")).unwrap());
        let config = Configuration {
            endpoint: endpoint.clone(),
            username: "listener".into(),
            authenticated_account: None,
            visible: true,
            allow_fallback: false,
        };
        store.set_json_preference(PREF, &json!(config)).unwrap();
        let service = Service::new(store, dir.path());
        let account = config.account();
        let client = Client::connect(&endpoint, "listener", "test".into())
            .await
            .unwrap();
        *service.configured_client.lock().await =
            Some((account.clone(), ServerClient::Subsonic(client)));
        let song = Song {
            id: "track".into(),
            title: "Title".into(),
            album_id: Some("album".into()),
            ..Song::default()
        };
        service
            .with_cache(|conn| subsonic_cache::replace(conn, &account, &[song.clone()], true))
            .unwrap();
        let track = server_track("subsonic", &account, &song);
        assert_eq!(
            service.track_artwork(&track.id).await.unwrap().as_deref(),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        let off_account = server_track("subsonic", &"a".repeat(64), &song);
        assert!(service
            .track_artwork(&off_account.id)
            .await
            .unwrap()
            .is_none());
        server.abort();
    }

    #[test]
    fn tracks_from_the_server_can_be_saved_to_a_burrata_playlist() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("library.sqlite")).unwrap();
        let collection = store.create_collection("Saved server tracks").unwrap();
        let track = server_track(
            "subsonic",
            &"a".repeat(64),
            &Song {
                id: "server-track".into(),
                title: "Title".into(),
                artist: Some("Artist".into()),
                album: Some("Album".into()),
                ..Song::default()
            },
        );
        store
            .append_playlist_entries(&collection.id, &[track.clone()])
            .unwrap();
        let saved = store
            .list_tracks("collection", Some(&collection.id))
            .unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, track.id);
        assert_eq!(saved[0].provider_id, track.provider_id);
    }

    #[test]
    fn replacement_library_keeps_current_device_connection_and_discards_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let current = Store::open(&dir.path().join("current.sqlite")).unwrap();
        let staged = Store::open(&dir.path().join("staged.sqlite")).unwrap();
        current
            .set_json_preference(
                PREF,
                &json!({
                    "endpoint": "https://music.example.com",
                    "username": "listener",
                    "visible": false,
                    "allowFallback": true,
                    "password": "discard-this",
                }),
            )
            .unwrap();
        preserve_current_connection(&current, &staged).unwrap();
        assert_eq!(
            staged.get_json_preference(PREF).unwrap(),
            Some(json!({
                "endpoint": "https://music.example.com",
                "username": "listener",
                "visible": false,
                "allowFallback": true,
            }))
        );
    }

    #[test]
    fn fallback_requires_recording_proof_and_rejects_conflicting_versions_and_works() {
        use crate::models::Candidate;
        let mut source = ResolvedSource {
            origin: ResolvedSourceOrigin::Existing,
            source: AudioSource {
                input: "memory:test".into(),
                ffmpeg: PathBuf::new(),
                headers: Vec::new(),
                decoder_hint: None,
                cenc: None,
            },
            quality: json!({}),
            candidate: Some(Candidate {
                source_metadata: Some(json!({"matching":{"assessment":{
                    "relationship": "exact",
                    "action": "accept",
                    "reasons": [],
                    "proofStrength": "recordingIdentifier",
                    "dimensions": {
                        "title":"agree", "artists":"agree", "guests":"agree",
                        "version":"agree", "runtime":"agree", "isrc":"agree", "work":"agree"
                    }
                }}})),
                ..Candidate::default()
            }),
            station: None,
            duration: Some(200.0),
        };
        assert!(verified_alternate(&source));
        fn metadata(source: &mut ResolvedSource) -> &mut serde_json::Value {
            source
                .candidate
                .as_mut()
                .unwrap()
                .source_metadata
                .as_mut()
                .unwrap()
        }
        metadata(&mut source)["matching"]["assessment"]["proofStrength"] = json!("metadataOnly");
        assert!(!verified_alternate(&source));
        metadata(&mut source)["matching"]["assessment"]["proofStrength"] =
            json!("recordingIdentifier");
        metadata(&mut source)["matching"]["assessment"]["dimensions"]["version"] =
            json!("conflict");
        assert!(!verified_alternate(&source));
        metadata(&mut source)["matching"]["assessment"]["dimensions"]["version"] = json!("agree");
        metadata(&mut source)["matching"]["assessment"]["dimensions"]["work"] = json!("conflict");
        assert!(!verified_alternate(&source));
    }

    #[test]
    fn account_bound_references_survive_metadata_edits_and_expose_no_credentials() {
        let account = scope_hash(b"https://one.invalid", b"user");
        assert_ne!(account, scope_hash(b"https://two.invalid", b"user"));
        assert_ne!(account, scope_hash(b"https://one.invalid", b"other-user"));
        let song = Song {
            id: "shared/ID:1".into(),
            title: "Concerto".into(),
            artist: Some("Artist".into()),
            movement: Some("II. Andante".into()),
            ..Song::default()
        };
        let mut track = server_track("subsonic", &account, &song);
        assert_eq!(track.title, "Concerto: II. Andante");
        track.title = "Manual title".into();
        assert_eq!(
            server_track_reference("subsonic", &track),
            Some((account.as_str(), song.id))
        );
        assert!(track.source_url.is_empty() && track.preview_url.is_none());
        track.id.push('x');
        assert!(server_track_reference("subsonic", &track).is_none());
    }

    #[test]
    fn strawberry_seed_requires_matching_live_track_album_and_performer() {
        let song = Song {
            id: "one".into(),
            album_id: Some("album".into()),
            title: "Title".into(),
            artist: Some("Artist".into()),
            duration: Some(175.0),
            ..Song::default()
        };
        assert!(seed_identity_agrees(&song, &song));
        assert!(!seed_identity_agrees(
            &song,
            &Song {
                album_id: Some("different".into()),
                ..song.clone()
            }
        ));
        assert!(!seed_identity_agrees(
            &song,
            &Song {
                artist: Some("Different".into()),
                ..song.clone()
            }
        ));
        assert!(!seed_identity_agrees(
            &song,
            &Song {
                duration: Some(190.0),
                ..song.clone()
            }
        ));
    }
}
