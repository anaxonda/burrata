//! Per-user native messaging host registration for Burrata's browser companion.
//!
//! The manifests point at a fixed Burrata executable. No browser supplied value is
//! ever interpreted as a command or shell fragment.

use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const HOST: &str = "org.burrata.browser";
const FIREFOX_ID: &str = "burrata-browser@burrata.local";
const CHROMIUM_MANIFEST: &str = include_str!("../../extension/manifest.chromium.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Chrome,
    Chromium,
    Edge,
    Firefox,
}

impl Browser {
    fn all() -> [Browser; 4] {
        [Self::Firefox, Self::Chrome, Self::Chromium, Self::Edge]
    }
    fn key(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
        }
    }
    fn is_firefox(self) -> bool {
        self == Self::Firefox
    }
    fn directory(self, home: &Path) -> PathBuf {
        let platform = if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        };
        self.directory_for(home, &config_home(), &data_home(), platform)
    }

    fn directory_for(
        self,
        home: &Path,
        config: &Path,
        local_data: &Path,
        platform: Platform,
    ) -> PathBuf {
        if platform == Platform::MacOs {
            let base = home.join("Library/Application Support");
            return match self {
                Self::Firefox => base.join("Mozilla/NativeMessagingHosts"),
                Self::Chrome => base.join("Google/Chrome/NativeMessagingHosts"),
                Self::Chromium => base.join("Chromium/NativeMessagingHosts"),
                Self::Edge => base.join("Microsoft Edge/NativeMessagingHosts"),
            };
        }
        if platform == Platform::Windows {
            return local_data
                .join("Burrata/NativeMessagingHosts")
                .join(self.key());
        }
        match self {
            Self::Firefox => home.join(".mozilla/native-messaging-hosts"),
            Self::Chrome => config.join("google-chrome/NativeMessagingHosts"),
            Self::Chromium => config.join("chromium/NativeMessagingHosts"),
            Self::Edge => config.join("microsoft-edge/NativeMessagingHosts"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Linux,
    MacOs,
    Windows,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInstallStatus {
    pub browser: Browser,
    pub installed: bool,
    pub manifest_path: Option<PathBuf>,
    pub executable_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStatus {
    pub browsers: Vec<BrowserInstallStatus>,
}

fn data_home() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("AppData/Local"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir()
            .unwrap_or_default()
            .join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
    }
}
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
}

fn chromium_extension_id() -> Result<String, String> {
    let manifest: serde_json::Value =
        serde_json::from_str(CHROMIUM_MANIFEST).map_err(|e| e.to_string())?;
    let key = manifest["key"]
        .as_str()
        .ok_or("Chromium extension key is missing")?;
    fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u8;
        for byte in input.bytes().take_while(|b| *b != b'=') {
            let value = alphabet
                .iter()
                .position(|candidate| *candidate == byte)
                .ok_or("Invalid Chromium extension key")? as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                output.push((buffer >> bits) as u8);
            }
        }
        Ok(output)
    }
    let bytes = decode_base64(key)?;
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes);
    Ok(hash[..16]
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>()
        .chars()
        .map(|c| char::from(b'a' + c.to_digit(16).unwrap() as u8))
        .collect())
}

fn installed_host_executable(resource_dir: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "burrata-browser-host.exe"
    } else {
        "burrata-browser-host"
    };
    let source = bundled_host_path(resource_dir, crate::tools::target_triple(), cfg!(windows));
    if !source.is_file() {
        return Err(format!(
            "Bundled browser host is missing: {}",
            source.display()
        ));
    }
    let target = data_home()
        .join("org.burrata.discovery/integrations")
        .join(name);
    if source != target {
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        // Copy via a temporary file so a running/old host is never partially overwritten.
        let tmp = target.with_extension(if cfg!(windows) { "tmp.exe" } else { "tmp" });
        fs::copy(&source, &tmp).map_err(|e| format!("Could not install browser host: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        if target.exists() {
            fs::remove_file(&target).map_err(|e| e.to_string())?;
        }
        fs::rename(tmp, &target).map_err(|e| e.to_string())?;
    }
    Ok(target)
}

fn bundled_host_path(resource_dir: &Path, target: &str, windows: bool) -> PathBuf {
    resource_dir.join("tools").join(target).join(if windows {
        "burrata-browser-host.exe"
    } else {
        "burrata-browser-host"
    })
}

fn manifest(browser: Browser, executable: &Path) -> Result<serde_json::Value, String> {
    let allowed = if browser.is_firefox() {
        serde_json::json!({"allowed_extensions":[FIREFOX_ID]})
    } else {
        serde_json::json!({"allowed_origins":[format!("chrome-extension://{}/",chromium_extension_id()?)]})
    };
    let mut value = serde_json::json!({"name":HOST,"description":"Burrata local browser import bridge","path":executable,"type":"stdio"});
    if let Some(map) = allowed.as_object() {
        for (key, field) in map {
            value_set(&mut value, key, field.clone());
        }
    }
    Ok(value)
}
fn value_set(target: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    target[key] = value;
}

