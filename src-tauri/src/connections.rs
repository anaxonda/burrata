//! Persisted connection preferences and OS-keyring credentials.
use crate::storage::Store;
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const KEYRING_SERVICE: &str = "burrata";
const SETTINGS_KEY: &str = "connections";
const SESSION_ONLY_KEY: &str = "connections_session_only";

trait CredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
    fn set(&self, provider: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, provider: &str) -> Result<(), String>;
}

struct OsCredentialStore;

impl CredentialStore for OsCredentialStore {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        let entry = Entry::new(KEYRING_SERVICE, provider).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn set(&self, provider: &str, secret: &str) -> Result<(), String> {
        Entry::new(KEYRING_SERVICE, provider)
            .map_err(|e| e.to_string())?
            .set_password(secret)
            .map_err(|e| e.to_string())
    }

    fn delete(&self, provider: &str) -> Result<(), String> {
        let entry = Entry::new(KEYRING_SERVICE, provider).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SlskdSettings {
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub downloads_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsTranslationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub target_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSettings {
    #[serde(default)]
    pub slskd: SlskdSettings,
    #[serde(default)]
    pub discogs_enabled: bool,
    #[serde(default)]
    pub lastfm_enabled: bool,
    #[serde(default)]
    pub acoustid_enabled: bool,
    #[serde(default)]
    pub lyrics_translation: LyricsTranslationSettings,
}

#[derive(Clone)]
pub struct SlskdConnection {
    pub base_url: String,
    pub api_key: String,
    pub downloads_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionUpdate {
    pub settings: ConnectionSettings,
    /// Empty string clears the saved secret. `None` leaves it unchanged.
    pub slskd_api_key: Option<String>,
    pub discogs_token: Option<String>,
    pub lastfm_api_key: Option<String>,
    #[serde(default)]
    pub acoustid_api_key: Option<String>,
    /// OpenAI-compatible lyrics translation key; never stored in SQLite/settings.
    #[serde(default)]
    pub lyrics_translation_api_key: Option<String>,
    /// When true, changed credentials are held only in memory for this run.
    #[serde(default)]
    pub session_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub present: bool,
    pub session_only: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub settings: ConnectionSettings,
    pub slskd_credential: CredentialStatus,
    pub discogs_credential: CredentialStatus,
    pub lastfm_credential: CredentialStatus,
    pub acoustid_credential: CredentialStatus,
    #[serde(default)]
    pub lyrics_translation_credential: CredentialStatus,
}

/// Avoid storing credentials in Settings or SQLite. Session credentials are an
/// explicit user choice and live only in this process.
pub struct ConnectionManager {
    store: Arc<Store>,
    session: Mutex<HashMap<String, String>>,
    keyring: Arc<dyn CredentialStore>,
}

impl ConnectionManager {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            session: Mutex::new(HashMap::new()),
            keyring: Arc::new(OsCredentialStore),
        }
    }

    #[cfg(test)]
    fn with_keyring(store: Arc<Store>, keyring: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            session: Mutex::new(HashMap::new()),
            keyring,
        }
    }

    pub fn settings(&self) -> Result<ConnectionSettings, String> {
        self.store
            .get_json_preference(SETTINGS_KEY)?
            .map(serde_json::from_value)
            .transpose()
            .map(|settings| settings.unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    pub fn status(&self) -> Result<ConnectionStatus, String> {
        let settings = self.settings()?;
        Ok(ConnectionStatus {
            slskd_credential: self.credential_status_if_enabled("slskd", settings.slskd.enabled),
            discogs_credential: self
                .credential_status_if_enabled("discogs", settings.discogs_enabled),
            lastfm_credential: self.credential_status_if_enabled("lastfm", settings.lastfm_enabled),
            acoustid_credential: self
                .credential_status_if_enabled("acoustid", settings.acoustid_enabled),
            lyrics_translation_credential: self.credential_status_if_enabled(
                "lyrics_translation",
                settings.lyrics_translation.enabled,
            ),
            settings,
        })
    }

    pub fn update(&self, update: ConnectionUpdate) -> Result<ConnectionStatus, String> {
        update.settings.validate_lyrics_translation()?;
        for (provider, value) in [
            ("slskd", update.slskd_api_key),
            ("discogs", update.discogs_token),
            ("lastfm", update.lastfm_api_key),
            ("acoustid", update.acoustid_api_key),
            ("lyrics_translation", update.lyrics_translation_api_key),
        ] {
            if let Some(value) = value {
                self.set_credential(&provider, &value, update.session_only)?;
            }
        }
        self.store.set_json_preference(
            SETTINGS_KEY,
            &serde_json::to_value(update.settings).map_err(|e| e.to_string())?,
        )?;
        self.status()
    }

    pub fn credential(&self, provider: &str) -> Result<Option<String>, String> {
        let username = credential_name(provider)?;
        if let Some(secret) = self
            .session
            .lock()
            .map_err(|e| e.to_string())?
            .get(username)
        {
            return Ok(Some(secret.clone()));
        }
        if self.is_session_only_provider(username)? {
            return Ok(None);
        }
        self.keyring.get(username).map_err(|error| {
            format!("Cannot read {provider} credential from the system keyring: {error}")
        })
    }

    pub fn set_credential(
        &self,
        provider: &str,
        secret: &str,
        session_only: bool,
    ) -> Result<(), String> {
        let username = credential_name(provider)?;
        if secret.is_empty() {
            self.session
                .lock()
                .map_err(|e| e.to_string())?
                .remove(username);
            if let Err(error) = self.keyring.delete(username) {
                // Suppress any old persistent credential even if the keyring is
                // locked; never fall back to it on the next app launch.
                self.set_session_only_provider(username, true)?;
                return Err(error);
            }
            self.set_session_only_provider(username, false)?;
            return Ok(());
        }
        if session_only {
            // Persist only a non-secret suppression marker so an older saved
            // token cannot silently return after this temporary token expires.
            self.set_session_only_provider(username, true)?;
            self.session
                .lock()
                .map_err(|e| e.to_string())?
                .insert(username.into(), secret.into());
            // Session mode must work when the keyring is unavailable. Clear an
            // older saved credential when possible; the in-memory value wins.
            let _ = self.keyring.delete(username);
        } else {
            self.keyring.set(username, secret).map_err(|e| {
                format!("Cannot save {provider} credential to the system keyring: {e}")
            })?;
            self.session
                .lock()
                .map_err(|e| e.to_string())?
                .remove(username);
            self.set_session_only_provider(username, false)?;
        }
        Ok(())
    }

    pub fn credential_status(&self, provider: &str) -> Result<CredentialStatus, String> {
        let username = credential_name(provider)?;
        if self
            .session
            .lock()
            .map_err(|e| e.to_string())?
            .contains_key(username)
        {
            return Ok(CredentialStatus {
                present: true,
                session_only: true,
                error: None,
            });
        }
        if self.is_session_only_provider(username)? {
            return Ok(CredentialStatus {
                present: false,
                session_only: true,
                error: None,
            });
        }
        match self.credential(provider) {
            Ok(secret) => Ok(CredentialStatus {
                present: secret.is_some(),
                session_only: false,
                error: None,
            }),
            Err(error) => Ok(CredentialStatus {
                present: false,
                session_only: false,
                error: Some(error),
            }),
        }
    }

    fn credential_status_if_enabled(&self, provider: &str, enabled: bool) -> CredentialStatus {
        if !enabled {
            return CredentialStatus::default();
        }
        self.credential_status(provider)
            .unwrap_or_else(|error| CredentialStatus {
                present: false,
                session_only: false,
                error: Some(error),
            })
    }

    fn is_session_only_provider(&self, provider: &str) -> Result<bool, String> {
        let providers = self
            .store
            .get_json_preference(SESSION_ONLY_KEY)?
            .unwrap_or_else(|| serde_json::json!([]));
        Ok(providers
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(provider))))
    }

    fn set_session_only_provider(&self, provider: &str, enabled: bool) -> Result<(), String> {
        let mut providers = self
            .store
            .get_json_preference(SESSION_ONLY_KEY)?
            .unwrap_or_else(|| serde_json::json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        providers.retain(|item| item.as_str() != Some(provider));
        if enabled {
            providers.push(serde_json::Value::String(provider.into()));
        }
        self.store
            .set_json_preference(SESSION_ONLY_KEY, &serde_json::Value::Array(providers))
    }

    pub fn slskd(&self) -> Result<Option<SlskdConnection>, String> {
        let settings = self.settings()?.slskd;
        if !settings.enabled {
            return Ok(None);
        }
        let parsed = url::Url::parse(settings.base_url.trim())
            .map_err(|_| "slskd URL must be a valid HTTP(S) URL".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "slskd URL must be an HTTP(S) base URL without credentials or query parameters"
                    .into(),
            );
        }
        let api_key = self
            .credential("slskd")?
            .ok_or("Enter a slskd API key in Connections")?;
        Ok(Some(SlskdConnection {
            base_url: settings.base_url.trim().trim_end_matches('/').into(),
            api_key,
            downloads_path: settings.downloads_path,
        }))
    }

    pub fn lyrics_translation(&self) -> Result<Option<LyricsTranslationConnection>, String> {
        let settings = self.settings()?.lyrics_translation;
        if !settings.enabled {
            return Ok(None);
        }
        settings.validate()?;
        let api_key = self
            .credential("lyrics_translation")?
            .ok_or("Enter a translation API key in Connections")?;
        Ok(Some(LyricsTranslationConnection {
            base_url: settings.base_url,
            model: settings.model,
            target_language: settings.target_language,
            api_key,
        }))
    }
}

#[derive(Clone)]
pub struct LyricsTranslationConnection {
    pub base_url: String,
    pub model: String,
    pub target_language: String,
    pub api_key: String,
}

impl ConnectionSettings {
    fn validate_lyrics_translation(&self) -> Result<(), String> {
        self.lyrics_translation.validate()
    }
}

impl LyricsTranslationSettings {
    fn validate(&self) -> Result<(), String> {
        let base_url = self.base_url.trim();
        let model = self.model.trim();
        let language = self.target_language.trim();
        if base_url.len() > 512
            || model.len() > 128
            || language.len() > 80
            || [base_url, model, language]
                .iter()
                .any(|value| value.chars().any(char::is_control))
        {
            return Err("Lyrics translation settings exceed their limits".into());
        }
        if !base_url.is_empty() {
            let parsed = url::Url::parse(base_url).map_err(|_| {
                "Translation base URL must be an HTTPS API URL or loopback HTTP URL"
            })?;
            let loopback_http = parsed.scheme() == "http"
                && parsed.host_str().is_some_and(|host| {
                    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
                });
            if (parsed.scheme() != "https" && !loopback_http)
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err("Translation base URL must use HTTPS, or HTTP on loopback, without credentials or query parameters".into());
            }
        }
        if self.enabled && (base_url.is_empty() || model.is_empty() || language.is_empty()) {
            return Err(
                "Set a translation API URL, model, and target language before enabling translation"
                    .into(),
            );
        }
        Ok(())
    }
}

