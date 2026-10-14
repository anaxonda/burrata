//! Public Traxsource page adapter. Preview URLs must come from page data;
//! an external page is the fallback when no playable excerpt is exposed.
use super::{ChartProvider, PreviewProvider, SearchProvider};
use crate::models::{Chart, Genre, Preview, Track};
use async_trait::async_trait;
use reqwest::{redirect::Policy, Client};
use scraper::{ElementRef, Html, Selector};
use std::{
    collections::HashSet,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

static REQUEST_GATE: OnceLock<Mutex<Instant>> = OnceLock::new();
const BASE: &str = "https://www.traxsource.com";
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

pub struct Traxsource {
    client: Client,
}

impl Traxsource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("Burrata/0.1 (local music discovery)")
                .timeout(Duration::from_secs(25))
                .redirect(Policy::none())
                .build()
                .expect("HTTP client configuration is valid"),
        }
    }

    async fn page(&self, input: &str, detail: bool) -> Result<(String, String), String> {
        let mut url = validate_page_url(input, detail)?;
        // Shared by every adapter instance, including instances created by commands.
        let gate = REQUEST_GATE.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(1)));
        let mut previous = gate.lock().await;
        for redirect in 0..=3 {
            let remaining = Duration::from_secs(1).saturating_sub(previous.elapsed());
            tokio::time::sleep(remaining).await;
            *previous = Instant::now();
            let mut response = self
                .client
                .get(url.clone())
                .send()
                .await
                .map_err(|e| format!("Traxsource request failed: {e}"))?;
            if response.status().is_redirection() {
                if redirect == 3 {
                    return Err("Traxsource redirected too many times".into());
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or("Traxsource redirect has no valid Location")?;
                let target = url
                    .join(location)
                    .map_err(|_| "Invalid Traxsource redirect")?;
                url = validate_page_url(target.as_str(), detail)?;
                continue;
            }
            if !response.status().is_success() {
                if response.status() == reqwest::StatusCode::FORBIDDEN {
                    return Err("Traxsource access is restricted (HTTP 403). Open the source page in your browser or use an imported tracklist; cached charts remain available.".into());
                }
                let retry = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| format!("; retry after {v}"))
                    .unwrap_or_default();
                return Err(format!(
                    "Traxsource returned HTTP {}{retry}",
                    response.status()
                ));
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_PAGE_BYTES as u64)
            {
                return Err("Traxsource page exceeds the size limit".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("Traxsource response failed: {e}"))?
            {
                if bytes.len() + chunk.len() > MAX_PAGE_BYTES {
                    return Err("Traxsource page exceeds the size limit".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok((
                url.to_string(),
                String::from_utf8(bytes).map_err(|_| "Traxsource returned invalid UTF-8")?,
            ));
        }
        Err("Traxsource redirect failed".into())
    }
}

#[async_trait]
impl ChartProvider for Traxsource {
    fn id(&self) -> &'static str {
        "traxsource"
    }

    fn list_genres(&self) -> Vec<Genre> {
        [
            (4, "House", "house"),
            (13, "Deep House", "deep-house"),
            (24, "Soulful House", "soulful-house"),
            (29, "Garage", "garage"),
            (27, "Afro House", "afro-house"),
            (17, "Nu Disco / Indie Dance", "nu-disco-indie-dance"),
            (15, "Jackin House", "jackin-house"),
            (18, "Tech House", "tech-house"),
            (20, "Techno", "techno"),
            (12, "Classic House", "classic-house"),
            (3, "Soul / Funk / Disco", "soul-funk-disco"),
            (23, "Afro / Latin / Brazilian", "afro-latin-brazilian"),
            (
                19,
                "Melodic / Progressive House",
                "melodic-progressive-house",
            ),
            (16, "Minimal / Deep Tech", "minimal-deep-tech"),
            (1, "Lounge / Chill Out", "lounge-chill-out"),
            (5, "Electronica", "electronica"),
            (2, "Broken Beat / Nu-Jazz", "broken-beat-nu-jazz"),
            (30, "World", "world"),
            (31, "Drum & Bass", "drum-and-bass"),
            (32, "Pop / Dance", "pop-dance"),
            (11, "Electro House", "electro-house"),
            (14, "Leftfield", "leftfield"),
            (6, "R&B / Hip Hop", "r-and-b-hip-hop"),
            (21, "Sounds, Samples & Loops", "sounds-samples-and-loops"),
            (7, "Acapella", "acapella"),
            (9, "Beats", "beats"),
            (10, "Efx / DJ Tools", "efx"),
            (28, "STEMS", "stems"),
        ]
        .into_iter()
        .map(|(id, name, slug)| Genre {
            id: id.to_string(),
            name: name.into(),
            chart_url: format!("{BASE}/genre/{id}/{slug}/top"),
        })
        .collect()
    }

    async fn get_chart(&self, url: &str) -> Result<Chart, String> {
        let (source, html) = self.page(url, false).await?;
        parse_chart(&html, &source)
    }
}

#[async_trait]
impl SearchProvider for Traxsource {
    async fn get_track_details(&self, track: &Track) -> Result<Track, String> {
        let (source, html) = self.page(&track.source_url, true).await?;
        let mut updated = parse_details(&html, track)?;
        updated.source_url = source;
        Ok(updated)
    }

    async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 256 {
            return Err("Traxsource search requires 1–256 characters".into());
        }
        let mut url = Url::parse(&format!("{BASE}/search/tracks")).expect("constant URL");
        url.query_pairs_mut().append_pair("term", query);
        let (source, html) = self.page(url.as_str(), false).await?;
        parse_search(&html, &source)
    }
}

