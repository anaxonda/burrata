//! Request-scoped Spotify contributor-role evidence used only during playback
//! matching. It is never copied into Track provenance or library metadata.

use crate::{
    models::Track,
    spotify_catalog_identity::{spotify_artist_credit_fingerprint, SpotifyArtistCreditFingerprint},
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

// Worker fetches at most two pages of 64; preserve that full bounded result.
const MAX_ROWS: usize = 128;
const MAX_CREDIT_TEXT: usize = 512;
const MAX_ROLE_TEXT: usize = 128;
const PINNED_QUERY_HASH: &str = "e1ae46a21911a3075c1aa29bf09a6c60f9a45b6a2b1132429f10ad06c299b5d7";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpotifyRoleRow {
    pub name: String,
    pub role: String,
    pub role_group: String,
    pub artist_uri: String,
}

/// A read-only positive role context bound to one exact Spotify request.
/// Pagination completion is deliberately separate from credit completeness.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpotifyTrackRoleContext {
    request: Track,
    fingerprint: SpotifyArtistCreditFingerprint,
    query_hash: String,
    rows: Vec<SpotifyRoleRow>,
    pagination_terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CandidateRole {
    Performer,
    NonPerformer(String),
    Unknown,
}

#[derive(Debug, Clone)]
struct CandidateArtist {
    name: String,
    explicit_role: Option<String>,
}

impl SpotifyTrackRoleContext {
    /// Parse the separate provider-operation envelope and bind it to the
    /// request's complete verified Spotify catalog fingerprint.
    pub(crate) fn from_operation_for_track(track: &Track, operation: &Value) -> Option<Self> {
        Self::from_operation_for_request(track, track, operation)
    }

    /// Bind fresh exact-ID catalog metadata to an otherwise unmodified saved
    /// request, then bind the role response to that verified catalog identity.
    /// This permits legacy rows with no stored catalog sidecar only when the
    /// caller has independently performed and validated a fresh exact-ID read.
    pub(crate) fn from_operation_for_request(
        request: &Track,
        verified_catalog_track: &Track,
        operation: &Value,
    ) -> Option<Self> {
        if !request_matches_catalog(request, verified_catalog_track) {
            return None;
        }
        let fingerprint = eligible_fingerprint(verified_catalog_track)?;
        let payload = operation.get("spotifyTrackCredits")?;
        if payload.get("provider").and_then(Value::as_str) != Some("spotify")
            || payload.get("operation").and_then(Value::as_str) != Some("queryNpvArtist")
        {
            return None;
        }
        let query_hash = payload.get("queryHash").and_then(Value::as_str)?;
        if query_hash != PINNED_QUERY_HASH {
            return None;
        }
        let binding = payload.get("binding")?;
        let expected_ids = fingerprint
            .artists
            .iter()
            .map(|artist| artist.id.as_str())
            .collect::<Vec<_>>();
        if expected_ids.iter().collect::<HashSet<_>>().len() != expected_ids.len() {
            return None;
        }
        let returned_ids = binding.get("artistIds")?.as_array()?;
        if binding.get("trackId").and_then(Value::as_str) != Some(fingerprint.track_id.as_str())
            || binding.get("trackUri").and_then(Value::as_str)
                != Some(format!("spotify:track:{}", fingerprint.track_id).as_str())
            || binding.get("trackTitle").and_then(Value::as_str) != Some(fingerprint.title.as_str())
            || binding
                .get("expectedDurationMilliseconds")
                .and_then(Value::as_u64)
                != Some(fingerprint.duration_ms)
            || returned_ids.len() != expected_ids.len()
            || returned_ids
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?
                != expected_ids
            || binding.get("mainArtistUri").and_then(Value::as_str)
                != expected_ids
                    .first()
                    .map(|id| format!("spotify:artist:{id}"))
                    .as_deref()
            || binding.get("bindsThrough").and_then(Value::as_str)
                != Some("exact queryNpvArtist trackUri variable")
            || binding
                .get("providerEchoedTrackIdentity")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return None;
        }

        let credits = payload.get("credits")?;
        let pagination_terminal = credits.get("paginationTerminal").and_then(Value::as_bool)?;
        if credits.get("paginationStatus").and_then(Value::as_str)
            != Some(if pagination_terminal {
                "terminal"
            } else {
                "bounded"
            })
            || !pagination_terminal
            || credits.get("pagesFetched").and_then(Value::as_u64)? == 0
            || credits.get("pagesFetched").and_then(Value::as_u64)? > 2
            || credits.get("rowsWellFormed").and_then(Value::as_bool) != Some(true)
            || credits.get("roleEvidenceComplete").and_then(Value::as_bool) != Some(false)
            || credits
                .get("performerCreditsComplete")
                .and_then(Value::as_bool)
                != Some(false)
            || credits.get("classificationReady").and_then(Value::as_bool) != Some(false)
        {
            return None;
        }
        let values = credits.get("rows")?.as_array()?;
        if values.is_empty()
            || values.len() > MAX_ROWS
            || credits.get("rowCount").and_then(Value::as_u64) != Some(values.len() as u64)
        {
            return None;
        }
        if let Some(total) = credits.get("totalCount").filter(|value| !value.is_null()) {
            let total = total.as_u64()?;
            if total < values.len() as u64 || total != values.len() as u64 {
                // A terminal pagination signal paired with a larger declared
                // total is contradictory; it cannot support role projection.
                return None;
            }
        }
        let mut rows = Vec::with_capacity(values.len());
        let mut seen_rows = HashSet::new();
        let mut uri_names = HashMap::<String, String>::new();
        for value in values {
            let name = bounded_text(value.get("name")?.as_str()?, MAX_CREDIT_TEXT)?;
            let role = bounded_text(value.get("role")?.as_str()?, MAX_ROLE_TEXT)?;
            let role_group = bounded_text(value.get("roleGroup")?.as_str()?, MAX_ROLE_TEXT)?;
            let artist_uri = value.get("artistUri")?.as_str()?.to_owned();
            let id = artist_uri.strip_prefix("spotify:artist:")?;
            if !valid_spotify_id(id) {
                return None;
            }
            if uri_names
                .insert(artist_uri.clone(), name.clone())
                .is_some_and(|previous| previous != name)
            {
                return None;
            }
            let key = row_identity_key(&artist_uri, &role, &role_group);
            if !seen_rows.insert(key) {
                return None;
            }
            rows.push(SpotifyRoleRow {
                name,
                role,
                role_group,
                artist_uri,
            });
        }

        Some(Self {
            request: request.clone(),
            fingerprint,
            query_hash: query_hash.to_owned(),
            rows,
            pagination_terminal,
        })
    }

    pub(crate) fn is_for_track(&self, track: &Track) -> bool {
        same_original_playback_request(&self.request, track, &self.fingerprint)
    }

    pub(crate) fn fingerprint(&self) -> &SpotifyArtistCreditFingerprint {
        &self.fingerprint
    }

    pub(crate) fn query_hash(&self) -> &str {
        &self.query_hash
    }

    pub(crate) fn pagination_terminal(&self) -> bool {
        self.pagination_terminal
    }

    /// Explicit role-group evidence is positive only. Generic Artist/Main
    /// Artist credits are intentionally not promoted to performer roles.
    pub(crate) fn performer_names(&self) -> Vec<String> {
        for artist in &self.fingerprint.artists {
            let roles = self
                .rows
                .iter()
                .filter(|row| row.artist_uri == format!("spotify:artist:{}", artist.id))
                .map(
                    |row| match candidate_role_for_row(&row.role_group, &row.role) {
                        CandidateRole::Performer => 1,
                        CandidateRole::NonPerformer(_) => 2,
                        CandidateRole::Unknown => 0,
                    },
                )
                .filter(|role| *role != 0)
                .collect::<HashSet<_>>();
            if roles.is_empty() {
                // This provider operation is explicitly non-exhaustive. Do
                // not infer that a generic or absent original billing row is
                // a nonperformer just because another artist has a positive
                // performer role.
                return Vec::new();
            }
        }
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for row in &self.rows {
            if candidate_role_for_row(&row.role_group, &row.role) != CandidateRole::Performer {
                continue;
            }
            let id = row.artist_uri.as_str();
            if seen.insert(id) {
                names.push(row.name.clone());
            }
        }
        names
    }

    /// Return a comparison-only role-tagged artist list for the candidate's
    /// raw artist metadata. Unknown names remain untagged (and therefore stay
    /// in normal comparison); only a unique exact-name match to explicit
    /// request roles can classify a candidate item.
    pub(crate) fn project_candidate_artists(&self, metadata: &Value) -> Option<Vec<Value>> {
        if !self.pagination_terminal || self.performer_names().is_empty() {
            return None;
        }
        let raw_names = candidate_artist_names(metadata)?;
        if raw_names.is_empty() || raw_names.len() > 32 {
            return None;
        }
        if raw_names.iter().any(|artist| {
            artist.explicit_role.as_deref().is_some_and(|role| {
                !is_spotify_performer_role(role)
                    && !crate::recording_identity::is_non_performer_credit_role(role)
            })
        }) {
            // Do not silently erase an explicit candidate role just because
            // this Spotify response does not know how to classify it.
            return None;
        }
        let mut role_by_name: HashMap<String, BTreeSet<(String, CandidateRole)>> = HashMap::new();
        for row in &self.rows {
            let role = candidate_role_for_row(&row.role_group, &row.role);
            if role == CandidateRole::Unknown {
                continue;
            }
            role_by_name
                .entry(crate::matching::normalize_single_artist_name(&row.name))
                .or_default()
                .insert((row.artist_uri.clone(), role));
        }
        for artist in &raw_names {
            let key = crate::matching::normalize_single_artist_name(&artist.name);
            let Some(roles) = role_by_name.get(&key) else {
                continue;
            };
            let ids = roles
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<HashSet<_>>();
            if ids.len() != 1 {
                continue;
            }
            let categories = roles
                .iter()
                .map(|(_, role)| match role {
                    CandidateRole::Performer => 1_u8,
                    CandidateRole::NonPerformer(_) => 2_u8,
                    CandidateRole::Unknown => 0_u8,
                })
                .collect::<HashSet<_>>();
            if !categories.contains(&1) && categories.contains(&2) {
                let role_names = roles
                    .iter()
                    .filter_map(|(_, role)| match role {
                        CandidateRole::NonPerformer(role) => Some(role.as_str()),
                        _ => None,
                    })
                    .collect::<HashSet<_>>();
                if role_names.len() > 1 {
                    return None;
                }
            }
        }
        Some(
            raw_names
                .into_iter()
                .map(|artist| {
                    let name = artist.name;
                    if let Some(role) = artist.explicit_role {
                        return json!({"name":name,"role":role});
                    }
                    let key = crate::matching::normalize_single_artist_name(&name);
                    let matches = role_by_name.get(&key);
                    let unique_ids = matches.map(|rows| {
                        rows.iter()
                            .map(|(id, _)| id.as_str())
                            .collect::<HashSet<_>>()
                    });
                    let unique_id = unique_ids
                        .as_ref()
                        .filter(|ids| ids.len() == 1)
                        .and_then(|ids| ids.iter().next());
                    let roles = matches.filter(|_| unique_id.is_some());
                    if let Some(roles) = roles {
                        if roles
                            .iter()
                            .any(|(_, role)| *role == CandidateRole::Performer)
                        {
                            return json!({"name":name,"role":"performer"});
                        }
                        if let Some((_, CandidateRole::NonPerformer(role))) = roles.iter().next() {
                            // Preserve the actual role (for example lyricist
                            // or remixer); never relabel every nonperformer as
                            // a composer.
                            return json!({"name":name,"role":role});
                        }
                    }
                    json!(name)
                })
                .collect(),
        )
    }
}

