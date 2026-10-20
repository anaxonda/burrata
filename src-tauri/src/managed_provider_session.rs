//! Experimental isolated provider sessions for provider operations.
//!
//! Provider credentials stay inside a browser session or the OS keyring. Rust sees only bounded,
//! normalized provider results.
#[path = "spotify_auth_cache.rs"]
mod spotify_auth_cache;
#[path = "spotify_auth_refresh_process.rs"]
mod spotify_auth_refresh_process;
#[path = "verification_creation_intent.rs"]
mod verification_creation_intent;
use self::spotify_auth_cache::{AuthEntry, AuthScope, SpotifyAuthCache};
use self::spotify_auth_refresh_process::run_private_auth_refresh;
use crate::{
    browser_bridge::{BrowserBridge, ProviderTransport},
    tools,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command as TokioCommand,
    sync::Semaphore,
};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MANAGED_SESSION_ENABLED: &str = "BURRATA_EXPERIMENTAL_MANAGED_PROVIDER_SESSION";
const MANAGED_BROWSER_BINARY: &str = "BURRATA_MANAGED_BROWSER_BINARY";
const NATIVE_SESSION_ENABLED: &str = "BURRATA_EXPERIMENTAL_NATIVE_PROVIDER_SESSION";
const NATIVE_LOGIN_LABEL: &str = "native-provider-login-spotify";
const NATIVE_LOGIN_URL: &str = "https://accounts.spotify.com/en/login";
const NATIVE_YOUTUBE_LOGIN_LABEL: &str = "native-provider-login-youtube";
const NATIVE_YOUTUBE_LOGIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fmusic.youtube.com";
const NATIVE_MODE_CONFIRMED: &str = "native-confirmed";
const NATIVE_MODE_READY: &str = "native-ready";
const NATIVE_KEYRING_SERVICE: &str = "org.burrata.discovery.provider-session";
const YOUTUBE_LOGIN_ORIGINS: &[&str] = &["https://www.youtube.com", "https://music.youtube.com"];
const NATIVE_WORKER_FILES: &[(&str, &str)] = &[
    (
        "scripts/provider-session-worker.mjs",
        include_str!("../../scripts/provider-session-worker.mjs"),
    ),
    (
        "scripts/youtube-session-worker.mjs",
        include_str!("../../scripts/youtube-session-worker.mjs"),
    ),
    (
        "extension/spotify-auth.js",
        include_str!("../../extension/spotify-auth.js"),
    ),
    (
        "extension/provider-operations.js",
        include_str!("../../extension/provider-operations.js"),
    ),
    (
        "extension/youtube-session-provider.js",
        include_str!("../../extension/youtube-session-provider.js"),
    ),
];
const EXTENSION_ICONS: &[(&str, &[u8])] = &[
    (
        "icons/burrata-16.png",
        include_bytes!("../../extension/icons/burrata-16.png"),
    ),
    (
        "icons/burrata-32.png",
        include_bytes!("../../extension/icons/burrata-32.png"),
    ),
    (
        "icons/burrata-48.png",
        include_bytes!("../../extension/icons/burrata-48.png"),
    ),
    (
        "icons/burrata-128.png",
        include_bytes!("../../extension/icons/burrata-128.png"),
    ),
];
const EXTENSION_FILES: &[(&str, &str)] = &[
    (
        "background.js",
        include_str!("../../extension/background.js"),
    ),
    ("content.js", include_str!("../../extension/content.js")),
    ("parsers.js", include_str!("../../extension/parsers.js")),
    ("popup.html", include_str!("../../extension/popup.html")),
    ("popup.js", include_str!("../../extension/popup.js")),
    (
        "provider-operations.js",
        include_str!("../../extension/provider-operations.js"),
    ),
    (
        "spotify-auth.js",
        include_str!("../../extension/spotify-auth.js"),
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSession {
    session_id: String,
    #[serde(default)]
    accounts: BTreeMap<String, ConfirmedAccount>,
    #[serde(default)]
    background_opt_in: bool,
    #[serde(default)]
    native_transport_opt_in: bool,
    #[serde(default)]
    native_stopped: bool,
    #[serde(default)]
    youtube_reconnect_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmedAccount {
    account_id: String,
    revision: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderSessionStatus {
    pub enabled: bool,
    pub running: bool,
    pub mode: String,
    pub spotify_account_id: Option<String>,
    pub youtube_account_id: Option<String>,
    pub pending_youtube_account_id: Option<String>,
    pub pending_youtube_display_name: Option<String>,
    pub account_revisions: BTreeMap<String, u64>,
    pub connection_seen: bool,
    pub message: String,
    pub transport: String,
    pub credential_storage: String,
    pub youtube_login_page: Option<String>,
    pub youtube_reconnect_required: bool,
}

struct SessionState {
    persisted: PersistedSession,
    mode: String,
    child: Option<Child>,
    last_activity: Instant,
    background_opt_in: bool,
}

struct ActiveJobGuard<'a> {
    session: &'a ManagedProviderSession,
}

struct SecretBytes(Vec<u8>);

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Clone)]
struct SecretString(String);

impl Drop for SecretString {
    fn drop(&mut self) {
        overwrite_string(&mut self.0);
    }
}

fn overwrite_string(value: &mut String) {
    // NUL bytes remain valid UTF-8; overwrite secret data before releasing its allocation.
    unsafe {
        value.as_mut_vec().fill(0);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeWorkerRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    prototype_enabled: Option<bool>,
    provider: &'a str,
    operation: &'a str,
    args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cookie_header: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<Value>,
    deadline_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    internal_auth_refresh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_auth: Option<CachedSpotifyAuth<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    creation_request_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedSpotifyAuth<'a> {
    access_token: &'a str,
    expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpotifyAuthRefreshRequest<'a> {
    provider: &'static str,
    operation: &'static str,
    args: Value,
    cookie_header: &'a str,
    deadline_ms: u64,
    internal_auth_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YoutubeSession {
    origin: String,
    cookie_header: String,
    account_id: String,
    display_name: String,
    client_name: String,
    client_version: String,
    api_key: String,
    visitor_data: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_null_string",
        skip_serializing_if = "Option::is_none"
    )]
    data_sync_id: Option<String>,
    auth_user: String,
    #[serde(default)]
    page_context_captured: bool,
    hl: String,
    gl: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct YoutubePageContext {
    visitor_data: String,
    data_sync_id: Option<String>,
    auth_user: String,
    logged_in: Option<bool>,
}

fn deserialize_optional_non_null_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

impl Drop for YoutubeSession {
    fn drop(&mut self) {
        overwrite_string(&mut self.cookie_header);
    }
}

#[derive(Clone)]
struct YoutubeHandoffPairing {
    port: u16,
    nonce_hash: [u8; 32],
    expires_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeHandoffPairingCode {
    pub code: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedProviderAccount {
    pub provider: String,
    pub account_id: String,
    pub display_name: String,
    pub revision: u64,
}

impl Drop for ActiveJobGuard<'_> {
    fn drop(&mut self) {
        self.session.active_jobs.fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut state) = self.session.state.lock() {
            state.last_activity = Instant::now();
        }
    }
}

pub struct ManagedProviderSession {
    app: AppHandle,
    bridge: Arc<BrowserBridge>,
    data_dir: PathBuf,
    enabled: bool,
    native_enabled: bool,
    native_requested: bool,
    active_jobs: AtomicUsize,
    native_jobs: Arc<Semaphore>,
    native_generation: AtomicU64,
    native_shutdown: AtomicBool,
    native_control: Mutex<()>,
    verification_intent_accounts: verification_creation_intent::AccountFences,
    native_cancellations: Mutex<Vec<Weak<AtomicBool>>>,
    native_cookie_header: Mutex<Option<SecretString>>,
    spotify_auth_cache: SpotifyAuthCache,
    spotify_auth_salt: [u8; 32],
    native_spotify_pending: Mutex<Option<(u64, SecretString)>>,
    native_youtube_session: Mutex<Option<SecretString>>,
    native_youtube_pending: Mutex<Option<(SecretString, String, String)>>,
    native_credential_storage: Mutex<String>,
    youtube_handoff_pairing: Mutex<Option<YoutubeHandoffPairing>>,
    state: Mutex<SessionState>,
}

impl ManagedProviderSession {
    fn start_native_login(&self, provider: &str) -> Result<ManagedProviderSessionStatus, String> {
        let (label, login_url, profile) = match provider {
            "spotify" => (
                NATIVE_LOGIN_LABEL,
                NATIVE_LOGIN_URL,
                "native-profile-spotify",
            ),
            "youtube" => (
                NATIVE_YOUTUBE_LOGIN_LABEL,
                NATIVE_YOUTUBE_LOGIN_URL,
                "native-profile-youtube",
            ),
            _ => return Err("Choose Spotify or YouTube for the isolated sign-in".into()),
        };
        let _control = self
            .native_control
            .lock()
            .map_err(|_| "Managed provider session is unavailable")?;
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        self.persist_native_transport_opt_in()?;
        signal_native_stop(&self.native_generation, &self.native_cancellations);
        self.spotify_auth_cache.invalidate_now();
        if provider == "spotify" {
            if let Ok(mut pending) = self.native_spotify_pending.lock() {
                *pending = None;
            }
        }
        if provider == "youtube" {
            if let Ok(mut pending) = self.native_youtube_pending.lock() {
                *pending = None;
            }
        }
        let packaged_worker = crate::release_resource_root(&self.app)
            .ok()
            .map(|path| path.join("native-provider-session"));
        stage_native_worker_snapshot(
            &self.data_dir.join("managed-provider-session/native-worker"),
            packaged_worker.as_deref(),
            NATIVE_WORKER_FILES,
        )?;
        let data_directory = self
            .data_dir
            .join(format!("managed-provider-session/{profile}"));
        secure_create_dir(&data_directory)?;
        let login_url =
            url::Url::parse(login_url).map_err(|_| "Provider sign-in URL is invalid")?;
        if self.app.get_webview_window(label).is_none() {
            let builder =
                WebviewWindowBuilder::new(&self.app, label, WebviewUrl::External(login_url))
                    .title(if provider == "spotify" {
                        "Sign in to Spotify — Burrata"
                    } else {
                        "Sign in to YouTube — Burrata"
                    })
                    .inner_size(980.0, 760.0);
            // On Linux, WebKitGTK's incognito mode ignores its WebContext. Keep each provider
            // non-incognito inside its own app-owned 0700 data directory so pre-confirmation
            // sign-in survives restarts without sharing provider cookies.
            #[cfg(target_os = "linux")]
            let builder = builder.incognito(false);
            #[cfg(not(target_os = "linux"))]
            let builder = builder.incognito(true);
            builder
                .data_directory(data_directory)
                .build()
                .map_err(|_| "Could not open the isolated provider sign-in window".to_owned())?;
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed provider session is unavailable")?;
        state.mode = format!("native-login:{provider}");
        drop(state);
        Ok(self.status())
    }

    fn persist_native_transport_opt_in(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed provider session is unavailable")?;
        if state.persisted.native_transport_opt_in {
            return Ok(());
        }
        state.persisted.native_transport_opt_in = true;
        if let Err(error) = persist_session(&self.data_dir, &state.persisted) {
            state.persisted.native_transport_opt_in = false;
            return Err(error);
        }
        Ok(())
    }

    fn load_native_cookie(&self) -> Result<SecretString, String> {
        if let Some(cookie) = self
            .native_cookie_header
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .clone()
        {
            return Ok(cookie);
        }
        let session_id = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .session_id
            .clone();
        let username = format!("spotify:{session_id}");
        let entry = keyring::Entry::new(NATIVE_KEYRING_SERVICE, &username).map_err(|_| {
            "Secure Spotify session storage is unavailable; sign in again after restart".to_owned()
        })?;
        let cookie = SecretString(entry.get_password().map_err(|_| {
            "Spotify session is not confirmed; sign in and confirm the account".to_owned()
        })?);
        if !valid_spotify_cookie_header(&cookie.0) {
            return Err("Stored Spotify session is invalid; sign in again".into());
        }
        self.persist_native_transport_opt_in()?;
        *self
            .native_cookie_header
            .lock()
            .map_err(|_| "Provider session is unavailable")? = Some(cookie.clone());
        *self
            .native_credential_storage
            .lock()
            .map_err(|_| "Provider session is unavailable")? = "os-keyring".into();
        Ok(cookie)
    }

    fn persist_native_cookie(&self, cookie: &str) -> Result<(), String> {
        let session_id = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .session_id
            .clone();
        let username = format!("spotify:{session_id}");
        let entry = keyring::Entry::new(NATIVE_KEYRING_SERVICE, &username)
            .map_err(|_| SPOTIFY_STORAGE_ERROR.to_owned())?;
        let saved = verify_persisted_spotify_cookie(
            cookie,
            || entry.set_password(cookie),
            || entry.get_password(),
        )?;
        self.spotify_auth_cache.invalidate_now();
        *self
            .native_cookie_header
            .lock()
            .map_err(|_| "Provider session is unavailable")? = Some(saved);
        *self
            .native_credential_storage
            .lock()
            .map_err(|_| "Provider session is unavailable")? = "os-keyring".into();
        Ok(())
    }

    fn load_native_youtube_session(&self) -> Result<SecretString, String> {
        if let Some(session) = self
            .native_youtube_session
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .clone()
        {
            return Ok(session);
        }
        let session_id = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .session_id
            .clone();
        let entry =
            keyring::Entry::new(NATIVE_KEYRING_SERVICE, &format!("youtube:{session_id}"))
                .map_err(|_| "Secure YouTube session storage is unavailable; reconnect YouTube")?;
        let session = SecretString(
            entry
                .get_password()
                .map_err(|_| "YouTube session is not confirmed; use the session handoff again")?,
        );
        let parsed = serde_json::from_str::<YoutubeSession>(&session.0)
            .map_err(|_| "Stored YouTube session is invalid; reconnect YouTube")?;
        valid_youtube_session(&parsed)?;
        self.persist_native_transport_opt_in()?;
        *self
            .native_youtube_session
            .lock()
            .map_err(|_| "Provider session is unavailable")? = Some(session.clone());
        if let Ok(mut storage) = self.native_credential_storage.lock() {
            *storage = "os-keyring".into();
        }
        Ok(session)
    }

    fn persist_native_youtube_session(&self, session: &YoutubeSession) -> Result<(), String> {
        let serialized = SecretString(
            serde_json::to_string(session)
                .map_err(|_| "Could not prepare YouTube session storage")?,
        );
        self.persist_native_youtube_credential(&serialized.0)
    }

    fn persist_native_youtube_credential(&self, credential: &str) -> Result<(), String> {
        let session_id = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .session_id
            .clone();
        let entry = keyring::Entry::new(NATIVE_KEYRING_SERVICE, &format!("youtube:{session_id}"))
            .map_err(|_| YOUTUBE_STORAGE_ERROR.to_owned())?;
        let saved = verify_persisted_provider_credential(
            credential,
            YOUTUBE_STORAGE_ERROR,
            || entry.set_password(credential),
            || entry.get_password(),
        )?;
        *self
            .native_youtube_session
            .lock()
            .map_err(|_| "YouTube session storage is unavailable")? = Some(saved);
        *self
            .native_credential_storage
            .lock()
            .map_err(|_| "Provider session is unavailable")? = "os-keyring".into();
        Ok(())
    }

    pub fn create_youtube_handoff_pairing_code(&self) -> Result<YoutubeHandoffPairingCode, String> {
        if !self.native_enabled {
            return Err("Native YouTube handoff is unavailable in this build".into());
        }
        let _control = self
            .native_control
            .lock()
            .map_err(|_| "YouTube session pairing is unavailable")?;
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        let bridge_url = url::Url::parse(&self.bridge.base_url())
            .map_err(|_| "The app loopback endpoint is unavailable")?;
        if bridge_url.scheme() != "http"
            || bridge_url.host_str() != Some("127.0.0.1")
            || bridge_url.path() != "/"
            || bridge_url.query().is_some()
            || bridge_url.fragment().is_some()
            || !bridge_url.username().is_empty()
            || bridge_url.password().is_some()
        {
            return Err("The app loopback endpoint is unavailable".into());
        }
        let port = bridge_url
            .port()
            .ok_or("The app loopback endpoint is unavailable")?;
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let code = format!("{port}-{nonce}");
        let nonce_hash: [u8; 32] = Sha256::digest(nonce.as_bytes()).into();
        *self
            .youtube_handoff_pairing
            .lock()
            .map_err(|_| "YouTube session pairing is unavailable")? = Some(YoutubeHandoffPairing {
            port,
            nonce_hash,
            expires_at: Instant::now() + Duration::from_secs(120),
        });
        Ok(YoutubeHandoffPairingCode {
            code,
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339(),
        })
    }

    pub async fn accept_youtube_handoff(
        &self,
        pairing_code: &str,
        value: Value,
    ) -> Result<ConfirmedProviderAccount, String> {
        if !self.native_enabled {
            return Err("Native YouTube session handoff is unavailable in this build".into());
        }
        let (port_text, nonce) = pairing_code
            .split_once('-')
            .ok_or("YouTube session pairing code is invalid")?;
        if port_text.is_empty()
            || port_text.len() > 5
            || !port_text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("YouTube session pairing code is invalid".into());
        }
        let port = port_text
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or("YouTube session pairing code is invalid")?;
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("YouTube session pairing code is invalid".into());
        }
        let supplied_hash: [u8; 32] = Sha256::digest(nonce.as_bytes()).into();
        let generation;
        {
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "YouTube session pairing is unavailable")?;
            if !native_runtime_open(&self.native_shutdown) {
                return Err("Managed provider session is shutting down".into());
            }
            generation = self.native_generation.load(Ordering::SeqCst);
            let mut pairing = self
                .youtube_handoff_pairing
                .lock()
                .map_err(|_| "YouTube session pairing is unavailable")?;
            let expected = pairing
                .as_ref()
                .ok_or("Create a new YouTube pairing code in Burrata first")?;
            if Instant::now() >= expected.expires_at {
                *pairing = None;
                return Err("YouTube pairing code expired; create a new code in Burrata".into());
            }
            let diff = expected
                .nonce_hash
                .iter()
                .zip(supplied_hash.iter())
                .fold(0_u8, |diff, (left, right)| diff | (left ^ right));
            if expected.port != port || diff != 0 {
                return Err("YouTube pairing code does not match this Burrata instance".into());
            }
            // A matching code authorizes exactly one attempt, even if provider identity fails.
            *pairing = None;
        }
        let mut session: YoutubeSession = serde_json::from_value(value)
            .map_err(|_| "YouTube session handoff is malformed".to_owned())?;
        valid_youtube_session(&session)?;
        let credential = SecretString(
            serde_json::to_string(&session)
                .map_err(|_| "Could not prepare YouTube session verification".to_owned())?,
        );
        let result = self
            .run_native_job(
                "youtube",
                "status",
                serde_json::json!({}),
                credential,
                None,
                generation,
                None,
            )
            .await?;
        if self.native_generation.load(Ordering::SeqCst) != generation {
            return Err("YouTube session handoff was cancelled".into());
        }
        let account_id = result
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|id| valid_youtube_channel_id(id))
            .ok_or("YouTube could not verify the active channel identity")?
            .to_owned();
        let display_name = result
            .get("displayName")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty() && name.len() <= 256)
            .unwrap_or(&session.display_name)
            .to_owned();
        if session.account_id.starts_with("UC") && session.account_id != account_id {
            return Err(
                "The active YouTube account changed. Use the session handoff again.".into(),
            );
        }
        session.account_id = account_id.clone();
        session.display_name = display_name.clone();
        let _control = self
            .native_control
            .lock()
            .map_err(|_| "Managed provider session is unavailable")?;
        if !native_runtime_open(&self.native_shutdown)
            || self.native_generation.load(Ordering::SeqCst) != generation
        {
            return Err("YouTube session handoff was cancelled".into());
        }
        self.persist_native_youtube_session(&session)?;
        // A fresh handoff can replace credentials for the same channel identity.
        self.invalidate_playback_session("youtube");
        self.record_account("youtube", &account_id)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed provider session is unavailable")?;
        clear_youtube_reconnect_required(&mut state.persisted);
        activate_native_session(&mut state);
        persist_session(&self.data_dir, &state.persisted)?;
        let revision = state.persisted.accounts["youtube"].revision;
        drop(state);
        Ok(ConfirmedProviderAccount {
            provider: "youtube".into(),
            account_id,
            display_name,
            revision,
        })
    }

    async fn run_native_job(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        credential: SecretString,
        cancel: Option<Arc<AtomicBool>>,
        generation: u64,
        confirmed_account: Option<String>,
    ) -> Result<Value, String> {
        self.run_native_job_with_creation_receipt(
            provider,
            operation,
            args,
            credential,
            cancel,
            generation,
            confirmed_account,
            None,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn run_native_job_with_creation_receipt(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        credential: SecretString,
        cancel: Option<Arc<AtomicBool>>,
        generation: u64,
        confirmed_account: Option<String>,
        creation_request_id: Option<&str>,
    ) -> Result<Value, NativeJobFailure> {
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if !native_operation_allowed(provider, operation) {
            return Err("This native provider operation is not available".into());
        }
        let permit = tokio::select! {
            permit = self.native_jobs.clone().acquire_owned() => permit.map_err(|_| "Provider worker is unavailable")?,
            _ = wait_cancel(cancel.clone()) => return Err("Provider request was cancelled".into()),
            _ = wait_generation(&self.native_generation, generation) => return Err("Provider request was cancelled".into()),
        };
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        let stop_cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .native_cancellations
                .lock()
                .map_err(|_| "Provider worker is unavailable")?;
            active.retain(|job| job.strong_count() > 0);
            active.push(Arc::downgrade(&stop_cancel));
        }
        if !native_runtime_open(&self.native_shutdown)
            || self.native_generation.load(Ordering::SeqCst) != generation
        {
            stop_cancel.store(true, Ordering::SeqCst);
        }
        // Content-addressed snapshots are immutable: a running job keeps using the exact
        // worker tree it started with even when a newer app build stages refreshed code.
        let packaged_worker = crate::release_resource_root(&self.app)
            .ok()
            .map(|path| path.join("native-provider-session"));
        let worker_root = stage_native_worker_snapshot(
            &self.data_dir.join("managed-provider-session/native-worker"),
            packaged_worker.as_deref(),
            NATIVE_WORKER_FILES,
        )?;
        let worker_script = worker_root.join(match provider {
            "youtube" => "scripts/youtube-session-worker.mjs",
            _ => "scripts/provider-session-worker.mjs",
        });
        if !worker_script.is_file() {
            drop(permit);
            return Err(
                "Provider worker assets are unavailable; reconnect the provider session".into(),
            );
        }
        let node = tools::bundled_executable("node")
            .unwrap_or_else(|| tools::executable("system:node", "node"));
        let (cookie_header, session) = if provider == "youtube" {
            let session: Value = serde_json::from_str(&credential.0)
                .map_err(|_| "Stored YouTube session is invalid".to_owned())?;
            (None, Some(session))
        } else {
            (Some(credential.0.as_str()), None)
        };
        let auth_entry = if provider == "spotify" {
            if let Some(account_id) = confirmed_account.as_deref() {
                let fingerprint = spotify_credential_fingerprint(
                    &self.spotify_auth_salt,
                    credential.0.as_bytes(),
                );
                let scope = AuthScope {
                    generation,
                    account_id: account_id.to_owned(),
                    credential_fingerprint: fingerprint,
                };
                let refresh_node = node.clone();
                let refresh_script = worker_script.clone();
                let refresh_cookie = credential.clone();
                let refresh_stop = stop_cancel.clone();
                let refresh = self
                    .spotify_auth_cache
                    .get_or_refresh(
                        scope,
                        cancel.clone(),
                        stop_cancel.clone(),
                        move || async move {
                            let request = serde_json::to_vec(&SpotifyAuthRefreshRequest {
                                provider: "spotify",
                                operation: "status",
                                args: serde_json::json!({}),
                                cookie_header: refresh_cookie.0.as_str(),
                                deadline_ms: 20_000,
                                internal_auth_refresh: true,
                            })
                            .map_err(|_| "Could not prepare Spotify auth request".to_owned())?;
                            run_private_auth_refresh(
                                refresh_node,
                                refresh_script,
                                request,
                                refresh_stop,
                                Duration::from_secs(20),
                            )
                            .await
                        },
                    )
                    .await?;
                Some(refresh)
            } else {
                None
            }
        } else {
            None
        };
        let cached_auth = if let Some(refresh) = auth_entry.as_ref() {
            let access_token = refresh
                .access_token
                .expose()
                .map_err(|_| "Spotify access token was invalid".to_owned())?;
            Some(CachedSpotifyAuth {
                access_token,
                expires_at: refresh.expires_at_unix_ms,
            })
        } else {
            None
        };
        if creation_request_id.is_some()
            && cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            return Err("Verification intent account changed before worker dispatch".into());
        }
        let request = serde_json::to_vec(&NativeWorkerRequest {
            creation_request_id,
            prototype_enabled: (provider == "youtube").then_some(true),
            provider,
            operation,
            args,
            cookie_header,
            session,
            deadline_ms: 20_000,
            internal_auth_refresh: None,
            cached_auth,
        })
        .map_err(|_| "Could not prepare the provider request".to_owned())?;
        let (result, auth_rejected) = if auth_entry.is_some() || creation_request_id.is_some() {
            let typed_result = run_worker_process_typed(
                node,
                worker_script,
                SecretBytes(request),
                cancel,
                stop_cancel,
                Duration::from_secs(21),
            )
            .await;
            let rejected = typed_result
                .as_ref()
                .err()
                .is_some_and(NativeWorkerFailure::invalidates_token);
            (typed_result.map_err(NativeJobFailure::Worker), rejected)
        } else {
            (
                run_worker_process(
                    node,
                    worker_script,
                    SecretBytes(request),
                    cancel,
                    stop_cancel,
                    Duration::from_secs(21),
                )
                .await
                .map_err(NativeJobFailure::Legacy),
                false,
            )
        };
        if let Some(used_auth) = auth_entry.as_ref() {
            let wrong_account = result.as_ref().ok().is_some_and(|value| {
                confirmed_account.as_deref().is_some_and(|expected| {
                    value.get("accountId").and_then(Value::as_str) != Some(expected)
                })
            });
            if auth_rejected || wrong_account {
                self.spotify_auth_cache.invalidate_if_current(used_auth);
            }
        }
        drop(permit);
        if creation_request_id.is_none()
            && (operation == "createVerificationPlaylist"
                || (provider == "spotify" && operation == "applySyncPreview"))
            && result.as_ref().err().is_some_and(|error| {
                let error = error.to_string();
                [
                    "cancelled",
                    "deadline",
                    "exceeded its size limit",
                    "invalid data",
                    "worker could not finish",
                    "worker failed",
                    "Could not read Spotify worker response",
                    "Could not read YouTube worker response",
                    "timed out",
                    "timeout",
                ]
                .iter()
                .any(|marker| {
                    error
                        .to_ascii_lowercase()
                        .contains(&marker.to_ascii_lowercase())
                })
            })
        {
            return Err(if operation == "createVerificationPlaylist" {
                "The verification playlist may have been created. Inspect the provider; DO NOT RETRY creation.".into()
            } else {
                "Spotify may have received playlist changes. Refresh the playlist before trying again".into()
            });
        }
        result
    }

    pub fn new(app: AppHandle, bridge: Arc<BrowserBridge>, data_dir: PathBuf) -> Arc<Self> {
        let enabled = cfg!(target_os = "linux")
            && std::env::var(MANAGED_SESSION_ENABLED).is_ok_and(|value| value == "1");
        let persisted = load_or_create_session(&data_dir);
        let native_environment_override = std::env::var(NATIVE_SESSION_ENABLED).ok();
        let native_requested = native_transport_requested(
            native_environment_override.as_deref(),
            persisted.native_transport_opt_in,
        );
        // Linux YouTube and Spotify use separate app-owned persistent WebKitGTK profiles. The
        // successful-confirm path clears browsing data, so this only preserves pre-confirmation
        // login state across restarts; confirmed sessions live in the OS keyring. WKWebView stays
        // ephemeral until its persistent-profile isolation is verified.
        // Windows WebView2 only guarantees incognito mode on newer runtimes, so this prototype
        // stays disabled there.
        let native_enabled = native_session_enabled(
            native_requested,
            cfg!(any(target_os = "linux", target_os = "macos")),
        );
        let background_opt_in = persisted.background_opt_in;
        let has_confirmed_account = persisted_has_confirmed_provider(&persisted.accounts);
        let native_stopped = persisted.native_stopped;
        let mut spotify_auth_salt = [0_u8; 32];
        spotify_auth_salt[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        spotify_auth_salt[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let session = Arc::new(Self {
            app,
            bridge,
            data_dir,
            enabled,
            native_enabled,
            native_requested,
            active_jobs: AtomicUsize::new(0),
            native_jobs: Arc::new(Semaphore::new(2)),
            native_generation: AtomicU64::new(0),
            native_shutdown: AtomicBool::new(false),
            native_control: Mutex::new(()),
            verification_intent_accounts: Default::default(),
            native_cancellations: Mutex::new(Vec::new()),
            native_cookie_header: Mutex::new(None),
            spotify_auth_cache: SpotifyAuthCache::default(),
            spotify_auth_salt,
            native_spotify_pending: Mutex::new(None),
            native_youtube_session: Mutex::new(None),
            native_youtube_pending: Mutex::new(None),
            native_credential_storage: Mutex::new("not-confirmed".into()),
            youtube_handoff_pairing: Mutex::new(None),
            state: Mutex::new(SessionState {
                persisted,
                mode: native_initial_mode(
                    native_enabled,
                    background_opt_in,
                    has_confirmed_account,
                    native_stopped,
                )
                .into(),
                child: None,
                last_activity: Instant::now(),
                background_opt_in,
            }),
        });
        Self::start_idle_monitor(&session);
        session
    }

    fn start_idle_monitor(session: &Arc<Self>) {
        let weak = Arc::downgrade(session);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(session) = weak.upgrade() else { break };
                let mut state = match session.state.lock() {
                    Ok(state) => state,
                    Err(_) => continue,
                };
                if state.mode == "background"
                    && session.active_jobs.load(Ordering::SeqCst) == 0
                    && state.last_activity.elapsed() >= IDLE_TIMEOUT
                {
                    stop_child(&mut state.child);
                    state.mode = "idle".into();
                    let _ = session.bridge.set_managed_session(None);
                }
            }
        });
    }

    pub fn status(&self) -> ManagedProviderSessionStatus {
        let state = self.state.lock().unwrap();
        let connection_seen = !self.native_enabled
            && self
                .bridge
                .managed_session_seen(&state.persisted.session_id);
        let accounts = &state.persisted.accounts;
        ManagedProviderSessionStatus {
            enabled: self.native_enabled || self.enabled,
            running: if self.native_enabled {
                native_session_ready(&state.mode)
                    || self.app.get_webview_window(NATIVE_LOGIN_LABEL).is_some()
                    || self
                        .app
                        .get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL)
                        .is_some()
            } else {
                state.child.is_some()
            },
            mode: state.mode.clone(),
            youtube_login_page: self.app.get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL).map(
                |window| {
                    window
                        .url()
                        .map(|url| youtube_login_page_classification(&url).to_owned())
                        .unwrap_or_else(|_| "unavailable".into())
                },
            ),
            youtube_reconnect_required: state.persisted.youtube_reconnect_required,
            spotify_account_id: accounts.get("spotify").map(|a| a.account_id.clone()),
            youtube_account_id: accounts.get("youtube").map(|a| a.account_id.clone()),
            pending_youtube_account_id: self
                .native_youtube_pending
                .lock()
                .ok()
                .and_then(|pending| pending.as_ref().map(|(_, account, _)| account.clone())),
            pending_youtube_display_name: self
                .native_youtube_pending
                .lock()
                .ok()
                .and_then(|pending| pending.as_ref().map(|(_, _, name)| name.clone())),
            account_revisions: accounts
                .iter()
                .map(|(provider, account)| (provider.clone(), account.revision))
                .collect(),
            connection_seen,
            message: if self.native_enabled && state.persisted.youtube_reconnect_required {
                "YouTube needs to reconnect. Sign in again and confirm your channel; the saved session is kept until confirmation succeeds.".into()
            } else if self.native_requested && !self.native_enabled {
                "The native provider session is unavailable on this platform because its WebView cannot guarantee isolated sign-in storage.".into()
            } else if self.native_enabled {
                if !accounts.is_empty() && native_session_ready(&state.mode) {
                    "Native provider access is active. YouTube session handoff and bounded HTTP workers run only when needed.".into()
                } else if !accounts.is_empty() && state.mode == "stopped" {
                    "The managed provider session is stopped. Choose Enable read-only access to resume it.".into()
                } else if !accounts.is_empty() && state.mode == NATIVE_MODE_CONFIRMED {
                    "A provider account was previously confirmed. Choose Enable read-only access to resume it.".into()
                } else if state.mode == "native-login:spotify" {
                    "Finish signing in, then confirm the Spotify account to enable read-only access.".into()
                } else if state.mode == "native-login:youtube" {
                    "Finish signing in to YouTube, then check the account and confirm the displayed channel.".into()
                } else if state.mode == "native-awaiting-youtube-confirmation" {
                    "YouTube verified a channel. Review the displayed name and channel ID, then confirm to enable read-only access.".into()
                } else if !accounts.is_empty() {
                    "The provider account needs to be confirmed before read-only access is enabled."
                        .into()
                } else {
                    "Sign in to Spotify or YouTube in its isolated app window, then verify and confirm the account. You can also use the one-time YouTube companion handoff. This transport is read-only.".into()
                }
            } else if !self.enabled {
                "The experimental managed provider session is disabled in this build.".into()
            } else if accounts.is_empty() {
                "Sign in interactively to Spotify or YouTube in the isolated browser profile, then confirm the account.".into()
            } else if state.mode == "background" {
                "The isolated provider session is running in the background.".into()
            } else if state.mode == "idle" {
                "The background browser is idle; it will restart when a provider job needs it."
                    .into()
            } else {
                "A signed-in provider account was confirmed in the managed browser profile.".into()
            },
            transport: if self.native_enabled {
                "native-webview-http".into()
            } else if self.native_requested {
                "native-unsupported".into()
            } else if self.enabled {
                "isolated-chromium".into()
            } else {
                "companion".into()
            },
            credential_storage: self
                .native_credential_storage
                .lock()
                .map(|v| v.clone())
                .unwrap_or_else(|_| "unavailable".into()),
        }
    }

    pub fn start_login(&self, provider: &str) -> Result<ManagedProviderSessionStatus, String> {
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if self.native_enabled {
            return self.start_native_login(provider);
        }
        let _control = self
            .native_control
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if !self.enabled {
            return Err(
                "The experimental managed provider session is disabled in this build".into(),
            );
        }
        if !matches!(provider, "spotify" | "youtube") {
            return Err("Choose Spotify or YouTube for the isolated sign-in".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        stop_child(&mut state.child);
        persist_session(&self.data_dir, &state.persisted)?;
        self.prepare_files(&state.persisted)?;
        self.bridge
            .set_managed_session(Some(state.persisted.session_id.clone()))?;
        state.child = match self.launch(&state.persisted.session_id, provider, false) {
            Ok(child) => Some(child),
            Err(error) => {
                let _ = self.bridge.set_managed_session(None);
                return Err(error);
            }
        };
        state.mode = format!("login:{provider}");
        state.last_activity = Instant::now();
        drop(state);
        Ok(self.status())
    }

    pub fn start_background(&self) -> Result<ManagedProviderSessionStatus, String> {
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if self.native_enabled {
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if !native_runtime_open(&self.native_shutdown) {
                return Err("Managed provider session is shutting down".into());
            }
            let state = self
                .state
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if state.persisted.accounts.is_empty() {
                return Err("Confirm a provider account first".into());
            }
            drop(state);
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            state.mode = NATIVE_MODE_READY.into();
            state.background_opt_in = true;
            state.persisted.background_opt_in = true;
            state.persisted.native_transport_opt_in = true;
            state.persisted.native_stopped = false;
            persist_session(&self.data_dir, &state.persisted)?;
            drop(state);
            return Ok(self.status());
        }
        let _control = self
            .native_control
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if !self.enabled {
            return Err(
                "The experimental managed provider session is disabled in this build".into(),
            );
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        if state.persisted.accounts.is_empty() {
            return Err(
                "Sign in and confirm the provider account in the managed browser profile first"
                    .into(),
            );
        }
        if state.mode == "background" && state.child.is_some() {
            drop(state);
            return Ok(self.status());
        }
        stop_child(&mut state.child);
        self.prepare_files(&state.persisted)?;
        self.bridge
            .set_managed_session(Some(state.persisted.session_id.clone()))?;
        state.child = match self.launch(&state.persisted.session_id, "spotify", true) {
            Ok(child) => Some(child),
            Err(error) => {
                let _ = self.bridge.set_managed_session(None);
                return Err(error);
            }
        };
        state.mode = "background".into();
        state.background_opt_in = true;
        state.persisted.background_opt_in = true;
        persist_session(&self.data_dir, &state.persisted)?;
        state.last_activity = Instant::now();
        drop(state);
        Ok(self.status())
    }

    pub fn stop(&self) -> Result<ManagedProviderSessionStatus, String> {
        if self.native_enabled {
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if !native_runtime_open(&self.native_shutdown) {
                return Err("Managed provider session is shutting down".into());
            }
            signal_native_stop(&self.native_generation, &self.native_cancellations);
            self.spotify_auth_cache.invalidate_now();
            if let Ok(mut cookie) = self.native_cookie_header.lock() {
                *cookie = None;
            }
            if let Ok(mut pending) = self.native_spotify_pending.lock() {
                *pending = None;
            }
            if let Ok(mut session) = self.native_youtube_session.lock() {
                *session = None;
            }
            if let Ok(mut pending) = self.native_youtube_pending.lock() {
                *pending = None;
            }
            if let Ok(mut storage) = self.native_credential_storage.lock() {
                if storage.as_str() == "memory-only" {
                    *storage = "cleared-by-stop".into();
                }
            }
            if let Some(window) = self.app.get_webview_window(NATIVE_LOGIN_LABEL) {
                let _ = window.clear_all_browsing_data();
                let _ = window.close();
            }
            if let Some(window) = self.app.get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL) {
                let _ = window.clear_all_browsing_data();
                let _ = window.close();
            }
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            state.mode = "stopped".into();
            state.background_opt_in = false;
            state.persisted.background_opt_in = false;
            state.persisted.native_stopped = true;
            persist_session(&self.data_dir, &state.persisted)?;
            drop(state);
            return Ok(self.status());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        stop_child(&mut state.child);
        state.mode = "stopped".into();
        state.background_opt_in = false;
        state.persisted.background_opt_in = false;
        persist_session(&self.data_dir, &state.persisted)?;
        // Pending requests keep their original discriminator, so switching to the regular
        // companion cannot deliver managed-session results to the wrong caller.
        self.bridge.set_managed_session(None)?;
        drop(state);
        Ok(self.status())
    }

    pub async fn confirm_account(
        &self,
        provider: &str,
    ) -> Result<ManagedProviderSessionStatus, String> {
        if self.native_enabled {
            if !native_runtime_open(&self.native_shutdown) {
                return Err("Managed provider session is shutting down".into());
            }
            if provider == "youtube" {
                return self.confirm_native_youtube_account().await;
            }
            if provider != "spotify" {
                return Err("Choose Spotify or YouTube to confirm the account".into());
            }
            let generation = self.native_generation.load(Ordering::SeqCst);
            let window = self
                .app
                .get_webview_window(NATIVE_LOGIN_LABEL)
                .ok_or("Open the isolated Spotify sign-in window first")?;
            let retry_cookie = {
                let mut pending = self
                    .native_spotify_pending
                    .lock()
                    .map_err(|_| "Spotify confirmation is unavailable")?;
                pending_spotify_cookie(&mut pending, generation)
            };
            let has_retry_cookie = retry_cookie.is_some();
            let cookie_header = if let Some(cookie) = retry_cookie {
                cookie
            } else {
                let mut cookies = native_login_cookies(window.clone()).await?;
                let captured = spotify_cookie_header(&cookies).map(SecretString);
                for (_, value) in &mut cookies {
                    overwrite_string(value);
                }
                let captured = captured?;
                if self.native_generation.load(Ordering::SeqCst) != generation {
                    return Err("Spotify account confirmation was cancelled".into());
                }
                captured
            };
            if self.native_generation.load(Ordering::SeqCst) != generation {
                return Err("Spotify account confirmation was cancelled".into());
            }
            if !has_retry_cookie {
                let _control = self
                    .native_control
                    .lock()
                    .map_err(|_| "Spotify confirmation is unavailable")?;
                if self.native_generation.load(Ordering::SeqCst) != generation {
                    return Err("Spotify account confirmation was cancelled".into());
                }
                let mut pending = self
                    .native_spotify_pending
                    .lock()
                    .map_err(|_| "Spotify confirmation is unavailable")?;
                pending_spotify_cookie_store(&mut pending, generation, cookie_header.clone());
            }
            // Preserve the captured session across bounded navigation/blanking failures.
            // Only explicit account confirmation reaches this point with validated cookies.
            blank_native_spotify_login_page(&window, &self.native_generation, generation).await?;
            if self.native_generation.load(Ordering::SeqCst) != generation {
                return Err("Spotify account confirmation was cancelled".into());
            }
            let result = self
                .run_native_job(
                    "spotify",
                    "status",
                    serde_json::json!({}),
                    cookie_header.clone(),
                    None,
                    generation,
                    None,
                )
                .await?;
            if self.native_generation.load(Ordering::SeqCst) != generation {
                return Err("Spotify account confirmation was cancelled".into());
            }
            let account_id = result
                .get("accountId")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty() && v.len() <= 256)
                .ok_or("Spotify did not return a verified account identity")?
                .to_owned();
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if self.native_generation.load(Ordering::SeqCst) != generation {
                return Err("Spotify account confirmation was cancelled".into());
            }
            self.persist_native_cookie(&cookie_header.0)?;
            self.record_account("spotify", &account_id)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            activate_native_session(&mut state);
            persist_session(&self.data_dir, &state.persisted)?;
            drop(state);
            if let Ok(mut pending) = self.native_spotify_pending.lock() {
                *pending = None;
            }
            if let Some(window) = self.app.get_webview_window(NATIVE_LOGIN_LABEL) {
                let _ = window.clear_all_browsing_data();
                let _ = window.close();
            }
            return Ok(self.status());
        }
        if !self.enabled {
            return Err(
                "The experimental managed provider session is disabled in this build".into(),
            );
        }
        if !matches!(provider, "spotify" | "youtube") {
            return Err("Choose Spotify or YouTube to confirm the account".into());
        }
        self.provider_operation(provider, "status", serde_json::json!({}))
            .await?;
        Ok(self.status())
    }

    pub async fn return_youtube_login_to_music(
        &self,
    ) -> Result<ManagedProviderSessionStatus, String> {
        let generation = self.native_generation.load(Ordering::SeqCst);
        self.return_youtube_login_to_music_for_generation(generation)
            .await
    }

    async fn return_youtube_login_to_music_for_generation(
        &self,
        expected_generation: u64,
    ) -> Result<ManagedProviderSessionStatus, String> {
        if !self.native_enabled {
            return Err("The isolated YouTube sign-in window is unavailable".into());
        }
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        if self.native_generation.load(Ordering::SeqCst) != expected_generation {
            return Err("YouTube sign-in page changed during confirmation; try again".into());
        }
        #[cfg(target_os = "linux")]
        {
            let window = self
                .app
                .get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL)
                .ok_or("Open the isolated YouTube sign-in window first")?;
            let current_url = window
                .url()
                .map_err(|_| "Could not verify the YouTube sign-in page")?;
            if valid_youtube_music_page_url(&current_url) {
                return Ok(self.status());
            }
            if !valid_youtube_google_accounts_url(&current_url) {
                return Err(
                    "Return to Music is available only from the owned Google sign-in page".into(),
                );
            }

            // The explicit MCP control is only offered for the owned Google sign-in window.
            // Require the same bounded authentication-cookie family used by account handoff
            // before returning to Music; never expose the cookie to status or tool output.
            let cookie = native_youtube_cookie(window.clone()).await?;
            drop(cookie);
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if self.native_generation.load(Ordering::SeqCst) != expected_generation
                || !native_runtime_open(&self.native_shutdown)
            {
                return Err("YouTube sign-in page changed during confirmation; try again".into());
            }
            let current_url = window
                .url()
                .map_err(|_| "Could not verify the YouTube sign-in page")?;
            validate_youtube_login_return(&current_url, true)?;
            let music_url = url::Url::parse("https://music.youtube.com/")
                .expect("fixed YouTube Music origin is valid");
            window
                .navigate(music_url)
                .map_err(|_| "Could not return the owned YouTube window to Music")?;
            return Ok(self.status());
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(
                "Returning from Google sign-in is supported only in the Linux native session"
                    .into(),
            )
        }
    }

    async fn confirm_native_youtube_account(&self) -> Result<ManagedProviderSessionStatus, String> {
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        let generation = self.native_generation.load(Ordering::SeqCst);
        let pending = {
            self.native_youtube_pending
                .lock()
                .map_err(|_| "YouTube confirmation is unavailable")?
                .clone()
        };
        if let Some((credential, account_id, _display_name)) = pending {
            let verified = self
                .run_native_job(
                    "youtube",
                    "status",
                    serde_json::json!({}),
                    credential.clone(),
                    None,
                    generation,
                    None,
                )
                .await?;
            let live_account = verified
                .get("accountId")
                .and_then(Value::as_str)
                .filter(|id| valid_youtube_channel_id(id))
                .ok_or("YouTube could not reverify the selected channel")?;
            if live_account != account_id {
                return Err(
                    "The active YouTube account changed. Check the channel and confirm again."
                        .into(),
                );
            }
            let _control = self
                .native_control
                .lock()
                .map_err(|_| "Managed provider session is unavailable")?;
            if !native_runtime_open(&self.native_shutdown)
                || self.native_generation.load(Ordering::SeqCst) != generation
            {
                return Err("YouTube account confirmation was cancelled".into());
            }
            self.persist_native_youtube_credential(&credential.0)?;
            self.record_account("youtube", &account_id)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Provider session is unavailable")?;
            clear_youtube_reconnect_required(&mut state.persisted);
            activate_native_session(&mut state);
            persist_session(&self.data_dir, &state.persisted)?;
            drop(state);
            *self
                .native_youtube_pending
                .lock()
                .map_err(|_| "YouTube confirmation is unavailable")? = None;
            if let Some(window) = self.app.get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL) {
                let _ = window.clear_all_browsing_data();
                let _ = window.close();
            }
            return Ok(self.status());
        }

        let window = self
            .app
            .get_webview_window(NATIVE_YOUTUBE_LOGIN_LABEL)
            .ok_or("Open the isolated YouTube sign-in window first")?;
        if self.native_generation.load(Ordering::SeqCst) != generation {
            return Err("YouTube account confirmation was cancelled".into());
        }
        #[cfg(target_os = "linux")]
        {
            let current_url = window
                .url()
                .map_err(|_| "Could not verify the YouTube sign-in page")?;
            if youtube_confirm_requires_music_return(&current_url)? {
                // Runs only after explicit Continue. This helper rechecks generation and the
                // owned Google origin, requires an auth cookie, and navigates to fixed Music.
                // It does not infer or confirm an account identity.
                return self
                    .return_youtube_login_to_music_for_generation(generation)
                    .await;
            }
        }
        #[cfg(target_os = "linux")]
        let page_context =
            Some(native_youtube_page_context(&window, &self.native_generation, generation).await?);
        #[cfg(not(target_os = "linux"))]
        let page_context: Option<YoutubePageContext> = None;
        let cookie = native_youtube_cookie(window).await?;
        if self.native_generation.load(Ordering::SeqCst) != generation {
            return Err("YouTube account confirmation was cancelled".into());
        }
        let mut session = YoutubeSession {
            origin: "https://music.youtube.com".into(),
            cookie_header: cookie.0.clone(),
            account_id: "@pending".into(),
            display_name: "YouTube account".into(),
            client_name: "WEB".into(),
            client_version: "0.0".into(),
            api_key: String::new(),
            visitor_data: page_context
                .as_ref()
                .map(|context| context.visitor_data.clone())
                .unwrap_or_default(),
            data_sync_id: page_context
                .as_ref()
                .and_then(|context| context.data_sync_id.clone()),
            auth_user: page_context
                .as_ref()
                .map(|context| context.auth_user.clone())
                .unwrap_or_else(|| "0".into()),
            page_context_captured: page_context.is_some(),
            hl: "en".into(),
            gl: "US".into(),
        };
        if self.native_generation.load(Ordering::SeqCst) != generation {
            return Err("YouTube account confirmation was cancelled".into());
        }
        let credential = SecretString(
            serde_json::to_string(&session)
                .map_err(|_| "Could not prepare YouTube session verification")?,
        );
        let result = self
            .run_native_job(
                "youtube",
                "bootstrap",
                serde_json::json!({}),
                credential,
                None,
                generation,
                None,
            )
            .await?;
        if self.native_generation.load(Ordering::SeqCst) != generation {
            return Err("YouTube account confirmation was cancelled".into());
        }
        let account_id = result
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|id| valid_youtube_channel_id(id))
            .ok_or("YouTube did not return a verified channel identity")?
            .to_owned();
        let display_name = result
            .get("displayName")
            .and_then(Value::as_str)
            .filter(|name| {
                !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
            })
            .ok_or("YouTube did not return a verified account name")?
            .to_owned();
        session.account_id = account_id.clone();
        session.display_name = display_name;
        session.client_name = result
            .get("clientName")
            .and_then(Value::as_str)
            .unwrap_or("WEB")
            .to_owned();
        session.client_version = result
            .get("clientVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        session.api_key = result
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        session.visitor_data = result
            .get("visitorData")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        session.data_sync_id = result
            .get("dataSyncId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        session.auth_user = result
            .get("authUser")
            .and_then(Value::as_str)
            .unwrap_or("0")
            .to_owned();
        session.hl = result
            .get("hl")
            .and_then(Value::as_str)
            .unwrap_or("en")
            .to_owned();
        session.gl = result
            .get("gl")
            .and_then(Value::as_str)
            .unwrap_or("US")
            .to_owned();
        valid_youtube_session(&session)?;
        let pending = SecretString(
            serde_json::to_string(&session)
                .map_err(|_| "Could not prepare YouTube session confirmation")?,
        );
        *self
            .native_youtube_pending
            .lock()
            .map_err(|_| "YouTube confirmation is unavailable")? =
            Some((pending, account_id, session.display_name.clone()));
        if let Ok(mut state) = self.state.lock() {
            state.mode = "native-awaiting-youtube-confirmation".into();
        }
        Ok(self.status())
    }

    fn prepare_files(&self, session: &PersistedSession) -> Result<(), String> {
        let root = self.data_dir.join("managed-provider-session");
        let profile = root.join("profile");
        let extension = root.join("extension");
        secure_create_dir(&root)?;
        secure_create_dir(&profile)?;
        secure_create_dir(&extension)?;
        secure_create_dir(&root.join("config"))?;
        let source_extension = self.extension_source()?;
        let manifest_text = fs::read_to_string(source_extension.join("manifest.chromium.json"))
            .or_else(|_| fs::read_to_string(source_extension.join("manifest.json")))
            .map_err(|_| "Managed browser extension manifest is unavailable".to_owned())?;
        let mut manifest: Value =
            serde_json::from_str(&manifest_text).map_err(|e| e.to_string())?;
        let required_sites = [
            "https://open.spotify.com/*",
            "https://www.youtube.com/*",
            "https://music.youtube.com/*",
        ];
        if !manifest
            .get("host_permissions")
            .is_some_and(Value::is_array)
        {
            manifest["host_permissions"] = Value::Array(Vec::new());
        }
        let required = manifest
            .get_mut("host_permissions")
            .and_then(Value::as_array_mut)
            .ok_or("Managed extension manifest has no host permissions list")?;
        for site in required_sites {
            if !required.iter().any(|host| host.as_str() == Some(site)) {
                required.push(Value::String(site.into()));
            }
        }
        if let Some(optional) = manifest
            .get_mut("optional_host_permissions")
            .and_then(Value::as_array_mut)
        {
            optional.retain(|host| !required_sites.contains(&host.as_str().unwrap_or("")));
        }
        fs::write(
            extension.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for (name, embedded) in EXTENSION_FILES {
            let source = source_extension.join(name);
            let bytes = fs::read(&source).unwrap_or_else(|_| embedded.as_bytes().to_vec());
            fs::write(extension.join(name), bytes).map_err(|e| e.to_string())?;
        }
        stage_extension_icons(&source_extension, &extension)?;
        fs::write(
            extension.join("managed-session.json"),
            serde_json::to_vec(&serde_json::json!({"sessionId":session.session_id}))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        secure_file(&extension.join("managed-session.json"))?;

        let resource_dir = crate::release_resource_root(&self.app)?;
        let host = managed_host_executable(
            &resource_dir,
            &root,
            &std::env::current_exe().map_err(|error| error.to_string())?,
            cfg!(debug_assertions),
        )?;
        let extension_id = crate::install_integration::chromium_extension_id()?;
        let host_manifest = serde_json::json!({
            "name":"org.burrata.browser",
            "description":"Burrata isolated provider session bridge",
            "path":host,
            "type":"stdio",
            "allowed_origins":[format!("chrome-extension://{extension_id}/")]
        });
        #[cfg(target_os = "linux")]
        for host_dir in managed_host_manifest_dirs(&root) {
            secure_create_dir(&host_dir)?;
            let target = host_dir.join("org.burrata.browser.json");
            fs::write(
                &target,
                serde_json::to_vec(&host_manifest).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            secure_file(&target)?;
        }
        Ok(())
    }

    fn extension_source(&self) -> Result<PathBuf, String> {
        let packaged = crate::release_resource_root(&self.app)?.join("provider-extension");
        if packaged.join("manifest.chromium.json").is_file() {
            return Ok(packaged);
        }
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../extension");
        if source.join("manifest.chromium.json").is_file() {
            Ok(source)
        } else {
            Err("Managed browser companion assets were not packaged".into())
        }
    }

    fn launch(&self, _session_id: &str, provider: &str, headless: bool) -> Result<Child, String> {
        let root = self.data_dir.join("managed-provider-session");
        let profile = root.join("profile");
        let extension = root.join("extension");
        let config_home = root.join("config");
        let binary = std::env::var_os(MANAGED_BROWSER_BINARY).map(PathBuf::from);
        let candidates = binary
            .into_iter()
            .chain(["chromium", "google-chrome", "google-chrome-stable"].map(PathBuf::from));
        let mut last_error = None;
        for candidate in candidates {
            match spawn_browser(
                &candidate,
                &profile,
                &extension,
                &config_home,
                &self.data_dir,
                provider,
                headless,
            ) {
                Ok(child) => return Ok(child),
                Err(error) => last_error = Some(error),
            }
        }
        Err(format!(
            "Could not start the managed Chromium session: {}",
            last_error.map_or_else(|| "no browser executable found".into(), |e| e.to_string())
        ))
    }
}

// Data-only worker receipt; absence/malformed transport output is always unknown.
fn bounded_creation_receipt(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.len() != 5
        || ![
            "version",
            "requestId",
            "mutationDispatched",
            "phase",
            "playlistId",
        ]
        .iter()
        .all(|key| object.contains_key(*key))
        || value.get("version")?.as_u64()? != 1
    {
        return None;
    }
    let request_id = value.get("requestId")?.as_str()?;
    if request_id.len() != 32
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let dispatched = value.get("mutationDispatched")?;
    if !dispatched.is_boolean() && !dispatched.is_null() {
        return None;
    }
    let phase = value.get("phase")?.as_str()?;
    if !matches!(
        phase,
        "worker_start"
            | "token_exchange"
            | "spotify_clock"
            | "totp_registry"
            | "hash_registry"
            | "account_profile"
            | "playlist_create"
            | "provider_request"
            | "bootstrap"
            | "bootstrap_config"
            | "innertube"
            | "account_identity"
            | "account_selection"
            | "channel_identity"
            | "playlist_library"
            | "playlist_creation"
            | "playlist_creation_response"
            | "worker_deadline"
    ) {
        return None;
    }
    let playlist = value.get("playlistId")?;
    if !playlist.is_null()
        && !playlist.as_str().is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 80
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    {
        return None;
    }
    if !playlist.is_null() && dispatched.as_bool() != Some(true) {
        return None;
    }
    Some(value.clone())
}

fn bound_creation_receipt(value: &Value, expected_request_id: &str) -> Option<Value> {
    bounded_creation_receipt(value).filter(|receipt| {
        receipt.get("requestId").and_then(Value::as_str) == Some(expected_request_id)
    })
}

async fn run_worker_process(
    node: PathBuf,
    worker: PathBuf,
    request: SecretBytes,
    caller_cancel: Option<Arc<AtomicBool>>,
    stop_cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Value, String> {
    let mut command = TokioCommand::new(node);
    tools::configure_no_console_tokio(&mut command);
    let mut child = command
        .arg(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            "Could not start the isolated Spotify worker; install Node.js or package the runtime"
                .to_owned()
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("Provider worker input is unavailable")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Provider worker output is unavailable")?;
    let result = {
        let operation = async {
            stdin
                .write_all(&request.0)
                .await
                .map_err(|_| "Could not send request to Spotify worker".to_owned())?;
            stdin
                .shutdown()
                .await
                .map_err(|_| "Could not close Spotify worker input".to_owned())?;
            // Explicitly close the pipe so workers waiting for stdin EOF can proceed.
            drop(stdin);
            let mut output = Vec::new();
            stdout
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut output)
                .await
                .map_err(|_| "Could not read Spotify worker response".to_owned())?;
            if output.len() > 2 * 1024 * 1024 {
                return Err("Spotify worker response exceeded its size limit".into());
            }
            let status = child
                .wait()
                .await
                .map_err(|_| "Spotify worker could not finish".to_owned())?;
            if !status.success() {
                return Err("Spotify worker failed".into());
            }
            let response: Value = serde_json::from_slice(&output)
                .map_err(|_| "Spotify worker returned invalid data".to_owned())?;
            if response.get("ok").and_then(Value::as_bool) == Some(true) {
                response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Spotify worker returned no result".into())
            } else {
                let code = response
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .filter(|code| {
                        code.len() <= 64
                            && code
                                .bytes()
                                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                    });
                let message = response
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Spotify request failed");
                Err(code
                    .map(|code| format!("{code}: {message}"))
                    .unwrap_or_else(|| message.to_owned()))
            }
        };
        tokio::pin!(operation);
        tokio::select! {
            result = tokio::time::timeout(timeout, &mut operation) => match result {
                Ok(result) => result,
                Err(_) => Err("Spotify request exceeded its deadline".to_owned()),
            },
            _ = wait_cancel_any(caller_cancel, stop_cancel) => Err("Spotify request was cancelled".into()),
        }
    };
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeWorkerFailureKind {
    SignInRequired,
    VerificationRequired,
    RateLimited,
    RequestTimeout,
    ApiDenied,
    NotFound,
    ProviderChanged,
    CursorResetRequired,
    OutcomeUnknown,
    Cancelled,
    Deadline,
    Other,
}
#[derive(Debug)]
enum NativeJobFailure {
    Legacy(String),
    Worker(NativeWorkerFailure),
}
impl From<String> for NativeJobFailure {
    fn from(message: String) -> Self {
        Self::Legacy(message)
    }
}
impl From<&str> for NativeJobFailure {
    fn from(message: &str) -> Self {
        Self::Legacy(message.to_owned())
    }
}
impl std::fmt::Display for NativeJobFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Legacy(message) => f.write_str(message),
            Self::Worker(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

#[derive(Debug, Clone)]
struct NativeWorkerFailure {
    kind: NativeWorkerFailureKind,
    retry_after_seconds: Option<u64>,
    phase: Option<&'static str>,
    operation_name: Option<&'static str>,
    http_status: Option<u16>,
    transport_reason: Option<NativeTransportFailure>,
    creation_receipt: Option<Value>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeTransportFailure {
    Start,
    Input,
    Output,
    Write,
    Close,
    Read,
    Oversize,
    Wait,
    Exit,
    InvalidData,
    MissingResult,
}
impl NativeWorkerFailure {
    fn invalidates_token(&self) -> bool {
        self.kind == NativeWorkerFailureKind::SignInRequired
    }
    fn from_worker(
        code: Option<&str>,
        phase: Option<&str>,
        operation_name: Option<&str>,
        status: Option<u64>,
        retry_after_seconds: Option<u64>,
    ) -> Self {
        let kind = match code {
            Some("SIGN_IN_REQUIRED") => NativeWorkerFailureKind::SignInRequired,
            Some("VERIFICATION_REQUIRED") => NativeWorkerFailureKind::VerificationRequired,
            Some("RATE_LIMITED") => NativeWorkerFailureKind::RateLimited,
            Some("REQUEST_TIMEOUT") => NativeWorkerFailureKind::RequestTimeout,
            Some("API_DENIED") => NativeWorkerFailureKind::ApiDenied,
            Some("TRACK_NOT_FOUND" | "ARTIST_NOT_FOUND" | "ALBUM_NOT_FOUND") => {
                NativeWorkerFailureKind::NotFound
            }
            Some("PROVIDER_CHANGED") => NativeWorkerFailureKind::ProviderChanged,
            Some("CURSOR_RESET_REQUIRED") => NativeWorkerFailureKind::CursorResetRequired,
            Some("OUTCOME_UNKNOWN") => NativeWorkerFailureKind::OutcomeUnknown,
            _ => NativeWorkerFailureKind::Other,
        };
        Self {
            kind,
            retry_after_seconds: if kind == NativeWorkerFailureKind::RateLimited {
                retry_after_seconds.filter(|s| (1..=3600).contains(s))
            } else {
                None
            },
            phase: Self::safe_phase(phase),
            operation_name: Self::safe_graphql_operation(phase, operation_name),
            http_status: status
                .and_then(|s| u16::try_from(s).ok())
                .filter(|s| (100..=599).contains(s)),
            transport_reason: None,
            creation_receipt: None,
        }
    }
    fn safe_phase(phase: Option<&str>) -> Option<&'static str> {
        match phase {
            Some("worker_start") => Some("worker_start"),
            Some("token_exchange") => Some("token_exchange"),
            Some("spotify_clock") => Some("spotify_clock"),
            Some("totp_registry") => Some("totp_registry"),
            Some("hash_registry") => Some("hash_registry"),
            Some("account_profile") => Some("account_profile"),
            Some("owner_profile") => Some("owner_profile"),
            Some("playlist_create") => Some("playlist_create"),
            Some("catalog_rest") => Some("catalog_rest"),
            Some("catalog_graphql") => Some("catalog_graphql"),
            Some("provider_request") => Some("provider_request"),
            _ => None,
        }
    }
    fn safe_graphql_operation(
        phase: Option<&str>,
        operation_name: Option<&str>,
    ) -> Option<&'static str> {
        if phase != Some("catalog_graphql") {
            return None;
        }
        match operation_name {
            Some("queryArtistMinimal") => Some("queryArtistMinimal"),
            Some("queryArtistDiscographyAll") => Some("queryArtistDiscographyAll"),
            Some("getAlbum") => Some("getAlbum"),
            Some("queryAlbumTracks") => Some("queryAlbumTracks"),
            _ => None,
        }
    }
    fn cancelled() -> Self {
        Self {
            kind: NativeWorkerFailureKind::Cancelled,
            retry_after_seconds: None,
            phase: None,
            operation_name: None,
            http_status: None,
            transport_reason: None,
            creation_receipt: None,
        }
    }
    fn deadline() -> Self {
        Self {
            kind: NativeWorkerFailureKind::Deadline,
            retry_after_seconds: None,
            phase: None,
            operation_name: None,
            http_status: None,
            transport_reason: None,
            creation_receipt: None,
        }
    }
    fn transport(reason: NativeTransportFailure) -> Self {
        Self {
            kind: NativeWorkerFailureKind::Other,
            retry_after_seconds: None,
            phase: None,
            operation_name: None,
            http_status: None,
            transport_reason: Some(reason),
            creation_receipt: None,
        }
    }
}
impl std::fmt::Display for NativeWorkerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (code, message) = match self.kind {
            NativeWorkerFailureKind::SignInRequired => (
                "SIGN_IN_REQUIRED",
                "Spotify authentication is unavailable. Sign in again and retry.",
            ),
            NativeWorkerFailureKind::VerificationRequired => (
                "VERIFICATION_REQUIRED",
                "Spotify requires verification. Complete it in the browser and retry.",
            ),
            NativeWorkerFailureKind::RateLimited => (
                "RATE_LIMITED",
                "Spotify is rate limiting requests. Try again shortly.",
            ),
            NativeWorkerFailureKind::RequestTimeout => (
                "REQUEST_TIMEOUT",
                "Spotify request timed out. Try again shortly.",
            ),
            NativeWorkerFailureKind::Deadline => (
                "REQUEST_TIMEOUT",
                "Spotify request deadline expired. Try again shortly.",
            ),
            NativeWorkerFailureKind::ApiDenied => ("API_DENIED", "Spotify denied this request."),
            NativeWorkerFailureKind::NotFound => (
                "TRACK_NOT_FOUND",
                "Spotify does not expose this catalog item through the current API.",
            ),
            NativeWorkerFailureKind::ProviderChanged => (
                "PROVIDER_CHANGED",
                "Spotify returned a response that did not match the request.",
            ),
            NativeWorkerFailureKind::CursorResetRequired => (
                "CURSOR_RESET_REQUIRED",
                "Spotify playlist listing changed. Refresh the profile and retry.",
            ),
            NativeWorkerFailureKind::OutcomeUnknown => ("OUTCOME_UNKNOWN", "Spotify playlist creation or changes may have succeeded. Do not retry the operation; inspect the provider."),
            NativeWorkerFailureKind::Cancelled => {
                return f.write_str("Provider request was cancelled")
            }
            NativeWorkerFailureKind::Other => {
                return f.write_str(match self.transport_reason {
                    Some(NativeTransportFailure::Start) => "Could not start the isolated Spotify worker; install Node.js or package the runtime",
                    Some(NativeTransportFailure::Input) => "Provider worker input is unavailable",
                    Some(NativeTransportFailure::Output) => "Provider worker output is unavailable",
                    Some(NativeTransportFailure::Write) => "Could not send request to Spotify worker",
                    Some(NativeTransportFailure::Close) => "Could not close Spotify worker input",
                    Some(NativeTransportFailure::Read) => "Could not read Spotify worker response",
                    Some(NativeTransportFailure::Oversize) => "Spotify worker response exceeded its size limit",
                    Some(NativeTransportFailure::Wait) => "Spotify worker could not finish",
                    Some(NativeTransportFailure::Exit) => "Spotify worker failed",
                    Some(NativeTransportFailure::InvalidData) => "Spotify worker returned invalid data",
                    Some(NativeTransportFailure::MissingResult) => "Spotify worker returned no result",
                    None => "Spotify worker failed",
                });
            }
        };
        if self.kind == NativeWorkerFailureKind::RateLimited {
            if let Some(seconds) = self.retry_after_seconds {
                write!(
                    f,
                    "{code}: Spotify is rate limiting requests. Try again in {seconds} seconds."
                )?;
            } else {
                write!(f, "{code}: {message}")?;
            }
        } else {
            write!(f, "{code}: {message}")?;
        }
        if let Some(phase) = self.phase {
            write!(f, " (phase={phase}")?;
            if let Some(operation_name) = self.operation_name {
                write!(f, ", operation={operation_name}")?;
            }
            if let Some(status) = self.http_status {
                write!(f, ", HTTP {status}")?;
            }
            f.write_str(")")?;
        } else if let Some(status) = self.http_status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}

async fn run_worker_process_typed(
    node: PathBuf,
    worker: PathBuf,
    request: SecretBytes,
    caller_cancel: Option<Arc<AtomicBool>>,
    stop_cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Value, NativeWorkerFailure> {
    let creation_request_id = serde_json::from_slice::<Value>(&request.0)
        .ok()
        .filter(|job| {
            job.get("operation").and_then(Value::as_str) == Some("createVerificationPlaylist")
        })
        .and_then(|job| {
            job.get("creationRequestId")
                .and_then(Value::as_str)
                .filter(|id| {
                    id.len() == 32
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
                .map(str::to_owned)
        });
    if creation_request_id.is_some()
        && caller_cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    {
        return Err(NativeWorkerFailure::cancelled());
    }
    let mut command = TokioCommand::new(node);
    tools::configure_no_console_tokio(&mut command);
    let mut child = command
        .arg(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::Start))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| NativeWorkerFailure::transport(NativeTransportFailure::Input))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| NativeWorkerFailure::transport(NativeTransportFailure::Output))?;
    let handoff_cancel = caller_cancel.clone();
    let result = {
        let operation = async {
            if creation_request_id.is_some()
                && handoff_cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                return Err(NativeWorkerFailure::cancelled());
            }
            stdin
                .write_all(&request.0)
                .await
                .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::Write))?;
            stdin
                .shutdown()
                .await
                .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::Close))?;
            // Explicitly close the pipe so workers waiting for stdin EOF can proceed.
            drop(stdin);
            let mut output = Vec::new();
            stdout
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut output)
                .await
                .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::Read))?;
            if output.len() > 2 * 1024 * 1024 {
                return Err(NativeWorkerFailure::transport(
                    NativeTransportFailure::Oversize,
                ));
            }
            let status = child
                .wait()
                .await
                .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::Wait))?;
            if !status.success() {
                return Err(NativeWorkerFailure::transport(NativeTransportFailure::Exit));
            }
            let response: Value = serde_json::from_slice(&output)
                .map_err(|_| NativeWorkerFailure::transport(NativeTransportFailure::InvalidData))?;
            if response.get("ok").and_then(Value::as_bool) == Some(true) {
                response.get("result").cloned().ok_or_else(|| {
                    NativeWorkerFailure::transport(NativeTransportFailure::MissingResult)
                })
            } else {
                let code = response.pointer("/error/code").and_then(Value::as_str);
                let phase = response.pointer("/error/phase").and_then(Value::as_str);
                let operation_name = response
                    .pointer("/error/operationName")
                    .and_then(Value::as_str);
                let status = response
                    .pointer("/error/httpStatus")
                    .and_then(Value::as_u64);
                let retry = response
                    .pointer("/error/retryAfterSeconds")
                    .and_then(Value::as_u64);
                let mut failure =
                    NativeWorkerFailure::from_worker(code, phase, operation_name, status, retry);
                failure.creation_receipt =
                    response
                        .pointer("/error/creationReceipt")
                        .and_then(|receipt| {
                            creation_request_id
                                .as_deref()
                                .and_then(|expected| bound_creation_receipt(receipt, expected))
                        });
                Err(failure)
            }
        };
        tokio::pin!(operation);
        tokio::select! {
            result = tokio::time::timeout(timeout, &mut operation) => match result {
                Ok(result) => result,
                Err(_) => Err(NativeWorkerFailure::deadline()),
            },
            _ = wait_cancel_any(caller_cancel, stop_cancel) => Err(NativeWorkerFailure::cancelled()),
        }
    };
    result
}

