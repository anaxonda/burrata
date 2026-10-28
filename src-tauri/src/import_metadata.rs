//! Public page metadata is evidence, never proof of recording identity.
use serde_json::{json, Map, Value};

pub fn validate(value: &Value) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let object = value
        .as_object()
        .ok_or("Import metadata must be an object")?;
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 48_000 {
        return Err("Import metadata exceeds 48 KB".into());
    }
    for (key, value) in object {
        if value.is_null() {
            continue;
        }
        match key.as_str() {
            "albumUrl" | "artistUrl" | "channelUrl" | "labelUrl" => {
                public_url(value.as_str().ok_or("Page links must be strings")?)?;
            }
            "artistUrls" => {
                let values = value.as_array().ok_or("Artist links must be an array")?;
                if values.len() > 50 {
                    return Err("Too many artist links".into());
                }
                for value in values {
                    public_url(value.as_str().ok_or("Artist links must be strings")?)?;
                }
            }
            "tags" | "remixers" => {
                let values = value.as_array().ok_or("Tags and remixers must be arrays")?;
                if values.len() > 100
                    || values
                        .iter()
                        .any(|v| v.as_str().is_none_or(|s| s.len() > 500))
                {
                    return Err("Invalid tags or remixers".into());
                }
            }
            "trackNumber" | "discNumber" | "albumOrder" => {
                if value.as_u64().is_none_or(|n| n == 0 || n > 10000) {
                    return Err("Invalid import position".into());
                }
            }
            "plays" | "listeners" => {
                if value.as_u64().is_none() {
                    return Err("Invalid public count".into());
                }
            }
            "description" | "credits" | "lyrics" | "artistBio" => {
                if value.as_str().is_none_or(|s| s.len() > 12000) {
                    return Err("Import description is too long".into());
                }
            }
            "albumArtist" | "location" | "musicbrainzId" | "channelName" | "videoTitle"
            | "availability" => {
                if value.as_str().is_none_or(|s| s.len() > 2000) {
                    return Err("Invalid import text".into());
                }
            }
            _ => return Err(format!("Unsupported import metadata field: {key}")),
        }
    }
    Ok(())
}

