# Playback audit — implementation and final qualification in progress

This audit covers saved Library, search, album, artist, and Spotify-imported entry paths; provider selection; matching options; and delivered native audio. Previous-version comparisons are diagnostic where useful. Tests use isolated profiles and private virtual output; normal-library writes and host routing changes are excluded. User steering defers live Monochrome checks while that service is down and makes YouTube the primary audio source.

## Packaged playback evidence

The observations below qualify source **a164c991477a5930dc0a9d51d97e14bb805d668d**, ELF SHA-256 **fcdb160fd82a89039f5b30071465c86636887a75b8277bde94d180348f9bb883**. They do **not** qualify the pending source fixes. The native gate requires at least 5 seconds of observation after first audio and 4.5 active seconds, separately from the 20-second startup deadline. Each successful run reported unchanged host audio inventory.

| Entry path / request | Policy or selected delivery | Evidence / result |
| --- | --- | --- |
| Fresh YouTube Bacc Of The Bach | Flexible, separate 30/30-second bounds | Pass, 5.2 s active |
| Legacy saved Discogs Bacc, duration absent | Ordinary Flexible duration rule | Pass, 5.1 s active |
| Same saved Discogs Bacc | Explicit 30/30-second bounds | Identity refusal; missing runtime cannot establish the requested delta |
| Actual Library pointer click, saved search-derived Bacc | YouTube | Pass, 5.0 s active; entrypoint-ui/evidence-2 |
| Albums → Prolific → Play Bacc | Legacy Discogs metadata, ordinary Flexible | Pass, 5.0 s active; library-entities/entities-run-4 |
| Artists → Nipsey Hussle → All tracks → Play Bacc | Same saved Discogs identity | Pass, 5.0 s active; same entity evidence |
| Renderer Online search → Bacc occurrence | YouTube | Pass, 5.0 s active; entrypoint-batch/evidence-6 |
| Renderer Library search → saved Bacc occurrence | YouTube | Pass, 5.1 s active; same batch evidence |
| Fresh Lady Soul (Oscar P Dub) search entry | YouTube | Pass, 5.1 s active |
| Fresh Bandcamp Fly Down search entry | Delivered YouTube | Pass, 4.8 s active; entry path is not proof of Bandcamp delivery |
| Explicit Amame (En Vivo) SoundCloud entry | SoundCloud | Pass, 4.9 s active; not the Spotify studio recording |
| Saved Spotify Mary / Cama y Mesa | Historical Monochrome-preferred rescue through YouTube | Pass, 4.8 / 4.7 s active; retained, not repeated after user steering |
| Fresh Spotify Velvet Ring / Paul, 2023 Remasters | Balanced and Flexible; delivered YouTube | Both pass in both policies, 4.8–5.2 s active |
| Fresh Spotify Vampire Empire | Balanced and Flexible; Bandcamp rescue wins | Startup succeeds; 2.5 / 2.6 s active fails the strict initial activity gate |
| Direct YouTube Vampire Empire control | Flexible | Startup succeeds; 2.7 s active fails the same activity gate |

Private evidence roots are `/tmp/burrata-playback-audit`, `/tmp/burrata-playback-entrypoint-ui`, `/tmp/burrata-playback-library-entities`, and `/tmp/burrata-playback-entrypoint-batch`. Their source/fixture fingerprints and failed attempts are retained. Earlier harness failures (schema enum, readiness, seed, undefined helper, inert artwork click, timing gate, quota, and an abandoned pointer-search observation) are excluded from playback pass/fail counts. A conformance regression now checks renderer-search enums against the native API.

The Vampire Empire diagnostic observes PCM directly at FFmpeg stdout, before the mixer: the first 3.7 seconds remain below −60 dBFS, and 5.5 seconds below −50 dBFS. No seek, time limit, or audio filter was added. Cleanup was verified. Its normal activity gate still fails; this does not establish a transport/startup failure or distinguish an intentionally quiet recording from decoder concealment. Evidence: `/tmp/burrata-vampire-source-prefix/evidence` and its referenced isolated profile. Do not silently waive the gate or label this recording fully qualified.

The earlier diverse cohort also includes Gillian Welch, electronic/house, hip-hop, Django Reinhardt, and Halka Halka. Django has nine accepted recordings and three refusals with unresolved performer evidence; Halka Halka remains unresolved. These refusals cannot justify weakening performer, version, identifier-conflict, or source-lock protections.

## Causes and pending fixes

Bacc is a demonstrated same-binary metadata/settings interaction, not proof of a version regression. The saved row lacks duration while the direct source provides it. Existing trusted-complete-credit exceptions remain intact; incomplete credits do not become complete merely because artist counts match. A regression now distinguishes those cases, and settings help explains that enabling duration bounds can prevent an automatic match when saved duration is missing. No duration is invented from a proposed candidate.

Retrieval now strips only a valid terminal display runtime from separately stored version text. Meaningful mix labels and original request metadata remain intact. Lady Soul supplied the motivating different retrieval contexts; the evidence does not prove the clock alone caused its earlier empty search. The three retrieval regressions passed CI 41686819531. That run failed its Bacc fixture because it incorrectly claimed exhaustive credits; the pending corrected fixture mirrors actual incomplete provider metadata and separately checks the legitimate complete-credit case.