#[cfg(windows)]
fn registry(browser: Browser, manifest: &Path, remove: bool) -> Result<(), String> {
    let key = match browser {
        Browser::Firefox => r"HKCU\Software\Mozilla\NativeMessagingHosts\org.burrata.browser",
        Browser::Chrome => r"HKCU\Software\Google\Chrome\NativeMessagingHosts\org.burrata.browser",
        Browser::Chromium => r"HKCU\Software\Chromium\NativeMessagingHosts\org.burrata.browser",
        Browser::Edge => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\org.burrata.browser",
    };
    let mut cmd = Command::new("reg.exe");
    if remove {
        cmd.args(["delete", key, "/f"]);
    } else {
        cmd.args(["add", key, "/ve", "/t", "REG_SZ", "/d"])
            .arg(manifest)
            .arg("/f");
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() && !remove {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(())
}

fn write_manifest(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = target.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    // Unix rename replaces atomically; Windows requires deleting an existing target first.
    #[cfg(windows)]
    if target.exists() {
        fs::remove_file(target).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, target).map_err(|e| e.to_string())
}

pub fn status() -> InstallStatus {
    let browsers = Browser::all()
        .into_iter()
        .map(|browser| {
            let manifest = browser
                .directory(&dirs::home_dir().unwrap_or_default())
                .join(format!("{HOST}.json"));
            let parsed = fs::read(&manifest)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            let executable = parsed
                .as_ref()
                .and_then(|v| v["path"].as_str())
                .map(PathBuf::from);
            let installed = executable.as_ref().is_some_and(|path| path.is_file())
                && parsed
                    .as_ref()
                    .is_some_and(|v| v["name"] == HOST && v["type"] == "stdio");
            BrowserInstallStatus {
                browser,
                installed,
                manifest_path: manifest.exists().then_some(manifest),
                executable_path: executable,
            }
        })
        .collect();
    InstallStatus { browsers }
}

pub fn register(resource_dir: &Path) -> Result<InstallStatus, String> {
    let executable = installed_host_executable(resource_dir)?;
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    for browser in Browser::all() {
        let directory = browser.directory(&home);
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let target = directory.join(format!("{HOST}.json"));
        let bytes = serde_json::to_vec_pretty(&manifest(browser, &executable)?)
            .map_err(|e| e.to_string())?;
        write_manifest(&target, &bytes)?;
        #[cfg(windows)]
        registry(browser, &target, false)?;
    }
    Ok(status())
}

pub fn unregister() -> Result<InstallStatus, String> {
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    for browser in Browser::all() {
        let target = browser.directory(&home).join(format!("{HOST}.json"));
        if let Ok(bytes) = fs::read(&target) {
            let parsed: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            // Only remove a manifest that identifies itself as ours.
            if parsed["name"] == HOST {
                #[cfg(windows)]
                registry(browser, &target, true)?;
                fs::remove_file(target).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(status())
}

/// Entry point for `burrata --mcp`; the console sidecar owns protocol stdio and
/// resolves Node and the bridge relative to the immutable packaged resource tree.
pub fn mcp_launcher_main() -> Result<(), String> {
    use std::process::Stdio;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let binary = if cfg!(windows) {
        "burrata-mcp.exe"
    } else {
        "burrata-mcp"
    };
    let mut candidates = Vec::new();
    for ancestor in current.ancestors() {
        candidates.push(
            ancestor
                .join("tools")
                .join(crate::tools::target_triple())
                .join(binary),
        );
        candidates.push(
            ancestor
                .join("Resources/tools")
                .join(crate::tools::target_triple())
                .join(binary),
        );
        candidates.push(
            ancestor
                .join("resources/tools")
                .join(crate::tools::target_triple())
                .join(binary),
        );
    }
    let launcher = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or("Burrata MCP launcher is missing")?;
    let status = Command::new(launcher)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Burrata MCP launcher exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_manifests_preserve_committed_extension_ids() {
        let path = Path::new("/tmp/burrata-browser-host");
        let firefox = manifest(Browser::Firefox, path).unwrap();
        let chromium = manifest(Browser::Edge, path).unwrap();
        assert_eq!(firefox["allowed_extensions"][0], FIREFOX_ID);
        assert_eq!(
            chromium_extension_id().unwrap(),
            "gmiknopfepcnfapmogmmemghmcfhdgcd"
        );
        assert_eq!(
            chromium["allowed_origins"][0],
            format!("chrome-extension://{}/", chromium_extension_id().unwrap())
        );
        assert_eq!(firefox["type"], "stdio");
    }

    #[test]
    fn native_host_resource_tree_matches_packaged_layout() {
        let resources = Path::new("/app/resources");
        assert_eq!(
            bundled_host_path(resources, "x86_64-unknown-linux-gnu", false),
            resources.join("tools/x86_64-unknown-linux-gnu/burrata-browser-host")
        );
        assert_eq!(
            bundled_host_path(resources, "x86_64-pc-windows-msvc", true),
            resources.join("tools/x86_64-pc-windows-msvc/burrata-browser-host.exe")
        );
    }

    #[test]
    fn native_messaging_directories_match_per_user_platform_locations() {
        let home = Path::new("/home/fixture");
        let config = Path::new("/config/fixture");
        let local = Path::new("/local/fixture");
        assert_eq!(
            Browser::Firefox.directory_for(home, config, local, Platform::Linux),
            home.join(".mozilla/native-messaging-hosts")
        );
        assert_eq!(
            Browser::Chrome.directory_for(home, config, local, Platform::Linux),
            config.join("google-chrome/NativeMessagingHosts")
        );
        assert_eq!(
            Browser::Edge.directory_for(home, config, local, Platform::Linux),
            config.join("microsoft-edge/NativeMessagingHosts")
        );
        assert_eq!(
            Browser::Firefox.directory_for(home, config, local, Platform::MacOs),
            home.join("Library/Application Support/Mozilla/NativeMessagingHosts")
        );
        assert_eq!(
            Browser::Edge.directory_for(home, config, local, Platform::MacOs),
            home.join("Library/Application Support/Microsoft Edge/NativeMessagingHosts")
        );
        assert_eq!(
            Browser::Chromium.directory_for(home, config, local, Platform::Windows),
            local.join("Burrata/NativeMessagingHosts/chromium")
        );
    }
}
