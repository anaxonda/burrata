//! Exact release evidence for repair of a malformed chart duration.
use super::{check_access_page, selector, text, Track, Traxsource, Url};
use scraper::{ElementRef, Html};
use serde_json::Value;
use std::collections::HashSet;

const MAX_RELEASE_TRACKS: usize = 80;

#[derive(Debug)]
pub(crate) struct TraxsourceReleaseEvidence {
    pub release_id: String,
    pub source_url: String,
    pub title: String,
    pub label: String,
    pub release_date: String,
    pub catalog_number: String,
    pub rows: Vec<TraxsourceReleaseTrackEvidence>,
}

#[derive(Debug)]
pub(crate) struct TraxsourceReleaseTrackEvidence {
    pub track_id: String,
    pub source_url: String,
    pub title: String,
    pub artist_credits: Vec<String>,
    pub version: Option<String>,
    pub disc_number: u32,
    pub track_number: u32,
    pub duration_seconds: Option<f64>,
    pub malformed_duration: Option<String>,
}

pub(super) struct ExactPage {
    url: Url,
    kind: &'static str,
    id: String,
}

impl ExactPage {
    fn new(input: &str, kind: &'static str) -> Result<Self, String> {
        let url = Url::parse(input).map_err(|_| "Invalid Traxsource evidence URL")?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("www.traxsource.com" | "traxsource.com")
            )
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Invalid Traxsource evidence URL".into());
        }
        let parts = url
            .path_segments()
            .ok_or("Invalid Traxsource evidence route")?
            .collect::<Vec<_>>();
        let [route, id, slug] = parts.as_slice() else {
            return Err("Invalid Traxsource evidence route".into());
        };
        if *route != kind
            || id.is_empty()
            || id.len() > 20
            || !id.bytes().all(|byte| byte.is_ascii_digit())
            || slug.is_empty()
            || slug.len() > 240
            || !slug
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("Invalid Traxsource evidence route".into());
        }
        let id = (*id).to_owned();
        Ok(Self { url, kind, id })
    }

    pub(super) fn validate(&self, input: &str) -> Result<Url, String> {
        let candidate = Self::new(input, self.kind)?;
        if candidate.url.host_str() != self.url.host_str() || candidate.id != self.id {
            return Err("Traxsource evidence redirected to another item".into());
        }
        Ok(candidate.url)
    }

    fn link(&self, raw: &str, kind: &'static str) -> Result<Self, String> {
        let linked = self
            .url
            .join(raw)
            .map_err(|_| "Invalid Traxsource evidence link")?;
        let linked = Self::new(linked.as_str(), kind)?;
        if linked.url.host_str() != self.url.host_str() {
            return Err("Traxsource evidence link changed host".into());
        }
        Ok(linked)
    }
}

impl Traxsource {
    /// Callers bound this read (including both HTTP requests) with the active
    /// playback deadline and cancellation. It updates no chart or saved track.
    pub(crate) async fn get_exact_release_for_malformed_track(
        &self,
        track: &Track,
    ) -> Result<TraxsourceReleaseEvidence, String> {
        let requested = ExactPage::new(&track.source_url, "track")?;
        if track.provider != "traxsource" || track.provider_id != requested.id {
            return Err("Traxsource evidence did not identify the requested track".into());
        }
        let (source, html) = self
            .scoped_page(requested.url.as_str(), true, Some(&requested))
            .await?;
        let detail = parse_track_page(&html, &ExactPage::new(&source, "track")?)?;
        let (source, html) = self
            .scoped_page(detail.release.url.as_str(), false, Some(&detail.release))
            .await?;
        parse_release_page(&html, &ExactPage::new(&source, "title")?, &detail)
    }
}

struct LinkedRelease {
    track_id: String,
    release: ExactPage,
    release_title: String,
    malformed_duration: String,
}