fn eligible_fingerprint(track: &Track) -> Option<SpotifyArtistCreditFingerprint> {
    if !request_is_unmodified_exact_spotify_track(track) {
        return None;
    }
    spotify_artist_credit_fingerprint(track)
}

pub(crate) fn request_matches_catalog(request: &Track, catalog: &Track) -> bool {
    request_is_unmodified_exact_spotify_track(request)
        && request.provider == catalog.provider
        && request.provider_id == catalog.provider_id
        && request.id == catalog.id
        && request.source_url == catalog.source_url
        && request.title == catalog.title
        && request.artist == catalog.artist
        && request
            .duration_seconds
            .zip(catalog.duration_seconds)
            .is_some_and(|(saved, provider)| {
                saved_duration_agrees_with_precise_catalog(saved, provider)
            })
        && request.isrc.as_deref().is_none_or(|saved| {
            catalog
                .isrc
                .as_deref()
                .and_then(crate::recording_identity::normalize_isrc)
                .is_some_and(|provider| {
                    crate::recording_identity::normalize_isrc(saved).as_deref()
                        == Some(provider.as_str())
                })
        })
}

fn same_original_playback_request(
    request: &Track,
    current: &Track,
    fingerprint: &SpotifyArtistCreditFingerprint,
) -> bool {
    let isrc_matches = match (request.isrc.as_deref(), current.isrc.as_deref()) {
        (Some(saved), Some(current)) => crate::recording_identity::same_isrc(saved, current),
        (Some(_), None) => false,
        (None, None) => true,
        (None, Some(current)) => fingerprint
            .isrc
            .as_deref()
            .is_some_and(|verified| crate::recording_identity::same_isrc(current, verified)),
    };
    request_is_unmodified_exact_spotify_track(request)
        && current.provider == request.provider
        && current.provider_id == request.provider_id
        && current.id == request.id
        && current.source_url == request.source_url
        && current.title == request.title
        && current.artist == request.artist
        && current.duration_seconds == request.duration_seconds
        && current.album == request.album
        && current.version == request.version
        && current.playback_range == request.playback_range
        && current.provenance["manualFields"] == request.provenance["manualFields"]
        && isrc_matches
        && fingerprint.track_id == current.provider_id
}

