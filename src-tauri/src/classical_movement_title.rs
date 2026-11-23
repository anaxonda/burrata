use regex::Regex;
use std::sync::LazyLock;

static NUMBERED_BWV_MOVEMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\A",
        r"(?:(?<composer>[\p{L}\p{M}.]+(?: +[\p{L}\p{M}.'’\-]+)*): +)?",
        r"(?<work>[a-z]+(?: +[a-z]+){0,3}) +no(?:\. *| +)",
        r"(?<number>[1-9][0-9]{0,2}) +in +(?<note>[a-g])",
        r"(?<accidental>[#♯♭]|[- ](?:sharp|flat))?(?: +(?<mode>major|minor))?",
        r" *, *bwv +(?<catalog>[1-9][0-9]{0,3})",
        r"(?: *: *| +[-–—] +)(?<ordinal>[1-9][0-9]?|[ivx]+)\. *",
        r"(?<tempo>[^\r\n]+)\z"
    ))
    .expect("numbered BWV movement caption regex")
});

#[derive(Debug, PartialEq, Eq)]
enum Accidental {
    Natural,
    Sharp,
    Flat,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Major,
    Minor,
}

#[derive(Debug, PartialEq, Eq)]
struct NumberedBwvMovement {
    composer: Option<String>,
    work: String,
    number: u16,
    note: char,
    accidental: Accidental,
    mode: Mode,
    catalog: u16,
    ordinal: u8,
    tempo: String,
}

impl NumberedBwvMovement {
    fn parse(title: &str) -> Option<Self> {
        if title.len() > 512 || title.chars().any(char::is_control) {
            return None;
        }
        let captures = NUMBERED_BWV_MOVEMENT.captures(title.trim())?;
        let work = crate::matching::normalize(&captures["work"]);
        if !matches!(
            work.as_str(),
            "brandenburg concerto"
                | "concerto"
                | "harpsichord concerto"
                | "violin concerto"
                | "oboe concerto"
                | "orchestral suite"
                | "suite"
                | "cello suite"
                | "english suite"
                | "french suite"
                | "partita"
                | "sonata"
                | "violin sonata"
                | "trio sonata"
        ) {
            return None;
        }
        let accidental = match captures.name("accidental") {
            None => Accidental::Natural,
            Some(value) => match value
                .as_str()
                .trim_start_matches([' ', '-'])
                .to_ascii_lowercase()
                .as_str()
            {
                "sharp" | "#" | "♯" => Accidental::Sharp,
                "flat" | "♭" => Accidental::Flat,
                _ => return None,
            },
        };
        let note = captures["note"].chars().next()?;
        let mode = match captures.name("mode") {
            None if note.is_ascii_uppercase() => Mode::Major,
            Some(value) if value.as_str().eq_ignore_ascii_case("major") => Mode::Major,
            Some(value) if value.as_str().eq_ignore_ascii_case("minor") => Mode::Minor,
            _ => return None,
        };
        let ordinal = match captures["ordinal"].to_ascii_uppercase().as_str() {
            "I" | "1" => 1,
            "II" | "2" => 2,
            "III" | "3" => 3,
            "IV" | "4" => 4,
            "V" | "5" => 5,
            "VI" | "6" => 6,
            "VII" | "7" => 7,
            "VIII" | "8" => 8,
            "IX" | "9" => 9,
            "X" | "10" => 10,
            _ => return None,
        };
        let tempo = captures["tempo"].trim();
        let tempo = if let Some(inner) = tempo.strip_prefix('(') {
            inner.strip_suffix(')')?
        } else if let Some(inner) = tempo.strip_prefix('[') {
            inner.strip_suffix(']')?
        } else {
            tempo
        };
        if !tempo.chars().any(char::is_alphabetic)
            || !tempo
                .chars()
                .all(|c| c.is_alphabetic() || matches!(c, ' ' | '-'))
        {
            return None;
        }
        Some(Self {
            composer: captures
                .name("composer")
                .map(|composer| crate::matching::normalize(composer.as_str())),
            work,
            number: captures["number"].parse().ok()?,
            note: note.to_ascii_uppercase(),
            accidental,
            mode,
            catalog: captures["catalog"].parse().ok()?,
            ordinal,
            tempo: crate::matching::normalize(tempo),
        })
    }
}

