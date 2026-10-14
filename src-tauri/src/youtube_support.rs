//! Managed yt-dlp support and anonymous BgUtils token-provider lifecycle.
//!
//! All managed updater state lives below app data. The bundled executable and
//! arbitrary system executables are only ever read, never modified.
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use std::{
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

const REPO_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp-nightly-builds/releases/latest";
const RECOMMENDED_REPO_API: &str =
    "https://api.github.com/repos/anaxonda/burrata/releases?per_page=10";
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const HELPER_PORT: u16 = 4416;
const MAX_UPSTREAM_ASSET_BYTES: usize = 120 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportStatus {
    pub bundled_version: Option<String>,
    pub managed_version: Option<String>,
    pub active_version: Option<String>,
    pub active_source: String,
    pub update_available: Option<bool>,
    pub recommended_update: String,
    pub advanced_update: String,
    pub provider: ProviderStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub version: String,
    pub available: bool,
    pub helper_running: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub channel: String,
    pub version: Option<String>,
    pub published_at: Option<String>,
    pub available: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosisCategory {
    Ready,
    InvalidUrl,
    MissingRuntime,
    MissingEjs,
    TokenProvider,
    LoginRequired,
    Http403,
    ExpiredStream,
    VideoUnavailable,
    Network,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportDiagnosis {
    pub ok: bool,
    pub category: DiagnosisCategory,
    pub yt_dlp_version: Option<String>,
    pub provider_available: bool,
    pub audio_accessible: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionPointer {
    version: String,
    executable: String,
    previous: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendedManifest {
    version: String,
    targets: std::collections::HashMap<String, RecommendedTarget>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendedTarget {
    asset: String,
    sha256: String,
    sha512: String,
}

#[derive(Debug, Clone)]
struct ReleaseAsset {
    name: String,
    url: String,
}

#[derive(Clone)]
pub struct YouTubeSupportManager {
    data_dir: PathBuf,
    bundled_executable: PathBuf,
    node_executable: Option<PathBuf>,
    helper_script: Option<PathBuf>,
    plugin_dir: Option<PathBuf>,
    helper: Arc<Mutex<Option<Child>>>,
    helper_port: Arc<AtomicU64>,
    last_used: Arc<AtomicU64>,
    update_lock: Arc<Mutex<()>>,
    provider_verified: Arc<AtomicBool>,
}

impl YouTubeSupportManager {
    /// `bundled_executable`, Node, server script and plugin directory are
    /// read-only resources supplied by packaging. Node/provider can be absent.
    pub fn new(
        data_dir: impl Into<PathBuf>,
        bundled_executable: impl Into<PathBuf>,
        node_executable: Option<PathBuf>,
        helper_script: Option<PathBuf>,
        plugin_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            bundled_executable: bundled_executable.into(),
            node_executable,
            helper_script,
            plugin_dir,
            helper: Arc::new(Mutex::new(None)),
            helper_port: Arc::new(AtomicU64::new(HELPER_PORT.into())),
            last_used: Arc::new(AtomicU64::new(now_secs())),
            update_lock: Arc::new(Mutex::new(())),
            provider_verified: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn active_executable(&self) -> PathBuf {
        self.read_pointer("current.json")
            .map(|p| self.safe_version_executable(&p))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| self.bundled_executable.clone())
    }

    pub async fn status(&self) -> SupportStatus {
        let bundled = version(&self.bundled_executable).await.ok();
        let pointer = self.read_pointer("current.json");
        let managed = pointer
            .as_ref()
            .map(|p| p.version.clone())
            .filter(|v| v != "@bundled");
        let active = self.active_executable();
        let active_version = version(&active).await.ok();
        let running = {
            let mut helper = self.helper.lock().await;
            let exited = match helper.as_mut() {
                Some(child) => child.try_wait().ok().flatten().is_some(),
                None => false,
            };
            if exited {
                *helper = None;
            }
            helper.is_some()
        };
        let provider_ok = self.provider_assets_ready();
        SupportStatus {
            bundled_version: bundled,
            managed_version: managed.clone(),
            active_version,
            active_source: if managed.is_some() && active != self.bundled_executable {
                "managed"
            } else {
                "bundled"
            }
            .into(),
            update_available: None,
            recommended_update:
                "Check the recommended update feed from Burrata's configured trusted GitHub release.".into(),
            advanced_update: "Check GitHub for the official yt-dlp nightly build.".into(),
            provider: ProviderStatus {
                version: "2.0.2".into(),
                available: provider_ok && self.provider_verified.load(Ordering::Relaxed),
                helper_running: running,
                message: if provider_ok && self.provider_verified.load(Ordering::Relaxed) {
                    "Anonymous BgUtils provider passed a live compatibility probe. Tokens are generated locally without an account or cookies.".into()
                } else if provider_ok {
                    "Anonymous BgUtils assets are packaged, but live token-provider compatibility has not been verified yet.".into()
                } else {
                    "Anonymous BgUtils provider is unavailable. Package the matching provider 2.0.2 plugin, server build, and bundled Node runtime.".into()
                },
            },
        }
    }

    /// Recommended updates remain explicitly unavailable until Burrata
    /// publishes a signed/trusted manifest feed. No third-party manifest is
    /// accepted as a normal update source.
    pub async fn check_recommended_update(&self) -> UpdateCheck {
        match self.load_recommended().await {
            Ok(Some((release_version, published))) => {
                let current = version(&self.active_executable()).await.ok();
                UpdateCheck { channel: "recommended".into(),
                    available: current.as_deref() != Some(&release_version),
                    version: Some(release_version), published_at: published,
                    message: "Recommended yt-dlp update came from the configured Burrata GitHub release and includes both SHA2 checksums.".into() }
            }
            Ok(None) => UpdateCheck {
                channel: "recommended".into(),
                version: None,
                published_at: None,
                available: false,
                message:
                    "The selected published Burrata release does not publish youtube-support.json yet."
                        .into(),
            },
            Err(message) => UpdateCheck {
                channel: "recommended".into(),
                version: None,
                published_at: None,
                available: false,
                message,
            },
        }
    }

    pub async fn check_advanced_update(&self) -> Result<UpdateCheck, String> {
        let client = github_client()?;
        let release: Value = client
            .get(REPO_API)
            .send()
            .await
            .map_err(|e| format!("Could not check yt-dlp nightly: {e}"))?
            .error_for_status()
            .map_err(|e| format!("GitHub rejected the yt-dlp nightly check: {e}"))?
            .json()
            .await
            .map_err(|e| format!("GitHub returned invalid release metadata: {e}"))?;
        let release_version = release["tag_name"]
            .as_str()
            .ok_or("GitHub nightly release has no tag_name")?
            .to_owned();
        let published = release["published_at"].as_str().map(str::to_owned);
        let current = version(&self.active_executable()).await.ok();
        Ok(UpdateCheck { channel: "official-nightly".into(), available: current.as_deref() != Some(&release_version), version: Some(release_version), published_at: published,
            message: "Official yt-dlp nightly release metadata is available. Download verifies both upstream SHA2 checksum manifests before staging.".into() })
    }

    /// Downloads an official nightly, checks it against upstream SHA256 and
    /// SHA512 manifests, probes it, then atomically switches the app-data
    /// pointer. Existing installs and bundled/system files are untouched.
    pub async fn update_advanced(&self) -> Result<SupportStatus, String> {
        let _update_guard = self.update_lock.lock().await;
        let client = github_client()?;
        let release: Value = client
            .get(REPO_API)
            .send()
            .await
            .map_err(|e| format!("Could not fetch yt-dlp nightly metadata: {e}"))?
            .error_for_status()
            .map_err(|e| format!("GitHub rejected the nightly request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Invalid GitHub release metadata: {e}"))?;
        let version = release["tag_name"]
            .as_str()
            .ok_or("Release is missing tag_name")?
            .to_owned();
        validate_version(&version)?;
        let assets = parse_assets(&release)?;
        let binary_name = platform_asset_name()?;
        let binary_asset = assets
            .iter()
            .find(|a| a.name == binary_name)
            .ok_or_else(|| {
                format!("Official nightly has no {binary_name} asset for this platform")
            })?;
        let sums256 = assets
            .iter()
            .find(|a| a.name == "SHA2-256SUMS")
            .ok_or("Nightly has no official SHA2-256SUMS manifest")?;
        let sums512 = assets
            .iter()
            .find(|a| a.name == "SHA2-512SUMS")
            .ok_or("Nightly has no official SHA2-512SUMS manifest")?;
        let bytes = client
            .get(&binary_asset.url)
            .send()
            .await
            .map_err(|e| format!("Nightly binary download failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Nightly binary download failed: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("Could not read nightly binary: {e}"))?;
        let manifest256 = fetch_bytes(&client, &sums256.url).await?;
        let manifest512 = fetch_bytes(&client, &sums512.url).await?;
        verify_manifest_sha256(&binary_name, &bytes, &manifest256)?;
        verify_manifest_sha512(&binary_name, &bytes, &manifest512)?;

        self.stage_activate(&version, &bytes).await
    }

    pub async fn update_recommended(&self) -> Result<SupportStatus, String> {
        let _update_guard = self.update_lock.lock().await;
        let (target, asset, version) = self.load_recommended_bundle().await?;
        let bytes = fetch_bytes(&github_client()?, &asset.url).await?;
        verify_digest_hex("SHA256", &target.sha256, &bytes)?;
        verify_digest_hex("SHA512", &target.sha512, &bytes)?;
        self.stage_activate(&version, &bytes).await
    }

    async fn stage_activate(&self, version: &str, bytes: &[u8]) -> Result<SupportStatus, String> {
        let manager = self;
        self.stage_activate_with_probe(version, bytes, move |path| async move {
            manager.probe_candidate(&path).await
        })
        .await
    }

    async fn stage_activate_with_probe<F, Fut>(
        &self,
        expected_version: &str,
        bytes: &[u8],
        probe: F,
    ) -> Result<SupportStatus, String>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let root = self.managed_root();
        fs::create_dir_all(root.join("versions"))
            .map_err(|e| format!("Could not create managed YouTube support directory: {e}"))?;
        let version_dir = root.join("versions").join(expected_version);
        if version_dir.exists() {
            return Err(format!(
                "Version {expected_version} is already staged; it was left intact."
            ));
        }
        let staging = root
            .join("versions")
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging).map_err(|e| format!("Could not create staging directory: {e}"))?;
        let staged_exe = staging.join(executable_filename());
        let write_result = (|| -> Result<(), String> {
            let mut f = File::create(&staged_exe)
                .map_err(|e| format!("Could not create staged executable: {e}"))?;
            f.write_all(bytes)
                .map_err(|e| format!("Could not write staged executable: {e}"))?;
            f.sync_all()
                .map_err(|e| format!("Could not flush staged executable: {e}"))?;
            set_executable(&staged_exe)
        })();
        if let Err(e) = write_result {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
        let actual_version = match version(&staged_exe).await {
            Ok(actual) => actual,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(format!(
                    "Staged yt-dlp version probe failed; active version was kept: {error}"
                ));
            }
        };
        if actual_version != expected_version {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!(
                "Staged yt-dlp reported version {actual_version}, expected {expected_version}; active version was kept."
            ));
        }
        if let Err(e) = probe(staged_exe.clone()).await {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!(
                "Compatibility probe failed; active version was kept: {e}"
            ));
        }
        fs::rename(&staging, &version_dir).map_err(|e| {
            let _ = fs::remove_dir_all(&staging);
            format!("Could not commit staged version: {e}")
        })?;
        let previous = self
            .read_pointer("current.json")
            .map(|p| p.version)
            .unwrap_or_else(|| "@bundled".into());
        let pointer = VersionPointer {
            version: expected_version.to_owned(),
            executable: executable_filename().to_owned(),
            previous: Some(previous),
        };
        if let Err(e) = self.write_pointer_atomically("current.json", &pointer) {
            let _ = fs::remove_dir_all(&version_dir);
            return Err(e);
        }
        Ok(self.status().await)
    }

    async fn load_recommended(&self) -> Result<Option<(String, Option<String>)>, String> {
        let release = self.fetch_recommended_release().await?;
        let published = release["published_at"].as_str().map(str::to_owned);
        match self
            .load_recommended_manifest_from_release(&release)
            .await?
        {
            Some((_, _, version)) => Ok(Some((version, published))),
            None => Ok(None),
        }
    }

    async fn load_recommended_bundle(
        &self,
    ) -> Result<(RecommendedTarget, ReleaseAsset, String), String> {
        let release = self.fetch_recommended_release().await?;
        self.load_recommended_manifest_from_release(&release)
            .await?
            .ok_or_else(|| {
                "The selected published Burrata release does not publish youtube-support.json yet."
                    .to_owned()
            })
    }

    async fn fetch_recommended_release(&self) -> Result<Value, String> {
        let releases: Value = github_client()?
            .get(RECOMMENDED_REPO_API)
            .send()
            .await
            .map_err(|e| format!("Could not check Burrata's recommended update feed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Could not check Burrata's recommended update feed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Burrata's recommended release metadata is invalid: {e}"))?;
        select_recommended_release(&releases)
            .cloned()
            .ok_or_else(|| {
                "No published Burrata release in the latest 10 releases contains youtube-support.json."
                    .to_owned()
            })
    }

    async fn load_recommended_manifest_from_release(
        &self,
        release: &Value,
    ) -> Result<Option<(RecommendedTarget, ReleaseAsset, String)>, String> {
        let assets = parse_assets(release)?;
        let Some(feed_asset) = assets.iter().find(|a| a.name == "youtube-support.json") else {
            return Ok(None);
        };
        let feed = fetch_bytes(&github_client()?, &feed_asset.url).await?;
        let manifest: RecommendedManifest = serde_json::from_slice(&feed)
            .map_err(|e| format!("Burrata youtube-support.json is invalid: {e}"))?;
        validate_version(&manifest.version)?;
        let target = manifest
            .targets
            .get(target_triple())
            .ok_or_else(|| format!("Recommended feed has no artifact for {}", target_triple()))?;
        validate_checksum(&target.sha256, 64, "SHA256")?;
        validate_checksum(&target.sha512, 128, "SHA512")?;
        let asset = assets
            .iter()
            .find(|a| a.name == target.asset)
            .cloned()
            .ok_or("Recommended feed names an asset absent from this release")?;
        let asset_url =
            url::Url::parse(&asset.url).map_err(|_| "Recommended feed asset URL is invalid")?;
        if asset_url.scheme() != "https"
            || asset_url.host_str() != Some("github.com")
            || !asset_url
                .path()
                .starts_with("/anaxonda/burrata/releases/download/")
        {
            return Err(
                "Recommended feed asset is outside the configured Burrata GitHub release source"
                    .into(),
            );
        }
        Ok(Some((
            RecommendedTarget {
                asset: target.asset.clone(),
                sha256: target.sha256.clone(),
                sha512: target.sha512.clone(),
            },
            asset,
            manifest.version,
        )))
    }

    pub async fn rollback(&self) -> Result<SupportStatus, String> {
        let _update_guard = self.update_lock.lock().await;
        let current = self
            .read_pointer("current.json")
            .ok_or("No managed YouTube version is active; bundled version is already protected")?;
        let previous = current
            .previous
            .ok_or("No previous managed YouTube version is available")?;
        if previous != "@bundled" {
            let prior_path = self
                .managed_root()
                .join("versions")
                .join(&previous)
                .join(executable_filename());
            if !prior_path.is_file() {
                return Err(
                    "Previous managed version is missing; current version remains active".into(),
                );
            }
        }
        let next = VersionPointer {
            version: previous,
            executable: executable_filename().to_owned(),
            previous: Some(current.version),
        };
        self.write_pointer_atomically("current.json", &next)?;
        Ok(self.status().await)
    }

    /// Ensure the bundled server is answering at the loopback URL. Caller must
    /// include `--extractor-args youtubepot-bgutilhttp:base_url=...` and
    /// `--plugin-dirs <trusted plugin dir>` for yt-dlp invocations.
    pub async fn ensure_provider(&self) -> Result<String, String> {
        if !self.provider_assets_ready() {
            return Err("BgUtils provider is unavailable. Install/package the matching 2.0.2 plugin, server build and bundled Node runtime.".into());
        }
        let _ = self.stop_helper_if_idle().await;
        let mut lock = self.helper.lock().await;
        if let Some(child) = lock.as_mut() {
            if child
                .try_wait()
                .map_err(|e| format!("Could not check BgUtils helper: {e}"))?
                .is_some()
            {
                *lock = None;
            }
        }
        if lock.is_none() {
            let port = pick_port().await?;
            let node = self.node_executable.as_ref().expect("checked assets");
            let script = self.helper_script.as_ref().expect("checked assets");
            let child = Command::new(node)
                .arg(script)
                .args(["--port", &port.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| {
                    format!("Could not start bundled BgUtils helper with bundled Node: {e}")
                })?;
            *lock = Some(child);
            self.helper_port.store(port.into(), Ordering::Relaxed);
            self.start_idle_monitor();
        }
        self.last_used.store(now_secs(), Ordering::Relaxed);
        let port = self.helper_port.load(Ordering::Relaxed) as u16;
        drop(lock);
        let url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;
        for _ in 0..20 {
            if let Ok(r) = client.get(format!("{url}/ping")).send().await {
                if r.status().is_success() {
                    return Ok(url);
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.provider_verified.store(false, Ordering::Relaxed);
        let _ = self.shutdown().await;
        Err("BgUtils helper started but its /ping probe did not succeed. Check the bundled provider server build.".into())
    }

    /// Args to append to each yt-dlp invocation that can use YouTube. This
    /// starts the local provider on demand and points yt-dlp at only the
    /// packaged, trusted plugin directory.
    pub async fn provider_args(&self) -> Result<Vec<std::ffi::OsString>, String> {
        if !self.provider_assets_present() {
            return Ok(Vec::new());
        }
        if !self.provider_assets_ready() {
            return Err("BgUtils provider bundle is incomplete. Package its matching plugin 2.0.2, server build, and bundled Node runtime.".into());
        }
        let url = self.ensure_provider().await?;
        let plugin_dir = self
            .plugin_dir
            .as_ref()
            .ok_or("Trusted BgUtils plugin directory is not packaged")?;
        Ok(vec![
            "--plugin-dirs".into(),
            plugin_dir.as_os_str().to_owned(),
            "--js-runtimes".into(),
            format!(
                "node:{}",
                self.node_executable
                    .as_ref()
                    .ok_or("Bundled Node runtime is not packaged")?
                    .display()
            )
            .into(),
            "--extractor-args".into(),
            format!("youtubepot-bgutilhttp:base_url={url}").into(),
        ])
    }

    pub async fn stop_helper_if_idle(&self) -> Result<bool, String> {
        if now_secs().saturating_sub(self.last_used.load(Ordering::Relaxed))
            < IDLE_TIMEOUT.as_secs()
        {
            return Ok(false);
        }
        let mut lock = self.helper.lock().await;
        if let Some(child) = lock.as_mut() {
            child
                .start_kill()
                .map_err(|e| format!("Could not stop idle BgUtils helper: {e}"))?;
            *lock = None;
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn probe_candidate(&self, executable: &Path) -> Result<(), String> {
        if !self.provider_assets_ready() {
            return Err("Cannot verify candidate without the packaged Node, BgUtils server, and trusted plugin assets.".into());
        }
        let result = self
            .diagnose_executable(executable, "https://www.youtube.com/watch?v=jNQXAC9IVRw")
            .await;
        if candidate_probe_succeeded(&result) {
            Ok(())
        } else {
            Err(format!(
                "YouTube audio compatibility probe failed ({:?}): {}",
                result.category, result.message
            ))
        }
    }

    /// Run a real public YouTube extraction through the packaged plugin and
    /// local helper. Version checks alone do not mark provider support healthy.
    pub async fn capability_check(&self) -> Result<SupportStatus, String> {
        self.probe_candidate(&self.active_executable()).await?;
        Ok(self.status().await)
    }

    /// Diagnose a caller-selected YouTube URL (or use the known public test
    /// video when `None`). Metadata extraction is followed by a 1-byte ranged
    /// request to the selected audio stream. Never returns the URL or raw
    /// yt-dlp output because both may contain short-lived query tokens.
    pub async fn diagnose_url(&self, url: Option<&str>) -> SupportDiagnosis {
        let sample = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
        let url = url.unwrap_or(sample);
        if !is_supported_youtube_url(url) {
            return diagnosis(
                false,
                DiagnosisCategory::InvalidUrl,
                None,
                self.provider_assets_ready(),
                false,
                "Enter an HTTPS YouTube video URL to run this support check.",
            );
        }
        self.diagnose_executable(&self.active_executable(), url)
            .await
    }

    async fn diagnose_executable(&self, executable: &Path, url: &str) -> SupportDiagnosis {
        let version = version(executable).await.ok();
        let provider = self.provider_args().await;
        let provider_available =
            !provider.as_ref().is_ok_and(|v| v.is_empty()) && self.provider_assets_ready();
        let args = match provider {
            Ok(args) => args,
            Err(_) => return diagnosis(false, DiagnosisCategory::TokenProvider, version, false, false,
                "The bundled anonymous token provider could not start. Check the provider bundle and Node runtime.")
        };
        let mut command = Command::new(executable);
        command.args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-playlist",
            "--no-warnings",
            "-f",
            "bestaudio",
            "-g",
        ]);
        command
            .args(args)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output =
            match run_capped(command, Duration::from_secs(45), 64 * 1024).await {
                Err(e) if e.contains("timed out") => {
                    return diagnosis(
                        false,
                        DiagnosisCategory::Network,
                        version,
                        provider_available,
                        false,
                        "YouTube extraction exceeded 45 seconds. Check your connection and retry.",
                    )
                }
                Err(e) if e.contains("output limit") => {
                    return diagnosis(
                        false,
                        DiagnosisCategory::Unsupported,
                        version,
                        provider_available,
                        false,
                        "yt-dlp returned an unexpectedly large response; no data was retained.",
                    )
                }
                Err(_) => return diagnosis(
                    false,
                    DiagnosisCategory::MissingRuntime,
                    version,
                    provider_available,
                    false,
                    "yt-dlp could not start. Check the bundled or configured yt-dlp executable.",
                ),
                Ok(output) => output,
            };
        if !output.status.success() {
            let category = classify_ytdlp_error(&String::from_utf8_lossy(&output.stderr));
            let message = diagnosis_message(&category);
            return diagnosis(false, category, version, provider_available, false, message);
        }
        let stream = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let stream_url = match url::Url::parse(&stream) {
            Ok(value)
                if value.scheme() == "https"
                    && value.host_str().is_some_and(|h| {
                        h == "googlevideo.com" || h.ends_with(".googlevideo.com")
                    }) =>
            {
                value
            }
            _ => {
                return diagnosis(
                    false,
                    DiagnosisCategory::Unsupported,
                    version,
                    provider_available,
                    false,
                    "yt-dlp did not return a recognized YouTube audio stream URL.",
                )
            }
        };
        let client = match Client::builder().timeout(Duration::from_secs(15)).build() {
            Ok(c) => c,
            Err(_) => {
                return diagnosis(
                    false,
                    DiagnosisCategory::Network,
                    version,
                    provider_available,
                    false,
                    "Could not initialize the audio stream check.",
                )
            }
        };
        let response = client
            .get(stream_url)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() && (response.status() == reqwest::StatusCode::PARTIAL_CONTENT || response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("audio/") || v.starts_with("video/") || v == "application/octet-stream")) => {
                if provider_available { self.provider_verified.store(true, Ordering::Relaxed); }
                diagnosis(true, DiagnosisCategory::Ready, version, provider_available, true,
                    "YouTube metadata extraction and a short audio stream access check both succeeded.")
            }
            Ok(response) if response.status() == reqwest::StatusCode::FORBIDDEN => diagnosis(false, DiagnosisCategory::ExpiredStream, version, provider_available, false,
                "YouTube returned 403 for the selected audio stream. The stream URL may have expired; retry extraction."),
            Ok(response) => diagnosis(false, DiagnosisCategory::Network, version, provider_available, false,
                &format!("The selected audio stream did not accept a short access check (HTTP {}).", response.status().as_u16())),
            Err(_) => diagnosis(false, DiagnosisCategory::Network, version, provider_available, false,
                "The selected audio stream could not be reached. Check your connection and retry."),
        }
    }

    /// Call on application exit. Child kill-on-drop is also enabled as a
    /// fallback, while this method gives shutdown code a deterministic stop.
    pub async fn shutdown(&self) -> Result<(), String> {
        let mut guard = self.helper.lock().await;
        if let Some(child) = guard.as_mut() {
            child
                .start_kill()
                .map_err(|e| format!("Could not stop BgUtils helper during shutdown: {e}"))?;
        }
        *guard = None;
        Ok(())
    }

    fn provider_assets_ready(&self) -> bool {
        self.node_executable.as_ref().is_some_and(|p| p.is_file())
            && self.helper_script.as_ref().is_some_and(|p| p.is_file())
            && self
                .plugin_dir
                .as_ref()
                .is_some_and(|p| p.is_dir() || p.is_file())
    }
    fn provider_assets_present(&self) -> bool {
        self.node_executable.as_ref().is_some_and(|p| p.exists())
            || self.helper_script.as_ref().is_some_and(|p| p.exists())
            || self.plugin_dir.as_ref().is_some_and(|p| p.exists())
    }
    fn managed_root(&self) -> PathBuf {
        self.data_dir.join("youtube-support")
    }
    fn pointer_path(&self, name: &str) -> PathBuf {
        self.managed_root().join(name)
    }
    fn read_pointer(&self, name: &str) -> Option<VersionPointer> {
        let data = fs::read(self.pointer_path(name)).ok()?;
        let pointer: VersionPointer = serde_json::from_slice(&data).ok()?;
        if pointer.version != "@bundled" {
            validate_version(&pointer.version).ok()?;
        }
        Some(pointer)
    }
    fn safe_version_executable(&self, pointer: &VersionPointer) -> PathBuf {
        if pointer.version == "@bundled" {
            return self.bundled_executable.clone();
        }
        let name = Path::new(&pointer.executable);
        if name.components().count() != 1
            || name
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return PathBuf::new();
        }
        self.managed_root()
            .join("versions")
            .join(&pointer.version)
            .join(name)
    }
    fn write_pointer_atomically(&self, name: &str, pointer: &VersionPointer) -> Result<(), String> {
        let root = self.managed_root();
        fs::create_dir_all(&root)
            .map_err(|e| format!("Could not create pointer directory: {e}"))?;
        let temp = root.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
        let mut f =
            File::create(&temp).map_err(|e| format!("Could not create pointer temp file: {e}"))?;
        serde_json::to_writer(&mut f, pointer)
            .map_err(|e| format!("Could not serialize version pointer: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("Could not flush pointer: {e}"))?;
        replace_atomically(&temp, &self.pointer_path(name)).map_err(|e| {
            let _ = fs::remove_file(&temp);
            format!("Could not atomically switch YouTube version pointer: {e}")
        })
    }
    fn start_idle_monitor(&self) {
        let helper = self.helper.clone();
        let last_used = self.last_used.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                if now_secs().saturating_sub(last_used.load(Ordering::Relaxed))
                    < IDLE_TIMEOUT.as_secs()
                {
                    continue;
                }
                let mut guard = helper.lock().await;
                if let Some(child) = guard.as_mut() {
                    let _ = child.start_kill();
                    *guard = None;
                }
                break;
            }
        });
    }
}

/// Process-wide manager hook. The application initializes this once during
/// Tauri setup with resolved paths under `resources/tools/<target>/...` and
/// app data. Individual modules can then request the active executable and
/// async provider arguments without changing their public command contracts.
static GLOBAL_MANAGER: std::sync::OnceLock<YouTubeSupportManager> = std::sync::OnceLock::new();

pub fn initialize_global(manager: YouTubeSupportManager) -> Result<(), String> {
    GLOBAL_MANAGER
        .set(manager)
        .map_err(|_| "YouTube support manager was initialized twice".into())
}
pub fn global() -> Option<&'static YouTubeSupportManager> {
    GLOBAL_MANAGER.get()
}
pub fn active_executable() -> PathBuf {
    GLOBAL_MANAGER
        .get()
        .map(YouTubeSupportManager::active_executable)
        .unwrap_or_else(|| PathBuf::from("yt-dlp"))
}
pub async fn yt_dlp_provider_args() -> Result<Vec<std::ffi::OsString>, String> {
    GLOBAL_MANAGER
        .get()
        .ok_or("YouTube support manager has not been initialized")?
        .provider_args()
        .await
}

pub fn verify_manifest_sha256(name: &str, bytes: &[u8], manifest: &[u8]) -> Result<(), String> {
    let expected = manifest_digest(name, manifest)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(format!(
            "SHA256 mismatch for {name}; downloaded bytes were discarded"
        ))
    }
}
pub fn verify_manifest_sha512(name: &str, bytes: &[u8], manifest: &[u8]) -> Result<(), String> {
    let expected = manifest_digest(name, manifest)?;
    let actual = format!("{:x}", Sha512::digest(bytes));
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(format!(
            "SHA512 mismatch for {name}; downloaded bytes were discarded"
        ))
    }
}
fn manifest_digest(name: &str, manifest: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(manifest).map_err(|_| "Checksum manifest is not UTF-8")?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let digest = fields.next().unwrap_or("").trim_start_matches('*');
        let file = fields.next().unwrap_or("").trim_start_matches('*');
        if file == name && digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(digest.to_owned());
        }
    }
    Err(format!(
        "Official checksum manifest does not contain {name}"
    ))
}

