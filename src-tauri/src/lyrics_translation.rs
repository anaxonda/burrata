//! User-triggered, OpenAI-compatible translation for one current lyric document.
//! The adapter receives only lyric text and stable line IDs; credentials are
//! supplied by ConnectionManager and never persisted with this module.

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
    hash.update(b"burrata-lyrics-translation-cache-v2\0");
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
    match tokio::time::timeout(Duration::from_secs(10),free).await {
        Ok(Ok(lines)) => Ok((lines,false)),
        _ if key_available => Ok((fallback.await?,true)),
        _ => Err("Free translation is unavailable. Configure an API key in Connections to use automatic fallback".into()),
    }
}
pub async fn translate_current_document(
    connection: LyricsTranslationConnection,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
) -> Result<LyricsTranslationResponse, String> {
    validate_source(&source_hash, &lines)?;
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
            let (translated, used_fallback) = free_first_translation(
                translate_google(&connection.target_language, &lines),
                translate_openai(&connection, &source_hash, &lines),
                !connection.api_key.is_empty()
                    && !connection.base_url.trim().is_empty()
                    && !connection.model.trim().is_empty(),
            )
            .await?;
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
            {"role":"system","content":format!("Translate each supplied lyric line into {}. Preserve line meaning and tone. Keep empty source lines empty. Return exactly one JSON object with a `lines` array. Each array item must contain exactly the original `id` and the translated `text`. Keep the same number and order of lines; do not add notes, stage directions, or extra line breaks.", connection.target_language)},
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

fn parse_google_translation(html: &str, expected_lines: usize) -> Result<Vec<String>, String> {
    let document = scraper::Html::parse_document(html);
    let selector =
        scraper::Selector::parse("div.result-container, div.t0").expect("fixed selector");
    let mut results = document.select(&selector);
    let result = results
        .next()
        .ok_or("Google Translate returned no translation; try again later")?;
    if results.next().is_some() {
        return Err("Google Translate returned an ambiguous response".into());
    }
    let mut text = String::new();
    for node in result.descendants() {
        match node.value() {
            scraper::node::Node::Text(value) => text.push_str(value),
            scraper::node::Node::Element(element) if element.name() == "br" => text.push('\n'),
            _ => {}
        }
    }
    let text = text.replace("\r\n", "\n");
    let lines = text
        .trim_end_matches('\n')
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() != expected_lines
        || lines.iter().any(|line| {
            line.trim().is_empty()
                || line.len() > MAX_TRANSLATED_LINE_BYTES
                || line.chars().any(char::is_control)
        })
    {
        return Err(
            "Google Translate did not preserve lyric lines; try the AI provider instead".into(),
        );
    }
    Ok(lines)
}

async fn translate_google(
    language: &str,
    lines: &[lyrics::LyricsLine],
) -> Result<Vec<TranslatedLine>, String> {
    // The public mobile page is the same keyless interface used by
    // deep-translator. It can be rate limited; never retry it automatically or
    // silently send the document to another provider.
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
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|_| "Could not initialize Google Translate")?;
    let mut translations = HashMap::new();
    for chunk in google_chunks(&unique)? {
        let mut response = client
            .get("https://translate.google.com/m")
            .query(&[("sl", "auto"), ("tl", language), ("q", &chunk.join("\n"))])
            .send()
            .await
            .map_err(|_| "Google Translate request failed; try again later")?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err("Google Translate is temporarily rate limited; try again later or select the AI provider".into());
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "Google Translate returned HTTP {}; try again later",
                response.status().as_u16()
            ));
        }
        const LIMIT: usize = 256 * 1024;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err("Google Translate response exceeds the size limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(part) = response
            .chunk()
            .await
            .map_err(|_| "Google Translate response was incomplete")?
        {
            if bytes.len().saturating_add(part.len()) > LIMIT {
                return Err("Google Translate response exceeds the size limit".into());
            }
            bytes.extend_from_slice(&part);
        }
        let html =
            std::str::from_utf8(&bytes).map_err(|_| "Google Translate returned invalid text")?;
        let translated_lines = parse_google_translation(html, chunk.len())?;
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
    fn google_response_preserves_lines_and_rejects_wrong_or_collapsed_content() {
        assert_eq!(
            parse_google_translation(
                "<div class='result-container'>Hello<br>World &amp; stars</div>",
                2
            )
            .unwrap(),
            vec!["Hello", "World & stars"]
        );

        assert_eq!(
            parse_google_translation(
                "<div class='result-container'>Hello &amp; welcome.\nThe stars shine.</div>",
                2
            )
            .unwrap(),
            vec!["Hello & welcome.", "The stars shine."]
        );
        assert!(
            parse_google_translation("<div class='result-container'>Merged lines.</div>", 2)
                .is_err()
        );
        assert!(parse_google_translation("<div>Unrelated page</div>", 1).is_err());
        assert!(
            parse_google_translation("<div class='t0'>One</div><div class='t0'>Two</div>", 2)
                .is_err()
        );
        let long = "é".repeat(2000);
        assert_eq!(google_chunks(&[&long, "short"]).unwrap().len(), 2);
        assert!(google_chunks(&[&"x".repeat(4001)]).is_err());
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
