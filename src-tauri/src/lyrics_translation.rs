//! User-triggered, OpenAI-compatible translation for one current lyric document.
//! The adapter receives only lyric text and stable line IDs; credentials are
//! supplied by ConnectionManager and never persisted with this module.

#[path = "google_lyrics.rs"]
mod google;

use crate::{
    connections::{LyricsTranslationConnection, LyricsTranslationProvider},
    lyrics,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use url::Url;

const MAX_SOURCE_LINES: usize = 300;
const MAX_INPUT_BYTES: usize = 24 * 1024;
const MAX_REQUEST_BYTES: usize = 40 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TRANSLATED_LINE_BYTES: usize = 2000;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_LIMIT: usize = 128;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatedLine {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LyricsTranslationResponse {
    pub source_hash: String,
    pub target_language: String,
    pub lines: Vec<TranslatedLine>,
    #[serde(default)]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub fallback_used: bool,
}

#[derive(Clone)]
struct CacheEntry {
    inserted: Instant,
    value: LyricsTranslationResponse,
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn endpoint(base_url: &str) -> Result<Url, String> {
    let mut base = Url::parse(base_url.trim()).map_err(|_| "Invalid translation service URL")?;
    let local_http = base.scheme() == "http"
        && base
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
    if (base.scheme() != "https" && !local_http)
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err("Translation service must use HTTPS or loopback HTTP".into());
    }
    let path = format!("{}/", base.path().trim_end_matches('/'));
    base.set_path(&path);
    base.join("chat/completions")
        .map_err(|_| "Invalid translation service URL".into())
}

fn cache_key(connection: &LyricsTranslationConnection, source_hash: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"burrata-lyrics-translation-cache-v3\0");
    hash.update(match connection.provider {
        LyricsTranslationProvider::OpenaiCompatible => b"openai".as_slice(),
        LyricsTranslationProvider::GoogleFree => b"google-free".as_slice(),
        LyricsTranslationProvider::Auto => b"auto-google-openai".as_slice(),
    });
    for part in [
        source_hash,
        &connection.base_url,
        &connection.model,
        &connection.target_language,
    ] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_source(source_hash: &str, lines: &[lyrics::LyricsLine]) -> Result<(), String> {
    if !lyrics::validate_source_document(source_hash, lines) {
        return Err(
            "Lyric source changed or is malformed; reload the current lyrics and try again".into(),
        );
    }
    if lines.len() > MAX_SOURCE_LINES {
        return Err("This lyric document is too long to translate in one request".into());
    }
    let total_bytes: usize = lines.iter().map(|line| line.text.len()).sum();
    if total_bytes > MAX_INPUT_BYTES {
        return Err("This lyric document exceeds the translation size limit".into());
    }
    Ok(())
}

/// Translate exactly the supplied timed or plain line document. Any missing, reordered,
/// duplicated, or fabricated response ID rejects the whole translation.
async fn free_first_translation(
    free: impl std::future::Future<Output = Result<Vec<TranslatedLine>, String>>,
    fallback: impl std::future::Future<Output = Result<Vec<TranslatedLine>, String>>,
    key_available: bool,
) -> Result<(Vec<TranslatedLine>, bool), String> {
    match tokio::time::timeout(Duration::from_secs(if key_available {3} else {10}),free).await {
        Ok(Ok(lines)) => Ok((lines,false)),
        _ if key_available => Ok((fallback.await?,true)),
        _ => Err("Free translation is unavailable. Configure an API key in Connections to use automatic fallback".into()),
    }
}
fn free_cooldown() -> &'static Mutex<Option<Instant>> {
    static COOLDOWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    COOLDOWN.get_or_init(|| Mutex::new(None))
}

pub async fn translate_current_document(
    connection: LyricsTranslationConnection,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
) -> Result<LyricsTranslationResponse, String> {
    validate_source(&source_hash, &lines)?;
    translate_validated_document(connection, source_hash, lines).await
}

pub(crate) async fn translate_current_chunk(
    connection: LyricsTranslationConnection,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
    start: usize,
    end: usize,
) -> Result<LyricsTranslationResponse, String> {
    validate_source(&source_hash, &lines)?;
    if start >= end || end > lines.len() || end - start > 16 {
        return Err("Invalid lyric translation batch".into());
    }
    if connection.provider == LyricsTranslationProvider::GoogleFree {
        // Detect language from the whole song, not a short chorus-sized UI batch.
        // Serialize the shared cache fill so concurrent display batches send one document.
        static GOOGLE_DOCUMENT: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let _guard = GOOGLE_DOCUMENT
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let mut result = translate_current_document(connection, source_hash, lines).await?;
        result.lines = result.lines[start..end].to_vec();
        return Ok(result);
    }
    let chunk = lines[start..end].to_vec();
    // Stable IDs belong to the full document, including repeated-line ordinals.
    // Validate that full document above, then retain those IDs inside each batch.
    let key = lyrics::source_hash_for_lines(&chunk);
    let mut result = translate_validated_document(connection, key, chunk).await?;
    result.source_hash = source_hash;
    Ok(result)
}

async fn translate_validated_document(
    connection: LyricsTranslationConnection,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
) -> Result<LyricsTranslationResponse, String> {
    if connection.provider == LyricsTranslationProvider::OpenaiCompatible
        && (connection.model.trim().is_empty()
            || connection.target_language.trim().is_empty()
            || connection.api_key.is_empty()
            || connection.api_key.len() > 4096
            || connection.api_key.chars().any(char::is_control))
    {
        return Err("Translation is not fully configured in Connections".into());
    }
    let key = cache_key(&connection, &source_hash);
    if let Ok(mut entries) = cache().lock() {
        entries.retain(|_, entry| entry.inserted.elapsed() < CACHE_TTL);
        if let Some(entry) = entries.get(&key) {
            return Ok(entry.value.clone());
        }
    }

    let mut used_api = connection.provider == LyricsTranslationProvider::OpenaiCompatible;
    let mut fallback_used = false;
    let translated = match connection.provider {
        LyricsTranslationProvider::Auto => {
            let key_available = !connection.api_key.is_empty()
                && !connection.base_url.trim().is_empty()
                && !connection.model.trim().is_empty();
            let cooling = key_available
                && free_cooldown()
                    .lock()
                    .ok()
                    .and_then(|value| *value)
                    .is_some_and(|until| Instant::now() < until);
            let free = async {
                if cooling {
                    Err("Free translation is cooling down".into())
                } else {
                    translate_google(&connection.target_language, &lines).await
                }
            };
            let (translated, used_fallback) = free_first_translation(
                free,
                translate_openai(&connection, &source_hash, &lines),
                key_available,
            )
            .await?;
            if let Ok(mut cooldown) = free_cooldown().lock() {
                *cooldown = used_fallback.then(|| Instant::now() + Duration::from_secs(60));
            }
            used_api = used_fallback;
            fallback_used = used_fallback;
            translated
        }
        LyricsTranslationProvider::OpenaiCompatible => {
            translate_openai(&connection, &source_hash, &lines).await?
        }
        LyricsTranslationProvider::GoogleFree => tokio::time::timeout(
            Duration::from_secs(45),
            translate_google(&connection.target_language, &lines),
        )
        .await
        .map_err(|_| "Google Translate took too long; try again later")??,
    };
    let result = LyricsTranslationResponse {
        source_hash,
        target_language: connection.target_language,
        lines: translated,
        provider: if used_api {
            if url::Url::parse(&connection.base_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .as_deref()
                == Some("openrouter.ai")
            {
                "openRouter"
            } else {
                "openaiCompatible"
            }
        } else {
            "googleFree"
        }
        .into(),
        model: used_api.then(|| connection.model.clone()),
        fallback_used,
    };
    if let Ok(mut entries) = cache().lock() {
        if entries.len() >= CACHE_LIMIT {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            CacheEntry {
                inserted: Instant::now(),
                value: result.clone(),
            },
        );
    }
    Ok(result)
}

async fn translate_openai(
    connection: &LyricsTranslationConnection,
    source_hash: &str,
    lines: &[lyrics::LyricsLine],
) -> Result<Vec<TranslatedLine>, String> {
    let url = endpoint(&connection.base_url)?;
    let source = lines
        .iter()
        .map(|line| json!({"id":line.id,"text":line.text}))
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": connection.model,
        "max_completion_tokens": 6000,
        "response_format": {"type":"json_object"},
        "messages": [
            {"role":"system","content":format!("Translate each supplied lyric line into {}. Preserve line meaning and tone. The input may be romanized or transliterated; infer the underlying language and translate its meaning rather than copying phonetic spelling. Keep empty source lines empty. Return exactly one JSON object with a `lines` array. Each array item must contain exactly the original `id` and the translated `text`. Keep the same number and order of lines; do not add notes, stage directions, or extra line breaks.", connection.target_language)},
            {"role":"user","content":serde_json::to_string(&json!({"sourceHash":source_hash,"lines":source})).map_err(|_| "Could not prepare translation request")?}
        ]
    });
    if url.host_str() == Some("openrouter.ai") && connection.model == "openai/gpt-6-luna" {
        body["reasoning"] = json!({"effort":"none"});
    }
    let body_bytes =
        serde_json::to_vec(&body).map_err(|_| "Could not prepare translation request")?;
    if body_bytes.len() > MAX_REQUEST_BYTES {
        return Err("Translation request exceeds the size limit".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the translation service connection")?;
    let mut response = client
        .post(url)
        .bearer_auth(&connection.api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body_bytes)
        .send()
        .await
        .map_err(|_| "Translation service request failed; check endpoint, network, and API key")?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(format!(
            "Translation service returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) | Err(_) => {
                return Err("Translation service response was too large or incomplete".into())
            }
            Ok(None) => break,
        }
    }
    let envelope: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Translation service returned invalid JSON")?;
    let content = envelope
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or("Translation service returned no translated content")?;
    let payload: TranslationPayload = serde_json::from_str(content)
        .map_err(|_| "Translation response was not the required line JSON")?;
    validate_translations(lines, payload.lines)
}

fn google_chunks<'a>(texts: &[&'a str]) -> Result<Vec<Vec<&'a str>>, String> {
    let mut chunks = Vec::new();
    let mut chunk = Vec::new();
    let mut bytes = 0;
    for text in texts {
        if text.len() > 4000 {
            return Err("A lyric line exceeds Google Translate's free request limit".into());
        }
        if !chunk.is_empty() && (bytes + text.len() + 1 > 4000 || chunk.len() >= 40) {
            chunks.push(std::mem::take(&mut chunk));
            bytes = 0;
        }
        bytes += text.len() + 1;
        chunk.push(*text);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    Ok(chunks)
}

