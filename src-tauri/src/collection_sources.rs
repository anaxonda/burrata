//! Direct, bounded readers for public charts and collection pages.
//!
//! Source pages only provide an ordered tracklist. They do not trigger media
//! acquisition; imported metadata remains tied to its original page.
use crate::{
    browser_bridge::{BrowserEntry, BrowserSnapshot},
    connections::ConnectionManager,
};
use futures_util::{stream, StreamExt};
use reqwest::{redirect::Policy, Client, Url};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

const MAX_BYTES: usize = 6 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;
const MAX_BANDCAMP_ALBUMS: usize = 20;
const UA: &str = "Burrata/0.1 (music collection importer)";

pub async fn import_source(
    url: &str,
    connections: &ConnectionManager,
) -> Result<BrowserSnapshot, String> {
    let parsed = Url::parse(url)
        .map_err(|_| "Enter a valid public chart, user chart, or Bandcamp album URL")?;
    match parsed.host_str() {
        Some("shazam.com" | "www.shazam.com") => import_shazam(url).await,
        Some("last.fm" | "www.last.fm") => import_lastfm(url, connections).await,
        Some(host) if host.ends_with(".bandcamp.com") && host.len() > ".bandcamp.com".len() => import_bandcamp(url).await,
        _ => Err("Direct import supports public Shazam charts, Last.fm charts/user pages, and Bandcamp album pages. Open this source in the browser for other URLs.".into()),
    }
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(25))
        .redirect(Policy::none())
        .build()
        .map_err(|_| "Cannot configure the source importer".into())
}

async fn get_text(
    client: &Client,
    url: Url,
    allowed_hosts: &[&str],
) -> Result<(Url, String), String> {
    let mut current = url;
    for _ in 0..=4 {
        if current.scheme() != "https"
            || current.port().is_some_and(|p| p != 443)
            || !current.username().is_empty()
            || current.password().is_some()
            || !allowed_hosts.contains(&current.host_str().unwrap_or(""))
        {
            return Err("Source redirected outside its approved HTTPS host".into());
        }
        let response = client.get(current.clone()).send().await.map_err(|_| {
            "Source could not be reached. Open it in the browser or try again later.".to_owned()
        })?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("Source redirect was invalid")?;
            current = current
                .join(location)
                .map_err(|_| "Source redirect was invalid")?;
            continue;
        }
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 | 403 | 429 => "Source access is restricted or rate limited. Open the page in the browser to import it.".into(),
                _ => format!("Source returned HTTP {}", response.status().as_u16()),
            });
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err("Source page exceeds the import size limit".into());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Could not read source response".to_owned())?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err("Source page exceeds the import size limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        return Ok((current, body));
    }
    Err("Source redirected too many times".into())
}

fn check_source_url(input: &str, host: &str) -> Result<Url, String> {
    let url = Url::parse(input).map_err(|_| "Invalid source URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some(host)
        || url.port().is_some_and(|p| p != 443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use a public HTTPS URL on the source's official domain".into());
    }
    Ok(url)
}

async fn import_shazam(input: &str) -> Result<BrowserSnapshot, String> {
    let page = Url::parse(input).map_err(|_| "Invalid Shazam chart URL")?;
    let host = page.host_str().ok_or("Invalid Shazam host")?;
    let allowed = vec![host];
    let parts: Vec<_> = page
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let Some(charts) = parts.iter().position(|p| *p == "charts") else {
        return Err("Use a public Shazam chart URL such as /charts/top-200/world".into());
    };
    if parts.len() != charts + 3 || !matches!(parts[charts + 1], "top-200" | "top-50") {
        return Err("Direct Shazam import currently supports top-200 and top-50 charts. Use the browser for other chart types.".into());
    }
    let kind = parts[charts + 1];
    let region = parts[charts + 2];
    if !region
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid Shazam chart region".into());
    }
    let source = format!("https://{host}/charts/{kind}/{region}");
    let csv_url = check_source_url(
        &format!("https://{host}/services/charts/csv/{kind}/{region}"),
        host,
    )?;
    let client = http_client()?;
    let (_, body) = get_text(&client, csv_url, &allowed).await?;
    let entries = parse_shazam_csv(&body)?;
    let expected = if kind == "top-200" {
        Some(200)
    } else {
        Some(50)
    };
    let complete = entries.len() == expected.unwrap();
    let mut warnings = Vec::new();
    if !complete {
        warnings.push(format!(
            "Shazam returned {} of {} expected chart rows; import is marked partial.",
            entries.len(),
            expected.unwrap()
        ));
    }
    Ok(snapshot(
        "shazam",
        "chart",
        shazam_title(region, kind),
        source,
        entries,
        expected,
        complete,
        warnings,
    ))
}

fn shazam_title(region: &str, kind: &str) -> String {
    let area = if region == "world" {
        "Global".to_owned()
    } else {
        region
            .split('-')
            .map(|s| {
                let mut c = s.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "Shazam {area} {}",
        if kind == "top-200" {
            "Top 200"
        } else {
            "Top 50"
        }
    )
}

fn parse_shazam_csv(body: &str) -> Result<Vec<BrowserEntry>, String> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    // Shazam prepends a quoted chart description before the actual CSV header.
    // Locate a recognized header row rather than treating that prelude as data.
    let mut offset = 0;
    let mut csv_body = None;
    for line in body.split_inclusive('\n') {
        let candidate = line.trim_end_matches(['\r', '\n']);
        let mut probe = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(candidate.as_bytes());
        if let Some(Ok(record)) = probe.records().next() {
            let has = |name: &str| {
                record
                    .iter()
                    .any(|cell| cell.trim().eq_ignore_ascii_case(name))
            };
            if (has("rank") || has("position")) && has("artist") && has("title") {
                csv_body = Some(&body[offset..]);
                break;
            }
        }
        offset += line.len();
    }
    let body = csv_body.ok_or("Shazam chart CSV has no recognized Rank, Artist, Title header")?;
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(body.as_bytes());
    let headers = reader
        .headers()
        .map_err(|_| "Shazam chart CSV has no header")?
        .clone();
    let col = |names: &[&str]| {
        headers
            .iter()
            .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
    };
    let rank_i = col(&["position", "rank", "chart position", "#"]);
    let title_i = col(&["title", "track", "track name", "song"]);
    let artist_i = col(&["artist", "artist name"]);
    let album_i = col(&["album", "release"]);
    let url_i = col(&["url", "track url", "shazam url"]);
    let artwork_i = col(&["artwork", "artwork url", "image", "coverart", "cover art"]);
    let isrc_i = col(&["isrc"]);
    let artist_url_i = col(&["artist url"]);
    let (Some(title_i), Some(artist_i)) = (title_i, artist_i) else {
        return Err("Shazam CSV format changed; open the chart in the browser instead.".into());
    };
    let mut entries = Vec::new();
    for row in reader.records() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let row = row.map_err(|_| "Shazam chart CSV contains a malformed row")?;
        let title = row.get(title_i).unwrap_or("").trim();
        let artist = row.get(artist_i).unwrap_or("").trim();
        if title.is_empty() || artist.is_empty() {
            continue;
        }
        let rank = rank_i
            .and_then(|i| row.get(i))
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0);
        let entry_url = url_i
            .and_then(|i| row.get(i))
            .filter(|s| {
                s.starts_with("https://www.shazam.com/") || s.starts_with("https://shazam.com/")
            })
            .map(str::to_owned);
        let entry = BrowserEntry {
            metadata: serde_json::json!({"artistUrl":artist_url_i.and_then(|i|row.get(i)).and_then(|s|crate::import_metadata::public_url(s).ok())}),
            provider_id: None,
            source_url: entry_url,
            artist: artist.into(),
            title: title.into(),
            version: None,
            position: (entries.len() + 1) as u32,
            rank,
            duration_seconds: None,
            bpm: None,
            key: None,
            album: album_i
                .and_then(|i| row.get(i))
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            label: None,
            genre: None,
            release_date: None,
            artwork_url: artwork_i
                .and_then(|i| row.get(i))
                .and_then(|s| crate::import_metadata::public_url(s).ok()),
            isrc: isrc_i
                .and_then(|i| row.get(i))
                .and_then(crate::recording_identity::normalize_isrc),
            catalog_number: None,
        };
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(
            "Shazam chart CSV contained no readable track rows; open the chart in the browser."
                .into(),
        );
    }
    Ok(entries)
}

