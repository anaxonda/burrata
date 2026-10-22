//! Explicit-network diagnostic only: one resolved URL, unchanged access checks,
//! then old/new decoder routing in alternating order. Never prints the URL.
use super::*;

fn count_setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    let value = std::env::var(name).map_or(default, |value| {
        value.parse::<usize>().expect("invalid bounded pair count")
    });
    assert!((min..=max).contains(&value), "pair count outside bounds");
    value
}

fn eligible(arm: &Value, legacy: bool) -> bool {
    arm["ok"] == true
        && arm["pcm"]["decodedFrames"]
            .as_u64()
            .is_some_and(|frames| frames >= 240_000)
        && arm["pcm"]["firstPcmMs"]
            .as_f64()
            .is_some_and(|ms| ms.is_finite() && ms >= 0.)
        && arm["pcm"]["decoder"]["decoderKind"] == "ffmpeg"
        && arm["pcm"]["decoder"]["failedNativeAttempt"].is_object() == legacy
}

#[test]
fn pair_eligibility_retains_failures_and_requires_route_evidence() {
    let good = json!({"ok":true,"pcm":{"decodedFrames":240000,"firstPcmMs":10.,
        "decoder":{"decoderKind":"ffmpeg"}}});
    assert!(eligible(&good, false));
    assert!(!eligible(&good, true));
    let mut legacy = good.clone();
    legacy["pcm"]["decoder"]["failedNativeAttempt"] = json!({"reason":"decoder_setup"});
    assert!(eligible(&legacy, true));
    for field in ["ok", "frames", "first", "decoder"] {
        let mut bad = good.clone();
        match field {
            "ok" => bad["ok"] = json!(false),
            "frames" => bad["pcm"]["decodedFrames"] = json!(239999),
            "first" => bad["pcm"]["firstPcmMs"] = Value::Null,
            _ => bad["pcm"]["decoder"]["decoderKind"] = json!("rodio"),
        }
        assert!(!eligible(&bad, false));
    }
}