fn spawn_browser(
    binary: &Path,
    profile: &Path,
    extension: &Path,
    config_home: &Path,
    app_data: &Path,
    provider: &str,
    headless: bool,
) -> std::io::Result<Child> {
    let mut command = Command::new(binary);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--load-extension={}", extension.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-mode")
        .arg("--new-window");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(target_os = "linux")]
    {
        command
            .env("XDG_CONFIG_HOME", config_home)
            .env("XDG_DATA_HOME", config_home.join("../data"))
            .env("XDG_CACHE_HOME", config_home.join("../cache"))
            .env("XDG_STATE_HOME", config_home.join("../state"))
            .env("BURRATA_APP_DATA", app_data);
    }
    if headless {
        command.arg("--headless=new");
    }
    command.arg(match provider {
        "youtube" => "https://music.youtube.com/",
        _ => "https://open.spotify.com/",
    });
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

async fn wait_cancel(cancel: Option<Arc<AtomicBool>>) {
    let Some(cancel) = cancel else {
        return std::future::pending::<()>().await;
    };
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn wait_generation(generation: &AtomicU64, expected: u64) {
    loop {
        if generation.load(Ordering::SeqCst) != expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn wait_cancel_any(caller: Option<Arc<AtomicBool>>, stop: Arc<AtomicBool>) {
    tokio::select! {
        _ = wait_cancel(caller) => {},
        _ = wait_cancel(Some(stop)) => {},
    }
}

fn signal_native_stop(generation: &AtomicU64, jobs: &Mutex<Vec<Weak<AtomicBool>>>) {
    generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut active) = jobs.lock() {
        for job in active.iter().filter_map(Weak::upgrade) {
            job.store(true, Ordering::SeqCst);
        }
        active.retain(|job| job.strong_count() > 0);
    }
}

async fn native_login_cookies(window: WebviewWindow) -> Result<Vec<(String, String)>, String> {
    #[cfg(target_os = "windows")]
    let result = tauri::async_runtime::spawn_blocking(move || read_native_login_cookies(window))
        .await
        .map_err(|_| "Could not read cookies from the isolated Spotify sign-in window")??;
    #[cfg(not(target_os = "windows"))]
    let result = read_native_login_cookies(window)?;
    Ok(result)
}

fn valid_native_spotify_capture_url(url: &url::Url) -> bool {
    if url.as_str() == "about:blank" {
        return true;
    }
    url.scheme() == "https"
        && url.host_str() == Some("open.spotify.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn valid_native_spotify_accounts_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("accounts.spotify.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn pending_spotify_cookie(
    pending: &mut Option<(u64, SecretString)>,
    generation: u64,
) -> Option<SecretString> {
    if pending
        .as_ref()
        .is_some_and(|(stored_generation, _)| *stored_generation != generation)
    {
        *pending = None;
    }
    pending.as_ref().map(|(_, cookie)| cookie.clone())
}

fn pending_spotify_cookie_store(
    pending: &mut Option<(u64, SecretString)>,
    generation: u64,
    cookie: SecretString,
) -> SecretString {
    if let Some((stored_generation, stored_cookie)) = pending.as_ref() {
        if *stored_generation == generation {
            return stored_cookie.clone();
        }
    }
    *pending = Some((generation, cookie.clone()));
    cookie
}

async fn blank_native_spotify_login_page(
    window: &WebviewWindow,
    generation: &AtomicU64,
    expected_generation: u64,
) -> Result<(), String> {
    if window.label() != NATIVE_LOGIN_LABEL {
        return Err("Open the isolated Spotify sign-in window first".into());
    }
    if generation.load(Ordering::SeqCst) != expected_generation {
        return Err("Spotify account confirmation was cancelled".into());
    }
    let current_url = window
        .url()
        .map_err(|_| "Could not verify the Spotify sign-in page")?;
    if valid_native_spotify_accounts_url(&current_url) {
        // Confirmation already captured a valid Spotify authentication cookie. Return only
        // this owned sign-in window to the fixed player origin before freezing its page.
        // Recheck the origin in-page to reject a navigation race.
        if generation.load(Ordering::SeqCst) != expected_generation {
            return Err("Spotify account confirmation was cancelled".into());
        }
        window
            .eval("if (window.top === window && location.origin === 'https://accounts.spotify.com') { window.location.replace('https://open.spotify.com/'); }")
            .map_err(|_| "Could not return the Spotify sign-in page to the player")?;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if generation.load(Ordering::SeqCst) != expected_generation {
                    return Err("Spotify account confirmation was cancelled".to_owned());
                }
                let url = window
                    .url()
                    .map_err(|_| "Could not verify the Spotify sign-in page".to_owned())?;
                if valid_native_spotify_capture_url(&url) {
                    return Ok(());
                }
                if !valid_native_spotify_accounts_url(&url) {
                    return Err("Spotify sign-in navigated outside its owned origins".to_owned());
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        })
        .await
        .map_err(|_| "Spotify sign-in page did not return to the player in time")??;
    } else if !valid_native_spotify_capture_url(&current_url) {
        return Err("Finish signing in on Spotify before confirming the account".into());
    }
    let current_url = window
        .url()
        .map_err(|_| "Could not verify the Spotify sign-in page")?;
    if !valid_native_spotify_capture_url(&current_url) {
        return Err("Spotify sign-in page changed during confirmation; try again".into());
    }
    if current_url.as_str() != "about:blank" {
        // Recheck origin inside the page at execution time in case a navigation won the race
        // between `url()` above and this queued WebView evaluation.
        window
            .eval("if (window.top === window && location.origin === 'https://open.spotify.com') { window.stop(); window.location.replace('about:blank'); }")
            .map_err(|_| "Could not blank the Spotify sign-in page")?;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if generation.load(Ordering::SeqCst) != expected_generation {
                    return Err("Spotify account confirmation was cancelled".to_owned());
                }
                let url = window
                    .url()
                    .map_err(|_| "Could not verify the Spotify sign-in page".to_owned())?;
                if url.as_str() == "about:blank" {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "Spotify sign-in page did not blank in time")??;
    }
    if generation.load(Ordering::SeqCst) != expected_generation {
        return Err("Spotify account confirmation was cancelled".into());
    }
    Ok(())
}

async fn native_youtube_cookie(window: WebviewWindow) -> Result<SecretString, String> {
    #[cfg(target_os = "windows")]
    let rows = tauri::async_runtime::spawn_blocking(move || read_native_youtube_cookies(window))
        .await
        .map_err(|_| "Could not read the isolated YouTube sign-in session")??;
    #[cfg(not(target_os = "windows"))]
    let rows = read_native_youtube_cookies(window)?;
    youtube_cookie_from_rows(rows)
}

#[cfg(target_os = "linux")]
async fn native_youtube_page_context(
    window: &WebviewWindow,
    generation: &AtomicU64,
    expected_generation: u64,
) -> Result<YoutubePageContext, String> {
    if generation.load(Ordering::SeqCst) != expected_generation {
        return Err("YouTube account confirmation was cancelled".into());
    }
    if window.label() != NATIVE_YOUTUBE_LOGIN_LABEL {
        return Err("Open the isolated YouTube sign-in window first".into());
    }
    let page_url = window
        .url()
        .map_err(|_| "Could not verify the YouTube sign-in page")?;
    if !valid_youtube_music_page_url(&page_url) {
        return Err("Finish signing in on music.youtube.com before confirming YouTube".into());
    }

    const SCRIPT: &str = r#"(()=>{
      if (location.origin !== 'https://music.youtube.com' || window.top !== window) return null;
      const c = window.yt && window.yt.config_;
      if (!c || typeof c !== 'object') return null;
      return JSON.stringify({
        visitorData: typeof c.VISITOR_DATA === 'string' ? c.VISITOR_DATA : null,
        dataSyncId: typeof c.DATASYNC_ID === 'string' ? c.DATASYNC_ID : null,
        authUser: typeof c.SESSION_INDEX === 'string' || Number.isInteger(c.SESSION_INDEX) ? String(c.SESSION_INDEX) : null,
        loggedIn: typeof c.LOGGED_IN === 'boolean' ? c.LOGGED_IN : null
      });
    })()"#;

    let (sender, receiver) = tokio::sync::oneshot::channel::<Option<String>>();
    window
        .with_webview(move |platform| {
            use webkit2gtk::WebViewExt;
            platform.inner().run_javascript(
                SCRIPT,
                None::<&webkit2gtk::gio::Cancellable>,
                move |result| {
                    let text = result
                        .ok()
                        .and_then(|result| result.js_value())
                        .map(|value| value.to_string());
                    let _ = sender.send(text.filter(|text| text.len() <= 2_048));
                },
            );
        })
        .map_err(|_| "Could not read YouTube sign-in page context")?;

    let raw = tokio::time::timeout(Duration::from_secs(2), receiver)
        .await
        .map_err(|_| "YouTube sign-in page context timed out; wait for Music to finish loading")?
        .map_err(|_| "Could not read YouTube sign-in page context")?
        .ok_or("YouTube sign-in page context is incomplete; finish loading Music and try again")?;
    if generation.load(Ordering::SeqCst) != expected_generation {
        return Err("YouTube account confirmation was cancelled".into());
    }
    let current_url = window
        .url()
        .map_err(|_| "Could not verify the YouTube sign-in page")?;
    if !valid_youtube_music_page_url(&current_url) {
        return Err("YouTube sign-in page changed during confirmation; try again".into());
    }
    let context = parse_youtube_page_context(&raw)?;
    if generation.load(Ordering::SeqCst) != expected_generation {
        return Err("YouTube account confirmation was cancelled".into());
    }
    Ok(context)
}

fn valid_youtube_google_accounts_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("accounts.google.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn validate_youtube_login_return(
    url: &url::Url,
    authentication_cookie_present: bool,
) -> Result<(), String> {
    if !valid_youtube_google_accounts_url(url) {
        return Err("Return to Music is available only from the owned Google sign-in page".into());
    }
    if !authentication_cookie_present {
        return Err("Finish signing in to Google before returning to Music".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn youtube_confirm_requires_music_return(url: &url::Url) -> Result<bool, String> {
    if valid_youtube_music_page_url(url) {
        Ok(false)
    } else if valid_youtube_google_accounts_url(url) {
        Ok(true)
    } else {
        Err("Finish signing in on music.youtube.com before confirming YouTube".into())
    }
}

fn youtube_login_page_classification(url: &url::Url) -> &'static str {
    if valid_youtube_music_page_url(url) {
        "music"
    } else if valid_youtube_google_accounts_url(url) {
        "google-accounts"
    } else if url.scheme() == "https" && url.host_str() == Some("www.youtube.com") {
        "youtube-www"
    } else {
        "other"
    }
}

fn valid_youtube_music_page_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("music.youtube.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn parse_youtube_page_context(raw: &str) -> Result<YoutubePageContext, String> {
    if raw.len() > 2_048 {
        return Err("YouTube sign-in page context is too large".into());
    }
    let mut context: YoutubePageContext = serde_json::from_str(raw).map_err(|_| {
        "YouTube sign-in page context is incomplete; finish loading Music and try again"
    })?;
    if context.visitor_data.is_empty()
        || context.visitor_data.len() > 512
        || context.visitor_data.chars().any(char::is_control)
        || context.auth_user.is_empty()
        || context.auth_user.len() > 3
        || !context.auth_user.bytes().all(|byte| byte.is_ascii_digit())
        || context.logged_in == Some(false)
    {
        return Err(
            "YouTube sign-in page context is incomplete; finish signing in and try again".into(),
        );
    }
    if let Some(raw_id) = context.data_sync_id.take() {
        if raw_id.len() > 512 {
            return Err("YouTube sign-in page context is invalid".into());
        }
        let id = raw_id.split("||").next().unwrap_or_default();
        if id.is_empty() && raw_id.contains("||") {
            return Err("YouTube sign-in page context is invalid".into());
        }
        if !id.is_empty()
            && (id.len() > 256
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':')))
        {
            return Err("YouTube sign-in page context is invalid".into());
        }
        context.data_sync_id = (!id.is_empty()).then(|| id.to_owned());
    }
    Ok(context)
}

fn youtube_cookie_from_rows(rows: Vec<(String, String)>) -> Result<SecretString, String> {
    const COOKIE_NAMES: [&str; 15] = [
        "SAPISID",
        "__Secure-1PAPISID",
        "__Secure-3PAPISID",
        "LOGIN_INFO",
        "SID",
        "HSID",
        "SSID",
        "APISID",
        "__Secure-1PSID",
        "__Secure-3PSID",
        "SIDCC",
        "__Secure-1PSIDTS",
        "__Secure-3PSIDTS",
        "__Secure-1PSIDCC",
        "__Secure-3PSIDCC",
    ];
    const AUTH_COOKIE_NAMES: [&str; 3] = ["SAPISID", "__Secure-1PAPISID", "__Secure-3PAPISID"];
    let mut candidates: BTreeMap<String, String> = BTreeMap::new();
    let mut invalid = false;
    for (name, mut value) in rows {
        if COOKIE_NAMES.contains(&name.as_str()) {
            if value.len() < 8
                || value.len() > 4096
                || value.chars().any(|character| {
                    character.is_control() || character.is_whitespace() || character == ';'
                })
                || candidates.contains_key(&name)
            {
                invalid = true;
            } else {
                candidates.insert(name, value.clone());
            }
        }
        overwrite_string(&mut value);
    }
    if invalid {
        for value in candidates.values_mut() {
            overwrite_string(value);
        }
        return Err(
            "YouTube sign-in returned an ambiguous or invalid session; refresh and try again"
                .into(),
        );
    }
    if !AUTH_COOKIE_NAMES
        .iter()
        .any(|name| candidates.contains_key(*name))
    {
        for value in candidates.values_mut() {
            overwrite_string(value);
        }
        return Err("YouTube sign-in is incomplete; sign in and confirm the account".into());
    }
    let mut header = String::new();
    for name in COOKIE_NAMES {
        if let Some(value) = candidates.get(name) {
            if !header.is_empty() {
                header.push_str("; ");
            }
            header.push_str(name);
            header.push('=');
            header.push_str(value);
        }
    }
    for value in candidates.values_mut() {
        overwrite_string(value);
    }
    if header.len() > 16_384 {
        overwrite_string(&mut header);
        return Err("YouTube sign-in session is too large".into());
    }
    Ok(SecretString(header))
}

fn read_native_youtube_cookies(window: WebviewWindow) -> Result<Vec<(String, String)>, String> {
    let url =
        url::Url::parse("https://music.youtube.com/").map_err(|_| "YouTube URL is invalid")?;
    let cookies = window
        .cookies_for_url(url)
        .map_err(|_| "Could not read the isolated YouTube sign-in session")?;
    Ok(cookies
        .into_iter()
        // cookies_for_url scopes this collection to cookies applicable to music.youtube.com.
        // Keep only authentication/session cookies needed by YouTube Music; analytics and
        // unrelated Google cookies never enter the native worker header.
        .filter(|cookie| youtube_cookie_name_allowed(cookie.name()))
        .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
        .collect())
}

fn read_native_login_cookies(window: WebviewWindow) -> Result<Vec<(String, String)>, String> {
    let url = url::Url::parse("https://open.spotify.com/").map_err(|_| "Spotify URL is invalid")?;
    let cookies = window
        .cookies_for_url(url)
        .map_err(|_| "Could not read cookies from the isolated Spotify sign-in window")?;
    Ok(cookies
        .into_iter()
        .filter(|cookie| matches!(cookie.name(), "sp_dc" | "sp_key"))
        .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
        .collect())
}

fn spotify_cookie_header(cookies: &[(String, String)]) -> Result<String, String> {
    let mut sp_dc = None;
    let mut sp_key = None;
    for (name, value) in cookies {
        if value.is_empty() || value.chars().any(|c| matches!(c, ';' | '\r' | '\n' | '\0')) {
            continue;
        }
        match name.as_str() {
            "sp_dc" if sp_dc.is_none() => sp_dc = Some(value.as_str()),
            "sp_key" if sp_key.is_none() => sp_key = Some(value.as_str()),
            "sp_dc" | "sp_key" => {
                return Err("Spotify sign-in returned an ambiguous session".into())
            }
            _ => {}
        }
    }
    let sp_dc = sp_dc.ok_or("Spotify sign-in is incomplete; sign in, then confirm again")?;
    let header = match sp_key {
        Some(key) => format!("sp_dc={sp_dc}; sp_key={key}"),
        None => format!("sp_dc={sp_dc}"),
    };
    if !valid_spotify_cookie_header(&header) {
        return Err("Spotify sign-in did not provide a usable session".into());
    }
    Ok(header)
}

fn valid_spotify_cookie_header(value: &str) -> bool {
    if value.is_empty() || value.len() > 16_384 || value.chars().any(|c| c.is_control()) {
        return false;
    }
    let mut has_dc = false;
    let mut names = BTreeMap::<&str, ()>::new();
    for field in value.split(';') {
        let Some((name, cookie)) = field.trim().split_once('=') else {
            return false;
        };
        if !matches!(name, "sp_dc" | "sp_key")
            || cookie.trim().is_empty()
            || names.insert(name, ()).is_some()
        {
            return false;
        }
        has_dc |= name == "sp_dc";
    }
    has_dc
}

fn native_session_ready(mode: &str) -> bool {
    mode == NATIVE_MODE_READY
}

fn native_transport_requested(environment_override: Option<&str>, persisted_opt_in: bool) -> bool {
    match environment_override {
        Some("1") => true,
        Some(_) => false,
        None => persisted_opt_in,
    }
}

fn native_session_enabled(requested: bool, platform_supported: bool) -> bool {
    requested && platform_supported
}

fn native_initial_mode(
    native_enabled: bool,
    background_opt_in: bool,
    has_confirmed_account: bool,
    native_stopped: bool,
) -> &'static str {
    if native_enabled && background_opt_in {
        NATIVE_MODE_READY
    } else if native_enabled && has_confirmed_account && !native_stopped {
        NATIVE_MODE_CONFIRMED
    } else if background_opt_in {
        "idle"
    } else {
        "stopped"
    }
}

