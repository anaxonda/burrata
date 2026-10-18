# v0.1.3 verification checkpoints

Current status: all six WebView source/startup/paused-seek/resume cases pass on frozen binary `6fdffa3158fd`, including both original Spotify Creep editions. All six native-engine cases also pass; normal-profile preservation and desktop UI checks pass. Release delivery remains pending. The chronological checkpoints below retain earlier failures and their scope.

## Anonymous YouTube import check

A fresh headless Chromium profile loaded one public video and one public playlist without using the user browser profile or signing in. The video parsed as BAD TIMES by Imael Angel (168 seconds). The playlist rendered 182 ordered cards after document scrolling; the current DOM uses `yt-lockup-view-model` cards with explicit playlist `index` values, title links, channel links, and visible duration badges. The extension parser returned 182 entries, 182 distinct IDs, no duplicate occurrence keys, and complete status with no warnings.

This was initially a parser-only check; the later isolated extension run below supersedes that scope. The captured playlist had no unavailable/private-deleted placeholder row, so that row shape remains untested. Full DOM, account data, and cookies were not retained.

## Isolated extension and native-host import

On 2026-10-18, Chromium 154 on Arch Linux loaded the extension in a fresh isolated profile. A bounded `strace` showed Chromium checking `<user-data-dir>/NativeMessagingHosts/org.burrata.browser.json` (the initially staged XDG locations were not searched); that single profile-local manifest was then used. No system or normal-profile host registration was changed.

The extension popup imported the anonymous public BAD TIMES video as one complete item and the public Popular Music Videos playlist as 182 complete items. Read-only checks of the isolated database receipts and collection memberships confirmed 1/1 and 182/182 imported, complete receipts with zero warnings; playlist membership had 182 distinct recordings. The sole-exact-app guard still passed for the isolated Burrata process. Sanitized details and source hashes are in [youtube-native-import.json](youtube-native-import.json).

The single video is represented by the app as a one-item collection. This playlist had no duplicate IDs or unavailable placeholders, so neither behavior was exercised. No cookies, credentials, account identifiers, or full syscall trace were retained. The headless staged manifest granted YouTube host access in place of a normal toolbar click’s activeTab grant; that permission gesture itself was not exercised.

## Integrated startup checkpoint

[Four exact-source WebView cases](webview-startup-four.json) pass five seconds of stable media progression each on development binary `af9ae0dec4e1`: 500 Miles, BbY WOW, BAD TIMES and the exact 4:43 MusicBrainz Creep recording. This is media-time evidence, not acoustic or statistical qualification. It does not establish that the older Spotify Creep credit resolves automatically. Paused-seek resume returned an actual app cancellation; the fix and final qualification are still pending at this checkpoint.

The native library suite passes 1,109 tests (21 ignored). The integrated frontend suite passes 989 tests in 82 files; the subsequent Search light-theme CSS correction passes the 50-test SearchPanel suite and sixteen headless artwork/component captures across both desktop widths and themes. These counts precede the final Creep and resume follow-ups.

## Native follow-up checkpoint

The exact artist-ID recording-credit crosswalk compiles and its ten focused identity tests pass. The full compiled native suite then passes 1,112 tests with 21 ignored when local loopback sockets are available. The initial restricted-sandbox run had 40 failures from socket permission denials; that run is retained privately and is superseded for test outcome by the loopback-enabled execution. This checkpoint precedes the bounded-alias prioritization follow-up and final live playback verification.

## Deterministic checkpoint before resolver-proof integration

After bounded alias-priority and ISRC-enrichment corrections, the final native suite passes 1,112 tests (21 ignored), including all ten artist-credit identity tests. The final frontend suite passes 993 tests across 82 files, the component verification-action tests pass 15/15, Svelte reports zero errors/warnings, and eight command/permission contract checks pass. Formatting and diff whitespace checks pass. The app binary is being rebuilt for the remaining live checks; these deterministic results alone do not establish live Creep or paused-seek success.

## Live resume verification and remaining crosswalk integration

On development binary `17d9e1ae6b95`, the [retained live attempt](webview-seeks-crosswalk-gate-failure.json) verifies startup, five seconds of media progression, paused seek to ten seconds, successful resume and another five seconds of media progression for 500 Miles, BbY WOW, BAD TIMES and the exact MusicBrainz Creep recording. Thus the WebView pending-play-promise fix is verified for these four sources. This is not acoustic output or statistical performance evidence.

