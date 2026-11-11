//! Pure proof for enriching a Discogs catalog occurrence's missing duration
//! from a complete Apple album listing. The proof aligns the ordered album
//! programme and the exact target occurrence; it does not bind barcodes,
//! recording identifiers, playback URLs, or performer completeness.
use crate::models::Track;
use crate::recording_identity::{parse_recording_text, AnnotationKind, ParsedRecordingText};
use chrono::Datelike;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const MAX_ALBUM_TRACKS: usize = 100;
const MAX_ALBUM_NAME: usize = 512;
const MAX_TITLE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppleExplicitness {
    Explicit,
    NotExplicit,
    Cleaned,
}

fn apple_explicitness(value: &Value) -> Option<AppleExplicitness> {
    match value.as_str()? {
        "explicit" => Some(AppleExplicitness::Explicit),
        "notExplicit" => Some(AppleExplicitness::NotExplicit),
        "cleaned" => Some(AppleExplicitness::Cleaned),
        _ => None,
    }
}

fn apple_program_has_no_clean_edit(apple_album: &Value, apple_rows: &[&Value]) -> bool {
    apple_explicitness(&apple_album["collectionExplicitness"])
        .is_some_and(|value| value != AppleExplicitness::Cleaned)
        && apple_rows.iter().all(|row| {
            apple_explicitness(&row["trackExplicitness"])
                .is_some_and(|value| value != AppleExplicitness::Cleaned)
        })
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DiscogsAppleDurationProof {
    pub duration_seconds: f64,
    /// A stable, bounded description of the evidence. This is explanatory
    /// metadata only; callers must keep the original request fingerprint.
    pub evidence: Value,
}

fn bounded_text(value: &Value, max: usize) -> Option<&str> {
    value
        .as_str()
        .filter(|text| {
            !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
        })
        .map(str::trim)
}

fn positive_id(value: &Value) -> Option<u64> {
    value.as_u64().filter(|id| *id > 0)
}

fn release_id_text(track: &Track) -> Option<&str> {
    let id = track.provenance["releaseId"].as_str()?;
    (!id.is_empty()
        && id.len() <= 20
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id.parse::<u64>().is_ok_and(|n| n > 0))
    .then_some(id)
}

fn manual_identity_is_clear(track: &Track) -> bool {
    let Some(value) = track.provenance.get("manualFields") else {
        return true;
    };
    let Some(fields) = value.as_object() else {
        return false;
    };
    [
        "title",
        "artist",
        "artistCredits",
        "album",
        "albumArtist",
        "durationSeconds",
        "version",
        "trackNumber",
        "discNumber",
        "isrc",
        "catalogNumber",
    ]
    .iter()
    .all(|field| {
        fields.get(*field).is_none_or(Value::is_boolean)
            && fields.get(*field).and_then(Value::as_bool) != Some(true)
    })
}

fn consistent_discogs_resource_id(value: &Value, field: &str, id: &str, plural: bool) -> bool {
    let Some(raw) = value.get(field) else {
        return true;
    };
    let Some(raw) = bounded_text(raw, 1024) else {
        return false;
    };
    let prefix = if plural { "releases" } else { "release" };
    let expected = format!(
        "https://{}discogs.com/{prefix}/{id}",
        if plural { "api." } else { "www." }
    );
    if plural {
        raw == expected
    } else {
        raw == expected
            || raw.strip_prefix(&expected).is_some_and(|tail| {
                tail.starts_with('-')
                    && tail[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
    }
}

fn exact_safe_catalog_number(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))
        .then_some(value)
}

fn source_catalog_numbers(release: &Value) -> Option<(HashSet<String>, Option<String>)> {
    let mut numbers = HashSet::new();
    let mut barcodes = HashSet::new();
    let identifiers = match release.get("identifiers") {
        None | Some(Value::Null) => None,
        Some(Value::Array(rows)) => Some(rows),
        Some(_) => return None,
    };
    if let Some(rows) = identifiers {
        for row in rows {
            let kind = row.get("type")?.as_str()?;
            let value = row.get("value")?;
            if kind.eq_ignore_ascii_case("barcode") {
                let value = exact_safe_catalog_number(value.as_str()?)?;
                if !value.bytes().all(|byte| byte.is_ascii_digit())
                    || !(8..=14).contains(&value.len())
                {
                    return None;
                }
                numbers.insert(value.to_owned());
                barcodes.insert(value.to_owned());
            }
        }
    }
    let labels = match release.get("labels") {
        None | Some(Value::Null) => None,
        Some(Value::Array(rows)) => Some(rows),
        Some(_) => return None,
    };
    if let Some(rows) = labels {
        for row in rows {
            match row.get("catno") {
                None | Some(Value::Null) => {}
                Some(Value::String(value)) => {
                    if let Some(value) = exact_safe_catalog_number(value) {
                        numbers.insert(value.to_owned());
                    } else {
                        return None;
                    }
                }
                Some(_) => return None,
            }
        }
    }
    if barcodes.len() > 1 {
        return None;
    }
    Some((numbers, barcodes.into_iter().next()))
}

fn requested_catalog_number_matches(track: &Track, release: &Value) -> bool {
    let Some((source_numbers, _)) = source_catalog_numbers(release) else {
        return false;
    };
    match track.catalog_number.as_deref() {
        None => true,
        Some(requested) => exact_safe_catalog_number(requested)
            .is_some_and(|requested| source_numbers.contains(requested)),
    }
}

fn valid_apple_upc(album: &Value) -> Option<Option<String>> {
    match album.get("upc") {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => {
            let value = exact_safe_catalog_number(value)?;
            (value.bytes().all(|byte| byte.is_ascii_digit()) && (8..=14).contains(&value.len()))
                .then(|| Some(value.to_owned()))
        }
        Some(_) => None,
    }
}

fn valid_optional_identifier_array(value: &Value) -> bool {
    let Some(array) = value.get("identifiers") else {
        return true;
    };
    let Some(rows) = array.as_array() else {
        return false;
    };
    rows.iter().all(|row| {
        row.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| !kind.trim().is_empty())
            && row
                .get("value")
                .and_then(Value::as_str)
                .is_some_and(|value| exact_safe_catalog_number(value).is_some())
    })
}

fn source_occurrence(track: &Track) -> Option<(String, u64)> {
    if track.provider != "catalog"
        || track.provenance["catalogProvider"] != "discogs"
        || track.provenance["sourceKind"] != "catalog"
        || track.duration_seconds.is_some()
        || track.playback_range.is_some()
        || track.local_path.is_some()
        || track.asset_id.is_some()
        || track.provenance["chapters"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
        || !manual_identity_is_clear(track)
        || crate::recording_identity::RecordingIdentity::from_track(track).identifier_conflict
        || track
            .isrc
            .as_deref()
            .is_some_and(|code| crate::recording_identity::normalize_isrc(code).is_none())
        || track.title.trim().is_empty()
        || track.title.len() > MAX_TITLE
        || track.artist.trim().is_empty()
        || track
            .album
            .as_deref()
            .is_none_or(|album| album.trim().is_empty() || album.len() > MAX_ALBUM_NAME)
    {
        return None;
    }
    let release_id = release_id_text(track)?;
    let provider_hash = track.provider_id.strip_prefix("discogs:")?;
    if provider_hash.is_empty()
        || provider_hash.len() > 128
        || !provider_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || track.id != format!("catalog:discogs:{provider_hash}")
    {
        return None;
    }
    let position = track.provenance["trackPosition"].as_str()?;
    let position = position.parse::<u64>().ok().filter(|n| *n > 0)?;
    if track.provenance["mediumPosition"].as_u64() != Some(1)
        || track.disc_number.is_some_and(|disc| disc != 1)
        || track
            .track_number
            .is_some_and(|number| u64::from(number) != position)
        || track
            .provenance
            .get("trackId")
            .is_some_and(|id| !id.is_null())
    {
        return None;
    }
    let source_url = format!("https://www.discogs.com/release/{release_id}");
    if track.source_url != source_url
        || track.provenance["sourceUrl"].as_str() != Some(source_url.as_str())
        || track.provenance["requestedReleaseSourceUrl"]
            .as_str()
            .is_some_and(|url| url != source_url)
    {
        return None;
    }
    Some((release_id.to_owned(), position))
}

fn no_unknown_request_identity(track: &Track) -> bool {
    let parsed = parse_recording_text(&track.title);
    !parsed.parsing_incomplete
        && !parsed
            .annotations
            .iter()
            .any(|annotation| annotation.kind == AnnotationKind::Unresolved)
        && track.version.as_deref().is_none_or(|version| {
            let version = crate::matching::normalize_version(version);
            version.is_empty()
                || parsed
                    .semantic_versions()
                    .any(|actual| crate::matching::normalize_version(actual) == version)
        })
}

fn parse_exact_title(value: &str) -> Option<ParsedRecordingText> {
    if value.is_empty() || value.len() > MAX_TITLE {
        return None;
    }
    let parsed = parse_recording_text(value);
    (!parsed.parsing_incomplete).then_some(parsed)
}

fn non_feature_annotations(parsed: &ParsedRecordingText) -> Vec<(AnnotationKind, String)> {
    parsed
        .annotations
        .iter()
        .filter(|annotation| annotation.kind != AnnotationKind::FeaturedCredit)
        .map(|annotation| {
            (
                annotation.kind,
                crate::matching::normalize(&annotation.value),
            )
        })
        .collect()
}

fn titles_align_for_program(discogs: &str, apple: &str) -> bool {
    let (Some(discogs), Some(apple)) = (parse_exact_title(discogs), parse_exact_title(apple))
    else {
        return false;
    };
    // Keep semantic versions and every non-feature annotation. A shared
    // unresolved annotation can align only when it is textually identical on
    // both providers; no unknown wording is peeled away to manufacture a hit.
    crate::matching::normalize(&discogs.base_title) == crate::matching::normalize(&apple.base_title)
        && discogs
            .semantic_versions()
            .map(crate::matching::normalize_version)
            .collect::<Vec<_>>()
            == apple
                .semantic_versions()
                .map(crate::matching::normalize_version)
                .collect::<Vec<_>>()
        && non_feature_annotations(&discogs) == non_feature_annotations(&apple)
}

pub(crate) fn apple_search_collection_ids(track: &Track, search: &Value) -> Option<Vec<u64>> {
    let rows = apple_search_target_rows(track, search)?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(positive_id(&row["collectionId"])?);
        if ids.len() > 3 {
            return None;
        }
    }
    (!ids.is_empty()).then(|| ids.into_iter().collect())
}

fn apple_search_target_rows<'a>(track: &Track, search: &'a Value) -> Option<Vec<&'a Value>> {
    let rows = search["results"].as_array()?;
    // Apple search is capped at 25; a full page cannot prove the candidate set
    // complete, so refuse it instead of truncating away another edition.
    if rows.is_empty()
        || rows.len() >= 25
        || search["resultCount"].as_u64() != Some(rows.len() as u64)
    {
        return None;
    }
    let wanted_album = plain_album_title(track.album.as_deref()?)?;
    let targets = rows
        .iter()
        .filter(|row| {
            row["kind"] == "song"
                && bounded_text(&row["collectionName"], MAX_ALBUM_NAME)
                    .and_then(plain_album_title)
                    .as_deref()
                    == Some(wanted_album.as_str())
                && bounded_text(&row["artistName"], 2048).is_some_and(|artist| {
                    crate::source_fallback::normalize_artist_credit(artist)
                        == crate::source_fallback::normalize_artist_credit(&track.artist)
                })
                && bounded_text(&row["trackName"], MAX_TITLE)
                    .is_some_and(|title| titles_align_for_program(&track.title, title))
        })
        .collect::<Vec<_>>();
    if targets.is_empty()
        || targets
            .iter()
            .any(|row| positive_id(&row["collectionId"]).is_none())
    {
        return None;
    }
    Some(targets)
}