#[async_trait]
impl PreviewProvider for Traxsource {
    async fn resolve_preview(&self, track: &Track) -> Result<Preview, String> {
        validate_page_url(&track.source_url, true)?;
        let preview = if let Some(url) = track
            .preview_url
            .as_ref()
            .and_then(|u| media_url(u, &track.source_url))
        {
            Some(url)
        } else {
            self.get_track_details(track).await?.preview_url
        };
        Ok(match preview {
            Some(url) => Preview {
                kind: "audio".into(),
                url,
                can_seek: true,
                can_volume: true,
                can_autoplay: true,
                is_excerpt: true,
            },
            None => Preview {
                kind: "external".into(),
                url: track.source_url.clone(),
                can_seek: false,
                can_volume: false,
                can_autoplay: false,
                is_excerpt: true,
            },
        })
    }
}

fn validate_page_url(input: &str, detail: bool) -> Result<Url, String> {
    let url = Url::parse(input).map_err(|_| "Invalid Traxsource URL")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.traxsource.com" | "traxsource.com")
        )
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Traxsource URLs must use HTTPS on traxsource.com".into());
    }
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let allowed = if detail {
        parts.len() == 3 && parts[0] == "track" && numeric(parts[1]) && !parts[2].is_empty()
    } else {
        (parts.len() == 4 && parts[0] == "genre" && numeric(parts[1]) && parts[3] == "top")
            || (parts.len() == 3 && parts[0] == "title" && numeric(parts[1]))
            || parts == ["top", "tracks"]
            || parts == ["search", "tracks"]
    };
    if !allowed {
        return Err("Unsupported Traxsource chart or track URL path".into());
    }
    Ok(url)
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("constant selector")
}
fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}
fn first_text(element: ElementRef<'_>, css: &str) -> Option<String> {
    element
        .select(&selector(css))
        .map(text)
        .find(|s| !s.is_empty())
}
fn duration(raw: &str) -> Option<f64> {
    let clean = raw.trim().trim_matches(|c| c == '(' || c == ')');
    let parts: Vec<u32> = clean
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if !(2..=3).contains(&parts.len()) || parts[1..].iter().any(|n| *n >= 60) {
        return None;
    }
    Some(parts.into_iter().fold(0, |sum, n| sum * 60 + n) as f64)
}
fn date(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .find(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok())
        .map(str::to_string)
}
fn absolute_link(raw: &str, base: &str) -> Option<String> {
    Url::parse(base)
        .ok()?
        .join(raw)
        .ok()
        .filter(|u| u.scheme() == "https")
        .map(|u| u.to_string())
}
fn media_url(raw: &str, base: &str) -> Option<String> {
    let url = Url::parse(&absolute_link(raw, base)?).ok()?;
    let host = url.host_str()?;
    if !(host == "traxsource.com" || host.ends_with(".traxsource.com"))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    [".mp3", ".m4a", ".aac", ".ogg", ".opus", ".wav"]
        .iter()
        .any(|ext| path.ends_with(ext))
        .then(|| url.to_string())
}

fn preview_from_element(root: ElementRef<'_>, base: &str) -> Option<String> {
    for attr in [
        "data-preview",
        "data-preview-url",
        "data-sample",
        "data-sample-url",
    ] {
        if let Some(url) = root.value().attr(attr).and_then(|v| media_url(v, base)) {
            return Some(url);
        }
    }
    for element in root.select(&selector("audio[src], audio source[src], [data-preview], [data-preview-url], [data-sample], [data-sample-url]")) {
        for attr in ["src", "data-preview", "data-preview-url", "data-sample", "data-sample-url"] {
            if let Some(url) = element.value().attr(attr).and_then(|v| media_url(v, base)) { return Some(url); }
        }
    }
    None
}