fn persisted_has_confirmed_provider(accounts: &BTreeMap<String, ConfirmedAccount>) -> bool {
    accounts.contains_key("spotify") || accounts.contains_key("youtube")
}

fn is_youtube_reconnect_required_error(error: &str) -> bool {
    error.starts_with("SIGN_IN_REQUIRED: ") || error.starts_with("ACCOUNT_CHANGED: ")
}

fn mark_youtube_reconnect_required(
    session: &mut PersistedSession,
    expected_account: &str,
    current_generation: u64,
    request_generation: u64,
) -> bool {
    if current_generation != request_generation
        || !session
            .accounts
            .get("youtube")
            .is_some_and(|account| account.account_id == expected_account)
    {
        return false;
    }
    session.youtube_reconnect_required = true;
    true
}

fn native_youtube_reconnect_gate(
    provider: &str,
    reconnect_required: bool,
) -> Result<(), &'static str> {
    if provider == "youtube" && reconnect_required {
        Err("YouTube needs to reconnect. Sign in again and confirm your channel.")
    } else {
        Ok(())
    }
}

fn clear_youtube_reconnect_required(session: &mut PersistedSession) {
    session.youtube_reconnect_required = false;
}

fn activate_native_session(state: &mut SessionState) {
    state.mode = NATIVE_MODE_READY.into();
    state.background_opt_in = true;
    state.persisted.background_opt_in = true;
    state.persisted.native_transport_opt_in = true;
    state.persisted.native_stopped = false;
}

