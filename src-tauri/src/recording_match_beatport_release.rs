//! Request-scoped evidence for a label upload whose title omits "Extended".
use super::{
    caption_splits, collaboration_caption_credits, label_key, native_catalog_annotation_source,
    norm_artist, semantic_version, structured_primary_credits, title_key, valid_duration,
};
use crate::{
    models::{Candidate, Track},
    recording_identity::{numbered_remix_name, parse_title, RecordingIdentity},
};
use serde_json::Value;

fn numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn public_https(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Retain only an unambiguous, first-party release ID, never the description.
pub(crate) fn soundcloud_purchase_release_id(info: &Value) -> Option<String> {
    let description = info["description"].as_str()?;
    if description.len() > 8192 {
        return None;
    }
    let mut links = description.lines().filter_map(|line| {
        let (purpose, link) = line.split_once(':')?;
        purpose
            .trim()
            .eq_ignore_ascii_case("buy the track")
            .then_some(link.trim())
    });
    let url = url::Url::parse(links.next()?).ok()?;
    if links.next().is_some()
        || !public_https(&url)
        || !matches!(url.host_str(), Some("www.beatport.com" | "beatport.com"))
    {
        return None;
    }
    let mut parts = url.path_segments()?;
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("release"), Some(slug), Some(id), None) if !slug.is_empty() && numeric_id(id) => {
            Some(id.to_owned())
        }
        _ => None,
    }
}

fn source_is_public_track(candidate: &Candidate, meta: &Value) -> bool {
    let Ok(url) = url::Url::parse(&candidate.url) else {
        return false;
    };
    let Some(mut parts) = url.path_segments() else {
        return false;
    };
    public_https(&url)
        && matches!(
            url.host_str(),
            Some("soundcloud.com" | "www.soundcloud.com")
        )
        && matches!((parts.next(), parts.next(), parts.next()), (Some(user), Some(track), None)
            if !user.is_empty() && !track.is_empty() && track != "sets")
        && candidate
            .id
            .strip_prefix("soundcloud:")
            .is_some_and(|id| numeric_id(id) && meta["resolvedProviderId"].as_str() == Some(id))
}

pub(super) struct ReleaseCaption {
    pub(super) credits: Vec<String>,
    pub(super) fresh: bool,
}

