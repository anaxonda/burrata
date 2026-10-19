//! Recovery runs only on missing/failed station images, never on playback startup.
use super::{get_radio_stations_saved, stable_url_hash, validate_stream_url};
use crate::{storage::Store, tools};
use base64::Engine as _;
use serde_json::json;
use std::{collections::HashSet, time::Duration};

const CACHE: &str = "radio.station-artwork.v1";
const MAX_HTML: usize = 256 * 1024;
const MAX_IMAGE: usize = 2 * 1024 * 1024;

pub async fn resolve_station_artwork(
    station_id: String,
    store: &Store,
) -> Result<Option<String>, String> {
    let station = get_radio_stations_saved(store)?
        .into_iter()
        .find(|s| s.id == station_id)
        .ok_or("Radio station no longer exists")?;
    let key = stable_url_hash(
        &json!([
            station.id,
            station.stream_url,
            station.favicon_url,
            station.homepage_url
        ])
        .to_string(),
    );
    if let Some(cached) = store.get_provider_cache(CACHE, &key)? {
        return Ok(cached["dataUrl"].as_str().map(str::to_owned));
    }
    static LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let work = async {
        let _permit = LIMIT
            .acquire()
            .await
            .map_err(|_| "Station artwork recovery stopped")?;
        let homepage = station
            .homepage_url
            .as_deref()
            .ok_or("Station has no authoritative homepage")?;
        let (final_homepage, headers, html) =
            fetch_bounded(validate_stream_url(homepage)?, MAX_HTML, false).await?;
        let mime = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !mime.to_ascii_lowercase().starts_with("text/html") {
            return Err("Station homepage is not HTML".to_owned());
        }
        let html = String::from_utf8_lossy(&html);
        let icons = discover_icons(&html, &final_homepage);
        for icon in icons.into_iter().take(3) {
            let Ok((_, headers, bytes)) = fetch_bounded(icon, MAX_IMAGE, true).await else {
                continue;
            };
            let mime = headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !supported_raster(mime, &bytes) {
                continue;
            }
            if let Ok(png) = decode_thumbnail(&bytes).await {
                return Ok(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(png)
                ));
            }
        }
        Err("Station homepage has no usable artwork".to_owned())
    };
    // Failure is a clean placeholder. Cache it briefly so navigating does not
    // continually retry a dead homepage; successful sanitized thumbnails last 7d.
    let data = tokio::time::timeout(Duration::from_secs(8), work)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = store.set_provider_cache(
        CACHE,
        &key,
        &json!({"dataUrl":data}),
        if data.is_some() { 7 * 86400 } else { 600 },
    );
    Ok(data)
}

async fn fetch_bounded(
    mut url: url::Url,
    max: usize,
    require_https: bool,
) -> Result<(url::Url, reqwest::header::HeaderMap, Vec<u8>), String> {
    let mut seen = HashSet::new();
    for hop in 0..=3 {
        url = validate_stream_url(url.as_str())?;
        if require_https && url.scheme() != "https" {
            return Err("Station artwork must use HTTPS".into());
        }
        url.set_fragment(None);
        if !seen.insert(url.to_string()) {
            return Err("Station artwork redirect loop".into());
        }
        let mut addresses = crate::acquisition::public_addresses(&url).await?;
        let port = url
            .port_or_known_default()
            .ok_or("Station artwork has no port")?;
        for address in &mut addresses {
            address.set_port(port);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .resolve_to_addrs(
                url.host_str().ok_or("Station artwork has no host")?,
                &addresses,
            )
            .build()
            .map_err(|_| "Could not prepare station artwork request")?;
        let mut response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| "Station artwork request failed")?;
        if response.status().is_redirection() {
            if hop == 3 {
                return Err("Station artwork redirect limit".into());
            }
            url = url
                .join(
                    response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .ok_or("Station artwork redirect has no destination")?,
                )
                .map_err(|_| "Station artwork redirect is invalid")?;
            continue;
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > max as u64)
        {
            return Err("Station artwork response is unavailable or too large".into());
        }
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Station artwork response interrupted")?
        {
            if bytes.len() + chunk.len() > max {
                return Err("Station artwork response is too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((url, headers, bytes));
    }
    Err("Station artwork redirect limit".into())
}

fn discover_icons(html: &str, homepage: &url::Url) -> Vec<url::Url> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("link[rel][href]").unwrap();
    let mut seen = HashSet::new();
    document
        .select(&selector)
        .filter_map(|element| {
            let rel = element.value().attr("rel")?;
            if !rel.split_ascii_whitespace().any(|part| {
                part.eq_ignore_ascii_case("icon") || part.eq_ignore_ascii_case("apple-touch-icon")
            }) {
                return None;
            }
            let mut url = homepage.join(element.value().attr("href")?).ok()?;
            if url.scheme() == "http" {
                url.set_scheme("https").ok()?;
            }
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.host_str().is_none()
            {
                return None;
            }
            seen.insert(url.to_string()).then_some(url)
        })
        .take(8)
        .collect()
}

fn supported_raster(mime: &str, bytes: &[u8]) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    }
}

async fn decode_thumbnail(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let file = tempfile::NamedTempFile::new().map_err(|_| "Could not prepare station artwork")?;
    tokio::fs::write(file.path(), bytes)
        .await
        .map_err(|_| "Could not prepare station artwork")?;
    let mut command = tokio::process::Command::new(tools::executable("", "ffmpeg"));
    tools::configure_no_console_tokio(&mut command);
    command
        .kill_on_drop(true)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-xerror",
            "-max_alloc",
            "33554432",
            "-max_pixels",
            "16777216",
            "-i",
        ])
        .arg(file.path())
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=128:128:force_original_aspect_ratio=decrease",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(Duration::from_secs(2), command.output())
        .await
        .map_err(|_| "Station artwork decoding timed out")?
        .map_err(|_| "Station artwork decoder unavailable")?;
    if !output.status.success()
        || output.stdout.len() > 128 * 1024
        || !output.stdout.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("Station artwork could not be decoded".into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_discovery_uses_authoritative_final_homepage_and_rejects_active_urls() {
        let base = url::Url::parse("https://www.radiofrance.fr/franceinter").unwrap();
        let urls = discover_icons(
            r#"<link rel="icon" href="/external/favicons/franceinter/favicon.png"><link rel="stylesheet" href="/style.css"><link rel="icon" href="data:image/svg+xml,bad"><link rel="icon" href="https://user:password@example.test/a.png"><link rel="icon" href="javascript:alert(1)"><link rel="shortcut icon" href="/external/favicons/franceinter/favicon.png">"#,
            &base,
        );
        assert_eq!(urls.len(), 1);
        assert_eq!(
            urls[0].as_str(),
            "https://www.radiofrance.fr/external/favicons/franceinter/favicon.png"
        );
    }
    #[test]
    fn image_type_must_match_raster_signature_and_active_formats_are_rejected() {
        assert!(supported_raster("image/png", b"\x89PNG\r\n\x1a\nfixture"));
        assert!(!supported_raster("image/png", b"<html>error</html>"));
        assert!(!supported_raster("image/svg+xml", b"<svg/>"));
        assert!(!supported_raster("text/html", b"\x89PNG\r\n\x1a\n"));
    }
}
