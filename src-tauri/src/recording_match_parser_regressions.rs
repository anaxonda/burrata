use crate::models::{Candidate, Track};
use crate::recording_match::{
    assess, assess_with_options, FlexibleMatchingOptions, MatchAction, MatchLevel,
    MatchRelationship, MatchStage,
};
use serde_json::json;

fn pair(request_title: &str, candidate_title: &str) -> (Track, Candidate) {
    let request = Track {
        provider: "spotify".into(),
        provider_id: "parser-regression".into(),
        artist: "Artist".into(),
        title: request_title.into(),
        duration_seconds: Some(200.0),
        ..Track::default()
    };
    let candidate = Candidate {
        provider: "monochrome".into(),
        id: "monochrome:123".into(),
        title: candidate_title.into(),
        duration_seconds: Some(200.0),
        source_metadata: Some(
            json!({"provider":"monochrome", "trackId":"123", "catalogTitle":candidate_title, "artistNames":["Artist"], "performerCreditsComplete":true, "duration":200.0}),
        ),
        ..Candidate::default()
    };
    (request, candidate)
}
fn assert_same_recording(request: &Track, candidate: &Candidate) {
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        let search = assess(request, candidate, level, MatchStage::Search);
        assert!(
            matches!(search.action, MatchAction::Inspect | MatchAction::Accept),
            "{level:?} {request:?} {candidate:?} {search:?}"
        );
        let resolved = assess(request, candidate, level, MatchStage::Resolved);
        assert_eq!(
            resolved.action,
            MatchAction::Accept,
            "{level:?} {request:?} {candidate:?} {resolved:?}"
        );
    }
}

fn traxsource_mix(title: &str, version: &str, duration: f64, credits: &[&str]) -> Track {
    Track {
        provider: "traxsource".into(),
        title: title.into(),
        version: Some(version.into()),
        artist: credits.join(", "),
        duration_seconds: Some(duration),
        provenance: json!({
            "sourceKind": "chart",
            "artistCredits": credits.iter().map(|name| json!({
                "name": name, "role": "artist", "provenance": "traxsourceRenderedArtistLink"
            })).collect::<Vec<_>>()
        }),
        ..Track::default()
    }
}

fn youtube_mix(id: &str, title: &str, duration: f64, stage: MatchStage) -> Candidate {
    let origin = if stage == MatchStage::Search {
        "ytDlpCandidateMetadata"
    } else {
        "ytDlpFullInfo"
    };
    Candidate {
        provider: "youtube".into(),
        id: format!("youtube:{id}"),
        url: format!("https://www.youtube.com/watch?v={id}"),
        title: title.into(),
        duration_seconds: Some(duration),
        source_metadata: Some(json!({
            "sourceMetadataOrigin": origin, "resolvedProviderId": id,
            "track": title, "catalogTitle": title, "duration": duration
        })),
        ..Candidate::default()
    }
}

