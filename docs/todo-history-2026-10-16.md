# todo
- we need to have a directory browsing view as well as our playlist view, user can add dir that should be able to be browsed.
- in queue should be able to click/drag to change tracks positoin, or ctrl click to select, or shift click also for multile consecutive, then arrow keys to change position up or down.
- if I search for spotify playlists I should be able to add a playlist to library, either from the serach overview or the playlist view. should be able to select items and add them to any playlist. this goes for non-spotify items also. ie album view, other playlist view. 
- profile browsing only shows 4 playlists, can we browse through all or not?
- "PROVIDER_CHANGED: Spotify returned a response that did not match the request. (phase=catalog_graphql, HTTP 200)" when trying to open spotify artist view album
- why can't I play this song, it just skips: "34 ▶	
Sasa
Timmy Regisford · Main Version (4:41)"

Latest authoritative checkpoint (2026-10-16): bounded actual isolated local duplicate/chapter saves pass. Destination ends with four ordered entries `[full,full,full,canonical chapter]`, source remains3, integrity is ok and foreign-key errors are zero. The corrected chapter picker target was actually viewed before UI Add. Its initial `Invalid track identity` was correct rejection of a malformed fixture ID; isolated-only atomic reference migration repaired that fixture without changing range or recording/asset mapping, retaining the failure evidence. [Safe final counts](docs/performance-results/2026-10-16-youtube-channel-identity/occurrence-ui-local-final-counts.json). Captures and raw snapshots remain private; no raw private snapshot is published.

Final isolated runtime is emptyqueue0/index−1/pausedtrue/idle/position0, with no audio started. Main is not restored. Local duplicate/chapter saving is accepted only for this fixture; remote import and all-view UI acceptance remain pending, so overall bulk import/selection stays open. Current queue checks pass27tests across two files in2.80s; directory integration and the new queue build remain pending. User-authored directory, queue and cross-provider bullets above remain unchanged and open.

Historical availability-offset source checkpoint passed806 native tests,0failures,16ignored, ten focused checks,186Node tests, Svelte0errors/warnings and production build5.43s. The earlier main988a launch preserved paused state exactly; the subsequent isolated handoff is recorded above. The selected YouTube-playlist acceptance below is historical61b5 evidence; broader provider acceptance remains open. Full frontend run74148 passes **610tests/62files in94.70s**; all recorded source hashes match after the run. The earlier599-test baseline remains historical. [Verified full-suite manifest](docs/performance-results/2026-10-16-youtube-channel-identity/provider-ui-current-full-frontend-manifest.json). [Live GUI manifest](docs/performance-results/2026-10-16-youtube-channel-identity/anchor30-gui-manifest.json), [new source manifest](docs/performance-results/2026-10-16-youtube-channel-identity/availability-offset-native-manifest.json).

Fresh61b5 YouTube read passes the bounded selected-playlist check:240unique occurrences in116+113+11pages,total240,terminal `complete=true`. Available flags are106/105/10 and unavailable flags10/8/1, totaling221true and19false. This qualifies that playlist's count/availability handling, not all accounts, playlists or acoustic playability. [Live availability evidence](docs/performance-results/2026-10-16-youtube-channel-identity/youtube-61b5-availability-counts.json). Earlier f991 incorrectly-true availability remains historical.

Historical988a Spotify profile paging returned30+30=60unique playlists, then page3 fails: this is partial, not complete. Inspector totals vary by page:0/30→30rows/110total;58/30 and60/30→29rows/109total;90/30→10rows/90total. Investigation continues without a further fix claim. [Partial profile evidence](docs/performance-results/2026-10-16-youtube-channel-identity/spotify-profile-988a-partial-counts.json). The anchor build passes806native/four focused,189Node/52focused. ReleaseView duplicate-position and artist-expanded-release duplicate playback indexing under descending source order are source-fixed, with five+19targeted tests, Svelte0errors/warnings and production build4.62s. These two new tests postdate the matching-source610-test full baseline; no full612-test claim is made. [Check](docs/performance-results/2026-10-16-youtube-channel-identity/release-artist-occurrence-check.log), [build](docs/performance-results/2026-10-16-youtube-channel-identity/release-artist-occurrence-build.log). No provider writes, audio or queue changes were made by these reads.

Bounded61b5 desktop visual checks cover main, queue and Info views at1914×515: finite geometry inspection reports `findings=[]`, footer98.5px and all controls visible, with queue/Info contained above it. Root viewed actual app-owned captures, retained privately under `.cache/private-ui-checks/61b5-{desktop,queue,info}.png`; screenshots containing track metadata are not published. The panel was closed afterwards, without queue/audio writes. This does not complete remaining desktop/half-screen UI qualification.

Bounded isolated988a playback checks: local before/after and duplicate-occurrence advancement pass five-second mixer progression. Corrected paused seek passes serial+generation advancement, position10, remaining paused without audio; resume passes five-second progression. A repaired chapter fixture preserves15-second identity and five-second progression, and the corrected terminal test reaches ended/max position15 while ignoring intermediate buffering. The original same-generation assertion and missing chapter asset mapping were fixture/test errors; an earlier chapter-end attempt stopped prematurely on buffering and remains a test limitation, not a silently discarded pass. [Corrected seek/remote](docs/performance-results/2026-10-16-youtube-channel-identity/isolated-paused-seek-remote.json), [chapter progression](docs/performance-results/2026-10-16-youtube-channel-identity/isolated-chapter-repaired.json), [terminal check](docs/performance-results/2026-10-16-youtube-channel-identity/isolated-chapter-terminal.json), [initial report](docs/performance-results/2026-10-16-youtube-channel-identity/isolated-initial-seek-chapter-report.json).

The one-shot overall report remains **FAIL**: two Sasa cases give meaningful unavailable results in4.725/6.031s, and Poso fails the duration gate. Poso separately passes five-second mixer progression but has actual duration250 versus expected245.48; an official-lyric YouTube title does not prove recording equivalence. Broad playback, exact Poso matching and correct-version Sasa playback remain open. [Overall report](docs/performance-results/2026-10-16-youtube-channel-identity/isolated-functional-overall-report.json).

Remaining actionable scope: verify Spotify continuation and extend provider acceptance beyond the one verified YouTube playlist; finish ordinary-track/DJ/concert recognition and cancellation acceptance, Sasa exact-version playback and broader playback/performance/platform checks; finish remaining desktop/half-screen UI views and cache-only versus intentional-save checks; qualify reviewed cross-provider matching/writeback and complete performance/asset/write-contention gates. Mobile UI remains deferred by the user. Bounded artist/album/owner-navigation and earlier UI successes remain evidence, not a blanket pass. The detailed open-item table and original requirements below retain the full scope.

Earlier checkpoint evidence below is historical unless explicitly updated by the authoritative checkpoint above.

Historical a217 checkpoint (2026-10-16): the sole verified GUI is build `a217`, SHA-256 `a21733938f82a05c183a355e62e89a9c080ba64321c67993db891e44f67efa8f`, PID 2147434 (launcher PID 2147325, session 6975). Fresh runtime snapshots preserve the paused one-item queue, position 699.309783 seconds and volume 0.8 exactly, without playback or queue writes. Restart remains authorized at any time. That checkpoint compositor is 1914×515: a requested 954px resize was ignored, so no current half-width visual acceptance is claimed. Saved-library counts after the bounded reads are 4,949 tracks, 36 playlists and 15 assets; no import was performed. Earlier counts and queue transitions remain historical. General YouTube library listing returns 61 playlists, including nonmusic collections, without fresh login; the earlier bounded listing had no cursor, `truncated=false` and `complete=null`. An earlier ordinary playlist read returned 116 tracks then 113 through continuation: the first page has a cursor and the second does not, but both report `complete=false` and `total=null`. These 229 returned tracks do not establish complete playlist content. That earlier read-only YouTube checkpoint reported schema9,4,948saved listings and15assets; current counts are stated above. Bounded YouTube pages up to 1,000, artwork startup scoping, search-kind disabling and Spotify diagnostics are compiled in that GUI; individual live acceptance remains limited to the observations recorded here.

