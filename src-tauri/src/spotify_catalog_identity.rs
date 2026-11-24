//! Pure exact-Spotify catalog identity used by role evidence and artist aliases.
use crate::models::Track;
use serde_json::Value;

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

/// Compare a numbered movement caption with a provider-bound Spotify title.
/// This only recognizes formatting and supplies no performer or release proof.
pub(crate) fn numbered_movement_caption_matches(
    fingerprint: &SpotifyArtistCreditFingerprint,
    caption: &str,
    duration: f64,
) -> bool {
    if !is_numbered_movement(&fingerprint.title)
        || !duration.is_finite()
        || (duration - fingerprint.duration_ms as f64 / 1000.0).abs() > 1.5
    {
        return false;
    }
    let Some((prefix, caption)) = caption.split_once(": ") else {
        return false;
    };
    (caption.eq_ignore_ascii_case(&fingerprint.title)
        || explicit_live_base(caption)
            .zip(explicit_live_base(&fingerprint.title))
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
        && fingerprint
            .artists
            .iter()
            .filter(|artist| name_matches_caption_prefix(prefix, &artist.name))
            .count()
            == 1
}

pub(crate) fn name_matches_caption_prefix(prefix: &str, name: &str) -> bool {
    prefix.eq_ignore_ascii_case(name)
        || name
            .split_whitespace()
            .next_back()
            .is_some_and(|surname| prefix.eq_ignore_ascii_case(surname))
        || has_initialed_surname(prefix, name)
}

// Preserve the explicit performance designation on both sides.
fn explicit_live_base(title: &str) -> Option<&str> {
    title
        .strip_suffix(" (Live)")
        .or_else(|| title.strip_suffix(" - Live"))
}

fn is_numbered_movement(title: &str) -> bool {
    let Some((work, movement)) = title.split_once(": ") else {
        return false;
    };
    let Some((ordinal, movement_name)) = movement.split_once(". ") else {
        return false;
    };
    let work = crate::matching::normalize(work);
    let tokens = work.split_whitespace().collect::<Vec<_>>();
    ["symphony", "concerto", "sonata", "quartet", "suite"]
        .iter()
        .any(|kind| tokens.contains(kind))
        && tokens.windows(2).any(|pair| {
            matches!(pair[0], "op" | "bwv" | "hwv" | "rv" | "k" | "kv")
                && pair[1].parse::<u16>().is_ok()
        })
        && matches!(ordinal, "I" | "II" | "III" | "IV" | "V" | "VI")
        && !movement_name.trim().is_empty()
}

pub(crate) fn numbered_movement_album_request(
    track: &Track,
) -> Option<(&str, SpotifyArtistCreditFingerprint)> {
    if ["title", "artist", "album", "durationSeconds"]
        .iter()
        .any(|field| track.provenance["manualFields"][field] == true)
    {
        return None;
    }
    let fingerprint = spotify_artist_credit_fingerprint(track)?;
    let album = track.album.as_deref().filter(|album| {
        !album.trim().is_empty()
            && track.provenance["remoteCatalog"]["album"].as_str() == Some(*album)
    })?;
    is_numbered_movement(&fingerprint.title).then_some((album, fingerprint))
}

fn has_initialed_surname(prefix: &str, full_name: &str) -> bool {
    let Some((given_names, surname)) = full_name.rsplit_once(' ') else {
        return false;
    };
    if !prefix.contains('.') {
        return false;
    }
    let initials = given_names
        .split_whitespace()
        .filter_map(|name| name.chars().next().filter(|letter| letter.is_alphabetic()))
        .map(|letter| letter.to_string())
        .collect::<Vec<_>>();
    if initials.len() < 2 {
        return false;
    }
    let signature = format!("{} {surname}", initials.join(" "));
    crate::matching::normalize(prefix) == crate::matching::normalize(&signature)
}

pub(crate) fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use super::*;
    pub(crate) fn two_performer_track() -> Track {
        Track {
            id: "spotify:cccccccccccccccccccccc".into(),
            provider: "spotify".into(),
            provider_id: "cccccccccccccccccccccc".into(),
            source_url: "https://open.spotify.com/track/cccccccccccccccccccccc".into(),
            title: "Song of Two Choirs".into(),
            artist: "Choir Alpha, Choir Beta".into(),
            duration_seconds: Some(190.0),
            isrc: Some("USNO18716512".into()),
            provenance: serde_json::json!({
                "catalog":{"verified":true,"provider":"spotify","providerId":"cccccccccccccccccccccc","artistCount":2,"artists":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Choir Alpha"},{"provider":"spotify","id":"dddddddddddddddddddddd","name":"Choir Beta"}]},
                "remoteCatalog":{"trackId":"cccccccccccccccccccccc","title":"Song of Two Choirs","artist":"Choir Alpha, Choir Beta","durationSeconds":190.0,"isrc":"USNO18716512","artistCount":2,"artistRefs":[{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"Choir Alpha","joinPhrase":" & "},{"provider":"spotify","id":"dddddddddddddddddddddd","name":"Choir Beta","joinPhrase":""}]}
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_movement_live_caption_preserves_performance_and_ordinal() {
        let title = "Piano Concerto No. 1 in B-Flat Minor, Op. 23: I. Allegro non troppo";
        let mut fingerprint =
            spotify_artist_credit_fingerprint(&test_fixtures::two_performer_track()).unwrap();
        fingerprint.title = format!("{title} - Live");
        assert!(numbered_movement_caption_matches(
            &fingerprint,
            &format!("Alpha: {title} (Live)"),
            190.0
        ));
        assert!(!numbered_movement_caption_matches(
            &fingerprint,
            &format!("Alpha: {title}"),
            190.0
        ));
        let other = title.replace(": I.", ": II.");
        assert!(!numbered_movement_caption_matches(
            &fingerprint,
            &format!("Alpha: {other} (Live)"),
            190.0
        ));
    }

    #[test]
    fn complete_two_performer_fixture_matches_spotify_rendering() {
        let track = test_fixtures::two_performer_track();
        let fingerprint = spotify_artist_credit_fingerprint(&track)
            .expect("shared fixture must represent the actual comma-separated Spotify rendering");
        assert_eq!(fingerprint.artists.len(), 2);
    }

    #[test]
    fn modified_credit_rendering_does_not_authorize_a_cached_crosswalk() {
        let mut track = test_fixtures::two_performer_track();
        track.artist = "Choir Alpha & Choir Beta".into();
        track.provenance["remoteCatalog"]["artist"] = serde_json::json!(track.artist);
        assert!(spotify_artist_credit_fingerprint(&track).is_none());
    }
}