#[test]
fn traxsource_extended_mix_can_credit_the_remixer_in_the_version() {
    let request = traxsource_mix(
        "Samba Magic",
        "Reboot Extended Mix (6:11)",
        371.0,
        &["Basement Jaxx", "Reboot"],
    );
    let assess_caption = |title, seconds, stage| {
        assess(
            &request,
            &youtube_mix("bbccFSenC9c", title, seconds, stage),
            MatchLevel::Flexible,
            stage,
        )
    };
    let caption = "Basement Jaxx - Samba Magic (Reboot Extended Mix)";
    assert!(matches!(
        assess_caption(caption, 371.0, MatchStage::Search).action,
        MatchAction::Inspect | MatchAction::Accept
    ));
    assert_eq!(
        assess_caption(caption, 371.0, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    for (wrong, seconds) in [
        ("Other Artist - Samba Magic (Reboot Extended Mix)", 371.0),
        (
            "Basement Jaxx - Samba Magic (Other Remixer Extended Mix)",
            371.0,
        ),
        ("Basement Jaxx - Samba Magic (Reboot Remix)", 193.0),
    ] {
        assert_ne!(
            assess_caption(wrong, seconds, MatchStage::Resolved).action,
            MatchAction::Accept
        );
    }
}

#[test]
fn sambei_credit_correction_is_scoped_to_the_catalog_occurrence() {
    let mut request = traxsource_mix(
        "Sambei",
        "Extended Mix (4:58)",
        298.0,
        &["Eran Hersh", "Aycond", "NOTHANKYOUU"],
    );
    request.provider_id = "15140553".into();
    request.source_url = "https://www.traxsource.com/track/15140553/sambei-extended-mix".into();
    let caption = "Eran Hersh, Aycond, NOTHANKYOU - Sambei (Extended Mix)";
    let candidate = |stage| youtube_mix("lUGIZE0oX2U", caption, 298.0, stage);
    let assess_source =
        |request: &Track, stage| assess(request, &candidate(stage), MatchLevel::Flexible, stage);
    assert!(matches!(
        assess_source(&request, MatchStage::Search).action,
        MatchAction::Inspect | MatchAction::Accept
    ));
    assert_eq!(
        assess_source(&request, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    let mut edited = request.clone();
    edited.provenance["manualFields"] = json!({"artistCredits": true});
    assert_ne!(
        assess_source(&edited, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    request.provider_id = "another-release".into();
    assert_ne!(
        assess_source(&request, MatchStage::Resolved).action,
        MatchAction::Accept
    );
}

#[test]
fn promotional_caption_tags_do_not_hide_the_extended_mix() {
    let request = traxsource_mix(
        "Badabum Cha Cha",
        "Extended Mix (5:02)",
        302.0,
        &["Ilkay Sencan", "ARVIII"],
    );
    let assess_caption = |title, seconds, stage| {
        assess(
            &request,
            &youtube_mix("qZ8isyaBxvg", title, seconds, stage),
            MatchLevel::Flexible,
            stage,
        )
    };
    let caption =
        "Ilkay Sencan, ARVIII - Badabum Cha Cha (Extended Mix) - #afrohouse #thesacredtribe";
    assert!(matches!(
        assess_caption(caption, 302.0, MatchStage::Search).action,
        MatchAction::Inspect | MatchAction::Accept
    ));
    assert_eq!(
        assess_caption(caption, 302.0, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    for (wrong, seconds) in [
        (
            "Ilkay Sencan, ARVIII - Badabum Cha Cha (Radio Edit) - #afrohouse #thesacredtribe",
            302.0,
        ),
        (
            "Ilkay Sencan, ARVIII - Badabum Cha Cha (Extended Mix) - #afrohouse #radioedit",
            302.0,
        ),
        (caption, 185.0),
    ] {
        assert_eq!(
            assess_caption(wrong, seconds, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
}

#[test]
fn traxsource_runtime_only_label_accepts_a_matching_generic_extension_as_probable() {
    let request = traxsource_mix("Foreva", "(5:28)", 328.0, &["DJ Chus", "SIIRO (ES)"]);
    let title = "DJ Chus, SIIRO (ES) - Foreva (Extended Mix) [STEREO PRODUCTIONS]";
    let mut request = request;
    request.label = Some("Stereo Productions".into());
    let candidate = |title, duration, stage| youtube_mix("aTxIS5PivZE", title, duration, stage);
    let search = assess(
        &request,
        &candidate(title, 328.0, MatchStage::Search),
        MatchLevel::Flexible,
        MatchStage::Search,
    );
    assert_eq!(search.action, MatchAction::Inspect, "{search:?}");
    let resolved = assess(
        &request,
        &candidate(title, 328.0, MatchStage::Resolved),
        MatchLevel::Flexible,
        MatchStage::Resolved,
    );
    assert_eq!(
        (resolved.action, resolved.relationship),
        (MatchAction::Accept, MatchRelationship::Probable),
        "{resolved:?}"
    );
    let full = candidate(title, 328.0, MatchStage::Resolved);
    let balanced = assess(&request, &full, MatchLevel::Balanced, MatchStage::Resolved);
    assert_eq!(
        (balanced.action, balanced.relationship),
        (MatchAction::Accept, MatchRelationship::Probable)
    );
    assert_ne!(
        assess(&request, &full, MatchLevel::Exact, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    let mut identified = request.clone();
    identified.isrc = Some("USAAA2600001".into());
    let mut bound = full;
    bound.source_metadata.as_mut().unwrap()["isrc"] = json!(identified.isrc);
    let exact = assess(&identified, &bound, MatchLevel::Exact, MatchStage::Resolved);
    assert_eq!(
        (exact.action, exact.relationship),
        (MatchAction::Accept, MatchRelationship::Exact),
        "{exact:?}"
    );
    let close = candidate(title, 326.5, MatchStage::Resolved);
    let mut options = FlexibleMatchingOptions {
        allow_different_lengths: true,
        max_shorter_by_seconds: 2,
        ..Default::default()
    };
    let assess_close = |options: &FlexibleMatchingOptions| {
        assess_with_options(
            &request,
            &close,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            options,
        )
    };
    assert_eq!(assess_close(&options).action, MatchAction::Accept);
    options.max_shorter_by_seconds = 1;
    assert_eq!(assess_close(&options).action, MatchAction::Reject);
    for (caption, duration) in [
        ("DJ Chus, Someone Else - Foreva (Extended Mix)", 328.0),
        ("DJ Chus, SIIRO (ES) - Foreva (Other Remixer Remix)", 328.0),
        (title, 200.0),
    ] {
        let rejected = assess(
            &request,
            &candidate(caption, duration, MatchStage::Resolved),
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(rejected.action, MatchAction::Reject, "{rejected:?}");
    }
}

#[test]
fn traxsource_runtime_only_tracks_accept_matching_structured_extended_mix_credits() {
    for (title, version, seconds, id, credits) in [
        (
            "Cleo's Groove",
            "(6:10)",
            370.0,
            "-lxYKvuh1kM",
            &["Craig Leo"][..],
        ),
        (
            "Bem-te-vi",
            "(6:44)",
            404.0,
            "2200000000",
            &["BAKKA (BR)", "Nezello", "Jéssica Gaspar"][..],
        ),
    ] {
        let request = traxsource_mix(title, version, seconds, credits);
        let caption = format!("{title} (Extended Mix)");
        let mut candidate = youtube_mix(id, &caption, seconds, MatchStage::Resolved);
        if title == "Bem-te-vi" {
            candidate.provider = "soundcloud".into();
            candidate.id = format!("soundcloud:{id}");
            candidate.url = "https://soundcloud.com/dawnpatrolart/bem-te-vi".into();
            candidate.source_metadata.as_mut().unwrap()["artists"] = json!([credits.join(", ")]);
        } else {
            candidate.source_metadata.as_mut().unwrap()["artists"] = json!(credits);
        }
        let resolved = assess(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
        );
        assert_eq!(
            (resolved.action, resolved.relationship),
            (MatchAction::Accept, MatchRelationship::Probable),
            "{title}: {resolved:?}"
        );
    }
}

#[test]
fn traxsource_accent_variant_requires_the_same_structured_collaboration() {
    let request = traxsource_mix(
        "Celia Con Cuero",
        "(7:54)",
        474.0,
        &["Max Sanchez", "LemonSouldj"],
    );
    let candidate = |names: &[&str], seconds, stage| {
        let mut candidate = youtube_mix("GwSmOvmW2i4", "Celia Con Cuero", seconds, stage);
        candidate.source_metadata.as_mut().unwrap()["artists"] = json!(names);
        candidate
    };
    let credits = ["Max Sánchez", "LemonSoulDJ"];
    let search = assess(
        &request,
        &candidate(&credits, 474.0, MatchStage::Search),
        MatchLevel::Flexible,
        MatchStage::Search,
    );
    assert_eq!(search.action, MatchAction::Inspect, "{search:?}");
    let resolved = assess(
        &request,
        &candidate(&credits, 474.0, MatchStage::Resolved),
        MatchLevel::Flexible,
        MatchStage::Resolved,
    );
    assert_eq!(resolved.action, MatchAction::Accept, "{resolved:?}");
    let full = candidate(&credits, 474.0, MatchStage::Resolved);
    let balanced = assess(&request, &full, MatchLevel::Balanced, MatchStage::Resolved);
    assert_eq!(balanced.action, MatchAction::Accept, "{balanced:?}");
    assert_ne!(
        assess(&request, &full, MatchLevel::Exact, MatchStage::Resolved).action,
        MatchAction::Accept
    );
    let mut identified = request.clone();
    identified.isrc = Some("USAAA2600002".into());
    let mut bound = full;
    bound.source_metadata.as_mut().unwrap()["isrc"] = json!(identified.isrc);
    let exact = assess(&identified, &bound, MatchLevel::Exact, MatchStage::Resolved);
    assert_eq!(
        (exact.action, exact.relationship),
        (MatchAction::Accept, MatchRelationship::Exact),
        "{exact:?}"
    );
    for (names, duration) in [
        (&["Max Sánchez"][..], 474.0),
        (&["Max Sánchez", "Other Artist"][..], 474.0),
        (&["Max Sánchez", "LemonSoulDJ", "Other Artist"][..], 474.0),
        (&credits[..], 440.0),
    ] {
        assert_ne!(
            assess(
                &request,
                &candidate(names, duration, MatchStage::Resolved),
                MatchLevel::Flexible,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }
}

#[test]
fn resolved_matching_preserves_distinct_combining_marks() {
    for (left, right) in [
        ("ป่า", "ป้า"),
        ("ປ່າ", "ປ້າ"),
        ("राज", "राज़"),
        ("கல", "கல்"),
        ("కల", "కల్"),
        ("കല", "കല്"),
    ] {
        let (request, candidate) = pair(left, right);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_eq!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Reject,
                "{left} vs {right}"
            );
        }
        let (request, candidate) = pair(left, left);
        assert_same_recording(&request, &candidate);
    }
    let (mut request, mut candidate) = pair("แสง", "แสง");
    request.artist = "ป่า".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["ป้า"]);
    assert_eq!(
        assess(
            &request,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Reject
    );
}

#[test]
fn localized_versions_preserve_performance_and_mastering_evidence() {
    for (request, candidate) in [
        ("Nada (versión acústica)", "Nada (Acoustic)"),
        ("Ecos (En directo)", "Ecos (Live)"),
        ("Song - Ao Vivo", "Song (Live)"),
        (
            "Song (Ao Vivo no Noites Cariocas)",
            "Song (Live at Noites Cariocas)",
        ),
        ("Song (Remasterisé 2011)", "Song (Remastered 2011)"),
        ("光【ライブ】", "光 (Live)"),
        ("빛 (라이브 버전)", "빛 (Live)"),
        ("光（现场版）", "光 (Live)"),
    ] {
        let (request, candidate) = pair(request, candidate);
        assert_same_recording(&request, &candidate);
    }
    for (request, candidate) in [
        ("Song (En Vivo en Madrid)", "Song (Live in Barcelona)"),
        ("光【Live】", "光"),
        ("Luz (con amor)", "Luz"),
    ] {
        let (request, candidate) = pair(request, candidate);
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }
}

#[test]
fn localized_presentation_and_credits_keep_structured_artist_boundaries() {
    for title in ["光【官方MV】", "光 (Vídeo Oficial)", "光 (Clip officiel)"] {
        let (request, candidate) = pair("光", title);
        assert_same_recording(&request, &candidate);
    }
    let (request, mut candidate) = pair("光（ｆｅａｔ． 山田）", "光 (feat. 山田)");
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Artist", "山田"]);
    assert_same_recording(&request, &candidate);

    for separator in ['、', '，', '،'] {
        let (mut request, mut candidate) = pair("光", "光");
        let whole_name = format!("周杰倫{separator}蔡依林");
        request.artist = whole_name.clone();
        let metadata = candidate.source_metadata.as_mut().unwrap();
        metadata["artistNames"] = json!(["周杰倫", "蔡依林"]);
        assert_same_recording(&request, &candidate);

        request.provenance = json!({"artistCredits":[{"name":whole_name}]});
        assert_eq!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Reject
        );
        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!([whole_name]);
        assert_same_recording(&request, &candidate);
    }
}

#[test]
fn fullwidth_caption_separator_still_requires_requested_artist_and_title() {
    let (request, _) = pair("光", "光");
    let mut candidate = Candidate {
        provider: "youtube".into(),
        id: "youtube:abcdefghijk".into(),
        uploader: Some("Artist".into()),
        duration_seconds: Some(200.0),
        source_metadata: Some(json!({
            "provider":"youtube", "resolvedProviderId":"abcdefghijk",
            "sourceMetadataOrigin":"ytDlpFullInfo", "channel":"Artist", "duration":200.0
        })),
        ..Candidate::default()
    };
    for separator in ['－', '｜', '：'] {
        candidate.title = format!("Artist{separator}光");
        assert_same_recording(&request, &candidate);
        candidate.title = format!("Other artist{separator}光");
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }
}

#[test]
fn square_brackets_share_balanced_trimmed_delimiters() {
    for label in [
        "Live",
        "Radio Edit",
        "DJ Named Remix",
        "Remastered 2009",
        "feat. Guest",
    ] {
        let plain = crate::recording_identity::parse_recording_text(&format!("Song ({label})"));
        let padded =
            crate::recording_identity::parse_recording_text(&format!("Song [  {label}  ]"));
        assert_eq!(plain.base_title, padded.base_title);
        assert_eq!(plain.annotations[0].kind, padded.annotations[0].kind);
        assert_eq!(plain.annotations[0].value, padded.annotations[0].value);
        let span = &padded.annotations[0].content_span;
        assert_eq!(&padded.original[span.start..span.end], label);
    }
    for malformed in ["Song [[Live]", "Song ] [Live]", "Song [Live", "[Live]"] {
        assert!(
            crate::matching::split_balanced_trailing_square_group(malformed).is_none(),
            "{malformed}"
        );
    }
    let (request, candidate) = pair("Song [ Radio Edit ]", "Song (Radio Edit)");
    assert_same_recording(&request, &candidate);
}
#[test]
fn mastering_and_recording_annotations_are_composable_and_order_independent() {
    for recording in ["Live at Wembley", "Acoustic", "DJ Named Remix"] {
        let reference = format!("Song ({recording}) (Remastered 2009)");
        let labels = [recording, "Remastered 2009", "Official Audio"];
        for i in 0..3 {
            for j in 0..3 {
                if i == j {
                    continue;
                }
                let k = 3 - i - j;
                for (open, close) in [("(", ")"), ("[ ", " ]")] {
                    let rendered = format!(
                        "Song {open}{}{close} {open}{}{close} {open}{}{close}",
                        labels[i], labels[j], labels[k]
                    );
                    let (request, candidate) = pair(&reference, &rendered);
                    assert_same_recording(&request, &candidate);
                }
            }
        }
        let (mut request, candidate) = pair(&format!("Song ({recording})"), &reference);
        request.version = Some("Remastered 2009".into());
        assert_same_recording(&request, &candidate);
    }
}
#[test]
fn neutral_labels_and_guest_suffix_order_do_not_conflict_with_mastering() {
    for neutral in ["Original Mix", "Album Version"] {
        let (request, candidate) = pair(
            &format!("Song ({neutral}) (Remastered 2009)"),
            "Song (Remastered 2009)",
        );
        assert_same_recording(&request, &candidate);
    }
    let (mut request, mut candidate) = pair(
        "Song (feat. Guest) (Remastered 2009)",
        "Song (Remastered 2009) (feat. Guest)",
    );
    request.artist = "Artist feat. Guest".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Artist", "Guest"]);
    assert_same_recording(&request, &candidate);
}
#[test]
fn structural_normalization_preserves_wrong_recording_and_unknown_title_negatives() {
    for (requested, observed) in [
        (
            "Song (Live at Wembley) (Remastered 2009)",
            "Song (Live at Madrid) (Remastered 2009)",
        ),
        (
            "Song (DJ Alpha Remix) (Remastered 2009)",
            "Song (DJ Beta Remix) (Remastered 2009)",
        ),
        (
            "Song (Live) (Remastered 2009)",
            "Other Song (Live) (Remastered 2009)",
        ),
        ("Song (At Last) (Live)", "Song (Other Words) (Live)"),
    ] {
        let (request, candidate) = pair(requested, observed);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_ne!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{requested} / {observed}"
            );
        }
    }
    let (request, mut candidate) = pair(
        "Song (Live) (Remastered 2009)",
        "Song (Remastered 2009) (Live)",
    );
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Other Artist"]);
    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        assert_eq!(
            assess(&request, &candidate, level, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    for conflicting in [
        "Song (Live at Wembley) (Live at Madrid)",
        "Song (Remastered 2009) (Remastered 2019)",
    ] {
        let (request, candidate) = pair(conflicting, conflicting);
        let options = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options
            )
            .action,
            MatchAction::Accept,
            "{conflicting}"
        );
    }
}

#[test]
fn structured_version_labels_match_title_labels_and_respect_mix_permission() {
    for label in [
        "Extended Version",
        "Dub",
        "DJ Named Dub",
        "A Cappella",
        "Mono",
        "Stereo",
        "Sped Up",
        "Slowed + Reverb",
    ] {
        let (mut request, candidate) = pair("Song", &format!("Song ({label})"));
        request.version = Some(label.into());
        assert_same_recording(&request, &candidate);
        let (request, candidate) = pair(&format!("Song - {label}"), &format!("Song ({label})"));
        assert_same_recording(&request, &candidate);
        let (request, candidate) = pair("Song", &format!("Song ({label})"));
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_ne!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{label}: {level:?}"
            );
        }
        let mixes = FlexibleMatchingOptions {
            allow_other_releases: false,
            allow_other_mixes: true,
            ..Default::default()
        };
        let accepted = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &mixes,
        );
        assert_eq!(
            accepted.action,
            MatchAction::Accept,
            "{label}: {accepted:?}"
        );
        assert_eq!(
            accepted.alternative_kind,
            Some(crate::recording_match::AlternativeKind::Remix)
        );
        let performance_only = FlexibleMatchingOptions {
            allow_other_releases: false,
            allow_other_performances: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &performance_only
            )
            .action,
            MatchAction::Accept
        );
    }
    let (mut request, candidate) = pair("Song", "Song (A Cappella)");
    request.version = Some("Acapella".into());
    assert_same_recording(&request, &candidate);
    for unknown in ["Stereo Love", "A Cappella Dreams", "Mystery Label"] {
        let (request, candidate) = pair("Song", &format!("Song ({unknown})"));
        let all = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &all
            )
            .action,
            MatchAction::Accept,
            "{unknown}"
        );
    }
}
#[test]
fn flexible_options_apply_only_to_changed_recording_dimensions() {
    let (request, candidate) = pair(
        "Song (Live at Wembley) (Remastered 2009)",
        "Song (Remastered 2019) (Live at Wembley)",
    );
    for level in [MatchLevel::Exact, MatchLevel::Balanced] {
        assert_eq!(
            assess(&request, &candidate, level, MatchStage::Resolved).action,
            MatchAction::Reject
        );
    }
    let release_only = FlexibleMatchingOptions::default();
    let release = assess_with_options(
        &request,
        &candidate,
        MatchLevel::Flexible,
        MatchStage::Resolved,
        &release_only,
    );
    assert_eq!(release.action, MatchAction::Accept, "{release:?}");
    assert_eq!(
        release.alternative_kind,
        Some(crate::recording_match::AlternativeKind::Release)
    );
    assert_eq!(
        crate::recording_match::alternative_rank(&release, &request, &candidate),
        1
    );
    let (_, different_venue) = pair("Song", "Song (Remastered 2019) (Live at Madrid)");
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &release_only
        )
        .action,
        MatchAction::Reject
    );
    let performance_only = FlexibleMatchingOptions {
        allow_other_releases: false,
        allow_other_performances: true,
        ..Default::default()
    };
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &performance_only
        )
        .action,
        MatchAction::Reject
    );
    let both = FlexibleMatchingOptions {
        allow_other_performances: true,
        ..Default::default()
    };
    assert_eq!(
        assess_with_options(
            &request,
            &different_venue,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &both
        )
        .action,
        MatchAction::Accept
    );
}

#[test]
fn search_budget_preserves_verified_title_and_artist_alternatives() {
    use crate::search_plan::{SearchPlan, SearchQueryReason};
    let (mut track, _) = pair("Canonical Title", "Canonical Title");
    let original = track.clone();
    let plan = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert_eq!(plan.queries.len(), 2);
    assert_eq!(plan.queries[0].text, "Artist Canonical Title");
    assert_eq!(plan.queries[1].text, "Verified Artist Verified Title");
    assert_eq!(
        plan.queries[1].reason,
        SearchQueryReason::VerifiedTitleAlternative
    );
    assert_eq!(track.title, original.title);
    assert_eq!(track.artist, original.artist);
    track.provenance = json!({"manualFields":{"artist":true}});
    let manual_artist = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert_eq!(manual_artist.queries[1].text, "Artist Verified Title");
    track.provenance = json!({"manualFields":{"title":true}});
    let manual_title = SearchPlan::for_track_with_alternatives(
        &track,
        &["Verified Artist".into()],
        &["Verified Title".into()],
    );
    assert!(manual_title
        .queries
        .iter()
        .all(|query| query.title == "Canonical Title"));
    let equivalent = SearchPlan::for_track_with_alternatives(
        &original,
        &["Verified Artist".into()],
        &["Canonical  Title".into()],
    );
    assert_eq!(
        equivalent.queries[1].reason,
        SearchQueryReason::VerifiedArtistAlternative
    );
}
#[test]
fn retrieval_uses_shared_version_keys_and_preserves_performance_context() {
    use crate::search_plan::SearchPlan;
    for (title, version) in [
        ("Song (Radio Edit) (Official Audio)", "Radio Edit"),
        ("Song [ Radio Edit ]", "Radio Edit (3:20)"),
        ("Song (2009 Digital Remaster)", "Remastered 2009"),
        ("Song (A Cappella)", "Acapella"),
        ("Song (Radio Edit) (Official Audio)", "Song Radio Edit"),
    ] {
        let (mut track, _) = pair(title, title);
        track.version = Some(version.into());
        let plan = SearchPlan::for_track(&track, &[]);
        assert_eq!(plan.queries[0].title, title, "{title} / {version}");
    }
    let (track, _) = pair("Song (Live at Remaster Hall)", "Song");
    let unchanged = SearchPlan::for_track_with_release_policy(&track, &[], true);
    assert!(unchanged
        .queries
        .iter()
        .all(|query| query.title.contains("Live at Remaster Hall")));
    let (track, _) = pair("Song (Live at Wembley) (Remastered 2009)", "Song");
    let release = SearchPlan::for_track_with_release_policy(&track, &[], true);
    assert_eq!(release.queries.len(), 2);
    assert_eq!(release.queries[1].title, "Song (Live at Wembley)");
    assert_eq!(track.title, "Song (Live at Wembley) (Remastered 2009)");
}

#[test]
fn original_labels_still_conflict_with_explicit_performance_or_mix() {
    for version in ["Live", "DJ Named Remix", "Dub"] {
        let (mut request, mut candidate) =
            pair(&format!("Song ({version})"), &format!("Song ({version})"));
        request.version = Some("Original Mix".into());
        candidate.source_metadata.as_mut().unwrap()["version"] = json!("Original Mix");
        let all = FlexibleMatchingOptions {
            allow_other_releases: true,
            allow_other_performances: true,
            allow_other_mixes: true,
            allow_other_artists: true,
            ..Default::default()
        };
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let result =
                assess_with_options(&request, &candidate, level, MatchStage::Resolved, &all);
            assert_ne!(result.action, MatchAction::Accept, "{version}: {result:?}");
            assert!(result
                .reasons
                .contains(&crate::recording_match::MatchReason::ConflictingRequestVersions));
        }
    }
}

