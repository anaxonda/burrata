//! Public Google website translation with source-verified line preservation.
use reqwest::{Client, Response, StatusCode};
use serde_json::{json, Value};

const LIMIT: usize = 256 * 1024;
async fn bounded_response(mut response: Response) -> Result<String, String> {
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(
            "Google Translate is temporarily rate limited; try again later or retry with fallback"
                .into(),
        );
    }
    if response.status() != StatusCode::OK {
        return Err(format!(
            "Google Translate returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > LIMIT as u64)
    {
        return Err("Google response exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Google response was incomplete")?
    {
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err("Google response exceeds the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| "Google response was not UTF-8".into())
}

pub(super) fn parse_rpc(body: &str, source: &str) -> Result<(String, String), String> {
    for line in body.lines().filter(|line| line.starts_with('[')) {
        let Ok(rows) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        for row in rows.as_array().into_iter().flatten() {
            if row[0] != "wrb.fr" || row[1] != "MkEWBc" {
                continue;
            }
            let raw: Value = serde_json::from_str(
                row[2]
                    .as_str()
                    .ok_or("Google website returned no translation")?,
            )
            .map_err(|_| "Google website returned invalid translation JSON")?;
            let echoes = [raw.pointer("/1/4/0"), raw.pointer("/0/6/0")];
            if !echoes
                .iter()
                .any(|echo| echo.and_then(Value::as_str) == Some(source))
                || echoes
                    .iter()
                    .flatten()
                    .any(|echo| echo.as_str() != Some(source))
            {
                return Err("Google website returned a different source document".into());
            }
            let language = raw
                .get(2)
                .and_then(Value::as_str)
                .or_else(|| raw.pointer("/0/2").and_then(Value::as_str))
                .unwrap_or("")
                .to_owned();
            let segments = raw
                .pointer("/1/0/0/5")
                .and_then(Value::as_array)
                .ok_or("Google website returned no translated lines")?;
            let mut text = String::new();
            for segment in segments {
                text.push_str(
                    segment[0]
                        .as_str()
                        .ok_or("Google website returned an invalid translation segment")?,
                );
            }
            if text.trim().is_empty() {
                return Err("Google website returned empty translation".into());
            }
            return Ok((text, language));
        }
    }
    Err("Google website response did not contain the requested translation".into())
}

async fn website_translation(
    client: &Client,
    source: &str,
    from: &str,
    to: &str,
) -> Result<(String, String), String> {
    let payload = json!([[[
        "MkEWBc",
        serde_json::to_string(&json!([[source, from, to, true], [null]]))
            .map_err(|_| "Could not prepare Google translation")?,
        null,
        "generic"
    ]]]);
    let response = client
        .post("https://translate.google.com/_/TranslateWebserverUi/data/batchexecute")
        .query(&[("rpcids", "MkEWBc"), ("hl", "en"), ("rt", "c")])
        .form(&[("f.req", payload.to_string())])
        .send()
        .await
        .map_err(|_| "Google website translation request failed")?;
    parse_rpc(&bounded_response(response).await?, source)
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn unchanged(source: &str, translated: &str) -> bool {
    let source = normalized(source);
    source.chars().count() >= 24 && source == normalized(translated)
}
fn romanized_bulgarian(source: &str, translated: &str, detected: &str) -> bool {
    detected == "bg"
        && source.chars().any(|c| c.is_ascii_alphabetic())
        && !source
            .chars()
            .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
        && unchanged(source, translated)
}

pub(super) fn parse_transliteration(value: &Value, source: &str) -> Result<String, String> {
    if value[0] != "SUCCESS" {
        return Err("Google could not normalize the transliterated lyrics".into());
    }
    let parts = value[1]
        .as_array()
        .filter(|parts| !parts.is_empty())
        .ok_or("Google returned no transliteration")?;
    let mut cursor = 0;
    let mut output = String::new();
    for part in parts {
        let fragment = part[0]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or("Google returned an invalid transliteration source")?;
        let translated = part
            .pointer("/1/0")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or("Google omitted a transliterated phrase")?;
        let start = source[cursor..]
            .find(fragment)
            .map(|position| cursor + position)
            .ok_or("Google transliterated a different source")?;
        let gap = &source[cursor..start];
        if gap.chars().any(char::is_alphanumeric) {
            return Err("Google omitted source words during transliteration".into());
        }
        output.push_str(gap);
        output.push_str(translated);
        cursor = start + fragment.len();
    }
    let tail = &source[cursor..];
    if tail.chars().any(char::is_alphanumeric) {
        return Err("Google omitted trailing source words during transliteration".into());
    }
    output.push_str(tail);
    if output.lines().count() != source.lines().count()
        || !output
            .chars()
            .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
    {
        return Err("Google transliteration did not preserve the lyric lines".into());
    }
    Ok(output)
}

pub(super) async fn translate(client: &Client, to: &str, lines: &[&str]) -> Result<Value, String> {
    let source = lines.join("\n");
    let (mut translated, detected) = website_translation(client, &source, "auto", to).await?;
    if romanized_bulgarian(&source, &translated, &detected) {
        let response = client
            .get("https://inputtools.google.com/request")
            .query(&[
                ("text", source.as_str()),
                ("itc", "bg-t-i0-und"),
                ("num", "1"),
                ("cp", "0"),
                ("cs", "1"),
                ("ie", "utf-8"),
                ("oe", "utf-8"),
            ])
            .send()
            .await
            .map_err(|_| "Google transliteration request failed")?;
        let value: Value = serde_json::from_str(&bounded_response(response).await?)
            .map_err(|_| "Google returned invalid transliteration JSON")?;
        let converted = parse_transliteration(&value, &source)?;
        let result = website_translation(client, &converted, "bg", to).await?;
        if to != "bg" && unchanged(&converted, &result.0) {
            return Err("Google returned untranslated text; retry with fallback".into());
        }
        translated = result.0;
    }
    if detected != to && unchanged(&source, &translated) {
        return Err("Google returned untranslated text; retry with fallback".into());
    }
    // Both website requests validate their echoed source. Keep the caller's
    // original text/IDs: normalization is private to this translation attempt.
    Ok(json!([[[translated, source]], null, detected]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn website_rpc_checks_the_echoed_document_and_reassembles_segments() {
        let raw = json!([
            [null, null, "bg", null, null, null, ["First\nSecond"]],
            [[[null, null, null, null, null, [["Premier\n"], ["Deuxième"]]]]],
            "bg"
        ]);
        let body = format!(
            ")]}}'\n\n42\n{}",
            json!([["wrb.fr", "MkEWBc", raw.to_string()]])
        );
        assert_eq!(
            parse_rpc(&body, "First\nSecond").unwrap(),
            ("Premier\nDeuxième".into(), "bg".into())
        );
        assert!(parse_rpc(&body, "Different").is_err());
        assert!(parse_rpc("invalid", "First").is_err());
    }
    #[test]
    fn transliteration_preserves_source_gaps_and_rejects_dropped_or_changed_words() {
        let value = json!([
            "SUCCESS",
            [
                ["parvo libe", ["първо либе"]],
                [" suho darvo", ["сухо дърво"]]
            ]
        ]);
        assert_eq!(
            parse_transliteration(&value, "parvo libe,\n suho darvo!").unwrap(),
            "първо либе,\n сухо дърво!"
        );
        assert!(parse_transliteration(&value, "omitted parvo libe, suho darvo").is_err());
        assert!(parse_transliteration(&value, "parvo libe, suho darvo omitted").is_err());
        assert!(parse_transliteration(&value, "different text").is_err());
    }
    #[test]
    fn romanized_detection_requires_google_language_evidence_and_unchanged_text() {
        let text = "parvo libe suho darvo Denko mari";
        assert!(romanized_bulgarian(text, text, "bg"));
        assert!(!romanized_bulgarian(text, text, "en"));
        assert!(!romanized_bulgarian(
            text,
            "My first love from the walnut tree",
            "bg"
        ));
        assert!(!romanized_bulgarian(
            "първо либе сухо дърво Денко мари",
            "първо либе сухо дърво Денко мари",
            "bg"
        ));
    }
}
