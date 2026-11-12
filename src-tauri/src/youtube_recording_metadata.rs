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
fn channel_id(value: &str) -> bool {
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
    let album = text(&meta["catalogAlbum"], 512)?;
    if key(album) != key(text(&meta["catalogRequestedAlbum"], 512)?) {
        return None;
    }
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
    text(&meta["catalogTitle"], 512)?;
    Some(names)
}

/// Research the exact album's linked item before a caption-only upload. This
/// grants extraction priority, never permission to skip fresh source verification.
pub(crate) fn preferred_album_locator(track: &Track, candidate: &Candidate) -> bool {
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
        "music-album-credits-1",
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
            let explicit = [
                "recorded live ",
                "recorded live.",
                "grabado en vivo ",
                "grabada en vivo ",
                "live at ",
                "live from ",
                "en vivo en ",
                "en vivo desde ",
            ]
            .iter()
            .any(|p| folded.starts_with(p));
            let dated_venue = folded.chars().filter(char::is_ascii_digit).count() >= 6
                && [
                    " tocando en el teatro ",
                    " tocando en el estadio ",
                    " tocando en el auditorio ",
                    " performing at the theatre ",
                    " performing at the theater ",
                ]
                .iter()
                .any(|p| folded.contains(p));
            explicit || dated_venue
        })
        .map(str::to_owned)
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
        wrong.source_metadata.as_mut().unwrap()["catalogRequestedAlbum"] = json!("Live album");
        assert!(catalog_identity(&wrong, &f["officialFullInfo"]).is_none());
        wrong = c;
        wrong.source_metadata.as_mut().unwrap()["catalogArtistCredits"][0]["providerId"] =
            Value::Null;
        assert!(catalog_identity(&wrong, &f["officialFullInfo"]).is_none());
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
        ] {
            assert!(
                live_performance_line(&json!({"description":line})).is_none(),
                "{line}"
            );
        }
    }
}
