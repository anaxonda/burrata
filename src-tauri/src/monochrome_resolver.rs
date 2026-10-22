//! Protocol and data model for Monochrome's unified recording resolver.
//!
//! This is deliberately separate from `monochrome::Client`, which speaks the
//! numeric Tracks catalog protocol. Resolver resources are short lived and must
//! not be written into track metadata or caches.
use crate::{
    models::{Candidate, Track},
    recording_identity::normalize_isrc,
    recording_match::{self, MatchAction, MatchAssessment, MatchLevel, MatchStage},
};
use serde_json::Value;
use std::fmt;
use std::time::Duration;

const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetQuality {
    Lossless,
    HiResLossless,
}

impl TargetQuality {
    pub fn as_api_value(self) -> &'static str {
        match self {
            Self::Lossless => "LOSSLESS",
            Self::HiResLossless => "HI_RES_LOSSLESS",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingRequest {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_seconds: Option<f64>,
    pub quality: TargetQuality,
}

impl RecordingRequest {
    /// Build the documented `/api/v2/track/` query without converting provider
    /// IDs or storing ephemeral resolver data.
    pub fn query_pairs(&self) -> Result<Vec<(&'static str, String)>, &'static str> {
        let title = bounded(&self.title, 500)?;
        if self.artists.len() > 32 {
            return Err("Resolver requests support at most 32 artists");
        }
        let artists = self
            .artists
            .iter()
            .map(|s| bounded(s, 500))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pairs = vec![
            ("track", title),
            ("intent", "stream".to_owned()),
            ("quality", self.quality.as_api_value().to_owned()),
        ];
        if !artists.is_empty() {
            pairs.push(("artist", artists.join(", ")));
        }
        if let Some(album) = self.album.as_deref() {
            pairs.push(("album", bounded(album, 500)?));
        }
        if let Some(isrc) = self.isrc.as_deref() {
            pairs.push(("isrc", bounded(isrc, 32)?));
        }
        if let Some(duration) = self.duration_seconds {
            if !duration.is_finite() || duration <= 0.0 || duration > 86_400.0 {
                return Err("Resolver duration is outside the supported range");
            }
            pairs.push(("duration", format!("{}", duration.round() as u64)));
        }
        Ok(pairs)
    }
}

fn bounded(value: &str, max: usize) -> Result<String, &'static str> {
    let v = value.trim();
    if v.is_empty() || v.chars().count() > max {
        Err("Invalid resolver request field")
    } else {
        Ok(v.to_owned())
    }
}

/// Explicit API bearer plus optional Turnstile JWT. The frontend can attach a
/// cached JWT with a custom API token and retries 401/428 with a refreshed JWT;
/// acquiring that proof belongs to an authorized session integration.
#[derive(Clone)]
pub struct AuthConfig {
    api_token: String,
    turnstile_jwt: Option<String>,
}
impl AuthConfig {
    pub fn new(api_token: String, turnstile_jwt: Option<String>) -> Self {
        Self {
            api_token,
            turnstile_jwt,
        }
    }
    pub fn authorization_header(&self) -> Result<String, &'static str> {
        safe_header_token(&self.api_token).map(|t| format!("Bearer {t}"))
    }
    pub fn turnstile_header(&self) -> Result<Option<String>, &'static str> {
        self.turnstile_jwt
            .as_deref()
            .map(|t| safe_header_token(t).map(str::to_owned))
            .transpose()
    }
}
impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("api_token", &"[REDACTED]")
            .field(
                "turnstile_jwt",
                &self.turnstile_jwt.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}
