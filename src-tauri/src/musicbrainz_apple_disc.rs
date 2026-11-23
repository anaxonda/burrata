//! Complete-disc, request-scoped MusicBrainz/Apple recording crosswalk.
//! Audio identity still requires independent verification after this lookup.
use crate::{models::Track, recording_identity::RecordingIdentity, storage::Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};
use tokio::{sync::watch, time::Instant};

const MAX_DISCS: usize = 12;
const MAX_TRACKS: usize = 200;
const SEARCH_LIMIT: usize = 8;
const MIN_DISC_TRACKS: usize = 6;
const TOLERANCE_MS: u64 = 2;
const BODY_LIMIT: usize = 1_500_000;
const DISC_PROOF_CACHE: &str = "playback.musicbrainz-apple-disc-proof.v1";
const DISC_PROOF_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifiedAppleDisc {
    pub(crate) musicbrainz_release_id: String,
    pub(crate) musicbrainz_recording_id: String,
    pub(crate) apple_album_id: u64,
    pub(crate) apple_track_id: u64,
    pub(crate) disc_number: u16,
    pub(crate) track_number: u16,
    pub(crate) duration_ms: u64,
    pub(crate) catalog_title: String,
    pub(crate) catalog_release_day: String,
    pub(crate) catalog_artist_credit: String,
    artist_aliases: Vec<String>,
    request_fingerprint: String,
}

impl VerifiedAppleDisc {
    pub(crate) fn bound_to(&self, track: &Track) -> bool {
        eligible(track)
            && request_fingerprint(track).is_some_and(|key| key == self.request_fingerprint)
            && track.provenance["releaseId"].as_str() == Some(self.musicbrainz_release_id.as_str())
            && track.provider_id == self.musicbrainz_recording_id
            && self.apple_album_id > 0
            && self.apple_track_id > 0
            && (1..=MAX_DISCS).contains(&usize::from(self.disc_number))
            && (1..=MAX_TRACKS).contains(&usize::from(self.track_number))
            && valid_ms(self.duration_ms)
            && track.duration_seconds.is_none_or(|seconds| {
                seconds.is_finite()
                    && (seconds * 1000.0 - self.duration_ms as f64).abs() <= TOLERANCE_MS as f64
            })
            && track
                .disc_number
                .is_none_or(|number| number == u32::from(self.disc_number))
            && track
                .track_number
                .is_none_or(|number| number == u32::from(self.track_number))
            && self.catalog_release_day.len() == 10
            && chrono::NaiveDate::parse_from_str(&self.catalog_release_day, "%Y-%m-%d").is_ok()
            && crate::classical_movement_title::bwv_catalog_movement_titles_equivalent(
                &track.title,
                &self.catalog_title,
            )
            && self.artist_aliases.len() <= MAX_ALIASES
            && self
                .artist_aliases
                .iter()
                .all(|name| page_text(name).is_some())
    }
}

fn request_fingerprint(track: &Track) -> Option<String> {
    serde_json::to_string(&serde_json::json!({
        "identityKey": crate::remote_playlists::alias_hydration_key(track).ok()?,
        "releaseId": track.provenance["releaseId"],
        "catalogArtistCredit": track.provenance["catalog"]["artistCredit"],
        "trackNumber": track.track_number,
        "discNumber": track.disc_number,
    }))
    .ok()
}

fn mbid(id: &str) -> bool {
    crate::spotify_mb_crosswalk::valid_mbid(id)
}

fn page_text(raw: &str) -> Option<&str> {
    let text = raw.trim();
    (!text.is_empty() && text.len() <= 512 && !text.chars().any(char::is_control)).then_some(text)
}

fn same_title(left: &str, right: &str) -> bool {
    page_text(left)
        .zip(page_text(right))
        .is_some_and(|(left, right)| {
            let left = crate::matching::normalize(left);
            !left.is_empty() && left == crate::matching::normalize(right)
        })
}

pub(crate) fn eligible(track: &Track) -> bool {
    if track.provider != "musicbrainz"
        || !mbid(&track.provider_id)
        || track.id != format!("musicbrainz:{}", track.provider_id)
        || !track.provenance[PLAYBACK_PROOF].is_null()
        || track.source_url != format!("https://musicbrainz.org/recording/{}", track.provider_id)
        || track.provenance["catalog"]["provider"] != "musicbrainz"
        || track.provenance["catalog"]["verified"] != true
        || track.provenance["catalog"]["recordingId"].as_str() != Some(track.provider_id.as_str())
        || !track.provenance["releaseId"].as_str().is_some_and(mbid)
        || track.provenance["catalog"]["artistCredit"]
            .as_array()
            .is_none_or(|credits| {
                !(1..=16).contains(&credits.len())
                    || credits
                        .iter()
                        .any(|value| value.as_str().and_then(page_text).is_none())
            })
        || track.album.as_deref().and_then(page_text).is_none()
        || page_text(&track.title).is_none()
        || track.playback_range.is_some()
        || track.local_path.is_some()
        || track.asset_id.is_some()
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
        || [
            "title",
            "artist",
            "artistCredits",
            "version",
            "durationSeconds",
            "isrc",
            "album",
            "releaseDate",
            "trackNumber",
            "discNumber",
        ]
        .iter()
        .any(|field| track.provenance["manualFields"][*field] == true)
    {
        return false;
    }
    let identity = RecordingIdentity::from_track(track);
    identity.isrc.is_none() && !identity.identifier_conflict
}

#[derive(Deserialize)]
struct MbRelease {
    id: String,
    title: String,
    media: Vec<MbDisc>,
}

#[derive(Deserialize)]
struct MbDisc {
    position: u16,
    #[serde(rename = "track-count")]
    track_count: usize,
    tracks: Vec<MbTrack>,
}

#[derive(Deserialize)]
struct MbTrack {
    position: u16,
    title: String,
    length: Option<u64>,
    recording: MbRecording,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Value,
}

