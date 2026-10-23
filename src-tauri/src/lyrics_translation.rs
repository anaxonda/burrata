//! User-triggered, OpenAI-compatible translation for one current lyric document.
//! The adapter receives only lyric text and stable line IDs; credentials are
//! supplied by ConnectionManager and never persisted with this module.

use crate::{connections::LyricsTranslationConnection, lyrics};
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
    hash.update(b"burrata-lyrics-translation-cache-v1\0");
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
pub async fn translate_current_document(
    connection: LyricsTranslationConnection,
    source_hash: String,
    lines: Vec<lyrics::LyricsLine>,
) -> Result<LyricsTranslationResponse, String> {
    validate_source(&source_hash, &lines)?;
    if connection.model.trim().is_empty()
        || connection.target_language.trim().is_empty()
        || connection.api_key.is_empty()
        || connection.api_key.len() > 4096
        || connection.api_key.chars().any(char::is_control)
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

    let url = endpoint(&connection.base_url)?;
    let source = lines
        .iter()
        .map(|line| json!({"id":line.id,"text":line.text}))
        .collect::<Vec<_>>();
    let body = json!({
        "model": connection.model,
        "max_completion_tokens": 6000,
        "response_format": {"type":"json_object"},
        "messages": [
            {"role":"system","content":format!("Translate each supplied lyric line into {}. Preserve line meaning and tone. Keep empty source lines empty. Return exactly one JSON object with a `lines` array. Each array item must contain exactly the original `id` and the translated `text`. Keep the same number and order of lines; do not add notes, stage directions, or extra line breaks.", connection.target_language)},
            {"role":"user","content":serde_json::to_string(&json!({"sourceHash":source_hash,"lines":source})).map_err(|_| "Could not prepare translation request")?}
        ]
    });
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
    let translated = validate_translations(&lines, payload.lines)?;
    let result = LyricsTranslationResponse {
        source_hash,
        target_language: connection.target_language,
        lines: translated,
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