fn native_read_operation_allowed(operation: &str) -> bool {
    matches!(
        operation,
        "status"
            | "search"
            | "listPlaylists"
            | "getPlaylist"
            | "getTrack"
            | "getArtist"
            | "getArtistAlbums"
            | "getArtistTracks"
            | "getAlbum"
            | "getAlbumTracks"
    )
}

fn native_operation_allowed(provider: &str, operation: &str) -> bool {
    match provider {
        "spotify" => {
            native_read_operation_allowed(operation)
                || matches!(
                    operation,
                    "applySyncPreview"
                        | "createVerificationPlaylist"
                        | "verifyVerificationPlaylist"
                )
        }
        "youtube" => {
            native_read_operation_allowed(operation)
                || operation == "createVerificationPlaylist"
                || operation == "applyPlaylistEdits"
                || operation == "bootstrap"
        }
        _ => false,
    }
}

fn stage_native_worker_snapshot(
    root: &Path,
    packaged_root: Option<&Path>,
    embedded: &[(&str, &str)],
) -> Result<PathBuf, String> {
    secure_create_dir(root)?;
    let mut assets = Vec::with_capacity(embedded.len());
    let mut digest = Sha256::new();
    for (relative, fallback) in embedded {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Provider worker asset path is invalid".into());
        }
        let bytes = packaged_root
            .map(|base| base.join(path))
            .filter(|candidate| candidate.is_file())
            .and_then(|candidate| fs::read(candidate).ok())
            .unwrap_or_else(|| fallback.as_bytes().to_vec());
        digest.update(relative.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
        assets.push((path.to_path_buf(), bytes));
    }
    let digest = digest.finalize();
    let version = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let target = root.join(format!("sha256-{version}"));
    let matches_snapshot = |directory: &Path| -> bool {
        assets.iter().all(|(relative, bytes)| {
            fs::read(directory.join(relative))
                .is_ok_and(|existing| existing.as_slice() == bytes.as_slice())
        })
    };
    if target.is_dir() {
        if matches_snapshot(&target) {
            return Ok(target);
        }
        return Err(
            "Provider worker snapshot is inconsistent; restart Burrata to refresh it".into(),
        );
    }

    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    secure_create_dir(&staging)?;
    let staged = (|| {
        for (relative, bytes) in &assets {
            let file = staging.join(relative);
            if let Some(parent) = file.parent() {
                secure_create_dir(parent)?;
            }
            fs::write(&file, bytes)
                .map_err(|_| "Could not prepare the isolated provider worker".to_owned())?;
            secure_file(&file)?;
        }
        match fs::rename(&staging, &target) {
            Ok(()) => {}
            Err(_) if target.is_dir() && matches_snapshot(&target) => {
                fs::remove_dir_all(&staging).map_err(|_| {
                    "Could not clean up the isolated provider worker staging directory".to_owned()
                })?;
            }
            Err(_) => return Err("Could not activate the isolated provider worker snapshot".into()),
        }
        Ok::<(), String>(())
    })();
    if staged.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    staged?;
    Ok(target)
}