#[test]
fn bare_extended_is_a_recording_version_not_an_unresolved_annotation() {
    let (mut request, candidate) = pair("Song (Extended)", "Song (Extended Mix)");
    request.version = Some("Extended".into());
    assert_same_recording(&request, &candidate);
    for wrong in ["Song (Radio Edit)", "Song (Live)", "Song (DJ Remix)"] {
        let (_, candidate) = pair("Song", wrong);
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );
    }
    let parsed = crate::recording_identity::parse_recording_text("Song (Extended)");
    assert_eq!(parsed.base_title, "Song");
    assert_eq!(
        parsed.semantic_versions().collect::<Vec<_>>(),
        vec!["Extended"]
    );
}

#[test]
fn a_duplicate_feature_in_mix_field_does_not_create_a_requested_edition() {
    assert_eq!(
        crate::recording_match::semantic_version("Say What (feat. Chuala)", "feat. Chuala"),
        ""
    );
    assert!(!crate::recording_match::semantic_version(
        "Say What (feat. Chuala)",
        "feat. Other Guest"
    )
    .is_empty());
    assert!(!crate::recording_match::semantic_version("Say What", "feat. Chuala").is_empty());
    assert!(
        !crate::recording_match::semantic_version("Say What (feat. Chuala)", "DJ Remix").is_empty()
    );
    let (mut request, mut candidate) = pair("Say What (feat. Chuala)", "Say What (feat. Chuala)");
    request.version = Some("feat. Chuala".into());
    request.artist = "Keinemusik".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Keinemusik", "Chuala"]);
    // Guest matching is evaluated independently of the empty semantic edition.
    assert_eq!(
        assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept
    );
}