pub(crate) fn apple_search_artist_id(track: &Track, search: &Value) -> Option<u64> {
    let rows = apple_search_target_rows(track, search)?;
    let mut artist_ids = HashSet::new();
    for row in rows {
        artist_ids.insert(positive_id(&row["artistId"])?);
    }
    (artist_ids.len() == 1).then(|| *artist_ids.iter().next().unwrap())
}

/// Treat the Apple artist album response as an ID locator only. Its pagination
/// and completeness are never identity evidence; every returned ID still has
/// to pass a fresh complete-album crosswalk independently.
pub(crate) fn apple_artist_album_collection_ids(
    track: &Track,
    artist_id: u64,
    locator: &Value,
) -> Option<Vec<u64>> {
    let rows = locator["results"].as_array()?;
    if rows.is_empty() || rows.len() > 201 {
        return None;
    }
    let wanted_album = plain_album_title(track.album.as_deref()?)?;
    let wanted_artist = crate::source_fallback::normalize_artist_credit(&track.artist);
    let mut ids: HashMap<u64, &Value> = HashMap::new();
    for row in rows {
        if row["wrapperType"] != "collection"
            || row["collectionType"] != "Album"
            || row["artistId"].as_u64() != Some(artist_id)
            || bounded_text(&row["collectionName"], MAX_ALBUM_NAME)
                .and_then(plain_album_title)
                .as_deref()
                != Some(wanted_album.as_str())
            || bounded_text(&row["artistName"], 2048).is_none_or(|artist| {
                crate::source_fallback::normalize_artist_credit(artist) != wanted_artist
            })
        {
            continue;
        }
        let id = positive_id(&row["collectionId"])?;
        if let Some(previous) = ids.insert(id, row) {
            if previous != row {
                return None;
            }
        }
        if ids.len() > 3 {
            return None;
        }
    }
    (!ids.is_empty()).then(|| ids.into_keys().collect())
}

