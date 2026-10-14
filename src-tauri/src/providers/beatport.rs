use super::{ChartProvider, PreviewProvider, SearchProvider};
use crate::models::{Chart, Genre, Preview, Track};
use async_trait::async_trait;
use scraper::{Html, Selector};
use serde_json::Value;
use std::{collections::HashSet, sync::OnceLock, time::Duration};
use tokio::sync::Mutex;
use url::Url;

pub struct Beatport {
    client: reqwest::Client,
}
static REQUEST_GATE: OnceLock<Mutex<Option<tokio::time::Instant>>> = OnceLock::new();

impl Beatport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .user_agent("Burrata/0.1 (local music discovery)")
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 5 {
                        return attempt.error("Too many Beatport redirects");
                    }
                    if valid_host(attempt.url()) {
                        attempt.follow()
                    } else {
                        attempt.error("Beatport redirected outside its HTTPS catalog")
                    }
                }))
                .build()
                .expect("HTTP client configuration"),
        }
    }
    async fn fetch(&self, input: &str) -> Result<String, String> {
        validate_url(input)?;
        let mut gate = REQUEST_GATE.get_or_init(|| Mutex::new(None)).lock().await;
        if let Some(previous) = *gate {
            let remaining = (previous + Duration::from_secs(1))
                .saturating_duration_since(tokio::time::Instant::now());
            tokio::time::sleep(remaining).await;
        }
        *gate = Some(tokio::time::Instant::now());
        let response = self
            .client
            .get(input)
            .send()
            .await
            .map_err(|e| format!("Beatport request failed: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(format!(
                "Beatport denied public catalog access ({status}). Open the chart in your browser or use a saved/imported chart. No access restriction was bypassed."
            ));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|x| x.to_str().ok())
                .unwrap_or("60");
            let delay = retry
                .parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| {
                    chrono::DateTime::parse_from_rfc2822(retry)
                        .ok()
                        .and_then(|date| {
                            (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                                .to_std()
                                .ok()
                        })
                })
                .unwrap_or(Duration::from_secs(60));
            *gate = Some(tokio::time::Instant::now() + delay);
            return Err(format!(
                "Beatport rate limited this request. Retry after {retry}."
            ));
        }
        if !status.is_success() {
            return Err(format!("Beatport returned {status}"));
        }
        if response.content_length().is_some_and(|n| n > 12_000_000) {
            return Err("Beatport response exceeds catalog size limit".into());
        }
        let body = response
            .text()
            .await
            .map_err(|e| format!("Cannot read Beatport response: {e}"))?;
        if body.len() > 12_000_000 {
            return Err("Beatport response exceeds catalog size limit".into());
        }
        Ok(body)
    }
}