Historical 9482 owned footer capture at 954×1036 reports `findings=[]` and displays complete 500×500 artwork from the bounded data-image fallback. The asset-protocol probe still fails: this is fallback success, not a protocol repair. The actual capture is private at `.cache/private-ui-checks/9482-current-footer.png`. At that 9482 checkpoint, one MCP radio identification succeeded with artist/title present; the SongRec/catalog match remains unverified. Playback afterwards remains playing without errors, with no queue or tag edits. Ordinary-track, DJ-set and concert recognition remain unqualified. [Historical 9482 GUI manifest](docs/performance-results/2026-10-16-youtube-channel-identity/user-facing-gui-manifest.json), [artwork and recognition evidence](docs/performance-results/2026-10-16-youtube-channel-identity/user-facing-artwork-recognition-live.json).

Historical 4251 replacement observation: playback changed from a one-item Ena Proino queue at 181.858671875 seconds to a 200-item queue, index 5, Nine at 15.242102 seconds, paused with volume 0.8. User activity versus stale persisted-state restoration remains unproven; no queue overwrite was performed. That unexplained transition does not invalidate the separately observed exact preservation across that a8cc restart. [4251 live evidence](docs/performance-results/2026-10-16-youtube-channel-identity/final-current-live-redacted.json).

The retained artwork-fallback native checkpoint passed **791 tests, zero failures, 16 ignored in 13.12 seconds**, test SHA-256 `eb3612c098ed40a8604095716f1e73c0230b5053a88a498b8d501432fa77592d`; targeted display and managed-diagnostics checks pass three cases each. Mounted/helper frontend checks pass 12/12, Node checks pass 42/42, Svelte retry reports zero errors/warnings and the frontend production build passes. [Native manifest](docs/performance-results/2026-10-16-youtube-channel-identity/artwork-fallback-native-manifest.json). The historical replacement candidate was frozen at SHA-256 `4c482f6328e8b3eee19dc897ffb0977ef31c2b4f2ee893f4ca9c5d6be86d24e4` and was **not launched** (`runtimeStatus=frozen_not_launched`): at that historical checkpoint the fresh probe found user-active playback. The user subsequently authorized restart at any time, so that observation no longer blocks replacement. That retained 791-test candidate was not launched; 9482 was launched later as recorded in the historical evidence. The complete serial App retry passes 208 tests in 43.21 seconds after the multi-route History functional timeout was raised from five to ten seconds, retaining all assertions and separate performance checks; the initial 207-pass/one-timeout log remains. Those source-only checks did not establish live acceptance; the later 9482 fallback success and remaining artist failure are recorded above. Prior failures remain logged. The complete frontend suite passes **564 tests across 60 files in 86.45 seconds** on the serial rerun. The initial concurrent run recorded 563 passes and one History timeout; its log is retained rather than treated as a clean pass. After correcting declaration order for the temporal-dead-zone error, Svelte reports zero errors/warnings. The final production frontend build passes in 5.18 seconds with a chunk-size warning. YouTube worker fixtures pass 48/48, Spotify worker fixtures pass 25 actual cases, and all seven contracts pass. These source checks do not extend the bounded live GUI observations recorded above. Andrew Kelsh UI search returned three results including exact profile `dkkelsh1`; direct profile-URL browsing returned 30 playlists with a continuation cursor. The dd729 build produced the actual app-owned 1920×988 `capture_main_ui` screenshot and owned-WebView geometry `findings=[]`; broader view/responsive qualification remains open.

The compact Library toolbar was implemented and live-verified through Vite in the historical 4251 GUI at 954×1036, with a 707px content pane. Library/Favorites/Collections share one compact row; duration controls sit behind a Filters disclosure with an active badge and Clear; Library options moved into the header ellipsis menu, removing the empty strip; Load more sits at the list end; selection controls wrap and popup bounds are constrained. In the actual owned Library screenshot, the table header moved from y=344 to y=125 (about 219px less toolbar height), showing roughly 13 rows instead of nine. The full playback snapshot remained unchanged through HMR: 200 queue entries, index 5, paused at 15.242102 seconds, volume 0.8. The [private local screenshot](.cache/private-ui-checks/compact-toolbar-verified.png) is retained under `.cache`, not published evidence. Before the bbe restart, both Filters and Library options were opened and closed using PID- and executable-path-guarded app-owned AT-SPI actions, without focus changes or global input. Their actual private captures (`.cache/private-ui-checks/compact-toolbar-filters-open.png` and `.cache/private-ui-checks/compact-toolbar-options-open.png`) show both popups bounded within the 954×1036 window and half-width Library pane. Mobile UI checks are deferred at the user’s request and are not a current completion requirement. Desktop and half-screen checks across the remaining views stay in scope. Mounted filter-badge/Clear coverage passes, the complete App suite passes 207/207, artwork diagnostics pass five cases, Svelte reports zero errors/warnings and the compact-toolbar production build passes. These checks do not complete the broad UI pass.

Historical0214 album failure (HTTP200/PROVIDER_CHANGED, no pages) remains in [unqualified album evidence](docs/performance-results/2026-10-16-youtube-channel-identity/large-album-live-0214-unqualified.json); the later success below supersedes that current-status limitation.

Historical a217 bounded live acceptance: `69 Love Songs` album paging succeeds with 30+30+9 tracks, 69 unique track and occurrence IDs, positions 1–69, positive fractional durations and a terminal cursor. Radiohead search→exact guarded app-owned click now loads artist artwork; the earlier 28b7 missing-artwork proof is retained, and the row-data artwork wire fix has two focused App tests. Compact release controls are live with 20 releases loaded of 45 reported. [Current GUI manifest](docs/performance-results/2026-10-16-youtube-channel-identity/album-youtube-combined-gui-manifest.json), [album proof](docs/performance-results/2026-10-16-youtube-channel-identity/large-album-live-a217.json), [artist artwork proof](docs/performance-results/2026-10-16-youtube-channel-identity/search-artist-artwork-live-a217.json).

Actual profile-owner UI acceptance covers `dkkelsh1` search (one result)→native profile→public playlist (16 tracks)→Browse Andrew Kelsh playlists→internal profile→Back restoring detail. The 110 reported playlists remain bounded/incomplete, not fully qualified. Root viewed the private captures. At actual 1914×515, compact browser/detail controls show three full track rows and footer with `findings=[]`; the previously oversized duration inputs/clipped selects are corrected in this bounded view. [Owner-navigation proof](docs/performance-results/2026-10-16-youtube-channel-identity/profile-owner-ui-live-a217.json). This does not complete broader desktop UI qualification. Fresh live identification cancellation remains untested while paused.

Latest source checkpoint: the complete frontend suite passes **590 tests across 62 files in 96.61 seconds**; the finite-inspection native suite passes **796 tests in 11.86 seconds**, with four focused tool checks. Append-metadata UI coverage passes 16 cases, Svelte reports zero errors/warnings and production build passes in 4.89 seconds. [Full frontend evidence](docs/performance-results/2026-10-16-youtube-channel-identity/provider-ui-full-frontend-manifest.json), [check](docs/performance-results/2026-10-16-youtube-channel-identity/continuation-metadata-check.log), [build](docs/performance-results/2026-10-16-youtube-channel-identity/continuation-metadata-build.log). At that historical checkpoint, GUI `6035cc3ffe635043ca7f07baf7905295060902e5769ac55e1a90f9c692771375` was frozen after a 2m08s build and its replacement verification and full Node run were pending; these are not current running-job claims. Live YouTube zero-page behavior and profile investigation remain unqualified.

