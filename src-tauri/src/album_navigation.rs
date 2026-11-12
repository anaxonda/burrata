//! Album navigation resolves recording membership separately from display text.
use crate::{
    artists::{ArtistRef, ArtistReleaseRef},
    models::Track,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumChoice {
    pub release: ArtistReleaseRef,
    pub artist: ArtistRef,
    pub reason: String,
    pub evidence: AlbumMembershipEvidence,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AlbumMembershipEvidence {
    RecordingOccurrence,
    CompleteTimeline,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AlbumResolution {
    Resolved {
        destination: AlbumChoice,
    },
    Ambiguous {
        choices: Vec<AlbumChoice>,
    },
    Unavailable {
        reason: String,
        #[serde(rename = "sourceAlbumUrl")]
        source_album_url: Option<String>,
    },
}
fn key(value: &str) -> String {
    crate::recording_identity::search_fold(value)
}
const MEMBERSHIP_CACHE: &str = "navigation.album-membership.v2";
fn cache_key(track: &Track) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::json!({
                "id":track.id,"provider":track.provider,"providerId":track.provider_id,
                "title":track.title,"artist":track.artist,"album":track.album,
                "albumArtist":track.album_artist,"duration":track.duration_seconds,
                "version":track.version,"isrc":track.isrc,"provenance":track.provenance
            })
            .to_string()
            .as_bytes()
        )
    )
}
pub fn album_artist(track: &Track) -> String {
    if let Some(artist) = track
        .album_artist
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        return artist.to_owned();
    }
    let identity = crate::recording_identity::RecordingIdentity::from_track(track);
    identity
        .credits
        .iter()
        .find(|credit| crate::recording_identity::is_performer_credit_role(&credit.role))
        .map(|credit| credit.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| track.artist.clone())
}
/// A matching display title alone never proves membership in a release.
pub fn recording_member(request: &Track, tracks: &[Track]) -> bool {
    if request.title.trim().is_empty()
        || crate::recording_identity::RecordingIdentity::from_track(request).identifier_conflict
    {
        return false;
    }
    let matches = tracks
        .iter()
        .filter(|member| {
            let title = crate::recording_identity::parse_title(&request.title);
            let actual = crate::recording_identity::parse_title(&member.title);
            key(&title.title) == key(&actual.title)
                && title.version.as_deref().map(key) == actual.version.as_deref().map(key)
                && request.version.as_deref().map(key) == member.version.as_deref().map(key)
                && request
                    .duration_seconds
                    .zip(member.duration_seconds)
                    .is_some_and(|(a, b)| {
                        a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && (a - b).abs() <= 2.0
                    })
                && match (request.isrc.as_deref(), member.isrc.as_deref()) {
                    (Some(a), Some(b)) => crate::recording_identity::normalize_isrc(a)
                        .zip(crate::recording_identity::normalize_isrc(b))
                        .is_some_and(|(a, b)| a == b),
                    _ => {
                        let expected = key(&album_artist(request));
                        !expected.is_empty() && expected == key(&album_artist(member))
                    }
                }
        })
        .count();
    matches == 1
}

fn same_album_timeline(
    source: &[crate::youtube_music::YouTubeMusicItem],
    catalog: &[Track],
) -> bool {
    source.len() >= 2
        && source.len() == catalog.len()
        && source.iter().zip(catalog).all(|(left, right)| {
            let left_title = crate::recording_identity::parse_title(&left.title);
            let right_title = crate::recording_identity::parse_title(&right.title);
            !key(&left_title.title).is_empty()
                && key(&left_title.title) == key(&right_title.title)
                && left_title.version.as_deref().map(key) == right_title.version.as_deref().map(key)
                && right.version.as_deref().is_none_or(|v| v.trim().is_empty())
                && left
                    .duration_seconds
                    .zip(right.duration_seconds)
                    .is_some_and(|(a, b)| {
                        a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && (a - b).abs() <= 2.0
                    })
        })
}

