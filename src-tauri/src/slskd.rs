//! Small async client for an already configured slskd daemon.
//!
//! Credentials are supplied by the caller and are only held in memory for the
//! lifetime of this client. This module never starts or configures a daemon.
use reqwest::{header, Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct SlskdClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub id: String,
    pub search_text: String,
    pub search_timeout: u32,
    pub response_limit: u32,
    pub file_limit: u32,
    pub filter_responses: bool,
    pub minimum_response_file_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedFolder {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub local_path: String,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub directories: u64,
    #[serde(default)]
    pub excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedShareFolder {
    pub id: String,
    pub alias: String,
    pub directory_path: String,
}

/// Replace daemon-generated IDs for shares created by the managed settings
/// controls, so UI remove actions pass the corresponding configuration ID.
pub fn associate_managed_share_ids(shares: &mut [SharedFolder], managed: &[ManagedShareFolder]) {
    for share in shares {
        if let Some(folder) = managed.iter().find(|folder| {
            let marker = format!("/burrata-shares/{}", folder.id);
            share.local_path.trim_end_matches('/').ends_with(&marker) || share.alias == folder.alias
        }) {
            share.id = folder.id.clone();
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFile {
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub bit_rate: Option<u32>,
    #[serde(default)]
    pub length: Option<f64>,
    #[serde(default)]
    pub attributes: Vec<RemoteAttribute>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAttribute {
    pub attribute: Option<String>,
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub username: String,
    #[serde(default)]
    pub files: Vec<RemoteFile>,
    #[serde(default)]
    pub has_free_upload_slot: Option<bool>,
    #[serde(default)]
    pub queue_length: Option<u32>,
    #[serde(default)]
    pub upload_speed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatus {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub is_complete: Option<bool>,
    #[serde(default)]
    pub responses: Vec<SearchResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueBatchRequest<'a> {
    id: &'a str,
    search_id: Option<&'a str>,
    username: &'a str,
    files: [EnqueueBatchFile<'a>; 1],
    options: EnqueueBatchOptions<'a>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueBatchFile<'a> {
    filename: &'a str,
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueBatchOptions<'a> {
    destination: &'a str,
    external_id: &'a str,
}

impl SlskdClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, String> {
        let parsed = url::Url::parse(base_url.trim()).map_err(|_| "Invalid slskd URL")?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !parsed.query().is_none()
            || !parsed.fragment().is_none()
        {
            return Err("slskd URL must be an HTTP(S) origin without credentials or query".into());
        }
        if parsed.scheme() != "https" && !is_loopback_host(parsed.host_str().unwrap_or_default()) {
            return Err("Use HTTPS for a remote slskd server".into());
        }
        if api_key.trim().is_empty() {
            return Err("slskd API key is not configured".into());
        }
        let mut headers = header::HeaderMap::new();
        let key =
            header::HeaderValue::from_str(api_key.trim()).map_err(|_| "Invalid slskd API key")?;
        headers.insert("X-API-Key", key);
        let client = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| format!("Cannot create slskd HTTP client: {e}"))?;
        Ok(Self {
            base_url: parsed.as_str().trim_end_matches('/').to_owned(),
            client,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/api/v0/{path}", self.base_url)
    }

    async fn response(&self, request: reqwest::RequestBuilder) -> Result<Response, String> {
        let request = request
            .build()
            .map_err(|e| format!("Invalid slskd request: {e}"))?;
        let operation = format!("{} {}", request.method(), request.url().path());
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|e| format!("slskd request failed for {operation}: {e}"))?;
        if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
            return Ok(response);
        }
        let status = response.status();
        // Keep body text bounded and avoid returning request headers or API keys.
        Err(format!("slskd returned HTTP {status} for {operation}"))
    }

    /// Read-only API-key check; does not start or mutate a Soulseek search.
    pub async fn check_connection(&self) -> Result<(), String> {
        let response = self
            .response(self.client.get(self.endpoint("application/version")))
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("slskd returned HTTP {}", response.status()))
        }
    }

    /// Read the daemon's configured local shares. The public slskd API exposes
    /// listing and scanning, but not directory add/remove operations.
    pub async fn shared_folders(&self) -> Result<Vec<SharedFolder>, String> {
        let response = self
            .response(self.client.get(self.endpoint("shares")))
            .await?;
        let value: Value = response
            .json()
            .await
            .map_err(|e| format!("Invalid slskd shares response: {e}"))?;
        let local = value
            .get("local")
            .and_then(Value::as_array)
            .or_else(|| value.as_array())
            .ok_or("Invalid slskd shares response: local shares are missing")?;
        local
            .iter()
            .map(|share| {
                Ok(SharedFolder {
                    id: share["id"].as_str().unwrap_or_default().to_owned(),
                    alias: share["alias"].as_str().unwrap_or_default().to_owned(),
                    local_path: share["localPath"].as_str().unwrap_or_default().to_owned(),
                    remote_path: share["remotePath"].as_str().unwrap_or_default().to_owned(),
                    files: share["files"].as_u64().unwrap_or(0),
                    directories: share["directories"].as_u64().unwrap_or(0),
                    excluded: share["isExcluded"].as_bool().unwrap_or(false),
                })
            })
            .collect()
    }

    /// Rescan the shares already configured in slskd. This never adds paths.
    pub async fn rescan_shared_folders(&self) -> Result<(), String> {
        let response = self
            .response(self.client.put(self.endpoint("shares")))
            .await?;
        let _ = response.bytes().await;
        Ok(())
    }

    pub async fn start_search(&self, query: &str, timeout_seconds: u32) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        let id = Uuid::new_v4().to_string();
        let request = SearchRequest {
            id: id.clone(),
            search_text: query.to_owned(),
            // The API documents seconds, but affected upstream versions interpret
            // this field as milliseconds. Convert to ms so normal searches persist.
            search_timeout: timeout_seconds.clamp(5, 120).saturating_mul(1_000),
            response_limit: 100,
            file_limit: 10_000,
            filter_responses: true,
            minimum_response_file_count: 1,
        };
        let response = self
            .response(self.client.post(self.endpoint("searches")).json(&request))
            .await?;
        // Search may return a full object or no body depending on slskd release;
        // our client-generated GUID remains the durable polling key.
        let _ = response.bytes().await;
        Ok(id)
    }

    pub async fn search_status(&self, id: &str) -> Result<SearchStatus, String> {
        validate_guid(id)?;
        let response = self
            .response(
                self.client
                    .get(self.endpoint(&format!("searches/{id}?includeResponses=true"))),
            )
            .await?;
        response
            .json()
            .await
            .map_err(|e| format!("Invalid slskd search status: {e}"))
    }

    pub async fn search_responses(&self, id: &str) -> Result<Vec<SearchResponse>, String> {
        validate_guid(id)?;
        let response = self
            .response(
                self.client
                    .get(self.endpoint(&format!("searches/{id}/responses"))),
            )
            .await?;
        let value: Value = response
            .json()
            .await
            .map_err(|e| format!("Invalid slskd search results: {e}"))?;
        decode_search_responses(value)
    }

    pub async fn cancel_search(&self, id: &str) -> Result<(), String> {
        validate_guid(id)?;
        let _ = self
            .response(self.client.put(self.endpoint(&format!("searches/{id}"))))
            .await?;
        Ok(())
    }

    pub async fn delete_search(&self, id: &str) -> Result<(), String> {
        validate_guid(id)?;
        let _ = self
            .response(self.client.delete(self.endpoint(&format!("searches/{id}"))))
            .await?;
        Ok(())
    }

    /// Queue one exact remote path. `batch_id` can be the Burrata job ID so a
    /// restart/retry can reconcile against slskd's persisted transfer batch.
    pub async fn enqueue_download(
        &self,
        batch_id: &str,
        search_id: Option<&str>,
        username: &str,
        filename: &str,
        size: u64,
    ) -> Result<Value, String> {
        validate_guid(batch_id)?;
        if let Some(search_id) = search_id {
            validate_guid(search_id)?;
        }
        validate_remote_path(filename)?;
        audio_extension(filename)?;
        if size == 0 || size > 2 * 1024 * 1024 * 1024 {
            return Err("Selected Soulseek file has an invalid size".into());
        }
        let body = EnqueueBatchRequest {
            id: batch_id,
            search_id,
            username,
            files: [EnqueueBatchFile { filename, size }],
            options: EnqueueBatchOptions {
                destination: batch_id,
                external_id: batch_id,
            },
        };
        let response = self
            .response(
                self.client
                    .post(self.endpoint("transfers/downloads/batches"))
                    .json(&body),
            )
            .await?;
        response
            .json()
            .await
            .map_err(|e| format!("Invalid slskd enqueue response: {e}"))
    }

    pub async fn download_batch(&self, batch_id: &str) -> Result<Value, String> {
        validate_guid(batch_id)?;
        self.response(
            self.client
                .get(self.endpoint(&format!("transfers/downloads/batches/{batch_id}"))),
        )
        .await?
        .json()
        .await
        .map_err(|e| format!("Invalid slskd batch response: {e}"))
    }

    pub async fn download_status(
        &self,
        username: &str,
        transfer_id: &str,
    ) -> Result<Value, String> {
        validate_guid(transfer_id)?;
        let username =
            url::form_urlencoded::byte_serialize(username.as_bytes()).collect::<String>();
        self.response(
            self.client
                .get(self.endpoint(&format!("transfers/downloads/{username}/{transfer_id}"))),
        )
        .await?
        .json()
        .await
        .map_err(|e| format!("Invalid slskd transfer status: {e}"))
    }

    pub async fn queue_position(&self, username: &str, transfer_id: &str) -> Result<u64, String> {
        validate_guid(transfer_id)?;
        let username =
            url::form_urlencoded::byte_serialize(username.as_bytes()).collect::<String>();
        self.response(self.client.get(self.endpoint(&format!(
            "transfers/downloads/{username}/{transfer_id}/position"
        ))))
        .await?
        .json()
        .await
        .map_err(|_| "Peer did not return a queue position".into())
    }

    /// Reattach active/successful batches. A terminal failed batch cannot be
    /// restarted by polling it, or by POSTing the same (conflicting) batch ID.
    pub async fn prepare_retry(
        &self,
        source: &mut crate::models::CandidateSource,
    ) -> Result<(), String> {
        let crate::models::CandidateSource::Soulseek {
            batch_id,
            transfer_id,
            search_id,
            ..
        } = source;
        *search_id = None;
        let Some(id) = batch_id.as_deref() else {
            return Ok(());
        };
        let restart = match self.download_batch(id).await {
            Ok(batch) => batch_needs_new_attempt(&batch),
            Err(error) if error.contains("HTTP 404") => true,
            Err(error) => return Err(error),
        };
        if restart {
            *batch_id = Some(Uuid::new_v4().to_string());
            *transfer_id = None;
        }
        Ok(())
    }

    pub async fn cancel_download(&self, username: &str, transfer_id: &str) -> Result<(), String> {
        validate_guid(transfer_id)?;
        let username =
            url::form_urlencoded::byte_serialize(username.as_bytes()).collect::<String>();
        let _ = self
            .response(self.client.delete(self.endpoint(&format!(
                "transfers/downloads/{username}/{transfer_id}?remove=true"
            ))))
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ManagedSharePaths {
    root: PathBuf,
    compose: PathBuf,
    config: PathBuf,
}

static MANAGED_SHARE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn managed_share_paths() -> Result<ManagedSharePaths, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Managed slskd deployment directory is unavailable")?
        .join("deploy/slskd");
    let paths = ManagedSharePaths {
        compose: root.join("compose.yaml"),
        config: root.join("data/slskd.yml"),
        root,
    };
    if !paths.compose.is_file() || !paths.config.is_file() {
        return Err(
            "The managed local slskd deployment is not installed in this application directory"
                .into(),
        );
    }
    Ok(paths)
}

pub fn managed_share_controls_available() -> bool {
    managed_share_paths().is_ok()
}

/// Avoid ever editing the local managed deployment for a remote or otherwise
/// configured slskd service.
pub fn is_managed_slskd_connection(base_url: &str, downloads_path: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url.trim()) else {
        return false;
    };
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        || url.port().unwrap_or(80) != 5030
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Ok(paths) = managed_share_paths() else {
        return false;
    };
    let Ok(expected) = fs::canonicalize(paths.root.join("data/downloads")) else {
        return false;
    };
    let Ok(actual) = fs::canonicalize(downloads_path) else {
        return false;
    };
    actual == expected
}

pub fn list_managed_share_folders() -> Result<Vec<ManagedShareFolder>, String> {
    let _guard = MANAGED_SHARE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Managed slskd share settings lock poisoned")?;
    let paths = managed_share_paths()?;
    let config = fs::read_to_string(&paths.config)
        .map_err(|e| format!("Cannot read managed slskd share settings: {e}"))?;
    let compose = fs::read_to_string(&paths.compose)
        .map_err(|e| format!("Cannot read managed slskd deployment: {e}"))?;
    managed_shares_from_text(&config, &compose)
}

/// Add one explicitly selected host directory to this repository's managed
/// slskd instance. It is mounted read-only and remains opt-in.
pub fn add_managed_share_folder(directory_path: &str) -> Result<Vec<ManagedShareFolder>, String> {
    let _guard = MANAGED_SHARE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Managed slskd share settings lock poisoned")?;
    let paths = managed_share_paths()?;
    let selected = Path::new(directory_path.trim());
    if !selected.is_absolute() {
        return Err("Choose an absolute folder path to share".into());
    }
    let canonical =
        fs::canonicalize(selected).map_err(|e| format!("Cannot access selected folder: {e}"))?;
    if !canonical.is_dir() || canonical.parent().is_none() {
        return Err(
            "The selected path must be a directory; filesystem roots cannot be shared".into(),
        );
    }
    validate_managed_share_path(&canonical, &paths)?;
    let directory_path = canonical
        .to_str()
        .ok_or("Selected folder path is not valid Unicode")?
        .to_owned();
    let id = managed_share_id(&canonical);
    let old_config = fs::read_to_string(&paths.config)
        .map_err(|e| format!("Cannot read managed slskd share settings: {e}"))?;
    let old_compose = fs::read_to_string(&paths.compose)
        .map_err(|e| format!("Cannot read managed slskd deployment: {e}"))?;
    let mut current = managed_shares_from_text(&old_config, &old_compose)?;
    if current.iter().any(|share| share.id == id) {
        return Ok(current);
    }
    let alias = next_share_alias(&current, &old_config)?;
    let (new_config, new_compose) =
        add_share_texts(&old_config, &old_compose, &id, &alias, &directory_path)?;
    apply_managed_share_texts(&paths, &old_config, &old_compose, &new_config, &new_compose)?;
    current.push(ManagedShareFolder {
        id,
        alias,
        directory_path,
    });
    Ok(current)
}

/// Remove only a share created by Burrata from the managed configuration.
/// The original host directory and its contents are never modified.
pub fn remove_managed_share_folder(share_id: &str) -> Result<Vec<ManagedShareFolder>, String> {
    if share_id.len() != 16 || !share_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid managed share ID".into());
    }
    let _guard = MANAGED_SHARE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Managed slskd share settings lock poisoned")?;
    let paths = managed_share_paths()?;
    let old_config = fs::read_to_string(&paths.config)
        .map_err(|e| format!("Cannot read managed slskd share settings: {e}"))?;
    let old_compose = fs::read_to_string(&paths.compose)
        .map_err(|e| format!("Cannot read managed slskd deployment: {e}"))?;
    let current = managed_shares_from_text(&old_config, &old_compose)?;
    if !current.iter().any(|share| share.id == share_id) {
        return Err("Managed share was not found".into());
    }
    let (new_config, new_compose) = remove_share_texts(&old_config, &old_compose, share_id)?;
    apply_managed_share_texts(&paths, &old_config, &old_compose, &new_config, &new_compose)?;
    managed_shares_from_text(&new_config, &new_compose)
}

fn managed_share_id(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_managed_share_path(canonical: &Path, paths: &ManagedSharePaths) -> Result<(), String> {
    let managed_data = fs::canonicalize(paths.root.join("data"))
        .map_err(|e| format!("Cannot validate managed slskd data directory: {e}"))?;
    let downloads = fs::canonicalize(paths.root.join("data/downloads"))
        .map_err(|e| format!("Cannot validate managed slskd downloads directory: {e}"))?;
    if managed_data.starts_with(canonical) {
        return Err("The selected folder contains the managed slskd configuration or credentials and cannot be shared".into());
    }
    if canonical.starts_with(&managed_data) && !canonical.starts_with(&downloads) {
        return Err("Only the explicitly selected slskd downloads folder may be shared from its private data directory".into());
    }
    Ok(())
}

fn next_share_alias(shares: &[ManagedShareFolder], config: &str) -> Result<String, String> {
    let configured = configured_share_aliases(config)?;
    let mut suffix = 1;
    loop {
        let alias = format!("Shared {suffix}");
        if !shares
            .iter()
            .any(|share| share.alias.eq_ignore_ascii_case(&alias))
            && !configured
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&alias))
        {
            return Ok(alias);
        }
        suffix += 1;
    }
}

fn configured_share_aliases(config: &str) -> Result<Vec<String>, String> {
    let (lines, directories_at, list_end) = share_directory_lines(config)?;
    let mut aliases = Vec::new();
    for line in &lines[directories_at + 1..list_end] {
        let Some(raw) = line.trim().strip_prefix("- ") else {
            continue;
        };
        let value = unquote_yaml_scalar(raw);
        let value = value.trim_start_matches(['!', '-']);
        let alias = if let Some((alias, _)) = value.split_once(']') {
            alias.trim_start_matches('[').to_owned()
        } else {
            value
                .rsplit(['/', '\\'])
                .find(|component| !component.is_empty())
                .unwrap_or(value)
                .to_owned()
        };
        if !alias.is_empty() {
            aliases.push(alias);
        }
    }
    Ok(aliases)
}

fn yaml_scalar(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("Folder path contains unsupported control characters".into());
    }
    serde_json::to_string(value).map_err(|e| format!("Cannot encode folder path: {e}"))
}

fn unquote_yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if let Ok(value) = serde_json::from_str::<String>(value) {
        return value;
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_owned()
}

fn share_directory_lines(text: &str) -> Result<(Vec<String>, usize, usize), String> {
    let lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    let shares_at = lines
        .iter()
        .position(|line| line == "shares:")
        .ok_or("Managed slskd YAML has no top-level shares section")?;
    let section_end = (shares_at + 1..lines.len())
        .find(|index| {
            let line = lines[*index].as_str();
            !line.trim().is_empty() && !line.starts_with(' ') && !line.starts_with('#')
        })
        .unwrap_or(lines.len());
    let directories_at = (shares_at + 1..section_end)
        .find(|index| lines[*index] == "  directories:" || lines[*index] == "  directories: []")
        .ok_or("Managed slskd YAML has no shares.directories list")?;
    let list_start = directories_at + 1;
    let list_end = (list_start..section_end)
        .find(|index| {
            let line = lines[*index].as_str();
            !line.trim().is_empty() && !line.starts_with("    ") && !line.starts_with("  #")
        })
        .unwrap_or(section_end);
    Ok((lines, directories_at, list_end))
}

fn managed_shares_from_text(
    config: &str,
    compose: &str,
) -> Result<Vec<ManagedShareFolder>, String> {
    let (lines, directories_at, list_end) = share_directory_lines(config)?;
    let mut entries = Vec::new();
    for line in &lines[directories_at + 1..list_end] {
        let Some(raw) = line.trim().strip_prefix("- ") else {
            continue;
        };
        let value = unquote_yaml_scalar(raw);
        let Some(path) = value.split_once("]/burrata-shares/") else {
            continue;
        };
        let (alias, id) = (path.0.trim_start_matches('['), path.1);
        let id = id.trim_matches('/');
        if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let directory_path = compose_mount_source(compose, id).unwrap_or_default();
        entries.push(ManagedShareFolder {
            id: id.into(),
            alias: alias.into(),
            directory_path,
        });
    }
    Ok(entries)
}

fn add_share_texts(
    config: &str,
    compose: &str,
    id: &str,
    alias: &str,
    directory_path: &str,
) -> Result<(String, String), String> {
    let mut config_lines = config.lines().map(str::to_owned).collect::<Vec<_>>();
    let (config_lines_found, directories_at, list_end) = share_directory_lines(config)?;
    let _ = config_lines_found;
    if config_lines[directories_at].trim_end().ends_with("[]") {
        config_lines[directories_at] = "  directories:".into();
    }
    let entry = format!(
        "    - {}",
        yaml_scalar(&format!("[{alias}]/burrata-shares/{id}"))?
    );
    config_lines.insert(list_end, entry);
    let mut new_config = config_lines.join("\n");
    if config.ends_with('\n') {
        new_config.push('\n');
    }

    let encoded_path = yaml_scalar(directory_path)?;
    let mut compose_lines = compose.lines().map(str::to_owned).collect::<Vec<_>>();
    let service_at = compose_lines
        .iter()
        .position(|line| line == "  slskd:")
        .ok_or("Managed slskd Compose service is missing")?;
    let service_end = (service_at + 1..compose_lines.len())
        .find(|index| {
            compose_lines[*index].starts_with("  ")
                && !compose_lines[*index].starts_with("    ")
                && !compose_lines[*index].trim().is_empty()
        })
        .unwrap_or(compose_lines.len());
    if !compose_lines[service_at..service_end]
        .iter()
        .any(|line| line.trim() == "container_name: burrata-slskd")
    {
        return Err("Compose service is not the managed burrata-slskd instance".into());
    }
    let volumes_at = (service_at + 1..service_end)
        .find(|index| compose_lines[*index].trim() == "volumes:")
        .ok_or("Managed slskd Compose file has no volume list for the service")?;
    let data_mount = (volumes_at + 1..service_end)
        .find(|index| compose_lines[*index].trim() == "- ./data:/app")
        .ok_or("Managed slskd Compose file no longer has its expected /app data mount")?;
    if data_mount < volumes_at {
        return Err("Managed slskd Compose file has no volume list for the service".into());
    }
    compose_lines.splice(
        data_mount + 1..data_mount + 1,
        [
            "      - type: bind".to_owned(),
            format!("        source: {encoded_path}"),
            format!("        target: /burrata-shares/{id}"),
            "        read_only: true".to_owned(),
        ],
    );
    let mut new_compose = compose_lines.join("\n");
    if compose.ends_with('\n') {
        new_compose.push('\n');
    }
    Ok((new_config, new_compose))
}

fn remove_share_texts(config: &str, compose: &str, id: &str) -> Result<(String, String), String> {
    let mut config_lines = config.lines().map(str::to_owned).collect::<Vec<_>>();
    let (lines, directories_at, list_end) = share_directory_lines(config)?;
    let marker = format!("/burrata-shares/{id}");
    let matching = (directories_at + 1..list_end)
        .find(|index| {
            unquote_yaml_scalar(lines[*index].trim().strip_prefix("- ").unwrap_or(""))
                .ends_with(&marker)
        })
        .ok_or("Managed share entry is missing from slskd configuration")?;
    config_lines.remove(matching);
    let mut new_config = config_lines.join("\n");
    if config.ends_with('\n') {
        new_config.push('\n');
    }

    let mut compose_lines = compose.lines().map(str::to_owned).collect::<Vec<_>>();
    let target = format!("target: /burrata-shares/{id}");
    let target_at = compose_lines
        .iter()
        .position(|line| line.trim() == target)
        .ok_or("Managed read-only folder mount is missing from Compose configuration")?;
    let block_start = (0..target_at)
        .rev()
        .find(|index| compose_lines[*index].trim() == "- type: bind")
        .ok_or("Managed folder mount has an unexpected Compose format")?;
    let block_end = (target_at + 1..compose_lines.len())
        .find(|index| {
            let line = compose_lines[*index].as_str();
            !line.starts_with("        ") && !line.trim().is_empty()
        })
        .unwrap_or(compose_lines.len());
    if block_end - block_start != 4 || compose_lines[block_end - 1].trim() != "read_only: true" {
        return Err(
            "Managed folder mount has an unexpected Compose shape; refusing to edit it".into(),
        );
    }
    compose_lines.drain(block_start..block_end);
    let mut new_compose = compose_lines.join("\n");
    if compose.ends_with('\n') {
        new_compose.push('\n');
    }
    Ok((new_config, new_compose))
}

fn compose_mount_source(compose: &str, id: &str) -> Option<String> {
    let lines = compose.lines().collect::<Vec<_>>();
    let target = format!("target: /burrata-shares/{id}");
    let target_at = lines.iter().position(|line| line.trim() == target)?;
    let source = lines[..target_at]
        .iter()
        .rev()
        .find_map(|line| line.trim().strip_prefix("source: "))?;
    Some(unquote_yaml_scalar(source))
}

fn apply_managed_share_texts(
    paths: &ManagedSharePaths,
    old_config: &str,
    old_compose: &str,
    new_config: &str,
    new_compose: &str,
) -> Result<(), String> {
    apply_managed_share_texts_with(
        paths,
        old_config,
        old_compose,
        new_config,
        new_compose,
        || run_managed_compose(paths),
    )
}

fn apply_managed_share_texts_with<F>(
    paths: &ManagedSharePaths,
    old_config: &str,
    old_compose: &str,
    new_config: &str,
    new_compose: &str,
    mut apply_service: F,
) -> Result<(), String>
where
    F: FnMut() -> Result<(), String>,
{
    write_managed_file(&paths.compose, new_compose)?;
    if let Err(error) = write_managed_file(&paths.config, new_config) {
        let _ = restore_managed_file(&paths.compose, old_compose);
        return Err(error);
    }
    match apply_service() {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = restore_managed_file(&paths.compose, old_compose);
            let _ = restore_managed_file(&paths.config, old_config);
            if apply_service().is_ok() {
                Err(format!(
                    "{error}; the prior settings and running service were restored"
                ))
            } else {
                Err(format!("{error}; the files were restored, but Docker Compose could not reapply the previous service configuration"))
            }
        }
    }
}

