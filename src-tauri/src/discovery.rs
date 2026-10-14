//! Per-source unified discovery API. The UI can call `search_source` once per
//! selected source and render each bounded response as it arrives.
use crate::{
    acquisition::{AcquisitionManager, SlskdSearchPoll},
    artists::{ArtistCredentials, ArtistService},
    connections::ConnectionManager,
    models::Track,
    storage::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const MAX_PAGE: usize = 50;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRequest {
    pub query: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub isrc: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub id: String,
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub url: Option<String>,
    pub track: Option<Track>,
    pub data: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPage {
    pub source_id: String,
    pub results: Vec<DiscoveryResult>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub progress: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapability {
    pub id: String,
    pub name: String,
    pub kinds: Vec<String>,
    pub enabled: bool,
    pub detail: Option<String>,
}
pub fn source_registry(connections: &ConnectionManager) -> Result<Vec<SourceCapability>, String> {
    let c = connections.settings()?;
    Ok(vec![
        cap("library", &["track"], true, None),
        cap("musicbrainz", &["artist", "album", "track"], true, None),
        cap(
            "discogs",
            &["artist", "album"],
            c.discogs_enabled && connections.credential("discogs")?.is_some(),
            Some("Enable Discogs in Connections"),
        ),
        cap("youtube", &["track", "video"], true, None),
        cap("soundcloud", &["track", "video"], true, None),
        cap(
            "monochrome",
            &["track"],
            true,
            Some("Direct catalog audio; quality is verified per track"),
        ),
        cap(
            "soulseek",
            &["track", "file"],
            c.slskd.enabled,
            Some("Enable and configure slskd in Connections"),
        ),
    ])
}
fn cap(id: &str, kinds: &[&str], enabled: bool, detail: Option<&str>) -> SourceCapability {
    SourceCapability {
        id: id.into(),
        name: match id {
            "library" => "Library",
            "musicbrainz" => "MusicBrainz",
            "discogs" => "Discogs",
            "beatport" => "Beatport",
            "traxsource" => "Traxsource",
            "spotify" => "Spotify",
            "youtube" => "YouTube",
            "soundcloud" => "SoundCloud",
            "soulseek" => "Soulseek",
            "monochrome" => "Monochrome",
            _ => id,
        }
        .into(),
        kinds: kinds.iter().map(|x| (*x).into()).collect(),
        enabled,
        detail: detail.map(str::to_owned),
    }
}
fn youtube_tokens(value: &str) -> std::collections::HashSet<String> {
    value
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}
fn youtube_relevance(row: &DiscoveryResult, query: &str) -> (usize, usize, usize) {
    let title = row
        .track
        .as_ref()
        .map(|t| t.title.as_str())
        .unwrap_or(row.title.as_str());
    let artist = row
        .track
        .as_ref()
        .map(|t| t.artist.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            row.data
                .pointer("/youtubeMusic/artist")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            row.data
                .pointer("/sourceMetadata/artist")
                .and_then(Value::as_str)
        })
        .or(row.subtitle.as_deref())
        .unwrap_or("");
    let title_tokens = youtube_tokens(title);
    let artist_tokens = youtube_tokens(artist);
    let query_tokens = youtube_tokens(query);
    let title_hits = query_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token))
        .count();
    let total = query_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token) || artist_tokens.contains(*token))
        .count();
    (
        (total == query_tokens.len() && !query_tokens.is_empty()) as usize,
        total,
        title_hits,
    )
}
fn rank_youtube_results(
    rows: &mut [DiscoveryResult],
    query: &str,
    prefer_video: bool,
    prefer_track: bool,
) {
    rows.sort_by(|left, right| {
        let l = youtube_relevance(left, query);
        let r = youtube_relevance(right, query);
        let l_kind = if prefer_video {
            matches!(left.kind.as_str(), "video" | "set")
        } else if prefer_track {
            left.kind == "track"
        } else {
            false
        };
        let r_kind = if prefer_video {
            matches!(right.kind.as_str(), "video" | "set")
        } else if prefer_track {
            right.kind == "track"
        } else {
            false
        };
        (r.0, r.1, r.2, r_kind).cmp(&(l.0, l.1, l.2, l_kind))
    });
}
fn library_search_page(
    store: &Store,
    query: &str,
    page: u32,
    limit: usize,
) -> Result<(Vec<Track>, Option<String>), String> {
    let limit = limit.clamp(1, MAX_PAGE);
    let page = page.clamp(1, 200);
    let offset = (page as usize - 1) * limit;
    let terms = query
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (rows, total) = store.search_library_tracks(&terms, limit, offset)?;
    let next = (offset.saturating_add(limit) < total).then(|| (page + 1).to_string());
    Ok((rows, next))
}
async fn search_musicbrainz_recordings(
    query: &str,
    isrc: Option<&str>,
    limit: usize,
    page: u32,
) -> Result<(Vec<Value>, bool), String> {
    let mut url =
        url::Url::parse("https://musicbrainz.org/ws/2/recording").map_err(|e| e.to_string())?;
    let isrc = isrc.filter(|s| {
        !s.trim().is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    let lucene = isrc
        .map(|s| format!("isrc:{}", s.trim()))
        .unwrap_or_else(|| query.split_whitespace().collect::<Vec<_>>().join(" "));
    let limit = limit.clamp(1, 50);
    url.query_pairs_mut()
        .append_pair("query", &lucene)
        .append_pair("fmt", "json")
        .append_pair("limit", &limit.to_string())
        .append_pair(
            "offset",
            &page
                .saturating_sub(1)
                .saturating_mul(limit as u32)
                .to_string(),
        )
        .append_pair("inc", "artist-credits+isrcs+genres+releases");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Burrata/0.1.0 (music discovery)")
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("MusicBrainz recording search failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("MusicBrainz returned HTTP {}", response.status()));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Invalid MusicBrainz recording response: {e}"))?;
    let more = body["count"]
        .as_u64()
        .is_some_and(|count| count > page.saturating_mul(limit as u32) as u64);
    Ok((
        body["recordings"].as_array().cloned().unwrap_or_default(),
        more,
    ))
}

pub struct DiscoveryService {
    pub artists: Arc<ArtistService>,
    pub acquisition: Arc<AcquisitionManager>,
    pub store: Arc<Store>,
    pub connections: Arc<ConnectionManager>,
}
impl DiscoveryService {
    pub fn new(
        artists: Arc<ArtistService>,
        acquisition: Arc<AcquisitionManager>,
        store: Arc<Store>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        Self {
            artists,
            acquisition,
            store,
            connections,
        }
    }
    pub async fn search_source(
        &self,
        request: DiscoveryRequest,
        source_id: &str,
    ) -> Result<DiscoveryPage, String> {
        let registered_id = if source_id == "slskd" {
            "soulseek"
        } else {
            source_id
        };
        let capability=source_registry(&self.connections)?.into_iter().find(|source|source.id==registered_id).ok_or_else(||format!("{source_id} is not a searchable catalog. Use My library to search saved/imported tracks."))?;
        if !capability.enabled {
            return Err(format!("{} is not configured for search", capability.name));
        }
        let q = request.query.trim();
        if q.is_empty() || q.chars().count() > 500 {
            return Err("Search query must contain 1 to 500 characters".into());
        }
        if !request.source_ids.is_empty() && !request.source_ids.iter().any(|s| s == source_id) {
            return Err("Source is not selected for this search".into());
        }
        let limit = request.limit.unwrap_or(25).clamp(1, MAX_PAGE);
        if matches!(source_id, "slskd" | "soulseek") {
            if let Some(cursor) = request.cursor.as_deref() {
                return self.poll_slskd(cursor).await;
            }
        }
        let page = request
            .cursor
            .as_deref()
            .unwrap_or("1")
            .parse::<u32>()
            .unwrap_or(1)
            .clamp(1, 200);
        let mut out = DiscoveryPage {
            source_id: source_id.into(),
            results: vec![],
            next_cursor: None,
            complete: true,
            progress: None,
            warnings: vec![],
        };
        match source_id {
            "monochrome" => {
                if kind_ok(&request, "track") {
                    for (track, _) in crate::monochrome::Client::new()?
                        .search(q)
                        .await?
                        .into_iter()
                        .take(limit)
                    {
                        let track = self.store.save_track(&track)?;
                        out.results.push(track_result(source_id, track));
                    }
                }
            }
            "library" => {
                if kind_ok(&request, "track") {
                    let (tracks, next) = library_search_page(&self.store, q, page, limit)?;
                    out.next_cursor = next;
                    out.results = tracks
                        .into_iter()
                        .map(|track| track_result(source_id, track))
                        .collect();
                }
            }
            "musicbrainz" => {
                if kind_ok(&request, "artist") {
                    let p = self
                        .artists
                        .search_artists_for(
                            "musicbrainz",
                            q,
                            page,
                            limit.min(25) as u32,
                            &ArtistCredentials::default(),
                        )
                        .await?;
                    for a in p
                        .results
                        .into_iter()
                        .filter(|a| a.artist.provider == "musicbrainz")
                    {
                        let id = format!("musicbrainz:{}", a.artist.id);
                        out.results.push(DiscoveryResult {
                            id,
                            kind: "artist".into(),
                            source_id: source_id.into(),
                            title: a.name.clone(),
                            subtitle: a.disambiguation.clone(),
                            url: Some(a.source_url.clone()),
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    out.next_cursor = p.has_more.then(|| (page + 1).to_string());
                    out.warnings = p.warnings;
                }
                if kind_ok(&request, "track") {
                    let (recordings, more) =
                        search_musicbrainz_recordings(q, request.isrc.as_deref(), limit, page)
                            .await?;
                    if more && out.next_cursor.is_none() {
                        out.next_cursor = Some((page + 1).to_string());
                    }
                    for r in recordings {
                        let rid = r["id"].as_str().unwrap_or_default();
                        let title = r["title"].as_str().unwrap_or("Untitled recording");
                        let credits = r["artist-credit"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|x| {
                                x["name"]
                                    .as_str()
                                    .or(x["artist"]["name"].as_str())
                                    .map(str::to_owned)
                            })
                            .collect::<Vec<_>>();
                        let artist = credits.join(", ");
                        let artist_ids=r["artist-credit"].as_array().into_iter().flatten().filter_map(|x|x["artist"]["id"].as_str().map(|id|json!({"provider":"musicbrainz","id":id,"name":x["artist"]["name"]}))).collect::<Vec<_>>();
                        let genre = r["genres"]
                            .as_array()
                            .and_then(|v| v.first())
                            .and_then(|v| v["name"].as_str())
                            .map(str::to_owned);
                        let recording_track = Track {
                            id: format!("musicbrainz:{rid}"),
                            provider: "musicbrainz".into(),
                            provider_id: rid.into(),
                            source_url: format!("https://musicbrainz.org/recording/{rid}"),
                            artist: artist.clone(),
                            title: title.into(),
                            version: r["disambiguation"]
                                .as_str()
                                .filter(|v| !v.trim().is_empty())
                                .map(str::to_owned),
                            isrc: r["isrcs"]
                                .as_array()
                                .and_then(|v| v.first())
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            duration_seconds: r["length"].as_u64().map(|ms| ms as f64 / 1000.0),
                            release_date: r["first-release-date"].as_str().map(str::to_owned),
                            album: r["releases"]
                                .as_array()
                                .and_then(|v| v.first())
                                .and_then(|v| v["title"].as_str())
                                .map(str::to_owned),
                            genre: genre.clone(),
                            normalized_genre: genre,
                            provenance: json!({"catalog":{"provider":"musicbrainz","recordingId":rid,"artistCredit":credits,"artists":artist_ids,"isrcs":r["isrcs"],"verified":true}}),
                            ..Track::default()
                        };
                        out.results.push(DiscoveryResult {
                            id: format!("musicbrainz:{rid}"),
                            kind: "track".into(),
                            source_id: source_id.into(),
                            title: title.into(),
                            subtitle: (!artist.is_empty()).then_some(artist),
                            url: Some(format!("https://musicbrainz.org/recording/{rid}")),
                            track: Some(recording_track),
                            data: r.clone(),
                        });
                    }
                }
                if kind_ok(&request, "album") {
                    let a = self
                        .artists
                        .search_albums_for("musicbrainz", q, page, &ArtistCredentials::default())
                        .await?;
                    for r in a
                        .items
                        .into_iter()
                        .filter(|r| r.release.provider == "musicbrainz")
                        .take(limit)
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("musicbrainz:{}", r.release.id),
                            kind: "album".into(),
                            source_id: source_id.into(),
                            title: r.title.clone(),
                            subtitle: r.credits.first().map(|c| c.name.clone()),
                            url: Some(r.source_url.clone()),
                            track: None,
                            data: serde_json::to_value(r).unwrap_or(Value::Null),
                        });
                    }
                    if out.next_cursor.is_none() {
                        out.next_cursor = a.next_page.map(|n| n.to_string());
                    }
                    out.warnings.extend(a.warnings);
                }
            }
            "discogs" => {
                let settings = self.connections.settings()?;
                if !settings.discogs_enabled {
                    return Err("Enable Discogs in Connections before searching it".into());
                }
                let credentials = ArtistCredentials {
                    discogs_token: self.connections.credential("discogs")?,
                    ..Default::default()
                };
                let artists = kind_ok(&request, "artist");
                let albums = kind_ok(&request, "album");
                if artists {
                    let p = self
                        .artists
                        .search_artists_for("discogs", q, page, limit.min(25) as u32, &credentials)
                        .await?;
                    for a in p
                        .results
                        .into_iter()
                        .filter(|a| a.artist.provider == "discogs")
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("discogs:{}", a.artist.id),
                            kind: "artist".into(),
                            source_id: source_id.into(),
                            title: a.name.clone(),
                            subtitle: a.disambiguation.clone(),
                            url: Some(a.source_url.clone()),
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    out.warnings.extend(p.warnings);
                }
                if albums {
                    let p = self
                        .artists
                        .search_albums_for("discogs", q, page, &credentials)
                        .await?;
                    for a in p
                        .items
                        .into_iter()
                        .filter(|a| a.release.provider == "discogs")
                        .take(limit)
                    {
                        out.results.push(DiscoveryResult {
                            id: format!("discogs:{}", a.release.id),
                            kind: "album".into(),
                            source_id: source_id.into(),
                            title: a.title.clone(),
                            subtitle: a.credits.first().map(|c| c.name.clone()),
                            url: Some(a.source_url.clone()),
                            track: None,
                            data: serde_json::to_value(a).unwrap_or(Value::Null),
                        });
                    }
                    if let Some(n) = p.next_page {
                        out.next_cursor = Some(n.to_string());
                    }
                    out.warnings.extend(p.warnings);
                }
            }
            "youtube" | "soundcloud" => {
                let settings = self.store.settings().map_err(|e| e.to_string())?;
                let audio_sources = [source_id.to_owned()];
                let wants_track = kind_ok(&request, "track");
                let wants_video = kind_ok(&request, "video") || kind_ok(&request, "set");
                let raw_search = async {
                    if source_id == "youtube" && !wants_video {
                        None
                    } else {
                        Some(
                            self.acquisition
                                .search_audio_tracks(q, &audio_sources, &settings)
                                .await,
                        )
                    }
                };
                let music_search = async {
                    if source_id == "youtube" && wants_track {
                        Some(
                            crate::youtube_music::search(
                                q,
                                request.isrc.as_deref(),
                                &settings.yt_dlp_path,
                            )
                            .await,
                        )
                    } else {
                        None
                    }
                };
                let (rows, music) = tokio::join!(raw_search, music_search);
                let rows = match rows {
                    None => vec![],
                    Some(Ok(rows)) => rows,
                    Some(Err(error)) => {
                        out.warnings.push(error);
                        vec![]
                    }
                };
                for t in rows.into_iter().take(limit) {
                    let mut row = track_result(source_id, t);
                    if source_id == "youtube" {
                        row.kind = "video".into();
                    } else if kind_ok(&request, "video") && !kind_ok(&request, "track") {
                        row.kind = "video".into();
                    }
                    if kind_ok(&request, &row.kind) {
                        out.results.push(row);
                    }
                }
                if let Some(music) = music {
                    let (music, w) = match music {
                        Ok(v) => v,
                        Err(error) => {
                            out.warnings.push(error);
                            (vec![], vec![])
                        }
                    };
                    out.warnings.extend(w);
                    for r in music.into_iter().take(limit) {
                        let source_metadata = json!({"provider":"youtube","catalogVerified":false,"artist":r.artist,"song":r.title,"album":r.album,"sourceGenres":[]});
                        let candidate = json!({"id":r.id,"provider":"youtube","url":r.url,"title":r.title,"uploader":r.artist,"durationSeconds":r.duration_seconds,"formats":[],"permission":"unknown","confidence":"review","score":0.0,"reasons":["YouTube Music result; source identity is unverified"],"official":false,"sourceMetadata":source_metadata});
                        let track = Track {
                            id: format!("youtube:{}", r.id),
                            provider: "youtube".into(),
                            provider_id: r.id.clone(),
                            source_url: r.url.clone(),
                            title: r.title.clone(),
                            artist: r.artist.clone().unwrap_or_default(),
                            album: r.album.clone(),
                            isrc: r.isrc.clone(),
                            artwork_url: r.artwork_url.clone(),
                            duration_seconds: r.duration_seconds,
                            provenance: json!({"source":{"provider":"youtube","id":r.id,"url":r.url,"catalogVerified":false},"sourceMetadata":source_metadata,"audioCandidate":candidate}),
                            ..Track::default()
                        };
                        out.results.push(DiscoveryResult {
                            id: format!("youtube:{}", r.id),
                            kind: "track".into(),
                            source_id: source_id.into(),
                            title: r.title.clone(),
                            subtitle: r.artist.clone(),
                            url: Some(r.url.clone()),
                            track: Some(track),
                            data: json!({"youtubeMusic":r,"candidate":candidate}),
                        });
                    }
                }
            }
            "slskd" | "soulseek" => {
                let id = self.acquisition.start_slskd_search(q).await?;
                out.source_id = "soulseek".into();
                out.complete = false;
                out.progress = Some("searching".into());
                out.next_cursor = Some(id);
            }
            _ => return Err(format!("Unknown discovery source: {source_id}")),
        }
        let mut positions = std::collections::HashMap::<String, usize>::new();
        let mut unique: Vec<DiscoveryResult> = Vec::with_capacity(out.results.len());
        for item in out.results.drain(..) {
            if let Some(position) = positions.get(&item.id).copied() {
                if unique[position].data.get("candidate").is_none()
                    && item.data.get("candidate").is_some()
                {
                    unique[position] = item;
                }
            } else {
                positions.insert(item.id.clone(), unique.len());
                unique.push(item);
            }
        }
        out.results = unique;
        if source_id == "youtube" {
            let wants_video = kind_ok(&request, "video") || kind_ok(&request, "set");
            let wants_track = kind_ok(&request, "track");
            let prefer_video = wants_video && !wants_track;
            let prefer_track = wants_track && !wants_video;
            rank_youtube_results(&mut out.results, q, prefer_video, prefer_track);
        }
        out.results.truncate(limit);
        Ok(out)
    }
    pub async fn poll_slskd(&self, search_id: &str) -> Result<DiscoveryPage, String> {
        let (id, offset) = if let Some((id, offset)) = search_id.split_once('@') {
            (
                id,
                offset
                    .parse::<usize>()
                    .map_err(|_| "Invalid Soulseek result cursor")?,
            )
        } else {
            (search_id, 0)
        };
        if offset > 10_000 {
            return Err("Soulseek result cursor is out of range".into());
        }
        let poll: SlskdSearchPoll = self.acquisition.poll_slskd_search(id).await?;
        let page = slskd_page(poll, id, offset);
        for row in &page.results {
            if let Some(track) = &row.track {
                self.store.save_track(track)?;
            }
        }
        Ok(page)
    }
    pub async fn enrich_source_track(&self, mut track: Track) -> Result<Track, String> {
        if !matches!(track.provider.as_str(), "youtube" | "soundcloud") {
            return Err("Source metadata enrichment supports YouTube and SoundCloud URLs".into());
        }
        let settings = self.store.settings().map_err(|e| e.to_string())?;
        let metadata = crate::youtube_music::inspect_url_with_runtime(
            &track.source_url,
            &settings.yt_dlp_path,
            &settings.js_runtime_path,
        )
        .await?;
        let source_metadata = serde_json::to_value(&metadata).map_err(|e| e.to_string())?;
        track.provenance["sourceMetadata"] = source_metadata.clone();
        if track.artist.is_empty() {
            track.artist = metadata.artist.clone().unwrap_or_default();
        }
        if track.title.is_empty() {
            track.title = metadata
                .song
                .clone()
                .unwrap_or_else(|| metadata.title.clone());
        }
        if track.album.is_none() {
            track.album = metadata.album.clone();
        }
        if track.duration_seconds.is_none() {
            track.duration_seconds = metadata.duration_seconds;
        }
        track.provenance["audioCandidate"] = json!({"id":metadata.id,"provider":track.provider,"url":metadata.url,"title":metadata.title,"uploader":metadata.channel,"durationSeconds":metadata.duration_seconds,"chapters":metadata.chapters,"formats":[],"permission":"unknown","confidence":"review","score":0.0,"reasons":["Source-page metadata is not a catalog identity"],"official":false,"sourceMetadata":source_metadata});
        Ok(track)
    }
    /// MCP-facing aggregate API; each source is isolated so a failing provider
    /// produces its own warning page while other configured sources complete.
    pub async fn async_shared_search(&self, request: DiscoveryRequest) -> Vec<DiscoveryPage> {
        let sources = if request.source_ids.is_empty() {
            source_registry(&self.connections)
                .unwrap_or_default()
                .into_iter()
                .filter(|source| {
                    source.enabled
                        && source.id != "library"
                        && (request.kinds.is_empty()
                            || source.kinds.iter().any(|kind| request.kinds.contains(kind)))
                })
                .map(|source| source.id)
                .collect::<Vec<_>>()
        } else {
            request.source_ids.clone()
        };
        let pages = futures_util::future::join_all(
            sources
                .iter()
                .map(|source| self.search_source(request.clone(), source)),
        )
        .await;
        pages
            .into_iter()
            .zip(sources)
            .map(|(page, source)| {
                page.unwrap_or_else(|error| DiscoveryPage {
                    source_id: source,
                    results: vec![],
                    next_cursor: None,
                    complete: true,
                    progress: None,
                    warnings: vec![error],
                })
            })
            .collect()
    }
}
fn kind_ok(req: &DiscoveryRequest, k: &str) -> bool {
    req.kinds.is_empty() || req.kinds.iter().any(|x| x == k)
}
fn track_result(source: &str, t: Track) -> DiscoveryResult {
    let id = t.id.clone();
    DiscoveryResult {
        id,
        kind: "track".into(),
        source_id: source.into(),
        title: t.title.clone(),
        subtitle: Some(t.artist.clone()),
        url: Some(t.source_url.clone()),
        data: json!({"provenance":t.provenance,"providerId":t.provider_id}),
        track: Some(t),
    }
}
fn slskd_page(p: SlskdSearchPoll, search_id: &str, offset: usize) -> DiscoveryPage {
    let total = p.tracks.len();
    let next_cursor = if !p.complete {
        Some(search_id.to_owned())
    } else if offset.saturating_add(MAX_PAGE) < total {
        Some(format!("{search_id}@{}", offset + MAX_PAGE))
    } else {
        None
    };
    DiscoveryPage {
        source_id: "soulseek".into(),
        results: p
            .tracks
            .into_iter()
            .skip(offset)
            .take(MAX_PAGE)
            .map(|t| track_result("soulseek", t))
            .collect(),
        next_cursor,
        complete: p.complete,
        progress: Some(format!("{} · {} results", p.state, total)),
        warnings: p.warnings,
    }
}

/// Convenient entry points for Tauri command registration.
pub async fn search_source(
    service: &DiscoveryService,
    query: String,
    source_ids: Vec<String>,
    kinds: Vec<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    isrc: Option<String>,
    search_id: Option<String>,
) -> Result<DiscoveryPage, String> {
    if let Some(id) = search_id {
        return service.poll_slskd(&id).await;
    }
    let source = source_ids.first().cloned().ok_or("Select a source")?;
    service
        .search_source(
            DiscoveryRequest {
                query,
                source_ids,
                kinds,
                cursor,
                limit,
                isrc,
            },
            &source,
        )
        .await
}
pub async fn poll_search_source(
    service: &DiscoveryService,
    search_id: &str,
    _source_id: &str,
) -> Result<DiscoveryPage, String> {
    service.poll_slskd(search_id).await
}
pub async fn inspect_youtube_source(
    url: &str,
    yt_dlp_path: &str,
) -> Result<crate::youtube_music::YouTubeSourceMetadata, String> {
    crate::youtube_music::inspect_url(url, yt_dlp_path).await
}
pub async fn enrich_search_result(
    service: &DiscoveryService,
    track: Track,
) -> Result<Track, String> {
    service.enrich_source_track(track).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn yt_row(kind: &str, title: &str, artist: &str, id: &str) -> DiscoveryResult {
        DiscoveryResult {
            id: id.into(),
            kind: kind.into(),
            source_id: "youtube".into(),
            title: title.into(),
            subtitle: Some(artist.into()),
            url: None,
            track: Some(Track {
                title: title.into(),
                artist: artist.into(),
                ..Track::default()
            }),
            data: Value::Null,
        }
    }
    #[test]
    fn youtube_video_intent_ranks_matching_video_ahead_of_music_suggestions() {
        let mut rows = vec![
            yt_row("track", "Salsa Na Ma", "Camilo Azuquita", "music"),
            yt_row("video", "Gia Fu Salsa Dance", "Gia Fu", "video"),
        ];
        rank_youtube_results(&mut rows, "Gia Fu salsa", true, false);
        assert_eq!(rows[0].id, "video");
    }
    #[test]
    fn youtube_track_intent_ranks_native_song_search_by_artist_and_title() {
        let mut rows = vec![
            yt_row("track", "Salsa Na Ma", "Camilo Azuquita", "unrelated"),
            yt_row("track", "Mary", "Big Thief", "mary"),
        ];
        rank_youtube_results(&mut rows, "Mary Big Thief", false, true);
        assert_eq!(rows[0].id, "mary");
    }
    #[test]
    fn registry_contains_searchable_catalogs_and_library_only() {
        let store = Arc::new(Store::open(std::path::Path::new(":memory:")).unwrap());
        let connections = ConnectionManager::new(store);
        let registry = source_registry(&connections).unwrap();
        assert_eq!(
            registry
                .iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "library",
                "musicbrainz",
                "discogs",
                "youtube",
                "soundcloud",
                "monochrome",
                "soulseek"
            ]
        );
        assert!(
            !registry
                .iter()
                .find(|source| source.id == "discogs")
                .unwrap()
                .enabled
        );
        assert!(
            !registry
                .iter()
                .find(|source| source.id == "soulseek")
                .unwrap()
                .enabled
        );
    }
    #[test]
    fn library_search_finds_imported_remixer_metadata_and_pages_saved_tracks() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        for index in 0..4 {
            store
                .save_track(&Track {
                    id: format!("beatport:{index}"),
                    provider: "beatport".into(),
                    provider_id: index.to_string(),
                    artist: "Original Artist".into(),
                    title: format!("Song {index}"),
                    version: Some("Extended Remix".into()),
                    remixers: Some(vec!["Kerri Chandler".into()]),
                    ..Track::default()
                })
                .unwrap();
        }
        let (first, next) = library_search_page(&store, "kerri remix", 1, 3).unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(next.as_deref(), Some("2"));
        let (last, next) = library_search_page(&store, "kerri remix", 2, 3).unwrap();
        assert_eq!(last.len(), 1);
        assert!(next.is_none());
        assert!(!first.iter().any(|track| track.id == last[0].id));
        assert_eq!(last[0].provider, "beatport");
        assert!(library_search_page(&store, "missing artist", 1, 3)
            .unwrap()
            .0
            .is_empty());
    }
    #[test]
    fn library_search_preserves_unicode_case_folding_and_page_order() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        for (id, title) in [("first", "Björk – Jóga"), ("second", "Other")] {
            store
                .save_track(&Track {
                    id: format!("fixture:{id}"),
                    provider: "fixture".into(),
                    provider_id: id.into(),
                    artist: "ÁRNI".into(),
                    title: title.into(),
                    ..Track::default()
                })
                .unwrap();
        }
        let (page, next) = library_search_page(&store, "árni björk", 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "fixture:first");
        assert!(next.is_none());
    }
    fn poll(complete: bool, count: usize) -> SlskdSearchPoll {
        SlskdSearchPoll {
            id: "search".into(),
            state: if complete { "complete" } else { "searching" }.into(),
            complete,
            warnings: vec![],
            candidates: vec![],
            tracks: (0..count)
                .map(|index| Track {
                    id: format!("track-{index}"),
                    provider_id: format!("track-{index}"),
                    ..Track::default()
                })
                .collect(),
        }
    }
    #[test]
    fn empty_partial_search_keeps_poll_cursor_and_complete_results_page_without_gaps() {
        let partial = slskd_page(poll(false, 0), "search", 0);
        assert!(!partial.complete);
        assert_eq!(partial.next_cursor.as_deref(), Some("search"));
        let first = slskd_page(poll(true, 111), "search", 0);
        let second = slskd_page(poll(true, 111), "search", 50);
        let last = slskd_page(poll(true, 111), "search", 100);
        assert_eq!(
            (
                first.results.len(),
                second.results.len(),
                last.results.len()
            ),
            (50, 50, 11)
        );
        assert_eq!(first.next_cursor.as_deref(), Some("search@50"));
        assert_eq!(second.next_cursor.as_deref(), Some("search@100"));
        assert!(last.next_cursor.is_none());
        let ids = first
            .results
            .into_iter()
            .chain(second.results)
            .chain(last.results)
            .map(|row| row.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 111);
    }
}