// A group/master tracklist selects an edition. Keep that fetched edition
// rather than asking the group to select an edition again on navigation.
fn fetched_edition(mut release: ArtistReleaseRef, tracks: &[Track]) -> Option<ArtistReleaseRef> {
    let first = tracks.first()?;
    let id = first.provenance["releaseId"].as_str()?;
    if tracks
        .iter()
        .any(|track| track.provenance["releaseId"].as_str() != Some(id))
    {
        return None;
    }
    let url = match release.release.provider.as_str() {
        "musicbrainz" if crate::spotify_mb_crosswalk::valid_mbid(id) => {
            format!("https://musicbrainz.org/release/{id}")
        }
        "discogs" if !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()) => {
            format!("https://www.discogs.com/release/{id}")
        }
        _ => return None,
    };
    release.release.id = id.to_owned();
    release.source_url = url;
    release.track_count = Some(tracks.len().try_into().ok()?);
    if let Some(album) = first
        .album
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        release.title = album.to_owned();
    }
    Some(release)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            title: "La Nave del Olvido".into(),
            artist: "Main Act".into(),
            album: Some("Album".into()),
            duration_seconds: Some(235.542),
            ..Track::default()
        }
    }
    #[test]
    fn membership_requires_unique_compatible_recording() {
        let request = track();
        let mut member = request.clone();
        member.duration_seconds = Some(236.0);
        assert!(recording_member(&request, &[member.clone()]));
        assert!(!recording_member(
            &request,
            &[member.clone(), member.clone()]
        ));
        for (title, artist, duration) in [
            ("La Nave del Olvido (Live)", "Main Act", Some(236.0)),
            ("La Nave del Olvido", "Other Singer", Some(236.0)),
            ("La Nave del Olvido", "Main Act", None),
            ("La Nave del Olvido", "Main Act", Some(210.0)),
        ] {
            member.title = title.into();
            member.artist = artist.into();
            member.duration_seconds = duration;
            assert!(!recording_member(&request, &[member.clone()]));
        }
    }
    #[test]
    fn invalid_identifiers_do_not_prove_membership() {
        let mut request = track();
        request.isrc = Some("invalid".into());
        let mut member = request.clone();
        member.isrc = Some("also invalid".into());
        assert!(!recording_member(&request, &[member]));
    }
    #[test]
    fn group_navigation_binds_the_fetched_edition_and_rejects_mixed_editions() {
        let release: ArtistReleaseRef = serde_json::from_value(serde_json::json!({
            "release":{"provider":"musicbrainz","id":"group"},"title":"Group title",
            "sourceUrl":"https://musicbrainz.org/release-group/group","credits":[]
        }))
        .unwrap();
        let mut member = track();
        member.provenance = serde_json::json!({"releaseId":"12345678-1234-4234-8234-123456789abc"});
        let bound = fetched_edition(release.clone(), &[member.clone()]).unwrap();
        assert_eq!(
            bound.source_url,
            "https://musicbrainz.org/release/12345678-1234-4234-8234-123456789abc"
        );
        assert_eq!(bound.title, "Album");
        let mut other = member.clone();
        other.provenance["releaseId"] = serde_json::json!("12345678-1234-4234-8234-123456789abd");
        assert!(fetched_edition(release, &[member, other]).is_none());
    }
    #[test]
    fn missing_artist_does_not_prove_membership_and_cache_tracks_edits() {
        let mut request = track();
        let original = cache_key(&request);
        request.album = Some("Other album".into());
        assert_ne!(cache_key(&request), original);
        request.artist.clear();
        assert!(!recording_member(&request, &[request.clone()]));
    }
    #[test]
    fn album_timeline_requires_complete_order_and_version_agreement() {
        let mut second = track();
        second.title = "Other Song".into();
        let catalog = vec![track(), second];
        let source = catalog
            .iter()
            .enumerate()
            .map(|(index, item)| crate::youtube_music::YouTubeMusicItem {
                id: format!("fixture{index}"),
                title: item.title.clone(),
                artist: Some(item.artist.clone()),
                album: Some("Alternate album title".into()),
                duration_seconds: item.duration_seconds,
                url: String::new(),
                artwork_url: None,
                isrc: None,
                artist_credits: Vec::new(),
                album_browse_id: None,
            })
            .collect::<Vec<_>>();
        assert!(same_album_timeline(&source, &catalog));
        assert!(!same_album_timeline(&source[..1], &catalog));
        let mut reordered = catalog.clone();
        reordered.reverse();
        assert!(!same_album_timeline(&source, &reordered));
        let mut wrong_version = source.clone();
        wrong_version[0].title.push_str(" (Live)");
        assert!(!same_album_timeline(&wrong_version, &catalog));
        let mut wrong_duration = source;
        wrong_duration[0].duration_seconds = Some(200.0);
        assert!(!same_album_timeline(&wrong_duration, &catalog));
    }
}
pub async fn resolve(state: &crate::AppState, track: &Track) -> Result<AlbumResolution, String> {
    let membership_key = cache_key(track);
    if let Some(value) = state
        .store
        .get_provider_cache(MEMBERSHIP_CACHE, &membership_key)?
    {
        if let Ok(result @ AlbumResolution::Resolved { .. }) = serde_json::from_value(value) {
            return Ok(result);
        }
    }
    let source_album_url = track.provenance["albumBrowseId"]
        .as_str()
        .filter(|id| {
            track.provider == "youtube"
                && id.starts_with("MPREb_")
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
        .map(|id| format!("https://music.youtube.com/browse/{id}"));
    let album = track.album.as_deref().unwrap_or("").trim();
    if album.is_empty() {
        return Ok(AlbumResolution::Unavailable {
            reason: "Album metadata is unavailable for this track".into(),
            source_album_url,
        });
    }
    let artist = album_artist(track);
    let credentials = crate::artist_credentials(state.connections.clone()).await?;
    let source_timeline = if track.provider == "youtube" {
        if let Some(browse_id) = track.provenance["albumBrowseId"].as_str() {
            crate::youtube_music::album_tracks(browse_id)
                .await
                .ok()
                .filter(|items| {
                    items
                        .iter()
                        .filter(|item| item.id == track.provider_id)
                        .count()
                        == 1
                })
        } else {
            None
        }
    } else {
        None
    };
    let page = match state.artists.search_albums(album, 1, &credentials).await {
        Ok(page) => page,
        Err(_) => {
            return Ok(AlbumResolution::Unavailable {
                reason: "Album catalogs are temporarily unavailable".into(),
                source_album_url,
            })
        }
    };
    // Preserve catalog ambiguity; do not silently pick an edition from page one.
    let mut choices = Vec::new();
    let mut candidates = page.items;
    let mut discovery_incomplete = page.next_page.is_some();
    let exact_album_found = candidates.iter().any(|release| {
        key(&release.title) == key(album)
            && release
                .credits
                .iter()
                .any(|credit| key(&credit.name) == key(&artist))
    });
    if exact_album_found {
        candidates.retain(|release| key(&release.title) == key(album));
    }
    // The source may use an alternate album title. Discover releases through
    // an exact catalog artist, then present verified recording occurrences as
    // choices; a single song cannot prove two differently titled albums equal.
    if !exact_album_found {
        if let Ok(artists) = state
            .artists
            .search_artists_for("musicbrainz", &artist, 1, 10, &credentials)
            .await
        {
            let exact = artists
                .results
                .into_iter()
                .filter(|row| key(&row.name) == key(&artist))
                .collect::<Vec<_>>();
            if exact.len() == 1 {
                if let Ok(releases) = state
                    .artists
                    .get_artist_releases(&exact[0].artist, None, &credentials)
                    .await
                {
                    discovery_incomplete |= releases.next_cursor.is_some();
                    candidates.extend(releases.items);
                }
            }
        }
    }
    candidates.sort_by_key(|release| {
        (
            key(&release.title) != key(album),
            release.release.provider.clone(),
            release.release.id.clone(),
        )
    });
    candidates.dedup_by(|a, b| a.release == b.release);
    let candidates_truncated = candidates.len() > 20;
    for release in candidates.into_iter().take(20) {
        let Some(credit) = release
            .credits
            .iter()
            .find(|credit| key(&credit.name) == key(&artist))
        else {
            continue;
        };
        let Some(artist_ref) = credit.artist.clone() else {
            continue;
        };
        let Ok(tracks) = state
            .artists
            .get_release_tracks(&release, &artist_ref, &credentials)
            .await
        else {
            continue;
        };
        let timeline_matches = source_timeline
            .as_ref()
            .is_some_and(|source| same_album_timeline(source, &tracks));
        if release
            .track_count
            .is_some_and(|count| count as usize != tracks.len())
            || !(recording_member(track, &tracks) || timeline_matches)
        {
            continue;
        }
        let Some(release) = fetched_edition(release, &tracks) else {
            continue;
        };
        choices.push(AlbumChoice {
            release,
            artist: artist_ref,
            reason: if timeline_matches {
                "Complete ordered source and catalog album timelines agree"
            } else {
                "Matching recording in the independently fetched release tracklist"
            }
            .into(),
            evidence: if timeline_matches {
                AlbumMembershipEvidence::CompleteTimeline
            } else {
                AlbumMembershipEvidence::RecordingOccurrence
            },
        });
    }
    choices.sort_by_key(|choice| {
        (
            if choice.release.release.provider == "musicbrainz" {
                0
            } else {
                1
            },
            choice.release.release.id.clone(),
        )
    });
    choices.dedup_by(|a, b| a.release.release == b.release.release);
    let result = if choices.len() == 1
        && !discovery_incomplete
        && !candidates_truncated
        && (key(&choices[0].release.title) == key(album)
            || choices[0].evidence == AlbumMembershipEvidence::CompleteTimeline)
    {
        AlbumResolution::Resolved {
            destination: choices.remove(0),
        }
    } else if !choices.is_empty() {
        AlbumResolution::Ambiguous { choices }
    } else {
        AlbumResolution::Unavailable {
            reason: "No verified album membership was found".into(),
            source_album_url,
        }
    };
    if matches!(&result, AlbumResolution::Resolved { .. }) {
        state.store.set_provider_cache(
            MEMBERSHIP_CACHE,
            &membership_key,
            &serde_json::to_value(&result).map_err(|e| e.to_string())?,
            24 * 60 * 60,
        )?;
    }
    Ok(result)
}
