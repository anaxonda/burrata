//! Shared Unicode text behavior for the UI-facing SQL and native queue comparators.
use icu_collator::{
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
    CollatorBorrowed, CollatorPreferences,
};
use icu_locale_core::Locale;
use rusqlite::{functions::FunctionFlags, Connection};
use std::sync::OnceLock;

pub const SORT_LOCALE: &str = "en-US";

/// Parse the shared track-sort date contract: YYYY, YYYY-MM, YYYY-MM-DD, or RFC3339.
/// Partial dates map to the first day at midnight UTC; invalid calendar dates remain unknown.
pub fn parse_sort_date(value: &str) -> Option<i64> {
    use chrono::{DateTime, NaiveDate};
    let value = value.trim();
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(|byte| byte.is_ascii_digit()))
    };
    if value.len() == 4 && digits(0..4) {
        let year = value.parse::<i32>().ok()?;
        return Some(
            NaiveDate::from_ymd_opt(year, 1, 1)?
                .and_hms_opt(0, 0, 0)?
                .and_utc()
                .timestamp_millis(),
        );
    }
    if value.len() == 7 && digits(0..4) && bytes[4] == b'-' && digits(5..7) {
        let year = value[..4].parse::<i32>().ok()?;
        let month = value[5..7].parse::<u32>().ok()?;
        return Some(
            NaiveDate::from_ymd_opt(year, month, 1)?
                .and_hms_opt(0, 0, 0)?
                .and_utc()
                .timestamp_millis(),
        );
    }
    if value.len() == 10
        && digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
    {
        return Some(
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)?
                .and_utc()
                .timestamp_millis(),
        );
    }
    let (date_part, time_part) = value.split_once('T')?;
    let date_bytes = date_part.as_bytes();
    if date_part.len() != 10
        || !date_bytes[..4].iter().all(|byte| byte.is_ascii_digit())
        || date_bytes[4] != b'-'
        || !date_bytes[5..7].iter().all(|byte| byte.is_ascii_digit())
        || date_bytes[7] != b'-'
        || !date_bytes[8..10].iter().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    let (clock, zone) = if let Some(clock) = time_part.strip_suffix('Z') {
        (clock, "Z")
    } else {
        let offset_at = time_part
            .char_indices()
            .rev()
            .find(|(_, ch)| *ch == '+' || *ch == '-')?
            .0;
        (&time_part[..offset_at], &time_part[offset_at..])
    };
    let (whole_clock, fraction) = match clock.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (clock, None),
    };
    let clock_bytes = whole_clock.as_bytes();
    if whole_clock.len() != 8
        || clock_bytes[2] != b':'
        || clock_bytes[5] != b':'
        || !clock_bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 2 | 5) || byte.is_ascii_digit())
        || fraction.is_some_and(|part| {
            part.is_empty() || part.len() > 9 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }
    let hour = whole_clock[0..2].parse::<u32>().ok()?;
    let minute = whole_clock[3..5].parse::<u32>().ok()?;
    let second = whole_clock[6..8].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if zone != "Z" {
        let zone_bytes = zone.as_bytes();
        if zone.len() != 6
            || !zone_bytes.iter().all(|byte| byte.is_ascii())
            || !matches!(zone_bytes[0], b'+' | b'-')
            || zone_bytes[3] != b':'
            || !zone_bytes[1..3].iter().all(|byte| byte.is_ascii_digit())
            || !zone_bytes[4..6].iter().all(|byte| byte.is_ascii_digit())
            || zone[1..3].parse::<u32>().ok()? > 23
            || zone[4..6].parse::<u32>().ok()? > 59
        {
            return None;
        }
    }
    Some(DateTime::parse_from_rfc3339(value).ok()?.timestamp_millis())
}

