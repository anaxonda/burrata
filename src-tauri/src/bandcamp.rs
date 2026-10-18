//! Direct playback resolution for an explicitly linked public Bandcamp track.
//!
//! Bandcamp playback is an internal, default-off experiment. It accepts only a
//! single track page URL, reads its public `data-tralbum` metadata, and uses the
//! page's ordinary MP3 streaming URL. It never searches or downloads releases.

use reqwest::{redirect::Policy, Client, Url};
use serde_json::Value;
use std::{net::SocketAddr, time::Duration};
use tokio::{sync::watch, time::Instant};

const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const PAGE_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = "Burrata/0.1 (public Bandcamp playback)";

fn has_explicit_port(raw: &str) -> bool {
    raw.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .is_some_and(|authority| authority.rsplit_once(':').is_some())
}

#[derive(Debug)]
pub(crate) struct TrackAudio {
    pub provider_id: String,
    pub title: String,
    pub artist: String,
    pub duration_seconds: f64,
    pub stream_url: Url,
    pub stream_addresses: Vec<SocketAddr>,
}

pub(crate) fn playback_enabled() -> bool {
    std::env::var("BURRATA_EXPERIMENTAL_BANDCAMP_PLAYBACK").is_ok_and(|value| value == "1")
}

pub(crate) fn validate_track_page_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid Bandcamp track URL")?;
    let host = url.host_str().ok_or("Invalid Bandcamp track host")?;
    let artist_label = host.strip_suffix(".bandcamp.com").unwrap_or_default();
    let artist_host = !artist_label.is_empty()
        && !artist_label.contains('.')
        && host.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !part.starts_with('-')
                && !part.ends_with('-')
        });
    let slug = url.path().strip_prefix("/track/").unwrap_or_default();
    if url.scheme() != "https"
        || !artist_host
        || has_explicit_port(raw)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || slug.is_empty()
        || slug.len() > 200
        || slug.contains('/')
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(
            "Use a public HTTPS artist Bandcamp track URL without query or fragment".into(),
        );
    }
    Ok(url)
}

fn validate_stream_url(raw: &str, track_id: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Bandcamp did not provide a usable audio stream")?;
    let host = url.host_str().unwrap_or_default();
    let cdn_host = host
        .strip_prefix('t')
        .and_then(|rest| rest.strip_suffix(".bcbits.com"))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        });
    let segments = url
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let stream_token = segments.get(1).copied().unwrap_or_default();
    if url.scheme() != "https"
        || !cdn_host
        || has_explicit_port(raw)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_none_or(str::is_empty)
        || url.fragment().is_some()
        || segments.len() != 4
        || segments.first() != Some(&"stream")
        || stream_token.len() != 32
        || !stream_token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || segments.get(2) != Some(&"mp3-128")
        || segments.get(3).copied() != Some(track_id)
    {
        return Err("Bandcamp returned an unsupported audio stream URL".into());
    }
    Ok(url)
}

fn parse_track_page(html: &str) -> Result<(String, String, String, f64, Url), String> {
    let data = crate::collection_sources::extract_tralbum_data(html)
        .ok_or("Bandcamp track page has no readable track data")?;
    let tracks = data
        .get("trackinfo")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() == 1)
        .ok_or("Bandcamp page is not a single track")?;
    let track = &tracks[0];
    let current = data.get("current").unwrap_or(&Value::Null);
    if current.get("streaming").and_then(Value::as_i64) != Some(1)
        || current.get("type").and_then(Value::as_str) != Some("track")
    {
        return Err("Bandcamp track is not publicly streamable".into());
    }
    let track_id = track
        .get("track_id")
        .or_else(|| track.get("id"))
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or("Bandcamp track ID is missing")?
        .to_string();
    if current
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        .as_deref()
        != Some(track_id.as_str())
    {
        return Err("Bandcamp track identity is inconsistent".into());
    }
    let title = track
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("Bandcamp track title is missing")?
        .trim()
        .to_owned();
    let artist = data
        .get("artist")
        .and_then(Value::as_str)
        .or_else(|| data.get("artist_name").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .ok_or("Bandcamp track artist is missing")?
        .trim()
        .to_owned();
    let duration = track
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or("Bandcamp track duration is missing")?;
    let stream = track
        .get("file")
        .and_then(Value::as_object)
        .and_then(|formats| formats.get("mp3-128"))
        .and_then(Value::as_str)
        .ok_or("Bandcamp track has no public MP3 stream")?;
    let stream = if stream.starts_with("//") {
        format!("https:{stream}")
    } else {
        stream.to_owned()
    };
    Ok((
        track_id.clone(),
        title,
        artist,
        duration,
        validate_stream_url(&stream, &track_id)?,
    ))
}