fn parse_google_translation(value: &Value, source_lines: &[&str]) -> Result<Vec<String>, String> {
    let segments = value
        .get(0)
        .and_then(Value::as_array)
        .filter(|segments| !segments.is_empty())
        .ok_or("Google Translate returned no translation")?;
    let mut translated = String::new();
    let mut source = String::new();
    for segment in segments {
        let text = segment
            .get(0)
            .and_then(Value::as_str)
            .ok_or("Google Translate returned an invalid translation segment")?;
        let original = segment
            .get(1)
            .and_then(Value::as_str)
            .ok_or("Google Translate did not identify the translated source")?;
        translated.push_str(text);
        source.push_str(original);
    }
    // Google's response can split a sentence into several segments. Reassemble
    // those segments before validating line boundaries against the echoed input.
    if source != source_lines.join("\n") {
        return Err("Google Translate returned a different source document".into());
    }
    let translated = translated.replace("\r\n", "\n");
    let lines = translated
        .trim_end_matches('\n')
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() != source_lines.len()
        || lines.iter().any(|line| {
            line.trim().is_empty()
                || line.len() > MAX_TRANSLATED_LINE_BYTES
                || line.chars().any(char::is_control)
        })
    {
        return Err("Google Translate did not preserve lyric line boundaries".into());
    }
    Ok(lines)
}