Historical a217 YouTube failure: library listing returns 61 items with all track counts unknown; the first ordinary playlist page returns 116 tracks, reported total 240 and a cursor, but continuation returns zero tracks, reported total 5, no cursor and `complete=false`. This inconsistency is a live failure, not a validated parser fix. [Unqualified live evidence](docs/performance-results/2026-10-16-youtube-channel-identity/youtube-playlist-live-a217-unqualified.json). The finite read-only `inspect_youtube_playlist_page` tool is now implemented and native-tested, returning structural counts without provider IDs, tokens or raw responses; live inspection acceptance awaits the replacement GUI. Default-off response-shape diagnostics, continuation-total-null handling and UI preservation of initial-page append metadata are compiled in the new candidate and source-tested; they are not yet claimed live-qualified. The running a217 still contains the earlier 59-test parser behavior. [Source shared checks](docs/performance-results/2026-10-16-youtube-channel-identity/youtube-response-shape-shared-tests.log). Current source validation: Worker/adapter checks pass 42/42; RemotePlaylists passes 27 tests including scroll restoration, and focused owner/art App cases pass two tests. The full 792-test native checkpoint is historical; the current diagnostic build has four focused native checks and two Node checks, not a new full native-suite claim. The prior artwork-wire Svelte check passed zero errors/warnings and production build passed in 4.79 seconds; latest Remote UI Svelte check passes zero errors/warnings and production build passes in 4.64 seconds with a chunk warning. [Check](docs/performance-results/2026-10-16-youtube-channel-identity/remote-ui-check.log), [build](docs/performance-results/2026-10-16-youtube-channel-identity/remote-ui-build.log).

Profile-owner navigation now has source-only validation: the shared Spotify profile URL normalizer accepts bounded legacy/Unicode IDs, decodes once and canonicalizes safely, rejecting path separators, whitespace/control characters and URL authority/query/fragment extras. Two focused App tests and 40 child tests pass, including creator→native profile→Back restoration of filter, sort and two rows without refetch. Those source checks preceded the bounded a217 live owner-navigation acceptance recorded above; no new full native-suite count is claimed. [Source check](docs/performance-results/2026-10-16-youtube-channel-identity/owner-navigation-check.log).

On historical 9482, exact `poso s'agapo` searches returned Spotify 30 and YouTube 20 without errors; each source reported complete request status while `hasMore`/incomplete remained true, so catalog exhaustion is not qualified. [Bounded search evidence](docs/performance-results/2026-10-16-youtube-channel-identity/user-facing-poso-search-live.json). Two further frontend fixes now have focused coverage: partial-playlist retry retains existing rows/incomplete warning on failure and replaces only successful first-page results (12/12); GUI recognition cancellation completes MCP while the recognizer is unresolved, with late-result/next-request guards (five targeted passes, 204 skipped). Svelte passes zero errors/warnings and production build passes in 6.06 seconds with a chunk warning. These are source checks, not live acceptance of those paths.

Historical a8cc recognition checkpoint: one MCP attempt on WebView radio failed after about 4.6 seconds with `OPERATION_FAILED: Selected stream stopped producing audio`. Immediate playback status still reported playing (`paused=false`, volume 0.8), with no interruption or queue change and no retry. A playing clock is not acoustic-output proof, and the failure cause remains unproven. A Linux first-PCM timing fix is now applied in source: parec 100ms latency/50ms process time, strict two-second initial grace and ownership checks, retaining the 750ms post-data deadline and 12-second total limit, plus a delayed-packet fixture. The latest native checkpoint now passes 792 tests, with eight Linux capture checks passing. The capture/artwork fixes were verified in 9482; the current artist fix is in 0214; bounded live outcomes are recorded above. Functional live QA remains the priority. Frozen 4c remains an unlaunched historical candidate; broader live acceptance beyond the historical9482 observations remains pending. [Recognition checkpoint](docs/performance-results/2026-10-16-youtube-channel-identity/radio-identification-live-a8cc.json).

A sustained native playback gate requiring five seconds of progression is implemented, covered by four test-file entries and wired into CI. It has not yet passed live qualification. The new isolated seed profile `f7dx` has completed preflight only and has not been launched. Playback performance, audible output and platform qualification remain open; no todo is completed from the gate implementation alone.

The user authorized one empty private **Burrata verification** playlist on each provider. Each provider's single creation attempt failed with an unknown outcome and retained an `ATTEMPTED` marker without a playlist ID. Spotify inspection covered 113 recognized playlists across three pages, with zero Burrata verification matches; the subsequent 61-item YouTube read also found no match. These observations do not prove that no playlist was created. No creation retry or track writes occurred, and no verified private playlist links are available. Matching and writeback remain unqualified.

A muted WebView resume-and-seek functional check on dd729 advanced one track to 14.801061 seconds after a 10-second seek, but reported `readyState=2` and `status=buffering`. At that earlier dd729 checkpoint, it was restored paused with a one-item queue, volume 0.8 and position 181.858671875 seconds; the later 4251 queue differed as recorded above. This single run does not qualify latency, audible output, match correctness or uninterrupted playback. Earlier exact 200-item queue preservation remains historical evidence; the current 9482 queue is the one-item playing queue recorded above.

A paired three-sample page exploration completed in 113.78 seconds while the main GUI remained paused, an explicit confounder: source-order median fell 256.233→192.324 ms without the candidate index and 153.919→107.878 ms with it; BPM fell 338.484→278.135 ms without and 151.191→115.068 ms with it. These are exploratory batch-hydration comparisons, not 20-sample qualification, write-contention or asset-backed acceptance; no production index is enabled. [Paired exploration](docs/performance-results/2026-10-16-youtube-channel-identity/paired-page-exploratory-main-idle.json). New benchmark expansion is deferred in favor of functional live QA; the latest staged coverage fix is unapplied. A test-only paired metadata-invalidation/membership-write gate and distinct 1KiB asset-file fixtures for 1k/5k/20k/50k are also applied, without completed benchmark qualification. These use synthetic metadata, not audio, and optional database statistics; no write/asset qualification is claimed. The canonical-metrics partial-index candidate passed the corrected three-sample exploration and paired 20-sample aggregate-SQL run on a disposable 50k Store. Baseline→coverage-index median/p95 (ms): count **90.895/101.775→12.292/13.468**; count plus availability OR **123.830/159.151→41.854/50.544**; with BPM coverage **144.913/160.946→47.758/57.119**. Semantic comparisons pass and foreign-key errors are zero. This remains experimental and unpromoted: forced SQL-path timings exclude page hydration, mutex contention, GUI, CPU and end-to-end work; index-build observations are not qualification. [Exploration](docs/performance-results/2026-10-16-youtube-channel-identity/canonical-metrics-index-exploratory-retry.json), [20-sample results](docs/performance-results/2026-10-16-youtube-channel-identity/canonical-metrics-index-20.json). Batch hydration and observer normalization are applied in source. At that source checkpoint, focused frontend checks passed 5/5, production build passes and Svelte reports zero errors/warnings (`batch-observer*` logs). The invalid fixture CHECK value was corrected from `keep` to `shortlist`; all five hydration and two observer targeted cases now pass, followed by the complete 788-test native suite. The original failure log is retained. The historical a8cc GUI contained the batch-hydration checkpoint; historical9482 includes it. The earlier queue-save-failure-banner serial App checkpoint passed 208/208 in 29.42 seconds; the latest serial retry and its timeout adjustment are recorded above. Build and Svelte checks pass. The earlier concurrent run (204 passes, four failures) and initial focused text mismatch remain in the logs. A storage-default fixture correction is a test fix, not a hidden production change. The post-join-elision Store benchmark completed a three-sample exploration followed by 20 measured samples (three warmups). At 50k metadata-only listings, first-page global-availability median/p95/worst is **155.201/165.595/172.546 ms**; concurrent-reader latency is **161.286/177.107/177.651 ms**, and favorite-writer latency is **11.053/161.903/169.576 ms**. The writer p95 still exceeds the interactive target. These are Store wall-time measurements on the immutable test artifact and storage source SHA-256 `60f2e56ab2d9862acb28c0582b213ac1da16fb7a436b8a1208be73f24f3a7c54`; they do not establish UI/end-to-end, CPU, release-build or asset-backed qualification. The earlier paired SQL-only comparison is retained below. No open todo is completed by this checkpoint. [Historical general-context GUI manifest](docs/performance-results/2026-10-16-youtube-channel-identity/general-context-gui-manifest.json), [general YouTube live read](docs/performance-results/2026-10-16-youtube-channel-identity/general-context-live-redacted.json), [WebView functional check](docs/performance-results/2026-10-16-youtube-channel-identity/current-webview-functional.json), [20-sample Store results](docs/performance-results/2026-10-16-youtube-channel-identity/global-availability-join-20.json).