fn validate_checksum(value: &str, length: usize, label: &str) -> Result<(), String> {
    if value.len() != length || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "Recommended feed contains an invalid {label} checksum"
        ));
    }
    Ok(())
}
fn verify_digest_hex(label: &str, expected: &str, bytes: &[u8]) -> Result<(), String> {
    let actual = if label == "SHA256" {
        format!("{:x}", Sha256::digest(bytes))
    } else if label == "SHA512" {
        format!("{:x}", Sha512::digest(bytes))
    } else {
        return Err("Unsupported checksum algorithm".into());
    };
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "{label} mismatch for recommended yt-dlp; downloaded bytes were discarded"
        ))
    }
}
fn target_triple() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        "unsupported-target"
    }
}
fn is_supported_youtube_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|h| {
                matches!(
                    h,
                    "youtube.com"
                        | "www.youtube.com"
                        | "music.youtube.com"
                        | "youtu.be"
                        | "m.youtube.com"
                )
            })
    })
}
fn classify_ytdlp_error(stderr: &str) -> DiagnosisCategory {
    let s = stderr.to_ascii_lowercase();
    if s.contains("video unavailable")
        || s.contains("this video is unavailable")
        || s.contains("video has been removed")
    {
        DiagnosisCategory::VideoUnavailable
    } else if s.contains("javascript runtime")
        || s.contains("js runtime")
        || s.contains("node not found")
        || s.contains("deno not found")
    {
        DiagnosisCategory::MissingRuntime
    } else if s.contains("ejs")
        || s.contains("javascript challenge")
        || s.contains("challenge solver")
    {
        DiagnosisCategory::MissingEjs
    } else if s.contains("token provider")
        || s.contains("bgutil")
        || s.contains("po token")
        || s.contains("pot provider")
    {
        DiagnosisCategory::TokenProvider
    } else if s.contains("sign in")
        || s.contains("login required")
        || s.contains("authentication required")
        || s.contains("confirm you're not a bot")
    {
        DiagnosisCategory::LoginRequired
    } else if s.contains("403") || s.contains("forbidden") {
        DiagnosisCategory::Http403
    } else if s.contains("expired") || s.contains("signature has expired") {
        DiagnosisCategory::ExpiredStream
    } else if s.contains("unsupported url") || s.contains("not a valid url") {
        DiagnosisCategory::Unsupported
    } else {
        DiagnosisCategory::Network
    }
}
fn diagnosis_message(category: &DiagnosisCategory) -> &'static str {
    match category {
        DiagnosisCategory::MissingRuntime => "yt-dlp could not find a usable JavaScript runtime. Check the bundled Node runtime.",
        DiagnosisCategory::MissingEjs => "The yt-dlp JavaScript challenge solver (EJS) is missing or could not run.",
        DiagnosisCategory::TokenProvider => "The anonymous BgUtils token provider failed. Check its matching plugin, helper, and Node runtime.",
        DiagnosisCategory::LoginRequired => "YouTube requires sign-in for this video or network. Burrata does not use account cookies.",
        DiagnosisCategory::Http403 => "YouTube returned HTTP 403. Retry with the current yt-dlp nightly or run this check again later.",
        DiagnosisCategory::ExpiredStream => "YouTube returned an expired stream. Re-extract the video to get a fresh audio URL.",
        DiagnosisCategory::VideoUnavailable => "YouTube reports that this video is unavailable. Check that it is public and playable in your region.",
        DiagnosisCategory::Unsupported => "yt-dlp did not recognize this video URL.",
        _ => "YouTube extraction failed. Check your connection and try again.",
    }
}
fn diagnosis(
    ok: bool,
    category: DiagnosisCategory,
    version: Option<String>,
    provider_available: bool,
    audio_accessible: bool,
    message: &str,
) -> SupportDiagnosis {
    SupportDiagnosis {
        ok,
        category,
        yt_dlp_version: version,
        provider_available,
        audio_accessible,
        message: message.to_owned(),
    }
}
fn candidate_probe_succeeded(result: &SupportDiagnosis) -> bool {
    result.ok && result.audio_accessible
}

