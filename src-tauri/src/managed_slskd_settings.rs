//! Narrow controls for the local, repository-managed slskd download policy.
//! This deliberately edits only known non-secret YAML scalar values.

use serde::{Deserialize, Serialize};
use std::fs;

use crate::slskd::{
    apply_managed_share_texts, is_managed_slskd_connection, managed_share_paths, MANAGED_SHARE_LOCK,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDownloadSettings {
    /// Total simultaneous downloads, controlled by the managed Compose setting.
    pub slots: u32,
    /// Global hard download limit in KiB/s. `None` means no explicit limit.
    pub speed_limit_kib: Option<u32>,
    pub retry_attempts: u32,
    pub retry_delay_ms: u32,
    pub retry_max_delay_ms: u32,
    /// `resume` or `overwrite`.
    pub partial_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDownloadSettingsState {
    pub managed: bool,
    pub can_edit: bool,
    pub settings: Option<ManagedDownloadSettings>,
    pub guidance: String,
}

impl Default for ManagedDownloadSettings {
    fn default() -> Self {
        Self {
            slots: 32,
            speed_limit_kib: None,
            retry_attempts: 3,
            retry_delay_ms: 5_000,
            retry_max_delay_ms: 60_000,
            partial_policy: "resume".into(),
        }
    }
}

pub fn state(base_url: &str, downloads_path: &str) -> ManagedDownloadSettingsState {
    let managed = is_managed_slskd_connection(base_url, downloads_path);
    if !managed {
        return ManagedDownloadSettingsState {
            managed: false,
            can_edit: false,
            settings: None,
            guidance:
                "Download settings are read-only for this separately configured slskd service."
                    .into(),
        };
    }
    let can_edit = crate::slskd::managed_share_controls_available();
    if !can_edit {
        return ManagedDownloadSettingsState {
            managed: true,
            can_edit: false,
            settings: None,
            guidance: "Managed download settings are read-only on this platform.".into(),
        };
    }
    match read_settings() {
        Ok(settings) => ManagedDownloadSettingsState {
            managed: true,
            can_edit,
            settings: Some(settings),
            guidance: "Applying changes restarts the managed slskd service; active transfers may be interrupted.".into(),
        },
        Err(error) => ManagedDownloadSettingsState {
            managed: true,
            can_edit: false,
            settings: None,
            guidance: format!("Managed download settings could not be read: {error}"),
        },
    }
}

pub fn update(
    base_url: &str,
    downloads_path: &str,
    settings: ManagedDownloadSettings,
) -> Result<ManagedDownloadSettingsState, String> {
    if !is_managed_slskd_connection(base_url, downloads_path) {
        return Err(
            "Download settings can only be changed for the managed local slskd service".into(),
        );
    }
    if !crate::slskd::managed_share_controls_available() {
        return Err("Managed slskd settings cannot be changed on this platform".into());
    }
    apply_download_settings(&managed_share_paths()?, settings)
}

// Keep the public connection/platform guards above. This shared core also lets
// the opt-in Docker qualification exercise the real file/restart transaction
// against a disposable deployment instead of touching a user's daemon.
fn apply_download_settings(
    paths: &crate::slskd::ManagedSharePaths,
    settings: ManagedDownloadSettings,
) -> Result<ManagedDownloadSettingsState, String> {
    validate(&settings)?;
    let _guard = MANAGED_SHARE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| "Managed slskd settings lock poisoned")?;
    let old_config = fs::read_to_string(&paths.config)
        .map_err(|e| format!("Cannot read managed slskd settings: {e}"))?;
    let old_compose = fs::read_to_string(&paths.compose)
        .map_err(|e| format!("Cannot read managed slskd deployment: {e}"))?;
    let new_config = patch_download_config(&old_config, &settings)?;
    let new_compose = patch_download_slots(&old_compose, settings.slots)?;
    apply_managed_share_texts(paths, &old_config, &old_compose, &new_config, &new_compose)?;
    Ok(ManagedDownloadSettingsState {
        managed: true,
        can_edit: true,
        settings: Some(settings),
        guidance: "Settings applied. The managed slskd service restarted; active transfers may have been interrupted.".into(),
    })
}

fn read_settings() -> Result<ManagedDownloadSettings, String> {
    let _guard = MANAGED_SHARE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| "Managed slskd settings lock poisoned")?;
    let paths = managed_share_paths()?;
    let config = fs::read_to_string(&paths.config)
        .map_err(|e| format!("Cannot read managed slskd settings: {e}"))?;
    let compose = fs::read_to_string(&paths.compose)
        .map_err(|e| format!("Cannot read managed slskd deployment: {e}"))?;
    read_download_settings(&config, &compose)
}