/// This compares only a work and movement caption. A matching caption does
/// not verify the performers, release, version, duration, or recording.
pub(crate) fn bwv_movement_titles_equivalent(requested: &str, observed: &str) -> bool {
    NumberedBwvMovement::parse(requested)
        .zip(NumberedBwvMovement::parse(observed))
        .is_some_and(|(requested, observed)| requested == observed)
}

/// An independently bound catalog occurrence may omit the composer prefix.
pub(crate) fn bwv_catalog_movement_titles_equivalent(requested: &str, observed: &str) -> bool {
    NumberedBwvMovement::parse(requested)
        .zip(NumberedBwvMovement::parse(observed))
        .is_some_and(|(requested, mut observed)| {
            if observed.composer.is_none() {
                observed.composer.clone_from(&requested.composer);
            }
            requested == observed
        })
}

pub(crate) fn bwv_composer_credit_matches(caption: &str, credit: &str) -> bool {
    let is_bach = |name: &str| {
        matches!(
            crate::matching::normalize(name).as_str(),
            "j s bach" | "johann sebastian bach"
        )
    };
    NumberedBwvMovement::parse(caption)
        .and_then(|movement| movement.composer)
        .is_some_and(|composer| is_bach(&composer) && is_bach(credit))
}

#[cfg(test)]
mod bwv_movement_title_tests {
    use super::*;
    const REQUEST: &str = "J.S. Bach: Brandenburg Concerto No.3 in G, BWV 1048 - 1. (Allegro)";
    const OBSERVED: &str =
        "J.S. Bach: Brandenburg Concerto No. 3 in G Major, BWV 1048: I. [Allegro]";

    #[test]
    fn normalizes_only_caption_format_of_the_same_numbered_bwv_movement() {
        assert!(bwv_movement_titles_equivalent(REQUEST, OBSERVED));
        assert!(bwv_movement_titles_equivalent(OBSERVED, REQUEST));
        let without_composer = OBSERVED.strip_prefix("J.S. Bach: ").unwrap();
        assert!(!bwv_movement_titles_equivalent(REQUEST, without_composer));
        assert!(bwv_catalog_movement_titles_equivalent(
            REQUEST,
            without_composer
        ));
        assert!(bwv_composer_credit_matches(
            OBSERVED,
            "Johann Sebastian Bach"
        ));
        assert!(!bwv_composer_credit_matches(
            OBSERVED,
            "Carl Philipp Emanuel Bach"
        ));
        assert!(bwv_movement_titles_equivalent(
            "Violin Concerto No. 1 in A Minor, BWV 1041 – 2. (Andante)",
            "Violin Concerto No.1 in A minor, BWV 1041: II. Andante",
        ));
        assert!(bwv_movement_titles_equivalent(
            "Concerto No. 1 in B-flat, BWV 1051: I. Allegro",
            "Concerto No.1 in B♭ Major, BWV 1051 – 1. [Allegro]",
        ));
    }

    #[test]
    fn preserves_work_key_movement_and_performance_discriminators() {
        for (from, to) in [
            ("G Major", "G Minor"),
            ("G Major", "g"),
            ("G Major", "E Minor"),
            ("G Major", "G-sharp Major"),
            ("No. 3", "No. 4"),
            ("BWV 1048", "BWV 1049"),
            ("BWV 1048", "Op. 1048"),
            ("BWV 1048", "BWV 1048a"),
            ("Brandenburg Concerto", "Violin Concerto"),
            ("I. [Allegro]", "II. [Allegro]"),
            ("I. [Allegro]", "I. [Adagio]"),
            ("[Allegro]", "[Allegro] (Live)"),
            ("[Allegro]", "[Allegro] (2010 Remaster)"),
            ("[Allegro]", "[Allegro] – 2010"),
            ("[Allegro]", "[Allegro (arr. Smith)]"),
            ("[Allegro]", "[Allegro)"),
            ("J.S. Bach", "C.P.E. Bach"),
        ] {
            let changed = OBSERVED.replace(from, to);
            assert_ne!(changed, OBSERVED);
            assert!(
                !bwv_movement_titles_equivalent(REQUEST, &changed),
                "{changed}"
            );
        }
    }
}
