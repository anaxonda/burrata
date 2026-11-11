//! Bounded, public YouTube channel upload browsing through the packaged yt-dlp.
//! The flat listing contains direct-upload identities; channel/uploader labels
//! are deliberately kept in provenance and never promoted to recording artist.

use crate::{acquisition, models::Track};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_PAGE_SIZE: usize = 25;
pub const MAX_PAGE_SIZE: usize = 50;
const MAX_OFFSET: usize = 10_000;
const MAX_OWNER_LENGTH: usize = 2048;
const MAX_CURSOR_LENGTH: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeChannelVideosPage {
    pub channel_id: String,
    pub title: String,
    pub url: String,
    pub tracks: Vec<Track>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    channel_id: String,
    offset: usize,
    limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner {
    Id(String),
    Url { expected_page_url: String },
}

fn valid_channel_id(id: &str) -> bool {
    id.len() == 24
        && id.starts_with("UC")
        && id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_handle_segment(value: &str) -> bool {
    let Some(handle) = value.strip_prefix('@') else {
        return false;
    };
    !handle.is_empty()
        && handle.len() <= 100
        && handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn normalize_owner(raw: &str) -> Result<Owner, String> {
    if raw.trim() != raw {
        return Err("YouTube channel owner is invalid".into());
    }
    if valid_channel_id(raw) {
        return Ok(Owner::Id(raw.to_owned()));
    }
    if raw.len() > MAX_OWNER_LENGTH || raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("YouTube channel owner is invalid".into());
    }
    let parsed = url::Url::parse(raw).map_err(|_| "YouTube channel owner URL is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("www.youtube.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("YouTube channel owner must use a safe www.youtube.com HTTPS URL".into());
    }
    let path = parsed
        .path_segments()
        .ok_or("YouTube channel URL path is invalid")?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let route = match path.as_slice() {
        ["channel", channel_id] if valid_channel_id(channel_id) => {
            format!("/channel/{channel_id}/videos")
        }
        ["channel", channel_id, "videos"] if valid_channel_id(channel_id) => {
            format!("/channel/{channel_id}/videos")
        }
        [handle] if valid_handle_segment(handle) => format!("/{handle}/videos"),
        [handle, "videos"] if valid_handle_segment(handle) => format!("/{handle}/videos"),
        _ => return Err("YouTube channel URL must identify a channel ID or @handle".into()),
    };
    let expected_page_url = format!("https://www.youtube.com{route}");
    Ok(Owner::Url { expected_page_url })
}

fn encode_cursor(channel_id: &str, offset: usize, limit: usize) -> Result<String, String> {
    if !valid_channel_id(channel_id) || offset == 0 || offset > MAX_OFFSET.saturating_sub(limit) {
        return Err("YouTube channel cursor bounds are invalid".into());
    }
    let value = Cursor {
        version: 1,
        channel_id: channel_id.to_owned(),
        offset,
        limit,
    };
    let bytes =
        serde_json::to_vec(&value).map_err(|_| "Could not encode YouTube channel cursor")?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(
    raw: Option<&str>,
    channel_id: Option<&str>,
    requested_limit: usize,
) -> Result<(usize, Option<String>), String> {
    let Some(raw) = raw else { return Ok((0, None)) };
    if raw.is_empty() || raw.len() > MAX_CURSOR_LENGTH {
        return Err("YouTube channel cursor is invalid".into());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| "YouTube channel cursor is invalid")?;
    let cursor: Cursor =
        serde_json::from_slice(&bytes).map_err(|_| "YouTube channel cursor is invalid")?;
    if cursor.version != 1
        || !valid_channel_id(&cursor.channel_id)
        || cursor.offset == 0
        || cursor.offset > MAX_OFFSET.saturating_sub(requested_limit)
        || !(1..=MAX_PAGE_SIZE).contains(&cursor.limit)
        || cursor.limit != requested_limit
        || channel_id.is_some_and(|expected| expected != cursor.channel_id)
    {
        return Err("YouTube channel cursor does not match this channel page".into());
    }
    Ok((cursor.offset, Some(cursor.channel_id)))
}

fn owner_page_url(owner: &Owner) -> String {
    match owner {
        Owner::Id(id) => format!("https://www.youtube.com/channel/{id}/videos"),
        Owner::Url {
            expected_page_url, ..
        } => expected_page_url.clone(),
    }
}

fn safe_channel_url(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("www.youtube.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let path = parsed
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match path.as_slice() {
        ["channel", id] if valid_channel_id(id) => {
            Some(format!("https://www.youtube.com/channel/{id}"))
        }
        _ => None,
    }
}

fn optional_metadata_string<'a>(root: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match root.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("yt-dlp returned invalid YouTube {key} metadata")),
    }
}

fn parse_root(
    raw: &str,
    owner: &Owner,
    expected_channel_id: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<YoutubeChannelVideosPage, String> {
    if raw.len() > 8 * 1024 * 1024 {
        return Err("YouTube channel listing exceeded the response limit".into());
    }
    let root: Value =
        serde_json::from_str(raw).map_err(|_| "yt-dlp returned malformed YouTube channel data")?;
    let channel_id_value = optional_metadata_string(&root, "channel_id")?;
    let id_value = optional_metadata_string(&root, "id")?;
    for value in [channel_id_value, id_value].into_iter().flatten() {
        if !valid_channel_id(value) {
            return Err("yt-dlp did not return a valid YouTube channel ID".into());
        }
    }
    if channel_id_value
        .zip(id_value)
        .is_some_and(|(channel_id, id)| channel_id != id)
    {
        return Err("YouTube channel response identity is inconsistent".into());
    }
    let channel_id = channel_id_value
        .or(id_value)
        .ok_or("yt-dlp did not return a valid YouTube channel ID")?;
    if expected_channel_id.is_some_and(|expected| expected != channel_id) {
        return Err("YouTube channel response did not match the requested channel".into());
    }
    if optional_metadata_string(&root, "availability")? != Some("public") {
        return Err("YouTube channel response is not confirmed public".into());
    }
    if let Owner::Url {
        expected_page_url, ..
    } = owner
    {
        let actual = optional_metadata_string(&root, "webpage_url")?
            .ok_or("yt-dlp did not confirm the requested YouTube channel URL")?;
        if actual.trim_end_matches('/') != expected_page_url.trim_end_matches('/') {
            return Err("YouTube channel response did not match the requested channel URL".into());
        }
    }
    if let Some(channel_url) = optional_metadata_string(&root, "channel_url")? {
        let canonical = safe_channel_url(channel_url)
            .ok_or("yt-dlp returned an invalid YouTube channel URL")?;
        if canonical != format!("https://www.youtube.com/channel/{channel_id}") {
            return Err("YouTube channel response identity is inconsistent".into());
        }
    }
    let title = root
        .get("channel")
        .and_then(Value::as_str)
        .or_else(|| root.get("uploader").and_then(Value::as_str))
        .filter(|title| !title.trim().is_empty() && title.len() <= 512)
        .ok_or("yt-dlp did not return a valid YouTube channel title")?
        .to_owned();
    let entries = root
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("yt-dlp did not return YouTube channel entries")?;
    if entries.len() > limit + 1 {
        return Err("yt-dlp returned too many YouTube channel entries".into());
    }
    let has_more = entries.len() > limit;
    let tracks = entries
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, entry)| parse_entry(entry, channel_id, &title, offset + index + 1))
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        Some(encode_cursor(
            channel_id,
            offset
                .checked_add(limit)
                .ok_or("YouTube channel offset overflow")?,
            limit,
        )?)
    } else {
        None
    };
    Ok(YoutubeChannelVideosPage {
        channel_id: channel_id.to_owned(),
        title,
        url: format!("https://www.youtube.com/channel/{channel_id}"),
        tracks,
        complete: !has_more,
        next_cursor,
        warnings: Vec::new(),
    })
}