fn validate(settings: &ManagedDownloadSettings) -> Result<(), String> {
    if !(1..=500).contains(&settings.slots) {
        return Err("Download slots must be between 1 and 500".into());
    }
    if settings
        .speed_limit_kib
        .is_some_and(|value| !(1..=2_147_483_647).contains(&value))
    {
        return Err("Download speed limit must be a positive KiB/s value".into());
    }
    if !(1..=100).contains(&settings.retry_attempts) {
        return Err("Retry attempts must be between 1 and 100".into());
    }
    if settings.retry_delay_ms > 3_600_000 || settings.retry_max_delay_ms > 3_600_000 {
        return Err("Retry delays must be at most one hour".into());
    }
    if settings.retry_max_delay_ms < settings.retry_delay_ms {
        return Err("Maximum retry delay must be at least the initial delay".into());
    }
    if !matches!(settings.partial_policy.as_str(), "resume" | "overwrite") {
        return Err("Partial download policy must be resume or overwrite".into());
    }
    Ok(())
}

fn read_download_settings(config: &str, compose: &str) -> Result<ManagedDownloadSettings, String> {
    let mut settings = ManagedDownloadSettings::default();
    settings.slots = compose_slot_value(compose)?;
    settings.speed_limit_kib = yaml_path(config, &["transfers", "download", "speed_limit"])
        .filter(|value| !matches!(yaml_scalar(value).as_str(), "null" | "~"))
        .map(parse_u32)
        .transpose()?
        .filter(|value| *value != i32::MAX as u32);
    if let Some(value) = yaml_path(config, &["transfers", "download", "retry", "attempts"]) {
        settings.retry_attempts = parse_u32(value)?;
    }
    if let Some(value) = yaml_path(config, &["transfers", "download", "retry", "delay"]) {
        settings.retry_delay_ms = parse_u32(value)?;
    }
    if let Some(value) = yaml_path(config, &["transfers", "download", "retry", "max_delay"]) {
        settings.retry_max_delay_ms = parse_u32(value)?;
    }
    if let Some(value) = yaml_path(config, &["transfers", "download", "retry", "partial"]) {
        settings.partial_policy = yaml_scalar(value);
    }
    validate(&settings)?;
    Ok(settings)
}

fn parse_u32(value: &str) -> Result<u32, String> {
    yaml_scalar(value)
        .parse::<u32>()
        .map_err(|_| "Managed slskd download settings contain an invalid number".into())
}

fn yaml_scalar(value: &str) -> String {
    let value = value.split('#').next().unwrap_or("").trim();
    value.trim_matches(['"', '\'']).to_string()
}

fn yaml_path<'a>(text: &'a str, path: &[&str]) -> Option<&'a str> {
    let mut section = Vec::<(usize, &str)>::new();
    for line in text.lines() {
        // Shared directories and other unrelated YAML sequences are opaque.
        // They are not mapping keys and must survive scalar-control updates.
        if line.trim_start().starts_with('#')
            || line.trim().is_empty()
            || line.trim_start().starts_with("- ")
        {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let (key, value) = line.trim_start().split_once(':')?;
        while section.last().is_some_and(|(level, _)| *level >= indent) {
            section.pop();
        }
        let mut current = section.iter().map(|(_, key)| *key).collect::<Vec<_>>();
        current.push(key.trim());
        if current == path {
            let value = value.trim();
            return (!value.is_empty()).then_some(value);
        }
        if value.trim().is_empty() {
            section.push((indent, key.trim()));
        }
    }
    None
}

