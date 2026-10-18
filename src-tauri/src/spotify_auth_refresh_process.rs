//! Dedicated private IPC for the Spotify auth-only worker operation.
//! This parser never returns serde_json::Value to ordinary provider code.
use super::spotify_auth_cache::SecretToken;
use std::process::Stdio;
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command as TokioCommand,
};

const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_TOKEN_TTL: u64 = 24 * 60 * 60 * 1000;
const EXPIRY_SKEW_MS: u64 = 30_000;
struct SensitiveRequest(Vec<u8>);
impl Drop for SensitiveRequest {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(crate) async fn run_private_auth_refresh(
    node: PathBuf,
    worker: PathBuf,
    request: Vec<u8>,
    stop_cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(SecretToken, Instant, u64, bool), String> {
    let request = SensitiveRequest(request);
    let mut child = TokioCommand::new(node)
        .arg(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Could not start the isolated Spotify auth worker".to_owned())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("Spotify auth worker input is unavailable")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Spotify auth worker output is unavailable")?;
    let operation = async {
        stdin
            .write_all(&request.0)
            .await
            .map_err(|_| "Could not send Spotify auth request".to_owned())?;
        stdin
            .shutdown()
            .await
            .map_err(|_| "Could not close Spotify auth input".to_owned())?;
        drop(stdin);
        let mut output = Vec::new();
        stdout
            .take((MAX_RESPONSE + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "Could not read Spotify auth response".to_owned())?;
        if output.len() > MAX_RESPONSE {
            output.fill(0);
            return Err("Spotify auth response exceeded its size limit".into());
        }
        let status = child
            .wait()
            .await
            .map_err(|_| "Spotify auth worker could not finish".to_owned())?;
        if !status.success() {
            output.fill(0);
            return Err("Spotify auth worker failed".into());
        }
        parse_private_auth_response(&mut output)
    };
    tokio::select! {
        biased;
        _ = wait_for_stop(stop_cancel) => Err("Provider request was cancelled".into()),
        result = tokio::time::timeout(timeout, operation) => match result {
            Ok(value) => value,
            Err(_) => Err("Spotify token refresh timed out".into()),
        },
    }
}

fn parse_private_auth_response(
    output: &mut Vec<u8>,
) -> Result<(SecretToken, Instant, u64, bool), String> {
    let parsed = serde_json::from_slice::<serde_json::Value>(output);
    output.fill(0);
    let mut response =
        parsed.map_err(|_| "Spotify auth worker returned invalid data".to_owned())?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let code = response
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str);
        let phase = response
            .pointer("/error/phase")
            .and_then(serde_json::Value::as_str);
        let status = response
            .pointer("/error/httpStatus")
            .and_then(serde_json::Value::as_u64);
        let retry = response
            .pointer("/error/retryAfterSeconds")
            .and_then(serde_json::Value::as_u64)
            .filter(|seconds| (1..=3600).contains(seconds));
        return Err(safe_refresh_failure(code, phase, status, retry));
    }
    let mut envelope = response
        .as_object_mut()
        .and_then(|root| root.remove("internalAuth"))
        .ok_or("Spotify auth worker returned no private token envelope")?;
    if response
        .as_object()
        .map_or(true, |root| root.len() != 1 || !root.contains_key("ok"))
    {
        return Err("Spotify auth worker returned an invalid private response".into());
    }
    let envelope = envelope
        .as_object_mut()
        .ok_or("Spotify auth envelope was invalid")?;
    if envelope.len() != 3 {
        return Err("Spotify auth envelope was invalid".into());
    }
    let token = match envelope.remove("accessToken") {
        Some(serde_json::Value::String(token)) => token,
        _ => return Err("Spotify auth envelope was invalid".into()),
    };
    if token.is_empty() {
        return Err("Spotify auth envelope was invalid".into());
    }
    let token = SecretToken::new(token)?;
    let expires_at_ms = envelope
        .remove("expiresAt")
        .and_then(|value| value.as_u64())
        .ok_or("Spotify token expiry was invalid")?;
    let cacheable = envelope
        .remove("cacheable")
        .and_then(|value| value.as_bool())
        .ok_or("Spotify token expiry was invalid")?;
    if !envelope.is_empty() {
        return Err("Spotify auth envelope was invalid".into());
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System clock is invalid".to_owned())?
        .as_millis() as u64;
    let ttl_ms = expires_at_ms.saturating_sub(now_ms);
    if ttl_ms < 30_000 || ttl_ms > MAX_TOKEN_TTL || (cacheable && ttl_ms <= EXPIRY_SKEW_MS) {
        return Err("Spotify token expiry was invalid".into());
    }
    Ok((
        token,
        Instant::now() + Duration::from_millis(ttl_ms),
        expires_at_ms,
        cacheable,
    ))
}

fn safe_refresh_failure(
    code: Option<&str>,
    phase: Option<&str>,
    status: Option<u64>,
    retry: Option<u64>,
) -> String {
    let mut message = match code {
        Some("SIGN_IN_REQUIRED") => "SIGN_IN_REQUIRED: Spotify authentication is unavailable. Sign in again and retry.".into(),
        Some("VERIFICATION_REQUIRED") => "VERIFICATION_REQUIRED: Spotify requires verification. Complete it in the browser and retry.".into(),
        Some("RATE_LIMITED") => match retry {
            Some(seconds) => format!("RATE_LIMITED: Spotify is rate limiting requests. Try again in {seconds} seconds."),
            None => "RATE_LIMITED: Spotify is rate limiting requests. Try again shortly.".into(),
        },
        Some("REQUEST_TIMEOUT") => "REQUEST_TIMEOUT: Spotify token refresh timed out. Try again shortly.".into(),
        Some("API_DENIED") => "API_DENIED: Spotify denied this request.".into(),
        _ => "Spotify token refresh failed".into(),
    };
    let phase = match phase {
        Some("worker_start") => Some("worker_start"),
        Some("token_exchange") => Some("token_exchange"),
        Some("spotify_clock") => Some("spotify_clock"),
        Some("totp_registry") => Some("totp_registry"),
        Some("hash_registry") => Some("hash_registry"),
        Some("account_profile") => Some("account_profile"),
        Some("owner_profile") => Some("owner_profile"),
        Some("catalog_rest") => Some("catalog_rest"),
        Some("catalog_graphql") => Some("catalog_graphql"),
        Some("provider_request") => Some("provider_request"),
        _ => None,
    };
    let status = status
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| (100..=599).contains(s));
    if let Some(phase) = phase {
        message.push_str(&format!(" (phase={phase}"));
        if let Some(status) = status {
            message.push_str(&format!(", HTTP {status}"));
        }
        message.push(')');
    } else if let Some(status) = status {
        message.push_str(&format!(" (HTTP {status})"));
    }
    message
}

async fn wait_for_stop(flag: Arc<AtomicBool>) {
    while !flag.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(cacheable: bool, ttl_ms: u64) -> Vec<u8> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "internalAuth": {
                "accessToken": "fake-private-fixture-token",
                "expiresAt": now_ms + ttl_ms,
                "cacheable": cacheable
            }
        }))
        .unwrap()
    }

    #[test]
    fn accepts_cacheable_expiry_and_one_job_lease() {
        for (cacheable, ttl) in [(true, 120_000), (false, 60_000)] {
            let mut bytes = response(cacheable, ttl);
            let (_, _, _, got_cacheable) = parse_private_auth_response(&mut bytes).unwrap();
            assert_eq!(got_cacheable, cacheable);
        }
    }

    #[test]
    fn rejects_invalid_expiry_and_public_response_shapes() {
        for ttl in [1_000, 25 * 60 * 60 * 1000] {
            let mut bytes = response(false, ttl);
            assert!(parse_private_auth_response(&mut bytes).is_err());
        }
        let mut public = br#"{"ok":true,"result":{"accountId":"fixture"}}"#.to_vec();
        assert!(parse_private_auth_response(&mut public).is_err());
    }

    #[test]
    fn exposes_only_allowlisted_auth_errors_and_bounded_retry() {
        for (code, retry, expected) in [
            (
                "RATE_LIMITED",
                Some(30),
                "RATE_LIMITED: Spotify is rate limiting requests. Try again in 30 seconds.",
            ),
            (
                "RATE_LIMITED",
                Some(4000),
                "RATE_LIMITED: Spotify is rate limiting requests. Try again shortly.",
            ),
            (
                "REQUEST_TIMEOUT",
                Some(30),
                "REQUEST_TIMEOUT: Spotify token refresh timed out. Try again shortly.",
            ),
            (
                "SIGN_IN_REQUIRED",
                None,
                "SIGN_IN_REQUIRED: Spotify authentication is unavailable. Sign in again and retry.",
            ),
            ("UNTRUSTED_RAW_TEXT", None, "Spotify token refresh failed"),
        ] {
            let got = safe_refresh_failure(
                Some(code),
                None,
                None,
                retry.filter(|seconds| (1..=3600).contains(seconds)),
            );
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn preserves_only_allowlisted_phase_and_http_status() {
        assert!(safe_refresh_failure(
            Some("SIGN_IN_REQUIRED"),
            Some("token_exchange"),
            Some(401),
            None
        )
        .contains("phase=token_exchange, HTTP 401"));
        let unsafe_message = safe_refresh_failure(
            Some("SIGN_IN_REQUIRED"),
            Some("cookie=secret"),
            Some(999),
            None,
        );
        assert!(!unsafe_message.contains("cookie=secret"));
        assert!(!unsafe_message.contains("HTTP 999"));
    }
}