#[derive(Deserialize)]
struct MbRecording {
    id: String,
    length: Option<u64>,
}

struct RequestedDisc {
    release_id: String,
    album: String,
    disc_count: usize,
    disc_number: u16,
    track_number: u16,
    runtime_vector: Vec<u64>,
    artist_aliases: Vec<String>,
}

fn valid_ms(value: u64) -> bool {
    (1_000..=86_400_000).contains(&value)
}

fn requested_disc(
    track: &Track,
    body: Value,
    exact_title: &impl Fn(&str, &str) -> bool,
) -> Option<RequestedDisc> {
    let release: MbRelease = serde_json::from_value(body).ok()?;
    if track.provenance["releaseId"].as_str() != Some(release.id.as_str())
        || !(2..=MAX_DISCS).contains(&release.media.len())
        || !same_title(track.album.as_deref()?, &release.title)
    {
        return None;
    }
    let mut occurrence = None;
    let mut total = 0usize;
    for (disc_index, disc) in release.media.iter().enumerate() {
        if usize::from(disc.position) != disc_index + 1
            || disc.track_count != disc.tracks.len()
            || disc.tracks.is_empty()
        {
            return None;
        }
        total = total.checked_add(disc.track_count)?;
        if total > MAX_TRACKS {
            return None;
        }
        for (index, row) in disc.tracks.iter().enumerate() {
            if usize::from(row.position) != index + 1 || !mbid(&row.recording.id) {
                return None;
            }
            if row.recording.id == track.provider_id && occurrence.replace((disc, row)).is_some() {
                return None;
            }
        }
    }

    let (disc, row) = occurrence?;
    let duration = row.length.filter(|&length| valid_ms(length))?;
    if disc.track_count < MIN_DISC_TRACKS
        || !exact_title(&track.title, page_text(&row.title)?)
        || track.duration_seconds.is_some_and(|current| {
            !current.is_finite() || (current * 1000.0 - duration as f64).abs() > TOLERANCE_MS as f64
        })
        || row
            .recording
            .length
            .is_some_and(|ms| ms.abs_diff(duration) > TOLERANCE_MS)
        || track
            .track_number
            .is_some_and(|position| position != u32::from(row.position))
        || track
            .disc_number
            .is_some_and(|number| number != u32::from(disc.position))
    {
        return None;
    }
    let runtime_vector = disc
        .tracks
        .iter()
        .map(|row| row.length.filter(|&ms| valid_ms(ms)))
        .collect::<Option<Vec<_>>>()?;
    // Require varying timing evidence independent of the requested track.
    let mut siblings = runtime_vector
        .iter()
        .enumerate()
        .filter_map(|(index, &ms)| (index + 1 != usize::from(row.position)).then_some(ms))
        .collect::<Vec<_>>();
    siblings.sort_unstable();
    siblings.dedup_by(|a, b| a.abs_diff(*b) <= TOLERANCE_MS);
    if siblings.len() < MIN_DISC_TRACKS - 1 {
        return None;
    }
    let artist_aliases = mb_artist_anchor(track)
        .and_then(|anchor| occurrence_artist_aliases(&row.artist_credit, &anchor))
        .unwrap_or_default();
    Some(RequestedDisc {
        release_id: release.id,
        album: release.title,
        disc_count: release.media.len(),
        disc_number: disc.position,
        track_number: row.position,
        runtime_vector,
        artist_aliases,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppleResults {
    result_count: usize,
    results: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppleAlbum {
    wrapper_type: String,
    collection_type: String,
    collection_id: u64,
    collection_name: String,
    track_count: usize,
    country: String,
}

impl AppleAlbum {
    fn matches(&self, album: &str) -> bool {
        self.wrapper_type == "collection"
            && self.collection_type == "Album"
            && self.collection_id > 0
            && (MIN_DISC_TRACKS..MAX_TRACKS).contains(&self.track_count)
            && self.country == "GBR"
            && same_title(album, &self.collection_name)
    }
}

fn unique_apple_album(album: &str, body: Value) -> Option<AppleAlbum> {
    let page: AppleResults = serde_json::from_value(body).ok()?;
    // A full search window cannot establish uniqueness of the named album.
    if page.result_count != page.results.len() || page.result_count >= SEARCH_LIMIT {
        return None;
    }
    let candidates = page
        .results
        .into_iter()
        .map(serde_json::from_value::<AppleAlbum>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut exact = candidates
        .into_iter()
        .filter(|candidate| same_title(album, &candidate.collection_name));
    let found = exact.next()?;
    (found.matches(album) && exact.next().is_none()).then_some(found)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppleTrack {
    wrapper_type: String,
    kind: String,
    collection_id: u64,
    collection_name: String,
    track_id: u64,
    track_time_millis: u64,
    release_date: Option<String>,
    track_name: String,
    artist_name: String,
    artist_id: u64,
    disc_number: u16,
    disc_count: usize,
    track_number: u16,
    track_count: usize,
    country: String,
}

fn complete_disc_tracks(
    body: Value,
    album: &AppleAlbum,
    requested: &RequestedDisc,
) -> Option<Vec<Vec<AppleTrack>>> {
    let page: AppleResults = serde_json::from_value(body).ok()?;
    if page.result_count != page.results.len() || page.result_count != album.track_count + 1 {
        return None;
    }
    let mut rows = page.results.into_iter();
    let header: AppleAlbum = serde_json::from_value(rows.next()?).ok()?;
    if !header.matches(&requested.album)
        || header.collection_id != album.collection_id
        || header.track_count != album.track_count
    {
        return None;
    }
    let mut discs = (0..requested.disc_count)
        .map(|_| Vec::new())
        .collect::<Vec<_>>();
    let mut ids = HashSet::new();
    for row in rows {
        let row: AppleTrack = serde_json::from_value(row).ok()?;
        if row.wrapper_type != "track"
            || row.kind != "song"
            || row.country != "GBR"
            || row.collection_id != album.collection_id
            || !same_title(&requested.album, &row.collection_name)
            || row.disc_count != requested.disc_count
            || row.track_id == 0
            || !ids.insert(row.track_id)
        {
            return None;
        }
        let index = usize::from(row.disc_number).checked_sub(1)?;
        discs.get_mut(index)?.push(row);
    }
    for disc in &mut discs {
        let count = disc.len();
        if count == 0 {
            return None;
        }
        // Sorting by declared positions accommodates provider response order,
        // while rejecting missing or duplicate positions.
        disc.sort_unstable_by_key(|row| row.track_number);
        if disc.iter().enumerate().any(|(index, row)| {
            usize::from(row.track_number) != index + 1 || row.track_count != count
        }) {
            return None;
        }
    }
    Some(discs)
}

fn retains_known_credit(track: &Track, roster: &str) -> bool {
    let Some(primary) = track.provenance["catalog"]["artistCredit"].as_array() else {
        return false;
    };
    let mut joins = [", ", " & "]
        .into_iter()
        .flat_map(|join| {
            roster
                .match_indices(join)
                .map(move |(at, _)| (at, at + join.len()))
        })
        .take(25)
        .collect::<Vec<_>>();
    if joins.len() > 24 || !(1..=16).contains(&primary.len()) {
        return false;
    }
    joins.sort_unstable();
    let starts = std::iter::once(0).chain(joins.iter().map(|&(_, end)| end));
    let ends = joins
        .iter()
        .map(|&(start, _)| start)
        .chain(std::iter::once(roster.len()));
    let normalize = crate::matching::normalize_single_artist_name;
    primary.iter().all(|name| {
        name.as_str().and_then(page_text).is_some_and(|name| {
            let name = normalize(name);
            !name.is_empty()
                && starts.clone().any(|start| {
                    ends.clone()
                        .filter(|&end| end > start)
                        .any(|end| normalize(&roster[start..end]) == name)
                })
        })
    })
}

fn provider_utc_day(value: &str) -> Option<String> {
    let (day, _) = value.split_once('T')?;
    if day.len() != 10 || !value.ends_with('Z') {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    chrono::DateTime::parse_from_rfc3339(value).ok()?;
    (date.to_string() == day).then(|| day.to_owned())
}

fn match_disc(
    track: &Track,
    requested: RequestedDisc,
    album: AppleAlbum,
    body: Value,
    exact_title: &impl Fn(&str, &str) -> bool,
) -> Option<VerifiedAppleDisc> {
    let discs = complete_disc_tracks(body, &album, &requested)?;
    let mut exact = discs.into_iter().filter(|rows| {
        rows.len() == requested.runtime_vector.len()
            && rows
                .iter()
                .zip(&requested.runtime_vector)
                .all(|(row, &ms)| {
                    valid_ms(row.track_time_millis)
                        && row.track_time_millis.abs_diff(ms) <= TOLERANCE_MS
                })
    });
    let rows = exact.next()?;
    if exact.next().is_some() {
        return None;
    }
    let position = usize::from(requested.track_number).checked_sub(1)?;
    let row = rows.get(position)?;
    if row.disc_number != requested.disc_number
        || row.track_number != requested.track_number
        || row.artist_id == 0
        || !exact_title(&track.title, page_text(&row.track_name)?)
    {
        return None;
    }
    // The retailer's full display credit is evidence. It cannot tell us
    // which credited name has which role, or authorize omitting one.
    let artist = page_text(&row.artist_name)?;
    if !retains_known_credit(track, artist) {
        return None;
    }
    let catalog_artist_credit = artist.to_owned();
    Some(VerifiedAppleDisc {
        musicbrainz_release_id: requested.release_id,
        musicbrainz_recording_id: track.provider_id.clone(),
        apple_album_id: album.collection_id,
        apple_track_id: row.track_id,
        disc_number: row.disc_number,
        track_number: row.track_number,
        duration_ms: row.track_time_millis,
        catalog_title: row.track_name.clone(),
        catalog_release_day: provider_utc_day(row.release_date.as_deref()?)?,
        catalog_artist_credit,
        artist_aliases: requested.artist_aliases,
        request_fingerprint: request_fingerprint(track)?,
    })
}

const PLAYBACK_PROOF: &str = "playbackMusicBrainzAppleDiscProof";
const MAX_ALIASES: usize = 32;

struct MbArtistAnchor<'a> {
    id: &'a str,
    name: &'a str,
    canonical_name: &'a str,
}

fn mb_artist_anchor(track: &Track) -> Option<MbArtistAnchor<'_>> {
    let [name] = track.provenance["catalog"]["artistCredit"]
        .as_array()?
        .as_slice()
    else {
        return None;
    };
    let [artist] = track.provenance["catalog"]["artists"]
        .as_array()?
        .as_slice()
    else {
        return None;
    };
    let name = page_text(name.as_str()?)?;
    let id = artist["id"].as_str().filter(|id| mbid(id))?;
    let display = page_text(artist["name"].as_str()?)?;
    let canonical_name = artist["canonicalName"]
        .as_str()
        .and_then(page_text)
        .unwrap_or(display);
    let norm = crate::matching::normalize_single_artist_name;
    (artist["provider"] == "musicbrainz"
        && norm(display) == norm(name)
        && norm(&track.artist) == norm(name)
        && artist["role"]
            .as_str()
            .is_some_and(crate::recording_identity::is_primary_performer_role))
    .then_some(MbArtistAnchor {
        id,
        name,
        canonical_name,
    })
}

fn anchored_apple_roster(roster: &str, known: &str) -> Option<([String; 2], usize)> {
    let norm = crate::matching::normalize_single_artist_name;
    let normalized = norm(known);
    let joins = [", ", " & "];
    let mut splits = joins
        .iter()
        .flat_map(|&join| {
            roster.match_indices(join).filter_map(move |(index, _)| {
                Some((
                    page_text(&roster[..index])?,
                    page_text(&roster[index + join.len()..])?,
                ))
            })
        })
        .filter_map(|(left, right)| {
            let (left_name, right_name) = (norm(left), norm(right));
            let (anchor, other) = match (left_name == normalized, right_name == normalized) {
                (true, false) => (0, right),
                (false, true) => (1, left),
                _ => return None,
            };
            // Only the known atomic name may itself contain a join. The second
            // cell must be one performer, not an arbitrary unverified list.
            (!normalized.is_empty()
                && !norm(other).is_empty()
                && !joins.iter().any(|join| other.contains(join)))
            .then(|| ([left.to_owned(), right.to_owned()], anchor))
        });
    let found = splits.next()?;
    splits.next().is_none().then_some(found)
}

fn exact_artist_name_aliases(profile: &Value, anchor: &MbArtistAnchor<'_>) -> Option<Vec<String>> {
    if profile["id"].as_str() != Some(anchor.id) {
        return None;
    }
    let canonical = page_text(profile["name"].as_str()?)?;
    let rows = profile["aliases"].as_array()?;
    if rows.len() > 128 {
        return None;
    }
    let norm = crate::matching::normalize_single_artist_name;
    let expected_name = norm(anchor.canonical_name);
    if expected_name.is_empty() || norm(canonical) != expected_name {
        return None;
    }
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();
    for row in rows {
        if row["type"] != "Artist name" {
            continue;
        }
        let name = page_text(row["name"].as_str()?)?;
        let key = norm(name);
        if key.is_empty() {
            return None;
        }
        if key != expected_name && seen.insert(key) {
            aliases.push(name.to_owned());
        }
    }
    (aliases.len() <= MAX_ALIASES).then_some(aliases)
}

fn occurrence_artist_aliases(credit: &Value, anchor: &MbArtistAnchor<'_>) -> Option<Vec<String>> {
    let credits = credit.as_array().filter(|rows| rows.len() <= 16)?;
    let mut anchored = credits
        .iter()
        .filter(|row| row["artist"]["id"] == anchor.id);
    let row = anchored.next()?;
    if anchored.next().is_some()
        || crate::matching::normalize_single_artist_name(page_text(row["name"].as_str()?)?)
            != crate::matching::normalize_single_artist_name(anchor.name)
    {
        return None;
    }
    exact_artist_name_aliases(&row["artist"], anchor)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackDiscProof {
    provider: String,
    catalog_title: String,
    catalog_release_day: String,
    catalog_artist_credit: String,
    apple_album_id: u64,
    apple_track_id: u64,
    musicbrainz_release_id: String,
    musicbrainz_recording_id: String,
    disc_number: u16,
    track_number: u16,
    duration_ms: u64,
    canonical_performers: [String; 2],
    anchor_index: usize,
    anchor_artist_id: String,
    artist_aliases: Vec<String>,
    prepared_fingerprint: String,
}

/// Replace comparison credits on a clone while leaving the existing catalog
/// recording ID intact. The new credits are Apple disc observations rather
/// than MusicBrainz claims or global aliases.
fn project_roster(
    track: &Track,
    names: &[String; 2],
    anchor_index: usize,
    anchor_id: &str,
) -> Option<Track> {
    let mut projected = track.clone();
    projected.artist = names.join(", ");
    let catalog = projected.provenance["catalog"].as_object_mut()?;
    for key in ["artists", "artistCount", "performerCreditsComplete"] {
        catalog.remove(key);
    }
    if !projected.provenance["remoteCatalog"]["artistRefs"].is_null() {
        return None;
    }
    projected.provenance["artistCredits"] = serde_json::json!(names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            serde_json::json!({
                "name":name, "role":"artist",
                "id": (index == anchor_index).then_some(anchor_id),
                "provenance":"musicbrainzAppleCompleteDisc",
            })
        })
        .collect::<Vec<_>>());
    Some(projected)
}

fn playback_projection(original: &Track, proof: VerifiedAppleDisc) -> Option<Track> {
    if !proof.bound_to(original) {
        return None;
    }
    let anchor = mb_artist_anchor(original)?;
    let (names, anchor_index) = anchored_apple_roster(&proof.catalog_artist_credit, anchor.name)?;
    let mut prepared = project_roster(original, &names, anchor_index, anchor.id)?;
    prepared.duration_seconds = Some(proof.duration_ms as f64 / 1000.0);
    let prepared_fingerprint = request_fingerprint(&prepared)?;
    prepared.provenance[PLAYBACK_PROOF] = serde_json::to_value(PlaybackDiscProof {
        provider: "musicbrainzAppleCompleteDisc".into(),
        catalog_title: proof.catalog_title,
        catalog_release_day: proof.catalog_release_day,
        catalog_artist_credit: proof.catalog_artist_credit,
        apple_album_id: proof.apple_album_id,
        apple_track_id: proof.apple_track_id,
        musicbrainz_release_id: proof.musicbrainz_release_id,
        musicbrainz_recording_id: proof.musicbrainz_recording_id,
        disc_number: proof.disc_number,
        track_number: proof.track_number,
        duration_ms: proof.duration_ms,
        canonical_performers: names,
        anchor_index,
        anchor_artist_id: anchor.id.to_owned(),
        artist_aliases: proof.artist_aliases,
        prepared_fingerprint,
    })
    .ok()?;
    Some(prepared)
}

/// Consume only the resolver-owned clone returned by `prepare_playback_track`.
/// The caller still authenticates source ID, work/movement and media runtime.
pub(crate) fn project_observed_performers(prepared: &Track, observed: &[String]) -> Option<Track> {
    let [_, _] = observed else {
        return None;
    };
    let proof: PlaybackDiscProof =
        serde_json::from_value(prepared.provenance[PLAYBACK_PROOF].clone()).ok()?;
    if proof.provider != "musicbrainzAppleCompleteDisc"
        || proof.anchor_index > 1
        || proof.artist_aliases.len() > MAX_ALIASES
        || !mbid(&proof.anchor_artist_id)
        || prepared.provider_id != proof.musicbrainz_recording_id
        || prepared.provenance["releaseId"].as_str() != Some(proof.musicbrainz_release_id.as_str())
        || request_fingerprint(prepared).as_deref() != Some(proof.prepared_fingerprint.as_str())
    {
        return None;
    }
    let norm = crate::matching::normalize_single_artist_name;
    let mut names = proof.canonical_performers.clone();
    let mut used = HashSet::new();
    for (index, wanted) in names.iter_mut().enumerate() {
        let wanted_key = norm(wanted);
        let mut matches = observed.iter().filter_map(|name| {
            let name = page_text(name)?;
            let key = norm(name);
            (!key.is_empty()
                && (key == wanted_key
                    || index == proof.anchor_index
                        && proof.artist_aliases.iter().any(|alias| norm(alias) == key)))
            .then_some((name, key))
        });
        let (name, key) = matches.next()?;
        if matches.next().is_some() || !used.insert(key) {
            return None;
        }
        *wanted = name.to_owned();
    }
    project_roster(
        prepared,
        &names,
        proof.anchor_index,
        &proof.anchor_artist_id,
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Request {
    MusicBrainzRelease(String),
    AppleSearch(String),
    AppleAlbum(u64),
}

async fn fetch_json(client: &reqwest::Client, request: Request) -> Result<Value, String> {
    let pending = match request {
        Request::MusicBrainzRelease(id) => {
            crate::artists::musicbrainz_rate_limit().await;
            client
                .get(format!("https://musicbrainz.org/ws/2/release/{id}"))
                .query(&[
                    ("inc", "recordings+artist-credits+aliases"),
                    ("fmt", "json"),
                ])
        }
        Request::AppleSearch(album) => client.get("https://itunes.apple.com/search").query(&[
            ("term", album.as_str()),
            ("entity", "album"),
            ("country", "GB"),
            ("limit", "8"),
        ]),
        Request::AppleAlbum(id) => client.get("https://itunes.apple.com/lookup").query(&[
            ("id", id.to_string().as_str()),
            ("entity", "song"),
            ("country", "GB"),
            ("limit", "200"),
        ]),
    };
    let mut response = pending
        .send()
        .await
        .map_err(|_| "Public catalog lookup is unavailable")?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|len| len > BODY_LIMIT as u64)
    {
        return Err("Public catalog response is unavailable".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Public catalog response is unavailable")?
    {
        if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err("Public catalog response exceeds its size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid public catalog response".into())
}

async fn prepare_with_fetch<F, Fut>(
    track: &Track,
    exact_title: &impl Fn(&str, &str) -> bool,
    fetch: F,
) -> Result<Option<VerifiedAppleDisc>, String>
where
    F: Fn(Request) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    if !eligible(track) || !exact_title(&track.title, &track.title) {
        return Ok(None);
    }
    let Some(release_id) = track.provenance["releaseId"].as_str() else {
        return Ok(None);
    };
    let Some(saved_album) = track.album.as_deref() else {
        return Ok(None);
    };
    // The saved album seeds a speculative search. Use its response only after
    // the MusicBrainz occurrence validates that exact query.
    let (requested, search) = {
        let mut release_read = std::pin::pin!(async {
            fetch(Request::MusicBrainzRelease(release_id.to_owned())).await
        });
        let mut apple_search =
            std::pin::pin!(async { fetch(Request::AppleSearch(saved_album.to_owned())).await });
        let (release, search) = tokio::select! {
            biased;
            release = &mut release_read => (release?, None),
            search = &mut apple_search => (release_read.await?, Some(search)),
        };
        let Some(requested) = requested_disc(track, release, exact_title) else {
            return Ok(None);
        };
        let search = if requested.album == saved_album {
            Some(match search {
                Some(result) => result?,
                None => apple_search.await?,
            })
        } else {
            None
        };
        (requested, search)
    };
    let search = match search {
        Some(search) => search,
        None => fetch(Request::AppleSearch(requested.album.clone())).await?,
    };
    let Some(album) = unique_apple_album(&requested.album, search) else {
        return Ok(None);
    };
    let details = fetch(Request::AppleAlbum(album.collection_id)).await?;
    Ok(match_disc(track, requested, album, details, exact_title))
}

fn public_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Burrata/0.1 (public recording metadata; https://github.com/anaxonda/burrata)")
        .build()
        .map_err(|_| "Public catalog client is unavailable".into())
}

/// Resolve only the current verified occurrence. The same release response
/// supplies exact-ID aliases for the ephemeral playback clone.
pub(crate) async fn prepare_playback_track(
    store: &Store,
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Option<Track>, String> {
    let exact_title = crate::classical_movement_title::bwv_catalog_movement_titles_equivalent;
    prepare_playback_with_lookup(store, track, cancel, deadline, async {
        let client = public_client()?;
        prepare_with_fetch(track, &exact_title, |request| fetch_json(&client, request)).await
    })
    .await
}

async fn prepare_playback_with_lookup(
    store: &Store,
    track: &Track,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    lookup: impl std::future::Future<Output = Result<Option<VerifiedAppleDisc>, String>>,
) -> Result<Option<Track>, String> {
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let exact_title = crate::classical_movement_title::bwv_catalog_movement_titles_equivalent;
    if !eligible(track)
        || !exact_title(&track.title, &track.title)
        || mb_artist_anchor(track).is_none()
    {
        return Ok(None);
    }
    let deadline = deadline.min(Instant::now() + Duration::from_secs(4));
    let saved = store.get_track(&track.id)?;
    let Some(fingerprint) = request_fingerprint(track) else {
        return Ok(None);
    };
    if saved.as_ref().is_some_and(|row| {
        !eligible(row) || request_fingerprint(row).as_deref() != Some(fingerprint.as_str())
    }) {
        return Ok(None);
    }
    if saved.is_some() {
        let cached = store
            .get_provider_cache(DISC_PROOF_CACHE, &track.id)
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value::<VerifiedAppleDisc>(value).ok())
            .filter(|proof| !proof.artist_aliases.is_empty())
            .and_then(|proof| playback_projection(track, proof));
        if let Some(projected) = cached {
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            return Ok(
                (Instant::now() < deadline && store.get_track(&track.id)? == saved)
                    .then_some(projected),
            );
        }
    }
    let proof = tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel.clone()) => return Err("Cancelled".into()),
        _ = tokio::time::sleep_until(deadline) => return Ok(None),
        result = lookup => result?,
    };
    let unchanged = store.get_track(&track.id)? == saved;
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    if Instant::now() >= deadline || !unchanged {
        return Ok(None);
    }
    let Some(proof) = proof else {
        return Ok(None);
    };
    let payload = (saved.is_some() && !proof.artist_aliases.is_empty())
        .then(|| serde_json::to_value(&proof).ok())
        .flatten();
    let projected = playback_projection(track, proof);
    if let Some(payload) = payload.filter(|_| projected.is_some()) {
        let _ = store.set_provider_cache(
            DISC_PROOF_CACHE,
            &track.id,
            &payload,
            DISC_PROOF_TTL_SECONDS,
        );
    }
    Ok(projected)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    const RECORDING_ID: &str = "5a6316c5-32f8-4792-b10a-e83cfee63bab";
    const MB_TITLE: &str = "J.S. Bach: Brandenburg Concerto No.3 in G, BWV 1048 - 1. (Allegro)";
    const APPLE_TITLE: &str = "Brandenburg Concerto No. 3 in G Major, BWV 1048: I. (Allegro)";

    fn fixture() -> (Track, Value) {
        let bodies: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/bach-mb-apple-disc-fixture.json"
        ))
        .unwrap();
        let track = Track {
            id: format!("musicbrainz:{RECORDING_ID}"),
            provider: "musicbrainz".into(),
            provider_id: RECORDING_ID.into(),
            source_url: format!("https://musicbrainz.org/recording/{RECORDING_ID}"),
            title: MB_TITLE.into(),
            artist: "Gewandhausorchester Leipzig".into(),
            album: Some(bodies["musicbrainz"]["title"].as_str().unwrap().into()),
            duration_seconds: Some(344.533),
            provenance: json!({
                "releaseId": bodies["musicbrainz"]["id"],
                "catalog": {
                    "provider":"musicbrainz", "verified":true,
                    "recordingId": RECORDING_ID,
                    "artistCredit":["Gewandhausorchester Leipzig"],
                    "artists":[{
                        "id":"c51f9557-009f-420b-a933-47889780aad3",
                        "provider":"musicbrainz", "role":"artist credit",
                        "name":"Gewandhausorchester Leipzig", "canonicalName":"Gewandhausorchester Leipzig",
                    }],
                }
            }),
            ..Default::default()
        };
        (track, bodies)
    }

    async fn read(track: &Track, bodies: &Value) -> (Option<VerifiedAppleDisc>, Vec<Request>) {
        let calls = RefCell::new(Vec::new());
        let proof = prepare_with_fetch(
            track,
            &crate::classical_movement_title::bwv_catalog_movement_titles_equivalent,
            |request| {
                let response = match &request {
                    Request::MusicBrainzRelease(id) => {
                        assert_eq!(id, bodies["musicbrainz"]["id"].as_str().unwrap());
                        &bodies["musicbrainz"]
                    }
                    Request::AppleSearch(title) => {
                        assert_eq!(title, bodies["musicbrainz"]["title"].as_str().unwrap());
                        &bodies["appleSearch"]
                    }
                    Request::AppleAlbum(id) => {
                        assert_eq!(*id, 1440750309);
                        &bodies["appleLookup"]
                    }
                };
                calls.borrow_mut().push(request);
                std::future::ready(Ok(response.clone()))
            },
        )
        .await
        .unwrap();
        (proof, calls.into_inner())
    }

    pub(crate) async fn prepared_bach_recording() -> Track {
        let (track, bodies) = fixture();
        let proof = read(&track, &bodies).await.0.unwrap();
        playback_projection(&track, proof).unwrap()
    }

    #[tokio::test]
    async fn reuses_only_current_verified_disc_evidence_after_store_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.db");
        let store = Store::open(&path).unwrap();
        let (track, bodies) = fixture();
        let track = store.save_track(&track).unwrap();
        let (_sender, cancel) = watch::channel(false);
        let deadline = || Instant::now() + Duration::from_secs(1);
        let first =
            prepare_playback_with_lookup(&store, &track, cancel.clone(), deadline(), async {
                Ok(read(&track, &bodies).await.0)
            })
            .await
            .unwrap()
            .unwrap();
        let payload = store
            .get_provider_cache(DISC_PROOF_CACHE, &track.id)
            .unwrap()
            .unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        let warm =
            prepare_playback_with_lookup(&store, &track, cancel.clone(), deadline(), async {
                Err("Warm replay must not fetch the catalog".into())
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(warm, first);
        assert_eq!(store.get_track(&track.id).unwrap().unwrap(), track);

        let mut wrong = payload.clone();
        wrong["durationMs"] = json!(369000);
        store
            .set_provider_cache(DISC_PROOF_CACHE, &track.id, &wrong, 300)
            .unwrap();
        assert!(
            prepare_playback_with_lookup(&store, &track, cancel.clone(), deadline(), async {
                Ok(None)
            })
            .await
            .unwrap()
            .is_none()
        );

        store
            .set_provider_cache(DISC_PROOF_CACHE, &track.id, &payload, 0)
            .unwrap();
        assert!(
            prepare_playback_with_lookup(&store, &track, cancel, deadline(), async { Ok(None) })
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn artist_aliases_are_bound_to_the_unique_exact_occurrence_credit() {
        let (track, bodies) = fixture();
        let mut conflicting = bodies.clone();
        let credit = &mut conflicting["musicbrainz"]["media"][1]["tracks"][4]["artist-credit"][0];
        assert_eq!(credit["artist"]["id"], mb_artist_anchor(&track).unwrap().id);
        credit["artist"]["id"] = json!("ce0b2ab1-2be9-4892-9b43-a5f5d8bf58a0");
        let proof = read(&track, &conflicting).await.0.unwrap();
        assert!(proof.artist_aliases.is_empty());

        let mut duplicated = bodies;
        let credits = duplicated["musicbrainz"]["media"][1]["tracks"][4]["artist-credit"]
            .as_array_mut()
            .unwrap();
        credits.push(credits[0].clone());
        let proof = read(&track, &duplicated).await.0.unwrap();
        assert!(proof.artist_aliases.is_empty());
    }

    #[tokio::test]
    async fn overlaps_album_search_but_rechecks_a_different_exact_query() {
        let (mut track, bodies) = fixture();
        let canonical = bodies["musicbrainz"]["title"].as_str().unwrap();
        let saved = canonical.to_lowercase();
        track.album = Some(saved.clone());
        let search_started = tokio::sync::Notify::new();
        let calls = RefCell::new(Vec::new());
        let proof = tokio::time::timeout(
            Duration::from_secs(1),
            prepare_with_fetch(
                &track,
                &crate::classical_movement_title::bwv_catalog_movement_titles_equivalent,
                |request| {
                    let (calls, search_started, bodies, saved) =
                        (&calls, &search_started, &bodies, &saved);
                    async move {
                        calls.borrow_mut().push(request.clone());
                        Ok(match request {
                            Request::MusicBrainzRelease(_) => {
                                search_started.notified().await;
                                bodies["musicbrainz"].clone()
                            }
                            Request::AppleSearch(title) if title == *saved => {
                                search_started.notify_one();
                                json!({"resultCount":0,"results":[]})
                            }
                            Request::AppleSearch(title) => {
                                assert_eq!(title, canonical);
                                bodies["appleSearch"].clone()
                            }
                            Request::AppleAlbum(1440750309) => bodies["appleLookup"].clone(),
                            Request::AppleAlbum(id) => panic!("Unexpected Apple album {id}"),
                        })
                    }
                },
            ),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(proof.apple_track_id, 1440751180);
        assert_eq!(
            calls.into_inner(),
            [
                Request::MusicBrainzRelease(
                    track.provenance["releaseId"].as_str().unwrap().to_owned()
                ),
                Request::AppleSearch(saved),
                Request::AppleSearch(canonical.to_owned()),
                Request::AppleAlbum(1440750309),
            ]
        );
    }

    #[tokio::test]
    async fn aligns_target_disc_despite_an_extra_track_on_another_disc() {
        let (track, bodies) = fixture();
        let musicbrainz_count: usize = bodies["musicbrainz"]["media"]
            .as_array()
            .unwrap()
            .iter()
            .map(|disc| disc["tracks"].as_array().unwrap().len())
            .sum();
        assert_eq!(musicbrainz_count, 110);
        assert_eq!(bodies["appleLookup"]["resultCount"], 112);
        let original = track.clone();
        let (proof, calls) = read(&track, &bodies).await;
        let proof = proof.expect("complete second disc should agree");
        assert_eq!(calls.len(), 3);
        assert!(proof.bound_to(&track));
        assert_eq!(proof.musicbrainz_recording_id, RECORDING_ID);
        assert_eq!((proof.disc_number, proof.track_number), (2, 5));
        assert_eq!(proof.apple_album_id, 1440750309);
        assert_eq!(proof.apple_track_id, 1440751180);
        assert_eq!(proof.duration_ms, 344533);
        assert_eq!(proof.catalog_release_day, "2010-01-01");
        assert_eq!(proof.catalog_title, APPLE_TITLE);
        assert_eq!(
            proof.catalog_artist_credit,
            "Gewandhausorchester Leipzig & Riccardo Chailly"
        );
        assert_eq!(track, original);
        let mut changed = track;
        changed.provenance["manualFields"]["artist"] = json!(true);
        assert!(!proof.bound_to(&changed));
    }

    #[tokio::test]
    async fn fails_closed_on_incomplete_moved_or_duplicate_occurrences() {
        let (track, bodies) = fixture();
        let mut duplicated = bodies.clone();
        duplicated["musicbrainz"]["media"][0]["tracks"][0]["recording"]["id"] = json!(RECORDING_ID);
        let (proof, calls) = read(&track, &duplicated).await;
        assert!(proof.is_none());
        assert_eq!(calls.len(), 1);

        let mut missing = bodies.clone();
        missing["musicbrainz"]["media"][1]["tracks"]
            .as_array_mut()
            .unwrap()
            .pop();
        let (proof, calls) = read(&track, &missing).await;
        assert!(proof.is_none());
        assert_eq!(calls.len(), 1);

        let mut changed = track.clone();
        changed.track_number = Some(6);
        assert!(read(&changed, &bodies).await.0.is_none());
        let mut moved = bodies.clone();
        moved["appleLookup"]["results"][24]["trackNumber"] = json!(5);
        assert!(read(&track, &moved).await.0.is_none());

        let mut sibling_difference = bodies.clone();
        sibling_difference["appleLookup"]["results"][22]["trackTimeMillis"] = json!(339000);
        assert!(read(&track, &sibling_difference).await.0.is_none());

        let mut ambiguous = bodies.clone();
        let rows = ambiguous["appleLookup"]["results"].as_array_mut().unwrap();
        for position in 1..=20 {
            rows[position]["trackTimeMillis"] = rows[20 + position]["trackTimeMillis"].clone();
        }
        assert!(read(&track, &ambiguous).await.0.is_none());

        let mut incomplete = bodies;
        incomplete["appleLookup"]["results"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(read(&track, &incomplete).await.0.is_none());
    }

    #[tokio::test]
    async fn exact_album_title_movement_and_existing_performer_are_required() {
        let (track, bodies) = fixture();
        let mut duplicate = bodies.clone();
        duplicate["appleSearch"]["results"][1] = duplicate["appleSearch"]["results"][0].clone();
        let (proof, calls) = read(&track, &duplicate).await;
        assert!(proof.is_none());
        assert_eq!(calls.len(), 2);

        let mut full = bodies.clone();
        let first = full["appleSearch"]["results"][0].clone();
        full["appleSearch"]["results"]
            .as_array_mut()
            .unwrap()
            .push(first);
        full["appleSearch"]["resultCount"] = json!(8);
        assert!(read(&track, &full).await.0.is_none());

        let mut different_work = bodies.clone();
        different_work["appleLookup"]["results"][25]["trackName"] =
            json!("Brandenburg Concerto No. 3 in G Major, BWV 1048: III. (Allegro)");
        assert!(read(&track, &different_work).await.0.is_none());

        let mut different_artist = bodies;
        different_artist["appleLookup"]["results"][25]["artistName"] =
            json!("A Different Orchestra & Riccardo Chailly");
        assert!(read(&track, &different_artist).await.0.is_none());
    }

    #[tokio::test]
    async fn alias_replaces_only_the_exact_recording_artist_and_retains_the_other_credit() {
        let (track, fixture) = fixture();
        let original = track.clone();
        let proof = read(&track, &fixture).await.0.unwrap();
        let anchor = mb_artist_anchor(&track).unwrap();
        let profile = json!({
            "id":anchor.id, "name":anchor.canonical_name,
            "aliases":[
                {"type":"Artist name","name":"Gewandhausorchester"},
                {"type":"Search hint","name":"Gewandhaus"},
            ]
        });
        let aliases = exact_artist_name_aliases(&profile, &anchor).unwrap();
        assert_eq!(aliases, ["Gewandhausorchester"]);
        assert!(proof.artist_aliases.contains(&aliases[0]));
        let prepared = playback_projection(&track, proof).unwrap();
        assert_eq!(prepared.title, track.title);
        assert_eq!(
            prepared.artist,
            "Gewandhausorchester Leipzig, Riccardo Chailly"
        );
        assert_eq!(
            prepared.provenance[PLAYBACK_PROOF]["catalogReleaseDay"],
            "2010-01-01"
        );
        assert_eq!(
            prepared.provenance[PLAYBACK_PROOF]["catalogTitle"],
            APPLE_TITLE
        );
        assert_eq!(prepared.provenance["catalog"]["recordingId"], RECORDING_ID);
        let heard = |first: &str, second: &str| [first.to_owned(), second.to_owned()];
        let projected = project_observed_performers(
            &prepared,
            &heard("Riccardo Chailly", "Gewandhausorchester"),
        )
        .unwrap();
        assert_eq!(projected.artist, "Gewandhausorchester, Riccardo Chailly");
        assert_eq!(projected.provenance["artistCredits"][0]["id"], anchor.id);
        assert_eq!(
            projected.provenance["artistCredits"][1]["name"],
            "Riccardo Chailly"
        );
        assert_eq!(projected.id, track.id);
        assert_eq!(track, original);
        assert!(project_observed_performers(
            &prepared,
            &heard("Gewandhausorchester Leipzig", "Riccardo Chailly")
        )
        .is_some());
        for [first, second] in [
            ["Gewandhaus", "Riccardo Chailly"],
            ["Gewandhausorchester", "Another Conductor"],
            ["Gewandhausorchester Leipzig", "Gewandhausorchester"],
            ["Riccardo Chailly", "Riccardo Chailly"],
        ] {
            assert!(project_observed_performers(&prepared, &heard(first, second)).is_none());
        }
        assert!(project_observed_performers(&prepared, &["Gewandhausorchester".into()]).is_none());
        let mut changed = prepared;
        changed.duration_seconds = Some(350.0);
        assert!(project_observed_performers(
            &changed,
            &heard("Gewandhausorchester", "Riccardo Chailly")
        )
        .is_none());

        for (field, value) in [
            ("id", json!("00000000-0000-1000-8000-000000000001")),
            ("name", json!("A Different Orchestra")),
        ] {
            let mut wrong_profile = profile.clone();
            wrong_profile[field] = value;
            assert!(exact_artist_name_aliases(&wrong_profile, &anchor).is_none());
        }
    }

    #[test]
    fn credit_split_refuses_ambiguity_and_date_requires_a_valid_utc_timestamp() {
        let names =
            anchored_apple_roster("Ensemble & Players & Conductor", "Ensemble & Players").unwrap();
        assert_eq!(
            names,
            (["Ensemble & Players".into(), "Conductor".into()], 0)
        );
        assert!(anchored_apple_roster("Ensemble & Players & Conductor", "Ensemble").is_none());
        assert!(anchored_apple_roster("Ensemble & Ensemble", "Ensemble").is_none());
        assert!(anchored_apple_roster("Conductor & Orchestra", "Ensemble").is_none());
        assert_eq!(
            provider_utc_day("2010-01-01T12:00:00Z").as_deref(),
            Some("2010-01-01")
        );
        for invalid in [
            "2010-02-30T12:00:00Z",
            "2010-01-01",
            "2010-01-01T12:00:00+00:00",
            "2010-01-01Txx:00:00Z",
        ] {
            assert!(provider_utc_day(invalid).is_none());
        }
    }

    #[tokio::test]
    async fn invalid_or_edited_requests_make_no_network_calls() {
        let (original, bodies) = fixture();
        let mut changed = original.clone();
        changed.provenance["manualFields"]["durationSeconds"] = json!(true);
        assert!(read(&changed, &bodies).await.1.is_empty());
        let mut changed = original.clone();
        changed.isrc = Some("USDBB0610599".into());
        assert!(read(&changed, &bodies).await.1.is_empty());
        let mut changed = original;
        changed.source_url.push_str("?forged");
        assert!(read(&changed, &bodies).await.1.is_empty());
    }
}