fn parse_track_page(html: &str, page: &ExactPage) -> Result<LinkedRelease, String> {
    check_access_page(html)?;
    let document = Html::parse_document(html);
    verify_canonical(&document, page)?;
    let release_links = selector(".ttl-info a[href*='/title/']");
    let mut links = document.select(&release_links);
    let release = links.next().ok_or("Traxsource track has no release link")?;
    if links.next().is_some() {
        return Err("Traxsource track has ambiguous release links".into());
    }
    let release_title = bounded_text(&text(release), 512)
        .ok_or("Traxsource track has no release title")?
        .to_owned();
    let release = page.link(
        release
            .value()
            .attr("href")
            .ok_or("Missing Traxsource release link")?,
        "title",
    )?;
    let parts = document
        .root_element()
        .text()
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let mut clocks = parts
        .windows(2)
        .filter_map(|pair| (pair[0] == "Length:").then_some(pair[1]));
    let raw = clocks
        .next()
        .ok_or("Traxsource track has no raw duration")?;
    let Some(DisplayedClock::Malformed(clock)) = displayed_clock(raw) else {
        return Err("Traxsource track has no supported malformed duration".into());
    };
    if clocks.next().is_some() {
        return Err("Traxsource track has ambiguous duration fields".into());
    }
    Ok(LinkedRelease {
        track_id: page.id.clone(),
        release,
        release_title,
        malformed_duration: clock.to_owned(),
    })
}

fn verify_canonical(document: &Html, page: &ExactPage) -> Result<(), String> {
    let canonical_links = selector("link[rel='canonical']");
    let mut links = document.select(&canonical_links);
    if let Some(link) = links.next() {
        let canonical = page.link(
            link.value().attr("href").ok_or("Missing canonical URL")?,
            page.kind,
        )?;
        if canonical.id != page.id || links.next().is_some() {
            return Err("Ambiguous canonical Traxsource item".into());
        }
    }
    Ok(())
}

fn parse_release_page(
    html: &str,
    page: &ExactPage,
    detail: &LinkedRelease,
) -> Result<TraxsourceReleaseEvidence, String> {
    check_access_page(html)?;
    detail.release.validate(page.url.as_str())?;
    let document = Html::parse_document(html);
    verify_canonical(&document, page)?;
    let info = release_info(&document, page, &detail.release_title)
        .ok_or("Traxsource release did not declare an exact complete track list")?;
    let mut rows = Vec::with_capacity(info.total);
    let mut seen = HashSet::new();
    for (index, row) in document.select(&selector(".trk-row")).enumerate() {
        if index >= info.total {
            return Err("Traxsource release contains unexpected additional rows".into());
        }
        let row = parse_release_row(row, page, index + 1)
            .ok_or("Traxsource release has an invalid or out-of-order row")?;
        if info
            .track_ids
            .as_ref()
            .is_some_and(|ids| ids[index] != row.track_id)
        {
            return Err("Traxsource release disagrees with its structured track list".into());
        }
        if !seen.insert(row.track_id.clone()) {
            return Err("Traxsource release contains duplicate tracks".into());
        }
        let valid_clock = if row.track_id == detail.track_id {
            row.malformed_duration.as_deref() == Some(detail.malformed_duration.as_str())
        } else {
            row.duration_seconds.is_some()
        };
        if !valid_clock {
            return Err("Traxsource release duration evidence is incomplete or conflicting".into());
        }
        rows.push(row);
    }
    if rows.len() != info.total || !seen.contains(&detail.track_id) {
        return Err(
            "Traxsource release track list is incomplete or missing the linked track".into(),
        );
    }
    Ok(TraxsourceReleaseEvidence {
        release_id: page.id.clone(),
        source_url: page.url.to_string(),
        title: detail.release_title.clone(),
        label: info.label,
        release_date: info.date,
        catalog_number: info.catalog_number,
        rows,
    })
}

struct ReleaseInfo {
    total: usize,
    track_ids: Option<Vec<String>>,
    label: String,
    date: String,
    catalog_number: String,
}