fn valid_youtube_channel_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("UC") else {
        return false;
    };
    (20..=30).contains(&rest.len())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_youtube_session(session: &YoutubeSession) -> Result<(), String> {
    if !YOUTUBE_LOGIN_ORIGINS.contains(&session.origin.as_str()) {
        return Err("YouTube session origin is invalid".into());
    }
    if !valid_youtube_cookie_header(&session.cookie_header) {
        return Err("YouTube session credential is invalid".into());
    }
    let account_ok = valid_youtube_channel_id(&session.account_id)
        || (session.account_id.starts_with('@')
            && (4..=101).contains(&session.account_id.len())
            && session.account_id[1..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')));
    if !account_ok
        || session.display_name.trim().is_empty()
        || session.display_name.len() > 256
        || session.display_name.chars().any(char::is_control)
        || !session
            .client_name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        || !(2..=32).contains(&session.client_name.len())
        || session.client_version.is_empty()
        || session.client_version.len() > 64
        || !session
            .client_version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || (!session.api_key.is_empty()
            && (session.api_key.len() > 160
                || !session
                    .api_key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')))
        || session.visitor_data.len() > 512
        || session.visitor_data.chars().any(char::is_control)
        || (session.page_context_captured && session.visitor_data.is_empty())
        || session.data_sync_id.as_ref().is_some_and(|value| {
            value.is_empty()
                || value.len() > 256
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
        })
        || session.auth_user.len() > 3
        || !session.auth_user.bytes().all(|byte| byte.is_ascii_digit())
        || !(2..=16).contains(&session.hl.len())
        || !session
            .hl
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || !session.hl.as_bytes()[..2]
            .iter()
            .all(u8::is_ascii_lowercase)
        || session.gl.len() != 2
        || !session.gl.bytes().all(|byte| byte.is_ascii_uppercase())
    {
        return Err("YouTube session context is invalid".into());
    }
    Ok(())
}

fn valid_youtube_cookie_header(header: &str) -> bool {
    const COOKIE_NAMES: [&str; 15] = [
        "SAPISID",
        "__Secure-1PAPISID",
        "__Secure-3PAPISID",
        "LOGIN_INFO",
        "SID",
        "HSID",
        "SSID",
        "APISID",
        "__Secure-1PSID",
        "__Secure-3PSID",
        "SIDCC",
        "__Secure-1PSIDTS",
        "__Secure-3PSIDTS",
        "__Secure-1PSIDCC",
        "__Secure-3PSIDCC",
    ];
    const AUTH_COOKIE_NAMES: [&str; 3] = ["SAPISID", "__Secure-1PAPISID", "__Secure-3PAPISID"];
    if header.is_empty() || header.len() > 16_384 {
        return false;
    }
    let mut seen = [false; COOKIE_NAMES.len()];
    for item in header.split("; ") {
        let Some((name, value)) = item.split_once('=') else {
            return false;
        };
        let Some(index) = COOKIE_NAMES.iter().position(|known| *known == name) else {
            return false;
        };
        if seen[index]
            || value.len() < 8
            || value.len() > 4096
            || value.chars().any(|character| {
                character.is_control() || character.is_whitespace() || character == ';'
            })
        {
            return false;
        }
        seen[index] = true;
    }
    AUTH_COOKIE_NAMES.iter().any(|name| {
        COOKIE_NAMES
            .iter()
            .position(|known| *known == *name)
            .is_some_and(|index| seen[index])
    })
}

fn youtube_cookie_name_allowed(name: &str) -> bool {
    matches!(
        name,
        "SAPISID"
            | "__Secure-1PAPISID"
            | "__Secure-3PAPISID"
            | "LOGIN_INFO"
            | "SID"
            | "HSID"
            | "SSID"
            | "APISID"
            | "__Secure-1PSID"
            | "__Secure-3PSID"
            | "SIDCC"
            | "__Secure-1PSIDTS"
            | "__Secure-3PSIDTS"
            | "__Secure-1PSIDCC"
            | "__Secure-3PSIDCC"
    )
}

fn managed_host_executable(
    resource_dir: &Path,
    session_root: &Path,
    current_exe: &Path,
    debug_build: bool,
) -> Result<PathBuf, String> {
    let bundled = resource_dir
        .join("tools")
        .join(tools::target_triple())
        .join(if cfg!(windows) {
            "burrata-browser-host.exe"
        } else {
            "burrata-browser-host"
        });
    if bundled.is_file() {
        return Ok(bundled);
    }
    #[cfg(target_os = "linux")]
    if debug_build && current_exe.is_file() {
        let launcher = session_root.join("native-host-launcher");
        secure_create_dir(session_root)?;
        let quoted_exe = shell_single_quote(&current_exe.to_string_lossy());
        fs::write(
            &launcher,
            format!("#!/bin/sh\nexec {quoted_exe} --browser-native-host\n"),
        )
        .map_err(|error| error.to_string())?;
        secure_executable(&launcher)?;
        return Ok(launcher);
    }
    Err(format!(
        "Bundled browser companion host is missing: {}",
        bundled.display()
    ))
}

#[cfg(target_os = "linux")]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
fn managed_host_manifest_dirs(session_root: &Path) -> Vec<PathBuf> {
    let config = session_root.join("config");
    vec![
        config.join("chromium/NativeMessagingHosts"),
        config.join("google-chrome/NativeMessagingHosts"),
        config.join("google-chrome-for-testing/NativeMessagingHosts"),
    ]
}

impl ManagedProviderSession {
    /// Explicit one-shot creation. A durable attempt marker prevents duplicate writes after
    /// timeouts or verification failures; reads never call this operation.
    pub async fn create_verification_playlist(&self, provider: &str) -> Result<Value, String> {
        if !matches!(provider, "spotify" | "youtube") {
            return Err("Choose Spotify or YouTube".into());
        }
        if !self.native_enabled {
            return Err("The experimental managed provider session is disabled".into());
        }
        let expected = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .accounts
            .get(provider)
            .map(|account| account.account_id.clone())
            .ok_or("Confirm this provider account before creating a verification playlist")?;
        let status = self
            .provider_operation(provider, "status", serde_json::json!({}))
            .await?;
        if status.get("accountId").and_then(Value::as_str) != Some(expected.as_str()) {
            return Err("Provider account changed. Confirm the active account again.".into());
        }
        let generation = self.native_generation.load(Ordering::SeqCst);
        let (mode, reconnect_required) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "Provider session is unavailable")?;
            (
                state.mode.clone(),
                state.persisted.youtube_reconnect_required,
            )
        };
        if !native_session_ready(&mode) {
            return Err("Confirm the provider account and enable native access first".into());
        }
        native_youtube_reconnect_gate(provider, reconnect_required).map_err(str::to_owned)?;
        let credential = if provider == "spotify" {
            self.load_native_cookie()?
        } else {
            self.load_native_youtube_session()?
        };
        let _legacy_creation_admission = {
            let state = self
                .state
                .lock()
                .map_err(|_| "Provider session unavailable")?;
            let account = state
                .persisted
                .accounts
                .get(provider)
                .ok_or("Provider account unavailable")?;
            self.verification_intent_accounts.register(
                provider,
                &account.account_id,
                account.revision,
                &expected,
                account.revision,
            )?
        };
        let attempt_id = uuid::Uuid::new_v4().simple().to_string();
        let key = format!(
            "{:x}",
            Sha256::digest(format!("{provider}:{expected}").as_bytes())
        );
        let attempt = self
            .data_dir
            .join(format!("verification-playlist-{key}.json"));
        // create_new is the cross-call/process lock. Never remove this marker on error.
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&attempt)
            .map_err(|_| "A verification playlist creation was already attempted for this account; inspect the provider before any further action")?;
        use std::io::Write;
        let attempted = serde_json::json!({"status":"ATTEMPTED","requestId":attempt_id,"mutationDispatched":null});
        file.write_all(
            &serde_json::to_vec(&attempted).map_err(|_| "Cannot encode verification attempt")?,
        )
        .map_err(|_| "Cannot persist verification playlist attempt")?;
        file.sync_all()
            .map_err(|_| "Cannot persist verification playlist attempt")?;
        let created = self.run_native_job_with_creation_receipt(provider, "createVerificationPlaylist", serde_json::json!({"accountId":expected}), credential, None, generation, Some(expected.clone()), Some(&attempt_id)).await
            .map_err(|error| {
                // Only typed worker metadata can establish non-dispatch. Missing output is UNKNOWN.
                let worker = match &error { NativeJobFailure::Worker(worker) => Some(worker), NativeJobFailure::Legacy(_) => None };
                let worker_receipt = worker.and_then(|failure| failure.creation_receipt.clone());
                let not_dispatched = worker_receipt.as_ref().is_some_and(|receipt| receipt.get("mutationDispatched").and_then(Value::as_bool) == Some(false));
                let diagnostic = serde_json::json!({"kind":worker.map(|failure| format!("{:?}", failure.kind)),"httpStatus":worker.and_then(|failure| failure.http_status),"phase":worker.and_then(|failure| failure.phase),"creationReceipt":worker_receipt});
                let failed = serde_json::json!({"status":if not_dispatched {"NOT_DISPATCHED"} else {"UNKNOWN_OUTCOME"},"provider":provider,"requestId":attempt_id,"diagnostic":diagnostic});
                use std::io::{Seek, SeekFrom};
                if let Ok(bytes) = serde_json::to_vec(&failed) {
                    let _ = file.set_len(0).and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ())).and_then(|_| file.write_all(&bytes)).and_then(|_| file.sync_all());
                }
                if not_dispatched {
                    format!("The worker confirmed no playlist mutation was dispatched. The one-shot attempt remains blocked for review; DO NOT RETRY automatically. Diagnostic: {diagnostic}")
                } else {
                    format!("The verification playlist may have been created. Inspect the provider and saved attempt marker; DO NOT RETRY creation. Diagnostic: {diagnostic}")
                }
            })?;
        let id = created
            .get("playlistId")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 256
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .ok_or("Playlist creation returned no bounded playlist ID; inspect the provider")?;
        let url = if provider == "spotify" {
            format!("https://open.spotify.com/playlist/{id}")
        } else {
            format!("https://www.youtube.com/playlist?list={id}")
        };
        let creation_receipt = created
            .get("creationReceipt")
            .and_then(|receipt| bound_creation_receipt(receipt, &attempt_id));
        let valid_creation_receipt = creation_receipt.as_ref().is_some_and(|receipt| {
            receipt.get("mutationDispatched").and_then(Value::as_bool) == Some(true)
                && receipt.get("playlistId").and_then(Value::as_str) == Some(id)
        }) && created.get("accountId").and_then(Value::as_str)
            == Some(expected.as_str())
            && if provider == "spotify" {
                id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            } else {
                id.starts_with("PL") && (12..=80).contains(&id.len())
            };
        let pending = serde_json::json!({"status":"CREATED_PENDING_VERIFICATION","provider":provider,"requestId":attempt_id,"creationReceipt":creation_receipt,"playlistId":id,"url":url});
        file.set_len(0).map_err(|e| e.to_string())?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(&pending).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        if !valid_creation_receipt {
            return Err(format!("Playlist {id} may have been created ({url}); worker receipt validation failed. Inspect the provider; DO NOT RETRY creation."));
        }
        let (operation, args) = if provider == "spotify" {
            (
                "verifyVerificationPlaylist",
                serde_json::json!({"accountId":expected,"playlistId":id}),
            )
        } else {
            (
                "getPlaylist",
                serde_json::json!({"playlistId":id,"limit":1}),
            )
        };
        let readback = self.provider_operation(provider, operation, args).await
            .map_err(|_| format!("Playlist {id} may have been created ({url}); private/empty verification failed. Inspect the provider; DO NOT RETRY creation."))?;
        let playlist = readback
            .get("playlist")
            .ok_or("Created playlist could not be verified")?;
        if readback.get("accountId").and_then(Value::as_str) != Some(expected.as_str())
            || playlist.get("id").and_then(Value::as_str) != Some(id)
            || playlist.get("ownerId").and_then(Value::as_str) != Some(expected.as_str())
            || readback.get("complete").and_then(Value::as_bool) != Some(true)
            || playlist.get("title").and_then(Value::as_str) != Some("Burrata verification")
            || playlist.get("visibility").and_then(Value::as_str) != Some("private")
            || readback.get("total").and_then(Value::as_u64) != Some(0)
            || readback
                .get("entries")
                .and_then(Value::as_array)
                .is_none_or(|entries| !entries.is_empty())
            || readback
                .get("nextCursor")
                .is_some_and(|cursor| !cursor.is_null())
        {
            return Err(format!("Playlist {id} was created ({url}) but private/empty verification failed. Inspect the provider; DO NOT RETRY creation."));
        }
        let url = if provider == "spotify" {
            format!("https://open.spotify.com/playlist/{id}")
        } else {
            format!("https://www.youtube.com/playlist?list={id}")
        };
        let receipt = serde_json::json!({"provider":provider,"accountId":expected,"playlistId":id,"name":"Burrata verification","visibility":"private","total":0,"url":url,"verified":true,"requestId":attempt_id,"creationReceipt":creation_receipt});
        fs::write(
            &attempt,
            serde_json::to_vec(&receipt).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(receipt)
    }
    pub fn preview_verification_playlist_intent(&self, provider: &str) -> Result<Value, String> {
        if !self.native_enabled || !matches!(provider, "spotify" | "youtube") {
            return Err("Native Spotify or YouTube access is required".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "Provider session unavailable")?;
        if !native_session_ready(&state.mode) {
            return Err("Confirm the provider account and enable native access first".into());
        }
        native_youtube_reconnect_gate(provider, state.persisted.youtube_reconnect_required)
            .map_err(str::to_owned)?;
        let account = state
            .persisted
            .accounts
            .get(provider)
            .ok_or("Confirm the provider account first")?;
        verification_creation_intent::preview(
            &self.data_dir,
            provider,
            &account.account_id,
            account.revision,
            self.native_generation.load(Ordering::SeqCst),
        )
    }

    pub async fn apply_verification_playlist_intent(
        &self,
        provider: &str,
        intent_id: &str,
        review_token: &str,
        contract_hash: &str,
    ) -> Result<Value, String> {
        if !matches!(provider, "spotify" | "youtube") {
            return Err("Choose Spotify or YouTube".into());
        }
        if !self.native_enabled {
            return Err("The experimental managed provider session is disabled".into());
        }
        let expected = self
            .state
            .lock()
            .map_err(|_| "Provider session is unavailable")?
            .persisted
            .accounts
            .get(provider)
            .map(|account| account.account_id.clone())
            .ok_or("Confirm this provider account before creating a verification playlist")?;
        let local = self.status();
        verification_creation_intent::validate(
            &self.data_dir,
            intent_id,
            review_token,
            contract_hash,
            provider,
            &expected,
            *local
                .account_revisions
                .get(provider)
                .ok_or("Confirmed account revision unavailable")?,
            self.native_generation.load(Ordering::SeqCst),
        )?;
        let status = self
            .provider_operation(provider, "status", serde_json::json!({}))
            .await?;
        if status.get("accountId").and_then(Value::as_str) != Some(expected.as_str()) {
            return Err("Provider account changed. Confirm the active account again.".into());
        }
        let generation = self.native_generation.load(Ordering::SeqCst);
        let (mode, reconnect_required) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "Provider session is unavailable")?;
            (
                state.mode.clone(),
                state.persisted.youtube_reconnect_required,
            )
        };
        if !native_session_ready(&mode) {
            return Err("Confirm the provider account and enable native access first".into());
        }
        native_youtube_reconnect_gate(provider, reconnect_required).map_err(str::to_owned)?;
        let credential = if provider == "spotify" {
            self.load_native_cookie()?
        } else {
            self.load_native_youtube_session()?
        };
        // Atomic with record_account: validate/register/consume while holding account state.
        // Never hold this synchronous state guard across a worker await.
        let (contract, mut file, account_fence) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "Provider session unavailable")?;
            let account = state
                .persisted
                .accounts
                .get(provider)
                .ok_or("Provider account unavailable")?;
            if account.account_id != expected
                || self.native_generation.load(Ordering::SeqCst) != generation
            {
                return Err("Provider account changed before intent dispatch".into());
            }
            verification_creation_intent::consume_fenced(
                &self.data_dir,
                intent_id,
                review_token,
                contract_hash,
                provider,
                &account.account_id,
                account.revision,
                generation,
                &self.verification_intent_accounts,
            )?
        };
        let title = contract.title;
        let attempt_id = uuid::Uuid::new_v4().simple().to_string();
        use std::io::Write;
        let attempted = serde_json::json!({"status":"ATTEMPTED","intentId":intent_id,"contractHash":contract_hash,"requestId":attempt_id,"mutationDispatched":null});
        file.write_all(
            &serde_json::to_vec(&attempted).map_err(|_| "Cannot encode verification attempt")?,
        )
        .map_err(|_| "Cannot persist verification playlist attempt")?;
        file.sync_all()
            .map_err(|_| "Cannot persist verification playlist attempt")?;
        let created = self.run_native_job_with_creation_receipt(provider, "createVerificationPlaylist", serde_json::json!({"accountId":expected,"verificationIntentId":intent_id}), credential, Some(account_fence.cancel_flag()), generation, Some(expected.clone()), Some(&attempt_id)).await
            .map_err(|error| {
                // Only typed worker metadata can establish non-dispatch. Missing output is UNKNOWN.
                let worker = match &error { NativeJobFailure::Worker(worker) => Some(worker), NativeJobFailure::Legacy(_) => None };
                let worker_receipt = worker.and_then(|failure| failure.creation_receipt.clone());
                let not_dispatched = worker_receipt.as_ref().is_some_and(|receipt| receipt.get("mutationDispatched").and_then(Value::as_bool) == Some(false));
                let diagnostic = serde_json::json!({"kind":worker.map(|failure| format!("{:?}", failure.kind)),"httpStatus":worker.and_then(|failure| failure.http_status),"phase":worker.and_then(|failure| failure.phase),"creationReceipt":worker_receipt});
                let failed = serde_json::json!({"status":if not_dispatched {"NOT_DISPATCHED"} else {"UNKNOWN_OUTCOME"},"intentId":intent_id,"contractHash":contract_hash,"provider":provider,"requestId":attempt_id,"diagnostic":diagnostic});
                use std::io::{Seek, SeekFrom};
                if let Ok(bytes) = serde_json::to_vec(&failed) {
                    let _ = file.set_len(0).and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ())).and_then(|_| file.write_all(&bytes)).and_then(|_| file.sync_all());
                }
                if not_dispatched {
                    format!("The worker confirmed no playlist mutation was dispatched. The one-shot attempt remains blocked for review; DO NOT RETRY automatically. Diagnostic: {diagnostic}")
                } else {
                    format!("The verification playlist may have been created. Inspect the provider and saved attempt marker; DO NOT RETRY creation. Diagnostic: {diagnostic}")
                }
            })?;
        let id = created
            .get("playlistId")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 256
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .ok_or("Playlist creation returned no bounded playlist ID; inspect the provider")?;
        let url = if provider == "spotify" {
            format!("https://open.spotify.com/playlist/{id}")
        } else {
            format!("https://www.youtube.com/playlist?list={id}")
        };
        let creation_receipt = created
            .get("creationReceipt")
            .and_then(|receipt| bound_creation_receipt(receipt, &attempt_id));
        let valid_creation_receipt = creation_receipt.as_ref().is_some_and(|receipt| {
            receipt.get("mutationDispatched").and_then(Value::as_bool) == Some(true)
                && receipt.get("playlistId").and_then(Value::as_str) == Some(id)
        }) && created.get("accountId").and_then(Value::as_str)
            == Some(expected.as_str())
            && if provider == "spotify" {
                id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            } else {
                id.starts_with("PL") && (12..=80).contains(&id.len())
            };
        let pending = serde_json::json!({"status":"CREATED_PENDING_VERIFICATION","intentId":intent_id,"contractHash":contract_hash,"provider":provider,"requestId":attempt_id,"creationReceipt":creation_receipt,"playlistId":id,"url":url});
        verification_creation_intent::persist_created_then(&self.data_dir,intent_id,&pending, || async {
        file.set_len(0).map_err(|_| "Cannot persist verification intent state")?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).map_err(|_| "Cannot persist verification intent state")?;
        file.write_all(&serde_json::to_vec(&pending).map_err(|_| "Cannot persist verification intent state")?)
            .map_err(|_| "Cannot persist verification intent state")?;
        file.sync_all().map_err(|_| "Cannot persist verification intent state")?;
        if !valid_creation_receipt {
            return Err(format!("Playlist {id} may have been created ({url}); worker receipt validation failed. Inspect the provider; DO NOT RETRY creation."));
        }
        let (operation, args) = if provider == "spotify" {
            (
                "verifyVerificationPlaylist",
                serde_json::json!({"accountId":expected,"playlistId":id,"verificationIntentId":intent_id}),
            )
        } else {
            (
                "getPlaylist",
                serde_json::json!({"playlistId":id,"limit":1}),
            )
        };
        let readback = self.provider_operation(provider, operation, args).await
            .map_err(|_| format!("Playlist {id} may have been created ({url}); private/empty verification failed. Inspect the provider; DO NOT RETRY creation."))?;
        let playlist = readback
            .get("playlist")
            .ok_or("Created playlist could not be verified")?;
        if readback.get("accountId").and_then(Value::as_str) != Some(expected.as_str())
            || playlist.get("id").and_then(Value::as_str) != Some(id)
            || playlist.get("ownerId").and_then(Value::as_str) != Some(expected.as_str())
            || readback.get("complete").and_then(Value::as_bool) != Some(true)
            || playlist.get("title").and_then(Value::as_str) != Some(title.as_str())
            || playlist.get("visibility").and_then(Value::as_str) != Some("private")
            || readback.get("total").and_then(Value::as_u64) != Some(0)
            || readback
                .get("entries")
                .and_then(Value::as_array)
                .is_none_or(|entries| !entries.is_empty())
            || readback
                .get("nextCursor")
                .is_some_and(|cursor| !cursor.is_null())
        {
            return Err(format!("Playlist {id} was created ({url}) but private/empty verification failed. Inspect the provider; DO NOT RETRY creation."));
        }
        let url = if provider == "spotify" {
            format!("https://open.spotify.com/playlist/{id}")
        } else {
            format!("https://www.youtube.com/playlist?list={id}")
        };
        let receipt = serde_json::json!({"intentId":intent_id,"contractHash":contract_hash,"provider":provider,"accountId":expected,"playlistId":id,"name":title,"visibility":"private","total":0,"url":url,"verified":true,"requestId":attempt_id,"creationReceipt":creation_receipt});
        file.set_len(0).map_err(|_|"Cannot persist verified intent")?;
        file.seek(SeekFrom::Start(0)).map_err(|_|"Cannot persist verified intent")?;
        file.write_all(&serde_json::to_vec(&receipt).map_err(|_|"Cannot encode verified intent")?).and_then(|_|file.sync_all()).map_err(|_|"Cannot persist verified intent")?;
        Ok(receipt)
        }).await
    }
}

