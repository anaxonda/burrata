//! Find packaged or installed tools without requiring GUI launchers to inherit a shell PATH.
use std::path::PathBuf;
use std::sync::OnceLock;

/// Hide console-subsystem helper processes when launched by the Windows GUI.
/// Keep this scoped to Burrata-owned child processes; the optional external
/// player is configured separately because its visibility is user-facing.
pub fn configure_no_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

pub fn configure_no_console_tokio(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

static RESOURCE_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn initialize(resource_dir: PathBuf) {
    let _ = RESOURCE_ROOT.set(resource_dir);
}
pub fn target_triple() -> &'static str {
    env!("BURRATA_TARGET")
}
pub fn bundled_executable(name: &str) -> Option<PathBuf> {
    let name = if matches!(name, "songrec" | "SongRec") {
        "burrata-recognize"
    } else {
        name
    };
    let name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let mut roots = Vec::new();
    if let Some(root) = RESOURCE_ROOT.get() {
        roots.push(root.clone());
        roots.push(root.join("resources"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            roots.push(parent.to_owned());
            roots.push(parent.join("../Resources"));
        }
    }
    roots
        .into_iter()
        .flat_map(|root| {
            [
                root.join("tools").join(target_triple()).join(&name),
                root.join("tools").join(&name),
            ]
        })
        .find(|path| path.is_file())
}

pub fn executable(configured: &str, default_name: &str) -> PathBuf {
    let configured = configured.trim();
    if let Some(name) = configured.strip_prefix("bundled:") {
        return bundled_executable(name)
            .unwrap_or_else(|| PathBuf::from(format!("burrata-missing-bundled-{name}")));
    }
    let system_only = configured.starts_with("system:");
    let configured = configured.strip_prefix("system:").unwrap_or(configured);
    let configured = if configured.is_empty() {
        default_name
    } else {
        configured
    };
    if let Some(relative) = configured.strip_prefix("~/") {
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            return PathBuf::from(home).join(relative);
        }
    }
    let requested = PathBuf::from(configured);
    if requested.components().count() > 1 || requested.is_absolute() {
        return requested;
    }
    if !system_only {
        if let Some(path) = bundled_executable(configured) {
            return path;
        }
    }
    let name = if cfg!(windows) && requested.extension().is_none() {
        format!("{configured}.exe")
    } else {
        configured.to_owned()
    };
    let mut directories = Vec::new();
    if !system_only {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                directories.push(parent.join("tools"));
                directories.push(parent.join("../Resources/tools"));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    if cfg!(target_os = "macos") {
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        directories.push(home.join(".local/bin"));
        directories.push(home.join(".deno/bin"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(&name))
        .find(|path| path.is_file())
        .unwrap_or(requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_tool_path_is_preserved() {
        assert_eq!(
            executable("/custom/tools/ffmpeg", "ffmpeg"),
            PathBuf::from("/custom/tools/ffmpeg")
        );
    }
    #[test]
    fn missing_tools_retain_a_useful_command_name() {
        assert_eq!(
            executable("", "burrata-absent-tool-8d061"),
            PathBuf::from("burrata-absent-tool-8d061")
        );
    }
}
