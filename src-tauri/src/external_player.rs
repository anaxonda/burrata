//! Optional integration with an already installed mpv executable.
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use url::Url;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MpvStatus {
    pub available: bool,
}

/// Reports whether mpv can be found without exposing its local install path.
pub fn status() -> MpvStatus {
    MpvStatus {
        available: find_mpv().is_some(),
    }
}

/// Open a local file or an HTTP(S) stream in an installed mpv.
/// `video` selects video playback; false requests audio-only playback.
pub fn open_in_mpv(
    source: &str,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    video: bool,
    yt_dlp_configured: &str,
) -> Result<(), String> {
    open_in_mpv_with_provider_args(
        source,
        start_seconds,
        end_seconds,
        video,
        yt_dlp_configured,
        &[],
    )
}

/// Open media in mpv, supplying Burrata's managed YouTube runtime/provider
/// configuration when the selected URL is YouTube and yt-dlp is managed.
/// System/custom yt-dlp selections are left under the user's control.
pub async fn open_in_mpv_with_provider_support(
    source: &str,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    video: bool,
    yt_dlp_configured: &str,
) -> Result<(), String> {
    let source_kind = validate_source(source)?;
    let managed = is_managed_ytdlp(yt_dlp_configured);
    let provider_args = if managed
        && matches!(source_kind, ValidSource::ProviderUrl(_))
        && is_youtube_url(source)?
        && crate::youtube_support::global().is_some()
    {
        crate::youtube_support::yt_dlp_provider_args().await?
    } else {
        Vec::new()
    };
    open_in_mpv_with_provider_args(
        source,
        start_seconds,
        end_seconds,
        video,
        yt_dlp_configured,
        &provider_args,
    )
}

fn open_in_mpv_with_provider_args(
    source: &str,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    video: bool,
    yt_dlp_configured: &str,
    provider_args: &[std::ffi::OsString],
) -> Result<(), String> {
    let executable = find_mpv().ok_or_else(|| {
        "mpv is not installed or was not found. Install mpv or add it to PATH, then restart Burrata.".to_owned()
    })?;
    let ytdlp = if is_supported_provider_url(source)? {
        resolve_ytdlp(yt_dlp_configured)
    } else {
        None
    };
    let args = build_args(
        source,
        start_seconds,
        end_seconds,
        video,
        ytdlp.as_deref(),
        provider_args,
    )?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start mpv: {error}"))
}

fn is_managed_ytdlp(configured: &str) -> bool {
    let configured = configured.trim();
    configured.is_empty() || configured == "yt-dlp" || configured.starts_with("bundled:")
}

fn resolve_ytdlp(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    let managed =
        configured.is_empty() || configured == "yt-dlp" || configured.starts_with("bundled:");
    let path = if managed && crate::youtube_support::global().is_some() {
        crate::youtube_support::active_executable()
    } else {
        crate::tools::executable(configured, "yt-dlp")
    };
    path.is_file().then_some(path)
}

fn build_args(
    source: &str,
    start_seconds: Option<f64>,
    end_seconds: Option<f64>,
    video: bool,
    yt_dlp: Option<&Path>,
    provider_args: &[std::ffi::OsString],
) -> Result<Vec<String>, String> {
    let source = validate_source(source)?;
    validate_range(start_seconds, end_seconds)?;
    let mut args = vec!["--force-window=yes".to_owned()];
    if !video {
        args.push("--vid=no".into());
        if matches!(source, ValidSource::ProviderUrl(_)) {
            args.push("--ytdl-format=bestaudio/best".into());
        }
    }
    if let Some(start) = start_seconds {
        args.push(format!("--start={start}"));
    }
    if let Some(end) = end_seconds {
        args.push(format!("--end={end}"));
    }
    if let (ValidSource::ProviderUrl(_), Some(ytdl)) = (&source, yt_dlp) {
        // Use the single-option form so commas in user paths are not parsed as
        // delimiters in mpv's list-valued --script-opts syntax.
        args.push(format!(
            "--script-opt=ytdl_hook-ytdl_path={}",
            ytdl.display()
        ));
        if is_youtube_url(&source.as_argument())? && !provider_args.is_empty() {
            args.push(build_ytdl_raw_options(provider_args)?);
        }
    }
    args.push("--".into());
    args.push(source.as_argument());
    Ok(args)
}

