//! Recording metadata suggestions. Results carry field provenance and always
//! require a user's per-field review before the app applies them.
use crate::{
    artists::{
        ArtistCredentials, ArtistCredit, ArtistRef, ArtistReleaseRef, ArtistReleaseTrack,
        ArtistService,
    },
    models::Track,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strsim::jaro_winkler;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataReviewField {
    pub key: String,
    pub value: Option<String>,
    pub source: String,
    pub source_url: String,
    pub review_required: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRecordingMatch {
    pub recording: ArtistRef,
    pub title: String,
    pub artist: String,
    pub score: f64,
    pub first_release_date: Option<String>,
    pub isrcs: Vec<String>,
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataReview {
    pub source_track_id: String,
    pub recording: Option<ArtistRecordingMatch>,
    pub recording_candidates: Vec<ArtistRecordingMatch>,
    pub release_ref: Option<ArtistReleaseRef>,
    pub release_tracklist: Vec<ArtistReleaseTrack>,
    pub matched_release_track: Option<String>,
    pub fields: Vec<MetadataReviewField>,
    pub credits: Vec<ArtistCredit>,
    /// The recording's first known release date, distinct from this edition's date/year.
    pub recording_first_release_date: Option<String>,
    pub release_date: Option<String>,
    pub release_year: Option<u32>,
    pub selected_version_preserved: Option<String>,
    pub warnings: Vec<String>,
}

pub async fn resolve_recording_metadata(
    service: &ArtistService,
    track: &Track,
    release: Option<&ArtistReleaseRef>,
    recording: Option<&ArtistRef>,
    credentials: &ArtistCredentials,
) -> Result<MetadataReview, String> {
    if track.id.trim().is_empty() || track.title.trim().is_empty() {
        return Err("Select a recording with a title before resolving metadata".into());
    }
    if let Some(release) = release {
        validate_release_ref(&release.release)?;
        let (release_data, source, source_url) = match release.release.provider.as_str() {
            "musicbrainz" => {
                let json = service
                    .json_for_metadata(
                        "musicbrainz",
                        &format!(
                            "release/{}?inc=artist-credits+labels+recordings+media+isrcs+genres",
                            release.release.id
                        ),
                        "metadata.release.musicbrainz",
                        &release.release.id,
                        None,
                    )
                    .await?;
                (
                    json,
                    "MusicBrainz",
                    format!("https://musicbrainz.org/release/{}", release.release.id),
                )
            }
            "discogs" => {
                let is_master = release.source_url.contains("/master/");
                let (path, source_url) = if is_master {
                    (
                        format!("masters/{}", release.release.id),
                        format!("https://www.discogs.com/master/{}", release.release.id),
                    )
                } else {
                    (
                        format!("releases/{}", release.release.id),
                        format!("https://www.discogs.com/release/{}", release.release.id),
                    )
                };
                let json = service
                    .json_for_metadata(
                        "discogs",
                        &path,
                        "metadata.release.discogs",
                        &release.release.id,
                        credentials.discogs_token.as_deref(),
                    )
                    .await?;
                (json, "Discogs", source_url)
            }
            _ => return Err("Unsupported release provider".into()),
        };
        resolve_from_release(service, track, release, &release_data, source, &source_url).await
    } else if let Some(recording_ref) = recording {
        if recording_ref.provider != "musicbrainz" || !valid_mbid(&recording_ref.id) {
            return Err("Recording identity must be a valid MusicBrainz ID".into());
        }
        let value = service
            .json_for_metadata(
                "musicbrainz",
                &format!(
                    "recording/{}?inc=artist-credits+isrcs+genres+releases",
                    recording_ref.id
                ),
                "metadata.recording.musicbrainz",
                &recording_ref.id,
                None,
            )
            .await?;
        let candidate = parse_recording(&value, track);
        match candidate {
            Some(candidate) => resolve_from_recording(track, candidate, &value).await,
            None => Err("MusicBrainz recording response omitted its identity".into()),
        }
    } else {
        let data = service.recording_search(track).await?;
        let candidates = data["recordings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| parse_recording(item, track))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(MetadataReview {
                source_track_id: track.id.clone(),
                recording: None,
                recording_candidates: Vec::new(),
                release_ref: None,
                release_tracklist: Vec::new(),
                matched_release_track: None,
                fields: Vec::new(),
                credits: Vec::new(),
                recording_first_release_date: None,
                release_date: None,
                release_year: None,
                selected_version_preserved: track.version.clone(),
                warnings: vec![
                    "No MusicBrainz recording candidates were found. No metadata was changed."
                        .into(),
                ],
            });
        }
        Ok(MetadataReview {
            source_track_id: track.id.clone(),
            recording: None,
            recording_candidates: candidates,
            release_ref: None,
            release_tracklist: Vec::new(),
            matched_release_track: None,
            fields: Vec::new(),
            credits: Vec::new(),
            recording_first_release_date: None,
            release_date: None,
            release_year: None,
            selected_version_preserved: track.version.clone(),
            warnings: vec!["Choose the exact recording identity before loading field suggestions. Similar artist and title text is not treated as a verified identity.".into()],
        })
    }
}

async fn resolve_from_release(
    service: &ArtistService,
    track: &Track,
    release_ref: &ArtistReleaseRef,
    data: &Value,
    source: &str,
    source_url: &str,
) -> Result<MetadataReview, String> {
    let (
        edition_title,
        edition_date,
        edition_year,
        _edition_artist,
        label,
        catalog_number,
        edition_genres,
        edition_artwork,
        _credits,
        track_entries,
    ) = if release_ref.release.provider == "musicbrainz" {
        let edition_title = data["title"]
            .as_str()
            .unwrap_or(&release_ref.title)
            .to_owned();
        let date = data["date"].as_str().map(str::to_owned);
        let year = date.as_deref().and_then(parse_year).or(release_ref.year);
        let artist_credits = parse_mb_credits(&data["artist-credit"], source_url);
        let artist = render_credits(&artist_credits);
        let label_info = data["label-info"]
            .as_array()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let label = label_info
            .iter()
            .find_map(|entry| entry["label"]["name"].as_str())
            .map(str::to_owned)
            .or_else(|| release_ref.label.clone());
        let catalog = label_info
            .iter()
            .find_map(|entry| entry["catalog-number"].as_str())
            .map(str::to_owned)
            .or_else(|| release_ref.catalog_number.clone());
        let genres = names(&data["genres"]);
        let artwork = if release_ref.release.provider == "musicbrainz" {
            service
                .json_for_metadata(
                    "coverart",
                    &format!("release/{}", release_ref.release.id),
                    "metadata.coverart",
                    &release_ref.release.id,
                    None,
                )
                .await
                .ok()
                .and_then(|response| {
                    response["images"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|image| image["front"].as_bool().unwrap_or(false))
                        .and_then(|image| image["image"].as_str().and_then(secure_https))
                })
        } else {
            None
        };
        (
            edition_title,
            date,
            year,
            artist,
            label,
            catalog,
            genres,
            artwork,
            artist_credits,
            mb_track_entries(&data["media"]),
        )
    } else {
        let edition_title = data["title"]
            .as_str()
            .unwrap_or(&release_ref.title)
            .to_owned();
        let date = data["released"]
            .as_str()
            .map(str::to_owned)
            .filter(|v| !v.is_empty());
        let year = data["year"]
            .as_u64()
            .map(|v| v as u32)
            .or_else(|| date.as_deref().and_then(parse_year))
            .or(release_ref.year);
        let artist_credits = discogs_credits(&data["artists"], "artist", source_url);
        let mut credits = artist_credits.clone();
        credits.extend(discogs_credits(&data["extraartists"], "credit", source_url));
        let artist = render_credits(&artist_credits);
        let labels = data["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let label = labels
            .first()
            .and_then(|entry| entry["name"].as_str())
            .map(str::to_owned)
            .or_else(|| release_ref.label.clone());
        let catalog = labels
            .first()
            .and_then(|entry| entry["catno"].as_str())
            .map(str::to_owned)
            .or_else(|| release_ref.catalog_number.clone());
        let mut genres = names(&data["genres"]);
        genres.extend(names(&data["styles"]));
        let artwork = data["images"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|image| image["type"].as_str() == Some("primary"))
            .and_then(|image| image["uri"].as_str().and_then(secure_https));
        (
            edition_title,
            date,
            year,
            artist,
            label,
            catalog,
            genres,
            artwork,
            credits,
            discogs_track_entries(&data["tracklist"]),
        )
    };

    let scored = track_entries
        .iter()
        .map(|entry| {
            let title = entry["title"]
                .as_str()
                .or(entry["recording"]["title"].as_str())
                .unwrap_or("");
            (
                jaro_winkler(&normalize(&track.title), &normalize(title)),
                entry,
            )
        })
        .collect::<Vec<_>>();
    let best_score = scored
        .iter()
        .map(|(score, _)| *score)
        .max_by(f64::total_cmp)
        .unwrap_or(0.0);
    let tied_matches = scored
        .iter()
        .filter(|(score, _)| *score >= 0.72 && best_score - *score <= 0.01)
        .count();
    let ambiguous_match = tied_matches > 1;
    let matched = if ambiguous_match {
        None
    } else {
        scored.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))
    };
    let mut warnings = Vec::new();
    let (match_score, matched_entry) = match matched {
        Some((score, entry)) => (score, Some(entry)),
        None => (0.0, None),
    };
    if matched_entry.is_none() || match_score < 0.72 {
        warnings.push("The selected edition does not have a clear track-title match. Review every suggestion carefully.".into());
    } else if match_score < 0.95 {
        warnings.push("A possible release-track match is shown for review. The resolver did not select or infer a mix/version automatically.".into());
    }
    if ambiguous_match {
        warnings.push("Several release tracks match this title. No track-specific metadata was selected; verify the exact track and mix from the tracklist.".into());
    }
    if track
        .version
        .as_ref()
        .is_some_and(|version| !version.trim().is_empty())
    {
        warnings.push("The selected track version is preserved separately; release track titles were not used to replace or infer it.".into());
    }
    let track_title = matched_entry
        .and_then(|entry| {
            entry["recording"]["title"]
                .as_str()
                .or(entry["title"].as_str())
        })
        .unwrap_or(&track.title)
        .to_owned();
    let recording_data = matched_entry.and_then(|entry| entry.get("recording"));
    let recording_first_release_date = recording_data
        .and_then(|value| value["first-release-date"].as_str())
        .map(str::to_owned);
    let track_credits = matched_entry
        .map(|entry| {
            if release_ref.release.provider == "musicbrainz" {
                parse_mb_credits(&entry["artist-credit"], source_url)
            } else {
                let mut credits = discogs_credits(&entry["artists"], "artist", source_url);
                credits.extend(discogs_credits(
                    &entry["extraartists"],
                    "credit",
                    source_url,
                ));
                credits
            }
        })
        .filter(|credits| !credits.is_empty())
        .unwrap_or_default();
    let recording_artist = render_credits(&track_credits);
    let release_tracklist = track_entries
        .iter()
        .filter_map(|entry| {
            let title = entry["title"]
                .as_str()
                .or(entry["recording"]["title"].as_str())?;
            Some(ArtistReleaseTrack {
                position: entry["position"].as_str().map(str::to_owned),
                title: title.to_owned(),
                source_url: source_url.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let isrcs = recording_data
        .and_then(|value| value["isrcs"].as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            matched_entry.and_then(|entry| entry["isrc"].as_str().map(|s| vec![s.to_owned()]))
        })
        .unwrap_or_default();
    let source_url_recording = recording_data
        .and_then(|value| value["id"].as_str())
        .map(|id| format!("https://musicbrainz.org/recording/{id}"));
    let mut fields = Vec::new();
    if !recording_artist.is_empty() {
        fields.push(field("artist", recording_artist.clone(), source, source_url_recording.as_deref().unwrap_or(source_url), true,
            Some("Provider artist credits are shown separately by role. Compare them with the selected recording.")));
    }
    if matched_entry.is_some() && match_score >= 0.72 {
        fields.push(field(
            "title",
            track_title,
            source,
            source_url_recording.as_deref().unwrap_or(source_url),
            true,
            Some(if match_score < 0.95 {
                "Fuzzy release-track title match; confirm the exact mix."
            } else {
                "Confirm that this release track is the intended recording."
            }),
        ));
    }
    let isrc = isrcs.first().cloned();
    if let Some(isrc) = isrc {
        fields.push(field(
            "isrc",
            isrc,
            source,
            source_url_recording.as_deref().unwrap_or(source_url),
            true,
            None,
        ));
    }
    if let Some(genre) = edition_genres.first() {
        fields.push(field(
            "genre",
            genre.clone(),
            source,
            source_url,
            true,
            Some("Provider genre/style tags can differ from the chosen catalog genre."),
        ));
    }
    if let Some(label) = label {
        fields.push(field("label", label, source, source_url, true, None));
    }
    if let Some(catalog) = catalog_number {
        fields.push(field(
            "catalogNumber",
            catalog,
            source,
            source_url,
            true,
            None,
        ));
    }
    if let Some(artwork) = edition_artwork {
        fields.push(field(
            "artworkUrl",
            artwork,
            source,
            source_url,
            true,
            Some("Artwork comes from the selected release edition."),
        ));
    }
    if let Some(edition_release_date) = edition_date.clone() {
        fields.push(field("releaseDate", edition_release_date, source, source_url, true,
            Some("This date belongs to the selected release edition; it may differ from the recording's first-release date.")));
    }
    let release_year = edition_year.or_else(|| edition_date.as_deref().and_then(parse_year));
    let _ = edition_title;
    Ok(MetadataReview {
        source_track_id: track.id.clone(),
        recording: None,
        recording_candidates: Vec::new(),
        release_ref: Some(release_ref.clone()),
        release_tracklist,
        matched_release_track: matched_entry
            .map(|entry| {
                entry["title"]
                    .as_str()
                    .or(entry["recording"]["title"].as_str())
                    .unwrap_or("")
                    .to_owned()
            })
            .filter(|title| !title.is_empty()),
        fields,
        credits: track_credits,
        recording_first_release_date,
        release_date: edition_date,
        release_year,
        selected_version_preserved: track.version.clone(),
        warnings,
    })
}

async fn resolve_from_recording(
    track: &Track,
    candidate: ArtistRecordingMatch,
    data: &Value,
) -> Result<MetadataReview, String> {
    let source_url = candidate.source_url.clone();
    let source_date = candidate.first_release_date.clone();
    let credits = parse_mb_credits(&data["artist-credit"], &source_url);
    let mut fields = Vec::new();
    let artist_name = render_credits(&credits);
    if !artist_name.is_empty() {
        fields.push(field(
            "artist",
            artist_name,
            "MusicBrainz recording",
            &source_url,
            true,
            Some("Confirm the complete artist credit."),
        ));
    }
    if !candidate.title.is_empty() {
        fields.push(field(
            "title",
            candidate.title.clone(),
            "MusicBrainz recording",
            &source_url,
            true,
            Some("Confirm the exact recording and mix."),
        ));
    }
    if let Some(isrc) = candidate.isrcs.first() {
        fields.push(field(
            "isrc",
            isrc.clone(),
            "MusicBrainz recording",
            &source_url,
            true,
            None,
        ));
    }
    let genres = names(&data["genres"]);
    if let Some(genre) = genres.first() {
        fields.push(field(
            "genre",
            genre.clone(),
            "MusicBrainz recording",
            &source_url,
            true,
            None,
        ));
    }
    let release_date = None;
    let release_year = None;
    let mut warnings = vec!["No release edition was selected. Edition-specific label, catalog number, artwork, release date, and physical format are not included.".into()];
    if candidate.score < 0.85 {
        warnings.push(
            "This recording identity is only a text match; verify it before applying any fields."
                .into(),
        );
    }
    Ok(MetadataReview {
        source_track_id: track.id.clone(),
        recording: Some(candidate),
        recording_candidates: Vec::new(),
        release_ref: None,
        release_tracklist: Vec::new(),
        matched_release_track: None,
        fields,
        credits,
        recording_first_release_date: source_date,
        release_date,
        release_year,
        selected_version_preserved: track.version.clone(),
        warnings,
    })
}

fn parse_recording(value: &Value, track: &Track) -> Option<ArtistRecordingMatch> {
    let id = value["id"].as_str()?.to_owned();
    let title = value["title"].as_str()?.to_owned();
    let credits = parse_mb_credits(
        &value["artist-credit"],
        &format!("https://musicbrainz.org/recording/{id}"),
    );
    let artist = render_credits(&credits);
    let title_similarity = jaro_winkler(&normalize(&track.title), &normalize(&title));
    let artist_similarity = jaro_winkler(&normalize(&track.artist), &normalize(&artist));
    let score = (title_similarity * 0.72 + artist_similarity * 0.28).clamp(0.0, 1.0);
    Some(ArtistRecordingMatch {
        recording: ArtistRef {
            provider: "musicbrainz".into(),
            id: id.clone(),
        },
        title,
        artist,
        score,
        first_release_date: value["first-release-date"].as_str().map(str::to_owned),
        isrcs: value["isrcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        source_url: format!("https://musicbrainz.org/recording/{id}"),
    })
}

fn parse_mb_credits(value: &Value, source_url: &str) -> Vec<ArtistCredit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let artist = item.get("artist")?;
            let id = artist["id"].as_str()?.to_owned();
            let name = item["name"]
                .as_str()
                .or(artist["name"].as_str())?
                .to_owned();
            Some(ArtistCredit {
                name,
                artist: Some(ArtistRef {
                    provider: "musicbrainz".into(),
                    id,
                }),
                role: "artist credit".into(),
                credited_as: item["name"].as_str().map(str::to_owned),
                join_phrase: item["joinphrase"].as_str().map(str::to_owned),
                source_url: source_url.into(),
            })
        })
        .collect()
}

fn discogs_credits(value: &Value, default_role: &str, source_url: &str) -> Vec<ArtistCredit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item["name"].as_str()?.to_owned();
            let role = item["role"].as_str().unwrap_or(default_role).to_owned();
            let id = item["id"].as_u64().map(|id| id.to_string());
            Some(ArtistCredit {
                name: name.clone(),
                artist: id.map(|id| ArtistRef {
                    provider: "discogs".into(),
                    id,
                }),
                role,
                credited_as: Some(name),
                join_phrase: None,
                source_url: source_url.into(),
            })
        })
        .collect()
}

