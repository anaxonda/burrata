//! Opt-in, path-redacted diagnostics for Tauri's built-in asset protocol logs.
//! Enabled only by BURRATA_ARTWORK_PROTOCOL_TRACE=1. Never emits log arguments.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    InvalidPath,
    ScopeDenied,
    NotFound,
    OsPermissionDenied,
    AndroidExternalStorageOpenFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallState {
    Disabled,
    Installed,
    Unavailable,
}
impl InstallState {
    fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Installed => "installed",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Snapshot {
    state: Option<InstallState>,
    invalid_path: usize,
    scope_denied: usize,
    not_found: usize,
    os_permission_denied: usize,
    android_external_storage_open_failure: usize,
}

static INSTALL_STATE: OnceLock<InstallState> = OnceLock::new();
static COUNTS: [AtomicUsize; 5] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
static LOGGER: AssetLogger = AssetLogger;

struct AssetLogger;
impl log::Log for AssetLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Error && metadata.target() == "tauri::protocol::asset"
    }
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Convert only to a transient string for prefix matching. It is never stored or emitted.
        let message = record.args().to_string();
        if let Some(failure) = classify(&message) {
            let counter = match failure {
                Failure::InvalidPath => &COUNTS[0],
                Failure::ScopeDenied => &COUNTS[1],
                Failure::NotFound => &COUNTS[2],
                Failure::OsPermissionDenied => &COUNTS[3],
                Failure::AndroidExternalStorageOpenFailure => &COUNTS[4],
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn flush(&self) {}
}

fn classify(message: &str) -> Option<Failure> {
    if message.starts_with("asset protocol path ") && message.contains(" is not valid:") {
        Some(Failure::InvalidPath)
    } else if message.starts_with("asset protocol not configured to allow the path:") {
        Some(Failure::ScopeDenied)
    } else if message.starts_with("File does not exist at path:") {
        Some(Failure::NotFound)
    } else if message.starts_with("Missing OS permission to access path ") {
        Some(Failure::OsPermissionDenied)
    } else if message.starts_with("Failed to open Android external storage file '") {
        Some(Failure::AndroidExternalStorageOpenFailure)
    } else {
        None
    }
}

pub(crate) fn initialize() -> InstallState {
    *INSTALL_STATE.get_or_init(|| {
        if std::env::var_os("BURRATA_ARTWORK_PROTOCOL_TRACE").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return InstallState::Disabled;
        }
        if log::set_logger(&LOGGER).is_err() {
            return InstallState::Unavailable;
        }
        // This logger is installed only when the global slot is empty. It records only the
        // fixed asset error categories above and deliberately drops every other record.
        log::set_max_level(log::LevelFilter::Error);
        InstallState::Installed
    })
}

pub(crate) fn snapshot() -> Snapshot {
    Snapshot {
        state: INSTALL_STATE.get().copied(),
        invalid_path: COUNTS[0].load(Ordering::Relaxed),
        scope_denied: COUNTS[1].load(Ordering::Relaxed),
        not_found: COUNTS[2].load(Ordering::Relaxed),
        os_permission_denied: COUNTS[3].load(Ordering::Relaxed),
        android_external_storage_open_failure: COUNTS[4].load(Ordering::Relaxed),
    }
}

impl Snapshot {
    pub(crate) fn report_since(self, before: Self) -> serde_json::Value {
        let state = self.state.or(before.state);
        serde_json::json!({
            "enabled": state == Some(InstallState::Installed),
            "registrationSucceeded": state == Some(InstallState::Installed),
            "registrationUnavailable": state == Some(InstallState::Unavailable),
            "registrationState": state.map(InstallState::label).unwrap_or("notInitialized"),
            "scopeDenied": self.scope_denied.saturating_sub(before.scope_denied),
            "notFound": self.not_found.saturating_sub(before.not_found),
            "invalidPath": self.invalid_path.saturating_sub(before.invalid_path),
            "osPermissionDenied": self.os_permission_denied.saturating_sub(before.os_permission_denied),
            "androidExternalStorageOpenFailure": self.android_external_storage_open_failure.saturating_sub(before.android_external_storage_open_failure),
            "attribution": "all_asset_protocol_errors_during_observation_window"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_known_asset_error_prefixes_without_retaining_suffixes() {
        let cases = [
            (
                "asset protocol path \"/private/art.jpg\" is not valid: detail",
                Failure::InvalidPath,
            ),
            (
                "asset protocol not configured to allow the path: /private/art.jpg",
                Failure::ScopeDenied,
            ),
            (
                "File does not exist at path: /private/art.jpg",
                Failure::NotFound,
            ),
            (
                "Missing OS permission to access path \"/private/art.jpg\": denied",
                Failure::OsPermissionDenied,
            ),
            (
                "Failed to open Android external storage file '/private/art.jpg': denied",
                Failure::AndroidExternalStorageOpenFailure,
            ),
        ];
        for (message, expected) in cases {
            assert_eq!(classify(message), Some(expected));
            let report = Snapshot {
                state: Some(InstallState::Installed),
                scope_denied: 1,
                ..Snapshot::default()
            }
            .report_since(Snapshot::default())
            .to_string();
            assert!(!report.contains("/private"));
            assert!(!report.contains("art.jpg"));
            assert!(!report.contains("detail"));
        }
    }

    #[test]
    fn unknown_messages_are_discarded_and_registration_state_is_fixed() {
        assert_eq!(classify("private path /secret: unexpected failure"), None);
        let output = Snapshot {
            state: Some(InstallState::Unavailable),
            ..Snapshot::default()
        }
        .report_since(Snapshot::default());
        assert_eq!(output["registrationSucceeded"], false);
        assert_eq!(output["registrationUnavailable"], true);
        assert_eq!(output["registrationState"], "unavailable");
        assert_eq!(
            output["attribution"],
            "all_asset_protocol_errors_during_observation_window"
        );
        assert!(!output.to_string().contains("/secret"));
    }
}