fn credential_name(provider: &str) -> Result<&'static str, String> {
    match provider {
        "slskd" => Ok("slskd"),
        "discogs" => Ok("discogs"),
        "lastfm" => Ok("lastfm"),
        "acoustid" => Ok("acoustid"),
        "lyrics_translation" => Ok("lyrics_translation"),
        _ => Err("Unknown credential provider".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingKeyring;
    impl CredentialStore for FailingKeyring {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("keyring locked".into())
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Err("keyring locked".into())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Err("keyring locked".into())
        }
    }

    #[derive(Default)]
    struct MemoryKeyring(Mutex<HashMap<String, String>>);
    impl CredentialStore for MemoryKeyring {
        fn get(&self, provider: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(provider).cloned())
        }
        fn set(&self, provider: &str, secret: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(provider.into(), secret.into());
            Ok(())
        }
        fn delete(&self, provider: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(provider);
            Ok(())
        }
    }

    #[test]
    fn explicit_session_secret_works_when_keyring_fails_and_suppresses_stale_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let manager = ConnectionManager::with_keyring(store.clone(), Arc::new(FailingKeyring));
        manager
            .set_credential("slskd", "temporary-secret", true)
            .unwrap();
        assert_eq!(
            manager.credential("slskd").unwrap().as_deref(),
            Some("temporary-secret")
        );
        let status = manager.credential_status("slskd").unwrap();
        assert!(status.present && status.session_only);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("temporary-secret"));
        drop(manager);

        let after_restart = ConnectionManager::with_keyring(store, Arc::new(FailingKeyring));
        assert_eq!(after_restart.credential("slskd").unwrap(), None);
        let status = after_restart.credential_status("slskd").unwrap();
        assert!(!status.present && status.session_only);
    }

    #[test]
    fn lyrics_translation_secret_uses_keyring_and_status_never_serializes_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("library.sqlite")).unwrap());
        let keyring = Arc::new(MemoryKeyring::default());
        let manager = ConnectionManager::with_keyring(store.clone(), keyring.clone());
        let mut settings = ConnectionSettings::default();
        settings.lyrics_translation = LyricsTranslationSettings {
            enabled: true,
            base_url: "https://api.example.test/v1".into(),
            model: "translate-model".into(),
            target_language: "fr".into(),
        };
        let status = manager
            .update(ConnectionUpdate {
                settings,
                lyrics_translation_api_key: Some("translation-secret".into()),
                ..ConnectionUpdate::default()
            })
            .unwrap();
        assert!(status.lyrics_translation_credential.present);
        assert!(!status.lyrics_translation_credential.session_only);
        assert!(!serde_json::to_string(&status)
            .unwrap()
            .contains("translation-secret"));
        assert!(!store
            .get_json_preference(SETTINGS_KEY)
            .unwrap()
            .unwrap()
            .to_string()
            .contains("translation-secret"));
        assert_eq!(
            keyring.get("lyrics_translation").unwrap().as_deref(),
            Some("translation-secret")
        );
        let connection = manager.lyrics_translation().unwrap().unwrap();
        assert_eq!(connection.model, "translate-model");
        assert_eq!(connection.target_language, "fr");
        assert_eq!(connection.api_key, "translation-secret");
    }

    #[test]
    fn lyrics_translation_settings_allow_https_or_loopback_http_only() {
        let good = [
            "https://api.example.test/v1",
            "http://localhost:1234/v1",
            "http://127.0.0.1:1234/v1",
        ];
        for base_url in good {
            let settings = LyricsTranslationSettings {
                enabled: true,
                base_url: base_url.into(),
                model: "model".into(),
                target_language: "en".into(),
            };
            assert!(settings.validate().is_ok(), "{base_url}");
        }
        for base_url in [
            "http://example.test/v1",
            "file:///tmp/api",
            "https://user:secret@example.test/v1",
            "https://example.test/v1?token=secret",
        ] {
            let settings = LyricsTranslationSettings {
                enabled: true,
                base_url: base_url.into(),
                model: "model".into(),
                target_language: "en".into(),
            };
            assert!(settings.validate().is_err(), "{base_url}");
        }
    }
}

/// Import the user's existing OneTagger Discogs token without exposing it to the UI.
pub fn import_onetagger_discogs(
    manager: &ConnectionManager,
    session_only: bool,
) -> Result<ConnectionStatus, String> {
    let path = dirs::config_dir()
        .ok_or("Cannot locate application settings")?
        .join("onetagger/settings.json");
    let metadata = std::fs::metadata(&path)
        .map_err(|_| "OneTagger settings were not found on this computer")?;
    if metadata.len() > 4 * 1024 * 1024 {
        return Err("OneTagger settings exceed the size limit".into());
    }
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).map_err(|_| "Cannot read OneTagger settings")?,
    )
    .map_err(|_| "Cannot parse OneTagger settings")?;
    let token = value
        .pointer("/ui/autoTaggerConfig/custom/discogs/token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or("OneTagger has no saved Discogs token")?;
    let mut settings = manager.settings()?;
    settings.discogs_enabled = true;
    manager.update(ConnectionUpdate {
        settings,
        discogs_token: Some(token.to_owned()),
        session_only,
        ..ConnectionUpdate::default()
    })
}
