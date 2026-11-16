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

pub(crate) fn chromium_extension_id() -> Result<String, String> {
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
    let source = bundled_host_path(resource_dir, crate::tools::target_triple(), cfg!(windows));
    let target = managed_host_path(&source, &default_host_path(), cfg!(windows))?;
    copy_host_if_changed(&source, &target)?;
    Ok(target)
}

fn default_host_path() -> PathBuf {
    let name = if cfg!(windows) {
        "burrata-browser-host.exe"
    } else {
        "burrata-browser-host"
    };
    data_home()
        .join("org.burrata.discovery/integrations")
        .join(name)
}

fn managed_host_path(source: &Path, fixed_path: &Path, windows: bool) -> Result<PathBuf, String> {
    if !windows {
        return Ok(fixed_path.to_path_buf());
    }
    let parent = fixed_path
        .parent()
        .ok_or("Browser host directory unavailable")?;
    let stem = fixed_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("Browser host filename unavailable")?;
    let extension = fixed_path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or("Browser host extension unavailable")?;
    let digest = crate::media::hash_file(source)?;
    Ok(parent.join(format!("{stem}-{digest}.{extension}")))
}

fn is_managed_host_path(path: &Path, fixed_path: &Path, windows: bool) -> bool {
    if path == fixed_path {
        return true;
    }
    if !windows || path.parent() != fixed_path.parent() {
        return false;
    }
    let Some(stem) = fixed_path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(extension) = fixed_path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(digest) = filename
        .strip_prefix(&format!("{stem}-"))
        .and_then(|name| name.strip_suffix(&format!(".{extension}")))
    else {
        return false;
    };
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn copy_host_if_changed(source: &Path, target: &Path) -> Result<bool, String> {
    if !source.is_file() {
        return Err(format!(
            "Bundled browser host is missing: {}",
            source.display()
        ));
    }
    if source == target
        || (target.is_file()
            && crate::media::hash_file(source)? == crate::media::hash_file(target)?)
    {
        return Ok(false);
    }
    {
        let parent = target
            .parent()
            .ok_or("Browser host directory unavailable")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        fs::copy(source, tmp.path()).map_err(|e| format!("Could not install browser host: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        tmp.as_file().sync_all().map_err(|e| e.to_string())?;
        // persist replaces atomically on Unix and Windows, retaining the old
        // executable if publication fails (for example an occupied Windows file).
        tmp.persist(target).map_err(|e| e.error.to_string())?;
    }
    Ok(true)
}

fn refresh_registered_host_from_paths(
    source: &Path,
    fixed_path: &Path,
    manifests: &[PathBuf],
    windows: bool,
) -> Result<bool, String> {
    let target = managed_host_path(source, fixed_path, windows)?;
    if fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Ok(false);
    }
    let mut owned_manifests = Vec::new();
    for path in manifests {
        if !fs::metadata(path).is_ok_and(|m| m.len() <= 65536) {
            continue;
        }
        let Some(mut value) = fs::read(path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        else {
            continue;
        };
        let registered_path = value["path"].as_str().map(PathBuf::from);
        if value["name"] == HOST
            && value["type"] == "stdio"
            && registered_path
                .as_deref()
                .is_some_and(|path| is_managed_host_path(path, fixed_path, windows))
        {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "path".into(),
                    serde_json::Value::String(target.to_string_lossy().into_owned()),
                );
            }
            let path_changed = registered_path.as_deref() != Some(target.as_path());
            owned_manifests.push((path, value, path_changed));
        }
    }
    if owned_manifests.is_empty() {
        return Ok(false);
    }
    let mut changed = copy_host_if_changed(source, &target)?;
    for (path, value, path_changed) in owned_manifests {
        if path_changed {
            let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
            write_manifest(path, &bytes)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Update only the standard helper for existing registrations. Custom paths,
/// manifests, browser pairing and unregistered browsers remain user-owned.
pub(crate) fn refresh_registered_host(resource_dir: &Path) -> Result<bool, String> {
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    let manifests =
        Browser::all().map(|browser| browser.directory(&home).join(format!("{HOST}.json")));
    refresh_registered_host_from_paths(
        &bundled_host_path(resource_dir, crate::tools::target_triple(), cfg!(windows)),
        &default_host_path(),
        &manifests,
        cfg!(windows),
    )
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
    let system_root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let mut cmd = Command::new(system_root.join("System32/reg.exe"));
    crate::tools::configure_no_console(&mut cmd);
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
    use std::io::Write;
    let parent = target
        .parent()
        .ok_or("Browser manifest directory unavailable")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    tmp.write_all(bytes).map_err(|e| e.to_string())?;
    tmp.as_file().sync_all().map_err(|e| e.to_string())?;
    tmp.persist(target).map_err(|e| e.error.to_string())?;
    Ok(())
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
    let candidates =
        mcp_launcher_candidates(&current, crate::tools::target_triple(), cfg!(windows));
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

fn mcp_launcher_candidates(current_exe: &Path, target: &str, windows: bool) -> Vec<PathBuf> {
    let binary = if windows {
        "burrata-mcp.exe"
    } else {
        "burrata-mcp"
    };
    let mut candidates = Vec::new();
    for ancestor in current_exe.ancestors() {
        for root in [
            "tools",
            "Resources/tools",
            "resources/tools",
            "Resources/resources/tools",
            "resources/resources/tools",
        ] {
            candidates.push(ancestor.join(root).join(target).join(binary));
        }
        // Debian and RPM packages install Tauri resources beside the executable
        // under /usr/lib/<app>/resources or /usr/share/<app>/resources.
        for root in [
            "../lib/Burrata/resources/tools",
            "../share/Burrata/resources/tools",
            "../lib/Burrata/tools",
            "../share/Burrata/tools",
            "../lib/burrata/resources/tools",
            "../share/burrata/resources/tools",
            "../lib/burrata/tools",
            "../share/burrata/tools",
        ] {
            candidates.push(ancestor.join(root).join(target).join(binary));
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_helper_refresh_preserves_manifests_and_skips_identical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled-host");
        let target = dir.path().join("installed-host");
        let manifest_path = dir.path().join("host.json");
        fs::write(&source, b"new protocol").unwrap();
        fs::write(&target, b"old protocol").unwrap();
        let manifest_bytes =
            serde_json::to_vec(&manifest(Browser::Firefox, &target).unwrap()).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        assert!(refresh_registered_host_from_paths(
            &source,
            &target,
            std::slice::from_ref(&manifest_path),
            false,
        )
        .unwrap());
        assert_eq!(fs::read(&target).unwrap(), b"new protocol");
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        assert!(!refresh_registered_host_from_paths(
            &source,
            &target,
            std::slice::from_ref(&manifest_path),
            false,
        )
        .unwrap());
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
        fs::remove_file(&target).unwrap();
        assert!(
            refresh_registered_host_from_paths(&source, &target, &[manifest_path], false).unwrap()
        );
    }

    #[test]
    fn refresh_does_not_register_browsers_or_replace_custom_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled-host");
        let target = dir.path().join("standard-host");
        let custom = dir.path().join("custom-host");
        let manifest_path = dir.path().join("host.json");
        fs::write(&source, b"new").unwrap();
        fs::write(&custom, b"custom").unwrap();
        assert!(!refresh_registered_host_from_paths(&source, &target, &[], false).unwrap());
        assert!(!target.exists());
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest(Browser::Firefox, &custom).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            !refresh_registered_host_from_paths(&source, &target, &[manifest_path], false).unwrap()
        );
        assert!(!target.exists());
        assert_eq!(fs::read(custom).unwrap(), b"custom");
    }

    #[test]
    fn failed_helper_refresh_keeps_the_existing_executable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("installed-host");
        fs::write(&target, b"old working host").unwrap();
        assert!(copy_host_if_changed(&dir.path().join("missing-bundle"), &target).is_err());
        assert_eq!(fs::read(target).unwrap(), b"old working host");
    }

    #[test]
    fn windows_refresh_publishes_immutable_helper_and_preserves_owned_manifest_fields() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled-host.exe");
        let old_source = dir.path().join("previous-host.exe");
        let fixed_path = dir.path().join("burrata-browser-host.exe");
        let manifest_path = dir.path().join("host.json");
        fs::write(&source, b"new protocol").unwrap();
        fs::write(&old_source, b"old protocol").unwrap();
        fs::write(&fixed_path, b"legacy locked executable").unwrap();
        let old_managed_path = managed_host_path(&old_source, &fixed_path, true).unwrap();
        fs::write(&old_managed_path, b"old protocol").unwrap();
        let new_managed_path = managed_host_path(&source, &fixed_path, true).unwrap();
        assert_ne!(old_managed_path, new_managed_path);
        let mut owned = manifest(Browser::Chrome, &old_managed_path).unwrap();
        owned["allowed_origins"] = serde_json::json!([
            "chrome-extension://existing-pairing/",
            "chrome-extension://additional-pairing/"
        ]);
        owned["customInstallerData"] = serde_json::json!({"keep": true});
        let original_origins = owned["allowed_origins"].clone();
        let original_custom = owned["customInstallerData"].clone();
        fs::write(&manifest_path, serde_json::to_vec_pretty(&owned).unwrap()).unwrap();

        assert!(refresh_registered_host_from_paths(
            &source,
            &fixed_path,
            std::slice::from_ref(&manifest_path),
            true,
        )
        .unwrap());
        assert_eq!(fs::read(&fixed_path).unwrap(), b"legacy locked executable");
        assert_eq!(fs::read(&old_managed_path).unwrap(), b"old protocol");
        assert_eq!(fs::read(&new_managed_path).unwrap(), b"new protocol");
        let refreshed: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            refreshed["path"],
            new_managed_path.to_string_lossy().as_ref()
        );
        assert_eq!(refreshed["allowed_origins"], original_origins);
        assert_eq!(refreshed["customInstallerData"], original_custom);
        assert!(
            !refresh_registered_host_from_paths(&source, &fixed_path, &[manifest_path], true,)
                .unwrap()
        );
    }

    #[test]
    fn windows_refresh_leaves_custom_helper_paths_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled-host.exe");
        let fixed_path = dir.path().join("burrata-browser-host.exe");
        let custom_path = dir.path().join("custom-browser-host.exe");
        let manifest_path = dir.path().join("host.json");
        fs::write(&source, b"new protocol").unwrap();
        fs::write(&custom_path, b"custom helper").unwrap();
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest(Browser::Chrome, &custom_path).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            !refresh_registered_host_from_paths(&source, &fixed_path, &[manifest_path], true,)
                .unwrap()
        );
        assert!(!fixed_path.exists());
        assert_eq!(fs::read(custom_path).unwrap(), b"custom helper");
    }

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
    fn mcp_launcher_resolves_inside_the_packaged_resource_tree() {
        let exe = Path::new("/opt/burrata/resources/tools/x86_64-unknown-linux-gnu/burrata-mcp");
        let candidates = mcp_launcher_candidates(exe, "x86_64-unknown-linux-gnu", false);
        assert!(candidates.contains(&exe.to_path_buf()));
        let windows_exe = Path::new(
            "C:/Program Files/Burrata/resources/tools/x86_64-pc-windows-msvc/burrata.exe",
        );
        let candidates = mcp_launcher_candidates(windows_exe, "x86_64-pc-windows-msvc", true);
        assert!(candidates
            .iter()
            .any(|path| path.ends_with("burrata-mcp.exe")));
    }

    #[test]
    fn main_mcp_resolves_extracted_linux_and_desktop_resource_layouts() {
        for (relative, executable, target, windows) in [
            (
                "usr/lib/Burrata/resources",
                "usr/bin/burrata",
                "x86_64-unknown-linux-gnu",
                false,
            ),
            (
                "Burrata.app/Contents/Resources",
                "Burrata.app/Contents/MacOS/burrata",
                "aarch64-apple-darwin",
                false,
            ),
            (
                "Burrata.app/Contents/Resources/resources",
                "Burrata.app/Contents/MacOS/burrata",
                "aarch64-apple-darwin",
                false,
            ),
            (
                "Program Files/Burrata/resources",
                "Program Files/Burrata/burrata.exe",
                "x86_64-pc-windows-msvc",
                true,
            ),
            (
                "Program Files/Burrata/resources/resources",
                "Program Files/Burrata/burrata.exe",
                "x86_64-pc-windows-msvc",
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let helper = directory
                .path()
                .join(relative)
                .join("tools")
                .join(target)
                .join(if windows {
                    "burrata-mcp.exe"
                } else {
                    "burrata-mcp"
                });
            fs::create_dir_all(helper.parent().unwrap()).unwrap();
            fs::write(&helper, "fixture").unwrap();
            let executable = directory.path().join(executable);
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            let selected = mcp_launcher_candidates(&executable, target, windows)
                .into_iter()
                .find(|candidate| candidate.is_file())
                .unwrap();
            assert_eq!(
                selected.canonicalize().unwrap(),
                helper.canonicalize().unwrap()
            );
        }
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