async fn translate_google(
    language: &str,
    lines: &[lyrics::LyricsLine],
) -> Result<Vec<TranslatedLine>, String> {
    // Public keyless JSON interface. It needs neither an account nor an API
    // credential. A rate limit ends this attempt; no mobile-page scraping,
    // account cookies, or endpoint rotation is used.
    let mut unique = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in lines.iter().filter(|line| !line.text.trim().is_empty()) {
        if seen.insert(line.text.as_str()) {
            unique.push(line.text.as_str());
        }
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Burrata/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Could not initialize Google Translate")?;
    let mut translations = HashMap::new();
    for chunk in google_chunks(&unique)? {
        let value = google::translate(&client, language, &chunk).await?;
        let translated_lines = parse_google_translation(&value, &chunk)?;
        for (source, translated) in chunk.into_iter().zip(translated_lines) {
            translations.insert(source, translated);
        }
    }
    let translated = lines
        .iter()
        .map(|line| {
            Ok(TranslatedLine {
                id: line.id.clone(),
                text: if line.text.trim().is_empty() {
                    String::new()
                } else {
                    translations
                        .get(line.text.as_str())
                        .ok_or("Google Translate omitted a lyric line")?
                        .clone()
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_translations(lines, translated)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TranslationPayload {
    lines: Vec<TranslatedLine>,
}

fn validate_translations(
    source: &[lyrics::LyricsLine],
    translated: Vec<TranslatedLine>,
) -> Result<Vec<TranslatedLine>, String> {
    if translated.len() != source.len()
        || translated.iter().zip(source).any(|(translated, source)| {
            translated.id != source.id
                || (translated.text.trim().is_empty() && !source.text.trim().is_empty())
                || translated.text.len() > MAX_TRANSLATED_LINE_BYTES
                || translated.text.chars().any(char::is_control)
        })
    {
        return Err("Translation response did not preserve the source line IDs and order".into());
    }
    Ok(translated)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn free_first_does_not_poll_api_until_free_fails_and_key_is_configured() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let result = super::free_first_translation(
            async { Ok(vec![]) },
            async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            },
            true,
        )
        .await
        .unwrap();
        assert!(!result.1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(super::free_first_translation(
            async { Err("429".into()) },
            async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            },
            false
        )
        .await
        .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let result = super::free_first_translation(
            async { Err("429".into()) },
            async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            },
            true,
        )
        .await
        .unwrap();
        assert!(result.1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn document() -> (String, Vec<lyrics::LyricsLine>) {
        let lines = vec![
            lyrics::LyricsLine {
                id: lyrics::stable_line_id(Some(1.25), "First line", 0),
                time_seconds: Some(1.25),
                text: "First line".into(),
            },
            lyrics::LyricsLine {
                id: lyrics::stable_line_id(Some(3.5), "Second line", 0),
                time_seconds: Some(3.5),
                text: "Second line".into(),
            },
        ];
        let source_hash = lyrics::source_hash_for_lines(&lines);
        (source_hash, lines)
    }

    #[tokio::test]
    async fn chunk_cache_preserves_full_document_repeated_line_ids() {
        let lines: Vec<_> = (0..4)
            .map(|ordinal| lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, "Repeated batch fixture", ordinal),
                time_seconds: None,
                text: "Repeated batch fixture".into(),
            })
            .collect();
        let hash = lyrics::source_hash_for_lines(&lines);
        assert!(lyrics::validate_source_document(&hash, &lines));
        let connection = LyricsTranslationConnection {
            provider: LyricsTranslationProvider::GoogleFree,
            base_url: String::new(),
            model: String::new(),
            target_language: "fr".into(),
            api_key: String::new(),
        };
        let chunk_hash = lyrics::source_hash_for_lines(&lines[2..]);
        assert!(
            !lyrics::validate_source_document(&chunk_hash, &lines[2..]),
            "IDs retain whole-document duplicate ordinals"
        );
        let response = LyricsTranslationResponse {
            source_hash: hash.clone(),
            target_language: "fr".into(),
            lines: lines
                .iter()
                .map(|line| TranslatedLine {
                    id: line.id.clone(),
                    text: "Répété".into(),
                })
                .collect(),
            provider: "googleFree".into(),
            model: None,
            fallback_used: false,
        };
        cache().lock().unwrap().insert(
            cache_key(&connection, &hash),
            CacheEntry {
                inserted: Instant::now(),
                value: response,
            },
        );
        let result = translate_current_chunk(connection.clone(), hash.clone(), lines.clone(), 2, 4)
            .await
            .unwrap();
        assert_eq!(result.source_hash, hash);
        assert_eq!(result.lines[0].id, lines[2].id);
        assert!(translate_current_chunk(connection, hash, lines, 2, 5)
            .await
            .is_err());
    }

    /// Explicit opt-in only; the caller supplies the authorized local .env path.
    #[tokio::test]
    #[ignore = "requires explicitly authorized OpenRouter credentials and a real provider request"]
    async fn authorized_openrouter_translation_preserves_document() {
        let path = std::env::var("BURRATA_TRANSLATION_QUALIFICATION_ENV")
            .expect("Set the authorized .env path");
        let content = std::fs::read_to_string(path).expect("Read authorized .env file");
        let field = |name: &str| {
            content
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(key, _)| key.trim() == name)
                .map(|(_, value)| {
                    value
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"')
                        .to_owned()
                })
                .filter(|value| !value.is_empty())
                .expect("Required provider setting must be present")
        };
        let source = ["Bonjour tout le monde.", "Je regarde les étoiles."];
        let lines = source
            .iter()
            .map(|text| lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, text, 0),
                time_seconds: None,
                text: (*text).to_owned(),
            })
            .collect::<Vec<_>>();
        let hash = lyrics::source_hash_for_lines(&lines);
        let result = translate_current_document(
            LyricsTranslationConnection {
                provider: if std::env::var("BURRATA_TRANSLATION_QUALIFICATION_AUTO").as_deref()
                    == Ok("1")
                {
                    LyricsTranslationProvider::Auto
                } else {
                    LyricsTranslationProvider::OpenaiCompatible
                },
                base_url: "https://openrouter.ai/api/v1".into(),
                model: std::env::var("BURRATA_TRANSLATION_QUALIFICATION_MODEL")
                    .unwrap_or_else(|_| "openai/gpt-6-luna".into()),
                target_language: "en".into(),
                api_key: field("OPENROUTER_API_KEY"),
            },
            hash.clone(),
            lines.clone(),
        )
        .await
        .unwrap();
        eprintln!(
            "Live translation provider={}, model={:?}, fallback={}, lines={}",
            result.provider,
            result.model,
            result.fallback_used,
            result.lines.len()
        );
        assert_eq!(result.source_hash, hash);
        assert_eq!(result.lines.len(), lines.len());
        for (translated, original) in result.lines.iter().zip(&lines) {
            assert_eq!(translated.id, original.id);
            assert!(!translated.text.trim().is_empty());
            assert_ne!(translated.text, original.text);
        }
    }

    #[test]
    fn google_json_preserves_lines_and_rejects_wrong_or_collapsed_content() {
        let response = json!([
            [
                ["Bonjour. ", "Hello. "],
                ["Au revoir.\n", "Goodbye.\n"],
                ["Les étoiles brillent.", "The stars shine."]
            ],
            null,
            "en"
        ]);
        assert_eq!(
            parse_google_translation(&response, &["Hello. Goodbye.", "The stars shine."]).unwrap(),
            vec!["Bonjour. Au revoir.", "Les étoiles brillent."]
        );
        assert!(
            parse_google_translation(&response, &["Different source", "The stars shine."]).is_err()
        );
        assert!(parse_google_translation(
            &json!([[["Merged translation", "First\nSecond"]]]),
            &["First", "Second"]
        )
        .is_err());
        assert!(
            parse_google_translation(&json!({"error":"Unexpected response"}), &["First"]).is_err()
        );
        assert!(
            parse_google_translation(&json!([[["Invalid\tcontrol", "First"]]]), &["First"])
                .is_err()
        );
        let long = "é".repeat(2000);
        assert_eq!(google_chunks(&[&long, "short"]).unwrap().len(), 2);
        assert!(google_chunks(&[&"x".repeat(4001)]).is_err());
    }

    #[tokio::test]
    #[ignore = "Explicit Google keyless translation check; no API key or paid fallback"]
    async fn live_google_translates_romanized_bulgarian_with_source_and_line_identity_preserved() {
        let texts = [
            "Kalimanko, Denko mari, hubava,",
            "do sega si mi bila, Denko mari, kalimana,",
            "ot sega nasetne, Denko mari, parvo libe.",
            "",
            "Izgori me, Denko mari, popari me,",
            "ta me stori, Denko mari, suho darvo,",
            "suho darvo, Denko mari, orehovo",
        ];
        let lines = texts
            .iter()
            .map(|text| lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, text, 0),
                time_seconds: None,
                text: (*text).into(),
            })
            .collect::<Vec<_>>();
        let original = lines.clone();
        let hash = lyrics::source_hash_for_lines(&lines);
        let started = Instant::now();
        let response = translate_current_document(
            LyricsTranslationConnection {
                provider: LyricsTranslationProvider::GoogleFree,
                base_url: String::new(),
                model: String::new(),
                target_language: "en".into(),
                api_key: String::new(),
            },
            hash.clone(),
            lines,
        )
        .await
        .unwrap();
        assert_eq!(response.source_hash, hash);
        assert_eq!(response.provider, "googleFree");
        assert!(!response.fallback_used);
        assert_eq!(response.lines.len(), original.len());
        for (source, result) in original.iter().zip(&response.lines) {
            assert_eq!(source.id, result.id);
            assert_eq!(source.text.is_empty(), result.text.is_empty());
        }
        let text = response
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(
            text.contains("beautiful"),
            "Expected meaningful translation, not copied transliteration"
        );
        assert!(text.contains("wood") || text.contains("tree"));
        assert!(text.contains("walnut"));
        assert_eq!(
            original
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            texts
        );
        eprintln!("Google keyless romanized Bulgarian: {} lines in {} ms, IDs/source/blank lines preserved, no API key or fallback",response.lines.len(),started.elapsed().as_millis());
    }

    #[tokio::test]
    #[ignore = "Explicit Google keyless translation check; no API key or paid fallback"]
    async fn live_google_keyless_translates_unicode_blank_and_repeated_lines_without_a_key() {
        for (target, texts) in [
            (
                "fr",
                vec![
                    "The quiet river shines.",
                    "",
                    "We are going home.",
                    "The quiet river shines.",
                    "The moon & stars are bright — tonight.",
                ],
            ),
            ("en", vec!["Реката е тиха.", "Тази вечер се прибираме."]),
        ] {
            let mut ordinals = HashMap::new();
            let lines: Vec<_> = texts
                .iter()
                .map(|text| {
                    let ordinal = ordinals.entry(*text).or_insert(0);
                    let line = lyrics::LyricsLine {
                        id: lyrics::stable_line_id(None, text, *ordinal),
                        time_seconds: None,
                        text: (*text).into(),
                    };
                    *ordinal += 1;
                    line
                })
                .collect();
            let hash = lyrics::source_hash_for_lines(&lines);
            let result = translate_current_document(
                LyricsTranslationConnection {
                    provider: LyricsTranslationProvider::GoogleFree,
                    base_url: String::new(),
                    model: String::new(),
                    target_language: target.into(),
                    api_key: String::new(),
                },
                hash.clone(),
                lines.clone(),
            )
            .await
            .unwrap();
            assert_eq!(result.provider, "googleFree");
            assert!(result.model.is_none());
            assert!(!result.fallback_used);
            assert_eq!(result.source_hash, hash);
            assert_eq!(result.lines.len(), lines.len());
            for (source, translated) in lines.iter().zip(&result.lines) {
                assert_eq!(source.id, translated.id);
                assert_eq!(
                    source.text.trim().is_empty(),
                    translated.text.trim().is_empty()
                );
                if !source.text.trim().is_empty() {
                    assert_ne!(source.text, translated.text);
                }
            }
            eprintln!("Google keyless translation passed: target={target}, lines={}, no API key or fallback",lines.len());
        }
    }

    #[tokio::test]
    #[ignore = "Explicit Google keyless timed-song translation check; public lyrics only, no API key"]
    async fn live_google_keyless_translates_reported_timed_song_without_a_key() {
        let doc = lyrics::get_current_track_lyrics(lyrics::LyricsRequest {
            track_name: "I've Got a Tiger By the Tail".into(),
            artist_name: "Buck Owens".into(),
            album_name: None,
            duration_seconds: 133.,
        })
        .await;
        assert_eq!(doc.status, "synced");
        let lines = doc.lines.unwrap();
        let hash = doc.source_hash.unwrap();
        let started = Instant::now();
        let result = translate_current_document(
            LyricsTranslationConnection {
                provider: LyricsTranslationProvider::GoogleFree,
                base_url: String::new(),
                model: String::new(),
                target_language: "fr".into(),
                api_key: String::new(),
            },
            hash.clone(),
            lines.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.provider, "googleFree");
        assert!(!result.fallback_used);
        assert!(result.model.is_none());
        assert_eq!(result.source_hash, hash);
        assert_eq!(result.lines.len(), lines.len());
        for (source, translated) in lines.iter().zip(&result.lines) {
            assert_eq!(source.id, translated.id);
            assert_eq!(
                source.text.trim().is_empty(),
                translated.text.trim().is_empty()
            );
        }
        eprintln!(
            "Google keyless timed lyrics: {} lines in {} ms, no key and no fallback",
            lines.len(),
            started.elapsed().as_millis()
        );
    }

    #[test]
    fn source_hash_and_ordered_line_ids_are_required() {
        let (hash, lines) = document();
        assert!(validate_source(&hash, &lines).is_ok());
        let good = lines
            .iter()
            .map(|line| TranslatedLine {
                id: line.id.clone(),
                text: format!("traduit {}", line.text),
            })
            .collect();
        assert_eq!(validate_translations(&lines, good).unwrap().len(), 2);
        let swapped = vec![
            TranslatedLine {
                id: lines[1].id.clone(),
                text: "two".into(),
            },
            TranslatedLine {
                id: lines[0].id.clone(),
                text: "one".into(),
            },
        ];
        assert!(validate_translations(&lines, swapped).is_err());
        let mut forged = lines.clone();
        forged[0].text.push('!');
        assert!(validate_source(&hash, &forged).is_err());
    }

    #[test]
    fn plain_lyrics_preserve_blank_paragraphs_and_allow_empty_translations() {
        let lines = vec![
            lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, "First paragraph", 0),
                time_seconds: None,
                text: "First paragraph".into(),
            },
            lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, "", 0),
                time_seconds: None,
                text: String::new(),
            },
            lyrics::LyricsLine {
                id: lyrics::stable_line_id(None, "Last paragraph", 0),
                time_seconds: None,
                text: "Last paragraph".into(),
            },
        ];
        let source_hash = lyrics::source_hash_for_lines(&lines);
        assert!(validate_source(&source_hash, &lines).is_ok());
        let translated = vec![
            TranslatedLine {
                id: lines[0].id.clone(),
                text: "Premier paragraphe".into(),
            },
            TranslatedLine {
                id: lines[1].id.clone(),
                text: String::new(),
            },
            TranslatedLine {
                id: lines[2].id.clone(),
                text: "Dernier paragraphe".into(),
            },
        ];
        assert_eq!(
            validate_translations(&lines, translated).unwrap()[1].text,
            ""
        );
        let fabricated_blank = vec![
            TranslatedLine {
                id: lines[0].id.clone(),
                text: String::new(),
            },
            TranslatedLine {
                id: lines[1].id.clone(),
                text: String::new(),
            },
            TranslatedLine {
                id: lines[2].id.clone(),
                text: "Dernier paragraphe".into(),
            },
        ];
        assert!(validate_translations(&lines, fabricated_blank).is_err());
    }

    #[test]
    fn endpoint_allows_https_and_loopback_http_only() {
        assert_eq!(
            endpoint("https://api.example.test/v1").unwrap().path(),
            "/v1/chat/completions"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:1234/v1").unwrap().path(),
            "/v1/chat/completions"
        );
        for url in [
            "http://example.test/v1",
            "https://u:p@example.test/v1",
            "https://example.test/v1?key=x",
        ] {
            assert!(endpoint(url).is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn compatible_local_fixture_preserves_exact_ids_and_sends_no_track_metadata() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (source_hash, lines) = document();
        let expected_ids = lines.iter().map(|line| line.id.clone()).collect::<Vec<_>>();
        let fixture_hash = source_hash.clone();
        let fixture_ids = expected_ids.clone();
        let content = serde_json::to_string(&json!({"lines":[
            {"id":expected_ids[0],"text":"Première ligne"},
            {"id":expected_ids[1],"text":"Deuxième ligne"}
        ]}))
        .unwrap();
        let response_body =
            serde_json::to_vec(&json!({"choices":[{"message":{"content":content}}]})).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buffer = [0_u8; 4096];
            let header_end = loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
                if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&received[..header_end]).into_owned();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap();
            while received.len() < header_end + content_length {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
            }
            let body: Value =
                serde_json::from_slice(&received[header_end..header_end + content_length]).unwrap();
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-secret"));
            assert!(headers.starts_with("POST /v1/chat/completions "));
            let user_text = body
                .pointer("/messages/1/content")
                .and_then(Value::as_str)
                .unwrap();
            let user: Value = serde_json::from_str(user_text).unwrap();
            assert_eq!(user["sourceHash"], fixture_hash);
            assert_eq!(user["lines"][0]["id"], fixture_ids[0]);
            assert!(user.get("artist").is_none() && user.get("title").is_none());
            assert!(user["lines"][0].get("timeSeconds").is_none());
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response_body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&response_body).unwrap();
        });
        let result = translate_current_document(
            LyricsTranslationConnection {
                provider: LyricsTranslationProvider::OpenaiCompatible,
                base_url: format!("http://{address}/v1"),
                model: "local-fixture-model".into(),
                target_language: "French".into(),
                api_key: "fixture-secret".into(),
            },
            source_hash.clone(),
            lines,
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.source_hash, source_hash);
        assert_eq!(result.target_language, "French");
        assert_eq!(result.lines[0].text, "Première ligne");
        assert_eq!(
            result
                .lines
                .iter()
                .map(|line| line.id.clone())
                .collect::<Vec<_>>(),
            expected_ids
        );
    }
}
