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
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Configuration {
    endpoint: String,
    username: String,
    visible: bool,
    allow_fallback: bool,
}

impl Configuration {
    fn account(&self) -> String {
        scope_hash(self.endpoint.as_bytes(), self.username.as_bytes())
    }
}

fn read_configuration(store: &Store) -> Result<Option<Configuration>, String> {
    store
        .get_json_preference(PREF)?
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value).map_err(|_| "Invalid Subsonic preferences".into())
        })
        .transpose()
}

pub(crate) fn suppress_automatic_login(store: &Store) -> Result<(), String> {
    store.set_json_preference(PREF, &serde_json::Value::Null)
}

/// Library restores retain the existing device connection. The backup itself
/// contains only saved track references and never carries a connection.
pub(crate) fn preserve_current_connection(current: &Store, staged: &Store) -> Result<(), String> {
    if current.get_json_preference(PREF)?.is_some() {
        let config = read_configuration(current)?;
        staged.set_json_preference(PREF, &json!(config))?;
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
}

#[derive(Clone)]
struct Lease {
    issued: Instant,
    account: String,
    id: String,
}

pub(crate) struct Service {
    store: Arc<Store>,
    cache_path: PathBuf,
    profile: String,
    cache: OnceLock<Mutex<Connection>>,
    configured_client: tokio::sync::Mutex<Option<(String, Client)>>,
    automatic_login_attempted: AtomicBool,
    updates: tokio::sync::Mutex<()>,
    bridge_base: OnceLock<String>,
    leases: Mutex<HashMap<String, Lease>>,
}

impl Service {
    pub(crate) fn new(store: Arc<Store>, profile_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            store,
            cache_path: profile_dir.join("subsonic").join("catalog.sqlite"),
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
            .route("/subsonic-audio/{token}", get(audio).head(audio))
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
            .ok_or("Invalid Subsonic cache path")?;
        std::fs::create_dir_all(dir)
            .map_err(|_| "Could not prepare the Subsonic cache directory")?;
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
            .map_err(|_| "Subsonic catalog unavailable")?;
        read(&mut conn)
    }

    fn configuration(&self) -> Result<Option<Configuration>, String> {
        read_configuration(&self.store)
    }

    fn account_config(&self) -> Result<Configuration, String> {
        self.configuration()?
            .ok_or("Connect a Subsonic server in Settings first".into())
    }

    fn keyring_id(&self, account: &str) -> String {
        format!("{}-{account}", self.profile)
    }

    async fn client(&self, expected: &Configuration) -> Result<Client, String> {
        let mut slot = self.configured_client.lock().await;
        let account = expected.account();
        self.check_current_account(&account)?;
        if let Some((active, client)) = slot.as_ref() {
            if active == &account {
                return Ok(client.clone());
            }
        }
        let key = self.keyring_id(&account);
        let password = tokio::task::spawn_blocking(move || {
            keyring::Entry::new(KEYRING_SERVICE, &key)
                .and_then(|entry| entry.get_password())
                .map_err(|_| "Subsonic password unavailable in the system keyring")
        })
        .await
        .map_err(|_| "Could not read the Subsonic password")??;
        let client = Client::connect(&expected.endpoint, &expected.username, password).await?;
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
            Err("The Subsonic connection changed".into())
        }
    }

    pub(crate) async fn status_with_existing_login(&self) -> Result<Status, String> {
        // A saved connection or explicit disconnect (JSON null) always wins.
        if self.store.get_json_preference(PREF)?.is_some()
            || self.automatic_login_attempted.swap(true, Ordering::AcqRel)
        {
            return self.status();
        }
        let Some(login) = crate::subsonic_login::discover_strawberry().await else {
            return self.status();
        };
        let _update = self.updates.lock().await;
        if self.store.get_json_preference(PREF)?.is_some() {
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
        &self,
        endpoint: &str,
        username: &str,
        password: String,
    ) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        self.connect_locked(endpoint, username, password).await
    }

    async fn connect_locked(
        &self,
        endpoint: &str,
        username: &str,
        password: String,
    ) -> Result<Status, String> {
        if password.is_empty() || password.len() > 4096 {
            return Err("Enter a valid Subsonic password".into());
        }
        let mut config = Configuration {
            endpoint: canonical_endpoint(endpoint)?,
            username: canonical_username(username)?,
            visible: true,
            allow_fallback: false,
        };
        let account = config.account();
        let previous = self.configuration()?;
        if let Some(previous) = &previous {
            if previous.account() == account {
                config.visible = previous.visible;
                config.allow_fallback = previous.allow_fallback;
            }
        }
        let client = Client::connect(&config.endpoint, &config.username, password.clone()).await?;
        // A usable cache is required before persisting a connection or password.
        self.cache()?;
        let key = self.keyring_id(&account);
        tokio::task::spawn_blocking(move || {
            keyring::Entry::new(KEYRING_SERVICE, &key)
                .and_then(|entry| entry.set_password(&password))
                .map_err(|_| "Could not save the Subsonic password in the system keyring")
        })
        .await
        .map_err(|_| "Could not open the Subsonic keyring")??;
        if let Err(error) = self.store.set_json_preference(PREF, &json!(&config)) {
            if previous
                .as_ref()
                .is_none_or(|previous| previous.account() != account)
            {
                remove_password(self.keyring_id(&account)).await;
            }
            return Err(error);
        }
        *self.configured_client.lock().await = Some((account.clone(), client));
        self.clear_leases();
        let mut status = self.status()?;
        if let Some(previous) = previous.filter(|previous| previous.account() != account) {
            status.warning = remove_password(self.keyring_id(&previous.account())).await;
        }
        Ok(status)
    }

    pub(crate) async fn disconnect(&self) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        if let Some(config) = self.configuration()? {
            suppress_automatic_login(&self.store)?;
            *self.configured_client.lock().await = None;
            self.clear_leases();
            let key = self.keyring_id(&config.account());
            return Ok(Status {
                warning: remove_password(key).await,
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
        &self,
        visible: bool,
        allow_fallback: bool,
    ) -> Result<Status, String> {
        let _update = self.updates.lock().await;
        let mut config = self.account_config()?;
        config.visible = visible;
        config.allow_fallback = allow_fallback;
        self.store.set_json_preference(PREF, &json!(config))?;
        self.status()
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
            strawberry_found: strawberry_default_path().is_some(),
            refresh_after_login: false,
            warning: None,
        })
    }

    pub(crate) fn page(&self, query: &str, offset: usize, limit: usize) -> Result<Page, String> {
        let Some(config) = self.configuration()?.filter(|config| config.visible) else {
            return Ok(Page {
                tracks: Vec::new(),
                total: 0,
            });
        };
        let account = config.account();
        let (songs, total) =
            self.with_cache(|conn| subsonic_cache::page(conn, &account, query, offset, limit))?;
        self.check_current_account(&account)?;
        Ok(Page {
            tracks: songs.iter().map(|song| to_track(&account, song)).collect(),
            total,
        })
    }

    pub(crate) async fn refresh(self: &Arc<Self>) -> Result<Status, String> {
        let _update = self
            .updates
            .try_lock()
            .map_err(|_| "A Subsonic update is already running")?;
        let config = self.account_config()?;
        self.refresh_configured(config).await
    }

    async fn refresh_configured(self: &Arc<Self>, config: Configuration) -> Result<Status, String> {
        let account = config.account();
        let client = self.client(&config).await?;
        let songs = if client.supports_song_catalog() {
            Self::catalog_from_songs(&client).await?
        } else {
            Self::catalog_from_albums(&client).await?
        };
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            service.check_current_account(&account)?;
            service.with_cache(|conn| subsonic_cache::replace(conn, &account, &songs, false))
        })
        .await
        .map_err(|_| "Could not save the Subsonic catalog")??;
        self.status()
    }

    async fn catalog_from_songs(client: &Client) -> Result<Vec<Song>, String> {
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
                return Err("The Subsonic song count exceeds the supported catalog size".into());
            }
            if page.iter().any(|song| !seen.insert(song.id.clone())) {
                return Err(
                    "The Subsonic song list repeated an earlier page; previous catalog retained"
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
        let (account, id) = track_reference(track)?;
        let config = self.configuration().ok()??;
        if !config.allow_fallback
            || config.account() != account
            || !self
                .store
                .recording_is_in_library(&track.id)
                .unwrap_or(false)
        {
            return None;
        }
        // Only a reconciled server snapshot can supply fallback recording
        // evidence. A reference restored from a backup does not certify tags.
        let song = self
            .with_cache(|conn| subsonic_cache::song(conn, account, &id))
            .ok()??;
        let cached = to_track(account, &song);
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
        let (account, id) = track_reference(track).ok_or("Invalid Subsonic track reference")?;
        let config = self.account_config()?;
        let client = self.client(&config).await?;
        self.check_current_account(account)?;
        let song = client.song(&id).await?;
        if song.id != id {
            return Err("The server did not return the requested Subsonic song".into());
        }
        let mut probe = client
            .stream(&id, Some(HeaderValue::from_static("bytes=0-1")))
            .await?;
        if !matches!(probe.status(), StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
            return Err("Subsonic did not return playable audio for the requested song".into());
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
            return Err("The Subsonic server returned no audio for the requested song".into());
        }
        let base = self
            .bridge_base
            .get()
            .ok_or("The Subsonic audio bridge is not ready")?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut leases = self
            .leases
            .lock()
            .map_err(|_| "The Subsonic audio bridge is unavailable")?;
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
                input: format!("{base}/subsonic-audio/{token}"),
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
            quality: json!({"sourceKind":"stream","provider":"subsonic"}),
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

fn delete_password(key: &str) -> Option<String> {
    match keyring::Entry::new(KEYRING_SERVICE, key).and_then(|entry| entry.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => None,
        Err(_) => {
            Some("The system keyring could not remove the previous Subsonic password.".into())
        }
    }
}

async fn remove_password(key: String) -> Option<String> {
    tokio::task::spawn_blocking(move || delete_password(&key))
        .await
        .unwrap_or_else(|_| Some("Could not open the Subsonic keyring.".into()))
}

pub(crate) fn reset_profile_credentials(profile_dir: &Path, quarantine: &Path) -> Option<String> {
    let database = quarantine.join("burrata.sqlite");
    if !database.is_file() {
        return None;
    }
    let saved = (|| {
        let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
            .ok()?;
        conn.query_row("SELECT value FROM settings WHERE key=?1", [PREF], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .ok()
    })();
    let config = match saved {
        None => return Some("Could not check for a saved Subsonic password during reset.".into()),
        Some(None) => return None,
        Some(Some(raw)) => match serde_json::from_str::<Option<Configuration>>(&raw) {
            Ok(config) => config?,
            Err(_) => {
                return Some("Could not read Subsonic connection metadata during reset.".into())
            }
        },
    };
    delete_password(&format!(
        "{}-{}",
        profile_scope(profile_dir),
        config.account()
    ))
}

fn track_reference(track: &Track) -> Option<(&str, String)> {
    if track.provider != "subsonic" || track.id != format!("subsonic:{}", track.provider_id) {
        return None;
    }
    let (account, encoded) = track.provider_id.split_once('.')?;
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
        || id.chars().any(char::is_control)
        || URL_SAFE_NO_PAD.encode(id.as_bytes()) != encoded
    {
        return None;
    }
    Some((account, id))
}

fn to_track(account: &str, song: &Song) -> Track {
    let provider_id = format!("{account}.{}", URL_SAFE_NO_PAD.encode(song.id.as_bytes()));
    let mut title = song.title.clone();
    if let Some(movement) = &song.movement {
        if !title.to_lowercase().contains(&movement.to_lowercase()) {
            title.push_str(": ");
            title.push_str(movement);
        }
    }
    Track {
        id: format!("subsonic:{provider_id}"),
        provider: "subsonic".into(),
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
            "subsonicAlbumId": song.album_id,
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
                .map_err(|_| std::io::Error::other("Subsonic stream interrupted")),
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
        let songs = Service::catalog_from_songs(&client).await.unwrap();
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
        let mut track = to_track(&account, &song);
        assert_eq!(track.title, "Concerto: II. Andante");
        track.title = "Manual title".into();
        assert_eq!(track_reference(&track), Some((account.as_str(), song.id)));
        assert!(track.source_url.is_empty() && track.preview_url.is_none());
        track.id.push('x');
        assert!(track_reference(&track).is_none());
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
