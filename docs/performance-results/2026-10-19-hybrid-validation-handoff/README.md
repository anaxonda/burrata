# Hybrid native validation reader experiment

The hybrid retained-prefix reader is worth further integration research. It reduced the fixture request count from three to two and improved first decoded PCM at every simulated response delay, while preserving the independent strict bounded tail check. This code is confined to `cfg(test)`; no production reader, URL security, validation or device path was changed or promoted.

## Three-arm comparison

The same range-capable loopback fixture as the [earlier negative same-reader experiment](../2026-10-19-validation-handoff/README.md) served a generated 15-second mono 48 kHz PCM WAV with 1,440,044 bytes. It paced 8 KiB writes by 2 ms and delayed each response header by 0, 40 or 100 ms. Three trials per delay rotated all three arm orders, yielding 27 normal rows. Every three-arm trial produced exactly equal 12,000-sample PCM vectors from a quarter-second decode. No device, account, live source, player or production switch participated.

The baseline used existing parallel bounded head/tail acquisition probes, followed by the unchanged native reader and decoder. The original owned-reader arm sought to the tail and rewound. The hybrid constructed one native reader while an independent bounded tail request ran in parallel through the unchanged production `send_preview_range` and `validate_preview_range` helpers. It checked the native initial response, expected total and audio MIME, read exactly 1,024 actual prefix bytes into a retained 32 KiB `BufReader`, rewound only inside that buffer, and passed the same reader to the native decoder. It never sought the downloader to the tail.

| Simulated response delay ms | Baseline first PCM ms | Tail-seek handoff first PCM ms | Hybrid first PCM ms | Baseline server accepted body bytes | Hybrid server accepted body bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 36.10 | 391.55 | 30.18 | 59,392 | 50,176 |
| 40 | 114.54 | 511.55 | 72.28 | 59,392 | 58,368 |
| 100 | 262.65 | 699.09 | 136.32 | 83,968 | 82,944 |

Values are medians of three observations per arm and delay, measured from validation start through receipt of the first nonempty PCM block. Baseline and tail-seek handoff each used three requests. Every hybrid normal row used exactly two: one full initial request beginning at byte zero and one strict bounded tail request. The hybrid avoided the large gap fill observed after the original handoff's tail seek and rewind. These within-run comparisons do not replace or alter the separately retained earlier negative results.

**Server accepted body bytes count successful server `write_all` calls at arm completion.** They are not client-received bytes, acknowledged socket traffic, or bytes required before first PCM. The tail-seek arm accepted median 1,448,236 / 1,603,884 / 1,841,452 server body bytes at the three delays. Requested interval overlap and server writes are separate measures. Test session decoder seeding omits native reader counters, so outer timing and explicit fixture request records are the evidence.

## Response safeguards and limits

All seven malformed hybrid fixtures rejected before PCM: wrong tail range, truncated tail, corrupt audio, malformed initial Content-Range, mismatched initial Content-Length, initial HTML MIME and a truncated initial prefix. Fourteen rows retain the corresponding baseline observations as well. The tail and prefix truncation checks record response errors even when the download manager can repair bytes; the prefix observer is a diagnostic guard, not a proposed production ban on recoverable network retries. Corrupt audio was rejected by both decoders; baseline's deliberately unavailable ffmpeg fallback also failed.

The baseline's earlier bounded head response remains valid in the three initial-full-response-only faults. Those baseline rows are explicitly observational rather than assumed to reject before PCM: malformed initial Content-Range was rejected, but a wrong length and an HTML MIME on the later native reopen each still produced 12,000 PCM samples. The hybrid rejected both using its expected-total and MIME checks. This does not establish broader production decoder vulnerability; it demonstrates the fixture distinction between validating one response and opening another.

The initial hybrid response is a native validated complete-resource 200/206 response with a retained prefix. It is **not** a synthetic bounded 206 head response, and reading its prefix cannot certify completion of the entire body before decode. Known prefix bytes remain retained; later body errors remain native transport errors. The independent tail retains current status, MIME, range, expected length and exact bounded-body checks. Future integration must preserve URL/DNS/session/cancellation protections and explicitly review this response-shape difference rather than claiming exact bounded-head protocol equivalence.

This fixture has no trailing container metadata dependency. Three trials with simulated response delay do not establish public CDN behavior, DNS/TLS latency, container coverage, percentile latency, packaged playback or release qualification. The earlier downloader-repair counterexample remains relevant and unchanged. The result supports a narrowly scoped integration design review, not production promotion.

## Reproduction and retained evidence

The [complete report](experiment.json) contains 27 normal rows and 14 safeguard observations; [summary.json](summary.json) groups exact medians and request counters. The [test log](test.log) records one passing ignored test, zero failures and 1,296 filtered tests in 10.78 seconds. Reports are checkpointed before assertions so failures preserve earlier rows. No source URL, credential or account metadata is stored.

```sh
BURRATA_HYBRID_HANDOFF_REPORT=/tmp/hybrid-handoff-repeat.json cargo test --locked --manifest-path src-tauri/Cargo.toml --lib native_output::validation_handoff_diagnostic::compare_preflight_same_reader_and_hybrid_handoff -- --ignored --exact --nocapture
```

The captured run used the existing feature-test binary after the full feature suite passed 1,270 tests with 27 ignored, with loopback permission enabled; no competing benchmark ran. The new tail bridge is `acquisition::diagnostic_verify_preview_audio_tail`; the three arms live in `native_output::validation_handoff_diagnostic`. Production behavior remains unchanged by the experiment.