async fn import_lastfm(
    input: &str,
    connections: &ConnectionManager,
) -> Result<BrowserSnapshot, String> {
    let url = Url::parse(input).map_err(|_| "Invalid Last.fm URL")?;
    let host = url.host_str().unwrap_or("");
    if host != "last.fm" && host != "www.last.fm" {
        return Err("Use Last.fm's public website".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    let (method, user, tag, mut title) = if segments.as_slice() == ["music", "charts"]
        || segments.as_slice() == ["charts"]
    {
        (
            "chart.getTopTracks",
            None,
            None,
            "Last.fm Top Tracks".to_owned(),
        )
    } else if segments.len() == 3 && segments[0] == "tag" && segments[2] == "tracks" {
        let tag = lastfm_tag_name_from_url(&url)?;
        (
            "tag.getTopTracks",
            None,
            Some(tag.clone()),
            format!("Last.fm {tag} Top Tracks"),
        )
    } else if segments.first() == Some(&"user")
        && segments.len() >= 2
        && segments.len() <= 4
        && (segments.len() == 2
            || segments[2] == "library" && (segments.len() == 3 || segments[3] == "tracks"))
        && valid_slug(segments[1])
    {
        (
            "user.getTopTracks",
            Some(segments[1].to_owned()),
            None,
            format!("{}'s Top Tracks", segments[1]),
        )
    } else {
        return Err("Use Last.fm /music/charts, /tag/{tag}/tracks, or a /user/{name}/library/tracks URL. Other pages can be imported in the browser.".into());
    };
    let period = if user.is_some() {
        lastfm_period(&url)?
    } else {
        if url.query_pairs().any(|(key, value)| {
            matches!(key.as_ref(), "period" | "date_preset" | "from" | "to") && !value.is_empty()
        }) {
            return Err("Date-filtered Last.fm charts require a user URL; open this chart in the browser instead.".into());
        }
        None
    };
    if let Some(period) = period.as_deref() {
        title.push_str(&format!(" ({})", lastfm_period_label(period)));
    }
    let mut public_page_unparsed = false;
    if user.is_none() && tag.is_none() {
        let public_page = Url::parse("https://www.last.fm/music/charts").unwrap();
        if let Ok((_, html)) =
            get_text(&http_client()?, public_page, &["last.fm", "www.last.fm"]).await
        {
            if let Some(snapshot) = parse_lastfm_public_chart(&html) {
                return Ok(snapshot);
            }
            public_page_unparsed = true;
        }
    }
    let api_key = connections
        .credential("lastfm")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());
    let Some(api_key) = api_key else {
        return Err(if public_page_unparsed {
            "Last.fm's public charts page requires client-side rendering or verification, so Burrata could not read its rows. Connect a Last.fm API key or use browser import."
        } else if tag.is_some() {
            "Connect a Last.fm API key for direct tag charts, or use browser import."
        } else if user.is_some() {
            "Connect a Last.fm API key for direct user charts, or use browser import."
        } else {
            "Connect a Last.fm API key for direct charts, or use browser import."
        }
        .into());
    };
    let client = http_client()?;
    let mut entries = Vec::new();
    let mut total = None;
    let page_size = 200usize;
    for page in 1..=5u32 {
        let mut api = Url::parse("https://ws.audioscrobbler.com/2.0/").unwrap();
        api.query_pairs_mut()
            .append_pair("method", method)
            .append_pair("api_key", &api_key)
            .append_pair("format", "json")
            .append_pair("page", &page.to_string())
            .append_pair("limit", &page_size.to_string());
        if let Some(user) = &user {
            api.query_pairs_mut()
                .append_pair("user", user)
                .append_pair("period", period.as_deref().unwrap_or("overall"));
        }
        if let Some(tag) = &tag {
            api.query_pairs_mut().append_pair("tag", tag);
        }
        let (_, body) = get_text(&client, api, &["ws.audioscrobbler.com"]).await?;
        let value: Value =
            serde_json::from_str(&body).map_err(|_| "Last.fm returned invalid chart data")?;
        if let Some(error) = value.get("error") {
            return Err(format!(
                "Last.fm could not provide this chart (error {}). Check the user name and API connection.",
                error.as_u64().unwrap_or(0)
            ));
        }
        // JSON tag/chart endpoints use `tracks`; user top tracks uses `toptracks`.
        let tracks = lastfm_track_list(&value, method).ok_or("Last.fm returned no chart tracks")?;
        let rows = tracks
            .get("track")
            .and_then(Value::as_array)
            .ok_or("Last.fm returned no chart tracks")?;
        total = tracks
            .get("@attr")
            .and_then(|a| a.get("total"))
            .and_then(|v| {
                v.as_str()
                    .and_then(|x| x.parse().ok())
                    .or_else(|| v.as_u64().map(|x| x as usize))
            })
            .or(total);
        for row in rows {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            let title = value_str(&row["name"]);
            let artist =
                value_str(&row["artist"]["name"]).or_else(|| value_str(&row["artist"]["#text"]));
            let (Some(title), Some(artist)) = (title, artist) else {
                continue;
            };
            let rank = row
                .get("@attr")
                .and_then(|v| v.get("rank"))
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse().ok())
                        .or_else(|| v.as_u64().map(|n| n as u32))
                })
                .unwrap_or((entries.len() + 1) as u32);
            let duration = value_str(&row["duration"])
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| *n > 0.0);
            let track_url = value_str(&row["url"]).and_then(|s| normalize_lastfm_track_url(&s));
            entries.push(BrowserEntry {
                metadata: serde_json::json!({"musicbrainzId":value_str(&row["mbid"]),"artistUrl":value_str(&row["artist"]["url"]),"plays":value_str(&row["playcount"]).and_then(|v|v.parse::<u64>().ok()),"listeners":value_str(&row["listeners"]).and_then(|v|v.parse::<u64>().ok())}),
                provider_id: value_str(&row["mbid"]).filter(|s| !s.is_empty()),
                source_url: track_url,
                artist,
                title,
                version: None,
                position: (entries.len() + 1) as u32,
                rank: Some(rank),
                duration_seconds: duration,
                bpm: None,
                key: None,
                album: value_str(&row["album"]["#text"]),
                label: None,
                genre: None,
                release_date: None,
                artwork_url: row["image"].as_array().and_then(|images|images.iter().rev().find_map(|image|value_str(&image["#text"]).filter(|s|s.starts_with("https://")))),
                isrc: None,
                catalog_number: None,
            });
        }
        if rows.len() < page_size
            || entries.len() >= MAX_ENTRIES
            || total.is_some_and(|n| entries.len() >= n)
        {
            break;
        }
    }
    if entries.is_empty() {
        return Err("Last.fm returned no usable tracks".into());
    }
    let complete = total.is_some_and(|n| n <= MAX_ENTRIES && entries.len() == n);
    let expected = total.filter(|n| *n <= MAX_ENTRIES);
    let warnings = if complete {
        Vec::new()
    } else {
        vec![format!(
            "Imported up to {} Last.fm entries ({}); remaining total is not captured, so the snapshot is partial.",
            entries.len(),
            total
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown total".into())
        )]
    };
    let is_user_collection = user.is_some();
    let canonical = if let Some(user) = user {
        lastfm_user_source_url(&user, period.as_deref())
    } else if tag.is_some() {
        lastfm_tag_source_url(&url)
    } else {
        format!("https://{host}/music/charts")
    };
    Ok(snapshot(
        "lastfm",
        if is_user_collection {
            "playlist"
        } else {
            "chart"
        },
        title,
        canonical,
        entries,
        expected,
        complete,
        warnings,
    ))
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn lastfm_period(url: &Url) -> Result<Option<String>, String> {
    let mut explicit_period = None;
    let mut explicit_preset = None;
    let mut has_range = false;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "from" | "to" if !value.is_empty() => has_range = true,
            "period" if !value.is_empty() => {
                let normalized = value.to_ascii_lowercase();
                if !matches!(
                    normalized.as_str(),
                    "overall" | "7day" | "1month" | "3month" | "6month" | "12month"
                ) {
                    return Err("Unsupported Last.fm period. Use overall, 7day, 1month, 3month, 6month, or 12month; use browser import for custom date ranges.".into());
                }
                explicit_period = Some(normalized);
            }
            "date_preset" if !value.is_empty() => {
                let mapped = match value.to_ascii_uppercase().as_str() {
                    "ALL_TIME" => "overall",
                    "LAST_7_DAYS" => "7day",
                    "LAST_30_DAYS" => "1month",
                    "LAST_90_DAYS" => "3month",
                    "LAST_180_DAYS" => "6month",
                    "LAST_365_DAYS" => "12month",
                    _ => return Err("This Last.fm date preset is not supported by the direct importer; use browser import for custom date ranges.".into()),
                };
                explicit_preset = Some(mapped.to_owned());
            }
            _ => {}
        }
    }
    if has_range {
        return Err("Custom Last.fm from/to date ranges are not supported by the direct importer; use browser import.".into());
    }
    if explicit_period.is_some() && explicit_preset.is_some() && explicit_period != explicit_preset
    {
        return Err(
            "The Last.fm period and date preset conflict; use a single supported period.".into(),
        );
    }
    Ok(explicit_period.or(explicit_preset))
}