fn build_ytdl_raw_options(provider_args: &[std::ffi::OsString]) -> Result<String, String> {
    let mut entries = Vec::new();
    let mut index = 0;
    while index < provider_args.len() {
        let flag = provider_args[index].to_str().ok_or_else(|| {
            "Managed YouTube provider arguments contain a non-Unicode path".to_owned()
        })?;
        let key = match flag {
            "--plugin-dirs" => "plugin-dirs",
            "--js-runtimes" => "js-runtimes",
            "--extractor-args" => "extractor-args",
            _ => {
                return Err("Managed YouTube provider returned an unsupported yt-dlp option".into())
            }
        };
        let value = provider_args
            .get(index + 1)
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                "Managed YouTube provider returned an incomplete argument pair".to_owned()
            })?;
        entries.push(format!("{key}={}", mpv_quote_suboption_value(value)));
        index += 2;
    }
    if entries.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("--ytdl-raw-options={}", entries.join(",")))
    }
}

/// mpv's documented fixed-length quoting keeps commas, colons, percent signs,
/// and spaces inside a value from being interpreted by its suboption parser.
fn mpv_quote_suboption_value(value: &str) -> String {
    format!("%{}%{value}", value.len())
}

fn is_youtube_url(source: &str) -> Result<bool, String> {
    let url = validate_http_url(source)?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    Ok(host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com"))
}

#[derive(Debug, PartialEq, Eq)]
enum ValidSource {
    LocalFile(PathBuf),
    ProviderUrl(String),
    DirectUrl(String),
}

impl ValidSource {
    fn as_argument(&self) -> String {
        match self {
            Self::LocalFile(path) => path.to_string_lossy().into_owned(),
            Self::ProviderUrl(url) => url.clone(),
            Self::DirectUrl(url) => url.clone(),
        }
    }
}

fn validate_source(source: &str) -> Result<ValidSource, String> {
    if source.trim().is_empty() || source.contains('\0') {
        return Err("Choose a valid local file or HTTP(S) media URL for mpv".into());
    }
    let path = Path::new(source);
    if path.is_file() {
        return path
            .canonicalize()
            .map(ValidSource::LocalFile)
            .map_err(|_| "The selected local media file is unavailable".into());
    }
    if is_supported_provider_url(source)? {
        return Ok(ValidSource::ProviderUrl(source.to_owned()));
    }
    validate_http_url(source)?;
    Ok(ValidSource::DirectUrl(source.to_owned()))
}

fn is_supported_provider_url(source: &str) -> Result<bool, String> {
    let url = validate_http_url(source)?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let youtube = host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com");
    let soundcloud = host == "soundcloud.com" || host.ends_with(".soundcloud.com");
    Ok(youtube || soundcloud)
}

fn validate_http_url(source: &str) -> Result<Url, String> {
    let url = Url::parse(source)
        .map_err(|_| "Choose a valid local file or HTTP(S) media URL for mpv".to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("mpv accepts only HTTP(S) media URLs without embedded credentials".into());
    }
    Ok(url)
}

fn validate_range(start: Option<f64>, end: Option<f64>) -> Result<(), String> {
    if start.is_some_and(|value| !value.is_finite() || value < 0.0)
        || end.is_some_and(|value| !value.is_finite() || value < 0.0)
        || matches!((start, end), (Some(start), Some(end)) if end <= start)
    {
        return Err(
            "Playback range must contain finite nonnegative times with end after start".into(),
        );
    }
    Ok(())
}

fn find_mpv() -> Option<PathBuf> {
    #[allow(unused_mut)]
    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        directories.extend([
            PathBuf::from("/Applications/mpv.app/Contents/MacOS"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            directories.push(PathBuf::from(home).join("Applications/mpv.app/Contents/MacOS"));
        }
    }
    #[cfg(windows)]
    {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            directories.push(PathBuf::from(local).join("Programs/mpv"));
        }
        for key in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(root) = std::env::var_os(key) {
                directories.push(PathBuf::from(root).join("mpv"));
            }
        }
        if let Some(home) = home {
            directories.push(home.join("scoop/apps/mpv/current"));
        }
    }
    find_mpv_in(&directories)
}