async fn version(executable: &Path) -> Result<String, String> {
    if !executable.is_file() {
        return Err("executable missing".into());
    }
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = run_capped(command, Duration::from_secs(10), 4096).await?;
    if !output.status.success() {
        return Err("version probe failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn run_capped(
    mut command: Command,
    timeout: Duration,
    byte_limit: usize,
) -> Result<std::process::Output, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start process: {e}"))?;
    let stdout = child.stdout.take().ok_or("stdout pipe was not created")?;
    let stderr = child.stderr.take().ok_or("stderr pipe was not created")?;
    let out_task = tokio::spawn(read_bounded(stdout, byte_limit));
    let err_task = tokio::spawn(read_bounded(stderr, byte_limit));
    tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) = tokio::join!(child.wait(), out_task, err_task);
        let status = status.map_err(|e| format!("process wait failed: {e}"))?;
        let (stdout, out_exceeded) = stdout
            .map_err(|e| format!("stdout reader failed: {e}"))?
            .map_err(|e| format!("stdout read failed: {e}"))?;
        let (stderr, err_exceeded) = stderr
            .map_err(|e| format!("stderr reader failed: {e}"))?
            .map_err(|e| format!("stderr read failed: {e}"))?;
        if out_exceeded || err_exceeded {
            return Err("process output limit exceeded".to_owned());
        }
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await
    .map_err(|_| "process timed out".to_owned())?
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> Result<(Vec<u8>, bool), std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut overflow = false;
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
        overflow |= read > remaining;
    }
    Ok((output, overflow))
}