async fn wait_for_cancellation(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn bounded<T>(
    deadline: Instant,
    cancel: watch::Receiver<bool>,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::select! {
        result = tokio::time::timeout_at(deadline, future) => result.unwrap_or_else(|_| Err("Bandcamp audio resolution exceeded the playback deadline".into())),
        _ = wait_for_cancellation(cancel) => Err("Cancelled".into()),
    }
}

pub(crate) async fn resolve_track_page_audio(
    raw_url: &str,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<TrackAudio, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("Bandcamp audio resolution exceeded the playback deadline".into());
    }
    let page = validate_track_page_url(raw_url)?;
    let page_addresses = bounded(
        deadline,
        cancel.clone(),
        crate::acquisition::public_addresses(&page),
    )
    .await?;
    let page_host = page.host_str().ok_or("Bandcamp track host is missing")?;
    let client = Client::builder()
        .no_proxy()
        .user_agent(USER_AGENT)
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(PAGE_TIMEOUT)
        .resolve_to_addrs(page_host, &page_addresses)
        .build()
        .map_err(|_| "Could not initialize Bandcamp track access".to_owned())?;
    let response = bounded(deadline, cancel.clone(), async {
        client
            .get(page.clone())
            .send()
            .await
            .map_err(|_| "Bandcamp track page could not be reached".to_owned())
    })
    .await?;
    if response.status().is_redirection() {
        return Err("Bandcamp track page redirects are not supported".into());
    }
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 | 429 => "Bandcamp track access is restricted or rate limited".into(),
            _ => "Bandcamp track page returned an error".into(),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PAGE_BYTES as u64)
    {
        return Err("Bandcamp track page exceeds the size limit".into());
    }
    let html = bounded(deadline, cancel.clone(), async move {
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Bandcamp track page could not be read".to_owned())?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_PAGE_BYTES {
                return Err("Bandcamp track page exceeds the size limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await?;
    let (provider_id, title, artist, duration_seconds, stream_url) = parse_track_page(&html)?;
    let stream_addresses = bounded(
        deadline,
        cancel,
        crate::acquisition::public_addresses(&stream_url),
    )
    .await?;
    Ok(TrackAudio {
        provider_id,
        title,
        artist,
        duration_seconds,
        stream_url,
        stream_addresses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REDACTED_TRACK_PAGE: &str = r#"<html><div data-tralbum="{&quot;artist&quot;:&quot;Ergo&quot;,&quot;current&quot;:{&quot;type&quot;:&quot;track&quot;,&quot;id&quot;:292222166,&quot;streaming&quot;:1},&quot;trackinfo&quot;:[{&quot;track_id&quot;:292222166,&quot;title&quot;:&quot;Sorrows Of The Moon (featuring Mary Halvorson)&quot;,&quot;duration&quot;:310.254,&quot;file&quot;:{&quot;mp3-128&quot;:&quot;https://t4.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=FIXTURE_NOT_REAL&quot;}}]}"></div></html>"#;

    #[test]
    fn exact_public_bandcamp_track_url_is_required() {
        let valid = "https://cuneiformrecords.bandcamp.com/track/sorrows-of-the-moon-featuring-mary-halvorson-2";
        assert!(validate_track_page_url(valid).is_ok());
        for invalid in [
            "http://cuneiformrecords.bandcamp.com/track/song",
            "https://bandcamp.com/track/song",
            "https://cuneiformrecords.bandcamp.com/album/song",
            "https://cuneiformrecords.bandcamp.com/track/song?from=discover",
            "https://cuneiformrecords.bandcamp.com/track/song#player",
            "https://user:pass@cuneiformrecords.bandcamp.com/track/song",
            "https://cuneiformrecords.bandcamp.com:444/track/song",
            "https://cuneiformrecords.bandcamp.com:443/track/song",
            "https://x.y.bandcamp.com/track/song",
            "https://cuneiformrecords.bandcamp.com.evil.test/track/song",
        ] {
            assert!(validate_track_page_url(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn only_https_stream_paths_on_numbered_bcbits_hosts_are_allowed() {
        assert!(validate_stream_url("https://t4.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=FIXTURE_NOT_REAL", "292222166").is_ok());
        assert!(validate_stream_url("https://t12.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=FIXTURE_NOT_REAL", "292222166").is_ok());
        for invalid in [
            "http://t4.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
            "https://t4.bcbits.com.evil.test/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
            "https://evil.test/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
            "https://t4.bcbits.com/download/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
            "https://t4.bcbits.com/stream/not-a-stream/mp3-128/292222166?token=x",
            "https://t4.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166",
            "https://user@t4.bcbits.com/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
            "https://t4.bcbits.com:444/stream/0123456789abcdef0123456789abcdef/mp3-128/292222166?token=x",
        ] {
            assert!(validate_stream_url(invalid, "292222166").is_err(), "{invalid}");
        }
    }

    #[test]
    fn redacted_static_metadata_requires_full_track_identity_and_mp3_stream() {
        let (id, title, artist, duration, stream) = parse_track_page(REDACTED_TRACK_PAGE).unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/playback/ergo-bandcamp-source.json"
        ))
        .unwrap();
        assert_eq!(id, fixture["providerTrackId"].as_u64().unwrap().to_string());
        assert_eq!(title, fixture["title"].as_str().unwrap());
        assert_eq!(artist, fixture["artist"].as_str().unwrap());
        assert!((duration - fixture["trackDurationSeconds"].as_f64().unwrap()).abs() < 0.001);
        assert_eq!(
            stream.host_str(),
            fixture["staticFormat"]["audioHost"].as_str()
        );
        assert!(stream.path().ends_with("/mp3-128/292222166"));
    }

    #[test]
    fn truncated_or_restricted_public_metadata_is_rejected() {
        let truncated = REDACTED_TRACK_PAGE.replace("310.254", "30.0");
        let (_, _, _, duration, _) = parse_track_page(&truncated).unwrap();
        assert_eq!(duration, 30.0);
        let no_stream =
            REDACTED_TRACK_PAGE.replace("&quot;streaming&quot;:1", "&quot;streaming&quot;:0");
        assert!(parse_track_page(&no_stream).is_err());
    }

    #[tokio::test]
    async fn page_resolution_stops_when_cancelled_or_deadline_has_elapsed() {
        let (tx, cancel) = watch::channel(true);
        let result = resolve_track_page_audio(
            "https://cuneiformrecords.bandcamp.com/track/song",
            cancel,
            Instant::now() + Duration::from_secs(20),
        )
        .await;
        assert_eq!(result.unwrap_err(), "Cancelled");
        drop(tx);
        let (_tx, cancel) = watch::channel(false);
        let result = resolve_track_page_audio(
            "https://cuneiformrecords.bandcamp.com/track/song",
            cancel,
            Instant::now() - Duration::from_secs(1),
        )
        .await;
        assert!(result.unwrap_err().contains("deadline"));
    }
}