impl Drop for AuthConfig {
    fn drop(&mut self) {
        fn clear(value: &mut String) {
            unsafe {
                for byte in value.as_bytes_mut() {
                    std::ptr::write_volatile(byte, 0);
                }
            }
        }
        clear(&mut self.api_token);
        if let Some(jwt) = &mut self.turnstile_jwt {
            clear(jwt);
        }
    }
}
fn safe_header_token(token: &str) -> Result<&str, &'static str> {
    if token.is_empty() || token.len() > 16_384 || token.bytes().any(|b| b.is_ascii_control()) {
        Err("Invalid Monochrome authorization value")
    } else {
        Ok(token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Direct,
    Dash,
    Hls,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptionScheme {
    CencAesCtr,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEncoding {
    Hex,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolverTrackMetadata {
    pub id: Option<String>,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_seconds: Option<f64>,
}

/// Ephemeral media decryption key with redacted diagnostics. Keep it out of persistence.
#[derive(Clone)]
pub struct EphemeralMediaKey(String);
impl EphemeralMediaKey {
    fn from_value(value: &Value) -> Option<Self> {
        let raw = value.as_str()?.trim();
        // CENC uses AES-128 here: 16 key bytes, represented as 32 hex chars.
        (raw.len() == 32 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| Self(raw.to_ascii_lowercase()))
    }
    pub fn expose_for_transport(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for EphemeralMediaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EphemeralMediaKey([REDACTED])")
    }
}
impl Drop for EphemeralMediaKey {
    fn drop(&mut self) {
        // Best-effort clear the owned decoded key string when the ephemeral
        // response/resource leaves scope. It is never serialized or logged.
        unsafe {
            for byte in self.0.as_bytes_mut() {
                std::ptr::write_volatile(byte, 0);
            }
        }
    }
}

#[derive(Clone)]
pub struct PlaybackResource {
    pub id: Option<String>,
    pub url: String,
    pub source: Option<String>,
    pub delivery: Delivery,
    pub kind: String,
    pub codec: Option<String>,
    pub quality: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub mime_type: Option<String>,
    pub container: Option<String>,
    pub lossless: Option<bool>,
    pub bitrate_kbps: Option<u32>,
    pub encryption_scheme: Option<EncryptionScheme>,
    pub key_encoding: Option<KeyEncoding>,
    pub media_key: Option<EphemeralMediaKey>,
    /// Decoded inline DASH manifest, held only in memory. Its body can contain
    /// signed media URLs, so Debug deliberately omits it.
    pub inline_dash_manifest: Option<String>,
}
impl fmt::Debug for PlaybackResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaybackResource")
            .field("url", &"[EPHEMERAL URL REDACTED]")
            .field("source", &self.source)
            .field("delivery", &self.delivery)
            .field("kind", &self.kind)
            .field("codec", &self.codec)
            .field("quality", &self.quality)
            .field("media_key", &self.media_key)
            .field("encryption_scheme", &self.encryption_scheme)
            .field(
                "inline_dash_manifest",
                &self.inline_dash_manifest.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct ResolverResponse {
    pub schema_version: Option<String>,
    /// Resolver envelope metadata; this is not independent identity proof.
    pub recording: Option<ResolverTrackMetadata>,
    pub selected_source: Option<String>,
    pub quality_requested: Option<String>,
    pub resources: Vec<PlaybackResource>,
}

impl ResolverResponse {
    /// Select only the observed end-to-end route that the native CENC decoder
    /// supports. Resolver track metadata (including its ISRC) remains echoed
    /// metadata and is intentionally not returned as independent proof.
    pub fn direct_lossless_cenc_flac(&self) -> Option<&PlaybackResource> {
        self.resources.iter().find(|resource| {
            resource.kind == "audio"
                && resource.delivery == Delivery::Direct
                && resource.lossless == Some(true)
                && resource
                    .codec
                    .as_deref()
                    .is_some_and(|codec| codec.eq_ignore_ascii_case("flac"))
                && resource.mime_type.as_deref() == Some("audio/mp4")
                && resource
                    .container
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case("mp4"))
                && resource.encryption_scheme == Some(EncryptionScheme::CencAesCtr)
                && resource.key_encoding == Some(KeyEncoding::Hex)
                && resource.media_key.is_some()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverMatchFailure {
    MissingRecordingIdentity,
    SourceBindingMismatch,
    ConflictingIsrc,
    UnsupportedResource,
    MatchRejected,
}

#[derive(Debug)]
pub struct AssessedPlaybackResource<'a> {
    pub resource: &'a PlaybackResource,
    pub candidate: Candidate,
    pub assessment: MatchAssessment,
}

/// Bind resolver metadata to the existing Monochrome catalog identity rule,
/// then require the shared resolved-stage policy to accept it. The echoed ISRC
/// is checked only for conflict and deliberately omitted from candidate proof.
pub fn assess_playback_resource<'a>(
    requested: &Track,
    response: &'a ResolverResponse,
    level: MatchLevel,
) -> Result<AssessedPlaybackResource<'a>, ResolverMatchFailure> {
    let resource = response
        .direct_lossless_cenc_flac()
        .ok_or(ResolverMatchFailure::UnsupportedResource)?;
    let recording = response
        .recording
        .as_ref()
        .ok_or(ResolverMatchFailure::MissingRecordingIdentity)?;
    let track_id = recording
        .id
        .as_deref()
        .filter(|v| safe_resolver_identifier(v, 128))
        .ok_or(ResolverMatchFailure::MissingRecordingIdentity)?;
    let title = recording
        .title
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or(ResolverMatchFailure::MissingRecordingIdentity)?;
    let source = response
        .selected_source
        .as_deref()
        .filter(|v| safe_resolver_identifier(v, 64))
        .ok_or(ResolverMatchFailure::MissingRecordingIdentity)?;
    if resource.source.as_deref() != Some(source) {
        return Err(ResolverMatchFailure::SourceBindingMismatch);
    }
    if let (Some(wanted), Some(echoed)) = (
        requested.isrc.as_deref().and_then(normalize_isrc),
        recording.isrc.as_deref().and_then(normalize_isrc),
    ) {
        if wanted != echoed {
            return Err(ResolverMatchFailure::ConflictingIsrc);
        }
    }
    if recording.artists.is_empty() || recording.artists.len() > 32 {
        return Err(ResolverMatchFailure::MissingRecordingIdentity);
    }
    let candidate = Candidate {
        id: format!("monochrome:{track_id}"),
        provider: "monochrome".into(),
        title: title.to_owned(),
        duration_seconds: recording.duration_seconds,
        source_metadata: Some(serde_json::json!({
            "provider": "monochrome",
            "resolverProtocol": "v2",
            "trackId": track_id,
            "catalogTitle": title,
            "artists": &recording.artists,
            "artistCount": recording.artists.len(),
            "resolverSource": source,
            "resolverEchoedIsrc": recording.isrc.as_deref().and_then(normalize_isrc),
        })),
        ..Candidate::default()
    };
    let assessment = recording_match::assess(requested, &candidate, level, MatchStage::Resolved);
    if assessment.action != MatchAction::Accept {
        return Err(ResolverMatchFailure::MatchRejected);
    }
    Ok(AssessedPlaybackResource {
        resource,
        candidate,
        assessment,
    })
}

/// Identifiers may be surfaced in candidate IDs, diagnostics, and quality
/// metadata. Reject URL-like or control-bearing values before they cross that
/// boundary; resolver resource URLs remain separate ephemeral data.
fn safe_resolver_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverFailure {
    Unauthorized,
    Forbidden,
    VerificationRequired,
    RateLimited,
    NotFound,
    Http(u16),
    InvalidRequest,
    InvalidAuth,
    Transport,
    ResponseTooLarge,
    InvalidResponse,
}

pub fn classify_status(status: u16) -> Result<(), ResolverFailure> {
    match status {
        200..=299 => Ok(()),
        401 => Err(ResolverFailure::Unauthorized),
        403 => Err(ResolverFailure::Forbidden),
        428 => Err(ResolverFailure::VerificationRequired),
        429 => Err(ResolverFailure::RateLimited),
        404 => Err(ResolverFailure::NotFound),
        n => Err(ResolverFailure::Http(n)),
    }
}

impl ResolverFailure {
    /// Indicates that the caller may obtain a fresh Turnstile proof through an
    /// authorized browser flow and explicitly retry once.
    pub fn needs_turnstile_refresh(&self) -> bool {
        matches!(self, Self::Unauthorized | Self::VerificationRequired)
    }
}

/// Explicit resolver endpoint and HTTP transport. No default host, API token,
/// browser profile, cookie jar, or session is discovered here. Redirects are
/// disabled so a bearer header cannot be forwarded to another origin.
#[derive(Clone)]
pub struct ResolverClient {
    http: reqwest::Client,
    endpoint: url::Url,
}
impl fmt::Debug for ResolverClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolverClient")
            .field("endpoint", &self.endpoint.origin().ascii_serialization())
            .finish_non_exhaustive()
    }
}
impl ResolverClient {
    pub fn new(endpoint: &str) -> Result<Self, &'static str> {
        let endpoint =
            url::Url::parse(endpoint).map_err(|_| "Invalid Monochrome resolver endpoint")?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !matches!(endpoint.path(), "" | "/")
        {
            return Err("Resolver endpoint must be a bare HTTPS origin");
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| "Could not initialize Monochrome resolver transport")?;
        Ok(Self { http, endpoint })
    }

    /// Issue one lookup. Errors contain only stable categories/status codes;
    /// response bodies, URLs and credentials are never included.
    pub async fn resolve(
        &self,
        request: &RecordingRequest,
        auth: &AuthConfig,
    ) -> Result<ResolverResponse, ResolverFailure> {
        let query = request
            .query_pairs()
            .map_err(|_| ResolverFailure::InvalidRequest)?;
        let mut url = self
            .endpoint
            .join("/api/v2/track/")
            .map_err(|_| ResolverFailure::InvalidResponse)?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let mut builder = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::AUTHORIZATION,
                auth.authorization_header()
                    .map_err(|_| ResolverFailure::InvalidAuth)?,
            );
        if let Some(jwt) = auth
            .turnstile_header()
            .map_err(|_| ResolverFailure::InvalidAuth)?
        {
            builder = builder.header("X-Turnstile-JWT", jwt);
        }
        crate::diagnostic_metrics::record_http_send_attempt();
        let mut response = builder
            .send()
            .await
            .map_err(|_| ResolverFailure::Transport)?;
        let status = response.status().as_u16();
        if let Err(error) = classify_status(status) {
            return Err(error);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ResolverFailure::Transport)?
        {
            if body.len() + chunk.len() > RESPONSE_LIMIT {
                return Err(ResolverFailure::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| ResolverFailure::InvalidResponse)?;
        parse_response(&value)
    }

    /// Retry once only when caller supplies a separately refreshed auth value.
    /// This method does not obtain or refresh credentials itself.
    pub async fn resolve_with_refreshed_auth(
        &self,
        request: &RecordingRequest,
        auth: &AuthConfig,
        refreshed_auth: Option<&AuthConfig>,
    ) -> Result<ResolverResponse, ResolverFailure> {
        match self.resolve(request, auth).await {
            Err(error) if error.needs_turnstile_refresh() => match refreshed_auth {
                Some(fresh) => self.resolve(request, fresh).await,
                None => Err(error),
            },
            result => result,
        }
    }
}

fn parse_data_dash(uri: &str) -> Option<String> {
    const PREFIX: &str = "data:application/dash+xml";
    let rest = uri.strip_prefix(PREFIX)?;
    let (metadata, payload) = rest
        .strip_prefix(',')
        .map(|payload| ("", payload))
        .or_else(|| {
            let after_semicolon = rest.strip_prefix(';')?;
            let (meta, payload) = after_semicolon.split_once(',')?;
            Some((meta, payload))
        })?;
    let bytes = if metadata
        .split(';')
        .any(|token| token.eq_ignore_ascii_case("base64"))
    {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?
    } else {
        percent_decode(payload)?
    };
    if bytes.len() > RESPONSE_LIMIT {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    (text.trim_start().starts_with("<MPD") || text.trim_start().starts_with("<?xml"))
        .then_some(text)
}

fn percent_decode(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            out.push((hex(hi)? << 4) | hex(lo)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse a resolver response. URL and media key are held only in the returned
/// short-lived resource; callers must not put this object in caches or metadata.
pub fn parse_response(data: &Value) -> Result<ResolverResponse, ResolverFailure> {
    let schema = data
        .get("schema_version")
        .and_then(value_text)
        .ok_or(ResolverFailure::InvalidResponse)?;
    if !matches!(schema.split('.').next(), Some("1" | "2")) {
        return Err(ResolverFailure::InvalidResponse);
    }
    let raw_resources = data
        .get("playback")
        .and_then(Value::as_array)
        .ok_or(ResolverFailure::InvalidResponse)?;
    let mut resources = Vec::new();
    for raw in raw_resources {
        let kind = raw.get("kind").and_then(Value::as_str).unwrap_or("");
        let delivery_text = raw
            .get("delivery")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(kind, "audio" | "manifest")
            || !matches!(delivery_text.as_str(), "direct" | "dash" | "hls")
        {
            continue;
        }
        let url = raw
            .get("url")
            .and_then(Value::as_str)
            .ok_or(ResolverFailure::InvalidResponse)?;
        let is_inline_dash = kind == "manifest"
            && delivery_text == "dash"
            && url.starts_with("data:application/dash+xml");
        let parsed = url::Url::parse(url).map_err(|_| ResolverFailure::InvalidResponse)?;
        if (!is_inline_dash && (parsed.scheme() != "https" || parsed.host_str().is_none()))
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(ResolverFailure::InvalidResponse);
        }
        let codec = raw.get("codec").and_then(Value::as_str).map(str::to_owned);
        let encryption_scheme = raw
            .pointer("/encryption/scheme")
            .and_then(Value::as_str)
            .map(|v| {
                if v == "cenc-aes-ctr" {
                    EncryptionScheme::CencAesCtr
                } else {
                    EncryptionScheme::Unknown
                }
            });
        let key_encoding = raw
            .pointer("/encryption/key/encoding")
            .and_then(Value::as_str)
            .map(|v| {
                if v == "hex" {
                    KeyEncoding::Hex
                } else {
                    KeyEncoding::Unknown
                }
            });
        // Only accept the observed nested CENC representation. Unknown legacy
        // shapes are metadata, never authority to decrypt a resource.
        let key = if encryption_scheme == Some(EncryptionScheme::CencAesCtr)
            && key_encoding == Some(KeyEncoding::Hex)
        {
            raw.pointer("/encryption/key/value")
                .and_then(EphemeralMediaKey::from_value)
        } else {
            None
        };
        let inline_dash_manifest = if is_inline_dash {
            Some(parse_data_dash(url).ok_or(ResolverFailure::InvalidResponse)?)
        } else {
            None
        };
        let delivery = match delivery_text.as_str() {
            "direct" => Delivery::Direct,
            "dash" => Delivery::Dash,
            "hls" => Delivery::Hls,
            _ => Delivery::Unknown,
        };
        resources.push(PlaybackResource {
            id: raw.get("id").and_then(value_text),
            url: url.to_owned(),
            source: raw.get("source").and_then(Value::as_str).map(str::to_owned),
            delivery,
            kind: kind.to_owned(),
            codec,
            quality: raw
                .get("quality")
                .and_then(Value::as_str)
                .map(str::to_owned),
            bit_depth: raw
                .get("bit_depth")
                .or_else(|| raw.get("bitDepth"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            sample_rate_hz: raw
                .get("sample_rate_hz")
                .or_else(|| raw.get("sampleRateHz"))
                .or_else(|| raw.get("sample_rate"))
                .or_else(|| raw.get("sampleRate"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            channels: raw
                .get("channels")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            mime_type: raw
                .get("mime_type")
                .or_else(|| raw.get("mimeType"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            container: raw
                .get("container")
                .and_then(Value::as_str)
                .map(str::to_owned),
            lossless: raw.get("lossless").and_then(Value::as_bool),
            bitrate_kbps: raw
                .get("bitrate_kbps")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            encryption_scheme,
            key_encoding,
            media_key: key,
            inline_dash_manifest,
        });
    }
    if resources.is_empty() {
        return Err(ResolverFailure::InvalidResponse);
    }
    let recording_raw = data.get("track");
    let recording = recording_raw.map(|r| ResolverTrackMetadata {
        id: r.get("id").and_then(value_text),
        title: r
            .get("title")
            .or_else(|| r.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        artists: r
            .get("artists")
            .or_else(|| r.get("artist_names"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|artist| {
                        artist
                            .as_str()
                            .or_else(|| artist.get("name").and_then(Value::as_str))
                    })
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        album: r
            .get("album")
            .and_then(|album| {
                album.as_str().or_else(|| {
                    album
                        .get("title")
                        .or_else(|| album.get("name"))
                        .and_then(Value::as_str)
                })
            })
            .map(str::to_owned),
        isrc: r.get("isrc").and_then(Value::as_str).map(str::to_owned),
        duration_seconds: r.get("duration").and_then(Value::as_f64).or_else(|| {
            r.get("duration_ms")
                .and_then(Value::as_f64)
                .map(|ms| ms / 1000.0)
        }),
    });
    Ok(ResolverResponse {
        schema_version: Some(schema),
        recording,
        selected_source: data
            .get("selected_source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        quality_requested: data
            .get("quality_requested")
            .and_then(Value::as_str)
            .map(str::to_owned),
        resources,
    })
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_identity_query_and_quality() {
        let req = RecordingRequest {
            title: "Alvafro".into(),
            artists: vec!["LATIN MAFIA & Fred again..".into()],
            album: None,
            isrc: Some("GBAHS2600508".into()),
            duration_seconds: Some(238.05),
            quality: TargetQuality::Lossless,
        };
        let pairs = req.query_pairs().unwrap();
        assert!(pairs.contains(&("track", "Alvafro".into())));
        assert!(pairs.contains(&("artist", "LATIN MAFIA & Fred again..".into())));
        assert!(pairs.contains(&("quality", "LOSSLESS".into())));
        assert!(pairs.contains(&("duration", "238".into())));
    }

    #[test]
    fn parses_resources_separately_and_redacts_debug() {
        let response = parse_response(&json!({"schema_version":"2.0", "track":{"title":"Alvafro","artists":["Latin Mafia & Fred again.."],"isrc":"GBAHS2600508","duration_ms":null}, "playback":[{"url":"https://cdn.example/audio.mp4?sig=secret","source":"amazon","kind":"audio","delivery":"direct","codec":"flac","quality":"LOSSLESS","lossless":true,"mime_type":"audio/mp4","container":"mp4","bit_depth":16,"sample_rate_hz":44100,"bitrate_kbps":1040,"channels":2,"encryption":{"scheme":"cenc-aes-ctr","key":{"encoding":"hex","value":"0123456789abcdef0123456789abcdef"},"iv_source":"redacted"}}]})).unwrap();
        assert_eq!(
            response.recording.as_ref().unwrap().isrc.as_deref(),
            Some("GBAHS2600508")
        );
        assert_eq!(response.resources[0].delivery, Delivery::Direct);
        assert_eq!(
            response.resources[0].encryption_scheme,
            Some(EncryptionScheme::CencAesCtr)
        );
        assert_eq!(response.resources[0].bitrate_kbps, Some(1040));
        let debug = format!("{:?}", response.resources[0]);
        assert!(!debug.contains("sig=secret"));
        assert!(!debug.contains("media-secret"));
        assert!(debug.contains("REDACTED"));
        assert!(!format!("{:?}", response).contains("0123456789abcdef"));
        assert!(response.direct_lossless_cenc_flac().is_some());
    }

    #[test]
    fn echoed_isrc_is_metadata_and_unknown_key_encodings_are_not_transport_keys() {
        let good = json!({"schema_version":"2.0","track":{"isrc":"GBAHS2600508"},"playback":[{"url":"https://cdn.example/audio.mp4","kind":"audio","delivery":"direct","codec":"flac","mime_type":"audio/mp4","container":"mp4","lossless":true,"encryption":{"scheme":"cenc-aes-ctr","key":{"encoding":"hex","value":"0123456789abcdef0123456789abcdef"}}}]});
        let parsed = parse_response(&good).unwrap();
        assert_eq!(
            parsed.recording.as_ref().unwrap().isrc.as_deref(),
            Some("GBAHS2600508")
        );
        assert!(parsed.direct_lossless_cenc_flac().is_some());
        let bad = json!({"schema_version":"2.0","playback":[{"url":"https://cdn.example/a","kind":"audio","delivery":"direct","codec":"flac","mime_type":"audio/mp4","container":"mp4","lossless":true,"encryption":{"scheme":"cenc-aes-ctr","key":{"encoding":"base64","value":"0123456789abcdef0123456789abcdef"}}}]});
        let parsed = parse_response(&bad).unwrap();
        assert!(parsed.resources[0].media_key.is_none());
        assert!(parsed.direct_lossless_cenc_flac().is_none());
        let wrong_length = json!({"schema_version":"2.0","playback":[{"url":"https://cdn.example/a","kind":"audio","delivery":"direct","codec":"flac","mime_type":"audio/mp4","container":"mp4","lossless":true,"encryption":{"scheme":"cenc-aes-ctr","key":{"encoding":"hex","value":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}]});
        assert!(parse_response(&wrong_length).unwrap().resources[0]
            .media_key
            .is_none());
    }

    fn resolver_match_response(title: &str, artists: &[&str], isrc: &str) -> ResolverResponse {
        resolver_match_response_with_duration(title, artists, isrc, None)
    }

    fn resolver_match_response_with_duration(
        title: &str,
        artists: &[&str],
        isrc: &str,
        duration_ms: Option<u64>,
    ) -> ResolverResponse {
        let playback = json!({"id":"amazon:B0HCDL81N4:HD","source":"amazon","kind":"audio","delivery":"direct","url":"https://cdn.example/ephemeral.mp4?sig=hidden","codec":"flac","quality":"LOSSLESS","lossless":true,"mime_type":"audio/mp4","container":"mp4","bit_depth":16,"sample_rate_hz":44100,"channels":2,"encryption":{"scheme":"cenc-aes-ctr","key":{"encoding":"hex","value":"0123456789abcdef0123456789abcdef"}}});
        parse_response(&json!({"schema_version":"2.0","selected_source":"amazon","track":{"id":"B0HCDL81N4","title":title,"artists":artists,"isrc":isrc,"duration_ms":duration_ms},"playback":[playback]})).unwrap()
    }

    fn resolver_requested_track() -> Track {
        Track {
            title: "Alvafro".into(),
            artist: "LATIN MAFIA & Fred again..".into(),
            album: Some("9 months & 50 hours".into()),
            duration_seconds: Some(238.0),
            isrc: Some("GBAHS2600508".into()),
            ..Track::default()
        }
    }

    #[test]
    fn resolved_resource_uses_shared_policy_and_keeps_echoed_isrc_out_of_proof() {
        let exact_response = resolver_match_response_with_duration(
            "Alvafro",
            &["Latin Mafia & Fred again.."],
            "GBAHS2600508",
            Some(238_000),
        );
        let exact = assess_playback_resource(
            &resolver_requested_track(),
            &exact_response,
            MatchLevel::Exact,
        )
        .unwrap();
        assert_eq!(exact.assessment.action, MatchAction::Accept);
        assert_eq!(
            exact.assessment.proof_strength,
            crate::recording_match::ProofStrength::MetadataOnly
        );

        let response =
            resolver_match_response("Alvafro", &["Latin Mafia & Fred again.."], "GBAHS2600508");
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            let assessed =
                assess_playback_resource(&resolver_requested_track(), &response, level).unwrap();
            assert_eq!(assessed.candidate.id, "monochrome:B0HCDL81N4");
            assert_eq!(assessed.candidate.provider, "monochrome");
            assert_eq!(assessed.candidate.duration_seconds, None);
            assert_eq!(assessed.assessment.action, MatchAction::Accept);
            // Resolver's echo is carried only under its explicit metadata name;
            // it is never an ISRC/recordingIdentifier observation for proof.
            let meta = assessed.candidate.source_metadata.unwrap();
            assert_eq!(meta["resolverEchoedIsrc"], "GBAHS2600508");
            assert!(meta.get("isrc").is_none());
            assert!(
                assessed.assessment.proof_strength
                    != crate::recording_match::ProofStrength::RecordingIdentifier
            );
        }
    }

    #[test]
    fn resolved_resource_rejects_conflicting_identity_and_source_binding() {
        let conflicting =
            resolver_match_response("Alvafro", &["Latin Mafia & Fred again.."], "USABC2600001");
        assert_eq!(
            assess_playback_resource(
                &resolver_requested_track(),
                &conflicting,
                MatchLevel::Flexible
            )
            .unwrap_err(),
            ResolverMatchFailure::ConflictingIsrc
        );
        let wrong_title = resolver_match_response(
            "Different Song",
            &["Latin Mafia & Fred again.."],
            "GBAHS2600508",
        );
        assert_eq!(
            assess_playback_resource(
                &resolver_requested_track(),
                &wrong_title,
                MatchLevel::Flexible
            )
            .unwrap_err(),
            ResolverMatchFailure::MatchRejected
        );
        let wrong_credit =
            resolver_match_response("Alvafro", &["Different Artist"], "GBAHS2600508");
        assert_eq!(
            assess_playback_resource(
                &resolver_requested_track(),
                &wrong_credit,
                MatchLevel::Flexible
            )
            .unwrap_err(),
            ResolverMatchFailure::MatchRejected
        );
        let mut wrong_source =
            resolver_match_response("Alvafro", &["Latin Mafia & Fred again.."], "GBAHS2600508");
        wrong_source.resources[0].source = Some("tidal".into());
        assert_eq!(
            assess_playback_resource(
                &resolver_requested_track(),
                &wrong_source,
                MatchLevel::Balanced
            )
            .unwrap_err(),
            ResolverMatchFailure::SourceBindingMismatch
        );
    }

    #[test]
    fn resolver_candidate_identifiers_cannot_embed_urls_or_unbounded_text() {
        let mut response =
            resolver_match_response("Alvafro", &["Latin Mafia & Fred again.."], "GBAHS2600508");
        response.recording.as_mut().unwrap().id = Some("https://cdn.example/leak".into());
        assert_eq!(
            assess_playback_resource(&resolver_requested_track(), &response, MatchLevel::Balanced)
                .unwrap_err(),
            ResolverMatchFailure::MissingRecordingIdentity
        );

        let mut response =
            resolver_match_response("Alvafro", &["Latin Mafia & Fred again.."], "GBAHS2600508");
        response.selected_source = Some("amazon?token=hidden".into());
        assert_eq!(
            assess_playback_resource(&resolver_requested_track(), &response, MatchLevel::Balanced)
                .unwrap_err(),
            ResolverMatchFailure::MissingRecordingIdentity
        );
    }

    #[test]
    fn parses_both_supported_inline_dash_data_forms_without_logging_manifest() {
        let percent_uri = "data:application/dash+xml,%3CMPD%3E%3C%2FMPD%3E";
        let base64_uri = "data:application/dash+xml;base64,PE1QRD48L01QRD4=";
        for uri in [percent_uri, base64_uri] {
            let parsed = parse_response(&json!({"schema_version":"2.0","playback":[{"url":uri,"kind":"manifest","delivery":"dash"}]})).unwrap();
            assert_eq!(
                parsed.resources[0].inline_dash_manifest.as_deref(),
                Some("<MPD></MPD>")
            );
            assert!(!format!("{:?}", parsed.resources[0]).contains("<MPD>"));
        }
        assert!(matches!(
            parse_response(
                &json!({"schema_version":"2.0","playback":[{"url":"data:application/dash+xml,%ZZ","kind":"manifest","delivery":"dash"}]})
            ),
            Err(ResolverFailure::InvalidResponse)
        ));
    }

    #[test]
    fn classifies_access_states_and_rejects_insecure_urls() {
        assert_eq!(
            classify_status(428),
            Err(ResolverFailure::VerificationRequired)
        );
        assert_eq!(classify_status(401), Err(ResolverFailure::Unauthorized));
        assert_eq!(classify_status(403), Err(ResolverFailure::Forbidden));
        assert!(ResolverFailure::Unauthorized.needs_turnstile_refresh());
        assert!(!ResolverFailure::Forbidden.needs_turnstile_refresh());
        assert_eq!(classify_status(429), Err(ResolverFailure::RateLimited));
        assert!(matches!(
            parse_response(
                &json!({"playback":[{"url":"http://cdn.example/x.flac","kind":"audio","delivery":"direct"}]})
            ),
            Err(ResolverFailure::InvalidResponse)
        ));
    }

    #[test]
    fn auth_debug_is_redacted() {
        let auth = AuthConfig::new("bearer-secret".into(), Some("jwt-secret".into()));
        let rendered = format!("{auth:?}");
        assert!(!rendered.contains("secret"));
    }
}