fn parse_entry(
    entry: &Value,
    channel_id: &str,
    channel_title: &str,
    position: usize,
) -> Result<Track, String> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .ok_or("YouTube channel entry has an invalid video ID")?;
    let title = entry
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| {
            !title.trim().is_empty() && title.len() <= 512 && !title.chars().any(char::is_control)
        })
        .ok_or("YouTube channel entry has an invalid video title")?;
    match entry.get("channel_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(value)) if value == channel_id => {}
        Some(Value::String(_)) => {
            return Err("YouTube channel entry belongs to a different channel".into())
        }
        Some(_) => return Err("YouTube channel entry has invalid channel metadata".into()),
    }
    let duration = match entry.get("duration") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let seconds = value
                .as_f64()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds <= 86_400.0)
                .ok_or("YouTube channel entry has an invalid duration")?;
            (seconds > 0.0).then_some(seconds)
        }
    };
    // Derive the artwork URL from the already-validated video ID. This avoids
    // trusting arbitrary thumbnail URLs from provider metadata while ensuring
    // flat listings without embedded thumbnail rows still show the upload art.
    let artwork = Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"));
    let uploader = entry
        .get("channel")
        .or_else(|| entry.get("uploader"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 512);
    let video_url = format!("https://www.youtube.com/watch?v={id}");
    Ok(Track {
        id: format!("youtube:{id}"),
        provider: "youtube".into(),
        provider_id: id.to_owned(),
        source_url: video_url.clone(),
        // Direct uploads cannot establish the performer/recording credit.
        artist: String::new(),
        title: title.to_owned(),
        duration_seconds: duration,
        artwork_url: artwork,
        provenance: json!({
            "sourceKind": "youtubeChannelUpload",
            "playbackMode": "source",
            "identity": "direct YouTube upload; recording artist unverified",
            "uploaderChannelId": channel_id,
            "uploaderChannelTitle": channel_title,
            "uploaderName": uploader,
            "position": position,
            "remoteUrl": video_url,
        }),
        ..Track::default()
    })
}

