//! Internal anonymous native HTTP experiment, available only in tests or behind
//! the off-default playback-direct-youtube-experiment feature.
//! Mirrors the pinned VISIONOS_0_1 request/visitor fixtures; never claims catalog proof.
use futures_util::StreamExt;
use icu_normalizer::ComposingNormalizerBorrowed;
use reqwest::{header::HeaderMap, Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, future::Future, sync::OnceLock, time::Duration};
use tokio::{sync::watch, time::Instant};
use url::Url;

#[cfg(all(test, not(feature = "experimental-opus")))]
#[path = "youtube_direct_pair_tests.rs"]
mod paired_tests;

const MAX_BODY: usize = 4 * 1024 * 1024;
const PLAYER: &str = "https://music.youtube.com/youtubei/v1/player?prettyPrint=false";
const PROFILE_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";
const VISITOR_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";
const OPUS: &str = "audio/webm; codecs=\"opus\"";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirectFailure {
    pub(crate) code: &'static str,
    pub(crate) stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) http_status: Option<u16>,
}
type Result<T> = std::result::Result<T, DirectFailure>;
fn fail(code: &'static str, stage: &'static str) -> DirectFailure {
    DirectFailure {
        code,
        stage,
        http_status: None,
    }
}
impl DirectFailure {
    fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }
}

/// The URL and fresh title are private playback inputs, not diagnostic output.
/// The caller must still attach its session epoch and run identity/access gates.
pub(crate) struct DirectResolved {
    pub(crate) audio: crate::acquisition::PreviewAudio,
    pub(crate) valid_until_unix_ms: f64,
}
impl std::fmt::Debug for DirectResolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectResolved")
            .field("audio", &"<redacted>")
            .field("valid_until_unix_ms", &self.valid_until_unix_ms)
            .finish()
    }
}

static ANONYMOUS_CLIENT: OnceLock<std::result::Result<DirectExperiment, DirectFailure>> =
    OnceLock::new();

/// Resolve only the caller's explicit provider ID. One cookie-free HTTP client
/// is reused; anonymous visitor context is fetched afresh inside each resolve.
/// This function neither caches media URLs nor creates recording/catalog proof.
pub(crate) async fn resolve_explicit(
    video_id: &str,
    budget_ms: u64,
    cancel: watch::Receiver<bool>,
) -> std::result::Result<DirectResolved, DirectFailure> {
    let started = Instant::now();
    let request = Request {
        video_id: video_id.to_owned(),
        expected_video_id: video_id.to_owned(),
        hl: "en".into(),
        gl: "US".into(),
        budget_ms,
        format_id: 251,
        expected_metadata: None,
        visitor_mode: VisitorMode::AnonymousSource,
    };
    request.validate()?;
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(fail("CANCELLED", "request"));
    }
    let client = match ANONYMOUS_CLIENT.get_or_init(DirectExperiment::new) {
        Ok(client) => client,
        Err(error) => return Err(*error),
    };
    client
        .resolve(&request, started + Duration::from_millis(budget_ms), cancel)
        .await
        .map(Resolved::into_explicit_audio)
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedMetadata {
    title_needle: String,
    channel_needle: String,
    duration_seconds: f64,
}
#[derive(Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum VisitorMode {
    #[default]
    Omit,
    AnonymousSource,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    video_id: String,
    expected_video_id: String,
    hl: String,
    gl: String,
    #[serde(default = "default_budget")]
    budget_ms: u64,
    #[serde(default = "default_format")]
    format_id: u64,
    #[serde(default)]
    expected_metadata: Option<ExpectedMetadata>,
    #[serde(default)]
    visitor_mode: VisitorMode,
}
fn default_budget() -> u64 {
    19_000
}
fn default_format() -> u64 {
    251
}
fn valid_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn normalized(value: &str) -> String {
    crate::matching::normalize(&ComposingNormalizerBorrowed::new_nfkc().normalize(value))
}
impl Request {
    fn validate(&self) -> Result<()> {
        let mut language = self.hl.split(['-', '_']);
        let first = language.next().unwrap_or_default();
        let valid_language = (2..=3).contains(&first.len())
            && first.bytes().all(|c| c.is_ascii_lowercase())
            && language.all(|part| {
                (2..=8).contains(&part.len()) && part.bytes().all(|c| c.is_ascii_alphanumeric())
            });
        if !valid_id(&self.video_id)
            || self.video_id != self.expected_video_id
            || !valid_language
            || self.gl.len() != 2
            || !self.gl.bytes().all(|c| c.is_ascii_uppercase())
            || self.format_id != 251
        {
            return Err(fail("INVALID_PLAYBACK_REQUEST", "request"));
        }
        if !(1..=20_000).contains(&self.budget_ms) {
            return Err(fail("INVALID_BUDGET", "request"));
        }
        if let Some(m) = &self.expected_metadata {
            if normalized(&m.title_needle).is_empty()
                || normalized(&m.channel_needle).is_empty()
                || m.title_needle.chars().count() > 256
                || m.channel_needle.chars().count() > 256
                || !m.duration_seconds.is_finite()
                || !(5.0..=86_400.0).contains(&m.duration_seconds)
            {
                return Err(fail("INVALID_PLAYBACK_REQUEST", "request"));
            }
        }
        Ok(())
    }
    fn accept_language(&self) -> String {
        let language = self.hl.replace('_', "-");
        let regional = if language.contains('-') {
            language.clone()
        } else {
            format!("{language}-{}", self.gl)
        };
        format!(
            "{regional},{};q=0.9",
            language.split('-').next().unwrap_or_default()
        )
    }
}

// Intentionally neither Debug nor Serialize: a visitor must stay in memory.
struct AnonymousVisitor(String);
fn valid_visitor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && value.bytes().all(|c| (0x21..=0x7e).contains(&c))
}
#[derive(Clone, Copy)]
enum VisitorSource {
    ServiceWorker,
    Homepage,
}
impl VisitorSource {
    fn stage(self) -> &'static str {
        match self {
            Self::ServiceWorker => "visitorServiceWorker",
            Self::Homepage => "visitorHomepage",
        }
    }
}
fn parse_visitor(text: &str, source: VisitorSource) -> Option<AnonymousVisitor> {
    let value = match source {
        VisitorSource::ServiceWorker => {
            let payload = text
                .split_once('\n')
                .map_or(text, |(_, rest)| rest)
                .trim_start();
            let data: Value = serde_json::from_str(payload).ok()?;
            data.pointer("/0/2/0/0/13")?.as_str()?.to_owned()
        }
        VisitorSource::Homepage => {
            // The source regex requires an exact quoted key, optional whitespace,
            // colon and quoted nonempty value; no JavaScript is evaluated.
            let mut found = None;
            for (offset, _) in text.match_indices('"') {
                let rest = &text[offset..];
                let Some(rest) = rest
                    .strip_prefix("\"VISITOR_DATA\"")
                    .or_else(|| rest.strip_prefix("\"visitorData\""))
                else {
                    continue;
                };
                let Some(rest) = rest.trim_start().strip_prefix(':') else {
                    continue;
                };
                let Some(rest) = rest.trim_start().strip_prefix('"') else {
                    continue;
                };
                let Some(end) = rest.find('"') else { continue };
                if end > 0 {
                    found = Some(rest[..end].to_owned());
                    break;
                }
            }
            found?
        }
    };
    valid_visitor(&value).then_some(AnonymousVisitor(value))
}

