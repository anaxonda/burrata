//! SQLite sort expressions shared by paged library and playback-history views.

use rusqlite::{
    functions::{Aggregate, Context, FunctionFlags},
    Connection,
};
use serde_json::Value;

/// Bump when field-comparability rules change; persisted masks with older
/// revisions are treated as pending and recomputed.
pub const AVAILABILITY_MASK_REVISION: i64 = 1;

pub fn pack_available_fields(fields: &[bool; 10]) -> i64 {
    fields.iter().enumerate().fold(0_i64, |mask, (bit, value)| {
        mask | (i64::from(*value) << bit)
    })
}

pub fn unpack_available_fields(mask: i64) -> [bool; 10] {
    std::array::from_fn(|bit| mask & (1_i64 << bit) != 0)
}

const AVAILABILITY_PENDING_BIT: i64 = 1 << 10;

struct AvailabilityBitOr;

impl Aggregate<i64, Option<i64>> for AvailabilityBitOr {
    fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<i64> {
        Ok(0)
    }

    fn step(&self, context: &mut Context<'_>, mask: &mut i64) -> rusqlite::Result<()> {
        match context.get::<Option<i64>>(0)? {
            Some(value) => *mask |= value & 1023,
            None => *mask |= AVAILABILITY_PENDING_BIT,
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, mask: Option<i64>) -> rusqlite::Result<Option<i64>> {
        Ok(mask)
    }
}

pub fn register_availability_aggregate(connection: &Connection) -> rusqlite::Result<()> {
    connection.create_aggregate_function(
        "sort_availability_or",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        AvailabilityBitOr,
    )
}

pub fn aggregate_result_is_pending(mask: Option<i64>) -> bool {
    mask.is_some_and(|value| value & AVAILABILITY_PENDING_BIT != 0)
}

/// Build a SQL expression for a supported track sort field from a safe JSON expression.
/// `metadata` is SQL supplied by the caller, such as `json_patch(l.metadata,r.manual_metadata)`.
/// Unknown fields (including source order) return `None` so callers can keep their row order.
pub fn expression(field: &str, metadata: &str) -> Option<String> {
    let json = |path: &str| format!("json_extract({metadata},'$.{path}')");
    Some(match field {
        "title" | "artist" | "album" => {
            format!("NULLIF(trim({}),'') COLLATE TRACK_TEXT", json(field))
        }
        "duration" => format!("CAST(({}) AS INTEGER)", duration_expression(metadata)),
        "releaseDate" => date_expression(&format!(
            "COALESCE({}, {})",
            json("releaseDate"),
            json("recordingDate")
        )),
        "dateAdded" => date_expression(&format!(
            "COALESCE({}, {}, {}, {}, {})",
            json("dateAdded"),
            json("provenance.dateAdded"),
            json("provenance.addedAt"),
            json("provenance.importedAt"),
            json("provenance.createdAt")
        )),
        "bpm" => format!(
            "CASE WHEN CAST({} AS REAL)>0 THEN CAST({} AS REAL) END",
            json("bpm"),
            json("bpm")
        ),
        "key" => key_expression(metadata),
        "plays" => count_expression(metadata, &["plays", "playCount", "play_count"]),
        "views" => count_expression(metadata, &["views", "viewCount", "view_count"]),
        _ => return None,
    })
}

/// Compute sort-field availability from one effective metadata value.
/// The order matches the page response fields in storage.rs.
pub fn available_fields(metadata: &Value) -> [bool; 10] {
    let mut found = [false; 10];
    extend_available_fields(metadata, &mut found);
    found
}

#[derive(Clone, Copy)]
pub struct AvailabilityJsonPath {
    pub path: &'static str,
    field: usize,
}

/// Return only JSON paths needed by fields still absent after the first-row SQL probe.
pub fn availability_json_paths(found: &[bool; 10]) -> Vec<AvailabilityJsonPath> {
    const PATHS: &[(&str, usize)] = &[
        ("$.title", 0),
        ("$.artist", 1),
        ("$.album", 2),
        ("$.durationSeconds", 3),
        ("$.playbackRange", 3),
        ("$.playbackRange.startTime", 3),
        ("$.playbackRange.endTime", 3),
        ("$.releaseDate", 4),
        ("$.recordingDate", 4),
        ("$.dateAdded", 5),
        ("$.provenance.dateAdded", 5),
        ("$.provenance.addedAt", 5),
        ("$.provenance.importedAt", 5),
        ("$.provenance.createdAt", 5),
        ("$.bpm", 6),
        ("$.key", 7),
        ("$.provenance.plays", 8),
        ("$.provenance.playCount", 8),
        ("$.provenance.play_count", 8),
        ("$.provenance.remoteCatalog.plays", 8),
        ("$.provenance.remoteCatalog.playCount", 8),
        ("$.provenance.remoteCatalog.play_count", 8),
        ("$.provenance.views", 9),
        ("$.provenance.viewCount", 9),
        ("$.provenance.view_count", 9),
        ("$.provenance.remoteCatalog.views", 9),
        ("$.provenance.remoteCatalog.viewCount", 9),
        ("$.provenance.remoteCatalog.view_count", 9),
    ];
    PATHS
        .iter()
        .filter(|(_, field)| !found[*field])
        .map(|(path, field)| AvailabilityJsonPath {
            path,
            field: *field,
        })
        .collect()
}

pub fn availability_projection_expression(
    metadata: &str,
    paths: &[AvailabilityJsonPath],
) -> String {
    let mut arguments = paths
        .iter()
        .map(|path| format!("'{}'", path.path))
        .collect::<Vec<_>>();
    // SQLite returns a scalar for a one-path json_extract. A second absent path
    // forces an array and retains JSON booleans/objects as JSON values.
    if arguments.len() == 1 {
        arguments.push("'$.__availability_dummy__'".to_owned());
    }
    format!("json_extract({metadata},{})", arguments.join(","))
}

/// Extend availability from the compact JSON array returned by SQLite multi-path extraction.
pub fn extend_available_fields_from_projection(
    projected: &[Value],
    paths: &[AvailabilityJsonPath],
    found: &mut [bool; 10],
) {
    let mut metadata = serde_json::Map::new();
    let mut range_marker = None;
    for (path, value) in paths.iter().zip(projected) {
        if path.path == "$.playbackRange" {
            range_marker = Some(value.clone());
            continue;
        }
        insert_projection_value(
            &mut metadata,
            path.path.trim_start_matches("$."),
            value.clone(),
        );
    }
    if let Some(marker) = range_marker {
        if marker.is_null() {
            metadata.insert("playbackRange".into(), Value::Null);
        } else if marker.is_object() {
            // Read range endpoints by separate SQL paths: SQLite chooses the first
            // duplicate key, while serde_json's normal map parser chooses the last.
            let range = metadata
                .remove("playbackRange")
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            metadata.insert("playbackRange".into(), range);
        } else {
            metadata.insert("playbackRange".into(), marker);
        }
    }
    extend_available_fields(&Value::Object(metadata), found);
}

fn insert_projection_value(root: &mut serde_json::Map<String, Value>, path: &str, value: Value) {
    let Some((head, tail)) = path.split_once('.') else {
        root.insert(path.to_owned(), value);
        return;
    };
    let child = root
        .entry(head.to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !child.is_object() {
        *child = Value::Object(serde_json::Map::new());
    }
    insert_projection_value(child.as_object_mut().unwrap(), tail, value);
}

/// Extend a global presence mask, skipping work for fields already known in prior rows.
pub fn extend_available_fields(metadata: &Value, found: &mut [bool]) {
    if !found[0] {
        found[0] = metadata.get("title").is_some_and(sqlite_text_nonempty);
    }
    if !found[1] {
        found[1] = metadata.get("artist").is_some_and(sqlite_text_nonempty);
    }
    if !found[2] {
        found[2] = metadata.get("album").is_some_and(sqlite_text_nonempty);
    }
    if !found[3] {
        found[3] = duration_is_available(metadata);
    }
    if !found[4] {
        let release_date =
            coalesce_non_null(metadata.get("releaseDate"), metadata.get("recordingDate"));
        found[4] = release_date
            .and_then(Value::as_str)
            .and_then(crate::track_sort_text::parse_sort_date)
            .is_some();
    }
    if !found[5] {
        let date_added = [
            metadata.get("dateAdded"),
            metadata
                .get("provenance")
                .and_then(|value| value.get("dateAdded")),
            metadata
                .get("provenance")
                .and_then(|value| value.get("addedAt")),
            metadata
                .get("provenance")
                .and_then(|value| value.get("importedAt")),
            metadata
                .get("provenance")
                .and_then(|value| value.get("createdAt")),
        ]
        .into_iter()
        .find(|value| value.is_some_and(|value| !value.is_null()))
        .flatten();
        found[5] = date_added
            .and_then(Value::as_str)
            .and_then(crate::track_sort_text::parse_sort_date)
            .is_some();
    }
    if !found[6] {
        found[6] = metadata
            .get("bpm")
            .and_then(sqlite_cast_real)
            .is_some_and(|value| value > 0.0);
    }
    if !found[7] {
        found[7] = metadata.get("key").is_some_and(key_is_available);
    }
    let provenance = metadata.get("provenance").unwrap_or(&Value::Null);
    if !found[8] {
        found[8] = count_is_available(provenance, &["plays", "playCount", "play_count"]);
    }
    if !found[9] {
        found[9] = count_is_available(provenance, &["views", "viewCount", "view_count"]);
    }
}

fn sqlite_text_nonempty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim_matches(' ').is_empty(),
        other => sqlite_json_text(other).is_some_and(|text| !text.trim_matches(' ').is_empty()),
    }
}