pub fn public_url(raw: &str) -> Result<String, String> {
    let mut url = url::Url::parse(raw).map_err(|_| "Invalid public page URL")?;
    if raw.len() > 4096
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
    {
        return Err("Public page links must use HTTPS without credentials".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn field_sources(
    entry: &crate::browser_bridge::BrowserEntry,
    provider: &str,
    captured_at: &str,
    page_source: &str,
) -> Value {
    let mut sources = Map::new();
    if let Ok(Value::Object(fields)) = serde_json::to_value(entry) {
        for (field, value) in fields {
            if !value.is_null() && !matches!(field.as_str(), "metadata" | "position" | "rank") {
                sources.insert(field, json!({"provider":provider,"url":entry.source_url.as_deref().unwrap_or(page_source),"capturedAt":captured_at,"verified":false}));
            }
        }
    }
    if let Some(fields) = entry.metadata.as_object() {
        for (field, value) in fields {
            if !value.is_null() {
                sources.insert(format!("importMetadata.{field}"),json!({"provider":provider,"url":entry.source_url.as_deref().unwrap_or(page_source),"capturedAt":captured_at,"verified":false}));
            }
        }
    }
    Value::Object(sources)
}

pub fn source_context(provider: &str, raw: &str, kind: &str, captured: &str) -> Value {
    let url = url::Url::parse(raw).ok();
    let parts = url
        .as_ref()
        .map(|u| {
            u.path()
                .split('/')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut context = json!({"url":raw,"kind":kind,"capturedAt":captured});
    if provider == "shazam" {
        context["chartCountry"] = json!(parts.last());
    }
    if (provider == "lastfm" && parts.first() == Some(&"tag"))
        || (provider == "bandcamp" && parts.first() == Some(&"discover"))
    {
        context["tag"] = json!(parts.get(1));
    }
    if let Some(url) = url {
        for (key, value) in url.query_pairs() {
            if matches!(
                key.as_ref(),
                "period" | "date_preset" | "from" | "to" | "date_from" | "date_to" | "sort"
            ) {
                context[key.as_ref()] = json!(value);
            }
        }
    }
    context
}

/// Fill gaps without replacing richer evidence or explicit user changes.
pub fn merge_evidence(old: &Value, incoming: &mut Value) {
    let Some(new) = incoming.as_object_mut() else {
        return;
    };
    let Some(previous) = old.as_object() else {
        return;
    };
    for (key, value) in previous {
        if key == "provenance" {
            continue;
        }
        let edited = old["provenance"]["manualFields"][key].as_bool() == Some(true);
        let missing = new
            .get(key)
            .is_none_or(|v| v.is_null() || v.as_str() == Some(""));
        if edited || (missing && !value.is_null()) {
            new.insert(key.clone(), value.clone());
        }
    }
    let evidence = new.entry("provenance").or_insert_with(|| json!({}));
    if !evidence.is_object() {
        *evidence = json!({});
    }
    if old["provenance"]["manualFields"]["playbackMode"] == true {
        evidence["playbackMode"] = old["provenance"]["playbackMode"].clone();
    }
    for key in ["manualFields", "importMetadata", "fieldSources"] {
        let mut merged = old["provenance"][key]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if let Some(fields) = evidence[key].as_object() {
            for (field, value) in fields {
                if !value.is_null()
                    && !(key == "fieldSources" && old["provenance"]["manualFields"][field] == true)
                    && (key != "manualFields" || !merged.contains_key(field))
                {
                    merged.insert(field.clone(), value.clone());
                }
            }
        }
        if !merged.is_empty() {
            evidence[key] = Value::Object(merged);
        }
    }
}

pub fn bandcamp_page(data: &Value, html: &str, page: &url::Url) -> Value {
    let document = scraper::Html::parse_document(html);
    let text = |selector: &str| -> Option<String> {
        let selector = scraper::Selector::parse(selector).ok()?;
        let value = document
            .select(&selector)
            .next()?
            .text()
            .collect::<String>();
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.chars().take(12000).collect())
    };
    let attr = |selector: &str, name: &str| -> Option<String> {
        let selector = scraper::Selector::parse(selector).ok()?;
        document
            .select(&selector)
            .next()?
            .value()
            .attr(name)
            .map(str::to_owned)
    };
    let bounded = |v: &Value| {
        v.as_str()
            .map(|s| s.chars().take(12000).collect::<String>())
    };
    let image = attr("meta[property='og:image']", "content")
        .or_else(|| data["art_url"].as_str().map(str::to_owned))
        .or_else(|| {
            data["art_id"]
                .as_u64()
                .or_else(|| data["art"].as_u64())
                .map(|id| format!("https://f4.bcbits.com/img/a{id}_10.jpg"))
        })
        .and_then(|s| public_url(&s).ok())
        .filter(|s| {
            url::Url::parse(s)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .is_some_and(|h| h.ends_with(".bcbits.com"))
        });
    let tags = scraper::Selector::parse("a.tag")
        .ok()
        .map(|selector| {
            document
                .select(&selector)
                .take(100)
                .map(|node| {
                    node.text()
                        .collect::<String>()
                        .chars()
                        .take(500)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let album_path = data["album_url"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| attr(".fromAlbum a[href*='/album/']", "href"));
    let album_url = album_path
        .as_deref()
        .and_then(|s| page.join(s).ok())
        .filter(|u| u.host_str() == page.host_str())
        .map(|u| u.to_string())
        .or_else(|| page.path().starts_with("/album/").then(|| page.to_string()));
    let album = data["album_title"]
        .as_str()
        .or_else(|| {
            data["current"]["title"]
                .as_str()
                .filter(|_| page.path().starts_with("/album/"))
        })
        .map(str::to_owned)
        .or_else(|| text(".fromAlbum a[href*='/album/']"));
    json!({"artworkUrl":image,"releaseDate":data["album_release_date"].as_str().or_else(||data["release_date"].as_str()),"album":album,"metadata":{
        "albumUrl":album_url,"artistUrl":format!("{}/",page.origin().ascii_serialization()),"albumArtist":data["artist"].as_str(),
        "description":bounded(&data["current"]["about"]).or_else(||text(".tralbum-about")),"credits":bounded(&data["current"]["credits"]).or_else(||text(".tralbum-credits")),"lyrics":bounded(&data["current"]["lyrics"]),"artistBio":text("#bio-text"),"location":text(".location").map(|s|s.chars().take(2000).collect::<String>()),"tags":tags}})
}

pub fn fill_bandcamp(
    store: &crate::storage::Store,
    track: &crate::models::Track,
    details: &Value,
    source: &str,
) -> Result<crate::models::Track, String> {
    validate(&details["metadata"])?;
    let mut updated = store
        .get_track(&track.id)?
        .ok_or("Saved track no longer exists")?;
    let mut changed = Map::new();
    for (name, slot) in [
        ("artworkUrl", &mut updated.artwork_url),
        ("album", &mut updated.album),
        ("releaseDate", &mut updated.release_date),
    ] {
        if slot.as_ref().is_none_or(|s| s.trim().is_empty())
            && updated.provenance["manualFields"][name] != true
        {
            if let Some(value) = details[name].as_str() {
                *slot = Some(value.to_owned());
                changed.insert(name.into(),json!({"provider":"bandcamp","url":source,"capturedAt":chrono::Utc::now().to_rfc3339(),"verified":false}));
            }
        }
    }
    if !updated.provenance.is_object() {
        updated.provenance = json!({});
    }
    let mut extra = updated.provenance["importMetadata"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = details["metadata"].as_object() {
        for (key, value) in fields {
            if !value.is_null() && extra.get(key).is_none_or(Value::is_null) {
                extra.insert(key.clone(), value.clone());
                changed.insert(format!("importMetadata.{key}"),json!({"provider":"bandcamp","url":source,"capturedAt":chrono::Utc::now().to_rfc3339(),"verified":false}));
            }
        }
    }
    updated.provenance["importMetadata"] = Value::Object(extra);
    let mut sources = updated.provenance["fieldSources"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    sources.extend(changed);
    updated.provenance["fieldSources"] = Value::Object(sources);
    store.cache_track(&updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_metadata_is_bounded_and_does_not_accept_transport_or_private_fields() {
        assert!(validate(&json!({"albumUrl":"https://artist.bandcamp.com/album/test","tags":["alternative"],"plays":12})).is_ok());
        for value in [
            json!({"streamUrl":"https://cdn.invalid/file?token=secret"}),
            json!({"cookies":"secret"}),
            json!({"albumUrl":"https://user:pass@example.com/album"}),
            json!({"tags":[{}]}),
            json!({"listeners":-1}),
        ] {
            assert!(validate(&value).is_err());
        }
    }
    #[test]
    fn refresh_preserves_manual_values_and_missing_source_evidence() {
        let old = json!({"title":"Edited title","artworkUrl":"https://images.invalid/cover.jpg","provenance":{"manualFields":{"title":true},"importMetadata":{"credits":"Original credits"}}});
        let mut next = json!({"title":"Provider title","artworkUrl":null,"album":"New album","provenance":{"importMetadata":{"albumUrl":"https://artist.bandcamp.com/album/release"}}});
        merge_evidence(&old, &mut next);
        assert_eq!(next["title"], "Edited title");
        assert_eq!(next["artworkUrl"], old["artworkUrl"]);
        assert_eq!(
            next["provenance"]["importMetadata"]["credits"],
            "Original credits"
        );
        assert_eq!(next["album"], "New album");
    }
    #[test]
    fn track_page_retains_its_explicit_album_link() {
        let page = url::Url::parse("https://artist.bandcamp.com/track/song").unwrap();
        let details = bandcamp_page(
            &json!({"artist":"Artist"}),
            "<span class='fromAlbum'>from <a href='/album/release'>Release</a></span>",
            &page,
        );
        assert_eq!(details["album"], "Release");
        assert_eq!(
            details["metadata"]["albumUrl"],
            "https://artist.bandcamp.com/album/release"
        );
    }
    #[test]
    fn bandcamp_page_extracts_public_metadata_without_stream_secrets() {
        let page = url::Url::parse("https://artist.bandcamp.com/album/release").unwrap();
        let data = json!({"artist":"Artist","art_id":123,"album_title":"Release","album_release_date":"2026-10-01","current":{"about":"Description","credits":"Credits"},"trackinfo":[{"file":{"mp3-128":"https://cdn.invalid/audio?token=SECRET"}}]});
        let result = bandcamp_page(&data, "<a class='tag'>alternative</a>", &page);
        assert_eq!(
            result["artworkUrl"],
            "https://f4.bcbits.com/img/a123_10.jpg"
        );
        assert_eq!(
            result["metadata"]["artistUrl"],
            "https://artist.bandcamp.com/"
        );
        assert_eq!(result["metadata"]["tags"], json!(["alternative"]));
        assert!(!result.to_string().contains("SECRET"));
        validate(&result["metadata"]).unwrap();
    }
}
