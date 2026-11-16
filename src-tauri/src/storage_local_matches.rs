//! Read-only reuse of registered files across catalog listings. The recording
//! graph is never merged: each request is checked against the current policy.
use super::*;
use crate::recording_match::{MatchAction, MatchRelationship, MatchStage};
const CANDIDATE_LIMIT: usize = 64;
const LISTING_LIMIT: usize = 256;

const CANDIDATE_SQL: &str = r#"
SELECT a.id,a.path,a.hash,a.payload FROM local_assets a
WHERE a.id IN (
    SELECT id FROM local_assets
    WHERE artist_match_key(json_extract(payload,'$.track.artist'))=?1
      AND CAST(json_extract(payload,'$.durationSeconds') AS REAL) BETWEEN ?2 AND ?3
    UNION
    SELECT a.id FROM recordings r JOIN local_assets a ON a.recording_id=r.id
    WHERE artist_match_key(json_extract(r.manual_metadata,'$.artist'))=?1
      AND CAST(json_extract(a.payload,'$.durationSeconds') AS REAL) BETWEEN ?2 AND ?3
)
AND length(a.payload)<=262144
AND local_recording_title_key(COALESCE(
    (SELECT json_extract(r.manual_metadata,'$.title') FROM recordings r WHERE r.id=a.recording_id),
    json_extract(a.payload,'$.track.title')))=?5
ORDER BY a.rowid LIMIT ?4
"#;

const LISTING_SQL: &str = r#"
SELECT l.id,l.metadata,r.favorite,r.disposition,r.manual_metadata
FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id
WHERE l.id IN (
    SELECT id FROM catalog_listings
    WHERE artist_match_key(json_extract(metadata,'$.artist'))=?1
      AND CAST(json_extract(metadata,'$.durationSeconds') AS REAL) BETWEEN ?2 AND ?3
    UNION
    SELECT l.id FROM recordings r JOIN catalog_listings l ON l.recording_id=r.id
    WHERE artist_match_key(json_extract(r.manual_metadata,'$.artist'))=?1
      AND CAST(COALESCE(json_extract(r.manual_metadata,'$.durationSeconds'),
          json_extract(l.metadata,'$.durationSeconds')) AS REAL) BETWEEN ?2 AND ?3
    UNION
    SELECT l.id FROM catalog_listings l JOIN recordings r ON r.id=l.recording_id
    WHERE artist_match_key(json_extract(l.metadata,'$.artist'))=?1
      AND CAST(json_extract(r.manual_metadata,'$.durationSeconds') AS REAL) BETWEEN ?2 AND ?3
)
AND length(l.metadata)<=262144
AND local_recording_title_key(COALESCE(json_extract(r.manual_metadata,'$.title'),
    json_extract(l.metadata,'$.title')))=?5
LIMIT ?4
"#;

pub(super) fn title_key(value: &str) -> Option<String> {
    let parsed = crate::recording_identity::parse_recording_text(value);
    (!parsed.parsing_incomplete)
        .then_some(parsed.normalized_title_key)
        .flatten()
}

pub(super) fn install(conn: &Connection) -> Result<()> {
    // Query-only function: persisted indexes intentionally do not depend on it.
    conn.create_scalar_function(
        "local_recording_title_key",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            Ok(context
                .get::<Option<String>>(0)?
                .as_deref()
                .and_then(title_key))
        },
    )
    .map_err(err)?;
    // Use a function already present in older releases: these optional indexes
    // must not make the profile unwritable after an executable rollback.
    conn.execute_batch("CREATE INDEX IF NOT EXISTS local_asset_artist_runtime_v1 ON local_assets(artist_match_key(json_extract(payload,'$.track.artist')),CAST(json_extract(payload,'$.durationSeconds') AS REAL));
        CREATE INDEX IF NOT EXISTS local_listing_artist_runtime_v1 ON catalog_listings(artist_match_key(json_extract(metadata,'$.artist')),CAST(json_extract(metadata,'$.durationSeconds') AS REAL));
        CREATE INDEX IF NOT EXISTS local_manual_artist_v1 ON recordings(artist_match_key(json_extract(manual_metadata,'$.artist')));")
        .map_err(err)
}