fn set_yaml_path(text: &str, path: &[&str], value: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("Invalid managed YAML path".into());
    }
    let lines = text.lines().collect::<Vec<_>>();
    let mut section = Vec::<(usize, &str, usize)>::new();
    for (index, line) in lines.iter().enumerate() {
        // Shared directories and other unrelated YAML sequences are opaque.
        // They are not mapping keys and must survive scalar-control updates.
        if line.trim_start().starts_with('#')
            || line.trim().is_empty()
            || line.trim_start().starts_with("- ")
        {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let (key, old_value) = line
            .trim_start()
            .split_once(':')
            .ok_or("Unexpected managed YAML structure")?;
        while section.last().is_some_and(|(level, _, _)| *level >= indent) {
            section.pop();
        }
        let mut current = section.iter().map(|(_, key, _)| *key).collect::<Vec<_>>();
        current.push(key.trim());
        if current == path {
            let prefix_len = line.find(':').ok_or("Unexpected managed YAML structure")? + 1;
            let suffix = old_value
                .find('#')
                .map(|offset| format!(" {}", old_value[offset..].trim()))
                .unwrap_or_default();
            let mut updated = lines
                .iter()
                .map(|line| (*line).to_string())
                .collect::<Vec<_>>();
            updated[index] = format!("{} {}{}", &line[..prefix_len], value, suffix);
            return Ok(join_lines(&updated, text.ends_with('\n')));
        }
        if old_value.trim().is_empty() {
            section.push((indent, key.trim(), index));
        }
    }
    // Only insert below the existing `transfers.download.retry` parent. This
    // keeps indentation and neighboring sections intact without serializing YAML.
    let parent = &path[..path.len() - 1];
    let Some(parent_index) = find_yaml_section(&lines, parent) else {
        if parent.len() > 1 {
            let ensured = set_yaml_path(text, parent, "")?;
            return set_yaml_path(&ensured, path, value);
        }
        return Err("Managed slskd YAML is missing the expected download settings section".into());
    };
    let indent = parent.len() * 2;
    let mut insert_at = lines.len();
    for (index, line) in lines.iter().enumerate().skip(parent_index + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let level = line.len() - line.trim_start().len();
        if level < indent {
            insert_at = index;
            break;
        }
    }
    let mut updated = lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    updated.insert(
        insert_at,
        format!("{}{}: {}", " ".repeat(indent), path[path.len() - 1], value),
    );
    Ok(join_lines(&updated, text.ends_with('\n')))
}

fn find_yaml_section(lines: &[&str], path: &[&str]) -> Option<usize> {
    let mut section = Vec::<(usize, &str)>::new();
    for (index, line) in lines.iter().enumerate() {
        // Shared directories and other unrelated YAML sequences are opaque.
        // They are not mapping keys and must survive scalar-control updates.
        if line.trim_start().starts_with('#')
            || line.trim().is_empty()
            || line.trim_start().starts_with("- ")
        {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let (key, value) = line.trim_start().split_once(':')?;
        while section.last().is_some_and(|(level, _)| *level >= indent) {
            section.pop();
        }
        let mut current = section.iter().map(|(_, key)| *key).collect::<Vec<_>>();
        current.push(key.trim());
        if current == path && value.trim().is_empty() {
            return Some(index);
        }
        if value.trim().is_empty() {
            section.push((indent, key.trim()));
        }
    }
    None
}

fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    let mut result = lines.join("\n");
    if trailing_newline {
        result.push('\n');
    }
    result
}