pub(crate) fn saved_duration_agrees_with_precise_catalog(saved: f64, provider: f64) -> bool {
    if !saved.is_finite() || !provider.is_finite() || saved <= 0.0 || provider <= 0.0 {
        return false;
    }
    saved == provider || (saved.fract() == 0.0 && provider > saved && provider.floor() == saved)
}

fn request_is_unmodified_exact_spotify_track(track: &Track) -> bool {
    track.provider == "spotify"
        && valid_spotify_id(&track.provider_id)
        && track.id == format!("spotify:{}", track.provider_id)
        && track.source_url == format!("https://open.spotify.com/track/{}", track.provider_id)
        && track.title.trim().len() > 0
        && track.artist.trim().len() > 0
        && track
            .duration_seconds
            .is_some_and(|duration| duration.is_finite() && duration > 0.0 && duration <= 86_400.0)
        && track.version.is_none()
        && track.playback_range.is_none()
        && ![
            "artist",
            "artistCredits",
            "title",
            "durationSeconds",
            "isrc",
            "version",
        ]
        .iter()
        .any(|field| track.provenance["manualFields"][field] == true)
}

fn candidate_artist_names(metadata: &Value) -> Option<Vec<CandidateArtist>> {
    if !metadata.is_object() {
        return None;
    }
    let mut found: Option<Vec<CandidateArtist>> = None;
    for key in ["artists", "artistNames"] {
        if let Some(value) = metadata.get(key) {
            if value.is_null() {
                continue;
            }
            let values = value.as_array()?;
            if values.is_empty() {
                continue;
            }
            if values.len() > 32 {
                return None;
            }
            let names = values
                .iter()
                .map(|value| {
                    let (name, explicit_role) = if let Some(name) = value.as_str() {
                        (name, None)
                    } else {
                        let name = value.get("name")?.as_str()?;
                        let role = match value.get("role") {
                            None | Some(Value::Null) => None,
                            Some(Value::String(role)) => Some(bounded_text(role, MAX_ROLE_TEXT)?),
                            Some(_) => return None,
                        };
                        (name, role)
                    };
                    Some(CandidateArtist {
                        name: bounded_text(name, MAX_CREDIT_TEXT)?,
                        explicit_role,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            if let Some(previous) = found.take() {
                found = Some(merge_candidate_sequences(&previous, &names)?);
            } else {
                found = Some(names);
            }
        }
    }
    let names = match found {
        Some(names) => names,
        None => vec![CandidateArtist {
            name: bounded_text(metadata.get("artist")?.as_str()?, MAX_CREDIT_TEXT)?,
            explicit_role: None,
        }],
    };
    for key in ["artistCount", "numberOfArtists", "artist_count"] {
        if let Some(count) = metadata.get(key) {
            if count.as_u64()? != names.len() as u64 {
                return None;
            }
        }
    }
    if let Some(artist) = metadata.get("artist").and_then(Value::as_str) {
        let joined = names
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if crate::matching::normalize_single_artist_name(artist)
            != crate::matching::normalize_single_artist_name(&joined)
        {
            let mut scalar_names = artist
                .split(", ")
                .map(crate::matching::normalize_single_artist_name)
                .collect::<Vec<_>>();
            let mut list_names = names
                .iter()
                .map(|artist| crate::matching::normalize_single_artist_name(&artist.name))
                .collect::<Vec<_>>();
            scalar_names.sort();
            list_names.sort();
            if scalar_names != list_names {
                return None;
            }
        }
    }
    Some(names)
}

fn merge_candidate_sequences(
    left: &[CandidateArtist],
    right: &[CandidateArtist],
) -> Option<Vec<CandidateArtist>> {
    if left.len() != right.len() {
        return None;
    }
    let mut used = vec![false; right.len()];
    let mut merged = Vec::with_capacity(left.len());
    for artist in left {
        let key = crate::matching::normalize_single_artist_name(&artist.name);
        let index = right.iter().enumerate().position(|(index, other)| {
            !used[index] && crate::matching::normalize_single_artist_name(&other.name) == key
        })?;
        used[index] = true;
        let other = &right[index];
        let explicit_role = match (
            artist.explicit_role.as_deref(),
            other.explicit_role.as_deref(),
        ) {
            (None, role) | (role, None) => role.map(str::to_owned),
            (Some(left), Some(right))
                if crate::matching::normalize(left) == crate::matching::normalize(right) =>
            {
                Some(left.to_owned())
            }
            (Some(left), Some(right)) => {
                let left_performer = is_spotify_performer_role(left);
                let right_performer = is_spotify_performer_role(right);
                match (left_performer, right_performer) {
                    (true, true) => Some(left.to_owned()),
                    // A positive explicit performer role survives a parallel
                    // composer/nonperformer credit for the same person.
                    (true, false)
                        if crate::recording_identity::is_non_performer_credit_role(right) =>
                    {
                        Some(left.to_owned())
                    }
                    (false, true)
                        if crate::recording_identity::is_non_performer_credit_role(left) =>
                    {
                        Some(right.to_owned())
                    }
                    _ => return None,
                }
            }
        };
        merged.push(CandidateArtist {
            name: artist.name.clone(),
            explicit_role,
        });
    }
    Some(merged)
}

fn candidate_role_for_row(role_group: &str, role: &str) -> CandidateRole {
    match crate::matching::normalize(role_group).as_str() {
        "performers" if is_spotify_performer_role(role) => CandidateRole::Performer,
        "composition lyrics" if crate::recording_identity::is_non_performer_credit_role(role) => {
            CandidateRole::NonPerformer(crate::matching::normalize(role))
        }
        _ => CandidateRole::Unknown,
    }
}

fn is_spotify_performer_role(role: &str) -> bool {
    crate::recording_identity::is_performer_credit_role(role)
        || matches!(
            crate::matching::normalize(role).as_str(),
            "vocalist" | "vocals" | "singer" | "singers"
        )
}

fn row_identity_key(uri: &str, role: &str, group: &str) -> String {
    format!(
        "{}\0{}\0{}",
        uri,
        crate::matching::normalize(role),
        crate::matching::normalize(group)
    )
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TRACK_ID: &str = "2u9k9X8d1uD4qDLjMhVxU0";
    const ARTISTS: [(&str, &str); 5] = [
        ("3eDT9fwXKuHWFvgZaaYC5v", "Sunidhi Chauhan"),
        ("0NErdIJtuKBjtxKmlqaWdj", "Divya Kumar"),
        ("7HCqGPJcQTyGJ2yqntbuyr", "Amit Trivedi"),
        ("5HcunTidTUrOaf8V0iJcvl", "Nusrat Fateh Ali Khan"),
        ("3GdSQUH1BRtl9UrrtuwJlP", "Irshad Kamil"),
    ];

    fn spotify_track() -> Track {
        let artist = ARTISTS
            .iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        let refs = ARTISTS
            .iter()
            .map(|(id, name)| json!({"provider":"spotify","id":id,"name":name}))
            .collect::<Vec<_>>();
        Track {
            id: format!("spotify:{TRACK_ID}"),
            provider: "spotify".into(),
            provider_id: TRACK_ID.into(),
            source_url: format!("https://open.spotify.com/track/{TRACK_ID}"),
            title: "Halka Halka (From \"Fanney Khan\")".into(),
            artist: artist.clone(),
            duration_seconds: Some(247.381),
            provenance: json!({
                "catalog":{"verified":true,"provider":"spotify","providerId":TRACK_ID,"artists":refs,"artistCount":5},
                "remoteCatalog":{"trackId":TRACK_ID,"title":"Halka Halka (From \"Fanney Khan\")","artist":artist,"durationSeconds":247.381,"artistRefs":refs,"artistCount":5}
            }),
            ..Track::default()
        }
    }

    fn row(index: usize, role: &str, group: &str) -> Value {
        let (artist_id, name) = ARTISTS[index];
        json!({
            "artistUri":format!("spotify:artist:{artist_id}"),
            "name":name,
            "role":role,
            "roleGroup":group
        })
    }

    fn operation(rows: Vec<Value>) -> Value {
        let track = spotify_track();
        let fingerprint = spotify_artist_credit_fingerprint(&track).unwrap();
        json!({
            "accountId":"account-a",
            "spotifyTrackCredits":{
                "provider":"spotify",
                "operation":"queryNpvArtist",
                "queryHash":PINNED_QUERY_HASH,
                "binding":{
                    "trackId":TRACK_ID,
                    "trackUri":format!("spotify:track:{TRACK_ID}"),
                    "trackTitle":fingerprint.title,
                    "expectedDurationMilliseconds":fingerprint.duration_ms,
                    "artistIds":fingerprint.artists.iter().map(|artist|artist.id.as_str()).collect::<Vec<_>>(),
                    "mainArtistUri":format!("spotify:artist:{}",ARTISTS[0].0),
                    "bindsThrough":"exact queryNpvArtist trackUri variable",
                    "providerEchoedTrackIdentity":false
                },
                "credits":{
                    "paginationStatus":"terminal",
                    "paginationTerminal":true,
                    "pagesFetched":2,
                    "rowCount":rows.len(),
                    "totalCount":null,
                    "rows":rows,
                    "rowsWellFormed":true,
                    "roleEvidenceComplete":false,
                    "performerCreditsComplete":false,
                    "classificationReady":false
                }
            }
        })
    }

    fn valid_rows() -> Vec<Value> {
        vec![
            row(0, "Main Artist", "Artist"),
            row(1, "Main Artist", "Artist"),
            row(2, "Main Artist", "Artist"),
            row(3, "Main Artist", "Artist"),
            row(4, "Main Artist", "Artist"),
            row(2, "Composer", "Composition & Lyrics"),
            row(3, "Composer", "Composition & Lyrics"),
            row(4, "Lyricist", "Composition & Lyrics"),
            row(0, "Vocalist", "Performers"),
            row(1, "Vocalist", "Performers"),
        ]
    }

    #[test]
    fn explicit_performer_roles_demote_only_correlated_composer_candidate() {
        let original = spotify_track();
        let operation = operation(valid_rows());
        let context = SpotifyTrackRoleContext::from_operation_for_track(&original, &operation)
            .expect("exact source-bound role context should validate");
        assert_eq!(
            context.performer_names(),
            ["Sunidhi Chauhan", "Divya Kumar"]
        );
        assert!(!context
            .performer_names()
            .contains(&"Amit Trivedi".to_owned()));
        assert!(context.pagination_terminal());
        assert_eq!(context.query_hash().len(), 64);

        let raw_candidate = json!({
            "artist":"Sunidhi Chauhan, Divya Kumar, Amit Trivedi",
            "artists":["Sunidhi Chauhan","Divya Kumar","Amit Trivedi"],
            "artistCount":3
        });
        let before = raw_candidate.clone();
        let projected = context
            .project_candidate_artists(&raw_candidate)
            .expect("coherent candidate artist metadata should be projectable");
        assert_eq!(
            raw_candidate, before,
            "the captured source metadata stays unchanged"
        );
        assert_eq!(
            projected[0],
            json!({"name":"Sunidhi Chauhan","role":"performer"})
        );
        assert_eq!(
            projected[1],
            json!({"name":"Divya Kumar","role":"performer"})
        );
        assert_eq!(
            projected[2],
            json!({"name":"Amit Trivedi","role":"composer"})
        );
    }

    #[test]
    fn generic_artist_main_artist_rows_never_become_performer_roles() {
        let track = spotify_track();
        let context = SpotifyTrackRoleContext::from_operation_for_track(
            &track,
            &operation(vec![
                row(0, "Main Artist", "Artist"),
                row(2, "Main Artist", "Artist"),
            ]),
        )
        .unwrap();
        assert!(context.performer_names().is_empty());
        assert!(context
            .project_candidate_artists(
                &json!({"artists":["Sunidhi Chauhan","Amit Trivedi"],"artistCount":2})
            )
            .is_none());
    }

    #[test]
    fn partial_role_rows_do_not_erase_unclassified_original_billing_credits() {
        let track = spotify_track();
        let context = SpotifyTrackRoleContext::from_operation_for_track(
            &track,
            &operation(vec![
                row(0, "Main Artist", "Artist"),
                row(1, "Vocalist", "Performers"),
            ]),
        )
        .unwrap();

        assert!(context.performer_names().is_empty());
        assert!(
            context
                .project_candidate_artists(&json!({
                    "artists":["Sunidhi Chauhan"],
                    "artistCount":1
                }))
                .is_none(),
            "a partial role list cannot drop generic original credits"
        );
        assert!(
            context
                .project_candidate_artists(&json!({
                    "artists":["Sunidhi Chauhan", "Divya Kumar"],
                    "artistCount":2
                }))
                .is_none(),
            "candidate billing still falls back to the baseline policy"
        );
    }

    #[test]
    fn missing_original_role_row_prevents_non_exhaustive_role_projection() {
        let track = spotify_track();
        let context = SpotifyTrackRoleContext::from_operation_for_track(
            &track,
            &operation(vec![row(1, "Vocalist", "Performers")]),
        )
        .unwrap();
        assert!(context.performer_names().is_empty());
        assert!(context
            .project_candidate_artists(&json!({"artists":["Sunidhi Chauhan"]}))
            .is_none());
    }

    #[test]
    fn unknown_candidate_names_and_explicit_candidate_performers_are_preserved() {
        let track = spotify_track();
        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(valid_rows()))
                .unwrap();
        let projected = context
            .project_candidate_artists(&json!({
                "artists":[
                    {"name":"Amit Trivedi","role":"performer"},
                    "Unlisted Candidate Artist"
                ],
                "artistCount":2
            }))
            .unwrap();
        assert_eq!(
            projected[0],
            json!({"name":"Amit Trivedi","role":"performer"})
        );
        assert_eq!(projected[1], json!("Unlisted Candidate Artist"));
    }

    #[test]
    fn malformed_or_contradictory_candidate_artist_metadata_disables_projection() {
        let track = spotify_track();
        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(valid_rows()))
                .unwrap();
        for malformed in [
            json!({"artists":["Sunidhi Chauhan","Divya Kumar"],"artistCount":3}),
            json!({"artists":["Sunidhi Chauhan"],"artistNames":["Divya Kumar"],"artistCount":1}),
            json!({"artists":["Sunidhi Chauhan"],"artist":"Different Artist","artistCount":1}),
            json!({"artists":["Sunidhi Chauhan",4],"artistCount":2}),
        ] {
            assert!(context.project_candidate_artists(&malformed).is_none());
        }
    }

    #[test]
    fn absent_artist_arrays_fall_through_and_valid_dual_arrays_merge_by_name() {
        let track = spotify_track();
        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(valid_rows()))
                .unwrap();

        let empty_then_valid = context
            .project_candidate_artists(&json!({
                "artists":[],
                "artistNames":["Sunidhi Chauhan", "Amit Trivedi"],
                "artistCount":2
            }))
            .unwrap();
        assert_eq!(
            empty_then_valid,
            vec![
                json!({"name":"Sunidhi Chauhan","role":"performer"}),
                json!({"name":"Amit Trivedi","role":"composer"})
            ]
        );

        let null_then_singular = context
            .project_candidate_artists(&json!({
                "artists":null,
                "artistNames":null,
                "artist":"Sunidhi Chauhan",
                "artistCount":1
            }))
            .unwrap();
        assert_eq!(
            null_then_singular,
            vec![json!({"name":"Sunidhi Chauhan","role":"performer"})]
        );

        assert!(context
            .project_candidate_artists(&json!({
                "artists":"malformed",
                "artistNames":["Sunidhi Chauhan"],
                "artistCount":1
            }))
            .is_none());

        let reordered = context
            .project_candidate_artists(&json!({
                "artists":[
                    {"name":"Sunidhi Chauhan","role":"performer"},
                    {"name":"Amit Trivedi","role":"composer"}
                ],
                "artistNames":[
                    {"name":"Amit Trivedi","role":"composer"},
                    {"name":"Sunidhi Chauhan","role":"performer"}
                ],
                "artistCount":2
            }))
            .unwrap();
        assert_eq!(
            reordered,
            vec![
                json!({"name":"Sunidhi Chauhan","role":"performer"}),
                json!({"name":"Amit Trivedi","role":"composer"})
            ]
        );

        let shared_performer_and_composer = context
            .project_candidate_artists(&json!({
                "artists":[{"name":"Sunidhi Chauhan","role":"performer"}],
                "artistNames":[{"name":"Sunidhi Chauhan","role":"composer"}],
                "artistCount":1
            }))
            .unwrap();
        assert_eq!(
            shared_performer_and_composer,
            vec![json!({"name":"Sunidhi Chauhan","role":"performer"})]
        );
    }

    #[test]
    fn role_context_rejects_fingerprint_role_completeness_and_pagination_conflicts() {
        let track = spotify_track();
        let valid = operation(valid_rows());
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&track, &valid).is_some());

        let mut wrong_title = valid.clone();
        wrong_title["spotifyTrackCredits"]["binding"]["trackTitle"] = json!("Other song");
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&track, &wrong_title).is_none());

        let mut wrong_ids = valid.clone();
        wrong_ids["spotifyTrackCredits"]["binding"]["artistIds"][0] = json!(ARTISTS[1].0);
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&track, &wrong_ids).is_none());

        let mut claimed_complete = valid.clone();
        claimed_complete["spotifyTrackCredits"]["credits"]["performerCreditsComplete"] =
            json!(true);
        assert!(
            SpotifyTrackRoleContext::from_operation_for_track(&track, &claimed_complete).is_none()
        );

        let mut nonterminal = valid.clone();
        nonterminal["spotifyTrackCredits"]["credits"]["paginationTerminal"] = json!(false);
        nonterminal["spotifyTrackCredits"]["credits"]["paginationStatus"] = json!("bounded");
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&track, &nonterminal).is_none());

        let mut manual = track.clone();
        manual.provenance["manualFields"]["artist"] = json!(true);
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&manual, &valid).is_none());

        let mut versioned = track.clone();
        versioned.version = Some("Live".into());
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&versioned, &valid).is_none());
    }

    #[test]
    fn role_context_rejects_unpinned_query_duplicate_request_ids_and_manual_identity() {
        let track = spotify_track();
        let valid = operation(valid_rows());
        let mut wrong_hash = valid.clone();
        wrong_hash["spotifyTrackCredits"]["queryHash"] = json!("a".repeat(64));
        assert!(SpotifyTrackRoleContext::from_operation_for_track(&track, &wrong_hash).is_none());

        let mut duplicate_ids = track.clone();
        let duplicate = duplicate_ids.provenance["catalog"]["artists"][0].clone();
        duplicate_ids.provenance["catalog"]["artists"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        duplicate_ids.provenance["catalog"]["artistCount"] = json!(6);
        duplicate_ids.provenance["remoteCatalog"]["artistRefs"] =
            duplicate_ids.provenance["catalog"]["artists"].clone();
        duplicate_ids.provenance["remoteCatalog"]["artistCount"] = json!(6);
        assert!(
            SpotifyTrackRoleContext::from_operation_for_track(&duplicate_ids, &valid).is_none()
        );

        for manual_field in [
            "artist",
            "artistCredits",
            "title",
            "durationSeconds",
            "isrc",
        ] {
            let mut manual = track.clone();
            manual.provenance["manualFields"][manual_field] = json!(true);
            assert!(
                SpotifyTrackRoleContext::from_operation_for_track(&manual, &valid).is_none(),
                "manual field {manual_field} must keep role context out"
            );
        }
    }

    #[test]
    fn explicit_dual_performer_and_nonperformer_roles_preserve_performer_proof() {
        let track = spotify_track();
        let mut contradictory = valid_rows();
        contradictory.push(row(0, "Composer", "Composition & Lyrics"));
        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(contradictory))
                .unwrap();
        assert_eq!(
            context.performer_names(),
            ["Sunidhi Chauhan", "Divya Kumar"]
        );
        assert_eq!(
            context
                .project_candidate_artists(&json!({"artists":["Sunidhi Chauhan"],"artistCount":1}))
                .unwrap(),
            vec![json!({"name":"Sunidhi Chauhan","role":"performer"})]
        );

        let mut missing = valid_rows();
        let missing_uri = format!("spotify:artist:{}", ARTISTS[1].0);
        missing.retain(|row| {
            row.get("artistUri").and_then(Value::as_str) != Some(missing_uri.as_str())
        });
        assert!(
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(missing))
                .unwrap()
                .performer_names()
                .is_empty()
        );

        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &operation(valid_rows()))
                .unwrap();
        let projected = context
            .project_candidate_artists(&json!({
                "artists":["Irshad Kamil"],
                "artistCount":1
            }))
            .unwrap();
        assert_eq!(
            projected,
            vec![json!({"name":"Irshad Kamil","role":"lyricist"})]
        );
    }

    #[test]
    fn fresh_exact_catalog_sidecar_can_bind_unstructured_legacy_request_only_on_full_identity_match(
    ) {
        let verified = spotify_track();
        let mut legacy = verified.clone();
        legacy.provenance = json!({});
        let operation = operation(valid_rows());
        let context =
            SpotifyTrackRoleContext::from_operation_for_request(&legacy, &verified, &operation)
                .expect("fresh exact-ID metadata may bind an unchanged legacy request");
        assert!(context.is_for_track(&legacy));
        assert_eq!(
            context.performer_names(),
            ["Sunidhi Chauhan", "Divya Kumar"]
        );

        let mut changed_title = legacy.clone();
        changed_title.title.push_str(" (Live)");
        assert!(!context.is_for_track(&changed_title));
        assert!(SpotifyTrackRoleContext::from_operation_for_request(
            &changed_title,
            &verified,
            &operation
        )
        .is_none());

        let mut changed_duration = legacy.clone();
        changed_duration.duration_seconds = Some(250.0);
        assert!(SpotifyTrackRoleContext::from_operation_for_request(
            &changed_duration,
            &verified,
            &operation
        )
        .is_none());

        for rejected_saved_duration in [246.0, 248.0] {
            let mut changed_duration = legacy.clone();
            changed_duration.duration_seconds = Some(rejected_saved_duration);
            assert!(SpotifyTrackRoleContext::from_operation_for_request(
                &changed_duration,
                &verified,
                &operation
            )
            .is_none());
        }

        let mut precise_saved = verified.clone();
        precise_saved.duration_seconds = Some(247.381);
        let mut different_precise_catalog = verified.clone();
        different_precise_catalog.duration_seconds = Some(247.5);
        assert!(SpotifyTrackRoleContext::from_operation_for_request(
            &precise_saved,
            &different_precise_catalog,
            &operation
        )
        .is_none());
    }

    #[test]
    fn post_read_identifier_hydration_is_allowed_only_when_it_matches_bound_catalog_isrc() {
        let request = spotify_track();
        let mut verified = request.clone();
        verified.isrc = Some("USUM71801420".into());
        verified.provenance["remoteCatalog"]["isrc"] = json!("USUM71801420");
        verified.provenance["remoteCatalog"]["recordingContext"]["isrc"] = json!("USUM71801420");
        let operation = operation(valid_rows());
        let context =
            SpotifyTrackRoleContext::from_operation_for_request(&request, &verified, &operation)
                .expect("the provider sidecar binds to the unchanged request");
        let mut hydrated = request.clone();
        hydrated.isrc = Some("USUM71801420".into());
        assert!(context.is_for_track(&hydrated));
        hydrated.isrc = Some("USUM00000000".into());
        assert!(!context.is_for_track(&hydrated));
    }
}