fn release_info(document: &Html, page: &ExactPage, title: &str) -> Option<ReleaseInfo> {
    let mut exact = None;
    for script in document.select(&selector("script[type='application/ld+json']")) {
        let raw = script.text().collect::<String>();
        if raw.len() > 256 * 1024 {
            return None;
        }
        let Ok(body) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let entries = body
            .as_array()
            .or_else(|| body["@graph"].as_array())
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(&body));
        for item in entries {
            if !matches!(item["@type"].as_str(), Some("MusicAlbum" | "MusicRelease"))
                || !item["url"]
                    .as_str()
                    .is_some_and(|url| page.validate(url).is_ok())
            {
                continue;
            }
            if exact.is_some() || item["name"].as_str() != Some(title) {
                return None;
            }
            let total = usize::try_from(item["numTracks"].as_u64()?).ok()?;
            if !(1..=MAX_RELEASE_TRACKS).contains(&total) {
                return None;
            }
            let track_ids = if let Some(tracks) = item.get("track") {
                let tracks = tracks.as_array()?;
                if tracks.len() != total {
                    return None;
                }
                Some(
                    tracks
                        .iter()
                        .map(|track| {
                            page.link(track["url"].as_str()?, "track")
                                .ok()
                                .map(|track| track.id)
                        })
                        .collect::<Option<Vec<_>>>()?,
                )
            } else {
                None
            };
            let label = bounded_text(item["recordLabel"]["name"].as_str()?, 256)?;
            let date = item["datePublished"].as_str()?;
            if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .ok()?
                .to_string()
                != date
            {
                return None;
            }
            let catalog_number = bounded_text(item["catalogNumber"].as_str()?, 96)?;
            exact = Some(ReleaseInfo {
                total,
                track_ids,
                label: label.to_owned(),
                date: date.to_owned(),
                catalog_number: catalog_number.to_owned(),
            });
        }
    }
    exact
}

fn parse_release_row(
    row: ElementRef<'_>,
    release: &ExactPage,
    position: usize,
) -> Option<TraxsourceReleaseTrackEvidence> {
    let position_elements = selector(".tnum");
    let mut positions = row.select(&position_elements);
    let track_number = text(positions.next()?).parse::<u32>().ok()?;
    if track_number as usize != position || positions.next().is_some() {
        return None;
    }
    let title_cells = selector(".title");
    let mut titles = row.select(&title_cells);
    let title_cell = titles.next()?;
    if titles.next().is_some() {
        return None;
    }
    let track_links = selector("a[href*='/track/']");
    let mut links = title_cell.select(&track_links);
    let title_link = links.next()?;
    if links.next().is_some() {
        return None;
    }
    let page = release
        .link(title_link.value().attr("href")?, "track")
        .ok()?;
    let title = bounded_text(&text(title_link), 512)?.to_owned();
    let version_elements = selector(".version, .mix, .mix-name");
    let mut versions = title_cell.select(&version_elements);
    let version = if let Some(version) = versions.next() {
        let value = text(version);
        if versions.next().is_some() {
            return None;
        }
        let value = value.trim();
        if value.is_empty() || displayed_clock(value).is_some() {
            None
        } else {
            Some(bounded_text(value, 512)?.to_owned())
        }
    } else {
        None
    };
    let mut clocks = title_cell
        .text()
        .filter_map(|raw| displayed_clock(raw).map(|clock| (raw.trim(), clock)));
    let (raw_clock, clock) = clocks.next()?;
    let (duration_seconds, malformed_duration) = match clock {
        DisplayedClock::Valid(value) => (Some(value), None),
        DisplayedClock::Malformed(value) => (None, Some(value.to_owned())),
    };
    if clocks.next().is_some() {
        return None;
    }
    // A stray unrecognized edition between the title and clock must not be
    // discarded so that a wrong mix can look like the named target.
    let expected_title_cell = [Some(title.as_str()), version.as_deref(), Some(raw_clock)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if text(title_cell) != expected_title_cell {
        return None;
    }
    let mut artist_credits = Vec::new();
    for artist in row.select(&selector(".artists a[href*='/artist/']")) {
        if artist
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| {
                parent
                    .value()
                    .classes()
                    .any(|class| class == "remixers" || class == "remixer")
            })
        {
            continue;
        }
        release.link(artist.value().attr("href")?, "artist").ok()?;
        artist_credits.push(bounded_text(&text(artist), 256)?.to_owned());
        if artist_credits.len() > 16 {
            return None;
        }
    }
    if artist_credits.is_empty() {
        return None;
    }
    // This listing must be numbered continuously. An explicitly numbered
    // multi-disc release needs a separate parser that proves disc boundaries.
    Some(TraxsourceReleaseTrackEvidence {
        track_id: page.id,
        source_url: page.url.to_string(),
        title,
        artist_credits,
        version,
        disc_number: 1,
        track_number,
        duration_seconds,
        malformed_duration,
    })
}