fn plain_album_title(value: &str) -> Option<String> {
    let parsed = parse_exact_title(value)?;
    if !parsed.annotations.is_empty() {
        return None;
    }
    let normalized = crate::matching::normalize(&parsed.base_title);
    (!normalized.is_empty()).then_some(normalized)
}

fn single_credit_key(value: &str) -> Option<String> {
    let normalized = crate::matching::normalize_single_artist_name(value);
    (!normalized.is_empty() && normalized.len() <= 512).then_some(normalized)
}

fn unique_credit_rows(rows: &[Value]) -> Option<Vec<(String, String)>> {
    if rows.is_empty() || rows.len() > 8 {
        return None;
    }
    let mut seen_names = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let name = bounded_text(&row["name"], 512)?;
        let id = row["id"].as_u64().filter(|id| *id > 0)?.to_string();
        match row.get("role") {
            None => {}
            Some(Value::String(role))
                if role.trim().is_empty()
                    || crate::recording_identity::is_primary_performer_role(role) => {}
            _ => return None,
        }
        if row.get("resource_url").is_some_and(|url| {
            url.as_str() != Some(format!("https://api.discogs.com/artists/{id}").as_str())
        }) {
            return None;
        }
        let name_key = single_credit_key(name)?;
        if !seen_names.insert(name_key.clone()) || !seen_ids.insert(id.clone()) {
            return None;
        }
        out.push((id, name_key));
    }
    Some(out)
}

fn request_credit_rows(track: &Track) -> Option<Vec<(String, String)>> {
    let rows = track.provenance["artistCredits"].as_array()?;
    if rows.is_empty() || rows.len() > 8 {
        return None;
    }
    let mut seen_names = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if !row["role"]
            .as_str()
            .is_some_and(crate::recording_identity::is_primary_performer_role)
            || row["artist"]["provider"] != "discogs"
        {
            return None;
        }
        let name = bounded_text(&row["name"], 512)?;
        let id = row["artist"]["id"].as_str()?.to_owned();
        if id.is_empty() || id.len() > 20 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let name_key = single_credit_key(name)?;
        if !seen_names.insert(name_key.clone()) || !seen_ids.insert(id.clone()) {
            return None;
        }
        out.push((id, name_key));
    }
    Some(out)
}

fn discogs_artist_rows(rows: &[Value]) -> Option<Vec<(String, String)>> {
    if rows.is_empty() || rows.len() > 8 {
        return None;
    }
    let mut seen_names = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let name = bounded_text(&row["name"], 512)?;
        let id = row["id"].as_u64()?.to_string();
        match row.get("role") {
            None => {}
            Some(Value::String(role))
                if role.trim().is_empty()
                    || crate::recording_identity::is_primary_performer_role(role) => {}
            _ => return None,
        }
        if row.get("resource_url").is_some_and(|url| {
            url.as_str() != Some(format!("https://api.discogs.com/artists/{id}").as_str())
        }) {
            return None;
        }
        let name_key = single_credit_key(name)?;
        if !seen_names.insert(name_key.clone()) || !seen_ids.insert(id.clone()) {
            return None;
        }
        out.push((id, name_key));
    }
    Some(out)
}

fn target_extraartists_are_understood(target: &Value) -> bool {
    let Some(raw) = target.get("extraartists") else {
        return true;
    };
    let Some(rows) = raw.as_array() else {
        return false;
    };
    if rows.len() > 64 {
        return false;
    }
    rows.iter().all(|row| {
        let Some(role) = row.get("role").and_then(Value::as_str) else {
            return false;
        };
        let role = crate::recording_identity::normalize_credit_role(role);
        if role.is_empty()
            || crate::recording_identity::is_primary_performer_role(&role)
            || crate::recording_identity::is_guest_performer_role(&role)
            || role == "remixer"
        {
            return false;
        }
        crate::recording_identity::is_non_performer_credit_role(&role)
    })
}