fn check_access_page(html: &str) -> Result<(), String> {
    let doc = Html::parse_document(html);
    let title = first_text(doc.root_element(), "title").unwrap_or_default();
    if title.eq_ignore_ascii_case("Just a moment...")
        || html.contains("window._cf_chl_opt")
        || html.contains("/cdn-cgi/challenge-platform/")
    {
        return Err("Traxsource requires browser verification. Open the source page in your browser or use an imported tracklist; the app cannot retrieve this chart automatically.".into());
    }
    Ok(())
}

fn parse_row(row: ElementRef<'_>, source: &str) -> Option<Track> {
    let title_cell = row.select(&selector(".title")).next()?;
    let link = title_cell.select(&selector("a[href*='/track/']")).next()?;
    let source_url = absolute_link(link.value().attr("href")?, source)?;
    let parsed = validate_page_url(&source_url, true).ok()?;
    let provider_id = parsed.path().split('/').nth(2)?.to_string();
    let title = text(link);
    if title.is_empty() {
        return None;
    }
    let bits: Vec<String> = title_cell
        .text()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let version = first_text(title_cell, ".version, .mix, .mix-name").or_else(|| {
        bits.iter()
            .find(|s| **s != title && duration(s).is_none())
            .cloned()
    });
    let duration_seconds = first_text(title_cell, ".duration, .length")
        .and_then(|s| duration(&s))
        .or_else(|| bits.iter().find_map(|s| duration(s)));
    let artists: Vec<String> = row
        .select(&selector(".artists a"))
        .filter(|e| {
            !e.ancestors().filter_map(ElementRef::wrap).any(|p| {
                p.value()
                    .classes()
                    .any(|c| c == "remixers" || c == "remixer")
            })
        })
        .map(text)
        .filter(|s| !s.is_empty())
        .collect();
    let artist = artists.join(", ");
    if artist.is_empty() {
        return None;
    }
    let rank = first_text(row, ".tnum, .position, .rank")
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|n| n.trim_matches('#').parse::<u32>().ok())
        })
        .filter(|n| *n > 0);
    let key_bpm: Vec<String> = row
        .select(&selector(".key-bpm"))
        .flat_map(|e| e.text())
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect();
    let bpm = first_text(row, ".bpm")
        .and_then(|v| v.parse().ok())
        .or_else(|| key_bpm.iter().find_map(|s| s.parse::<f64>().ok()))
        .filter(|n| *n > 0.0 && n.is_finite());
    let key = first_text(row, ".key").or_else(|| {
        key_bpm
            .iter()
            .find(|s| s.parse::<f64>().is_err() && s.as_str() != "BPM")
            .cloned()
    });
    let genre = first_text(row, ".genre");
    let artwork_url = row
        .select(&selector(".thumb img, img"))
        .next()
        .and_then(|e| e.value().attr("data-src").or_else(|| e.value().attr("src")))
        .and_then(|u| absolute_link(u, source));
    let remixers: Vec<_> = row
        .select(&selector(".remixers a, .remixer a"))
        .map(text)
        .collect();
    Some(Track {
        id: format!("traxsource:{provider_id}"),
        provider: "traxsource".into(),
        provider_id,
        source_url,
        artist,
        title,
        version,
        label: first_text(row, ".label"),
        genre,
        duration_seconds,
        bpm,
        key,
        rank,
        artwork_url,
        release_date: first_text(row, ".r-date, .release-date").and_then(|v| date(&v)),
        preview_url: preview_from_element(row, source),
        provenance: serde_json::json!({"metadataSource": source, "remixers": remixers}),
        ..Default::default()
    })
}