fn bounded_text(value: &str, maximum: usize) -> Option<&str> {
    (!value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control))
        .then_some(value)
}

enum DisplayedClock<'a> {
    Valid(f64),
    Malformed(&'a str),
}

fn displayed_clock(raw: &str) -> Option<DisplayedClock<'_>> {
    let raw = raw.trim();
    let value = raw
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(raw);
    let (minutes, seconds) = value.split_once(':')?;
    if minutes.is_empty()
        || minutes.len() > 4
        || seconds.len() != 2
        || !minutes
            .bytes()
            .chain(seconds.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds = seconds.parse::<u32>().ok()?;
    if seconds >= 60 {
        return (minutes < 60).then_some(DisplayedClock::Malformed(value));
    }
    let total = minutes * 60 + seconds;
    (1..=86_400)
        .contains(&total)
        .then_some(DisplayedClock::Valid(f64::from(total)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TRACK: &str = "https://www.traxsource.com/track/1494176/plastico-bugz-in-the-attic-remix";
    const RELEASE: &str = "https://www.traxsource.com/title/275301/fania-remixed";
    const TRACK_HTML: &str = "
        <div class='ttl-info'>Release: <a href='/title/275301/fania-remixed'>Fania Remixed</a></div>
        <table><tr><td>Length:</td><td>6:90</td></tr></table>";

    fn row(id: &str, position: u32, title: &str, mix: &str, clock: &str) -> String {
        format!(
            "<div class='trk-row'><span class='tnum'>{position}</span>
             <div class='title'><a href='/track/{id}/song'>{title}</a>
             <span class='version'>{mix}</span><span class='duration'>({clock})</span></div>
             <div class='artists'><a href='/artist/17/willie-colon'>Willie Colon</a>
             <a href='/artist/18/ruben-blades'>Ruben Blades</a>
             <div class='remixers'><a href='/artist/19/bugz'>Bugz In The Attic</a></div>
             </div></div>"
        )
    }

    fn release_info() -> Value {
        json!({
            "@type": "MusicRelease",
            "url": RELEASE,
            "name": "Fania Remixed",
            "numTracks": 2,
            "recordLabel": {"name": "Fania"},
            "datePublished": "2012-02-28",
            "catalogNumber": "G010002782582X",
            "track": [
                {"url": TRACK}, {"url": "https://www.traxsource.com/track/1494177/other"}
            ]
        })
    }

    fn release_html(info: &Value) -> String {
        let first = row(
            "1494176",
            1,
            "Plastico (Bugz In The Attic Remix)",
            "",
            "6:90",
        );
        let second = row("1494177", 2, "Saona", "Gilles Peterson Remix", "7:15");
        format!(
            "<head><link rel='canonical' href='{RELEASE}'>
             <script type='application/ld+json'>{info}</script></head><body>{first}{second}</body>"
        )
    }

    fn parse(html: &str) -> Result<TraxsourceReleaseEvidence, String> {
        let track = ExactPage::new(TRACK, "track").unwrap();
        let detail = parse_track_page(TRACK_HTML, &track).unwrap();
        parse_release_page(html, &detail.release, &detail)
    }

    #[test]
    fn exact_release_keeps_order_and_the_malformed_target_clock() {
        let release = parse(&release_html(&release_info())).unwrap();
        assert_eq!(release.release_id, "275301");
        assert_eq!(release.source_url, RELEASE);
        assert_eq!(release.title, "Fania Remixed");
        assert_eq!(release.release_date, "2012-02-28");
        assert_eq!(release.label, "Fania");
        assert_eq!(release.catalog_number, "G010002782582X");
        assert_eq!(release.rows.len(), 2);
        let [target, sibling] = release.rows.as_slice() else {
            unreachable!()
        };
        assert_eq!(target.track_id, "1494176");
        assert_eq!(target.disc_number, 1);
        assert_eq!(target.track_number, 1);
        assert_eq!(target.title, "Plastico (Bugz In The Attic Remix)");
        assert_eq!(target.artist_credits, ["Willie Colon", "Ruben Blades"]);
        assert_eq!(target.version, None);
        assert_eq!(target.duration_seconds, None);
        assert_eq!(target.malformed_duration.as_deref(), Some("6:90"));
        assert_eq!(sibling.version.as_deref(), Some("Gilles Peterson Remix"));
        assert_eq!(sibling.track_number, 2);
        assert_eq!(sibling.duration_seconds, Some(435.0));
    }

    #[test]
    fn evidence_routes_reject_changed_identity_or_ambiguous_track_page() {
        let page = ExactPage::new(TRACK, "track").unwrap();
        for input in [
            TRACK.replace("1494176", "999"),
            TRACK.replace("www.traxsource.com", "traxsource.com"),
            TRACK.replace("www.traxsource.com", "example.test"),
            format!("{TRACK}?page=2"),
        ] {
            assert!(page.validate(&input).is_err(), "{input}");
        }
        for html in [
            TRACK_HTML.replace("/title/275301/", "https://example.test/title/275301/"),
            TRACK_HTML.replace("6:90", "6:09"),
            format!(
                "<link rel='canonical' href='https://www.traxsource.com/track/5/song'>{TRACK_HTML}"
            ),
            format!("{TRACK_HTML}{TRACK_HTML}"),
        ] {
            assert!(parse_track_page(&html, &page).is_err(), "{html}");
        }
    }

    #[test]
    fn release_rows_need_a_positive_bound_total_and_unambiguous_identity() {
        let good = release_html(&release_info());
        for (field, value) in [
            ("url", json!("https://www.traxsource.com/title/999/wrong")),
            ("numTracks", json!(3)),
            ("numTracks", json!(null)),
            ("numTracks", json!(81)),
            ("name", json!("Different album")),
        ] {
            let mut info = release_info();
            info[field] = value;
            assert!(parse(&release_html(&info)).is_err(), "{field}");
        }
        let mut swapped = release_info();
        swapped["track"].as_array_mut().unwrap().swap(0, 1);
        assert!(parse(&release_html(&swapped)).is_err());
        let mut no_embedded_rows = release_info();
        no_embedded_rows.as_object_mut().unwrap().remove("track");
        let rendered = release_html(&no_embedded_rows);
        assert!(parse(&rendered).is_ok());
        for html in [
            rendered.replace("1494177", "1494176"),
            good.replace("class='tnum'>2", "class='tnum'>3"),
            good.replace("(7:15)", "(7:90)"),
            good.replace("(6:90)", "(6:80)"),
            good.replace("(6:90)", "(6:09)"),
            good.replace("/track/1494176/song", "/track/1494180/song"),
            good.replace("<span class='duration'>(6:90)</span>", "(6:90) (6:09)"),
        ] {
            assert!(parse(&html).is_err(), "{html}");
        }
    }
}
