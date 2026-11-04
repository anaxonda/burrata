//! Pure projection of legacy Discogs credit rows into conservative performer evidence.
use crate::models::Track;
use serde_json::Value;

const MAX_RECOVERABLE_DISCOGS_CREDITS: usize = 32;
const MAX_RECOVERABLE_CREDIT_TEXT: usize = 512;

fn discogs_credit_source_is_eligible(track: &Track) -> bool {
    track.provider == "catalog"
        && track.provenance["catalogProvider"] == "discogs"
        && track.provenance["manualFields"]["artist"] != true
        && track.provenance["manualFields"]["artistCredits"] != true
        && track.provenance["artistCredits"]
            .as_array()
            .is_some_and(|credits| {
                !credits.is_empty() && credits.len() <= MAX_RECOVERABLE_DISCOGS_CREDITS
            })
}

fn recoverable_credit_text(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|text| {
        !text.is_empty()
            && text.len() <= MAX_RECOVERABLE_CREDIT_TEXT
            && !text.chars().any(char::is_control)
    })
}

fn discogs_artist_field_is_placeholder(track: &Track) -> bool {
    matches!(
        track.artist.trim().to_lowercase().as_str(),
        "" | "unknown artist"
    )
}

/// True only for a legacy Discogs primary artist credit that omitted `role`.
/// This evidence is used as a derived view and never rewrites Track provenance.
pub(crate) fn is_legacy_discogs_roleless_primary_credit(track: &Track, credit: &Value) -> bool {
    if !discogs_credit_source_is_eligible(track) {
        return false;
    }
    if credit
        .get("role")
        .is_some_and(|role| role.as_str() != Some(""))
    {
        return false;
    }
    let Some(name) = recoverable_credit_text(&credit["name"]) else {
        return false;
    };
    let provider = credit["artist"]["provider"].as_str().unwrap_or_default();
    let id = credit["artist"]["id"].as_str().unwrap_or_default().trim();
    !name.is_empty()
        && provider.eq_ignore_ascii_case("discogs")
        && !id.is_empty()
        && id != "0"
        && id.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn legacy_discogs_roleless_primary_performer_names(
    track: &Track,
) -> Option<Vec<String>> {
    let credits = track.provenance["artistCredits"].as_array()?;
    let mut names = Vec::new();
    for credit in credits {
        if !is_legacy_discogs_roleless_primary_credit(track, credit) {
            continue;
        }
        let name = recoverable_credit_text(&credit["name"])?;
        if !names
            .iter()
            .any(|known: &String| known.eq_ignore_ascii_case(name))
        {
            names.push(name.to_owned());
        }
    }
    (!names.is_empty()).then_some(names)
}

/// Returns a read-only recovery for placeholder Discogs artist fields. Explicit
/// performer roles and the legacy roleless primary identity are accepted;
/// guest and nonperformer credits remain in provenance but do not drive search.
pub(crate) fn recovered_discogs_performer_artist(track: &Track) -> Option<String> {
    if !discogs_credit_source_is_eligible(track) || !discogs_artist_field_is_placeholder(track) {
        return None;
    }
    let credits = track.provenance["artistCredits"].as_array()?;
    if credits.is_empty() || credits.len() > MAX_RECOVERABLE_DISCOGS_CREDITS {
        return None;
    }
    let mut names = Vec::new();
    for credit in credits {
        let Some(name) = recoverable_credit_text(&credit["name"]) else {
            continue;
        };
        if is_legacy_discogs_roleless_primary_credit(track, credit) {
            if !names
                .iter()
                .any(|known: &String| known.eq_ignore_ascii_case(name))
            {
                names.push(name.to_owned());
            }
            continue;
        }
        let Some(role_value) = credit.get("role") else {
            continue;
        };
        let Some(role) = recoverable_credit_text(role_value) else {
            continue;
        };
        let role = role
            .trim()
            .to_lowercase()
            .replace('-', " ")
            .replace('_', " ");
        let provider = credit["artist"]["provider"].as_str().unwrap_or_default();
        let id = credit["artist"]["id"].as_str().unwrap_or_default().trim();
        if (!provider.is_empty() && !provider.eq_ignore_ascii_case("discogs"))
            || (provider.eq_ignore_ascii_case("discogs")
                && !id.is_empty()
                && (id == "0" || !id.bytes().all(|byte| byte.is_ascii_digit())))
        {
            continue;
        }
        if !crate::recording_identity::is_primary_performer_role(&role) {
            continue;
        }
        if !names
            .iter()
            .any(|known: &String| known.eq_ignore_ascii_case(name))
        {
            names.push(name.to_owned());
        }
    }
    (!names.is_empty()).then(|| names.join(", "))
}