fn parse_visitor_response(
    status: u16,
    text: &str,
    source: VisitorSource,
) -> Result<AnonymousVisitor> {
    if (200..300).contains(&status) {
        if let Some(visitor) = parse_visitor(text, source) {
            return Ok(visitor);
        }
    }
    Err(fail("VISITOR_CONTEXT_UNAVAILABLE", source.stage()).with_http_status(status))
}

struct Spec {
    endpoint: String,
    headers: BTreeMap<String, String>,
    body: Option<Value>,
}
fn player_spec(request: &Request, visitor: Option<&AnonymousVisitor>) -> Result<Spec> {
    request.validate()?;
    let mut headers: BTreeMap<String, String> = [
        ("Content-Type", "application/json"),
        ("Accept", "application/json"),
        ("Cache-Control", "no-cache"),
        ("X-Goog-Api-Format-Version", "1"),
        ("X-YouTube-Client-Name", "101"),
        ("X-YouTube-Client-Version", "0.1"),
        ("Origin", "https://music.youtube.com"),
        ("X-Origin", "https://music.youtube.com"),
        ("Referer", "https://music.youtube.com/"),
        ("User-Agent", PROFILE_UA),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    headers.insert("Accept-Language".into(), request.accept_language());
    let mut body = json!({"context":{"client":{"clientName":"VISIONOS","clientVersion":"0.1",
        "osName":"VISION_OS","osVersion":"1.3","deviceMake":"Apple","deviceModel":"RealityDevice14,1",
        "platform":"MOBILE","gl":request.gl,"hl":request.hl},
        "request":{"internalExperimentFlags":[],"useSsl":true},"user":{"lockedSafetyMode":false}},
        "videoId":request.video_id,"contentCheckOk":true,"racyCheckOk":true});
    if let Some(visitor) = visitor {
        if request.visitor_mode != VisitorMode::AnonymousSource || !valid_visitor(&visitor.0) {
            return Err(fail("INVALID_PLAYBACK_REQUEST", "request"));
        }
        headers.insert("X-Goog-Visitor-Id".into(), visitor.0.clone());
        body["context"]["client"]["visitorData"] = Value::String(visitor.0.clone());
    }
    Ok(Spec {
        endpoint: PLAYER.into(),
        headers,
        body: Some(body),
    })
}
fn visitor_spec(request: &Request, source: VisitorSource) -> Spec {
    let (endpoint, accept) = match source {
        VisitorSource::ServiceWorker => (
            "https://www.youtube.com/sw.js_data",
            "application/json,text/plain,*/*",
        ),
        VisitorSource::Homepage => (
            "https://music.youtube.com",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
    };
    Spec {
        endpoint: endpoint.into(),
        body: None,
        headers: [
            ("User-Agent".into(), VISITOR_UA.into()),
            ("Accept-Language".into(), request.accept_language()),
            ("Cache-Control".into(), "no-cache".into()),
            ("Accept".into(), accept.into()),
        ]
        .into(),
    }
}

struct Budget {
    deadline: Instant,
    cancel: watch::Receiver<bool>,
}
impl Budget {
    async fn run<T>(
        &self,
        stage: &'static str,
        task: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        if *self.cancel.borrow() {
            return Err(fail("CANCELLED", stage));
        }
        if Instant::now() >= self.deadline {
            return Err(fail("TIMEOUT", stage));
        }
        let mut cancel = self.cancel.clone();
        tokio::select! {
            biased;
            _ = async { loop { if *cancel.borrow_and_update() { break; } if cancel.changed().await.is_err() { std::future::pending::<()>().await; } } } => Err(fail("CANCELLED", stage)),
            _ = tokio::time::sleep_until(self.deadline) => Err(fail("TIMEOUT", stage)),
            value = task => {
                if *self.cancel.borrow() { Err(fail("CANCELLED", stage)) }
                else if Instant::now() >= self.deadline { Err(fail("TIMEOUT", stage)) }
                else { value }
            }
        }
    }
}

struct DirectExperiment {
    http: Client,
}
impl DirectExperiment {
    fn new() -> Result<Self> {
        // No cookie feature/store, default authentication or environment proxy.
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| fail("HTTP_CLIENT_UNAVAILABLE", "request"))?;
        Ok(Self { http })
    }
    async fn fetch(
        &self,
        spec: Spec,
        budget: &Budget,
        stage: &'static str,
    ) -> Result<(u16, String)> {
        budget
            .run(stage, async {
                let method = if spec.body.is_some() {
                    Method::POST
                } else {
                    Method::GET
                };
                let mut headers = HeaderMap::new();
                for (name, value) in &spec.headers {
                    let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| fail("INVALID_PLAYBACK_HEADERS", stage))?;
                    let value = reqwest::header::HeaderValue::from_str(value)
                        .map_err(|_| fail("INVALID_PLAYBACK_HEADERS", stage))?;
                    headers.insert(name, value);
                }
                let mut request = self.http.request(method, spec.endpoint).headers(headers);
                if let Some(body) = spec.body {
                    request = request.json(&body);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|_| fail("NETWORK_FAILED", stage))?;
                let status = response.status().as_u16();
                if !(200..300).contains(&status) {
                    return Ok((status, String::new()));
                }
                if response
                    .content_length()
                    .is_some_and(|size| size > MAX_BODY as u64)
                {
                    return Err(fail("RESPONSE_TOO_LARGE", stage).with_http_status(status));
                }
                let mut stream = response.bytes_stream();
                let mut bytes = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk
                        .map_err(|_| fail("NETWORK_FAILED", stage).with_http_status(status))?;
                    if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
                        return Err(fail("RESPONSE_TOO_LARGE", stage).with_http_status(status));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
            })
            .await
    }
    async fn resolve(
        &self,
        request: &Request,
        deadline: Instant,
        cancel: watch::Receiver<bool>,
    ) -> Result<Resolved> {
        request.validate()?;
        let started = Instant::now();
        // The caller's absolute deadline is never extended by a stage or retry.
        let budget = Budget {
            deadline: deadline.min(started + Duration::from_millis(request.budget_ms)),
            cancel,
        };
        let mut timings = BTreeMap::new();
        let mut visitor = None;
        if request.visitor_mode == VisitorMode::AnonymousSource {
            let mut visitor_failure = fail("VISITOR_CONTEXT_UNAVAILABLE", "visitorHomepage");
            for source in [VisitorSource::ServiceWorker, VisitorSource::Homepage] {
                let at = Instant::now();
                let fetched = self
                    .fetch(visitor_spec(request, source), &budget, source.stage())
                    .await;
                timings.insert(
                    source.stage().to_owned(),
                    at.elapsed().as_secs_f64() * 1000.0,
                );
                let parsed = fetched
                    .and_then(|(status, text)| parse_visitor_response(status, &text, source));
                match parsed {
                    Ok(found) => visitor = Some(found),
                    Err(error) if matches!(error.code, "CANCELLED" | "TIMEOUT") => {
                        return Err(error)
                    }
                    Err(error) => {
                        visitor_failure = fail("VISITOR_CONTEXT_UNAVAILABLE", source.stage());
                        visitor_failure.http_status = error.http_status;
                    }
                }
                if visitor.is_some() {
                    break;
                }
            }
            if visitor.is_none() {
                return Err(visitor_failure);
            }
        }
        let at = Instant::now();
        let (status, text) = self
            .fetch(player_spec(request, visitor.as_ref())?, &budget, "player")
            .await?;
        timings.insert("player".into(), at.elapsed().as_secs_f64() * 1000.0);
        budget
            .run("parse", async {
                let mut selected = select_player_response(status, &text, request, unix_ms())?;
                selected.report.stages = timings;
                selected.report.resolution_ms = started.elapsed().as_secs_f64() * 1000.0;
                Ok(selected)
            })
            .await
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeReport {
    profile: &'static str,
    video_id: String,
    format_id: u64,
    codec: &'static str,
    duration_seconds: f64,
    approximate_duration_ms: f64,
    content_length: u64,
    expires_in_ms: f64,
    metadata_matched: Option<bool>,
    resolution_ms: f64,
    stages: BTreeMap<String, f64>,
    qualification: bool,
}
struct Resolved {
    url: Url,
    report: SafeReport,
    // Fresh response facts only: these are neither artist credit proof nor a
    // verified-channel assertion. Never include them in safe reports or Debug.
    fresh_identity: FreshPlayerIdentity,
    valid_until_unix_ms: f64,
}
// Intentionally neither Debug nor Serialize; a title/channel can contain URLs
// or arbitrary provider text. A future identity adapter must opt in explicitly.
struct FreshPlayerIdentity {
    title: String,
    channel_name: String,
    channel_id: Option<String>,
}
impl Resolved {
    fn into_explicit_audio(self) -> DirectResolved {
        let title = self.fresh_identity.title.clone();
        let valid_until_unix_ms = self.valid_until_unix_ms;
        let (mut audio, _) = self.into_preview_audio();
        // This is the title from the same fresh player response and ID. Never
        // substitute the requested title or infer artist/channel verification.
        audio.resolved_title = Some(title);
        DirectResolved {
            audio,
            valid_until_unix_ms,
        }
    }
    /// This is an explicit-source diagnostic, not a catalog identity proof.
    /// Unknown credits, verified-channel state and chapters remain absent.
    fn into_preview_audio(self) -> (crate::acquisition::PreviewAudio, SafeReport) {
        let audio = crate::acquisition::PreviewAudio {
            provider_session: None,
            url: self.url.into(),
            mime_type: "audio/webm".into(),
            codec: Some("opus".into()),
            bitrate_kbps: None,
            duration_seconds: Some(self.report.duration_seconds),
            selected_format_id: Some("251".into()),
            selected_format_has_video: false,
            source_metadata: None,
            resolved_title: None,
            resolved_provider_id: Some(self.report.video_id.clone()),
            chapters: None,
            expected_content_length: Some(self.report.content_length),
            headers: Vec::new(),
        };
        (audio, self.report)
    }
}
impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("url", &"<redacted>")
            .field("fresh_identity", &"<redacted>")
            .field("report", &self.report)
            .finish()
    }
}
fn numeric(value: &Value) -> Option<f64> {
    let number = if let Some(number) = value.as_f64() {
        number
    } else {
        let text = value.as_str()?;
        let mut parts = text.split('.');
        let integer = parts.next()?;
        if integer.is_empty() || !integer.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if let Some(decimal) = parts.next() {
            if decimal.is_empty() || !decimal.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
        }
        if parts.next().is_some() {
            return None;
        }
        text.parse().ok()?
    };
    (number.is_finite() && number >= 0.0).then_some(number)
}
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
fn unix_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
fn select_player_response(
    status: u16,
    text: &str,
    request: &Request,
    now: f64,
) -> Result<Resolved> {
    if !(200..300).contains(&status) {
        return Err(fail("PLAYER_HTTP_ERROR", "player").with_http_status(status));
    }
    let body: Value = serde_json::from_str(text)
        .map_err(|_| fail("INVALID_RESPONSE", "parse").with_http_status(status))?;
    select_audio(&body, request, now).map_err(|error| error.with_http_status(status))
}
fn select_audio(body: &Value, request: &Request, now: f64) -> Result<Resolved> {
    let metadata = &body["videoDetails"];
    if metadata["videoId"].as_str() != Some(&request.video_id) || !valid_id(&request.video_id) {
        return Err(fail("RECORDING_MISMATCH", "parse"));
    }
    let title = metadata["title"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .ok_or(fail("METADATA_MISSING", "parse"))?;
    let author = metadata["author"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .ok_or(fail("METADATA_MISSING", "parse"))?;
    let duration = numeric(&metadata["lengthSeconds"])
        .filter(|v| *v >= 5.0)
        .ok_or(fail("METADATA_MISSING", "parse"))?;
    if ["isLiveContent", "isLive", "isUpcoming"]
        .iter()
        .any(|key| metadata[*key] == true)
        || json_truthy(&body["playabilityStatus"]["liveStreamability"])
    {
        return Err(fail("LIVE_OR_UPCOMING_SOURCE", "parse"));
    }
    if let Some(m) = &request.expected_metadata {
        if !normalized(title).contains(&normalized(&m.title_needle))
            || !normalized(author).contains(&normalized(&m.channel_needle))
            || (duration - m.duration_seconds).abs() > 1.0
        {
            return Err(fail("METADATA_MISMATCH", "parse"));
        }
    }
    if body["playabilityStatus"]["status"] != "OK" {
        return Err(fail("PLAYABILITY_REJECTED", "parse"));
    }
    let data = &body["streamingData"];
    let candidates: Vec<_> = ["formats", "adaptiveFormats"]
        .into_iter()
        .flat_map(|key| data[key].as_array().into_iter().flatten())
        .filter(|format| format["itag"].as_u64() == Some(request.format_id))
        .collect();
    if candidates.is_empty() {
        return Err(fail("FORMAT_UNAVAILABLE", "format"));
    }
    if candidates.len() != 1 {
        return Err(fail("FORMAT_MISMATCH", "format"));
    }
    let format = candidates[0];
    if format["mimeType"] != OPUS
        || format.get("width").is_some()
        || format.get("height").is_some()
        || format["hasAudio"] == false
    {
        return Err(fail("FORMAT_MISMATCH", "format"));
    }
    if json_truthy(&format["signatureCipher"]) || json_truthy(&format["cipher"]) {
        return Err(fail("CIPHER_REQUIRED", "format"));
    }
    let raw_url = format["url"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or(fail("UNSUPPORTED_TRANSPORT", "format"))?;
    // A player response may advertise SABR/HLS alongside valid direct audio.
    // Never substitute those siblings for the selected format's own URL, nor
    // accept the advertised streaming transport itself as a direct media URL.
    if ["serverAbrStreamingUrl", "hlsManifestUrl"]
        .iter()
        .any(|key| data[*key].as_str() == Some(raw_url))
    {
        return Err(fail("UNSUPPORTED_TRANSPORT", "format"));
    }
    let mut url = Url::parse(raw_url).map_err(|_| fail("INVALID_MEDIA_URL", "format"))?;
    let host = url.host_str().unwrap_or_default();
    // Narrower than the Node probe: only the production YouTube media host family.
    if url.scheme() != "https"
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != "/videoplayback"
        || !(host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
    {
        return Err(fail("INVALID_MEDIA_URL", "format"));
    }
    let raw_lower = raw_url.to_ascii_lowercase();
    if url.query_pairs().any(|(key, _)| key == "n")
        || ["?n=", "&n=", "?n%3d", "&n%3d", "%26n=", "%26n%3d"]
            .iter()
            .any(|pattern| raw_lower.contains(pattern))
    {
        return Err(fail("N_TRANSFORM_REQUIRED", "format"));
    }
    let length = numeric(&format["contentLength"])
        .filter(|v| *v > 0.0 && *v <= 9_007_199_254_740_991.0 && v.fract() == 0.0)
        .ok_or(fail("FORMAT_LENGTH_MISSING", "format"))? as u64;
    let approximate = numeric(&format["approxDurationMs"])
        .filter(|v| *v > 0.0)
        .ok_or(fail("FORMAT_DURATION_MISSING", "format"))?;
    if approximate < duration * 1000.0 - (duration * 10.0).max(1000.0) {
        return Err(fail("PREVIEW_OR_TRUNCATED_FORMAT", "format"));
    }
    let from_url = url
        .query_pairs()
        .find(|(key, _)| key == "expire")
        .and_then(|(_, v)| numeric(&Value::String(v.into_owned())))
        .filter(|v| *v > 0.0)
        .map(|v| v * 1000.0);
    let from_response = numeric(&data["expiresInSeconds"])
        .filter(|v| *v > 0.0)
        .map(|v| now + v * 1000.0);
    let expiry = from_url
        .into_iter()
        .chain(from_response)
        .reduce(f64::min)
        .filter(|expiry| expiry.is_finite() && *expiry > now + 30_000.0)
        .ok_or(fail("MANIFEST_EXPIRED", "format"))?;
    if !url.query_pairs().any(|(key, _)| key == "cpn") {
        use base64::Engine;
        let uuid = uuid::Uuid::new_v4();
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&uuid.as_bytes()[..12]);
        url.query_pairs_mut().append_pair("cpn", &nonce);
    }
    Ok(Resolved {
        url,
        valid_until_unix_ms: expiry,
        fresh_identity: FreshPlayerIdentity {
            title: title.to_owned(),
            channel_name: author.to_owned(),
            channel_id: metadata["channelId"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned),
        },
        report: SafeReport {
            profile: "VISIONOS_0_1",
            video_id: request.video_id.clone(),
            format_id: request.format_id,
            codec: OPUS,
            duration_seconds: duration,
            approximate_duration_ms: approximate,
            content_length: length,
            expires_in_ms: expiry - now,
            metadata_matched: request.expected_metadata.as_ref().map(|_| true),
            resolution_ms: 0.0,
            stages: BTreeMap::new(),
            qualification: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        serde_json::from_value(
            json!({"videoId":"uZGRATckoBM","expectedVideoId":"uZGRATckoBM","hl":"en","gl":"US"}),
        )
        .unwrap()
    }
    fn body() -> Value {
        json!({"playabilityStatus":{"status":"OK"},"videoDetails":{"videoId":"uZGRATckoBM","title":"Ｆｉｘｔｕｒｅ song","author":"Fixture author","lengthSeconds":"260"},
            "streamingData":{"expiresInSeconds":"3600","adaptiveFormats":[{"itag":251,"mimeType":OPUS,"contentLength":"4000000","approxDurationMs":"260020",
                "url":"https://r1---fixture.googlevideo.com/videoplayback?expire=9999999999&sig=private-signature"}]}})
    }
    fn sw(value: Value) -> String {
        let mut row = vec![Value::Null; 14];
        row[13] = value;
        format!(")]}}'\n{}", json!([[null, null, [[row]]]]))
    }

    #[test]
    fn player_failure_status_is_numeric_and_response_text_stays_private() {
        let private_body = "private-response https://private.invalid/?token=private-token";
        for status in [302, 403, 429, 500, 503] {
            let error =
                select_player_response(status, private_body, &request(), 1_000_000.).unwrap_err();
            assert_eq!(error.code, "PLAYER_HTTP_ERROR");
            assert_eq!(error.stage, "player");
            assert_eq!(error.http_status, Some(status));
            let safe = serde_json::to_value(error).unwrap();
            assert_eq!(safe["httpStatus"], status);
            assert_eq!(safe.as_object().unwrap().len(), 3);
            for forbidden in ["private-response", "private.invalid", "private-token"] {
                assert!(!safe.to_string().contains(forbidden));
                assert!(!format!("{error:?}").contains(forbidden));
            }
        }
        let malformed =
            select_player_response(200, private_body, &request(), 1_000_000.).unwrap_err();
        assert_eq!(malformed.code, "INVALID_RESPONSE");
        assert_eq!(malformed.http_status, Some(200));
        let mut wrong_id = body();
        wrong_id["videoDetails"]["videoId"] = json!("aqz-KE-bpKQ");
        let mismatch =
            select_player_response(200, &wrong_id.to_string(), &request(), 1_000_000.).unwrap_err();
        assert_eq!(mismatch.code, "RECORDING_MISMATCH");
        assert_eq!(mismatch.http_status, Some(200));
        let no_response = fail("NETWORK_FAILED", "player");
        assert!(no_response.http_status.is_none());
        assert!(serde_json::to_value(no_response)
            .unwrap()
            .get("httpStatus")
            .is_none());
    }

    #[test]
    fn visitor_failure_status_and_stage_belong_to_the_actual_response() {
        let denied = parse_visitor_response(403, "private-response", VisitorSource::ServiceWorker)
            .err()
            .unwrap();
        assert_eq!(denied.code, "VISITOR_CONTEXT_UNAVAILABLE");
        assert_eq!(denied.stage, "visitorServiceWorker");
        assert_eq!(denied.http_status, Some(403));
        // The anonymous homepage fallback can still succeed after that denial.
        let homepage = "{\"VISITOR_DATA\":\"synthetic-private-visitor\"}";
        let visitor = parse_visitor_response(200, homepage, VisitorSource::Homepage).unwrap();
        assert_eq!(visitor.0, "synthetic-private-visitor");
        for (status, expected) in [(200, Some(200)), (429, Some(429)), (503, Some(503))] {
            let error =
                parse_visitor_response(status, "private-invalid-body", VisitorSource::Homepage)
                    .err()
                    .unwrap();
            assert_eq!(error.stage, "visitorHomepage");
            assert_eq!(error.http_status, expected);
            let safe = serde_json::to_string(&error).unwrap();
            assert!(!safe.contains("private-invalid-body"));
            assert!(!safe.contains("synthetic-private-visitor"));
        }
    }

    #[test]
    fn fresh_player_identity_is_retained_privately_without_new_proof_claims() {
        let mut response = body();
        let title = "private-title https://private.invalid/?token=private-token";
        let channel = "private-channel-name";
        let channel_id = "private-channel-id";
        response["videoDetails"]["title"] = json!(title);
        response["videoDetails"]["author"] = json!(channel);
        response["videoDetails"]["channelId"] = json!(channel_id);
        let selected =
            select_player_response(200, &response.to_string(), &request(), 1_000_000.).unwrap();
        assert_eq!(selected.fresh_identity.title, title);
        assert_eq!(selected.fresh_identity.channel_name, channel);
        assert_eq!(
            selected.fresh_identity.channel_id.as_deref(),
            Some(channel_id)
        );
        let safe = serde_json::to_string(&selected.report).unwrap();
        let debug = format!("{selected:?}");
        for forbidden in [
            title,
            channel,
            channel_id,
            "private-signature",
            "googlevideo.com",
        ] {
            assert!(!safe.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }
        let (audio, _) = selected.into_preview_audio();
        assert!(
            audio.resolved_title.is_none(),
            "existing diagnostic handoff stays unchanged"
        );
        assert!(audio.source_metadata.is_none());
        assert!(audio.chapters.is_none());
        let absent = select_audio(&body(), &request(), 1_000_000.).unwrap();
        assert!(absent.fresh_identity.channel_id.is_none());
    }

    #[test]
    fn explicit_adapter_keeps_fresh_id_title_unknown_proof_and_minimum_expiry() {
        let now = 1_000_000.;
        for (url_expiry, expected_expiry) in [(Some(1075), 1_075_000.), (None, 1_120_000.)] {
            let mut response = body();
            response["videoDetails"]["title"] = json!("Fresh player title");
            response["videoDetails"]["channelId"] = json!("private-channel-id");
            response["streamingData"]["expiresInSeconds"] = json!("120");
            let query = url_expiry.map_or("sig=private-signature".into(), |expiry| {
                format!("expire={expiry}&sig=private-signature")
            });
            response["streamingData"]["adaptiveFormats"][0]["url"] = json!(format!(
                "https://r1---fixture.googlevideo.com/videoplayback?{query}"
            ));
            let selected = select_audio(&response, &request(), now).unwrap();
            let direct = selected.into_explicit_audio();
            assert_eq!(direct.valid_until_unix_ms, expected_expiry);
            assert_eq!(
                direct.audio.resolved_provider_id.as_deref(),
                Some("uZGRATckoBM")
            );
            assert_eq!(
                direct.audio.resolved_title.as_deref(),
                Some("Fresh player title")
            );
            assert_eq!(direct.audio.selected_format_id.as_deref(), Some("251"));
            assert_eq!(direct.audio.codec.as_deref(), Some("opus"));
            assert!(!direct.audio.selected_format_has_video);
            assert!(direct.audio.source_metadata.is_none());
            assert!(direct.audio.provider_session.is_none());
            assert!(direct.audio.chapters.is_none());
            let safe = format!("{direct:?}");
            for private in [
                "Fresh player title",
                "private-channel-id",
                "private-signature",
                "googlevideo.com",
            ] {
                assert!(!safe.contains(private));
            }
        }
    }

    #[tokio::test]
    async fn explicit_wrapper_rejects_invalid_request_and_cancellation_before_network() {
        let (_stop, cancel) = watch::channel(false);
        for id in [
            "",
            "bad-id",
            "https://www.youtube.com/watch?v=uZGRATckoBM",
            "uZGRATckoB!",
        ] {
            let error = resolve_explicit(id, 1000, cancel.clone())
                .await
                .unwrap_err();
            assert_eq!(error.code, "INVALID_PLAYBACK_REQUEST");
            assert_eq!(error.stage, "request");
            assert!(error.http_status.is_none());
        }
        for budget in [0, 20_001, u64::MAX] {
            let error = resolve_explicit("uZGRATckoBM", budget, cancel.clone())
                .await
                .unwrap_err();
            assert_eq!(error.code, "INVALID_BUDGET");
            assert!(error.http_status.is_none());
        }
        let (_stop, cancelled) = watch::channel(true);
        let error = resolve_explicit("uZGRATckoBM", 1000, cancelled)
            .await
            .unwrap_err();
        assert_eq!(error.code, "CANCELLED");
        assert_eq!(error.stage, "request");
        assert!(error.http_status.is_none());
        let (stop, closed) = watch::channel(false);
        drop(stop);
        assert_eq!(
            resolve_explicit("uZGRATckoBM", 1000, closed)
                .await
                .unwrap_err()
                .code,
            "CANCELLED"
        );
    }
    #[test]
    fn exact_profile_and_visitor_request_goldens() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../scripts/performance/innertube/fixtures/visionos-01-source-request.json"
        ))
        .unwrap();
        let req: Request = serde_json::from_value(golden["request"].clone()).unwrap();
        let spec = player_spec(&req, None).unwrap();
        assert_eq!(
            json!({"endpoint":spec.endpoint,"headers":spec.headers,"body":spec.body}),
            golden["expected"]
        );
        let visitor: Value = serde_json::from_str(include_str!("../../scripts/performance/innertube/fixtures/visionos-01-anonymous-visitor-source.json")).unwrap();
        let req: Request = serde_json::from_value(visitor["request"].clone()).unwrap();
        for (i, source) in [VisitorSource::ServiceWorker, VisitorSource::Homepage]
            .into_iter()
            .enumerate()
        {
            let spec = visitor_spec(&req, source);
            assert_eq!(spec.endpoint, visitor["expectedLookups"][i]["endpoint"]);
            assert_eq!(
                json!(spec.headers),
                visitor["expectedLookups"][i]["headers"]
            );
        }
        let secret = AnonymousVisitor("synthetic_fixture_visitor_only".into());
        let spec = player_spec(&req, Some(&secret)).unwrap();
        assert_eq!(
            spec.headers["X-Goog-Visitor-Id"],
            spec.body.as_ref().unwrap()["context"]["client"]["visitorData"]
        );
        assert!(spec
            .headers
            .keys()
            .all(|key| !["Cookie", "Authorization", "X-Goog-AuthUser"].contains(&key.as_str())));
    }
    #[test]
    fn strict_request_validation_and_locale() {
        let mut req = request();
        req.hl = "fr_CA".into();
        req.gl = "CA".into();
        assert_eq!(req.accept_language(), "fr-CA,fr;q=0.9");
        for patch in [
            json!({"cookie":"private"}),
            json!({"visitorData":"private"}),
            json!({"accountIndex":0}),
        ] {
            let mut value=serde_json::to_value(json!({"videoId":"uZGRATckoBM","expectedVideoId":"uZGRATckoBM","hl":"en","gl":"US"})).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
        req = request();
        req.expected_video_id = "abcdefghijk".into();
        assert_eq!(req.validate().unwrap_err().code, "INVALID_PLAYBACK_REQUEST");
        req = request();
        req.budget_ms = 20_001;
        assert_eq!(req.validate().unwrap_err().code, "INVALID_BUDGET");
        req = request();
        req.format_id = 140;
        assert!(req.validate().is_err());
    }
    #[test]
    fn visitor_parsers_preserve_source_shape_and_bound_values() {
        for text in [
            sw(json!("synthetic_visitor")),
            sw(json!("synthetic_visitor"))
                .split_once('\n')
                .unwrap()
                .1
                .to_owned(),
        ] {
            assert_eq!(
                parse_visitor(&text, VisitorSource::ServiceWorker)
                    .unwrap()
                    .0,
                "synthetic_visitor"
            );
        }
        for key in ["VISITOR_DATA", "visitorData"] {
            assert_eq!(
                parse_visitor(
                    &format!("{{\"{key}\" : \"synthetic_visitor\"}}"),
                    VisitorSource::Homepage
                )
                .unwrap()
                .0,
                "synthetic_visitor"
            );
        }
        for value in [
            Value::Null,
            json!(123),
            json!(""),
            json!(" "),
            json!("a\r\nb"),
            json!("x".repeat(4097)),
        ] {
            assert!(parse_visitor(&sw(value), VisitorSource::ServiceWorker).is_none());
        }
        assert!(parse_visitor(
            "{\"visitorData\":\"wrong-shape\"}",
            VisitorSource::ServiceWorker
        )
        .is_none());
        assert!(parse_visitor("{\"otherId\":\"private\"}", VisitorSource::Homepage).is_none());
    }
    #[test]
    fn selected_stream_is_private_and_not_catalog_proof() {
        let mut req = request();
        req.expected_metadata = Some(ExpectedMetadata {
            title_needle: "fixture".into(),
            channel_needle: "fixture author".into(),
            duration_seconds: 260.0,
        });
        let selected = select_audio(&body(), &req, 1_000_000.0).unwrap();
        assert_eq!(selected.report.metadata_matched, Some(true));
        assert!(!selected.report.qualification);
        assert_eq!(
            selected
                .url
                .query_pairs()
                .find(|(k, _)| k == "cpn")
                .unwrap()
                .1
                .len(),
            16
        );
        let safe = format!(
            "{selected:?} {}",
            serde_json::to_string(&selected.report).unwrap()
        );
        for secret in [
            "private-signature",
            "googlevideo.com",
            "Ｆｉｘｔｕｒｅ",
            "Fixture author",
        ] {
            assert!(!safe.contains(secret));
        }
    }

    #[test]
    fn otherwise_valid_vod_metadata_rejects_live_upcoming_and_hls_only_markers() {
        for key in ["isLiveContent", "isLive", "isUpcoming"] {
            let mut response = body();
            response["videoDetails"][key] = json!(true);
            let error = select_player_response(200, &response.to_string(), &request(), 1_000_000.)
                .unwrap_err();
            assert_eq!(error.code, "LIVE_OR_UPCOMING_SOURCE", "marker {key}");
            assert_eq!(error.stage, "parse");
            assert_eq!(error.http_status, Some(200));
        }
        let mut live_streamability = body();
        live_streamability["playabilityStatus"]["liveStreamability"] = json!({});
        assert_eq!(
            select_audio(&live_streamability, &request(), 1_000_000.)
                .unwrap_err()
                .code,
            "LIVE_OR_UPCOMING_SOURCE"
        );

        let mut hls_only = body();
        hls_only["streamingData"]["adaptiveFormats"] = json!([]);
        hls_only["streamingData"]["hlsManifestUrl"] = json!("https://private.invalid/live.m3u8");
        assert_eq!(
            select_audio(&hls_only, &request(), 1_000_000.)
                .unwrap_err()
                .code,
            "FORMAT_UNAVAILABLE"
        );

        let mut vod = body();
        for key in ["isLiveContent", "isLive", "isUpcoming"] {
            vod["videoDetails"][key] = json!(false);
        }
        vod["playabilityStatus"]["liveStreamability"] = Value::Null;
        vod["streamingData"]["serverAbrStreamingUrl"] = Value::Null;
        assert!(select_audio(&vod, &request(), 1_000_000.).is_ok());
    }

    #[test]
    fn direct_opus_url_can_coexist_with_sabr_hls_siblings_without_selecting_them() {
        let mut response = body();
        response["streamingData"]["serverAbrStreamingUrl"] =
            json!("https://private.invalid/?token=private-abr");
        response["streamingData"]["hlsManifestUrl"] =
            json!("https://private.invalid/?token=private-hls");
        let selected = select_audio(&response, &request(), 1_000_000.).unwrap();
        assert_eq!(selected.report.format_id, 251);
        assert_eq!(
            selected.url.host_str(),
            Some("r1---fixture.googlevideo.com")
        );
        assert_eq!(selected.url.path(), "/videoplayback");

        let mut only_sabr = response.clone();
        only_sabr["streamingData"]["adaptiveFormats"] = json!([]);
        only_sabr["streamingData"]
            .as_object_mut()
            .unwrap()
            .remove("hlsManifestUrl");
        assert_eq!(
            select_audio(&only_sabr, &request(), 1_000_000.)
                .unwrap_err()
                .code,
            "FORMAT_UNAVAILABLE"
        );

        let mut missing_direct = response.clone();
        missing_direct["streamingData"]["adaptiveFormats"][0]["url"] = Value::Null;
        assert_eq!(
            select_audio(&missing_direct, &request(), 1_000_000.)
                .unwrap_err()
                .code,
            "UNSUPPORTED_TRANSPORT"
        );

        // Even a URL that otherwise has the expected host/path is refused if
        // it is exactly the transport advertised as SABR or an HLS manifest.
        for sibling in ["serverAbrStreamingUrl", "hlsManifestUrl"] {
            let mut substituted = response.clone();
            substituted["streamingData"][sibling] =
                substituted["streamingData"]["adaptiveFormats"][0]["url"].clone();
            let error = select_audio(&substituted, &request(), 1_000_000.).unwrap_err();
            assert_eq!(error.code, "UNSUPPORTED_TRANSPORT");
            assert_eq!(error.stage, "format");
            assert!(!serde_json::to_string(&error).unwrap().contains("private-"));
        }

        // The sibling correction never permits a ciphered or n-bearing row.
        for cipher in ["cipher", "signatureCipher"] {
            let mut invalid = response.clone();
            invalid["streamingData"]["adaptiveFormats"][0][cipher] = json!("private-cipher");
            assert_eq!(
                select_audio(&invalid, &request(), 1_000_000.)
                    .unwrap_err()
                    .code,
                "CIPHER_REQUIRED"
            );
        }
        let mut n_required = response;
        let raw = n_required["streamingData"]["adaptiveFormats"][0]["url"]
            .as_str()
            .unwrap();
        n_required["streamingData"]["adaptiveFormats"][0]["url"] =
            json!(format!("{raw}&n=private-n"));
        assert_eq!(
            select_audio(&n_required, &request(), 1_000_000.)
                .unwrap_err()
                .code,
            "N_TRANSFORM_REQUIRED"
        );
    }
    #[test]
    fn player_rejections_are_typed_and_never_select_an_alternate() {
        let cases: Vec<(&str, fn(&mut Value))> = vec![
            ("RECORDING_MISMATCH", |b| {
                b["videoDetails"]["videoId"] = json!("abcdefghijk")
            }),
            ("METADATA_MISSING", |b| {
                b["videoDetails"]["author"] = Value::Null
            }),
            ("PLAYABILITY_REJECTED", |b| {
                b["playabilityStatus"]["status"] = json!("LOGIN_REQUIRED")
            }),
            ("FORMAT_UNAVAILABLE", |b| {
                b["streamingData"]["adaptiveFormats"][0]["itag"] = json!(140)
            }),
            ("CIPHER_REQUIRED", |b| {
                b["streamingData"]["adaptiveFormats"][0]["signatureCipher"] = json!("private")
            }),
            ("CIPHER_REQUIRED", |b| {
                b["streamingData"]["adaptiveFormats"][0]["cipher"] = json!(true)
            }),
            ("FORMAT_LENGTH_MISSING", |b| {
                b["streamingData"]["adaptiveFormats"][0]["contentLength"] =
                    json!("9007199254740992")
            }),
            ("FORMAT_DURATION_MISSING", |b| {
                b["streamingData"]["adaptiveFormats"][0]["approxDurationMs"] = Value::Null
            }),
            ("PREVIEW_OR_TRUNCATED_FORMAT", |b| {
                b["streamingData"]["adaptiveFormats"][0]["approxDurationMs"] = json!("30000")
            }),
            ("UNSUPPORTED_TRANSPORT", |b| {
                b["streamingData"]["adaptiveFormats"][0]["url"] = Value::Null
            }),
            ("MANIFEST_EXPIRED", |b| {
                b["streamingData"]["expiresInSeconds"] = json!(1);
            }),
            ("INVALID_MEDIA_URL", |b| {
                b["streamingData"]["adaptiveFormats"][0]["url"] =
                    json!("https://googlevideo.com.evil.test/videoplayback")
            }),
        ];
        for (code, mutate) in cases {
            let mut b = body();
            mutate(&mut b);
            assert_eq!(
                select_audio(&b, &request(), 1_000_000.0).unwrap_err().code,
                code
            );
        }
        for query in [
            "&n=private",
            "&%6e=private",
            "&n%3Dprivate",
            "%26n%3Dprivate",
        ] {
            let mut b = body();
            let url = b["streamingData"]["adaptiveFormats"][0]["url"]
                .as_str()
                .unwrap()
                .to_owned()
                + query;
            b["streamingData"]["adaptiveFormats"][0]["url"] = json!(url);
            assert_eq!(
                select_audio(&b, &request(), 1_000_000.0).unwrap_err().code,
                "N_TRANSFORM_REQUIRED"
            );
        }
        let mut b = body();
        b["streamingData"]["adaptiveFormats"][0]["url"] =
            json!("https://fixture.googlevideo.com/videoplayback?n%3Dprivate");
        assert_eq!(
            select_audio(&b, &request(), 1_000_000.0).unwrap_err().code,
            "N_TRANSFORM_REQUIRED"
        );
        let mut b = body();
        let format = b["streamingData"]["adaptiveFormats"][0].clone();
        b["streamingData"]["adaptiveFormats"]
            .as_array_mut()
            .unwrap()
            .push(format);
        assert_eq!(
            select_audio(&b, &request(), 1_000_000.0).unwrap_err().code,
            "FORMAT_MISMATCH"
        );
    }
    #[tokio::test]
    async fn shared_absolute_deadline_and_cancellation_are_terminal() {
        let (stop, cancel) = watch::channel(false);
        let budget = Budget {
            deadline: Instant::now() + Duration::from_millis(15),
            cancel,
        };
        assert_eq!(
            budget
                .run("visitorServiceWorker", std::future::pending::<Result<()>>())
                .await
                .unwrap_err()
                .code,
            "TIMEOUT"
        );
        assert_eq!(
            budget
                .run("player", async { Ok(()) })
                .await
                .unwrap_err()
                .code,
            "TIMEOUT"
        );
        let budget = Budget {
            deadline: Instant::now() + Duration::from_secs(5),
            cancel: stop.subscribe(),
        };
        stop.send(true).unwrap();
        assert_eq!(
            budget
                .run::<()>("player", async { panic!("cancelled stage executed") })
                .await
                .unwrap_err()
                .code,
            "CANCELLED"
        );
    }
    #[tokio::test]
    async fn production_access_bridge_rejects_private_destination_without_decode() {
        let selected = select_audio(&body(), &request(), 1_000_000.0).unwrap();
        let (mut audio, _) = selected.into_preview_audio();
        assert!(audio.source_metadata.is_none());
        assert!(audio.resolved_title.is_none());
        assert!(audio.chapters.is_none());
        audio.url = "https://127.0.0.1/videoplayback".into();
        let (_stop, cancel) = watch::channel(false);
        assert!(
            crate::acquisition::diagnostic_validate_prepared_preview_audio(
                audio,
                cancel,
                crate::playback_sources::PlaybackDeadline::after(Duration::from_secs(1))
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn reused_http_client_bounds_body_and_does_not_follow_redirects() {
        use axum::{body::Body, http::StatusCode, routing::get, Router};
        let app = Router::new()
            .route(
                "/redirect",
                get(|| async { (StatusCode::FOUND, [("location", "/must-not-fetch")]) }),
            )
            .route(
                "/must-not-fetch",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route("/large", get(|| async { "x".repeat(MAX_BODY + 1) }))
            .route(
                "/stream-large",
                get(|| async {
                    Body::from_stream(futures_util::stream::iter([
                        Ok::<_, std::convert::Infallible>(vec![b'x'; MAX_BODY / 2 + 1]),
                        Ok(vec![b'x'; MAX_BODY / 2 + 1]),
                    ]))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let experiment = DirectExperiment::new().unwrap();
        let (_stop, cancel) = watch::channel(false);
        let budget = Budget {
            deadline: Instant::now() + Duration::from_secs(5),
            cancel,
        };
        let spec = |path: &str| Spec {
            endpoint: format!("http://{address}{path}"),
            headers: BTreeMap::new(),
            body: None,
        };
        assert_eq!(
            experiment
                .fetch(spec("/redirect"), &budget, "player")
                .await
                .unwrap()
                .0,
            302
        );
        for path in ["/large", "/stream-large"] {
            assert_eq!(
                experiment
                    .fetch(spec(path), &budget, "parse")
                    .await
                    .unwrap_err()
                    .code,
                "RESPONSE_TOO_LARGE"
            );
        }
        server.abort();
    }
    #[tokio::test]
    #[ignore = "explicit public-network opt-in; metadata only, no decode or catalog proof"]
    async fn live_three_public_ids_metadata_only() {
        assert_eq!(
            std::env::var("BURRATA_NATIVE_DIRECT_LIVE").as_deref(),
            Ok("1"),
            "requires explicit live opt-in"
        );
        let corpus: Value = serde_json::from_str(include_str!(
            "../../scripts/performance/innertube/fixtures/transport-music-tracks.json"
        ))
        .unwrap();
        let experiment = DirectExperiment::new().unwrap();
        let mut succeeded = 0;
        let outer_deadline = Instant::now() + Duration::from_secs(58);
        for row in corpus.as_array().unwrap() {
            let req:Request=serde_json::from_value(json!({"videoId":row["videoId"],"expectedVideoId":row["videoId"],"hl":"en","gl":"US",
                "visitorMode":"anonymous-source","expectedMetadata":{"titleNeedle":row["titleNeedle"],"channelNeedle":row["channelNeedle"],"durationSeconds":row["durationSeconds"]}})).unwrap();
            let (_stop, cancel) = watch::channel(false);
            let started = Instant::now();
            match experiment
                .resolve(
                    &req,
                    outer_deadline.min(started + Duration::from_secs(19)),
                    cancel,
                )
                .await
            {
                Ok(selected) => {
                    eprintln!(
                        "{}",
                        json!({"experiment":"native-direct-metadata-v1","ok":true,
                        "metadata":selected.report,"decodeAttempted":false})
                    );
                    succeeded += 1;
                }
                Err(error) => eprintln!(
                    "{}",
                    json!({"experiment":"native-direct-metadata-v1","ok":false,
                    "videoId":req.video_id,"failure":error.code,"stage":error.stage,"httpStatus":error.http_status,
                    "resolutionMs":started.elapsed().as_secs_f64()*1000.0,"decodeAttempted":false,"qualification":false})
                ),
            }
        }
        assert_eq!(
            succeeded, 3,
            "failures retained above; metadata capability is not decoding qualification"
        );
    }

    #[tokio::test]
    #[ignore = "explicit public-network opt-in; existing access checks and decoder PCM, no device/catalog proof"]
    async fn live_three_public_ids_existing_decoder_pcm() {
        assert_eq!(
            std::env::var("BURRATA_NATIVE_DIRECT_PCM_LIVE").as_deref(),
            Ok("1"),
            "requires explicit PCM live opt-in"
        );
        let corpus: Value = serde_json::from_str(include_str!(
            "../../scripts/performance/innertube/fixtures/transport-music-tracks.json"
        ))
        .unwrap();
        let experiment = DirectExperiment::new().unwrap();
        let outer_deadline = Instant::now() + Duration::from_secs(58);
        let ffmpeg = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/tools")
            .join(crate::tools::target_triple())
            .join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
        assert!(ffmpeg.is_file(), "bundled decoder asset is required");
        let mut succeeded = 0;
        for row in corpus.as_array().unwrap() {
            let request: Request = serde_json::from_value(json!({"videoId":row["videoId"],"expectedVideoId":row["videoId"],"hl":"en","gl":"US",
                "visitorMode":"anonymous-source","expectedMetadata":{"titleNeedle":row["titleNeedle"],"channelNeedle":row["channelNeedle"],"durationSeconds":row["durationSeconds"]}})).unwrap();
            let started = std::time::Instant::now();
            let deadline = outer_deadline.min(Instant::now() + Duration::from_secs(19));
            let (_stop, cancel) = watch::channel(false);
            let work = async {
                let selected = experiment
                    .resolve(&request, deadline, cancel.clone())
                    .await?;
                let (audio, metadata) = selected.into_preview_audio();
                let access_started = std::time::Instant::now();
                let audio = crate::acquisition::diagnostic_validate_prepared_preview_audio(
                    audio,
                    cancel.clone(),
                    crate::playback_sources::PlaybackDeadline::at(deadline),
                )
                .await
                .map_err(|error| {
                    let code = if *cancel.borrow() || error == "Cancelled" {
                        "CANCELLED"
                    } else if Instant::now() >= deadline {
                        "TIMEOUT"
                    } else {
                        "ACCESS_VALIDATION_FAILED"
                    };
                    fail(code, "access")
                })?;
                let access_ms = access_started.elapsed().as_secs_f64() * 1000.0;
                let source = crate::native_output::AudioSource {
                    input: audio.url,
                    ffmpeg: ffmpeg.clone(),
                    headers: audio.headers,
                    decoder_hint: Some(crate::native_output::AudioSourceDecoderHint {
                        mime_type: Some(audio.mime_type),
                        codec: audio.codec,
                        duration_seconds: audio.duration_seconds,
                        content_length: audio.expected_content_length,
                        has_video: false,
                    }),
                };
                let before_decode_ms = started.elapsed().as_secs_f64() * 1000.0;
                let pcm = crate::native_output::direct_pcm_experiment::collect_until(
                    source,
                    0.0,
                    5.0,
                    deadline.into_std(),
                    cancel,
                )
                .await;
                let pcm = serde_json::to_value(pcm)
                    .map_err(|_| fail("INVALID_DECODER_DIAGNOSTICS", "decode"))?;
                let ok = pcm["ok"] == true
                    && pcm["decodedFrames"]
                        .as_u64()
                        .is_some_and(|frames| frames >= 240_000);
                let first_pcm_ms = pcm["firstPcmMs"]
                    .as_f64()
                    .map(|first| first + before_decode_ms);
                Ok::<_, DirectFailure>(
                    json!({"experiment":"native-direct-existing-decoder-v1","ok":ok,"metadata":metadata,
                    "accessMs":access_ms,"pcm":pcm,"firstPcmFromRequestMs":first_pcm_ms,
                    "wallMs":started.elapsed().as_secs_f64()*1000.0,"qualification":false,
                    "scope":"explicit public provider ID; production access validation and existing decoder PCM only; no mixer/device/catalog/acoustic proof"}),
                )
            };
            match work.await {
                Ok(report) => {
                    if report["ok"] == true {
                        succeeded += 1;
                    }
                    eprintln!("{report}");
                }
                Err(error) => eprintln!(
                    "{}",
                    json!({"experiment":"native-direct-existing-decoder-v1","ok":false,
                    "videoId":request.video_id,"failure":error.code,"stage":error.stage,"httpStatus":error.http_status,
                    "wallMs":started.elapsed().as_secs_f64()*1000.0,"qualification":false})
                ),
            }
        }
        assert_eq!(
            succeeded, 3,
            "failures retained above; decoder PCM is not device or catalog qualification"
        );
    }
}