A paired 20-sample SQL join-elision comparison reduces plain-count median from 152.650 to 80.820 ms and p95 from 169.457 to 88.073 ms; count plus availability median falls from 180.836 to 106.973 ms and p95 from 194.683 to 124.092 ms. These are SQL component measurements, not whole-page, whole-app or release performance qualification. Older benchmark and session records remain historical evidence; no open todo is completed by this checkpoint.

Earlier post-fix Store checkpoint (historical), 20 samples at 50k: global pages median/p95/worst 244.93/270.98/272.29 ms; contention readers 262.13/291.19/292.78 ms; writers 6.03/258.02/281.96 ms. Reader p95 improves from the earlier 1224.61 ms, but writer tail latency still fails the interactive target. A separate exploratory three-sample profile took 85.62 seconds overall: aggregate 175–221 ms, plain count 148–155 ms, hydration 49–52 ms and page SQL about 0.8 ms. At that earlier checkpoint, source investigation continued; the later join-elision results are recorded above. A 9.46-second development idle sample observed three app processes (Burrata, WebKitNetwork, WebKitWeb), no helper/Chromium, median CPU 0.97% of one core and PSS 366.9–367.1 MiB; this is not resource qualification. Exact 200-item queue preservation was established for those earlier restarts; it does not establish preservation across the latest replacement or explain its observed queue change.

- currently playing track should show as playing tracklist (seems like not always does, be highilighted), even when it is in search results. should have option to continue playing search results. and a playbar toggle to autoplay radio after last song in queue (would this be based on last song or could it be algorithmitcally based on queue items? that might be hard) 
- be able to sort results by length or specify length
- should be able to id any currently playing song, not just radio item. when playbar is opened there shoud be button. this is useful for mistagged stuff and especially for dj sets and longer concert videos.
- should be able paginate to find more results, currently we just return to 10 or what? how can we improve that?
- we need to get artist and profile/user browsing working correctly from spotify.
- lets get spotify/youtube import/sync working without browser needing to be open.
- still UI inconsistencies, ex: onboarding strangely big checkbox. think like UI designer.  do a UI passover. we can remove redundant elements. use spotify UI as baseline.
- for writeback to spotify and youtube we will have to make sure we have track matching (ie if they came from different catalog, differnt nameing, etc)


- Complete playback performance qualification and Monochrome/backend investigation from the accepted plan. Keep public releases on hold.
- Support reviewed writeback to both Spotify and YouTube, including recordings originating in a different catalog. Live writes must wait for a concrete reviewed preview.
- when a search is done each result uses up too much space
- trying to play via monochrome get error: No playable audio match was found for Fruit Bats — The Bottom of It. No available source matched the requested recording and version. Open the catalog source or choose another source
- we should grey out sort options that aren't valid for any set 
- another playback error: No playable audio match was found for Ergo — Sorrows Of The Moon (featuring Mary Halvorson). No available source matched the requested recording and version. Open the catalog sourc
- modernize the UI filter/search/sort, currently awkward sizing when half screen. also redundancy and confusion, when there are options that won't work its not obvious to user, like profile and channel search but profile only works for spotify?
- track cover art often doesn't display on low playback bar.
-why were tese tracks added to libary after they were search results? "

↓

⚙
←
→
Library
＋ New playlist
Library options

Availability 

Minimum duration
Maximum duration
Ascending
Load more tracks
#	Title	Time	Actions
1 ▶	
Stuck in My Head Again (Official Audio)
Fruit Bats	2:12	
♡
↓
⋯
2 ▶	
Full Performance (Live on KEXP)
Fruit Bats	23:53	
♡
↓
⋯
3 ▶	
Sick of This Feeling (Official Video)
Fruit Bats	6:03	
♡
↓
⋯
♫ ▶	
▸When U Love Somebody (Live @Pickathon 2011)
Fruit Bats	4:11	
♡
↓
⋯
5 ▶	
Eagles Below Us (Icehouse Sessions)
Fruit Bats	3:35	
♡
↓
⋯
6 ▶	
Humbug Mountain Song (Live on KEXP)
Fruit Bats	3:17	
♡
↓
⋯
7 ▶	
A Bit Of Wind (Live on KEXP)
Fruit Bats	2:55	
♡
↓
⋯
8 ▶	
Cazadera - 11/5/2019 - Paste Studio NYC - New York, NY
Fruit Bats	3:35	
♡
↓
⋯
9 ▶	
That Goddamn Sun (Sonoma Session)
Fruit Bats (solo)	3:11	
♡
↓
⋯
10 ▶	
Fishin' For A Vision (Sonoma Session)
Fruit Bats (solo)	3:36	
♡
↓
⋯
11 ▶	
"A Lingering Love" (live on eTown)
Eric D. Johnson (Fruit Bats)	4:02	
♡
↓
⋯
12 ▶	
Gold Past Life (Official Music Video)
Fruit Bats	5:01	
♡
↓
⋯
13 ▶	
When U Love Somebody
Fruit Bats	4:31	
♡
↓
⋯
14 ▶	
Perhaps We're A Storm (Official Lyric Video)
Fruit Bats	3:07	
♡
↓
⋯
15 ▶	
🦇FULL SET LIVE🦇 - Nelsonville Music Festival - Sat June 20, 2026
Fruit Bats	46:05	
♡
↓
⋯
16 ▶	
You're Too Weird (OFFICIAL VIDEO)
Fruit Bats	3:48	
♡
↓
⋯
17 ▶	
Full Performance (Live on KEXP)
Fruit Bats	22:50	
♡
↓
⋯
18 ▶	
Humbug Mountain Song (Official Music Video)
Fruit Bats	3:41	
♡
↓
⋯
19 ▶	
In-Studio Session: Fruit Bats
	20:50	
♡
↓
⋯
20 ▶	
Fruit Bats
	—	
♡
↓
⋯
21 ▶	
"A Lingering Love"
The Fruit Bats	4:13	
♡
↓
⋯
22 ▶	
Slipping Through The Sensors
Fruit Bats	4:53	
♡
↓
⋯
23 ▶	
The Banishment Song
Fruit Bats	5:58	
♡
↓
⋯
24 ▶	
The Deep Well
Fruit Bats	3:43	
♡
↓
⋯
25 ▶	
The Bottom of It - Pickathon Paddock Stage 2025 S02E04
Fruit Bats	3:17	
♡
↓
⋯
26 ▶	
The Ruminant Band (OFFICIAL VIDEO)
Fruit Bats	3:39	
♡
↓
⋯
27 ▶	
The Balcony (Official Music Video)
Fruit Bats	4:16	
♡
↓
⋯
28 ▶	
The Bottom of It (Live on KEXP)
Fruit Bats	3:10	
♡
↓
⋯
29 ▶	
The Landfill (Official Music Video)
Fruit Bats	5:57	
♡
↓
⋯
30 ▶	
Fruit Bats "The Bottom of It"
	3:01	