pub(super) fn release_caption(
    track: &Track,
    candidate: &Candidate,
    meta: &Value,
    requested_versions: &[String],
    observed_versions: &[String],
) -> Option<ReleaseCaption> {
    if track.provider != "beatport"
        || candidate.provider != "soundcloud"
        || meta["provider"] != "soundcloud"
        || meta["identityMetadataUncertain"] == true
        || meta["isPreview"] == true
        || meta["isFullAlbum"] == true
        || candidate.permission == "restricted"
        || candidate
            .chapters
            .as_ref()
            .is_some_and(|chapters| !chapters.is_empty())
        || track.playback_range.is_some()
        || [
            "title",
            "artist",
            "artistCredits",
            "version",
            "durationSeconds",
            "label",
            "album",
        ]
        .iter()
        .any(|field| track.provenance["manualFields"][*field] == true)
    {
        return None;
    }
    let extracted_release = meta["soundcloudBeatportReleaseId"]
        .as_str()
        .filter(|id| numeric_id(id))?;
    let fresh = match meta["sourceMetadataOrigin"].as_str()? {
        "ytDlpFullInfo" => true,
        "ytDlpCandidateMetadata" => false,
        _ => return None,
    };
    if !source_is_public_track(candidate, meta) {
        return None;
    }
    let projected;
    let details = if native_catalog_annotation_source(track) {
        track
    } else {
        projected =
            serde_json::from_value::<Track>(track.provenance["playbackCatalogAnnotations"].clone())
                .ok()?;
        if !native_catalog_annotation_source(&projected)
            || projected.provider_id != track.provider_id
        {
            return None;
        }
        &projected
    };
    if details.provenance["releaseId"].as_str() != Some(extracted_release)
        || details.title != track.title
        || details.artist != track.artist
        || details.version != track.version
        || details.duration_seconds != track.duration_seconds
    {
        return None;
    }
    let duration = valid_duration(details.duration_seconds).filter(|duration| *duration > 30.)?;
    let candidate_duration = valid_duration(candidate.duration_seconds)?;
    let extracted_duration = valid_duration(meta["duration"].as_f64())?;
    if (duration - candidate_duration).abs() > 1.0
        || (candidate_duration - extracted_duration).abs() >= 0.001
    {
        return None;
    }
    let label = label_key(details.label.as_deref()?);
    let uploader = meta["uploader"].as_str()?;
    let extracted_names = super::artist_array_observation(meta);
    // This extractor may put the uploader in an artist field. Interpret that
    // field as publisher context only when it is the sole name on this exact
    // release link. An independent performer or malformed list still vetoes.
    if label.is_empty()
        || label_key(uploader) != label
        || candidate
            .uploader
            .as_deref()
            .is_none_or(|name| label_key(name) != label)
        || extracted_names.uncertain
        || extracted_names.names.len() != 1
        || label_key(&extracted_names.names[0]) != label
        || ["artist", "catalogArtist"].iter().any(|field| {
            meta[*field]
                .as_str()
                .is_some_and(|name| label_key(name) != label)
        })
    {
        return None;
    }
    let ([requested], [observed]) = (requested_versions, observed_versions) else {
        return None;
    };
    let shorter = requested.strip_suffix(" extended")?;
    let remixer = numbered_remix_name(shorter, ' ')?;
    if observed != shorter
        || semantic_version(&details.title, details.version.as_deref()?) != *requested
        || !super::guest_from_title(&track.title).is_empty()
        || RecordingIdentity::from_track(track)
            .credits
            .iter()
            .any(|credit| crate::recording_identity::is_guest_performer_role(&credit.role))
    {
        return None;
    }
    let catalog_credits = structured_primary_credits(track, &track.artist);
    let performers = catalog_credits
        .iter()
        .filter(|name| norm_artist(name) != norm_artist(remixer))
        .cloned()
        .collect::<Vec<_>>();
    let caption = meta["track"].as_str()?;
    if caption != candidate.title
        || performers.is_empty()
        || performers.len() > 16
        || catalog_credits.len().saturating_sub(performers.len()) > 1
    {
        return None;
    }
    let (mut credits, mut splits) = (None, caption_splits(caption, true));
    for (artist, title) in &mut splits {
        let title = parse_title(title);
        if title_key(&title.title) == title_key(&track.title)
            && title.featured_credit.is_none()
            && title
                .version
                .as_deref()
                .is_some_and(|version| semantic_version(&title.title, version) == shorter)
        {
            let names = collaboration_caption_credits(artist, &performers);
            if !names.is_empty() && credits.replace(names).is_some() {
                return None;
            }
        }
    }
    let mut credits = credits?;
    // The exact edition itself names this remixer if the catalog also lists
    // the same name among the performers.
    credits.extend(
        catalog_credits
            .into_iter()
            .filter(|name| norm_artist(name) == norm_artist(remixer)),
    );
    Some(ReleaseCaption { credits, fresh })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording_match::{assess, Comparison, MatchAction, MatchLevel, MatchStage};
    use serde_json::json;

    const DESCRIPTION: &str = "BUY THE TRACK: https://www.beatport.com/release/like-dat-ape-drums-remix-20-extended/7587008\n\nLISTEN TO OUR PLAYLISTS";
    const CAPTION: &str = "DANIDANE, MAAURA - LIKE DAT (APE DRUMS REMIX 2.0)";

    fn source(origin: &str) -> Candidate {
        Candidate {
            id: "soundcloud:2411098299".into(),
            provider: "soundcloud".into(),
            url: "https://soundcloud.com/klub-record/danidance-maaura-like-dat-ape".into(),
            title: CAPTION.into(),
            uploader: Some("KLUB RECORD".into()),
            duration_seconds: Some(425.635),
            source_metadata: Some(json!({
                "provider":"soundcloud", "sourceMetadataOrigin":origin,
                "resolvedProviderId":"2411098299", "track":CAPTION,
                "artists":["KLUB RECORD"], "uploader":"KLUB RECORD", "duration":425.635,
                "soundcloudBeatportReleaseId":soundcloud_purchase_release_id(&json!({"description":DESCRIPTION}))
            })),
            ..Default::default()
        }
    }

    fn details() -> Track {
        Track {
            id: "beatport:30967065".into(),
            provider: "beatport".into(),
            provider_id: "30967065".into(),
            source_url: "https://www.beatport.com/track/like-dat/30967065".into(),
            artist: "Danidane, MAAURA".into(),
            title: "Like Dat".into(),
            version: Some("Ape Drums Remix 2.0 Extended".into()),
            label: Some("Klub Record".into()),
            duration_seconds: Some(425.0),
            provenance: json!({
                "catalog":"beatport", "catalogProvider":"beatport",
                "releaseId":"7587008", "raw":{"id":"30967065"},
            }),
            ..Default::default()
        }
    }

    #[test]
    fn purchase_release_link_is_unique_exact_and_public() {
        let parse =
            |description| soundcloud_purchase_release_id(&json!({"description":description}));
        assert_eq!(parse(DESCRIPTION.to_owned()).as_deref(), Some("7587008"));
        for description in [
            DESCRIPTION.replace("www.beatport.com", "www.beatport.com.example"),
            DESCRIPTION.replace("https://", "https://name@"),
            DESCRIPTION.replace("7587008", "7587008?key=private"),
            DESCRIPTION.replace("/release/", "/track/"),
            format!("{DESCRIPTION}\nBUY THE TRACK: https://beatport.com/release/other/1"),
        ] {
            assert_eq!(parse(description), None);
        }
    }

    #[test]
    fn extended_numbered_remix_requires_fresh_label_upload_bound_to_verified_release() {
        let requested = details();
        let flat = source("ytDlpCandidateMetadata");
        let fresh = source("ytDlpFullInfo");
        let check = |track: &Track, candidate: &Candidate, stage| {
            assess(track, candidate, MatchLevel::Balanced, stage)
        };
        assert_eq!(
            check(&requested, &flat, MatchStage::Search).action,
            MatchAction::Inspect
        );
        assert_ne!(
            check(&requested, &flat, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        let matched = check(&requested, &fresh, MatchStage::Resolved);
        assert_eq!(matched.dimensions.version, Comparison::Agree, "{matched:?}");
        assert_eq!(matched.action, MatchAction::Accept, "{matched:?}");

        let mut legacy = requested.clone();
        legacy.provenance =
            json!({"sourceKind":"chart", "browserImport":true, "captureProvider":"beatport"});
        legacy.label = Some("Klub RecordApe Drums".into());
        assert_ne!(
            check(&legacy, &fresh, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        legacy.provenance["playbackCatalogAnnotations"] = serde_json::to_value(&requested).unwrap();
        assert_eq!(
            check(&legacy, &fresh, MatchStage::Resolved).action,
            MatchAction::Accept
        );

        for (field, mismatch) in [
            ("soundcloudBeatportReleaseId", json!("12")),
            ("resolvedProviderId", json!("12")),
            ("artists", json!(["KLUB RECORD", "Other performer"])),
            ("artist", json!("Other performer")),
            ("uploader", json!("Other label")),
            ("duration", json!(294.0)),
            ("identityMetadataUncertain", json!(true)),
            ("isPreview", json!(true)),
            ("isFullAlbum", json!(true)),
        ] {
            let mut changed = fresh.clone();
            changed.source_metadata.as_mut().unwrap()[field] = mismatch;
            assert_ne!(
                check(&requested, &changed, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{field}"
            );
        }
        let mut wrong_edition = fresh.clone();
        wrong_edition.title = CAPTION.replace("2.0", "3.0");
        wrong_edition.source_metadata.as_mut().unwrap()["track"] = json!(wrong_edition.title);
        assert_ne!(
            check(&requested, &wrong_edition, MatchStage::Resolved).action,
            MatchAction::Accept
        );
        let mut wrong_credits = fresh.clone();
        wrong_credits.title = CAPTION.replace("MAAURA", "OTHER");
        wrong_credits.source_metadata.as_mut().unwrap()["track"] = json!(wrong_credits.title);
        assert_ne!(
            check(&requested, &wrong_credits, MatchStage::Resolved).action,
            MatchAction::Accept
        );

        let mut short_youtube = fresh;
        short_youtube.id = "youtube:UT4KozFa7KY".into();
        short_youtube.provider = "youtube".into();
        short_youtube.url = "https://www.youtube.com/watch?v=UT4KozFa7KY".into();
        short_youtube.duration_seconds = Some(294.0);
        short_youtube.source_metadata = None;
        assert_ne!(
            check(&requested, &short_youtube, MatchStage::Resolved).action,
            MatchAction::Accept
        );
    }
}