fn lastfm_period_label(period: &str) -> &'static str {
    match period {
        "7day" => "last 7 days",
        "1month" => "last month",
        "3month" => "last 3 months",
        "6month" => "last 6 months",
        "12month" => "last 12 months",
        _ => "all time",
    }
}

fn lastfm_user_source_url(user: &str, period: Option<&str>) -> String {
    let mut url = Url::parse(&format!("https://www.last.fm/user/{user}/library/tracks"))
        .expect("validated Last.fm user");
    if let Some(period) = period {
        let preset = match period {
            "overall" => "ALL_TIME",
            "7day" => "LAST_7_DAYS",
            "1month" => "LAST_30_DAYS",
            "3month" => "LAST_90_DAYS",
            "6month" => "LAST_180_DAYS",
            "12month" => "LAST_365_DAYS",
            _ => "ALL_TIME",
        };
        url.query_pairs_mut().append_pair("date_preset", preset);
    }
    url.to_string()
}

fn decode_lastfm_tag(segment: &str) -> Result<String, String> {
    // Last.fm uses `+` for spaces in tag path slugs. Form decoding also handles
    // percent escapes while preserving an explicitly encoded `%2B` as a plus.
    let query = format!("tag={segment}");
    let tag = url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "tag")
        .map(|(_, value)| value.trim().to_owned())
        .filter(|tag| {
            !tag.is_empty()
                && tag.len() <= 120
                && !tag
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        })
        .ok_or("Invalid Last.fm tag path")?;
    Ok(tag)
}

fn lastfm_tag_name_from_url(url: &Url) -> Result<String, String> {
    let segments: Vec<_> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() != 3 || segments[0] != "tag" || segments[2] != "tracks" {
        return Err("Use a Last.fm /tag/{tag}/tracks URL".into());
    }
    decode_lastfm_tag(segments[1])
}

fn lastfm_tag_source_url(input: &Url) -> String {
    let mut url = input.clone();
    let _ = url.set_host(Some("www.last.fm"));
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn lastfm_track_list<'a>(value: &'a Value, method: &str) -> Option<&'a Value> {
    value.get(if method == "user.getTopTracks" {
        "toptracks"
    } else {
        "tracks"
    })
}

