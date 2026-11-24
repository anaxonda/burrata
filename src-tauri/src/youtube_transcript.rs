use crate::lyrics::LyricsLine;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const MAX_CAPTION_BYTES: usize = 1024 * 1024;
const MAX_CUES: usize = 5_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Transcript {
    language: String,
    automatic: bool,
    lines: Vec<LyricsLine>,
}

struct CaptionTrack {
    language: String,
    automatic: bool,
    url: Url,
}

fn caption_track(metadata: &Value, video_id: &str) -> Option<CaptionTrack> {
    for (key, automatic) in [("subtitles", false), ("automatic_captions", true)] {
        let Some(languages) = metadata[key].as_object() else {
            continue;
        };
        let mut candidates: Vec<_> = languages.iter().collect();
        candidates.sort_by_key(|(language, _)| match language.as_str() {
            "en-orig" => 0,
            "en" => 1,
            language if language.ends_with("-orig") => 2,
            _ => 3,
        });
        for (language, formats) in candidates {
            if language.is_empty()
                || language.len() > 35
                || !language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                continue;
            }
            let Some(formats) = formats.as_array() else {
                continue;
            };
            for format in formats {
                if format["ext"].as_str() != Some("json3") {
                    continue;
                }
                let Some(url) = format["url"].as_str().and_then(|raw| Url::parse(raw).ok()) else {
                    continue;
                };
                let mut video_ids = url
                    .query_pairs()
                    .filter_map(|(name, value)| (name == "v").then_some(value));
                if url.scheme() != "https"
                    || url.host_str() != Some("www.youtube.com")
                    || url.path() != "/api/timedtext"
                    || url.port().is_some()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                    || video_ids.next().as_deref() != Some(video_id)
                    || video_ids.next().is_some()
                    || url
                        .query_pairs()
                        .any(|(name, value)| name == "tlang" && !value.is_empty())
                {
                    continue;
                }
                return Some(CaptionTrack {
                    language: language
                        .strip_suffix("-orig")
                        .unwrap_or(language)
                        .to_owned(),
                    automatic,
                    url,
                });
            }
        }
    }
    None
}

fn caption_cues(bytes: &[u8], duration: Option<f64>) -> Vec<LyricsLine> {
    let Ok(document) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    let Some(events) = document["events"].as_array() else {
        return Vec::new();
    };
    let max_seconds = duration.unwrap_or(86_400.0);
    let mut lines: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let seconds = event["tStartMs"].as_u64()? as f64 / 1000.0;
            if seconds > max_seconds {
                return None;
            }
            let segment_text: String = event["segs"]
                .as_array()?
                .iter()
                .filter_map(|segment| segment["utf8"].as_str())
                .collect();
            let text = segment_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() || text.contains('\0') {
                return None;
            }
            Some(LyricsLine {
                id: format!("caption-{index}"),
                time_seconds: Some(seconds),
                text: text.chars().take(1000).collect(),
            })
        })
        .take(MAX_CUES)
        .collect();
    lines.sort_by(|left, right| left.time_seconds.partial_cmp(&right.time_seconds).unwrap());
    lines
        .dedup_by(|left, right| left.time_seconds == right.time_seconds && left.text == right.text);
    lines
}

pub(crate) async fn load(
    video_id: &str,
    settings: &crate::models::Settings,
) -> Result<Option<Transcript>, String> {
    static REQUEST: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _permit = REQUEST
        .try_acquire()
        .map_err(|_| "Another YouTube transcript is loading")?;
    let metadata = crate::acquisition::youtube_caption_metadata(video_id, settings)
        .await
        .map_err(|_| "YouTube transcript lookup failed. Try again.")?;
    let Some(caption) = caption_track(&metadata, video_id) else {
        return Ok(None);
    };
    let mut response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|_| "Could not prepare the YouTube transcript request")?
        .get(caption.url)
        .send()
        .await
        .map_err(|_| "Could not load the YouTube transcript")?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_CAPTION_BYTES as u64)
    {
        return Err("YouTube transcript is unavailable".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "YouTube transcript response was interrupted")?
    {
        if chunk.len() > MAX_CAPTION_BYTES - bytes.len() {
            return Err("YouTube transcript is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let duration = metadata["duration"].as_f64().filter(|value| *value > 0.0);
    let lines = caption_cues(&bytes, duration);
    Ok((!lines.is_empty()).then_some(Transcript {
        language: caption.language,
        automatic: caption.automatic,
        lines,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captions_require_the_exact_video_and_prefer_an_untranslated_creator_track() {
        let metadata = json!({"subtitles": {"de": [
            {"ext":"json3", "url":"https://evil.example/api/timedtext?v=video"},
            {"ext":"json3", "url":"https://www.youtube.com/api/timedtext?v=other"},
            {"ext":"json3", "url":"https://www.youtube.com/api/timedtext?v=video&v=other"},
            {"ext":"json3", "url":"https://www.youtube.com/api/timedtext?v=video&tlang=de"},
            {"ext":"json3", "url":"https://www.youtube.com/api/timedtext?v=video&lang=de"}
        ]}, "automatic_captions": {"en": [
            {"ext":"json3", "url":"https://www.youtube.com/api/timedtext?v=video&lang=en"}
        ]}});
        let track = caption_track(&metadata, "video").unwrap();
        assert_eq!(track.language, "de");
        assert!(!track.automatic);
        assert!(caption_track(&metadata, "unmatched").is_none());
    }

    #[test]
    fn json3_cues_use_absolute_seconds_and_ignore_invalid_events() {
        let lines = caption_cues(
            br#"{"events":[
            {"tStartMs":1500,"segs":[{"utf8":"first"},{"utf8":" line"}]},
            {"tStartMs":0,"segs":[{"utf8":"start\n"}]},
            {"tStartMs":2000}, {"tStartMs":999999,"segs":[{"utf8":"out of bounds"}]}
        ]}"#,
            Some(60.0),
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_seconds, Some(0.0));
        assert_eq!(lines[1].time_seconds, Some(1.5));
        assert_eq!(lines[1].text, "first line");
    }
}