fn github_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent("Burrata-YouTube-Support/1.0")
        .build()
        .map_err(|e| e.to_string())
}
async fn fetch_bytes(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Could not fetch upstream checksum manifest: {e}"))?;
    let response = response
        .error_for_status()
        .map_err(|e| format!("Could not fetch upstream release asset: {e}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_UPSTREAM_ASSET_BYTES as u64)
    {
        return Err("Upstream YouTube support asset exceeds the 120 MiB size limit".into());
    }
    let mut stream = response.bytes_stream();
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Could not read upstream release asset: {e}"))?;
        if output.len().saturating_add(chunk.len()) > MAX_UPSTREAM_ASSET_BYTES {
            return Err("Upstream YouTube support asset exceeds the 120 MiB size limit".into());
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}
fn parse_assets(release: &Value) -> Result<Vec<ReleaseAsset>, String> {
    release["assets"]
        .as_array()
        .ok_or("Release has no assets")?
        .iter()
        .map(|a| {
            Ok(ReleaseAsset {
                name: a["name"]
                    .as_str()
                    .ok_or("Release asset has no name")?
                    .into(),
                url: a["browser_download_url"]
                    .as_str()
                    .ok_or("Release asset has no download URL")?
                    .into(),
            })
        })
        .collect()
}
fn select_recommended_release(releases: &Value) -> Option<&Value> {
    releases.as_array()?.iter().find(|release| {
        release["draft"].as_bool() == Some(false)
            && release["published_at"].as_str().is_some()
            && release["assets"].as_array().is_some_and(|assets| {
                assets
                    .iter()
                    .any(|asset| asset["name"] == "youtube-support.json")
            })
    })
}
fn platform_asset_name() -> Result<&'static str, String> {
    if cfg!(target_os = "windows") {
        Ok("yt-dlp.exe")
    } else if cfg!(target_os = "macos") {
        Ok("yt-dlp_macos")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Ok("yt-dlp_linux")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Ok("yt-dlp_linux_aarch64")
    } else {
        Err("Official nightly updates are unavailable for this operating system/architecture; the bundled baseline remains active".into())
    }
}
fn executable_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}
fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Could not mark staged yt-dlp executable: {e}"))?;
    }
    Ok(())
}
fn replace_atomically(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        #[link(name = "Kernel32")]
        extern "system" {
            fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: both strings are NUL-terminated UTF-16 buffers valid for this call.
        let ok = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)
    }
}
fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > 80
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err("Unsafe release version string".into());
    }
    Ok(())
}
async fn pick_port() -> Result<u16, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("Could not reserve local BgUtils port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifies_upstream_checksum_manifest_entries() {
        let bytes = b"managed yt-dlp";
        let sha256 = format!("{}  yt-dlp_linux\n", format!("{:x}", Sha256::digest(bytes)));
        let sha512 = format!("{} *yt-dlp_linux\n", format!("{:x}", Sha512::digest(bytes)));
        assert!(verify_manifest_sha256("yt-dlp_linux", bytes, sha256.as_bytes()).is_ok());
        assert!(verify_manifest_sha512("yt-dlp_linux", bytes, sha512.as_bytes()).is_ok());
        assert!(verify_manifest_sha256("yt-dlp_linux", b"tampered", sha256.as_bytes()).is_err());
        assert!(
            verify_digest_hex("SHA256", &format!("{:x}", Sha256::digest(bytes)), bytes).is_ok()
        );
        assert!(
            verify_digest_hex("SHA512", &format!("{:x}", Sha512::digest(bytes)), bytes).is_ok()
        );
    }

    #[test]
    fn rejects_path_traversal_in_release_pointer() {
        assert!(validate_version("../system").is_err());
        let pointer = VersionPointer {
            version: "nightly.1".into(),
            executable: "../../usr/bin/yt-dlp".into(),
            previous: None,
        };
        let manager = YouTubeSupportManager::new("/tmp/data", "/usr/bin/yt-dlp", None, None, None);
        assert!(manager
            .safe_version_executable(&pointer)
            .as_os_str()
            .is_empty());
    }

    #[test]
    fn pointer_is_app_data_scoped_and_switches_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = YouTubeSupportManager::new(tmp.path(), "/usr/bin/yt-dlp", None, None, None);
        let root = manager.managed_root();
        fs::create_dir_all(root.join("versions/v1")).unwrap();
        let executable = executable_filename();
        fs::write(root.join("versions/v1").join(&executable), b"fixture").unwrap();
        let p = VersionPointer {
            version: "v1".into(),
            executable: executable.into(),
            previous: None,
        };
        manager
            .write_pointer_atomically("current.json", &p)
            .unwrap();
        assert_eq!(
            manager.active_executable(),
            root.join("versions/v1").join(executable_filename())
        );
        assert!(root.join("current.json").is_file());
    }

    #[tokio::test]
    async fn rollback_returns_to_bundled_baseline_then_toggles_back() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("resources/yt-dlp");
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, b"bundled fixture").unwrap();
        let manager =
            YouTubeSupportManager::new(tmp.path().join("data"), &bundled, None, None, None);
        let root = manager.managed_root();
        fs::create_dir_all(root.join("versions/v2")).unwrap();
        fs::write(
            root.join("versions/v2").join(executable_filename()),
            b"managed fixture",
        )
        .unwrap();
        manager
            .write_pointer_atomically(
                "current.json",
                &VersionPointer {
                    version: "v2".into(),
                    executable: executable_filename().into(),
                    previous: Some("@bundled".into()),
                },
            )
            .unwrap();
        manager.rollback().await.unwrap();
        assert_eq!(manager.active_executable(), bundled);
        let bundled_pointer = manager.read_pointer("current.json").unwrap();
        assert_eq!(bundled_pointer.version, "@bundled");
        assert_eq!(bundled_pointer.previous.as_deref(), Some("v2"));
        manager.rollback().await.unwrap();
        assert_eq!(
            manager.active_executable(),
            root.join("versions/v2").join(executable_filename())
        );
        let managed_pointer = manager.read_pointer("current.json").unwrap();
        assert_eq!(managed_pointer.version, "v2");
        assert_eq!(managed_pointer.previous.as_deref(), Some("@bundled"));
    }

    #[tokio::test]
    async fn corrupt_pointer_falls_back_to_bundled_and_cannot_be_rolled_back() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("resources/yt-dlp");
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, b"bundled fixture").unwrap();
        let manager =
            YouTubeSupportManager::new(tmp.path().join("data"), &bundled, None, None, None);
        fs::create_dir_all(manager.managed_root()).unwrap();
        fs::write(manager.pointer_path("current.json"), b"{ broken pointer").unwrap();

        assert!(manager.read_pointer("current.json").is_none());
        assert_eq!(manager.active_executable(), bundled);
        assert!(manager.rollback().await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_candidate_probe_keeps_existing_active_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("resources/yt-dlp");
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, b"bundled fixture").unwrap();
        let manager =
            YouTubeSupportManager::new(tmp.path().join("data"), &bundled, None, None, None);
        let root = manager.managed_root();
        let active_dir = root.join("versions/active.1");
        fs::create_dir_all(&active_dir).unwrap();
        let active = active_dir.join(executable_filename());
        fs::write(&active, b"existing managed fixture").unwrap();
        manager
            .write_pointer_atomically(
                "current.json",
                &VersionPointer {
                    version: "active.1".into(),
                    executable: executable_filename().into(),
                    previous: Some("@bundled".into()),
                },
            )
            .unwrap();

        let staged_script = b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo candidate.2; exit 0; fi\nexit 1\n";
        let failure = manager
            .stage_activate_with_probe("candidate.2", staged_script, |_| async {
                Err("mock YouTube extraction failure".into())
            })
            .await
            .unwrap_err();

        assert!(failure.contains("Compatibility probe failed"));
        assert!(failure.contains("mock YouTube extraction failure"));
        assert_eq!(manager.active_executable(), active);
        assert_eq!(
            manager.read_pointer("current.json").unwrap().version,
            "active.1"
        );
        assert!(!root.join("versions/candidate.2").exists());
        let version_dirs = fs::read_dir(root.join("versions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(version_dirs.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_binary_must_report_the_requested_release_version() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("resources/yt-dlp");
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, b"bundled fixture").unwrap();
        let manager =
            YouTubeSupportManager::new(tmp.path().join("data"), &bundled, None, None, None);

        let script = b"#!/bin/sh\necho wrong.version\n";
        let failure = manager
            .stage_activate_with_probe("expected.version", script, |_| async {
                panic!("compatibility probe must not run when version differs");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap_err();

        assert!(failure.contains("reported version wrong.version"));
        assert!(failure.contains("expected expected.version"));
        assert_eq!(manager.active_executable(), bundled);
        assert!(manager.read_pointer("current.json").is_none());
        let version_dirs = fs::read_dir(manager.managed_root().join("versions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(version_dirs.is_empty());
    }

    #[test]
    fn diagnostics_classify_common_failures_without_echoing_input() {
        assert_eq!(
            classify_ytdlp_error("No supported JavaScript runtime could be found"),
            DiagnosisCategory::MissingRuntime
        );
        assert_eq!(
            classify_ytdlp_error("yt-dlp-ejs is required to solve challenge"),
            DiagnosisCategory::MissingEjs
        );
        assert_eq!(
            classify_ytdlp_error("BgUtil token provider could not fetch POT"),
            DiagnosisCategory::TokenProvider
        );
        assert_eq!(
            classify_ytdlp_error("Sign in to confirm you are not a bot"),
            DiagnosisCategory::LoginRequired
        );
        assert_eq!(
            classify_ytdlp_error("ERROR: [youtube] Video unavailable"),
            DiagnosisCategory::VideoUnavailable
        );
        assert!(!is_supported_youtube_url(
            "http://youtube.com/watch?v=secret"
        ));
        assert!(!is_supported_youtube_url(
            "https://example.com/watch?v=secret"
        ));
    }

    #[test]
    fn candidate_activation_requires_audio_access_not_metadata_only() {
        let metadata_only = diagnosis(
            true,
            DiagnosisCategory::Ready,
            Some("candidate".into()),
            true,
            false,
            "metadata extracted",
        );
        assert!(!candidate_probe_succeeded(&metadata_only));

        let audio_verified = diagnosis(
            true,
            DiagnosisCategory::Ready,
            Some("candidate".into()),
            true,
            true,
            "audio reachable",
        );
        assert!(candidate_probe_succeeded(&audio_verified));
    }

    #[test]
    fn recommended_feed_selects_newest_published_release_including_prereleases() {
        let releases = serde_json::json!([
            {"tag_name":"draft","draft":true,"prerelease":false,"published_at":null,
             "assets":[{"name":"youtube-support.json"}]},
            {"tag_name":"candidate","draft":false,"prerelease":true,"published_at":"2026-10-12T12:00:00Z",
             "assets":[{"name":"youtube-support.json"}]},
            {"tag_name":"older","draft":false,"prerelease":false,"published_at":"2026-10-10T12:00:00Z",
             "assets":[{"name":"youtube-support.json"}]}
        ]);
        let selected = select_recommended_release(&releases).unwrap();
        assert_eq!(selected["tag_name"], "candidate");
    }

    #[test]
    fn recommended_feed_without_published_manifest_is_not_selected() {
        let releases = serde_json::json!([
            {"tag_name":"draft","draft":true,"published_at":null,
             "assets":[{"name":"youtube-support.json"}]},
            {"tag_name":"published-without-feed","draft":false,"published_at":"2026-10-12T12:00:00Z",
             "assets":[{"name":"burrata.tar.gz"}]}
        ]);
        assert!(select_recommended_release(&releases).is_none());
        assert!(select_recommended_release(&serde_json::json!([])).is_none());
    }
}