♡
↓
⋯
31 ▶	
Tempted
Squeeze	4:00	
♡
↓
⋯
32 ▶	
Wild Boy
mgk, Waka Flocka Flame	3:51	
♡
↓
⋯
33 ▶	
Fruit Bat
Alexandre Alekhine	1:44	
♡
↓
⋯
34 ▶	
The Balcony
Fruit Bats	4:08	
♡
↓
⋯
35 ▶	
When U Love Somebody
Fruit Bats	4:31	
♡
↓
⋯
36 ▶	
A Lingering Love
Fruit Bats	3:36	
♡
↓
⋯
37 ▶	
The Bottom of It
Fruit Bats	3:01	
♡
↓
⋯
38 ▶	
Poso S' Agapo
Aliki Vougiouklaki	3:19	
♡
↓
⋯
39 ▶	
Nine
Patti Smith	—	
♡
↓
⋯
40 ▶	
Poso S' Agapo
	4:04	
♡
↓
⋯
41 ▶	
Poso S' Agapo
	3:32	
♡
↓
⋯
42 ▶	
Den Xero Poso S' Agapo (Live)
	2:49	
♡
↓
⋯
43 ▶	
Poso s agapo
	5:32	
♡
↓
⋯
44 ▶	
Alexiou Glikeria Marinela & CO Den ksero poso s' agapo
	2:36	
♡
↓
⋯
45 ▶	
Poso S' Agapo
Anna Vissi	3:30	
♡
↓
⋯
46 ▶	
Nikos Vertis Poso S' Agapo english lyrics
	4:15	
♡
↓
⋯
47 ▶	
Giannis Kotsiras Poso s' agapo
	2:31	
♡
↓
⋯
48 ▶	
Den Xero Poso S' Agapo
Vicky Mosholiou	3:00	
♡
↓
⋯
49 ▶	
Poso S' Agapo
Manos Loizos, Bouzouki Kings, Lefteris Papadopoulos	3:37	
♡
↓
⋯
50 ▶	
02 Den ksero poso s' agapo Original
	2:51	
♡
↓
⋯
51 ▶	
Πόσο Σ' Αγαπώ | Stamatis Kraounakis - Poso S' Agapo (Official Lyric Video)
Σταμάτης Κραουνάκης	4:10	
♡
↓
⋯
52 ▶	
Dimitris Mitropanos & Peggy Zina ► Den Ksero Poso S' Agapo (Live)
	1:23	
♡
↓
⋯
53 ▶	
Poso S'agapo-nikos vertis live
	2:14	
♡
↓
⋯
54 ▶	
Poso s'agapo (Српски превод)
Nikos Vertis	4:16	
♡
↓
⋯
55 ▶	
Then Ksero Poso S' agapo (Δεν ξέρω πόσο σ' αγαπώ)
Theodoridou	2:49	
♡
↓
⋯
56 ▶	
Den Xero Poso S'Agapo
Natassa Theodoridou	2:47	
♡
↓
⋯
57 ▶	
Poso s' agapo (How much i love you) by Giannis Parios
ANTONIS TZELEPIS	1:37	
♡
↓
⋯
58 ▶	
Giannis Parios, Poso S'agapo ,להנאתכם מחנוך אהרוני
	4:41	
♡
↓
⋯
59 ▶	
Den Xero Poso S´ Agapo
Natasa Theodoridou & Eirini Toumpaki	3:52	
♡
↓
⋯
60 ▶	
Poso s'agapo
Dimitris Giannos	5:04	
♡
↓
⋯
61 ▶	
Πόσο Σ' Αγαπώ (Live) | Stamatis Kraounakis - Poso S' Agapo (Official Video)
Σταμάτης Κραουνάκης	4:06	
♡
↓
⋯
62 ▶	
Parios Poso s' agapo
	3:43	
♡
↓
⋯
63 ▶	
Νίκος Βέρτης | Nikos Vertis - Poso s'agapo
Πόσο σ'αγαπώ	4:14	
♡
↓
⋯
64 ▶	
21. Den xero poso s'agapo
	3:04	
♡
↓
⋯
65 ▶	
20 Den Xero Poso S' Agapo
	2:56	
♡
↓
⋯
66 ▶	
02 Den Xero Poso S' Agapo
	2:48	
♡
↓
⋯
67 ▶	
01-05 - Nikos Vertis - Poso S' Agapo
	4:12	
♡
↓
⋯
68 ▶	
Poso S' Agapo
05	4:12	
♡
↓
⋯
69 ▶	
02 - Anna Vissi - Poso S' Agapo
	3:31	
