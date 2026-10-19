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

Complete controlled native/live comparisons with the reported recordings, retain failures and incorrect matches, inspect the remaining desktop settings/player states, and qualify physical Apple Silicon macOS and Windows playback before promoting resolver/codec changes. The isolated 30-minute debug workload and bounded installed gates below are complete within their stated scope. They do not establish remote-stream recovery or physical-device reliability. The five-second startup p95 target remains; raising the timeout setting does not satisfy it. Public release is on hold at the user's request until the research is complete.

## v0.1.4 candidate

The retained v0.1.4 installer runtime is tagged at `7fd86094f2c648cc24503cbd56c4ddba5808ce2e` and remains unpublished. Later commits `c61e44c31094108daf2588d087eedee9c8b23fc9` and `d614c8919e5ab008a9577d78dd6dad0b43f80355` correct the qualification harness; the successful reruns below install the retained `7fd8609` binaries rather than rebuilding them. Publication is held at the user's request pending completion of the research. Local Svelte check and production frontend build pass; the final Linux native library suite passes 1,259 tests, zero failures, 25 ignored, and the frontend suite passes 1,065 tests.

The initial source `ed6ae8ae6a710285fd2c3bf6d0982f4f8b1e84af` and its [first qualification run](https://github.com/anaxonda/burrata/actions/runs/39294067482) are superseded. Their failures remain part of the qualification history; they are not current running jobs.

The actual StreamDownload cancellation regression failed before the second dependency patch: an uncached range request awaiting headers did not wake its blocking seeker within 500 ms after cancellation. The outer cancellation-select fix makes the seek return an error and closes the pending request; all 11 HTTP tests pass.

Foreground helper prewarming was attempted with three alternating pairs, all six producing five seconds of PCM and cleaning up helpers. Helper waiting fell from a median 3,046 ms to 2 ms, but the two pairs with matching source IDs averaged 774 ms slower overall. The production behavior remains unchanged. [Raw comparison](../../../scripts/performance/innertube/results/2026-10-19-matthew-prewarm-ab/comparison.json).

### Corrected release gates and actual native playback

The first candidate failed CI before installer publication: the smoke test compared canonicalized fixture paths to aliased temporary roots (macOS/Windows), Windows exposed unclosed SQLite handles, and the SQL contract still expected schema 8 after the delivered migration reached schema 10. The harness resolves both sides, explicitly closes connections, tests a symlinked fixture root and verifies handle closure; the Store migration/merge SQL is unchanged. The intermediate candidate `fe2a8ee0ea82383d9965b3655a144270bdcc0e84` and its [corrected platform run](https://github.com/anaxonda/burrata/actions/runs/39295959263) are also superseded. No release was published from these candidates.

The v0.1.4 debug binary completed the actual Linux native/MCP local fixture qualification: three warmups, 20 starts, 60 seeks (20 each at 1%, 50%, 95%) and paused seek/resume, zero failures. Start wall-to-first-audio observation median 64 ms, p95 82 ms, worst 540 ms. These figures concern a local synthetic WAV and include MCP/polling; they do not measure provider startup. Playback-bar geometry during active playback at 954×1036 reports no overlap findings. [Local playback report](debug-memory-30m/local-playback-qualification.json), [geometry](debug-memory-30m/player-layout.json).

### Completed 30-minute debug workload

The isolated Linux debug/Vite workload completed its 1,800-second measurement from 17:32:34 to 18:02:35 UTC, using binary SHA-256 `c9e3e3c06295043037223e971028fbef934fde12cce4056a1cb30211c38d7e2e`. It seeded 20,000 metadata-only library rows, played six queued copies of a six-minute synthetic local WAV at volume zero, and completed 117 library-search pages and five queue transitions. All 117 sampled playback states were `playing`; no sampled playback stop was observed. This does not exclude an interruption between samples.

Application/process-tree RSS median was 684.95 MiB and peak 696.43 MiB. The first and last five-minute RSS medians were 683.42 and 564.15 MiB, respectively. Application CPU median was 27.01% of one logical core, p95 36.51%. These are debug measurements from one Linux host, not release-build resource targets or proof that no leak exists. Process RSS can double-count shared pages, short-lived descendants can fall between polls, and the separately measured owned Vite process group is excluded from application totals. No public-provider or network playback was exercised and muted output does not establish audible quality.

[Run metadata](debug-memory-30m/metadata.json), [workload observations](debug-memory-30m/workload.ndjson), [RSS windows](debug-memory-30m/rss-windows.json), [compressed raw samples](debug-memory-30m/samples.json.gz), [sample integrity](debug-memory-30m/samples-integrity.json).

### Bounded installed gates for the retained runtime

The [source release run](https://github.com/anaxonda/burrata/actions/runs/39297682337) built and ran the platform test suites for runtime `7fd8609`. Its Windows installer job passed. The original macOS and Linux installed checks failed; their corrected-harness reruns are separate evidence and do not turn that original workflow's failed conclusion into a pass.

| Platform / artifact | Harness | Installed result and evidence |
| --- | --- | --- |
| Windows x64 / NSIS | Original `7fd8609` release job | The [Windows job](https://github.com/anaxonda/burrata/actions/runs/39297682337/job/118214414599), including its installed-artifact smoke step, passes. The bounded gate checks packaged tools/media, GUI/MCP/native host and local AAC WebView progression; no Windows numeric clock report is copied here. |
| Apple Silicon macOS / DMG | `c61e44c` rerun against retained `7fd8609` installer | [Rerun job](https://github.com/anaxonda/burrata/actions/runs/39303041772/job/118232957749) passes: packaged bundle ATS is limited to `127.0.0.1`, broad exemptions are absent, and local AAC WebView clock advances 0.776 s. [Retained report](installed-gates/mac-passed-c61e44c.json). |
| Linux x64 / DEB | `d614c89` rerun against retained `7fd8609` installer | [Final rerun job](https://github.com/anaxonda/burrata/actions/runs/39305102479/job/118240009851) passes: local AAC WebView clock advances 0.714 s between steady samples, with the same synthetic track/entry and no media error. [Retained report](installed-gates/linux-passed-d614c89.json). |

The macOS failure was a harness argument mismatch: it inspected `Contents/Info.plist` relative to the executable instead of the installed `.app` bundle. The corrected checker retains the same narrow ATS requirements. The [original macOS failure](installed-gates/mac-original-failure.json) is retained.

The [original Linux failure](installed-gates/linux-original-failure.json) and [diagnostic rerun failure](installed-gates/linux-diagnostic-failure-c61e44c.json) are retained. The diagnostic samples showed startup preroll with only 0.102 s clock advancement during the initial sampling interval, rather than proving sustained playback failure. Harness `d614c89` first requires actual startup clock progress within its bounded wait, then requires at least 0.5 s advance between steady samples. The successful report records media-clock positions of 0.116 s on initial readiness, 0.650 s on first progress and 1.364 s on the next sample; the steady samples are 915 ms apart. This corrects the measurement boundary without changing the installer or accepting a stationary clock.

These hosted-runner installed gates use temporary app data and generated local media. They do not qualify public provider identity/availability, native output on macOS/Windows, physical speakers or USB/Bluetooth devices, sleep/wake, downloaded-app security prompts, or the affected French Mac. Linux installed smoke covers DEB; RPM/AppImage remain build artifacts rather than separately installed/runtime-qualified packages. Broader live research and physical-device acceptance remain open; passing these gates does not lift the publication hold.

The parallel Luna audits are incorporated in [startup research](../../playback-startup-research.md), with upstream references and falsifiable experiments. No additional production startup optimization is promoted.
