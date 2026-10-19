# France macOS v0.1.3 report

The report came from an Apple Silicon MacBook (M1/M2, recent macOS presumed). Radio Nova works in the user's browser. The screenshots show three distinct observations; they do not establish one common failure.

## Radio Nova

Both v0.1.3 and the pre-investigation current code sent the saved HTTP URL directly to WebView audio. The packaged app has no ATS exemption; WKWebView can reject plaintext media even when Safari plays the station. The generic `The operation is not supported` rejection is consistent with this path, but its exact device cause is not proven without Mac diagnostics. [Apple's WebView ATS documentation](https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowsarbitraryloadsinwebcontent).

A live probe of the reported Nova mount over HTTP and HTTPS returned HTTP 200 and audio/mpeg, with matching initial MP3 frame bytes. The bundled Linux FFmpeg also decoded three seconds from the HTTPS stream successfully. This verifies delivery/decoding here, not the affected Mac WebView. Playback now prefers a verified HTTPS counterpart at the same host/path/query. A four-second bounded, no-redirect header probe rejects HTML/JSON; positive results are cached for 24 hours against station and original URL. Saved station/source URLs are preserved. Existing HTTPS streams need no extra probe. Radio setup/play remains bounded by its 20-second startup deadline and rejects stale completions. HTTP-only stations are not universally repaired by this change; no broad ATS exemption was added.

## Albums searched by artist

The MusicBrainz Albums query searched only the `releasegroup` title field. Thus `nipsey hussle` missed albums whose title did not contain his name. The corrected query searches title OR credited artist/name, escaping Lucene literals. A new cache namespace avoids replaying the prior title-only results. A live artist-field query returned 87 release groups, including Crenshaw and Bullets Ain't Got No Name volumes credited to Nipsey Hussle. This is catalog coverage, not a claim every release is an album or has playable audio.

## Song startup deadline

The screenshot's `Victory Lap` is credited to **Pengus**, a different artist from Nipsey Hussle. `Audio lookup exceeded the remaining playback startup budget` is the shared resolver deadline, before audio attachment; it does not identify a decoder error or prove that every song fails. The bounded deadline is intentional. Current read-only search returned the official Nipsey Hussle Victory Lap video, and explicit inspection extracted five audio formats. This Linux lookup does not qualify Mac/French CDN playback or automatic recording matching.

Post-v0.1.3 stall recovery, rate-limit cooldown and verified resumed-audio fixes remain in the local current source; they cannot be assumed to explain or fix this particular startup timeout. The affected Mac needs one correctly identified recording and the existing Settings YouTube Support diagnosis (extraction plus stream access) to narrow helper/network/CDN failure. Passing installer smokes cover startup, bundled tools and local media, not public network playback.

## Desktop follow-up

The playbar artwork now has an explicit 12px gap before its text; station-name alignment follows that inset. This repairs the missing inner gap after the footer wrapper consolidation. Previous alias/radio/history/menu work is preserved.

These changes are local post-release work, not included in the public v0.1.3 installers. macOS live acceptance remains outstanding.

Verification so far: 1,030 frontend tests pass across 85 files; Svelte check has zero errors/warnings; frontend production and native debug builds pass. Native search regressions pass 2/2; radio suite passes 58 (5 ignored); full native library suite passes 1,151 (23 ignored), all zero failed. The subsequent opt-in live resolver/decoder harness passes separately. Final rebuilt Linux GUI was visually reviewed at 954×1036: 12px artwork spacing, station links and footer geometry pass with no overlap findings. Exactly one GUI is open; persisted Radio Nova queue was preserved and left paused at 9:14.

## Exact Pengus local attempt

A single isolated Linux/debug attempt used the exact MusicBrainz Pengus recording (260 seconds), because the screenshot does not reveal the original provider ID. It used a temporary database with default settings and the production resolver and decoder, without mutating the normal app queue. Automatic resolution accepted a matching YouTube source in **11,925 ms**. First decoded PCM arrived **9,018 ms later**, **20,943 ms from request**, and five seconds of stereo PCM decoded successfully. Cold helper preparation/extraction accounted for about 8.5 seconds of lookup. Artist alias research was skipped because ordinary matching succeeded.

[Machine-readable result](pengus-local-resolver-decode.json). The harness bounds resolution and decoding separately; **this is not a pass of the whole-app 20-second startup deadline**, mixer progress or audible output. It establishes that this recording has a working local source, and exposes significant startup latency; it does not reproduce the reported resolver timeout or prove French Mac success. Network, helper and decoder readiness need to stay separate when interpreting that error.

## Native AAC startup optimization

The instrumented baseline showed the native MP4 demuxer scanning every atom before decoding: 4,210,738 bytes received, 2,556,453 bytes consumed before the first PCM block, reader setup 327 ms and decoder setup 7,716 ms. This was a finite audio-only AAC MP4 stream.

Verified finite remote AAC/MP4 now uses the existing FFmpeg path. Dispatch requires a remote URL, audio/mp4 MIME, AAC codec, positive content length and a finite positive duration; local files, unknown hints and other codecs retain their routes. Source identity, preview, accessibility and head/tail validation remain unchanged. Scoped FFmpeg `-xerror` is essential: the negative-control half-body fixture otherwise emitted errors but returned success. Repeated seeks, decoded tone/sample counts, chapter endpoints, stalled-input cancellation/reaping, HTTP403 and transport truncation pass.

[All exploratory outcomes](pengus-native-startup-comparison.json): successful post-lookup first PCM was **111–271 ms** (median 269 ms), versus **8,046 ms** in the instrumented baseline. Successful total startup was **14.96–15.65 seconds**, versus **21.72 seconds** before. The four optimized cold attempts include **one 20-second resolver failure**: helper preparation ended at 7.67 seconds, then YouTube extraction exhausted the remaining budget. No failed attempt was discarded or retried in that comparison. Source lookup remains a concrete unresolved bottleneck even though decoder startup improved.

These measurements are Linux/debug native resolver+decoder PCM, not mixer/audibility, macOS WebView qualification or a statistical p95 benchmark. One baseline and four optimized attempts were run; compilation overlapped some measurements. Whole-test CPU/RSS receipts include descendants and cannot establish decoder-only CPU savings. A separate helper compilation-cache experiment improved warm readiness only 11.5%, slowed initial population, and was not adopted. Standalone flat searches still do not initialize the token helper.

The final full native suite passes **1,154 tests**, zero failures, 25 opt-in tests ignored; the live cases run separately. Production binary `c745b4d71451` was launched by the primary agent through the verified Vite/MCP launcher. The app is open with the preserved Radio Nova queue, paused at 9:14. Actual 954×1036 screenshot and layout inspection show no overlaps. These changes remain local, with public v0.1.3 unchanged.


## Current platform candidate (supersedes earlier verification checkpoints)

Binary `457e65a6a99b` includes the authenticated direct-audio relay, exact loopback ATS bundle exception, helper/extraction overlap and Windows hidden-console configuration. See the [platform audit](../../platform-playback-compatibility.md) for transport constraints and conditional Ogg/HLS, signing and Linux-package gaps. HTTP-only saved stations now have an exact-stream relay path; the earlier HTTPS-only limitation above describes that historical checkpoint.

Three cold production resolver/decoder attempts produced first PCM at 12.539, 12.803 and 12.170 seconds. Each decoded five seconds of the same matching 260-second source. Extraction overlapped 2.28–2.54 seconds of helper readiness. No paired old-production run was taken in this qualification, so those totals do not independently measure the broker's causal benefit. Restricted-sandbox DNS failures were excluded from network-provider outcomes. [Safe phase summary](pengus-production-broker-summary.json).

The actual isolated Linux WebView played local AAC with 0.926 seconds of clock advancement, and Pengus with first playable observation at 4.851 seconds and 0.534 seconds of subsequent progression. The Pengus WebView run was warmed, not a cold latency sample. These establish full application media attachment/progression here, not physical audibility or Mac success. An earlier seven-second test-client timeout was corrected and retained separately as a harness failure. [Local fixture](webview-local-aac-smoke.json), [Pengus](pengus-webview-smoke.json).

Final source passes 1,166 native tests (25 ignored), 1,032 frontend tests, Svelte diagnostics and both builds. The new installed-app smoke exercises generated AAC over loopback in WebView and verifies packaged Mac ATS configuration. Physical Mac/Windows acceptance and the original French network remain unverified; the public installers have not changed.