fn verify_main_artist_crosswalk(track: &Track, release: &Value, apple_album: &Value) -> bool {
    let Some(request) = request_credit_rows(track) else {
        return false;
    };
    let Some(release_artists) = release["artists"]
        .as_array()
        .and_then(|rows| unique_credit_rows(rows))
    else {
        return false;
    };
    let Some(release_sort) = bounded_text(&release["artists_sort"], 2048) else {
        return false;
    };
    let Some(apple_artist) = bounded_text(&apple_album["artistName"], 2048) else {
        return false;
    };
    request == release_artists
        && crate::source_fallback::normalize_artist_credit(&track.artist)
            == crate::source_fallback::normalize_artist_credit(release_sort)
        && crate::source_fallback::normalize_artist_credit(&track.artist)
            == crate::source_fallback::normalize_artist_credit(apple_artist)
}

fn rights_name_key(name: &str) -> Option<String> {
    // Discogs label display names and Apple's copyright line sometimes differ
    // only by a terminal legal label suffix (e.g. "Records" vs "Recording
    // Corporation"). Strip only these enumerated terminal suffixes, never
    // fuzzy-match or compare arbitrary substrings.
    let mut key = crate::matching::normalize(name);
    for suffix in [
        " recording corporation",
        " recording company",
        " records",
        " record company",
        " corporation",
        " incorporated",
        " inc",
        " limited",
        " ltd",
        " llc",
        " company",
    ] {
        if let Some(base) = key.strip_suffix(suffix) {
            key = base.trim().to_owned();
            break;
        }
    }
    (!key.is_empty()).then_some(key)
}