fn find_mpv_in(directories: &[PathBuf]) -> Option<PathBuf> {
    for directory in directories {
        #[cfg(windows)]
        let candidate = directory.join("mpv.exe");
        #[cfg(not(windows))]
        let candidate = directory.join("mpv");
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&candidate)
                    .ok()
                    .is_some_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                {
                    return Some(candidate);
                }
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_file_args_keep_path_after_end_of_options_and_apply_range() {
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("--audio file.flac");
        std::fs::write(&media, b"fixture").unwrap();
        let args = build_args(
            &media.to_string_lossy(),
            Some(12.5),
            Some(20.0),
            false,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(args[0], "--force-window=yes");
        assert_eq!(args[1], "--vid=no");
        assert_eq!(args[2], "--start=12.5");
        assert_eq!(args[3], "--end=20");
        assert_eq!(args[4], "--");
        assert_eq!(args[5], media.canonicalize().unwrap().to_string_lossy());
    }

    #[test]
    fn provider_args_use_exact_url_and_configured_ytdlp_without_a_shell() {
        let ytdlp = Path::new("/custom/tools/yt,dlp");
        let url = "https://www.youtube.com/watch?v=a&list=b";
        let args = build_args(url, None, None, true, Some(ytdlp), &[]).unwrap();
        assert_eq!(args[0], "--force-window=yes");
        assert_eq!(
            args[1],
            "--script-opt=ytdl_hook-ytdl_path=/custom/tools/yt,dlp"
        );
        assert_eq!(args[2], "--");
        assert_eq!(args[3], url);

        let audio = build_args(url, None, None, false, None, &[]).unwrap();
        assert!(audio.contains(&"--vid=no".into()));
        assert!(audio.contains(&"--ytdl-format=bestaudio/best".into()));
    }

    #[test]
    fn provider_url_allowlist_rejects_lookalikes_and_credentials() {
        assert!(is_supported_provider_url("https://youtu.be/abc").unwrap());
        assert!(is_supported_provider_url("https://on.soundcloud.com/abc").unwrap());
        assert!(!is_supported_provider_url("https://youtube.com.attacker.example/video").unwrap());
        assert!(is_supported_provider_url("https://user:pass@youtube.com/watch?v=x").is_err());
        assert!(is_supported_provider_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn direct_radio_stream_urls_are_allowed_without_enabling_ytdlp() {
        let url = "http://192.168.1.20:8000/live.mp3?mount=main";
        let args = build_args(
            url,
            None,
            None,
            false,
            Some(Path::new("/usr/bin/yt-dlp")),
            &[],
        )
        .unwrap();
        assert_eq!(args[0], "--force-window=yes");
        assert_eq!(args[1], "--vid=no");
        assert_eq!(args[2], "--");
        assert_eq!(args[3], url);
        assert!(!args.iter().any(|arg| arg.starts_with("--script-opt=")));
        assert!(!args.iter().any(|arg| arg.starts_with("--ytdl-format=")));
    }

    #[test]
    fn managed_provider_args_become_escaped_mpv_raw_options() {
        let options = build_ytdl_raw_options(&[
            "--plugin-dirs".into(),
            "/opt/Burrata Plug-ins, safe".into(),
            "--js-runtimes".into(),
            "node:/opt/Burrata Runtime/node".into(),
            "--extractor-args".into(),
            "youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416".into(),
        ])
        .unwrap();
        assert_eq!(
            options,
            "--ytdl-raw-options=plugin-dirs=%27%/opt/Burrata Plug-ins, safe,js-runtimes=%30%node:/opt/Burrata Runtime/node,extractor-args=%52%youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416"
        );
    }

    #[test]
    fn provider_raw_options_only_attach_to_youtube_urls() {
        let provider_args: Vec<std::ffi::OsString> = vec![
            "--plugin-dirs".into(),
            "/trusted/plugins".into(),
            "--js-runtimes".into(),
            "node:/trusted/node".into(),
        ];
        let youtube = build_args(
            "https://youtu.be/abcdefghijk",
            None,
            None,
            true,
            Some(Path::new("/bundled/yt-dlp")),
            &provider_args,
        )
        .unwrap();
        assert!(youtube
            .iter()
            .any(|arg| arg.starts_with("--ytdl-raw-options=")));

        let soundcloud = build_args(
            "https://soundcloud.com/artist/track",
            None,
            None,
            true,
            Some(Path::new("/bundled/yt-dlp")),
            &provider_args,
        )
        .unwrap();
        assert!(!soundcloud
            .iter()
            .any(|arg| arg.starts_with("--ytdl-raw-options=")));
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        assert!(validate_range(Some(-1.0), None).is_err());
        assert!(validate_range(Some(f64::NAN), None).is_err());
        assert!(validate_range(Some(12.0), Some(12.0)).is_err());
        assert!(validate_range(Some(12.0), Some(14.0)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn mpv_search_accepts_executable_from_an_explicit_directory() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mpv");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            find_mpv_in(&[directory.path().to_path_buf()]),
            Some(executable)
        );
    }
}