fn key_is_available(value: &Value) -> bool {
    match value {
        Value::String(value) => crate::track_sort_text::parse_sort_key(value).is_some(),
        other => sqlite_json_text(other)
            .and_then(|text| crate::track_sort_text::parse_sort_key(&text))
            .is_some(),
    }
}

fn coalesce_non_null<'a>(first: Option<&'a Value>, second: Option<&'a Value>) -> Option<&'a Value> {
    first
        .filter(|value| !value.is_null())
        .or_else(|| second.filter(|value| !value.is_null()))
}

fn count_is_available(provenance: &Value, names: &[&str]) -> bool {
    fn first_non_null<'a>(container: &'a Value, names: &[&str]) -> Option<&'a Value> {
        names
            .iter()
            .filter_map(|name| container.get(*name))
            .find(|value| !value.is_null())
    }
    first_non_null(provenance, names)
        .and_then(|value| crate::track_sort_text::parse_sort_count(value))
        .or_else(|| {
            provenance
                .get("remoteCatalog")
                .and_then(|container| first_non_null(container, names))
                .and_then(|value| crate::track_sort_text::parse_sort_count(value))
        })
        .is_some()
}

fn duration_is_available(metadata: &Value) -> bool {
    if let Some(range) = metadata
        .get("playbackRange")
        .filter(|value| !value.is_null())
    {
        let start = range
            .get("startTime")
            .filter(|value| !value.is_null())
            .and_then(sqlite_cast_real);
        let end = range
            .get("endTime")
            .filter(|value| !value.is_null())
            .and_then(sqlite_cast_real);
        return matches!((start, end), (Some(start), Some(end)) if end - start > 0.0);
    }
    metadata
        .get("durationSeconds")
        .and_then(sqlite_cast_real)
        .is_some_and(|value| value > 0.0)
}

