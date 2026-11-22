//! Album navigation resolves recording membership separately from display text.
use crate::{
    artists::{ArtistRef, ArtistReleasePage, ArtistReleaseRef},
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
    RadioTracklistMatch,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AlbumResolution {
    Spotify {
        #[serde(rename = "albumId")]
        album_id: String,
        title: String,
    },
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
const MEMBERSHIP_CACHE: &str = "navigation.album-membership.v5";
const MEMBERSHIP_CHOICES_CACHE: &str = "navigation.album-choices.v1";
const MEMBERSHIP_TTL: u64 = 30 * 24 * 60 * 60;
fn cache_key(track: &Track) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::json!({
                "id":track.id,"provider":track.provider,"providerId":track.provider_id,
                "title":track.title,"artist":track.artist,"album":track.album,
                "albumArtist":track.album_artist,"duration":track.duration_seconds,
                "version":track.version,"isrc":track.isrc,"sourceUrl":track.source_url,
                "playbackRange":track.playback_range,
                "sourceKind":track.provenance["sourceKind"],
                "albumBrowseId":track.provenance["albumBrowseId"],
                "artistCredits":crate::recording_identity::declared_credit_names(track),
                "identity":crate::recording_identity::RecordingIdentity::from_track(track),
                "recordingContext":track.provenance["recordingContext"],
                "manualFields":track.provenance["manualFields"]
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

// Radio song metadata often has no duration. For album navigation, require
// the station's album, artist and song to agree with the catalog tracklist.
fn radio_without_duration(track: &Track) -> bool {
    track.provider == "radio_song"
        && track.provenance["sourceKind"] == "radioSong"
        && track.duration_seconds.is_none()
}

pub(crate) fn radio_album_member(
    request: &Track,
    album_title: &str,
    edition_artist: Option<&ArtistRef>,
    tracks: &[Track],
) -> bool {
    let title = key(&request.title);
    let artist = key(&request.artist);
    let identity = crate::recording_identity::RecordingIdentity::from_track(request);
    if !radio_without_duration(request)
        || title.is_empty()
        || artist.is_empty()
        || identity.identifier_conflict
        || request
            .album
            .as_deref()
            .is_none_or(|album| key(album) != key(album_title))
    {
        return false;
    }
    tracks
        .iter()
        .filter(|member| {
            key(&member.title) == title
                && (key(&member.artist) == artist
                    || edition_artist.is_some_and(|edition_artist| {
                        // MusicBrainz can credit the same performer differently on
                        // the recording and its edition. Require the exact artist
                        // ID on the song and the station's credit on the edition.
                        edition_artist.provider == "musicbrainz"
                            && member.provenance["catalogProvider"] == "musicbrainz"
                            && member.album_artist.as_deref().is_some_and(|name| key(name) == artist)
                            && member.provenance["artistCredits"].as_array().is_some_and(|credits| {
                                matches!(credits.as_slice(), [credit]
                                    if credit["artist"]["provider"] == "musicbrainz"
                                        && credit["artist"]["id"].as_str() == Some(edition_artist.id.as_str()))
                            })
                    }))
                && member.version.as_deref().map(key) == request.version.as_deref().map(key)
                && match (request.isrc.as_deref(), member.isrc.as_deref()) {
                    (Some(a), Some(b)) => crate::recording_identity::normalize_isrc(a)
                        .zip(crate::recording_identity::normalize_isrc(b))
                        .is_some_and(|(a, b)| a == b),
                    _ => true,
                }
        })
        .count()
        == 1
}

async fn resolve_credited_radio_edition(
    artists: &crate::artists::ArtistService,
    credentials: &crate::artists::ArtistCredentials,
    track: &Track,
    album: &str,
) -> Option<AlbumResolution> {
    if !radio_without_duration(track) {
        return None;
    }
    let editions = artists
        .find_musicbrainz_credited_editions(album, &track.artist)
        .await?;
    let mut verified_groups = std::collections::HashSet::new();
    let mut choices = Vec::new();
    let group_count = editions
        .iter()
        .map(|(_, _, group)| group)
        .collect::<std::collections::HashSet<_>>()
        .len();
    for (release, artist, group) in editions.into_iter().take(6) {
        if verified_groups.contains(&group) {
            continue;
        }
        let Ok(tracks) = artists
            .get_release_tracks(&release, &artist, credentials)
            .await
        else {
            continue;
        };
        if !verified_edition_tracks(&release, &group, &tracks) {
            continue;
        }
        let Some(release) = fetched_edition(release, &tracks) else {
            continue;
        };
        if radio_album_member(track, &release.title, Some(&artist), &tracks) {
            verified_groups.insert(group);
            choices.push(AlbumChoice {
                release,
                artist,
                reason: "Station metadata matches a single song on this album".into(),
                evidence: AlbumMembershipEvidence::RadioTracklistMatch,
            });
        }
    }
    select_verified_albums(choices, group_count == 1)
}

fn select_verified_albums(
    mut choices: Vec<AlbumChoice>,
    complete: bool,
) -> Option<AlbumResolution> {
    match choices.len() {
        0 => None,
        1 if complete => Some(AlbumResolution::Resolved {
            destination: choices.remove(0),
        }),
        _ => Some(AlbumResolution::Ambiguous { choices }),
    }
}

fn verified_edition_tracks(release: &ArtistReleaseRef, group: &str, tracks: &[Track]) -> bool {
    complete_catalog_timeline(tracks)
        && tracks[0].provenance["releaseId"] == release.release.id
        && (group == release.release.id || tracks[0].provenance["releaseGroupId"] == group)
}

struct RecordingAlbumHint {
    recording_id: String,
    release: ArtistReleaseRef,
    artist: ArtistRef,
    group: String,
    priority: u8,
}

fn recording_album_releases(
    track: &Track,
    title: &crate::recording_identity::ParsedRecordingTitle,
    performers: &[String],
    isrc: Option<&str>,
    response: &serde_json::Value,
) -> Option<Vec<RecordingAlbumHint>> {
    use crate::recording_identity::{normalize_isrc, parse_title};
    use crate::spotify_mb_crosswalk::valid_mbid;
    let recordings = response["recordings"].as_array()?;
    if response["offset"].as_u64() != Some(0)
        || response["count"].as_u64() != Some(recordings.len() as u64)
        || recordings.len() > 12
    {
        return None;
    }
    let expected_version = track
        .version
        .as_deref()
        .or(title.version.as_deref())
        .map(key);
    let names = |row: &serde_json::Value| -> Option<Vec<String>> {
        row["artist-credit"]
            .as_array()?
            .iter()
            .map(|credit| credit["name"].as_str().map(key))
            .collect()
    };
    let expected_names = performers
        .iter()
        .map(|artist| key(artist))
        .collect::<Vec<_>>();
    let album = track.album.as_deref().map(key);
    let mut hints = Vec::new();
    for recording in recordings {
        let Some(recording_id) = recording["id"].as_str().filter(|id| valid_mbid(id)) else {
            continue;
        };
        let Some(credited_title) = recording["title"].as_str().map(parse_title) else {
            continue;
        };
        if key(&credited_title.title) != key(&title.title)
            || credited_title.version.as_deref().map(key) != expected_version
            || credited_title.soundtrack.as_deref().map(key) != title.soundtrack.as_deref().map(key)
            || credited_title
                .featured_credit
                .as_deref()
                .is_some_and(|guest| performers.get(1).is_none_or(|name| key(guest) != key(name)))
            || names(recording).as_ref() != Some(&expected_names)
            || isrc.is_some_and(|expected| {
                !recording["isrcs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|item| item.as_str().and_then(normalize_isrc).as_deref() == Some(expected))
            })
            || track.duration_seconds.is_some_and(|seconds| {
                !recording["length"]
                    .as_u64()
                    .is_some_and(|ms| (seconds - ms as f64 / 1000.0).abs() <= 2.0)
            })
        {
            continue;
        }
        let Some(artist_id) = recording["artist-credit"][0]["artist"]["id"]
            .as_str()
            .filter(|id| valid_mbid(id))
        else {
            continue;
        };
        let artist = ArtistRef {
            provider: "musicbrainz".into(),
            id: artist_id.to_owned(),
        };
        for release in recording["releases"].as_array().into_iter().flatten() {
            let Some(id) = release["id"].as_str().filter(|id| valid_mbid(id)) else {
                continue;
            };
            let group = &release["release-group"];
            let group_id = group["id"]
                .as_str()
                .filter(|id| valid_mbid(id))
                .unwrap_or(id);
            let original = release["status"] == "Official"
                && group["primary-type"] == "Album"
                && group["secondary-types"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
                && names(release).as_ref() == Some(&expected_names);
            let Some(release) = crate::artists::parse_musicbrainz_release(release, &artist) else {
                continue;
            };
            let priority = if album.as_ref() == Some(&key(&release.title)) {
                0
            } else if original {
                1
            } else {
                2
            };
            hints.push(RecordingAlbumHint {
                recording_id: recording_id.to_owned(),
                release,
                artist: artist.clone(),
                group: group_id.to_owned(),
                priority,
            });
        }
    }
    hints.sort_by_key(|hint| {
        (
            hint.priority,
            hint.group.clone(),
            hint.release
                .release_date
                .as_deref()
                .filter(|date| !date.is_empty())
                .unwrap_or("9999")
                .to_owned(),
            hint.release.release.id.clone(),
        )
    });
    hints.dedup_by(|a, b| {
        a.release.release == b.release.release && a.recording_id == b.recording_id
    });
    Some(hints)
}

async fn resolve_recording_albums(
    artists: &crate::artists::ArtistService,
    credentials: &crate::artists::ArtistCredentials,
    track: &Track,
) -> Option<AlbumResolution> {
    use crate::recording_identity::{
        declared_credit_names, normalize_isrc, parse_title, RecordingIdentity,
    };
    let title = parse_title(&track.title);
    let isrc = track.isrc.as_deref().and_then(normalize_isrc);
    if track.playback_range.is_some()
        || title.title.trim().is_empty()
        || title.title.chars().count() > 120
        || track.isrc.is_some() && isrc.is_none()
        || RecordingIdentity::from_track(track).identifier_conflict
        || !radio_without_duration(track)
            && isrc.is_none()
            && !track
                .duration_seconds
                .is_some_and(|seconds| seconds.is_finite() && seconds > 0.0)
    {
        return None;
    }
    let mut performers =
        declared_credit_names(track).unwrap_or_else(|| vec![track.artist.trim().to_owned()]);
    if let Some(guest) = &title.featured_credit {
        if !performers.iter().any(|name| key(name) == key(guest)) {
            performers.push(guest.clone());
        }
    }
    if performers.len() > 4
        || performers
            .iter()
            .any(|artist| artist.trim().is_empty() || artist.chars().count() > 120)
    {
        return None;
    }
    let response = artists
        .album_recording_search(&title.title, &performers, isrc.as_deref())
        .await
        .ok()?;
    let hints = recording_album_releases(track, &title, &performers, isrc.as_deref(), &response)?;
    let best = hints.first()?.priority;
    let candidates = hints
        .into_iter()
        .take_while(|hint| hint.priority == best)
        .collect::<Vec<_>>();
    let group_count = candidates
        .iter()
        .map(|hint| &hint.group)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let mut verified = std::collections::HashSet::new();
    let mut choices = Vec::new();
    for hint in candidates.into_iter().take(6) {
        if verified.contains(&hint.group) {
            continue;
        }
        let Ok(tracks) = artists
            .get_release_tracks(&hint.release, &hint.artist, credentials)
            .await
        else {
            continue;
        };
        if !verified_edition_tracks(&hint.release, &hint.group, &tracks)
            || tracks
                .iter()
                .filter(|track| track.provenance["recordingId"] == hint.recording_id)
                .count()
                != 1
        {
            continue;
        }
        let Some(release) = fetched_edition(hint.release, &tracks) else {
            continue;
        };
        verified.insert(hint.group);
        choices.push(AlbumChoice {
            release,
            artist: hint.artist,
            reason: "The catalog recording appears in the verified tracklist".into(),
            evidence: AlbumMembershipEvidence::RecordingOccurrence,
        });
    }
    // A recording match confirms membership, but a compilation alone does not
    // establish which album the listener intended.
    select_verified_albums(choices, group_count == 1 && best < 2)
}

fn same_album_timeline(
    source: &[crate::youtube_music::YouTubeMusicItem],
    catalog: &[Track],
) -> bool {
    if source.len() < 2 || source.len() != catalog.len() {
        return false;
    }
    let mut shortened_titles = 0;
    let aligned = source.iter().zip(catalog).all(|(left, right)| {
        let left_title = crate::recording_identity::parse_title(&left.title);
        let right_title = crate::recording_identity::parse_title(&right.title);
        let left_key = key(&left_title.title);
        let right_key = key(&right_title.title);
        let title_agrees = if left_key == right_key {
            !left_key.is_empty()
        } else {
            let (short, long) = if left_key.len() < right_key.len() {
                (&left_key, &right_key)
            } else {
                (&right_key, &left_key)
            };
            // A provider can use a shorter song label ("Cada Domingo").
            // This is navigation evidence only: at least nine other titles
            // and every ordered runtime must agree in a complete programme.
            let shortened = short.split_whitespace().count() >= 2
                && long.split_whitespace().count() >= 4
                && long.starts_with(&format!("{short} "));
            shortened_titles += usize::from(shortened);
            shortened
        };
        title_agrees
            && left_title.version.as_deref().map(key) == right_title.version.as_deref().map(key)
            && right.version.as_deref().is_none_or(|v| v.trim().is_empty())
            && left
                .duration_seconds
                .zip(right.duration_seconds)
                .is_some_and(|(a, b)| {
                    a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && (a - b).abs() <= 2.0
                })
    });
    aligned && (shortened_titles == 0 || source.len() >= 10 && shortened_titles == 1)
}

fn preferred_choice(choices: &[AlbumChoice], album: &str, incomplete: bool) -> Option<usize> {
    if incomplete {
        return None;
    }
    let timelines = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| choice.evidence == AlbumMembershipEvidence::CompleteTimeline)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    // A complete programme can select one edition even when this song also
    // occurs on other albums or an edition with a different running order.
    if timelines.len() == 1 {
        return Some(timelines[0]);
    }
    (choices.len() == 1 && key(&choices[0].release.title) == key(album)).then_some(0)
}

fn complete_primary_discography(page: &ArtistReleasePage, artist: &ArtistRef) -> bool {
    let coverage = page
        .source_coverage
        .iter()
        .filter(|source| source.provider == artist.provider)
        .collect::<Vec<_>>();
    let editions = page
        .items
        .iter()
        .filter(|release| release.release.provider == artist.provider)
        .collect::<Vec<_>>();
    artist.provider == "musicbrainz"
        && page.artist == *artist
        && page.next_cursor.is_none()
        && coverage.len() == 1
        && coverage[0].status == "available"
        && !editions.is_empty()
        && editions.iter().all(|release| {
            crate::spotify_mb_crosswalk::valid_mbid(&release.release.id)
                && release.source_url
                    == format!("https://musicbrainz.org/release/{}", release.release.id)
                && release
                    .credits
                    .iter()
                    .any(|credit| credit.artist.as_ref() == Some(artist))
        })
}

fn include_artist_editions(
    candidates: &mut Vec<ArtistReleaseRef>,
    page: ArtistReleasePage,
    artist: &ArtistRef,
) -> bool {
    let complete = complete_primary_discography(&page, artist);
    if complete {
        // Complete artist editions supersede broad search's duplicate group
        // representatives. Partial pages must retain those fallback locators.
        candidates.retain(|release| release.release.provider != artist.provider);
    }
    candidates.extend(page.items);
    complete
}

fn programme_count_conflicts(release: &ArtistReleaseRef, source_count: Option<usize>) -> bool {
    source_count
        .zip(release.track_count.filter(|count| *count > 0))
        .is_some_and(|(source, catalog)| source != catalog as usize)
}

fn complete_catalog_timeline(tracks: &[Track]) -> bool {
    !tracks.is_empty()
        && tracks.iter().all(|track| {
            track.provenance["libraryAlbumSnapshot"]["complete"] == true
                && track.provenance["libraryAlbumSnapshot"]["total"].as_u64()
                    == Some(tracks.len() as u64)
        })
}

// Discogs search summaries often expose only a display label of the form
// "artist - album". Use it to limit requests for full edition metadata.
fn uncredited_discogs_hit(release: &ArtistReleaseRef, album: &str, artist: &str) -> bool {
    release.release.provider == "discogs"
        && !release.credits.iter().any(|credit| credit.artist.is_some())
        && (key(&release.title) == key(album)
            || release.title.match_indices(" - ").any(|(i, separator)| {
                key(&release.title[..i]) == key(artist)
                    && key(&release.title[i + separator.len()..]) == key(album)
            }))
}

fn needs_tracklist(
    release: &ArtistReleaseRef,
    source_count: Option<usize>,
    programme_found: bool,
) -> bool {
    // Once a complete programme is verified, a known different-sized edition
    // cannot compete with it. Unknown counts and all possible full matches
    // still need inspection; weak occurrences are researched if no programme matches.
    !programme_found || !programme_count_conflicts(release, source_count)
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
    fn radio_song_can_open_its_album_without_a_duration() {
        let album = "Favourite Worst Nightmare";
        let member = Track {
            title: "505".into(),
            artist: "Arctic Monkeys".into(),
            version: None,
            duration_seconds: Some(253.0),
            ..Track::default()
        };
        let request = Track {
            provider: "radio_song".into(),
            album: Some(album.into()),
            duration_seconds: None,
            provenance: serde_json::json!({"sourceKind":"radioSong"}),
            ..member.clone()
        };
        assert!(radio_album_member(
            &request,
            album,
            None,
            std::slice::from_ref(&member)
        ));
        assert!(!recording_member(&request, std::slice::from_ref(&member)));
        assert!(!radio_album_member(
            &request,
            album,
            None,
            &[member.clone(), member.clone()]
        ));
        for unmatched in [
            Track {
                album: Some("Other album".into()),
                ..request.clone()
            },
            Track {
                artist: "Other artist".into(),
                ..request.clone()
            },
            Track {
                version: Some("Live".into()),
                ..request.clone()
            },
            Track {
                duration_seconds: Some(30.0),
                ..request.clone()
            },
            Track {
                provider: "import".into(),
                ..request.clone()
            },
        ] {
            assert!(!radio_album_member(
                &unmatched,
                album,
                None,
                std::slice::from_ref(&member)
            ));
        }
    }
    #[tokio::test]
    async fn radio_credit_finds_the_kalimankou_edition_and_verifies_the_recording_credit() {
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            crate::storage::Store::open(&temp.path().join("album.sqlite")).unwrap(),
        );
        let artists = crate::artists::ArtistService::new(store.clone()).unwrap();
        let credentials = crate::artists::ArtistCredentials::default();
        let release_id = "7da26152-ccc7-4e0c-9f75-28b5edbcaea5";
        let choir_id = "80fb9c75-af15-4a6b-9528-715f9ef4ebea";
        let song = "Kalimankou Denkou (The Evening Gathering)";
        let album = "Le Mystère des voix bulgares";
        let station_album = "Le Mystere Des voix Bulgares";
        let station_artist = "Bulgarian State Television Female Choir";
        let request = Track {
            provider: "radio_song".into(),
            title: song.into(),
            artist: station_artist.into(),
            album: Some(station_album.into()),
            provenance: json!({"sourceKind":"radioSong"}),
            ..Track::default()
        };
        let edition_credit = json!([{"name":station_artist,"artist":{"id":choir_id}}]);
        let group_id = "ac5b8d06-08ed-3060-8c6b-0880f16dff5b";
        let search = json!({"count":3,"offset":0,"releases":[
            {"id":release_id,"title":album,"artist-credit":edition_credit,"release-group":{"id":group_id},"date":"1987"},
            {"id":"00000000-0000-4000-8000-000000000013","title":album,"artist-credit":edition_credit,"release-group":{"id":group_id},"date":"1990"},
            {"id":"00000000-0000-4000-8000-000000000014","title":"Le Mystère des voix bulgares Live","artist-credit":edition_credit}
        ]});
        let query = format!("release:\"{station_album}\" AND creditname:\"{station_artist}\"");
        store
            .set_provider_cache(
                "navigation.credited-edition.musicbrainz.v2",
                &query,
                &search,
                60,
            )
            .unwrap();
        let recording = json!({
            "id":"df27ce01-abf3-4c10-afa2-fdfe2bfc5abf",
            "length":310000,
            "artist-credit":[{"name":"The Bulgarian State Radio and Television Female Vocal Choir","artist":{"id":choir_id}}]
        });
        let mut edition = json!({
            "id":release_id,"title":album,"artist-credit":edition_credit,"release-group":{"id":group_id},
            "media":[{"position":1,"track-count":1,"tracks":[
                {"position":1,"title":song,"recording":recording}
            ]}]
        });
        let cache = "artists.release-tracks.musicbrainz";
        store
            .set_provider_cache(cache, release_id, &edition, 60)
            .unwrap();
        let AlbumResolution::Resolved {
            destination: matched,
        } = resolve_credited_radio_edition(&artists, &credentials, &request, station_album)
            .await
            .unwrap()
        else {
            panic!("expected an exact verified edition")
        };
        assert_eq!(matched.release.release.id, release_id);
        assert_eq!(matched.artist.id, choir_id);
        assert!(matched.evidence == AlbumMembershipEvidence::RadioTracklistMatch);

        edition["media"][0]["tracks"][0]["recording"]["artist-credit"][0]["artist"]["id"] =
            json!("00000000-0000-4000-8000-000000000002");
        store
            .set_provider_cache(cache, release_id, &edition, 60)
            .unwrap();
        store
            .set_provider_cache(
                cache,
                "00000000-0000-4000-8000-000000000013",
                &json!({}),
                60,
            )
            .unwrap();
        assert!(
            resolve_credited_radio_edition(&artists, &credentials, &request, station_album)
                .await
                .is_none()
        );
    }
    #[tokio::test]
    async fn recording_fallback_verifies_diaraby_on_the_original_album() {
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            crate::storage::Store::open(&temp.path().join("fallback.sqlite")).unwrap(),
        );
        let artists = crate::artists::ArtistService::new(store.clone()).unwrap();
        let credentials = crate::artists::ArtistCredentials::default();
        let track = Track {
            provider: "radio_song".into(),
            title: "Diaraby  (w/ Ry Cooder)".into(),
            artist: "Ali Farka Touré".into(),
            album: Some("Ali Farka Touré".into()),
            provenance: json!({"sourceKind":"radioSong"}),
            ..Track::default()
        };
        let ali = "9be2a5ac-8201-489b-b5f6-91f958bf9060";
        let ry = "a4e2650b-938b-4b72-95bc-6e229cd34601";
        let recording = "b09bf481-3621-4c02-93d0-bf56a3eb3286";
        let other_recording = "f81e2bf5-4cc3-4476-bad4-f3dceadd146c";
        let release_id = "c1fb68f6-0183-49b0-beff-4f1c8b6b0b4a";
        let group = "a5a7a716-a039-3815-aee3-88ac135721c5";
        let credits = json!([
            {"name":"Ali Farka Touré","joinphrase":" with ","artist":{"id":ali}},
            {"name":"Ry Cooder","artist":{"id":ry}}
        ]);
        let album = json!({"id":release_id,"title":"Talking Timbuktu","status":"Official","artist-credit":credits,
            "release-group":{"primary-type":"Album","id":group}});
        let compilation = json!({"id":"c231d73d-655c-4765-b162-17cebd81e3b4","title":"Music for the New World",
            "status":"Official","release-group":{"primary-type":"Album","secondary-types":["Compilation"]}});
        let soundtrack = json!({"id":"192dcad3-7142-4cf8-b4b3-a754b25a7350","title":"Besieged","status":"Official",
            "release-group":{"primary-type":"Album","secondary-types":["Soundtrack"]}});
        let search = json!({"count":2,"offset":0,"recordings":[
            {"id":other_recording,"title":"Diaraby","artist-credit":credits,"releases":[soundtrack]},
            {"id":recording,"title":"Diaraby","artist-credit":credits,"releases":[compilation,album]}
        ]});
        store
            .set_provider_cache(
                "navigation.recording-search.musicbrainz.v1",
                "recording:\"Diaraby\" AND artist:\"Ali Farka Touré\" AND artist:\"Ry Cooder\"",
                &search,
                60,
            )
            .unwrap();
        let mut detail = album.clone();
        detail["media"] = json!([{"position":1,"track-count":1,"tracks":[
            {"position":1,"title":"Diaraby","recording":{"id":recording,"artist-credit":credits}}
        ]}]);
        let tracks_cache = "artists.release-tracks.musicbrainz";
        store
            .set_provider_cache(tracks_cache, release_id, &detail, 60)
            .unwrap();
        let Some(AlbumResolution::Resolved { destination }) =
            resolve_recording_albums(&artists, &credentials, &track).await
        else {
            panic!("expected the original verified album")
        };
        assert_eq!(destination.release.title, "Talking Timbuktu");
        // Only a verified option recorded for this track can become the saved destination.
        let fingerprint = cache_key(&track);
        assert!(cached_resolution(&store, &track).unwrap().is_none());
        cache_resolution(
            &store,
            &fingerprint,
            AlbumResolution::Ambiguous {
                choices: vec![destination.clone()],
            },
        )
        .unwrap();
        let mut with_art = track.clone();
        with_art.provenance["artworkAlternates"] = json!(["https://example.com/album.jpg"]);
        assert_eq!(cache_key(&with_art), fingerprint);
        let mut other = destination.release.release.clone();
        other.id = group.into();
        remember_choice(&store, &with_art, &other).unwrap();
        assert!(cached_resolution(&store, &track).unwrap().is_none());
        remember_choice(&store, &with_art, &destination.release.release).unwrap();
        assert!(cached_resolution(&store, &track).unwrap().is_some());

        detail["media"][0]["tracks"][0]["recording"]["id"] = json!(other_recording);
        store
            .set_provider_cache(tracks_cache, release_id, &detail, 60)
            .unwrap();
        assert!(resolve_recording_albums(&artists, &credentials, &track)
            .await
            .is_none());
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
    fn uncredited_discogs_search_result_keeps_exact_title_and_artist_candidates() {
        let release: ArtistReleaseRef = serde_json::from_value(serde_json::json!({
            "release":{"provider":"discogs","id":"29389585"},
            "title":"Antoine \"Tato\" Garcia - Rumbason",
            "sourceUrl":"https://www.discogs.com/release/29389585","credits":[]
        }))
        .unwrap();
        assert!(uncredited_discogs_hit(
            &release,
            "Rumbason",
            "Antoine Tato Garcia"
        ));
        assert!(!uncredited_discogs_hit(
            &release,
            "Rumbason",
            "Different artist"
        ));
        assert!(!uncredited_discogs_hit(
            &release,
            "Other album",
            "Antoine Tato Garcia"
        ));
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
                views: None,
                view_count_text: None,
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

    #[test]
    fn reported_album_programme_selects_cd_edition_despite_one_short_label() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/macha-album-navigation.json"
        ))
        .unwrap();
        let request: Track = serde_json::from_value(fixture["request"].clone()).unwrap();
        let source: Vec<crate::youtube_music::YouTubeMusicItem> =
            serde_json::from_value(fixture["sourceAlbum"]["items"].clone()).unwrap();
        let artist = ArtistRef {
            provider: "musicbrainz".into(),
            id: "d50b0808-1c23-43b2-b485-7248849268b2".into(),
        };
        let mut choices = Vec::new();
        let mut cd_tracks = Vec::new();
        for (index, raw) in fixture["releases"].as_array().unwrap().iter().enumerate() {
            let id = raw["id"].as_str().unwrap();
            let url = format!("https://musicbrainz.org/release/{id}");
            let tracks = crate::artists::mark_library_album_snapshot(
                crate::artists::parse_musicbrainz_release_tracks(raw, id, &url, false, None),
                Some(14),
            );
            assert_eq!(tracks.len(), 14);
            assert_eq!(
                tracks[2].album_artist.as_deref(),
                Some("Macha y el Bloque Depresivo")
            );
            // The album's artist must remain distinct from recording-line credits.
            assert_eq!(tracks[2].provenance["artistCredits"][0]["name"], "Macha");
            assert!(recording_member(&request, &tracks));
            let aligned =
                complete_catalog_timeline(&tracks) && same_album_timeline(&source, &tracks);
            assert_eq!(
                aligned,
                index == 0,
                "The digital edition reverses the last two songs"
            );
            let release: ArtistReleaseRef = serde_json::from_value(serde_json::json!({
                "release":{"provider":"musicbrainz","id":id},"title":raw["title"],
                "sourceUrl":url,"credits":[],"tracks":[]
            }))
            .unwrap();
            choices.push(AlbumChoice {
                release,
                artist: artist.clone(),
                reason: String::new(),
                evidence: if aligned {
                    AlbumMembershipEvidence::CompleteTimeline
                } else {
                    AlbumMembershipEvidence::RecordingOccurrence
                },
            });
            if index == 0 {
                cd_tracks = tracks;
            }
        }
        assert_eq!(
            preferred_choice(&choices, request.album.as_deref().unwrap(), false),
            Some(0)
        );
        assert_eq!(
            preferred_choice(&choices, request.album.as_deref().unwrap(), true),
            None
        );
        choices[1].evidence = AlbumMembershipEvidence::CompleteTimeline;
        assert_eq!(
            preferred_choice(&choices, request.album.as_deref().unwrap(), false),
            None
        );
        assert!(
            !same_album_timeline(&source[..9], &cd_tracks[..9]),
            "Short releases need exact titles"
        );
        let mut second_short_label = source.clone();
        second_short_label[12].title = "Lo Que".into();
        assert!(!same_album_timeline(&second_short_label, &cd_tracks));
        let mut wrong_song = source.clone();
        wrong_song[8].title = "An unrelated song".into();
        assert!(!same_album_timeline(&wrong_song, &cd_tracks));
        let mut wrong_version = source;
        wrong_version[8].title.push_str(" (Live)");
        assert!(!same_album_timeline(&wrong_version, &cd_tracks));
    }

    #[test]
    fn complete_catalogue_programme_requires_every_normalized_snapshot_row() {
        assert!(!complete_catalog_timeline(&[]));
        let row = Track {
            provenance: serde_json::json!({"libraryAlbumSnapshot":{"complete":true,"total":2}}),
            ..Default::default()
        };
        let mut tracks = vec![row.clone(), row];
        assert!(complete_catalog_timeline(&tracks));
        assert!(!complete_catalog_timeline(&tracks[..1]));
        tracks[1].provenance["libraryAlbumSnapshot"]["complete"] = false.into();
        assert!(!complete_catalog_timeline(&tracks));
        tracks[1].provenance = serde_json::json!({});
        assert!(!complete_catalog_timeline(&tracks));
    }

    #[test]
    fn complete_discography_avoids_redundant_groups_and_researches_possible_programmes_first() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/macha-album-navigation.json"
        ))
        .unwrap();
        let page: ArtistReleasePage =
            serde_json::from_value(fixture["discography"].clone()).unwrap();
        let artist = page.artist.clone();
        assert!(complete_primary_discography(&page, &artist));
        let mut group = page.items[5].clone();
        group.release.id = "830f149f-3395-412b-8df4-21c2443f8c0e".into();
        group.source_url = format!("https://musicbrainz.org/release-group/{}", group.release.id);
        group.track_count = None;
        let mut candidates = vec![group.clone()];
        assert!(include_artist_editions(
            &mut candidates,
            page.clone(),
            &artist
        ));
        assert_eq!(candidates.len(), 9);
        assert!(candidates
            .iter()
            .all(|release| !release.source_url.contains("release-group/")));
        candidates.sort_by_key(|release| programme_count_conflicts(release, Some(14)));
        assert_eq!(
            candidates[0].release.id,
            "6c56fb1d-e220-40ee-b836-10b6bf992ab7"
        );
        assert_eq!(
            candidates[1].release.id,
            "7b337019-a478-41ad-bf5e-2b4ac1171f83"
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|release| needs_tracklist(release, Some(14), true))
                .count(),
            2
        );
        assert!(
            needs_tracklist(&group, Some(14), true),
            "Unknown counts still need inspection"
        );
        group.track_count = Some(0);
        assert!(needs_tracklist(&group, Some(14), true));
        assert!(candidates
            .iter()
            .all(|release| needs_tracklist(release, Some(14), false)));
        assert!(candidates
            .iter()
            .all(|release| needs_tracklist(release, None, true)));
        for next_cursor in [Some("r3:100:x".to_owned()), None] {
            let mut incomplete = page.clone();
            incomplete.next_cursor = next_cursor;
            if incomplete.next_cursor.is_none() {
                incomplete
                    .source_coverage
                    .iter_mut()
                    .find(|source| source.provider == "musicbrainz")
                    .unwrap()
                    .status = "partial".into();
            }
            let mut fallback = vec![group.clone()];
            assert!(!include_artist_editions(&mut fallback, incomplete, &artist));
            assert_eq!(fallback.len(), 10);
            assert!(fallback
                .iter()
                .any(|release| release.source_url.contains("release-group/")));
        }
        let mut wrong_artist = artist.clone();
        wrong_artist.id = "00000000-0000-4000-8000-000000000000".into();
        assert!(!complete_primary_discography(&page, &wrong_artist));
        let mut unknown_artist = page.clone();
        unknown_artist.items[0].credits.clear();
        assert!(!complete_primary_discography(&unknown_artist, &artist));
        let mut unbound = page;
        unbound.items[0].source_url =
            "https://musicbrainz.org/release-group/830f149f-3395-412b-8df4-21c2443f8c0e".into();
        assert!(!complete_primary_discography(&unbound, &artist));
    }
}
fn exact_spotify_album_request(track: &Track) -> bool {
    track.provider == "spotify"
        && track.provider_id.len() == 22
        && track
            .provider_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        && track.id == format!("spotify:{}", track.provider_id)
        && track.source_url == format!("https://open.spotify.com/track/{}", track.provider_id)
        && track.playback_range.is_none()
        && ![
            "title",
            "artist",
            "album",
            "albumArtist",
            "version",
            "durationSeconds",
            "isrc",
        ]
        .iter()
        .any(|field| track.provenance["manualFields"][field] == true)
}