fn patch_download_config(text: &str, settings: &ManagedDownloadSettings) -> Result<String, String> {
    let mut result = text.to_string();
    for (path, value) in [
        (
            &["transfers", "download", "speed_limit"][..],
            settings
                .speed_limit_kib
                .map(|x| x.to_string())
                .unwrap_or_else(|| i32::MAX.to_string()),
        ),
        (
            &["transfers", "download", "retry", "attempts"][..],
            settings.retry_attempts.to_string(),
        ),
        (
            &["transfers", "download", "retry", "delay"][..],
            settings.retry_delay_ms.to_string(),
        ),
        (
            &["transfers", "download", "retry", "max_delay"][..],
            settings.retry_max_delay_ms.to_string(),
        ),
        (
            &["transfers", "download", "retry", "partial"][..],
            settings.partial_policy.clone(),
        ),
    ] {
        result = set_yaml_path(&result, path, &value)?;
    }
    Ok(result)
}

fn compose_slot_value(text: &str) -> Result<u32, String> {
    let lines = text.lines().collect::<Vec<_>>();
    let env = compose_slskd_environment(&lines)?;
    let values = env
        .filter(|index| lines[*index].trim().starts_with("SLSKD_DOWNLOAD_SLOTS:"))
        .filter_map(|index| {
            let (_, value) = lines[index].trim().split_once(':')?;
            Some(yaml_scalar(value))
        })
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err("Managed Compose file must define one slskd download slot value".into());
    }
    values[0]
        .parse()
        .map_err(|_| "Managed Compose download slot value is invalid".into())
}