fn sqlite_json_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

/// Approximate SQLite CAST(... AS REAL) numeric-prefix parsing for JSON scalar values.
fn sqlite_cast_real(value: &Value) -> Option<f64> {
    match value {
        Value::Null | Value::Array(_) | Value::Object(_) => Some(0.0),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::Number(value) => value.as_f64(),
        Value::String(value) => Some(sqlite_numeric_prefix(value)),
    }
}

fn sqlite_numeric_prefix(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let mut end = bytes
        .iter()
        .position(|byte| !matches!(*byte, b' ' | b'\t'..=b'\r'))
        .unwrap_or(bytes.len());
    let start = end;
    if bytes
        .get(end)
        .is_some_and(|byte| matches!(*byte, b'+' | b'-'))
    {
        end += 1;
    }
    let mut digits = 0;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    let exponent_start = end;
    if bytes
        .get(end)
        .is_some_and(|byte| matches!(*byte, b'e' | b'E'))
    {
        end += 1;
        if bytes
            .get(end)
            .is_some_and(|byte| matches!(*byte, b'+' | b'-'))
        {
            end += 1;
        }
        let exponent_digits = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == exponent_digits {
            end = exponent_start;
        }
    }
    std::str::from_utf8(&bytes[start..end])
        .ok()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0.0)
}