#[test]
fn spaced_artist_title_caption_is_inspectable_without_proving_missing_guest_credits() {
    let request = Track {
        provider: "spotify".into(),
        artist: "Karamelo Santo, Manu Chao".into(),
        title: "La Picadura".into(),
        duration_seconds: Some(214.146),
        ..Track::default()
    };
    let candidate = Candidate {
        provider: "youtube".into(),
        id: "youtube:lY6AsjXwNqQ".into(),
        title: "Karamelo Santo   La Picadura".into(),
        duration_seconds: Some(214.),
        ..Candidate::default()
    };
    let search = assess(
        &request,
        &candidate,
        MatchLevel::Balanced,
        MatchStage::Search,
    );
    assert_eq!(
        search.dimensions.title,
        crate::recording_match::Comparison::Agree
    );
    assert_eq!(search.action, MatchAction::Inspect);
    assert_ne!(
        assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept
    );
    let wrong = Candidate {
        title: "Karamelo Santo   Different Song".into(),
        ..candidate.clone()
    };
    assert_eq!(
        assess(&request, &wrong, MatchLevel::Balanced, MatchStage::Search).action,
        MatchAction::Reject
    );
}

#[test]
fn quoted_artist_title_captions_preserve_mode_and_length_permissions() {
    let request = Track {
        provider: "spotify".into(),
        artist: "Artist".into(),
        title: "Song (Versión Video)".into(),
        duration_seconds: Some(200.),
        ..Default::default()
    };
    let options = FlexibleMatchingOptions {
        allow_other_releases: true,
        allow_other_performances: true,
        allow_other_mixes: true,
        allow_other_artists: false,
        allow_different_lengths: true,
        max_shorter_by_seconds: 30,
        max_longer_by_seconds: 30,
        ..Default::default()
    };
    for title in [
        "Artist \"Song\"",
        "Artist “Song”",
        "Artist 'Song'",
        "Artist ‘Song’",
        "Artist「Song」",
        "Artist『Song』",
    ] {
        let candidate = Candidate {
            provider: "youtube".into(),
            id: "youtube:abcdefghijk".into(),
            title: title.into(),
            duration_seconds: Some(180.),
            source_metadata: Some(
                json!({"provider":"youtube","resolvedProviderId":"abcdefghijk",
                "sourceMetadataOrigin":"ytDlpFullInfo","duration":180.0,"channel":"Uploader"}),
            ),
            ..Default::default()
        };
        let flexible = assess_with_options(
            &request,
            &candidate,
            MatchLevel::Flexible,
            MatchStage::Resolved,
            &options,
        );
        assert_eq!(
            flexible.dimensions.title,
            crate::recording_match::Comparison::Agree,
            "{title}: {flexible:?}"
        );
        assert_eq!(
            flexible.dimensions.artists,
            crate::recording_match::Comparison::Agree,
            "{title}: {flexible:?}"
        );
        assert_eq!(
            flexible.action,
            MatchAction::Accept,
            "{title}: {flexible:?}"
        );
        for mode in [MatchLevel::Exact, MatchLevel::Balanced] {
            assert_ne!(
                assess_with_options(&request, &candidate, mode, MatchStage::Resolved, &options)
                    .action,
                MatchAction::Accept
            );
        }
        let mut prohibited = options;
        prohibited.allow_other_mixes = false;
        assert_ne!(
            assess_with_options(
                &request,
                &candidate,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &prohibited
            )
            .action,
            MatchAction::Accept
        );
        for bad in [
            "Other Artist \"Song\"",
            "Artist \"Different Song\"",
            "Artist \"Song",
            "Artist「Song",
            "Artist『Song」",
            "Other Artist「Song」",
            "Artist \"Song \"Remix\"\"",
        ] {
            let mut wrong = candidate.clone();
            wrong.title = bad.into();
            assert_ne!(
                assess_with_options(
                    &request,
                    &wrong,
                    MatchLevel::Flexible,
                    MatchStage::Resolved,
                    &options
                )
                .action,
                MatchAction::Accept,
                "{bad}"
            );
        }
        let plain = Track {
            title: "Song".into(),
            ..request.clone()
        };
        let mut live = candidate.clone();
        live.title = "Artist \"Song (Live)\"".into();
        live.duration_seconds = Some(200.);
        live.source_metadata.as_mut().unwrap()["duration"] = json!(200.);
        assert_ne!(
            assess(&plain, &live, MatchLevel::Balanced, MatchStage::Resolved).action,
            MatchAction::Accept,
            "Quoted presentation must not hide the live edition"
        );
        let mut too_short = candidate.clone();
        too_short.duration_seconds = Some(150.);
        too_short.source_metadata.as_mut().unwrap()["duration"] = json!(150.);
        assert_ne!(
            assess_with_options(
                &request,
                &too_short,
                MatchLevel::Flexible,
                MatchStage::Resolved,
                &options
            )
            .action,
            MatchAction::Accept
        );
    }
}

