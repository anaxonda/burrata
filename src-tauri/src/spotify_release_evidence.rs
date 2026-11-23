//! Complete, account-checked Spotify release metadata for independent comparison.
use super::*;
use tokio::{sync::watch, time::Instant};

const SEARCH_PAGE_SIZE: usize = 30;
const MAX_SEARCH_RESULTS: usize = SEARCH_PAGE_SIZE * 2;
const MAX_ALBUM_CANDIDATES: usize = 3;
const TRACK_PAGE_SIZE: usize = 50;
const MAX_ALBUM_TRACKS: usize = TRACK_PAGE_SIZE * 2;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndependentAlbumEvidence {
    pub id: String,
    pub title: String,
    pub label: String,
    pub release_date: String,
    pub product_codes: Vec<String>,
    pub tracks: Vec<IndependentAlbumTrackEvidence>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndependentAlbumTrackEvidence {
    pub id: String,
    pub title: String,
    pub version: Option<String>,
    pub artist_credits: Vec<String>,
    pub disc_number: u32,
    pub track_number: u32,
    pub duration_seconds: f64,
    pub isrc: Option<String>,
}

struct ReleaseQuery {
    title: String,
    label: String,
    date: chrono::NaiveDate,
}

struct ConnectedRead<'a> {
    service: &'a RemotePlaylistService,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    session_revision: Option<u64>,
    operation_cancel: Arc<AtomicBool>,
}

impl Drop for ConnectedRead<'_> {
    fn drop(&mut self) {
        self.operation_cancel.store(true, Ordering::Release);
    }
}

impl ConnectedRead<'_> {
    async fn operation(&self, name: &str, args: Value) -> Result<Option<Value>> {
        if self.service.bridge.provider_session_revision() != self.session_revision {
            return Ok(None);
        }
        let outcome = await_crosswalk_step(
            self.deadline,
            self.cancel.clone(),
            self.service.bridge.provider_operation_cancellable(
                "spotify",
                name,
                args,
                Some(self.operation_cancel.clone()),
            ),
        )
        .await;
        if *self.cancel.borrow() {
            return Err("Spotify release lookup was canceled".into());
        }
        Ok(outcome.ok().filter(|_| {
            Instant::now() < self.deadline
                && self.service.bridge.provider_session_revision() == self.session_revision
        }))
    }
}

impl RemotePlaylistService {
    /// A successful result includes every eligible release, with every track in
    /// source order. The caller must independently establish release equivalence.
    pub(crate) async fn find_spotify_release_candidates(
        &self,
        title: &str,
        release_date: &str,
        label: &str,
        cancel: watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<Option<Vec<IndependentAlbumEvidence>>> {
        if *cancel.borrow() {
            return Err("Spotify release lookup was canceled".into());
        }
        let Some(query) = release_query(title, release_date, label) else {
            return Ok(None);
        };
        if Instant::now() >= deadline
            || self.load_state()?.forgotten_providers.contains("spotify")
            || !self.bridge.provider_operations_supported()
        {
            return Ok(None);
        }
        let read = ConnectedRead {
            service: self,
            cancel,
            deadline,
            session_revision: self.bridge.provider_session_revision(),
            operation_cancel: Arc::new(AtomicBool::new(false)),
        };
        let Some(status) = read.operation("status", json!({})).await? else {
            return Ok(None);
        };
        let Some(account) = response_account(&status) else {
            return Ok(None);
        };
        let Some(ids) = read_search(&read, account, title.trim(), &query).await? else {
            return Ok(None);
        };
        let mut releases = Vec::with_capacity(ids.len());
        for id in ids {
            match read_album(&read, account, &id, &query).await? {
                AlbumRead::Ineligible => {}
                AlbumRead::Incomplete => return Ok(None),
                AlbumRead::Complete(album) => releases.push(album),
            }
        }
        let Some(final_status) = read.operation("status", json!({})).await? else {
            return Ok(None);
        };
        if response_account(&final_status) != Some(account)
            || self.load_state()?.forgotten_providers.contains("spotify")
            || Instant::now() >= deadline
            || self.bridge.provider_session_revision() != read.session_revision
        {
            return Ok(None);
        }
        if *read.cancel.borrow() {
            return Err("Spotify release lookup was canceled".into());
        }
        Ok(Some(releases))
    }
}

fn bounded_text(value: &Value, max_len: usize) -> Option<&str> {
    value.as_str().map(str::trim).filter(|value| {
        !value.is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
    })
}

fn key(value: &str) -> String {
    crate::recording_identity::search_fold(value)
}

fn full_date(value: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|date| date.format("%Y-%m-%d").to_string() == value)
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .filter(|date| date.offset().local_minus_utc() == 0)
                .filter(|date| date.time() == chrono::NaiveTime::MIN)
                .map(|date| date.date_naive())
        })
}