pub async fn resolve_exact_spotify(
    state: &crate::AppState,
    track: &Track,
) -> Result<AlbumResolution, String> {
    if exact_spotify_album_request(track) {
        let fresh = state
            .remote_playlists
            .get_remote_track("spotify", &track.provider_id)
            .await?;
        let context = &fresh.provenance["recordingContext"];
        if let Some(album_id) = context["albumId"].as_str().filter(|id| {
            id.len() == 22
                && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                && context["albumUri"] == format!("spotify:album:{id}")
        }) {
            return Ok(AlbumResolution::Spotify {
                album_id: album_id.into(),
                title: fresh
                    .album
                    .unwrap_or_else(|| track.album.clone().unwrap_or_default()),
            });
        }
    }
    Ok(AlbumResolution::Unavailable {
        reason: "Spotify did not provide an album for this track".into(),
        source_album_url: None,
    })
}

pub(crate) fn cached_resolution(
    store: &crate::storage::Store,
    track: &Track,
) -> Result<Option<AlbumResolution>, String> {
    let value = store.get_provider_cache(MEMBERSHIP_CACHE, &cache_key(track))?;
    Ok(value.and_then(|value| match serde_json::from_value(value) {
        Ok(result @ AlbumResolution::Resolved { .. }) => Some(result),
        _ => None,
    }))
}