#[async_trait::async_trait]
impl ProviderTransport for ManagedProviderSession {
    fn native_write_sync_enabled(&self) -> bool {
        self.native_enabled
    }

    fn provider_operations_supported(&self) -> bool {
        if self.native_enabled {
            return self.state.lock().ok().is_some_and(|state| {
                (state.persisted.accounts.contains_key("spotify")
                    || state.persisted.accounts.contains_key("youtube"))
                    && native_session_ready(&state.mode)
            });
        }
        if !self.enabled {
            return self.bridge.provider_operations_supported();
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
        {
            stop_child(&mut state.child);
            state.mode = if state.background_opt_in && !state.persisted.accounts.is_empty() {
                "idle".into()
            } else {
                "stopped".into()
            };
            let _ = self.bridge.set_managed_session(None);
        }
        if state.child.is_none() {
            if state.background_opt_in && !state.persisted.accounts.is_empty() {
                return true;
            }
            drop(state);
            return self.bridge.provider_operations_supported();
        }
        self.bridge.provider_operations_supported()
    }

    async fn provider_operation(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
    ) -> Result<Value, String> {
        self.provider_operation_cancellable(provider, operation, args, None)
            .await
    }

    async fn provider_operation_cancellable(
        &self,
        provider: &str,
        operation: &str,
        args: Value,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Value, String> {
        if !native_runtime_open(&self.native_shutdown) {
            return Err("Managed provider session is shutting down".into());
        }
        let operation_generation = self.native_generation.load(Ordering::SeqCst);
        if self.native_enabled {
            if !matches!(provider, "spotify" | "youtube") {
                return Err("This native provider is not available".into());
            }
            if operation == "bootstrap" {
                return Err(
                    "This YouTube operation is only available during account confirmation".into(),
                );
            }
            let generation = self.native_generation.load(Ordering::SeqCst);
            let (mode, youtube_reconnect_required) = {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| "Provider session is unavailable")?;
                (
                    state.mode.clone(),
                    state.persisted.youtube_reconnect_required,
                )
            };
            if !native_session_ready(&mode) {
                return Err("Confirm the provider account and enable native access first".into());
            }
            native_youtube_reconnect_gate(provider, youtube_reconnect_required)
                .map_err(str::to_owned)?;
            let expected_account = self
                .state
                .lock()
                .map_err(|_| "Provider session is unavailable")?
                .persisted
                .accounts
                .get(provider)
                .map(|account| account.account_id.clone())
                .ok_or("Confirm this provider account before using native access")?;
            let credential = if provider == "spotify" {
                self.load_native_cookie()?
            } else {
                self.load_native_youtube_session()?
            };
            if self.native_generation.load(Ordering::SeqCst) != generation {
                if let Ok(mut cached) = self.native_cookie_header.lock() {
                    *cached = None;
                }
                return Err("Provider request was cancelled".into());
            }
            let result = self
                .run_native_job(
                    provider,
                    operation,
                    args,
                    credential,
                    cancel,
                    generation,
                    Some(expected_account.to_owned()),
                )
                .await;
            if provider == "youtube"
                && result
                    .as_ref()
                    .err()
                    .is_some_and(|error| is_youtube_reconnect_required_error(error))
            {
                // Serialize the status transition against login/stop generation changes. Do
                // not mark a cancelled or older channel's failure as the active session.
                let mut reconnect_changed = false;
                if let Ok(_control) = self.native_control.lock() {
                    if native_runtime_open(&self.native_shutdown)
                        && self.native_generation.load(Ordering::SeqCst) == generation
                    {
                        if let Ok(mut state) = self.state.lock() {
                            reconnect_changed = mark_youtube_reconnect_required(
                                &mut state.persisted,
                                &expected_account,
                                self.native_generation.load(Ordering::SeqCst),
                                generation,
                            );
                            if reconnect_changed {
                                let _ = persist_session(&self.data_dir, &state.persisted);
                            }
                        }
                    }
                }
                if reconnect_changed {
                    self.invalidate_playback_session("youtube");
                }
            }
            let result = result?;
            let active_account = result
                .get("accountId")
                .and_then(Value::as_str)
                .ok_or("Provider did not return a verified account identity")?;
            if active_account != expected_account {
                return Err("Provider account changed. Confirm the active account again.".into());
            }
            return Ok(result);
        }
        if !self.enabled {
            if !native_runtime_open(&self.native_shutdown)
                || self.native_generation.load(Ordering::SeqCst) != operation_generation
            {
                return Err("Provider request was cancelled".into());
            }
            return self
                .bridge
                .provider_operation_cancellable(provider, operation, args, cancel)
                .await;
        }
        let restart = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Managed browser session is unavailable")?;
            if state
                .child
                .as_mut()
                .is_some_and(|child| child.try_wait().ok().flatten().is_some())
            {
                stop_child(&mut state.child);
                state.mode = if state.background_opt_in && !state.persisted.accounts.is_empty() {
                    "idle".into()
                } else {
                    "stopped".into()
                };
                let _ = self.bridge.set_managed_session(None);
            }
            state.mode == "idle" && state.background_opt_in && !state.persisted.accounts.is_empty()
        };
        if restart {
            self.start_background()?;
        }
        let use_companion_fallback = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Managed browser session is unavailable")?;
            if state
                .child
                .as_mut()
                .is_some_and(|child| child.try_wait().ok().flatten().is_some())
            {
                stop_child(&mut state.child);
                state.mode = if state.background_opt_in && !state.persisted.accounts.is_empty() {
                    "idle".into()
                } else {
                    "stopped".into()
                };
                let _ = self.bridge.set_managed_session(None);
                if state.mode == "idle" {
                    return Err("The background provider session exited before the request started. Retry to restart it.".into());
                }
            }
            if state.child.is_none() {
                true
            } else {
                self.active_jobs.fetch_add(1, Ordering::SeqCst);
                state.last_activity = Instant::now();
                false
            }
        };
        if use_companion_fallback {
            if !native_runtime_open(&self.native_shutdown)
                || self.native_generation.load(Ordering::SeqCst) != operation_generation
            {
                return Err("Provider request was cancelled".into());
            }
            return self
                .bridge
                .provider_operation_cancellable(provider, operation, args, cancel)
                .await;
        }
        let active_job = ActiveJobGuard { session: self };
        let ready = self.wait_for_connection(cancel.as_ref()).await;
        let result = match ready {
            Ok(())
                if native_runtime_open(&self.native_shutdown)
                    && self.native_generation.load(Ordering::SeqCst) == operation_generation =>
            {
                self.bridge
                    .provider_operation_cancellable(provider, operation, args, cancel)
                    .await
            }
            Ok(()) => Err("Provider request was cancelled".into()),
            Err(error) => Err(error),
        };
        drop(active_job);
        let result = result?;
        if operation == "status" {
            if let Some(account_id) = result
                .get("accountId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 256)
            {
                self.record_account(provider, account_id)?;
            }
        }
        Ok(result)
    }
}

impl ManagedProviderSession {
    async fn wait_for_connection(&self, cancel: Option<&Arc<AtomicBool>>) -> Result<(), String> {
        for _ in 0..120 {
            if self.bridge.provider_operations_supported() {
                return Ok(());
            }
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
                return Err("Managed browser provider operation was cancelled".into());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err("Managed browser companion did not connect. In its profile, enable app-driven imports in the extension popup and retry.".into())
    }

    fn invalidate_playback_session(&self, provider: &str) {
        // AppState is installed after session construction. No callback retains
        // the application or session, and ordinary healthy status reads do nothing.
        if let Some(state) = self.app.try_state::<crate::AppState>() {
            crate::invalidate_managed_playback_sources(&state, &[provider]);
        }
    }

    fn record_account(&self, provider: &str, account_id: &str) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Managed browser session is unavailable")?;
        let current = state.persisted.accounts.get(provider);
        let changed = current.map(|value| value.account_id.as_str()) != Some(account_id);
        let persisted = if changed {
            self.verification_intent_accounts.invalidate(provider)?;
            if provider == "spotify" {
                self.spotify_auth_cache.invalidate_now();
            }
            let revision = current.map_or(1, |value| value.revision.saturating_add(1));
            state.persisted.accounts.insert(
                provider.to_owned(),
                ConfirmedAccount {
                    account_id: account_id.to_owned(),
                    revision,
                },
            );
            persist_session(&self.data_dir, &state.persisted)
        } else {
            Ok(())
        };
        state.last_activity = Instant::now();
        drop(state);
        if changed {
            self.invalidate_playback_session(provider);
        }
        persisted
    }
}

const SPOTIFY_STORAGE_ERROR: &str = "Spotify sign-in could not be saved securely. Unlock or enable your system keyring, then confirm again. Your sign-in window has been kept open.";

const YOUTUBE_STORAGE_ERROR: &str = "YouTube sign-in could not be saved securely. Unlock or enable your system keyring, then confirm again or retry handoff with a new pairing code. Browser sign-in data has been kept.";

fn verify_persisted_spotify_cookie<E>(
    cookie: &str,
    store: impl FnOnce() -> Result<(), E>,
    read: impl FnOnce() -> Result<String, E>,
) -> Result<SecretString, String> {
    verify_persisted_provider_credential(cookie, SPOTIFY_STORAGE_ERROR, store, read)
}

fn verify_persisted_provider_credential<E>(
    credential: &str,
    storage_error: &str,
    store: impl FnOnce() -> Result<(), E>,
    read: impl FnOnce() -> Result<String, E>,
) -> Result<SecretString, String> {
    store().map_err(|_| storage_error.to_owned())?;
    let saved = SecretString(read().map_err(|_| storage_error.to_owned())?);
    if saved.0 != credential {
        return Err(storage_error.into());
    }
    Ok(saved)
}

fn spotify_credential_fingerprint(key: &[u8; 32], credential: &[u8]) -> [u8; 32] {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for index in 0..key.len() {
        inner_pad[index] ^= key[index];
        outer_pad[index] ^= key[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(credential);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    inner_pad.fill(0);
    outer_pad.fill(0);
    outer.finalize().into()
}

fn load_or_create_session(data_dir: &Path) -> PersistedSession {
    let path = data_dir.join("managed-provider-session/session.json");
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(session) = serde_json::from_slice::<PersistedSession>(&bytes) {
            if (16..=128).contains(&session.session_id.len())
                && session
                    .session_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return session;
            }
        }
    }
    PersistedSession {
        session_id: uuid::Uuid::new_v4().simple().to_string(),
        accounts: BTreeMap::new(),
        background_opt_in: false,
        native_transport_opt_in: false,
        native_stopped: false,
        youtube_reconnect_required: false,
    }
}