fn run_managed_compose(paths: &ManagedSharePaths) -> Result<(), String> {
    use std::process::Stdio;
    let mut child = Command::new("docker")
        .args(["compose", "-f"])
        .arg(&paths.compose)
        .args(["up", "-d", "--force-recreate", "slskd"])
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Docker Compose could not be started".to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "Docker Compose could not apply the managed slskd change (exit {})",
                    status.code().unwrap_or(-1)
                ));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Docker Compose timed out while applying the managed slskd change".into(),
                );
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not check the managed slskd service restart".into());
            }
        }
    }
}

fn write_managed_file(path: &Path, content: &str) -> Result<(), String> {
    let backup = path.with_extension("burrata-backup");
    fs::copy(path, &backup)
        .map_err(|e| format!("Cannot create private backup for managed settings: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension(format!("burrata-tmp-{}", Uuid::new_v4()));
    let permissions = fs::metadata(path)
        .map_err(|e| format!("Cannot inspect managed settings permissions: {e}"))?
        .permissions();
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("Cannot write managed settings safely: {e}"))?;
        file.set_permissions(permissions)
            .map_err(|e| format!("Cannot preserve managed settings permissions: {e}"))?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Cannot flush managed settings: {e}"))?;
        fs::rename(&temporary, path)
            .map_err(|e| format!("Cannot atomically update managed settings: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn restore_managed_file(path: &Path, content: &str) -> Result<(), String> {
    let temporary = path.with_extension(format!("burrata-restore-{}", Uuid::new_v4()));
    let permissions = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.set_permissions(permissions)
            .map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_guid(id: &str) -> Result<(), String> {
    Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| "Invalid slskd operation ID".into())
}

pub async fn wait_for_batch_file(
    client: &SlskdClient,
    batch_id: &str,
    username: &str,
    source: &crate::models::CandidateSource,
    downloads_root: &std::path::Path,
    staging: &std::path::Path,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    mut progress: impl FnMut(f64, Option<String>, &str, &crate::models::SoulseekProgress),
) -> Result<std::path::PathBuf, String> {
    use crate::models::CandidateSource::Soulseek;
    use tokio::time::Instant;

    let (filename, expected_size) = match source {
        Soulseek {
            filename,
            size_bytes,
            ..
        } => (filename, *size_bytes),
    };
    validate_remote_path(filename)?;
    let root = tokio::fs::canonicalize(downloads_root)
        .await
        .map_err(|e| format!("Cannot access configured slskd downloads directory: {e}"))?;
    let mut deadline = Instant::now() + Duration::from_secs(60 * 60);
    let mut last_percent = -1.0;
    let mut transfer_id = match source {
        Soulseek { transfer_id, .. } => transfer_id.clone(),
    };
    let mut enqueued = transfer_id.is_some();
    let mut request_deadline = Instant::now() + Duration::from_secs(120);
    let mut next_queue_check = Instant::now();
    let mut details = crate::models::SoulseekProgress::default();
    let mut last_byte_progress = Instant::now();
    let mut last_bytes = 0;
    loop {
        if *cancel.borrow() {
            if let Some(id) = transfer_id.as_deref() {
                let _ = client.cancel_download(username, id).await;
            }
            return Err("Cancelled".into());
        }
        if !enqueued {
            if Instant::now() >= request_deadline {
                return Err("slskd could not establish a transfer batch within two minutes".into());
            }
            // Retrying an already submitted job is idempotent: reuse its job ID
            // as the batch ID, then reconcile the extant batch on conflict.
            match client
                .enqueue_download(
                    batch_id,
                    match source {
                        Soulseek { search_id, .. } => search_id.as_deref(),
                    },
                    username,
                    filename,
                    expected_size,
                )
                .await
            {
                Ok(response) => {
                    validate_enqueue_response(&response)?;
                    transfer_id = extract_transfer_id(&response);
                    enqueued = true;
                    progress(0.0, transfer_id.clone(), "queued", &details);
                }
                Err(error) if error.contains("HTTP 409") => enqueued = true,
                Err(error) if error.contains("HTTP 429") => {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    continue;
                }
                Err(error) => return Err(error),
            }
            if *cancel.borrow() {
                if let Some(id) = transfer_id.as_deref() {
                    let _ = client.cancel_download(username, id).await;
                }
                return Err("Cancelled".into());
            }
        }
        let batch = match client.download_batch(batch_id).await {
            Ok(batch) => batch,
            Err(error) if error.contains("HTTP 404") => {
                enqueued = false;
                transfer_id = None;
                tokio::time::sleep(Duration::from_millis(700)).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        let transfers = batch
            .pointer("/batch/transfers")
            .or_else(|| batch.get("transfers"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let (complete, failed, percent, mut state) = summarize_transfers(&transfers);
        if transfers.is_empty() {
            if let Some(error) = enqueue_failure(&batch) {
                return Err(error);
            }
            if Instant::now() >= request_deadline {
                return Err("slskd returned an empty batch; no file was queued".into());
            }
        } else {
            request_deadline = Instant::now() + Duration::from_secs(120);
        }
        // A remote queue is healthy waiting, not a stalled download. Also keep
        // monitoring slow transfers as long as bytes continue arriving.
        if state == "waiting_for_peer" || state == "queued" || percent > last_percent {
            deadline = Instant::now() + Duration::from_secs(60 * 60);
        }
        last_percent = percent;
        if transfer_id.is_none() {
            transfer_id = extract_transfer_id(&batch);
        }
        update_transfer_details(&mut details, &transfers);
        if details.bytes_received > last_bytes {
            last_byte_progress = Instant::now();
            details.last_progress_at = Some(chrono::Utc::now().to_rfc3339());
        }
        last_bytes = details.bytes_received;
        state = transfer_phase(&state, last_byte_progress.elapsed()).into();
        if state == "waiting_for_peer" && !failed && Instant::now() >= next_queue_check {
            next_queue_check = Instant::now() + Duration::from_secs(300);
            if let Some(id) = transfer_id.as_deref() {
                let result = tokio::select! {
                    result = tokio::time::timeout(Duration::from_secs(5), client.queue_position(username, id)) => result,
                    _ = cancel.changed() => { continue; },
                };
                details.queue_checked_at = Some(chrono::Utc::now().to_rfc3339());
                match result {
                    Ok(Ok(position)) => {
                        details.queue_position = Some(position);
                        details.queue_error = None;
                    }
                    _ => {
                        details.queue_error = Some("Peer did not answer the queue check".into());
                    }
                }
            }
        }
        progress(
            percent.clamp(0.0, 90.0),
            transfer_id.clone(),
            &state,
            &details,
        );
        if failed {
            return Err(transfer_failure(&transfers));
        }
        if complete {
            let batch_root = root.join(batch_id);
            let batch_root = tokio::fs::canonicalize(&batch_root).await.map_err(|_| {
                "slskd reports completion, but the shared batch directory is missing"
            })?;
            if !batch_root.starts_with(&root) {
                return Err(
                    "slskd batch directory is outside the configured downloads folder".into(),
                );
            }
            let source_path = find_completed_file(&batch_root, filename, expected_size).await?;
            let extension = audio_extension(filename)?.to_ascii_lowercase();
            let destination = staging.join(format!("source.{extension}"));
            tokio::fs::copy(&source_path, &destination)
                .await
                .map_err(|e| format!("Cannot stage completed slskd transfer: {e}"))?;
            return Ok(destination);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for slskd transfer ({state}); the queued transfer remains in slskd. Choose another peer or cancel it in slskd."
            ));
        }
        let poll_interval =
            if state == "waiting_for_peer" || state == "queued" || state == "stalled" {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(700)
            };
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {},
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() {
                    if let Some(id) = transfer_id.as_deref() {
                        let _ = client.cancel_download(username, id).await;
                    }
                    return Err("Cancelled".into());
                }
            }
        }
    }
}

fn transfer_phase(state: &str, without_bytes: Duration) -> &str {
    if state == "downloading" && without_bytes >= Duration::from_secs(120) {
        "stalled"
    } else {
        state
    }
}

fn batch_transfers(batch: &Value) -> &[Value] {
    batch
        .pointer("/batch/transfers")
        .or_else(|| batch.get("transfers"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn batch_needs_new_attempt(batch: &Value) -> bool {
    let transfers = batch_transfers(batch);
    if transfers.is_empty() {
        return true;
    }
    let (_, failed, _, _) = summarize_transfers(transfers);
    failed
}

fn bounded_message(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect()
}

fn enqueue_failure(response: &Value) -> Option<String> {
    response
        .get("failures")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .map(|items| {
            format!(
                "slskd could not enqueue the file: {}",
                items
                    .iter()
                    .filter_map(|item| item.get("message").and_then(Value::as_str))
                    .take(3)
                    .map(bounded_message)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
}

fn validate_enqueue_response(response: &Value) -> Result<(), String> {
    if let Some(error) = enqueue_failure(response) {
        return Err(error);
    }
    if batch_transfers(response).is_empty() {
        return Err("slskd accepted the request but did not queue a file".into());
    }
    Ok(())
}

fn transfer_failure(transfers: &[Value]) -> String {
    let message = transfers
        .iter()
        .filter_map(|t| t.get("exception").and_then(Value::as_str))
        .find(|s| !s.is_empty());
    let state = transfers
        .iter()
        .filter_map(|t| t.get("state").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "slskd transfer failed ({}): {}",
        bounded_message(&state),
        bounded_message(message.unwrap_or("Peer did not complete the transfer"))
    )
}

fn update_transfer_details(details: &mut crate::models::SoulseekProgress, transfers: &[Value]) {
    let Some(transfer) = transfers.first() else {
        return;
    };
    details.bytes_received = transfer
        .get("bytesTransferred")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    details.speed_bytes_per_second = transfer
        .get("averageSpeed")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(0.0);
    details.attempts = transfer
        .get("attempts")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    details.next_attempt_at = transfer
        .get("nextAttemptAt")
        .and_then(Value::as_str)
        .map(str::to_owned);
    details.daemon_state = transfer
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into();
    if let Some(position) = transfer.get("placeInQueue").and_then(Value::as_u64) {
        details.queue_position = Some(position);
    }
}

fn extract_transfer_id(batch: &Value) -> Option<String> {
    let transfers = batch
        .pointer("/batch/transfers")
        .or_else(|| batch.get("transfers"))?
        .as_array()?;
    transfers
        .first()?
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn summarize_transfers(transfers: &[Value]) -> (bool, bool, f64, String) {
    if transfers.is_empty() {
        return (false, false, 0.0, "queued".into());
    }
    let mut complete = true;
    let mut failed = false;
    let mut percent = 0.0;
    let mut states = Vec::new();
    for transfer in transfers {
        let state = transfer
            .get("state")
            .or_else(|| transfer.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !state.is_empty() && !states.contains(&state) {
            states.push(state.clone());
        }
        // "Completed" is a terminal category, not proof of successful receipt.
        // slskd also returns "Completed, TimedOut" and "Completed, Cancelled".
        let done = matches!(
            state.as_str(),
            "complete" | "completed" | "succeeded" | "finished"
        ) || state.split(',').any(|part| part.trim() == "succeeded");
        let retry_pending = transfer
            .get("nextAttemptAt")
            .or_else(|| transfer.get("next_attempt_at"))
            .is_some_and(|next| !next.is_null());
        let unsuccessful = state.contains("fail")
            || state.contains("cancel")
            || state.contains("reject")
            || state.contains("timedout")
            || state.contains("timed out")
            || state.contains("error")
            || (state.contains("completed") && !done);
        failed |= unsuccessful && !retry_pending;
        complete &= done;
        percent += transfer
            .get("percentComplete")
            .or_else(|| transfer.get("percent_complete"))
            .and_then(Value::as_f64)
            .unwrap_or(if done { 100.0 } else { 0.0 });
    }
    let state = if complete {
        "completed".to_owned()
    } else if states
        .iter()
        .any(|s| s.contains("queued") && s.contains("remote"))
    {
        "waiting_for_peer".to_owned()
    } else if states.iter().any(|s| s.contains("queued")) {
        "queued".to_owned()
    } else if states.iter().any(|s| {
        s.contains("transferring")
            || s.contains("downloading")
            || s.contains("inprogress")
            || s.contains("in progress")
    }) {
        "downloading".to_owned()
    } else if failed {
        "failed".to_owned()
    } else {
        "waiting_for_peer".to_owned()
    };
    (
        complete,
        failed,
        percent / transfers.len() as f64 * 0.9,
        state,
    )
}

async fn find_completed_file(
    directory: &std::path::Path,
    remote_filename: &str,
    expected_size: u64,
) -> Result<std::path::PathBuf, String> {
    use tokio::fs;
    let expected = remote_basename(remote_filename).ok_or("Invalid Soulseek filename")?;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let mut entries = fs::read_dir(&current)
            .await
            .map_err(|e| format!("Cannot inspect slskd completed directory: {e}"))?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();
            let kind = entry.file_type().await.map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && entry.file_name().to_string_lossy() == expected
                && entry.metadata().await.map_err(|e| e.to_string())?.len() == expected_size
            {
                let real = fs::canonicalize(&path).await.map_err(|e| e.to_string())?;
                if real.starts_with(directory) {
                    return Ok(real);
                }
            }
        }
    }
    Err("slskd completed the transfer, but the expected file is not present in the shared batch folder".into())
}

/// Soulseek shares can report Windows-style paths even when slskd stores the
/// completed file under a POSIX directory. Keep the original path for API
/// requests, but compare only its final component when locating the local file.
fn remote_basename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|part| !part.is_empty())
}

/// Convert remote peer results into review-required choices. Peer-provided
/// names and tags are unverified, so this never marks a result as an exact ID.
pub fn candidates_from_responses(
    responses: &[SearchResponse],
    search_id: &str,
) -> Vec<crate::models::Candidate> {
    let mut candidates = Vec::new();
    for response in responses {
        for file in &response.files {
            if candidates.len() >= 500 {
                return candidates;
            }
            let extension = file
                .filename
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
                .unwrap_or_default();
            if ![
                "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "aiff", "wma",
            ]
            .contains(&extension.as_str())
            {
                continue;
            }
            if crate::slskd::validate_remote_path(&file.filename).is_err()
                || file.size == 0
                || file.size > 2 * 1024 * 1024 * 1024
            {
                continue;
            }
            let stem = file
                .filename
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&file.filename)
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(&file.filename)
                .to_owned();
            let mut digest = Sha256::new();
            digest.update(response.username.as_bytes());
            digest.update([0]);
            digest.update(file.filename.as_bytes());
            let digest = digest.finalize();
            let stable_id = format!("slskd:{}", hex_digest(&digest));
            let availability = match response.has_free_upload_slot {
                Some(true) => "availability:free-upload-slot",
                Some(false) => "availability:queued-remotely",
                None => "availability:unknown",
            };
            let mut reasons = vec![
                "Peer filename and metadata are unverified; review before downloading".into(),
                availability.into(),
            ];
            if let Some(queue) = response.queue_length {
                reasons.push(format!("availability:queue-length:{queue}"));
            }
            if let Some(speed) = response.upload_speed {
                reasons.push(format!("availability:upload-speed:{speed}"));
            }
            let lossless = matches!(extension.as_str(), "flac" | "wav" | "aiff");
            let bitrate = file
                .bit_rate
                .or_else(|| file_attribute_u32(file, &["bitrate", "bit_rate"]));
            let sample_rate = file_attribute_u32(file, &["samplerate", "sample_rate"]);
            candidates.push(crate::models::Candidate {
                id: stable_id,
                provider: "slskd".into(),
                url: String::new(),
                title: stem,
                uploader: Some(response.username.clone()),
                duration_seconds: file.length,
                formats: vec![crate::models::AudioFormat {
                    id: extension.clone(),
                    codec: extension.clone(),
                    extension,
                    bitrate: bitrate.map(f64::from),
                    sample_rate,
                    lossless,
                }],
                permission: "unknown".into(),
                confidence: "review".into(),
                score: 0.0,
                reasons,
                source: Some(crate::models::CandidateSource::Soulseek {
                    username: response.username.clone(),
                    filename: file.filename.clone(),
                    size_bytes: file.size,
                    bitrate_kbps: bitrate,
                    search_id: Some(search_id.to_owned()),
                    batch_id: None,
                    transfer_id: None,
                }),
                ..Default::default()
            });
        }
    }
    candidates
}

fn file_attribute_u32(file: &RemoteFile, names: &[&str]) -> Option<u32> {
    file.attributes.iter().find_map(|attribute| {
        let name = attribute.attribute.as_deref()?.to_ascii_lowercase();
        if !names.contains(&name.as_str()) {
            return None;
        }
        attribute
            .value
            .as_ref()?
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| attribute.value.as_ref()?.as_str()?.parse().ok())
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub fn validate_remote_path(path: &str) -> Result<(), String> {
    let path = path.replace('\\', "/");
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.split('/').any(|part| part == ".." || part.is_empty())
        || path.contains('\0')
    {
        return Err("Invalid Soulseek remote filename".into());
    }
    Ok(())
}

fn audio_extension(path: &str) -> Result<&str, String> {
    let basename = path.rsplit('/').next().ok_or("Invalid Soulseek filename")?;
    let extension = basename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .ok_or("Selected Soulseek file has no extension")?;
    if ![
        "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "aiff", "wma",
    ]
    .iter()
    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    {
        return Err("Selected Soulseek file is not a supported audio format".into());
    }
    Ok(extension)
}

fn decode_search_responses(value: Value) -> Result<Vec<SearchResponse>, String> {
    let value = if let Some(responses) = value.get("responses") {
        responses.clone()
    } else {
        value
    };
    let array = value
        .as_array()
        .ok_or("slskd search results are not a response array")?;
    array
        .iter()
        .filter_map(|item| {
            let response = item.get("response").unwrap_or(item);
            serde_json::from_value::<SearchResponse>(response.clone()).ok()
        })
        .collect::<Vec<_>>()
        .pipe(Ok)
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_share_text_changes_are_read_only_aliases_and_reversible() {
        let config = "soulseek:\n  username: secret-placeholder\nshares:\n  directories:\n    - \"[Shared 1]/app/manual-share\"\n";
        let compose = "services:\n  slskd:\n    container_name: burrata-slskd\n    volumes:\n      - ./data:/app\n    environment:\n      SLSKD_REMOTE_CONFIGURATION: \"false\"\n";
        let id = "0123456789abcdef";
        assert_eq!(next_share_alias(&[], config).unwrap(), "Shared 2");
        let (added_config, added_compose) =
            add_share_texts(config, compose, id, "Shared 2", "/home/test/My Music").unwrap();
        assert!(added_config.contains("/app/manual-share"));
        assert!(added_config.contains("[Shared 2]/burrata-shares/0123456789abcdef"));
        assert!(added_compose.contains("source: \"/home/test/My Music\""));
        assert!(added_compose.contains("target: /burrata-shares/0123456789abcdef"));
        assert!(added_compose.contains("read_only: true"));
        let shares = managed_shares_from_text(&added_config, &added_compose).unwrap();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].directory_path, "/home/test/My Music");
        assert_eq!(shares[0].alias, "Shared 2");

        let (removed_config, removed_compose) =
            remove_share_texts(&added_config, &added_compose, id).unwrap();
        assert!(removed_config.contains("/app/manual-share"));
        assert!(!removed_config.contains("/burrata-shares/0123456789abcdef"));
        assert_eq!(removed_compose, compose);
    }

    #[test]
    fn managed_share_config_refuses_unexpected_shapes_and_invalid_ids() {
        assert!(add_share_texts(
            "shares:\n  directories: []\n",
            "services:\n  slskd:\n    container_name: other\n    volumes:\n      - ./data:/app\n",
            "0123456789abcdef",
            "Shared 1",
            "/music",
        )
        .is_err());
        assert!(remove_managed_share_folder("../../credentials").is_err());
    }

    #[test]
    fn managed_share_path_validation_protects_config_ancestors_and_private_data() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("deploy/slskd");
        let data = root.join("data");
        let downloads = data.join("downloads");
        let incomplete = data.join("incomplete");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&incomplete).unwrap();
        let paths = ManagedSharePaths {
            root: root.clone(),
            compose: root.join("compose.yaml"),
            config: data.join("slskd.yml"),
        };
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let canonical_data = std::fs::canonicalize(&data).unwrap();
        let canonical_downloads = std::fs::canonicalize(&downloads).unwrap();
        let canonical_incomplete = std::fs::canonicalize(&incomplete).unwrap();
        assert!(validate_managed_share_path(&canonical_root, &paths).is_err());
        assert!(validate_managed_share_path(&canonical_data, &paths).is_err());
        assert!(validate_managed_share_path(&canonical_incomplete, &paths).is_err());
        assert!(validate_managed_share_path(&canonical_downloads, &paths).is_ok());
    }

    #[test]
    fn managed_share_settings_restore_files_when_compose_apply_fails() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let compose_path = root.join("compose.yaml");
        let config_path = root.join("slskd.yml");
        let old_compose = "services:\n  slskd:\n    container_name: burrata-slskd\n";
        let old_config = "shares:\n  directories: []\n";
        std::fs::write(&compose_path, old_compose).unwrap();
        std::fs::write(&config_path, old_config).unwrap();
        let paths = ManagedSharePaths {
            root,
            compose: compose_path.clone(),
            config: config_path.clone(),
        };
        let attempts = AtomicUsize::new(0);
        let result = apply_managed_share_texts_with(
            &paths,
            old_config,
            old_compose,
            "shares:\n  directories:\n    - \"[Shared 1]/burrata-shares/0123456789abcdef\"\n",
            "services:\n  slskd:\n    container_name: burrata-slskd\n    volumes:\n      - /music:/burrata-shares/0123456789abcdef:ro\n",
            || {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("mock compose apply failed".into())
                } else {
                    Ok(())
                }
            },
        );
        assert!(result
            .unwrap_err()
            .contains("prior settings and running service were restored"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(std::fs::read_to_string(compose_path).unwrap(), old_compose);
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), old_config);
    }

    #[tokio::test]
    async fn shared_folder_list_and_rescan_use_documented_slskd_endpoints() {
        use axum::{routing::get, Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::net::TcpListener;

        let scans = std::sync::Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/api/v0/shares",
            get(|| async {
                Json(serde_json::json!({"local":[{
                    "id":"share-1", "alias":"Music", "localPath":"/music",
                    "remotePath":"Music", "files":23, "directories":4, "isExcluded":false
                }]}))
            })
            .put({
                let scans = scans.clone();
                move || {
                    scans.fetch_add(1, Ordering::SeqCst);
                    async { axum::http::StatusCode::NO_CONTENT }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = SlskdClient::new(&format!("http://{address}"), "mock-api-key").unwrap();
        let shares = client.shared_folders().await.unwrap();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].local_path, "/music");
        assert_eq!(shares[0].files, 23);
        client.rescan_shared_folders().await.unwrap();
        assert_eq!(scans.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn failed_retry_creates_new_batch_and_reports_queue_and_http_200_rejection() {
        use axum::{
            extract::Path,
            routing::{delete, get, post},
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        let old = "00000000-0000-0000-0000-000000000010";
        let tid = "00000000-0000-0000-0000-000000000011";
        let posted = Arc::new(Mutex::new(Vec::<Value>::new()));
        let reject = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let app = Router::new()
            .route("/api/v0/transfers/downloads/batches/{id}", get(move |Path(id): Path<String>| async move {
                Json(serde_json::json!({"batch":{"transfers":[{"id":tid,"state":if id == old { "Completed, Errored" } else { "Queued, Remotely" },"exception":"Application shut down","attempts":1}]}}))
            }))
            .route("/api/v0/transfers/downloads/batches", post({
                let posted = posted.clone(); let reject = reject.clone();
                move |Json(body): Json<Value>| {
                    posted.lock().unwrap().push(body);
                    let rejected = reject.load(std::sync::atomic::Ordering::SeqCst);
                    async move { Json(if rejected {
                        serde_json::json!({"batch":{"transfers":[]},"failures":[{"message":"File is already queued"}]})
                    } else { serde_json::json!({"batch":{"transfers":[{"id":tid}]}}) }) }
                }
            }))
            .route("/api/v0/transfers/downloads/peer/{id}/position", get(|| async { Json(17u64) }))
            .route("/api/v0/transfers/downloads/peer/{id}", delete(|| async { axum::http::StatusCode::NO_CONTENT }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = SlskdClient::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "test-key",
        )
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut source = crate::models::CandidateSource::Soulseek {
            username: "peer".into(),
            filename: "Music\\Track.flac".into(),
            size_bytes: 100,
            bitrate_kbps: None,
            search_id: None,
            batch_id: Some(old.into()),
            transfer_id: Some(tid.into()),
        };
        client.prepare_retry(&mut source).await.unwrap();
        let crate::models::CandidateSource::Soulseek {
            batch_id,
            transfer_id,
            ..
        } = &source;
        let new_id = batch_id.clone().unwrap();
        assert_ne!(new_id, old);
        assert!(transfer_id.is_none());
        let temp = tempfile::tempdir().unwrap();
        let (tx, mut cancel) = tokio::sync::watch::channel(false);
        let mut observed = None;
        let error = wait_for_batch_file(
            &client,
            &new_id,
            "peer",
            &source,
            temp.path(),
            temp.path(),
            &mut cancel,
            |_, _, state, details| {
                if state == "waiting_for_peer" {
                    observed = Some(details.clone());
                    tx.send(true).unwrap();
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Cancelled");
        assert_eq!(observed.unwrap().queue_position, Some(17));
        assert_eq!(posted.lock().unwrap()[0]["id"], new_id);
        assert_eq!(
            posted.lock().unwrap()[0]["files"][0]["filename"],
            "Music\\Track.flac"
        );
        reject.store(true, std::sync::atomic::Ordering::SeqCst);
        let (_tx, mut cancel) = tokio::sync::watch::channel(false);
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_batch_file(
                &client,
                &new_id,
                "peer",
                &source,
                temp.path(),
                temp.path(),
                &mut cancel,
                |_, _, _, _| {},
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.contains("File is already queued"), "{error}");
        server.abort();
    }

    #[test]
    fn retry_reconciles_active_success_and_pending_retry_without_new_submission() {
        for state in ["Queued, Remotely", "InProgress", "Completed, Succeeded"] {
            assert!(!batch_needs_new_attempt(
                &serde_json::json!({"batch":{"transfers":[{"state":state}]}})
            ));
        }
        assert!(!batch_needs_new_attempt(
            &serde_json::json!({"transfers":[{"state":"Completed, TimedOut","nextAttemptAt":"2026-10-13T20:00:00Z"}]})
        ));
        for state in [
            "Completed, Errored",
            "Completed, TimedOut",
            "Completed, Cancelled",
        ] {
            assert!(batch_needs_new_attempt(
                &serde_json::json!({"transfers":[{"state":state}]})
            ));
        }
        assert!(batch_needs_new_attempt(
            &serde_json::json!({"transfers":[]})
        ));
    }

    #[test]
    fn telemetry_filters_invalid_speed_and_keeps_failure_reason() {
        let transfers = vec![
            serde_json::json!({"state":"Completed, Errored","exception":"Application shut down","bytesTransferred":123,"averageSpeed":-1,"attempts":3}),
        ];
        let mut details = crate::models::SoulseekProgress::default();
        update_transfer_details(&mut details, &transfers);
        assert_eq!(details.bytes_received, 123);
        assert_eq!(details.speed_bytes_per_second, 0.0);
        assert_eq!(details.attempts, 3);
        assert!(transfer_failure(&transfers).contains("Application shut down"));
        assert!(validate_enqueue_response(&serde_json::json!({"batch":{"transfers":[]}})).is_err());
    }

    #[test]
    fn stalled_transfer_is_distinct_from_a_slow_transfer_or_remote_wait() {
        assert_eq!(
            transfer_phase("downloading", Duration::from_secs(119)),
            "downloading"
        );
        assert_eq!(
            transfer_phase("downloading", Duration::from_secs(120)),
            "stalled"
        );
        assert_eq!(
            transfer_phase("waiting_for_peer", Duration::from_secs(3600)),
            "waiting_for_peer"
        );
        assert_eq!(transfer_phase("downloading", Duration::ZERO), "downloading");
    }

    #[test]
    fn remote_paths_reject_traversal_and_allow_normal_share_paths() {
        assert!(validate_remote_path("House/Artist - Track.flac").is_ok());
        assert!(validate_remote_path("../private.mp3").is_err());
        assert!(validate_remote_path("House/../../private.mp3").is_err());
        assert!(validate_remote_path("/absolute.mp3").is_err());
    }

    #[test]
    fn response_shapes_decode_flat_and_wrapped_items() {
        let response = serde_json::json!({"username":"peer","files":[{"filename":"A/B.mp3","size":12,"bitRate":320,"length":200}]});
        let flat = decode_search_responses(serde_json::json!([response.clone()])).unwrap();
        assert_eq!(flat[0].username, "peer");
        assert_eq!(flat[0].files[0].bit_rate, Some(320));
        let wrapped =
            decode_search_responses(serde_json::json!({"responses":[{"response":response}]}))
                .unwrap();
        assert_eq!(wrapped[0].files[0].size, 12);
    }

    #[test]
    fn daemon_url_requires_https_off_loopback() {
        assert!(SlskdClient::new("http://localhost:5030", "test-key").is_ok());
        assert!(SlskdClient::new("http://192.168.1.4:5030", "test-key").is_err());
        assert!(SlskdClient::new("https://slskd.example:5030", "test-key").is_ok());
    }

    #[test]
    fn older_web_candidates_deserialize_without_typed_source() {
        let candidate: crate::models::Candidate = serde_json::from_value(serde_json::json!({
            "id":"youtube:abc",
            "provider":"youtube",
            "url":"https://www.youtube.com/watch?v=abc",
            "title":"Track",
            "formats":[],
            "permission":"unknown",
            "confidence":"review",
            "score":0.4,
            "reasons":[],
            "official":false
        }))
        .unwrap();
        assert!(candidate.source.is_none());
    }

    #[test]
    fn candidate_identity_is_stable_and_uploader_is_not_promoted_to_artist() {
        let responses = vec![SearchResponse {
            username: "peer-account".into(),
            has_free_upload_slot: Some(true),
            queue_length: Some(3),
            upload_speed: Some(12_000),
            files: vec![RemoteFile {
                filename: "House/Artist Name - Night Drive.flac".into(),
                size: 4_000,
                bit_rate: Some(900),
                length: Some(180.0),
                attributes: vec![],
            }],
        }];
        let first = candidates_from_responses(&responses, "00000000-0000-0000-0000-000000000001");
        let second = candidates_from_responses(&responses, "00000000-0000-0000-0000-000000000001");
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[0].provider, "slskd");
        assert_eq!(first[0].title, "Artist Name - Night Drive");
        assert_eq!(first[0].uploader.as_deref(), Some("peer-account"));
        assert_eq!(first[0].confidence, "review");
        assert!(first[0]
            .reasons
            .iter()
            .any(|r| r == "availability:free-upload-slot"));
        assert!(first[0]
            .reasons
            .iter()
            .any(|r| r == "availability:queue-length:3"));
        assert!(matches!(
            first[0].source.as_ref(),
            Some(crate::models::CandidateSource::Soulseek { .. })
        ));
    }

    #[test]
    fn terminal_timeout_is_not_mistaken_for_a_completed_download() {
        for state in [
            "Completed, TimedOut",
            "Completed, Errored",
            "Completed, Cancelled",
            "Completed, Rejected",
        ] {
            let (complete, failed, _, phase) =
                summarize_transfers(&[serde_json::json!({"state": state})]);
            assert!(!complete, "{state}");
            assert!(failed, "{state}");
            assert_eq!(phase, "failed");
        }
        let (complete, failed, _, phase) =
            summarize_transfers(&[serde_json::json!({"state": "Completed, Succeeded"})]);
        assert!(complete);
        assert!(!failed);
        assert_eq!(phase, "completed");
    }

    #[test]
    fn daemon_retry_is_left_to_slskd_instead_of_reported_as_final_failure() {
        let (complete, failed, _, phase) = summarize_transfers(&[serde_json::json!({
            "state": "Completed, TimedOut", "nextAttemptAt": "2026-10-13T19:00:00Z"
        })]);
        assert!(!complete);
        assert!(!failed);
        assert_eq!(phase, "waiting_for_peer");
    }

    #[test]
    fn remote_basename_accepts_windows_and_posix_share_paths() {
        assert_eq!(remote_basename("Folder/01 song.mp3"), Some("01 song.mp3"));
        assert_eq!(remote_basename(r"Folder\01 song.mp3"), Some("01 song.mp3"));
        assert_eq!(
            remote_basename(r"Folder\Nested/01 song.mp3"),
            Some("01 song.mp3")
        );
        assert_eq!(remote_basename("Folder/"), None);
    }

    #[test]
    fn transfer_summary_surfaces_remote_queue_and_terminal_states() {
        let queued = summarize_transfers(&[serde_json::json!({
            "state":"Queued, Remotely", "percentComplete":0
        })]);
        assert_eq!(queued, (false, false, 0.0, "waiting_for_peer".into()));
        let active = summarize_transfers(&[serde_json::json!({
            "state":"InProgress", "percentComplete":25
        })]);
        assert_eq!(active.3, "downloading");
        let done = summarize_transfers(&[serde_json::json!({
            "state":"Succeeded", "percentComplete":100
        })]);
        assert_eq!(done.0, true);
        assert_eq!(done.3, "completed");
    }

    #[tokio::test]
    async fn search_start_poll_responses_cancel_and_delete_use_slskd_api_contract() {
        use axum::{
            extract::Path,
            routing::{get, post},
            Json, Router,
        };
        use serde_json::{json, Value};
        use std::sync::{Arc, Mutex};
        use tokio::net::TcpListener;

        let observed_request = Arc::new(Mutex::new(None::<Value>));
        let observed_cancel = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed_delete = Arc::new(Mutex::new(Vec::<String>::new()));
        let app = Router::new()
            .route(
                "/api/v0/searches",
                post({
                    let observed = observed_request.clone();
                    move |Json(body): Json<Value>| {
                        let observed = observed.clone();
                        async move {
                            *observed.lock().unwrap() = Some(body);
                            Json(json!({"state":"InProgress"}))
                        }
                    }
                }),
            )
            .route(
                "/api/v0/searches/{id}",
                get(|Path(id): Path<String>| async move {
                    Json(json!({"id":id,"state":"InProgress","isComplete":false}))
                })
                .put({
                    let observed = observed_cancel.clone();
                    move |Path(id): Path<String>| {
                        let observed = observed.clone();
                        async move {
                            observed.lock().unwrap().push(id);
                            axum::http::StatusCode::NO_CONTENT
                        }
                    }
                })
                .delete({
                    let observed = observed_delete.clone();
                    move |Path(id): Path<String>| {
                        let observed = observed.clone();
                        async move {
                            observed.lock().unwrap().push(id);
                            axum::http::StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
            .route(
                "/api/v0/searches/{id}/responses",
                get(|Path(_id): Path<String>| async move {
                    Json(json!([{"username":"peer","files":[{"filename":"Album/Artist - Track.flac","size":4096,"bitRate":320,"length":210.0}]}]))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = SlskdClient::new(&format!("http://{address}"), "mock-api-key").unwrap();
        let id = client.start_search("Artist Track", 30).await.unwrap();
        assert_eq!(
            client.search_status(&id).await.unwrap().is_complete,
            Some(false)
        );
        let responses = client.search_responses(&id).await.unwrap();
        assert_eq!(responses[0].username, "peer");
        assert_eq!(responses[0].files[0].bit_rate, Some(320));
        client.cancel_search(&id).await.unwrap();
        client.delete_search(&id).await.unwrap();

        let request = observed_request.lock().unwrap().clone().unwrap();
        assert_eq!(request["id"], id);
        assert_eq!(request["searchText"], "Artist Track");
        assert_eq!(request["searchTimeout"], 30_000);
        assert_eq!(request["responseLimit"], 100);
        assert_eq!(request["fileLimit"], 10_000);
        assert_eq!(*observed_cancel.lock().unwrap(), vec![id.clone()]);
        assert_eq!(*observed_delete.lock().unwrap(), vec![id]);
        server.abort();
    }

    #[tokio::test]
    async fn completed_batch_file_is_confined_and_copied_into_staging() {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use serde_json::json;
        use tokio::net::TcpListener;

        let batch_id = "00000000-0000-0000-0000-000000000001";
        let transfer_id = "00000000-0000-0000-0000-000000000002";
        let search_id = "00000000-0000-0000-0000-000000000003";
        let completed = json!({"batch":{"transfers":[{
            "id":transfer_id,
            "state":"Succeeded",
            "percentComplete":100
        }]}});
        let submissions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/api/v0/transfers/downloads/batches",
                post({
                    let submissions = submissions.clone();
                    move || {
                        submissions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        async move { Json(json!({"batch":{"transfers":[{"id":transfer_id}]}})) }
                    }
                }),
            )
            .route(
                "/api/v0/transfers/downloads/batches/{id}",
                get({
                    let completed = completed.clone();
                    move || {
                        let completed = completed.clone();
                        async move { Json(completed) }
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let temporary = tempfile::tempdir().unwrap();
        let downloads = temporary.path().join("downloads");
        let staging = temporary.path().join("staging");
        let batch_directory = downloads.join(batch_id).join("House");
        tokio::fs::create_dir_all(&batch_directory).await.unwrap();
        tokio::fs::create_dir_all(&staging).await.unwrap();
        let mut audio_fixture = Vec::new();
        let data_size = 800u32 * 2;
        audio_fixture.extend_from_slice(b"RIFF");
        audio_fixture.extend_from_slice(&(36 + data_size).to_le_bytes());
        audio_fixture.extend_from_slice(b"WAVEfmt ");
        audio_fixture.extend_from_slice(&16u32.to_le_bytes());
        audio_fixture.extend_from_slice(&1u16.to_le_bytes()); // PCM
        audio_fixture.extend_from_slice(&1u16.to_le_bytes()); // mono
        audio_fixture.extend_from_slice(&8_000u32.to_le_bytes());
        audio_fixture.extend_from_slice(&16_000u32.to_le_bytes());
        audio_fixture.extend_from_slice(&2u16.to_le_bytes());
        audio_fixture.extend_from_slice(&16u16.to_le_bytes());
        audio_fixture.extend_from_slice(b"data");
        audio_fixture.extend_from_slice(&data_size.to_le_bytes());
        audio_fixture.resize(audio_fixture.len() + data_size as usize, 0);
        tokio::fs::write(batch_directory.join("Night Drive.wav"), &audio_fixture)
            .await
            .unwrap();
        let source = crate::models::CandidateSource::Soulseek {
            username: "peer-account".into(),
            // Windows separators are used by some Soulseek peers; slskd stores
            // the received file in its local batch folder with POSIX separators.
            filename: r"House\Night Drive.wav".into(),
            size_bytes: audio_fixture.len() as u64,
            bitrate_kbps: Some(320),
            search_id: Some(search_id.into()),
            batch_id: Some(batch_id.into()),
            transfer_id: None,
        };
        let client = SlskdClient::new(&format!("http://{address}"), "mock-api-key").unwrap();
        let (_cancel_sender, mut cancel) = tokio::sync::watch::channel(false);
        let progress_transfer = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let observed = progress_transfer.clone();
        let path = wait_for_batch_file(
            &client,
            batch_id,
            "peer-account",
            &source,
            &downloads,
            &staging,
            &mut cancel,
            move |_, transfer_id, _state, _details| {
                if transfer_id.is_some() {
                    *observed.lock().unwrap() = transfer_id;
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), audio_fixture);
        assert_eq!(
            progress_transfer.lock().unwrap().as_deref(),
            Some(transfer_id)
        );
        assert!(path.starts_with(&staging));

        // Model a restart after submission: the database interrupts the job,
        // but retains the daemon identity so reconciliation never enqueues twice.
        let recovery_db = temporary.path().join("recovery.sqlite");
        let store = crate::storage::Store::open(&recovery_db).unwrap();
        let mut recovered_source = source.clone();
        let crate::models::CandidateSource::Soulseek {
            transfer_id: stored,
            ..
        } = &mut recovered_source;
        *stored = Some(transfer_id.into());
        let job = crate::models::DownloadJob {
            id: batch_id.into(),
            track: crate::models::Track {
                id: "slskd:recovery-recording".into(),
                provider: "slskd".into(),
                provider_id: "recovery-recording".into(),
                title: "Night Drive".into(),
                ..crate::models::Track::default()
            },
            candidate: crate::models::Candidate {
                provider: "slskd".into(),
                source: Some(recovered_source),
                ..crate::models::Candidate::default()
            },
            state: "downloading".into(),
            progress: 90.0,
            error: None,
            output_path: None,
            created_at: String::new(),
            output_profile: "source".into(),
            format_id: None,
            chapter_ids: None,
            soulseek: None,
        };
        store.save_job(&job).unwrap();
        drop(store);
        let store = crate::storage::Store::open(&recovery_db).unwrap();
        let recovered = store.get_job(batch_id).unwrap();
        assert_eq!(recovered.state, "interrupted");
        let recovered_path = wait_for_batch_file(
            &client,
            batch_id,
            "peer-account",
            recovered.candidate.source.as_ref().unwrap(),
            &downloads,
            &staging,
            &mut cancel,
            |_, _, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(recovered_path).await.unwrap(),
            audio_fixture
        );
        assert_eq!(submissions.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Exercise the real publication and persistence path after the peer
        // handoff. The fixture has a known BPM/key, so this checks tag writing
        // without depending on analysis of a very short silent WAV.
        let music = temporary.path().join("music");
        tokio::fs::create_dir_all(&music).await.unwrap();
        let settings = crate::models::Settings {
            music_directory: music.to_string_lossy().into_owned(),
            ..crate::models::Settings::default()
        };
        let mut track = crate::models::Track {
            id: "slskd:test-recording".into(),
            provider: "slskd".into(),
            provider_id: "test-recording".into(),
            artist: "Artist Name".into(),
            title: "Night Drive".into(),
            duration_seconds: Some(0.1),
            bpm: Some(120.0),
            key: Some("8A".into()),
            ..crate::models::Track::default()
        };
        track.provenance = json!({"acquisition":{"provider":"slskd","id":"slskd:test-candidate","source":source.clone()}});
        let candidate = crate::models::Candidate {
            source_metadata: None,
            id: "slskd:test-candidate".into(),
            provider: "slskd".into(),
            url: String::new(),
            title: "Artist Name - Night Drive".into(),
            uploader: Some("peer-account".into()),
            duration_seconds: None,
            artwork_url: None,
            chapters: None,
            formats: vec![],
            permission: "unknown".into(),
            confidence: "review".into(),
            score: 0.8,
            reasons: vec![],
            official: false,
            source: Some(source),
        };
        let store_path = temporary.path().join("burrata.sqlite");
        let store = crate::storage::Store::open(&store_path).unwrap();
        store.save_track(&track).unwrap();
        let asset = crate::media::finalize_download(
            &path,
            &track,
            &candidate,
            &settings,
            "source",
            "slskd-test-job",
        )
        .await
        .unwrap();
        crate::media::acknowledge_publication(&settings, "slskd-test-job").unwrap();
        store.save_asset(&asset).unwrap();

        let hydrated = store.get_track(&track.id).unwrap().unwrap();
        assert_eq!(hydrated.local_availability, "available");
        assert_eq!(hydrated.local_path.as_deref(), Some(asset.path.as_str()));
        assert_eq!(hydrated.asset_id.as_deref(), Some(asset.id.as_str()));
        use lofty::file::TaggedFileExt;
        let tagged = lofty::probe::Probe::open(&asset.path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).unwrap();
        use lofty::tag::Accessor;
        assert_eq!(tag.artist().as_deref(), Some("Artist Name"));
        assert_eq!(tag.title().as_deref(), Some("Night Drive"));
        let inspected =
            crate::media::inspect_local_file(std::path::Path::new(&asset.path), &settings)
                .await
                .unwrap();
        assert_eq!(inspected.bpm, Some(120.0));
        assert_eq!(inspected.key.as_deref(), Some("8A"));
        server.abort();
    }
}