#[test]
fn compatible_edition_labels_preserve_each_recording_dimension() {
    for (requested, observed) in [
        ("Signal (Live) (Acoustic)", "Signal (Acoustic) (Live)"),
        (
            "Signal (Live) (Instrumental)",
            "Signal (Live) (Instrumental)",
        ),
        (
            "Signal (Ava Remix) (Radio Edit)",
            "Signal (Ava Remix) (Radio Edit)",
        ),
        (
            "Signal (Live) (Acoustic) (2015 Remaster)",
            "Signal (Live) (Acoustic) (2015 Remaster)",
        ),
    ] {
        let (request, candidate) = pair(requested, observed);
        assert_same_recording(&request, &candidate);
    }
    for (requested, observed) in [
        (
            "Signal (Live at the Ritz) (Acoustic)",
            "Signal (Live at the Opera) (Acoustic)",
        ),
        (
            "Signal (Live at the Ritz) (Acoustic)",
            "Signal (Live at the Ritz)",
        ),
        (
            "Signal (Ava Remix) (Radio Edit)",
            "Signal (Eva Remix) (Radio Edit)",
        ),
        (
            "Signal (Ava Remix) (Eva Remix)",
            "Signal (Ava Remix) (Eva Remix)",
        ),
    ] {
        let (request, candidate) = pair(requested, observed);
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "{requested:?} vs {observed:?}",
        );
    }
}

#[test]
fn remix_name_corroborates_only_the_named_remixer_credit() {
    let (mut request, mut candidate) = pair("Signal (Ava Remix)", "Signal (Ava Remix)");
    request.artist = "Lead, Ava".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Lead"]);
    assert_same_recording(&request, &candidate);

    for credits in ["Lead, Ava, Bea", "Lead, Bea, Ava", "Lead, Av"] {
        request.artist = credits.into();
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept,
            "{credits}",
        );
    }

    request.artist = "Lead, Ava, Bea".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Lead", "Bea"]);
    assert_same_recording(&request, &candidate);
}

