//! Public recording-duration evidence for a malformed Traxsource chart clock.
use super::{artist_keys, is_eligible, recording_key, track_artist_credits};
use crate::{
    models::Track,
    recording_identity::{
        artist_search_fold, canonical_version_key, normalize_isrc, RecordingIdentity,
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

const ORIGIN: &str = "https://api.deezer.com";
const SEARCH_LIMIT: usize = 12;
const DETAIL_LIMIT: usize = 4;
const BODY_LIMIT: usize = 256 * 1024;
const DEEZER_DURATION_PROVIDER: &str = "traxsourceDeezerRecordingFamily";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Request {
    Search(String),
    Detail(u64),
    Isrc(String),
}

#[derive(Deserialize)]
struct SearchPage {
    total: usize,
    data: Vec<TrackSummary>,
    next: Option<Value>,
}

#[derive(Deserialize)]
struct TrackSummary {
    id: u64,
    title: String,
    title_short: String,
    title_version: String,
    duration: u32,
    artist: Artist,
}

#[derive(Deserialize)]
struct Artist {
    id: u64,
    name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
enum Role {
    Main,
    Featured,
    Remixer,
}

#[derive(Deserialize)]
struct Contributor {
    id: u64,
    name: String,
    role: Role,
}

#[derive(Deserialize)]
struct Album {
    id: u64,
}

#[derive(Deserialize)]
struct TrackDetail {
    #[serde(flatten)]
    summary: TrackSummary,
    isrc: String,
    contributors: Vec<Contributor>,
    album: Album,
}

struct RequestedRecording {
    title: String,
    version: String,
    remixer: String,
    primary_keys: Vec<String>,
    first_artist: String,
    query: String,
}

impl RequestedRecording {
    fn from_track(track: &Track) -> Option<Self> {
        if !is_eligible(track) {
            return None;
        }
        let (title, version) = recording_key(&track.title, None)?;
        let remixer = version.strip_suffix(" remix")?;
        let remixer = artist_search_fold(remixer);
        let artists = track_artist_credits(track)?;
        artist_keys(&artists)?;
        let first_artist = artists.first()?.trim();
        let query = format!("{} {first_artist}", track.title);
        if remixer.is_empty()
            || first_artist.is_empty()
            || query.len() > 400
            || query.chars().any(char::is_control)
        {
            return None;
        }
        let primary_keys = artists
            .iter()
            .map(|name| artist_search_fold(name))
            .collect();
        Some(Self {
            title,
            version,
            remixer,
            primary_keys,
            first_artist: artist_search_fold(first_artist),
            query,
        })
    }

    fn same_listed_recording(&self, track: &TrackSummary) -> bool {
        artist_search_fold(&track.artist.name) == self.first_artist
            && recording_key(&track.title, None)
                .is_some_and(|(title, version)| title == self.title && version == self.version)
    }

    fn matches(&self, track: &TrackSummary) -> bool {
        self.same_listed_recording(track)
            && track.id != 0
            && track.artist.id != 0
            && (31..=86_400).contains(&track.duration)
            && recording_key(&track.title_short, None)
                .is_some_and(|(title, version)| title == self.title && version.is_empty())
            && crate::matching::split_balanced_trailing_parenthetical(&format!(
                "{} {}",
                track.title_short, track.title_version
            ))
            .is_some_and(|(_, version)| canonical_version_key(version) == self.version)
    }
}

#[derive(PartialEq, Eq)]
struct Credit {
    key: String,
    name: String,
    roles: BTreeSet<Role>,
}

struct ConfirmedTrack {
    id: u64,
    album_id: u64,
    title: String,
    duration: u32,
    isrc: String,
    credits: HashMap<u64, Credit>,
}

impl ConfirmedTrack {
    fn from_detail(request: &RequestedRecording, detail: TrackDetail) -> Option<Self> {
        let track = detail.summary;
        if !request.matches(&track)
            || detail.album.id == 0
            || !(1..=24).contains(&detail.contributors.len())
        {
            return None;
        }
        let mut credits = HashMap::<u64, Credit>::new();
        for row in detail.contributors {
            let key = artist_search_fold(&row.name);
            if row.id == 0
                || key.is_empty()
                || row.name.len() > 256
                || row.name.chars().any(char::is_control)
            {
                return None;
            }
            let credit = credits.entry(row.id).or_insert_with(|| Credit {
                key: key.clone(),
                name: row.name.clone(),
                roles: BTreeSet::new(),
            });
            // A provider may repeat the same person under distinct roles. A
            // second name for that ID or a repeated identical role is ambiguous.
            if credit.key != key
                || crate::matching::normalize_single_artist_name(&credit.name)
                    != crate::matching::normalize_single_artist_name(&row.name)
                || !credit.roles.insert(row.role)
            {
                return None;
            }
        }
        let mut names = HashSet::new();
        for credit in credits.values() {
            if !names.insert(credit.key.as_str()) {
                return None;
            }
            if request.primary_keys.contains(&credit.key) {
                if !credit.roles.contains(&Role::Main)
                    || credit.key != request.remixer && credit.roles.contains(&Role::Remixer)
                {
                    return None;
                }
            } else if credit.key != request.remixer {
                return None;
            }
        }
        if request
            .primary_keys
            .iter()
            .any(|artist| !names.contains(artist.as_str()))
            || credits
                .get(&track.artist.id)
                .is_none_or(|artist| artist.key != request.first_artist)
        {
            return None;
        }
        Some(Self {
            id: track.id,
            album_id: detail.album.id,
            title: track.title,
            duration: track.duration,
            isrc: normalize_isrc(&detail.isrc)?,
            credits,
        })
    }
}

async fn fetch_json(client: &reqwest::Client, request: Request) -> Result<Value, String> {
    let url = match request {
        Request::Search(query) => reqwest::Url::parse_with_params(
            &format!("{ORIGIN}/search"),
            [("q", query), ("limit", SEARCH_LIMIT.to_string())],
        )
        .map_err(|_| "Invalid Deezer search URL")?,
        Request::Detail(id) => reqwest::Url::parse(&format!("{ORIGIN}/track/{id}"))
            .map_err(|_| "Invalid Deezer track URL")?,
        Request::Isrc(code) => reqwest::Url::parse(&format!("{ORIGIN}/track/isrc:{code}"))
            .map_err(|_| "Invalid Deezer recording URL")?,
    };
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Public Deezer catalog request is unavailable")?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|size| size > BODY_LIMIT as u64)
    {
        return Err("Public Deezer catalog response is unavailable".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Public Deezer catalog response is unavailable")?
    {
        if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err("Public Deezer catalog response exceeds its size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid public Deezer catalog response".into())
}

fn materialize(
    original: &Track,
    request: &RequestedRecording,
    observed: &[ConfirmedTrack],
    canonical: ConfirmedTrack,
) -> Option<Track> {
    let known = RecordingIdentity::from_track(original);
    if known.identifier_conflict
        || known
            .isrc
            .as_ref()
            .is_some_and(|isrc| isrc != &canonical.isrc)
        || original
            .isrc
            .as_deref()
            .is_some_and(|isrc| normalize_isrc(isrc).as_ref() != Some(&canonical.isrc))
    {
        return None;
    }
    let confirmations = observed.iter().chain(std::iter::once(&canonical));
    let mut consistent_ids = HashMap::new();
    let mut albums = HashSet::new();
    let (mut shortest, mut longest) = (canonical.duration, canonical.duration);
    for item in confirmations {
        if item.isrc != canonical.isrc
            || item.credits != canonical.credits
            || consistent_ids
                .insert(item.id, (item.album_id, item.duration))
                .is_some_and(|evidence| evidence != (item.album_id, item.duration))
        {
            return None;
        }
        shortest = shortest.min(item.duration);
        longest = longest.max(item.duration);
        albums.insert(item.album_id);
    }
    if consistent_ids.len() < 2 || albums.len() < 2 || longest - shortest > 1 {
        return None;
    }
    let artist_credits = request
        .primary_keys
        .iter()
        .map(|key| {
            canonical
                .credits
                .values()
                .find(|credit| &credit.key == key)
                .map(|credit| credit.name.clone())
        })
        .collect::<Option<Vec<_>>>()?;
    let mut verified = original.clone();
    verified.title = canonical.title;
    verified.artist = artist_credits.join(", ");
    verified.version = None;
    verified.duration_seconds = Some(f64::from(canonical.duration));
    verified.isrc = Some(canonical.isrc.clone());
    verified.provenance["artistCredits"] = json!(artist_credits
        .iter()
        .map(|name| json!({ "name":name, "role":"artist" }))
        .collect::<Vec<_>>());
    verified.provenance["playbackCatalogDurationProof"] = json!({
        "provider": DEEZER_DURATION_PROVIDER,
        "savedFingerprint": crate::remote_playlists::alias_hydration_key(original).ok()?,
        "candidateTrackId": canonical.id,
        "candidateRecordingIsrc": canonical.isrc,
        "corroboratingTrackIds": BTreeSet::from_iter(consistent_ids.keys().copied()),
    });
    Some(verified)
}

async fn prepare_with_fetch<F, Fut>(original: &Track, mut fetch: F) -> Result<Option<Track>, String>
where
    F: FnMut(Request) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let Some(request) = RequestedRecording::from_track(original) else {
        return Ok(None);
    };
    let raw = fetch(Request::Search(request.query.clone())).await?;
    let Ok(page) = serde_json::from_value::<SearchPage>(raw) else {
        return Ok(None);
    };
    if page.total != page.data.len() || page.total > SEARCH_LIMIT || page.next.is_some() {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    if page.data.iter().any(|row| !seen.insert(row.id)) {
        return Ok(None);
    }
    let compatible = page
        .data
        .into_iter()
        .filter(|row| request.same_listed_recording(row))
        .collect::<Vec<_>>();
    // A same-title, same-mix result with contradictory fields makes the
    // complete page ambiguous, even if another result has usable details.
    if compatible.len() > DETAIL_LIMIT || compatible.iter().any(|row| !request.matches(row)) {
        return Ok(None);
    }
    let mut observed = Vec::<ConfirmedTrack>::new();
    for found in compatible {
        let raw = fetch(Request::Detail(found.id)).await?;
        let Ok(detail) = serde_json::from_value::<TrackDetail>(raw) else {
            return Ok(None);
        };
        if detail.summary.id != found.id
            || detail.summary.artist.id != found.artist.id
            || detail.summary.duration != found.duration
        {
            return Ok(None);
        }
        let Some(current) = ConfirmedTrack::from_detail(&request, detail) else {
            return Ok(None);
        };
        if observed
            .first()
            .is_some_and(|previous| previous.isrc != current.isrc)
        {
            return Ok(None);
        }
        observed.push(current);
    }
    let Some(first) = observed.first() else {
        return Ok(None);
    };
    let raw = fetch(Request::Isrc(first.isrc.clone())).await?;
    let Ok(detail) = serde_json::from_value::<TrackDetail>(raw) else {
        return Ok(None);
    };
    let Some(canonical) = ConfirmedTrack::from_detail(&request, detail) else {
        return Ok(None);
    };
    Ok(materialize(original, &request, &observed, canonical))
}

pub(super) async fn prepare(original: &Track) -> Result<Option<Track>, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Burrata/0.1 (public recording metadata)")
        .build()
        .map_err(|_| "Public Deezer catalog client is unavailable")?;
    prepare_with_fetch(original, |request| fetch_json(&client, request)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const EXACT_TITLE: &str = "Plástico (Bugz In The Attic Remix)";
    const QUERY: &str = "Plastico (Bugz In The Attic Remix) Willie Colon";
    const REISSUE_ID: u64 = 687771722;
    const ORIGINAL_ID: u64 = 687068872;
    const ISRC: &str = "USDBB0610599";

    fn saved() -> Track {
        let source = "https://www.traxsource.com/track/1494176/plastico-bugz-in-the-attic-remix";
        let title = "Plastico (Bugz In The Attic Remix)";
        let artist = "Willie Colon, Ruben Blades";
        let credits = json!([
            {"name":"Willie Colon","role":"artist"},
            {"name":"Ruben Blades","role":"artist"},
        ]);
        Track {
            id: "traxsource:1494176".into(),
            provider: "traxsource".into(),
            provider_id: "1494176".into(),
            source_url: source.into(),
            artist: artist.into(),
            title: title.into(),
            version: Some("(6:90)".into()),
            duration_seconds: Some(450.),
            release_date: Some("2012-02-28".into()),
            label: Some("Fania".into()),
            provenance: json!({
                "sourceKind":"chart", "artistCredits":credits,
                "rawCaptures":[{"providerId":"1494176","sourceUrl":source,"title":title,
                    "artist":artist,"version":"(6:90)","durationSeconds":450.0,"artistCredits":credits}]
            }),
            ..Default::default()
        }
    }

    fn row(id: u64) -> Value {
        json!({
            "id":id, "title":EXACT_TITLE, "title_short":"Plástico",
            "title_version":"(Bugz In The Attic Remix)", "duration":369,
            "artist":{"id":5611,"name":"Willie Colón"}
        })
    }

    fn detail(id: u64) -> Value {
        let mut value = row(id);
        value["album"] = json!({"id":if id == REISSUE_ID { 98557592 } else { 98434862 }});
        value["isrc"] = json!(ISRC);
        value["contributors"] = json!([
            {"id":5611,"name":"Willie Colón","role":"Main"},
            {"id":15796,"name":"Ruben Blades","role":"Main"},
            {"id":10434,"name":"Bugz in the Attic","role":"Main"},
            {"id":10434,"name":"Bugz in the Attic","role":"Featured"}
        ]);
        value
    }

    async fn lookup(
        saved: &Track,
        search: Value,
        first: Value,
        canonical: Value,
    ) -> (Option<Track>, Vec<Request>) {
        let calls = RefCell::new(Vec::new());
        let result = prepare_with_fetch(saved, |request| {
            let response = match &request {
                Request::Search(query) => {
                    assert_eq!(query, QUERY);
                    search.clone()
                }
                Request::Detail(id) if *id == REISSUE_ID => first.clone(),
                Request::Detail(id) if *id == ORIGINAL_ID => canonical.clone(),
                Request::Isrc(code) => {
                    assert_eq!(code, ISRC);
                    canonical.clone()
                }
                unexpected => panic!("Unexpected API request {unexpected:?}"),
            };
            calls.borrow_mut().push(request);
            std::future::ready(Ok(response))
        })
        .await
        .unwrap();
        (result, calls.into_inner())
    }

    fn terminal() -> Value {
        json!({"total":1,"data":[row(REISSUE_ID)]})
    }

    #[tokio::test]
    async fn real_metadata_family_requires_a_distinct_isrc_confirmation_and_preserves_saved_row() {
        let original = saved();
        let (verified, requests) = lookup(
            &original,
            terminal(),
            detail(REISSUE_ID),
            detail(ORIGINAL_ID),
        )
        .await;
        let verified = verified.expect("release family should agree");
        assert_eq!(
            requests,
            [
                Request::Search(QUERY.into()),
                Request::Detail(REISSUE_ID),
                Request::Isrc(ISRC.into())
            ]
        );
        assert_eq!(verified.id, original.id);
        assert_eq!(verified.source_url, original.source_url);
        assert_eq!(verified.release_date, original.release_date);
        assert_eq!(
            verified.provenance["rawCaptures"],
            original.provenance["rawCaptures"]
        );
        assert_eq!(verified.duration_seconds, Some(369.0));
        assert_eq!(verified.version, None);
        assert_eq!(verified.title, EXACT_TITLE);
        assert_eq!(verified.artist, "Willie Colón, Ruben Blades");
        assert_eq!(verified.isrc.as_deref(), Some(ISRC));
        assert_eq!(original.duration_seconds, Some(450.));
        assert_eq!(original.version.as_deref(), Some("(6:90)"));
        assert_eq!(original.isrc, None);
    }

    #[tokio::test]
    async fn incomplete_page_wrong_detail_id_and_unproven_single_release_are_rejected() {
        let original = saved();
        let mut contradictory = row(ORIGINAL_ID);
        contradictory["title_version"] = json!("(Original Mix)");
        for search in [
            json!({"total":2,"data":[row(REISSUE_ID)]}),
            json!({"total":1,"data":[row(REISSUE_ID)],"next":"https://api.deezer.com/search?index=1"}),
            json!({"total":2,"data":[row(REISSUE_ID),row(REISSUE_ID)]}),
            json!({"total":2,"data":[row(REISSUE_ID),contradictory]}),
        ] {
            let (result, calls) =
                lookup(&original, search, detail(REISSUE_ID), detail(ORIGINAL_ID)).await;
            assert!(result.is_none());
            assert_eq!(calls.len(), 1);
        }
        let (result, calls) = lookup(
            &original,
            terminal(),
            detail(ORIGINAL_ID),
            detail(ORIGINAL_ID),
        )
        .await;
        assert!(result.is_none());
        assert_eq!(calls.len(), 2);
        let (result, _) = lookup(
            &original,
            terminal(),
            detail(REISSUE_ID),
            detail(REISSUE_ID),
        )
        .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn conflicting_edition_credits_duration_or_isrc_cannot_enter_the_family() {
        let original = saved();
        let release = detail(REISSUE_ID);
        for (pointer, replacement) in [
            ("/title_version", json!("(Original Mix)")),
            ("/title_short", json!("Other Title")),
            ("/duration", json!(337)),
            ("/isrc", json!("USDBB0610699")),
            ("/contributors/3/name", json!("Someone Else")),
            ("/contributors/3/id", json!(1234)),
            ("/contributors/3/role", json!("Composer")),
            ("/contributors/3/role", json!("Main")),
            ("/contributors/0/role", json!("Featured")),
            ("/album/id", json!(98557592)),
        ] {
            let mut competing = detail(ORIGINAL_ID);
            *competing.pointer_mut(pointer).unwrap() = replacement;
            let (result, _) = lookup(&original, terminal(), release.clone(), competing).await;
            assert!(result.is_none(), "{pointer}");
        }
        let mut competing = detail(ORIGINAL_ID);
        competing["isrc"] = json!("USDBB0610699");
        let search = json!({"total":2,"data":[row(REISSUE_ID),row(ORIGINAL_ID)]});
        let (result, calls) = lookup(&original, search, release, competing).await;
        assert!(result.is_none());
        assert_eq!(calls.len(), 3);
    }

    #[tokio::test]
    async fn no_lookup_for_changed_capture_or_manual_edit() {
        let mut original = saved();
        original.provenance["manualFields"]["title"] = json!(true);
        let (result, calls) = lookup(
            &original,
            terminal(),
            detail(REISSUE_ID),
            detail(ORIGINAL_ID),
        )
        .await;
        assert!(result.is_none());
        assert!(calls.is_empty());
        original.provenance["manualFields"] = Value::Null;
        original.provenance["rawCaptures"][0]["durationSeconds"] = json!(369.0);
        let (result, calls) = lookup(
            &original,
            terminal(),
            detail(REISSUE_ID),
            detail(ORIGINAL_ID),
        )
        .await;
        assert!(result.is_none());
        assert!(calls.is_empty());
    }
}
