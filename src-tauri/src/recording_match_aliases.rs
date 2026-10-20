//! Candidate-scoped name aliases backed by already cached MusicBrainz artist
//! profiles. An alias here changes only the artist on a temporary Track value;
//! it never establishes a global identity mapping.
use crate::{
    models::{Candidate, Track},
    Store,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn norm(name: &str) -> String {
    crate::matching::normalize_single_artist_name(name)
}

/// Structured primary artist evidence. Uploader/channel labels are purposely
/// excluded: they describe the source account, not necessarily the recording.
fn candidate_primary(candidate: &Candidate) -> Option<String> {
    let structured = candidate.source_metadata.as_ref().and_then(|meta| {
        ["artists", "artistNames"]
            .iter()
            .find_map(|key| {
                meta[*key]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                ["artist", "catalogArtist"]
                    .iter()
                    .find_map(|key| meta[*key].as_str())
            })
    });
    structured
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| caption_primary(&candidate.title))
}

fn caption_primary(title: &str) -> Option<String> {
    let title = title.trim();
    // Only accept the conventional "Artist - Title" caption prefix. Do not
    // infer a performer from arbitrary title text or from uploader metadata.
    let (artist, _) = title.split_once(" - ")?;
    let artist = artist.trim();
    (!artist.is_empty() && artist.len() <= 256).then(|| artist.to_owned())
}

fn valid_mbid(id: &str) -> bool {
    let parts = id.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(parts)
            .all(|(len, part)| part.len() == *len && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn profile_names(profile: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for key in ["name", "canonicalName"] {
        if let Some(name) = profile[key].as_str() {
            let normalized = norm(name);
            if !normalized.is_empty() {
                names.insert(normalized);
            }
        }
    }
    if let Some(aliases) = profile["aliases"].as_array() {
        for alias in aliases {
            let name = alias.as_str().or_else(|| alias["name"].as_str());
            if let Some(name) = name {
                let normalized = norm(name);
                if !normalized.is_empty() {
                    names.insert(normalized);
                }
            }
        }
    }
    names
}

/// Return a temporary candidate-specific artist variant when both the saved
/// primary artist and candidate primary artist are exact names on one uniquely
/// identified cached MusicBrainz artist profile. Caller still applies normal
/// title/runtime/release evidence to this variant.
pub fn scoped_name_alias(store: &Store, track: &Track, candidate: &Candidate) -> Option<Track> {
    let requested = norm(&track.artist);
    let candidate_name = norm(&candidate_primary(candidate)?);
    if requested.is_empty() || candidate_name.is_empty() || requested == candidate_name {
        return None;
    }

    let mut profiles = store
        .cached_musicbrainz_artist_profiles_for_name(&track.artist)
        .unwrap_or_default();
    if let Some(candidate_raw) = candidate_primary(candidate) {
        profiles.extend(
            store
                .cached_musicbrainz_artist_profiles_for_name(&candidate_raw)
                .unwrap_or_default(),
        );
    }
    // Keep only profiles whose one MB entity explicitly carries both names.
    // Deduplicate the same cached entity returned by each name lookup.
    let mut matching = std::collections::BTreeMap::<String, Value>::new();
    for profile in profiles {
        let artist = if profile.get("artist").is_some() {
            &profile["artist"]
        } else {
            &profile
        };
        let id = artist["id"].as_str().or(profile["id"].as_str());
        let Some(id) = id.filter(|id| valid_mbid(id)) else {
            continue;
        };
        let names = profile_names(artist)
            .into_iter()
            .chain(profile_names(&profile))
            .collect::<BTreeSet<_>>();
        if names.contains(&requested) && names.contains(&candidate_name) {
            matching.entry(id.to_owned()).or_insert(profile);
        }
    }
    if matching.len() != 1 {
        return None;
    }
    let mut alias = track.clone();
    alias.artist = candidate_primary(candidate)?;
    Some(alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_structured_artist_or_caption_prefix_is_candidate_primary() {
        let mut c = Candidate {
            uploader: Some("Lakis Chalkias".into()),
            title: "To Miroloi tis Manas".into(),
            ..Candidate::default()
        };
        assert_eq!(candidate_primary(&c), None);
        c.title = "Lakis Chalkias - To Miroloi tis Manas".into();
        assert_eq!(candidate_primary(&c).as_deref(), Some("Lakis Chalkias"));
        c.source_metadata = Some(json!({"artists":["Lakis Halkias", "Ensemble 127.626"]}));
        assert_eq!(candidate_primary(&c).as_deref(), Some("Lakis Halkias"));
    }

    #[test]
    fn profile_alias_names_are_exactly_normalized() {
        let names = profile_names(&json!({"name":"Lakis Halkias","aliases":["Lakis Chalkias"]}));
        assert!(names.contains(&norm("Lakis Chalkias")));
        assert!(names.contains(&norm("Lakis Halkias")));
        assert!(!names.contains(&norm("Lakis Halkias and Ensemble 127.626")));
    }

    fn cached_profile(store: &Store, id: &str) {
        store
            .set_provider_cache(
                "artists.profile.musicbrainz",
                id,
                &json!({
                    "id": id,
                    "name": "Lakis Halkias",
                    "aliases": [{"name":"Lakis Chalkias"}]
                }),
                3600,
            )
            .unwrap();
    }

    fn test_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("aliases.sqlite")).unwrap();
        (dir, store)
    }

    #[test]
    fn cached_unique_profile_produces_only_a_temporary_primary_artist_variant() {
        let (_dir, store) = test_store();
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        cached_profile(&store, id);
        let track = Track {
            artist: "Lakis Chalkias".into(),
            title: "To Miroloi tis Manas (feat. Ensemble 127.626)".into(),
            ..Track::default()
        };
        let candidate = Candidate {
            title: "To Miroloi tis Manas".into(),
            source_metadata: Some(json!({"artists":["Lakis Halkias", "Ensemble 127.626"]})),
            ..Candidate::default()
        };
        let variant = scoped_name_alias(&store, &track, &candidate).unwrap();
        assert_eq!(variant.artist, "Lakis Halkias");
        assert_eq!(variant.title, track.title);
        assert_eq!(track.artist, "Lakis Chalkias");
        assert_eq!(track.title, "To Miroloi tis Manas (feat. Ensemble 127.626)");
    }

    #[test]
    fn ambiguous_cached_profiles_do_not_produce_an_alias() {
        let (_dir, store) = test_store();
        cached_profile(&store, "612d1573-bdea-4657-bfc9-f78593c9ab09");
        cached_profile(&store, "712d1573-bdea-4657-bfc9-f78593c9ab09");
        let track = Track {
            artist: "Lakis Chalkias".into(),
            ..Track::default()
        };
        let candidate = Candidate {
            source_metadata: Some(json!({"artists":["Lakis Halkias", "Ensemble 127.626"]})),
            ..Candidate::default()
        };
        assert!(scoped_name_alias(&store, &track, &candidate).is_none());
    }
}
