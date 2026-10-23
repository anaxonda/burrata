//! Recording-scoped lyric search variants. Never transliterate names by guesswork.
use crate::{
    artist_credit_aliases, artists::ArtistService, lyrics::LyricsRequest, models::Track, Store,
};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

fn valid(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
}

fn add(values: &mut Vec<String>, text: &str, limit: usize) {
    if values.len() < limit
        && valid(text)
        && !values
            .iter()
            .any(|old| crate::matching::normalize(old) == crate::matching::normalize(text))
    {
        values.push(text.to_owned());
    }
}

fn recording_titles(
    occurrence: &artist_credit_aliases::VerifiedMusicBrainzOccurrence,
    raw: &Value,
) -> Vec<String> {
    let ids = raw["artist-credit"].as_array().and_then(|credits| {
        credits
            .iter()
            .map(|credit| credit["artist"]["id"].as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    });
    if occurrence.artist_ids.is_empty()
        || raw["id"].as_str() != Some(occurrence.recording_id.as_str())
        || ids.as_ref() != Some(&occurrence.artist_ids)
        || raw["length"].as_f64().is_none_or(|milliseconds| {
            !milliseconds.is_finite()
                || (milliseconds / 1000.0 - occurrence.duration_seconds).abs() > 2.0
        })
    {
        return Vec::new();
    }
    if !occurrence.isrcs.is_empty()
        && !raw["isrcs"].as_array().is_some_and(|codes| {
            codes
                .iter()
                .filter_map(Value::as_str)
                .filter_map(crate::recording_identity::normalize_isrc)
                .any(|code| occurrence.isrcs.contains(&code))
        })
    {
        return Vec::new();
    }
    let mut titles = Vec::new();
    if let Some(title) = raw["title"].as_str() {
        add(&mut titles, title, 4);
    }
    for alias in raw["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .take(64)
        .filter(|alias| alias["type"].as_str() != Some("Search hint"))
    {
        if let Some(name) = alias["name"].as_str() {
            add(&mut titles, name, 4);
        }
    }
    titles
}

fn artist_renderings(raw: &Value, profiles: &[Value]) -> Vec<String> {
    let Some(credits) = raw["artist-credit"].as_array() else {
        return Vec::new();
    };
    if credits.is_empty() || credits.len() > 16 {
        return Vec::new();
    }
    let mut choices = Vec::new();
    for (index, credit) in credits.iter().enumerate() {
        let Some(id) = credit["artist"]["id"].as_str() else {
            return Vec::new();
        };
        let Some(name) = credit["name"]
            .as_str()
            .or(credit["artist"]["name"].as_str())
            .filter(|name| valid(name))
        else {
            return Vec::new();
        };
        let Some(join) = artist_credit_aliases::verified_musicbrainz_joinphrase(
            credit,
            index + 1 == credits.len(),
        ) else {
            return Vec::new();
        };
        let mut names = vec![name.to_owned()];
        for profile in profiles
            .iter()
            .filter(|profile| profile["id"].as_str() == Some(id))
        {
            if let Some(canonical) = profile["name"].as_str() {
                add(&mut names, canonical, 8);
            }
            for alias in profile["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
                .filter(|alias| alias["type"].as_str() != Some("Search hint"))
            {
                if let Some(name) = alias["name"].as_str() {
                    add(&mut names, name, 8);
                }
            }
        }
        choices.push((names, join.to_owned()));
    }
    let mut result = Vec::new();
    let render = |replace: Option<(usize, &str)>| {
        choices
            .iter()
            .enumerate()
            .map(|(i, (names, join))| {
                format!(
                    "{}{}",
                    replace
                        .filter(|(index, _)| *index == i)
                        .map(|(_, name)| name)
                        .unwrap_or(&names[0]),
                    join
                )
            })
            .collect::<String>()
    };
    add(&mut result, &render(None), 8);
    for (index, (names, _)) in choices.iter().enumerate() {
        for name in names.iter().skip(1) {
            add(&mut result, &render(Some((index, name))), 8);
        }
    }
    result
}

pub(crate) async fn requests(
    store: &Store,
    service: &ArtistService,
    track: &Track,
    request: &LyricsRequest,
) -> Vec<LyricsRequest> {
    let mut titles = vec![request.track_name.clone()];
    let mut artists = vec![request.artist_name.clone()];
    for artist in artist_credit_aliases::cached_verified_playback_artist_aliases(store, track)
        .search_artist_candidates()
    {
        add(&mut artists, &artist, 8);
    }
    if let Some(occurrence) = artist_credit_aliases::verified_musicbrainz_occurrence(store, track) {
        if let Ok(Ok(raw)) = tokio::time::timeout(
            Duration::from_secs(3),
            service.json_for_metadata(
                "musicbrainz",
                &format!(
                    "recording/{}?inc=aliases+artist-credits+isrcs",
                    occurrence.recording_id
                ),
                "lyrics.alias-recording.musicbrainz.v1",
                &occurrence.recording_id,
                None,
            ),
        )
        .await
        {
            let verified_titles = recording_titles(&occurrence, &raw);
            if !verified_titles.is_empty() {
                for title in verified_titles {
                    add(&mut titles, &title, 4);
                }
                let mut profiles = Vec::new();
                for id in occurrence.artist_ids.iter().take(2) {
                    if let Ok(Ok(profile)) = tokio::time::timeout(
                        Duration::from_secs(2),
                        service.json_for_metadata(
                            "musicbrainz",
                            &format!("artist/{id}?inc=aliases"),
                            "lyrics.alias-artist.musicbrainz.v1",
                            id,
                            None,
                        ),
                    )
                    .await
                    {
                        profiles.push(profile);
                    }
                }
                for artist in artist_renderings(&raw, &profiles) {
                    add(&mut artists, &artist, 8);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for title in titles {
        for artist in &artists {
            if seen.insert((
                crate::matching::normalize(&title),
                crate::matching::normalize(artist),
            )) {
                result.push(LyricsRequest {
                    track_name: title.clone(),
                    artist_name: artist.clone(),
                    album_name: request.album_name.clone(),
                    duration_seconds: request.duration_seconds,
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recording_aliases_require_the_same_recording_credits_and_runtime() {
        let occurrence = artist_credit_aliases::VerifiedMusicBrainzOccurrence {
            release_id: "release".into(),
            recording_id: "recording".into(),
            track_guid: "track".into(),
            title: "Kalimankou Denkou".into(),
            duration_seconds: 310.0,
            artist_ids: vec!["choir".into()],
            isrcs: vec!["BGABC1234567".into()],
        };
        let raw = json!({"id":"recording", "title":"Kalimankou Denkou", "length":310000,
            "artist-credit":[{"artist":{"id":"choir"}}], "isrcs":["BGABC1234567"],
            "aliases":[{"name":"Калиманку Денку"},{"name":"ignore me","type":"Search hint"}]});
        assert_eq!(
            recording_titles(&occurrence, &raw),
            vec!["Kalimankou Denkou", "Калиманку Денку"]
        );
        for (field, value) in [
            ("id", json!("other")),
            ("length", json!(320000)),
            ("isrcs", json!(["BGABC1234568"])),
            (
                "artist-credit",
                json!([{"artist":{"id":"choir"}}, {"artist":{}}]),
            ),
        ] {
            let mut changed = raw.clone();
            changed[field] = value;
            assert!(
                recording_titles(&occurrence, &changed).is_empty(),
                "{field}"
            );
        }
    }

    #[test]
    fn artist_aliases_keep_guests_and_ignore_other_artist_profiles() {
        let raw = json!({"artist-credit":[
            {"name":"Choir","artist":{"id":"choir"},"joinphrase":" feat. "},
            {"name":"Guest","artist":{"id":"guest"},"joinphrase":""}]});
        let profiles = vec![
            json!({"id":"choir","aliases":[{"name":"Хор"}]}),
            json!({"id":"other","aliases":[{"name":"Wrong choir"}]}),
        ];
        let variants = artist_renderings(&raw, &profiles);
        assert!(variants.contains(&"Хор feat. Guest".to_owned()));
        assert!(variants.iter().all(|name| name.ends_with(" feat. Guest")));
        assert!(!variants.iter().any(|name| name.contains("Wrong")));
    }
}
