# Implementation status

## Active release checkpoint — 2026-10-17

The sole normal Linux debug app runs immutable native `4bfab35be85c` with the final reachable identity action and compact Search controls. Exact 30-occurrence queue, selected index, paused position 277.684804476s, volume, repeat and shuffle settings are preserved. Its predecessor successfully replays the previously stalled recording through 305.37s before restoring that position.

Final source passes 1,092 native library tests (21 ignored), 978 frontend tests across 81 files, source/permission contracts and the secret-pattern scan. Production frontend/native debug builds pass with 5,645 pinned inputs unchanged. Provider-worker153, extension77, MCP12, player5 and decoder/launcher checks pass within their stated scopes. The complete Cargo command passes, including binary targets and doc tests. Final fixture-only corrections preserve production executable bytes (`4bfab35be85c`); code whitespace, source audit and command contracts pass.

Live exact hinted-album verification succeeds; a wrong-edition hint fails closed. Actual narrow Details displays the identity action, Search sort text is readable and playback geometry reports no overlap. See [crosswalk evidence](performance-results/2026-10-17-verified-source-crosswalk/README.md) and [desktop evidence](performance-results/2026-10-17-release-desktop-pass/README.md). Catalog identity does not guarantee playable audio.

Remaining work is final release checks and tagged platform builds/installed-package smoke checks, then the non-public GitHub release. No tag or push has been made yet. Spotify playlist work remains paused, YouTube writes remain off pending review and experiments remain off. [Current todo](../todo.md).

## Historical checkpoints

The records below retain earlier bounded evidence and limitations. Their “current” runtime names and old release holds are historical, not the active release status above.

## Current checkpoint — 2026-10-17

The sole normal app runs immutable Linux debug native `2bb09a9b0ce9` and App `33260ec39305`, with the restored 30-entry queue, exact occurrence IDs/order, index 0, paused position 3.287602s, volume 0.8, repeat off and shuffle off verified unchanged. Native source passes 1,067 library tests (21 ignored) and its executable build. The frontend has a 930-test full run plus three focused passes after test-only provenance type corrections, Svelte 0/0, contracts and production builds. The compact search toolbar and honest continuation status are running; actual full/half-width screenshots pass the bounded default-state review. Full queue/playback and Library membership stay unchanged.

Controlled helper fixtures cover the shared 20-second search/nested-extraction deadline, search/extraction cancellation, cleanup and the single-provider two-attempt cap. Real network/probe/cache callers and full playback qualification remain open. Kaufman still has 19 unavailable outcomes; the current diagnostic evaluates 184 candidates with no extraction. Fruit Bats has two warm sustained passes and one reproduced underflow, plus four separate 30-second controls without buffering. These do not establish reliable playback or recovery of the failed operation.

Current Spotify reads return eight releases for the reported Bulgarian choir artist and all 16 Vol.2 album tracks, each with duration/artwork. Library counts and the full paused playback snapshot stay unchanged. A read after restart returns the same 8/8 releases on the same account without a new sign-in. This is bounded API acceptance, not complete Spotify/profile/writeback qualification. Spotify playlist work remains deferred; YouTube writes still require the pending exact reviewed approval. No release has been created; public release stays on hold.

Current Downloads/Settings dark-theme full/half-width checks pass, including repeated Settings → Back to Downloads, with full queue/playback and original tiling preserved. Both current managed provider reads survive the latest restart on the same response accounts: Spotify eight releases and YouTube62 unique playlists. Library counts remain unchanged; complete account coverage and expiry remain open. See [desktop verification](performance-results/2026-10-17-desktop-navigation-compact/README.md) and [provider restart reads](performance-results/2026-10-17-managed-provider-restart-reads/README.md).

Use [todo](../todo.md) for remaining scope, [controlled deadlines](performance-results/2026-10-17-resolution-terminal-trace/README.md), [Kaufman gates](performance-results/2026-10-17-kaufman-gate-diagnostic/README.md), [Fruit underflow/UI](performance-results/2026-10-17-fruit-current-underflow/README.md), and [current Spotify reads](performance-results/2026-10-17-current-spotify-artist-album/README.md) for evidence and limits.

One fresh unpinned Monochrome-only Fruit Bats request now resolves the expected source in 1.666 seconds and shows initial WebView progress, with the full normal queue restored afterward. This does not close sustained playback or imported-row matching. [Bounded result](performance-results/2026-10-17-fruit-unpinned-webview/README.md).