#[cfg(test)]
mod captured_action_tests {
    use super::*;
    use crate::recording_match::{
        self, Comparison, FlexibleMatchingOptions, MatchAction, MatchLevel, MatchStage,
    };
    use serde::{de::DeserializeOwned, Serialize};

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/spotify-halka-role-context.json"
        ))
        .unwrap()
    }

    fn overlay<T: Default + Serialize + DeserializeOwned>(input: &Value) -> T {
        let mut value = serde_json::to_value(T::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(input.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn captured_halka_roles_affect_search_and_resolved_facts_without_claiming_completeness() {
        let fixture = fixture();
        let track: Track = overlay(&fixture["freshRequestProjection"]);
        let candidate: crate::models::Candidate = overlay(&fixture["candidate"]);
        let raw_candidate = candidate.clone();
        let context =
            SpotifyTrackRoleContext::from_operation_for_track(&track, &fixture["operation"])
                .unwrap();
        let projected_artists = context
            .project_candidate_artists(candidate.source_metadata.as_ref().unwrap())
            .unwrap();
        let performers = context.performer_names();
        assert_eq!(performers, ["Sunidhi Chauhan", "Divya Kumar"]);
        let mut projected = candidate.clone();
        projected.source_metadata.as_mut().unwrap()["artists"] = json!(projected_artists);

        for stage in [MatchStage::Search, MatchStage::Resolved] {
            for level in [
                MatchLevel::Exact,
                MatchLevel::Balanced,
                MatchLevel::Flexible,
            ] {
                let assessment = recording_match::assess_with_primary_credit_override_and_options(
                    &track,
                    &projected,
                    level,
                    stage,
                    &performers,
                    &FlexibleMatchingOptions::default(),
                );
                assert_eq!(assessment.dimensions.artists, Comparison::Agree);
                assert_eq!(assessment.dimensions.title, Comparison::Agree);
                assert_eq!(assessment.dimensions.runtime, Comparison::Agree);
                let evidence = assessment.evidence.as_ref().unwrap();
                assert!(!evidence.complete_performer_credits);
                assert!(!evidence.requested_performer_credits_complete);
                if stage == MatchStage::Resolved && level != MatchLevel::Exact {
                    assert_eq!(assessment.action, MatchAction::Accept);
                }
                if level == MatchLevel::Exact {
                    assert_ne!(assessment.action, MatchAction::Accept);
                }
            }
        }
        assert_eq!(
            serde_json::to_value(&candidate).unwrap(),
            serde_json::to_value(&raw_candidate).unwrap(),
            "raw source metadata must remain unchanged"
        );
        let legacy: Track = overlay(&fixture["legacyRequest"]);
        assert!(
            SpotifyTrackRoleContext::from_operation_for_track(&legacy, &fixture["operation"])
                .is_none()
        );
        assert!(SpotifyTrackRoleContext::from_operation_for_request(
            &legacy,
            &track,
            &fixture["operation"]
        )
        .is_some());
    }

    #[test]
    fn captured_role_projection_keeps_wrong_artist_version_and_isrc_conflicts() {
        let fixture = fixture();
        for kind in ["artist", "version", "isrc"] {
            let mut track: Track = overlay(&fixture["freshRequestProjection"]);
            let mut candidate: crate::models::Candidate = overlay(&fixture["candidate"]);
            if kind == "isrc" {
                track.isrc = Some("INS181801579".into());
            }
            let context =
                SpotifyTrackRoleContext::from_operation_for_track(&track, &fixture["operation"])
                    .unwrap();
            let metadata = candidate.source_metadata.as_mut().unwrap();
            match kind {
                "artist" => {
                    metadata["artists"] = json!(["Wrong Singer"]);
                    metadata["artist"] = json!("Wrong Singer");
                }
                "version" => {
                    metadata["track"] = json!("Halka Halka (Live)");
                    candidate.title = "Halka Halka (Live)".into();
                }
                "isrc" => metadata["isrc"] = json!("USZZZ2600001"),
                _ => unreachable!(),
            }
            if let Some(projected) = context.project_candidate_artists(metadata) {
                metadata["artists"] = json!(projected);
            }
            for stage in [MatchStage::Search, MatchStage::Resolved] {
                for level in [
                    MatchLevel::Exact,
                    MatchLevel::Balanced,
                    MatchLevel::Flexible,
                ] {
                    let assessment =
                        recording_match::assess_with_primary_credit_override_and_options(
                            &track,
                            &candidate,
                            level,
                            stage,
                            &context.performer_names(),
                            &FlexibleMatchingOptions::default(),
                        );
                    assert_ne!(
                        assessment.action,
                        MatchAction::Accept,
                        "{kind}: {assessment:#?}"
                    );
                }
            }
        }
    }
}
