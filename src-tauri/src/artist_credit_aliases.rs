//! Exact-ID-scoped MusicBrainz artist alias context for playback search/matching.
//!
//! This module deliberately fails closed: caller-supplied Track provenance is
//! accepted only after its exact occurrence and full artist credits match the
//! raw cached MusicBrainz release response.
use crate::{models::Track, Store};
use serde_json::Value;

const RELEASE_CACHE: &str = "artists.release-tracks.musicbrainz";
const ARTIST_PROFILE_CACHE: &str = "artists.profile.musicbrainz";

/// Shared cache-only alias context for ordinary resolution and same-source recovery.
pub(crate) fn cached_verified_playback_artist_alias_tracks(
    store: &Store,
    track: &Track,
) -> Vec<Track> {
    let mut aliases = cached_verified_artist_alias_tracks(store, track);
    if track.provider == "spotify" {
        aliases.extend(cached_verified_spotify_recording_alias_tracks(store, track));
    }
    aliases.sort_by(|left, right| left.artist.cmp(&right.artist));
    aliases.dedup_by(|left, right| left.artist == right.artist);
    aliases
}

pub(crate) fn cached_verified_spotify_recording_alias_tracks(
    store: &Store,
    track: &Track,
) -> Vec<Track> {
    if track.provider != "spotify"
        || !valid_spotify_id(&track.provider_id)
        || track.id != format!("spotify:{}", track.provider_id)
        || track.source_url != format!("https://open.spotify.com/track/{}", track.provider_id)
        || track.version.is_some()
        || track.title.trim().is_empty()
        || track.artist.trim().is_empty()
        || track
            .duration_seconds
            .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0 || seconds > 86_400.0)
    {
        return Vec::new();
    }
    // Embedded provenance can outlive a provider album change or be edited with
    // the library row. Always bind it to the cache written by the exact provider
    // lookup and complete crosswalk verification; an absent/ambiguous cache must
    // not let an older album sidecar authorize aliases.
    let Some(context) =
        crate::remote_playlists::read_verified_spotify_track_context(store, &track.provider_id)
    else {
        return Vec::new();
    };
    if context.title != track.title || context.artist != track.artist {
        return Vec::new();
    }
    if let (Some(actual), Some(expected)) = (track.duration_seconds, context.duration_ms) {
        if !actual.is_finite()
            || actual <= 0.0
            || ((actual * 1000.0).round() as u64).abs_diff(expected) > 5_000
        {
            return Vec::new();
        }
    } else if context.duration_ms.is_some() != track.duration_seconds.is_some() {
        return Vec::new();
    }
    if let Some(embedded) = track.provenance.get("recordingContext") {
        let Some(id) = embedded.get("albumId").and_then(Value::as_str) else {
            return Vec::new();
        };
        if !valid_spotify_id(id)
            || id != context.album_id
            || embedded["albumUri"].as_str() != Some(format!("spotify:album:{id}").as_str())
        {
            return Vec::new();
        }
    }
    let album_id = context.album_id;
    let Some(occurrence) = crate::artists::read_verified_spotify_recording_sidecar(
        store,
        &album_id,
        &track.provider_id,
    )
    .ok()
    .flatten() else {
        return Vec::new();
    };
    if occurrence.spotify_album_id != album_id
        || occurrence.spotify_track_id != track.provider_id
        || occurrence.spotify_title != track.title
        || occurrence.spotify_artist != track.artist
        || occurrence
            .spotify_duration_ms
            .zip(
                track
                    .duration_seconds
                    .map(|seconds| (seconds * 1000.0).round() as u64),
            )
            .is_some_and(|(expected, actual)| actual.abs_diff(expected) > 5_000)
        || !valid_mbid(&occurrence.musicbrainz_release_id)
        || !valid_mbid(&occurrence.musicbrainz_recording_id)
        || !valid_mbid(&occurrence.musicbrainz_track_id)
    {
        return Vec::new();
    }
    let Ok(Some(release)) =
        store.get_provider_cache(RELEASE_CACHE, &occurrence.musicbrainz_release_id)
    else {
        return Vec::new();
    };
    if release["id"].as_str() != Some(occurrence.musicbrainz_release_id.as_str()) {
        return Vec::new();
    }
    let mut parsed = crate::artists::parse_musicbrainz_release_tracks(
        &release,
        &occurrence.musicbrainz_release_id,
        &format!(
            "https://musicbrainz.org/release/{}",
            occurrence.musicbrainz_release_id
        ),
        false,
        None,
    );
    let mut exact = parsed.drain(..).filter(|candidate| {
        candidate.provenance["recordingId"].as_str()
            == Some(occurrence.musicbrainz_recording_id.as_str())
            && candidate.provenance["trackGuid"].as_str()
                == Some(occurrence.musicbrainz_track_id.as_str())
            && candidate.provenance["mediumPosition"].as_u64()
                == Some(occurrence.medium_position as u64)
            && candidate.provenance["trackPosition"].as_str()
                == Some(occurrence.medium_track_position.to_string().as_str())
    });
    let Some(musicbrainz_track) = exact.next() else {
        return Vec::new();
    };
    if exact.next().is_some() {
        return Vec::new();
    }
    cached_verified_artist_alias_tracks(store, &musicbrainz_track)
        .into_iter()
        .filter_map(|alias| {
            if alias.artist == musicbrainz_track.artist {
                return None;
            }
            let mut replacement = track.clone();
            replacement.artist = alias.artist;
            Some(replacement)
        })
        .fold(Vec::<Track>::new(), |mut aliases, alias| {
            if !aliases.iter().any(|item| item.artist == alias.artist) {
                aliases.push(alias);
            }
            aliases
        })
}

fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(crate) fn cached_verified_artist_alias_tracks(store: &Store, track: &Track) -> Vec<Track> {
    let Some((release_id, recording_id, track_guid)) = verified_occurrence_ids(track) else {
        return Vec::new();
    };
    let Ok(Some(release)) = store.get_provider_cache(RELEASE_CACHE, release_id) else {
        return Vec::new();
    };
    let Some(raw) = exact_release_occurrence(&release, release_id, recording_id, track_guid) else {
        return Vec::new();
    };
    if !matches_cached_parser_occurrence(track, &release, recording_id, track_guid) {
        return Vec::new();
    }
    let Some(raw_credits) = occurrence_credits(raw, &release) else {
        return Vec::new();
    };
    if raw_credits.len() > 16
        || !track_matches_cached_occurrence(track, &release, raw, &raw_credits)
    {
        return Vec::new();
    }
    let mut variants = Vec::new();
    for (index, credit) in raw_credits.iter().enumerate() {
        let Some(mbid) = credit.artist_id.as_deref() else {
            continue;
        };
        let Ok(Some(profile)) = store.get_provider_cache(ARTIST_PROFILE_CACHE, mbid) else {
            continue;
        };
        if profile["id"].as_str() != Some(mbid) {
            continue;
        }
        let mut names = Vec::<String>::new();
        if let Some(name) = profile["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
        {
            names.push(name.to_owned());
        }
        names.extend(
            profile["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
                .filter_map(|alias| alias["name"].as_str())
                .filter(|name| !name.trim().is_empty() && name.len() <= 256)
                .map(str::to_owned),
        );
        let canonical = names.first().cloned();
        names = canonical
            .into_iter()
            .chain(names.into_iter().skip(1).collect::<Vec<_>>())
            .collect();
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| seen.insert(name.clone()));
        for name in names {
            if variants.len() >= 32 {
                return variants;
            }
            if name.len() > 256 || name == credit.name {
                continue;
            }
            let mut replacement = track.clone();
            replacement.artist = raw_credits
                .iter()
                .enumerate()
                .map(|(credit_index, current)| {
                    let current_name = if credit_index == index {
                        name.as_str()
                    } else {
                        current.name.as_str()
                    };
                    format!(
                        "{current_name}{}",
                        current.join_phrase.as_deref().unwrap_or_default()
                    )
                })
                .collect::<String>();
            if !variants
                .iter()
                .any(|variant: &Track| variant.artist == replacement.artist)
            {
                variants.push(replacement);
            }
        }
    }
    variants
}

fn valid_mbid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(parts).all(|(len, part)| {
            part.len() == *len && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn verified_occurrence_ids(track: &Track) -> Option<(&str, &str, &str)> {
    if track.provider != "catalog"
        || track.provenance["sourceKind"].as_str() != Some("catalog")
        || track.provenance["catalogProvider"].as_str() != Some("musicbrainz")
    {
        return None;
    }
    let release_id = track.provenance["releaseId"].as_str()?;
    let recording_id = track.provenance["recordingId"].as_str()?;
    let track_guid = track.provenance["trackGuid"].as_str()?;
    if !valid_mbid(release_id) || !valid_mbid(recording_id) || !valid_mbid(track_guid) {
        return None;
    }
    let expected_id = format!(
        "catalog:musicbrainz:{release_id}:{}:{}:{}:{recording_id}",
        track.provenance["mediumPosition"].as_u64()?,
        track.provenance["trackPosition"].as_str()?,
        track.provenance["occurrence"].as_u64()?
    );
    if track.id != expected_id || track.provider_id != expected_id.strip_prefix("catalog:")? {
        return None;
    }
    Some((release_id, recording_id, track_guid))
}

fn exact_release_occurrence<'a>(
    release: &'a Value,
    release_id: &str,
    recording_id: &str,
    track_guid: &str,
) -> Option<&'a Value> {
    if release["id"].as_str() != Some(release_id) {
        return None;
    }
    let mut matched = release["media"]
        .as_array()?
        .iter()
        .flat_map(|medium| medium["tracks"].as_array().into_iter().flatten())
        .filter(|row| {
            row["id"].as_str() == Some(track_guid)
                && row["recording"]["id"].as_str() == Some(recording_id)
        });
    let one = matched.next()?;
    matched.next().is_none().then_some(one)
}

fn matches_cached_parser_occurrence(
    track: &Track,
    release: &Value,
    recording_id: &str,
    track_guid: &str,
) -> bool {
    let Some(release_id) = release["id"].as_str() else {
        return false;
    };
    let parsed = crate::artists::parse_musicbrainz_release_tracks(
        release,
        release_id,
        &track.provenance["requestedReleaseSourceUrl"]
            .as_str()
            .unwrap_or_default(),
        track.provenance["representativeEdition"]
            .as_bool()
            .unwrap_or(false),
        None,
    );
    let mut matches = parsed.into_iter().filter(|candidate| {
        candidate.provenance["recordingId"].as_str() == Some(recording_id)
            && candidate.provenance["trackGuid"].as_str() == Some(track_guid)
    });
    let Some(expected) = matches.next() else {
        return false;
    };
    if matches.next().is_some() {
        return false;
    }
    expected.id == track.id
        && expected.provider == track.provider
        && expected.provider_id == track.provider_id
        && expected.source_url == track.source_url
        && expected.title == track.title
        && expected.artist == track.artist
        && expected.album == track.album
        && expected.duration_seconds == track.duration_seconds
        && expected.provenance["mediumPosition"] == track.provenance["mediumPosition"]
        && expected.provenance["trackPosition"] == track.provenance["trackPosition"]
        && expected.provenance["occurrence"] == track.provenance["occurrence"]
        && expected.provenance["artistCredits"] == track.provenance["artistCredits"]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawCredit {
    name: String,
    artist_id: Option<String>,
    credited_as: Option<String>,
    join_phrase: Option<String>,
}

fn credits(value: &Value) -> Vec<RawCredit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    item["artist"]["name"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                })?;
            Some(RawCredit {
                name: name.to_owned(),
                artist_id: item["artist"]["id"]
                    .as_str()
                    .filter(|id| valid_mbid(id))
                    .map(str::to_owned),
                credited_as: item["name"].as_str().map(str::to_owned),
                join_phrase: item["joinphrase"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

fn occurrence_credits<'a>(row: &'a Value, release: &'a Value) -> Option<Vec<RawCredit>> {
    let recording = credits(&row["recording"]["artist-credit"]);
    let track = credits(&row["artist-credit"]);
    let source = if !recording.is_empty() {
        &row["recording"]["artist-credit"]
    } else if !track.is_empty() {
        &row["artist-credit"]
    } else {
        &release["artist-credit"]
    };
    let parsed = credits(source);
    (!parsed.is_empty()).then_some(parsed)
}

fn track_matches_cached_occurrence(
    track: &Track,
    release: &Value,
    row: &Value,
    raw_credits: &[RawCredit],
) -> bool {
    let release_id = track.provenance["releaseId"].as_str().unwrap_or_default();
    if track.source_url != format!("https://musicbrainz.org/release/{release_id}")
        || track.provenance["sourceUrl"].as_str() != Some(track.source_url.as_str())
        || track.provenance["releaseId"].as_str() != release["id"].as_str()
        || track.album.as_deref() != release["title"].as_str()
        || row["title"].as_str() != Some(track.title.as_str())
    {
        return false;
    }
    let length_ms = row["recording"]["length"]
        .as_u64()
        .or_else(|| row["length"].as_u64());
    let Some(expected) = length_ms.map(|ms| ms as f64 / 1000.0) else {
        return false;
    };
    if track
        .duration_seconds
        .is_none_or(|value| !value.is_finite() || (value - expected).abs() > 0.000_001)
    {
        return false;
    }
    let provenance_credits = track.provenance["artistCredits"].as_array();
    let Some(provenance_credits) = provenance_credits else {
        return false;
    };
    if provenance_credits.len() != raw_credits.len() {
        return false;
    }
    for (actual, raw) in provenance_credits.iter().zip(raw_credits) {
        if actual["name"].as_str() != Some(raw.name.as_str())
            || actual["creditedAs"].as_str() != raw.credited_as.as_deref()
            || actual["joinPhrase"].as_str() != raw.join_phrase.as_deref()
            || actual["role"].as_str() != Some("artist credit")
            || actual["artist"]["provider"].as_str() != Some("musicbrainz")
            || actual["artist"]["id"].as_str() != raw.artist_id.as_deref()
        {
            return false;
        }
    }
    let rendered = raw_credits
        .iter()
        .map(|credit| {
            format!(
                "{}{}",
                credit.name,
                credit.join_phrase.as_deref().unwrap_or_default()
            )
        })
        .collect::<String>();
    track.artist == rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn fixture_track() -> Track {
        Track {
            id: "catalog:musicbrainz:4f7f6950-29cc-448d-a25b-445e383612f1:1:2:1:d0b8d1b1-05c3-4422-ae75-a5aae859d8dd".into(),
            provider: "catalog".into(), provider_id: "musicbrainz:4f7f6950-29cc-448d-a25b-445e383612f1:1:2:1:d0b8d1b1-05c3-4422-ae75-a5aae859d8dd".into(), source_url: "https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1".into(),
            artist: "Le Mystère Des Voix Bulgares".into(), title: "Duda E Bolna Legnada (Duda Lays Sick)".into(),
            album: Some("A Portrait of Nikolai Kaufman".into()), duration_seconds: Some(162.32),
            provenance: json!({"sourceKind":"catalog","catalogProvider":"musicbrainz","sourceUrl":"https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1","releaseId":"4f7f6950-29cc-448d-a25b-445e383612f1","mediumPosition":1,"trackPosition":"2","occurrence":1,"recordingId":"d0b8d1b1-05c3-4422-ae75-a5aae859d8dd","trackGuid":"dc536c4a-0300-407f-b89f-50b6da98045d","artistCredits":[{"name":"Le Mystère Des Voix Bulgares","artist":{"provider":"musicbrainz","id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea"},"role":"artist credit","creditedAs":"Le Mystère Des Voix Bulgares","joinPhrase":"","sourceUrl":"https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1"}]}),
            ..Track::default()
        }
    }
    fn release_fixture() -> Value {
        json!({"id":"4f7f6950-29cc-448d-a25b-445e383612f1","title":"A Portrait of Nikolai Kaufman","artist-credit":[{"name":"Le Mystère Des Voix Bulgares","joinphrase":"","artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea","name":"Bulgarian State Radio & Television Female Vocal Choir"}}],"media":[{"position":1,"tracks":[{"id":"dc536c4a-0300-407f-b89f-50b6da98045d","number":"2","position":2,"title":"Duda E Bolna Legnada (Duda Lays Sick)","artist-credit":[{"name":"Le Mystère Des Voix Bulgares","joinphrase":"","artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea","name":"Bulgarian State Radio & Television Female Vocal Choir"}}],"recording":{"id":"d0b8d1b1-05c3-4422-ae75-a5aae859d8dd","title":"Duda E Bolna Legnada (Duda Lays Sick)","length":162320,"artist-credit":[{"name":"Le Mystère Des Voix Bulgares","joinphrase":"","artist":{"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea","name":"Bulgarian State Radio & Television Female Vocal Choir"}}]}}]}]})
    }
    fn cached_store(release: &Value, profile: Option<&Value>) -> (tempfile::TempDir, Store) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .set_provider_cache(
                RELEASE_CACHE,
                "4f7f6950-29cc-448d-a25b-445e383612f1",
                release,
                3600,
            )
            .unwrap();
        if let Some(profile) = profile {
            store
                .set_provider_cache(
                    ARTIST_PROFILE_CACHE,
                    "80fb9c75-af15-4a6b-9528-715f9ef4ebea",
                    profile,
                    3600,
                )
                .unwrap();
        }
        (dir, store)
    }
    fn profile() -> Value {
        json!({"id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea","name":"Bulgarian State Radio & Television Female Vocal Choir","aliases":[{"name":"Le Mystère Des Voix Bulgares"},{"name":"The Bulgarian State Television and Radio Female Choir"}]})
    }

    #[test]
    fn actual_cached_release_parser_round_trips_exact_occurrence_and_rejects_forged_coordinates() {
        let release: Value = serde_json::from_str(include_str!(
            "../fixtures/musicbrainz-release-4f7f6950.json"
        ))
        .unwrap();
        let mut parsed = crate::artists::parse_musicbrainz_release_tracks(
            &release,
            "4f7f6950-29cc-448d-a25b-445e383612f1",
            "https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1",
            false,
            None,
        );
        let track = parsed
            .drain(..)
            .find(|track| {
                track.provenance["trackGuid"].as_str()
                    == Some("dc536c4a-0300-407f-b89f-50b6da98045d")
            })
            .unwrap();
        let (_directory, store) = cached_store(&release, Some(&profile()));
        assert!(!cached_verified_artist_alias_tracks(&store, &track).is_empty());

        let mut forged = track.clone();
        forged.provenance["trackPosition"] = json!("9");
        forged.id = format!(
            "catalog:musicbrainz:{}:{}:{}:{}:{}",
            forged.provenance["releaseId"].as_str().unwrap(),
            forged.provenance["mediumPosition"].as_u64().unwrap(),
            forged.provenance["trackPosition"].as_str().unwrap(),
            forged.provenance["occurrence"].as_u64().unwrap(),
            forged.provenance["recordingId"].as_str().unwrap(),
        );
        forged.provider_id = forged.id.strip_prefix("catalog:").unwrap().to_owned();
        assert!(cached_verified_artist_alias_tracks(&store, &forged).is_empty());
    }

    #[test]
    fn exact_release_occurrence_and_credit_cache_are_required_before_alias_expansion() {
        let release = release_fixture();
        let (_directory, store) = cached_store(&release, Some(&profile()));
        let track = fixture_track();
        let variants = cached_verified_artist_alias_tracks(&store, &track);
        assert!(variants
            .iter()
            .any(|v| v.artist == "Bulgarian State Radio & Television Female Vocal Choir"));
        assert!(!variants
            .iter()
            .any(|v| v.artist == "Bulgarian State Television Female Choir"));

        let mut forged = track.clone();
        forged.artist = "Unrelated Artist".into();
        forged.title = "Unrelated Recording".into();
        assert!(cached_verified_artist_alias_tracks(&store, &forged).is_empty());
        let mut wrong_mbid = track.clone();
        wrong_mbid.provenance["artistCredits"][0]["artist"]["id"] =
            json!("00000000-0000-4000-8000-000000000001");
        assert!(cached_verified_artist_alias_tracks(&store, &wrong_mbid).is_empty());
        let mut wrong_occurrence = track.clone();
        wrong_occurrence.provenance["recordingId"] = json!("00000000-0000-4000-8000-000000000001");
        assert!(cached_verified_artist_alias_tracks(&store, &wrong_occurrence).is_empty());

        for (field, value) in [
            ("mediumPosition", json!(2)),
            ("trackPosition", json!("9")),
            ("occurrence", json!(2)),
        ] {
            let mut forged_coordinate = track.clone();
            forged_coordinate.provenance[field] = value;
            assert!(
                cached_verified_artist_alias_tracks(&store, &forged_coordinate).is_empty(),
                "forged {field} must not authorize aliases"
            );
        }
    }

    #[test]
    fn multi_artist_credit_aliases_substitute_only_the_attached_ordered_member() {
        let mut release = release_fixture();
        let credit = |name: &str, join: &str, id: &str, canonical: &str| json!({"name":name,"joinphrase":join,"artist":{"id":id,"name":canonical}});
        let first = credit(
            "Le Mystère Des Voix Bulgares",
            " / ",
            "80fb9c75-af15-4a6b-9528-715f9ef4ebea",
            "Bulgarian State Radio & Television Female Vocal Choir",
        );
        let second_id = "00000000-0000-4000-8000-000000000001";
        let second = credit("Second Choir", "", second_id, "Canonical Second Choir");
        release["media"][0]["tracks"][0]["artist-credit"] = json!([first.clone(), second.clone()]);
        release["media"][0]["tracks"][0]["recording"]["artist-credit"] =
            json!([first.clone(), second.clone()]);
        let (_directory, store) = cached_store(&release, Some(&profile()));
        store.set_provider_cache(ARTIST_PROFILE_CACHE, second_id, &json!({"id":second_id,"name":"Canonical Second Choir","aliases":[{"name":"Second Choir Alias"}]}), 3600).unwrap();
        let mut track = fixture_track();
        track.artist = "Le Mystère Des Voix Bulgares / Second Choir".into();
        track.provenance["trackGuid"] = json!("dc536c4a-0300-407f-b89f-50b6da98045d");
        track.provenance["artistCredits"] = json!([
            {"name":"Le Mystère Des Voix Bulgares","artist":{"provider":"musicbrainz","id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea"},"role":"artist credit","creditedAs":"Le Mystère Des Voix Bulgares","joinPhrase":" / ","sourceUrl":"https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1"},
            {"name":"Second Choir","artist":{"provider":"musicbrainz","id":second_id},"role":"artist credit","creditedAs":"Second Choir","joinPhrase":"","sourceUrl":"https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1"}
        ]);
        let variants = cached_verified_artist_alias_tracks(&store, &track);
        assert!(variants.iter().any(|variant| variant.artist
            == "Bulgarian State Radio & Television Female Vocal Choir / Second Choir"));
        assert!(variants.iter().any(
            |variant| variant.artist == "Le Mystère Des Voix Bulgares / Canonical Second Choir"
        ));
        assert!(variants
            .iter()
            .all(|variant| !variant.artist.starts_with("Second Choir /")));
        assert!(variants
            .iter()
            .all(|variant| !variant.artist.ends_with(" /")));
        assert!(variants
            .iter()
            .all(|variant| variant.artist.split(" / ").count() == 2));
    }

    #[test]
    fn missing_profile_or_release_cache_fails_closed() {
        let release = release_fixture();
        let (_directory, store) = cached_store(&release, None);
        assert!(cached_verified_artist_alias_tracks(&store, &fixture_track()).is_empty());
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("empty.sqlite")).unwrap();
        assert!(cached_verified_artist_alias_tracks(&store, &fixture_track()).is_empty());
    }
    fn spotify_context_fixture_track() -> Track {
        Track {
            id: "spotify:bbbbbbbbbbbbbbbbbbbbbb".into(),
            provider: "spotify".into(),
            provider_id: "bbbbbbbbbbbbbbbbbbbbbb".into(),
            source_url: "https://open.spotify.com/track/bbbbbbbbbbbbbbbbbbbbbb".into(),
            title: "Duda E Bolna Legnada (Duda Lays Sick)".into(),
            artist: "Ensemble Trakia Choir".into(),
            album: Some("Exact Spotify album".into()),
            duration_seconds: Some(162.32),
            provenance: json!({"recordingContext":{"albumId":"aaaaaaaaaaaaaaaaaaaaaa","albumUri":"spotify:album:aaaaaaaaaaaaaaaaaaaaaa"}}),
            ..Track::default()
        }
    }

    fn spotify_occurrence_fixture() -> crate::artists::ReleaseOccurrence {
        crate::artists::ReleaseOccurrence {
            spotify_album_id: "aaaaaaaaaaaaaaaaaaaaaa".into(),
            spotify_track_id: "bbbbbbbbbbbbbbbbbbbbbb".into(),
            spotify_position: 2,
            spotify_title: "Duda E Bolna Legnada (Duda Lays Sick)".into(),
            spotify_artist: "Ensemble Trakia Choir".into(),
            spotify_duration_ms: Some(162_320),
            musicbrainz_release_id: "4f7f6950-29cc-448d-a25b-445e383612f1".into(),
            musicbrainz_recording_id: "d0b8d1b1-05c3-4422-ae75-a5aae859d8dd".into(),
            musicbrainz_track_id: "dc536c4a-0300-407f-b89f-50b6da98045d".into(),
            medium_position: 1,
            medium_track_position: 2,
            flattened_release_position: 2,
            provider_artist_credit_matches: false,
            musicbrainz_title: "Duda E Bolna Legnada (Duda Lays Sick)".into(),
            musicbrainz_artist: "Le Mystère Des Voix Bulgares".into(),
            musicbrainz_artist_credits: vec![crate::spotify_mb_crosswalk::VerifiedArtistCredit {
                artist_id: "80fb9c75-af15-4a6b-9528-715f9ef4ebea".into(),
                name: "Le Mystère Des Voix Bulgares".into(),
                joinphrase: String::new(),
            }],
            musicbrainz_duration_ms: Some(162_320),
        }
    }

    fn cache_verified_spotify_track_context(store: &Store, track: &Track, album_id: &str) {
        store
            .set_provider_cache(
                "artists.spotify-track-context.verified-v1",
                &track.provider_id,
                &json!({
                    "version": 1,
                    "status": "verified",
                    "trackId": track.provider_id,
                    "albumId": album_id,
                    "title": track.title,
                    "artist": track.artist,
                    "durationMs": 162320,
                    "accountIdHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }),
                3600,
            )
            .unwrap();
    }

    #[test]
    fn spotify_crosswalk_alias_requires_exact_cache_mapping_and_preserves_source_track() {
        let release = release_fixture();
        let (_directory, store) = cached_store(&release, Some(&profile()));
        crate::artists::write_verified_spotify_recording_sidecar(
            &store,
            &spotify_occurrence_fixture(),
            1,
        )
        .unwrap();
        let source = spotify_context_fixture_track();
        cache_verified_spotify_track_context(&store, &source, "aaaaaaaaaaaaaaaaaaaaaa");
        let aliases = cached_verified_spotify_recording_alias_tracks(&store, &source);
        assert!(!aliases.is_empty());
        assert!(aliases.iter().any(|alias| {
            alias.artist == "Bulgarian State Radio & Television Female Vocal Choir"
                && alias.id == source.id
                && alias.provider_id == source.provider_id
                && alias.source_url == source.source_url
                && alias.title == source.title
                && alias.version == source.version
                && alias.album == source.album
                && alias.duration_seconds == source.duration_seconds
                && alias.provenance == source.provenance
        }));

        let mut manual_edit = source.clone();
        manual_edit.title.push_str(" (manual edit)");
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &manual_edit).is_empty());
        let mut wrong_album = source.clone();
        wrong_album.provenance["recordingContext"]["albumId"] = json!("cccccccccccccccccccccc");
        wrong_album.provenance["recordingContext"]["albumUri"] =
            json!("spotify:album:cccccccccccccccccccccc");
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &wrong_album).is_empty());
        let mut versioned = source;
        versioned.version = Some("Live version".into());
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &versioned).is_empty());
    }

    #[test]
    fn spotify_crosswalk_alias_can_use_only_recent_verified_track_context_cache() {
        let release = release_fixture();
        let (_directory, store) = cached_store(&release, Some(&profile()));
        crate::artists::write_verified_spotify_recording_sidecar(
            &store,
            &spotify_occurrence_fixture(),
            1,
        )
        .unwrap();
        let track = spotify_context_fixture_track();
        cache_verified_spotify_track_context(&store, &track, "aaaaaaaaaaaaaaaaaaaaaa");
        let mut legacy = track.clone();
        legacy.provenance = json!({"remotePlaylist":true});
        assert!(!cached_verified_spotify_recording_alias_tracks(&store, &legacy).is_empty());
        legacy.artist = "User edited artist".into();
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &legacy).is_empty());
    }

    #[test]
    fn spotify_embedded_album_context_cannot_bypass_missing_ambiguous_or_conflicting_cache() {
        let release = release_fixture();
        let (_directory, store) = cached_store(&release, Some(&profile()));
        crate::artists::write_verified_spotify_recording_sidecar(
            &store,
            &spotify_occurrence_fixture(),
            1,
        )
        .unwrap();
        let track = spotify_context_fixture_track();

        assert!(cached_verified_spotify_recording_alias_tracks(&store, &track).is_empty());

        store
            .set_provider_cache(
                "artists.spotify-track-context.verified-v1",
                &track.provider_id,
                &json!({"version":1,"status":"ambiguous"}),
                3600,
            )
            .unwrap();
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &track).is_empty());

        cache_verified_spotify_track_context(&store, &track, "cccccccccccccccccccccc");
        assert!(cached_verified_spotify_recording_alias_tracks(&store, &track).is_empty());
    }
}