fn persist_session(data_dir: &Path, session: &PersistedSession) -> Result<(), String> {
    let root = data_dir.join("managed-provider-session");
    secure_create_dir(&root)?;
    let target = root.join("session.json");
    let pending = root.join(format!("session-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(
        &pending,
        serde_json::to_vec(session).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    secure_file(&pending)?;
    fs::rename(pending, target).map_err(|e| e.to_string())
}

fn stage_extension_icons(source_extension: &Path, extension: &Path) -> Result<(), String> {
    for (relative_path, embedded) in EXTENSION_ICONS {
        let target = extension.join(relative_path);
        let parent = target
            .parent()
            .ok_or_else(|| "Managed extension icon path is invalid".to_owned())?;
        secure_create_dir(parent)?;
        let source = source_extension.join(relative_path);
        let bytes = fs::read(source).unwrap_or_else(|_| embedded.to_vec());
        fs::write(&target, bytes).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        secure_file(&target)?;
    }
    Ok(())
}

fn secure_create_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn secure_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn stop_child(child: &mut Option<Child>) {
    let Some(mut process) = child.take() else {
        return;
    };
    #[cfg(unix)]
    unsafe {
        libc::kill(-(process.id() as libc::pid_t), libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if process.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(process.id() as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = process.kill();
    let _ = process.wait();
}

impl ManagedProviderSession {
    pub(crate) fn shutdown(&self) {
        // Serialize with login-window creation so a call that starts after shutdown cannot
        // reopen a provider window. Runtime worker admission is independently gated below.
        let _control = self.native_control.lock().ok();
        stop_native_runtime(
            &self.native_shutdown,
            &self.native_generation,
            &self.native_cancellations,
            &self.spotify_auth_cache,
        );
        if let Ok(mut cookie) = self.native_cookie_header.lock() {
            *cookie = None;
        }
        if let Ok(mut pending) = self.native_spotify_pending.lock() {
            *pending = None;
        }
        if let Ok(mut session) = self.native_youtube_session.lock() {
            *session = None;
        }
        if let Ok(mut pending) = self.native_youtube_pending.lock() {
            *pending = None;
        }
        if let Ok(mut pairing) = self.youtube_handoff_pairing.lock() {
            *pairing = None;
        }
        if let Ok(mut state) = self.state.lock() {
            stop_child(&mut state.child);
        }
    }
}

fn native_runtime_open(shutdown: &AtomicBool) -> bool {
    !shutdown.load(Ordering::SeqCst)
}

fn stop_native_runtime(
    shutdown: &AtomicBool,
    generation: &AtomicU64,
    jobs: &Mutex<Vec<Weak<AtomicBool>>>,
    auth_cache: &SpotifyAuthCache,
) {
    shutdown.store(true, Ordering::SeqCst);
    signal_native_stop(generation, jobs);
    auth_cache.invalidate_now();
}

impl Drop for ManagedProviderSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_extension_staging_materializes_every_manifest_icon() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source-extension");
        let staged = temp.path().join("managed-extension");
        secure_create_dir(&source).unwrap();
        secure_create_dir(&source.join("icons")).unwrap();
        secure_create_dir(&staged).unwrap();

        // Packaged files win when present; the other three exercise embedded fallback.
        fs::write(source.join("icons/burrata-16.png"), b"packaged-icon").unwrap();
        stage_extension_icons(&source, &staged).unwrap();

        let manifest: Value =
            serde_json::from_str(include_str!("../../extension/manifest.chromium.json")).unwrap();
        let mut icon_paths = std::collections::BTreeSet::new();
        for value in manifest["icons"].as_object().unwrap().values() {
            icon_paths.insert(value.as_str().unwrap().to_owned());
        }
        for value in manifest["action"]["default_icon"]
            .as_object()
            .unwrap()
            .values()
        {
            icon_paths.insert(value.as_str().unwrap().to_owned());
        }
        assert_eq!(icon_paths.len(), EXTENSION_ICONS.len());
        for icon in icon_paths {
            let path = staged.join(&icon);
            assert!(path.is_file(), "manifest icon missing: {icon}");
            assert!(
                !fs::read(&path).unwrap().is_empty(),
                "manifest icon empty: {icon}"
            );
        }
        assert_eq!(
            fs::read(staged.join("icons/burrata-16.png")).unwrap(),
            b"packaged-icon"
        );
        for (relative, embedded) in EXTENSION_ICONS
            .iter()
            .filter(|(path, _)| *path != "icons/burrata-16.png")
        {
            assert_eq!(fs::read(staged.join(relative)).unwrap(), embedded.to_vec());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(staged.join("icons"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            for icon in icon_paths_for_test() {
                assert_eq!(
                    fs::metadata(staged.join(icon))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }

    #[cfg(unix)]
    fn icon_paths_for_test() -> impl Iterator<Item = &'static str> {
        EXTENSION_ICONS.iter().map(|(path, _)| *path)
    }

    #[test]
    fn native_restart_mode_recognizes_either_confirmed_provider() {
        let account = ConfirmedAccount {
            account_id: "fixture".into(),
            revision: 1,
        };
        let mut accounts = BTreeMap::new();
        accounts.insert("youtube".into(), account.clone());
        assert!(persisted_has_confirmed_provider(&accounts));
        accounts.clear();
        accounts.insert("spotify".into(), account);
        assert!(persisted_has_confirmed_provider(&accounts));
        accounts.clear();
        assert!(!persisted_has_confirmed_provider(&accounts));
        assert_eq!(
            native_initial_mode(true, false, true, false),
            NATIVE_MODE_CONFIRMED
        );
        assert_eq!(native_initial_mode(true, false, true, true), "stopped");
        assert_eq!(
            native_initial_mode(true, true, true, false),
            NATIVE_MODE_READY
        );
        assert_eq!(native_initial_mode(false, false, true, false), "stopped");
        assert_eq!(native_initial_mode(false, true, true, false), "idle");
        assert_eq!(native_initial_mode(true, false, false, false), "stopped");
    }

    #[test]
    fn native_transport_opt_in_migrates_conservatively_and_honors_explicit_override() {
        let legacy = serde_json::json!({
            "sessionId": "legacy-session-id",
            "accounts": { "spotify": { "accountId": "fixture", "revision": 1 } },
            "backgroundOptIn": true,
            "nativeStopped": false
        });
        let loaded: PersistedSession = serde_json::from_value(legacy).unwrap();
        assert!(
            !loaded.native_transport_opt_in,
            "legacy confirmed accounts alone do not enable the new transport"
        );
        assert!(!native_transport_requested(None, false));
        assert!(native_transport_requested(None, true));
        assert!(native_transport_requested(Some("1"), false));
        assert!(!native_transport_requested(Some("0"), true));
        assert!(!native_transport_requested(Some(""), true));
        assert!(
            !native_session_enabled(true, false),
            "unsupported platforms remain gated"
        );
        assert!(native_session_enabled(true, true));
    }

    #[test]
    fn native_transport_opt_in_survives_a_profile_restart_without_changing_stop_state() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = load_or_create_session(temp.path());
        let id = session.session_id.clone();
        session.accounts.insert(
            "spotify".into(),
            ConfirmedAccount {
                account_id: "fixture-account".into(),
                revision: 1,
            },
        );
        session.native_transport_opt_in = true;
        session.native_stopped = true;
        session.background_opt_in = false;
        persist_session(temp.path(), &session).unwrap();

        let restored = load_or_create_session(temp.path());
        assert_eq!(restored.session_id, id);
        assert!(restored.native_transport_opt_in);
        assert!(restored.native_stopped);
        assert!(!restored.background_opt_in);
        assert_eq!(
            native_initial_mode(
                true,
                restored.background_opt_in,
                true,
                restored.native_stopped
            ),
            "stopped"
        );
    }

    #[test]
    fn youtube_reconnect_marker_is_exact_scoped_and_preserves_confirmed_identity() {
        for reconnect_error in [
            "SIGN_IN_REQUIRED: YouTube session expired or was denied",
            "ACCOUNT_CHANGED: The active YouTube account changed. Confirm the account again before continuing.",
        ] {
            assert!(is_youtube_reconnect_required_error(reconnect_error));
        }
        for other in [
            "Provider request was cancelled",
            "RATE_LIMITED: try again",
            "API_DENIED: request denied",
            "some text SIGN_IN_REQUIRED: forged",
            "some text ACCOUNT_CHANGED: forged",
            "SIGN_IN_REQUIRED without delimiter",
            "ACCOUNT_CHANGED without delimiter",
        ] {
            assert!(!is_youtube_reconnect_required_error(other));
        }

        let account = ConfirmedAccount {
            account_id: "UC0000000000000000000000".into(),
            revision: 7,
        };
        let mut accounts = BTreeMap::new();
        accounts.insert("youtube".into(), account);
        let mut persisted = PersistedSession {
            session_id: "fixture-session".into(),
            accounts,
            background_opt_in: true,
            native_transport_opt_in: false,
            native_stopped: false,
            youtube_reconnect_required: false,
        };
        assert!(!mark_youtube_reconnect_required(
            &mut persisted,
            "UC0000000000000000000000",
            12,
            11,
        ));
        assert!(!persisted.youtube_reconnect_required);
        assert!(!mark_youtube_reconnect_required(
            &mut persisted,
            "UC1111111111111111111111",
            12,
            12,
        ));
        assert!(!persisted.youtube_reconnect_required);
        assert!(mark_youtube_reconnect_required(
            &mut persisted,
            "UC0000000000000000000000",
            12,
            12,
        ));
        assert!(persisted.youtube_reconnect_required);
        assert!(native_youtube_reconnect_gate("youtube", true).is_err());
        assert!(native_youtube_reconnect_gate("spotify", true).is_ok());
        assert!(native_youtube_reconnect_gate("youtube", false).is_ok());

        // Replacement identity is reviewed by the normal pending-account confirmation
        // path; the gate prevents stale-session calls but does not forbid an explicit switch.
        let reviewed_replacement = "UC1111111111111111111111";
        assert!(valid_youtube_channel_id(reviewed_replacement));
        clear_youtube_reconnect_required(&mut persisted);
        assert!(!persisted.youtube_reconnect_required);
    }

    #[test]
    fn old_persisted_session_defaults_youtube_reconnect_flag_to_false() {
        let old = serde_json::json!({
            "sessionId": "fixture-session",
            "accounts": {},
            "backgroundOptIn": true,
            "nativeStopped": false
        });
        let loaded: PersistedSession = serde_json::from_value(old).unwrap();
        assert!(!loaded.youtube_reconnect_required);
        assert!(!loaded.native_transport_opt_in);
    }

    #[tokio::test]
    async fn shutdown_cancels_refresh_and_preserves_persisted_resume_state() {
        let cache = SpotifyAuthCache::default();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(0);
        let jobs = Mutex::new(Vec::<Weak<AtomicBool>>::new());
        let stop = Arc::new(AtomicBool::new(false));
        jobs.lock().unwrap().push(Arc::downgrade(&stop));
        let persisted = PersistedSession {
            session_id: "fixture-session".into(),
            accounts: BTreeMap::new(),
            background_opt_in: true,
            native_transport_opt_in: false,
            native_stopped: false,
            youtube_reconnect_required: true,
        };
        let before = persisted.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let refresh_cache = cache.clone();
        let refresh_stop = stop.clone();
        let waiter = tokio::spawn(async move {
            refresh_cache
                .get_or_refresh(
                    AuthScope {
                        generation: 0,
                        account_id: "fixture-account".into(),
                        credential_fingerprint: [7; 32],
                    },
                    None,
                    refresh_stop,
                    move || async move {
                        let _ = started_tx.send(());
                        std::future::pending::<Result<_, String>>().await
                    },
                )
                .await
        });
        started_rx.await.unwrap();

        stop_native_runtime(&shutdown, &generation, &jobs, &cache);

        assert!(!native_runtime_open(&shutdown));
        assert_eq!(generation.load(Ordering::SeqCst), 1);
        assert!(stop.load(Ordering::SeqCst));
        assert!(waiter.await.unwrap().is_err());
        assert_eq!(persisted.background_opt_in, before.background_opt_in);
        assert_eq!(persisted.native_stopped, before.native_stopped);
        assert_eq!(persisted.accounts.len(), before.accounts.len());
    }

    #[test]
    fn spotify_login_capture_blanks_only_owned_spotify_origin_or_retry_blank() {
        assert!(valid_native_spotify_capture_url(
            &url::Url::parse("https://open.spotify.com/redirect?fixture=1").unwrap()
        ));
        assert!(valid_native_spotify_capture_url(
            &url::Url::parse("about:blank").unwrap()
        ));
        for invalid in [
            "https://accounts.spotify.com/en/login",
            "https://open.spotify.com.evil.example/",
            "http://open.spotify.com/",
            "https://user@open.spotify.com/",
            "https://open.spotify.com:444/",
        ] {
            assert!(!valid_native_spotify_capture_url(
                &url::Url::parse(invalid).unwrap()
            ));
        }
    }

    #[test]
    fn spotify_login_return_accepts_only_owned_https_accounts_origin() {
        assert!(valid_native_spotify_accounts_url(
            &url::Url::parse("https://accounts.spotify.com/en/status?fixture=1").unwrap()
        ));
        for invalid in [
            "https://accounts.spotify.com.evil.example/",
            "https://evilaccounts.spotify.com/",
            "https://open.spotify.com/",
            "http://accounts.spotify.com/",
            "https://user@accounts.spotify.com/",
            "https://user:password@accounts.spotify.com/",
            "https://accounts.spotify.com:444/",
            "about:blank",
        ] {
            assert!(!valid_native_spotify_accounts_url(
                &url::Url::parse(invalid).unwrap()
            ));
        }
    }

    #[test]
    fn pending_spotify_cookie_is_scoped_to_the_confirmation_generation() {
        let mut pending = Some((4, SecretString("stale-fixture-cookie".into())));
        assert!(pending_spotify_cookie(&mut pending, 5).is_none());
        assert!(pending.is_none());

        let stored = pending_spotify_cookie_store(
            &mut pending,
            5,
            SecretString("current-fixture-cookie".into()),
        );
        assert_eq!(stored.0, "current-fixture-cookie");
        assert_eq!(
            pending_spotify_cookie(&mut pending, 5).unwrap().0,
            "current-fixture-cookie"
        );

        let reused = pending_spotify_cookie_store(
            &mut pending,
            5,
            SecretString("must-not-replace-cookie".into()),
        );
        assert_eq!(reused.0, "current-fixture-cookie");
    }

    #[test]
    fn native_youtube_cookie_capture_preserves_allowed_session_cookies_only() {
        let cookie = youtube_cookie_from_rows(vec![
            ("ANALYTICS".into(), "must-not-leave-webview".into()),
            ("__Secure-3PSIDCC".into(), "secure-3psidcc-value".into()),
            ("__Secure-1PSIDCC".into(), "secure-1psidcc-value".into()),
            ("__Secure-3PSIDTS".into(), "secure-3psidts-value".into()),
            ("__Secure-1PSIDTS".into(), "secure-1psidts-value".into()),
            ("SIDCC".into(), "sidcc-value-123456".into()),
            ("__Secure-3PSID".into(), "secure-3psid-value".into()),
            ("__Secure-1PSID".into(), "secure-1psid-value".into()),
            ("APISID".into(), "apisid-value-123456".into()),
            ("SSID".into(), "ssid-value-123456".into()),
            ("HSID".into(), "hsid-value-123456".into()),
            ("SID".into(), "google-wide-cookie".into()),
            ("LOGIN_INFO".into(), "login-info-value-123456".into()),
            ("__Secure-3PAPISID".into(), "third-value-123456".into()),
            ("SAPISID".into(), "sap-value-123456".into()),
            ("__Secure-1PAPISID".into(), "secure-value-123456".into()),
        ])
        .unwrap();
        assert_eq!(
            cookie.0,
            "SAPISID=sap-value-123456; __Secure-1PAPISID=secure-value-123456; __Secure-3PAPISID=third-value-123456; LOGIN_INFO=login-info-value-123456; SID=google-wide-cookie; HSID=hsid-value-123456; SSID=ssid-value-123456; APISID=apisid-value-123456; __Secure-1PSID=secure-1psid-value; __Secure-3PSID=secure-3psid-value; SIDCC=sidcc-value-123456; __Secure-1PSIDTS=secure-1psidts-value; __Secure-3PSIDTS=secure-3psidts-value; __Secure-1PSIDCC=secure-1psidcc-value; __Secure-3PSIDCC=secure-3psidcc-value"
        );
        assert!(valid_youtube_cookie_header(&cookie.0));
        assert!(!cookie.0.contains("ANALYTICS"));
    }

    #[test]
    fn native_youtube_cookie_capture_rejects_ambiguity_injection_and_oversized_headers() {
        assert!(youtube_cookie_from_rows(vec![
            ("SAPISID".into(), "first-value-123456".into()),
            ("SAPISID".into(), "second-value-123456".into()),
        ])
        .is_err());
        assert!(youtube_cookie_from_rows(vec![
            ("SAPISID".into(), "first-value-123456".into()),
            ("SID".into(), "session-value-123456".into()),
            ("SID".into(), "second-session-value".into()),
        ])
        .is_err());
        assert!(
            youtube_cookie_from_rows(vec![("SID".into(), "google-wide-cookie".into())]).is_err()
        );
        for injected in [
            "value; SID=x",
            "value with-space",
            "value\tbad",
            "value\nbad",
        ] {
            assert!(youtube_cookie_from_rows(vec![("SAPISID".into(), injected.into())]).is_err());
        }
        assert!(youtube_cookie_from_rows(vec![("SAPISID".into(), "short".into())]).is_err());
        assert!(youtube_cookie_from_rows(vec![("SAPISID".into(), "x".repeat(4097))]).is_err());
        assert!(youtube_cookie_from_rows(vec![
            ("SAPISID".into(), "a".repeat(4096)),
            ("__Secure-1PAPISID".into(), "b".repeat(4096)),
            ("__Secure-3PAPISID".into(), "c".repeat(4096)),
            ("SID".into(), "d".repeat(4096)),
            ("HSID".into(), "e".repeat(4096)),
        ])
        .is_err());
        assert!(!valid_youtube_cookie_header(&format!(
            "SAPISID={}",
            "x".repeat(16_385)
        )));
    }

    #[test]
    fn native_youtube_cookie_header_accepts_legacy_single_cookie_and_family_header() {
        assert!(valid_youtube_cookie_header(
            "__Secure-1PAPISID=legacy-value-123456"
        ));
        assert!(valid_youtube_cookie_header(
            "SAPISID=sap-value-123456; __Secure-1PAPISID=secure-value-123456; __Secure-3PAPISID=third-value-123456"
        ));
        assert!(valid_youtube_cookie_header("SAPISID=YWJjZGVm=="));
        assert!(!valid_youtube_cookie_header("SAPISID=short"));
        assert!(!valid_youtube_cookie_header("SAPISID=one; SAPISID=two"));
        assert!(!valid_youtube_cookie_header("SID=session-value-123456"));
        let session = YoutubeSession {
            origin: "https://music.youtube.com".into(),
            cookie_header: "__Secure-1PAPISID=legacy-value-123456".into(),
            account_id: "UC12345678901234567890".into(),
            display_name: "Fixture".into(),
            client_name: "WEB_REMIX".into(),
            client_version: "1.0".into(),
            api_key: String::new(),
            visitor_data: String::new(),
            data_sync_id: None,
            auth_user: "0".into(),
            page_context_captured: false,
            hl: "en".into(),
            gl: "US".into(),
        };
        assert!(valid_youtube_session(&session).is_ok());
        let mut with_sync_id = session;
        with_sync_id.data_sync_id = Some("V1:fixture-context".into());
        assert!(valid_youtube_session(&with_sync_id).is_ok());
        with_sync_id.data_sync_id = Some("invalid context".into());
        assert!(valid_youtube_session(&with_sync_id).is_err());
    }

    #[test]
    fn youtube_session_without_data_sync_id_remains_backward_compatible() {
        let legacy = serde_json::json!({
            "origin":"https://music.youtube.com",
            "cookieHeader":"SAPISID=legacy-value-123456",
            "accountId":"UC12345678901234567890",
            "displayName":"Fixture",
            "clientName":"WEB_REMIX",
            "clientVersion":"1.0",
            "apiKey":"",
            "visitorData":"",
            "authUser":"0",
            "hl":"en",
            "gl":"US"
        });
        let session: YoutubeSession = serde_json::from_value(legacy).unwrap();
        assert_eq!(session.data_sync_id, None);
        assert!(valid_youtube_session(&session).is_ok());
        let mut explicit_null = serde_json::to_value(session).unwrap();
        explicit_null["dataSyncId"] = Value::Null;
        assert!(serde_json::from_value::<YoutubeSession>(explicit_null).is_err());
    }

    #[test]
    fn youtube_login_return_requires_owned_google_origin_and_auth_cookie() {
        let google = url::Url::parse("https://accounts.google.com/ServiceLogin?private=1").unwrap();
        assert!(validate_youtube_login_return(&google, false).is_err());
        assert!(validate_youtube_login_return(&google, true).is_ok());
        for invalid in [
            "http://accounts.google.com/",
            "https://accounts.google.com.evil.example/",
            "https://music.youtube.com/",
            "https://user@accounts.google.com/",
            "https://accounts.google.com:444/",
        ] {
            let url = url::Url::parse(invalid).unwrap();
            assert!(validate_youtube_login_return(&url, true).is_err());
        }
        assert_eq!(
            youtube_login_page_classification(&google),
            "google-accounts"
        );
        assert!(!youtube_login_page_classification(&google).contains("private"));
        assert_eq!(
            youtube_login_page_classification(
                &url::Url::parse("https://music.youtube.com/watch?v=private").unwrap()
            ),
            "music"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn explicit_youtube_continue_routes_only_google_or_music_pages() {
        let google = url::Url::parse("https://accounts.google.com/signin/v2").unwrap();
        let music = url::Url::parse("https://music.youtube.com/").unwrap();
        let other = url::Url::parse("https://www.youtube.com/").unwrap();
        assert_eq!(youtube_confirm_requires_music_return(&google), Ok(true));
        assert_eq!(youtube_confirm_requires_music_return(&music), Ok(false));
        assert!(youtube_confirm_requires_music_return(&other).is_err());
        assert!(validate_youtube_login_return(&google, false).is_err());
        assert!(validate_youtube_login_return(&google, true).is_ok());
    }

    #[test]
    fn native_youtube_page_context_is_strictly_scoped_and_bounded() {
        assert!(valid_youtube_music_page_url(
            &url::Url::parse("https://music.youtube.com/watch?v=fixture").unwrap()
        ));
        for invalid in [
            "http://music.youtube.com/",
            "https://www.youtube.com/",
            "https://music.youtube.com.evil.example/",
            "https://music.youtube.com:444/",
            "https://user@music.youtube.com/",
        ] {
            assert!(!valid_youtube_music_page_url(
                &url::Url::parse(invalid).unwrap()
            ));
        }

        let parsed = parse_youtube_page_context(
            r#"{"visitorData":"visitor-fixture","dataSyncId":"V1:sync-fixture||tail","authUser":"2","loggedIn":true}"#,
        ).unwrap();
        assert_eq!(parsed.visitor_data, "visitor-fixture");
        assert_eq!(parsed.data_sync_id.as_deref(), Some("V1:sync-fixture"));
        assert_eq!(parsed.auth_user, "2");
        assert_eq!(parsed.logged_in, Some(true));

        for invalid in [
            r#"{"visitorData":"","dataSyncId":null,"authUser":"0","loggedIn":true}"#,
            r#"{"visitorData":"visitor","dataSyncId":"||tail","authUser":"0","loggedIn":true}"#,
            r#"{"visitorData":"visitor","dataSyncId":null,"authUser":"not-numeric","loggedIn":true}"#,
            r#"{"visitorData":"visitor","dataSyncId":null,"authUser":"0","loggedIn":false}"#,
        ] {
            assert!(parse_youtube_page_context(invalid).is_err());
        }
    }

    #[test]
    fn native_worker_refresh_uses_immutable_content_addressed_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let worker_root = temp.path().join("native-worker");
        let v1 = [("scripts/provider.mjs", "fixture-worker-v1")];
        let first = stage_native_worker_snapshot(&worker_root, None, &v1).unwrap();
        let repeated = stage_native_worker_snapshot(&worker_root, None, &v1).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(
            fs::read_to_string(first.join("scripts/provider.mjs")).unwrap(),
            "fixture-worker-v1"
        );

        let packaged = temp.path().join("packaged");
        secure_create_dir(&packaged).unwrap();
        secure_create_dir(&packaged.join("scripts")).unwrap();
        fs::write(packaged.join("scripts/provider.mjs"), "fixture-worker-v2").unwrap();
        let second = stage_native_worker_snapshot(&worker_root, Some(&packaged), &v1).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(first.join("scripts/provider.mjs")).unwrap(),
            "fixture-worker-v1"
        );
        assert_eq!(
            fs::read_to_string(second.join("scripts/provider.mjs")).unwrap(),
            "fixture-worker-v2"
        );
    }

    #[test]
    fn native_account_confirmation_enables_read_only_access() {
        let mut state = SessionState {
            persisted: PersistedSession {
                session_id: "fixture-session".into(),
                accounts: BTreeMap::new(),
                background_opt_in: false,
                native_transport_opt_in: false,
                native_stopped: false,
                youtube_reconnect_required: false,
            },
            mode: NATIVE_MODE_CONFIRMED.into(),
            child: None,
            last_activity: Instant::now(),
            background_opt_in: false,
        };
        activate_native_session(&mut state);
        assert!(native_session_ready(&state.mode));
        assert!(state.background_opt_in);
        assert!(state.persisted.background_opt_in);
        assert!(state.persisted.native_transport_opt_in);
    }

    #[test]
    fn native_login_opens_the_spotify_sign_in_route() {
        let url = url::Url::parse(NATIVE_LOGIN_URL).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("accounts.spotify.com"));
        assert!(url.path().ends_with("/login"));
    }

    #[test]
    fn persisted_session_has_stable_id_and_advances_account_revision() {
        let temp = tempfile::tempdir().unwrap();
        let first = load_or_create_session(temp.path());
        persist_session(temp.path(), &first).unwrap();
        let reloaded = load_or_create_session(temp.path());
        assert_eq!(first.session_id, reloaded.session_id);
        assert!(reloaded.accounts.is_empty());
        assert!(!reloaded.native_stopped);
        let legacy: PersistedSession = serde_json::from_str(
            r#"{"sessionId":"legacy-session-id","accounts":{},"backgroundOptIn":false}"#,
        )
        .unwrap();
        assert!(!legacy.native_stopped);
        let mut confirmed = reloaded;
        confirmed.accounts.insert(
            "spotify".into(),
            ConfirmedAccount {
                account_id: "spotify-user".into(),
                revision: 1,
            },
        );
        confirmed.accounts.insert(
            "youtube".into(),
            ConfirmedAccount {
                account_id: "youtube-channel".into(),
                revision: 3,
            },
        );
        persist_session(temp.path(), &confirmed).unwrap();
        let final_state = load_or_create_session(temp.path());
        assert_eq!(final_state.accounts["spotify"].account_id, "spotify-user");
        assert_eq!(final_state.accounts["spotify"].revision, 1);
        assert_eq!(
            final_state.accounts["youtube"].account_id,
            "youtube-channel"
        );
        assert_eq!(final_state.accounts["youtube"].revision, 3);
    }

    #[test]
    fn native_cookie_forwarding_keeps_only_the_two_spotify_session_cookies() {
        let header = spotify_cookie_header(&[
            ("sp_dc".into(), "session-value".into()),
            ("sp_key".into(), "csrf-value".into()),
            ("other".into(), "must-not-forward".into()),
        ])
        .unwrap();
        assert_eq!(header, "sp_dc=session-value; sp_key=csrf-value");
        assert!(valid_spotify_cookie_header(&header));
        assert!(!valid_spotify_cookie_header("sp_dc=ok; other=no"));
        assert!(spotify_cookie_header(&[("sp_dc".into(), "bad; injected=x".into())]).is_err());
        assert!(spotify_cookie_header(&[("sp_key".into(), "csrf".into())]).is_err());
        assert!(spotify_cookie_header(&[
            ("sp_dc".into(), "first".into()),
            ("sp_dc".into(), "second".into()),
        ])
        .is_err());
        assert!(native_read_operation_allowed("getArtist"));
        assert!(native_read_operation_allowed("getTrack"));
        assert!(!native_read_operation_allowed("updatePlaylist"));
    }

    #[test]
    fn native_stop_signals_every_active_worker_and_advances_generation() {
        let generation = AtomicU64::new(7);
        let signal = Arc::new(AtomicBool::new(false));
        let jobs = Mutex::new(vec![Arc::downgrade(&signal)]);
        signal_native_stop(&generation, &jobs);
        assert_eq!(generation.load(Ordering::SeqCst), 8);
        assert!(signal.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_worker_fake_process_is_bounded_and_cancellable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let success = temp.path().join("fake-node-success");
        fs::write(
            &success,
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"ok\":true,\"result\":{\"accountId\":\"fixture-user\"}}\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&success, fs::Permissions::from_mode(0o700)).unwrap();
        let result = run_worker_process(
            success,
            temp.path().join("worker.mjs"),
            SecretBytes(b"fixture request".to_vec()),
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(result["accountId"], "fixture-user");

        let sleeper = temp.path().join("fake-node-sleep");
        fs::write(&sleeper, "#!/bin/sh\ncat >/dev/null\nexec sleep 30\n").unwrap();
        fs::set_permissions(&sleeper, fs::Permissions::from_mode(0o700)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_signal = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            cancel_signal.store(true, Ordering::SeqCst);
        });
        let error = run_worker_process(
            sleeper.clone(),
            temp.path().join("worker.mjs"),
            SecretBytes(Vec::new()),
            None,
            cancelled,
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(error.contains("cancelled"));

        let error = run_worker_process(
            sleeper,
            temp.path().join("worker.mjs"),
            SecretBytes(Vec::new()),
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(40),
        )
        .await
        .unwrap_err();
        assert!(error.contains("deadline"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn debug_managed_host_uses_current_binary_wrapper_and_isolated_manifest_path() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let resource_dir = temp.path().join("resources");
        let session_root = temp.path().join("managed-provider-session");
        let current_exe = temp.path().join("burrata's debug binary");
        fs::write(&current_exe, "debug binary").unwrap();
        let host =
            managed_host_executable(&resource_dir, &session_root, &current_exe, true).unwrap();
        assert_eq!(host, session_root.join("native-host-launcher"));
        let script = fs::read_to_string(&host).unwrap();
        assert!(script.contains("burrata'\\''s debug binary"));
        assert!(script.ends_with(" --browser-native-host\n"));
        assert_eq!(
            fs::metadata(&host).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let dirs = managed_host_manifest_dirs(&session_root);
        assert!(dirs
            .iter()
            .any(|dir| { dir == &session_root.join("config/chromium/NativeMessagingHosts") }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_host_wins_and_release_build_does_not_use_debug_wrapper_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let resource_dir = temp.path().join("resources");
        let bundled = resource_dir
            .join("tools")
            .join(tools::target_triple())
            .join("burrata-browser-host");
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, "packaged host").unwrap();
        let current_exe = temp.path().join("burrata");
        fs::write(&current_exe, "debug binary").unwrap();
        assert_eq!(
            managed_host_executable(
                &resource_dir,
                &temp.path().join("session"),
                &current_exe,
                true,
            )
            .unwrap(),
            bundled
        );

        fs::remove_file(&bundled).unwrap();
        let error = managed_host_executable(
            &resource_dir,
            &temp.path().join("session"),
            &current_exe,
            false,
        )
        .unwrap_err();
        assert!(error.contains("Bundled browser companion host is missing"));
        assert!(!temp.path().join("session/native-host-launcher").exists());
    }

    #[cfg(unix)]
    #[test]
    fn fake_browser_gets_only_profile_scoped_arguments_and_stops() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let args_file = temp.path().join("args");
        let binary = temp.path().join("fake-chromium");
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}.tmp'\nsleep 0.05\nprintf 'XDG_CONFIG_HOME=%s\\nXDG_DATA_HOME=%s\\nXDG_CACHE_HOME=%s\\nXDG_STATE_HOME=%s\\nBURRATA_APP_DATA=%s\\n' \"$XDG_CONFIG_HOME\" \"$XDG_DATA_HOME\" \"$XDG_CACHE_HOME\" \"$XDG_STATE_HOME\" \"$BURRATA_APP_DATA\" >> '{}.tmp'\nmv '{}.tmp' '{}'\nsleep 30\n",
                args_file.display(),
                args_file.display(),
                args_file.display(),
                args_file.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let profile = temp.path().join("isolated-profile");
        let extension = temp.path().join("extension");
        let config_home = temp.path().join("isolated-config");
        let app_data = temp.path().join("app-data");
        let mut child = Some(
            spawn_browser(
                &binary,
                &profile,
                &extension,
                &config_home,
                &app_data,
                "spotify",
                true,
            )
            .unwrap(),
        );
        for _ in 0..100 {
            if args_file.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // The fixture atomically publishes only after both argument and
        // environment captures are complete. File creation alone is too early.
        let args = fs::read_to_string(args_file);
        stop_child(&mut child);
        assert!(child.is_none());
        let args = args.unwrap();
        assert!(args.contains(&format!("--user-data-dir={}\n", profile.display())));
        assert!(args.contains(&format!("--load-extension={}\n", extension.display())));
        assert!(args.contains("--headless=new"));
        assert!(args.contains("https://open.spotify.com/"));
        #[cfg(target_os = "linux")]
        {
            assert!(args.contains(&format!("XDG_CONFIG_HOME={}\n", config_home.display())));
            assert!(args.contains("XDG_DATA_HOME="));
            assert!(args.contains("XDG_CACHE_HOME="));
            assert!(args.contains("XDG_STATE_HOME="));
            assert!(args.contains(&format!("BURRATA_APP_DATA={}\n", app_data.display())));
        }
        assert!(!args.to_lowercase().contains("token"));
    }

    #[test]
    fn spotify_credential_fingerprint_matches_hmac_sha256_and_changes_with_process_key() {
        assert_eq!(
            spotify_credential_fingerprint(&[0x0b; 32], b"Hi There"),
            [
                25, 138, 96, 126, 180, 75, 251, 198, 153, 3, 160, 241, 207, 43, 189, 197, 186, 10,
                163, 243, 217, 174, 60, 28, 122, 59, 22, 150, 160, 182, 140, 247
            ]
        );
        assert_ne!(
            spotify_credential_fingerprint(&[1; 32], b"fixture-cookie"),
            spotify_credential_fingerprint(&[2; 32], b"fixture-cookie")
        );
    }

    #[test]
    fn cursor_reset_failure_is_finite_sanitized_and_does_not_invalidate_auth() {
        let failure = NativeWorkerFailure::from_worker(
            Some("CURSOR_RESET_REQUIRED"),
            Some("https://private.example/profile?token=secret"),
            Some("private-operation"),
            Some(9999),
            Some(300),
        );
        assert_eq!(failure.kind, NativeWorkerFailureKind::CursorResetRequired);
        assert!(!failure.invalidates_token());
        assert_eq!(failure.retry_after_seconds, None);
        assert_eq!(failure.to_string(),"CURSOR_RESET_REQUIRED: Spotify playlist listing changed. Refresh the profile and retry.");
        assert!(!failure.to_string().contains("private"));
        assert!(!failure.to_string().contains("secret"));
    }

    #[test]
    fn rate_limit_diagnostic_keeps_retry_phase_and_http_status() {
        let error = NativeWorkerFailure::from_worker(
            Some("RATE_LIMITED"),
            Some("owner_profile"),
            None,
            Some(429),
            Some(30),
        );
        assert_eq!(
            error.to_string(),
            "RATE_LIMITED: Spotify is rate limiting requests. Try again in 30 seconds. (phase=owner_profile, HTTP 429)"
        );
    }

    #[test]
    fn cached_auth_failure_invalidation_is_scoped_to_auth_rejection() {
        assert!(NativeWorkerFailure::from_worker(
            Some("SIGN_IN_REQUIRED"),
            Some("account_profile"),
            None,
            Some(401),
            None
        )
        .invalidates_token());
        for code in [
            "RATE_LIMITED",
            "REQUEST_TIMEOUT",
            "API_DENIED",
            "VERIFICATION_REQUIRED",
        ] {
            assert!(!NativeWorkerFailure::from_worker(
                Some(code),
                Some("catalog_graphql"),
                None,
                Some(429),
                Some(30)
            )
            .invalidates_token());
        }
        assert_eq!(
            NativeWorkerFailure::from_worker(Some("RATE_LIMITED"), None, None, None, Some(3601))
                .retry_after_seconds,
            None
        );
        assert_eq!(
            NativeWorkerFailure::from_worker(Some("RATE_LIMITED"), None, None, None, Some(30))
                .retry_after_seconds,
            Some(30)
        );
        let diagnostic = NativeWorkerFailure::from_worker(
            Some("SIGN_IN_REQUIRED"),
            Some("account_profile"),
            None,
            Some(401),
            None,
        )
        .to_string();
        assert!(diagnostic.contains("phase=account_profile, HTTP 401"));
        let malicious = NativeWorkerFailure::from_worker(
            Some("SIGN_IN_REQUIRED"),
            Some("token=private"),
            Some("https://attacker.example/private"),
            Some(999),
            None,
        )
        .to_string();
        assert!(!malicious.contains("token=private"));
        assert!(!malicious.contains("HTTP 999"));
        let artist = NativeWorkerFailure::from_worker(
            Some("PROVIDER_CHANGED"),
            Some("catalog_graphql"),
            Some("queryArtistDiscographyAll"),
            Some(200),
            None,
        )
        .to_string();
        assert!(artist.contains("operation=queryArtistDiscographyAll"));
        for operation in ["getAlbum", "queryAlbumTracks"] {
            let album = NativeWorkerFailure::from_worker(
                Some("PROVIDER_CHANGED"),
                Some("catalog_graphql"),
                Some(operation),
                Some(200),
                None,
            )
            .to_string();
            assert!(album.contains(&format!("operation={operation}")));
            assert!(album.contains("phase=catalog_graphql"));
            assert!(album.contains("HTTP 200"));
            let wrong_phase = NativeWorkerFailure::from_worker(
                Some("PROVIDER_CHANGED"),
                Some("account_profile"),
                Some(operation),
                Some(200),
                None,
            )
            .to_string();
            assert!(!wrong_phase.contains("operation="));
        }

        let untrusted_operation = NativeWorkerFailure::from_worker(
            Some("PROVIDER_CHANGED"),
            Some("catalog_graphql"),
            Some("https://attacker.example/private"),
            Some(200),
            None,
        )
        .to_string();
        assert!(!untrusted_operation.contains("attacker.example"));
    }

    #[tokio::test]
    async fn typed_worker_preserves_spotify_diagnostics_and_transport_classes() {
        let temp = tempfile::tempdir().unwrap();
        let worker = temp.path().join("spotify-error.mjs");
        fs::write(&worker, "process.stdout.write(JSON.stringify({ok:false,error:{code:'SIGN_IN_REQUIRED',message:'private details must not escape',phase:'account_profile',httpStatus:401}}));").unwrap();
        let error = run_worker_process_typed(
            "node".into(),
            worker.clone(),
            SecretBytes(b"{}".to_vec()),
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, NativeWorkerFailureKind::SignInRequired);
        assert!(error.invalidates_token());
        let safe = error.to_string();
        assert!(safe.contains("phase=account_profile, HTTP 401"));
        assert!(!safe.contains("private details"));

        fs::write(&worker, "process.stdout.write('x'.repeat(2*1024*1024+1));").unwrap();
        let error = run_worker_process_typed(
            "node".into(),
            worker.clone(),
            SecretBytes(b"{}".to_vec()),
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.transport_reason,
            Some(NativeTransportFailure::Oversize)
        );
        assert!(error.to_string().contains("size limit"));

        fs::write(&worker, "setInterval(()=>{},1000);").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_task = tokio::spawn(run_worker_process_typed(
            "node".into(),
            worker,
            SecretBytes(b"{}".to_vec()),
            None,
            stop.clone(),
            Duration::from_secs(5),
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.store(true, Ordering::SeqCst);
        let error = worker_task.await.unwrap().unwrap_err();
        assert_eq!(error.kind, NativeWorkerFailureKind::Cancelled);
    }

    #[tokio::test]
    async fn legacy_youtube_worker_keeps_its_provider_message_and_phase() {
        let temp = tempfile::tempdir().unwrap();
        let worker = temp.path().join("youtube-error.mjs");
        fs::write(&worker, "process.stdout.write(JSON.stringify({ok:false,error:{code:'RATE_LIMITED',message:'YouTube temporarily limited this request. Try again shortly. (phase=innertube, HTTP 429)',phase:'innertube',httpStatus:429}}));").unwrap();
        let error = run_worker_process(
            "node".into(),
            worker,
            SecretBytes(b"{}".to_vec()),
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(error.contains("YouTube temporarily limited this request"));
        assert!(error.contains("phase=innertube"));
        assert!(error.contains("HTTP 429"));
    }
    #[test]
    fn spotify_keyring_store_failure_does_not_read_or_confirm() {
        let result = verify_persisted_spotify_cookie(
            "fixture-cookie",
            || Err::<(), _>("locked"),
            || -> Result<String, &str> { panic!("read after failed store") },
        );
        assert_eq!(result.err().unwrap(), SPOTIFY_STORAGE_ERROR);
    }

    #[test]
    fn spotify_keyring_read_failure_does_not_confirm() {
        let result = verify_persisted_spotify_cookie(
            "fixture-cookie",
            || Ok(()),
            || Err::<String, _>("unavailable"),
        );
        assert_eq!(result.err().unwrap(), SPOTIFY_STORAGE_ERROR);
    }

    #[test]
    fn spotify_keyring_read_mismatch_does_not_confirm() {
        let result = verify_persisted_spotify_cookie(
            "fixture-cookie",
            || Ok::<(), &str>(()),
            || Ok("stale-cookie".to_owned()),
        );
        assert_eq!(result.err().unwrap(), SPOTIFY_STORAGE_ERROR);
    }

    #[test]
    fn spotify_keyring_verified_store_returns_the_saved_cookie() {
        let stored = std::cell::Cell::new(false);
        let result = verify_persisted_spotify_cookie(
            "fixture-cookie",
            || {
                stored.set(true);
                Ok::<(), &str>(())
            },
            || {
                assert!(stored.get(), "readback must follow storage");
                Ok("fixture-cookie".to_owned())
            },
        );
        assert_eq!(result.ok().unwrap().0, "fixture-cookie");
    }

    #[test]
    fn youtube_keyring_failures_preserve_actionable_storage_error() {
        let failed_store = verify_persisted_provider_credential(
            "youtube-fixture",
            YOUTUBE_STORAGE_ERROR,
            || Err::<(), _>("locked"),
            || -> Result<String, &str> { panic!("read after failed store") },
        );
        assert_eq!(failed_store.err().unwrap(), YOUTUBE_STORAGE_ERROR);
        let failed_read = verify_persisted_provider_credential(
            "youtube-fixture",
            YOUTUBE_STORAGE_ERROR,
            || Ok(()),
            || Err::<String, _>("unavailable"),
        );
        assert_eq!(failed_read.err().unwrap(), YOUTUBE_STORAGE_ERROR);
        let mismatch = verify_persisted_provider_credential(
            "youtube-fixture",
            YOUTUBE_STORAGE_ERROR,
            || Ok::<(), &str>(()),
            || Ok("stale".to_owned()),
        );
        assert_eq!(mismatch.err().unwrap(), YOUTUBE_STORAGE_ERROR);
    }

    #[test]
    fn youtube_keyring_readback_precedes_verified_credential_use() {
        let stage = std::cell::Cell::new(0);
        let saved = verify_persisted_provider_credential(
            "youtube-fixture",
            YOUTUBE_STORAGE_ERROR,
            || {
                stage.set(1);
                Ok::<(), &str>(())
            },
            || {
                assert_eq!(stage.get(), 1);
                stage.set(2);
                Ok("youtube-fixture".to_owned())
            },
        )
        .ok()
        .unwrap();
        assert_eq!(stage.get(), 2);
        assert_eq!(saved.0, "youtube-fixture");
    }
    #[test]
    fn creation_unknown_preserves_finite_http_diagnostics_without_retry_advice() {
        // Captured from the real JS executeJob fixture with a 403 create response.
        let reply: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/spotify-create-http403.json"
        ))
        .unwrap();
        let error = &reply["error"];
        let failure = NativeWorkerFailure::from_worker(
            error["code"].as_str(),
            error["phase"].as_str(),
            error["operationName"].as_str(),
            error["httpStatus"].as_u64(),
            None,
        );
        let message = failure.to_string();
        assert!(message.contains("OUTCOME_UNKNOWN"));
        assert!(message.contains("HTTP 403"));
        assert!(message.contains("phase=playlist_create"));
        assert!(message.contains("Do not retry"));
        assert!(!message.contains("Try again"));
    }
}

#[cfg(test)]
mod verification_creation_receipt_tests {
    use super::*;
    fn receipt(dispatched: Value, playlist_id: Value) -> Value {
        serde_json::json!({"version":1,"requestId":"0123456789abcdef0123456789abcdef","mutationDispatched":dispatched,"phase":"playlist_creation","playlistId":playlist_id})
    }
    #[test]
    fn receipt_retains_explicit_predispatch_and_postdispatch_facts() {
        let before =
            bounded_creation_receipt(&receipt(serde_json::json!(false), Value::Null)).unwrap();
        assert_eq!(before["mutationDispatched"], false);
        let after =
            bounded_creation_receipt(&receipt(serde_json::json!(true), Value::Null)).unwrap();
        assert_eq!(after["mutationDispatched"], true);
        let unknown = bounded_creation_receipt(&receipt(Value::Null, Value::Null)).unwrap();
        assert!(unknown["mutationDispatched"].is_null());
    }
    #[test]
    fn receipt_is_bound_to_native_attempt_and_missing_is_unknown() {
        let value = receipt(serde_json::json!(false), Value::Null);
        assert!(bound_creation_receipt(&value, "0123456789abcdef0123456789abcdef").is_some());
        assert!(bound_creation_receipt(&value, "abcdef0123456789abcdef0123456789").is_none());
        assert!(bound_creation_receipt(&Value::Null, "0123456789abcdef0123456789abcdef").is_none());
        assert!(bounded_creation_receipt(&receipt(
            serde_json::json!(false),
            serde_json::json!("PLverification1234567890")
        ))
        .is_none());
    }
    #[tokio::test]
    async fn typed_worker_receipt_preserves_dispatch_without_string_framing_and_rejects_wrong_attempt(
    ) {
        let temp = tempfile::tempdir().unwrap();
        let worker = temp.path().join("creation-receipt.mjs");
        for (dispatched, matched) in [(false, true), (true, true), (false, false)] {
            let receipt = receipt(serde_json::json!(dispatched), Value::Null);
            let response = serde_json::json!({"ok":false,"error":{"code":"OUTCOME_UNKNOWN","message":"private raw provider detail","phase":"playlist_create","httpStatus":403,"creationReceipt":receipt}});
            fs::write(
                &worker,
                format!(
                    "process.stdout.write({});",
                    serde_json::to_string(&response.to_string()).unwrap()
                ),
            )
            .unwrap();
            let request = serde_json::json!({"operation":"createVerificationPlaylist","creationRequestId":if matched {"0123456789abcdef0123456789abcdef"} else {"abcdef0123456789abcdef0123456789"}});
            let failure = run_worker_process_typed(
                "node".into(),
                worker.clone(),
                SecretBytes(serde_json::to_vec(&request).unwrap()),
                None,
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
            assert_eq!(failure.creation_receipt.is_some(), matched);
            if matched {
                assert_eq!(
                    failure.creation_receipt.as_ref().unwrap()["mutationDispatched"],
                    dispatched
                );
            }
            let public = failure.to_string();
            assert!(!public.contains("private raw"));
            assert!(!public.contains("0123456789abcdef"));
            assert!(!public.contains("CREATION_RECEIPT="));
        }
    }
    #[test]
    fn receipt_rejects_secret_fields_and_unbounded_or_malformed_values() {
        let valid = receipt(
            serde_json::json!(true),
            serde_json::json!("PLverification1234567890"),
        );
        assert!(bounded_creation_receipt(&valid).is_some());
        for (key, value) in [
            ("requestId", serde_json::json!("https://private.invalid")),
            ("phase", serde_json::json!("credential_private")),
            ("mutationDispatched", serde_json::json!("false")),
            ("playlistId", serde_json::json!("https://private.invalid")),
            ("version", serde_json::json!(2)),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(bounded_creation_receipt(&bad).is_none());
        }
        let mut extra = valid.clone();
        extra["cookie"] = serde_json::json!("not allowed");
        assert!(bounded_creation_receipt(&extra).is_none());
        assert!(bounded_creation_receipt(&Value::Null).is_none());
    }
}

#[cfg(test)]
mod verification_intent_account_fence_tests {
    use super::*;
    #[tokio::test]
    async fn changed_account_cancels_actual_creation_worker_before_spawn() {
        let fences = verification_creation_intent::AccountFences::default();
        let guard = fences
            .register("spotify", "account", 1, "account", 1)
            .unwrap();
        // Simulate completion of an auth prerequisite, then record_account's identity transition.
        fences.invalidate("spotify").unwrap();
        let request=SecretBytes(serde_json::to_vec(&serde_json::json!({"operation":"createVerificationPlaylist","creationRequestId":"0123456789abcdef0123456789abcdef"})).unwrap());
        let error = run_worker_process_typed(
            PathBuf::from("nonexistent-node-never-spawn"),
            PathBuf::from("unused-worker"),
            request,
            Some(guard.cancel_flag()),
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, NativeWorkerFailureKind::Cancelled);
        assert!(error.transport_reason.is_none());
        assert!(error.creation_receipt.is_none());
    }
}