fn release_query(title: &str, date: &str, label: &str) -> Option<ReleaseQuery> {
    if title.trim().len() > 200
        || label.trim().len() > 256
        || title.chars().chain(label.chars()).any(char::is_control)
    {
        return None;
    }
    let title = key(title);
    let label = key(label);
    if title.is_empty() || label.is_empty() {
        return None;
    }
    Some(ReleaseQuery {
        title,
        label,
        date: full_date(date)?,
    })
}

fn response_account(page: &Value) -> Option<&str> {
    bounded_text(&page["accountId"], 256).filter(|id| page["accountId"].as_str() == Some(*id))
}

struct CompletePage<'a> {
    rows: &'a [Value],
    total: usize,
    next: Option<usize>,
}

fn complete_page<'a>(
    page: &'a Value,
    account: &str,
    offset: usize,
    limit: usize,
    max: usize,
    prior_total: Option<usize>,
) -> Option<CompletePage<'a>> {
    if response_account(page) != Some(account) || page["unrecognizedCount"].as_u64() != Some(0) {
        return None;
    }
    let rows = page["items"].as_array()?;
    let total = usize::try_from(page["total"].as_u64()?).ok()?;
    if total > max
        || prior_total.is_some_and(|prior_total| total != prior_total)
        || rows.len() != total.checked_sub(offset)?.min(limit)
        || page["rawCount"].as_u64() != Some(rows.len() as u64)
    {
        return None;
    }
    let end = offset.checked_add(rows.len())?;
    let next = if end == total {
        if !page.get("nextCursor")?.is_null() {
            return None;
        }
        None
    } else {
        if page["nextCursor"].as_str()? != end.to_string() {
            return None;
        }
        Some(end)
    };
    Some(CompletePage { rows, total, next })
}

fn page_arguments(mut base: Value, offset: usize) -> Value {
    if offset != 0 {
        base["cursor"] = json!(offset.to_string());
    }
    base
}

fn album_row(row: &Value) -> Option<(&str, &str, &Value)> {
    let id = row["id"].as_str().filter(|id| valid_spotify_id(id))?;
    let title = bounded_text(&row["title"], 512)?;
    let data = row.get("data")?;
    (row["kind"] == "album"
        && row["url"] == format!("https://open.spotify.com/album/{id}")
        && data["provider"] == "spotify"
        && data["albumId"] == id
        && data["uri"] == format!("spotify:album:{id}")
        && bounded_text(&data["name"], 512)? == title)
        .then_some((id, title, data))
}

fn supported_album_type(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("album") || kind.eq_ignore_ascii_case("compilation")
}

fn search_row_may_match(title: &str, data: &Value, query: &ReleaseQuery) -> bool {
    key(title) == query.title
        && bounded_text(&data["albumType"], 32).is_none_or(supported_album_type)
        && bounded_text(&data["label"], 256).is_none_or(|label| key(label) == query.label)
        && bounded_text(&data["releaseDate"], 64)
            .is_none_or(|date| full_date(date).is_none_or(|date| date == query.date))
}