#[tokio::test]
#[ignore = "explicit public-network opt-in; matched decoder-only pairs, no device/catalog proof"]
async fn live_three_public_ids_paired_opus_routes() {
    assert_eq!(
        std::env::var("BURRATA_NATIVE_DIRECT_PCM_PAIR_LIVE").as_deref(),
        Ok("1"),
        "requires explicit paired PCM live opt-in"
    );
    let repetitions = count_setting("BURRATA_NATIVE_DIRECT_PCM_PAIR_REPETITIONS", 3, 1, 20);
    let warmups = count_setting("BURRATA_NATIVE_DIRECT_PCM_PAIR_WARMUPS", 0, 0, 3);
    let total = repetitions + warmups;
    // Each pair has one 19s resolution plus two independent 19s access+PCM
    // deadlines. Cleanup join may need the caller's process-group hard cap.
    let outer_deadline = Instant::now() + Duration::from_secs((total * 57) as u64);
    let corpus: Value = serde_json::from_str(include_str!(
        "../../scripts/performance/innertube/fixtures/transport-music-tracks.json"
    ))
    .unwrap();
    let corpus = corpus.as_array().unwrap();
    assert_eq!(corpus.len(), 3, "fixed public three-ID corpus required");
    let experiment = DirectExperiment::new().unwrap();
    let ffmpeg = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources/tools")
        .join(crate::tools::target_triple())
        .join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
    assert!(
        ffmpeg.is_file(),
        "bundled decoder asset required for live diagnostic"
    );
    let mut successful_pairs = 0;
    let mut successful_warmups = 0;
    for pair_index in 0..total {
        let is_warmup = pair_index < warmups;
        let row = &corpus[pair_index % corpus.len()];
        let request: Request = serde_json::from_value(json!({"videoId":row["videoId"],
            "expectedVideoId":row["videoId"],"hl":"en","gl":"US","visitorMode":"anonymous-source",
            "expectedMetadata":{"titleNeedle":row["titleNeedle"],"channelNeedle":row["channelNeedle"],
                "durationSeconds":row["durationSeconds"]}})).unwrap();
        let order = if pair_index % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        };
        let common_started = std::time::Instant::now();
        let (_stop, cancel) = watch::channel(false);
        let resolved = experiment
            .resolve(
                &request,
                outer_deadline.min(Instant::now() + Duration::from_secs(19)),
                cancel.clone(),
            )
            .await;
        let common_resolution_ms = common_started.elapsed().as_secs_f64() * 1000.;
        let (audio, metadata) = match resolved {
            Ok(selected) => selected.into_preview_audio(),
            Err(error) => {
                eprintln!(
                    "{}",
                    json!({"experiment":"native-direct-opus-route-pair-v1",
                    "pairIndex":pair_index,"warmup":is_warmup,"videoId":request.video_id,
                    "ok":false,"failure":error.code,"stage":error.stage,"httpStatus":error.http_status,"arms":[],
                    "commonResolutionMs":common_resolution_ms,"qualification":false})
                );
                continue;
            }
        };
        assert!(
            audio.selected_format_id.as_deref() == Some("251")
                && metadata.format_id == 251
                && audio.codec.as_deref() == Some("opus")
                && !audio.selected_format_has_video,
            "common selected format is outside the fixed Opus profile"
        );
        let mut arms = Vec::new();
        for legacy in order {
            let arm_started = std::time::Instant::now();
            let deadline = outer_deadline.min(Instant::now() + Duration::from_secs(19));
            let access = crate::acquisition::diagnostic_validate_prepared_preview_audio(
                audio.clone(),
                cancel.clone(),
                crate::playback_sources::PlaybackDeadline::at(deadline),
            )
            .await;
            let access_ms = arm_started.elapsed().as_secs_f64() * 1000.;
            let mut arm = match access {
                Err(_) => json!({"ok":false,"failure":if Instant::now() >= deadline {"TIMEOUT"}
                    else {"ACCESS_VALIDATION_FAILED"},"stage":"access","accessMs":access_ms}),
                Ok(checked) => {
                    assert!(
                        checked.url == audio.url,
                        "access validation changed the common source"
                    );
                    let source = crate::native_output::AudioSource {
                        input: checked.url,
                        ffmpeg: ffmpeg.clone(),
                        headers: checked.headers,
                        decoder_hint: Some(crate::native_output::AudioSourceDecoderHint {
                            mime_type: Some(checked.mime_type),
                            codec: checked.codec,
                            duration_seconds: checked.duration_seconds,
                            content_length: checked.expected_content_length,
                            has_video: false,
                        }),

                        cenc: None,
                    };
                    let pcm = crate::native_output::direct_pcm_experiment::collect_until_with_opus_control(
                        source,0.,5.,deadline.into_std(),cancel.clone(),legacy,false).await;
                    let first = pcm.first_pcm_ms.map(|ms| ms + access_ms);
                    json!({"ok":pcm.ok,"accessMs":access_ms,"firstPcmFromArmMs":first,"pcm":pcm})
                }
            };
            arm["route"] = json!(if legacy {
                "legacy-native-probe"
            } else {
                "direct-ffmpeg"
            });
            arm["wallMs"] = json!(arm_started.elapsed().as_secs_f64() * 1000.);
            arm["eligible"] = json!(eligible(&arm, legacy));
            arms.push(arm);
        }
        let pair_ok = arms.iter().all(|arm| arm["eligible"] == true);
        if pair_ok {
            if is_warmup {
                successful_warmups += 1;
            } else {
                successful_pairs += 1;
            }
        }
        eprintln!(
            "{}",
            json!({"experiment":"native-direct-opus-route-pair-v1","pairIndex":pair_index,
            "warmup":is_warmup,"videoId":request.video_id,"ok":pair_ok,"sameResolvedUrl":true,
            "metadata":metadata,"commonResolutionMs":common_resolution_ms,"arms":arms,
            "repetitions":repetitions,"warmups":warmups,"armBudgetSeconds":19,"qualification":false,
            "scope":"same explicit public ID and format; resolver timing common, access+decoder arms independent; no device/catalog/acoustic proof"})
        );
    }
    eprintln!(
        "{}",
        json!({"experiment":"native-direct-opus-route-pair-summary-v1",
        "successfulPairs":successful_pairs,"repetitions":repetitions,"successfulWarmups":successful_warmups,
        "warmups":warmups,"qualification":false,"httpClientReused":true})
    );
    assert_eq!(successful_pairs, repetitions, "failed pairs retained above");
    assert_eq!(successful_warmups, warmups, "failed warmups retained above");
}