#[test]
fn collaboration_join_respects_structured_or_atomic_performer_names() {
    for join in [" x ", " × ", " And "] {
        let (mut request, mut candidate) = pair("Signal", "Signal");
        request.artist = format!("Aya{join}Bea");
        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Aya", "Bea"]);
        assert_same_recording(&request, &candidate);

        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["Aya", "Bia"]);
        assert_ne!(
            assess(
                &request,
                &candidate,
                MatchLevel::Exact,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Accept
        );

        candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!([request.artist]);
        assert_same_recording(&request, &candidate);
    }
}

#[test]
fn catalog_retrieval_uses_the_verifiers_title_comparison() {
    use crate::recording_match::same_retrieval_title;
    for (requested, observed) in [
        (
            "Don't Think Twice, It's All Right",
            "Dont Think Twice, Its All Right",
        ),
        ("光", "光【官方MV】"),
        ("Lumière", "Lumière (Clip officiel)"),
        ("Signal", "Signal Official Audio"),
        ("Signal", "Signal (Official Audio)"),
    ] {
        assert!(
            same_retrieval_title(requested, observed),
            "{requested:?} vs {observed:?}"
        );
    }
    assert!(!same_retrieval_title("Signal", "Different Song"));
    assert!(!same_retrieval_title("", ""));
    assert!(same_retrieval_title("Signal (Live)", "Signal (Acoustic)"));
}

#[test]
fn abbey_road_2019_mix_matches_label_upload_and_rejects_other_editions() {
    let (mut request, mut candidate) =
        pair("Golden Slumbers - 2019 Mix", "Golden Slumbers (2019 Mix)");
    request.provider_id = "4FWAzXcVNVIRVA4UQxhvbd".into();
    request.artist = "The Beatles".into();
    request.album = Some("Abbey Road (Super Deluxe Edition)".into());
    request.duration_seconds = Some(91.44);
    candidate.id = "youtube:hwi2ynHPB80".into();
    candidate.provider = "youtube".into();
    candidate.duration_seconds = Some(92.0);
    candidate.source_metadata = Some(json!({
        "provider": "youtube", "resolvedProviderId": "hwi2ynHPB80",
        "sourceMetadataOrigin": "ytDlpFullInfo",
        "track": "Golden Slumbers (2019 Mix)", "artist": "The Beatles",
        "artists": ["The Beatles"], "performerCreditsComplete": true, "duration": 92
    }));
    assert_same_recording(&request, &candidate);
    for other in [
        "Golden Slumbers (2009 Remaster)",
        "Golden Slumbers / Carry That Weight (Takes 1-3)",
    ] {
        candidate.title = other.into();
        candidate.source_metadata.as_mut().unwrap()["track"] = json!(other);
        assert_eq!(
            assess(
                &request,
                &candidate,
                MatchLevel::Balanced,
                MatchStage::Resolved
            )
            .action,
            MatchAction::Reject,
            "{other}"
        );
    }
}

#[test]
fn meaningful_title_symbols_and_preview_word_survive_verification() {
    for (requested, observed) in [
        ("Sonata in F♯ minor", "Sonata in F minor"),
        ("Concerto in B♭ major", "Concerto in B major"),
        ("Signal 🌙", "Signal ☀"),
        ("Signal", "Signal Preview"),
    ] {
        let (request, candidate) = pair(requested, observed);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            assert_eq!(
                assess(&request, &candidate, level, MatchStage::Resolved).action,
                MatchAction::Reject,
                "{requested:?} vs {observed:?} {level:?}",
            );
        }
    }
    let (request, candidate) = pair("★", "★");
    assert_same_recording(&request, &candidate);

    let (mut request, mut candidate) = pair("Signal", "Signal");
    request.artist = "!!!".into();
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["!!!"]);
    assert_same_recording(&request, &candidate);
    candidate.source_metadata.as_mut().unwrap()["artistNames"] = json!(["???"]);
    assert_eq!(
        assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Reject,
    );
}

#[test]
fn video_edition_lyrics_suffix_accepts_typographic_dashes() {
    for title in [
        "Song – Video Version – Lyrics",
        "Song (Video Version — Lyrics)",
        "Song [Video Version － Lyric Video]",
    ] {
        let (request, candidate) = pair("Song (Video Version)", title);
        assert_same_recording(&request, &candidate);
        let (studio, video) = pair("Song", title);
        assert_ne!(
            assess(&studio, &video, MatchLevel::Exact, MatchStage::Resolved).action,
            MatchAction::Accept
        );
    }
}

#[test]
fn dash_in_title_does_not_turn_a_guest_name_into_a_remix() {
    let title = "Part One - Finale (feat. DJ Remix)";
    let parsed = crate::recording_identity::parse_title(title);
    assert_eq!(parsed.title, "Part One - Finale");
    assert_eq!(parsed.featured_credit.as_deref(), Some("DJ Remix"));
    assert_eq!(parsed.version, None);
    let evidence = crate::recording_identity::parse_recording_text(title);
    assert_eq!(
        evidence.annotations[0].kind,
        crate::recording_identity::AnnotationKind::FeaturedCredit
    );
}

#[test]
fn chart_clock_version_preserves_the_title_edition_without_implying_original_mix() {
    let title = "Scream & Shout (Extended Mix)";
    let (mut request, mut candidate) = pair(title, title);
    request.provider = "traxsource".into();
    request.artist = "Jewel Kid".into();
    request.version = Some("(6:41)".into());
    request.duration_seconds = Some(401.0);
    candidate.duration_seconds = Some(401.0);
    let metadata = candidate.source_metadata.as_mut().unwrap();
    metadata["artistNames"] = json!(["Jewel Kid"]);
    metadata["duration"] = json!(401);
    assert_same_recording(&request, &candidate);

    request.version = Some("Original Mix".into());
    assert_ne!(
        assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept
    );
}

fn bind_chart_capture(track: &mut Track, provider: &str, id: &str, url: &str) {
    track.provider = provider.into();
    track.provider_id = id.into();
    track.id = format!("{provider}:{id}");
    track.source_url = url.into();
    track.provenance["browserImport"] = json!(true);
    track.provenance["captureProvider"] = json!(provider);
    track.provenance["rawCaptures"] = json!([{
        "providerId": id, "sourceUrl": url, "title": track.title, "version": track.version
    }]);
}