async fn read_search(
    read: &ConnectedRead<'_>,
    account: &str,
    title: &str,
    query: &ReleaseQuery,
) -> Result<Option<Vec<String>>> {
    let mut offset = 0;
    let mut prior_total = None;
    let mut seen = HashSet::new();
    let mut matches = Vec::new();
    loop {
        let args = page_arguments(
            json!({"query":title,"kind":"album","limit":SEARCH_PAGE_SIZE}),
            offset,
        );
        let Some(raw) = read.operation("search", args).await? else {
            return Ok(None);
        };
        let Some(page) = complete_page(
            &raw,
            account,
            offset,
            SEARCH_PAGE_SIZE,
            MAX_SEARCH_RESULTS,
            prior_total,
        ) else {
            return Ok(None);
        };
        for row in page.rows {
            let Some((id, title, data)) = album_row(row) else {
                return Ok(None);
            };
            if !seen.insert(id.to_owned()) {
                return Ok(None);
            }
            if search_row_may_match(title, data, query) {
                matches.push(id.to_owned());
                if matches.len() > MAX_ALBUM_CANDIDATES {
                    return Ok(None);
                }
            }
        }
        match page.next {
            Some(next) => {
                offset = next;
                prior_total = Some(page.total);
            }
            None => return Ok(Some(matches)),
        }
    }
}

enum AlbumRead {
    Ineligible,
    Incomplete,
    Complete(IndependentAlbumEvidence),
}

fn album_metadata(row: &Value, expected_id: &str, query: &ReleaseQuery, total: usize) -> AlbumRead {
    let Some((id, title, data)) = album_row(row) else {
        return AlbumRead::Incomplete;
    };
    if id != expected_id || data["totalTracks"].as_u64() != Some(total as u64) || total == 0 {
        return AlbumRead::Incomplete;
    }
    let (Some(date), Some(label), Some(kind)) = (
        bounded_text(&data["releaseDate"], 64).and_then(full_date),
        bounded_text(&data["label"], 256),
        bounded_text(&data["albumType"], 32),
    ) else {
        return AlbumRead::Incomplete;
    };
    if key(title) != query.title
        || key(label) != query.label
        || date != query.date
        || !supported_album_type(kind)
    {
        return AlbumRead::Ineligible;
    }
    let mut product_codes = Vec::new();
    if let Some(value) = data.get("productCodes") {
        let Some(rows) = value.as_array().filter(|rows| rows.len() <= 4) else {
            return AlbumRead::Incomplete;
        };
        for row in rows {
            let Some(code) = row.as_str().filter(|code| {
                matches!(code.len(), 8 | 12..=14) && code.bytes().all(|byte| byte.is_ascii_digit())
            }) else {
                return AlbumRead::Incomplete;
            };
            if !product_codes.iter().any(|existing| existing == code) {
                product_codes.push(code.to_owned());
            }
        }
        product_codes.sort();
    }
    AlbumRead::Complete(IndependentAlbumEvidence {
        id: id.to_owned(),
        title: title.to_owned(),
        label: label.to_owned(),
        release_date: date.to_string(),
        product_codes,
        tracks: Vec::new(),
    })
}