fn apple_copyright_entities(copyright: &str) -> Option<Vec<String>> {
    if copyright.len() > 1024 || !copyright.contains('℗') {
        return None;
    }
    let mut clauses = Vec::new();
    for part in copyright.split(" under exclusive license to ") {
        let part = part.trim().trim_start_matches('℗').trim();
        let part = part
            .split_once(' ')
            .filter(|(year, _)| year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or(part, |(_, remainder)| remainder.trim());
        if !part.is_empty() {
            clauses.push(
                part.trim_matches(|c: char| c == '.' || c == ';' || c == ',' || c.is_whitespace())
                    .to_owned(),
            );
        }
    }
    (!clauses.is_empty()).then_some(clauses)
}

fn rights_agree(release: &Value, apple_album: &Value) -> bool {
    let Some(copyright) = bounded_text(&apple_album["copyright"], 1024) else {
        return false;
    };
    let Some(apple_entities) = apple_copyright_entities(copyright) else {
        return false;
    };
    let apple_keys = apple_entities
        .iter()
        .filter_map(|name| rights_name_key(name))
        .collect::<HashSet<_>>();
    if apple_keys.is_empty() {
        return false;
    }
    // Prefer explicit Discogs rightsholder/company rows when present. The
    // exact cached release has none, so its declared release labels are the
    // only available independent legal-name anchors.
    let companies = release["companies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let kind = row["entity_type_name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            (kind.contains("copyright")
                || kind.contains("rightsholder")
                || kind.contains("rights holder"))
            .then(|| row["name"].as_str().map(str::to_owned))
            .flatten()
        })
        .collect::<Vec<_>>();
    let names = if companies.is_empty() {
        let Some(labels) = release["labels"].as_array() else {
            return false;
        };
        labels
            .iter()
            .filter_map(|row| row["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    } else {
        companies
    };
    names
        .iter()
        .filter_map(|name| rights_name_key(name))
        .any(|key| apple_keys.contains(&key))
}

fn same_calendar_day(discogs: &Value, apple: &Value) -> bool {
    let Some(released) = bounded_text(&discogs["released"], 32) else {
        return false;
    };
    let Some(release_date) = bounded_text(&apple["releaseDate"], 64) else {
        return false;
    };
    let apple_day = release_date.get(..10).unwrap_or_default();
    released.len() == 10
        && released.as_bytes().get(4) == Some(&b'-')
        && released.as_bytes().get(7) == Some(&b'-')
        && apple_day == released
        && discogs["year"]
            .as_u64()
            .is_none_or(|year| year.to_string() == released[..4])
}

fn same_year_calendar_day_mismatch(discogs: &Value, apple: &Value) -> bool {
    let Some(released) = bounded_text(&discogs["released"], 32) else {
        return false;
    };
    let Some(apple_date) = bounded_text(&apple["releaseDate"], 64) else {
        return false;
    };
    let Some(discogs_day) = chrono::NaiveDate::parse_from_str(released, "%Y-%m-%d").ok() else {
        return false;
    };
    let Some(apple_day_text) = apple_date.get(..10) else {
        return false;
    };
    let Some(apple_day) = chrono::NaiveDate::parse_from_str(apple_day_text, "%Y-%m-%d").ok() else {
        return false;
    };
    if apple_date.len() != 10 && chrono::DateTime::parse_from_rfc3339(apple_date).is_err() {
        return false;
    }
    let Some(year) = discogs["year"].as_u64() else {
        return false;
    };
    discogs_day != apple_day
        && discogs_day.year() == apple_day.year()
        && year == discogs_day.year() as u64
}

fn release_matches_request(track: &Track, release: &Value, release_id: &str) -> bool {
    let source_url = format!("https://www.discogs.com/release/{release_id}");
    let Some(release_title) = bounded_text(&release["title"], MAX_ALBUM_NAME) else {
        return false;
    };
    let Some(request_album) = track.album.as_deref() else {
        return false;
    };
    let Some(request_title) = plain_album_title(request_album) else {
        return false;
    };
    let Some(release_title) = plain_album_title(release_title) else {
        return false;
    };
    release["id"].as_u64().map(|id| id.to_string()).as_deref() == Some(release_id)
        && consistent_discogs_resource_id(release, "resource_url", release_id, true)
        && consistent_discogs_resource_id(release, "uri", release_id, false)
        && track.source_url == source_url
        && track.provenance["sourceUrl"].as_str() == Some(source_url.as_str())
        && request_title == release_title
        && requested_catalog_number_matches(track, release)
}

fn target_primary_credit_rows(track: &Track, target: &Value) -> bool {
    let Some(request) = request_credit_rows(track) else {
        return false;
    };
    let Some(target_artists) = target["artists"]
        .as_array()
        .and_then(|rows| discogs_artist_rows(rows))
    else {
        return false;
    };
    // The target must have exactly the requested, structured primary artist
    // group. Any target guest/extra credit makes this proof inapplicable; the
    // Apple endpoint exposes only its display credit, not a complete guest
    // role set. We do not infer completeness from artist counts.
    request == target_artists
        && target_extraartists_are_understood(target)
        && parse_exact_title(&track.title).is_some_and(|parsed| {
            !parsed.annotations.iter().any(|annotation| {
                annotation.kind == AnnotationKind::FeaturedCredit
                    || annotation.kind == AnnotationKind::Unresolved
            })
        })
}

fn complete_discogs_program(release: &Value) -> Option<Vec<&Value>> {
    let rows = release["tracklist"].as_array()?;
    if rows.is_empty() || rows.len() > MAX_ALBUM_TRACKS {
        return None;
    }
    let mut ordered: Vec<Option<&Value>> = vec![None; rows.len()];
    for row in rows {
        if row["type_"].as_str() != Some("track")
            || row["sub_tracks"]
                .as_array()
                .is_some_and(|children| !children.is_empty())
            || bounded_text(&row["title"], MAX_TITLE).is_none()
        {
            return None;
        }
        let position = row["position"].as_str()?.parse::<usize>().ok()?;
        if position == 0 || position > rows.len() || ordered[position - 1].replace(row).is_some() {
            return None;
        }
    }
    ordered.into_iter().collect()
}

fn complete_apple_program<'a>(
    apple: &'a Value,
    expected_count: usize,
) -> Option<(&'a Value, Vec<&'a Value>)> {
    let rows = apple["results"].as_array()?;
    if rows.len() < 2
        || rows.len() > MAX_ALBUM_TRACKS + 1
        || apple["resultCount"].as_u64() != Some(rows.len() as u64)
    {
        return None;
    }
    let albums = rows
        .iter()
        .filter(|row| row["wrapperType"] == "collection")
        .collect::<Vec<_>>();
    if albums.len() != 1
        || albums[0]["collectionType"] != "Album"
        || albums[0]["trackCount"].as_u64() != Some(expected_count as u64)
    {
        return None;
    }
    let album_id = positive_id(&albums[0]["collectionId"])?;
    let songs = rows
        .iter()
        .filter(|row| row["kind"] == "song")
        .collect::<Vec<_>>();
    if songs.len() != expected_count || songs.len() + 1 != rows.len() {
        return None;
    }
    if !valid_optional_identifier_array(albums[0])
        || valid_apple_upc(albums[0]).is_none()
        || rows.iter().any(|row| !valid_optional_identifier_array(row))
    {
        return None;
    }
    let album_upc = valid_apple_upc(albums[0])?;
    let mut ordered: Vec<Option<&Value>> = vec![None; expected_count];
    let mut ids = HashSet::new();
    for song in songs {
        if valid_apple_upc(song).is_none()
            || valid_apple_upc(song)?.is_some_and(|upc| album_upc.as_deref() != Some(&upc))
        {
            return None;
        }
        if song["collectionId"].as_u64() != Some(album_id)
            || song["discNumber"].as_u64() != Some(1)
            || song["discCount"].as_u64() != Some(1)
            || positive_id(&song["trackId"]).is_none()
            || !ids.insert(song["trackId"].as_u64()?)
        {
            return None;
        }
        let position = usize::try_from(song["trackNumber"].as_u64()?).ok()?;
        if position == 0
            || position > expected_count
            || ordered[position - 1].replace(song).is_some()
        {
            return None;
        }
    }
    Some((albums[0], ordered.into_iter().collect::<Option<Vec<_>>>()?))
}

/// Verify a complete ordered Discogs release against a fresh Apple album
/// lookup, returning only the target Apple runtime. This function performs no
/// network, cache, persistence, source selection, or track mutation.
fn verify_with_release_date_policy(
    track: &Track,
    raw_discogs_release: &Value,
    apple_lookup: &Value,
    allow_same_year_date_mismatch: bool,
) -> Option<DiscogsAppleDurationProof> {
    let (release_id, target_position) = source_occurrence(track)?;
    if !no_unknown_request_identity(track)
        || !release_matches_request(track, raw_discogs_release, &release_id)
    {
        return None;
    }
    let discogs_rows = complete_discogs_program(raw_discogs_release)?;
    let (apple_album, apple_rows) = complete_apple_program(apple_lookup, discogs_rows.len())?;
    if !apple_program_has_no_clean_edit(apple_album, &apple_rows) {
        return None;
    }
    let release_date_matches = same_calendar_day(raw_discogs_release, apple_album)
        || (allow_same_year_date_mismatch
            && same_year_calendar_day_mismatch(raw_discogs_release, apple_album));
    if !verify_main_artist_crosswalk(track, raw_discogs_release, apple_album)
        || !rights_agree(raw_discogs_release, apple_album)
        || !release_date_matches
        || plain_album_title(bounded_text(
            &apple_album["collectionName"],
            MAX_ALBUM_NAME,
        )?)? != plain_album_title(track.album.as_deref()?)?
    {
        return None;
    }
    let mut aligned = Vec::with_capacity(discogs_rows.len());
    let mut target_discogs = None;
    let mut target_apple = None;
    for (index, (discogs_row, apple_row)) in discogs_rows.iter().zip(&apple_rows).enumerate() {
        let position = (index + 1) as u64;
        let discogs_title = bounded_text(&discogs_row["title"], MAX_TITLE)?;
        let apple_title = bounded_text(&apple_row["trackName"], MAX_TITLE)?;
        if !titles_align_for_program(discogs_title, apple_title)
            || apple_row["collectionName"].as_str() != apple_album["collectionName"].as_str()
        {
            return None;
        }
        if position == target_position {
            target_discogs = Some(*discogs_row);
            target_apple = Some(*apple_row);
        }
        aligned.push(
            json!({"position":position,"discogsTitle":discogs_title,"appleTitle":apple_title}),
        );
    }
    let target_discogs = target_discogs?;
    let target_apple = target_apple?;
    if bounded_text(&target_discogs["title"], MAX_TITLE).is_none_or(|title| {
        crate::matching::normalize(title) != crate::matching::normalize(&track.title)
    }) || !target_primary_credit_rows(track, target_discogs)
        || bounded_text(&target_apple["artistName"], 2048).is_none_or(|artist| {
            crate::source_fallback::normalize_artist_credit(artist)
                != crate::source_fallback::normalize_artist_credit(&track.artist)
        })
    {
        return None;
    }
    let target_title = parse_exact_title(bounded_text(&target_apple["trackName"], MAX_TITLE)?)?;
    if target_title.annotations.iter().any(|annotation| {
        annotation.kind == AnnotationKind::FeaturedCredit
            || annotation.kind == AnnotationKind::Unresolved
    }) {
        return None;
    }
    let known_isrc = track
        .isrc
        .as_deref()
        .and_then(crate::recording_identity::normalize_isrc);
    let candidate_isrc = match target_apple.get("isrc") {
        None | Some(Value::Null) => None,
        Some(Value::String(code)) => Some(crate::recording_identity::normalize_isrc(code)?),
        Some(_) => return None,
    };
    if known_isrc.is_some() && candidate_isrc.is_none() {
        return None;
    }
    if let Some(candidate_isrc) = candidate_isrc {
        if known_isrc
            .as_deref()
            .is_some_and(|known| known != candidate_isrc)
        {
            return None;
        }
    }
    let duration_ms = target_apple["trackTimeMillis"].as_u64()?;
    if !(1_000..=86_400_000).contains(&duration_ms) {
        return None;
    }
    let duration_seconds = duration_ms as f64 / 1_000.0;
    let apple_track_id = positive_id(&target_apple["trackId"])?;
    let album_id = positive_id(&apple_album["collectionId"])?;
    let apple_upc = valid_apple_upc(apple_album)?.as_deref().map(str::to_owned);
    let (_, source_barcode) = source_catalog_numbers(raw_discogs_release)?;
    if apple_upc
        .as_deref()
        .zip(source_barcode.as_deref())
        .is_some_and(|(apple, discogs)| apple != discogs)
    {
        return None;
    }
    let barcode_bound = apple_upc
        .as_deref()
        .zip(source_barcode.as_deref())
        .is_some_and(|(apple, discogs)| apple == discogs);
    Some(DiscogsAppleDurationProof {
        duration_seconds,
        evidence: json!({
            "version": 1,
            "method": "discogs-apple-complete-album-program-v1",
            "discogsReleaseId": release_id,
            "discogsTrackPosition": target_position,
            "appleCollectionId": album_id,
            "appleTrackId": apple_track_id,
            "albumTrackCount": aligned.len(),
            "durationMillis": duration_ms,
            "appleUpcPresent": apple_upc.is_some(),
            "discogsBarcodePresent": source_barcode.is_some(),
            "barcodeBound": barcode_bound,
            "releaseDateClaim": if same_calendar_day(raw_discogs_release, apple_album) { "dateMatched" } else { "sameYearOtherReleaseDate" },
            "performerCompletenessProven": false,
            "nonTargetFeaturedCreditsCompared": false,
            "orderedProgram": aligned,
        }),
    })
}

/// Strict proof: the provider release dates must identify the same calendar day.
/// Kept as the default verifier for all existing callers and policy levels.
pub(crate) fn verify(
    track: &Track,
    raw_discogs_release: &Value,
    apple_lookup: &Value,
) -> Option<DiscogsAppleDurationProof> {
    verify_with_release_date_policy(track, raw_discogs_release, apple_lookup, false)
}

/// Weaker edition proof for a separately policy-gated Flexible caller. Every
/// identity/programme/rights/identifier guard remains shared with `verify`;
/// only a valid release day in the same year may differ. The result explicitly
/// disclaims an exact-edition claim.
pub(crate) fn verify_same_year_other_release(
    track: &Track,
    raw_discogs_release: &Value,
    apple_lookup: &Value,
    matching_level: crate::recording_match::MatchLevel,
    flexible_options: crate::recording_match::FlexibleMatchingOptions,
) -> Option<DiscogsAppleDurationProof> {
    if matching_level != crate::recording_match::MatchLevel::Flexible
        || !flexible_options.allow_other_releases
    {
        return None;
    }
    verify_with_release_date_policy(track, raw_discogs_release, apple_lookup, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn actual() -> (Track, Value, Value) {
        let track: Track = serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-discogs-request-track.json"
        ))
        .unwrap();
        let release: Value =
            serde_json::from_str(include_str!("../tests/fixtures/bacc-discogs-release.json"))
                .unwrap();
        let apple: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-album-lookup.json"
        ))
        .unwrap();
        (track, release, apple)
    }

    fn current_apple() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-current-album-lookup.json"
        ))
        .unwrap()
    }

    fn apple_artist_albums() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/bacc-apple-artist-albums.json"
        ))
        .unwrap()
    }

    fn current_song_search() -> Value {
        let apple = current_apple();
        let target = apple["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["trackName"] == "Bacc of the Bach")
            .unwrap();
        json!({
            "resultCount": 1,
            "results": [{
                "kind": "song",
                "artistId": target["artistId"],
                "artistName": target["artistName"],
                "collectionId": target["collectionId"],
                "collectionName": target["collectionName"],
                "trackName": target["trackName"],
            }]
        })
    }

    fn mark_program_explicit(apple: &mut Value) {
        apple["results"][0]["collectionExplicitness"] = json!("explicit");
        for row in apple["results"].as_array_mut().unwrap().iter_mut().skip(1) {
            row["trackExplicitness"] = json!("explicit");
        }
    }

    #[test]
    fn actual_bacc_crosswalk_proves_target_runtime_without_barcode_or_credit_completeness() {
        let (track, release, apple) = actual();
        let proof = verify(&track, &release, &apple).expect("captured complete album crosswalk");
        assert_eq!(proof.duration_seconds, 197.9);
        assert_eq!(proof.evidence["discogsReleaseId"], "38549214");
        assert_eq!(proof.evidence["discogsTrackPosition"], 15);
        assert_eq!(proof.evidence["appleTrackId"], 6777173927_u64);
        assert_eq!(proof.evidence["albumTrackCount"], 15);
        assert_eq!(proof.evidence["barcodeBound"], false);
        assert_eq!(proof.evidence["performerCompletenessProven"], false);
        assert_eq!(proof.evidence["nonTargetFeaturedCreditsCompared"], false);
        // The sources differ on a non-target guest spelling. This proof does
        // not alias or normalize that person; it aligns the complete title
        // programme and corroborates only the exact Bacc primary credit set.
        assert_eq!(
            proof.evidence["orderedProgram"][1]["discogsTitle"],
            "Sacrifices"
        );
        assert_eq!(
            proof.evidence["orderedProgram"][1]["appleTitle"],
            "Sacrifices (feat. James Fauntleroy)"
        );
    }

    #[test]
    fn cleaned_apple_release_is_never_authorized_by_other_release_policy() {
        let (track, release, _) = actual();
        let current = current_apple();
        assert!(verify(&track, &release, &current).is_none());
        let flexible = crate::recording_match::MatchLevel::Flexible;
        let options = crate::recording_match::FlexibleMatchingOptions::default();
        assert!(
            verify_same_year_other_release(&track, &release, &current, flexible, options).is_none()
        );

        // A release-date mismatch remains admissible under the explicit
        // Flexible release policy when the Apple edition is unedited.
        let mut unedited_other_release = current.clone();
        mark_program_explicit(&mut unedited_other_release);
        assert!(verify(&track, &release, &unedited_other_release).is_none());
        let proof = verify_same_year_other_release(
            &track,
            &release,
            &unedited_other_release,
            flexible,
            options,
        )
        .expect("same-year release mismatch without a cleaned-version marker");
        assert_eq!(proof.duration_seconds, 197.9);
        assert_eq!(
            proof.evidence["releaseDateClaim"],
            "sameYearOtherReleaseDate"
        );
        for level in [
            crate::recording_match::MatchLevel::Exact,
            crate::recording_match::MatchLevel::Balanced,
        ] {
            assert!(
                verify_same_year_other_release(&track, &release, &current, level, options)
                    .is_none()
            );
        }
        let release_disabled = crate::recording_match::FlexibleMatchingOptions {
            allow_other_releases: false,
            ..options
        };
        assert!(verify_same_year_other_release(
            &track,
            &release,
            &unedited_other_release,
            flexible,
            release_disabled
        )
        .is_none());

        let mut target_cleaned = unedited_other_release.clone();
        target_cleaned["results"][15]["trackExplicitness"] = json!("cleaned");
        assert!(verify(&track, &release, &target_cleaned).is_none());
        assert!(verify_same_year_other_release(
            &track,
            &release,
            &target_cleaned,
            flexible,
            options
        )
        .is_none());

        let mut album_cleaned = unedited_other_release.clone();
        album_cleaned["results"][0]["collectionExplicitness"] = json!("cleaned");
        assert!(verify(&track, &release, &album_cleaned).is_none());
        assert!(verify_same_year_other_release(
            &track,
            &release,
            &album_cleaned,
            flexible,
            options
        )
        .is_none());

        let mut unknown_status = unedited_other_release.clone();
        unknown_status["results"][15]["trackExplicitness"] = json!("maybeClean");
        assert!(verify(&track, &release, &unknown_status).is_none());

        let mut unknown_collection_status = unedited_other_release.clone();
        unknown_collection_status["results"][0]["collectionExplicitness"] = json!("maybeClean");
        assert!(verify(&track, &release, &unknown_collection_status).is_none());

        let mut missing_target_status = unedited_other_release;
        missing_target_status["results"][15]
            .as_object_mut()
            .unwrap()
            .remove("trackExplicitness");
        assert!(verify(&track, &release, &missing_target_status).is_none());

        let (mut wrong_artist, release, _) = actual();
        wrong_artist.artist = "Wrong Artist".into();
        assert!(verify_same_year_other_release(
            &wrong_artist,
            &release,
            &current,
            flexible,
            options
        )
        .is_none());

        let (track, mut wrong_version, _) = actual();
        wrong_version["tracklist"][14]["title"] = json!("Bacc Of The Bach (Live)");
        assert!(verify_same_year_other_release(
            &track,
            &wrong_version,
            &current,
            flexible,
            options
        )
        .is_none());

        let (mut conflicting_id, release, _) = actual();
        conflicting_id.provenance["recognitionIdentifiers"]["isrc"] = json!("USABC26000001");
        conflicting_id.isrc = Some("USXYZ26000001".into());
        assert!(verify_same_year_other_release(
            &conflicting_id,
            &release,
            &current,
            flexible,
            options
        )
        .is_none());
    }

    #[test]
    fn validated_song_artist_id_locates_both_editions_without_claiming_completeness() {
        let (track, _, _) = actual();
        let search = current_song_search();
        assert_eq!(
            apple_search_collection_ids(&track, &search),
            Some(vec![6801271209])
        );
        let artist_id = apple_search_artist_id(&track, &search).unwrap();
        assert_eq!(artist_id, 128860700);
        let locator = apple_artist_albums();
        let mut ids = apple_artist_album_collection_ids(&track, artist_id, &locator).unwrap();
        ids.sort_unstable();
        assert_eq!(ids, vec![6777173608, 6801271209]);

        let mut wrong_artist = locator.clone();
        for row in wrong_artist["results"].as_array_mut().unwrap() {
            if row["collectionId"].as_u64() == Some(6777173608) {
                row["artistId"] = json!(7);
            }
        }
        assert_eq!(
            apple_artist_album_collection_ids(&track, artist_id, &wrong_artist),
            Some(vec![6801271209])
        );

        let mut too_many = locator;
        for id in [7770000001_u64, 7770000002, 7770000003] {
            let mut extra = too_many["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["collectionName"] == "PROLIFIC")
                .unwrap()
                .clone();
            extra["collectionId"] = json!(id);
            too_many["results"].as_array_mut().unwrap().push(extra);
        }
        assert!(apple_artist_album_collection_ids(&track, artist_id, &too_many).is_none());
    }

    #[test]
    fn rejects_wrong_position_or_request_title() {
        let (mut track, release, apple) = actual();
        track.provenance["trackPosition"] = json!("14");
        assert!(verify(&track, &release, &apple).is_none());
        let (mut track, release, apple) = actual();
        track.title = "I Just Wanna Know".into();
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_wrong_album_artist_rights_or_release_day() {
        let (track, release, mut apple) = actual();
        apple["results"][0]["artistName"] = json!("Wrong Artist");
        assert!(verify(&track, &release, &apple).is_none());
        let (track, mut release, apple) = actual();
        release["labels"][0]["name"] = json!("Unrelated Records");
        release["labels"][1]["name"] = json!("Different Label");
        assert!(verify(&track, &release, &apple).is_none());
        let (track, mut release, apple) = actual();
        release["released"] = json!("2026-08-15");
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_wrong_album_title_or_reordered_program() {
        let (track, release, mut apple) = actual();
        apple["results"][0]["collectionName"] = json!("Other Album");
        assert!(verify(&track, &release, &apple).is_none());
        let (track, release, mut apple) = actual();
        apple["results"][1]["trackNumber"] = json!(2);
        apple["results"][2]["trackNumber"] = json!(1);
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_partial_or_ambiguous_program() {
        let (track, release, mut apple) = actual();
        apple["results"].as_array_mut().unwrap().pop();
        apple["resultCount"] = json!(15);
        assert!(verify(&track, &release, &apple).is_none());
        let (track, mut release, apple) = actual();
        release["tracklist"][14]["position"] = json!("14");
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_target_artist_guest_version_and_identifier_conflicts() {
        let (track, mut release, apple) = actual();
        release["tracklist"][14]["artists"][1]["name"] = json!("Wrong Performer");
        assert!(verify(&track, &release, &apple).is_none());
        let (track, release, mut apple) = actual();
        apple["results"][15]["trackName"] = json!("Bacc of the Bach (feat. Other Artist)");
        assert!(verify(&track, &release, &apple).is_none());
        let (track, mut release, apple) = actual();
        release["tracklist"][14]["title"] = json!("Bacc Of The Bach (Live)");
        assert!(verify(&track, &release, &apple).is_none());
        let (mut track, release, apple) = actual();
        track.provenance["recognitionIdentifiers"]["isrc"] = json!("USABC26000001");
        track.isrc = Some("USXYZ26000001".into());
        assert!(verify(&track, &release, &apple).is_none());
        let (track, release, mut apple) = actual();
        apple["results"][15]["isrc"] = json!("not-an-isrc");
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_wrong_source_or_manual_identity_and_duration_edits() {
        let (mut track, release, apple) = actual();
        track.provenance["releaseId"] = json!("12345");
        assert!(verify(&track, &release, &apple).is_none());
        let (mut track, release, apple) = actual();
        track.provenance["manualFields"]["durationSeconds"] = json!(true);
        assert!(verify(&track, &release, &apple).is_none());
        let (mut track, release, apple) = actual();
        track.provenance["manualFields"]["title"] = json!(true);
        assert!(verify(&track, &release, &apple).is_none());
        let (track, release, mut apple) = actual();
        apple["results"][0]["upc"] = json!("000000000000");
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn accepts_shared_primary_role_alias_but_rejects_malformed_role_evidence() {
        let (mut track, release, apple) = actual();
        track.provenance["artistCredits"][0]["role"] = json!("MAIN_ARTIST");
        assert!(verify(&track, &release, &apple).is_some());

        let (mut track, release, apple) = actual();
        track.provenance["artistCredits"][0]["role"] = json!(17);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["tracklist"][14]["artists"][0]["role"] = json!(false);
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_target_guest_remixer_unknown_or_malformed_extra_credit() {
        let (track, mut release, apple) = actual();
        release["tracklist"][14]["extraartists"] =
            json!([{"name":"Hidden Guest","role":"Featured Artist"}]);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["tracklist"][14]["extraartists"] =
            json!([{"name":"Hidden Remixer","role":"Remixer"}]);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["tracklist"][14]["extraartists"] =
            json!([{"name":"Unknown","role":"Uncredited musician"}]);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["tracklist"][14]["extraartists"] = json!([{"name":"Missing role"}]);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["tracklist"][14]["extraartists"] = json!([{"name":"Writer","role":"Lyricist"}]);
        assert!(verify(&track, &release, &apple).is_some());
    }

    #[test]
    fn rejects_malformed_manual_fields_and_track_namespace_mismatch() {
        let (mut track, release, apple) = actual();
        track.provenance["manualFields"] = json!("unknown");
        assert!(verify(&track, &release, &apple).is_none());

        let (mut track, release, apple) = actual();
        track.provenance["manualFields"]["durationSeconds"] = json!("false");
        assert!(verify(&track, &release, &apple).is_none());

        let (mut track, release, apple) = actual();
        track.provider_id = "spotify:fake".into();
        assert!(verify(&track, &release, &apple).is_none());

        let (mut track, release, apple) = actual();
        track.id = "catalog:discogs:other".into();
        assert!(verify(&track, &release, &apple).is_none());
    }

    #[test]
    fn rejects_catalog_number_resource_and_identifier_conflicts() {
        let (mut track, release, apple) = actual();
        track.catalog_number = Some("different".into());
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["uri"] = json!("https://www.discogs.com/release/12345-Prolific");
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["resource_url"] = json!("https://api.discogs.com/releases/12345");
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["identifiers"][0]["value"] = json!(false);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, mut release, apple) = actual();
        release["identifiers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"Barcode","value":"111111111111"}));
        assert!(verify(&track, &release, &apple).is_none());

        let (track, release, mut apple) = actual();
        apple["results"][0]["upc"] = json!(false);
        assert!(verify(&track, &release, &apple).is_none());

        let (track, release, mut apple) = actual();
        apple["results"][0]["identifiers"] = json!("malformed");
        assert!(verify(&track, &release, &apple).is_none());
    }
}
