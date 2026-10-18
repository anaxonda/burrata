//! Temporary, bounded observation of only the current app-owned artwork request.
//! Never exposes a URI, filesystem path, response body, or error message.
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::Manager;

#[derive(Clone)]
pub(crate) struct Observation {
    report: Arc<Mutex<Report>>,
    protocol_trace_before: crate::artwork_protocol_trace::Snapshot,
    finished_tx: tokio::sync::watch::Sender<bool>,
    finished_rx: tokio::sync::watch::Receiver<bool>,
    started_at: Arc<Mutex<Option<tokio::time::Instant>>>,
}
struct Report {
    state: &'static str,
    matched: usize,
    asset_seen: usize,
    asset_http_seen: usize,
    resource_events: usize,
    listener_attached: bool,
    normalized_matches: usize,
    security: Value,
    requests: Vec<Value>,
}
impl Observation {
    fn new(state: &'static str) -> Self {
        let (finished_tx, finished_rx) = tokio::sync::watch::channel(false);
        Self {
            report: Arc::new(Mutex::new(Report {
                state,
                matched: 0,
                asset_seen: 0,
                asset_http_seen: 0,
                resource_events: 0,
                listener_attached: false,
                normalized_matches: 0,
                security: Value::Null,
                requests: Vec::new(),
            })),
            protocol_trace_before: crate::artwork_protocol_trace::snapshot(),
            finished_tx,
            finished_rx,
            started_at: Arc::new(Mutex::new(None)),
        }
    }
    pub(crate) fn snapshot(&self) -> Value {
        let protocol_trace =
            crate::artwork_protocol_trace::snapshot().report_since(self.protocol_trace_before);
        self.report.lock().map(|report|json!({"state":report.state,"dispatch":dispatch_state(&report),"listenerAttached":report.listener_attached,"resourceEvents":report.resource_events,"matchedRequests":report.matched,"assetRequestsSeen":report.asset_seen,"assetLocalhostHttpRequestsSeen":report.asset_http_seen,"normalizedMatches":report.normalized_matches,"assetSecurity":report.security,"requests":report.requests,"protocolTrace":protocol_trace}))
            .unwrap_or_else(|_|json!({"state":"unavailable","matchedRequests":0,"requests":[]}))
    }
    pub(crate) async fn wait_until_finished(&self) {
        let started_at = self
            .started_at
            .lock()
            .ok()
            .and_then(|value| *value)
            .unwrap_or_else(tokio::time::Instant::now);
        self.wait_until(started_at + std::time::Duration::from_millis(4500))
            .await;
    }
    async fn wait_until(&self, deadline: tokio::time::Instant) {
        if self
            .report
            .lock()
            .map(|report| report.state != "observing")
            .unwrap_or(true)
        {
            return;
        }
        let mut finished = self.finished_rx.clone();
        let timed_out = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            finished.wait_for(|done| *done),
        )
        .await
        .is_err();
        if timed_out && !*finished.borrow() {
            if let Ok(mut report) = self.report.lock() {
                if report.state == "observing" {
                    report.state = "timedOut";
                }
            }
        }
    }
    fn mark_finished(&self) {
        if let Ok(mut report) = self.report.lock() {
            report.state = "finished";
        }
        self.finished_tx.send_replace(true);
    }
}
fn encoded_asset_uri(path: &str) -> String {
    let mut uri = String::from("asset://localhost/");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}
