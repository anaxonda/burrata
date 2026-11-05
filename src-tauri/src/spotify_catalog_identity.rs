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

pub(crate) fn valid_spotify_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