fn parse_track(
    row: &Value,
    album_id: &str,
    position: usize,
) -> Option<IndependentAlbumTrackEvidence> {
    let id = row["id"].as_str().filter(|id| valid_spotify_id(id))?;
    if row["provider"] != "spotify"
        || row["trackId"] != id
        || row["albumId"] != album_id
        || row["albumUri"] != format!("spotify:album:{album_id}")
        || row["url"] != format!("https://open.spotify.com/track/{id}")
        || row["position"].as_u64() != Some(position as u64)
        || (!row["linkedUri"].is_null() && row["linkedUri"] != format!("spotify:track:{id}"))
    {
        return None;
    }
    let title = bounded_text(&row["title"], 512)?;
    let disc_number = u32::try_from(row["discNumber"].as_u64()?).ok()?;
    let track_number = u32::try_from(row["trackNumber"].as_u64()?).ok()?;
    let duration_ms = row["durationMilliseconds"]
        .as_u64()
        .filter(|value| (1_000..=86_400_000).contains(value))?;
    let duration_seconds = duration_ms as f64 / 1_000.0;
    if disc_number == 0
        || track_number == 0
        || row["durationSeconds"]
            .as_f64()
            .is_none_or(|reported| (duration_seconds - reported).abs() > 0.000_001)
    {
        return None;
    }
    let artist_rows = row["artistRefs"].as_array()?;
    if artist_rows.is_empty()
        || artist_rows.len() > 32
        || row["artistCount"].as_u64() != Some(artist_rows.len() as u64)
    {
        return None;
    }
    let mut artist_ids = HashSet::new();
    let artist_credits = artist_rows
        .iter()
        .map(|row| {
            let id = row["id"].as_str().filter(|id| valid_spotify_id(id))?;
            (row["provider"] == "spotify" && artist_ids.insert(id))
                .then(|| bounded_text(&row["name"], 256).map(str::to_owned))?
        })
        .collect::<Option<Vec<_>>>()?;
    if row["artist"] != artist_credits.join(", ") {
        return None;
    }
    let context = &row["recordingContext"];
    if (!context["albumId"].is_null() && context["albumId"] != album_id)
        || (!context["albumUri"].is_null()
            && context["albumUri"] != format!("spotify:album:{album_id}"))
        || context["discNumber"].as_u64() != Some(u64::from(disc_number))
        || context["trackNumber"].as_u64() != Some(u64::from(track_number))
    {
        return None;
    }
    let isrc = if context["isrc"].is_null() {
        None
    } else {
        Some(crate::recording_identity::normalize_isrc(
            context["isrc"].as_str()?,
        )?)
    };
    let version = if row["version"].is_null() {
        None
    } else {
        Some(bounded_text(&row["version"], 256)?.to_owned())
    };
    Some(IndependentAlbumTrackEvidence {
        id: id.to_owned(),
        title: title.to_owned(),
        version,
        artist_credits,
        disc_number,
        track_number,
        duration_seconds,
        isrc,
    })
}

fn track_follows(
    previous: Option<&IndependentAlbumTrackEvidence>,
    current: &IndependentAlbumTrackEvidence,
) -> bool {
    let position = (current.disc_number, current.track_number);
    previous.map_or(position == (1, 1), |prior| {
        (prior.disc_number == current.disc_number
            && prior.track_number.checked_add(1) == Some(current.track_number))
            || (prior.disc_number.checked_add(1) == Some(current.disc_number)
                && current.track_number == 1)
    })
}