fn parse_lastfm_public_chart(html: &str) -> Option<BrowserSnapshot> {
    let document = scraper::Html::parse_document(html);
    // Last.fm has shipped the same visible chart table with and without the
    // legacy `chartlist-row` class. Match the semantic cells and tolerate
    // either table rows or list-style rows.
    let rows = scraper::Selector::parse("tr, li, .chartlist-row, [data-track-row]").ok()?;
    let title_selector = scraper::Selector::parse(
        "td.chartlist-name a, .chartlist-name a, td.chartlist-name, .chartlist-name",
    )
    .ok()?;
    let artist_selector = scraper::Selector::parse(
        "td.chartlist-artist a, .chartlist-artist a, td.chartlist-artist, .chartlist-artist",
    )
    .ok()?;
    let mut entries = Vec::new();
    for row in document.select(&rows).take(MAX_ENTRIES) {
        let title_anchor = row.select(&title_selector).next();
        let Some(title) = title_anchor
            .map(|node| node.text().collect::<String>())
            .or_else(|| row.value().attr("data-track-name").map(str::to_owned))
        else {
            continue;
        };
        let artist = row
            .select(&artist_selector)
            .next()
            .map(|node| node.text().collect::<String>())
            .or_else(|| row.value().attr("data-artist-name").map(str::to_owned));
        let Some(artist) = artist else {
            continue;
        };
        let title = title.trim().to_owned();
        let artist = artist.trim().to_owned();
        if title.is_empty() || artist.is_empty() {
            continue;
        }
        let href = title_anchor.and_then(|node| node.value().attr("href"));
        let track_url = href
            .and_then(|path| Url::parse("https://www.last.fm").ok()?.join(path).ok())
            .filter(|u| u.host_str() == Some("www.last.fm") && u.scheme() == "https")
            .map(|u| u.to_string());
        let position = (entries.len() + 1) as u32;
        entries.push(BrowserEntry {
            metadata: serde_json::json!({"artistUrl":row.select(&artist_selector).next().and_then(|n|n.value().attr("href")).and_then(|s|Url::parse("https://www.last.fm").ok()?.join(s).ok()).map(|u|u.to_string())}),
            provider_id: None,
            source_url: track_url,
            artist,
            title,
            version: None,
            position,
            rank: scraper::Selector::parse(".chartlist-index,.chartlist-rank").ok().and_then(|selector|row.select(&selector).next().and_then(|n|n.text().collect::<String>().trim().parse::<u32>().ok())).filter(|n|*n>0),
            duration_seconds: None,
            bpm: None,
            key: None,
            album: None,
            label: None,
            genre: None,
            release_date: None,
            artwork_url: scraper::Selector::parse("img").ok().and_then(|selector|row.select(&selector).next().and_then(|n|n.value().attr("src")).and_then(|s|crate::import_metadata::public_url(s).ok())),
            isrc: None,
            catalog_number: None,
        });
    }
    if entries.is_empty() {
        return None;
    }
    Some(snapshot(
        "lastfm",
        "chart",
        "Last.fm Top Tracks".into(),
        "https://www.last.fm/music/charts".into(),
        entries,
        None,
        false,
        vec!["Only the publicly rendered Last.fm chart rows were captured; the snapshot is marked partial.".into()],
    ))
}