fn patch_download_slots(text: &str, slots: u32) -> Result<String, String> {
    let lines = text.lines().collect::<Vec<_>>();
    let indices = compose_slskd_environment(&lines)?
        .filter_map(|index| {
            lines[index]
                .trim()
                .starts_with("SLSKD_DOWNLOAD_SLOTS:")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if indices.len() != 1 {
        return Err("Managed Compose file must define one slskd download slot setting".into());
    }
    let mut updated = lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    let line = lines[indices[0]];
    let colon = line
        .find(':')
        .ok_or("Invalid managed Compose download slot setting")?;
    let comment = line
        .find('#')
        .map(|index| format!(" {}", line[index..].trim()))
        .unwrap_or_default();
    updated[indices[0]] = format!("{}: \"{}\"{}", &line[..colon], slots, comment);
    Ok(join_lines(&updated, text.ends_with('\n')))
}

fn compose_slskd_environment<'a>(
    lines: &'a [&'a str],
) -> Result<impl Iterator<Item = usize> + 'a, String> {
    let service = lines
        .iter()
        .position(|line| line.trim() == "slskd:")
        .ok_or("Managed Compose file is missing its slskd service")?;
    let end = (service + 1..lines.len())
        .find(|index| {
            !lines[*index].trim().is_empty()
                && lines[*index].starts_with("  ")
                && !lines[*index].starts_with("    ")
        })
        .unwrap_or(lines.len());
    let env = (service + 1..end)
        .find(|index| lines[*index].trim() == "environment:")
        .ok_or("Managed slskd Compose service has no environment section")?;
    let end_env = (env + 1..end)
        .find(|index| !lines[*index].trim().is_empty() && !lines[*index].starts_with("      "))
        .unwrap_or(end);
    Ok(env + 1..end_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "soulseek:\n  username: private-user\n  password: private-password\ntransfers:\n  download:\n    destination:\n      subdirectory: ${BATCH_ID}\n";
    const COMPOSE: &str = "services:\n  slskd:\n    environment:\n      SLSKD_REMOTE_CONFIGURATION: \"false\"\n      SLSKD_DOWNLOAD_SLOTS: \"32\"\n";

    #[test]
    fn reads_defaults_and_patches_only_download_controls() {
        let defaults = read_download_settings(CONFIG, COMPOSE).unwrap();
        assert_eq!(defaults, ManagedDownloadSettings::default());
        let desired = ManagedDownloadSettings {
            slots: 18,
            speed_limit_kib: Some(2048),
            retry_attempts: 5,
            retry_delay_ms: 2500,
            retry_max_delay_ms: 30000,
            partial_policy: "overwrite".into(),
        };
        let config = patch_download_config(CONFIG, &desired).unwrap();
        let compose = patch_download_slots(COMPOSE, desired.slots).unwrap();
        assert!(config.contains("password: private-password"));
        assert!(!config.contains("SLSKD"));
        assert!(config.contains("    speed_limit: 2048"));
        assert!(config.contains("      partial: overwrite"));
        assert!(compose.contains("SLSKD_DOWNLOAD_SLOTS: \"18\""));
        assert_eq!(read_download_settings(&config, &compose).unwrap(), desired);
    }

    #[test]
    fn download_controls_preserve_shared_folder_sequences_before_transfers() {
        let config = format!("shares:\n  directories:\n    - /music/shared\n{CONFIG}");
        let desired = ManagedDownloadSettings {
            speed_limit_kib: Some(1024),
            ..Default::default()
        };
        let updated = patch_download_config(&config, &desired).unwrap();
        assert!(updated.starts_with("shares:\n  directories:\n    - /music/shared\n"));
        assert!(updated.contains("password: private-password"));
        assert_eq!(read_download_settings(&updated, COMPOSE).unwrap(), desired);
    }

    #[test]
    fn refuses_unknown_yaml_layout_and_out_of_range_values() {
        let mut invalid = ManagedDownloadSettings::default();
        invalid.slots = 0;
        assert!(validate(&invalid).is_err());
        assert!(patch_download_slots(
            "services:\n  other:\n    environment:\n      SLSKD_DOWNLOAD_SLOTS: 2\n",
            2
        )
        .is_err());
    }

    #[test]
    fn numeric_and_partial_values_are_narrow_scalar_replacements() {
        let existing = "transfers:\n  download:\n    retry:\n      attempts: 3 # attempts\n      delay: 5000\n      max_delay: 60000\n      partial: resume\n";
        let value = ManagedDownloadSettings {
            slots: 32,
            speed_limit_kib: None,
            retry_attempts: 8,
            retry_delay_ms: 4000,
            retry_max_delay_ms: 50000,
            partial_policy: "resume".into(),
        };
        let updated = patch_download_config(existing, &value).unwrap();
        assert!(updated.contains("attempts: 8 # attempts"));
        assert!(updated.contains("speed_limit: 2147483647"));
    }
    /// Opt in with `cargo test managed_download_settings_disposable_daemon -- --ignored`.
    /// Uses only a cached image, fresh private data and a network-disabled container.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires Docker Compose and the cached pinned slskd image"]
    fn managed_download_settings_disposable_daemon() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        struct Fixture {
            paths: crate::slskd::ManagedSharePaths,
            _directory: tempfile::TempDir,
            api_key: String,
        }
        impl Fixture {
            fn compose(&self, args: &[&str]) -> std::process::Output {
                Command::new("docker")
                    .args(["compose", "-f"])
                    .arg(&self.paths.compose)
                    .args(args)
                    .current_dir(&self.paths.root)
                    .stdin(Stdio::null())
                    .output()
                    .expect("Docker Compose must be installed for this opt-in test")
            }
            fn container_id(&self) -> String {
                let output = self.compose(&["ps", "-q", "slskd"]);
                assert!(output.status.success());
                let id = String::from_utf8(output.stdout).unwrap().trim().to_string();
                assert!(!id.is_empty(), "Disposable daemon must be running");
                id
            }
            fn options(&self) -> serde_json::Value {
                let header = format!("--header=X-API-Key: {}", self.api_key);
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let response = self.compose(&[
                        "exec",
                        "-T",
                        "slskd",
                        "wget",
                        "-qO-",
                        "--timeout=1",
                        &header,
                        "http://127.0.0.1:5030/api/v0/options",
                    ]);
                    if response.status.success() {
                        if let Ok(options) = serde_json::from_slice(&response.stdout) {
                            return options;
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "Disposable daemon options API did not become ready"
                    );
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            fn assert_effective(&self, expected: &ManagedDownloadSettings) {
                let options = self.options();
                let download = &options["transfers"]["download"];
                assert_eq!(download["slots"], expected.slots);
                assert_eq!(
                    download["speedLimit"],
                    expected.speed_limit_kib.unwrap_or(i32::MAX as u32)
                );
                assert_eq!(download["retry"]["attempts"], expected.retry_attempts);
                assert_eq!(download["retry"]["delay"], expected.retry_delay_ms);
                assert_eq!(download["retry"]["maxDelay"], expected.retry_max_delay_ms);
                assert_eq!(download["retry"]["partial"], expected.partial_policy);
                assert_eq!(options["flags"]["noConnect"], true);
                assert_eq!(options["flags"]["noShareScan"], true);
                assert_eq!(options["flags"]["noVersionCheck"], true);
                assert_eq!(options["flags"]["noConfigWatch"], true);
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                // Only this randomly named project and its empty private mount.
                let _ = self.compose(&["down", "--timeout", "5"]);
            }
        }

        let directory = tempfile::Builder::new()
            .prefix("burrata-slskd-settings-")
            .tempdir()
            .unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = directory.path().metadata().unwrap();
        let data = directory.path().join("data");
        fs::create_dir(&data).unwrap();
        let paths = crate::slskd::ManagedSharePaths {
            root: directory.path().to_path_buf(),
            compose: directory.path().join("compose.yaml"),
            config: data.join("slskd.yml"),
        };
        let api_key = uuid::Uuid::new_v4().simple().to_string();
        let project = format!("burrata-settings-{}", uuid::Uuid::new_v4().simple());
        fs::write(&paths.config, format!("web:\n  authentication:\n    api_keys:\n      qualification:\n        key: {api_key}\n        role: Administrator\ntransfers:\n  download:\n    speed_limit: 2147483647\n")).unwrap();
        fs::set_permissions(&paths.config, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(
            &paths.compose,
            format!(
                r#"name: {project}
services:
  slskd:
    image: slskd/slskd@sha256:588af49abc3088d639c8f1174d3bfad6e92c093a275a593140e3ac9311ebf6c4
    pull_policy: never
    user: "{}:{}"
    network_mode: none
    volumes:
      - ./data:/app
    environment:
      SLSKD_DOWNLOAD_SLOTS: "32"
      SLSKD_NO_CONNECT: "true"
      SLSKD_NO_SHARE_SCAN: "true"
      SLSKD_NO_VERSION_CHECK: "true"
      SLSKD_NO_CONFIG_WATCH: "true"
      SLSKD_NO_HTTPS: "true"
"#,
                metadata.uid(),
                metadata.gid()
            ),
        )
        .unwrap();
        let fixture = Fixture {
            paths,
            _directory: directory,
            api_key,
        };
        let defaults = ManagedDownloadSettings::default();
        apply_download_settings(&fixture.paths, defaults.clone()).unwrap();
        fixture.assert_effective(&defaults);
        let initial = fixture.container_id();
        let desired = ManagedDownloadSettings {
            slots: 17,
            speed_limit_kib: Some(2048),
            retry_attempts: 7,
            retry_delay_ms: 9000,
            retry_max_delay_ms: 36000,
            partial_policy: "overwrite".into(),
        };
        let result = apply_download_settings(&fixture.paths, desired.clone()).unwrap();
        assert_eq!(result.settings, Some(desired.clone()));
        fixture.assert_effective(&desired);
        let changed = fixture.container_id();
        assert_ne!(
            initial, changed,
            "Apply must recreate the disposable daemon"
        );
        apply_download_settings(&fixture.paths, defaults.clone()).unwrap();
        fixture.assert_effective(&defaults);
        assert_ne!(
            changed,
            fixture.container_id(),
            "Restoring defaults must recreate the daemon"
        );
        assert!(fixture
            .compose(&["down", "--timeout", "5"])
            .status
            .success());
        assert!(fixture.compose(&["ps", "-q", "slskd"]).stdout.is_empty());
    }
}
