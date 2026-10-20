//! Gather retained recording evidence for every consumer of the policy evaluator.
//! These scoped variants are inputs to one evaluator, never independent gates.
use crate::{
    models::{Candidate, Track},
    recording_match::{
        self, Comparison, MatchAction, MatchAssessment, MatchLevel, MatchRelationship, MatchStage,
    },
    storage::Store,
};

pub(crate) fn priority(a: &MatchAssessment) -> (u8, u8) {
    let relationship = match a.relationship {
        MatchRelationship::Exact => 0,
        MatchRelationship::Probable => 1,
        MatchRelationship::Alternative => 2,
        MatchRelationship::Unresolved => 3,
    };
    let action = match a.action {
        MatchAction::Accept => 0,
        MatchAction::Inspect => 1,
        MatchAction::Enrich => 2,
        MatchAction::Reject => 3,
    };
    (
        if a.action == MatchAction::Reject {
            4
        } else {
            relationship
        },
        action,
    )
}
pub(crate) fn assess(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
) -> MatchAssessment {
    let aliases =
        crate::artist_credit_aliases::cached_verified_playback_artist_aliases(store, track);
    assess_with_aliases(
        store,
        track,
        candidate,
        level,
        stage,
        aliases.tracks(),
        aliases.isrc_enriched(),
        aliases.verified_recording_isrc(),
    )
}
pub(crate) fn assess_with_aliases(
    store: &Store,
    track: &Track,
    candidate: &Candidate,
    level: MatchLevel,
    stage: MatchStage,
    aliases: &[Track],
    isrc_aliases: &[Track],
    known_isrc: Option<&str>,
) -> MatchAssessment {
    let mut requested = track.clone();
    if requested.isrc.is_none() {
        requested.isrc = known_isrc.map(str::to_owned);
    }
    let mut best = recording_match::assess(&requested, candidate, level, stage);
    for variant in aliases {
        let mut same = track.clone();
        same.artist = variant.artist.clone();
        if isrc_aliases.contains(variant) {
            same.isrc = variant.isrc.clone();
        }
        if &same != variant {
            continue;
        }
        let next = recording_match::assess(variant, candidate, level, stage);
        if priority(&next) < priority(&best) {
            best = next;
        }
    }
    if level != MatchLevel::Exact
        && best.dimensions.title == Comparison::Agree
        && best.dimensions.artists != Comparison::Agree
    {
        if let Some(variant) =
            crate::recording_match_aliases::scoped_name_alias(store, track, candidate)
        {
            let next = recording_match::assess(&variant, candidate, level, stage);
            if priority(&next) < priority(&best) {
                best = next;
            }
        }
    }
    best
}