#[test]
fn captured_traxsource_repeated_title_keeps_specific_mix_over_the_release_heading() {
    let title = "(I'm Sick) I've Been Infected Wit House Musick";
    let mix = "Terry Hunter Sick Wit It Mix";
    let mut request = traxsource_mix(
        &format!("{title} (Remix)"),
        &format!("I'm Sick I've Been Infected Wit House Musick [{mix}] (7:44)"),
        464.0,
        &["Mike Dunn", "Terry Hunter"],
    );
    bind_chart_capture(
        &mut request,
        "traxsource",
        "15057928",
        "https://www.traxsource.com/track/15057928/im-sick",
    );
    let corrected = crate::matching::chart_playback_identity(&request);
    assert_eq!(corrected.title, title);
    assert_eq!(corrected.version.as_deref(), Some(mix));

    let (_, mut candidate) = pair(title, &format!("{title} [{mix}]"));
    candidate.duration_seconds = Some(464.0);
    let meta = candidate.source_metadata.as_mut().unwrap();
    meta["artistNames"] = json!(["Mike Dunn", "Terry Hunter"]);
    meta["duration"] = json!(464);
    assert_same_recording(&request, &candidate);

    let wrong = format!("{title} [Terry Hunter Sick Wit It Instrumental]");
    candidate.title = wrong.clone();
    candidate.source_metadata.as_mut().unwrap()["catalogTitle"] = json!(wrong);
    assert_ne!(
        assess(
            &request,
            &candidate,
            MatchLevel::Balanced,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept
    );

    request.provenance["manualFields"]["version"] = json!(true);
    assert!(matches!(
        crate::matching::chart_playback_identity(&request),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn captured_traxsource_clock_rollover_cannot_override_the_named_remix() {
    let title = "Plastico (Bugz In The Attic Remix)";
    let mut request = traxsource_mix(title, "(6:90)", 450.0, &["Willie Colon", "Ruben Blades"]);
    bind_chart_capture(
        &mut request,
        "traxsource",
        "1494176",
        "https://www.traxsource.com/track/1494176/plastico-bugz-in-the-attic-remix",
    );
    let corrected = crate::matching::chart_playback_identity(&request);
    assert_eq!(corrected.version, None);
    assert_eq!(corrected.duration_seconds, None);
    assert_eq!(corrected.title, title);
    assert_eq!(request.version.as_deref(), Some("(6:90)"));
    assert_eq!(request.duration_seconds, Some(450.0));
    let mut upload = Candidate {
        title: format!("Willie Colon, Ruben Blades - {title}"),
        duration_seconds: Some(369.0),
        ..Candidate::default()
    };
    crate::matching::score_candidate(&request, &mut upload);
    assert!(upload
        .reasons
        .iter()
        .any(|reason| reason.contains("duration is unavailable")));
    assert!(upload
        .reasons
        .iter()
        .all(|reason| !reason.contains("version conflicts")));
    assert_ne!(upload.confidence, "high");
    request.duration_seconds = Some(400.0);
    assert!(matches!(
        crate::matching::chart_playback_identity(&request),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn captured_traxsource_vocal_remix_keeps_its_edition_and_literal_subtitle() {
    use crate::recording_match::Comparison;

    let title = "Victory (Are You Ready For The Drums?)";
    let mix = "Original Man Remix Vocal";
    let artists = ["Cherita", "K'Ture", "DJ Oji", "DJ Oji aka Original Man"];
    let mut request = traxsource_mix(
        &format!("{title} Remix"),
        &format!("{mix} (6:48)"),
        408.,
        &artists,
    );
    request.provenance["artistCredits"] = serde_json::Value::Null;
    bind_chart_capture(
        &mut request,
        "traxsource",
        "15086879",
        "https://www.traxsource.com/track/15086879/victory-are-you-ready-for-the-drums-remix-original-man-remix-vocal",
    );

    let corrected = crate::matching::chart_playback_identity(&request);
    assert_eq!(corrected.title, title);
    assert_eq!(corrected.version.as_deref(), Some(mix));
    assert_eq!(corrected.duration_seconds, Some(408.));
    for (caption, expected) in [
        (format!("{title} ({mix})"), true),
        (format!("{title} - {mix}"), true),
        (format!("{title} (Original Man Remix Dubstrumental)"), false),
        (format!("{title} (Another DJ Remix Vocal)"), false),
        (format!("{title} (Original Man Remix)"), false),
        (title.into(), false),
        (format!("Victory ({mix})"), false),
    ] {
        let mut candidate = youtube_mix("AaBbCcDdEe1", &caption, 408., MatchStage::Resolved);
        candidate.uploader = Some("POJI Records".into());
        candidate.source_metadata.as_mut().unwrap()["artists"] = json!(artists);
        for level in [
            MatchLevel::Exact,
            MatchLevel::Balanced,
            MatchLevel::Flexible,
        ] {
            let result = assess(&request, &candidate, level, MatchStage::Resolved);
            if expected {
                assert_eq!(result.dimensions.title, Comparison::Agree, "{result:?}");
                assert_eq!(result.dimensions.version, Comparison::Agree, "{result:?}");
                if level != MatchLevel::Exact {
                    assert_eq!(result.action, MatchAction::Accept, "{result:?}");
                }
            } else {
                assert_ne!(result.action, MatchAction::Accept, "{result:?}");
                if caption.contains("Dubstrumental") {
                    assert_eq!(
                        result.dimensions.version,
                        Comparison::Conflict,
                        "{result:?}"
                    );
                }
            }
        }
    }

    let mut mismatched = request.clone();
    mismatched.provenance["rawCaptures"][0]["version"] =
        json!("Original Man Remix Dubstrumental (6:48)");
    assert!(matches!(
        crate::matching::chart_playback_identity(&mismatched),
        std::borrow::Cow::Borrowed(_)
    ));
    request.provenance["manualFields"]["title"] = json!(true);
    assert!(matches!(
        crate::matching::chart_playback_identity(&request),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn captured_traxsource_collaboration_edition_matches_literal_caption() {
    use crate::recording_match::Comparison;

    let title = "Right Here Right Now";
    let caption = "Right Here Right Now (Jill Scott x Terry Hunter)";
    let mut request = traxsource_mix(
        title,
        "Jill Scott x Terry Hunter (6:14)",
        374.0,
        &["Jill Scott", "Terry Hunter"],
    );
    // The live capture has separate artist text but no structured artist rows.
    request.provenance["artistCredits"] = serde_json::Value::Null;
    bind_chart_capture(
        &mut request,
        "traxsource",
        "15081865",
        "https://www.traxsource.com/track/15081865/right-here-right-now-jill-scott-x-terry-hunter",
    );
    let label = "Blues Babe Records / Human Re Sources / The Orchard";
    request.label = Some(label.into());
    request.provenance["rawCaptures"][0]["label"] = json!(label);
    let mut candidate = youtube_mix("DQCXk4rQmCQ", caption, 374.0, MatchStage::Resolved);
    candidate.uploader = Some("Jill Scott".into());
    candidate.source_metadata.as_mut().unwrap()["artists"] = json!([
        "Jill Scott",
        "Terry Hunter",
        "Om'Mas Keith",
        "Yountie Strickland",
        "Carvin Haggins",
        "Malek Ben-Yisrael",
        "Lamar Andrews"
    ]);

    for level in [
        MatchLevel::Exact,
        MatchLevel::Balanced,
        MatchLevel::Flexible,
    ] {
        let resolved = assess(&request, &candidate, level, MatchStage::Resolved);
        assert_eq!(resolved.dimensions.title, Comparison::Agree);
        assert_eq!(resolved.dimensions.version, Comparison::Agree);
        if level == MatchLevel::Flexible {
            assert_eq!(resolved.dimensions.artists, Comparison::Agree);
            assert_eq!(resolved.action, MatchAction::Accept, "{resolved:?}");
        }
    }

    let artists = candidate.source_metadata.as_ref().unwrap()["artists"].clone();
    let credit_line = artists
        .as_array()
        .unwrap()
        .iter()
        .map(|artist| artist.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" · ");
    let info = json!({
        "id": "DQCXk4rQmCQ",
        "title": caption,
        "track": caption,
        "artists": artists,
        "channel": "Jill Scott",
        "channel_id": "UCYAoAgoEY3HPmybt1JxsT9Q",
        "channel_is_verified": true,
        "duration": 374.0,
        "description": format!(
            "Provided to YouTube by The Orchard Enterprises\n\n{caption} · {credit_line}\n\nProducer: Om'Mas Keith\n\nAuto-generated by YouTube."
        ),
    });
    let (distributor, source_credits) =
        crate::youtube_recording_metadata::generated_recording_credits(&candidate, &info).unwrap();
    let metadata = candidate.source_metadata.as_mut().unwrap();
    metadata["provider"] = json!("youtube");
    metadata["channel"] = json!("Jill Scott");
    metadata["channelId"] = json!("UCYAoAgoEY3HPmybt1JxsT9Q");
    metadata["channelIsVerified"] = json!(true);
    metadata["youtubeGeneratedArtistCredits"] = json!(source_credits);
    metadata["youtubeGeneratedDistributor"] = json!(distributor);
    let balanced = |track: &Track, source: &Candidate| {
        assess(track, source, MatchLevel::Balanced, MatchStage::Resolved)
    };
    let resolved = balanced(&request, &candidate);
    assert_eq!(resolved.action, MatchAction::Accept, "{resolved:?}");
    assert_eq!(resolved.relationship, MatchRelationship::Probable);
    assert_eq!(resolved.evidence.unwrap().extra_performers.len(), 5);
    assert_eq!(
        assess(
            &request,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Inspect
    );
    for (field, value) in [
        ("channel", json!("Different Channel")),
        ("channelIsVerified", json!(false)),
        ("resolvedProviderId", json!("unrelatedId")),
        ("duration", json!(375.0)),
        ("artistNames", json!(["Different Artist"])),
        ("version", json!("Other Remix")),
        ("youtubeGeneratedArtistCredits", json!(null)),
        (
            "youtubeGeneratedDistributor",
            json!("Different Distributor"),
        ),
        ("sourceMetadataOrigin", json!("ytDlpCandidateMetadata")),
    ] {
        let mut changed = candidate.clone();
        changed.source_metadata.as_mut().unwrap()[field] = value;
        assert_ne!(
            balanced(&request, &changed).action,
            MatchAction::Accept,
            "{field}"
        );
    }
    let mut reordered = candidate.clone();
    let metadata = reordered.source_metadata.as_mut().unwrap();
    metadata["artists"].as_array_mut().unwrap().swap(1, 2);
    metadata["youtubeGeneratedArtistCredits"] = metadata["artists"].clone();
    assert_ne!(balanced(&request, &reordered).action, MatchAction::Accept);
    let mut label_conflict = request.clone();
    label_conflict.provenance["rawCaptures"][0]["label"] = json!("Different label");
    assert_ne!(
        balanced(&label_conflict, &candidate).action,
        MatchAction::Accept
    );

    for (wrong_title, duration) in [
        (title, 264.0),
        (title, 374.0),
        ("Right Here Right Now (E.Morelli Remix)", 374.0),
        (
            "Right Here Right Now (E.Morelli Remix) (Jill Scott x Terry Hunter)",
            374.0,
        ),
    ] {
        let mut unrelated = candidate.clone();
        unrelated.title = wrong_title.into();
        unrelated.duration_seconds = Some(duration);
        let meta = unrelated.source_metadata.as_mut().unwrap();
        meta["track"] = json!(wrong_title);
        meta["catalogTitle"] = json!(wrong_title);
        meta["duration"] = json!(duration);
        for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
            assert_ne!(
                assess(&request, &unrelated, level, MatchStage::Resolved).action,
                MatchAction::Accept
            );
        }
    }

    request.provenance["manualFields"]["version"] = json!(true);
    let edited = assess(
        &request,
        &candidate,
        MatchLevel::Flexible,
        MatchStage::Resolved,
    );
    assert_ne!(edited.dimensions.version, Comparison::Agree);
    assert_ne!(edited.action, MatchAction::Accept);
}

#[test]
fn captured_beatport_subtitle_does_not_override_a_real_remix_marker() {
    let (mut request, _) = pair("Lady (Hear Me Tonight) - Remix", "");
    request.version = Some("Hear Me Tonight".into());
    bind_chart_capture(
        &mut request,
        "beatport",
        "21628116",
        "https://www.beatport.com/track/lady/21628116",
    );
    let corrected = crate::matching::chart_playback_identity(&request);
    assert_eq!(corrected.title, request.title);
    assert_eq!(corrected.version, None);
    request.provenance["rawCaptures"][0]["providerId"] = json!("different");
    assert!(matches!(
        crate::matching::chart_playback_identity(&request),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn captured_mulemba_xangola_requires_verified_presenter_and_full_lineup() {
    let title = "Mulemba Xangola";
    let caption = "Africanism presents Bob Sinclar, Jesús Fernandez & Bonga ft. Lura - Mulemba Xangola (Visualizer)";
    let request = Track {
        provider: "spotify".into(),
        provider_id: "6NWXqaeYRSqo788RsqKeXj".into(),
        artist: "Africanism, Bob Sinclar, Jesús Fernández, Bonga, Lura".into(),
        title: title.into(),
        duration_seconds: Some(226.571),
        provenance: json!({
            "identity": "unverified browser capture",
            "browserImport": true,
            "captureProvider": "spotify",
            "sourceKind": "playlist"
        }),
        ..Track::default()
    };
    let id = "o54MNSUAbq0";
    let search = youtube_mix(id, caption, 227.0, MatchStage::Search);
    let resolved = |caption: &str, duration: f64| {
        let mut candidate = youtube_mix(id, caption, duration, MatchStage::Resolved);
        candidate.uploader = Some("Africanism".into());
        let metadata = candidate.source_metadata.as_mut().unwrap();
        metadata["provider"] = json!("youtube");
        metadata["channel"] = json!("Africanism");
        metadata["channelId"] = json!("UCxGaEJiyyoKfCmQBlM8qp-A");
        metadata["channelIsVerified"] = json!(true);
        metadata["artists"] = serde_json::Value::Null;
        candidate
    };
    let candidate = resolved(caption, 227.0);
    for level in [MatchLevel::Balanced, MatchLevel::Flexible] {
        assert_eq!(
            assess(&request, &search, level, MatchStage::Search).action,
            MatchAction::Inspect
        );
        let result = assess(&request, &candidate, level, MatchStage::Resolved);
        assert_eq!(result.action, MatchAction::Accept, "{result:?}");
        assert_eq!(
            result.dimensions.artists,
            crate::recording_match::Comparison::Agree
        );
        for (field, value) in [
            ("channel", json!("Other Channel")),
            ("channelId", json!("invalid")),
            ("channelIsVerified", json!(false)),
            ("resolvedProviderId", json!("differentID")),
            ("identityMetadataUncertain", json!(true)),
            ("artists", json!(["Other Singer"])),
        ] {
            let mut wrong = candidate.clone();
            wrong.source_metadata.as_mut().unwrap()[field] = value;
            assert_ne!(
                assess(&request, &wrong, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{field}"
            );
        }
        for (changed, duration) in [
            (caption.replace("ft. Lura", "ft. Other Singer"), 227.0),
            (caption.replace(" & Bonga", ""), 227.0),
            (caption.replace("(Visualizer)", "(Live)"), 227.0),
            (caption.to_owned(), 321.0),
        ] {
            let wrong = resolved(&changed, duration);
            assert_ne!(
                assess(&request, &wrong, level, MatchStage::Resolved).action,
                MatchAction::Accept,
                "{changed} {duration}"
            );
        }
    }
    assert_ne!(
        assess(
            &request,
            &candidate,
            MatchLevel::Exact,
            MatchStage::Resolved
        )
        .action,
        MatchAction::Accept,
        "the accent-folded caption does not satisfy the Exact credit requirement"
    );
}
