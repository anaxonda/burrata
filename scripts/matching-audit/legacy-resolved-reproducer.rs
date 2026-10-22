#![allow(dead_code)]
#[path = "../../src-tauri/src/matching.rs"]
mod matching;
#[path = "../../src-tauri/src/models.rs"]
mod models;
#[path = "../../src-tauri/src/recording_filename.rs"]
mod recording_filename;
#[path = "../../src-tauri/src/recording_identity.rs"]
mod recording_identity;
#[path = "../../src-tauri/src/recording_match.rs"]
mod recording_match;
#[cfg(test)]
#[path = "../../src-tauri/src/recording_match_corpus.rs"]
mod recording_match_corpus;
use models::{Candidate, Track};
use recording_match::{assess, MatchLevel, MatchStage};
use serde_json::json;
fn show(name: &str, t: &Track, c: &Candidate) {
    for l in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        match std::panic::catch_unwind(|| assess(t, c, l, MatchStage::Resolved)) {
            Ok(a) => println!(
                "{name} {l:?}: {:?}/{:?} {:?}",
                a.relationship, a.action, a.dimensions
            ),
            Err(_) => println!("{name} {l:?}: PANIC"),
        }
    }
}
fn main() {
    let t = Track {
        title: "Song".into(),
        artist: "Artist".into(),
        duration_seconds: Some(200.),
        ..Default::default()
    };
    let c = Candidate {
        title: "Artist - Song".into(),
        provider: "youtube".into(),
        duration_seconds: Some(200.),
        source_metadata: Some(json!({"artists":["Artist"],"artistCount":1})),
        ..Default::default()
    };
    show("baseline", &t, &c);
    for title in [
        "Artist - Song (Live)",
        "Artist - Song (Live) (Official Audio)",
        "Artist - Song [Live]",
        "Artist - Song [432Hz]",
        "Artist - Song [Remastered 2024]",
    ] {
        show(
            title,
            &t,
            &Candidate {
                title: title.into(),
                ..c.clone()
            },
        );
    }
    let cat = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:1".into(),
        duration_seconds: None,
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"1","catalogTitle":"Song","artists":["Artist"],"artistCount":1}),
        ),
        ..c.clone()
    };
    show("missing catalog runtime", &t, &cat);
    show(
        "explicit Original Mix",
        &Track {
            version: Some("Original Mix".into()),
            ..t.clone()
        },
        &c,
    );
    show(
        "punctuation-only title",
        &Track {
            title: "!!!".into(),
            ..t.clone()
        },
        &Candidate {
            title: "???".into(),
            ..c.clone()
        },
    );
    show(
        "Unicode expansion",
        &Track {
            title: "İ feat. Å".into(),
            ..t.clone()
        },
        &c,
    );
    show(
        "nested ISRC conflict",
        &Track {
            provenance: json!({"remoteCatalog":{"isrc":"USABC2600001"}}),
            ..t.clone()
        },
        &Candidate {
            source_metadata: Some(
                json!({"artists":["Artist"],"artistCount":1,"isrc":"USABC2600002"}),
            ),
            ..c.clone()
        },
    );
    show(
        "extra complete performer",
        &t,
        &Candidate {
            source_metadata: Some(json!({"artists":["Artist","Other Singer"],"artistCount":2})),
            ..c.clone()
        },
    );
    for (name, title, meta_title) in [
        (
            "full caption metadata preserves live",
            "Artist - Song (Live) (Official Audio)",
            "Artist - Song (Live) (Official Audio)",
        ),
        (
            "clean structured title hides live caption",
            "Artist - Song (Live)",
            "Song",
        ),
        (
            "full caption metadata unknown bracket",
            "Artist - Song [432Hz]",
            "Artist - Song [432Hz]",
        ),
        (
            "full caption metadata unknown bracket slowed",
            "Artist - Song [Slowed + Reverb]",
            "Artist - Song [Slowed + Reverb]",
        ),
    ] {
        let mut cc = c.clone();
        cc.title = title.into();
        cc.source_metadata.as_mut().unwrap()["track"] = json!(meta_title);
        cc.source_metadata.as_mut().unwrap()["catalogTitle"] = json!(meta_title);
        show(name, &t, &cc);
    }
    show(
        "request title live explicit original conflict",
        &Track {
            title: "Song (Live)".into(),
            version: Some("Original Mix".into()),
            ..t.clone()
        },
        &c,
    );
    show(
        "same catalog missing both runtimes",
        &Track {
            duration_seconds: None,
            ..t.clone()
        },
        &cat,
    );
    let mut mismatch = c.clone();
    mismatch.title = "Artist - Song (Radio Edit)".into();
    mismatch.duration_seconds = Some(200.);
    let expected = Track {
        version: Some("Extended Mix".into()),
        ..t.clone()
    };
    matching::score_candidate(&expected, &mut mismatch);
    println!(
        "legacy mismatch score {} confidence {}",
        mismatch.score, mismatch.confidence
    );
    show("legacy mismatch scored candidate", &expected, &mismatch);

    let original = Track {
        version: Some("Original Mix".into()),
        ..t.clone()
    };
    let mut remix = Candidate {
        title: "Artist - Song (Alice Remix)".into(),
        provider: "monochrome".into(),
        id: "monochrome:2".into(),
        uploader: Some("Artist".into()),
        source_metadata: Some(
            json!({"provider":"monochrome","trackId":"2","catalogTitle":"Song (Alice Remix)","artists":["Artist"],"artistCount":1}),
        ),
        ..c.clone()
    };
    matching::score_candidate(&original, &mut remix);
    println!(
        "original versus remix legacy score {} confidence {}",
        remix.score, remix.confidence
    );
    show("original versus remix policy", &original, &remix);
}