fn asset_path(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("asset://localhost/")
        .or_else(|| uri.strip_prefix("http://asset.localhost/"))?
        .split(['?', '#'])
        .next()?;
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            decoded.push(((high << 4) | low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    if decoded.contains(&0) {
        return None;
    }
    String::from_utf8(decoded).ok()
}
fn dispatch_state(report: &Report) -> &'static str {
    if !report.listener_attached {
        "listener_unavailable"
    } else if report.matched > 0 {
        "matched_request"
    } else if report.asset_seen > 0 {
        "unmatched_asset_request"
    } else if report.resource_events > 0 {
        "other_resource_only"
    } else {
        "no_resource_event"
    }
}
fn same_asset(uri: &str, expected: &str) -> bool {
    match (asset_path(uri), asset_path(expected)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn error_category(error: &webkit2gtk::glib::Error) -> &'static str {
    use webkit2gtk::{NetworkError as N, PolicyError as P};
    if error.matches(N::UnknownProtocol) {
        "unknownProtocol"
    } else if error.matches(N::FileDoesNotExist) {
        "fileDoesNotExist"
    } else if error.matches(N::Transport) {
        "transport"
    } else if error.matches(N::Cancelled) {
        "cancelled"
    } else if error.matches(P::CannotShowMimeType) {
        "cannotShowMimeType"
    } else if error.matches(P::CannotShowUri) {
        "cannotShowUri"
    } else if error.is::<N>() {
        "networkOther"
    } else if error.is::<P>() {
        "policyOther"
    } else if error.is::<webkit2gtk::gio::IOErrorEnum>() {
        "ioError"
    } else {
        "other"
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn start(app: &tauri::AppHandle, path: Option<PathBuf>) -> Observation {
    use webkit2gtk::glib::prelude::ObjectExt;
    use webkit2gtk::{
        SecurityManagerExt, URIRequestExt, URIResponseExt, WebContextExt, WebResourceExt,
        WebViewExt,
    };
    let observation = Observation::new("unavailable");
    let Some(path) = path else {
        return observation;
    };
    let Ok(directory) = app.path().app_data_dir().map(|path| path.join("artwork")) else {
        return observation;
    };
    // The path comes from native playback, and must be a direct child of our cache.
    if path.parent() != Some(directory.as_path()) {
        return observation;
    }
    let Some(path) = path.to_str() else {
        return observation;
    };
    let expected = encoded_asset_uri(path);
    let Some(window) = app.get_webview_window("main") else {
        return observation;
    };
    let shared = observation.report.clone();
    let finished = observation.clone();
    let started_at = observation.started_at.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let scheduled = window.with_webview(move |platform| {
        let view = platform.inner();
        if let Some(manager) = view.context().and_then(|context|context.security_manager()) {
            if let Ok(mut report)=shared.lock() {
                report.security=json!({"secure":manager.uri_scheme_is_secure("asset"),"local":manager.uri_scheme_is_local("asset"),"noAccess":manager.uri_scheme_is_no_access("asset"),"displayIsolated":manager.uri_scheme_is_display_isolated("asset"),"corsEnabled":manager.uri_scheme_is_cors_enabled("asset")});
            }
        }
        let shared_for_requests = shared.clone();
        let handler = view.connect_resource_load_started(move |_,resource,request| {
            let uri = request.uri();
            if let Ok(mut report)=shared_for_requests.lock() {
                report.resource_events=(report.resource_events+1).min(16);
                if let Some(uri) = uri.as_deref() {
                    if uri.starts_with("asset://localhost/") {
                        report.asset_seen=(report.asset_seen+1).min(16);
                    } else if uri.starts_with("http://asset.localhost/") {
                        report.asset_seen=(report.asset_seen+1).min(16);
                        report.asset_http_seen=(report.asset_http_seen+1).min(16);
                    }
                }
            }
            let Some(uri) = uri else { return; };
            if !uri.starts_with("asset://localhost/") && !uri.starts_with("http://asset.localhost/") { return; }
            if !same_asset(&uri,&expected) { return; }
            if let Ok(mut report) = shared_for_requests.lock() {
                if asset_path(&uri) != asset_path(&expected) || uri.split(['?', '#']).next() != Some(expected.as_str()) { report.normalized_matches=(report.normalized_matches+1).min(4); }
                if report.matched >= 4 { return; }
                report.matched += 1;
            } else { return; }
            let failed = Arc::new(Mutex::new("none"));
            let failure = failed.clone();
            resource.connect_failed(move |_,error| { if let Ok(mut value)=failure.lock() { *value=error_category(error); } });
            let shared = shared_for_requests.clone();
            resource.connect_finished(move |resource| {
                let response = resource.response();
                let status = response.as_ref().map(|response|response.status_code()).unwrap_or(0).min(999);
                let mime = response.as_ref().and_then(|response|response.mime_type());
                let mime = match mime.as_deref() { Some("image/jpeg")=>"jpeg",Some("image/png")=>"png",Some("image/webp")=>"webp",Some("text/plain")=>"text",None=>"none",_=>"other" };
                let error = failed.lock().map(|value|*value).unwrap_or("other");
                if let Ok(mut report)=shared.lock() { if report.requests.len()<4 { report.requests.push(json!({"httpStatus":status,"mimeType":mime,"errorCategory":error})); } }
            });
        });
        if let Ok(mut started_at) = started_at.lock() { *started_at = Some(tokio::time::Instant::now()); }
        if let Ok(mut report)=shared.lock() { report.listener_attached=true; report.state="observing"; }
        let view = view.clone();
        webkit2gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(4),move || {
            view.disconnect(handler);
            finished.mark_finished();
        });
        let _ = tx.send(());
    });
    if scheduled.is_err()
        || !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx).await,
            Ok(Ok(()))
        )
    {
        if let Ok(mut report) = observation.report.lock() {
            report.state = "unavailable";
        }
        observation.finished_tx.send_replace(true);
    }
    observation
}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn start(_app: &tauri::AppHandle, _path: Option<PathBuf>) -> Observation {
    Observation::new("unsupported")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observes_only_exact_asset_path_while_ignoring_probe_query() {
        let expected = encoded_asset_uri("/owned/artwork/a b.jpg");
        assert_eq!(expected, "asset://localhost/%2Fowned%2Fartwork%2Fa%20b.jpg");
        assert!(same_asset(
            &format!("{expected}?burrataArtworkProbe=123"),
            &expected
        ));
        assert!(same_asset(
            "http://asset.localhost/%2Fowned%2Fartwork%2Fa%20b.jpg?burrataArtworkProbe=123",
            &expected
        ));
        assert!(same_asset(
            "asset://localhost//owned/artwork/a%20b.jpg",
            &expected
        ));
        assert!(same_asset(
            "asset://localhost/%2fowned%2fartwork%2fa%20b.jpg",
            &expected
        ));
        assert!(!same_asset(
            "asset://localhost/%252Fowned%252Fartwork%252Fa%2520b.jpg",
            &expected
        ));
        assert!(!same_asset(
            "asset://localhost/%2Fowned%2Fartwork%2Fa%00b.jpg",
            &expected
        ));
        assert!(!same_asset(
            "asset://localhost/%2Fprivate%2Fsecret.jpg",
            &expected
        ));
        assert!(!same_asset(
            "https://localhost/%2Fowned%2Fartwork%2Fa%20b.jpg",
            &expected
        ));
    }
    #[test]
    fn dispatch_report_uses_fixed_enums_without_uri_data() {
        let mut report = Report {
            state: "observing",
            matched: 0,
            asset_seen: 0,
            asset_http_seen: 0,
            resource_events: 0,
            listener_attached: false,
            normalized_matches: 0,
            security: Value::Null,
            requests: Vec::new(),
        };
        assert_eq!(dispatch_state(&report), "listener_unavailable");
        report.listener_attached = true;
        assert_eq!(dispatch_state(&report), "no_resource_event");
        report.resource_events = 1;
        assert_eq!(dispatch_state(&report), "other_resource_only");
        report.asset_seen = 1;
        assert_eq!(dispatch_state(&report), "unmatched_asset_request");
        report.matched = 1;
        assert_eq!(dispatch_state(&report), "matched_request");
        let report = Observation {
            report: Arc::new(Mutex::new(report)),
            finished_tx: tokio::sync::watch::channel(false).0,
            finished_rx: tokio::sync::watch::channel(false).1,
            started_at: Arc::new(Mutex::new(None)),
            protocol_trace_before: crate::artwork_protocol_trace::snapshot(),
        };
        let output = report.snapshot().to_string();
        assert!(!output.contains("asset://"));
        assert!(!output.contains("localhost"));
        assert!(!output.contains("owned"));
    }
    #[test]
    fn report_contains_no_paths_urls_or_error_messages() {
        let report = Observation::new("observing");
        report
            .report
            .lock()
            .unwrap()
            .requests
            .push(json!({"httpStatus":403,"mimeType":"text","errorCategory":"policyOther"}));
        let result = report.snapshot();
        assert_eq!(result["requests"][0]["httpStatus"], 403);
        let output = result.to_string();
        assert!(!output.contains("asset://"));
        assert!(!output.contains("/owned"));
        assert!(!output.contains("message"));
    }

    #[tokio::test]
    async fn completion_wait_returns_only_after_observation_finishes() {
        let observation = Observation::new("observing");
        let finisher = observation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            finisher.mark_finished();
        });
        tokio::time::sleep(std::time::Duration::from_millis(1)).await; // UI replies quickly; collection still completes later.
        observation
            .wait_until(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
            .await;
        assert_eq!(observation.snapshot()["state"], "finished");
    }

    #[tokio::test]
    async fn missing_glib_completion_is_reported_as_timed_out_at_deadline() {
        let observation = Observation::new("observing");
        observation
            .wait_until(tokio::time::Instant::now() + std::time::Duration::from_millis(10))
            .await;
        assert_eq!(observation.snapshot()["state"], "timedOut");
    }
}