/// Parse the shared count grammar used by play and view sort fields.
pub fn parse_sort_count(value: &serde_json::Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return (number.is_finite() && number >= 0.0).then_some(number);
    }
    let raw = value.as_str()?.trim().to_ascii_lowercase().replace(',', "");
    let mut number = raw.as_str();
    for label in ["plays", "play", "views", "view"] {
        if let Some(prefix) = number.strip_suffix(label) {
            number = prefix.trim_end();
            break;
        }
    }
    let (number, multiplier) = match number.as_bytes().last().copied() {
        Some(b'k') => (&number[..number.len() - 1], 1_000.0),
        Some(b'm') => (&number[..number.len() - 1], 1_000_000.0),
        Some(b'b') => (&number[..number.len() - 1], 1_000_000_000.0),
        _ => (number, 1.0),
    };
    let number = number.trim();
    let mut parts = number.split('.');
    let whole = parts.next()?;
    let fraction = parts.next();
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction
            .is_some_and(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        || parts.next().is_some()
    {
        return None;
    }
    let parsed = number.parse::<f64>().ok()? * multiplier;
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

/// Normalize a key using the same pitch contract as the UI and the shared SQL sort expression.
pub fn parse_sort_key(value: &str) -> Option<i32> {
    let normalized = value
        .trim_matches(' ')
        .to_ascii_lowercase()
        .replace('♯', "#")
        .replace('♭', "b")
        .replace(' ', "");
    if let Some((digits, suffix)) = normalized
        .strip_suffix('a')
        .map(|digits| (digits, 'a'))
        .or_else(|| normalized.strip_suffix('b').map(|digits| (digits, 'b')))
    {
        if let Ok(number) = digits.parse::<i32>() {
            if (1..=12).contains(&number) && number.to_string() == digits {
                let minor_pitch = (number * 7 + 1) % 12;
                return Some(if suffix == 'a' {
                    minor_pitch * 2 + 1
                } else {
                    ((minor_pitch + 3) % 12) * 2
                });
            }
        }
    }
    match normalized.as_str() {
        "c" | "b#" | "cmajor" | "cmaj" | "b#major" | "b#maj" => Some(0),
        "cminor" | "cmin" | "cm" | "b#minor" | "b#min" | "b#m" => Some(1),
        "c#" | "db" | "c#major" | "c#maj" | "dbmajor" | "dbmaj" => Some(2),
        "c#minor" | "c#min" | "c#m" | "dbminor" | "dbmin" | "dbm" => Some(3),
        "d" | "dmajor" | "dmaj" => Some(4),
        "dminor" | "dmin" | "dm" => Some(5),
        "d#" | "eb" | "d#major" | "d#maj" | "ebmajor" | "ebmaj" => Some(6),
        "d#minor" | "d#min" | "d#m" | "ebminor" | "ebmin" | "ebm" => Some(7),
        "e" | "fb" | "emajor" | "emaj" | "fbmajor" | "fbmaj" => Some(8),
        "eminor" | "emin" | "em" | "fbminor" | "fbmin" | "fbm" => Some(9),
        "e#" | "f" | "e#major" | "e#maj" | "fmajor" | "fmaj" => Some(10),
        "e#minor" | "e#min" | "e#m" | "fminor" | "fmin" | "fm" => Some(11),
        "f#" | "gb" | "f#major" | "f#maj" | "gbmajor" | "gbmaj" => Some(12),
        "f#minor" | "f#min" | "f#m" | "gbminor" | "gbmin" | "gbm" => Some(13),
        "g" | "gmajor" | "gmaj" => Some(14),
        "gminor" | "gmin" | "gm" => Some(15),
        "g#" | "ab" | "g#major" | "g#maj" | "abmajor" | "abmaj" => Some(16),
        "g#minor" | "g#min" | "g#m" | "abminor" | "abmin" | "abm" => Some(17),
        "a" | "amajor" | "amaj" => Some(18),
        "aminor" | "amin" | "am" => Some(19),
        "a#" | "bb" | "a#major" | "a#maj" | "bbmajor" | "bbmaj" => Some(20),
        "a#minor" | "a#min" | "a#m" | "bbminor" | "bbmin" | "bbm" => Some(21),
        "b" | "cb" | "bmajor" | "bmaj" | "cbmajor" | "cbmaj" => Some(22),
        "bminor" | "bmin" | "bm" | "cbminor" | "cbmin" | "cbm" => Some(23),
        _ => None,
    }
}

pub fn lowercase(value: &str) -> String {
    value.to_lowercase()
}
static COLLATOR: OnceLock<std::result::Result<CollatorBorrowed<'static>, String>> = OnceLock::new();

/// Return the one configured collator. Initialization errors propagate; there is no lexical fallback.
pub fn collator() -> std::result::Result<&'static CollatorBorrowed<'static>, String> {
    match COLLATOR.get_or_init(|| {
        let locale = Locale::try_from_str(SORT_LOCALE).map_err(|error| error.to_string())?;
        let mut preferences = CollatorPreferences::from(locale);
        preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        let mut options = CollatorOptions::default();
        options.strength = Some(Strength::Primary);
        CollatorBorrowed::try_new(preferences, options).map_err(|error| error.to_string())
    }) {
        Ok(value) => Ok(value),
        Err(error) => Err(error.clone()),
    }
}

/// Install the Unicode sort collation and lowercase function on every SQL connection that uses
/// track-sort expressions. Locale/data initialization failure prevents the connection from opening.
pub fn register_sqlite(connection: &Connection) -> std::result::Result<(), String> {
    let text_collator = collator()?;
    connection
        .create_collation("TRACK_TEXT", move |left, right| {
            text_collator.compare(left, right)
        })
        .map_err(|error| error.to_string())?;
    connection
        .create_scalar_function(
            "strict_sort_count",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let value: rusqlite::types::Value = context.get(0)?;
                let json = match value {
                    rusqlite::types::Value::Integer(number) => serde_json::json!(number),
                    rusqlite::types::Value::Real(number) => serde_json::Number::from_f64(number)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    rusqlite::types::Value::Text(value) => serde_json::Value::String(value),
                    _ => serde_json::Value::Null,
                };
                Ok(parse_sort_count(&json))
            },
        )
        .map_err(|error| error.to_string())?;
    connection
        .create_scalar_function(
            "strict_sort_date",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let value: rusqlite::types::Value = context.get(0)?;
                let parsed = match value {
                    rusqlite::types::Value::Text(value) => parse_sort_date(&value),
                    _ => None,
                };
                Ok(parsed)
            },
        )
        .map_err(|error| error.to_string())?;
    connection
        .create_scalar_function(
            "unicode_lower",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let value: Option<String> = context.get(0)?;
                Ok(value.map(|value| lowercase(&value)))
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixtures() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/track-sort-conformance.json"
        ))
        .unwrap()
    }

    #[test]
    fn shared_text_fixtures_match_sql_unicode_lowercase_and_collation() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CollatorBorrowed<'static>>();
        let connection = Connection::open_in_memory().unwrap();
        register_sqlite(&connection).unwrap();
        let fixtures = fixtures();
        assert_eq!(fixtures["locale"].as_str().unwrap(), SORT_LOCALE);
        for case in fixtures["queryCases"].as_array().unwrap() {
            let actual: bool = connection
                .query_row(
                    "SELECT instr(unicode_lower(?1), unicode_lower(?2)) > 0",
                    rusqlite::params![
                        case["text"].as_str().unwrap(),
                        case["query"].as_str().unwrap()
                    ],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, case["matches"].as_bool().unwrap());
        }
        for case in fixtures["sortCases"].as_array().unwrap() {
            let input = case["input"].as_array().unwrap();
            connection.execute_batch("DROP TABLE IF EXISTS fixture_sort; CREATE TEMP TABLE fixture_sort(value TEXT, occurrence INTEGER);").unwrap();
            for (index, value) in input.iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO fixture_sort(value, occurrence) VALUES(?1, ?2)",
                        rusqlite::params![value.as_str().unwrap(), index as i64],
                    )
                    .unwrap();
            }
            let actual = connection
                .prepare(
                    "SELECT value FROM fixture_sort ORDER BY value COLLATE TRACK_TEXT, occurrence",
                )
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let expected = case["expected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                actual.iter().map(String::as_str).collect::<Vec<_>>(),
                expected
            );
        }
        for pair in fixtures["equalPairs"].as_array().unwrap() {
            let ordering: bool = connection
                .query_row(
                    "SELECT ?1 COLLATE TRACK_TEXT = ?2",
                    rusqlite::params![pair[0].as_str().unwrap(), pair[1].as_str().unwrap()],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(ordering, "fixture pair should compare equal: {pair}");
        }
        let tied = fixtures["stableTies"].as_array().unwrap();
        connection.execute_batch("DROP TABLE IF EXISTS fixture_sort; CREATE TEMP TABLE fixture_sort(value TEXT, occurrence INTEGER);").unwrap();
        for (index, value) in tied.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO fixture_sort(value, occurrence) VALUES(?1, ?2)",
                    rusqlite::params![value.as_str().unwrap(), index as i64],
                )
                .unwrap();
        }
        let actual = connection
            .prepare("SELECT value FROM fixture_sort ORDER BY value COLLATE TRACK_TEXT, occurrence")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let expected = tied
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actual.iter().map(String::as_str).collect::<Vec<_>>(),
            expected,
            "primary-strength equality preserves occurrence order"
        );
    }
}
