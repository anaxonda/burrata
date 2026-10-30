//! Exact-ID-scoped MusicBrainz artist alias context for playback search/matching.
//!
//! This module deliberately fails closed: caller-supplied Track provenance is
//! accepted only after its exact occurrence and full artist credits match raw
//! cached MusicBrainz recording/release data and exact provider-ID relations.
use crate::{models::Track, Store};
use serde_json::Value;
use sha2::Digest;

const RELEASE_CACHE: &str = "artists.release-tracks.musicbrainz";
const ARTIST_PROFILE_CACHE: &str = "artists.profile.musicbrainz";
const SPOTIFY_RECORDING_CREDIT_CACHE: &str = "artists.spotify-recording-credit-crosswalk.v1";
const SPOTIFY_RECORDING_CREDIT_RAW_CACHE: &str =
    "artists.spotify-recording-credit-crosswalk.musicbrainz-recording";
const SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE: &str =
    "artists.spotify-recording-credit-crosswalk.musicbrainz-artist";
const MAX_CREDIT_ARTISTS: usize = 16;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpotifyArtistCreditRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpotifyArtistCreditFingerprint {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    pub artists: Vec<SpotifyArtistCreditRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpotifyArtistCreditCrosswalkStatus {
    pub status: String,
    pub spotify_track_id: String,
    pub musicbrainz_recording_id: String,
    pub source_isrc: String,
    pub artist_credit_count: usize,
    pub album_link_verified: bool,
}

pub(crate) fn spotify_recording_credit_raw_cache_namespace() -> &'static str {
    SPOTIFY_RECORDING_CREDIT_RAW_CACHE
}

pub(crate) fn spotify_recording_credit_profile_cache_namespace() -> &'static str {
    SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE
}

pub(crate) fn valid_musicbrainz_recording_id(value: &str) -> bool {
    valid_mbid(value)
}

/// MusicBrainz omits the empty join phrase on a final artist-credit element.
/// Missing/null is accepted only in that position; malformed non-string joins
/// and missing joins between credits remain incomplete evidence.
pub(crate) fn verified_musicbrainz_joinphrase(credit: &Value, is_final: bool) -> Option<&str> {
    match credit.get("joinphrase") {
        Some(Value::String(join)) => Some(join),
        None | Some(Value::Null) if is_final => Some(""),
        _ => None,
    }
}