/// Chapter ranges take precedence; invalid, zero, and negative values sort as unknown.
pub fn duration_expression(metadata: &str) -> String {
    let start = format!("CAST(json_extract({metadata},'$.playbackRange.startTime') AS REAL)");
    let end = format!("CAST(json_extract({metadata},'$.playbackRange.endTime') AS REAL)");
    let duration = format!("CAST(json_extract({metadata},'$.durationSeconds') AS REAL)");
    format!(
        "CASE WHEN json_extract({metadata},'$.playbackRange') IS NOT NULL THEN CASE WHEN json_extract({metadata},'$.playbackRange.endTime') IS NOT NULL AND json_extract({metadata},'$.playbackRange.startTime') IS NOT NULL AND {end}-{start}>0 THEN {end}-{start} END ELSE CASE WHEN {duration}>0 THEN {duration} END END"
    )
}

fn date_expression(value: &str) -> String {
    // Avoid crossing the Rust UDF boundary for the common missing-metadata case.
    format!(
        "CASE WHEN typeof({value})='text' AND length(trim({value}))>0 THEN strict_sort_date({value}) END"
    )
}

fn key_expression(metadata: &str) -> String {
    let key = format!(
        "lower(replace(replace(replace(trim(json_extract({metadata},'$.key')),'♯','#'),'♭','b'),' ',''))"
    );
    let mut cases = Vec::new();
    for n in 1..=12 {
        let pitch = (n * 7 + 1) % 12;
        cases.push(format!("WHEN '{n}a' THEN {}", pitch * 2 + 1));
        cases.push(format!("WHEN '{n}b' THEN {}", ((pitch + 3) % 12) * 2));
    }
    for (note, pitch) in [
        ("c", 0),
        ("b#", 0),
        ("c#", 1),
        ("db", 1),
        ("d", 2),
        ("d#", 3),
        ("eb", 3),
        ("e", 4),
        ("fb", 4),
        ("e#", 5),
        ("f", 5),
        ("f#", 6),
        ("gb", 6),
        ("g", 7),
        ("g#", 8),
        ("ab", 8),
        ("a", 9),
        ("a#", 10),
        ("bb", 10),
        ("b", 11),
        ("cb", 11),
    ] {
        for suffix in ["", "major", "maj"] {
            cases.push(format!("WHEN '{note}{suffix}' THEN {}", pitch * 2));
        }
        for suffix in ["minor", "min", "m"] {
            cases.push(format!("WHEN '{note}{suffix}' THEN {}", pitch * 2 + 1));
        }
    }
    format!("CASE {key} {} END", cases.join(" "))
}

