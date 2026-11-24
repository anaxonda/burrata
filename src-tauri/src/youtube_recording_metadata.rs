//! Keep Music's linked recording artists separate from untyped extractor credits.
//! A caption or a verified channel alone does not establish an album recording.
use crate::models::{Candidate, Track};
use serde_json::{json, Value};

fn text(value: &Value, max: usize) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.chars().count() <= max && !v.chars().any(char::is_control))
}
fn key(value: &str) -> String {
    crate::recording_identity::search_fold(value)
}
fn duration(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0. && *v <= 86_400.)
}
pub(crate) fn channel_id(value: &str) -> bool {
    value.len() == 24
        && value.starts_with("UC")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

/// This context is an inspection hint until the selected video is freshly read.
/// Matching album/title/runtime and atomic linked artist names are all required.
pub(crate) fn catalog_album_artists(candidate: &Candidate) -> Option<Vec<String>> {
    let meta = candidate.source_metadata.as_ref()?;
    let id = candidate.id.strip_prefix("youtube:")?;
    if candidate.provider != "youtube"
        || id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        || meta["provider"] != "youtube"
        || meta["playbackMusicCatalogCandidate"] != true
        || meta["catalogMetadataConflict"] == true
    {
        return None;
    }
    text(&meta["catalogAlbum"], 512)?;
    text(&meta["catalogRequestedAlbum"], 512)?;
    let source = duration(&meta["catalogDurationSeconds"])?;
    if (source - duration(&meta["catalogRequestedDurationSeconds"])?).abs() > 1.5 {
        return None;
    }
    let browse = text(&meta["albumBrowseId"], 128)?;
    if !browse.starts_with("MPREb_")
        || !browse
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    let rows = meta["catalogArtistCredits"].as_array()?;
    if rows.is_empty() || rows.len() > 32 {
        return None;
    }
    let mut names = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        let name = text(&row["name"], 256)?;
        let id = text(&row["providerId"], 24)?;
        if !channel_id(id) || !ids.insert(id) || names.iter().any(|n: &String| key(n) == key(name))
        {
            return None;
        }
        names.push(name.to_owned());
    }
    if key(&names.join(", ")) != key(text(&meta["catalogArtist"], 8192)?) {
        return None;
    }
    text(&meta["catalogTitle"], 512)?;
    Some(names)
}

/// Research the exact album's linked item before a caption-only upload. This
/// grants extraction priority, never permission to skip fresh source verification.
pub(crate) fn preferred_album_locator(track: &Track, candidate: &Candidate) -> bool {
    let (Some(album), Some(meta)) = (track.album.as_deref(), candidate.source_metadata.as_ref())
    else {
        return false;
    };
    if key(album) != key(meta["catalogAlbum"].as_str().unwrap_or_default()) {
        return false;
    }
    preferred_recording_locator(track, candidate)
        || preferred_numbered_movement_locator(track, candidate, album, meta)
}

fn preferred_numbered_movement_locator(
    track: &Track,
    candidate: &Candidate,
    album: &str,
    meta: &Value,
) -> bool {
    let Some((request_album, fingerprint)) =
        crate::spotify_catalog_identity::numbered_movement_album_request(track)
    else {
        return false;
    };
    if request_album != album
        || meta["catalogRequestedAlbum"].as_str() != Some(album)
        || meta["catalogAlbum"].as_str() != Some(album)
        || meta["catalogRequestedDurationSeconds"].as_f64() != track.duration_seconds
    {
        return false;
    }
    let Some(names) = catalog_album_artists(candidate) else {
        return false;
    };
    let Some(title) = text(&meta["catalogTitle"], 512) else {
        return false;
    };
    let Some(runtime) = duration(&meta["catalogDurationSeconds"]) else {
        return false;
    };
    let expected = fingerprint
        .artists
        .iter()
        .map(|artist| key(&artist.name))
        .collect::<std::collections::HashSet<_>>();
    names.len() == expected.len()
        && names.iter().all(|name| expected.contains(&key(name)))
        && crate::spotify_catalog_identity::numbered_movement_caption_matches(
            &fingerprint,
            title,
            runtime,
        )
}

/// A compilation may reuse a recording whose canonical video belongs to a
/// single. Keep both release labels and the request scope; do not equate albums.
pub(crate) fn preferred_recording_locator(track: &Track, candidate: &Candidate) -> bool {
    let Some(names) = catalog_album_artists(candidate) else {
        return false;
    };
    let Some(meta) = candidate.source_metadata.as_ref() else {
        return false;
    };
    let identity = crate::recording_identity::RecordingIdentity::from_track(track);
    if identity.identifier_conflict
        || identity.credits_uncertain
        || key(&track.title) != key(meta["catalogTitle"].as_str().unwrap_or_default())
        || track.album.as_deref().is_none_or(|album| {
            key(album) != key(meta["catalogRequestedAlbum"].as_str().unwrap_or_default())
        })
        || track
            .duration_seconds
            .zip(duration(&meta["catalogRequestedDurationSeconds"]))
            .is_none_or(|(a, b)| !a.is_finite() || (a - b).abs() > 0.001)
    {
        return false;
    }
    let mut requested = identity
        .credits
        .iter()
        .filter(|c| crate::recording_identity::is_performer_credit_role(&c.role))
        .map(|c| key(&c.name))
        .collect::<Vec<_>>();
    let mut observed = names.iter().map(|n| key(n)).collect::<Vec<_>>();
    requested.sort();
    observed.sort();
    !requested.is_empty() && requested == observed
}

