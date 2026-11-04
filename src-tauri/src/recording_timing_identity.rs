//! Store-backed recognition-to-request timing eligibility. Kept out of the pure
//! recording matcher so the matcher can be compiled independently for audits.
use crate::{
    models::{Candidate, Track},
    recording_identity::parse_title,
    recording_match::{assess, semantic_version, Comparison, MatchLevel, MatchStage},
    Store,
};

/// Require a recognized result to agree with the currently playing catalog
/// recording before its point offset can drive lyric timing. The recognition
/// fields remain source observations; this function only returns an eligibility
/// decision and never copies request metadata into them.
pub(crate) fn recognition_matches_timing_identity(
    store: &Store,
    requested: &Track,
    observed_title: &str,
    observed_artist: &str,
) -> bool {
    if observed_title.trim().is_empty()
        || observed_title.len() > 512
        || observed_artist.trim().is_empty()
        || observed_artist.len() > 512
    {
        return false;
    }

    let observed = Candidate {
        id: "songrec:current-recognition".into(),
        provider: "songrec".into(),
        title: observed_title.to_owned(),
        source_metadata: Some(serde_json::json!({
            "sourceMetadataOrigin": "songrecRecognition",
            "track": observed_title,
            "artist": observed_artist,
        })),
        ..Candidate::default()
    };

    let requested_title = parse_title(&requested.title);
    let recognized_title = parse_title(observed_title);
    if crate::matching::normalize_title_ignoring_intraword_apostrophes(&requested_title.title)
        != crate::matching::normalize_title_ignoring_intraword_apostrophes(&recognized_title.title)
    {
        return false;
    }

    let requested_artist = crate::matching::normalize_single_artist_name(&requested.artist);
    let recognized_artist = crate::matching::normalize_single_artist_name(observed_artist);
    let comparison_track = if requested_artist == recognized_artist {
        requested.clone()
    } else {
        // This is a read of already-cached MusicBrainz profiles only. The
        // helper requires one unique profile to contain both exact names.
        let Some(alias) =
            crate::recording_match_aliases::scoped_name_alias(store, requested, &observed)
        else {
            return false;
        };
        alias
    };

    let assessment = assess(
        &comparison_track,
        &observed,
        MatchLevel::Balanced,
        MatchStage::Resolved,
    );
    let any_version = !semantic_version(
        &requested.title,
        requested.version.as_deref().unwrap_or_default(),
    )
    .is_empty()
        || !semantic_version(observed_title, "").is_empty();
    assessment.dimensions.title == Comparison::Agree
        && assessment.dimensions.artists == Comparison::Agree
        && if any_version {
            assessment.dimensions.version == Comparison::Agree
        } else {
            assessment.dimensions.version != Comparison::Conflict
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Track;
    use serde_json::json;

    #[test]
    fn songrec_timing_identity_requires_title_artist_and_version_agreement() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&directory.path().join("timing-identity.sqlite")).unwrap();
        let track = Track {
            artist: "Original Artist".into(),
            title: "Song Name".into(),
            duration_seconds: Some(210.0),
            ..Track::default()
        };
        assert!(recognition_matches_timing_identity(
            &store,
            &track,
            "Song Name",
            "Original Artist"
        ));
        assert!(!recognition_matches_timing_identity(
            &store,
            &track,
            "Different Song",
            "Original Artist"
        ));
        assert!(!recognition_matches_timing_identity(
            &store,
            &track,
            "Song Name",
            "Cover Artist"
        ));
        assert!(!recognition_matches_timing_identity(
            &store,
            &track,
            "Song Name (Extended Remix)",
            "Original Artist"
        ));

        let versioned_track = Track {
            version: Some("Radio Edit".into()),
            ..track.clone()
        };
        assert!(!recognition_matches_timing_identity(
            &store,
            &versioned_track,
            "Song Name",
            "Original Artist"
        ));
        assert!(recognition_matches_timing_identity(
            &store,
            &versioned_track,
            "Song Name (Radio Edit)",
            "Original Artist"
        ));
    }

    #[test]
    fn songrec_timing_artist_alias_requires_unique_cached_musicbrainz_profile() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&directory.path().join("timing-alias.sqlite")).unwrap();
        let track = Track {
            artist: "Stage Name".into(),
            title: "Song Name".into(),
            ..Track::default()
        };

        assert!(!recognition_matches_timing_identity(
            &store,
            &track,
            "Song Name",
            "Verified Alias"
        ));
        store
            .set_retained_provider_cache(
                "artists.profile.musicbrainz",
                "80fb9c75-af15-4a6b-9528-715f9ef4ebea",
                &json!({
                    "id":"80fb9c75-af15-4a6b-9528-715f9ef4ebea",
                    "name":"Stage Name",
                    "aliases":[{"name":"Verified Alias","type":"Artist name"}],
                }),
            )
            .unwrap();
        assert!(recognition_matches_timing_identity(
            &store,
            &track,
            "Song Name",
            "Verified Alias"
        ));
    }
}