pub(crate) fn remember_choice(
    store: &crate::storage::Store,
    track: &Track,
    release: &ArtistRef,
) -> Result<(), String> {
    let key = cache_key(track);
    let Some(value) = store.get_provider_cache(MEMBERSHIP_CHOICES_CACHE, &key)? else {
        return Ok(());
    };
    if let Ok(AlbumResolution::Ambiguous { choices }) = serde_json::from_value(value) {
        if let Some(destination) = choices
            .into_iter()
            .find(|choice| choice.release.release == *release)
        {
            cache_resolution(store, &key, AlbumResolution::Resolved { destination })?;
        }
    }
    Ok(())
}

pub async fn resolve(state: &crate::AppState, track: &Track) -> Result<AlbumResolution, String> {
    let membership_key = cache_key(track);
    if let Some(result) = cached_resolution(&state.store, track)? {
        return Ok(result);
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
    let credentials = crate::artist_credentials(state.connections.clone()).await?;
    let mut recording_searched = false;
    let mut recording_fallback = None;
    if radio_without_duration(track) || album.is_empty() {
        if !album.is_empty() {
            if let Some(result) =
                resolve_credited_radio_edition(&state.artists, &credentials, track, album).await
            {
                return cache_resolution(&state.store, &membership_key, result);
            }
        }
        recording_searched = true;
        if let Some(result) = resolve_recording_albums(&state.artists, &credentials, track).await {
            if album.is_empty() || matches!(result, AlbumResolution::Resolved { .. }) {
                return cache_resolution(&state.store, &membership_key, result);
            }
            recording_fallback = Some(result);
        }
    }
    if album.is_empty() {
        return Ok(AlbumResolution::Unavailable {
            reason: "Album metadata is unavailable for this track".into(),
            source_album_url,
        });
    }
    let artist = album_artist(track);
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
            if let Some(result) = recording_fallback {
                return cache_resolution(&state.store, &membership_key, result);
            }
            if !recording_searched {
                if let Some(result) =
                    resolve_recording_albums(&state.artists, &credentials, track).await
                {
                    return cache_resolution(&state.store, &membership_key, result);
                }
            }
            return Ok(AlbumResolution::Unavailable {
                reason: "Album catalogs are temporarily unavailable".into(),
                source_album_url,
            });
        }
    };
    // Preserve catalog ambiguity; do not silently pick an edition from page one.
    let mut choices = Vec::new();
    let mut candidates = page.items;
    let mut catalog_artist = None;
    let mut discovery_incomplete = page.next_page.is_some();
    let exact_album_found = candidates.iter().any(|release| {
        key(&release.title) == key(album)
            && release
                .credits
                .iter()
                .any(|credit| key(&credit.name) == key(&artist))
    });
    if exact_album_found {
        candidates.retain(|release| {
            key(&release.title) == key(album) || uncredited_discogs_hit(release, album, &artist)
        });
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
                catalog_artist = Some(exact[0].artist.clone());
                if let Ok(releases) = state
                    .artists
                    .get_artist_releases(&exact[0].artist, None, &credentials)
                    .await
                {
                    discovery_incomplete |= releases.next_cursor.is_some()
                        || releases.source_coverage.iter().any(|source| {
                            matches!(source.status.as_str(), "partial" | "unavailable")
                        });
                    let complete =
                        include_artist_editions(&mut candidates, releases, &exact[0].artist);
                    if complete
                        && credentials
                            .discogs_token
                            .as_deref()
                            .is_none_or(|token| token.trim().is_empty())
                    {
                        discovery_incomplete = false;
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|release| {
        (
            key(&release.title) != key(album) && !uncredited_discogs_hit(release, album, &artist),
            release.release.provider.clone(),
            release.release.id.clone(),
        )
    });
    candidates.dedup_by(|a, b| a.release == b.release);
    let source_count = source_timeline.as_ref().map(Vec::len);
    candidates.sort_by_key(|release| programme_count_conflicts(release, source_count));
    let candidates_truncated = candidates.len() > 20;
    for mut release in candidates.into_iter().take(20) {
        if !needs_tracklist(
            &release,
            source_count,
            choices.iter().any(|choice: &AlbumChoice| {
                choice.evidence == AlbumMembershipEvidence::CompleteTimeline
            }),
        ) {
            continue;
        }
        let detailed_tracks = if uncredited_discogs_hit(&release, album, &artist) {
            let Ok((edition, tracks)) = state
                .artists
                .discogs_navigation_edition(release, &credentials)
                .await
            else {
                continue;
            };
            if key(&edition.title) != key(album) || !complete_catalog_timeline(&tracks) {
                continue;
            }
            release = edition;
            Some(tracks)
        } else {
            None
        };
        let Some(credit) = release.credits.iter().find(|credit| {
            match catalog_artist
                .as_ref()
                .filter(|known| known.provider == release.release.provider)
            {
                Some(known) => credit.artist.as_ref() == Some(known),
                None => key(&credit.name) == key(&artist),
            }
        }) else {
            continue;
        };
        let Some(artist_ref) = credit.artist.clone() else {
            continue;
        };
        let tracks = if let Some(tracks) = detailed_tracks {
            tracks
        } else {
            let Ok(tracks) = state
                .artists
                .get_release_tracks(&release, &artist_ref, &credentials)
                .await
            else {
                continue;
            };
            tracks
        };
        let timeline_matches = complete_catalog_timeline(&tracks)
            && source_timeline
                .as_ref()
                .is_some_and(|source| same_album_timeline(source, &tracks));
        if release
            .track_count
            .is_some_and(|count| count as usize != tracks.len())
        {
            continue;
        }
        let (evidence, reason) = if timeline_matches {
            (
                AlbumMembershipEvidence::CompleteTimeline,
                "Complete ordered source and catalog album timelines agree",
            )
        } else if recording_member(track, &tracks) {
            (
                AlbumMembershipEvidence::RecordingOccurrence,
                "Matching recording in the independently fetched release tracklist",
            )
        } else if radio_album_member(track, &release.title, Some(&artist_ref), &tracks) {
            (
                AlbumMembershipEvidence::RadioTracklistMatch,
                "Station metadata matches a single song on this album",
            )
        } else {
            continue;
        };
        let Some(release) = fetched_edition(release, &tracks) else {
            continue;
        };
        choices.push(AlbumChoice {
            release,
            artist: artist_ref,
            reason: reason.into(),
            evidence,
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
    if choices.is_empty() {
        if let Some(result) = recording_fallback {
            return cache_resolution(&state.store, &membership_key, result);
        }
    }
    if choices.is_empty() && !recording_searched {
        if let Some(result) = resolve_recording_albums(&state.artists, &credentials, track).await {
            return cache_resolution(&state.store, &membership_key, result);
        }
    }
    let result = if let Some(index) = preferred_choice(
        &choices,
        album,
        discovery_incomplete || candidates_truncated,
    ) {
        AlbumResolution::Resolved {
            destination: choices.remove(index),
        }
    } else if !choices.is_empty() {
        AlbumResolution::Ambiguous { choices }
    } else {
        AlbumResolution::Unavailable {
            reason: "No verified album membership was found".into(),
            source_album_url,
        }
    };
    cache_resolution(&state.store, &membership_key, result)
}

fn cache_resolution(
    store: &crate::storage::Store,
    membership_key: &str,
    result: AlbumResolution,
) -> Result<AlbumResolution, String> {
    let (namespace, ttl) = match &result {
        AlbumResolution::Resolved { .. } => (MEMBERSHIP_CACHE, MEMBERSHIP_TTL),
        AlbumResolution::Ambiguous { .. } => (MEMBERSHIP_CHOICES_CACHE, 30 * 60),
        _ => return Ok(result),
    };
    store.set_provider_cache(
        namespace,
        membership_key,
        &serde_json::to_value(&result).map_err(|e| e.to_string())?,
        ttl,
    )?;
    Ok(result)
}
