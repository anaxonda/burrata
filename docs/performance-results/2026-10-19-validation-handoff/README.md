# Native validation reader handoff experiment

The naive same reader handoff should not be promoted. It eliminated a second request starting at byte zero but kept the total request count at three, caused a large gap fill after tail validation, and delayed first decoded PCM in every simulated latency group. The implementation is confined to `cfg(test)`; production validation, URL security, native reader ownership and device playback were not changed by this experiment.

## Controlled comparison

A range-capable loopback server served a generated 15-second mono 48 kHz PCM WAV containing 1,440,044 bytes. That exceeds the production 1 MiB tail-probe threshold. The server added 0, 40 or 100 ms before each response header and paced body writes in 8 KiB chunks with a 2 ms delay. Each delay had three paired trials with alternating arm order, for 18 normal rows. This simulates request latency without claiming equivalence to public CDN, DNS or TLS behavior.

The baseline ran the existing acquisition head and tail probes in parallel, followed by the unchanged native URL reader and decoder. The handoff arm constructed the same `NativeHttpClient`, `HttpStream`, adaptive 2 MiB storage and 32 KiB prefetch, read a 1 KiB head and tail from that owned reader, rewound it, and passed it to the same native decoder through a test `DecoderSession`. Both arms decoded a quarter-second excerpt. Every pair produced exactly equal 12,000-sample PCM vectors. No mixer, physical device, provider player, mpv, live recording or account credential participated.

## First PCM results

Values are medians of three observations per arm and response delay, measured from the start of validation through receipt of the first nonempty PCM block.

| Simulated response delay ms | Separate preflight first PCM ms | Owned handoff first PCM ms | Separate preflight server accepted body bytes | Owned handoff server accepted body bytes |
| --- | ---: | ---: | ---: | ---: |
| 0 | 50.34 | 424.57 | 59,392 | 1,456,428 |
| 40 | 139.16 | 544.54 | 75,776 | 1,603,884 |
| 100 | 245.61 | 720.89 | 83,968 | 1,849,644 |

All normal rows made three HTTP requests and one tail request. The baseline made two requests starting at byte zero; the handoff made one, but added a gap request covering bytes 40,960 through 1,439,019 after its tail seek and rewind. Requested interval overlap was 2,048 bytes for the baseline and 1,399,084 bytes for the handoff. The observation is specific to using the current download manager this way; it does not reject every possible prefix-buffer handoff design.

**Server accepted body bytes are counts of successful server `write_all` calls at the arm-completion snapshot.** They are not client bytes received, acknowledged socket traffic, consumed audio bytes, or bytes needed before first PCM. Requested interval overlap is a separate arithmetic measure of requested ranges, not network transfer telemetry. The owned decoder was seeded into a test session, so its native `readerReused` fields and zero reader counters must not be interpreted as a warm playback run; compare the outer timing and explicit request records.

## Validation mismatch and safeguards

The [exact first failing log](first-failure.log) and [extracted failing row](first-failure.json) preserve an important counterexample. A simple `read_exact` on the owned download reader reconstructed a truncated tail through follow-up ranges and produced PCM, while the existing bounded-response preflight rejected it. The first process had completed its 18 normal arms before panicking, but those rows were never emitted; they cannot be recovered. The successful report is a separately measured rerun, not a replacement for that first failure.

The rerun includes an explicit unguarded truncated-tail control. It reproduced three hidden HTTP body errors, four tail requests, six total requests and 12,000 PCM samples. A test-only response observer then recorded request, range and body validation errors before handoff, even if the downloader repaired the bytes. With that diagnostic guard, wrong tail ranges, truncated tails and corrupt audio each failed before any PCM in both arms: six safeguards passed. The guard establishes parity with the baseline for this experiment; it is not a production policy to reject every recoverable network retry.

The test checkpoints all rows before assertions, so a later failure preserves earlier observations. The [complete rerun](experiment.json) contains 18 normal arms, the unguarded control and six safeguards; [summary.json](summary.json) groups the medians and counters. The ignored diagnostic test passed once, with 1,294 other tests filtered out, in 12.08 seconds. Three trials and a WAV fixture do not establish a latency percentile, container coverage or production playback qualification.

## Reproduction

Run from the repository root in an environment that permits a loopback listener, using a new report path to preserve prior captures:

```sh
BURRATA_VALIDATION_HANDOFF_REPORT=/tmp/validation-handoff-repeat.json cargo test --locked --manifest-path src-tauri/Cargo.toml --lib native_output::validation_handoff_diagnostic::compare_preflight_and_owned_validated_reader_handoff -- --ignored --exact --nocapture
```

The test-only bridge is `acquisition::diagnostic_verify_preview_audio_access`; the fixture, observer and experiment are in `native_output::validation_handoff_diagnostic`. The production reader must retain its existing bounded range, length, representation, cancellation and session protections. Any future design should avoid tail-seeking this downloader before decode, preserve strict response evidence across repair, and demonstrate fewer requests or lower first-PCM latency before promotion.