pub(crate) fn catalog_context_key(candidate: &Candidate) -> Option<Value> {
    catalog_album_artists(candidate)?;
    let m = candidate.source_metadata.as_ref()?;
    Some(json!([
        "music-album-credits-2",
        candidate.id,
        key(m["catalogTitle"].as_str()?),
        key(m["catalogAlbum"].as_str()?),
        key(m["catalogRequestedAlbum"].as_str()?),
        m["catalogDurationSeconds"],
        m["catalogRequestedDurationSeconds"],
        m["albumBrowseId"],
        m["catalogArtistCredits"]
    ]))
}

pub(crate) fn catalog_context_matches_request(track: &Track, metadata: &Value) -> bool {
    if metadata["artistCreditSource"] != "youtubeMusicCatalogVerifiedAlbum" {
        return true;
    }
    track
        .album
        .as_deref()
        .zip(text(&metadata["catalogRequestedAlbum"], 512))
        .is_some_and(|(a, b)| key(a) == key(b))
        && track
            .duration_seconds
            .zip(duration(&metadata["catalogRequestedDurationSeconds"]))
            .is_some_and(|(a, b)| a.is_finite() && (a - b).abs() <= 0.001)
}

/// Read the canonical recording credit line from fresh distributor metadata.
/// The rest of the description can list contributors in other roles.
pub(crate) fn generated_recording_credits<'a>(
    candidate: &Candidate,
    info: &'a Value,
) -> Option<(&'a str, Vec<String>)> {
    let id = candidate.id.strip_prefix("youtube:")?;
    if candidate.provider != "youtube"
        || id.len() != 11
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || info["id"].as_str() != Some(id)
        || info["channel_is_verified"] != true
        || !info["channel_id"].as_str().is_some_and(channel_id)
    {
        return None;
    }
    text(&info["channel"], 200)?;
    let title = text(&info["title"], 512)?;
    if text(&info["track"], 512)? != title {
        return None;
    }
    duration(&info["duration"])?;
    let description = info["description"].as_str()?;
    if description.len() > 32_768 {
        return None;
    }
    let mut lines = description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let distributor = lines.next()?.strip_prefix("Provided to YouTube by ")?;
    if distributor.trim().is_empty()
        || distributor.chars().count() > 256
        || distributor.chars().any(char::is_control)
    {
        return None;
    }
    let credit_line = lines.next()?;
    if lines.last() != Some("Auto-generated by YouTube.") {
        return None;
    }
    let rows = info["artists"].as_array()?;
    if rows.is_empty() || rows.len() > 32 {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let artists = rows
        .iter()
        .map(|row| {
            let artist = text(row, 256)?;
            let key = crate::matching::normalize_single_artist_name(artist);
            (!key.is_empty() && seen.insert(key)).then(|| artist.to_owned())
        })
        .collect::<Option<Vec<_>>>()?;
    (credit_line == format!("{title} · {}", artists.join(" · "))).then_some((distributor, artists))
}

/// Preserve exact roles from a release header already validated by
/// generated_recording_credits. An uploader name never classifies a role.
pub(crate) fn generated_remix_role_rows(
    info: &Value,
    artists: &[String],
) -> Option<(String, Vec<Value>)> {
    if !remix_scalar_agrees(&info["artist"], &artists.join(", ")) {
        return None;
    }
    let date = text(&info["release_date"], 8)?;
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(date, "%Y%m%d")
        .ok()?
        .to_string();
    let mut footer = info["description"]
        .as_str()?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .skip(3);
    footer.next()?.strip_prefix("℗ ")?;
    if footer.next()?.strip_prefix("Released on: ")? != date {
        return None;
    }
    let mut roles = vec![None; artists.len()];
    let mut remixers = 0;
    loop {
        let row = footer.next()?;
        if row == "Auto-generated by YouTube." {
            break;
        }
        let (label, name) = row.split_once(": ")?;
        let role = match label {
            "Vocalist" | "Vocalist, Composer Lyricist" | "Composer Lyricist, Vocalist" => "artist",
            "Remixer" => {
                remixers += 1;
                "remixer"
            }
            _ => return None,
        };
        let index = artists.iter().position(|artist| artist == name)?;
        if roles[index].replace(role).is_some() {
            return None;
        }
    }
    if footer.next().is_some() || remixers != 1 || artists.len() < 2 {
        return None;
    }
    let rows = artists
        .iter()
        .zip(roles)
        .map(|(name, role)| Some(json!({"name":name,"role":role?})))
        .collect::<Option<Vec<_>>>()?;
    Some((date, rows))
}

fn remix_scalar_agrees(value: &Value, joined: &str) -> bool {
    let key = crate::matching::normalize_single_artist_name;
    value.is_null() || text(value, 4096).is_some_and(|name| key(name) == key(joined))
}

/// Use explicit publisher roles only for the same freshly resolved named remix.
/// Keep the flat extractor fields and their entire credit vector intact.
pub(crate) fn generated_named_remix_performers(
    track: &Track,
    candidate: &Candidate,
    metadata: &Value,
    expected: &[String],
    requested_remixer: &str,
) -> Option<Vec<String>> {
    let id = candidate.id.strip_prefix("youtube:")?;
    if candidate.provider != "youtube"
        || candidate.url != format!("https://www.youtube.com/watch?v={id}")
        || id.len() != 11
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || metadata["provider"] != "youtube"
        || metadata["resolvedProviderId"].as_str() != Some(id)
        || metadata["sourceMetadataOrigin"] != "ytDlpFullInfo"
        || metadata["channelIsVerified"] != true
        || !metadata["channelId"].as_str().is_some_and(channel_id)
        || metadata["identityMetadataUncertain"] == true
        || metadata["track"].as_str() != Some(candidate.title.as_str())
        || (!metadata["catalogTitle"].is_null() && metadata["catalogTitle"] != candidate.title)
        || duration(&metadata["duration"]) != candidate.duration_seconds
        || metadata["isPreview"] == true
        || metadata["isFullAlbum"] == true
        || candidate
            .chapters
            .as_ref()
            .is_some_and(|rows| !rows.is_empty())
        || track.playback_range.is_some()
    {
        return None;
    }
    text(&metadata["youtubeGeneratedDistributor"], 256)?;
    let album = text(&metadata["youtubeGeneratedAlbum"], 512)?;
    let date = text(&metadata["youtubeGeneratedReleaseDate"], 10)?;
    if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        || track
            .album
            .as_deref()
            .is_some_and(|requested| key(album) != key(requested))
        || (track.provenance["manualFields"]["releaseDate"] == true
            && track
                .release_date
                .as_deref()
                .is_some_and(|requested| requested != date))
        || (!metadata["album"].is_null() && text(&metadata["album"], 512) != Some(album))
        || (!metadata["topicReleaseDate"].is_null()
            && text(&metadata["topicReleaseDate"], 10) != Some(date))
    {
        return None;
    }
    let artists = metadata["artists"].as_array()?;
    let roles = metadata["youtubeGeneratedArtistRoles"].as_array()?;
    if !(2..=8).contains(&artists.len())
        || artists.len() != roles.len()
        || metadata["youtubeGeneratedArtistCredits"] != metadata["artists"]
        || (!metadata["artistNames"].is_null() && metadata["artistNames"] != metadata["artists"])
    {
        return None;
    }
    let names = artists
        .iter()
        .map(|name| text(name, 256))
        .collect::<Option<Vec<_>>>()?;
    let joined = names.join(", ");
    if ["artist", "catalogArtist"]
        .iter()
        .any(|field| !remix_scalar_agrees(&metadata[*field], &joined))
    {
        return None;
    }
    // Raw role names and arrays must already agree exactly. Accent folding
    // here only checks that those slots map one-to-one to the request.
    let artist_key = crate::recording_identity::artist_search_fold;
    let mut seen = std::collections::HashSet::new();
    let mut performer_keys = std::collections::HashSet::new();
    let mut performers = Vec::new();
    let mut found_remixer = false;
    for (row, name) in roles.iter().zip(names) {
        let key = artist_key(name);
        if row["name"].as_str() != Some(name) || key.is_empty() || !seen.insert(key.clone()) {
            return None;
        }
        match row["role"].as_str()? {
            "artist" => {
                performer_keys.insert(key);
                performers.push(name.to_owned());
            }
            "remixer"
                if !found_remixer && crate::matching::normalize(name) == requested_remixer =>
            {
                found_remixer = true;
            }
            _ => return None,
        }
    }
    let expected_keys = expected
        .iter()
        .map(|name| artist_key(name))
        .collect::<std::collections::HashSet<_>>();
    (found_remixer && expected.len() == expected_keys.len() && performer_keys == expected_keys)
        .then_some(performers)
}

/// Bind the distributor credits to the same typed Music song row, album,
/// duration and linked performer channel. A Topic caption alone is not proof.
pub(crate) fn corroborated_topic_credits(
    candidate: &Candidate,
    info: &Value,
    metadata: &serde_json::Map<String, Value>,
) -> Option<(String, String, Vec<String>)> {
    let names = catalog_album_artists(candidate)?;
    let catalog = candidate.source_metadata.as_ref()?;
    let title = text(&info["track"], 512)?;
    let display_title = text(&info["title"], 512)?;
    let same_display_title = display_title == title
        || display_title
            .strip_suffix("...")
            .or_else(|| display_title.strip_suffix('…'))
            .is_some_and(|prefix| prefix.chars().count() >= 64 && title.starts_with(prefix));
    let album = text(metadata.get("topicAlbum")?, 512)?;
    let composer = text(metadata.get("topicComposer")?, 256)?;
    let runtime = duration(&info["duration"])?;
    let channel = text(metadata.get("channelId")?, 24)?;
    if !title.split_once(": ").is_some_and(|(prefix, _)| {
        crate::spotify_catalog_identity::name_matches_caption_prefix(prefix, composer)
    }) {
        return None;
    }
    let artists = metadata
        .get("artists")?
        .as_array()?
        .iter()
        .map(|row| text(row, 256))
        .collect::<Option<Vec<_>>>()?;
    if !candidate
        .id
        .strip_prefix("youtube:")
        .is_some_and(|id| info["id"].as_str() == Some(id))
        || !same_display_title
        || text(&catalog["catalogTitle"], 512)? != title
        || text(&catalog["catalogAlbum"], 512)? != album
        || text(&catalog["catalogRequestedAlbum"], 512)? != album
        || (runtime - duration(&catalog["catalogDurationSeconds"])?).abs() > 1.5
        || (runtime - duration(&catalog["catalogRequestedDurationSeconds"])?).abs() > 1.5
        || !catalog["catalogArtistCredits"]
            .as_array()?
            .iter()
            .any(|row| {
                row["providerId"].as_str() == Some(channel)
                    && text(&row["name"], 256).is_some_and(|name| artists.contains(&name))
            })
        || !text(metadata.get("channel")?, 200)?
            .strip_suffix(" - Topic")
            .is_some_and(|channel_name| artists.contains(&channel_name))
    {
        return None;
    }
    let description = info["description"]
        .as_str()
        .filter(|value| value.len() <= 32_768)?;
    let mut lines = description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let distributor = lines.next()?.strip_prefix("Provided to YouTube by ")?;
    let credit_line = format!("{title} · {}", artists.join(" · "));
    if distributor.trim().is_empty()
        || lines.next()? != credit_line
        || lines.next()? != album
        || lines.last()? != "Auto-generated by YouTube."
    {
        return None;
    }
    let credit_count = artists.len();
    let mut reported = artists
        .into_iter()
        .map(key)
        .collect::<std::collections::HashSet<_>>();
    if reported.len() != credit_count {
        return None;
    }
    reported.insert(key(composer));
    if reported.len() != names.len() || names.iter().any(|name| !reported.contains(&key(name))) {
        return None;
    }
    Some((album.to_owned(), composer.to_owned(), names))
}

/// A verified album row can omit a credit that appears in the distributor's
/// recording line. Use the expanded list only if it names the entire request
/// and preserves every linked credit on the same album.
pub(crate) fn generated_album_credits_for_request(
    track: &Track,
    candidate: &Candidate,
    metadata: &Value,
    expected: &[String],
    linked: &[String],
) -> Option<Vec<String>> {
    if candidate.provider != "youtube"
        || metadata["provider"] != "youtube"
        || metadata["sourceMetadataOrigin"] != "ytDlpFullInfo"
        || metadata["artistCreditSource"] != "youtubeMusicCatalogVerifiedAlbum"
        || metadata["channelIsVerified"] != true
        || metadata["identityMetadataUncertain"] == true
        || !candidate
            .id
            .strip_prefix("youtube:")
            .is_some_and(|id| metadata["resolvedProviderId"].as_str() == Some(id))
        || !catalog_context_matches_request(track, metadata)
        || track
            .album
            .as_deref()
            .zip(text(&metadata["catalogAlbum"], 512))
            .is_none_or(|(requested, source)| key(requested) != key(source))
        || linked.is_empty()
    {
        return None;
    }
    text(&metadata["youtubeGeneratedDistributor"], 256)?;
    let credits = metadata["youtubeGeneratedArtistCredits"]
        .as_array()?
        .iter()
        .map(|row| text(row, 256).map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    if credits.len() <= linked.len() || credits.len() > 32 {
        return None;
    }
    let artist_key = |name: &String| crate::matching::normalize_single_artist_name(name);
    let mut expected = expected.iter().map(artist_key).collect::<Vec<_>>();
    let mut actual = credits.iter().map(artist_key).collect::<Vec<_>>();
    expected.sort();
    expected.dedup();
    actual.sort();
    (!actual.iter().any(String::is_empty)
        && expected == actual
        && linked
            .iter()
            .all(|name| actual.binary_search(&artist_key(name)).is_ok()))
    .then_some(credits)
}

/// Reconcile only a fresh, verified-channel, distributor-generated album item.
/// Extra untyped names remain unclassified credits; no composer/member role is inferred.
pub(crate) fn catalog_identity(candidate: &Candidate, info: &Value) -> Option<Value> {
    let names = catalog_album_artists(candidate)?;
    let meta = candidate.source_metadata.as_ref()?;
    let id = candidate.id.strip_prefix("youtube:")?;
    if info["id"].as_str() != Some(id) || info["channel_is_verified"] != true {
        return None;
    }
    let channel = text(&info["channel_id"], 24)?;
    if !channel_id(channel)
        || !meta["catalogArtistCredits"]
            .as_array()?
            .iter()
            .any(|r| r["providerId"].as_str() == Some(channel))
    {
        return None;
    }
    let title = text(&info["title"], 512)?;
    if key(title) != key(text(&meta["catalogTitle"], 512)?) {
        return None;
    }
    if info
        .get("track")
        .is_some_and(|v| !v.is_null() && text(v, 512).is_none_or(|v| key(v) != key(title)))
    {
        return None;
    }
    let album = text(&info["album"], 512)?;
    if key(album) != key(text(&meta["catalogAlbum"], 512)?) {
        return None;
    }
    let source_duration = duration(&info["duration"])?;
    if (source_duration - duration(&meta["catalogDurationSeconds"])?).abs() > 1.5
        || (source_duration - duration(&meta["catalogRequestedDurationSeconds"])?).abs() > 1.5
    {
        return None;
    }
    let description = info["description"].as_str()?;
    if description.len() > 32_768
        || !description
            .lines()
            .any(|l| l.starts_with("Provided to YouTube by "))
        || !description
            .lines()
            .any(|l| l.trim() == "Auto-generated by YouTube.")
    {
        return None;
    }
    let reported = info["artists"].as_array()?;
    if reported.is_empty() || reported.len() > 32 {
        return None;
    }
    let reported = reported
        .iter()
        .map(|v| text(v, 256).map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    if !names
        .iter()
        .all(|n| reported.iter().any(|r| key(r) == key(n)))
    {
        return None;
    }
    let unclassified = reported
        .iter()
        .filter(|r| !names.iter().any(|n| key(n) == key(r)))
        .cloned()
        .collect::<Vec<_>>();
    Some(
        json!({"provider":"youtube","channelId":channel,"channel":text(&info["channel"],200)?,
        "channelIsVerified":true,"track":title,"catalogTitle":title,"album":album,
        "artists":names,"artistCount":names.len(),"performerCreditsComplete":false,
        "artistCreditSource":"youtubeMusicCatalogVerifiedAlbum","playbackMusicCatalogCandidate":true,
        "catalogArtistCredits":meta["catalogArtistCredits"],"catalogArtist":meta["catalogArtist"],
        "catalogAlbum":album,"catalogRequestedAlbum":meta["catalogRequestedAlbum"],
        "catalogRequestedDurationSeconds":meta["catalogRequestedDurationSeconds"],
        "catalogDurationSeconds":meta["catalogDurationSeconds"],"albumBrowseId":meta["albumBrowseId"],
        "reportedSourceArtists":reported,"unclassifiedSourceArtistCredits":unclassified}),
    )
}

/// `is_live: false` describes livestream status, not the recorded performance.
/// Retain narrowly explicit performance statements from a bounded description line.
pub(crate) fn live_performance_line(info: &Value) -> Option<String> {
    let description = info["description"].as_str()?;
    let title = format!(" {} ", key(info["title"].as_str().unwrap_or_default()));
    description
        .lines()
        .take(40)
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && l.chars().count() <= 256
                && !l.chars().any(char::is_control)
                && !l.contains("http://")
                && !l.contains("https://")
        })
        .find(|line| {
            let folded = key(line);
            let explicit = ["recorded live", "grabado en vivo", "grabada en vivo"]
                .into_iter()
                .any(|prefix| {
                    folded == prefix
                        || folded
                            .strip_prefix(prefix)
                            .is_some_and(|rest| rest.starts_with(' '))
                });
            if explicit {
                return true;
            }
            // A venue mention alone may advertise a tour or livestream.
            // The dated-performance shorthand must name a performer in this upload.
            let padded = format!(" {folded} ");
            line.split_whitespace()
                .any(contains_numeric_performance_date)
                && ![
                    " tickets ",
                    " on sale ",
                    " next month ",
                    " proximamente ",
                    " entradas ",
                ]
                .into_iter()
                .any(|phrase| padded.contains(phrase))
                && [
                    " tocando en el teatro ",
                    " tocando en el estadio ",
                    " tocando en el auditorio ",
                    " performing at the theatre ",
                    " performing at the theater ",
                ]
                .into_iter()
                .any(|marker| {
                    folded.split_once(marker).is_some_and(|(performer, _)| {
                        !performer.is_empty() && title.contains(&format!(" {performer} "))
                    })
                })
        })
        .map(str::to_owned)
}

fn contains_numeric_performance_date(word: &str) -> bool {
    let word = word.trim_matches(|c: char| !c.is_ascii_digit());
    let mut parts = word.split('/');
    let (Some(day), Some(month), Some(year), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    year.len() == 4
        && year.parse::<u16>().is_ok_and(|year| year >= 1900)
        && day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/macha-official-source-selection.json"
        ))
        .unwrap()
    }
    fn candidate(f: &Value) -> Candidate {
        let row = &f["catalogItem"];
        Candidate {
            id: format!("youtube:{}", row["id"].as_str().unwrap()),
            provider: "youtube".into(),
            title: row["title"].as_str().unwrap().into(),
            duration_seconds: row["durationSeconds"].as_f64(),
            source_metadata: Some(
                json!({"provider":"youtube","playbackMusicCatalogCandidate":true,
                "catalogTitle":row["title"],"catalogArtistCredits":row["artistCredits"],"catalogAlbum":row["album"],
                "catalogArtist":row["artist"],
                "artists":row["artistCredits"].as_array().unwrap().iter().map(|credit|credit["name"].clone()).collect::<Vec<_>>(),
                "albumBrowseId":row["albumBrowseId"],"catalogDurationSeconds":row["durationSeconds"],
                "catalogRequestedAlbum":f["request"]["album"],"catalogRequestedDurationSeconds":f["request"]["durationSeconds"]}),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn album_artist_spelling_requires_the_fresh_linked_recording() {
        use crate::recording_match::{assess, MatchAction, MatchLevel, MatchStage};

        let artist_id = "UCVOsXpKahDO_PDwednC8R4g";
        let video_id = "OilHAe6Os3o";
        let track = Track {
            artist: "Rodríguez".into(),
            title: "Crucify Your Mind".into(),
            album: Some("Cold Fact".into()),
            duration_seconds: Some(152.346),
            provenance: json!({
                "catalog": {"artists": [{"name": "Rodríguez", "provider": "spotify"}]}
            }),
            ..Track::default()
        };
        let mut source = candidate(&json!({
            "request": track,
            "catalogItem": {
                "id": video_id, "title": track.title, "album": track.album,
                "artist": "Rodriguez", "durationSeconds": 153.0,
                "albumBrowseId": "MPREb_yNM2GLu8a2C",
                "artistCredits": [{"name": "Rodriguez", "providerId": artist_id}]
            }
        }));
        let action = |request: &Track, candidate: &Candidate, stage| {
            assess(request, candidate, MatchLevel::Balanced, stage).action
        };
        assert_eq!(
            action(&track, &source, MatchStage::Search),
            MatchAction::Inspect
        );
        assert_eq!(
            action(&track, &source, MatchStage::Resolved),
            MatchAction::Enrich
        );
        let mut other_edition = track.clone();
        other_edition.album = Some("Different Album".into());
        assert_eq!(
            action(&other_edition, &source, MatchStage::Search),
            MatchAction::Reject
        );

        let mut metadata = catalog_identity(&source, &json!({
            "id": video_id, "title": track.title, "album": track.album,
            "channel_id": artist_id, "channel": "SugarMan.org", "channel_is_verified": true,
            "duration": 153.0, "artists": ["Rodriguez"],
            "description": "Provided to YouTube by Universal Music Group\n\nAuto-generated by YouTube."
        })).unwrap();
        metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
        metadata["resolvedProviderId"] = json!(video_id);
        source.source_metadata = Some(metadata);
        assert_eq!(
            action(&track, &source, MatchStage::Resolved),
            MatchAction::Accept
        );
        source.source_metadata.as_mut().unwrap()["resolvedProviderId"] = json!("differentId");
        assert_eq!(
            action(&track, &source, MatchStage::Resolved),
            MatchAction::Reject
        );
    }

    #[test]
    fn observed_official_album_keeps_linked_artists_and_unclassified_credit() {
        let f = fixture();
        let c = candidate(&f);
        let m = catalog_identity(&c, &f["officialFullInfo"]).unwrap();
        assert_eq!(m["artistCount"], 2);
        assert_eq!(m["reportedSourceArtists"].as_array().unwrap().len(), 3);
        assert_eq!(m["unclassifiedSourceArtistCredits"], json!(["Aldo Asenjo"]));
        assert_eq!(m["performerCreditsComplete"], false);
        assert!(live_performance_line(&f["officialFullInfo"]).is_none());
    }
    #[test]
    fn source_album_channel_duration_and_raw_credit_guards_are_required() {
        let f = fixture();
        let c = candidate(&f);
        for (field, value) in [
            ("id", json!("H35xiJcS8RY")),
            ("album", json!("Another album")),
            ("channel_is_verified", json!(false)),
            ("channel_id", json!("UCaaaaaaaaaaaaaaaaaaaaaa")),
            ("duration", json!(231)),
            ("title", json!("La Nave del Olvido (Live)")),
            ("description", json!("Uploader claims official audio")),
            ("artists", json!(["Macha Y El Bloque Depresivo"])),
            (
                "artists",
                json!([{"name":"Additional performer","role":"performer"}]),
            ),
        ] {
            let mut info = f["officialFullInfo"].clone();
            info[field] = value;
            assert!(catalog_identity(&c, &info).is_none(), "{field}");
        }
        let mut wrong = c.clone();
        wrong.source_metadata.as_mut().unwrap()["catalogArtist"] =
            json!("Macha Y El Bloque Depresivo, Alvaro Henríquez, Another singer");
        assert!(catalog_identity(&wrong, &f["officialFullInfo"]).is_none());
        wrong = c.clone();
        wrong.source_metadata.as_mut().unwrap()["catalogRequestedAlbum"] = json!("Live album");
        let request: Track = serde_json::from_value(f["request"].clone()).unwrap();
        assert!(!preferred_recording_locator(&request, &wrong));
        let projected = catalog_identity(&wrong, &f["officialFullInfo"]).unwrap();
        assert!(!catalog_context_matches_request(&request, &projected));
        wrong = c;
        wrong.source_metadata.as_mut().unwrap()["catalogArtistCredits"][0]["providerId"] =
            Value::Null;
        assert!(catalog_identity(&wrong, &f["officialFullInfo"]).is_none());
    }
    #[test]
    fn compilation_requests_keep_distinct_releases_and_verify_the_same_recording() {
        use crate::recording_match::{assess, MatchAction, MatchLevel, MatchStage};
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/macha-compilation-recordings.json"
        ))
        .unwrap();
        for row in fixture["cases"].as_array().unwrap() {
            let request: Track = serde_json::from_value(row["request"].clone()).unwrap();
            let mut source = candidate(row);
            assert!(!preferred_album_locator(&request, &source));
            assert!(preferred_recording_locator(&request, &source));
            let mut metadata = catalog_identity(&source, &row["officialFullInfo"]).unwrap();
            assert_ne!(metadata["album"].as_str(), request.album.as_deref());
            assert_eq!(metadata["artistCount"], 1);
            assert!(
                metadata["unclassifiedSourceArtistCredits"]
                    .as_array()
                    .unwrap()
                    .len()
                    >= 1
            );
            metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
            metadata["resolvedProviderId"] = row["officialFullInfo"]["id"].clone();
            source.source_metadata = Some(metadata);
            let assessed = assess(
                &request,
                &source,
                MatchLevel::Balanced,
                MatchStage::Resolved,
            );
            assert_eq!(
                assessed.action,
                MatchAction::Accept,
                "{}: {assessed:?}",
                request.title
            );
            assert_eq!(request.album.as_deref(), Some("Bronceado de Cantina"));
        }
    }

    #[test]
    fn observed_album_recording_passes_balanced_and_recorded_concert_does_not() {
        use crate::recording_match::{assess, Comparison, MatchAction, MatchLevel, MatchStage};
        let f = fixture();
        let request: crate::models::Track = serde_json::from_value(f["request"].clone()).unwrap();
        let mut official = candidate(&f);
        assert!(preferred_album_locator(&request, &official));
        let search = assess(
            &request,
            &official,
            MatchLevel::Balanced,
            MatchStage::Search,
        );
        assert_ne!(search.action, MatchAction::Reject, "search: {search:?}");
        let mut metadata = catalog_identity(&official, &f["officialFullInfo"]).unwrap();
        metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
        metadata["resolvedProviderId"] = f["officialFullInfo"]["id"].clone();
        official.source_metadata = Some(metadata);
        let assessment = assess(
            &request,
            &official,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(
            assessment.action,
            MatchAction::Accept,
            "official: {assessment:?}"
        );
        let raw = &f["concertFullInfo"];
        let concert = Candidate {
            id: format!("youtube:{}", raw["id"].as_str().unwrap()),
            provider: "youtube".into(),
            title: raw["title"].as_str().unwrap().into(),
            duration_seconds: raw["duration"].as_f64(),
            source_metadata: Some(
                json!({"provider":"youtube","resolvedProviderId":raw["id"],"sourceMetadataOrigin":"ytDlpFullInfo","performanceVersion":"Live"}),
            ),
            ..Default::default()
        };
        let assessment = assess(
            &request,
            &concert,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        );
        assert_eq!(
            assessment.action,
            MatchAction::Reject,
            "concert: {assessment:?}"
        );
        assert_eq!(assessment.dimensions.version, Comparison::Conflict);
    }
    #[test]
    fn projected_metadata_and_extraction_cache_are_bound_to_request_album_and_duration() {
        let f = fixture();
        let request: Track = serde_json::from_value(f["request"].clone()).unwrap();
        let c = candidate(&f);
        let projected = catalog_identity(&c, &f["officialFullInfo"]).unwrap();
        assert!(catalog_context_matches_request(&request, &projected));
        let mut other = request.clone();
        other.album = Some("Live album".into());
        assert!(!catalog_context_matches_request(&other, &projected));
        other = request.clone();
        other.duration_seconds = Some(231.);
        assert!(!catalog_context_matches_request(&other, &projected));
        let mut restored = c.clone();
        restored.source_metadata = Some(projected);
        assert_eq!(catalog_context_key(&c), catalog_context_key(&restored));
        restored.source_metadata.as_mut().unwrap()["catalogRequestedDurationSeconds"] = json!(236.);
        assert_ne!(catalog_context_key(&c), catalog_context_key(&restored));
    }
    #[test]
    fn recorded_concert_is_live_even_when_it_is_not_a_livestream() {
        let f = fixture();
        assert_eq!(f["concertFullInfo"]["is_live"], false);
        assert!(live_performance_line(&f["concertFullInfo"])
            .unwrap()
            .contains("Teatro Caupolicán"));
        for line in [
            "Live Nation Records",
            "Listen to our live radio station",
            "See us live at the theatre next month",
            "Studio sessions at the theatre",
            "Live at Wembley next month — tickets on sale now!",
            "En vivo en Madrid el próximo mes",
            "Unrelated Artist performing at the theatre 25/05/2022.",
            "Artist performing at the theatre 25/05/2026. Tickets on sale now!",
        ] {
            assert!(
                live_performance_line(&json!({"description":line,"title":"Artist - Signal"}))
                    .is_none(),
                "{line}"
            );
        }
        for line in [
            "Recorded live at Wembley.",
            "Recorded live.",
            "Grabado en vivo en Madrid",
        ] {
            assert_eq!(
                live_performance_line(&json!({"description":line})).as_deref(),
                Some(line)
            );
        }
    }
}
#[test]
fn generated_say_what_album_line_corroborates_omitted_linked_credit() {
    use crate::recording_match::{assess, MatchAction, MatchLevel, MatchStage};

    let request = Track {
        provider: "spotify".into(),
        provider_id: "2GwsSbo6IbNDVvcm9rtmal".into(),
        title: "Say What".into(),
        album: Some("Say What".into()),
        artist: "Rampa, Adam Port, &ME, chuala, Keinemusik".into(),
        duration_seconds: Some(182.0),
        ..Track::default()
    };
    let id = "OYFYUXrBQH4";
    let mut candidate = Candidate {
        provider: "youtube".into(),
        id: format!("youtube:{id}"),
        title: "Say What".into(),
        duration_seconds: Some(182.0),
        source_metadata: Some(json!({
            "provider": "youtube", "playbackMusicCatalogCandidate": true,
            "catalogTitle": "Say What", "catalogArtist": "Rampa, Adam Port, &ME, Chuala",
            "catalogAlbum": "Say What", "catalogRequestedAlbum": "Say What",
            "catalogDurationSeconds": 182, "catalogRequestedDurationSeconds": 182,
            "albumBrowseId": "MPREb_eCtvJZU5L5X",
            "catalogArtistCredits": [
                {"name": "Rampa", "providerId": "UC0DerJOPJadu57EY7zH9NKQ"},
                {"name": "Adam Port", "providerId": "UCYBqBp_o9W58XpAuPM8eQjg"},
                {"name": "&ME", "providerId": "UCDybdK-dSinh17k7Upr6aag"},
                {"name": "Chuala", "providerId": "UCUNV4SLLAxLK0VQEriJ82qA"}
            ]
        })),
        ..Candidate::default()
    };
    let description = "Provided to YouTube by MTN Muting The Noise GmbH\n\nSay What · Rampa · Adam Port · &ME · Chuala · Keinemusik\n\nSay What\n\nProducer: Chuala\n\nAuto-generated by YouTube.";
    let info = json!({
        "id": id, "title": "Say What", "track": "Say What", "album": "Say What",
        "duration": 182, "channel": "Rampa Keinemusik",
        "channel_id": "UC0DerJOPJadu57EY7zH9NKQ", "channel_is_verified": true,
        "artists": ["Rampa", "Adam Port", "&ME", "Chuala", "Keinemusik"],
        "description": description
    });
    let (distributor, artists) = generated_recording_credits(&candidate, &info).unwrap();
    let mut metadata = catalog_identity(&candidate, &info).unwrap();
    assert_eq!(
        metadata["unclassifiedSourceArtistCredits"],
        json!(["Keinemusik"])
    );
    metadata["sourceMetadataOrigin"] = json!("ytDlpFullInfo");
    metadata["resolvedProviderId"] = json!(id);
    metadata["duration"] = json!(182);
    candidate.source_metadata = Some(metadata.clone());
    let balanced = |request: &Track, candidate: &Candidate| {
        assess(
            request,
            candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved,
        )
    };
    assert_ne!(balanced(&request, &candidate).action, MatchAction::Accept);
    metadata["youtubeGeneratedDistributor"] = json!(distributor);
    metadata["youtubeGeneratedArtistCredits"] = json!(artists);
    candidate.source_metadata = Some(metadata);
    for mode in [MatchLevel::Balanced, MatchLevel::Flexible] {
        let result = assess(&request, &candidate, mode, MatchStage::Resolved);
        assert_eq!(result.action, MatchAction::Accept, "{result:?}");
        let evidence = result.evidence.unwrap();
        assert!(evidence.missing_performers.is_empty());
        assert!(!evidence.complete_performer_credits);
    }
    let mut changed = request.clone();
    changed.artist = changed.artist.replace("Keinemusik", "Other Artist");
    assert_ne!(balanced(&changed, &candidate).action, MatchAction::Accept);
    changed = request.clone();
    changed.album = Some("Another Album".into());
    assert_ne!(balanced(&changed, &candidate).action, MatchAction::Accept);
    for (field, value) in [
        ("id", json!("1EVeFp-oKZg")),
        ("channel_is_verified", json!(false)),
        ("track", json!("Different Song")),
        (
            "description",
            json!(description.replace(" · Keinemusik", "")),
        ),
        (
            "description",
            json!(description.replace("Auto-generated by YouTube.", "")),
        ),
        (
            "artists",
            json!(["Rampa", "Adam Port", "&ME", "Keinemusik", "Chuala"]),
        ),
    ] {
        let mut changed = info.clone();
        changed[field] = value;
        assert!(
            generated_recording_credits(&candidate, &changed).is_none(),
            "{field}"
        );
    }
}