/// Shared cache-only alias context for ordinary resolution and same-source recovery.
pub(crate) fn cached_verified_playback_artist_alias_tracks(
    store: &Store,
    track: &Track,
) -> Vec<Track> {
    cached_verified_playback_artist_aliases(store, track).tracks
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VerifiedPlaybackArtistAliases {
    tracks: Vec<Track>,
    /// Exact aliases whose only identity-field addition is an independently
    /// verified recording ISRC. This subset is generated from the private
    /// crosswalk cache and is never populated from caller-supplied tracks.
    isrc_enriched: Vec<Track>,
    verified_recording_isrc: Option<String>,
    credit_sets: Vec<VerifiedCreditSet>,
    discogs_credit_sets: Vec<VerifiedDiscogsCreditSet>,
    musicbrainz_credit_sets: Vec<VerifiedMusicBrainzCreditSet>,
    release_runtime_variants: Vec<crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant>,
    source: Option<Track>,
}

#[derive(Debug, Clone)]
struct VerifiedCreditSet {
    names: Vec<Vec<String>>,
    join_phrases: Vec<String>,
    isrc: Option<String>,
}

#[derive(Debug, Clone)]
struct VerifiedDiscogsCreditSet {
    release_id: String,
    artist_ids: Vec<String>,
    credit_set: VerifiedCreditSet,
}

#[derive(Debug, Clone)]
struct VerifiedMusicBrainzCreditSet {
    release_id: String,
    artist_ids: Vec<String>,
    credit_set: VerifiedCreditSet,
}

pub(crate) use crate::recording_identity::VerifiedArtistCreditSetVariant;

impl VerifiedCreditSet {
    fn matches(&self, candidate: &str) -> bool {
        self.matching_names(candidate).is_some()
    }

    fn matching_names(&self, candidate: &str) -> Option<Vec<String>> {
        // Dynamic programming over candidate offsets keeps every retained alias
        // reachable without materializing a Cartesian product. Every credit and
        // its role-bearing join phrase must consume its part of the string.
        let joined_candidate = crate::source_fallback::normalize_artist_credit(candidate);
        let candidate = crate::matching::normalize(candidate);
        if candidate.is_empty() || self.names.is_empty() {
            return None;
        }
        for mode in 0..3 {
            // Conjunction normalization is safe for a flat catalog performer
            // list. Keep role-bearing joins (e.g. MusicBrainz `feat.`) exact:
            // replacing their semantics with `and` is not alias evidence.
            if mode == 2
                && self.join_phrases.iter().any(|join| {
                    !matches!(
                        join.trim().to_ascii_lowercase().as_str(),
                        "" | "," | "&" | "and"
                    )
                })
            {
                continue;
            }
            let candidate = if mode == 2 {
                joined_candidate.clone()
            } else {
                candidate.clone()
            };
            let mut states = vec![(0usize, Vec::<String>::new())];
            for (index, names) in self.names.iter().enumerate() {
                let mut next = Vec::new();
                for (offset, selected) in states {
                    for name in names {
                        let join = if mode == 2 {
                            if index + 1 == self.names.len() {
                                ""
                            } else {
                                " and "
                            }
                        } else if mode == 1 {
                            if index + 1 == self.names.len() {
                                ""
                            } else {
                                ", "
                            }
                        } else {
                            self.join_phrases[index].as_str()
                        };
                        let fragment = if mode == 2 {
                            crate::source_fallback::normalize_artist_credit(&format!(
                                "{name}{join}"
                            ))
                        } else {
                            crate::matching::normalize(&format!("{name}{join}"))
                        };
                        if fragment.is_empty() {
                            continue;
                        }
                        let Some(tail) = candidate[offset..].strip_prefix(&fragment) else {
                            continue;
                        };
                        let next_offset = if tail.is_empty() {
                            candidate.len()
                        } else if let Some(tail) = tail.strip_prefix(' ') {
                            candidate.len() - tail.len()
                        } else {
                            continue;
                        };
                        let mut next_selected = selected.clone();
                        next_selected.push(name.clone());
                        next.push((next_offset, next_selected));
                        if next.len() >= 512 {
                            break;
                        }
                    }
                    if next.len() >= 512 {
                        break;
                    }
                }
                states = next;
                if states.is_empty() {
                    break;
                }
            }
            if let Some((_, selected)) = states
                .into_iter()
                .find(|(offset, _)| *offset == candidate.len())
            {
                return Some(selected);
            }
        }
        None
    }
}

impl VerifiedPlaybackArtistAliases {
    /// These variants come only from the private, account-fenced catalog
    /// verification result, never from a saved provenance sidecar or uploader.
    pub(crate) fn from_catalog_recording_variants(source: &Track, artists: &[String]) -> Self {
        let mut result = Self {
            source: Some(source.clone()),
            ..Default::default()
        };
        for artist in artists
            .iter()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .take(30)
        {
            let mut track = source.clone();
            track.artist = artist.clone();
            result.tracks.push(track);
            result.credit_sets.push(VerifiedCreditSet {
                names: vec![vec![artist.clone()]],
                join_phrases: vec![String::new()],
                isrc: None,
            });
        }
        result
    }
    pub(crate) fn has_verified_credit_sets(&self) -> bool {
        !self.credit_sets.is_empty()
    }

    /// Complete search renderings, excluding the unchanged source spelling.
    /// Per-credit canonical choices precede the compatibility variant list.
    pub(crate) fn search_artist_candidates(&self) -> Vec<String> {
        let Some(source) = self.source.as_ref() else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        for credits in &self.credit_sets {
            for rank in [1usize, 0] {
                candidates.push(
                    credits
                        .names
                        .iter()
                        .zip(&credits.join_phrases)
                        .map(|(names, join)| {
                            format!(
                                "{}{}",
                                names
                                    .get(rank)
                                    .or_else(|| names.first())
                                    .map(String::as_str)
                                    .unwrap_or_default(),
                                join
                            )
                        })
                        .collect::<String>(),
                );
            }
        }
        candidates.extend(self.tracks.iter().map(|variant| variant.artist.clone()));
        let mut seen =
            std::collections::HashSet::from([crate::matching::normalize(&source.artist)]);
        candidates.retain(|artist| {
            !artist.trim().is_empty() && seen.insert(crate::matching::normalize(artist))
        });
        candidates
    }

    pub(crate) fn include_candidate_artist(&mut self, source: &Track, artist: &str) -> bool {
        let Some(variant) = self.matching_track(source, artist) else {
            return false;
        };
        if variant.isrc != source.isrc && !self.isrc_enriched.contains(&variant) {
            self.isrc_enriched.push(variant.clone());
        }
        if !self.tracks.contains(&variant) {
            self.tracks.push(variant);
        }
        true
    }

    /// Materialize only the complete artist rendering actually being assessed.
    pub(crate) fn matching_track(&self, source: &Track, candidate_artist: &str) -> Option<Track> {
        if self.source.as_ref() != Some(source) {
            return None;
        }
        let credits = self
            .credit_sets
            .iter()
            .find(|credits| credits.matches(candidate_artist))?;
        let mut variant = source.clone();
        variant.artist = candidate_artist.to_owned();
        if let Some(isrc) = &credits.isrc {
            variant.isrc = Some(isrc.clone());
        }
        Some(variant)
    }

    pub(crate) fn matching_credit_names(
        &self,
        source: &Track,
        candidate_artist: &str,
    ) -> Option<Vec<String>> {
        if self.source.as_ref() != Some(source) {
            return None;
        }
        self.credit_sets
            .iter()
            .find_map(|credits| credits.matching_names(candidate_artist))
    }

    pub(crate) fn discogs_credit_variant(
        &self,
        source: &Track,
        candidate_artist: &str,
    ) -> Option<VerifiedArtistCreditSetVariant> {
        if self.source.as_ref() != Some(source) {
            return None;
        }
        self.discogs_credit_sets.iter().find_map(|verified| {
            let candidate_credits = verified.credit_set.matching_names(candidate_artist)?;
            let requested_credits = verified
                .credit_set
                .names
                .iter()
                .map(|choices| choices.first().cloned())
                .collect::<Option<Vec<_>>>()?;
            (requested_credits.len() == candidate_credits.len()
                && candidate_credits.len() == verified.artist_ids.len())
            .then(|| VerifiedArtistCreditSetVariant {
                provider: "discogs".into(),
                evidence_scope: "exactDiscogsReleaseArtistNameVariationsOnly".into(),
                release_id: verified.release_id.clone(),
                artist_ids: verified.artist_ids.clone(),
                requested_credits,
                candidate_credits,
            })
        })
    }

    pub(crate) fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub(crate) fn isrc_enriched(&self) -> &[Track] {
        &self.isrc_enriched
    }

    pub(crate) fn verified_recording_isrc(&self) -> Option<&str> {
        self.verified_recording_isrc.as_deref()
    }

    pub(crate) fn release_runtime_variants(
        &self,
    ) -> &[crate::artist_alias_discovery::VerifiedReleaseRuntimeVariant] {
        &self.release_runtime_variants
    }

    pub(crate) fn musicbrainz_credit_variant(
        &self,
        source: &Track,
        candidate_artist: &str,
    ) -> Option<VerifiedArtistCreditSetVariant> {
        if self.source.as_ref() != Some(source) {
            return None;
        }
        self.musicbrainz_credit_sets.iter().find_map(|verified| {
            let candidate_credits = verified.credit_set.matching_names(candidate_artist)?;
            let requested_credits = verified
                .credit_set
                .names
                .iter()
                .map(|choices| choices.first().cloned())
                .collect::<Option<Vec<_>>>()?;
            (requested_credits.len() == candidate_credits.len()
                && candidate_credits.len() == verified.artist_ids.len())
            .then(|| VerifiedArtistCreditSetVariant {
                provider: "musicbrainz".into(),
                evidence_scope: "musicBrainzArtistProfileNameEquivalenceOnly".into(),
                release_id: verified.release_id.clone(),
                artist_ids: verified.artist_ids.clone(),
                requested_credits,
                candidate_credits,
            })
        })
    }
}

pub(crate) fn cached_verified_playback_artist_aliases(
    store: &Store,
    track: &Track,
) -> VerifiedPlaybackArtistAliases {
    let original = track;
    let hydrated = crate::remote_playlists::read_alias_hydrated_track(store, original);
    let track = hydrated.as_ref().unwrap_or(original);
    let mut credit_sets = Vec::new();
    let mut aliases =
        cached_verified_artist_alias_tracks_with_context(store, track, &mut credit_sets);
    let mut isrc_enriched = Vec::new();
    let mut verified_recording_isrc = None;
    let mut release_runtime_variants = Vec::new();
    let mut musicbrainz_credit_sets = Vec::new();
    if track.provider == "spotify" {
        // A unique MusicBrainz profile that owns the request spelling can add
        // scoped lexical aliases. It does not create a recording credit set or
        // a Spotify↔MusicBrainz identity binding.
        for artist in crate::artist_alias_discovery::cached_scoped_name_aliases(store, track) {
            let mut variant = original.clone();
            variant.artist = artist;
            aliases.push(variant);
        }
        aliases.extend(cached_verified_spotify_recording_alias_tracks_with_context(
            store,
            track,
            &mut credit_sets,
        ));
        let recording_aliases =
            cached_verified_spotify_recording_artist_id_alias_tracks_with_context(
                store,
                track,
                &mut credit_sets,
            );
        verified_recording_isrc = recording_aliases
            .iter()
            .find_map(|alias| alias.isrc.clone());
        isrc_enriched.extend(
            recording_aliases
                .iter()
                .filter(|alias| alias.isrc != original.isrc)
                .cloned(),
        );
        aliases.extend(recording_aliases);
        if let Some(credits) = cached_verified_provider_artist_credit_set(store, original) {
            // Search fanout stays bounded while matching keeps every alias.
            let canonical = credits
                .names
                .iter()
                .filter_map(|names| names.get(1).or_else(|| names.first()))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if !canonical.is_empty() && canonical != original.artist {
                let mut variant = original.clone();
                variant.artist = canonical;
                aliases.push(variant);
            }
            credit_sets.push(credits);
        }
        aliases.extend(
            crate::artist_alias_discovery::cached_isrc_authority_alias_tracks(store, original),
        );
        release_runtime_variants.extend(
            crate::artist_alias_discovery::cached_isrc_authority_runtime_variants(store, original),
        );
        if let Some(choices) =
            crate::artist_alias_discovery::cached_isrc_authority_credit_choices(store, original)
        {
            if verified_recording_isrc.is_none() {
                verified_recording_isrc =
                    spotify_artist_credit_fingerprint(track).and_then(|source| source.isrc);
            }
            credit_sets.push(VerifiedCreditSet {
                names: choices.iter().map(|(names, _)| names.clone()).collect(),
                join_phrases: choices.into_iter().map(|(_, join)| join).collect(),
                isrc: None,
            });
        }
    }
    if track.provider == "catalog" && track.provenance["catalogProvider"] == "musicbrainz" {
        if let Some(credits) = cached_musicbrainz_credit_set(store, track) {
            credit_sets.push(credits.credit_set.clone());
            musicbrainz_credit_sets.push(credits);
        }
        release_runtime_variants.extend(
            crate::artist_alias_discovery::cached_musicbrainz_occurrence_runtime_variants(
                store, original,
            ),
        );
    }
    if let Some(choices) =
        crate::artist_alias_discovery::cached_generic_isrc_authority_credit_choices(store, original)
    {
        if verified_recording_isrc.is_none() {
            verified_recording_isrc =
                crate::recording_identity::RecordingIdentity::from_track(original).isrc;
        }
        credit_sets.push(VerifiedCreditSet {
            names: choices.iter().map(|(names, _)| names.clone()).collect(),
            join_phrases: choices.into_iter().map(|(_, join)| join).collect(),
            isrc: None,
        });
    }
    let mut discogs_credit_sets = Vec::new();
    if let Some(verified_credits) = cached_discogs_credit_set(store, original) {
        let canonical = verified_credits
            .credit_set
            .names
            .iter()
            .filter_map(|names| names.get(1).or_else(|| names.first()))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if !canonical.is_empty() && canonical != original.artist {
            let mut variant = original.clone();
            variant.artist = canonical;
            aliases.push(variant);
        }
        credit_sets.push(verified_credits.credit_set.clone());
        discogs_credit_sets.push(verified_credits);
    }
    for runtime in &release_runtime_variants {
        if runtime.release_artist_credit != original.artist {
            let mut variant = original.clone();
            variant.artist = runtime.release_artist_credit.clone();
            aliases.push(variant);
        }
    }
    aliases.sort_by(|left, right| {
        left.artist
            .cmp(&right.artist)
            .then_with(|| left.isrc.cmp(&right.isrc))
    });
    aliases.dedup_by(|left, right| left.artist == right.artist && left.isrc == right.isrc);
    isrc_enriched.retain(|variant| aliases.contains(variant));
    // Hydration is proof context only: candidate variants retain every saved field.
    for variant in aliases.iter_mut().chain(isrc_enriched.iter_mut()) {
        let artist = variant.artist.clone();
        let isrc = variant.isrc.clone();
        *variant = original.clone();
        variant.artist = artist;
        variant.isrc = isrc;
    }
    VerifiedPlaybackArtistAliases {
        tracks: aliases,
        isrc_enriched,
        verified_recording_isrc,
        credit_sets,
        discogs_credit_sets,
        musicbrainz_credit_sets,
        release_runtime_variants,
        source: Some(original.clone()),
    }
}

pub(crate) const DISCOGS_ALIAS_RELEASE_CACHE: &str = "artists.alias-discovery.discogs-release.v1";
pub(crate) const DISCOGS_ALIAS_PROFILE_CACHE: &str = "artists.alias-discovery.discogs-artist.v1";

/// Bind every artist ID to the exact provider occurrence by reparsing public raw
/// release metadata. Caller provenance alone cannot authorize alias expansion.
pub(crate) fn verified_discogs_credit_refs(
    store: &Store,
    track: &Track,
) -> Option<Vec<(String, String)>> {
    if track.provider != "catalog"
        || track.provenance["catalogProvider"] != "discogs"
        || track.version.is_some()
        || track.playback_range.is_some()
        || track.provenance["manualFields"]["artist"] == true
        || track.provenance["manualFields"]["artistCredits"] == true
    {
        return None;
    }
    let id = track.provenance["releaseId"].as_str()?;
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let raw = store
        .get_provider_cache(DISCOGS_ALIAS_RELEASE_CACHE, id)
        .ok()
        .flatten()
        .or_else(|| {
            store
                .get_provider_cache("artists.release-tracks.discogs-release", id)
                .ok()
                .flatten()
        })?;
    if raw["id"].as_u64().map(|value| value.to_string()).as_deref() != Some(id) {
        return None;
    }
    let requested = track.provenance["requestedReleaseSourceUrl"].as_str()?;
    let expected = crate::artists::parse_discogs_release_tracks(
        &raw,
        id,
        requested,
        track.provenance["representativeEdition"]
            .as_bool()
            .unwrap_or(false),
        None,
    )
    .into_iter()
    .find(|row| row.id == track.id)?;
    if expected.provider_id != track.provider_id
        || expected.source_url != track.source_url
        || expected.title != track.title
        || expected.artist != track.artist
        || expected.album != track.album
        || expected.duration_seconds != track.duration_seconds
        // The saved row adds app-owned overlays such as `manualFields: {}`.
        // Bind provider occurrence/credits explicitly, as in the MB verifier,
        // instead of requiring all incidental app provenance to be identical.
        || ["sourceKind", "catalogProvider", "sourceUrl", "releaseId",
            "mediumPosition", "trackPosition", "occurrence", "trackId",
            "parentTitle", "artistCredits"].iter().any(|key| {
                expected.provenance[*key] != track.provenance[*key]
            })
    {
        return None;
    }
    let credits = expected.provenance["artistCredits"].as_array()?;
    let mut refs = Vec::new();
    for credit in credits {
        let role = credit["role"].as_str().unwrap_or_default();
        if !["artist", "main artist", "main"]
            .iter()
            .any(|value| role.eq_ignore_ascii_case(value))
        {
            continue;
        }
        if credit["artist"]["provider"] != "discogs" {
            return None;
        }
        let id = credit["artist"]["id"].as_str()?;
        if id.is_empty() || id.len() > 20 || !id.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        refs.push((id.to_owned(), credit["name"].as_str()?.to_owned()));
    }
    (!refs.is_empty() && refs.len() <= MAX_CREDIT_ARTISTS).then_some(refs)
}

fn cached_discogs_credit_set(store: &Store, track: &Track) -> Option<VerifiedDiscogsCreditSet> {
    let refs = verified_discogs_credit_refs(store, track)?;
    let mut artist_ids = Vec::with_capacity(refs.len());
    let mut names = Vec::new();
    for (id, name) in refs {
        let raw = store
            .get_provider_cache(DISCOGS_ALIAS_PROFILE_CACHE, &id)
            .ok()
            .flatten()?;
        if raw["id"].as_u64().map(|value| value.to_string()).as_deref() != Some(&id)
            || raw["name"].as_str() != Some(&name)
        {
            return None;
        }
        artist_ids.push(id);
        let mut choices = vec![name];
        for alias in raw["namevariations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !alias.trim().is_empty()
                && alias.len() <= 200
                && !choices.iter().any(|name| name == alias)
            {
                choices.push(alias.to_owned());
            }
        }
        names.push(choices);
    }
    let count = names.len();
    Some(VerifiedDiscogsCreditSet {
        release_id: track.provenance["releaseId"].as_str()?.to_owned(),
        artist_ids,
        credit_set: VerifiedCreditSet {
            names,
            join_phrases: (0..count)
                .map(|index| if index + 1 == count { "" } else { ", " }.to_owned())
                .collect(),
            isrc: None,
        },
    })
}

/// Use an exact MB artist ID and its cached profile as lexical performer
/// evidence. This does not assert that a candidate is the same recording or
/// release; it only permits names owned by each requested artist ID.
fn cached_musicbrainz_credit_set(
    store: &Store,
    track: &Track,
) -> Option<VerifiedMusicBrainzCreditSet> {
    if track.provider != "catalog"
        || track.provenance["sourceKind"] != "catalog"
        || track.provenance["catalogProvider"] != "musicbrainz"
        || track.provenance["manualFields"]["artist"] == true
        || track.provenance["manualFields"]["artistCredits"] == true
        || track.version.is_some()
        || track.playback_range.is_some()
    {
        return None;
    }
    let release_id = track.provenance["releaseId"].as_str()?;
    if !valid_mbid(release_id)
        || track.source_url != format!("https://musicbrainz.org/release/{release_id}")
        || track.provenance["sourceUrl"].as_str() != Some(track.source_url.as_str())
    {
        return None;
    }
    let credits = track.provenance["artistCredits"].as_array()?;
    if credits.is_empty() || credits.len() > MAX_CREDIT_ARTISTS {
        return None;
    }
    let mut artist_ids = Vec::with_capacity(credits.len());
    let mut names = Vec::with_capacity(credits.len());
    let mut join_phrases = Vec::with_capacity(credits.len());
    for (index, credit) in credits.iter().enumerate() {
        let role = credit["role"].as_str().unwrap_or_default();
        if ![
            "artist",
            "artist credit",
            "performer",
            "main artist",
            "main",
        ]
        .iter()
        .any(|candidate| role.eq_ignore_ascii_case(candidate))
        {
            return None;
        }
        if credit["artist"]["provider"] != "musicbrainz" {
            return None;
        }
        let artist_id = credit["artist"]["id"].as_str()?;
        if !valid_mbid(artist_id) {
            return None;
        }
        let request_name = credit["name"].as_str()?.trim();
        if request_name.is_empty()
            || request_name.len() > 256
            || credit["sourceUrl"].as_str() != Some(track.source_url.as_str())
        {
            return None;
        }
        let profile = store
            .get_provider_cache(ARTIST_PROFILE_CACHE, artist_id)
            .ok()
            .flatten()?;
        if profile["id"].as_str() != Some(artist_id) {
            return None;
        }
        let mut choices = vec![request_name.to_owned()];
        if let Some(canonical) = profile["name"].as_str() {
            if !canonical.trim().is_empty() && canonical.len() <= 256 {
                choices.push(canonical.to_owned());
            }
        }
        choices.extend(
            profile["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
                .filter(|alias| {
                    !alias["type"]
                        .as_str()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
                })
                .filter_map(|alias| alias["name"].as_str())
                .filter(|name| !name.trim().is_empty() && name.len() <= 256)
                .map(str::to_owned),
        );
        let mut seen = std::collections::HashSet::new();
        choices.retain(|name| seen.insert(crate::matching::normalize(name)));
        artist_ids.push(artist_id.to_owned());
        names.push(choices);
        join_phrases.push(
            credit["joinPhrase"]
                .as_str()
                .or_else(|| credit["joinphrase"].as_str())
                .unwrap_or(if index + 1 == credits.len() { "" } else { ", " })
                .to_owned(),
        );
    }
    if names.is_empty() || names.len() != artist_ids.len() {
        return None;
    }
    Some(VerifiedMusicBrainzCreditSet {
        release_id: release_id.to_owned(),
        artist_ids,
        credit_set: VerifiedCreditSet {
            names,
            join_phrases,
            isrc: None,
        },
    })
}

/// Read the exact saved Spotify row and its normalized provider catalog fields.
/// Caller-supplied provenance alone is never enough to create an artist alias.
pub(crate) fn spotify_artist_credit_fingerprint(
    track: &Track,
) -> Option<SpotifyArtistCreditFingerprint> {
    if track.provider != "spotify"
        || !valid_spotify_id(&track.provider_id)
        || track.id != format!("spotify:{}", track.provider_id)
        || track.source_url != format!("https://open.spotify.com/track/{}", track.provider_id)
        || track.version.is_some()
        || track.playback_range.is_some()
        || track.title.trim().is_empty()
        || track.artist.trim().is_empty()
    {
        return None;
    }
    let duration = track.duration_seconds?;
    if !duration.is_finite() || duration <= 0.0 || duration > 86_400.0 {
        return None;
    }
    let catalog = track.provenance.get("catalog")?;
    if catalog["verified"] != true
        || catalog["provider"].as_str() != Some("spotify")
        || catalog["providerId"].as_str() != Some(track.provider_id.as_str())
    {
        return None;
    }
    let remote = track.provenance.get("remoteCatalog")?;
    let remote_id = remote
        .get("trackId")
        .or_else(|| remote.get("id"))
        .and_then(Value::as_str)?;
    let remote_title = remote.get("title").and_then(Value::as_str)?;
    let remote_artist = remote.get("artist").and_then(Value::as_str)?;
    let remote_duration = remote.get("durationSeconds").and_then(Value::as_f64)?;
    if remote_id != track.provider_id
        || remote_title != track.title
        || remote_artist != track.artist
        || !remote_duration.is_finite()
        || (remote_duration - duration).abs() > 0.001
    {
        return None;
    }
    let raw_refs = catalog.get("artists").and_then(Value::as_array)?;
    let provider_refs = remote.get("artistRefs").and_then(Value::as_array)?;
    let artist_count = catalog.get("artistCount").and_then(Value::as_u64)? as usize;
    if raw_refs.is_empty()
        || raw_refs.len() > MAX_CREDIT_ARTISTS
        || raw_refs.len() != provider_refs.len()
        || raw_refs.len() != artist_count
        || remote.get("artistCount").and_then(Value::as_u64) != Some(artist_count as u64)
    {
        return None;
    }
    let mut artists = Vec::with_capacity(raw_refs.len());
    for (stored, provider) in raw_refs.iter().zip(provider_refs) {
        let id = stored.get("id").and_then(Value::as_str)?;
        let name = stored.get("name").and_then(Value::as_str)?.trim();
        if stored.get("provider").and_then(Value::as_str) != Some("spotify")
            || provider.get("provider").and_then(Value::as_str) != Some("spotify")
            || provider.get("id").and_then(Value::as_str) != Some(id)
            || provider.get("name").and_then(Value::as_str).map(str::trim) != Some(name)
            || !valid_spotify_id(id)
            || name.is_empty()
            || name.len() > 256
        {
            return None;
        }
        artists.push(SpotifyArtistCreditRef {
            id: id.to_owned(),
            name: name.to_owned(),
        });
    }
    if artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
        != track.artist
    {
        return None;
    }
    let normalized_isrc = |value: Option<&str>| -> Option<Option<String>> {
        match value {
            None => Some(None),
            Some(value) => crate::recording_identity::normalize_isrc(value).map(Some),
        }
    };
    let track_isrc = normalized_isrc(track.isrc.as_deref())?;
    let direct_remote_isrc = normalized_isrc(remote.get("isrc").and_then(Value::as_str))?;
    let context_remote_isrc = normalized_isrc(
        remote
            .get("recordingContext")
            .and_then(|context| context.get("isrc"))
            .and_then(Value::as_str),
    )?;
    if direct_remote_isrc.is_some()
        && context_remote_isrc.is_some()
        && direct_remote_isrc != context_remote_isrc
    {
        return None;
    }
    let remote_isrc = direct_remote_isrc.or(context_remote_isrc);
    if track_isrc.is_some() && remote_isrc.is_some() && track_isrc != remote_isrc {
        return None;
    }
    Some(SpotifyArtistCreditFingerprint {
        track_id: track.provider_id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        duration_ms: (duration * 1000.0).round() as u64,
        isrc: track_isrc.or(remote_isrc),
        artists,
    })
}

/// Validate that a raw MusicBrainz recording and every artist profile link the
/// exact saved Spotify credits in order. This is used by both prewarming and
/// playback-time cache reads so a stale/forged sidecar cannot authorize aliases.
pub(crate) fn verify_spotify_artist_credit_recording(
    store: &Store,
    track: &Track,
    recording_id: &str,
) -> Option<(SpotifyArtistCreditFingerprint, String, usize)> {
    if !valid_mbid(recording_id) {
        return None;
    }
    let fingerprint = spotify_artist_credit_fingerprint(track)?;
    let recording = store
        .get_provider_cache(SPOTIFY_RECORDING_CREDIT_RAW_CACHE, recording_id)
        .ok()??;
    if recording.get("id").and_then(Value::as_str) != Some(recording_id)
        || crate::matching::normalize(recording.get("title").and_then(Value::as_str)?)
            != crate::matching::normalize(&fingerprint.title)
    {
        return None;
    }
    let duration_ms = recording.get("length").and_then(Value::as_u64)?;
    if duration_ms == 0 || duration_ms.abs_diff(fingerprint.duration_ms) > 5_000 {
        return None;
    }
    let credits = recording.get("artist-credit").and_then(Value::as_array)?;
    if credits.len() != fingerprint.artists.len() || credits.len() > MAX_CREDIT_ARTISTS {
        return None;
    }
    let isrcs = recording.get("isrcs").and_then(Value::as_array)?;
    let mut normalized_isrcs = isrcs
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(|isrc| {
            (isrc.len() == 12 && isrc.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                .then(|| isrc.to_ascii_uppercase())
        })
        .collect::<Option<Vec<_>>>()?;
    normalized_isrcs.sort();
    normalized_isrcs.dedup();
    if normalized_isrcs.is_empty()
        || normalized_isrcs
            .iter()
            .any(|isrc| isrc.len() != 12 || !isrc.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        || fingerprint
            .isrc
            .as_deref()
            .is_some_and(|isrc| !normalized_isrcs.iter().any(|known| known == isrc))
    {
        return None;
    }
    for (index, (credit, spotify_artist)) in credits.iter().zip(&fingerprint.artists).enumerate() {
        let artist = credit.get("artist")?;
        let mbid = artist.get("id").and_then(Value::as_str)?;
        let credit_name = credit.get("name").and_then(Value::as_str)?.trim();
        let canonical_name = artist.get("name").and_then(Value::as_str)?.trim();
        let join_phrase = verified_musicbrainz_joinphrase(credit, index + 1 == credits.len())?;
        if !valid_mbid(mbid)
            || credit_name.is_empty()
            || canonical_name.is_empty()
            || credit_name.len() > 256
            || join_phrase.len() > 32
        {
            return None;
        }
        let profile = store
            .get_provider_cache(SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, mbid)
            .ok()??;
        if profile.get("id").and_then(Value::as_str) != Some(mbid)
            || profile.get("name").and_then(Value::as_str) != Some(canonical_name)
            || !profile_has_exact_spotify_artist_url(&profile, &spotify_artist.id)
        {
            return None;
        }
    }
    // Never promote a different ISRC from the same recording. If the source
    // lacks one, retain this crosswalk only when the recording has one
    // unambiguous identifier to report.
    let verified_isrc = match fingerprint.isrc.as_deref() {
        Some(requested) => requested.to_owned(),
        None if normalized_isrcs.len() == 1 => normalized_isrcs.remove(0),
        None => return None,
    };
    Some((fingerprint, verified_isrc, credits.len()))
}

pub(crate) fn invalidate_spotify_artist_credit_recording(
    store: &Store,
    provider_id: &str,
) -> Result<(), String> {
    store.set_retained_provider_cache(
        SPOTIFY_RECORDING_CREDIT_CACHE,
        provider_id,
        &serde_json::json!({"version":1,"status":"ambiguous"}),
    )
}

pub(crate) fn write_verified_spotify_artist_credit_recording(
    store: &Store,
    track: &Track,
    recording_id: &str,
) -> Result<SpotifyArtistCreditCrosswalkStatus, String> {
    let (fingerprint, source_isrc, artist_credit_count) =
        verify_spotify_artist_credit_recording(store, track, recording_id).ok_or(
            "Spotify and MusicBrainz artist credits do not have a complete verified ID match",
        )?;
    let saved = store
        .get_library_track(&track.id)?
        .ok_or("Saved Spotify source is missing")?;
    let hydrated_saved = crate::remote_playlists::read_alias_hydrated_track(store, &saved);
    if spotify_artist_credit_fingerprint(hydrated_saved.as_ref().unwrap_or(&saved)).as_ref()
        != Some(&fingerprint)
    {
        return Err("Saved Spotify source no longer matches verified provider evidence".into());
    }
    let payload = serde_json::json!({
        "version": 1,
        "spotifyTrackId": fingerprint.track_id,
        "musicbrainzRecordingId": recording_id,
        "fingerprint": fingerprint,
    });
    store.set_retained_provider_cache(
        SPOTIFY_RECORDING_CREDIT_CACHE,
        &track.provider_id,
        &payload,
    )?;
    Ok(SpotifyArtistCreditCrosswalkStatus {
        status: "recording_credits_verified".into(),
        spotify_track_id: track.provider_id.clone(),
        musicbrainz_recording_id: recording_id.to_owned(),
        source_isrc,
        artist_credit_count,
        album_link_verified: false,
    })
}

fn profile_has_exact_spotify_artist_url(profile: &Value, spotify_artist_id: &str) -> bool {
    let Some(relations) = profile.get("relations").and_then(Value::as_array) else {
        return false;
    };
    let mut linked_ids = Vec::new();
    for relation in relations {
        let Some(resource) = relation
            .get("url")
            .and_then(|url| url.get("resource"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(id) = spotify_artist_id_from_url(resource) {
            linked_ids.push(id);
        }
    }
    linked_ids.sort_unstable();
    linked_ids.dedup();
    linked_ids.len() == 1 && linked_ids[0] == spotify_artist_id
}

fn spotify_artist_id_from_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("open.spotify.com")
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() == 2 && segments[0] == "artist" && valid_spotify_id(segments[1]) {
        Some(segments[1].to_owned())
    } else {
        None
    }
}

fn cached_verified_spotify_recording_artist_id_alias_tracks(
    store: &Store,
    track: &Track,
) -> Vec<Track> {
    cached_verified_spotify_recording_artist_id_alias_tracks_with_context(
        store,
        track,
        &mut Vec::new(),
    )
}

fn cached_verified_spotify_recording_artist_id_alias_tracks_with_context(
    store: &Store,
    track: &Track,
    credit_sets: &mut Vec<VerifiedCreditSet>,
) -> Vec<Track> {
    let Some(fingerprint) = spotify_artist_credit_fingerprint(track) else {
        return Vec::new();
    };
    let Ok(Some(saved)) = store.get_library_track(&track.id) else {
        return Vec::new();
    };
    let hydrated_saved = crate::remote_playlists::read_alias_hydrated_track(store, &saved);
    if spotify_artist_credit_fingerprint(hydrated_saved.as_ref().unwrap_or(&saved)).as_ref()
        != Some(&fingerprint)
    {
        return Vec::new();
    }
    let Ok(Some(sidecar)) =
        store.get_provider_cache(SPOTIFY_RECORDING_CREDIT_CACHE, &track.provider_id)
    else {
        return Vec::new();
    };
    let Some(recording_id) = sidecar
        .get("musicbrainzRecordingId")
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    if sidecar.get("version").and_then(Value::as_u64) != Some(1)
        || sidecar.get("spotifyTrackId").and_then(Value::as_str) != Some(track.provider_id.as_str())
        || sidecar
            .get("fingerprint")
            .and_then(|value| {
                serde_json::from_value::<SpotifyArtistCreditFingerprint>(value.clone()).ok()
            })
            .as_ref()
            != Some(&fingerprint)
    {
        return Vec::new();
    }
    let Some((_, source_isrc, _)) =
        verify_spotify_artist_credit_recording(store, track, recording_id)
    else {
        return Vec::new();
    };
    let Ok(Some(recording)) =
        store.get_provider_cache(SPOTIFY_RECORDING_CREDIT_RAW_CACHE, recording_id)
    else {
        return Vec::new();
    };
    let Some(credits) = recording.get("artist-credit").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut choices: Vec<Vec<String>> = Vec::with_capacity(credits.len());
    let mut credited_names = Vec::with_capacity(credits.len());
    let mut canonical_names = Vec::with_capacity(credits.len());
    for credit in credits {
        let Some(mbid) = credit
            .get("artist")
            .and_then(|artist| artist.get("id"))
            .and_then(Value::as_str)
        else {
            return Vec::new();
        };
        let Some(credit_name) = credit
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            return Vec::new();
        };
        credited_names.push(credit_name.to_owned());
        let Ok(Some(profile)) =
            store.get_provider_cache(SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, mbid)
        else {
            return Vec::new();
        };
        let mut names = vec![credit_name.to_owned()];
        let Some(canonical) = profile
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        else {
            return Vec::new();
        };
        canonical_names.push(canonical.to_owned());
        names.push(canonical.to_owned());
        names.extend(
            profile
                .get("aliases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|alias| {
                    !alias
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
                })
                .filter_map(|alias| alias.get("name").and_then(Value::as_str))
                .filter(|name| !name.trim().is_empty() && name.len() <= 256)
                .map(str::to_owned),
        );
        names.extend(
            crate::artist_alias_discovery::verified_wikidata_alias_names(store, mbid, &profile),
        );
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| name.len() <= 256 && seen.insert(crate::matching::normalize(name)));
        if names.is_empty() {
            return Vec::new();
        }
        choices.push(names);
    }
    let Some(join_phrases) = credits
        .iter()
        .enumerate()
        .map(|(index, credit)| {
            verified_musicbrainz_joinphrase(credit, index + 1 == credits.len()).map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };
    credit_sets.push(VerifiedCreditSet {
        names: choices.clone(),
        join_phrases,
        isrc: Some(source_isrc.clone()),
    });
    // Keep both complete source credit renderings ahead of bounded alias
    // expansion. A large alias list on an early/later artist must never evict
    // the exact MusicBrainz credited names or canonical profile names.
    let mut combinations = Vec::<Vec<String>>::with_capacity(32);
    let comma_separated_credited = credited_names.join(", ");
    let comma_separated_canonical = canonical_names.join(", ");
    combinations.push(credited_names);
    if canonical_names != combinations[0] {
        combinations.push(canonical_names);
    }
    let mut expanded = vec![Vec::<String>::new()];
    for names in choices {
        let mut next = Vec::new();
        for prefix in expanded {
            for name in &names {
                let mut combination = prefix.clone();
                combination.push(name.clone());
                next.push(combination);
                if next.len() >= 32 {
                    break;
                }
            }
            if next.len() >= 32 {
                break;
            }
        }
        expanded = next;
    }
    let needs_isrc_enrichment = track.isrc.as_deref() != Some(source_isrc.as_str());
    // Reserve up to two slots for comma-separated renderings and one for the
    // unchanged saved-credit ISRC enrichment.
    let max_combinations = if needs_isrc_enrichment { 29 } else { 30 };
    for combination in expanded {
        if combinations.len() >= max_combinations {
            break;
        }
        if !combinations.contains(&combination) {
            combinations.push(combination);
        }
    }
    let mut aliases: Vec<Track> = combinations
        .into_iter()
        .filter_map(|names| {
            let artist = credits
                .iter()
                .zip(names)
                .map(|(credit, name)| {
                    format!(
                        "{}{}",
                        name,
                        credit
                            .get("joinphrase")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    )
                })
                .collect::<String>();
            let mut alias = track.clone();
            alias.artist = artist;
            alias.isrc = Some(source_isrc.clone());
            Some(alias)
        })
        .collect();
    for artist in [comma_separated_credited, comma_separated_canonical] {
        if !artist.trim().is_empty() && !aliases.iter().any(|alias| alias.artist == artist) {
            let mut alias = track.clone();
            alias.artist = artist;
            alias.isrc = Some(source_isrc.clone());
            aliases.push(alias);
        }
    }
    if needs_isrc_enrichment
        && !aliases.iter().any(|alias| {
            alias.artist == track.artist && alias.isrc.as_deref() == Some(source_isrc.as_str())
        })
    {
        let mut enriched = track.clone();
        enriched.isrc = Some(source_isrc);
        aliases.push(enriched);
    }
    aliases
}

pub(crate) fn cached_verified_spotify_recording_alias_tracks(
    store: &Store,
    track: &Track,
) -> Vec<Track> {
    cached_verified_spotify_recording_alias_tracks_with_context(store, track, &mut Vec::new())
}

fn cached_verified_spotify_recording_alias_tracks_with_context(
    store: &Store,
    track: &Track,
    credit_sets: &mut Vec<VerifiedCreditSet>,
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
    cached_verified_artist_alias_tracks_with_context(store, &musicbrainz_track, credit_sets)
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

/// Artist-ID mappings are reusable across recordings, but every ordered source
/// credit must have a unique exact provider URL lookup and matching raw profile.
fn cached_verified_provider_artist_credit_set(
    store: &Store,
    original: &Track,
) -> Option<VerifiedCreditSet> {
    let hydrated = crate::remote_playlists::read_alias_hydrated_track(store, original);
    let source = spotify_artist_credit_fingerprint(hydrated.as_ref().unwrap_or(original))?;
    let saved = store.get_library_track(&original.id).ok()??;
    let hydrated_saved = crate::remote_playlists::read_alias_hydrated_track(store, &saved);
    if spotify_artist_credit_fingerprint(hydrated_saved.as_ref().unwrap_or(&saved)).as_ref()
        != Some(&source)
    {
        return None;
    }
    let mut all_names = Vec::new();
    for credit in &source.artists {
        let evidence = store
            .get_provider_cache(
                "artists.alias-discovery.provider-artist.v1",
                &format!("spotify:{}", credit.id),
            )
            .ok()??;
        let mbid = evidence["musicbrainzArtistId"].as_str()?;
        let profile = &evidence["profile"];
        let lookup = &evidence["lookup"];
        let url = format!("https://open.spotify.com/artist/{}", credit.id);
        if evidence["version"] != 1
            || evidence["provider"] != "spotify"
            || evidence["providerArtistId"].as_str() != Some(credit.id.as_str())
            || !valid_mbid(mbid)
            || lookup["resource"].as_str() != Some(url.as_str())
        {
            return None;
        }
        let mapped = lookup["relations"]
            .as_array()?
            .iter()
            .filter(|relation| relation["target-type"] == "artist")
            .map(|relation| relation["artist"]["id"].as_str())
            .collect::<Option<std::collections::HashSet<_>>>()?;
        if mapped.len() != 1 || !mapped.contains(mbid) {
            return None;
        }
        if evidence["canonicalLookupOnly"] == true {
            let canonical =
                crate::artist_alias_discovery::exact_url_artist_name(lookup, &url, mbid)?;
            let mut names = vec![credit.name.clone()];
            if crate::matching::normalize(&credit.name) != crate::matching::normalize(canonical) {
                names.push(canonical.to_owned());
            }
            all_names.push(names);
            continue;
        }
        if profile["id"].as_str() != Some(mbid)
            || !profile_has_exact_spotify_artist_url(profile, &credit.id)
        {
            return None;
        }
        let canonical = profile["name"].as_str()?.trim();
        if canonical.is_empty() || canonical.len() > 256 {
            return None;
        }
        let mut names = vec![credit.name.clone(), canonical.to_owned()];
        names.extend(
            profile["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|alias| {
                    !alias["type"]
                        .as_str()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
                })
                .filter_map(|alias| alias["name"].as_str())
                .map(str::to_owned),
        );
        names.extend(
            crate::artist_alias_discovery::verified_wikidata_alias_names(store, mbid, profile),
        );
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| {
            !name.trim().is_empty()
                && name.len() <= 256
                && seen.insert(crate::matching::normalize(name))
        });
        all_names.push(names);
    }
    Some(VerifiedCreditSet {
        join_phrases: (0..all_names.len())
            .map(|index| {
                if index + 1 == all_names.len() {
                    String::new()
                } else {
                    ", ".into()
                }
            })
            .collect(),
        names: all_names,
        isrc: None,
    })
}

fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(crate) fn cached_verified_artist_alias_tracks(store: &Store, track: &Track) -> Vec<Track> {
    cached_verified_artist_alias_tracks_with_context(store, track, &mut Vec::new())
}

fn cached_verified_artist_alias_tracks_with_context(
    store: &Store,
    track: &Track,
    credit_sets: &mut Vec<VerifiedCreditSet>,
) -> Vec<Track> {
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
    let mut all_names: Vec<Vec<String>> = raw_credits
        .iter()
        .map(|credit| vec![credit.name.clone()])
        .collect();
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
                .filter(|alias| {
                    !alias
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("Search hint"))
                })
                .filter_map(|alias| alias["name"].as_str())
                .filter(|name| !name.trim().is_empty() && name.len() <= 256)
                .map(str::to_owned),
        );
        let canonical = names.first().cloned();
        names = canonical
            .into_iter()
            .chain(names.into_iter().skip(1).collect::<Vec<_>>())
            .collect();
        names.extend(
            crate::artist_alias_discovery::verified_wikidata_alias_names(store, mbid, &profile),
        );
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| {
            !name.trim().is_empty()
                && name.len() <= 256
                && seen.insert(crate::matching::normalize(name))
        });
        all_names[index].extend(names.clone());
        for name in names {
            if variants.len() >= 32 {
                break;
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
    credit_sets.push(VerifiedCreditSet {
        names: all_names,
        join_phrases: raw_credits
            .iter()
            .map(|credit| credit.join_phrase.clone().unwrap_or_default())
            .collect(),
        isrc: None,
    });
    variants
}

fn valid_mbid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(parts).all(|(len, part)| {
            part.len() == *len && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedMusicBrainzOccurrence {
    pub release_id: String,
    pub recording_id: String,
    pub track_guid: String,
    pub title: String,
    pub duration_seconds: f64,
    pub artist_ids: Vec<String>,
    pub isrcs: Vec<String>,
}

/// Revalidate a saved MusicBrainz release row against its exact cached raw
/// release before using its recording ID, ISRCs, credits, or runtime as a
/// locator for independent recording metadata.
pub(crate) fn verified_musicbrainz_occurrence(
    store: &Store,
    track: &Track,
) -> Option<VerifiedMusicBrainzOccurrence> {
    let (release_id, recording_id, track_guid) = verified_occurrence_ids(track)?;
    let release = store.get_provider_cache(RELEASE_CACHE, release_id).ok()??;
    let row = exact_release_occurrence(&release, release_id, recording_id, track_guid)?;
    if !matches_cached_parser_occurrence(track, &release, recording_id, track_guid) {
        return None;
    }
    let credits = occurrence_credits(row, &release)?;
    if credits.is_empty()
        || credits.len() > MAX_CREDIT_ARTISTS
        || !track_matches_cached_occurrence(track, &release, row, &credits)
    {
        return None;
    }
    let artist_ids = credits
        .iter()
        .map(|credit| credit.artist_id.clone())
        .collect::<Option<Vec<_>>>()?;
    if artist_ids.iter().any(|id| !valid_mbid(id) || id.is_empty()) {
        return None;
    }
    let duration_seconds = track.duration_seconds?;
    let isrcs = row["recording"]["isrcs"]
        .as_array()
        .or_else(|| release["isrcs"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(crate::recording_identity::normalize_isrc)
        .collect::<Option<std::collections::BTreeSet<_>>>()?
        .into_iter()
        .collect::<Vec<_>>();
    Some(VerifiedMusicBrainzOccurrence {
        release_id: release_id.to_owned(),
        recording_id: recording_id.to_owned(),
        track_guid: track_guid.to_owned(),
        title: track.title.clone(),
        duration_seconds,
        artist_ids,
        isrcs,
    })
}

/// Recording-specific ISRC linkage; release-wide ISRC lists are insufficient.
pub(crate) fn verified_musicbrainz_recording_isrc_membership(
    store: &Store,
    track: &Track,
    isrc: &str,
) -> Option<String> {
    let occurrence = verified_musicbrainz_occurrence(store, track)?;
    let normalized = crate::recording_identity::normalize_isrc(isrc)?;
    let release = store
        .get_provider_cache(RELEASE_CACHE, &occurrence.release_id)
        .ok()??;
    let row = exact_release_occurrence(
        &release,
        &occurrence.release_id,
        &occurrence.recording_id,
        &occurrence.track_guid,
    )?;
    row["recording"]["isrcs"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(crate::recording_identity::normalize_isrc)
        .any(|code| code == normalized)
        .then_some(occurrence.recording_id)
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

    #[test]
    fn musicbrainz_missing_joinphrase_means_empty_only_for_final_credit() {
        let final_credit =
            json!({"name":"Solo Artist","artist":{"id":"00000000-0000-4000-8000-000000000001"}});
        assert_eq!(
            verified_musicbrainz_joinphrase(&final_credit, true),
            Some("")
        );
        assert_eq!(verified_musicbrainz_joinphrase(&final_credit, false), None);
        let null_final = json!({"joinphrase":null});
        assert_eq!(verified_musicbrainz_joinphrase(&null_final, true), Some(""));
        let malformed_final = json!({"joinphrase":7});
        assert_eq!(
            verified_musicbrainz_joinphrase(&malformed_final, true),
            None
        );
        let explicit_join = json!({"joinphrase":" feat. "});
        assert_eq!(
            verified_musicbrainz_joinphrase(&explicit_join, false),
            Some(" feat. ")
        );
    }

    #[test]
    fn discogs_exact_occurrence_binds_each_performer_alias_without_inventing_names() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../fixtures/discogs-prolific-credit-identity.json"
        ))
        .unwrap();
        let raw = &fixture["release"];
        let track = crate::artists::parse_discogs_release_tracks(
            raw,
            "38549214",
            "https://www.discogs.com/release/38549214",
            false,
            None,
        )
        .remove(0);
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("discogs.sqlite")).unwrap();
        store
            .set_retained_provider_cache(DISCOGS_ALIAS_RELEASE_CACHE, "38549214", raw)
            .unwrap();
        assert!(
            cached_verified_playback_artist_aliases(&store, &track).has_verified_credit_sets()
                == false
        );
        for profile in fixture["profiles"].as_array().unwrap() {
            store
                .set_retained_provider_cache(
                    DISCOGS_ALIAS_PROFILE_CACHE,
                    &profile["id"].as_u64().unwrap().to_string(),
                    profile,
                )
                .unwrap();
        }
        let context = cached_verified_playback_artist_aliases(&store, &track);
        assert!(context.has_verified_credit_sets());
        let title = fixture["officialTitle"].as_str().unwrap();
        let artist = crate::source_fallback::youtube_flat_artist_credit(title).unwrap();
        let variant = context.matching_track(&track, &artist).unwrap();
        assert!(crate::source_fallback::youtube_catalog_title_matches(
            &variant,
            "Sacrifices",
            &artist
        ));
        assert!(context
            .matching_track(&track, "Nipsey Hussle, Bino Rideaux, Unrelated Guest")
            .is_none());
        assert!(context
            .matching_track(&track, "Nipsey Hussle, Bino Rideaux")
            .is_none());
        let mut forged = track.clone();
        forged.provenance["artistCredits"][2]["artist"]["id"] = json!("123");
        assert!(
            !cached_verified_playback_artist_aliases(&store, &forged).has_verified_credit_sets()
        );
        let mut changed = track.clone();
        changed.title = "Other song".into();
        assert!(
            !cached_verified_playback_artist_aliases(&store, &changed).has_verified_credit_sets()
        );
        assert!(track.duration_seconds.is_none());
        assert!(track.artist.ends_with("James Fauntleroy II"));
        let mut saved = track.clone();
        saved.provenance["manualFields"] = json!({});
        assert!(cached_verified_playback_artist_aliases(&store, &saved).has_verified_credit_sets());
        // App overlays are irrelevant; every provider identity coordinate and
        // complete artist credit still has to match raw public evidence.
        for key in [
            "sourceKind",
            "catalogProvider",
            "sourceUrl",
            "releaseId",
            "mediumPosition",
            "trackPosition",
            "occurrence",
            "trackId",
            "parentTitle",
            "artistCredits",
        ] {
            let mut changed = saved.clone();
            changed.provenance[key] = json!("changed");
            assert!(
                !cached_verified_playback_artist_aliases(&store, &changed)
                    .has_verified_credit_sets(),
                "{key}"
            );
        }
        saved.artist = "Nipsey Hussle,Bino Rideaux".into();
        saved.provenance["manualFields"] = json!({"artist":true});
        assert!(!cached_verified_playback_artist_aliases(&store, &saved).has_verified_credit_sets());
    }

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

    #[test]
    fn unique_scoped_name_profile_adds_lexical_alias_without_credit_set() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("scoped-name.sqlite")).unwrap();
        let mut track = Track::default();
        track.provider = "spotify".into();
        track.artist = "Lakis Chalkias".into();
        let key = format!(
            "v1:{:x}",
            sha2::Sha256::digest(crate::matching::normalize_single_artist_name(&track.artist))
        );
        let id = "612d1573-bdea-4657-bfc9-f78593c9ab09";
        store
            .set_provider_cache(
                "artists.alias-discovery.scoped-name-search.v1",
                &key,
                &serde_json::json!({"outcome":"unique_exact_name_profile","artistId":id}),
                3600,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                ARTIST_PROFILE_CACHE,
                id,
                &serde_json::json!({"id":id,"name":"Lakis Halkias","aliases":[{"name":"Lakis Chalkias"}]}),
            )
            .unwrap();

        let context = cached_verified_playback_artist_aliases(&store, &track);
        assert!(!context.has_verified_credit_sets());
        assert!(context
            .tracks()
            .iter()
            .any(|variant| variant.artist == "Lakis Halkias"));
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
    #[ignore = "explicit local cache/matching microbenchmark; excludes network and playback"]
    fn benchmark_verified_alias_cache_matching() {
        let mut raw_profile = profile();
        raw_profile["aliases"] = Value::Array(
            (0..80)
                .map(|index| json!({"name":format!("Synthetic alias {index}")}))
                .chain(std::iter::once(json!({"name":"Български хор"})))
                .collect(),
        );
        let (_directory, store) = cached_store(&release_fixture(), Some(&raw_profile));
        let source = fixture_track();
        let run = || {
            let mut context = cached_verified_playback_artist_aliases(&store, &source);
            assert!(context.has_verified_credit_sets());
            assert!(context.include_candidate_artist(&source, "Български хор"));
            assert!(context.matching_track(&source, "Български хор").is_some());
            assert!(!context.search_artist_candidates().is_empty());
            std::hint::black_box(context);
        };
        for _ in 0..3 {
            run();
        }
        #[cfg(target_os = "linux")]
        fn usage() -> Option<(f64, i64)> {
            let mut raw = std::mem::MaybeUninit::<libc::rusage>::uninit();
            if unsafe { libc::getrusage(libc::RUSAGE_SELF, raw.as_mut_ptr()) } != 0 {
                return None;
            }
            let raw = unsafe { raw.assume_init() };
            Some((
                raw.ru_utime.tv_sec as f64
                    + raw.ru_utime.tv_usec as f64 / 1_000_000.0
                    + raw.ru_stime.tv_sec as f64
                    + raw.ru_stime.tv_usec as f64 / 1_000_000.0,
                raw.ru_maxrss as i64,
            ))
        }
        #[cfg(not(target_os = "linux"))]
        fn usage() -> Option<(f64, i64)> {
            None
        }
        let before = usage();
        let mut timings_ms = Vec::new();
        for _ in 0..20 {
            let started = std::time::Instant::now();
            run();
            timings_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let after = usage();
        let cpu_seconds = before.zip(after).map(|(before, after)| after.0 - before.0);
        let peak_rss_kib = after.map(|value| value.1);
        println!(
            "{}",
            json!({"benchmark":"verified_alias_cache_matching_microbenchmark", "build":if cfg!(debug_assertions) {"debug"} else {"release"}, "warmupIterations":3,"measuredIterations":20,"timingsMs":timings_ms,"cpuSeconds":cpu_seconds,"processPeakRssKiB":peak_rss_kib,"scope":"local synthetic fixture: verified cache reader, late alias matching, variant and search rendering construction; excludes network, extraction, and playback"})
        );
    }

    #[test]
    fn verified_credit_sets_can_match_without_compatibility_tracks() {
        let source = fixture_track();
        let context = VerifiedPlaybackArtistAliases {
            source: Some(source.clone()),
            credit_sets: vec![VerifiedCreditSet {
                names: vec![vec![source.artist.clone(), "Български хор".into()]],
                join_phrases: vec![String::new()],
                isrc: None,
            }],
            ..Default::default()
        };
        assert!(context.tracks().is_empty());
        assert!(context.has_verified_credit_sets());
        assert!(context.matching_track(&source, "Български хор").is_some());
        assert_eq!(context.search_artist_candidates(), vec!["Български хор"]);
    }

    #[test]
    fn per_credit_matching_reaches_late_multilingual_aliases_without_dropping_guests() {
        let mut first: Vec<String> = (0..64).map(|i| format!("First alias {i}")).collect();
        first.push("Български хор".into());
        let mut second: Vec<String> = (0..64).map(|i| format!("Guest alias {i}")).collect();
        second.push("合唱団".into());
        let credits = VerifiedCreditSet {
            names: vec![first, second],
            join_phrases: vec![" feat. ".into(), "".into()],
            isrc: None,
        };
        assert!(credits.matches("Български хор feat. 合唱団"));
        assert!(credits.matches("Български хор, 合唱団"));
        assert!(!credits.matches("Български хор"));
        assert!(!credits.matches("合唱団 feat. Български хор"));
        assert!(!credits.matches("Български хор feat. Unknown Guest"));
        assert!(!credits.matches("Български хор and 合唱団"));
        assert!(!credits.matches("Български хор feat. 合唱団 feat. Extra"));
        let flat = VerifiedCreditSet {
            names: vec![
                vec!["Nipsey Hussle".into()],
                vec!["Bino Rideaux".into()],
                vec!["James Fauntleroy II".into(), "James Fauntleroy".into()],
            ],
            join_phrases: vec![", ".into(), ", ".into(), "".into()],
            isrc: None,
        };
        assert!(flat.matches("Nipsey Hussle and Bino Rideaux, James Fauntleroy"));
        for incomplete in [
            "Nipsey Hussle and Bino Rideaux",
            "Nipsey Hussle and James Fauntleroy",
            "Nipsey Hussle and Bino Rideaux, Wrong Guest",
            "Nipsey Hussle and Bino Rideaux, James",
            "Nipsey Hussle and Bino Rideaux, James Fauntleroy, Extra",
        ] {
            assert!(!flat.matches(incomplete), "{incomplete}");
        }
    }

    #[test]
    fn recognition_crosswalk_requires_exact_recording_isrc_not_release_codes() {
        let mut release = release_fixture();
        let code = "BGABC2600001";
        release["isrcs"] = json!([code]);
        let parsed = |raw: &Value| {
            crate::artists::parse_musicbrainz_release_tracks(
                raw,
                "4f7f6950-29cc-448d-a25b-445e383612f1",
                "https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1",
                false,
                None,
            )
            .remove(0)
        };
        let track = parsed(&release);
        let (_root, store) = cached_store(&release, None);
        assert_eq!(
            verified_musicbrainz_recording_isrc_membership(&store, &track, code),
            None
        );
        release["media"][0]["tracks"][0]["recording"]["isrcs"] = json!([code]);
        let mut track = parsed(&release);
        track.isrc = None;
        let (_root, store) = cached_store(&release, None);
        assert_eq!(
            verified_musicbrainz_recording_isrc_membership(&store, &track, code),
            Some("d0b8d1b1-05c3-4422-ae75-a5aae859d8dd".into())
        );
        assert_eq!(
            verified_musicbrainz_recording_isrc_membership(&store, &track, "BGABC2600002"),
            None
        );
        let mut edited = track.clone();
        edited.title = "Different recording".into();
        assert_eq!(
            verified_musicbrainz_recording_isrc_membership(&store, &edited, code),
            None
        );
        let mut recognition = crate::autotag::AudioRecognition {
            provider: "songrec".into(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            isrc: Some(code.into()),
            shazam_id: None,
            excerpt_count: 2,
            agreeing_identifiers: crate::recording_identity::RecognitionIdentifiers {
                isrc: Some(code.into()),
                ..Default::default()
            },
        };
        assert!(matches!(
            crate::download_verification::recognized_identifier_link(&store, &track, &recognition),
            Some(crate::download_verification::IdentifierLink::VerifiedCatalogRecording { .. })
        ));
        recognition.agreeing_identifiers.isrc = None;
        assert_eq!(
            crate::download_verification::recognized_identifier_link(&store, &track, &recognition),
            None
        );
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

    fn spotify_artist_credit_track() -> Track {
        let track_id = "bbbbbbbbbbbbbbbbbbbbbb";
        let artist_a = "aaaaaaaaaaaaaaaaaaaaaa";
        let artist_b = "cccccccccccccccccccccc";
        let artists = json!([
            {"provider":"spotify","id":artist_a,"name":"Spotify Act"},
            {"provider":"spotify","id":artist_b,"name":"Guest"}
        ]);
        Track {
            id: format!("spotify:{track_id}"),
            provider: "spotify".into(),
            provider_id: track_id.into(),
            source_url: format!("https://open.spotify.com/track/{track_id}"),
            title: "Song".into(),
            artist: "Spotify Act, Guest".into(),
            duration_seconds: Some(180.0),
            isrc: Some("USABC2600001".into()),
            provenance: json!({
                "catalog":{"verified":true,"provider":"spotify","providerId":track_id,"artistCount":2,"artists":artists},
                "remoteCatalog":{"trackId":track_id,"title":"Song","artist":"Spotify Act, Guest","durationSeconds":180.0,"isrc":"USABC2600001","artistCount":2,"artistRefs":artists}
            }),
            ..Track::default()
        }
    }

    fn recording_credit_fixture() -> (Value, Value, Value) {
        let recording_id = "00000000-0000-4000-8000-000000000001";
        let mb_a = "00000000-0000-4000-8000-000000000002";
        let mb_b = "00000000-0000-4000-8000-000000000003";
        let spotify_a = "aaaaaaaaaaaaaaaaaaaaaa";
        let spotify_b = "cccccccccccccccccccccc";
        let recording = json!({
            "id":recording_id,"title":"Song","length":180000,"isrcs":["USABC2600001"],"artist-credit":[
                {"name":"Provider Act","joinphrase":" feat. ","artist":{"id":mb_a,"name":"Canonical Act"}},
                {"name":"Guest","artist":{"id":mb_b,"name":"Guest"}}
            ]
        });
        let profile_a = json!({"id":mb_a,"name":"Canonical Act","aliases":[{"name":"Provider Act"}],"relations":[{"url":{"resource":format!("https://open.spotify.com/artist/{spotify_a}")}}]});
        let profile_b = json!({"id":mb_b,"name":"Guest","aliases":[],"relations":[{"url":{"resource":format!("https://open.spotify.com/artist/{spotify_b}")}}]});
        (recording, profile_a, profile_b)
    }

    #[test]
    fn exact_recording_and_ordered_artist_url_relations_enable_scoped_alias() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("cache.sqlite")).unwrap();
        let mut track = spotify_artist_credit_track();
        // The recording's trusted ISRC may enrich a row that did not contain
        // one, without mutating the saved row itself.
        track.isrc = None;
        store.save_track(&track).unwrap();
        let saved_before = store.get_library_track(&track.id).unwrap().unwrap();
        let (recording, profile_a, mut profile_b) = recording_credit_fixture();
        profile_b["aliases"] = Value::Array(
            (0..40)
                .map(|index| json!({"name":format!("Guest alternate {index}")}))
                .collect(),
        );
        let recording_id = recording["id"].as_str().unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &recording,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_a["id"].as_str().unwrap(),
                &profile_a,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_b["id"].as_str().unwrap(),
                &profile_b,
                3600,
            )
            .unwrap();

        let status =
            write_verified_spotify_artist_credit_recording(&store, &track, recording_id).unwrap();
        assert_eq!(status.status, "recording_credits_verified");
        assert!(!status.album_link_verified);
        let context = cached_verified_playback_artist_aliases(&store, &track);
        let late = context
            .matching_track(&track, "Canonical Act feat. Guest alternate 39")
            .unwrap();
        assert_eq!(late.isrc.as_deref(), Some("USABC2600001"));
        assert!(context.matching_track(&track, "Canonical Act").is_none());
        let aliases = cached_verified_spotify_recording_artist_id_alias_tracks(&store, &track);
        assert!(aliases
            .iter()
            .any(|alias| alias.artist == "Provider Act feat. Guest"));
        assert!(aliases
            .iter()
            .any(|alias| alias.artist == "Canonical Act feat. Guest"));
        assert!(aliases
            .iter()
            .any(|alias| alias.artist == track.artist
                && alias.isrc.as_deref() == Some("USABC2600001")));
        assert_eq!(
            store.get_library_track(&track.id).unwrap().unwrap(),
            saved_before
        );
        assert!(aliases
            .iter()
            .all(|alias| alias.isrc.as_deref() == Some("USABC2600001")));
    }

    #[test]
    fn provider_artist_mappings_are_reusable_and_require_every_unique_credit() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("artists.sqlite")).unwrap();
        let source = spotify_artist_credit_track();
        store.save_track(&source).unwrap();
        let (_, profile_a, profile_b) = recording_credit_fixture();
        for (provider_id, profile) in [
            ("aaaaaaaaaaaaaaaaaaaaaa", &profile_a),
            ("cccccccccccccccccccccc", &profile_b),
        ] {
            store.set_retained_provider_cache("artists.alias-discovery.provider-artist.v1", &format!("spotify:{provider_id}"), &json!({
                "version":1,"provider":"spotify","providerArtistId":provider_id,"musicbrainzArtistId":profile["id"],"profile":profile,
                "lookup":{"resource":format!("https://open.spotify.com/artist/{provider_id}"),"relations":[{"target-type":"artist","artist":{"id":profile["id"]}}]}
            })).unwrap();
        }
        let context = cached_verified_playback_artist_aliases(&store, &source);
        assert!(context
            .matching_track(&source, "Canonical Act, Guest")
            .is_some());
        assert!(context.matching_track(&source, "Canonical Act").is_none());
        assert!(context.isrc_enriched().is_empty());
        let mut second = source.clone();
        second.title = "Another recording".into();
        second.provenance["remoteCatalog"]["title"] = json!(second.title);
        store.save_track(&second).unwrap();
        assert!(cached_verified_playback_artist_aliases(&store, &second)
            .matching_track(&second, "Canonical Act, Guest")
            .is_some());
        store
            .set_retained_provider_cache(
                "artists.alias-discovery.provider-artist.v1",
                "spotify:cccccccccccccccccccccc",
                &json!({"version":1,"status":"ambiguous"}),
            )
            .unwrap();
        assert!(cached_verified_playback_artist_aliases(&store, &second)
            .matching_track(&second, "Canonical Act, Guest")
            .is_none());
    }

    #[test]
    fn url_only_canonical_names_require_complete_ordered_credit_set() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("artists.sqlite")).unwrap();
        let source = spotify_artist_credit_track();
        store.save_track(&source).unwrap();
        let namespace = "artists.alias-discovery.provider-artist.v1";
        let (_, a, b) = recording_credit_fixture();
        let entry = |provider: &str, profile: &Value| {
            json!({
                "version":1,"provider":"spotify","providerArtistId":provider,
                "musicbrainzArtistId":profile["id"],"canonicalLookupOnly":true,
                "lookup":{"resource":format!("https://open.spotify.com/artist/{provider}"),"relations":[
                    {"target-type":"artist","artist":{"id":profile["id"],"name":profile["name"]}}]}
            })
        };
        let first = entry("aaaaaaaaaaaaaaaaaaaaaa", &a);
        let second = entry("cccccccccccccccccccccc", &b);
        store
            .set_retained_provider_cache(namespace, "spotify:aaaaaaaaaaaaaaaaaaaaaa", &first)
            .unwrap();
        assert!(cached_verified_provider_artist_credit_set(&store, &source).is_none());
        store
            .set_retained_provider_cache(namespace, "spotify:cccccccccccccccccccccc", &second)
            .unwrap();
        assert!(cached_verified_playback_artist_aliases(&store, &source)
            .matching_track(&source, "Canonical Act, Guest")
            .is_some());
        let mut bad = second.clone();
        bad["lookup"]["resource"] = json!("https://open.spotify.com/artist/wrong");
        store
            .set_retained_provider_cache(namespace, "spotify:cccccccccccccccccccccc", &bad)
            .unwrap();
        assert!(cached_verified_provider_artist_credit_set(&store, &source).is_none());
    }

    #[test]
    fn verified_spotify_credit_aliases_reach_resolved_recording_assessment() {
        use crate::recording_match::{MatchAction, MatchLevel, MatchStage};
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("aliases.sqlite")).unwrap();
        let mut track = spotify_artist_credit_track();
        track.artist = "Quantic, Nidia Gongora".into();
        track.title = "Amor en Francia".into();
        track.duration_seconds = Some(270.293);
        track.isrc = None;
        let credits = [
            (
                "aaaaaaaaaaaaaaaaaaaaaa",
                "Quantic",
                "00000000-0000-4000-8000-000000000001",
            ),
            (
                "cccccccccccccccccccccc",
                "Nidia Gongora",
                "00000000-0000-4000-8000-000000000002",
            ),
        ];
        let refs = credits
            .iter()
            .map(|(id, name, _)| json!({"provider":"spotify","id":id,"name":name}))
            .collect::<Vec<_>>();
        track.provenance["catalog"]["artists"] = json!(refs);
        track.provenance["remoteCatalog"]["artistRefs"] = json!(refs);
        track.provenance["remoteCatalog"]["artist"] = json!(track.artist);
        track.provenance["remoteCatalog"]["title"] = json!(track.title);
        track.provenance["remoteCatalog"]["durationSeconds"] = json!(270.293);
        store.save_track(&track).unwrap();
        for (id, _, mbid) in credits {
            let name = if id.starts_with('a') {
                "Quantic"
            } else {
                "Nidia Góngora"
            };
            store.set_retained_provider_cache("artists.alias-discovery.provider-artist.v1", &format!("spotify:{id}"), &json!({"version":1,"provider":"spotify","providerArtistId":id,"musicbrainzArtistId":mbid,"canonicalLookupOnly":true,"lookup":{"resource":format!("https://open.spotify.com/artist/{id}"),"relations":[{"target-type":"artist","artist":{"id":mbid,"name":name}}]}})).unwrap();
        }
        let aliases = cached_verified_playback_artist_aliases(&store, &track);
        let candidate = crate::models::Candidate {
            provider: "youtube".into(),
            id: "youtube:x1aO1kcVeu4".into(),
            title: "Quantic, Nidia Góngora - Amor en Francia".into(),
            duration_seconds: Some(270.0),
            ..Default::default()
        };
        let assess = |candidate: &crate::models::Candidate| {
            crate::recording_match_context::assess_with_aliases(
                &store,
                &track,
                candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved,
                aliases.tracks(),
                aliases.isrc_enriched(),
                aliases.verified_recording_isrc(),
            )
        };
        assert_eq!(assess(&candidate).action, MatchAction::Accept);
        let wrong = crate::models::Candidate {
            title: "Quantic, Different Artist - Amor en Francia".into(),
            ..candidate.clone()
        };
        assert_ne!(assess(&wrong).action, MatchAction::Accept);
        let extra = crate::models::Candidate {
            title: "Quantic, Nidia Góngora, Other - Amor en Francia".into(),
            ..candidate.clone()
        };
        assert_ne!(assess(&extra).action, MatchAction::Accept);
        let different_runtime = crate::models::Candidate {
            duration_seconds: Some(300.0),
            ..candidate.clone()
        };
        assert_ne!(assess(&different_runtime).action, MatchAction::Accept);
        let different_version = crate::models::Candidate {
            title: "Quantic, Nidia Góngora - Amor en Francia (Live 1949)".into(),
            ..candidate.clone()
        };
        assert_ne!(assess(&different_version).action, MatchAction::Accept);
    }

    #[test]
    fn independent_isrc_fallback_retains_conflict_guard_without_enriching_track() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("fallback.sqlite")).unwrap();
        let mut source = spotify_artist_credit_track();
        source.isrc = None;
        store.save_track(&source).unwrap();
        let (recording, mut a, b) = recording_credit_fixture();
        a["relations"] = json!([]);
        a["aliases"] = json!([{"name":"Spotify Act"}]);
        for (namespace, value) in [
            (SPOTIFY_RECORDING_CREDIT_RAW_CACHE, &recording),
            (SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, &a),
            (SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, &b),
        ] {
            store
                .set_retained_provider_cache(namespace, value["id"].as_str().unwrap(), value)
                .unwrap();
        }
        store.set_retained_provider_cache("artists.alias-discovery.isrc-authority-binding.v1", &source.provider_id, &json!({"version":1,"fingerprint":spotify_artist_credit_fingerprint(&source),"recordingId":recording["id"],"search":{"count":1,"recordings":[{"id":recording["id"]}]}})).unwrap();
        let context = cached_verified_playback_artist_aliases(&store, &source);
        assert_eq!(context.verified_recording_isrc(), Some("USABC2600001"));
        let variant = context
            .matching_track(&source, "Canonical Act feat. Guest")
            .unwrap();
        let mut restored = variant.clone();
        restored.artist = source.artist.clone();
        assert_eq!(restored, source);
        assert!(context.isrc_enriched().is_empty());
    }

    #[test]
    fn generic_isrc_proof_retains_conflict_guard_and_all_source_fields() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("generic-guard.sqlite")).unwrap();
        let source = Track {
            id: "beatport:123".into(),
            provider: "beatport".into(),
            provider_id: "123".into(),
            source_url: "https://example.test/123".into(),
            title: "Song".into(),
            artist: "Source Act".into(),
            duration_seconds: Some(180.0),
            isrc: Some("USABC2600001".into()),
            genre: Some("Saved".into()),
            ..Track::default()
        };
        store.save_track(&source).unwrap();
        let (mut recording, mut profile, _) = recording_credit_fixture();
        recording["artist-credit"] = json!([{"name":"Canonical Act","joinphrase":"","artist":{"id":profile["id"],"name":"Canonical Act"}}]);
        profile["aliases"] = json!([{"name":"Source Act"}]);
        store
            .set_retained_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording["id"].as_str().unwrap(),
                &recording,
            )
            .unwrap();
        store
            .set_retained_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile["id"].as_str().unwrap(),
                &profile,
            )
            .unwrap();
        let fingerprint = json!({"id":source.id,"provider":source.provider,"providerId":source.provider_id,"sourceUrl":source.source_url,"title":source.title,"artist":source.artist,"version":source.version,"album":source.album,"albumArtist":source.album_artist,"duration":source.duration_seconds,"isrc":source.isrc,"playbackRange":source.playback_range,"remixers":source.remixers});
        store.set_retained_provider_cache("artists.alias-discovery.generic-isrc-authority.v1", &source.id, &json!({"version":1,"fingerprint":fingerprint,"recordingId":recording["id"],"search":{"count":1,"recordings":[{"id":recording["id"]}]}})).unwrap();
        let context = cached_verified_playback_artist_aliases(&store, &source);
        assert_eq!(context.verified_recording_isrc(), Some("USABC2600001"));
        let mut variant = context.matching_track(&source, "Canonical Act").unwrap();
        variant.artist = source.artist.clone();
        assert_eq!(variant, source);
        assert!(context.isrc_enriched().is_empty());
    }

    #[test]
    fn retained_hydration_preserves_original_fields_and_enrichment_proof() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("hydration.sqlite")).unwrap();
        let mut hydrated = spotify_artist_credit_track();
        hydrated.isrc = None;
        let mut original = hydrated.clone();
        original.provenance = json!({"remotePlaylist":true});
        original.genre = Some("Saved genre".into());
        hydrated.genre = original.genre.clone();
        store.save_track(&original).unwrap();
        let key = crate::remote_playlists::alias_hydration_key(&original).unwrap();
        store
            .set_retained_provider_cache(
                "artists.spotify-alias-hydration.v1",
                &key,
                &json!({"version":1,"original":original,"hydrated":hydrated,"checkedAt":0}),
            )
            .unwrap();
        let (recording, profile_a, profile_b) = recording_credit_fixture();
        for (namespace, value) in [
            (SPOTIFY_RECORDING_CREDIT_RAW_CACHE, &recording),
            (SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, &profile_a),
            (SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE, &profile_b),
        ] {
            store
                .set_retained_provider_cache(namespace, value["id"].as_str().unwrap(), value)
                .unwrap();
        }
        write_verified_spotify_artist_credit_recording(
            &store,
            &hydrated,
            recording["id"].as_str().unwrap(),
        )
        .unwrap();
        let context = cached_verified_playback_artist_aliases(&store, &original);
        let variant = context
            .matching_track(&original, "Canonical Act feat. Guest")
            .unwrap();
        assert_eq!(variant.provenance, original.provenance);
        assert_eq!(variant.genre, original.genre);
        assert_eq!(variant.isrc.as_deref(), Some("USABC2600001"));
        assert!(context.isrc_enriched().contains(&variant));
        let mut edited = original.clone();
        edited.genre = Some("Edited genre".into());
        assert!(context
            .matching_track(&edited, "Canonical Act feat. Guest")
            .is_none());
        store.save_track(&edited).unwrap();
        let refreshed = cached_verified_playback_artist_aliases(&store, &edited);
        let refreshed_variant = refreshed
            .matching_track(&edited, "Canonical Act feat. Guest")
            .unwrap();
        assert_eq!(refreshed_variant.genre, edited.genre);
        assert_eq!(refreshed_variant.provenance, edited.provenance);
        assert!(refreshed.isrc_enriched().contains(&refreshed_variant));
        let mut identity_edit = edited.clone();
        identity_edit.artist = "Edited artist".into();
        store.save_track(&identity_edit).unwrap();
        assert!(cached_verified_playback_artist_aliases(&store, &edited)
            .tracks()
            .is_empty());
        assert!(
            cached_verified_playback_artist_aliases(&store, &identity_edit)
                .tracks()
                .is_empty()
        );
    }

    #[test]
    fn recording_credit_crosswalk_rejects_incomplete_or_conflicting_source_identity() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("cache.sqlite")).unwrap();
        let track = spotify_artist_credit_track();
        store.save_track(&track).unwrap();
        let (recording, profile_a, profile_b) = recording_credit_fixture();
        let recording_id = recording["id"].as_str().unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &recording,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_a["id"].as_str().unwrap(),
                &profile_a,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_b["id"].as_str().unwrap(),
                &profile_b,
                3600,
            )
            .unwrap();
        assert!(verify_spotify_artist_credit_recording(&store, &track, recording_id).is_some());

        let mut multiple_codes = recording.clone();
        multiple_codes["isrcs"] = json!(["CH1648980010", "USABC2600001"]);
        store
            .set_retained_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &multiple_codes,
            )
            .unwrap();
        assert!(verify_spotify_artist_credit_recording(&store, &track, recording_id).is_some());
        let mut missing_requested = multiple_codes.clone();
        missing_requested["isrcs"] = json!(["CH1648980010"]);
        store
            .set_retained_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &missing_requested,
            )
            .unwrap();
        assert!(verify_spotify_artist_credit_recording(&store, &track, recording_id).is_none());
        store
            .set_retained_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &recording,
            )
            .unwrap();

        let mut wrong_order = track.clone();
        wrong_order.provenance["catalog"]["artists"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(spotify_artist_credit_fingerprint(&wrong_order).is_none());
        let mut wrong_title = track.clone();
        wrong_title.title = "Different Song".into();
        assert!(spotify_artist_credit_fingerprint(&wrong_title).is_none());
        let mut wrong_duration = track.clone();
        wrong_duration.duration_seconds = Some(190.0);
        wrong_duration.provenance["remoteCatalog"]["durationSeconds"] = json!(190.0);
        assert!(
            verify_spotify_artist_credit_recording(&store, &wrong_duration, recording_id).is_none()
        );
        let mut versioned = track.clone();
        versioned.version = Some("Live".into());
        assert!(spotify_artist_credit_fingerprint(&versioned).is_none());
        let mut wrong_isrc = track.clone();
        wrong_isrc.isrc = Some("USZZZ2600001".into());
        wrong_isrc.provenance["remoteCatalog"]["isrc"] = json!("USZZZ2600001");
        assert!(
            verify_spotify_artist_credit_recording(&store, &wrong_isrc, recording_id).is_none()
        );

        let mut wrong_profile = profile_a.clone();
        wrong_profile["relations"][0]["url"]["resource"] =
            json!("https://open.spotify.com/artist/dddddddddddddddddddddd");
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_a["id"].as_str().unwrap(),
                &wrong_profile,
                3600,
            )
            .unwrap();
        assert!(verify_spotify_artist_credit_recording(&store, &track, recording_id).is_none());
    }

    #[test]
    fn recording_credit_crosswalk_rejects_forged_or_stale_cache_context() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("cache.sqlite")).unwrap();
        let track = spotify_artist_credit_track();
        store.save_track(&track).unwrap();
        let (recording, profile_a, profile_b) = recording_credit_fixture();
        let recording_id = recording["id"].as_str().unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_RAW_CACHE,
                recording_id,
                &recording,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_a["id"].as_str().unwrap(),
                &profile_a,
                3600,
            )
            .unwrap();
        store
            .set_provider_cache(
                SPOTIFY_RECORDING_CREDIT_PROFILE_CACHE,
                profile_b["id"].as_str().unwrap(),
                &profile_b,
                3600,
            )
            .unwrap();
        let _ =
            write_verified_spotify_artist_credit_recording(&store, &track, recording_id).unwrap();
        let mut edited_track = track.clone();
        edited_track.artist = "Manually Edited Credit".into();
        assert!(
            cached_verified_spotify_recording_artist_id_alias_tracks(&store, &edited_track)
                .is_empty()
        );
        store.set_provider_cache(SPOTIFY_RECORDING_CREDIT_CACHE, &track.provider_id, &json!({"version":1,"spotifyTrackId":track.provider_id,"musicbrainzRecordingId":recording_id,"fingerprint":{"trackId":track.provider_id,"title":"Wrong","artist":track.artist,"durationMs":180000,"isrc":"USABC2600001","artists":[]}}), 3600).unwrap();
        assert!(
            cached_verified_spotify_recording_artist_id_alias_tracks(&store, &track).is_empty()
        );
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