Three fresh-profile Fruit Bats WebView explorations now pass five contiguous healthy seconds each, on the expected source with fixed playback ownership. No buffering/stalls were sampled; recovery and the separate native underflow remain open. The normal full queue/settings are restored exactly. [Results](performance-results/2026-10-17-fruit-unpinned-webview/README.md#three-fresh-profile-sustained-explorations).

The subsequent 20-request Monochrome-only WebView batch passes all healthy-window and cleanup gates, with one initial and 19 warm requests. Source-attachment median/p95/worst is 1,457/1,575/3,214 ms, not first audio. Resource sampling and bounded dark full/half-width artist top-track UI checks are retained; exact normal queue/settings restoration passes. The 319.6 MB kernel write counter belongs to WebKit, with file attribution still open. [Measured scope](performance-results/2026-10-17-fruit-webview-20/README.md).

The duplicate resume-position write fix is running after a verified launcher handoff, with exact complete queue/settings equality. Changed positions and explicit saves remain immediate; identical routine writes have a 60-second heartbeat and failed writes remain retryable. [Fix and validation](performance-results/2026-10-17-resume-position-dedupe/README.md).

The coded-local startup PCM calibration initially failed before dispatch. After timer, awaited-guard and pinned-file instrumentation fixes, one retry received the baseline and dispatched play, but fails the unchanged 25ms capture cadence threshold (93ms drift). Exact cancellation, recorder cleanup, volume restoration and final hashes pass; immediate pause acknowledgement was late and is confirmed by a separate subsequent read. The isolated app/sink are removed and the full normal queue/settings snapshot matches exactly after restoration. A further asynchronous-guard run passes exact pause acknowledgement and all cleanup, but still fails the unchanged 25ms cadence threshold at 48ms. Complete normal-state restoration is verified again. This supplies no qualified first-audio measurement. [Failure and scope](performance-results/2026-10-17-startup-pcm-calibration/README.md).

A new integrated search-continuation test proves the filtered/sorted queue snapshot survives later pages, sort changes and replacement queries. A negative control with continuation disabled fails at the intended queue assertion (`Beta` alone instead of `Alpha, Beta`). This is deterministic wiring evidence; broader live acceptance remains open. [Snapshot test](performance-results/2026-10-17-search-continuation-snapshot/README.md).

The native cache handoff/expiry additions now pass the full 1,066-test native suite (21 ignored), and the fresh executable build is byte-identical to `7a43ea0c8def`; all 541 recorded build inputs remain unchanged. The normal app is restored through the verified launcher with exact full queue/settings equality. One isolated single-UUID Fruit request reaches 30s with no post-progress interruption trigger; its private volume restore fails after cancellation removes the owner marker, so it remains unqualified and does not resolve the earlier underflow. [Native result](performance-results/2026-10-17-native-fruit-one-queued/README.md), [cache tests](performance-results/2026-10-17-cache-owner-expiry/README.md).

## Historical checkpoints

Every checkpoint below is retained historical text. Its references to “current”, pending builds, runtime versions and test counts describe that earlier checkpoint, not the current app or source.

## Worktree candidate awaiting full validation — queued playback and verified aliases

Queued playback is now integrated with caller-UUID receipt recovery, independent cancellation ownership, pause/seek/resume preservation, a bounded status outbox and retired-ID replay prevention. Native unavailable handling now traverses exact occurrence IDs finitely rather than stopping after three failures. The cache-only MusicBrainz alias verifier requires the exact canonical cached release occurrence before allowing complete artist-credit aliases. The existing normal app is still on native `ecc2c40a10e8` / frontend `9edf424726b2`; no runtime handoff has occurred.

Eight integrated contract checks, Svelte check (zero errors/warnings), 897 frontend tests, 1,033 native tests (21 ignored), twelve MCP bridge tests and the production frontend build pass. The executable build is in progress; no runtime handoff has occurred. The earlier alias-only run had 1,005 passes, two hand-written provenance fixture failures and 21 ignored; the fixture corrections are applied for this combined run. Focused private queued tests passed but are not substituted for the integrated runs. Live full-album acceptance, intermittent buffering recovery and broader release gates remain open.

Current frontend checkpoint: `9edf424726b2` on unchanged native debug `ecc2c40a10e8`; 879 frontend tests, Svelte0/0, eight contracts and frontend production build pass. Native999/21ignored results remain applicable to unchanged native inputs. Normal queue/occurrences/settings and paused3.287602s restored exactly; latest receipt `.cache/qualification-kaufman-buffering-current/normal-launch/restoration.json`. Buffering UI recovery is implemented/running, with live intermittent-case qualification still open. Exact album traversal reached14 owned failures before the45s legacy automation response timeout; queued contract is private work. Older checkpoints below are historical.

Current checkpoint (2026-10-17): normal app is immutable debug `ecc2c40a10e8`, 999 native tests/21 ignored, 873 frontend tests, 76 extension tests, contracts/Svelte/builds pass. The fresh 30-entry queue and paused position 3.287602s were verified unchanged after isolated test/restoration. Candidate: `.cache/qualification-track-failure-history/candidate.json`; album result: `.cache/qualification-kaufman-ledger-current/`; latest verified normal restoration: `.cache/qualification-seek-output-current/normal-launch/restoration.json`. The album run retained two failures but stopped on a harness WebView/native DTO mismatch; no full album qualification is claimed. Older checkpoints below remain historical.

## Legacy implementation status record

Current checkpoint (2026-10-17): the sole normal GUI is debug candidate `dd9474d3fdf9`, refreshed with the compact Spotify album toolbar. Its native suite passes989 tests (21 ignored), worker suite92, full frontend suite858 across74 files, frontend check0/0 and native/frontend builds. Bounded opt-in WebView event/buffer diagnostics and one-upcoming-item playback-search preparation are built into that candidate. The album toolbar has actual desktop visual acceptance with 34px controls and inline duration fields; the complete100-occurrence/index2 playback state was preserved exactly. Safe source-format projection and redacted candidate-cache tracing are compiled and tested; the normal window keeps those opt-in diagnostics off. The exact Kaufman acceptance remains open: two fresh checks exposed DTO/notice-observation limitations in the harness, with reports retained and no successful album-playback claim.

The full100-occurrence/index2 paused queue is exactly preserved through desktop checks and isolated playback handoffs. Compact History filters are reviewed open/closed at1914×515 and954×1036 with no player-layout findings. The footer artwork fallback renders; the underlying asset-scheme image failure remains unresolved. A separate offscreen WebKit scheme control passes all five cases, narrowing but not fixing the app-specific path.

YouTube test playlist `PLJHq5kM_QbX8` now has complete private empty readback, verified owner and typed additions-only capability. Its complete local import passed. One Magalenha addition is staged in its local working copy and saved proposal, with no removals/reordering/unresolved matches. Its in-memory preview must be refreshed after app restarts and compared to that exact plan before applying. Remote application awaits the user's exact approval; writeback remains off. No creation retry occurred. The previous approved Spotify single-track addition remains verified; further Spotify playlist work is explicitly deferred.

Magalenha V3/V4/V5 each prove five seconds of sustained WebView startup clock progress, with ready-source observations around8.1–9.0s. V4 proves95% seek target landing and subsequent clock advance to the end, but readyState2/statusbuffering persist, so the ready/playing seek gate still fails. Physical audibility, decoded-output proof and p95 remain unqualified. V5 bounded event diagnostics record full buffered ranges while readyState2/statusbuffering persist after seeking. They do not establish the decoder/CDN cause. Sasa now passes the accepted exact-version unavailable/notice/advancement branch on the current build; successful Sasa audio remains unavailable in tested sources. A local60-second WAV control now reproduces the same active-seek readiness symptom without a provider request; its five-second initial clock/readiness gate passes, while the seek gate fails. An after-end paused seek never reached resume, so no workaround is qualified. The new bounded failure-history transport is integrated in source and under full verification, not yet in the running executable. Public releases remain held.

See [Magalenha evidence](performance-results/2026-10-17-magalenha-source-review/README.md), [desktop evidence](performance-results/2026-10-17-desktop-history-navigation/README.md), [YouTube readback](performance-results/2026-10-17-reviewed-empty-playlist-creation/README.md) and [todo](../todo.md).

All checkpoints below are historical; they do not describe the currently running instance.

## Earlier verified checkpoint — 2026-10-17

This checkpoint supersedes older runtime statements below. The sole normal GUI is debug candidate `855b9a7d2074`; its complete fresh queue/settings snapshot was preserved across restart (20 occurrences, index 5, paused at exactly 63.804743 seconds). The reviewed-intent build passed 960 native tests (21 ignored), 143 provider-worker tests, 11 MCP proxy tests and eight contract checks. The restored live Spotify summary diagnostic returns 99 rows against a reported 179; completeness remains unqualified.

Both exact additional private-empty playlist creations were explicitly approved and dispatched once. Spotify returned HTTP 429 with an unknown creation outcome. YouTube returned playlist `PLJHq5kM_QbX8` with the exact title, but readback reported null owner, privacy and total, with `complete: false`. Receipts and one-shot markers are retained. Do not retry either creation. Successful track writeback is still unverified. [Approved creation evidence](performance-results/2026-10-17-reviewed-empty-playlist-creation/README.md).

The current worktree secret-pattern audit passes, with 10 scanner tests and 176 related JavaScript tests passing. This does not qualify history or packaged artifacts. The release storage benchmark compilation was interrupted (exit 143), then completed from 206 verified unchanged staged inputs. Its frozen release candidate passed the actual-Store benchmark; see the [release Store report](performance-results/2026-10-17-text-index-release-store/README.md). This does not qualify whole-app behavior or authorize enabling the experimental indexes.

The exact combined YouTube query failure was reproduced: implicit videos-only search returned unrelated rows, while the ordinary results URL found the expected video second using the same bundled helper. The browsing fix is implemented. The first native run passed 961 tests and exposed two invalid-ID fixtures, which were corrected; the combined-query native retry is active and has not passed yet. [Evidence](performance-results/2026-10-17-youtube-query-filter/README.md).

## Earlier same-day source checkpoints — superseded by the latest checkpoint above

At this earlier checkpoint, the normal GUI was closed after a launcher/source-age mismatch, and the matching executable build was still running. The current checkpoint above supersedes that runtime state and records the preserved GUI and completed build. The earlier source validation had passed 960 native tests (21 ignored), 11 authenticated stdio proxy tests, and eight contracts.

At that earlier checkpoint, reviewed verification-playlist intent had not yet been native-compiled and no preview or apply had run. The source binds exact private/empty UUID-title approval, account/revision/generation, and prior legacy/new unknown creation history; admission precedes one-shot consumption, and known IDs are persisted before readback. The latest checkpoint above records the completed reviewed-intent build and the two one-shot creation outcomes. Neither attempt should be retried; successful track writeback remains unverified.

Finite unavailable-queue continuation is merged and running: each remaining supplied occurrence is attempted once without failure repeat/radio, replacing the three-skip ceiling. All328 App/queue tests, Svelte0/0 and production build pass; the fresh full queue/settings snapshot is equal across restart. A fresh isolated fifth-occurrence run now passes: four failed occurrences of the same exact Sasa recording advance to the verified local fifth occurrence with 5.201251 seconds of actual WebView progress and an owned actionable notice. The normal full fresh state remains equal after restoration. Live album qualification remains open. [Report](performance-results/2026-10-17-finite-unavailable-queue/README.md).

The fresh exact Kaufman19 follow-up reached indices0–3 before the three-skip limit stopped playback; no ready media was observed and the original request rejected. The remaining15 tracks were not tested. The exact failure screenshot was reviewed and the normal full queue/settings snapshot is preserved after restoration. Source matching and automatic continuation past repeated unavailable entries remain open. [Follow-up](performance-results/2026-10-17-kaufman-opener-followup/README.md).

The source now includes a bounded read-only Spotify full-profile summary diagnostic. Root reran 56 worker, 17 adapter and 945 native tests successfully (21 native tests ignored). The initial sandbox native run failed 28 socket/subprocess permission boundaries; the required-access rerun passed in 16.58s. Eight contract checks also pass. It does not change production profile pagination or establish completeness. The tested successor built in 2m00s and is now running; the full fresh pre/post snapshot is equal. Live comparison found the exact same 99 ordered playlist IDs in summary and four paged reads, despite differing reported totals (summary 179; page totals 110/110/109/90). The alternative route exposes no additional playlists in this observation, so production browsing is unchanged and completeness remains open. [Finite diagnostic report](performance-results/2026-10-17-profile-summary-diagnostic/README.md).

The current frontend adds context-menu keyboard focus, bounded search-album Back/Forward/expiry recovery, and unavailable-initial-queue advancement. The final371-test batch,57 post-compatibility menu tests, Svelte0/0 and production build pass. Full fresh state is preserved after relaunch; actual desktop layout findings are empty. Live album playback remains unqualified. [Bounded report](performance-results/2026-10-17-menu-focus-album-history/README.md).

The isolated 50k text-index pilot completed title-ascending full-ID parity and 20 paired page reads at each of two offsets, then was intentionally stopped because repeated full-order pagination made the remaining matrix infeasible within its deadline. The revised same-Store debug harness passed all phases in 806.52s: 160 paired reads, full pre/post-update parity, 200 metadata edits and reopen. The release-mode same-Store harness also passed all phases in 171.61s. Across both builds, index creation and the measured edit/backfill batch carry added cost; the proposal remains isolated/default-off, and production indexing is unchanged. The debug and release reports use different temporary-storage environments, so their absolute timing difference is not attributed to build optimization alone. [Complete release storage report](performance-results/2026-10-17-text-index-release-store/README.md). [Complete debug storage report](performance-results/2026-10-17-text-index-actual-store/README.md). [Partial evidence](performance-results/2026-10-17-text-index-partial/README.md).

The sole normal GUI is candidate `fc32d24cf4e5` (full SHA `fc32d24cf4e55de8c1cd5e0d6c13b3103eda95732d92b3e15c0bab8d5c4a1af9`, launcher 59089; authenticated frontend mounted with 94 tools). Its fresh full snapshot exactly matches the pre-run snapshot: 13 queue occurrences, index 6, paused at 29.087175s, volume 0.8, repeat off and shuffle off. No manual queue restoration was used. [Preservation proof](../.cache/qualification-finite-queue/main-restore/after-private.json); [candidate](../.cache/qualification-profile-summary/candidate.json).

Current native checks pass 944 tests (21 ignored); reviewed-writeback MCP proxy tests pass 6 and contract checks pass 8. The latest frontend passes 307 App tests, Svelte 0 errors/0 warnings and production build. Paused artist/release indicators and truthful repeated-cursor messaging are merged. Both persisted provider sessions passed live read-only playlist requests after restart (Spotify 47 with a next cursor; YouTube 61 without completeness proof); live writeback remains unqualified.

The persistent unavailable-track notice fix passes a fresh isolated WebView run: the original queue request is accepted, exact unavailable Sasa advances to the verified local successor, and 5.378 seconds of actual media progress is observed. The corrected owned accessibility reader confirms one persistent actionable notice with zero text-extraction errors. Root reviewed the 954×1036 screenshot; the reason wraps and playbar controls do not overlap. The earlier failed reader report remains retained. This is one exploratory queue-resilience run, not successful Sasa playback, audible/recording identity or broad UI/performance qualification. [Bounded report](performance-results/2026-10-17-persistent-playback-notice/README.md).

The duration-input guard passes 55 direct worker tests and 942 native tests (0 failures, 21 ignored); the candidate build passes in 2m08s. This validates scalar parsing and per-row protection for manual, chapter-derived and known durations, but does not claim a live metadata-repair run. [Finite report](performance-results/2026-10-17-duration-guards/README.md); [build log](../.cache/duration-guards-native-build.log).

Search-history, occurrence selection and chapter-rendering changes pass 289 App tests, 50 focused SearchPanel/cache/scope tests, Svelte 0/0 and the production build. Search-to-artist history restores controls, pages/cursor and scroll without refetch in the tested bounded flow; bounded search-to-album restoration now also passes, while global Back/Forward remains open. Explicit occurrence selection starts at its range boundary, Resume preserves position, and the tested double-advance race is fixed. [Finite report](performance-results/2026-10-17-search-history-occurrence/README.md).

The import terminal-race and unavailable-track notice fixes pass 311 App tests across two files, Svelte 0/0 and the production frontend build. The Kaufman live attempt failed before media progress because the first queued track, `Vgradena`, was unavailable; it never reached `Duda`. The exact 19-track fixture and clean isolated shutdown are recorded separately, and the album playback request remains open. [Import/notice report](performance-results/2026-10-17-import-terminal-and-skip-notice/README.md); [Kaufman attempt](performance-results/2026-10-17-kaufman-live-failure/README.md).

A live exploratory reserve check on `51f9347cd3e6` passed the strict disabled control (2,320ms first mixer; five seconds after 95% and 1% seeks). One enabled pilot started at 2,026ms and released 180,224 samples versus a 176,400 target, but failed continued playback after the 95% seek; observed seek was 6,422ms with 36,864 samples produced/consumed. This is n=1 per arm, not causal or reliability evidence. Reserve remains default-off and disabled in the normal profile. [Finite report](performance-results/2026-10-17-reserve-live-exploratory/README.md).

Artwork cache and album-route regressions pass 289 App/artwork tests across two files (40.83s), Svelte check 0/0 and production frontend build. Nine cache and three App cases verify protection of the active footer and Details URLs (at most two total) within a three-image read cap without metadata mutation. The actual-window review showed 298×300 footer artwork via data fallback, footer height 98.5px and zero layout findings. Four frontend cache-related files changed; native source is unchanged from pin 513. This does not prove the asset protocol fixed. The actual Kaufman live album attempt failed before reaching `Duda` because the opener `Vgradena` was unavailable; exact playback remains unverified. Default-WebView Sasa also remains open. [Finite report](performance-results/2026-10-17-artwork-album-regression/README.md). [Tests](../.cache/artwork-album-final-tests.log); [Svelte](../.cache/artwork-album-final-svelte-check.log); [build](../.cache/artwork-cache-root-frontend-build.log).

A four-file SourceLink hit-target/contrast refinement is now merged: the optional icon is inside the full-width padded button, existing opener/range/callback behavior is preserved, and light danger text has 6.97:1 contrast on white. Focused frontend tests pass (52 across four files), Svelte check is 0/0, and the production build passes; a bounded post-patch dark-theme Library More review passes at 1914×515 with no footer overlap; the full queue/playback snapshot remained unchanged. Live light-theme and broad desktop acceptance remain open. [Finite report](performance-results/2026-10-17-menu-source-hit-target/README.md).

The latest context-menu refinement passes 51 tests across four frontend files, Svelte check 0/0 and production build. Sixteen component captures cover track, playlist, queue and list menus in both themes. The actual normal-WebKit Library More menu opened and closed above the player footer with no layout findings; lower actions scroll within the menu. Queue snapshot remained unchanged. Broader desktop UI remains open. [Bounded report](performance-results/2026-10-17-context-menu-polish/README.md).

A WebView Sasa diagnostic run observed 5.091s wall/5.066s media on the initial entry and 5.044s wall/5.216s media on the following local occurrence, with a 6,284ms transition upper bound. One initial stale media sample was rejected; none were observed after progress. The automated result is still unqualified because the AT-SPI notice reader returned false, although a separate privately reviewed capture visibly showed the skipped-version reason and Choose source action while the next local occurrence played. An earlier attempt failed before media evidence; another preregistration profile exited with an empty log while EDQUOT was observed, with cause unknown. These observations do not qualify default-WebView Sasa or establish cold-start, audible or broad playback. [Bounded diagnostic](performance-results/2026-10-17-sasa-native-queue-advancement/README.md).

Three focused repetitions of the ignored cached-reader/decoder seek fixture passed: immediate decoded consumption, controlled range starvation after a 1.5s held-input interval, then five seconds of recovery. The WAV payload is zero PCM; this is not audible/live playback or production-fix proof. [Finite report](performance-results/2026-10-17-cached-seek-causal/README.md). The original 5k in-memory text-sort experiment completed four single-pair cases in 9.351s, but every plan retained a full temp sort. A later isolated reverse-binary benchmark completed 50k rows with exact full-order parity and no temp sort across four cases (three warmups and 20 pairs each); its separate 5k test used one pair per case. This measures only in-memory ordering. A default-off integration proposal is staged, not merged, and production does not install the indexes. [Original result](performance-results/2026-10-17-text-sort-5k-exploratory/README.md); [reverse-binary result](performance-results/2026-10-17-text-sort-reverse-index/README.md).

A matched configured-root 50k startup-only comparison completed: historical baseline full wait 31,066.3ms vs current candidate 22,759.3ms; endpoint registration 15,340ms vs 9,270ms; asset grants 9,863ms vs 4,438ms. Both used 100 pages and a 20-second registration allowance; current classified 50,000 entries with zero file calls and reused all existing grants. This is n=1 per build, many source changes differ, and it excludes page reads and projection readiness; no broad performance conclusion. [Finite comparison](performance-results/2026-10-17-configured-root-startup/README.md).

Artwork cache and album-route tests pass 289 App/artwork tests across two files; Svelte check is 0/0 and frontend production build passes. The broader asset-protocol failure is not established as fixed, and the focused cache behavior leaves metadata unchanged.

The 50k asset-backed baseline passed 140 reads across seven scenarios, with one exact continuation per scenario and middle/tail checks for six longer-query cases; the short query has no middle/tail check. Projection readiness took 137885.6ms outside the timed reads, p95 ranged 232.6–1599ms, and startup phases recorded Store open 858ms, Downloads backfill 21385ms, asset grants 6157ms and endpoint registration 28473ms. [Baseline matrix](performance-results/2026-10-17-startup-trace/asset-matrix-50000-ready-baseline.json); [phase trace](performance-results/2026-10-17-startup-trace/fresh-50000-baseline-phases.json).

The asset-scope candidate classifies paths against the initial Tauri scope before granting initially denied paths. The 921-test native suite and nine startup-harness tests pass, and candidate `45eca63f814d` builds. However, one fresh 50k startup-only run shows no benefit on the external fixture: 50,000 grant calls, zero reused grants, 6,933ms asset grants and 11,257ms endpoint registration, versus 6,305ms and 10,930ms on the prior single optimized run; harness waits were 15,758.5ms vs 15,257ms. The runs differ in workload (startup-only versus page matrix), so refine before performance promotion. [Finite startup result](performance-results/2026-10-17-asset-scope-followup/README.md); [native tests](../.cache/native-scope-paced-text-index-tests.log); [focused startup harness](../.cache/library-startup-only-tests.log); [build](../.cache/native-asset-scope-build.log).

Current source pin `513` passes 941 host-access native tests (0 failures, 20 ignored); that suite covers the production worker/hash changes, while the ignored seek fixture is reported separately. The earlier sandbox run had 28 environment-denied bind cases. RemotePlaylistDetail filtered-availability tests pass (25), Svelte check is 0/0, and the frontend build passes. The reverse-binary benchmark completed, but its default-off Store integration is only staged, not merged, and no production index is installed. The current native candidate build is complete. The bounded nine-step queue sequence covers selection, arrow movement and drag; synthetic DOM events preserve exact occurrence order, current chapter position and controls after a one-second settle, with no audio commands. Three reviewed 1914×515 captures show no onboarding modal, the correct 0:30 chapter display and no overlap (zero layout findings). These checks do not include OS-level input or broad playback qualification. The initial JSON key-order harness failure is retained; corrected semantic comparison passes. Isolated shutdown had no remaining child processes or forced kills. No main-profile reserve flag or live writes/retries. The bounded interaction requirement is recorded in `todo.md` #done, while broader UI gates remain. [Interaction proof](performance-results/2026-10-17-queue-reserve-interactions/README.md); [native tests](../.cache/native-queue-provider-receipts-tests.log); [frontend tests](../.cache/queue-visual-followup-root-front-tests.log); [Svelte check](../.cache/queue-visual-followup-root-svelte-check.log); [frontend build](../.cache/queue-visual-followup-root-frontend-build.log); [source pin](../.cache/queue-visual-followup-runtime-source-pin.json).

The latest host-access native suite passes 941 tests (0 failures, 20 ignored; 14.98s). A prior sandboxed attempt passed 913 with 28 environment-denied bind cases; those are retained as sandbox limitations, not source regressions. The current RemotePlaylistDetail availability filter has 25 focused tests, Svelte check 0/0 and frontend build passing. Candidate `8ed8ca63de48` is stopped after the configured-root baseline; the app/artwork frontend checks pass. The 5k sort result above does not promote broad sorting; playback and cross-view sort acceptance remain open. [Native host log](../.cache/native-profile-hashes-instrumentation-host-tests.log).

The managed Downloads backfill candidate `a1b8013b10e1` passes 919 native tests, 0 failures, 16 ignored (17.12s), and its development build completed in 2m01s. The optimized 50k run completed with zero failed reads using the default 20-second registration wait (baseline used an explicit 180-second wait). Its retained phase trace measures Downloads backfill at 3,642ms versus 21,385ms baseline and endpoint registration at 10,930ms; matrix startup wait is separately 15,257ms, so this trace does not fully decompose that wait. The baseline profile was hash-verified and archived out of `/tmp` to recover space; no user data was touched. Seven Python benchmark checks and three pure-Node boundary checks are wired into Linux CI. [Optimized matrix](performance-results/2026-10-17-managed-download-backfill/asset-matrix-50000-optimized.json); [optimized phase trace](performance-results/2026-10-17-managed-download-backfill/optimized-startup-phases.json); [comparison](performance-results/2026-10-17-managed-download-backfill/README.md); [tests](../.cache/native-managed-download-backfill-tests.log); [build](../.cache/native-managed-download-backfill-build.log); [archive proof](../.cache/qualification-startup-trace/baseline-profile-archive.json).

Source review found a credentialed Qobuz Next.js server path pinned for identification only; host provenance, playback credentials and playable audio are not established. [Backend investigation](provider-backend-investigation.md).

A read-only profile hash-overlap audit returned 99 unique rows from standard offsets, with no duplicates or new rows from boundary probes. Terminal reads repeated ordered hash anchors at offsets 0/60/89/99; empty HTTP 200 bodies at offsets 100/110/120 had no decoded total and do not verify exhaustion, deletion or privacy. The page at offset 60 returned 29 rows, but offset 89 overlaps its final row, so the requested 30-row cursor span to 90 is appropriate; no cursor change is indicated. Counts vary by offset; this is not a completeness claim. No provider, queue or audio writes occurred. [Sanitized audit](performance-results/2026-10-17-profile-hash-overlap/README.md); [prior profile inspection](performance-results/2026-10-17-spotify-profile-adjacent/readonly-summary.json).

Fruit input starvation, playback/reliability, broader provider completeness/writeback, whole-app resource, larger-library and release qualification remain open. The live Library More-menu anchor is validated; the cached-MCP 1k/5k/20k matrices remain separate synthetic workloads from 50k. Public releases remain on hold.

## Earlier normal runtime checkpoint —941a before navigation frontend reload, 2026-10-17

Source ahead of the protected frontend now bounds existing WebView Resume at20s and cancels it on explicit pause/new seek/source change. Seek completion no longer creates another seek intent, avoiding delayed-event cancellation of a newer Resume. Sidebar playlist opening now records only its actual origin route; a failing-baseline/passing-fix test restores the prior Library filter/sort. A queue-panel regression preserves duplicate entry IDs through keyboard movement. Merged334 UI tests, Svelte0/0 and frontend build pass; live relaunch remains pending. [Checks](performance-results/2026-10-17-webview-media/resume-navigation-source-checks.json).

Sole normal window941a59f36128 (PID504134, launch96083,92 mounted tools) uses default WebView audio. Fresh13-entry queue/index6/paused29.087175 and published controls are preserved; legacy entries migrated to unique occurrence IDs. No manual queue restoration, seek or resume. Single-entry isolated restart retains its ID and controls; main persistence and duplicate restart acceptance remain open. [Preservation](performance-results/2026-10-17-webview-media/main-941a-preservation-proof.json), [isolated restart](performance-results/2026-10-17-webview-media/isolated-941a-occurrence-persistence.json).

Local360s WebView playback passes paused10s/250s seeks and five seconds of progression after each. Kaval has two complete exploratory passes and one10s Resume-observer timeout after startup/seek passed. Observed Monochrome candidate188668061304786944 has zero required headers; prior network failure did not recur. This establishes neither exact recording correctness nor reliable/performance-qualified playback. The unbounded HTML audio Resume path is being fixed separately. [Local](performance-results/2026-10-17-webview-media/local-941a-occurrence-pass.json), [first Kaval pass](performance-results/2026-10-17-webview-media/kaval-941a-exploratory-pass.json), [timeout](performance-results/2026-10-17-webview-media/kaval-941a-repeat2-resume-timeout.json), [third pass](performance-results/2026-10-17-webview-media/kaval-941a-repeat3-pass.json).

Actual1914×515 footer remains98.5px, findings=[], and root visually checked/closed the refreshed track menu above the footer. Artwork data fallback displays the cover; original asset probe still fails. Main launch initially failed due temporary Vite quota; only inactive generated caches/tones were cleaned. Launcher now uses workspace cache scratch, and verifies Vite-transformed modules plus the authenticated mounted frontend. [Geometry](performance-results/2026-10-17-webview-media/main-941a-layout.json).

Merged source checks:888 native/16 ignored,306 UI, Svelte0/0,8 contracts,6 MCP pass; production frontend and formatted native builds pass. Ignored benchmark helpers now use correct even medians and retain raw order; actual benchmark runs remain pending. The888-test run precedes formatting-only and ignored-helper changes; the final executable includes those changes. [Checks](performance-results/2026-10-17-webview-media/recovery-merged-source-checks.json).

At this checkpoint, source tests/builds and captures made on prior runtimes did not establish behavior in the then-live window.

## Previous normal runtime —39b6,2026-10-17

Sole normal window is39b6ed9626f2 (launch26561,92 mounted tools), using default WebView audio. Fresh ordered queue13/index6/paused29.087175 and controls match before/after the isolated handoff, without manual restoration, seek or resume. Occurrence-ID arrays were empty before and after; their equality did not qualify occurrence identity. [Preservation](performance-results/2026-10-17-webview-media/main-preservation-proof.json).

The local HTTP endpoint now streams full/range responses in64KiB chunks rather than clipping at8MiB. In an isolated functional check, the exact synthetic WAV reports360s, preserves paused10s and250s seeks and sustains5s media progression after both. Prior e268 reported190.216870749s, exactly the clipped payload. Physical audibility, startup latency and performance percentiles were not measured. [Reproduction](performance-results/2026-10-17-webview-media/local-truncation-reproduction.json), [local pass](performance-results/2026-10-17-webview-media/local-stream-webview-proof.json).

Kaval with Monochrome-first preferences initially reports131.726s versus131s catalogue metadata, then fails after2.222395s with a network error and enters idle. The selected provider/recording identity is not proved. The earlier observer-gap/race failures are retained; no provider playback pass is claimed. Recovery's repeated-candidate behavior and dropped transport headers are under investigation. [Failure](performance-results/2026-10-17-webview-media/kaval-webview-failure.json), [state](performance-results/2026-10-17-webview-media/kaval-webview-failure-state.json).

Current desktop1914×515 footer is98.5px with no geometry findings. Cached display fallback shows the current cover at300×298; the asset-scheme probe still fails, so the protocol itself is not qualified. Actual track-menu capture shows aligned SVG icons and no footer overlap, then the menu was closed without playback changes; private captures remain under`.cache/private-ui-checks/39b6-*.png`. [Geometry](performance-results/2026-10-17-webview-media/main-layout.json).

The new YouTube own-library inspector succeeds with the persisted confirmed account after relaunch:61 rows, terminal observed, no direct/wrapped cursor, complete unknown. This does not prove whole-library absence/completeness or verification-playlist creation outcomes. No provider writes occurred. [Bounded inspection](performance-results/2026-10-17-webview-media/youtube-library-inspection.json).

## Historical merged-source checkpoint — before941a launch

The shared source adds persistent per-occurrence WebView queue IDs and consistent TrackList overflow-menu styling. The merged validation passes 306 serial UI tests, Svelte check0/0 and the production frontend build. Synthetic TrackList desktop/half-screen renders show no horizontal overflow and16px footer clearance at707px. At this historical checkpoint the source had not been relaunched; see current941a evidence above for the subsequent bounded checks.

The host-access native suite passes888 tests, zero failures,16 ignored (17.00s), including the large-file endpoint and recovery/privacy regressions. A restricted run passed866 tests but denied local fixture/process operations in22 tests; the host-access result is the applicable full-suite count. New local HTTP/Pulse native regressions compiled after correcting six test-only HeaderValue/String assertions. Nullable-duration diagnostics are merged and pass the native suite. Bounded source recovery is merged: failed provider/ID pairs are request-local, attachments are capped at two per operation, and only five seconds of sustained playback without a seek renews the recovery budget. Finite source/header-policy diagnostics retain failed-source identity without exposing URLs or credentials. Seven staged regressions passed. Explicit artwork retry remains staged. None of these changes has been established in the live `39b6` runtime.

## Previous normal runtime — ae2636, 2026-10-17

Pending candidate: deferred paused/not-ready and ended seeks preserve operation context and the requested Resume position; skipped-source failures remain visible during advancement. Scoped YouTube own-library continuation/inspection, bounded WebView media-element inspection, explicit recorder/recognizer cleanup and chapter-scoped stale recognition checks are in source. Final native suite passes884/16 ignored (18.69s; compile3m30s), MCP5, contracts8, Svelte0/0 and frontend build5.16s. Serial full App/menu verification passes261 checks (254 App, seven menu;30.26s test time/41.73s total); the executable build is running. The earlier2cda09 candidate is obsolete and never launched; current main remains ae2636. Additional Pulse-inspector cancellation and explicit artwork retry fixes are staged outside the shared tree, not included or qualified. [Pre-chapter checkpoint](performance-results/2026-10-17-webview-media/pre-chapter-source-checks.json).

Context menus now use consistent SVG icons and grouped36px rows, omit unsupported source actions and close after a successful source open while retaining errors. Fresh synthetic real-component desktop/half-screen renders fit within the viewport6px above a90px player; the short-height track menu scrolls. These renders do not establish the current live app's menu behavior. [Visual report](performance-results/2026-10-16-context-menu/current-captures/context-menu-updated-visual-report.json).

Sole protected main is ae2636f63df1, launch53949,90 mounted tools, using WebView audio (read-only MCP state has the frontend media shape, and automation routes native-enabled states to a distinct explicit native snapshot). All native mixer checks below were explicitly isolated and do not qualify normal-main WebView playback. Its historical305-file native pin is retained; the shared source has newer changes. the final193-file frontend pin is `.cache/ae2636-directory-webkit-frontend-source-pin.json` (Directory CSS correction only after the250-test source run). Fresh complete queue13/index6/paused29.087175, full tracks, occurrence IDs and controls match both the isolated handoff and frontend reload; no restore, seek or resume. UI is returned to Library. [Preservation](performance-results/2026-10-17-directory-identification/main-preservation-proof.json), [footer geometry](performance-results/2026-10-17-directory-identification/main-layout.json).

- Local native startup and paused seek/resume pass five-second mixer checks; startup76ms. [Local](performance-results/2026-10-17-directory-identification/local-native-proof.json).
- Pinned Fruit startup sustains5.02s at181.4s after2128ms. Its paused seek hits the not-ready path: position/serial change but generation and seek context do not; overall check remains failed. A separate Resume sustains9.10s but has no seek timestamps. A grounded fix is staged at `/tmp/burrata-deferred-paused-seek.patch`. [Fruit](performance-results/2026-10-17-directory-identification/fruit-native-proof.json).
- One muted ordinary PCM recognition request reaches the pipeline but fails (1859ms); its reason was not retained, so no recognition-success or cleanup qualification. The corrected dedicated UUID cancellation test observes the actual SongRec child in isolated TMPDIR and confirms child reaped plus WAV removed before cancelled result, same occurrence and playback continuing. Initial wrong-TMPDIR observer failure remains retained. [Recognition](performance-results/2026-10-17-directory-identification/ordinary-identify-proof.json), [corrected cancellation](performance-results/2026-10-17-directory-identification/cancel-identify-proof-corrected.json).
- Exact Sasa middle queue advances to the correct duplicate occurrence at index2 and sustains5.23s, with a finite NoPlayable reason observed. The brief middle state timestamp was missed, so deadline/overall qualification remains failed. Persistent skipped-reason UI is staged separately at `/tmp/burrata-skipped-failure.patch`. [Advancement](performance-results/2026-10-17-directory-identification/album-native-proof.json).
- Opt-in Ergo Bandcamp page/source reaches actual mixer in1793ms, declared310.254s/playback310.253333333s; paused seek preserves occurrence and post-seek mixer sustains5.01s. Decoder seek-first32ms versus receipt-first1377ms includes paused time. This is one exploratory pass; experiment remains off in main, and no broad/default source, percentile, expiry or platform qualification is claimed. [Ergo](performance-results/2026-10-17-directory-identification/ergo-native-proof.json).
- Actual directory15-file view at1914×515 shows full Filename text after the WebKit padding fix, bounded34px controls and no footer overlap. AT-SPI reads disabled Date added/Plays/Views options, while present fields remain enabled; no sort/scan/play actions occurred. Final CSS check0/0, build4.89s; private before/after screenshots are in `.cache/private-ui-checks/ae2636-directory{,-webkit}.png`. [Sort states](performance-results/2026-10-17-directory-identification/directory-sort-accessibility.json). Broader folder/keyboard/stale and50k live acceptance stays open.

All live playback checks were muted and isolated; graceful shutdown leaves zero active owned descendants. No provider writes occurred. Sasa correctness, ordinary/DJ/concert identification, not-ready paused seeks and the broad matrices remain open.


## Source and historical checkpoint — 2026-10-17

The historical protected main was ef76646fbc06, launch55409, with89 mounted tools. The new source batch adds request-scoped native audio-identification cancellation, bounded SongRec child reaping, shutdown admission/drain, Unix SIGTERM cleanup, and stale queued local-file reconciliation. Missing files can select another registered asset or continue through the existing permitted source policy; explicit chapter ranges retain their source identity. App integration passes all 250 tests (34.17s test time, 42.18s total); the unchanged compiled native test artifact passes 863 tests, with 16 ignored (13.82s), on the host. The restricted run retained 23 local-fixture failures (loopback/subprocess restrictions), and the initial compile errors were corrected before the host run. Logs: `.cache/native-cancel-stale-assets-full.log`, `.cache/native-cancel-stale-assets-full-retry.log`, `.cache/native-cancel-stale-assets-host-full.log`. Native debug executable build passes in 2m05s. Immutable candidate: `.cache/immutable-debug-binaries/identify-stale-assets-9bf49702384d/burrata` (SHA256 `9bf49702384df56a6f525cbb5078c44efd35ce74a9b266a928fc020cc3742050`), with matching 302-file native and 193-file frontend pins. That candidate was not launched because the embedded-source guard caught the later owner-cursor update; ef766 was rebuilt and launched instead. Broader acceptance remains open and no additional todo items have moved to done.

Context-menu regression checks pass 6 tests across TrackMenu, PlaylistMenu and menu geometry (`.cache/context-menu-current-regression.log`). The deferred native-identification UI cancellation regression passes with the same request ID and rejects a late result without tag or playback writes (`.cache/native-identify-cancel-app-focused.log`). Svelte check remains zero errors/warnings for the new UI wiring; production build passes in 4.90s and all eight command/schema contracts pass. A fresh read-only current-runtime geometry check reports no footer findings at 1914×515 (`.cache/native-identify-current-runtime-layout.json`). Live ordinary/DJ/concert recognition and playback qualification remain open.
### Isolated ef766 exploratory checks

The newer embedded owner-cursor guard passes all 43 mocked worker tests on the host (`.cache/spotify-ownerless-worker-host-full.log`) and the native debug build passes in 1m57s. The launcher's stale-embedded-source rejection of 9bf497 was retained; no guard was bypassed. Sole isolated ef766 starts with a verified mounted frontend. Local startup sustains 5.09s of mixer progression (first audio93ms, duration360s), and pinned Fruit Bats sustains5.25s (first audio3443ms, expected provider/ID and declared/playback duration181.4s). Both paused seeks preserve occurrence, advance generation/serial, stay paused at10s. Both overall runs remain **failed** because resume never publishes seek-first-audio metrics, although the playhead progresses; the source cause is confirmed and a fix is staged. This is exploratory evidence, not p95 or independently verified recording identity. [Local proof](performance-results/2026-10-17-identification-cleanup/local-native-proof.json), [Fruit pinned proof](performance-results/2026-10-17-identification-cleanup/fruit-pinned-native-proof.json).

One ordinary native identification attempt on actual muted Fruit audio was rejected in18ms, preserved queue/session and left no new temporary WAV. Static inspection confirms the ordinary Identify gate uses the volume-dependent audible flag even though its PCM tap is before the sink. Muted recognition and explicit live cancellation are not qualified; a volume-independent playing-state correction is staged. [Recognition attempt](performance-results/2026-10-17-identification-cleanup/ordinary-identify-proof.json).
The metadata-only default Fruit request sustains5.06s at181s after9171ms startup, but selects a different provider/ID, so the Monochrome-control identity gate fails. This proves neither an exact recording match nor a Monochrome default-path pass. [Default proof](performance-results/2026-10-17-identification-cleanup/fruit-default-native-proof.json). Isolated graceful TERM exits with all three owned descendants gone and no forced kill. Normal main ef766 is returned with exact fresh queue/occurrences/index/pausedposition/controls equality and no queue restore or audio commands. Footer geometry findings remain empty. [Shutdown](performance-results/2026-10-17-identification-cleanup/isolated-shutdown-proof.json), [main preservation](performance-results/2026-10-17-identification-cleanup/main-preservation-proof.json), [layout](performance-results/2026-10-17-identification-cleanup/returned-main-layout.json).

Next source batch is merged, not live: paused-seek timing, volume-independent ordinary PCM Identify, finite UUID-scoped MCP cancellation (native and WebView pre-cancel tombstones), and batched directory-wide availability. Default filename browse stays page-only; cancellable summary batches at most200 paths, and filtered global browse returns availability before pagination. Directory17 focused tests and Svelte0/0 pass; MCP4 and contracts8 pass. The coordinated suite passes native870/16 ignored (15.78s), App250 (33.39s test/44.93s total), Directory17, MCP4, contracts8, Svelte0/0 and frontend build6.91s. Native debug executable build passes2m09s; immutable candidate `.cache/immutable-debug-binaries/directory-identify-seek-ae2636f63df1/burrata` SHA256 `ae2636f63df1734e836c88a7fed20b41b7318cee8e12f4085c7deb67eadfb1bd`,305 native/193 frontend source hashes match. It is not yet launched; main ef766 stays paused. Live qualification remains open. [Source proof](performance-results/2026-10-17-directory-identification/source-checks.json).




Historical checkpoint — 2026-10-16: then-sole protected main 74788300db6e has the final frontend reload, launch 38374, with 89 mounted tools. Fresh 13-entry/index 6/29.087175 paused queue, occurrence IDs and controls match before/after and after UI checks; no resume or seek. Actual 1914×515 search shows one title, full source/Select text, footer 98.5px and findings=[]. [Current safe UI proof](performance-results/2026-10-16-context-menu/compact-search-747-live.json).

Validation: native 840/16 ignored remains the tested binary baseline. Final App 249 passes in 37.25s, SearchPanel 38 tests pass, final Svelte check is 0/0 and CSS-only WebKit production build passes in 5.91s. The 193-file launch pin is `.cache/compact-search-webkit-frontend-source-pin.json`; future Unix SIGTERM source work is not in this binary. Isolated real-component ArtistPage, SpotifyArtist and ReleaseView renders at 707/1914px all show bounded 34px controls, legible dark text on the green primary button and source-ordered 14-track Save callbacks. Native Store 3, all-source filtered-save App integration and permission checks support the exact album Save requirement; this is not broad provider-writeback acceptance.

Native Kaval catalog artist ID/count and actual explicit Find on MusicBrainz artist-only search/user selection/Back Spotify/Back Library pass. [Catalog proof](performance-results/2026-10-16-context-menu/spotify-catalog-credit-747-live.json), [provider-switch proof](performance-results/2026-10-16-context-menu/artist-provider-switch-747-live.json). Exact album control sizing, green/white contrast, album Save, artist-provider navigation and compact search requirements are moved to done with these bounded checks. Queue interaction, recognition, real-track playback/matching, provider completeness/writeback, broader UI and performance remain open in [todo](../todo.md).

Root removed six old orphan helpers using exact process guards and no active clients; follow-up confirms those six absent and current tree 3. The old sample was 50.66 MiB PSS plus 558.82 MiB swap; this cleanup observation is not resource/performance qualification. No argv/environment or private screenshots are published here.

The prior todo is archived byte-for-byte in [todo history](todo-history-2026-10-16.md). Earlier checkpoints below are historical; their then-current process IDs, pending jobs and counts are not present runtime claims.

Historical availability-offset source checkpoint passed806 native tests,0failures,16ignored, ten focused checks,186Node tests, Svelte0errors/warnings and production build5.43s. The earlier main988a launch preserved paused state exactly; the subsequent isolated handoff is recorded above. The selected YouTube-playlist acceptance below is historical61b5 evidence; broader provider acceptance remains open. Full frontend run74148 passes **610tests/62files in94.70s**; all recorded source hashes match after the run. The earlier599-test baseline remains historical. [Verified full-suite manifest](performance-results/2026-10-16-youtube-channel-identity/provider-ui-current-full-frontend-manifest.json). [Live GUI manifest](performance-results/2026-10-16-youtube-channel-identity/anchor30-gui-manifest.json), [new source manifest](performance-results/2026-10-16-youtube-channel-identity/availability-offset-native-manifest.json).

Fresh61b5 YouTube read passes the bounded selected-playlist check:240unique occurrences in116+113+11pages,total240,terminal `complete=true`. Available flags are106/105/10 and unavailable flags10/8/1, totaling221true and19false. This qualifies that playlist's count/availability handling, not all accounts, playlists or acoustic playability. [Live availability evidence](performance-results/2026-10-16-youtube-channel-identity/youtube-61b5-availability-counts.json). Earlier f991 incorrectly-true availability remains historical.

Historical988a Spotify profile paging returned30+30=60unique playlists, then page3 fails: this is partial, not complete. Inspector totals vary by page:0/30→30rows/110total;58/30 and60/30→29rows/109total;90/30→10rows/90total. Investigation continues without a further fix claim. [Partial profile evidence](performance-results/2026-10-16-youtube-channel-identity/spotify-profile-988a-partial-counts.json). The anchor build passes806native/four focused,189Node/52focused. ReleaseView duplicate-position and artist-expanded-release duplicate playback indexing under descending source order are source-fixed, with five+19targeted tests, Svelte0errors/warnings and production build4.62s. These two new tests postdate the matching-source610-test full baseline; no full612-test claim is made. [Check](performance-results/2026-10-16-youtube-channel-identity/release-artist-occurrence-check.log), [build](performance-results/2026-10-16-youtube-channel-identity/release-artist-occurrence-build.log). No provider writes, audio or queue changes were made by these reads.

Bounded61b5 desktop visual checks cover main, queue and Info views at1914×515: finite geometry inspection reports `findings=[]`, footer98.5px and all controls visible, with queue/Info contained above it. Root viewed actual app-owned captures, retained privately under `.cache/private-ui-checks/61b5-{desktop,queue,info}.png`; screenshots containing track metadata are not published. The panel was closed afterwards, without queue/audio writes. This does not complete remaining desktop/half-screen UI qualification.

Bounded isolated988a playback checks: local before/after and duplicate-occurrence advancement pass five-second mixer progression. Corrected paused seek passes serial+generation advancement, position10, remaining paused without audio; resume passes five-second progression. A repaired chapter fixture preserves15-second identity and five-second progression, and the corrected terminal test reaches ended/max position15 while ignoring intermediate buffering. The original same-generation assertion and missing chapter asset mapping were fixture/test errors; an earlier chapter-end attempt stopped prematurely on buffering and remains a test limitation, not a silently discarded pass. [Corrected seek/remote](performance-results/2026-10-16-youtube-channel-identity/isolated-paused-seek-remote.json), [chapter progression](performance-results/2026-10-16-youtube-channel-identity/isolated-chapter-repaired.json), [terminal check](performance-results/2026-10-16-youtube-channel-identity/isolated-chapter-terminal.json), [initial report](performance-results/2026-10-16-youtube-channel-identity/isolated-initial-seek-chapter-report.json).

The one-shot overall report remains **FAIL**: two Sasa cases give meaningful unavailable results in4.725/6.031s, and Poso fails the duration gate. Poso separately passes five-second mixer progression but has actual duration250 versus expected245.48; an official-lyric YouTube title does not prove recording equivalence. Broad playback, exact Poso matching and correct-version Sasa playback remain open. [Overall report](performance-results/2026-10-16-youtube-channel-identity/isolated-functional-overall-report.json).

Remaining actionable scope: verify Spotify continuation and extend provider acceptance beyond the one verified YouTube playlist; finish ordinary-track/DJ/concert recognition and cancellation acceptance, Sasa exact-version playback and broader playback/performance/platform checks; finish remaining desktop/half-screen UI views and cache-only versus intentional-save checks; qualify reviewed cross-provider matching/writeback and complete performance/asset/write-contention gates. Mobile UI remains deferred by the user. Bounded artist/album/owner-navigation and earlier UI successes remain evidence, not a blanket pass. The detailed open-item table and original requirements below retain the full scope.

Earlier checkpoint evidence below is historical unless explicitly updated by the authoritative checkpoint above.

Historical a217 checkpoint (2026-10-16): the sole verified GUI is build `a217`, SHA-256 `a21733938f82a05c183a355e62e89a9c080ba64321c67993db891e44f67efa8f`, PID 2147434 (launcher PID 2147325, session 6975). Fresh runtime snapshots preserve the paused one-item queue, position 699.309783 seconds and volume 0.8 exactly, without playback or queue writes. Restart remains authorized at any time. That checkpoint compositor is 1914×515: a requested 954px resize was ignored, so no current half-width visual acceptance is claimed. Saved-library counts after the bounded reads are 4,949 tracks, 36 playlists and 15 assets; no import was performed. Earlier counts and queue transitions remain historical. General YouTube library listing returns 61 playlists, including nonmusic collections, without fresh login; the earlier bounded listing had no cursor, `truncated=false` and `complete=null`. An earlier ordinary playlist read returned 116 tracks then 113 through continuation: the first page has a cursor and the second does not, but both report `complete=false` and `total=null`. These 229 returned tracks do not establish complete playlist content. That earlier read-only YouTube checkpoint reported schema9,4,948saved listings and15assets; current counts are stated above. Bounded YouTube pages up to 1,000, artwork startup scoping, search-kind disabling and Spotify diagnostics are compiled in that GUI; individual live acceptance remains limited to the observations recorded here.

Historical 9482 owned footer capture at 954×1036 reports `findings=[]` and displays complete 500×500 artwork from the bounded data-image fallback. The asset-protocol probe still fails: this is fallback success, not a protocol repair. The actual capture is private at `.cache/private-ui-checks/9482-current-footer.png`. At that 9482 checkpoint, one MCP radio identification succeeded with artist/title present; the SongRec/catalog match remains unverified. Playback afterwards remains playing without errors, with no queue or tag edits. Ordinary-track, DJ-set and concert recognition remain unqualified. [Historical 9482 GUI manifest](performance-results/2026-10-16-youtube-channel-identity/user-facing-gui-manifest.json), [artwork and recognition evidence](performance-results/2026-10-16-youtube-channel-identity/user-facing-artwork-recognition-live.json).

Historical 4251 replacement observation: playback changed from a one-item Ena Proino queue at 181.858671875 seconds to a 200-item queue, index 5, Nine at 15.242102 seconds, paused with volume 0.8. User activity versus stale persisted-state restoration remains unproven; no queue overwrite was performed. That unexplained transition does not invalidate the separately observed exact preservation across that a8cc restart. [4251 live evidence](performance-results/2026-10-16-youtube-channel-identity/final-current-live-redacted.json).

The retained artwork-fallback native checkpoint passed **791 tests, zero failures, 16 ignored in 13.12 seconds**, test SHA-256 `eb3612c098ed40a8604095716f1e73c0230b5053a88a498b8d501432fa77592d`; targeted display and managed-diagnostics checks pass three cases each. Mounted/helper frontend checks pass 12/12, Node checks pass 42/42, Svelte retry reports zero errors/warnings and the frontend production build passes. [Native manifest](performance-results/2026-10-16-youtube-channel-identity/artwork-fallback-native-manifest.json). The historical replacement candidate was frozen at SHA-256 `4c482f6328e8b3eee19dc897ffb0977ef31c2b4f2ee893f4ca9c5d6be86d24e4` and was **not launched** (`runtimeStatus=frozen_not_launched`): at that historical checkpoint the fresh probe found user-active playback. The user subsequently authorized restart at any time, so that observation no longer blocks replacement. That retained 791-test candidate was not launched; 9482 was launched later as recorded in the historical evidence. The complete serial App retry passes 208 tests in 43.21 seconds after the multi-route History functional timeout was raised from five to ten seconds, retaining all assertions and separate performance checks; the initial 207-pass/one-timeout log remains. Those source-only checks did not establish live acceptance; the later 9482 fallback success and remaining artist failure are recorded above. Prior failures remain logged. The complete frontend suite passes **564 tests across 60 files in 86.45 seconds** on the serial rerun. The initial concurrent run recorded 563 passes and one History timeout; its log is retained rather than treated as a clean pass. After correcting declaration order for the temporal-dead-zone error, Svelte reports zero errors/warnings. The final production frontend build passes in 5.18 seconds with a chunk-size warning. YouTube worker fixtures pass 48/48, Spotify worker fixtures pass 25 actual cases, and all seven contracts pass. These source checks do not extend the bounded live GUI observations recorded above. Andrew Kelsh UI search returned three results including exact profile `dkkelsh1`; direct profile-URL browsing returned 30 playlists with a continuation cursor. The dd729 build produced the actual app-owned 1920×988 `capture_main_ui` screenshot and owned-WebView geometry `findings=[]`; broader view/responsive qualification remains open.

The compact Library toolbar was implemented and live-verified through Vite in the historical 4251 GUI at 954×1036, with a 707px content pane. Library/Favorites/Collections share one compact row; duration controls sit behind a Filters disclosure with an active badge and Clear; Library options moved into the header ellipsis menu, removing the empty strip; Load more sits at the list end; selection controls wrap and popup bounds are constrained. In the actual owned Library screenshot, the table header moved from y=344 to y=125 (about 219px less toolbar height), showing roughly 13 rows instead of nine. The full playback snapshot remained unchanged through HMR: 200 queue entries, index 5, paused at 15.242102 seconds, volume 0.8. The [private local screenshot](../.cache/private-ui-checks/compact-toolbar-verified.png) is retained under `.cache`, not published evidence. Before the bbe restart, both Filters and Library options were opened and closed using PID- and executable-path-guarded app-owned AT-SPI actions, without focus changes or global input. Their actual private captures (`.cache/private-ui-checks/compact-toolbar-filters-open.png` and `.cache/private-ui-checks/compact-toolbar-options-open.png`) show both popups bounded within the 954×1036 window and half-width Library pane. Mobile UI checks are deferred at the user’s request and are not a current completion requirement. Desktop and half-screen checks across the remaining views stay in scope. Mounted filter-badge/Clear coverage passes, the complete App suite passes 207/207, artwork diagnostics pass five cases, Svelte reports zero errors/warnings and the compact-toolbar production build passes. These checks do not complete the broad UI pass.

Historical0214 album failure (HTTP200/PROVIDER_CHANGED, no pages) remains in [unqualified album evidence](performance-results/2026-10-16-youtube-channel-identity/large-album-live-0214-unqualified.json); the later success below supersedes that current-status limitation.

Historical a217 bounded live acceptance: `69 Love Songs` album paging succeeds with 30+30+9 tracks, 69 unique track and occurrence IDs, positions 1–69, positive fractional durations and a terminal cursor. Radiohead search→exact guarded app-owned click now loads artist artwork; the earlier 28b7 missing-artwork proof is retained, and the row-data artwork wire fix has two focused App tests. Compact release controls are live with 20 releases loaded of 45 reported. [Current GUI manifest](performance-results/2026-10-16-youtube-channel-identity/album-youtube-combined-gui-manifest.json), [album proof](performance-results/2026-10-16-youtube-channel-identity/large-album-live-a217.json), [artist artwork proof](performance-results/2026-10-16-youtube-channel-identity/search-artist-artwork-live-a217.json).

Actual profile-owner UI acceptance covers `dkkelsh1` search (one result)→native profile→public playlist (16 tracks)→Browse Andrew Kelsh playlists→internal profile→Back restoring detail. The 110 reported playlists remain bounded/incomplete, not fully qualified. Root viewed the private captures. At actual 1914×515, compact browser/detail controls show three full track rows and footer with `findings=[]`; the previously oversized duration inputs/clipped selects are corrected in this bounded view. [Owner-navigation proof](performance-results/2026-10-16-youtube-channel-identity/profile-owner-ui-live-a217.json). This does not complete broader desktop UI qualification. Fresh live identification cancellation remains untested while paused.

Latest source checkpoint: the complete frontend suite passes **590 tests across 62 files in 96.61 seconds**; the finite-inspection native suite passes **796 tests in 11.86 seconds**, with four focused tool checks. Append-metadata UI coverage passes 16 cases, Svelte reports zero errors/warnings and production build passes in 4.89 seconds. [Full frontend evidence](performance-results/2026-10-16-youtube-channel-identity/provider-ui-full-frontend-manifest.json), [check](performance-results/2026-10-16-youtube-channel-identity/continuation-metadata-check.log), [build](performance-results/2026-10-16-youtube-channel-identity/continuation-metadata-build.log). At that historical checkpoint, GUI `6035cc3ffe635043ca7f07baf7905295060902e5769ac55e1a90f9c692771375` was frozen after a 2m08s build and its replacement verification and full Node run were pending; these are not current running-job claims. Live YouTube zero-page behavior and profile investigation remain unqualified.

Historical a217 YouTube failure: library listing returns 61 items with all track counts unknown; the first ordinary playlist page returns 116 tracks, reported total 240 and a cursor, but continuation returns zero tracks, reported total 5, no cursor and `complete=false`. This inconsistency is a live failure, not a validated parser fix. [Unqualified live evidence](performance-results/2026-10-16-youtube-channel-identity/youtube-playlist-live-a217-unqualified.json). The finite read-only `inspect_youtube_playlist_page` tool is now implemented and native-tested, returning structural counts without provider IDs, tokens or raw responses; live inspection acceptance awaits the replacement GUI. Default-off response-shape diagnostics, continuation-total-null handling and UI preservation of initial-page append metadata are compiled in the new candidate and source-tested; they are not yet claimed live-qualified. The running a217 still contains the earlier 59-test parser behavior. [Source shared checks](performance-results/2026-10-16-youtube-channel-identity/youtube-response-shape-shared-tests.log). Current source validation: Worker/adapter checks pass 42/42; RemotePlaylists passes 27 tests including scroll restoration, and focused owner/art App cases pass two tests. The full 792-test native checkpoint is historical; the current diagnostic build has four focused native checks and two Node checks, not a new full native-suite claim. The prior artwork-wire Svelte check passed zero errors/warnings and production build passed in 4.79 seconds; latest Remote UI Svelte check passes zero errors/warnings and production build passes in 4.64 seconds with a chunk warning. [Check](performance-results/2026-10-16-youtube-channel-identity/remote-ui-check.log), [build](performance-results/2026-10-16-youtube-channel-identity/remote-ui-build.log).

Profile-owner navigation now has source-only validation: the shared Spotify profile URL normalizer accepts bounded legacy/Unicode IDs, decodes once and canonicalizes safely, rejecting path separators, whitespace/control characters and URL authority/query/fragment extras. Two focused App tests and 40 child tests pass, including creator→native profile→Back restoration of filter, sort and two rows without refetch. Those source checks preceded the bounded a217 live owner-navigation acceptance recorded above; no new full native-suite count is claimed. [Source check](performance-results/2026-10-16-youtube-channel-identity/owner-navigation-check.log).

On historical 9482, exact `poso s'agapo` searches returned Spotify 30 and YouTube 20 without errors; each source reported complete request status while `hasMore`/incomplete remained true, so catalog exhaustion is not qualified. [Bounded search evidence](performance-results/2026-10-16-youtube-channel-identity/user-facing-poso-search-live.json). Two further frontend fixes now have focused coverage: partial-playlist retry retains existing rows/incomplete warning on failure and replaces only successful first-page results (12/12); GUI recognition cancellation completes MCP while the recognizer is unresolved, with late-result/next-request guards (five targeted passes, 204 skipped). Svelte passes zero errors/warnings and production build passes in 6.06 seconds with a chunk warning. These are source checks, not live acceptance of those paths.

Historical a8cc recognition checkpoint: one MCP attempt on WebView radio failed after about 4.6 seconds with `OPERATION_FAILED: Selected stream stopped producing audio`. Immediate playback status still reported playing (`paused=false`, volume 0.8), with no interruption or queue change and no retry. A playing clock is not acoustic-output proof, and the failure cause remains unproven. A Linux first-PCM timing fix is now applied in source: parec 100ms latency/50ms process time, strict two-second initial grace and ownership checks, retaining the 750ms post-data deadline and 12-second total limit, plus a delayed-packet fixture. The latest native checkpoint now passes 792 tests, with eight Linux capture checks passing. The capture/artwork fixes were verified in 9482; the current artist fix is in 0214; bounded live outcomes are recorded above. Functional live QA remains the priority. Frozen 4c remains an unlaunched historical candidate; broader live acceptance beyond the historical9482 observations remains pending. [Recognition checkpoint](performance-results/2026-10-16-youtube-channel-identity/radio-identification-live-a8cc.json).

A sustained native playback gate requiring five seconds of progression is implemented, covered by four test-file entries and wired into CI. It has not yet passed live qualification. The new isolated seed profile `f7dx` has completed preflight only and has not been launched. Playback performance, audible output and platform qualification remain open; no todo is completed from the gate implementation alone.

The user authorized one empty private **Burrata verification** playlist on each provider. Each provider's single creation attempt failed with an unknown outcome and retained an `ATTEMPTED` marker without a playlist ID. Spotify inspection covered 113 recognized playlists across three pages, with zero Burrata verification matches; the subsequent 61-item YouTube read also found no match. These observations do not prove that no playlist was created. No creation retry or track writes occurred, and no verified private playlist links are available. Matching and writeback remain unqualified.

A muted WebView resume-and-seek functional check on dd729 advanced one track to 14.801061 seconds after a 10-second seek, but reported `readyState=2` and `status=buffering`. At that earlier dd729 checkpoint, it was restored paused with a one-item queue, volume 0.8 and position 181.858671875 seconds; the later 4251 queue differed as recorded above. This single run does not qualify latency, audible output, match correctness or uninterrupted playback. Earlier exact 200-item queue preservation remains historical evidence; the current 9482 queue is the one-item playing queue recorded above.

A paired three-sample page exploration completed in 113.78 seconds while the main GUI remained paused, an explicit confounder: source-order median fell 256.233→192.324 ms without the candidate index and 153.919→107.878 ms with it; BPM fell 338.484→278.135 ms without and 151.191→115.068 ms with it. These are exploratory batch-hydration comparisons, not 20-sample qualification, write-contention or asset-backed acceptance; no production index is enabled. [Paired exploration](performance-results/2026-10-16-youtube-channel-identity/paired-page-exploratory-main-idle.json). New benchmark expansion is deferred in favor of functional live QA; the latest staged coverage fix is unapplied. A test-only paired metadata-invalidation/membership-write gate and distinct 1KiB asset-file fixtures for 1k/5k/20k/50k are also applied, without completed benchmark qualification. These use synthetic metadata, not audio, and optional database statistics; no write/asset qualification is claimed. The canonical-metrics partial-index candidate passed the corrected three-sample exploration and paired 20-sample aggregate-SQL run on a disposable 50k Store. Baseline→coverage-index median/p95 (ms): count **90.895/101.775→12.292/13.468**; count plus availability OR **123.830/159.151→41.854/50.544**; with BPM coverage **144.913/160.946→47.758/57.119**. Semantic comparisons pass and foreign-key errors are zero. This remains experimental and unpromoted: forced SQL-path timings exclude page hydration, mutex contention, GUI, CPU and end-to-end work; index-build observations are not qualification. [Exploration](performance-results/2026-10-16-youtube-channel-identity/canonical-metrics-index-exploratory-retry.json), [20-sample results](performance-results/2026-10-16-youtube-channel-identity/canonical-metrics-index-20.json). Batch hydration and observer normalization are applied in source. At that source checkpoint, focused frontend checks passed 5/5, production build passes and Svelte reports zero errors/warnings (`batch-observer*` logs). The invalid fixture CHECK value was corrected from `keep` to `shortlist`; all five hydration and two observer targeted cases now pass, followed by the complete 788-test native suite. The original failure log is retained. The historical a8cc GUI contained the batch-hydration checkpoint; historical9482 includes it. The earlier queue-save-failure-banner serial App checkpoint passed 208/208 in 29.42 seconds; the latest serial retry and its timeout adjustment are recorded above. Build and Svelte checks pass. The earlier concurrent run (204 passes, four failures) and initial focused text mismatch remain in the logs. A storage-default fixture correction is a test fix, not a hidden production change. The post-join-elision Store benchmark completed a three-sample exploration followed by 20 measured samples (three warmups). At 50k metadata-only listings, first-page global-availability median/p95/worst is **155.201/165.595/172.546 ms**; concurrent-reader latency is **161.286/177.107/177.651 ms**, and favorite-writer latency is **11.053/161.903/169.576 ms**. The writer p95 still exceeds the interactive target. These are Store wall-time measurements on the immutable test artifact and storage source SHA-256 `60f2e56ab2d9862acb28c0582b213ac1da16fb7a436b8a1208be73f24f3a7c54`; they do not establish UI/end-to-end, CPU, release-build or asset-backed qualification. The earlier paired SQL-only comparison is retained below. No open todo is completed by this checkpoint. [Historical general-context GUI manifest](performance-results/2026-10-16-youtube-channel-identity/general-context-gui-manifest.json), [general YouTube live read](performance-results/2026-10-16-youtube-channel-identity/general-context-live-redacted.json), [WebView functional check](performance-results/2026-10-16-youtube-channel-identity/current-webview-functional.json), [20-sample Store results](performance-results/2026-10-16-youtube-channel-identity/global-availability-join-20.json).

A paired 20-sample SQL join-elision comparison reduces plain-count median from 152.650 to 80.820 ms and p95 from 169.457 to 88.073 ms; count plus availability median falls from 180.836 to 106.973 ms and p95 from 194.683 to 124.092 ms. These are SQL component measurements, not whole-page, whole-app or release performance qualification. Older benchmark and session records remain historical evidence; no open todo is completed by this checkpoint. [GUI manifest](performance-results/2026-10-16-youtube-channel-identity/combined-gui-manifest.json), [native test manifest](performance-results/2026-10-16-youtube-channel-identity/combined-native-manifest.json), [paired SQL measurements](performance-results/2026-10-16-youtube-channel-identity/metrics-join-20.json).

Earlier post-fix Store checkpoint (historical), 20 samples at 50k: global pages median/p95/worst 244.93/270.98/272.29 ms; contention readers 262.13/291.19/292.78 ms; writers 6.03/258.02/281.96 ms. Reader p95 improves from the earlier 1224.61 ms, but writer tail latency still fails the interactive target. A separate exploratory three-sample profile took 85.62 seconds overall: aggregate 175–221 ms, plain count 148–155 ms, hydration 49–52 ms and page SQL about 0.8 ms. At that earlier checkpoint, source investigation continued; the later join-elision results are recorded above. A 9.46-second development idle sample observed three app processes (Burrata, WebKitNetwork, WebKitWeb), no helper/Chromium, median CPU 0.97% of one core and PSS 366.9–367.1 MiB; this is not resource qualification. Exact 200-item queue preservation was established for those earlier restarts; it does not establish preservation across the latest replacement or explain its observed queue change.

Historical a798 source checkpoint (2026-10-16): **763 native tests pass, zero failures, 15 ignored; 560 frontend tests across 60 files pass; Svelte checks have zero errors/warnings; production frontend build and seven contracts pass**. The sorting rebase, guarded no-op invalidation and bounded UI navigation command are integrated. Provider Back/Forward and cancelled/unmounted navigation have mounted App coverage. A 20-sample 50k known-metadata Store run gives global-page p95 345.96 ms, but concurrent favorite writes expose reader p95 1224.61 ms; the guarded-invalidation fix passes regressions and has post-fix timing recorded above. At that historical checkpoint, the normal Linux GUI compiled and launched through the verified launcher: `.cache/verification-target/debug/verified-main-provider-a798b5f5f460/burrata`, SHA-256 `a798b5f5f46045e2f3be575ae25af063c98a7485802b2e7027a03bf40f0a701b`. It was then the sole GUI. That historical GUI was the compiled and launched `.cache/verification-target/debug/verified-main-provider-a798b5f5f460/burrata` (SHA-256 `a798b5f5f46045e2f3be575ae25af063c98a7485802b2e7027a03bf40f0a701b`); its normal development build has 32 warnings in the verification log. The current native checkpoint passes 769 tests, zero failures, 15 ignored in 15.19 seconds; immutable test SHA-256 `c39e1b1aa66062e15658fd273cca40885437e60e0e3008c46dd2f9be151d1ebb`. The latest worker, adapter and contract checks pass 41, 16 and seven cases respectively. The earlier 768-test native baseline predates the latest embedded JavaScript; the current native checkpoint is 769. The last complete frontend run passes 560 tests across 60 files and predates one new test; the complete mounted App suite now passes 206/206, Svelte checks report zero errors/warnings and the production frontend build passes. No full 561-test run is claimed. Spotify accounts redirect, pending retry and strict keyring handling are fixed with five new native tests; the reason the original login was lost remains unproven.

Historical a798 restart/read checkpoint: both Spotify and YouTube recovered OS-keyring sessions with no sign-in window or companion. At that checkpoint, Spotify playlist listing returns 48 items and YouTube Music listing returns two items without a cursor. Those two YouTube IDs are the system playlists `LM` and `SE`; `get_remote_playlist` rejects both as `UNSUPPORTED` because of the minimum-ten-character ID validation. The subsequent live special-ID result is recorded in the current checkpoint above. No custom YouTube playlists were observed. Earlier Spotify library pages returned 48 then 49 items without overlap; profile-owner-ID pages returned 30 then 29 without overlap, and a profile-URL read returned 30 items. Live UI Spotify profile search now returns 30 results with Load more available after the App source-capabilities refresh fix; live UI YouTube `poso` track search returns 20 results with `hasMore=true`. These searches do not establish playback correctness. The upstream Music route remains unpinned; see the limitation in `docs/performance-results/2026-10-16-youtube-channel-identity/music-library-source.md`.

Historical a798 restart playback comparison: foreground WebView snapshots were exactly equal across all fields, preserving the then-current 200-item paused queue, occurrence IDs, current index 0 and position 145.421814 seconds. Live MCP `navigate_ui` to Library completed with `providerDataReady=not_measured`. Queue-panel geometry at 954×1036 reports `findings=[]`. A desktop capture was occluded by another app and deleted; it is not a UI visual pass. Writeback remains unqualified; no live remote writes or public release occurred. [Latest evidence](performance-results/2026-10-16-ui-navigation-control/README.md).

Earlier verified main checkpoint (2026-10-16, historical): **748 native tests pass, zero failures, 14 ignored; 546 frontend tests across 59 files pass; Svelte checks have zero errors/warnings; seven contracts and normal Linux GUI/production frontend builds pass**. History restores controls, loaded pages and virtual scroll through Back/Forward with stale-query/navigation guards; default uncontrolled TrackList scrolling remains intact. Imported owner filtering, gapped album source positions, account-change reconnect and disconnected write-sync opt-out are covered. Optional Store mutex wait diagnostics are scoped and tested. The unlaunched GUI SHA is `971702210bb6fee8e0ff9e8e0d41e4f7a6ad12afcaf0cfcb42f494f6418f7568`; live 472b58f8 and its login remain preserved. [Evidence](performance-results/2026-10-16-audit-followups/README.md). The older checkpoints below remain historical. v9 asset-backed performance, current-main rebase, provider navigation, live sessions/writeback and platform qualification remain open.

Isolated sorting follow-up (2026-10-16): the stale-projection partial-index candidate passes 719 host native tests and 74 focused storage tests on its separate older baseline. A 50k asset-backed run with three warmups and three measured samples gives ready indexed medians of 318.6–378.0 ms across first/later pages and ascending/descending BPM, versus 886.4–933.9 ms without the index. Pending checks correctly return no rows; the 500-row benchmark backfill takes 87.89 seconds with a 984.33 ms worst batch (runtime batches are 250). These are exploratory Store timings, not current-main, whole-app, release-build or 20-sample qualification. The current-main rebase remains in progress. [Evidence and raw-log limitation](performance-results/2026-10-16-v9-stale-index-assets-50k-exploratory/README.md). Separately, 72 direct provider-worker/authentication/adapter regression cases pass; [logs and scope](performance-results/2026-10-16-provider-worker-regression/README.md).

Latest combined native checkpoint (2026-10-16): **744 native tests pass, zero failures, 14 ignored**. All **541 frontend tests are covered by passing runs**, including the 14-test Connections suite, and all 35 YouTube worker tests pass. Svelte check has zero errors/warnings, the production frontend builds and seven contracts pass. Initial HTTP reader setup now observes the original deadline and Stop; ready seeks use fresh generation/epoch-guarded deadlines, including pause/resume. YouTube authentication rejection now produces a persisted reconnect-required state and blocks use of the rejected credential until explicit verified replacement; account switching remains available through confirmation. [Evidence and live limits](performance-results/2026-10-16-youtube-reconnect-verification/README.md). The normal Linux GUI build also passes and is retained unlaunched; the single older running GUI and its owned login are preserved. No public release or live writeback occurred.

UI/search follow-up (2026-10-16): all **540 frontend tests are covered by passing runs** (539 passed in the full run; its one outdated report assertion was corrected and the complete 199-test App suite then passed). Svelte check, production frontend build and seven contracts pass. Main-table canonical Playing/Paused/Buffering indicators, accurate sidebar filter empty states, late Soulseek-job cancellation and optional automation source-coverage reporting are merged. Native compilation of the new coverage sanitizer and natural queue-end test remains pending; the last combined native checkpoint is still 735. Read-only MCP player geometry reports no findings at 1914×341. [Logs and limits](performance-results/2026-10-16-ui-search-followups/README.md). Meld source comparison confirms login/HTTP separation; YouTube expired-session reconnect state and post-login resource/restart behavior remain under investigation. The isolated 50k sorting profile identifies page SQL as the dominant cost (~1.1 seconds), so that candidate remains unpromoted.

Verified combined checkpoint (2026-10-16): **735 native tests pass (zero failures, 14 ignored), 535 frontend tests pass across 59 files, Svelte checks have zero errors/warnings, seven contracts pass, and normal Linux GUI / production frontend builds succeed**. Artist Local App wiring now uses global database filters/sorts before 200-row pages; modern artist navigation no longer loads the entire asset inventory. Bounded exact source matching runs at page load; selected-row fallback respects Stop and preserves queue occurrences. Legacy fallback and no-match caches are bounded/revision-safe and tested. Narrow Downloads cards are visually corrected, and the running native player geometry reports no findings at 1914×341. The new GUI is retained **unlaunched**, preserving the older running app and login. [Exact artifacts and scope limits](performance-results/2026-10-16-main-735-verification/README.md). Earlier checkpoint paragraphs below are historical; native availability v9/performance, live provider/writeback, runtime and platform qualification remain open.

Merged startup/artist backend checkpoint (2026-10-16): **529 frontend tests across 59 files pass**, Svelte checks report zero errors/warnings, and all seven contracts pass. The delivered startup patch omitted six IPC registrations/permission files and the track-file grouping helper; merged-source checks caught and corrected those omissions before acceptance. The Artist Local backend is now merged with async blocking-task wrappers; its App callback integration remains in progress. The combined native build is active, so the separate 731-test startup candidate remains the latest new native evidence, not proof of this combined source. The Downloads activity panel now exposes View all, and a 500-job mounted-App regression verifies later-row Retry and Show file identity. Three exact unused alternate-target executable caches were removed after host process/inode checks, recovering 724,377,248 bytes; verified shared-target artifacts and the running app are preserved.

Combined frontend checkpoint (2026-10-16): **516 tests across 57 files pass**, Svelte checks have zero errors/warnings, and the production build passes. Top-track controls and staged Local paging controls, Downloads virtualization, and explicit YouTube Continue handoff UI are merged. Local native App wiring and native compilation of the handoff route remain pending. [Evidence and limits](performance-results/2026-10-16-artist-provider-download-frontend/README.md). An unused alternate-target static library was also removed (222,462,564 bytes); current shared build artifacts and running executables remain intact. Accidental duplicate audio files copied into isolated frontend test workspaces were removed only after matching size and SHA-256 against preserved original deploy files, recovering 402,120,148 bytes and relieving `/tmp` quota; all originals, databases and provider profiles remain preserved.

Responsive component follow-up (2026-10-16): eleven successful headless captures across 360/500/1000/1440 px cover long player text, unavailable errors, expanded queue/Details, chapters and Identify/Cancel progress. Visual review finds no obvious control overlap. The [archived matrix and hashes](performance-results/2026-10-16-disconnected-player-ui/README.md) explicitly limit this to disconnected components with mirrored footer markup, not full App/Tauri behavior. The existing native GUI/login was not controlled or restarted. A source audit confirms native position events call the lightweight `applyNativePosition`, rather than the full-snapshot list/state path; no position-driven library sorting/hydration bug was established.

Expanded artist-release controls (2026-10-16): shared query, sorting and duration controls now cover expanded release tracks while preserving original album playback indexes, including repeated occurrences. Full frontend verification passes 509 tests across 56 files, Svelte checks have zero errors/warnings, the production build passes, and `git diff --check` passes. Artist Local paging and overview Top tracks controls remain in progress.

Generated-cache retention update (2026-10-16): after the 727-test normal build succeeded, the superseded never-launched 725 GUI artifact was removed (329,438,000 bytes). Two older immutable test artifacts (721 main and the superseded v9 candidate) were also removed after process/hash checks, recovering 196,187,240 bytes. The running 472b58f8 GUI, latest 727 normal executable, latest test snapshots, profiles, credentials and private data backups remain preserved. A later exact hash/process-checked cleanup removed the superseded never-launched 719 GUI and three superseded BPM-index proof executables, reclaiming a further 622,923,904 bytes. Eight hash-identical copied resources in the unused alternate debug target were also replaced with references to the preserved original resources, recovering 340,178,190 bytes; hashes and paths are recorded in `/tmp/burrata-alternate-resource-duplicates.json`. No installer/update operation is permitted against those shared resource references. Earlier artifact-retention statements below are historical.

Verified recorder/asset follow-up (2026-10-16): **727 native tests pass, zero failures, 14 ignored**, in the same-binary host run (8.39 seconds); seven contracts pass. The corrected fake-recorder tests use generous startup allowance and an explicit producer-release handshake, asserting the cancellation branch rather than an arbitrary error. EOF/read-timeout paths explicitly kill and await the child; no capture artifact remains. Targeted autotag/recovery/recording-file queries are included. The protected test artifact is `verified-tests-main-727-d94dc04558a3/burrata_lib`; the updated normal Linux GUI development build also succeeds and is retained unlaunched as `verified-main-727-0cd060623c60/burrata` (SHA-256 `0cd060623c6072ad84a43fab87b75509f464a63937b768d9bb05bc3c711df0e5`). The full frontend checkpoint is now 509 tests across 56 files, with zero Svelte errors/warnings and a passing production build. Earlier 727-candidate failures below are historical and retained as evidence of the fixture correction. The isolated v9 fastpath passes 716 tests but remains unpromoted: 50k ready-page median 1057.39 ms / exploratory p95 1072.95 ms across three measured samples, above the 500 ms target.

Pending native follow-up (2026-10-16): main source now explicitly kills/awaits Linux capture children on EOF/read errors and uses targeted asset queries for autotag, tag recovery, file-manager validation and recording updates. The 727-test candidate is not a passing checkpoint: its full host run passes 724 and fails three capture fixtures with an overly tight 30 ms fake-recorder startup/read budget. Fixture synchronization and exact cancellation-path assertions are being corrected before rerunning. The last fully verified normal executable remains the unlaunched 725-test `0d008f9c` build; the running login GUI remains `472b58f8`. A [scoped open-login memory baseline](performance-results/2026-10-16-login-memory-baseline/README.md) records 588.19 MiB median PSS across five processes, without claiming post-login resource cost.

Meld-driven connection UI follow-up (2026-10-16): normal native confirmation already activates on-demand provider access automatically. The Resume control is now limited to explicitly stopped or legacy opt-in states and hidden during sign-in/YouTube account review. Copy no longer implies a persistent background browser session. Full frontend verification passes **508 tests across 56 files**; Svelte checks report zero errors/warnings and the production frontend build passes. Native backend remains the verified 725-test checkpoint. No provider login or running-GUI process was restarted.

Verified main native checkpoint (2026-10-16): **725 tests pass, zero failures, 14 ignored** using the exact compiled binary on the host (8.60 seconds). The sandbox run had 19 loopback and three owned-process fixture failures; all 22 pass on the host. This includes Linux output cancellation/deadline/explicit-stream capture fixtures, shared key-parser parity, queue metadata and indexed single-asset lookup. The immutable test artifact is `verified-tests-main-725-03c9d1ffc51d/burrata_lib`; the normal Linux GUI development build also succeeds and is retained unlaunched as `verified-main-725-0d008f9c3a7d/burrata` (SHA-256 `0d008f9c3a7d1550822e96752f0f6be138515280fdc1553b7a9cbb6345bb40f8`). The frontend remains at 507 passing tests, with the extended shared-key fixture additionally passing its focused nine-test suite. Live Burrata recognition and provider sessions remain unqualified.

Current verification follow-up (2026-10-16): **507 frontend tests across 56 files**, Svelte checks, the production frontend build and seven contracts pass. The next native run exposed one real key-normalization parity mismatch and three capture fixtures that fail inside the sandbox; the exact compiled capture module passes all five tests on the host. These are being resolved before declaring a new full native checkpoint. A fresh read-only player geometry inspection reports no overlaps at 1914×341. Provider status still reports `google-accounts` with no confirmed account; the current GUI and sign-in window remain intact. Meld’s login-only view/native HTTP pattern remains the implementation reference, with live session and resource qualification still open.

Linux capture integration checkpoint (2026-10-16): **721 native tests pass (14 ignored)** in the same-binary host run; **505 frontend tests across 56 files**, Svelte checks and the frontend build pass. The Linux path now captures only a unique app-owned stream with an explicit monitor and stream restriction, fresh exclusive-WebView checks, process identity revalidation and cancellation. Runtime Burrata recognition remains unqualified; the running GUI is still 472b58f8. Three additional fake-process cleanup tests and later natural-end pause/resume race regressions are staged in main and await the next combined run. The prior 719-test normal executable built successfully and is retained as an unlaunched rollback artifact (`63ff3041`). A [muted functional resume check](performance-results/2026-10-16-webview-restored-resume/README.md) on the existing GUI passed and restored playback state, with saved library counts unchanged. The v9 50k run is measuring the next bottleneck (first ready page ~1.15 seconds); it remains unpromoted. An initial 50k attempt accidentally selected a shared test executable replaced by main's build; its results were discarded. Main and candidate test executables now have separate immutable, hash-verified snapshots.

Native verification checkpoint (2026-10-16): the current combined main backend passes **719 tests, zero failures, 14 ignored** using the same compiled test binary with localhost access; seven contracts also pass. Its first sandbox run had 19 loopback-fixture permission failures, all resolved by that same-binary host run. This verifies the new reader/decoder/first-PCM timings and reuse fixture, catalog recording index, and cancellation/deadline guards; it is not a new running GUI. Runtime PCM temporary files now use `tempfile` as a normal dependency, rather than a test-only dependency; a normal executable build is still required. Additional native-stop/radio-race and honest incomplete-search-label changes pass 210 focused App/SearchPanel tests after the 502-test full frontend checkpoint. The isolated Pulse per-stream experiment and its limitations are archived in [the experiment report](performance-results/2026-10-16-pulse-stream-isolation/README.md). The v9 sorting candidate's optimized 20k ready-page exploratory p95 is 448.11 ms over three samples; 20-sample qualification and 50k remain outstanding, and schema v9 is not in main.

Combined frontend checkpoint (2026-10-16): **502 tests across 56 files pass**, Svelte diagnostics are 0 errors/0 warnings, and the production frontend builds. Main now includes shared-list playing/paused/buffering status markers, a no-op upcoming-sort boundary fix, Spotify multi-cursor-cycle protection and full App artist navigation coverage, queue delta publication with old-native fallback, cached/direct startup deadline enforcement, and the actual-PCM frontend/MCP flow. A scoped live screenshot confirms a restored paused track now says **Ready**, rather than the erroneous Unavailable label; the existing 1914×341 geometry reports no overlaps. This remains a single-size check. The current native executable is still `472b58f8`; no login window was restarted. **Linux Identify is not complete:** WebKitGTK 2.54.1 lacks captureStream; a process-scoped PulseAudio/PipeWire-Pulse fallback is being investigated without system-wide recording. Additional reader/decoder/first-PCM diagnostics and the catalog recording-ID index are in main source but still await native compilation.

Latest source checkpoint (2026-10-16): combined native queue-delta publication, shared-queue bounded reads, cancellable actual-PCM recognition and MCP frontend identification passed **718 native tests, zero failures, 14 ignored**, plus seven contracts. Subsequent small cancellation/timeout/metadata-only snapshot guards still require the next native run. Frontend PCM and queue publication are being recomposed against current main to preserve the sorting work; they have not entered the running executable. A prototype frontend directory was accidentally a symlink into main; it is now a real isolated copy. The affected optional sort-pending flow is retained: the full frontend suite passes 477 tests across 54 files, including 173 mounted-App cases, and Svelte diagnostics report zero errors/warnings. The 20k asset-backed sorting prototype remains unpromoted because its measured ready-page median (686 ms) exceeds 500 ms. The sole GUI remains PID 237044 / `472b58f8` / 698-test checkpoint, with its owned Google sign-in preserved. Meld's login-only browser architecture and Linux WebKit context-lifetime caveat are recorded in [the session investigation](meld-session-investigation.md); post-confirmation resource and restart tests remain open.

Main source follow-up (2026-10-16): the YouTube-only initial-mode fix, end-to-end YouTube Music continuation/cursor binding (including buffered ISRC binding), retained synthetic response-shape fixture, and bounded captured-WAV validation/recognition endpoint pass **709 main native tests, zero failures, 14 ignored**. This is a source/test checkpoint, not the running GUI: PID 237044 remains on the 472b58f8 executable while the user’s owned YouTube sign-in is pending. The frontend PCM flow has not been applied; cancellation during recognition and the MCP frontend-capture roundtrip remain under review, so actual-output WebView Identify is not yet complete.

The pinned Tauri runtime/Wry sources (tauri-runtime-wry 2.12.1, Wry 0.57.0) explicitly configure WebKitGTK persistent cookie storage under the provider-specific data directory. Metadata-only inspection confirms a cookie file exists in the isolated YouTube profile. Its contents were not read. This establishes the persistence mechanism, not a valid signed-in account; the blank Google form must not be described as proof of cookie-persistence failure.


Latest main native/live checkpoint (2026-10-16): the YouTube origin-class diagnostic, explicit cookie-gated return-to-Music MCP control, shutdown cancellation/admission guards, rate-limit phase/status diagnostics and Spotify zero/malformed-duration repair are now in the main source. **698 native tests pass, zero failures, 14 ignored**; the development executable builds and seven contracts pass. The verified launcher replaced the exact old GUI only after checking its executable hash, then confirmed 82 tools and the mounted frontend. Current single GUI: SHA-256 `472b58f8fb436d82d0de1dc196bab6f54e3a0ecd536fb3846ef57c67a766ea86`, PID 237044, same original profile, schema v8. It is running; E5 paragraphs below are historical. The new diagnostic reports `youtubeLoginPage=google-accounts` and neither provider confirmed in the main profile. The user has been asked to finish interactive sign-in in that owned window; no return/navigation or remote write has been dispatched from the new diagnostic.

A YouTube-only restart readiness gap has also been identified and a fix staged: initial native mode previously recognized only persisted Spotify accounts. It has not yet passed native tests or entered this running executable. Actual public YouTube Music continuation shape has been verified with one unauthenticated initial request and one continuation request: 20 IDs followed by 20 IDs with one overlap and another continuation. End-to-end cursor/dedup changes and a fully synthetic shape fixture are staged but remain uncompiled. The sorting prototype separately passes 709 native tests; its guarded single-asset fast path reduces the exploratory 5k read from about 2.8 s to about 1.2 s, still above the 500 ms target. It remains outside main.


Current live GUI checkpoint (2026-10-16): E5 was launched against the original profile after host inventory verified that the previous GUI had exited. The verified launcher confirmed both the authenticated MCP connection and mounted frontend. The live database is schema v8 with all 4,948 existing listings saved and all 15 assets retained. Public discovery for `poso s’agapo` returned 5 Monochrome, 20 YouTube and 30 SoundCloud results; SoundCloud supplied a continuation cursor, and Monochrome explicitly warned that coverage was unknown. A real SoundCloud continuation request returned another 18 results with zero overlapping result IDs, then exhausted its cursor (48 distinct results across two pages). The saved library count remained 4,948 after discovery. Fruit Bats — The Bottom of It played via Monochrome in three exploratory requests with the independently expected 181.4-second duration and positive playhead positions. Observed command/poll times were 5,506 / 797 / 793 ms; these are neither first-mixer measurements nor statistical startup qualification.

The initial populated-library screenshot exposed a short-window usability issue despite no overlap findings: header, filters and footer left no visible track rows at 1914×343. A frontend compact-height rule now reduces those controls without changing the player. A second scoped screenshot shows a visible track row and the complete footer; the mounted WebView reports 1914×341 with no overlap findings. Svelte checks pass with zero errors and zero warnings, and the frontend production build succeeds. Owned main-window screenshots of the expanded queue and track-details panels also show the complete player; their geometry inspections reported no overlap findings. This does not qualify every view or viewport. YouTube’s owned login window is open, but confirmation still rejects the page before account verification because it has not reached the required Music origin. No remote writes were performed. An explicit origin-class diagnostic and guarded return-to-Music control are staged separately, not in E5. Later checkpoint paragraphs below are historical where they describe E5 as not launched.



Latest v9 verification checkpoint (2026-10-16): preserved-data candidate `Store::open` migrations from schema v7 and v8 to v9 passed on disposable copies. All original-column row multisets across 21 tables matched for 4,948 listings; membership values/defaults were preserved, masks remained pending, SQLite quick-check passed, and foreign-key check found no errors. Startup reconciliation did not change the snapshot's 44 terminal download jobs. The separate v9 native prototype passed 705 tests with 15 ignored; the main native checkpoint remains 693, and these counts are from distinct builds. Asset-backed first-page reads remain a clear performance failure: at 1k rows the median was 609 ms, and at 5k it was 2,590 ms, against the 500 ms target. Three measured samples after three warmups are exploratory, not p95 qualification. Profiling points to per-request asset hydration as the dominant cost. The E5 GUI passed the verified-launcher mounted handshake; its Library had 4,948 tracks with 200 loaded, but the filters/header/player filled the 343 px captured surface and no row was visible. The 1,914×341 viewport and 86 px footer had zero overlap findings; compact-height correction remains in progress. Three Fruit Bats “Monochrome” plays succeeded with playback observed at 181.4 s; 5,506/797/793 ms are command/poll timings on warm caches, not first-mixer latency. YouTube account confirmation remains open; the UI still requests sign-in and no account was confirmed. Keep related TODOs open. See the [asset-backed v9 evidence](performance-results/2026-10-16-v9-asset-backed/README.md).

Main-source verification and replacement ready (2026-10-16): the main worktree now includes the tested process-memory Spotify token cache/singleflight/private IPC, scoped rejection invalidation, Spotify login blanking/retry handling and raw dateAdded DTO/Store regression. **693 main native tests pass, 0 fail, 14 ignored**; **475 frontend tests pass across 54 files**; Svelte diagnostics report zero errors/warnings; frontend build and seven contracts succeed. Provider worker fixtures pass 19 Spotify / 34 YouTube / 2 private-auth tests and are now included in both CI workflows. The main native development build succeeds.

Verified replacement: `/tmp/burrata-sorting-target/debug/verified-meld-auth-e5fe3dce/burrata`, SHA-256 `e5fe3dce1e1e727c6f0c2cf4c2b9ddd5f4c9a3a102ceb51d07b9a1a66c915e82`. It is **not launched**. Quota prevented a duplicate executable copy, so the complete artifact was moved within its build-cache filesystem and its Cargo hardlink aliases were detached; the partial copy was removed. The old running app and provider login remain preserved. Native sorting v9/runtime/aggregate remains isolated; its first 1k asset-backed fixture already exceeds 500 ms at roughly 610 ms and is now a measured optimization target. Live auth/cache/resource behavior and writeback remain unqualified.

Latest isolated native checkpoint (2026-10-16): the reviewed token-cache v3, compact aggregate, raw dateAdded DTO, bounded worker and login lifecycle pass **702 native tests, 0 failures, 14 ignored** (`/tmp/burrata-combined-v3-native-tests.log`). Two path ownership errors in a new mock test and a mock YouTube message that omitted the real worker's diagnostic suffix were corrected in the isolated copy; the previous failing log is retained. Cached Spotify jobs now invalidate only the exact rejected bearer, preserve safe phase/status diagnostics, and retain valid tokens on rate limit/timeout; YouTube retains its existing worker path. All-known and sparse no-asset exploratory reads are below 500 ms in three samples. Asset-backed first-page measurements are now being compiled and remain a landing gate. Main native sources, the running GUI and ready replacement binary remain unchanged.

Combined isolated native checkpoint (2026-10-16): **696 tests pass, 0 fail, 14 ignored** (`/tmp/burrata-token-v1-or-date-runtime-native-tests.log`). This combines the v9 Store/runtime, compact bit-OR aggregate, raw dateAdded DTO including an actual Store manual-edit/batch/page-order regression, Spotify login blanking/retry state and the first token-cache protocol/coordinator. The existing Spotify worker fixture suite passes 19/19 and the new private-protocol fixtures pass 2/2. The first cache revision still needs the reviewed scoped rejection invalidation and typed-error follow-up; that follow-up is being staged and has not been Cargo-tested. The compact aggregate's actual Store benchmark is running separately. This checkpoint is an isolated prototype, not a main-worktree/running-app acceptance claim; credentials and application data are untouched.

Final frontend paging checkpoint (2026-10-16): **475 tests across 54 files pass** (`/tmp/burrata-pending-pagination-full-frontend-tests.log`); production frontend build succeeds and seven repository contracts pass. Availability refreshes now follow a query revision independently of page loading, so Load More cannot consume the sole completion event and strand the view in Pending. The App integration regression preserves all appended rows, cursor and scroll while applying the ready map. The isolated native date-DTO/v9/runtime/login checkpoint passes **687 tests, 0 failures, 14 ignored** (`/tmp/burrata-v9-date-dto-lifecycle-native-tests.log`). It is not the main-worktree or running-app checkpoint. A combined token-cache/bit-OR build first found one aggregate error-conversion compile error, now corrected in the isolated candidate; its remaining verification is in progress. None of these native prototypes has been promoted or launched.

Pending availability and paged history (2026-10-16): the full frontend suite passes **474 tests across 54 files**, Svelte diagnostics are clean at the focused checkpoint, and the production frontend rebuild succeeds. Explicit pending availability keeps all choices enabled and readiness events fetch only metadata for the same query. App integration covers a stale-query response racing the current query, preserving loaded records, paging offset and scroll, plus legacy native responses without the pending flag. History remains permissive while older matching pages remain; a 201-entry regression proves a field found only on the final page remains selectable. The native event/backfill and login lifecycle work are isolated prototypes, not included in the running or ready replacement executable. The first persisted-mask sparse benchmark still fails the read target and is retained with its separate cold/backfill/warm costs in the performance report.

Current verification (2026-10-16): the complete frontend suite now passes **468 tests across 53 files** (`/tmp/burrata-current-full-frontend-tests.log`), including the chapter and source-opening cases described below. The YouTube worker fixtures independently pass **34/34** when executed directly (`/tmp/burrata-meld-youtube-direct-tests.log`); the Node test-runner wrapper only reported a file-level pass and is not used as the individual test count. The persisted-availability v9 candidate remains outside the worktree: its first isolated native checkpoint passes 56 storage tests and 8 sorting tests, with additional migration/asset invalidation regressions and cold/backfill/warm measurements still in progress. This does not qualify the migration or performance for promotion. The ready executable and existing GUI remain unchanged.

Queue, chapters and source opening (2026-10-16): the full frontend checkpoint passed 460 tests across 53 files after adding occurrence-aware queue states and the same-station radio stale-result guard. Subsequent targeted changes passed the complete App integration file (167 tests), an additional actual WebView chapter/parent-source case (1 pass, 167 skipped), and 18 queue/chapter tests. Current Svelte diagnostics report zero errors/warnings and the production frontend build succeeds. These scopes are separate; a full-suite count for the final added cases has not been run.

Queue rows now expose playing/paused/buffering glyphs and accessible state, while duplicate occurrences retain separate current-index state. Active chapter lookup runs once per update. Native ranged playback uses absolute source time for chapter selection and preserves canonical chapter IDs; existing ranged tracks avoid nested range identities. Source opening recognizes a parent only when parent ID and the actual selected source URL match the playing chapter, includes its offset, and rejects navigation after a playback change during an awaited pause. The native 72+17=89 and WebView absolute89 cases are covered through actual App interactions. The actual audible-output capture limitation for WebView radio remains open.

Native executable `d0f14b5d…` is still ready and unchanged. Frontend edits are served by Vite and do not change its embedded native/provider sources. The original main GUI and its focused login window remain preserved while the restart question is pending. The next native availability design is being prepared outside the worktree; none of the slower streaming candidates has been promoted.

Replacement build ready (2026-10-16): the development executable finished successfully and was copied to immutable `/tmp/burrata-meld-window-d0f14b5d/debug/burrata`, SHA-256 `d0f14b5d23a498cf68983b0e6ec0e4aeb0f319e8d550d8853673ad240ba51f9d`. It includes the 673-test checkpoint, native minimum-height correction and embedded YouTube account-list route. It is **not running yet**. The original GUI remains alive and its owned YouTube login window is focused; replacement is waiting for the user's login-interruption answer. No remote write or credential migration occurred. Independent availability optimization work is being prepared outside the worktree to preserve launch freshness.

Combined-edition verification (2026-10-16): the final native library run passes **673 tests, 0 failures, 14 ignored** (`/tmp/burrata-edition-final-native-tests.log`). Writeback candidate filtering now compares all recognized edition markers, preventing a shared Remix or Radio Edit label from hiding a Live mismatch. Candidate-search regressions retain the matching combined edition and reject the mismatched one. The subsequent development GUI build is running in job 27405; it has not yet replaced the existing main instance. Live pre-migration MCP counts are 4,948 tracks, 36 playlists and 15 assets, retained as the preservation baseline. The isolated sparse availability benchmark completed with 50k p95 3167.65 ms and remains an unresolved performance regression; its stage timings and limitations are retained in the performance report.

Native verification checkpoint (2026-10-16): the current native library suite passes **672 tests, 0 failures, 14 ignored** (`/tmp/burrata-meld-bandcamp-native-tests-final.log`). The retained local full-length Bandcamp MP3 independently passes the native decoder duration and 95%-seek test (1/1), without another network fetch. This does not qualify Bandcamp's live range responses or the whole playback pipeline. The new default-off source experiment preserves existing pinned YouTube, SoundCloud and Monochrome routes. YouTube worker fixtures pass 34/34 for the Meld-derived WEB/WWW account-list route; actual account confirmation remains unverified. The sparse availability profiling job is still running; the next native GUI build remains pending.

Short-window UI follow-up (2026-10-16): a read-only screenshot of the verified main-window rectangle shows the footer clipped below a 1914×343 compositor surface. The WebView still reports 1914×480, so an empty DOM overlap report does not qualify visibility on the real surface. The native minimum height is now 240 instead of 480, and desktop/touch app-shell rules no longer force 480px. Search results explicitly retain single-line ellipsis in narrow panes. Focused frontend checks and the production build pass; visual verification on the rebuilt native app remains required. The active owned YouTube login window was focused during the read-only host check, so the existing GUI has been preserved.

Runtime revalidation (2026-10-16): the host-level process inventory confirms the existing main GUI PID 3699217 is still running. An earlier sandbox-only process inventory could not see host processes and must not be used as evidence that the GUI exited. The latest completed native source checkpoint passed 663 tests with 14 ignored; the subsequent experimental Bandcamp probe and availability optimization are still under review and require a fresh compile. The full frontend checkpoint passed 457 tests. No replacement executable has been launched in this revalidation, and no provider credentials or remote playlists were changed.

Membership and owned-page checkpoint (2026-10-16): the full native suite passes 661 tests (14 ignored). Discovery, automatic enrichment, browsing, radio recommendations and playback history now cache recordings without adding Library membership. Explicit imports, positive favorite/shortlist actions, playlist additions and saved assets promote the canonical recording; merges preserve membership. The additive v8 migration keeps every preexisting row saved. New tests drive actual Monochrome discovery over a loopback fixture, history persistence, merged aliases, save dates and legacy migration. Search rows remain retrievable and playable. Historical search pollution is not guessed away or deleted.

The same source captures bounded session context from the owned Linux YouTube Music page with origin/window/generation checks and a two-second deadline, retaining captured account context when native bootstrap omits it and rejecting mismatches. Linux Spotify and YouTube profiles are structurally separate and persistent; live profile recovery and YouTube account compatibility remain unqualified. All 34 YouTube worker, 19 Spotify worker and 12 provider-adapter tests pass. All 457 frontend tests, Svelte checks, frontend build, MCP tests and seven contracts pass. Scroll restoration now survives late source arrival and same-panel empty-result/remount cycles. Provider counts retain reported zeroes and survive album/playlist/import normalization without changing playlist sync hashes. The running application remains `f3924ca5…`; these source changes are not yet launched. Guarded sparse-metadata performance is being measured before the next executable build.


Source verification checkpoint (2026-10-16): all 455 frontend tests across 53 files pass, Svelte reports zero errors/warnings, and the frontend build passes. This includes compact mixed-result virtualization and Spotify album occurrence regressions. The provider-adapter, Spotify native worker and YouTube worker test files also pass. These source checks do not update the running native executable (`f3924ca5…`). The all-known, metadata-only 50k Store benchmark now measures first-page global sort availability at 180.09 ms p95 versus 184.11 ms for the legacy read (three warmups, twenty samples, development build); the sparse metadata follow-up exposed a 8663.71 ms p95 at 50k. Reusing filtered metadata reduced that to 3786.87 ms; binary JSON reduced it further to 2977.86 ms. That remaining cost is not yet acceptable for promotion. These are storage timings, not whole-app or playback measurements. A fresh owned-window YouTube account review reports incomplete sign-in before a provider request; no account was accepted and no write occurred. A pinned Meld comparison identified missing per-channel `DATASYNC_ID` handling. The source now preserves a bounded optional value from fetched bootstrap config, omits it for accounts_list and uses it for channel-scoped requests; all 31 YouTube worker tests pass. Active login-page JavaScript capture and live compatibility remain unqualified, and the full native suite now passes 649 tests (14 ignored) with fixture-server access; development executable `bf61b80a…` compiled successfully and is retained without launch. The running app remains `f3924ca5…` so its owned sign-in window is undisturbed.


Todo parallel checkpoint (2026-10-15): development build `f3924ca5…` passed 637 native tests (13 ignored) and launched through the verified frontend/MCP launcher as the sole main-profile instance. It includes the bounded YouTube authentication/session-cookie handoff, Linux-only persistent isolated YouTube profile, and cancellation-aware candidate-cache waiters. The first three-proof-cookie check after sign-in returned bootstrap `LOGGED_IN=false`; broader context is not yet live-qualified. The new owned window is ready for interactive sign-in. Playback-bar geometry at 1914×480 reports no findings. The frontend suite passes 444 tests; the compact-row follow-up passes 23 focused tests, with zero Svelte diagnostics and a successful build. Source-aware queue-radio preparation and resumable all-result collection budgets are implemented. Global local sort-field availability and a reproduced missing Monochrome pinned-candidate fix are being validated separately; neither is claimed in the running native build. No remote writes or public release occurred.


YouTube sign-in checkpoint (2026-10-15): the user completed sign-in in the main owned window and explicitly authorized account review through MCP. Bootstrap fixes now separate numeric client headers from symbolic context names and supply a desktop user agent. Review then reached the API but found no selected account or active account header; no identity was accepted. A source-backed account-menu fallback retains the stable-channel-ID guard. The native collector had discarded two of the three already-allowed authentication-cookie variants; the corrected collector and worker preserve that family and generate independent matching proofs without reading other Google cookies or ordinary browser profiles. Twenty-four focused worker tests pass. The final native suite passes 636 tests (13 ignored); all 437 frontend tests pass, Svelte reports zero errors/warnings, and the frontend build passes. Development build `69576e11…` launched on the original profile with a mounted frontend and 81 tools; its owned YouTube sign-in window is open. No account is confirmed, no live write occurred, and the current sign-in window is preserved until the replacement is ready. The guided connection UI has nine focused passing tests. See [sanitized diagnostics](performance-results/2026-10-15-youtube-native-bootstrap/README.md).

Search collection is now independent of queue continuation, repeated/cycling source cursors stop with an incomplete-results warning, and overflow duplicates cannot bypass the collection cap. Playlist detail identity changes cancel old reads and reject stale responses; repeated cursors retain loaded occurrences and report partial scope. Thirty-three focused pagination tests pass. The full frontend suite passes 436 tests across 51 files; Svelte checks and build pass. Interactive remote browsing requests 30 records while full import batches retain 100; the native suite for that checkpoint passes 634 tests with 13 ignored. These changes do not establish live YouTube browsing or writeback success. The corrected native build has since replaced that instance for fresh sign-in verification.

Spotify artist diagnostic follow-up (2026-10-15): a single read on corrected development executable `529e1c47…` proved an actual `catalog_rest` HTTP 429 with a 29-second retry hint (1596 ms elapsed), rather than a read timeout. The request was not retried in a loop. Existing account/GraphQL reads and artist REST access must be treated separately; no artist/album page success is established. The provider-test window was closed and the sole main-profile instance restored with a mounted frontend and 81 tools. See [retained diagnostic follow-up](performance-results/2026-10-15-spotify-artist-readonly/README.md). No credential transfer or remote write occurred.

Album filter consistency checkpoint (2026-10-15): non-paged main track lists and artist tracks now include album names in their text query without removing their existing match fields. Focused App/ArtistPage coverage verifies album-only matches, excludes unrelated rows and intersects with duration bounds (173 component tests pass). Svelte checks report zero errors/warnings and the frontend build passes. The current launcher also checks provider/companion assets embedded by Rust, and a real stale-worker check correctly refused the prior executable. The rebuilt development executable `529e1c47…` passed all 633 native tests (13 ignored), then launched successfully through the strengthened freshness guard. Exactly one main-profile instance is running with the frontend mounted and 81 MCP tools. This does not complete the broader sorting/todo scope.

Provider error-classification checkpoint (2026-10-15): Spotify HTTP read deadlines now return `REQUEST_TIMEOUT`, separately from true HTTP 429 `RATE_LIMITED` responses. The shared executor, native worker and companion error allowlist retain the distinction. A native regression verifies timeout versus 429, including preservation of the latter's numeric retry hint and redaction. Existing reviewed-write ambiguity behavior remains `OUTCOME_UNKNOWN`. Direct suites pass: native Spotify worker 18, provider adapter 10, companion background 14, Spotify authentication 4, YouTube worker 17; seven contracts pass. The live artist driver now records only allowlisted phase/status/retry fields on future failed attempts; the earlier limited report was not retroactively inferred or rerun. General structured cooldown propagation across Rust/UI is still incomplete. No account intervention or remote write occurred.

Spotify artist live checkpoint (2026-10-15): new read-only MCP tools expose the existing artist and album browsing service, including exact ID checks and round-trip decimal-string cursors. All 633 native tests pass (13 ignored), plus four MCP transport tests and seven contracts. In the retained isolated native Spotify session, the first artist operation reported `RATE_LIMITED` after 1806 ms; no retry or page reads followed. The phase/HTTP status was not captured, and that worker revision also conflated read timeouts with rate limits, so a genuine HTTP 429 cannot be inferred retrospectively. Live artist paging remains unqualified. Sanitized session flags remained Spotify confirmed/native-ready and YouTube unconfirmed; no account confirmation, import, credential migration or remote write occurred. The current `c32e0ad8…` executable has been restored as the sole main instance on the original data/config roots, with its frontend mounted and 81 MCP tools. The actual main search UI separately returned six Monochrome results for the reported Poso query. See [live evidence and limitations](performance-results/2026-10-15-spotify-artist-readonly/README.md).

Spotify browsing follow-up (2026-10-15): profile playlist rows no longer inherit the browsed profile's owner identity or display name. Owner navigation is offered only for a valid explicit owner URI or an owner encoded in a legacy playlist URI; followed playlists with unknown owners retain unknown metadata. The shared adapter suite passes 10 tests, including owner attribution fixtures. Native worker fixtures now cover artist identity/auth headers, explicit API denial and two-page release cursors (17 worker tests pass); these do not establish live artist API access. Spotify album tracks expose all shared sort fields with an explicit collected-results scope and wrapping controls. Component regressions verify descending BPM after appending a page, missing-last behavior and equal whole-second duration bounds. Svelte diagnostics report zero errors/warnings; the frontend build and seven contracts pass. The full frontend suite passes 429 tests across 51 files; the final scope-label change also passed the focused component suite and frontend build. Full profile completeness and live artist paging remain unqualified, and no live writes were performed.

Playback tracing checkpoint (2026-10-15): controller-originated requests now receive native operation IDs when a client does not provide one. Startup and seek command byte counts measure reserialized controller JSON; complete controller-receipt seek timing is separate from the existing dispatch-to-audio timing. Live testing exposed and fixed prepared-next-track adoption retaining the prior startup ID. The targeted rebuilt control now verifies distinct startup, seek and transition IDs while preserving duplicate queue occurrences. The full native suite passes 632 tests with 13 ignored. One isolated public Poso request played at the expected duration through Monochrome (2010 ms to first mixer consumption); both Sasa metadata shapes returned a meaningful version-unavailable reason under 20 seconds. These single observations do not establish performance percentiles or complete Sasa source coverage. Actual playbar geometry at 954×1036 had zero findings. See [retained successes, failed control and corrected recheck](performance-results/2026-10-15-sasa-operation-ids-live/README.md). The sole main instance has been restored as `8fb80541…` on the original data/config roots with a mounted frontend and 79 MCP tools. Its read-only playbar check also reports zero findings. No remote writes or credential transfer occurred.

Spotify profile prefix checkpoint (2026-10-15): bounded, fingerprinted Load more
requests retrieved 77 distinct playlists instead of the earlier 49, alternating
ID and official URL references. It stopped on no growth with no duplicates.
Provider totals varied from 89 to 136 and remained larger than the exposed list,
so the result remains incomplete. Profile completeness no longer ignores dropped
rows, legacy playlist URIs are supported, and the UI clears obsolete partial
warnings only on a confirmed complete terminal response. Adapter/worker tests pass
10/14 respectively after the live-discovered short-growth fix; the preceding full
checkpoint passed 629 native / 428 frontend tests and seven contracts. See
[live prefix evidence and remaining limits](performance-results/2026-10-15-spotify-profile-prefix/README.md).
The sole main instance now runs development executable `500f9042…` with its
frontend mounted; a read-only playbar check reported zero findings at 954×1036.
No remote writes or credential transfer occurred.

Live Spotify read checkpoint (2026-10-15): the retained isolated confirmed session
successfully used native HTTP without the companion. Profile browsing by ID and
official URL returned 49 playlists but explicitly reported a truncated result
against 108 total. Two library pages returned 97 distinct playlists with no
duplicates and further paging available. The actual profile-search UI returned
30 profiles for the public query `Spotify`. No login/confirmation, credential
migration, import or remote writes occurred. See [live read evidence and limits](performance-results/2026-10-15-spotify-readonly-session/README.md).
Profile pagination remains incomplete. New regressions verify actual candidate
search and reject local edits made during remote validation before dispatch.
The remote-playlist suite passes 64 tests; the full native suite passes 629 with
13 ignored, and all seven contracts pass.
The isolated provider app was closed and the sole main instance restored using
development executable `db663c5a…`; its mounted-frontend handshake passed with
79 MCP tools. The retained Spotify credentials remain confined to the isolated
provider profile.

Reviewed cross-catalog writeback checkpoint (2026-10-15): unresolved local tracks
can search destination candidates, explicitly choose a recording and regenerate
the complete diff before a separate Apply action. Candidate tokens are scoped to
the binding/account/recording, bounded and expiring. Stale local identities and
ordered occurrences block apply. Duplicate occurrences share the recording match
without sharing occurrence identity; successful writeback advances the baseline
while preserving local tags and source identities. All 62 remote-playlist tests
and the full default native suite pass (627 passed / 13 ignored). The full
frontend suite passes 427 tests across 51 files, with the updated 15-test sync UI
suite also passing after the clear-choice assertions. Seven contracts pass.
See [matching behavior and remaining limits](writeback-recording-matching.md).
Live account matching/writeback remain unqualified; no live remote writes occurred.
The rebuilt development executable `8c02c680…` is now running on the original main
profile with a mounted frontend and 79 MCP tools. A read-only actual-app playbar
inspection reported zero findings at 954×1036 (footer 94.5 px); exactly one Burrata
GUI process was verified. This is not a visual qualification of the new matching
picker against a live provider account.

Provider cancellation/recovery checkpoint (2026-10-15): Spotify worker requests
now reject an already-aborted caller before dispatch, propagate in-flight aborts,
and install timeout/listener resources only after request validation. Direct Node
tests pass: Spotify worker 13, YouTube worker 17, provider adapter 10. Seven
contracts pass. Managed Chromium recovery now returns an opted-in crashed session
to lazy-startable idle state. A second exit before dispatch reports an explicit
retry reason rather than silently routing that request to the ordinary companion.
The focused native provider-session suite passes 11 tests; live crash recovery is
not qualified. The running `dc800d5d…` executable predates this Rust recovery edit.
At that checkpoint, source inspection found the missing reviewed cross-provider
selection flow; the subsequent matching checkpoint above implements that flow.
No live account or remote mutation was used for these checks.

Playback cancellation / indicator checkpoint (2026-10-15): the default native
suite passes 622 tests with 13 ignored. Playback-facing nested acquisition now
treats only an explicit true cancellation value as cancellation, including
shared-cache workers and waiting callers; a deterministic closed-false-channel
regression passes. The read-only player layout inspector now returns fixed-region
visible/current/paused/buffering row counts without track metadata. Its selectors
were checked against all five rendered list implementations, and clipping by outer
scroll panes has a regression test. The full frontend suite passed 423 tests in
51 files before that last clipping regression; the updated inspector suite then
passed all seven tests. Svelte diagnostics and the frontend build pass. Evidence:
`/tmp/burrata-nested-cancellation-full-native.log`,
`/tmp/burrata-final-indicator-frontend.log`, and
`/tmp/burrata-track-counts-build.log`.
These native changes were then rebuilt as development executable `dc800d5d…`.
An isolated muted native WAV played, and its visible search row correctly showed
current and paused indicators. Both actual DOM reports had zero player geometry
findings at the existing size. See [current indicator evidence](performance-results/2026-10-15-current-indicators/README.md).
The isolated fixture was closed before restoring the main profile with this build.
Actual-app Sasa and indicators in other lists remain outstanding. No account
confirmation or remote playlist write was performed in this checkpoint.

Latest public-duration checkpoint: default and custom-protocol native suites
each pass 611 tests / 13 ignored; frontend passes 417 tests across 51 files,
with clean Svelte diagnostics, frontend build, seven contracts and three MCP
bridge tests. The sole main instance runs development executable `25ac105a…`
with 79 tools. Two bounded, explicit public-metadata passes repaired all 396
missing Spotify durations in this library without sign-in; 182 known values,
manual metadata, playlist occurrence order/memberships and asset durations
remained unchanged. The exhausted repeat processed zero tracks. See
[live repair evidence](performance-results/2026-10-15-public-duration-live/README.md).
Spotify/YouTube live writeback still requires confirmed sessions and separate
qualification; the broader playback/todo plan remains incomplete.


Subsequent collection/mirror checkpoint: default native tests pass 597 with
13 ignored; all 50 frontend files pass 413 tests; the favorite-ID UI wiring then
passes all 156 App integration tests, Svelte check and frontend build. Collection edits preserve paging, and move requests use original
occurrence positions under filters/sorting. The native legacy-collection
position query computes positions before filtering. Mirror sync now batches
snapshot state hydration and reuses unchanged track lists, while actual edits
reload persisted tracks before rendering. One paired 120-second development capture measured mirror phases at
2,791–2,972 ms before and 1,427–1,450 ms after, with app CPU-seconds
7.750 versus 4.960 (36.0% lower in this pair). This is exploratory; frontend
state/uptime were not fully matched. See [measurement limitations](performance-results/2026-10-15-mirror-batching-fixed/README.md).
The YouTube-only native resume control is repaired and its focused tests pass.
The existing-session read-only probe found no confirmed native account and
reported companion unavailable; live YouTube writeback remains unqualified.


Latest checkpoint (2026-10-15): the full default native suite passes 594 tests
with 13 ignored, including bounded helper-spawn retry and shared SQL prefix
ordering. The custom-protocol suite also passes 593 tests / 13 ignored. Frontend tests
pass 411 across 50 files after fixing coalesced metadata updates and preserving
filtered/sorted pages during event refresh. Svelte check reports zero errors
and warnings. The >1,000-row prefix parity regression passes, including nullable duration
ordering and global filtering before paging. The deleted-row race test also passes. The rebuilt main app
(`8e2bdfa4…`) passed its mounted-frontend handshake with 76 tools and the exact
YouTube query `poso s'agapo` returned 20 rows. Exactly one Burrata GUI process
remained. See [refresh verification](performance-results/2026-10-15-prefix-refresh/README.md). Earlier runtime and test counts below are historical
checkpoints.

The expanded queue/player was checked on an isolated muted local fixture at
360×640, 500×800 and 1000×650. All sizes applied exactly and reported zero
geometry findings after fixing intrinsic text overflow. A visually inspected
360 px screenshot confirms the Identify/header/footer controls do not overlap.
See [expanded-player evidence](performance-results/2026-10-15-expanded-player-ui/README.md).
The main profile was preserved and closed during the sequential UI check.

A single paired 120-second development idle capture measured 13.63 versus
7.59 app CPU-seconds after suppressing the redundant frontend periodic sync
when the native watcher is healthy. This pilot pair is not release or
statistical qualification; see the [capture limitations](performance-results/2026-10-15-playlist-sync-fixed/README.md).


The subsequent automation checkpoint adds a bounded window-resize control and
a read-only saved-track paging tool. Default and custom-protocol native suites
each passed 552 tests with 13 ignored; two additional duration-argument boundary
tests passed separately. The actual-app library-scale driver has passed its
1k, 5k and 20k development-build scenarios; the 50k run is in progress.

The current main app is development build
`3c0a8b5d8fccf6800901f26afd807817e24f01d615520c32566224ebf62d63ac`,
with 73 authenticated tools and a mounted frontend. Poso played again in that
build. Actual paused-player geometry passed at 360, 500 and 1000 pixels wide
after temporarily floating the tiled window; tiling and volume were restored.
See the [responsive check](performance-results/2026-10-15-playbar-ui/main-responsive-paused-floating.json).

This is a status snapshot of work in the current checkout. It separates code
and deterministic test evidence from live-provider and release qualification;
it does not claim that the broader implementation plan is complete.

The preceding playback checkpoint's default and custom-protocol Rust library suites each
passed 547 tests / 13 ignored; the resolver small-stack regression also passed
explicitly. Full Cargo formatting passes. Frontend passed 391 tests across 49
files, followed by a targeted 3-test layout regression; Svelte reports zero
errors and warnings, and the frontend build passes. Launcher fixtures and seven
IPC/schema contracts pass. Unchanged earlier evidence includes extension 67,
Spotify/YouTube workers 8/17 and MCP bridge 4 tests. The rebuilt main app passed
its authenticated mounted-frontend handshake with 71 tools.

The preceding main playback checkpoint used debug SHA
`68dfb9bfc202b28ba2b40b69625109e7614f6482a5961bb6346062b9396e5734`.
It runs against the existing library after a private backup, preserving the
Monochrome-first preference. [Main-app checks](performance-results/2026-10-15-playbar-ui/qualification.md)
verified Poso advancing at 245.48 seconds through Monochrome, plus no measured
playbar overlap in initial, playing, paused and unavailable states. Sasa's exact
281-second Main Version found no eligible matches among 0/20/10 returned
Monochrome/YouTube/SoundCloud results; its actionable failure was visible after
9.7 seconds. An isolated native Monochrome run passed 20 starts and 60 seeks,
including paused seek/resume; cold first mixer audio was 2,613 ms once and warm
startup p95 922 ms. One-provider coverage does not qualify cold p95 or fallback.
The [30-minute release memory run](performance-results/2026-10-15-isolated-memory-30m-release/summary.json)
also completed without observed local playback failures.

The current default executable passed the authenticated mounted-frontend
handshake in a new isolated profile with the guarded Music fixture. Sanitized
status confirms `native-webview-http`. A user-driven Spotify sign-in and
account confirmation succeeded; the login window closed, a playlist listing
returned 48 entries in 2.438 seconds, and an authenticated restart recovery
reused the OS-keyring credential without interactive sign-in. The user also
reported a successful playlist import, not independently content-verified.
Public profile browsing through the Developer Web API is unavailable to
affected Development Mode apps because Spotify's [migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
says `GET /users/{id}/playlists`
without a replacement; `GET /me/playlists` is current-user scoped. The live
request did receive 429 before this policy limit was reviewed, and the app's
quota classification is not established. A separate undocumented Spotify web
profile-view endpoint has now returned 46 playlist entries for one public
official account in 2,188 ms; it had valid IDs and artwork, with no cursor or
error. This is one bounded private-endpoint read, not qualification of profile
search, pagination, UI, arbitrary accounts or completeness. Imported
track playback reached a reported playing status with 5.48 seconds of position
progress. The launcher did not force the native playback engine, engine identity
was not reported, and no first-mixer-audio evidence was captured. A later default-WebView shared resolver attempt crashed before provider work.
Boxing the resolver futures addressed that reproduced crash; the subsequent
[isolated live check](performance-results/2026-10-15-shared-webview-boxed-resolver/live-playback.json)
verified default-WebView playhead advancement with a 250.079-second source.
[Seek checks](performance-results/2026-10-15-shared-webview-boxed-resolver/stable-seek-status.json)
verified advancement from zero and 200 seconds; the near-end check advanced
but buffered intermittently and remains unqualified. Playback was muted, so
physical audibility was not measured. The earlier native duration failure was traced to Rodio: the
exact muxed MP4 contains 250.079 seconds of audio but Rodio reports 170.079.
A validated muxed-MP4 FFmpeg routing fix passes the exact private-file
late-seek-to-EOF regression test and one live native correctness gate. That
gate reported 250 seconds and advancing playback using FFmpeg; first mixer
audio at 7,489 ms still misses the five-second target; no performance conclusion follows from the incorrect-duration runs. The new
[repeated debug run](performance-results/2026-10-15-native-muxed-ffmpeg/summary.json)
completed 20 starts and 60 seeks without failures, plus paused seek/resume.
Cached startup p95 was 522 ms; this does not qualify cold startup or other sources. See the [live
qualification record](performance-results/2026-10-15-native-provider-session/live-read-qualification.md).

Playback-bar visual checks reproduced radio-toggle text overlapping the previous
control. The toggle now displays an icon while retaining its accessible name and
tooltip. Browser preview checks at 1920, 1100, 800, 650, and 500 pixels wide show
separated transport controls; the track-information panel was also checked at
1100, 800, and 650 pixels. Screenshots are retained under
`performance-results/2026-10-15-ui-check/`. These checks use a restored radio row
in the browser preview, not a native playback/device or long-title qualification.
A separate fresh 650×800 browser preview with the paused footer showed no
control overlap; its screenshot is
[playbar-650.jpg](performance-results/2026-10-15-ui-check/playbar-650.jpg).
The compact buffering ring was adjusted to remain inside the play button, but
buffering and long-title states have not been visually checked.

The [default-player Identify check](performance-results/2026-10-15-webview-identify/qualification.md)
returned a title matching Poso in 1,746 ms after verified muted playback. This
is one live excerpt, not broad recognition or latency qualification.

## Implemented with deterministic validation

- The player, native playback path, queue controls, and source-resolution
  fallback are implemented. Linux Rust/frontend tests and a generated-WAV
  no-device decode check are recorded in [native-playback qualification](native-playback.md).
  These checks do not establish physical-device behavior or clean-machine
  playback.
- Startup and seek operation IDs now connect renderer command entry to native
  IPC receipt and playback phases. Startup snapshots expose click-to-IPC,
  serialized command size, source-ready, and first-audio diagnostics. Nested
  search/extraction/probe/retry timings are now correlated through a resolution
  UUID and shared-lookup UUID. Automatic joins to process samples remain open.
- Search/list UI has tested loaded-result duration bounds, shared track sorting
  and filtering, playlist-name filtering in the left pane, and Spotify artist
  result navigation. Relevant coverage is in `SearchPanel.test.ts`,
  `track-sort.test.ts`, `LibrarySidebar.test.ts`, and `SearchPanel.test.ts`.
- Implemented UI paths also include canonical playing indicators, an off-by-default
  search queue snapshot, an off-by-default queue-end radio toggle, explicit upcoming
  queue sorting, searchable playlist destinations, primary Add to playlist actions,
  optional reported views/plays, independent source-opening settings, and current
  PCM identification. The complete frontend suite, including the final artist pagination guards,
  passed 354 tests across 46 files. Latest playback UI integration tests passed
  133/133 after batched automation IDs.
  This is regression evidence, not a claim that every interactive playback path
  has been manually exercised.
- Local track reads now support database-wide sorting/filtering before 200-row
  pagination and batched ID lookup. Remote search has bounded 30-row audio pages,
  query-bound cursors, loaded-result sorting, and cancellable bounded collection.
  Spotify artist pages preserve provider IDs/artwork and browse provider releases;
  artist/profile availability still depends on actual provider account access.
- Spotify duration normalization preserves fractional seconds and known values.
  Import/refresh includes bounded exact-ID backfill with manual/chapter guards.
  Provider adapter and persistence fixtures cover the supported response shapes;
  authenticated background imports and writeback still need live qualification.
- The library-discovery comparison script is parameterized and its synthetic
  1k/5k/20k/50k scale run is saved in
  [the benchmark report](performance-results/2026-10-15-library-synthetic/README.md).
  This measures Python SQLite/JSON work only; it is not an application
  performance result.
- A separate [running-app local WAV check](performance-results/2026-10-15-isolated-memory-smoke4/qualification-notes.md)
  passed three warmups, 20 starts, 60 seeks, and paused-seek/resume with no
  failures. Native session-start p95 was 81 ms and seek p95 was 87 ms. The
  6,407 ms worst harness wall observation and host `~/Music` scan confounder are
  recorded there; the private app database did not make the library workload
  fully isolated. This does not qualify remote playback or the pending memory
  run.
- Release CI configuration builds platform packages and runs scripted
  installer smoke checks. The details and limits are in
  [release readiness](release-readiness.md); this is not equivalent to
  reviewed public release qualification.

## Still pending qualification or implementation

- Correct recording identity, uninterrupted provider playback, seek/recovery
  behavior, and controlled comparisons across at least 20 attempts on the named
  provider matrix remain unqualified. Native local playback testing first
  produced mixer audio after 7,132 ms, missing the 5-second limit. A later
  warm 20-start sequence produced fresh generations, but the native-reported
  duration of 170.079 seconds disagreed with the verified 250-second source,
  so qualification halted before seeking and no seek measurements were
  collected. Another full-identity sample reported 251
  seconds against 245.48 seconds and was rejected. A subsequent default-WebView
  attempt crashed during resolver setup before provider work. Boxing the
  futures addressed the reproduced crash, and later live default-player and
  repeated native correctness checks passed as recorded above. Cold startup,
  the wider recording matrix and cross-platform playback remain unqualified. Spotify sign-in and playlist reading
  succeeded. Restart recovery succeeded once through the OS keyring. The
  attempted public REST owner-playlists request returned HTTP 429 with a
  22-second retry hint; after that cooldown, one retry remained rate-limited
  with an eight-second retry hint, then stopped. Spotify's [February 2026
  migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
  says `GET /users/{id}/playlists` has no replacement; `GET /me/playlists` is
  for the current authorized user. The guide says Development Mode apps must
  remove other-user playlist/profile fetches, while Extended Quota Mode apps
  are unaffected. The app's quota classification remains unresolved. A
  candidate internal endpoint is
  `GET https://spclient.wg.spotify.com/user-profile-view/v3/profile/{id}` with
  bounded `playlist_limit`, `artist_limit=0`, `episode_limit=0`, and
  `market=from_token`; the pinned [Spotifly `getUser` source](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/index.ts)
  and [user type](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/types/user.ts)
  describe it. One read-only request succeeded for a public official account:
  46 valid 22-character playlist IDs and 46 artwork URLs in 2,188 ms, without
  cursor or error. The app had only hot-staged worker/provider-operations JS;
  its WebView session was retained. The endpoint has no cursor/offset contract,
  so the result may be incomplete; this does not validate other accounts,
  profile search, pagination, UI or long-term availability. The endpoint is
  undocumented and unsupported. Thus this 429 is an observed failure
  for a request to a route that is no longer supported for affected apps, not
  evidence that the public profile endpoint is merely waiting for retry.
  Corrected bearer forwarding passes tests, but public profile browsing via
  the developer API is not available for affected apps. An imported
  track reached `playing` status and a 5.48-second position advance, but the
  playback engine was not identified and no first-mixer-audio evidence was
  collected for that provider-session run. Existing
  synthetic decoder results do not satisfy the playback requirement.
- Full nested acquisition tracing, automatically correlated process-group CPU
  and I/O, and whole-application measurements under controlled workloads.
- A later shared-profile default-WebView playback attempt timed out at the MCP
  client after 25 seconds and the app exited with SIGSEGV (exit 139) at
  `resolution_start`, before provider work. The function-only stack starts at
  `tokio::time::timeout<playback_sources::resolve_inner future>` and runs
  through `within_deadline`, `resolve`, `resolve_with_playback_budget`, and
  `resolve_playback_track`. Large unboxed-future stack exhaustion is suspected,
  not confirmed; this is not evidence of SQLite lock contention. See the
  [redacted crash summary](performance-results/2026-10-15-shared-webview-resolver/crash-summary.json).
  This failed checkpoint is retained as historical evidence; later boxed
  resolver playback passed. Broader crash/recovery stress coverage is pending.
- A 30-minute whole-app memory run. None is recorded in the current evidence;
  the reproduced native duration and resolver crash issues now have passing
  checkpoints. An embedded-frontend release checkpoint is being prepared
  before the long run to avoid contamination from frontend hot reloads.
- Manual native playback and reliability checks on clean macOS, Windows, and
  Linux systems, including device removal and sleep/wake. Public release
  qualification remains on hold until these checks are reviewed.
- User-reported issues that still appear in [`todo.md`](../todo.md), including
  playback skips, result sorting, current-track behavior in search, source
  opening controls, now-playing identification, and provider playlist sync.

## Internal experiments

After the user raised browser resource concerns, a second Spotify prototype is
implemented behind `BURRATA_EXPERIMENTAL_NATIVE_PROVIDER_SESSION=1` with a
login-only Tauri WebView and bounded HTTP worker jobs.
It reuses the extension's Spotify executor and captures the account session only
through the owned login window. Credentials belong in the OS credential store;
memory-only fallback must be disclosed. The combined extension and HTTP worker suite passes
67 tests. Native unit tests pass. Interactive native Spotify sign-in,
confirmation, a read-only playlist request, and one keyring restart recovery
now have live success. The profile API header fix was followed by HTTP 429s and
no successful browse. Spotify's [current migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
now establishes that
the attempted arbitrary-user endpoint has no replacement for affected
Development Mode apps; `GET /me/playlists` is current-user scoped. Burrata's
  quota classification remains unresolved. The private endpoint has one
  successful bounded profile-ID and URL reads recorded above. A direct
  profile-name search also returned 20 valid profile rows; complete playlist
  coverage and long-term stability remain unqualified.
Playback status advanced for an imported track, but
engine identity and mixer audio remain unverified for that provider-session
check. Later isolated debug evidence verifies native duration correctness,
20 starts and 60 seeks; the initial startup sample still misses the five-second
threshold, as detailed in the playback performance record. The earlier live Chromium preflight stopped before opening a
provider page because its debug native-host helper was missing; a private debug
fallback has been implemented and awaits separate live qualification.

The separate managed Spotify/YouTube Chromium transport is gated by
`BURRATA_EXPERIMENTAL_MANAGED_PROVIDER_SESSION=1` on supported Unix builds.
Settings then offers an isolated Chromium login, explicit account confirmation,
and a background helper. The helper keeps provider cookies in its own persistent
profile, stops after 60 idle seconds, and retains the companion as fallback.
Spotify writes still require the existing explicit opt-in and preview; YouTube
remains read-only. Its authenticated login, headless background compatibility,
restart, expiry, and account-switch qualification are pending; the native
WebView/HTTP live evidence above does not qualify this separate Chromium path.
Neither prototype establishes complete browser-independent provider support.

The native Opus prototype requires the Cargo feature `experimental-opus` and is
off in default builds. It pins Symphonia 0.5.5 and the libopus adapter 0.2.9 and
retains FFmpeg fallback. Generated-fixture checks and decoder measurements are
distinct from app-level seek qualification. Promotion still requires the planned
30% seek-start p95 improvement, no material steady-playback CPU increase, and
Linux/macOS/Windows qualification.

The [debug decoder comparison](performance-results/2026-10-15-opus-debug/notes.md)
recorded three warmups and 60 measured seeks per path, with zero failures.
Per-position p95 first PCM was 49–57 ms for retained Opus versus 93–102 ms for
fresh FFmpeg. This does not establish app-level seek latency or steady-playback
CPU, and no release-profile result is claimed.

The current evidence supports implemented components and deterministic
regression coverage. It does not support claims that the full plan is complete,
that provider performance targets have passed, or that native playback is
qualified for public release.

## Latest provider UI checkpoint

The destination profile search now sends the singular `profile` kind required by
the backend, accepts official profile IDs containing `._-`, and rejects stale
results after the query or mode changes. Direct authenticated MCP checks
returned 20 profile-name results and a bounded 46-playlist official profile URL
result (`totalCount:1545`, incomplete), recorded under
`performance-results/2026-10-15-webview-identify/`.

YouTube’s obsolete blanket read-only UI block has been removed from sync
previews. Both providers require explicit write opt-in, an editable current
preview with valid revisions, and no conflicts, errors or unresolved entries.
The backend continues to enforce account, revision, ownership, occurrence and
uncertain-outcome checks independently. The focused provider UI suites pass;
no live remote writes have been performed, and YouTube connection qualification
is still awaiting the user’s explicit existing-session handoff.

## Changes after the frozen release checkpoint

The embedded release benchmark uses binary
`cc3e982f4a054c19428854e4de14d5b3f7da3fdff0c5b84826522932882dc639` and the
[source manifest](performance-results/2026-10-15-release-checkpoint/source-manifest.json).
Later source edits do not change that running binary or its measured scope.

The following source fixes were applied after this checkpoint and passed the
latest default/custom-protocol Rust verification: local paged text filtering
now includes version and genre; SQL duration sorting uses the displayed whole
second consistently with frontend/native sorting; incomplete chapter ranges
remain unknown; native zero duration/BPM remains unknown while reported count
zero stays known. Added regressions cover matches outside the first page and
stable equal-duration ties. An audit also confirmed that native descending
null ordering was already correct; no null-ordering repair was needed.

The MCP discovery validator now accepts Monochrome, which it advertised in its
schema but previously rejected. Schema and request validation share the same
source list, with a regression checking every advertised source. This corrects
local routing; it does not override or qualify the hosted provider's observed
HTTP 403. The latest 547-test checkpoints include these source edits.

Nested safe playback tracing is compiled, tested and exercised in the current
main application. It correlates native startup/seek operation IDs
and WebView request generation with a resolution UUID, propagates a separate
shared-lookup UUID across coalesced waiters, and records DNS, provider preparation,
extraction, probe attempts and retry waits without source URLs or error bodies.
The remembered mapping, alternate-format and decoder/CDN recovery paths are
included. New tests cover source/caller identity and independent joiner
cancellation. They pass in the latest Rust checkpoint. The separate immutable
release memory capture completed before these verification jobs began.


Shared sorting now uses pinned ICU4X 2.3.0 for native/SQLite text comparisons and
an explicit en-US browser collator, with shared Unicode/numeric/date fixtures.
Rust and SQLite share strict count/date normalizers; durations use displayed
whole seconds consistently. Provider reads now have bounded, independent
cancellation and RAII cleanup. Remote browsing rejects stale responses,
collections cancel in-flight reads at two minutes, and over-returned page rows
are buffered instead of lost at the 10,000-track action cap. Playlist previews
retain duplicate occurrences and distinguish page-local positions. Source
opening preferences cover footer/Details/radio links and snapshot click-time
position before awaiting pause. Error details are visible in a wrapping player
row with Retry and Track info; retry preserves the current duplicate occurrence.
The new read-only MCP layout inspector returns geometry without track metadata.

## Search and provider qualification checkpoint — 2026-10-15

The exact query `poso s'agapo` exposed a mixed-kind YouTube search regression:
failed authenticated playlist/channel lookup aborted independent public track
searches. The native resolver now retains public results. App-specific MCP
`search_ui` reproduced zero results before the fix and 30 results afterwards;
redacted evidence is in `performance-results/2026-10-15-search-ui-repro/`.
No desktop keyboard or mouse input is required for this check. Search controls
also explicitly override global header/label spacing; the native UI was visually
checked after that change.

The later provider checkpoint passed 575 native tests (13 ignored), with both
default and custom-protocol configurations, and 399 frontend tests. Frontend
checking reported no errors or warnings. Buffered discovery pages preserve
partial status and warnings, and MCP accepts the same bounded discovery cursors
as the native decoder. These are functional checks, not release performance
qualification.

The read-only Spotify exact-track probe reached the catalog API through the
restored native session but received HTTP 429 twice, including a retry more than
105 seconds after the first response's 36-second cooldown. No remote writes
were performed. Existing verification imports have no missing durations, so
this does not prove a live duration repair. See
`performance-results/2026-10-15-spotify-exact-track/`. Browser-free YouTube login
and live Spotify writeback remain unqualified; no completion claim is made.

Live MCP pagination subsequently accepted a 69,811-character buffered YouTube
cursor, returned the next single-result page, and preserved partial status and
warnings. The check uses no desktop input; its safe report is
`performance-results/2026-10-15-search-ui-repro/buffered-cursor.json`.
The current main profile was relaunched through the verified launcher on binary
`1d554c0c7771ecfdb5da3f7c2f9e06071c3ea198959e7bf875fb29e8be227cfa`;
the authenticated handshake confirmed 75 tools and a mounted frontend.

The user subsequently expanded the scope to include YouTube writeback, still
requiring explicit opt-in and a preview. A backend audit found a concurrent
preview-apply race: two callers can clone one preview before either consumes it.
An atomic claim and mocked concurrency regression are being prepared; this is
an implementation gap independent of pending live session qualification. No
remote playlist writes have been performed. The user also requires only one
Burrata instance at a time; release benchmarks therefore temporarily close the
main app and run isolated profiles sequentially.

The next immutable Linux release benchmark binary is
`67025d222c61a8dbb1189f7408028155104f0e1def189ee6cacc723298424f4c`.
Its compilation completed locally; this is not a public release or performance
pass. Results must remain separate from the earlier debug library matrix and
the earlier release memory capture.

## Writeback guard and release matrix follow-up

The binding-scoped writeback claim now serializes actual apply calls for both
providers, including distinct previews of the same playlist. The focused native
provider suite passed 40 tests: the real YouTube apply entrypoint uses a blocked
mock fetch, rejects a second apply before mutation, then dispatches exactly one
mocked edit. Tests also cover release after review mismatch and cancellation.
Spotify duration repair stops at the first observed rate limit without moving
its chunk cursor or repeatedly retrying the background job. These source
changes are newer than the benchmark binary and still await a replacement app
build; no live writes were sent.

The immutable release matrix completed all 1k/5k/20k/50k sizes, five scenarios,
three warmups and 20 measured reads each, with zero failures. All continuation
and identity checks passed. Full median/p95/worst latency, app CPU-seconds and
limitations are in the
[release matrix report](performance-results/2026-10-15-library-page-matrix-release/README.md).
At 50k, median backend read latency ranged from 472 ms to 1,421 ms. These are
backend MCP measurements, not UI rendering latency or a historical engine
comparison. The main app was restored after all isolated benchmark instances
closed, preserving the one-instance constraint.


Latest seek evidence: direct-v5 local WAV retained readyState2/buffering while five seconds of advancing media and isolated private-sink PCM remained nonzero at the pre-seek level. Two direct runs and one active paused-seek control have output metrics, with finite ownership/cleanup checks. This distinguishes the readiness signal from silence for those controls; no p95, acoustic seek-position, physical-audibility or cross-codec claim is made. See `docs/performance-results/2026-10-17-isolated-seek-output/README.md`. A UI-only buffering recovery fix is being prepared privately.