async fn read_album(
    read: &ConnectedRead<'_>,
    account: &str,
    id: &str,
    query: &ReleaseQuery,
) -> Result<AlbumRead> {
    let mut offset = 0;
    let mut prior_total = None;
    let mut release = None::<IndependentAlbumEvidence>;
    let mut tracks = Vec::new();
    let mut seen_ids = HashSet::new();
    loop {
        let args = page_arguments(json!({"albumId":id,"limit":TRACK_PAGE_SIZE}), offset);
        let Some(raw) = read.operation("getAlbumTracks", args).await? else {
            return Ok(AlbumRead::Incomplete);
        };
        let Some(page) = complete_page(
            &raw,
            account,
            offset,
            TRACK_PAGE_SIZE,
            MAX_ALBUM_TRACKS,
            prior_total,
        ) else {
            return Ok(AlbumRead::Incomplete);
        };
        if raw["albumId"] != id || raw["reportedTotal"].as_u64() != Some(page.total as u64) {
            return Ok(AlbumRead::Incomplete);
        }
        let observed = match album_metadata(&raw["album"], id, query, page.total) {
            AlbumRead::Complete(album) => album,
            AlbumRead::Ineligible if offset == 0 => return Ok(AlbumRead::Ineligible),
            _ => return Ok(AlbumRead::Incomplete),
        };
        if release
            .as_ref()
            .is_some_and(|previous| previous != &observed)
        {
            return Ok(AlbumRead::Incomplete);
        }
        release = Some(observed);
        for (index, row) in page.rows.iter().enumerate() {
            let Some(track) = parse_track(row, id, offset + index + 1) else {
                return Ok(AlbumRead::Incomplete);
            };
            if !seen_ids.insert(track.id.clone()) || !track_follows(tracks.last(), &track) {
                return Ok(AlbumRead::Incomplete);
            }
            tracks.push(track);
        }
        match page.next {
            Some(next) => {
                offset = next;
                prior_total = Some(page.total);
            }
            None => {
                let mut release = release.expect("album metadata checked on each page");
                release.tracks = tracks;
                return Ok(AlbumRead::Complete(release));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    const ALBUM: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const OTHER: &str = "BBBBBBBBBBBBBBBBBBBBBB";
    const ARTIST: &str = "CCCCCCCCCCCCCCCCCCCCCC";

    fn album(id: &str) -> Value {
        json!({
            "id":id,"kind":"album","title":"Remixed Release","url":format!("https://open.spotify.com/album/{id}"),
            "data":{
                "provider":"spotify","albumId":id,"name":"Remixed Release","uri":format!("spotify:album:{id}"),
                "releaseDate":"2012-02-28T00:00:00Z","label":"Fania","productCodes":["123456789012"],
                "albumType":"COMPILATION","totalTracks":51
            }
        })
    }

    fn album_page(id: &str, offset: usize, mode: &str) -> Value {
        let end = (offset + TRACK_PAGE_SIZE).min(51);
        let mut items = (offset..end)
            .map(|index| {
                let track = format!("{index:022}");
                let (disc, track_number) = if index < 50 { (1, index + 1) } else { (2, 1) };
                json!({
                    "id":track,"trackId":track,"provider":"spotify","albumId":id,"albumUri":format!("spotify:album:{id}"),
                    "position":index+1,"discNumber":disc,"trackNumber":track_number,"title":"Named Mix",
                    "artistCount":1,"artist":"Artist","artistRefs":[{"id":ARTIST,"provider":"spotify","name":"Artist"}],
                    "durationMilliseconds":369360,"durationSeconds":369.360,"url":format!("https://open.spotify.com/track/{track}"),
                    "recordingContext":{"isrc":"USDBB0610599","albumId":id,"discNumber":disc,"trackNumber":track_number}
                })
            })
            .collect::<Vec<_>>();
        let unrecognized = usize::from(mode == "partial" && offset == 50);
        if unrecognized > 0 {
            items.clear();
        }
        if mode == "reordered" && offset == 0 {
            items.swap(0, 1);
        }
        json!({
            "accountId":if mode=="changed-account"&&offset==50 {"different"} else {"fixture"},
            "albumId":id,"album":album(id),"items":items,"total":51,"reportedTotal":51,
            "rawCount":end-offset,"unrecognizedCount":unrecognized,
            "nextCursor":(end<51).then(|| end.to_string())
        })
    }

    struct FixtureTransport {
        mode: &'static str,
        revision: AtomicU64,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ProviderTransport for FixtureTransport {
        fn provider_operations_supported(&self) -> bool {
            self.mode != "disconnected"
        }
        fn provider_session_revision(&self) -> Option<u64> {
            Some(self.revision.load(Ordering::Relaxed))
        }
        async fn provider_operation(&self, _: &str, operation: &str, args: Value) -> Result<Value> {
            self.calls.lock().unwrap().push(operation.into());
            match operation {
                "status" => Ok(json!({"accountId":"fixture"})),
                "search" => {
                    let ids = if self.mode == "competing" {
                        &[ALBUM, OTHER][..]
                    } else {
                        &[ALBUM][..]
                    };
                    let mut page = json!({
                        "accountId":"fixture","items":ids.iter().map(|id| album(id)).collect::<Vec<_>>(),
                        "rawCount":ids.len(),"unrecognizedCount":0,"total":ids.len(),"nextCursor":null
                    });
                    if self.mode == "incomplete-search" {
                        page["total"] = Value::Null;
                    }
                    Ok(page)
                }
                "getAlbumTracks" => {
                    if self.mode == "blocked" {
                        return std::future::pending().await;
                    }
                    let offset = args["cursor"].as_str().unwrap_or("0").parse().unwrap();
                    if self.mode == "changed-session" && offset == 50 {
                        self.revision.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(album_page(
                        args["albumId"].as_str().unwrap(),
                        offset,
                        self.mode,
                    ))
                }
                _ => Err("Unexpected mock Spotify operation".into()),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            provider: &str,
            operation: &str,
            args: Value,
            cancel: Option<Arc<AtomicBool>>,
        ) -> Result<Value> {
            assert!(!cancel.unwrap().load(Ordering::Acquire));
            self.provider_operation(provider, operation, args).await
        }
    }

    #[tokio::test]
    async fn spotify_album_evidence_requires_complete_ordered_account_bound_reads() {
        for (mode, expected) in [
            ("complete", Some(1)),
            ("competing", Some(2)),
            ("incomplete-search", None),
            ("partial", None),
            ("reordered", None),
            ("changed-account", None),
            ("changed-session", None),
            ("disconnected", None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(&dir.path().join("release-evidence.sqlite")).unwrap());
            let transport = Arc::new(FixtureTransport {
                mode,
                revision: AtomicU64::new(0),
                calls: Mutex::new(Vec::new()),
            });
            let service = RemotePlaylistService::new(store, transport.clone());
            let (_cancel, watch) = watch::channel(false);
            let releases = service
                .find_spotify_release_candidates(
                    "Remixed Release",
                    "2012-02-28",
                    "Fania",
                    watch,
                    Instant::now() + std::time::Duration::from_secs(2),
                )
                .await
                .unwrap();
            assert_eq!(releases.as_ref().map(Vec::len), expected, "{mode}");
            if let Some(releases) = releases {
                for release in releases {
                    assert_eq!(release.tracks.len(), 51);
                    assert_eq!(release.tracks[4].duration_seconds, 369.360);
                    assert_eq!(release.tracks[4].isrc.as_deref(), Some("USDBB0610599"));
                    assert_eq!(release.tracks[50].disc_number, 2);
                    assert_eq!(release.tracks[50].track_number, 1);
                    assert_eq!(release.product_codes, ["123456789012"]);
                }
                assert_eq!(transport.calls.lock().unwrap().last().unwrap(), "status");
            }
        }
    }

    #[tokio::test]
    async fn spotify_album_evidence_stops_at_the_original_deadline_or_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("release-evidence.sqlite")).unwrap());
        let transport = Arc::new(FixtureTransport {
            mode: "blocked",
            revision: AtomicU64::new(0),
            calls: Mutex::new(Vec::new()),
        });
        let service = RemotePlaylistService::new(store, transport.clone());
        let (stop, watch) = watch::channel(false);
        let deadline = Instant::now() + std::time::Duration::from_millis(50);
        let lookup = |watch, deadline| {
            service.find_spotify_release_candidates(
                "Remixed Release",
                "2012-02-28",
                "Fania",
                watch,
                deadline,
            )
        };
        assert!(lookup(watch.clone(), deadline).await.unwrap().is_none());
        assert_eq!(
            *transport.calls.lock().unwrap(),
            ["status", "search", "getAlbumTracks"]
        );

        let next_deadline = Instant::now() + std::time::Duration::from_secs(2);
        let (outcome, _) = tokio::join!(lookup(watch, next_deadline), async {
            tokio::task::yield_now().await;
            stop.send_replace(true);
        });
        assert_eq!(outcome.unwrap_err(), "Spotify release lookup was canceled");
        assert!(Instant::now() < next_deadline);
    }
}