Completed recording-search cache revision advances to v4 for changed orchestration. Raw-query cache keys remain unchanged.

## Independent audit disposition

| Finding | Implementation / disposition | Required remaining evidence |
| --- | --- | --- |
| Finite remote Opus/WebM status-0 truncated EOF | Incremental bounded stderr scanner rejects explicit demux/read truncation; clean EOF and cancellation semantics retained | Fake-FFmpeg clean/truncated tests and final native/package execution |
| macOS default-output disconnect callback suppressed by CPAL 0.17.3 | Minimal vendored same-version callback forwarding; existing pause behavior retained | Explicit dependency test in macOS CI; physical unplug remains unqualified |
| Queued successor generation combined with old source during recovery | Freeze queued activation with a shared gate, then drain/adopt a coherent reconnect snapshot; callback never blocks on the gate | Deterministic generation/source/bounds/gain regression; physical transition remains unqualified |
| Seek dropped while reconnecting before service notification | Update pending reconnect generation/position, preserving pause state | Paused/playing/stale-generation regressions |
| Windows healthy default-endpoint change | Source-supported routing gap; no physical reproduction, no speculative routing change | Windows endpoint and per-app-routing qualification |
| Silent callback stall / missing output-progress watchdog | Missing health path confirmed; driver-stall hypothesis not reproduced | Physical callback/sleep/wake qualification; remains limitation |
| Catalog refresh excludes sole source rejected against stale identity | Separate identity-only exclusions from sticky transport/extraction failures | Single-source corrected-identity and mixed-failure regressions |
| Unrelated stale future queue row prevents healthy selected row | Keep structural/size limits; validate row playability at activation and preloaded handoff | Store/file/identity and source-audition regressions |
| Portable reviewed ID depends on top-three text search | YouTube exact-ID fresh inspection plus unchanged final identity gate and bounded fallback | Exact/wrong-ID regression; SoundCloud/Monochrome still retain text-search dependency |
| Saved audioSearch embedded candidate suppresses replacement | Bypass only failed remembered candidate during unlocked recovery; true locks retained | Replacement and explicit-lock/lookup regressions |
| Completed zero-result search never refreshes stale catalog metadata | One bounded exact-ID refresh for automatic unpinned Spotify/Beatport/Traxsource; malformed/transport/timeout not treated as empty | Empty→corrected→playable and malformed-negative regressions |
| Slow Play / Stop ownership and artist preflight races | Playback intent generation checked before ownership mutation and command; redundant awaited loudness preflight removed | Deferred-command frontend regressions and packaged checks |
| Same-queue reuse ignores resolver identity/provenance changes | Compare provider ID, ISRC and provenance as well as prior playback fields | Corrected identity/policy queue tests |
| Chapter inspection implicitly source-locks lookup parent | Lookup ignores inspection-only candidate; real manual/source/chapter locks preserved | Source-lock tests and UI race probes |
| Mutable index-only Queue Play selects wrong occurrence | Paired expected revision/occurrence ID atomically checked before cancellation; legacy calls remain compatible | Duplicate/reorder/removal guards and frontend integration tests |

Source reports are retained at `/tmp/burrata-independent-playback-audit/report.txt`, `/tmp/burrata-saved-search-audit/report.txt`, and `/tmp/burrata-ui-playback-audit/report.txt`. Other platform test targets (alternate device configurations, blocked network-file reads, non-ASCII installations, physical outputs) remain unqualified rather than claimed defects fixed by Linux fixtures. Portable exact-ID and catalog refresh support are explicitly provider-scoped, not universal.

## Verification and release gate

Lightweight checks completed during implementation: Svelte/TypeScript 0 errors and warnings; MCP 12 tests; extension 98 tests; player 5 tests; repository contracts 8 checks; performance-harness 38 tests; renderer-entrypoint helper 10 tests; UI race probes 8 grouped cases; source-lock tests 9. The fresh full frontend rerun passes all 1539 tests in 153 files after correcting the three mock queue-identity fixtures. The extracted production FFmpeg stderr scanner also passes its isolated rustc regression; this does not compile the surrounding app decoder. Rust formatting, lockfile metadata, workflow parsing, and the heuristic source secret scan were checked. These are not substitutes for native execution.

The first complete batch, source `6bf03bf025e96ce5cbc26e8a65041339208f4982`, passed frontend/build checks in CI 41746021195 but failed native compilation before tests ran. Compiler diagnostics identified the missing `Path` import, omitted activation-gate wiring in included test fixtures/calls, Rodio nonzero channel/rate fixture arguments, temporary event-guard lifetimes, and a macOS callback-helper generic type mismatch. These corrections are being verified together; no package was produced by that failed run. Native tests for the corrected batch have not run. Local Cargo dependency compilation exhausted storage before producing a test binary. An inadvertently invoked decode-contract helper also started a second local dependency build; it produced no test binary. Reconstructible output from both task-owned attempts was removed with scoped receipts, preserving older target files and the main workspace index. After review and lightweight gates, one batched CI run will validate all native changes, including the explicit macOS dependency regression. Final-source packaging, isolated installed Linux audio qualification, affected route/recovery reruns, and controlled before/after performance measurements remain required before publication and local installation. Existing package results cannot qualify changed source. macOS/Windows physical audio and Monochrome live availability remain explicit limitations.