The same attempt then fails on the original Spotify Creep row: explicit recording-credit preparation succeeded, but the resolver's artist-only mutation guard rejects the newly verified ISRC enrichment. The runner reported its observation-count limit while waiting on the unavailable state; readback confirmed the underlying no-playable-match message. That resolver integration remains unfinished at this checkpoint, and the second Spotify edition was not reached. Both failures and successes are retained; no all-six pass is claimed.

## Resolver-proof negative test

The first full suite after proof threading passes 1,112 tests but fails the newly added conflicting-ISRC resolver-gate test (21 ignored). The generic title/artist path can otherwise bypass the catalog ISRC conflict check. A proof-context conflict guard is being added before all acceptance paths. This is an unresolved failure at this checkpoint; earlier green native counts do not claim completion of this integration.

## Actual recording-credit separator

The next retained [Spotify attempt](webview-starring-credit-failure.json) still has no eligible source. Its [categorical trace](creep-starring-gate-trace.json) shows one Monochrome candidate with matching title/duration descriptions but no accepting gate. The cached exact MusicBrainz recording credits use the join phrase `starring`; this is not a supported separator in the existing flat artist-credit matcher. Verified-ID alias generation now also constructs comma-separated complete ordered credits, preserving the original phrase variant and all recording/version/ISRC checks. The realistic resolver fixture and live rerun are pending at this checkpoint. A first fixture attempt accidentally used a candidate title matching the original Spotify credit instead of Monochrome's full credit; that fixture title was corrected to the actual adapter shape before rerunning.

## Original Spotify recordings now play

On frozen development binary `6fdffa3158fd`, [both original Spotify Creep editions and the three earlier regressions](webview-starring-fixed-five-pass-initial-buffer.json) pass exact-source startup, at least five seconds of media progression, paused seek to ten seconds, successful resume and another five seconds of media progression. The Creep rows resolve to `monochrome:160754332869255168` after explicit recording-credit preparation; saved metadata is unchanged. All 1,113 native tests and all 993 frontend tests pass on this source.

The sixth case, the separate MusicBrainz Creep fixture, fails the initial steady-phase sample after a brief playing-to-buffering transition: the owner/source remains correct, media is unpaused, readyState is 2, no media error is reported, and current time is 0.101693 seconds. The runner started the strict steady phase on the UI playing status alone; that startup boundary is being reviewed. The report remains an overall failure, not a clean six-case pass.

## Six-case WebView pass

The [final WebView run](webview-six-pass.json) passes all six exact-source cases on the unchanged `6fdffa3158fd` application binary: both original Spotify Creep editions, 500 Miles, BbY WOW, BAD TIMES and the MusicBrainz Creep fixture. Each case proves owned media startup progress within the shared twenty-second queue-dispatch deadline, then the unchanged strict five-second steady window, a paused seek to ten seconds, resume and another steady window. The isolated profile finishes paused with volume restored. Startup progress was observed in 3.0–8.0 seconds in this one exploratory run; this is not a p95 or acoustic qualification.

The earlier initial-buffer failure remains retained. A read-only harness review found that its UI-only playing edge preceded media readiness; the final runner adds an owner/source-bound >=250 ms media-progress startup phase inside the existing deadline. It does not relax the subsequent strict steady-state checks or change application playback code between those two runs.

## Native six-case pass and normal restoration

The [native run](native-six-pass.json) passes the same six expected sources on `6fdffa3158fd`, including first mixer-consumed audio, strict five-second position progression, paused seek to ten seconds and resumed mixer progression. The isolated profile finishes paused with its original volume restored. This is one exploratory repetition per case, not statistical latency qualification or a physical-audibility test.

The normal window is restored through the Vite/module/MCP-verified launcher. [Nine complete queue/playback checks and confirmed account revisions](normal-preservation.json) are unchanged. The saved Spotify Creep edition receives its verified recording-credit cache proof without editing tags or queue; the other edition is not saved and is correctly rejected without importing anything. The 4,965-track library remains present. Actual 954×1036 desktop capture and geometry inspection show compact controls and no playback-bar overlap.