fn valid_host(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host_str(), Some("beatport.com" | "www.beatport.com"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
}
fn validate_url(input: &str) -> Result<Url, String> {
    let url = Url::parse(input).map_err(|_| "Invalid Beatport URL")?;
    if !valid_host(&url)
        || ![
            "/genre/",
            "/chart/",
            "/charts/",
            "/track/",
            "/release/",
            "/top-100",
            "/search/tracks",
        ]
        .iter()
        .any(|prefix| url.path().starts_with(prefix))
    {
        return Err("Use an HTTPS Beatport chart, genre, release, or track URL".into());
    }
    Ok(url)
}

#[async_trait]
impl ChartProvider for Beatport {
    fn id(&self) -> &'static str {
        "beatport"
    }
    fn list_genres(&self) -> Vec<Genre> {
        [
            ("140-deep-dubstep-grime", "140 / Deep Dubstep / Grime", 95),
            ("afro-house", "Afro House", 89),
            ("amapiano", "Amapiano", 98),
            ("ambient-experimental", "Ambient / Experimental", 100),
            ("bass-club", "Bass / Club", 85),
            ("bass-house", "Bass House", 91),
            ("brazilian-funk", "Brazilian Funk", 101),
            (
                "breaks-breakbeat-uk-bass",
                "Breaks / Breakbeat / UK Bass",
                9,
            ),
            ("dance-pop", "Dance / Pop", 39),
            ("deep-house", "Deep House", 12),
            ("dj-tools-acapellas", "DJ Tools / Acapellas", 16),
            ("downtempo", "Downtempo", 63),
            ("drum-bass", "Drum & Bass", 1),
            ("dubstep", "Dubstep", 18),
            (
                "electro-classic-detroit-modern",
                "Electro (Classic / Detroit / Modern)",
                94,
            ),
            ("electronica", "Electronica", 3),
            ("funky-house", "Funky House", 81),
            (
                "hard-dance-hardcore-neo-rave",
                "Hard Dance / Hardcore / Neo Rave",
                8,
            ),
            ("hard-techno", "Hard Techno", 2),
            ("house", "House", 5),
            ("indie-dance", "Indie Dance", 37),
            ("indie-tech", "Indie Tech", 112),
            ("jackin-house", "Jackin House", 97),
            ("latin-electronic", "Latin Electronic", 111),
            ("mainstage", "Mainstage", 96),
            ("melodic-house-techno", "Melodic House & Techno", 90),
            ("minimal-deep-tech", "Minimal / Deep Tech", 14),
            ("nu-disco-disco", "Nu Disco / Disco", 50),
            ("organic-house", "Organic House", 93),
            ("progressive-house", "Progressive House", 15),
            ("psy-trance", "Psy-Trance", 13),
            ("tech-house", "Tech House", 11),
            (
                "techno-peak-time-driving",
                "Techno (Peak Time / Driving)",
                6,
            ),
            (
                "techno-raw-deep-hypnotic",
                "Techno (Raw / Deep / Hypnotic)",
                92,
            ),
            ("trance-main-floor", "Trance (Main Floor)", 7),
            (
                "trance-raw-deep-hypnotic",
                "Trance (Raw / Deep / Hypnotic)",
                99,
            ),
            ("trap-future-bass", "Trap / Future Bass", 38),
            ("uk-garage-bassline", "UK Garage / Bassline", 86),
            ("african", "African", 102),
            ("caribbean", "Caribbean", 103),
            ("country", "Country", 104),
            ("dj-edits", "DJ Edits", 110),
            ("hip-hop", "Hip-Hop", 105),
            ("latin", "Latin", 106),
            ("pop", "Pop", 107),
            ("rb", "R&B", 108),
            ("rock", "Rock", 109),
        ]
        .into_iter()
        .map(|(slug, name, id)| Genre {
            id: id.to_string(),
            name: name.into(),
            chart_url: format!("https://www.beatport.com/genre/{slug}/{id}/top-100"),
        })
        .collect()
    }
    async fn get_chart(&self, url: &str) -> Result<Chart, String> {
        let body = self.fetch(url).await?;
        parse_chart(&body, url)
    }
}

#[async_trait]
impl SearchProvider for Beatport {
    async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 300 {
            return Err("Beatport search must contain 1–300 characters".into());
        }
        let mut url = Url::parse("https://www.beatport.com/search/tracks").unwrap();
        url.query_pairs_mut().append_pair("q", query);
        let body = self.fetch(url.as_str()).await?;
        parse_search(&body)
    }
    async fn get_track_details(&self, track: &Track) -> Result<Track, String> {
        if track.provider != self.id() {
            return Err("Track does not belong to Beatport".into());
        }
        let body = self.fetch(&track.source_url).await?;
        let mut details = parse_chart(&body, &track.source_url)?
            .tracks
            .into_iter()
            .find(|item| item.provider_id == track.provider_id)
            .ok_or("Beatport track details no longer contain the requested recording")?;
        details.rank = track.rank;
        details.favorite = track.favorite;
        details.disposition = track.disposition.clone();
        details.local_path = track.local_path.clone();
        Ok(details)
    }
}

