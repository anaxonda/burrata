//! Private, pure-data prototype for binding a Spotify album occurrence to an
//! exact MusicBrainz release occurrence. No network, cache, or Track mutation.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedArtistCredit {
    pub artist_id: String,
    pub name: String,
    pub joinphrase: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseOccurrence {
    pub spotify_album_id: String,
    pub spotify_track_id: String,
    pub spotify_position: u32,
    pub spotify_title: String,
    pub spotify_artist: String,
    pub spotify_duration_ms: Option<u64>,
    pub musicbrainz_release_id: String,
    pub musicbrainz_recording_id: String,
    pub musicbrainz_track_id: String,
    pub medium_position: u32,
    pub medium_track_position: u32,
    pub flattened_release_position: u32,
    pub provider_artist_credit_matches: bool,
    pub musicbrainz_title: String,
    pub musicbrainz_artist: String,
    pub musicbrainz_artist_credits: Vec<VerifiedArtistCredit>,
    pub musicbrainz_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrosswalkError {
    InvalidAlbum,
    IncompleteSpotifyAlbum,
    TargetOccurrenceMissing,
    InvalidUrlRelation,
    AmbiguousReleaseEdition,
    ReleaseIdentityMismatch,
    IncompleteMusicBrainzRelease,
    PositionMismatch,
    MetadataConflict,
}

/// Convert MusicBrainz's exact `/url?resource=...&inc=release-rels` response into
/// the resolver's narrow relation DTO. Missing target identity fails closed.
pub fn normalize_url_relations(raw: &Value) -> Result<Value, CrosswalkError> {
    let resource = raw["resource"]
        .as_str()
        .ok_or(CrosswalkError::InvalidUrlRelation)?;
    let relations = raw["relations"]
        .as_array()
        .ok_or(CrosswalkError::InvalidUrlRelation)?;
    let mut normalized = Vec::with_capacity(relations.len());
    for relation in relations {
        let target_type = relation["target-type"]
            .as_str()
            .ok_or(CrosswalkError::InvalidUrlRelation)?;
        let ended = relation["ended"]
            .as_bool()
            .ok_or(CrosswalkError::InvalidUrlRelation)?;
        let release_id = if target_type == "release" {
            let id = relation["release"]["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .ok_or(CrosswalkError::InvalidUrlRelation)?;
            Some(id)
        } else {
            None
        };
        normalized.push(json!({"targetType":target_type,"ended":ended,"releaseId":release_id}));
    }
    Ok(json!({"resource":resource,"relations":normalized}))
}

/// Normalize the official MB release lookup payload without dropping position,
/// recording identity, duration, or track/recording artist credits.
pub fn normalize_musicbrainz_release(raw: &Value) -> Result<Value, CrosswalkError> {
    let id = raw["id"]
        .as_str()
        .filter(|id| valid_mbid(id))
        .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
    let media = raw["media"]
        .as_array()
        .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
    let mut normalized_media = Vec::with_capacity(media.len());
    for medium in media {
        let tracks = medium["tracks"]
            .as_array()
            .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
        let mut normalized_tracks = Vec::with_capacity(tracks.len());
        for track in tracks {
            let recording = &track["recording"];
            let track_id = track["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            let recording_id = recording["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            let title = track["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            let artist_credit = if recording["artist-credit"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
            {
                &recording["artist-credit"]
            } else {
                &track["artist-credit"]
            };
            let mut artist = String::new();
            if let Some(credits) = artist_credit.as_array() {
                for credit in credits {
                    artist.push_str(
                        credit["name"]
                            .as_str()
                            .or_else(|| credit["artist"]["name"].as_str())
                            .unwrap_or_default(),
                    );
                    artist.push_str(credit["joinphrase"].as_str().unwrap_or_default());
                }
            }
            let credits = artist_credit
                .as_array()
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            if credits.is_empty() || credits.len() > 32 {
                return Err(CrosswalkError::IncompleteMusicBrainzRelease);
            }
            let credits = credits
                .iter()
                .map(|credit| {
                    let id = credit["artist"]["id"]
                        .as_str()
                        .filter(|id| valid_mbid(id))
                        .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
                    let name = credit["name"]
                        .as_str()
                        .or_else(|| credit["artist"]["name"].as_str())
                        .filter(|name| !name.trim().is_empty() && name.len() <= 512)
                        .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
                    let joinphrase = credit["joinphrase"].as_str().unwrap_or_default();
                    if joinphrase.len() > 128 {
                        return Err(CrosswalkError::IncompleteMusicBrainzRelease);
                    }
                    Ok(VerifiedArtistCredit {
                        artist_id: id.to_owned(),
                        name: name.to_owned(),
                        joinphrase: joinphrase.to_owned(),
                    })
                })
                .collect::<Result<Vec<_>, CrosswalkError>>()?;
            normalized_tracks.push(json!({
                "position":track["position"], "trackId":track_id, "recordingId":recording_id,
                "title":title, "artist":artist.trim(), "artistCredits":credits,
                "durationMs":recording["length"].as_u64().or_else(||track["length"].as_u64()),
                "version":Value::Null
            }));
        }
        normalized_media.push(json!({
            "position":medium["position"], "track-count":medium["track-count"], "tracks":normalized_tracks
        }));
    }
    Ok(json!({"id":id,"media":normalized_media}))
}

/// Extract exactly one active release identity from normalized URL relations.
pub fn exact_release_id(
    url_relations: &Value,
    spotify_album_id: &str,
) -> Result<String, CrosswalkError> {
    if !valid_spotify_id(spotify_album_id) {
        return Err(CrosswalkError::InvalidAlbum);
    }
    let canonical_url = format!("https://open.spotify.com/album/{spotify_album_id}");
    if url_relations["resource"].as_str() != Some(canonical_url.as_str()) {
        return Err(CrosswalkError::InvalidUrlRelation);
    }
    let relations = url_relations["relations"]
        .as_array()
        .ok_or(CrosswalkError::InvalidUrlRelation)?;
    let release_ids: HashSet<&str> = relations
        .iter()
        .filter_map(|relation| {
            (relation["targetType"].as_str() == Some("release")
                && relation["ended"].as_bool() == Some(false))
            .then(|| relation["releaseId"].as_str())
            .flatten()
            .filter(|id| valid_mbid(id))
        })
        .collect();
    if release_ids.is_empty() {
        return Err(CrosswalkError::InvalidUrlRelation);
    }
    if release_ids.len() != 1 {
        return Err(CrosswalkError::AmbiguousReleaseEdition);
    }
    Ok((*release_ids.iter().next().unwrap()).to_owned())
}

/// Number of album occurrences for a provider track ID in a validated snapshot.
pub fn spotify_track_occurrence_count(spotify_album: &Value, track_id: &str) -> usize {
    spotify_album["tracks"]
        .as_array()
        .map(|tracks| {
            tracks
                .iter()
                .filter(|track| track["trackId"].as_str() == Some(track_id))
                .count()
        })
        .unwrap_or(0)
}

/// Validate the caller's complete Spotify album context before any catalog HTTP
/// request. `position` is one-based album track order, never artist-row order.
pub fn validate_spotify_target(
    spotify_album: &Value,
    target_track_id: &str,
    target_position: u32,
) -> Result<(), CrosswalkError> {
    let album_id = spotify_album["albumId"]
        .as_str()
        .ok_or(CrosswalkError::InvalidAlbum)?;
    let canonical_album_url = format!("https://open.spotify.com/album/{album_id}");
    if !valid_spotify_id(album_id)
        || spotify_album["albumUrl"].as_str() != Some(canonical_album_url.as_str())
    {
        return Err(CrosswalkError::InvalidAlbum);
    }
    let tracks = spotify_album["tracks"]
        .as_array()
        .ok_or(CrosswalkError::IncompleteSpotifyAlbum)?;
    if spotify_album["complete"].as_bool() != Some(true)
        || spotify_album["total"].as_u64() != Some(tracks.len() as u64)
        || tracks.is_empty()
    {
        return Err(CrosswalkError::IncompleteSpotifyAlbum);
    }
    let mut seen = HashSet::new();
    for (index, track) in tracks.iter().enumerate() {
        let position = track["position"]
            .as_u64()
            .ok_or(CrosswalkError::IncompleteSpotifyAlbum)?;
        if position != (index + 1) as u64
            || !seen.insert(position)
            || track["trackId"].as_str().is_none_or(str::is_empty)
            || track["title"].as_str().is_none_or(str::is_empty)
        {
            return Err(CrosswalkError::IncompleteSpotifyAlbum);
        }
    }
    let target_index = usize::try_from(
        target_position
            .checked_sub(1)
            .ok_or(CrosswalkError::TargetOccurrenceMissing)?,
    )
    .map_err(|_| CrosswalkError::TargetOccurrenceMissing)?;
    if tracks
        .get(target_index)
        .and_then(|track| track["trackId"].as_str())
        != Some(target_track_id)
    {
        return Err(CrosswalkError::TargetOccurrenceMissing);
    }
    Ok(())
}

/// Resolve only by exact Spotify album URL relation and same complete-album
/// ordinal. Text/duration are consistency checks after that identity anchor;
/// they are never used to search for or select a different occurrence.
pub fn resolve_occurrence(
    spotify_album: &Value,
    target_track_id: &str,
    target_position: u32,
    url_relations: &Value,
    musicbrainz_release: &Value,
) -> Result<ReleaseOccurrence, CrosswalkError> {
    validate_spotify_target(spotify_album, target_track_id, target_position)?;
    let album_id = spotify_album["albumId"]
        .as_str()
        .ok_or(CrosswalkError::InvalidAlbum)?;
    let spotify_tracks = spotify_album["tracks"]
        .as_array()
        .ok_or(CrosswalkError::IncompleteSpotifyAlbum)?;
    let target_index = usize::try_from(
        target_position
            .checked_sub(1)
            .ok_or(CrosswalkError::TargetOccurrenceMissing)?,
    )
    .map_err(|_| CrosswalkError::TargetOccurrenceMissing)?;
    let spotify_target = spotify_tracks
        .get(target_index)
        .ok_or(CrosswalkError::TargetOccurrenceMissing)?;
    if spotify_target["trackId"].as_str() != Some(target_track_id) {
        return Err(CrosswalkError::TargetOccurrenceMissing);
    }

    let release_id = exact_release_id(url_relations, album_id)?;
    if musicbrainz_release["id"].as_str() != Some(release_id.as_str()) {
        return Err(CrosswalkError::ReleaseIdentityMismatch);
    }

    let media = musicbrainz_release["media"]
        .as_array()
        .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
    if media.is_empty() {
        return Err(CrosswalkError::IncompleteMusicBrainzRelease);
    }
    let mut ordered = Vec::new();
    let mut expected_medium = 1u64;
    for medium in media {
        let medium_position = medium["position"]
            .as_u64()
            .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
        if medium_position != expected_medium {
            return Err(CrosswalkError::IncompleteMusicBrainzRelease);
        }
        expected_medium += 1;
        let tracks = medium["tracks"]
            .as_array()
            .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
        if medium["track-count"].as_u64() != Some(tracks.len() as u64) {
            return Err(CrosswalkError::IncompleteMusicBrainzRelease);
        }
        if tracks.is_empty() {
            return Err(CrosswalkError::IncompleteMusicBrainzRelease);
        }
        for (index, track) in tracks.iter().enumerate() {
            let position = track["position"]
                .as_u64()
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            let track_id = track["trackId"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            let recording_id = track["recordingId"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .ok_or(CrosswalkError::IncompleteMusicBrainzRelease)?;
            if position != (index + 1) as u64 || track["title"].as_str().is_none_or(str::is_empty) {
                return Err(CrosswalkError::IncompleteMusicBrainzRelease);
            }
            ordered.push((medium_position, position, track_id, recording_id, track));
        }
    }
    let mb_target = ordered
        .get(target_index)
        .ok_or(CrosswalkError::PositionMismatch)?;
    let (medium, medium_position, mb_track_id, recording_id, mb_track) = *mb_target;
    // Metadata only rejects a source-anchored positional pair; it never picks a
    // different row. Parenthetical translations are allowed after the primary title.
    let spotify_title = primary_title(spotify_target["title"].as_str().unwrap_or_default());
    let mb_title = primary_title(mb_track["title"].as_str().unwrap_or_default());
    if normalized(&spotify_title) != normalized(&mb_title)
        || version_conflict(spotify_target, mb_track)
    {
        return Err(CrosswalkError::MetadataConflict);
    }
    let spotify_artist = normalized(spotify_target["artist"].as_str().unwrap_or_default());
    let mb_artist = normalized(mb_track["artist"].as_str().unwrap_or_default());
    // Artist display credits are retained as a comparison signal, not identity:
    // provider/catalog aliases commonly differ even when the exact edition and
    // ordinal agree. Never reject or select by this text.
    let provider_artist_credit_matches =
        !spotify_artist.is_empty() && !mb_artist.is_empty() && spotify_artist == mb_artist;
    if let (Some(spotify_ms), Some(mb_ms)) = (
        spotify_target["durationMs"].as_u64(),
        mb_track["durationMs"].as_u64(),
    ) {
        if spotify_ms.abs_diff(mb_ms) > 5_000 {
            return Err(CrosswalkError::MetadataConflict);
        }
    }

    Ok(ReleaseOccurrence {
        spotify_album_id: album_id.to_owned(),
        spotify_track_id: target_track_id.to_owned(),
        spotify_position: target_position,
        spotify_title: spotify_target["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        spotify_artist: spotify_target["artist"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        spotify_duration_ms: spotify_target["durationMs"].as_u64(),
        musicbrainz_release_id: release_id.clone(),
        musicbrainz_recording_id: recording_id.to_owned(),
        musicbrainz_track_id: mb_track_id.to_owned(),
        medium_position: u32::try_from(medium)
            .map_err(|_| CrosswalkError::IncompleteMusicBrainzRelease)?,
        medium_track_position: u32::try_from(medium_position)
            .map_err(|_| CrosswalkError::IncompleteMusicBrainzRelease)?,
        flattened_release_position: target_position,
        provider_artist_credit_matches,
        musicbrainz_title: mb_track["title"].as_str().unwrap_or_default().to_owned(),
        musicbrainz_artist: mb_track["artist"].as_str().unwrap_or_default().to_owned(),
        musicbrainz_duration_ms: mb_track["durationMs"].as_u64(),
        musicbrainz_artist_credits: serde_json::from_value(mb_track["artistCredits"].clone())
            .map_err(|_| CrosswalkError::IncompleteMusicBrainzRelease)?,
    })
}

fn primary_title(title: &str) -> String {
    title
        .split_once(" (")
        .map(|(primary, _)| primary)
        .unwrap_or(title)
        .trim()
        .to_owned()
}
fn version_conflict(spotify: &Value, musicbrainz: &Value) -> bool {
    let spotify_version = spotify["version"]
        .as_str()
        .map(normalized)
        .filter(|value| !value.is_empty());
    let mb_version = musicbrainz["version"]
        .as_str()
        .map(normalized)
        .filter(|value| !value.is_empty());
    match (spotify_version, mb_version) {
        (Some(left), Some(right)) if left == right => {}
        (None, None) => {}
        _ => return true,
    }
    let spotify_title = normalized(spotify["title"].as_str().unwrap_or_default());
    let mb_title = normalized(musicbrainz["title"].as_str().unwrap_or_default());
    const VERSION_MARKERS: [&str; 8] = [
        "remix",
        "live",
        "acoustic",
        "instrumental",
        "remaster",
        "edit",
        "radio",
        "version",
    ];
    VERSION_MARKERS
        .iter()
        .any(|marker| spotify_title.contains(marker) != mb_title.contains(marker))
}
fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
pub fn valid_spotify_id(value: &str) -> bool {
    (22..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
pub fn valid_mbid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn read_fixture(path: &str) -> Value {
        let text = match path {
            "album" => include_str!("../fixtures/spotify-album.json"),
            "relations" => include_str!("../fixtures/mb-url-relations.json"),
            "release" => include_str!("../fixtures/mb-release-public.json"),
            "wrong_target" => include_str!("../fixtures/spotify-unrelated-target.json"),
            _ => panic!("unknown fixture"),
        };
        let value: Value = serde_json::from_str(text).unwrap();
        match path {
            "relations" => normalize_url_relations(&value).unwrap(),
            "release" => normalize_musicbrainz_release(&value).unwrap(),
            _ => value,
        }
    }

    #[test]
    fn incomplete_or_oversized_artist_credit_fails_closed() {
        let raw: Value =
            serde_json::from_str(include_str!("../fixtures/mb-release-public.json")).unwrap();
        let mut invalid = raw.clone();
        invalid["media"][0]["tracks"][0]["recording"]["artist-credit"][0]["artist"]["id"] =
            json!("invalid");
        assert_eq!(
            normalize_musicbrainz_release(&invalid),
            Err(CrosswalkError::IncompleteMusicBrainzRelease)
        );
        let mut oversized = raw;
        let credit = oversized["media"][0]["tracks"][0]["recording"]["artist-credit"][0].clone();
        oversized["media"][0]["tracks"][0]["recording"]["artist-credit"] = json!(vec![credit; 33]);
        assert_eq!(
            normalize_musicbrainz_release(&oversized),
            Err(CrosswalkError::IncompleteMusicBrainzRelease)
        );
    }

    #[test]
    fn exact_url_and_full_album_ordinal_resolve_the_occurrence() {
        let result = resolve_occurrence(
            &read_fixture("album"),
            "0j8ulfrtVtBnyBr6p0CfFu",
            1,
            &read_fixture("relations"),
            &read_fixture("release"),
        )
        .unwrap();
        assert_eq!(result.spotify_position, 1);
        assert_eq!(
            spotify_track_occurrence_count(&read_fixture("album"), "0j8ulfrtVtBnyBr6p0CfFu"),
            1
        );
        assert_eq!(
            result.musicbrainz_release_id,
            "b8254485-3b63-4e55-8776-7bd713fe3dd7"
        );
        assert_eq!(
            result.musicbrainz_recording_id,
            "5a1d6c4a-bd16-4d09-87da-6af8d4daf36c"
        );
        assert_eq!(result.medium_track_position, 1);
        assert_eq!(result.musicbrainz_artist_credits.len(), 1);
        assert_eq!(
            result.musicbrainz_artist_credits[0].artist_id,
            "80fb9c75-af15-4a6b-9528-715f9ef4ebea"
        );
        assert!(!result.provider_artist_credit_matches); // known provider/MB display-credit variation
    }

    #[test]
    fn same_title_from_unrelated_spotify_track_is_rejected_before_text_matching() {
        let wrong = read_fixture("wrong_target");
        assert_eq!(wrong["title"], "Kaval Sviri (The Flute Plays)");
        assert_eq!(
            resolve_occurrence(
                &read_fixture("album"),
                wrong["trackId"].as_str().unwrap(),
                1,
                &read_fixture("relations"),
                &read_fixture("release")
            ),
            Err(CrosswalkError::TargetOccurrenceMissing)
        );
    }

    #[test]
    fn incomplete_spotify_album_or_release_never_maps_a_shifted_ordinal() {
        let mut album = read_fixture("album");
        album["tracks"].as_array_mut().unwrap().pop();
        assert_eq!(
            resolve_occurrence(
                &album,
                "0j8ulfrtVtBnyBr6p0CfFu",
                1,
                &read_fixture("relations"),
                &read_fixture("release")
            ),
            Err(CrosswalkError::IncompleteSpotifyAlbum)
        );
        let mut release = read_fixture("release");
        release["media"][0]["tracks"].as_array_mut().unwrap().pop();
        assert_eq!(
            resolve_occurrence(
                &read_fixture("album"),
                "0j8ulfrtVtBnyBr6p0CfFu",
                1,
                &read_fixture("relations"),
                &release
            ),
            Err(CrosswalkError::IncompleteMusicBrainzRelease)
        );
    }

    #[test]
    fn wrong_resource_and_multiple_direct_editions_fail_closed() {
        let mut relations = read_fixture("relations");
        relations["resource"] = "https://open.spotify.com/album/another-album".into();
        assert_eq!(
            resolve_occurrence(
                &read_fixture("album"),
                "0j8ulfrtVtBnyBr6p0CfFu",
                1,
                &relations,
                &read_fixture("release")
            ),
            Err(CrosswalkError::InvalidUrlRelation)
        );
        let mut relations = read_fixture("relations");
        relations["relations"].as_array_mut().unwrap().push(serde_json::json!({
            "targetType":"release", "ended":false, "releaseId":"c8254485-3b63-4e55-8776-7bd713fe3dd7"
        }));
        assert_eq!(
            resolve_occurrence(
                &read_fixture("album"),
                "0j8ulfrtVtBnyBr6p0CfFu",
                1,
                &relations,
                &read_fixture("release")
            ),
            Err(CrosswalkError::AmbiguousReleaseEdition)
        );
    }

    #[test]
    fn metadata_conflict_rejects_the_anchored_position_without_searching_other_rows() {
        let mut release = read_fixture("release");
        release["media"][0]["tracks"][0]["title"] = "A Different Song".into();
        assert_eq!(
            resolve_occurrence(
                &read_fixture("album"),
                "0j8ulfrtVtBnyBr6p0CfFu",
                1,
                &read_fixture("relations"),
                &release
            ),
            Err(CrosswalkError::MetadataConflict)
        );
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn explicit_version_conflict_is_not_erased_by_parenthetical_title_matching() {
        let spotify = serde_json::json!({"title":"Kaval Sviri (Remix)","version":"Remix"});
        let mb = serde_json::json!({"title":"Kaval Sviri"});
        assert!(version_conflict(&spotify, &mb));
    }
}