fn mb_track_entries(media: &Value) -> Vec<Value> {
    media
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item["tracks"].as_array().cloned().unwrap_or_default())
        .collect()
}
fn discogs_track_entries(tracklist: &Value) -> Vec<Value> {
    tracklist
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type_"].as_str().unwrap_or("track") == "track")
        .cloned()
        .collect()
}
fn render_credits(credits: &[ArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| {
            format!(
                "{}{}",
                credit.name,
                credit.join_phrase.as_deref().unwrap_or("")
            )
        })
        .collect()
}
fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["name"].as_str().or(item.as_str()).map(str::to_owned))
        .collect()
}
fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn parse_year(value: &str) -> Option<u32> {
    value.get(0..4)?.parse().ok()
}
fn valid_mbid(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            })
}
fn validate_release_ref(value: &ArtistRef) -> Result<(), String> {
    if (value.provider == "musicbrainz" && valid_mbid(&value.id))
        || (value.provider == "discogs"
            && !value.id.is_empty()
            && value.id.chars().all(|character| character.is_ascii_digit()))
    {
        Ok(())
    } else {
        Err("Invalid selected release identity".into())
    }
}
fn secure_https(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
        .then(|| url.to_string())
}
fn field(
    key: &str,
    value: String,
    source: &str,
    source_url: &str,
    review_required: bool,
    note: Option<&str>,
) -> MetadataReviewField {
    MetadataReviewField {
        key: key.into(),
        value: Some(value),
        source: source.into(),
        source_url: source_url.into(),
        review_required,
        note: note.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source_track() -> Track {
        Track {
            id: "catalog:track-1".into(),
            artist: "Pulse feat. Nova".into(),
            title: "Night Drive".into(),
            version: Some("Extended Mix".into()),
            ..Track::default()
        }
    }

    #[test]
    fn candidate_search_scores_recordings_but_keeps_them_unconfirmed() {
        let source = source_track();
        let exact = parse_recording(&json!({
            "id":"00000000-0000-4000-8000-000000000001", "title":"Night Drive",
            "first-release-date":"2004", "isrcs":["USAAA0400001"],
            "artist-credit":[{"name":"Pulse","joinphrase":" feat. ","artist":{"id":"00000000-0000-4000-8000-000000000002","name":"Pulse"}},{"name":"Nova","joinphrase":"","artist":{"id":"00000000-0000-4000-8000-000000000003","name":"Nova"}}]
        }), &source).unwrap();
        assert!(exact.score > 0.95);
        assert_eq!(exact.isrcs, vec!["USAAA0400001"]);
        assert_eq!(exact.artist, "Pulse feat. Nova");
        assert_eq!(exact.recording.provider, "musicbrainz");
    }

    #[test]
    fn discogs_roles_remain_distinct_and_edition_year_is_separate() {
        let main = discogs_credits(
            &json!([{"id":7,"name":"Pulse"}]),
            "artist",
            "https://www.discogs.com/release/1",
        );
        let extras = discogs_credits(
            &json!([{"id":8,"name":"Nova","role":"Remixer"},{"id":9,"name":"A. Producer","role":"Producer"}]),
            "credit",
            "https://www.discogs.com/release/1",
        );
        assert_eq!(main[0].role, "artist");
        assert_eq!(extras[0].role, "Remixer");
        assert_eq!(extras[1].role, "Producer");
        assert_eq!(parse_year("2004-09-17"), Some(2004));
    }

    #[test]
    fn artist_match_does_not_strip_or_merge_roles() {
        let track = source_track();
        let candidate = parse_recording(&json!({
            "id":"00000000-0000-4000-8000-000000000004","title":"Night Drive",
            "artist-credit":[{"name":"Pulse","joinphrase":" feat. ","artist":{"id":"00000000-0000-4000-8000-000000000005","name":"Pulse"}},{"name":"Nova","artist":{"id":"00000000-0000-4000-8000-000000000006","name":"Nova"}}]
        }), &track).unwrap();
        assert_eq!(candidate.artist, "Pulse feat. Nova");
    }

    #[tokio::test]
    async fn selected_discogs_edition_keeps_edition_date_and_shows_tracklist() {
        let temp = tempfile::tempdir().unwrap();
        let store =
            std::sync::Arc::new(crate::Store::open(&temp.path().join("metadata.sqlite")).unwrap());
        let service = ArtistService::new(store).unwrap();
        let track = source_track();
        let release = ArtistReleaseRef {
            artwork_urls: vec![],
            release: ArtistRef {
                provider: "discogs".into(),
                id: "123".into(),
            },
            title: "Night Drive EP".into(),
            year: Some(2020),
            release_date: Some("2020".into()),
            country: None,
            label: None,
            catalog_number: None,
            format: None,
            primary_type: None,
            track_count: Some(1),
            tracks: Vec::new(),
            source_url: "https://www.discogs.com/release/123".into(),
            credits: Vec::new(),
        };
        let data = json!({
            "title":"Night Drive EP", "released":"2020-04-12", "year":2020,
            "artists":[{"id":1,"name":"Pulse","role":"Artist"}],
            "tracklist":[{"position":"A1","type_":"track","title":"Night Drive (Extended Mix)","artists":[{"id":1,"name":"Pulse","role":"Artist"}],"extraartists":[{"id":2,"name":"Nova","role":"Remixer"}]}]
        });
        let review = resolve_from_release(
            &service,
            &track,
            &release,
            &data,
            "Discogs",
            &release.source_url,
        )
        .await
        .unwrap();
        assert_eq!(review.release_year, Some(2020));
        assert_eq!(review.release_date.as_deref(), Some("2020-04-12"));
        assert_eq!(
            review.selected_version_preserved.as_deref(),
            Some("Extended Mix")
        );
        assert_eq!(review.release_tracklist.len(), 1);
        assert_eq!(
            review.matched_release_track.as_deref(),
            Some("Night Drive (Extended Mix)")
        );
        assert!(review.credits.iter().any(|credit| credit.role == "Remixer"));
        assert!(!review.fields.iter().any(|field| field.key == "version"));
    }
}