fn runtime_window(duration: f64) -> (f64, f64) {
    let margin = (duration * 0.1).max(10.);
    (duration - margin, duration + margin)
}

fn candidate(asset: &LocalAsset) -> Option<Candidate> {
    // Prefer the acquired source's retained observations over tags copied from
    // a requested catalog entry. They remain tied to this registered file.
    if let Some(raw) = asset
        .track
        .provenance
        .get("audioCandidate")
        .filter(|v| !v.is_null())
    {
        let mut candidate: Candidate = serde_json::from_value(raw.clone()).ok()?;
        candidate.duration_seconds = asset.duration_seconds;
        candidate.permission = "owned".into();
        if let Some(metadata) = candidate
            .source_metadata
            .as_mut()
            .and_then(|v| v.as_object_mut())
        {
            metadata.insert("duration".into(), serde_json::json!(asset.duration_seconds));
        }
        return Some(candidate);
    }
    let t = &asset.track;
    let title = t
        .version
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("{} ({v})", t.title))
        .unwrap_or_else(|| t.title.clone());
    let duration = asset.duration_seconds;
    Some(Candidate {
        id: format!("local:{}", asset.id),
        provider: "local".into(),
        url: format!("local:{}", asset.id),
        title: title.clone(),
        uploader: Some(t.artist.clone()),
        duration_seconds: duration,
        permission: "owned".into(),
        source_metadata: Some(serde_json::json!({"track":title,"artist":t.artist,
            "duration":duration,"isrc":t.isrc,"sourceMetadataOrigin":"registeredLocalAsset"})),
        ..Default::default()
    })
}

pub(super) fn accepts(request: &Track, asset: &LocalAsset, settings: &Settings) -> bool {
    if matches!(
        request.provider.as_str(),
        "local" | "radio" | "youtube" | "soundcloud" | "import"
    ) || request.provenance.get("portablePlaybackSource").is_some()
        || request.playback_range.is_some()
        || crate::playback_sources::source_resolution_is_pinned(request, false, settings, false)
        || asset.track.playback_range.is_some()
        || asset.track.provenance["automaticExcerpt"].is_object()
        || asset.track.provenance["chapter"].is_object()
        || !request
            .duration_seconds
            .is_some_and(|d| d.is_finite() && d > 0.)
        || !asset
            .duration_seconds
            .is_some_and(|d| d.is_finite() && d > 0.)
    {
        return false;
    }
    let Some(candidate) = candidate(asset) else {
        return false;
    };
    let assessment = crate::recording_match::assess_with_options(
        request,
        &candidate,
        settings.matching_level,
        MatchStage::Resolved,
        &settings.flexible_matching,
    );
    // A downloaded badge describes this recording, not an optional substitute.
    assessment.action == MatchAction::Accept && assessment.relationship == MatchRelationship::Exact
}

fn effective_asset(conn: &Connection, row: &AssetHydrationRow) -> Result<Option<LocalAsset>> {
    if !Path::new(&row.1).is_file() {
        return Ok(None);
    }
    let mut asset: LocalAsset = decode(row.3.clone())?;
    // Apply saved user edits without recursively hydrating another local match.
    let (values,recording_id): (RecordingHydration,String) = conn.query_row("SELECT r.favorite,r.disposition,r.manual_metadata,r.id FROM recordings r JOIN local_assets a ON a.recording_id=r.id WHERE a.id=?1", [&row.0],
        |r| Ok(((r.get(0)?,r.get(1)?,r.get(2)?),r.get(3)?))).map_err(err)?;
    asset.id = row.0.clone();
    asset.recording_id = recording_id;
    asset.track = apply_hydration_metadata(asset.track, values)?.0;
    asset.path = row.1.clone();
    asset.hash = row.2.clone();
    Ok(Some(asset))
}

pub(super) fn effective_file(conn: &Connection, row: &AssetHydrationRow) -> Result<LocalAsset> {
    let mut asset = effective_asset(conn, row)?.ok_or("Local file disappeared during matching")?;
    asset.track.local_path = Some(asset.path.clone());
    asset.track.asset_id = Some(asset.id.clone());
    asset.track.local_availability = "available".into();
    Ok(asset)
}