pub(crate) fn parse_chart(html: &str, source: &str) -> Result<Chart, String> {
    validate_page_url(source, false)?;
    check_access_page(html)?;
    let document = Html::parse_document(html);
    let rows: Vec<_> = document.select(&selector(".trk-row")).collect();
    if rows.is_empty() {
        return Err("Traxsource chart contains no recognizable track rows; the page may have changed or access may be restricted".into());
    }
    let mut tracks = Vec::new();
    let mut ids = HashSet::new();
    let mut skipped = 0;
    for row in rows {
        match parse_row(row, source) {
            Some(track) if track.rank.is_some() => {
                if ids.insert(track.provider_id.clone()) {
                    tracks.push(track);
                }
            }
            _ => skipped += 1,
        }
    }
    if tracks.is_empty() {
        return Err(
            "Traxsource chart has no valid ranked tracks; source ranks cannot be recovered".into(),
        );
    }
    tracks.sort_by_key(|t| t.rank);
    let title =
        first_text(document.root_element(), "h1").unwrap_or_else(|| "Traxsource chart".into());
    Ok(Chart {
        id: format!("traxsource:{source}"),
        provider: "traxsource".into(),
        title,
        source_url: source.into(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        tracks,
        stale: false,
        warning: (skipped > 0)
            .then(|| format!("Skipped {skipped} incomplete or unranked Traxsource rows")),
    })
}

fn parse_search(html: &str, source: &str) -> Result<Vec<Track>, String> {
    validate_page_url(source, false)?;
    check_access_page(html)?;
    let doc = Html::parse_document(html);
    let results = doc.select(&selector("#searchTrackList")).next().ok_or(
        "Traxsource search returned no recognizable results container; the page may have changed or access may be restricted",
    )?;
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();
    let mut rows = 0;
    for row in results.select(&selector(".trk-row")) {
        rows += 1;
        if let Some(mut track) = parse_row(row, source) {
            track.rank = None;
            if seen.insert(track.provider_id.clone()) {
                tracks.push(track);
            }
        }
    }
    if rows > 0 && tracks.is_empty() {
        return Err("Traxsource search rows could not be parsed".into());
    }
    Ok(tracks)
}

fn parse_details(html: &str, original: &Track) -> Result<Track, String> {
    validate_page_url(&original.source_url, true)?;
    check_access_page(html)?;
    let doc = Html::parse_document(html);
    let root = doc.root_element();
    let mut result = original.clone();
    // Detail pages render a labelled metadata table. Read label/value pairs,
    // rather than inferring values from arbitrary numbers in the document.
    let tokens: Vec<String> = root
        .text()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if root
        .select(&selector(".ttl-info a[href*='/title/']"))
        .next()
        .is_none()
        && !tokens
            .iter()
            .any(|s| matches!(s.as_str(), "BPM:" | "Key:" | "Length:"))
    {
        return Err("Traxsource returned no recognizable track details".into());
    }
    for pair in tokens.windows(2) {
        // Ignore the repeated horizontal table headings (Label / Released /
        // Length / Genre / Key / BPM), which are not label/value pairs.
        if !pair[0].ends_with(':') {
            continue;
        }
        let name = pair[0].trim_end_matches(':').trim().to_ascii_lowercase();
        let value = pair[1].trim();
        match name.as_str() {
            "bpm" => {
                if let Ok(n) = value.parse::<f64>() {
                    if n > 0.0 && n.is_finite() {
                        result.bpm = Some(n);
                    }
                }
            }
            "key" => {
                if !value.is_empty() && value.len() <= 16 && value != "BPM" {
                    result.key = Some(value.into());
                }
            }
            "length" => {
                if let Some(n) = duration(value) {
                    result.duration_seconds = Some(n);
                }
            }
            "label" => result.label = Some(value.into()),
            "genre" => result.genre = Some(value.into()),
            "released" => {
                if let Some(d) = date(value) {
                    result.release_date = Some(d);
                }
            }
            _ => {}
        }
    }
    result.album = first_text(root, ".ttl-info a[href*='/title/']").or(result.album);
    result.artwork_url = root
        .select(&selector(".t-image img"))
        .next()
        .and_then(|e| e.value().attr("src"))
        .and_then(|u| absolute_link(u, &original.source_url))
        .or(result.artwork_url);
    result.preview_url = preview_from_element(root, &original.source_url).or(result.preview_url);
    // Do not alter the chart's rank, identity, version, or user state.
    result.provenance["detailSource"] = serde_json::json!(original.source_url);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "https://www.traxsource.com/genre/13/deep-house/top";
    #[test]
    fn lists_the_complete_live_traxsource_genre_menu_with_chart_routes() {
        let genres = Traxsource::new().list_genres();
        assert_eq!(genres.len(), 28);
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
            let url = validate_page_url(&genre.chart_url, false).unwrap();
            assert_eq!(url.host_str(), Some("www.traxsource.com"));
            assert_eq!(
                url.path().trim_end_matches('/').rsplit('/').next(),
                Some("top")
            );
        }
        assert!(genres.iter().any(|g| g.name == "Pop / Dance"
            && g.chart_url == "https://www.traxsource.com/genre/32/pop-dance/top"));
        assert!(genres.iter().any(|g| g.name == "R&B / Hip Hop"
            && g.chart_url == "https://www.traxsource.com/genre/6/r-and-b-hip-hop/top"));
        assert!(genres.iter().any(|g| g.name == "Efx / DJ Tools"
            && g.chart_url == "https://www.traxsource.com/genre/10/efx/top"));
    }
    #[test]
    fn preserves_ranks_and_optional_metadata() {
        let chart = parse_chart(
            include_str!("../../tests/fixtures/traxsource_chart.html"),
            SOURCE,
        )
        .unwrap();
        assert_eq!(chart.tracks.len(), 2);
        let a = &chart.tracks[0];
        assert_eq!(a.rank, Some(2));
        assert_eq!(a.title, "All I Do Is Think");
        assert_eq!(a.version.as_deref(), Some("N.W.N. Remix"));
        assert_eq!(a.artist, "Oscar P");
        assert_eq!(a.duration_seconds, Some(383.0));
        assert_eq!(a.label.as_deref(), Some("Kolour Recordings"));
        assert!(a.bpm.is_none());
        assert_eq!(chart.tracks[1].rank, Some(8));
        assert!(chart.tracks[1].version.is_none());
        assert!(chart.tracks[1].label.is_none());
        assert!(chart.warning.is_some());
    }
    #[test]
    fn details_are_lazy_and_keep_chart_and_user_state() {
        let mut track = parse_chart(
            include_str!("../../tests/fixtures/traxsource_chart.html"),
            SOURCE,
        )
        .unwrap()
        .tracks
        .remove(0);
        track.favorite = true;
        let detail = parse_details(
            include_str!("../../tests/fixtures/traxsource_detail.html"),
            &track,
        )
        .unwrap();
        assert_eq!(detail.bpm, Some(123.0));
        assert_eq!(detail.key.as_deref(), Some("Emin"));
        assert_eq!(detail.label.as_deref(), Some("Kolour Recordings"));
        assert_eq!(detail.genre.as_deref(), Some("Deep House"));
        assert_eq!(detail.rank, Some(2));
        assert!(detail.favorite);
        assert!(detail
            .preview_url
            .as_ref()
            .unwrap()
            .contains("real-page-sample.mp3"));
    }
    #[test]
    fn missing_preview_is_never_fabricated() {
        let track = parse_chart(
            include_str!("../../tests/fixtures/traxsource_chart.html"),
            SOURCE,
        )
        .unwrap()
        .tracks
        .remove(0);
        assert!(track.preview_url.is_none());
        assert!(parse_details(
            "<h1>Track</h1><div class='ttl-info'><a href='/title/99/example'>Release</a></div>",
            &track
        )
        .unwrap()
        .preview_url
        .is_none());
        assert!(media_url("https://evil.example/sample.mp3", SOURCE).is_none());
        assert!(media_url("https://traxsource.com.evil.example/sample.mp3", SOURCE).is_none());
    }
    #[test]
    fn rejects_bad_urls_and_changed_pages() {
        for url in [
            "http://www.traxsource.com/genre/13/deep-house/top",
            "https://evil.example/genre/13/deep-house/top",
            "https://www.traxsource.com@evil.example/genre/13/deep-house/top",
            "https://www.traxsource.com/account",
        ] {
            assert!(validate_page_url(url, false).is_err());
        }
        assert!(parse_chart("<html>Access denied</html>", SOURCE).is_err());
        assert_eq!(duration("(1:02:03)"), Some(3723.0));
        assert!(duration("6:99").is_none());
    }

    #[test]
    fn search_results_are_unranked_and_scoped_to_search_container() {
        let results = parse_search(
            include_str!("../../tests/fixtures/traxsource_search.html"),
            "https://www.traxsource.com/search/tracks?term=artist",
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Track");
        assert_eq!(results[0].bpm, Some(123.0));
        assert_eq!(results[0].rank, None);
        assert!(parse_search("<h1>Access denied</h1>", SOURCE).is_err());
        assert!(parse_search("<div id='searchTrackList'></div>", SOURCE)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn real_sanitized_browser_challenge_is_reported() {
        let challenge = include_str!("../../../tests/fixtures/traxsource_access_denied.html");
        let chart_error = parse_chart(challenge, SOURCE).unwrap_err();
        assert!(chart_error.contains("browser verification"));
        assert!(parse_search(challenge, SOURCE)
            .unwrap_err()
            .contains("browser verification"));
        let track = Track {
            source_url: "https://www.traxsource.com/track/123/example".into(),
            ..Default::default()
        };
        assert!(parse_details(challenge, &track)
            .unwrap_err()
            .contains("browser verification"));
    }
}