#[async_trait]
impl PreviewProvider for Beatport {
    async fn resolve_preview(&self, track: &Track) -> Result<Preview, String> {
        if track.provider != self.id() {
            return Err("Track does not belong to Beatport".into());
        }
        validate_url(&track.source_url)?;
        let enriched = if track.preview_url.is_none() {
            self.get_track_details(track).await.ok()
        } else {
            None
        };
        let candidate = enriched.as_ref().unwrap_or(track);
        if let Some(url) = candidate
            .preview_url
            .as_ref()
            .and_then(|url| safe_media(url))
        {
            return Ok(Preview {
                kind: "audio".into(),
                url,
                can_seek: true,
                can_volume: true,
                can_autoplay: true,
                is_excerpt: true,
            });
        }
        validate_url(&track.source_url)?;
        Ok(Preview {
            kind: "external".into(),
            url: track.source_url.clone(),
            can_seek: false,
            can_volume: false,
            can_autoplay: false,
            is_excerpt: true,
        })
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|s| !s.trim().is_empty())
}
fn named(value: &Value, key: &str) -> Option<String> {
    text(value, key).or_else(|| value.get(key).and_then(|v| text(v, "name")))
}
fn number(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|n| n.is_finite() && *n > 0.)
}
fn safe_media(input: &str) -> Option<String> {
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || !(host == "beatport.com"
            || host.ends_with(".beatport.com")
            || host == "beatport.net"
            || host.ends_with(".beatport.net"))
    {
        return None;
    }
    Some(url.to_string())
}
fn track_from(value: &Value, _index: usize) -> Option<Track> {
    let raw = value.get("track").unwrap_or(value);
    let id = text(raw, "id")?;
    let title = text(raw, "name").or_else(|| text(raw, "title"))?;
    let artists = raw.get("artists")?.as_array()?;
    let artist = artists
        .iter()
        .filter_map(|a| text(a, "name"))
        .collect::<Vec<_>>()
        .join(", ");
    if artist.is_empty() {
        return None;
    }
    let empty_release = Value::Null;
    let release = raw.get("release").unwrap_or(&empty_release);
    let image = raw.get("image").or_else(|| release.get("image"));
    let artwork = image
        .and_then(|v| text(v, "uri").or_else(|| text(v, "dynamic_uri")))
        .map(|s| s.replace("{w}", "500").replace("{h}", "500"))
        .and_then(|s| safe_media(&s));
    let slug = text(raw, "slug").unwrap_or_else(|| "recording".into());
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let source_url = format!(
        "https://www.beatport.com/track/{}/{id}",
        slug.replace('/', "")
    );
    let rank = number(value, "rank")
        .or_else(|| number(value, "position"))
        .or_else(|| number(raw, "rank"))
        .filter(|v| v.fract() == 0. && *v <= u32::MAX as f64)
        .map(|v| v as u32);
    let preview = text(raw, "sample_url")
        .or_else(|| text(raw, "preview_url"))
        .or_else(|| raw.get("sample").and_then(|v| text(v, "url")))
        .and_then(|s| safe_media(&s));
    let genre = named(raw, "genre");
    Some(Track {
        id: format!("beatport:{id}"),
        provider: "beatport".into(),
        provider_id: id,
        source_url,
        artist,
        title,
        version: text(raw, "mix_name"),
        album: named(raw, "release"),
        label: named(release, "label").or_else(|| named(raw, "label")),
        genre: genre.clone(),
        normalized_genre: genre.map(|s| s.to_lowercase()),
        duration_seconds: number(raw, "length_ms")
            .map(|n| n / 1000.)
            .or_else(|| number(raw, "duration")),
        bpm: number(raw, "bpm"),
        key: named(raw, "key"),
        release_date: text(raw, "publish_date").or_else(|| text(release, "publish_date")),
        artwork_url: artwork,
        isrc: text(raw, "isrc"),
        catalog_number: text(release, "catalog_number"),
        rank,
        preview_url: preview,
        provenance: serde_json::json!({"catalog": "beatport", "raw": raw}),
        ..Default::default()
    })
}
fn insert_track(track: Track, output: &mut Vec<Track>, seen: &mut HashSet<String>) {
    if seen.insert(track.provider_id.clone()) {
        output.push(track);
    } else if track.rank.is_some() {
        if let Some(previous) = output
            .iter_mut()
            .find(|item| item.provider_id == track.provider_id && item.rank.is_none())
        {
            *previous = track;
        }
    }
}
fn collect_tracks(value: &Value, output: &mut Vec<Track>, seen: &mut HashSet<String>) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(track) = track_from(item, index) {
                    insert_track(track, output, seen);
                } else {
                    collect_tracks(item, output, seen);
                }
            }
        }
        Value::Object(map) => {
            // Response collections are visited before auxiliary objects to preserve source chart order.
            for key in ["results", "tracks", "data"] {
                if let Some(child) = map.get(key) {
                    collect_tracks(child, output, seen);
                }
            }
            if let Some(track) = track_from(value, output.len()) {
                insert_track(track, output, seen);
            } else {
                for (key, child) in map {
                    if !["results", "tracks", "data"].contains(&key.as_str()) {
                        collect_tracks(child, output, seen);
                    }
                }
            }
        }
        _ => {}
    }
}
fn parse_tracks(body: &str) -> Vec<Track> {
    let html = Html::parse_document(body);
    let scripts = Selector::parse("script#__NEXT_DATA__, script[type='application/json']").unwrap();
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();
    for script in html.select(&scripts) {
        if let Ok(value) = serde_json::from_str::<Value>(&script.inner_html()) {
            collect_tracks(&value, &mut tracks, &mut seen);
        }
    }
    tracks
}
fn parse_search(body: &str) -> Result<Vec<Track>, String> {
    let mut tracks = parse_tracks(body);
    for track in &mut tracks {
        track.rank = None;
    }
    if tracks.is_empty() {
        return Err("Beatport search page has no supported recording metadata. Public search may be unavailable or the page format changed.".into());
    }
    Ok(tracks)
}
fn parse_chart(body: &str, url: &str) -> Result<Chart, String> {
    let parsed_url = validate_url(url)?;
    let html = Html::parse_document(body);
    let mut tracks = parse_tracks(body);
    // Unranked recommendation/search arrays cannot establish chart membership or source rank.
    // A track detail page is the only context where an unranked recording is accepted.
    if !parsed_url.path().starts_with("/track/") {
        tracks.retain(|track| track.rank.is_some());
    }
    if tracks.is_empty() {
        return Err("Beatport page has no supported explicitly ranked chart metadata. The page format may have changed or access may be restricted; open the chart in your browser or import a tracklist.".into());
    }
    let title_selector = Selector::parse("title").unwrap();
    let title = html
        .select(&title_selector)
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_else(|| "Beatport chart".into());
    Ok(Chart {
        id: format!("beatport:{url}"),
        provider: "beatport".into(),
        title,
        source_url: url.into(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        tracks,
        stale: false,
        warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn genre_chart_urls_use_verified_beatport_routes() {
        let genres = Beatport::new().list_genres();
        assert_eq!(genres.len(), 47);
        let mut ids = HashSet::new();
        let mut urls = HashSet::new();
        for genre in &genres {
            assert!(
                ids.insert(genre.id.clone()),
                "duplicate genre id: {}",
                genre.id
            );
            assert!(
                urls.insert(genre.chart_url.clone()),
                "duplicate chart URL: {}",
                genre.chart_url
            );
            let url = validate_url(&genre.chart_url).unwrap();
            assert_eq!(url.host_str(), Some("www.beatport.com"));
            assert!(url.path().ends_with("/top-100"));
        }
        assert!(genres.iter().any(|g| g.name == "Amapiano"
            && g.chart_url == "https://www.beatport.com/genre/amapiano/98/top-100"));
        assert!(genres.iter().any(|g| g.name == "Tech House"
            && g.chart_url == "https://www.beatport.com/genre/tech-house/11/top-100"));
        assert!(genres.iter().any(|g| g.name == "Rock"
            && g.chart_url == "https://www.beatport.com/genre/rock/109/top-100"));
    }
    #[test]
    fn preserves_chart_rank_version_and_unknown_fields() {
        let chart = parse_chart(
            include_str!("../../tests/fixtures/beatport-chart.html"),
            "https://www.beatport.com/genre/house/5/top-100",
        )
        .unwrap();
        assert_eq!(chart.tracks.len(), 2);
        assert_eq!(chart.tracks[0].rank, Some(3));
        assert_eq!(chart.tracks[0].version.as_deref(), Some("Extended Mix"));
        assert_eq!(chart.tracks[0].duration_seconds, Some(360.));
        assert_eq!(chart.tracks[0].label.as_deref(), Some("Fixture Records"));
        assert_eq!(chart.tracks[1].rank, Some(7));
        assert!(chart.tracks[1].bpm.is_none());
        assert!(chart.tracks[1].preview_url.is_none());
    }
    #[test]
    fn search_results_are_unranked_and_chart_excludes_related_tracks() {
        let body = include_str!("../../tests/fixtures/beatport-search.html");
        let results = parse_search(body).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].rank.is_none());
        assert_eq!(results[0].version.as_deref(), Some("Radio Edit"));
        assert!(parse_chart(body, "https://www.beatport.com/genre/house/5/top-100").is_err());
    }
    #[test]
    fn ranked_metadata_wins_over_earlier_unranked_duplicate() {
        let body = r#"<script id="__NEXT_DATA__" type="application/json">{"queries":[{"data":{"results":[{"id":42,"name":"Related track","artists":[{"name":"Artist"}]}]}},{"data":{"results":[{"position":2,"track":{"id":42,"name":"Chart track","artists":[{"name":"Artist"}]}}]}}]}</script>"#;
        let chart = parse_chart(body, "https://www.beatport.com/genre/house/5/top-100").unwrap();
        assert_eq!(chart.tracks.len(), 1);
        assert_eq!(chart.tracks[0].rank, Some(2));
        assert_eq!(chart.tracks[0].title, "Chart track");
    }
    #[test]
    fn fractional_ranks_cannot_fabricate_chart_positions() {
        let body = r#"<script id="__NEXT_DATA__" type="application/json">{"results":[{"position":2.5,"track":{"id":42,"name":"Track","artists":[{"name":"Artist"}]}}]}</script>"#;
        assert!(parse_chart(body, "https://www.beatport.com/genre/house/5/top-100").is_err());
    }
    #[test]
    fn rejects_unknown_page_and_untrusted_urls() {
        assert!(parse_chart(
            "<html>Access denied</html>",
            "https://www.beatport.com/top-100"
        )
        .is_err());
        assert!(validate_url("https://www.beatport.com.evil.test/track/a/1").is_err());
        assert!(validate_url("http://www.beatport.com/track/a/1").is_err());
        assert!(safe_media("https://127.0.0.1/music.mp3").is_none());
    }
}
