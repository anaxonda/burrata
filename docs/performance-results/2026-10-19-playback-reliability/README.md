# Playback reliability implementation — 2026-10-19

This checkpoint implements the approved reliability plan on top of the existing dirty post-v0.1.3 work. Public installers are unchanged. Embedded provider web players and mpv are excluded.

## Implemented behavior

- Fresh extractor metadata drives one typed recording proof used for playback and mapping persistence. Schema 10 adds optional proof metadata; legacy mappings require fresh verification. Matthew-style catalog locators retain their verification policy without persisting stale channel/artist evidence. Fractional durations retain the ordinary tolerance; only quantized integer source durations use the extra allowance.
- Metadata extraction precedes final identity verification, which precedes CDN access validation. Existing range/head/tail/truncation and transient access checks remain. Shared foreground traces limit costly extractions to two; alternate formats from one response remain reusable. Optional catalog research is capped at three seconds while reserving eight seconds for playback stages.
- Safe Rust/TypeScript failure categories distinguish source access from recording absence, throttling, cancellation, storage and output failure. HTTP 403/404/410 alone do not advance the queue. Exact URL failures do not suppress fresh URLs for the same recording. Provider resets fence in-flight metadata, prepared alternatives, cached results and transport ownership.
- Startup timeout defaults to 20 seconds, with 30/45-second choices. Nested stages and seek recovery preserve their received absolute deadline. Explicit Retry starts a new operation. Live radio has bounded same-station recovery for errors, stalls and EOF; selected sources refresh without crossing source pins.
- Native preloads retain non-storage decoder failures until selection, including partial and zero PCM. Ownership transfers before first PCM when an upcoming source buffers; a startup watchdog bounds that handoff. Terminal native failures cancel work and stop owned output. Queue/session checkpoints are transactional and restore exact duplicate occurrences by ID.
- Native HTTP range responses validate status, bounds, totals, length and available representation validators. Changed or ignored ranges are rejected; an unsafe non-range reconnect cannot splice bytes from offset zero. FFmpeg stderr is bounded, drained and redacted instead of discarded.
- Isolated extraction and decoder experiments have cancellation, deadlines, idle shutdown, automatic token lifecycle support and per-child accounting. They do not change production defaults or access normal accounts.

## Verification

- Full frontend suite: 1,065 passed. Svelte check: zero errors and warnings.
- Isolated experiment tests: 17 Node and five Python tests passed.
- [Offline loopback matrix](../../../scripts/performance/innertube/results/2026-10-19-offline-loopback.json): 100/100 measured successful excerpts, 15 excluded warmups and one expected corrupt-media failure. This is FFmpeg decoder/fixture evidence; it excludes catalog matching, real provider availability, mixer/device output and cross-platform acceptance.
- Native suite: 1,259 passed, zero failures, 25 ignored. This includes local HTTP fixtures and native preload ownership/failure regressions. The final run includes the latest settings-navigation automation, public-error redaction and real pending-header cancellation tests.
- Strict range checks exposed an upstream stream-download 0.24.4 sparse-seek bug: a missing range before the requested position could produce a reversed request. A narrowly patched, licensed vendored dependency fixes gap selection; direct vendor regressions and the full native suite pass.
- Matthew production-resolver smoke produces five seconds of PCM, with exploratory first-PCM times around 12–14 seconds. The selected upload differs from the expected reference ID. Short-excerpt fingerprints support correspondence, but full-version equivalence remains unproven; later checks encountered HTTP 403. All outcomes are retained in the experiment reports.
- Desktop UI acceptance and broader native/live comparisons remain outstanding. Source fixes do not constitute live correctness or p95 qualification.

## Remaining acceptance

Run the final regression suite and controlled native/live comparisons with the reported recordings, retain all failures and incorrect matches, perform the 30-minute memory/recovery exercise, and verify packaged Apple Silicon macOS and Windows playback before promoting resolver/codec changes. The five-second startup p95 target remains; raising the timeout setting does not satisfy it.

## v0.1.4 candidate

Candidate source: `ed6ae8ae6a710285fd2c3bf6d0982f4f8b1e84af`. [Platform qualification run](https://github.com/anaxonda/burrata/actions/runs/39294067482) is running; the candidate is not published. Local Svelte check and production frontend build pass. The final Linux native library suite passes 1,259 tests, zero failures, 25 ignored; the full frontend suite passes 1,065 tests.

The actual StreamDownload cancellation regression failed before the second dependency patch: an uncached range request awaiting headers did not wake its blocking seeker within 500 ms after cancellation. The outer cancellation-select fix makes the seek return an error and closes the pending request; all 11 HTTP tests pass.

Foreground helper prewarming was attempted with three alternating pairs, all six producing five seconds of PCM and cleaning up helpers. Helper waiting fell from a median 3,046 ms to 2 ms, but the two pairs with matching source IDs averaged 774 ms slower overall. The production behavior remains unchanged. [Raw comparison](../../../scripts/performance/innertube/results/2026-10-19-matthew-prewarm-ab/comparison.json).