pub(super) fn find(
    conn: &Connection,
    request: &Track,
    settings: &Settings,
) -> Result<Option<AssetHydrationRow>> {
    if title_key(&request.title).is_none() {
        return Ok(None);
    }
    if matches!(
        request.provider.as_str(),
        "local" | "radio" | "youtube" | "soundcloud" | "import"
    ) {
        return Ok(None);
    }
    let Some(duration) = request
        .duration_seconds
        .filter(|d| d.is_finite() && *d > 0.)
    else {
        return Ok(None);
    };
    let artist = normalize_artist_name(&request.artist);
    if artist.is_empty() {
        return Ok(None);
    }
    let (min, max) = runtime_window(duration);
    let mut stmt = conn.prepare_cached(CANDIDATE_SQL).map_err(err)?;
    let rows = stmt
        .query_map(
            params![
                artist,
                min,
                max,
                (CANDIDATE_LIMIT + 1) as i64,
                title_key(&request.title)
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(err)?
        .collect::<std::result::Result<Vec<AssetHydrationRow>, _>>()
        .map_err(err)?;
    if rows.len() > CANDIDATE_LIMIT {
        return Ok(None);
    }
    let mut found = None;
    for row in rows {
        if let Some(asset) = effective_asset(conn, &row)? {
            if accepts(request, &asset, settings) {
                if found.is_some() {
                    return Ok(None);
                } // ambiguous files fail closed
                found = Some(row);
            }
        }
    }
    Ok(found)
}

pub(super) fn selected(
    conn: &Connection,
    request: &Track,
    settings: &Settings,
) -> Result<Option<SelectedHydrationAsset>> {
    let Some(row) = find(conn, request, settings)? else {
        return Ok(None);
    };
    let analysis = conn.query_row("SELECT status,version,payload FROM asset_analyses WHERE asset_id=?1 AND file_hash=?2 ORDER BY updated_at DESC LIMIT 1",
        params![row.0,row.2], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
    Ok(Some((row, analysis, true)))
}

pub(super) fn populate_available(conn: &Connection, settings: &Settings) -> Result<()> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS page_local_matches(listing_id TEXT PRIMARY KEY); DELETE FROM page_local_matches;").map_err(err)?;
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS page_local_match_checks(listing_id TEXT PRIMARY KEY); DELETE FROM page_local_match_checks;").map_err(err)?;
    // Stream assets rather than materializing the entire library's payloads.
    let mut assets = conn
        .prepare_cached(
            "SELECT id,path,hash,payload FROM local_assets WHERE length(payload)<=262144",
        )
        .map_err(err)?;
    let rows = assets
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(err)?;
    for row in rows {
        let row: AssetHydrationRow = row.map_err(err)?;
        let Some(asset) = effective_asset(conn, &row)? else {
            continue;
        };
        let Some(duration) = asset.duration_seconds.filter(|d| d.is_finite() && *d > 0.) else {
            continue;
        };
        let artist = normalize_artist_name(&asset.track.artist);
        if artist.is_empty() {
            continue;
        }
        let (min, max) = runtime_window(duration);
        let rows = {
            let mut stmt = conn.prepare_cached(LISTING_SQL).map_err(err)?;
            let mapped = stmt
                .query_map(
                    params![
                        artist,
                        min,
                        max,
                        (LISTING_LIMIT + 1) as i64,
                        title_key(&asset.track.title)
                    ],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            (r.get(2)?, r.get(3)?, r.get(4)?),
                        ))
                    },
                )
                .map_err(err)?;
            mapped
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
        };
        if rows.len() > LISTING_LIMIT {
            continue;
        }
        for (id, raw, values) in rows {
            if conn
                .execute(
                    "INSERT OR IGNORE INTO page_local_match_checks(listing_id) VALUES(?1)",
                    [&id],
                )
                .map_err(err)?
                == 0
            {
                continue;
            }
            let request = apply_hydration_metadata(decode(raw)?, values)?.0;
            if find(conn, &request, settings)?.is_some() {
                conn.execute(
                    "INSERT OR IGNORE INTO page_local_matches(listing_id) VALUES(?1)",
                    [id],
                )
                .map_err(err)?;
            }
        }
    }
    Ok(())
}