♡
↓
⋯
70 ▶	
An matheis poso s' agapo
17 Βαλάντης	4:10	
♡
↓
⋯
71 ▶	
Poso S' Agapo
Anna Vissi	0:30	
♡
↓
⋯
72 ▶	
Poso S' Agapo
Zig Zag	0:30	
♡
↓
⋯
73 ▶	
Poso S' Agapo
Anna Vissi	0:30	
♡
↓
⋯
74 ▶	
Poso S' Agapo
Dora Sitzani	0:30	
♡
↓
⋯
75 ▶	
Poso S' Agapo
Dakis	0:30	
♡
↓
⋯
76 ▶	
De sou pa poso s'agapo
Giorgos Giakoumakis	0:30	
♡
↓
⋯
77 ▶	
"Poso s'agapo" by Manos Loizos, played by Elefterios Stampouloglou
Πόσο σ' αγαπώ (Μάνου Λοΐζου)	3:17	
♡
↓
⋯
78 ▶	
Poso Na Sagapo - Official Music Track
Michalis Magkas	4:17	
♡
↓
⋯
79 ▶	
How Much Do I Love You (Poso S'Agapo)
Manos Loizos	0:30	
♡
↓
⋯
80 ▶	
KORG PA600 POSO SAGAPO
	4:38	
♡
↓
⋯
81 ▶	
Poso s'agapo feuturing Theodosia ( demo )
	4:19	
♡
↓
⋯
82 ▶	
Poso S'agapo(How Much I Love You)
Manos Loizos	3:33	
♡
↓
⋯
83 ▶	
Den ksero poso s'agapo (Live)
Giannis Dimitras	4:31	
♡
↓
⋯
84 ▶	
Kriti mou poso s'agapo
Haralampos Garganourakis	0:30	
♡
↓
⋯
85 ▶	
Poso Na Sagapo (instrumental)
Mιχάλης Μάγκας	3:49	
♡
↓
⋯
86 ▶	
Poso S' Agapo
Χοροσταλίτες, Vasilis Messaritakis, Evdokia Moraiti	2:59	
♡
↓
⋯
87 ▶	
Poso na sagapo
Mιχάλης Μάγκας	3:56	
♡
↓
⋯
88 ▶	
Poso s'agapo tis nyhtes
Takis Biniaris	3:47	
♡
↓
⋯
89 ▶	
Poso S' Agapo
Paris	4:19	
♡
↓
⋯
90 ▶	
Poso S Agapo
Giorgos Velissaris	3:33	
♡
↓
⋯
91 ▶	
Poso S'agapo - Master
Poso S'agapo	3:29	
♡
↓
⋯
92 ▶	
Poso S'Agapo
Stathis Aggelopoulos	4:58	
♡
↓
⋯
93 ▶	
EffiLove - Poso Sagapo - Nikad Nisam Prestao
EffiLove	3:24	
♡
↓
⋯
94 ▶	
Den xero poso sagapo - Konta sta ximeromata (feat. Pavlos Papantoniou)
Thanasis Polykandriotis	6:15	
♡
↓
⋯
95 ▶	
Dimitris Giotis - Poso s´agapo Dj Tassos RMX
Dimitris Giotis	3:38	
♡
↓
⋯
96 ▶	
Den ksero poso s'agapo
Viky Moscholiou	3:04	
♡
↓
⋯
97 ▶	
Den Xero Poso S'Agapo
Natassa Theodoridou	0:30	
♡
↓
⋯
98 ▶	
Poso S' Agapo
Nikos Vertis	0:30	
♡
↓
⋯
99 ▶	
Poso S' Agapo
Aliki Vougiouklaki	3:20	
♡
↓
⋯
100 ▶	
Poso S' Agapo
Manos Loizos, Bouzouki Kings	3:37	
♡
↓
⋯
101 ▶	
Den Xero Poso S' Agapo
Vicky Mosholiou	3:00	
♡
↓
⋯
102 ▶	
Ke s'agapo
Angélique Ionatos	2:44	
♡
↓
⋯
103 ▶	
Poso Akoma
Tales of June	3:37	
♡
↓
⋯
104 ▶	
Poso Lipamai
Selofan	—	
♡
↓
⋯
105 ▶	
Poso Lipamai
Selofan	—	
♡
↓
⋯
106 ▶	
El poso
Los Chenchos	3:00	
♡
↓
⋯
107 ▶	
Poso Lypamai
Imam Baildi	4:16	
♡
↓
⋯
108 ▶	
Poso Metaniosa
Νίκος Βέρτης	3:39	
♡
↓
⋯
109 ▶	
Poso Lipame
Imam Baildi	4:16	
♡
↓
⋯
110 ▶	
Poso Kanei
Skam	—	
♡
↓
⋯
111 ▶	
S'agapò
Vinicio Capossela	0:26	
♡
↓
⋯
112 ▶	
S'Agapó
Nicu Alifantis	—	
♡
↓
⋯
113 ▶	
S'agapò
Da Silva	3:35	
♡
↓
⋯
114 ▶	
S'agapò
Paolo Simoni	3:45	
♡
↓
⋯
115 ▶	
S'agapò
Folkabbestia	3:50	
♡
↓
⋯
116 ▶	
S’agapo
Vicky Leandros	3:32	
♡
↓
⋯
117 ▶	
S'agapo
Sophia Loren	3:13	
♡
↓
⋯
118 ▶	
S'agapo
Çiğdem Aslan	5:22	
♡
↓
⋯
119 ▶	
S'agapo
Елена Фролова	4:09	
♡
↓
⋯
120 ▶	
S'agapo
FarragoL	3:28	
♡
↓
⋯
121 ▶	
S'agapo
Vicky Leandros	3:32	
♡
↓
⋯
122 ▶	
S'agapo
Sedrenn	4:33	
♡
↓
⋯
123 ▶	
S'Agapo
Mandragore	4:09	
♡
↓
⋯
124 ▶	
S'agapo
Mireille Mathieu	2:57	
♡
↓
⋯
125 ▶	
S'agapo
Laila Kinnunen	2:36	
♡
↓
⋯
126 ▶	
S'Agapo
Ансамбль «Гренада»	4:54	
♡
↓
⋯
127 ▶	
S'Agapo
Elpida	2:39	
♡
↓
⋯
128 ▶	
S'agapo
Μαριάντα Πιερίδη	4:26	
♡
↓
⋯
129 ▶	
S’agapo
Sophia Loren	2:29	
♡
↓
⋯
130 ▶	
Poso
Sissi Rada	2:48	
♡
↓
⋯
131 ▶	
Den Xero Poso S'Agapo
Νατάσα Θεοδωρίδου	2:55	
♡
↓
⋯
132 ▶	
Poso mou lipi
Sotis Volanis	4:13	
♡
↓
⋯
133 ▶	
M'Agapas S'Agapo
Lefteris Pantazis	3:41	
♡
↓
⋯
134 ▶	
Kratises Misa Ta "S' Agapo"
Nikos Vertis	4:05	
♡
↓
⋯
135 ▶	
Poso S' Agapo
Aliki Vougiouklaki	3:20	
♡
↓
⋯
136 ▶	
Poso S Agapo
Vaggelis Pytharoulis	1:23	
♡
↓
⋯
137 ▶	
Agapi Pou 'gines Dikopo Maheri / Den Xero Poso S' Agapo (Live)
Stella Theofilou	2:44	
♡
↓
⋯
138 ▶	
To Xereis Poso S' Agapo
Alekos Kostas	3:05	
♡
↓
⋯
139 ▶	
Poso S' Agapo
Giorgos Polychroniadis	3:30	
♡
↓
⋯
140 ▶	
Kriti mou poso s'agapo
Haralampos Garganourakis	2:18	
♡
↓
⋯
141 ▶	
Poso S' Agapo
Lefteris Samson	2:43	
♡
↓
⋯
142 ▶	
Poso S' Agapo
Giorgos Hatzinasios	1:18	
♡
↓
⋯
143 ▶	
Poso S' Agapo
Spiros Lambrou， Pediki Horodia Spirou Lambrou， Giorgos Hatzinasios	2:27	
♡
↓
⋯
144 ▶	
How Much Do I Love You (Poso S'Agapo)
Manos Loizos	3:37	
♡
↓
⋯
145 ▶	
Poso S' Agapo
Χοροσταλίτες	2:59	
♡
↓
⋯
146 ▶	
Poso S' Agapo
Giannis Koutras	3:08	
♡
↓
⋯
147 ▶	
Den Xero Poso S' Agapo
Vicky Mosholiou	2:56	
♡
↓
⋯
148 ▶	
Poso S Agapo
Dimitris Giotis	3:15	
♡
↓
⋯
149 ▶	
Den Xero Poso S' Agapo (Live)
Dimitra Galani	2:49	
♡
↓
⋯
150 ▶	
Poso S' Agapo
Nikos Vertis	4:12	
♡
↓
⋯
151 ▶	
One More Time (12 Mix)
Daft Punk	8:01	
♡
↓
⋯
152 ▶	
End of Line (From "TRON: Legacy"/Score)
Daft Punk	2:37	
♡
↓
⋯
153 ▶	
Get Lucky
Baby Rockstar	4:56	
♡
↓
⋯
154 ▶	
get lucky
Takane Lui	3:03	
♡
↓
⋯
155 ▶	
Get Lucky
Vitamin String Quartet	4:54	
♡
↓
⋯
156 ▶	
Get Lucky (Computer Game Version)
8-Bit Arcade	4:01	
♡
↓
⋯
157 ▶	
Get Lucky
Phoenix Moon	4:19	
♡
↓
⋯
158 ▶	
Get Lucky
8-Bit Misfits	4:11	
♡
↓
⋯
159 ▶	
Get Lucky (Radio Edit)
I'm Jack	4:37	
♡
↓
⋯
160 ▶	
Daft Punk
Pentatonix	4:09	
♡
↓
⋯
161 ▶	
Get Lucky (8 Bit Version)
8 Bit Universe	4:18	
♡
↓
⋯
162 ▶	
Get Lucky
The Cairn String Quartet	3:08	
♡
↓
⋯
163 ▶	
Get Lucky (Metal Cover)
Leo	3:29	
♡
↓
⋯
164 ▶	
Get Lucky
The Countdown Kids	4:07	
♡
↓
⋯
165 ▶	
Get Lucky
Funk Punk	4:10	
♡
↓
⋯
166 ▶	
Get Lucky (feat. Pharrell Williams and Nile Rodgers)
Daft Punk	6:10	
♡
↓
⋯
167 ▶	
Get Lucky (Originally Performed By Daft Punk) [Full Vocal Version]
Paris Music	4:10	
♡
↓
⋯
168 ▶	
Get Lucky (In the Style of Daft Punk & Pharell Williams) [Karaoke Instrumental Version]
Karaoke All Hits	4:21	
♡
↓
⋯
169 ▶	
Get Lucky (Made Popular By Daft Punk ft. Pharrell Williams) [Vocal Version] (feat. Pharrell Williams)
Party Tyme Karaoke	4:05	
♡
↓
⋯
170 ▶	
Get Lucky (Originally Performed by Daft Punk & Pharrell Williams) [Instrumental]
DJ Cocktail	6:02	
♡
↓
⋯
171 ▶	
Champagne Humor
BLP KOSHER	2:55	
♡
↓
⋯
172 ▶	
Status Symbol 2
Nipsey Hussle, Buddy	2:00	
♡
↓
⋯
173 ▶	
Super Real
Nipsey Hussle, Bino Rideaux	4:40	
♡
↓
⋯
174 ▶	
All Summer (feat. BH)
Nipsey Hussle, Bino Rideaux, BH	4:00	
♡
↓
⋯
175 ▶	
I Don't Give a Fucc
Nipsey Hussle	3:10	
♡
↓
⋯
176 ▶	
Mac 11 On the Dresser
Nipsey Hussle	2:41	
♡
↓
⋯
177 ▶	
Nipsey Hussle
C5, Penny Lane	2:44	
♡
↓
⋯
178 ▶	
Juice Lee & Nipsey Hussle - Blue Dreams
Nipsey Hussle	4:55	
♡
↓
⋯
179 ▶	
Make Believe
Nipsey Hussle, Bino Rideaux	2:56	
♡
↓
⋯
180 ▶	
The Hussle Way
Nipsey Hussle	3:40	
♡
↓
⋯
181 ▶	
Reckless (feat. Static Major)
Nipsey Hussle, Bino Rideaux, Static Major	2:19	
♡
↓
⋯
182 ▶	
Killa
Nipsey Hussle	4:41	
♡
↓
⋯
183 ▶	
500 Horses
Nipsey Hussle, Bino Rideaux	3:49	
♡
↓
⋯
184 ▶	
That's How I Knew
Nipsey Hussle	4:58	
♡
↓
⋯
185 ▶	
Hussle & Motivate
Nipsey Hussle	4:19	
♡
↓
⋯
186 ▶	
I Do This
Nipsey Hussle, Young Thug, Mozzy	4:10	
♡
↓
⋯
187 ▶	
Perfect Ten (feat. Nipsey Hussle)
Mustard, Nipsey Hussle	4:12	
♡
↓
⋯
188 ▶	
Hussle in the House
Nipsey Hussle	3:20	
♡
↓
⋯
189 ▶	
Picture Me Rollin'
Nipsey Hussle	2:13	
♡
↓
⋯
190 ▶	
Late Nights and Early Mornings
Nipsey Hussle	2:19	
♡
↓
⋯
191 ▶	
Last Time That I Checc'd (feat. YG)
Nipsey Hussle, YG	3:46	
♡
↓
⋯
192 ▶	
Racks in the Middle (feat. Roddy Ricch and Hit-Boy)
Nipsey Hussle, Roddy Ricch, Hit-Boy	3:53	
♡
↓
⋯
193 ▶	
Victory Lap (feat. Stacy Barthe)
Nipsey Hussle, Stacy Barthe	3:58	
♡
↓
⋯
194 ▶	
Hussle & Motivate
Nipsey Hussle	4:19	
♡
↓
⋯
195 ▶	
Double Up (feat. Belly and Dom Kennedy)
Nipsey Hussle, Belly, Dom Kennedy	6:01	
♡
↓
⋯
196 ▶	
Grinding All My Life
Nipsey Hussle	2:55	
♡
↓
⋯
197 ▶	
Higher (feat. Nipsey Hussle & John Legend)
DJ Khaled	2:57	
♡
↓
⋯
198 ▶	
Nipsey
LandLord Lo	3:48	
♡
↓
⋯
199 ▶	
Nipsey
Trae tha Truth	4:50	
♡
↓
⋯
200 ▶	
The Mack
Nipsey Hustle · live, 2010: Japan	—	
♡
↓
⋯

♫
When U Love Somebody (Live @Pickathon 2011)"
## Current status — 2026-10-16

Items remain above until implementation **and the required validation** are complete. See [implementation evidence](docs/implementation-status.md) for checkpoint details.

Latest combined source and GUI evidence is recorded in the current live checkpoint above. [current GUI manifest](docs/performance-results/2026-10-16-youtube-channel-identity/artist-identity-gui-manifest.json), [earlier 776-test manifest](docs/performance-results/2026-10-16-youtube-channel-identity/combined-native-manifest.json), and [paired SQL measurements](docs/performance-results/2026-10-16-youtube-channel-identity/metrics-join-20.json) identify the exact artifacts and measurement scope. Earlier navigation and queue observations remain in [navigation evidence](docs/performance-results/2026-10-16-ui-navigation-control/README.md); they do not establish the current queue state.

The v9 sorting optimization is integrated in source but remains unqualified in the running app: an isolated stale-projection partial-index run reduces ready asset-backed 50k page medians to 319–378 ms (worst observed 381 ms), from the prior 509–555 ms medians. These three-sample Store measurements are exploratory; the full backfill takes 87.89 seconds in 500-row benchmark batches, versus 250-row runtime batches. The current-main rebase and shared-sort preflight now pass. Twenty-sample known-metadata first-page p95 at 50k is 346 ms, while concurrent favorite writes expose 1225 ms reader p95; the guarded-invalidation fix has post-fix timing recorded above. Full runtime qualification remains required. [Latest exploratory evidence](docs/performance-results/2026-10-16-v9-stale-index-assets-50k-exploratory/README.md). The separately pinned Meld Qobuz investigation tried all four client-configured catalog hosts without receiving usable catalog JSON; no new backend was promoted. [Sorting evidence](docs/performance-results/2026-10-16-v9-bpm-index-assets-varied-50k/README.md), [Meld evidence](docs/performance-results/2026-10-16-meld-qobuz-source/README.md).

| Open item | Implemented / verified | Remaining |
| --- | --- | --- |
| Sasa | Both metadata shapes return a meaningful recording/version-unavailable reason within 20 seconds; saved tags remain unchanged. | Correct-version playback or permitted verified fallback coverage. |
| Indicators, continue search, queue-end radio | Implemented; live search current/paused indicators verified; queue continuation and radio have deterministic coverage. | Queue/chapter occurrence, paused and buffering states now have frontend coverage, including native chapter coordinates. Remaining visual view checks and live radio/advancement qualification. |
| Sorting and duration filters | Shared sorting/filtering in the main library/search/provider track views; global local database ordering; album-query consistency fixed. History sort choices stay enabled while later pages remain (201-entry App regression passes). Explicit pending global availability flow and stale-query handling have mounted-App coverage. | Artist Top/Local/expanded-release controls and Local database paging are now implemented and covered by mounted tests; History route controls/pages/scroll and gapped album occurrence positions now have integration coverage; finish provider-native navigation restoration and qualify the native availability candidate/runtime. |
| Identify playing audio | Native PCM path and expanded-player action implemented. One historical9482 MCP radio identification succeeds with artist/title, without queue/tag edits or playback errors; SongRec/catalog correctness remains unverified. The earlier a8cc failed attempt is retained historically. | Live recognition across ordinary tracks, DJ sets and concerts. The actual-PCM frontend and native payload path are implemented, but Linux WebKitGTK lacks captureStream; the restricted app-output fallback is now implemented and compiles, with runtime recognition and cleanup qualification remaining. Same-station stale-result rejection has a passing App regression. |
| More results | Per-source loading, bounded collection and cancellation implemented. SoundCloud live continuation returns 30 + 18 distinct results, then exhausts the cursor. | YouTube Music continuation and query-bound cursor handling are implemented with native fixtures and are compiled into the current GUI; live continuation checks remain. Remaining providers and collection/UI qualification remain; incomplete batches must be labeled honestly. |
| Spotify browsing | Profile ID/URL reads reached 77 distinct playlists, honestly marked partial. Artist/album MCP controls and fixtures exist. | Source-backed session-native artist/release/album GraphQL paging, cursor-cycle guards and full App navigation are implemented with passing fixtures; live acceptance, album detail pages beyond 50, profile completeness and actual UI verification remain. Currenta217 Radiohead artwork/releasecontrols and69LoveSongs album69tracks30+30+9 are liveverified. Ownerprofile→playlist16tracks→internalowner→Backdetail accepted; reported110profileplaylists remainbounded/incomplete. Broader provider/UI qualification remainsopen. |
| Browser-independent provider work | Spotify native-session reads and restart recovery verified with one Linux account; successful import reported by user. YouTube login/handoff and worker paths implemented. | Both providers recover confirmed OS-keyring sessions after a full process restart. Earlier Spotify listing returned 48 items and YouTube Music returned system playlists `LM`/`SE`; special-ID reads are accepted but their content remains incomplete. Verified-channel YouTube listing returns61/unknowntrackcounts; current61b5 selectedYTplaylist240unique/116+113+11/completeTRUE,221available+19unavailableflags verified. Spotifyboundedoffset/limit totalsvary100vs110; protected4335actualprofileUI reaches99rows after4Loadmoreactions,0remainingbuttons/110reportedbounded-incomplete; partialUIaccepted/notall110. Earlier8152API99retentionterminalproofhistorical. Complete live reads and restart/expired-session/account-switch/job verification across supported platforms. |
| UI consistency | Compact Library toolbar implemented and actual closed-state Library capture verified on live 4251 at 954×1036 (707px pane): header y=125 versus 344, about 13 visible rows versus nine; full queue snapshot unchanged through HMR. Filters badge/Clear has mounted coverage; App 207/207 and production build pass. Filters and Library options popup bounds are visually verified in the actual 954×1036 half-width pane; mobile visuals are deferred at the user’s request; broader desktop UI qualification remains pending. Onboarding checkbox and compact search rows implemented; 546 frontend tests pass across 59 files. E5 fixes the native minimum height. Scoped live screenshots at 1914×343 verify the complete player, queue and Details; compact-height controls restore a visible library row. Current Svelte checks have zero errors/warnings and the frontend build passes. A disconnected visual matrix now covers 360/500/1000/1440 px with long titles, unavailable playback, queue/Details and Identify/Cancel controls. | Actual0214 HMR desktop checks now verify Library queue-open/closed toolbar labels/layout and compact artist title/date presentation; search-artwork preservation, release controls and remaining views stay open. These bounded checks do not complete broad UI qualification. |
| Cross-catalog matching and writeback | Candidate review, scoped expiring tokens, complete previews, occurrence preservation and guarded apply implemented with fixtures. | Live matching and reviewed test-playlist writes on **both** providers. Empty private playlist creation is authorized; both single attempts have unknown outcomes and must not be retried. No track writes occurred. Neither provider's writeback is live-qualified. |
| Search results appearing in Library | V8 membership/cache separation is implemented with native regressions. Live migration retains all 4,948 saved listings and 15 assets. Public three-provider search leaves the saved-library count unchanged. | Finish live browse/playback cache-only and intentional-save checks in the rebuilt application. |
| Playback performance / backends | Protected4335 integratedfailurehandling restoresidle/pausedtrue withoutresume; freshfull200before/afterqueueproof passes. Earlierresume timeout/50prefixlimitation remainhistorical. RestoredUIshowsduration0:00newopenbug; playbacknotqualified.  Cancellation/deadline, cache/mapping, native-seek and tracing changes implemented; the full native checkpoint passes796 tests(16ignored), frontend590/62passes; latest966e pre-replacement evidence and bounded historical artwork/recognition success do not complete broader functional qualification, including experimental Bandcamp and availability regressions; the retained MP3 passes local duration/95%-seek decoding. The sustained native five-second gate is implemented with CI/test coverage, but live qualification and the unlaunched f7dx seed-profile run remain pending. Monochrome and public backend source investigation recorded. | Full matched-recording/resource/performance matrix, remaining backend prototypes where usable, macOS/Windows verification and codec promotion gates. |

Historical a798 checkpoint: the then-sole GUI was the compiled and launched `.cache/verification-target/debug/verified-main-provider-a798b5f5f460/burrata` (SHA-256 `a798b5f5f46045e2f3be575ae25af063c98a7485802b2e7027a03bf40f0a701b`); its normal development build has 32 warnings in the verification log. The current native checkpoint passes 769 tests, zero failures, 15 ignored in 15.19 seconds; immutable test SHA-256 `c39e1b1aa66062e15658fd273cca40885437e60e0e3008c46dd2f9be151d1ebb`. The latest worker, adapter and contract checks pass 41, 16 and seven cases respectively. The earlier 768-test native baseline predates the latest embedded JavaScript; the current native checkpoint is 769. The last complete frontend run passes 560 tests across 60 files and predates one new test; the complete mounted App suite now passes 206/206, Svelte checks report zero errors/warnings and the production frontend build passes. No full 561-test run is claimed. Spotify accounts redirect, pending retry and strict keyring handling are fixed with five new native tests; the reason the original login was lost remains unproven.

Historical restart/read checkpoint: both Spotify and YouTube recovered OS-keyring sessions with no sign-in window or companion. At that checkpoint, Spotify playlist listing returns 48 items and YouTube Music listing returns two items without a cursor. Those two YouTube IDs are the system playlists `LM` and `SE`; `get_remote_playlist` rejects both as `UNSUPPORTED` because of the minimum-ten-character ID validation. The subsequent live special-ID result is recorded in the current checkpoint above. No custom YouTube playlists were observed. Earlier Spotify library pages returned 48 then 49 items without overlap; profile-owner-ID pages returned 30 then 29 without overlap, and a profile-URL read returned 30 items. Live UI Spotify profile search now returns 30 results with Load more available after the App source-capabilities refresh fix; live UI YouTube `poso` track search returns 20 results with `hasMore=true`. These searches do not establish playback correctness. The upstream Music route remains unpinned; see the limitation in `docs/performance-results/2026-10-16-youtube-channel-identity/music-library-source.md`.

Historical restart playback comparison: foreground WebView snapshots were exactly equal across all fields, preserving the then-current 200-item paused queue, occurrence IDs, current index 0 and position 145.421814 seconds. Live MCP `navigate_ui` to Library completed with `providerDataReady=not_measured`. Queue-panel geometry at 954×1036 reports `findings=[]`. A desktop capture was occluded by another app and deleted; it is not a UI visual pass. Writeback remains unqualified; no live remote writes or public release occurred.

The legacy zero/malformed Spotify duration repair is now landed with passing native regressions. A read-only check finds all 578 current Spotify listings have positive known durations (no zero or null rows); provider import/session qualification remains tracked above.

# done
- Repair legacy zero/malformed Spotify durations through the existing bounded scan, preserving manual values, chapter ranges and all finite positive known durations, including long recordings. Store/import regression fixtures pass in the 698-test native suite; the current live library has 578 positive known Spotify durations and no zero/null Spotify rows.
- Show optional provider Views and Plays in Details, retaining reported zeroes and omitting unknown values. Counts now survive Spotify search, album/playlist browsing and imports without changing playlist sync hashes. Validated by provider/worker fixtures, remote-entry/hash regressions and `PlayingInfo.test.ts`; the full suites pass 661 native and 457 frontend tests. Provider runtime compatibility remains tracked separately above.
- Repair existing missing Spotify durations through an explicit, cancellable public-metadata scan without login. Live verification filled 396 missing values, preserved 182 known durations and all manual metadata/playlist occurrences/asset durations; see `docs/performance-results/2026-10-15-public-duration-live/README.md`.
- Fix the oversized onboarding recognition checkbox. Visually verified at 360×640; see `docs/performance-results/2026-10-15-expanded-player-ui/onboarding-360.png`. The broader UI pass remains above.
- Use a single Playlists heading in the left pane. Covered by `LibrarySidebar.test.ts` (one heading with custom sections and source groups).
- Filter left-pane playlists by name (and account playlist rows by name or owner). Covered by `LibrarySidebar.test.ts`.
- Open Spotify artist search results at the Spotify artist profile instead of rerouting to MusicBrainz. Covered by `SearchPanel.test.ts`.
- Make Add to playlist a primary context-menu action. Audited `TrackMenu.svelte` and `TrackList.svelte`; `TrackMenu.test.ts` verifies the primary action, and App integration covers opening the picker directly.
- Search playlist destinations with autofocus, keyboard selection and useful empty states while preserving creation. Validated by App integration tests for filtering, focus, ArrowDown/Enter selection and Create & add.
- Add independent, default-off pause and current-position settings for opening the active source, including chapter offsets. Source links, Details, footer and radio links share the opener; App integration validates defaults, unrelated tracks, chapter offsets and a delayed-pause race.