pub async fn browse_youtube_channel_videos(
    owner_id: &str,
    cursor: Option<&str>,
    limit: Option<usize>,
    settings: &crate::models::Settings,
) -> Result<YoutubeChannelVideosPage, String> {
    let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err("YouTube channel page size must be between 1 and 50".into());
    }
    let owner = normalize_owner(owner_id)?;
    let expected_id = match owner {
        Owner::Id(ref id) => Some(id.as_str()),
        _ => None,
    };
    let (offset, cursor_channel_id) = decode_cursor(cursor, expected_id, limit)?;
    let bound_channel_id = expected_id.or(cursor_channel_id.as_deref());
    let input_url = owner_page_url(&owner);
    let mut args = acquisition::base_args(settings);
    args.extend([
        "--no-warnings".into(),
        "--no-progress".into(),
        "--skip-download".into(),
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        "--playlist-start".into(),
        offset
            .checked_add(1)
            .ok_or("YouTube channel offset overflow")?
            .to_string(),
        "--playlist-end".into(),
        offset
            .checked_add(limit)
            .and_then(|value| value.checked_add(1))
            .ok_or("YouTube channel offset overflow")?
            .to_string(),
        "--".into(),
        input_url,
    ]);
    let raw = acquisition::fetch_public_youtube_listing(settings, args, Duration::from_secs(35))
        .await
        .map_err(|_| "Public YouTube channel listing failed or timed out".to_owned())?;
    parse_root(&raw, &owner, bound_channel_id, offset, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(entries: Value) -> String {
        json!({
            "id":"UC8TZwtZ17WKFJSmwTZQpBTA",
            "channel":"MAJ",
            "channel_id":"UC8TZwtZ17WKFJSmwTZQpBTA",
            "title":"MAJ - Videos",
            "availability":"public",
            "channel_url":"https://www.youtube.com/channel/UC8TZwtZ17WKFJSmwTZQpBTA",
            "webpage_url":"https://www.youtube.com/@MyAnalogJournal/videos",
            "entries":entries,
        })
        .to_string()
    }

    #[test]
    fn resolves_safe_channel_urls_and_rejects_unsafe_owners() {
        assert_eq!(
            normalize_owner("UC8TZwtZ17WKFJSmwTZQpBTA"),
            Ok(Owner::Id("UC8TZwtZ17WKFJSmwTZQpBTA".into()))
        );
        assert!(normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").is_ok());
        for value in [
            "http://www.youtube.com/@x",
            "https://youtube.com/@x",
            "https://www.youtube.com/@x?x=1",
            "https://www.youtube.com/watch?v=abc",
        ] {
            assert!(normalize_owner(value).is_err(), "{value}");
        }
    }

    #[test]
    fn parses_direct_upload_without_claiming_uploader_as_artist() {
        let page = parse_root(
            &root(
                json!([{"id":"s6G2q4zNDuk","title":"Dub session","channel":"MAJ","duration":2836}]),
            ),
            &normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap(),
            None,
            0,
            25,
        )
        .unwrap();
        let track = &page.tracks[0];
        assert_eq!(track.id, "youtube:s6G2q4zNDuk");
        assert_eq!(track.provider_id, "s6G2q4zNDuk");
        assert_eq!(track.artist, "");
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://i.ytimg.com/vi/s6G2q4zNDuk/hqdefault.jpg")
        );
        assert_eq!(track.provenance["uploaderName"], "MAJ");
        assert_eq!(track.provenance["sourceKind"], "youtubeChannelUpload");
        assert!(page.complete && page.next_cursor.is_none());
    }

    #[test]
    fn parses_captured_public_uc_and_handle_listings() {
        let channel_id = "UC8TZwtZ17WKFJSmwTZQpBTA";
        let id_page = parse_root(
            include_str!("../tests/fixtures/youtube-channel-maj-uc.json"),
            &Owner::Id(channel_id.into()),
            Some(channel_id),
            0,
            25,
        )
        .unwrap();
        let handle_owner =
            normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap();
        let handle_page = parse_root(
            include_str!("../tests/fixtures/youtube-channel-maj-handle.json"),
            &handle_owner,
            None,
            0,
            25,
        )
        .unwrap();

        for page in [&id_page, &handle_page] {
            assert_eq!(page.channel_id, channel_id);
            assert_eq!(page.title, "MAJ");
            assert_eq!(page.tracks.len(), 5);
            assert!(page.complete && page.next_cursor.is_none());
            let first = &page.tracks[0];
            assert_eq!(first.id, "youtube:s6G2q4zNDuk");
            assert_eq!(first.provider_id, "s6G2q4zNDuk");
            assert_eq!(first.title, "Dub, Dancehall & Lovers Rock with Vanilla");
            assert_eq!(first.artist, "");
            assert_eq!(
                first.artwork_url.as_deref(),
                Some("https://i.ytimg.com/vi/s6G2q4zNDuk/hqdefault.jpg")
            );
            assert_eq!(
                first.source_url,
                "https://www.youtube.com/watch?v=s6G2q4zNDuk"
            );
        }
    }

    #[test]
    fn valid_empty_is_complete_and_malformed_or_wrong_owner_is_error() {
        let owner = normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap();
        let page = parse_root(&root(json!([])), &owner, None, 0, 25).unwrap();
        assert!(page.tracks.is_empty() && page.complete && page.next_cursor.is_none());
        let private = root(json!([])).replace(
            "\"availability\":\"public\"",
            "\"availability\":\"private\"",
        );
        assert!(parse_root(&private, &owner, None, 0, 25).is_err());
        assert!(parse_root("{}", &owner, None, 0, 25).is_err());
        assert!(parse_root(
            &root(json!([{"id":"bad","title":"bad"}])),
            &owner,
            None,
            0,
            25
        )
        .is_err());
        assert!(parse_root(
            &root(json!([])),
            &owner,
            Some("UCBJycsmduvYEL83R_U4JriQ"),
            0,
            25
        )
        .is_err());
    }

    #[test]
    fn rejects_conflicting_or_malformed_channel_identity_metadata() {
        let owner = normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap();
        let captured = include_str!("../tests/fixtures/youtube-channel-maj-handle.json");
        let mut value: Value = serde_json::from_str(captured).unwrap();

        value["id"] = json!("UCBJycsmduvYEL83R_U4JriQ");
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
        value["id"] = json!("UC8TZwtZ17WKFJSmwTZQpBTA");
        value["channel_id"] = json!(7);
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());

        value["channel_id"] = json!("UC8TZwtZ17WKFJSmwTZQpBTA");
        value["channel_url"] = json!(7);
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
        value["channel_url"] = json!("https://www.youtube.com/@MyAnalogJournal");
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
        value["channel_url"] = json!("https://www.youtube.com/channel/UCBJycsmduvYEL83R_U4JriQ");
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());

        value["channel_url"] = json!("https://www.youtube.com/channel/UC8TZwtZ17WKFJSmwTZQpBTA");
        value["webpage_url"] = json!(7);
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
        value["webpage_url"] = json!("https://www.youtube.com/@OtherChannel/videos");
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
    }

    #[test]
    fn accepts_absent_optional_metadata_for_canonical_id_owner() {
        let mut value: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/youtube-channel-maj-uc.json"
        ))
        .unwrap();
        value.as_object_mut().unwrap().remove("channel_id");
        value.as_object_mut().unwrap().remove("channel_url");
        value.as_object_mut().unwrap().remove("webpage_url");
        let page = parse_root(
            &value.to_string(),
            &Owner::Id("UC8TZwtZ17WKFJSmwTZQpBTA".into()),
            Some("UC8TZwtZ17WKFJSmwTZQpBTA"),
            0,
            25,
        )
        .unwrap();
        assert_eq!(page.tracks.len(), 5);

        value.as_object_mut().unwrap().remove("availability");
        assert!(parse_root(
            &value.to_string(),
            &Owner::Id("UC8TZwtZ17WKFJSmwTZQpBTA".into()),
            Some("UC8TZwtZ17WKFJSmwTZQpBTA"),
            0,
            25,
        )
        .is_err());
        value["availability"] = json!(7);
        assert!(parse_root(
            &value.to_string(),
            &Owner::Id("UC8TZwtZ17WKFJSmwTZQpBTA".into()),
            Some("UC8TZwtZ17WKFJSmwTZQpBTA"),
            0,
            25,
        )
        .is_err());
    }

    #[test]
    fn rejects_wrong_typed_entry_channel_id_but_allows_null_metadata() {
        let owner = normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap();
        let mut value: Value = serde_json::from_str(&root(json!([{
            "id":"s6G2q4zNDuk",
            "title":"Upload",
            "channel_id":null,
        }])))
        .unwrap();
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_ok());
        value["entries"][0]["channel_id"] = json!(7);
        assert!(parse_root(&value.to_string(), &owner, None, 0, 25).is_err());
    }

    #[test]
    fn cursor_binds_channel_and_page_size_and_more_always_has_cursor() {
        let owner = normalize_owner("https://www.youtube.com/@MyAnalogJournal/videos").unwrap();
        let entries = (0..3)
            .map(|i| json!({"id":format!("abcdefghij{i}"),"title":"Upload"}))
            .collect::<Vec<_>>();
        let page = parse_root(&root(json!(entries)), &owner, None, 0, 2).unwrap();
        assert!(!page.complete);
        let next = page.next_cursor.unwrap();
        assert_eq!(
            decode_cursor(Some(&next), Some("UC8TZwtZ17WKFJSmwTZQpBTA"), 2)
                .unwrap()
                .0,
            2
        );
        assert!(decode_cursor(Some(&next), Some("UCBJycsmduvYEL83R_U4JriQ"), 2).is_err());
        assert!(decode_cursor(Some(&next), Some("UC8TZwtZ17WKFJSmwTZQpBTA"), 3).is_err());
    }
}