fn value_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn normalize_lastfm_track_url(s: &str) -> Option<String> {
    let mut url = Url::parse(s).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("last.fm" | "www.last.fm"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| !matches!(port, 80 | 443))
    {
        return None;
    }
    url.set_scheme("https").ok()?;
    url.set_port(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

pub(crate) async fn import_bandcamp(input: &str) -> Result<BrowserSnapshot, String> {
    let source = validate_bandcamp_album_url(input)?;
    let url = Url::parse(&source).map_err(|_| "Invalid Bandcamp URL")?;
    let host = url.host_str().ok_or("Invalid Bandcamp host")?;
    let client = http_client()?;
    let (_, html) = get_text(&client, Url::parse(&source).unwrap(), &[host]).await?;
    let data = extract_tralbum_data(&html).ok_or(
        "Bandcamp album page has no readable tracklist data; open it in the browser instead.",
    )?;
    let tracks = data
        .get("trackinfo")
        .and_then(Value::as_array)
        .ok_or("Bandcamp album page has no tracklist")?;
    let artist = data
        .get("artist")
        .and_then(Value::as_str)
        .or_else(|| data.get("artist_name").and_then(Value::as_str))
        .unwrap_or("")
        .trim();
    let album = data
        .get("current")
        .and_then(|v| v.get("title"))
        .and_then(Value::as_str)
        .or_else(|| data.get("title").and_then(Value::as_str))
        .unwrap_or("")
        .trim();
    if artist.is_empty() || album.is_empty() {
        return Err("Bandcamp album is missing artist or album title".into());
    }
    let mut entries = Vec::new();
    let details = crate::import_metadata::bandcamp_page(&data, &html, &url);
    for row in tracks.iter().take(MAX_ENTRIES) {
        let title = row
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            continue;
        }
        let n = row
            .get("track_num")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or((entries.len() + 1) as u32);
        let track_url = row
            .get("title_link")
            .and_then(Value::as_str)
            .and_then(|p| Url::parse(&source).ok()?.join(p).ok())
            .filter(|u| u.scheme() == "https" && u.host_str() == Some(host))
            .map(|u| u.to_string());
        entries.push(BrowserEntry {
            metadata: {
                let mut m = details["metadata"].clone();
                m["trackNumber"] = serde_json::json!(row["track_num"]
                    .as_u64()
                    .unwrap_or(entries.len() as u64 + 1));
                m
            },
            provider_id: row["track_id"].as_u64().map(|id| id.to_string()),
            source_url: track_url,
            artist: artist.to_owned(),
            title: title.to_owned(),
            version: None,
            position: (entries.len() + 1) as u32,
            rank: None,
            duration_seconds: row
                .get("duration")
                .and_then(Value::as_f64)
                .filter(|d| *d > 0.0),
            bpm: None,
            key: None,
            album: Some(album.to_owned()),
            label: None,
            genre: None,
            release_date: details["releaseDate"].as_str().map(str::to_owned),
            artwork_url: details["artworkUrl"].as_str().map(str::to_owned),
            isrc: None,
            catalog_number: None,
        });
    }
    if entries.is_empty() {
        return Err("Bandcamp album has no named tracks to import".into());
    }
    let complete = entries.len() == tracks.len() && tracks.len() <= MAX_ENTRIES;
    let expected = (tracks.len() <= MAX_ENTRIES).then_some(tracks.len());
    let warnings = if complete {
        vec!["This is an album tracklist; imported entries are not a chart and album tracks are not inferred as standalone catalog recordings.".into()]
    } else {
        vec!["Bandcamp album tracklist exceeded supported bounds or had unnamed tracks; snapshot is partial.".into()]
    };
    Ok(snapshot(
        "bandcamp",
        "album",
        album.to_owned(),
        source,
        entries,
        expected,
        complete,
        warnings,
    ))
}

/// Import album cards rendered by Bandcamp Discover by fetching each album's
/// explicit public tracklist. Album cards themselves never become tracks.
pub async fn import_bandcamp_album_collection(
    source_url: &str,
    title: &str,
    album_urls: Vec<String>,
) -> Result<BrowserSnapshot, String> {
    let source = validate_bandcamp_discover_url(source_url)?;
    let title = title.trim();
    if title.is_empty() || title.len() > 1000 {
        return Err(
            "Bandcamp Discover page title is required and limited to 1,000 characters".into(),
        );
    }
    if album_urls.is_empty() || album_urls.len() > MAX_BANDCAMP_ALBUMS {
        return Err(format!(
            "Select 1–{MAX_BANDCAMP_ALBUMS} rendered Bandcamp album cards"
        ));
    }
    let mut urls = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in album_urls {
        let canonical = validate_bandcamp_album_url(&raw)?;
        if seen.insert(canonical.clone()) {
            urls.push(canonical);
        }
    }
    let fetches = stream::iter(
        urls.into_iter()
            .enumerate()
            .map(|(index, url)| async move { (index, url.clone(), import_bandcamp(&url).await) }),
    )
    .buffer_unordered(4)
    .collect::<Vec<_>>();
    let mut fetched = timeout(Duration::from_secs(110), fetches)
        .await
        .map_err(|_| "Bandcamp album collection import timed out; retry with fewer cards")?;
    fetched.sort_by_key(|(index, _, _)| *index);
    let snapshot = combine_bandcamp_album_snapshots(&source, title, title, fetched);
    if snapshot.entries.is_empty() {
        return Err("No explicit Bandcamp album tracklists could be imported. Open an album in the browser and retry.".into());
    }
    Ok(snapshot)
}

fn validate_bandcamp_discover_url(input: &str) -> Result<String, String> {
    let mut url = Url::parse(input).map_err(|_| "Invalid Bandcamp Discover URL")?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("bandcamp.com" | "www.bandcamp.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Use a public Bandcamp /discover/{tag} page".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() != 2 || segments[0] != "discover" || !valid_slug(segments[1]) {
        return Err("Use a public Bandcamp /discover/{tag} page".into());
    }
    let sort = url
        .query_pairs()
        .find(|(key, _)| key == "sort")
        .map(|(_, v)| v.into_owned())
        .filter(|v| matches!(v.as_str(), "top" | "new" | "rec" | "date"));
    url.set_query(None);
    if let Some(sort) = sort {
        url.query_pairs_mut().append_pair("sort", &sort);
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

fn validate_bandcamp_album_url(input: &str) -> Result<String, String> {
    let mut url = Url::parse(input).map_err(|_| "Invalid Bandcamp album URL")?;
    let host = url.host_str().ok_or("Invalid Bandcamp album host")?;
    if url.scheme() != "https"
        || !host.ends_with(".bandcamp.com")
        || host == "bandcamp.com"
        || !host.split('.').all(valid_slug)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Use an HTTPS artist-hosted Bandcamp album page".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    // Album slugs derive from release titles and can exceed the short slug
    // limit used for provider IDs and host labels.
    if segments.len() != 2
        || segments[0] != "album"
        || segments[1].len() > 2048
        || !segments[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("Bandcamp Discover cards must link to artist-hosted album pages".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn combine_bandcamp_album_snapshots(
    source: &str,
    title: &str,
    raw_page_title: &str,
    mut fetched: Vec<(usize, String, Result<BrowserSnapshot, String>)>,
) -> BrowserSnapshot {
    fetched.sort_by_key(|(index, _, _)| *index);
    let requested = fetched.len();
    let album_urls = fetched
        .iter()
        .map(|(_, url, _)| url.clone())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut warnings = vec![
        "Bandcamp Discover lists releases, not a song chart; only explicit album tracklists were imported. The result is a partial collection of the rendered cards.".to_owned(),
    ];
    let mut failed_sources = Vec::new();
    let mut imported_albums = 0usize;
    let mut truncated = false;
    for (album_index, album_url, result) in fetched {
        match result {
            Ok(album) if album.provider == "bandcamp" && album.kind == "album" => {
                imported_albums += 1;
                for mut entry in album.entries {
                    if entries.len() >= MAX_ENTRIES {
                        truncated = true;
                        break;
                    }
                    if !entry.metadata.is_object() {
                        entry.metadata = serde_json::json!({});
                    }
                    entry.metadata["albumOrder"] = serde_json::json!(album_index + 1);
                    entry.rank = None;
                    entry.position = entries.len() as u32 + 1;
                    if entry.source_url.is_none() {
                        entry.source_url = Some(album_url.clone());
                    }
                    entries.push(entry);
                }
            }
            Ok(_) => warnings.push(
                "A Bandcamp album page returned an unexpected result and was skipped.".into(),
            ),
            Err(_) => {
                failed_sources.push(album_url.clone());
                let route = Url::parse(&album_url)
                    .ok()
                    .map(|url| format!("{}{}", url.host_str().unwrap_or("Bandcamp"), url.path()))
                    .unwrap_or_else(|| "Bandcamp album".into());
                warnings.push(format!(
                    "Could not read the explicit tracklist from {route}; other albums were kept."
                ));
            }
        }
        if truncated {
            break;
        }
    }
    if truncated {
        warnings.push(format!(
            "Import stopped at the {MAX_ENTRIES}-track safety limit."
        ));
    }
    if imported_albums < requested {
        warnings.push(format!(
            "Imported {imported_albums} of {requested} rendered album cards."
        ));
    }
    if entries.len() >= MAX_ENTRIES {
        entries.truncate(MAX_ENTRIES);
    }
    let mut result = snapshot(
        "bandcamp",
        "collection",
        normalize_bandcamp_discover_title(title, source),
        source.to_owned(),
        entries,
        None,
        false,
        warnings,
    )
    .with_raw_page_title(raw_page_title);
    result.failed_sources = failed_sources;
    result.album_urls = album_urls;
    result
}

fn normalize_bandcamp_discover_title(raw: &str, source: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    let suffixes = [
        " music and merch on bandcamp",
        " | bandcamp",
        " - bandcamp",
        " on bandcamp",
    ];
    for suffix in suffixes {
        if lower.ends_with(suffix) {
            let cleaned = raw[..raw.len() - suffix.len()]
                .trim()
                .trim_end_matches([':', '|', '-', '–'])
                .trim();
            if !cleaned.is_empty() && cleaned.len() <= 1000 {
                return title_case_genre(cleaned);
            }
        }
    }
    if raw.is_empty() || raw.eq_ignore_ascii_case("bandcamp") {
        if let Ok(url) = Url::parse(source) {
            if let Some(tag) = url.path_segments().and_then(|mut parts| {
                parts.next();
                parts.next()
            }) {
                if tag.eq_ignore_ascii_case("hip-hop-rap") {
                    return "Hip-Hop / Rap".into();
                }
                let words = tag.replace('+', " ").replace('-', " ").replace('_', " ");
                let title = words
                    .split_whitespace()
                    .map(|word| {
                        let mut chars = word.chars();
                        chars
                            .next()
                            .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !title.is_empty() {
                    return title;
                }
            }
        }
    }
    raw.to_owned()
}

fn title_case_genre(label: &str) -> String {
    label
        .split_whitespace()
        .map(|word| {
            word.split('-')
                .map(|part| {
                    if part.chars().skip(1).any(char::is_uppercase) {
                        return part.to_owned();
                    }
                    let mut chars = part.chars();
                    chars
                        .next()
                        .map(|first| {
                            first.to_uppercase().collect::<String>()
                                + &chars.as_str().to_lowercase()
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

trait RawPageTitle {
    fn with_raw_page_title(self, raw_page_title: &str) -> Self;
}

impl RawPageTitle for BrowserSnapshot {
    fn with_raw_page_title(mut self, raw_page_title: &str) -> Self {
        self.raw_page_title = Some(raw_page_title.trim().to_owned());
        self
    }
}

pub(crate) fn extract_tralbum_data(html: &str) -> Option<Value> {
    // Bandcamp currently embeds a JSON object in its public page script. Keep
    // extraction bracket-aware so braces inside strings do not truncate it.
    let anchors = ["TralbumData =", "TralbumData=", "data-tralbum="];
    for anchor in anchors {
        let Some(i) = html.find(anchor) else {
            continue;
        };
        let tail = &html[i + anchor.len()..];
        let start = if anchor.starts_with("data-tralbum") {
            0
        } else {
            tail.find('{')?
        };
        let candidate = if anchor.starts_with("data-tralbum") {
            let quote = tail.chars().next()?;
            let end = tail[1..].find(quote)? + 1;
            let encoded = &tail[1..end];
            let decoded = encoded
                .replace("&quot;", "\"")
                .replace("&amp;", "&")
                .replace("&#39;", "'");
            serde_json::from_str::<Value>(&decoded).ok()?
        } else {
            let offset = start;
            let bytes = tail.as_bytes();
            let mut depth = 0i32;
            let mut string = false;
            let mut escaped = false;
            let mut end = None;
            for (p, b) in bytes.iter().enumerate().skip(offset) {
                if string {
                    if escaped {
                        escaped = false;
                    } else if *b == b'\\' {
                        escaped = true;
                    } else if *b == b'"' {
                        string = false;
                    }
                    continue;
                }
                match *b {
                    b'"' => string = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(p + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            serde_json::from_str::<Value>(&tail[offset..end?]).ok()?
        };
        if candidate.get("trackinfo").is_some() {
            return Some(candidate);
        }
    }
    None
}

fn snapshot(
    provider: &str,
    kind: &str,
    title: String,
    source: String,
    entries: Vec<BrowserEntry>,
    expected_total: Option<usize>,
    complete: bool,
    warnings: Vec<String>,
) -> BrowserSnapshot {
    BrowserSnapshot {
        failed_sources: vec![],
        album_urls: vec![],
        schema_version: 1,
        provider: provider.into(),
        kind: kind.into(),
        title,
        raw_page_title: None,
        source_url: source,
        captured_at: chrono::Utc::now().to_rfc3339(),
        complete,
        expected_total,
        warnings,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, sync::Arc};

    #[test]
    fn bandcamp_discover_urls_accept_only_discover_and_artist_album_routes() {
        assert_eq!(
            validate_bandcamp_discover_url("https://bandcamp.com/discover/hip-hop-rap?from=x")
                .unwrap(),
            "https://bandcamp.com/discover/hip-hop-rap"
        );
        assert!(validate_bandcamp_discover_url("https://artist.bandcamp.com/album/x").is_err());
        assert_eq!(
            validate_bandcamp_album_url(
                "https://daupe.bandcamp.com/album/beemer-on-broadway?from=discover"
            )
            .unwrap(),
            "https://daupe.bandcamp.com/album/beemer-on-broadway"
        );
        for invalid in [
            "https://bandcamp.com/album/x",
            "https://evil.invalid/album/x",
            "http://artist.bandcamp.com/album/x",
            "https://artist.bandcamp.com/track/x",
            "https://u:p@artist.bandcamp.com/album/x",
        ] {
            assert!(validate_bandcamp_album_url(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn bandcamp_album_urls_accept_long_release_titles_with_bounded_paths() {
        let slug = "a-long-alternative-release-title-".repeat(8);
        assert!(slug.len() > 100);
        let canonical = format!("https://artist.bandcamp.com/album/{slug}");
        assert_eq!(
            validate_bandcamp_album_url(&format!("{canonical}?from=discover_page#tracks")).unwrap(),
            canonical
        );
        assert!(validate_bandcamp_album_url(&format!(
            "https://artist.bandcamp.com/album/{}",
            "a".repeat(2048)
        ))
        .is_ok());
        for path in ["a".repeat(2049), "bad%2Fslug".into(), "bad/slug".into()] {
            assert!(validate_bandcamp_album_url(&format!(
                "https://artist.bandcamp.com/album/{path}"
            ))
            .is_err());
        }
    }

    #[test]
    fn bandcamp_discover_title_strips_storefront_boilerplate() {
        assert_eq!(
            normalize_bandcamp_discover_title(
                "Jazz music and merch on Bandcamp",
                "https://bandcamp.com/discover/jazz"
            ),
            "Jazz"
        );
        assert_eq!(
            normalize_bandcamp_discover_title(
                "Hip-hop / Rap music and merch on Bandcamp",
                "https://bandcamp.com/discover/hip-hop-rap"
            ),
            "Hip-Hop / Rap"
        );
        assert_eq!(
            normalize_bandcamp_discover_title(
                "Bandcamp",
                "https://bandcamp.com/discover/ambient-electronic"
            ),
            "Ambient Electronic"
        );
        assert_eq!(
            normalize_bandcamp_discover_title(
                "Bandcamp",
                "https://bandcamp.com/discover/hip-hop-rap"
            ),
            "Hip-Hop / Rap"
        );
    }

    #[test]
    fn discover_combines_only_album_tracklists_and_preserves_partial_warnings() {
        let album: BrowserSnapshot = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"provider":"bandcamp","kind":"album","title":"Record",
            "sourceUrl":"https://artist.bandcamp.com/album/record","capturedAt":"2026-10-13T00:00:00Z",
            "complete":false,"expectedTotal":1,"warnings":[],"entries":[{
                "providerId":"123","sourceUrl":"https://artist.bandcamp.com/track/song","artist":"Artist","title":"Song",
                "position":1,"rank":1,"album":"Record"
            }]
        })).unwrap();
        let result = combine_bandcamp_album_snapshots(
            "https://bandcamp.com/discover/hip-hop-rap",
            "Hip-hop / Rap music and merch on Bandcamp",
            "Hip-hop / Rap music and merch on Bandcamp",
            vec![
                (
                    0,
                    "https://artist.bandcamp.com/album/record".into(),
                    Ok(album),
                ),
                (
                    1,
                    "https://other.bandcamp.com/album/private".into(),
                    Err("restricted".into()),
                ),
            ],
        );
        assert_eq!(result.kind, "collection");
        assert_eq!(result.title, "Hip-Hop / Rap");
        assert_eq!(
            result.raw_page_title.as_deref(),
            Some("Hip-hop / Rap music and merch on Bandcamp")
        );
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].title, "Song");
        assert_eq!(result.entries[0].rank, None);
        assert_eq!(result.entries[0].position, 1);
        assert!(!result.complete);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("other.bandcamp.com/album/private")));
    }

    async fn validate_and_persist(source_url: &str) -> (BrowserSnapshot, usize) {
        let store = Arc::new(crate::storage::Store::open(Path::new(":memory:")).unwrap());
        let connections = ConnectionManager::new(Arc::clone(&store));
        let snapshot = import_source(source_url, &connections).await.unwrap();
        let chart = crate::browser_bridge::validate_snapshot(&snapshot).unwrap();
        let result = store.import_browser_chart(&chart, &snapshot).unwrap();
        assert_eq!(result.provider, snapshot.provider);
        assert_eq!(result.captured_count, snapshot.entries.len());
        let loaded = store.load_browser_import(&result.id).unwrap();
        assert_eq!(loaded.tracks.len(), snapshot.entries.len());
        (snapshot, result.imported_count)
    }

    #[tokio::test]
    #[ignore = "requires public Shazam chart access"]
    async fn live_shazam_top_200_import_validates_and_persists_complete_snapshot() {
        let (snapshot, imported) =
            validate_and_persist("https://www.shazam.com/charts/top-200/world").await;
        assert_eq!(snapshot.provider, "shazam");
        assert_eq!(snapshot.entries.len(), 200);
        assert_eq!(snapshot.expected_total, Some(200));
        assert!(snapshot.complete);
        assert_eq!(imported, 200);
    }

    #[tokio::test]
    #[ignore = "requires public Last.fm chart access"]
    async fn live_lastfm_public_chart_import_or_browser_fallback_is_truthful() {
        let store = Arc::new(crate::storage::Store::open(Path::new(":memory:")).unwrap());
        let connections = ConnectionManager::new(Arc::clone(&store));
        match import_source("https://www.last.fm/music/charts", &connections).await {
            Ok(snapshot) => {
                assert_eq!(snapshot.provider, "lastfm");
                assert!(!snapshot.entries.is_empty());
                let chart = crate::browser_bridge::validate_snapshot(&snapshot).unwrap();
                let result = store.import_browser_chart(&chart, &snapshot).unwrap();
                let loaded = store.load_browser_import(&result.id).unwrap();
                assert_eq!(loaded.tracks.len(), snapshot.entries.len());
            }
            Err(error) => {
                let error = error.to_ascii_lowercase();
                assert!(error.contains("browser") || error.contains("api key"));
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires public Bandcamp album access"]
    async fn live_bandcamp_debut_album_import_validates_and_persists_tracklist() {
        let (snapshot, imported) =
            validate_and_persist("https://bjork.bandcamp.com/album/debut").await;
        assert_eq!(snapshot.provider, "bandcamp");
        assert_eq!(snapshot.kind, "album");
        assert_eq!(snapshot.title, "Debut");
        assert!(!snapshot.entries.is_empty());
        assert_eq!(imported, snapshot.entries.len());
    }

    #[tokio::test]
    #[ignore = "requires public Bandcamp album access"]
    async fn live_bandcamp_discover_collection_imports_one_explicit_album_tracklist() {
        let store = Arc::new(crate::storage::Store::open(Path::new(":memory:")).unwrap());
        let snapshot = import_bandcamp_album_collection(
            "https://bandcamp.com/discover/hip-hop-rap",
            "Bandcamp Discover test",
            vec!["https://bjork.bandcamp.com/album/debut".into()],
        )
        .await
        .unwrap();
        assert_eq!(snapshot.kind, "collection");
        assert!(!snapshot.entries.is_empty());
        assert!(snapshot.entries.iter().all(|entry| {
            entry
                .source_url
                .as_deref()
                .is_some_and(|url| url.contains("/track/"))
        }));
        let chart = crate::browser_bridge::validate_snapshot(&snapshot).unwrap();
        let imported = store.import_browser_chart(&chart, &snapshot).unwrap();
        assert_eq!(
            store
                .load_browser_import(&imported.id)
                .unwrap()
                .tracks
                .len(),
            snapshot.entries.len()
        );
    }

    #[test]
    fn shazam_csv_preserves_row_order_and_rank_without_catalog_guessing() {
        let csv = "\u{feff}\"Shazam Top 200, updated weekly\"\nRank,Artist,Title,Album\n3,Artist B,Song B,Album B\n7,Artist A,Song A,\n";
        let entries = parse_shazam_csv(csv).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.rank.unwrap()).collect::<Vec<_>>(),
            [3, 7]
        );
        assert_eq!(
            entries.iter().map(|e| e.position).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(entries
            .iter()
            .all(|e| e.isrc.is_none() && e.duration_seconds.is_none()));
    }

    #[test]
    fn lastfm_tag_toptracks_uses_live_json_tracks_root() {
        let value = serde_json::json!({"tracks":{"track":[{"name":"Song","artist":{"name":"Artist"}}],"@attr":{"tag":"afro house","total":"1"}}});
        assert_eq!(
            lastfm_track_list(&value, "tag.getTopTracks").unwrap()["track"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(lastfm_track_list(&value, "user.getTopTracks").is_none());
    }
    #[test]
    fn lastfm_user_toptracks_uses_the_user_specific_response_key() {
        let fixture: Value = serde_json::from_str(
            r#"{"toptracks":{"@attr":{"total":"1"},"track":[{"name":"Song","artist":{"name":"Artist"},"@attr":{"rank":"1"}}]}}"#,
        ).unwrap();
        assert_eq!(
            lastfm_track_list(&fixture, "user.getTopTracks").unwrap()["track"][0]["name"],
            "Song"
        );
        assert!(lastfm_track_list(&fixture, "chart.getTopTracks").is_none());
        assert!(lastfm_track_list(&fixture, "tag.getTopTracks").is_none());
    }

    #[test]
    fn lastfm_tag_route_decodes_spaces_and_percent_escapes_without_losing_identity() {
        let plus = Url::parse("https://last.fm/tag/afro+house/tracks").unwrap();
        assert_eq!(lastfm_tag_name_from_url(&plus).unwrap(), "afro house");
        assert_eq!(
            lastfm_tag_source_url(&plus),
            "https://www.last.fm/tag/afro+house/tracks"
        );

        let escaped = Url::parse("https://www.last.fm/tag/ambient%20%26%20drone/tracks").unwrap();
        assert_eq!(
            lastfm_tag_name_from_url(&escaped).unwrap(),
            "ambient & drone"
        );
        assert!(lastfm_tag_name_from_url(
            &Url::parse("https://www.last.fm/tag/a%2Fb/tracks").unwrap()
        )
        .is_err());
        assert!(lastfm_tag_name_from_url(
            &Url::parse("https://www.last.fm/tag/a/tracks/extra").unwrap()
        )
        .is_err());
    }

    #[test]
    fn lastfm_explicit_period_and_presets_survive_in_canonical_source() {
        let explicit = Url::parse("https://www.last.fm/user/rj/library?period=3month").unwrap();
        let period = lastfm_period(&explicit).unwrap();
        assert_eq!(period.as_deref(), Some("3month"));
        assert_eq!(
            lastfm_user_source_url("rj", period.as_deref()),
            "https://www.last.fm/user/rj/library/tracks?date_preset=LAST_90_DAYS"
        );

        let preset = Url::parse("https://last.fm/user/rj?date_preset=LAST_7_DAYS").unwrap();
        let period = lastfm_period(&preset).unwrap();
        assert_eq!(period.as_deref(), Some("7day"));
        assert_eq!(
            lastfm_period_label(period.as_deref().unwrap()),
            "last 7 days"
        );
        assert_eq!(
            lastfm_user_source_url("rj", period.as_deref()),
            "https://www.last.fm/user/rj/library/tracks?date_preset=LAST_7_DAYS"
        );

        let default = Url::parse("https://www.last.fm/user/rj").unwrap();
        assert_eq!(lastfm_period(&default).unwrap(), None);
        assert_eq!(
            lastfm_user_source_url("rj", None),
            "https://www.last.fm/user/rj/library/tracks"
        );
        assert_eq!(
            lastfm_user_source_url("rj", Some("overall")),
            "https://www.last.fm/user/rj/library/tracks?date_preset=ALL_TIME"
        );
    }

    #[test]
    fn lastfm_rejects_unknown_periods_and_custom_date_ranges() {
        for input in [
            "https://www.last.fm/user/rj?period=2weeks",
            "https://www.last.fm/user/rj?from=1000&to=2000",
            "https://www.last.fm/user/rj?period=7day&date_preset=LAST_90_DAYS",
        ] {
            assert!(
                lastfm_period(&Url::parse(input).unwrap()).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn lastfm_http_track_links_are_normalized_only_for_exact_lastfm_hosts() {
        assert_eq!(
            normalize_lastfm_track_url("http://www.last.fm/music/A/_/B?from=old#x").as_deref(),
            Some("https://www.last.fm/music/A/_/B")
        );
        assert!(normalize_lastfm_track_url("http://last.fm.evil.test/music/A/_/B").is_none());
    }

    #[test]
    fn lastfm_public_html_rows_are_ordered_and_explicitly_partial() {
        let html = r#"<table class="chartlist"><tr class="chartlist-row"><td class="chartlist-name"><a href="/music/Artist/_/Song">Song</a></td><td class="chartlist-artist"><a href="/music/Artist">Artist</a></td></tr><tr><td class="chartlist-name"><a href="/music/Artist/_/Song-2">Song 2</a></td><td class="chartlist-artist"><a href="/music/Artist">Artist</a></td></tr></table>"#;
        let snapshot = parse_lastfm_public_chart(html).unwrap();
        assert_eq!(snapshot.entries[0].artist, "Artist");
        assert_eq!(snapshot.entries[0].title, "Song");
        assert_eq!(snapshot.entries[1].title, "Song 2");
        assert_eq!(snapshot.entries[1].position, 2);
        assert!(!snapshot.complete);
        assert_eq!(snapshot.expected_total, None);
    }

    #[test]
    fn lastfm_client_challenge_is_not_misread_as_an_empty_complete_chart() {
        let challenge =
            "<!doctype html><title>Client Challenge</title><script>renderChart()</script>";
        assert!(parse_lastfm_public_chart(challenge).is_none());
    }

    #[test]
    fn parses_tralbumdata_without_treating_album_as_recording_metadata() {
        // Sanitized from the publicly visible Björk / Debut Bandcamp page; its
        // current page is publicly readable with an ordered track list.
        let html = r#"<script>var TralbumData = {"artist":"Björk","current":{"title":"Debut"},"trackinfo":[{"track_num":1,"title":"Human Behaviour","duration":252.0,"title_link":"/track/human-behaviour"}]};</script>"#;
        let data = extract_tralbum_data(html).unwrap();
        assert_eq!(data["artist"], "Björk");
        assert_eq!(data["trackinfo"][0]["title"], "Human Behaviour");
        assert_eq!(data["current"]["title"], "Debut");
    }

    #[test]
    fn refuses_arbitrary_hosts_and_bad_bandcamp_routes() {
        assert!(
            check_source_url("http://artist.bandcamp.com/album/x", "artist.bandcamp.com").is_err()
        );
        assert!(!valid_slug("../private"));
    }
}