fn count_expression(metadata: &str, names: &[&str]) -> String {
    let expressions = |prefix: &str, extract: &str| {
        names
            .iter()
            .map(|name| format!("{extract}({metadata},'$.{prefix}{name}')"))
            .collect::<Vec<_>>()
    };
    let top = format!(
        "COALESCE({})",
        expressions("provenance.", "json_extract").join(",")
    );
    let remote = format!(
        "COALESCE({})",
        expressions("provenance.remoteCatalog.", "json_extract").join(",")
    );
    let top_type = format!(
        "COALESCE({})",
        expressions("provenance.", "json_type")
            .iter()
            .map(|expression| format!("NULLIF({expression},'null')"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let remote_type = format!(
        "COALESCE({})",
        expressions("provenance.remoteCatalog.", "json_type")
            .iter()
            .map(|expression| format!("NULLIF({expression},'null')"))
            .collect::<Vec<_>>()
            .join(",")
    );
    // Keep the remote fallback for invalid top-level values, while skipping UDF calls when a
    // source has no count at all (common for sparse provider metadata).
    format!(
        "COALESCE(CASE WHEN {top} IS NOT NULL AND {top_type} IN ('integer','real','text') THEN strict_sort_count({top}) END,CASE WHEN {remote} IS NOT NULL AND {remote_type} IN ('integer','real','text') THEN strict_sort_count({remote}) END)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_sort_expression_uses_whole_seconds_while_filter_expression_stays_exact() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let sort = expression("duration", "?1").unwrap();
        let exact = duration_expression("?1");
        for value in [30.1, 30.9] {
            let metadata = format!(r#"{{"durationSeconds":{value}}}"#);
            let sorted: i64 = conn
                .query_row(&format!("SELECT {sort}"), [&metadata], |row| row.get(0))
                .unwrap();
            let filtered: f64 = conn
                .query_row(&format!("SELECT {exact}"), [&metadata], |row| row.get(0))
                .unwrap();
            assert_eq!(sorted, 30);
            assert_eq!(filtered, value);
        }
    }

    #[test]
    fn partial_chapter_range_is_unknown_instead_of_falling_back_to_parent_duration() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let expression = duration_expression("?1");
        let actual: Option<f64> = conn
            .query_row(
                &format!("SELECT {expression}"),
                [r#"{"durationSeconds":240,"playbackRange":{"startTime":10}}"#],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual, None);
    }

    #[test]
    fn invalid_keys_and_counts_are_unknown_and_reported_zero_is_known() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&conn).unwrap();
        for (field, metadata, expected) in [
            ("key", r#"{"key":"Cgarbage"}"#, None),
            ("key", r#"{"key":"D♭ major"}"#, Some(2.0)),
            ("views", r#"{"provenance":{"views":"unknown"}}"#, None),
            ("views", r#"{"provenance":{"views":0}}"#, Some(0.0)),
            ("plays", r#"{"provenance":{"plays":true}}"#, None),
            (
                "plays",
                r#"{"provenance":{"plays":true,"remoteCatalog":{"plays":5}}}"#,
                Some(5.0),
            ),
            ("views", r#"{"provenance":{"views":false}}"#, None),
            (
                "plays",
                r#"{"provenance":{"remoteCatalog":{"plays":"1,250"}}}"#,
                Some(1250.0),
            ),
        ] {
            let expression = expression(field, "?1").unwrap();
            let actual: Option<f64> = conn
                .query_row(&format!("SELECT {expression}"), [metadata], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(actual, expected, "{field}: {metadata}");
        }
    }

    #[test]
    fn missing_date_and_count_values_stay_unknown_with_guarded_udfs() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&conn).unwrap();
        for (field, metadata) in [
            ("dateAdded", r#"{}"#),
            ("releaseDate", r#"{"releaseDate":"   "}"#),
            ("plays", r#"{}"#),
            ("views", r#"{"provenance":{"views":null},"provenance2":{}}"#),
        ] {
            let sql = expression(field, "?1").unwrap();
            let actual: Option<f64> = conn
                .query_row(&format!("SELECT {sql}"), [metadata], |row| row.get(0))
                .unwrap();
            assert_eq!(actual, None, "{field}: {metadata}");
        }
    }

    #[test]
    fn one_pass_availability_matches_shared_sql_expressions() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&conn).unwrap();
        let fields = [
            "title",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
            "views",
        ];
        let samples = [
            serde_json::json!({
                "title":"  Song ","artist":12,"album":null,"durationSeconds":"30.9x",
                "releaseDate":null,"recordingDate":"2020-02","dateAdded":"bad",
                "provenance":{"createdAt":"2022","plays":"unknown","remoteCatalog":{"plays":"1.25k"}},
                "bpm":"128.5 bpm","key":" D♭ major "
            }),
            serde_json::json!({
                "title":" ","artist":false,"album":{},"playbackRange":{"startTime":"10","endTime":"12"},
                "releaseDate":"2023-02-29","dateAdded":"2024-01-02T03:04:05Z",
                "provenance":{"plays":0,"views":"2 views"},"bpm":0,"key":"Cgarbage"
            }),
            serde_json::json!({
                "title":null,"artist":"Artist","album":"Album","playbackRange":{},
                "releaseDate":"2018","dateAdded":null,
                "provenance":{"dateAdded":"2021-03-04","views":"bad","remoteCatalog":{"views":9}},
                "durationSeconds":45,"bpm":-1,"key":"12b"
            }),
            serde_json::json!({
                "title":true,"artist":[],"album":"Album","durationSeconds":0.5,
                "releaseDate":false,"dateAdded":" ","provenance":{"plays":null},
                "bpm":"not a number","key":"A♯ minor"
            }),
            serde_json::json!({
                "provenance":{"plays":true,"remoteCatalog":{"plays":"1.25k"},"views":false}
            }),
            serde_json::json!({
                "playbackRange":{"startTime":null,"endTime":10},"durationSeconds":30,
                "releaseDate":"","recordingDate":"2020-01-01",
                "dateAdded":" ","provenance":{"dateAdded":"2021-01-01"},
                "key":" C\tmajor "
            }),
            serde_json::json!({
                "playbackRange":{"startTime":10,"endTime":null},"durationSeconds":30
            }),
            serde_json::json!({
                "playbackRange":{"startTime":10,"endTime":10.1},
                "releaseDate":"","recordingDate":"2020-01-01",
                "key":"　C major　"
            }),
        ];
        for metadata in samples {
            let raw = metadata.to_string();
            let rust_values = available_fields(&metadata);
            for (index, field) in fields.iter().enumerate() {
                let sql = expression(field, "?1").unwrap();
                let sql_value: bool = conn
                    .query_row(&format!("SELECT ({sql}) IS NOT NULL"), [&raw], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(
                    rust_values[index], sql_value,
                    "field={field} metadata={metadata}"
                );
            }
        }
    }

    #[test]
    fn compact_json_projection_matches_sql_presence_and_prior_mask() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&conn).unwrap();
        let fields = [
            "title",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
            "views",
        ];
        let samples = [
            r#"{"title":"first","title":" ","artist":null,"durationSeconds":30,"releaseDate":null,"recordingDate":"2020-01-02","dateAdded":null,"provenance":{"dateAdded":"2021","plays":true,"plays":4,"remoteCatalog":{"plays":0},"views":null,"remoteCatalog":{"views":"1.2k"}},"bpm":true,"key":"8A"}"#,
            r#"{"playbackRange":{"startTime":1,"startTime":null,"endTime":1.1},"durationSeconds":30,"releaseDate":"bad","recordingDate":"2020","provenance":{"plays":"bad","remoteCatalog":{"plays":0}},"key":"b"}"#,
            r#"{"provenance":{"views":false,"remoteCatalog":{"views":9}},"title":[],"artist":{},"album":"  ","bpm":"120x"}"#,
        ];
        for raw in samples {
            for mut prior in [
                [false; 10],
                [
                    true, false, false, true, false, false, false, false, false, true,
                ],
                [true, true, true, true, true, true, true, false, true, true],
            ] {
                let seeded = prior;
                let paths = availability_json_paths(&prior);
                let projection = availability_projection_expression("?1", &paths);
                let compact: String = conn
                    .query_row(&format!("SELECT {projection}"), [raw], |row| row.get(0))
                    .unwrap();
                let values: Vec<Value> = serde_json::from_str(&compact).unwrap();
                extend_available_fields_from_projection(&values, &paths, &mut prior);
                for (index, field) in fields.iter().enumerate() {
                    if seeded[index] {
                        assert!(prior[index], "field={field}; raw={raw}");
                        continue;
                    }
                    let sql = expression(field, "?1").unwrap();
                    let expected: bool = conn
                        .query_row(&format!("SELECT ({sql}) IS NOT NULL"), [raw], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(prior[index], expected, "field={field}; raw={raw}");
                }
            }
        }
    }

    #[test]
    fn one_pass_key_presence_matches_every_sql_note_and_camelot_case() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let notes = [
            "c", "b#", "c#", "db", "d", "d#", "eb", "e", "fb", "e#", "f", "f#", "gb", "g", "g#",
            "ab", "a", "a#", "bb", "b", "cb",
        ];
        let mut keys = Vec::new();
        for note in notes {
            for suffix in ["", "major", "maj", "minor", "min", "m"] {
                keys.push(format!("{note}{suffix}"));
                keys.push(format!(" {note}{suffix} "));
                keys.push(format!(
                    "{}{}",
                    note.to_ascii_uppercase(),
                    suffix.to_ascii_uppercase()
                ));
            }
        }
        for number in 1..=12 {
            keys.push(format!("{number}a"));
            keys.push(format!("{number}b"));
        }
        keys.extend(
            ["0a", "13b", "01a", "01b", "C\tmajor", "　C major　"]
                .into_iter()
                .map(str::to_owned),
        );

        for key in keys {
            let metadata = serde_json::json!({"key":key});
            let raw = metadata.to_string();
            let sql = expression("key", "?1").unwrap();
            let sql_value: bool = conn
                .query_row(&format!("SELECT ({sql}) IS NOT NULL"), [&raw], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(available_fields(&metadata)[7], sql_value, "key={key:?}");
        }
    }

    #[test]
    fn shared_sort_value_fixtures_match_sql_normalization() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::track_sort_text::register_sqlite(&connection).unwrap();
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/track-sort-conformance.json"
        ))
        .unwrap();
        let cases = &fixtures["sortNormalizationCases"];
        for case in cases["numeric"].as_array().unwrap() {
            let metadata = serde_json::json!({"provenance":{"plays":case["value"]}}).to_string();
            let parsed_metadata: Value = serde_json::from_str(&metadata).unwrap();
            let expression = expression("plays", "?1").unwrap();
            let actual: Option<f64> = connection
                .query_row(&format!("SELECT {expression}"), [&metadata], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(actual.is_some(), case["valid"].as_bool().unwrap(), "{case}");
            assert_eq!(
                available_fields(&parsed_metadata)[8],
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
            if let Some(expected) = case["expected"].as_f64() {
                assert_eq!(actual, Some(expected), "{case}");
            }
        }
        for case in cases["camelot"].as_array().unwrap() {
            let value = case["value"].as_str().unwrap();
            let json = serde_json::json!({"key":value});
            let metadata = json.to_string();
            let expression = expression("key", "?1").unwrap();
            let actual: Option<f64> = connection
                .query_row(&format!("SELECT {expression}"), [&metadata], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(actual.is_some(), case["valid"].as_bool().unwrap(), "{case}");
            assert_eq!(
                crate::track_sort_text::parse_sort_key(value).is_some(),
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
            assert_eq!(
                available_fields(&json)[7],
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
        for case in cases["dates"].as_array().unwrap() {
            let value = case["value"].as_str().unwrap();
            let actual = crate::track_sort_text::parse_sort_date(value);
            assert_eq!(actual.is_some(), case["valid"].as_bool().unwrap(), "{case}");
            let sql: Option<i64> = connection
                .query_row(
                    "SELECT strict_sort_date(?1)",
                    [case["value"].as_str().unwrap()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(sql, actual, "{case}");
            if let Some(expected) = case["expected"].as_i64() {
                assert_eq!(sql, Some(expected), "{case}");
            }
            let metadata = serde_json::json!({"releaseDate":value});
            assert_eq!(
                available_fields(&metadata)[4],
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
            let metadata = serde_json::json!({"provenance":{"importedAt":value}});
            assert_eq!(
                available_fields(&metadata)[5],
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn availability_or_aggregate_combines_masks_and_marks_unknown_rows() {
        let connection = Connection::open_in_memory().unwrap();
        register_availability_aggregate(&connection).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE masks(mask INTEGER); INSERT INTO masks VALUES (1),(8),(NULL);",
            )
            .unwrap();
        let combined: Option<i64> = connection
            .query_row("SELECT sort_availability_or(mask) FROM masks", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(combined, Some(1 | 8 | AVAILABILITY_PENDING_BIT));
        assert!(aggregate_result_is_pending(combined));
        let known: Option<i64> = connection
            .query_row(
                "SELECT sort_availability_or(mask) FROM masks WHERE mask IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(known, Some(9));
        assert!(!aggregate_result_is_pending(known));
        let empty: Option<i64> = connection
            .query_row(
                "SELECT sort_availability_or(mask) FROM masks WHERE 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(empty, None);
    }
}
