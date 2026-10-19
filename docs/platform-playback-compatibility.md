# Desktop playback compatibility investigation

Post-v0.1.3 work, 2026-10-18. The France report is from an Apple Silicon MacBook, likely on recent macOS. Nova plays in its browser. The reported song-start error is a resolver deadline, while the radio error comes from WebView playback; they need separate remedies.

## macOS transport fix

The released app passes saved HTTP radio URLs straight to WKWebView. CSP allows those URLs but does not override App Transport Security. Nova has a working HTTPS endpoint, now selected after verifying the exact same host/mount/query serves audio. For HTTP-only saved stations, the new backend relay fetches the exact saved stream and serves an opaque, authenticated, generation-owned loopback URL. It also preserves required upstream audio headers that an HTML audio element cannot set. No alternate recording or station is substituted; no playback-engine handoff is introduced.

A second packaging gap matters for both existing local audio and the new relay: [Apple documents](https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking) that macOS 14+ no longer exempts IP-address connections from ATS by default. Burrata supports macOS 14+ and uses `127.0.0.1`. The bundle now merges an exception for HTTP at exactly that address. Public HTTP domains, other IP addresses, and global arbitrary-load exceptions remain excluded. The installed-app smoke reads the actual packaged `Contents/Info.plist`, rather than trusting the source plist.

The relay accepts no upstream URL from HTTP clients. Rust registers only resolver-owned or exact saved-station sources; public DNS is checked and pinned. Secret URLs, allowed origins, four-request concurrency limits, single ranges, HEAD, backpressure, no redirects, cancellation and no-store responses constrain its lifetime and use. Anonymous CORS is set before attaching only these relay URLs, retaining cross-origin PCM access without changing other providers' loading behavior. HLS manifests are excluded because relative segment/key URLs would require separate validated rewriting.

This is a plausible targeted Mac fix with deterministic transport evidence. It is not a claim of physical-Mac playback qualification. The original Mac needs the new packaged bundle; changing a frontend alone cannot add the ATS exception.

## Startup delay

The native decoder previously received the whole 4.2 MB Pengus AAC MP4 before producing audio. Verified finite remote AAC now uses the existing FFmpeg path with strict transport errors. Successful first PCM after resolution fell from about eight seconds to 0.11–0.27 seconds. This improvement applies to native playback, not WKWebView. [All outcomes and limits](performance-results/2026-10-18-france-macos/pengus-native-startup-comparison.json).

A source lookup can still consume the shared 20-second budget. Some earlier cold trials overlapped compilation, so they cannot establish French network failure or a clean reliability rate. A controlled prototype starts the helper and extraction concurrently only after an actual extraction is requested, gating token-provider access until ready. In the conservative clean reverse pair it reduced the same recording's source startup from 9.47 to 7.95 seconds. Token/ad-context flags alone did not produce a repeatable gain and were not adopted. Production overlap is now implemented with independent helper leases, absolute-deadline readiness/token cancellation and race-safe last-user cleanup. Three isolated cold production resolver/decoder runs produced first PCM in 12.17–12.80 seconds, with 2.28–2.54 seconds of helper readiness overlapping extraction. This is three exploratory Linux samples, not p95 qualification or proof of a French network improvement. [Results](performance-results/2026-10-18-france-macos/pengus-production-broker-summary.json). Flat searches still do not start the helper.

## Other system-specific findings

| Area | Evidence and effect | Remedy / qualification |
|---|---|---|
| Older supported Mac codecs | Ogg support arrived in Safari 18.4 / macOS 15.4; macOS 14 is supported by Burrata. This can affect Ogg radio even when MP3 works. | Select a verified compatible encoding or transcode the exact stream through an owned backend path. Do not confuse this with Nova's MP3 case. [WebKit release notes](https://webkit.org/blog/16574/webkit-features-in-safari-18-4/). |
| Windows child consoles | FFmpeg/ffprobe/SongRec launches lacked the no-console flag already used by yt-dlp. | Shared Windows `CREATE_NO_WINDOW` configuration now covers these GUI-owned helper launches. Windows GUI observation remains necessary. |
| HLS on Linux / non-native HLS WebViews | hls.js uses cross-origin fetch; CSP permission cannot grant remote CORS. Existing adapter tests mock the library. | Native manifest/segment relay with bounded URL validation/rewrite, or a precise unsupported-transport error; qualify real fixtures with and without CORS. The direct-audio relay does not claim to fix HLS. |
| Downloaded Mac security | The prerelease is unsigned/not notarized. Existing installer smoke copies a DMG without quarantined-download prompts. | Sign nested executables/bundle, notarize/staple, and test a clean quarantined download before stable distribution. Existing instructions disclose the limitation. |
| Linux package variants | Installed smoke covers DEB; RPM and AppImage are built and checksummed. | Add installed/runtime smoke for each variant before claiming equal qualification. |

No concrete asset-protocol scope/CSP regression was found in the audited local-file registration paths. Bundled tools are target-triple scoped and installer smoke invokes them on all three desktop systems; that evidence covers tool launch/local decode, not public streaming or first-launch security prompts.

## Acceptance evidence

Final-source verification: 1,166 native tests pass (25 opt-in cases ignored), including six relay fixtures and actual header-required AAC decoding; 1,032 frontend tests pass with zero Svelte errors/warnings. Native and frontend builds pass. Six Python installed-smoke tests, the WebView protocol regression and four offline helper-shim tests pass. The installed smoke now checks the actual Mac bundle ATS exception and plays generated local AAC through WebView rather than checking only a heartbeat.

Actual Linux WebView checks on binary `457e65a6a99b`: isolated local AAC advanced 0.926 seconds; the exact Pengus fixture became playable at 4.851 seconds and advanced 0.534 seconds with the expected 260-second duration. The latter run used warmed state and must not be compared as a cold benchmark. An earlier observation client used a seven-second RPC timeout and abandoned its request as resolution finished; it is retained as a harness failure, not counted as a playback failure. [Local AAC](performance-results/2026-10-18-france-macos/webview-local-aac-smoke.json), [Pengus WebView](performance-results/2026-10-18-france-macos/pengus-webview-smoke.json).

Physical macOS and Windows runtime acceptance remains outstanding. No Mac bundle was built or exercised on this Linux host. The new ATS exception requires installation of a newly packaged Mac build; public v0.1.3 remains unchanged.


## Objective completion audit

| Requested outcome | Current evidence | Limit |
|---|---|---|
| Plausible remedy for the French Mac playback error | Exact Nova HTTPS stream decoded; exact saved/source relay implemented and decoded by fixture; Apple-documented macOS 14 loopback ATS policy addressed by bundled plist and installed-plist verifier | Cause on the original device is plausible, not proven; a new Mac package and affected-device playback are still needed |
| Investigate and improve delayed song startup | Exact Pengus recording resolves/decodes; native MP4 full-file scan removed; helper/extraction readiness overlaps; three cold production PCM runs and one warmed full WebView progression pass | Does not meet a proven five-second cold p95; no French network measurement |
| Identify other likely system-specific gaps | Codec, HLS/CORS, Windows helper-console, Mac signing/quarantine and Linux package qualification findings recorded above | Conditional codec/HLS remedies and physical-platform qualification remain follow-up |
| Leave current desktop usable | Latest binary `457e65a6a99b` launched with mounted frontend; exactly one GUI; persisted queue entries and confirmed account flags match pre-test state; paused startup preserved; 954×1036 visual capture and geometry show no footer overlaps | Linux desktop observation only |

Completed implementation and investigation are archived; physical-platform acceptance is deliberately retained in the current task index. No public release or account write was performed.


## Windows follow-up audit

The app defaults to WebView2 playback; native WASAPI output remains an internal opt-in. Windows has no macOS ATS policy, so the narrow Mac plist change is not a Windows fix. The new audio relay does preserve source headers and same-source range behavior on either platform, and its origin allowlist includes Tauri's Windows `http://tauri.localhost` origin.

| Finding | Current behavior / action | Evidence boundary |
|---|---|---|
| Background console flashes | Extended the existing `CREATE_NO_WINDOW` helper to BgUtils Node, provider/auth Node workers, metadata yt-dlp, identification/autotag FFmpeg/fpcalc/SongRec, registry integration and taskkill cleanup. Interactive browser visibility is preserved. | Source audit; Windows visual verification remains needed. |
| Missing WebView2 runtime | NSIS configuration already embeds the offline WebView2 installer. No change required. | [Tauri installer options](https://v2.tauri.app/distribute/windows-installer/) and current `tauri.conf.json`. |
| Windows N codecs | N editions omit media components including AAC/MP3-related codecs. Installing WebView2 alone does not establish that every media format is available. Test N separately; the supported OS remedy is Microsoft's optional Media Feature Pack and restart. | Conditional WebView2 media risk, not a Burrata reproduction. [Microsoft documentation](https://support.microsoft.com/en-us/windows/experience/platform-variants/media-feature-pack-for-windows-n). |
| Unsigned downloaded executables | Current release is unsigned. SmartScreen or Smart App Control may warn or block; corporate security tools may also prevent helper launch. | Do not advise disabling protection. Existing install guide explains trusted-download handling and administrator policy. [Microsoft distribution guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation). |
| Device changes / sleep / shutdown | Native device construction/control/drop stays on its worker thread; device-loss errors trigger cancellation and bounded reopening. No Windows-specific defect found. Device-driver teardown itself is not time-bounded. | Device-free decode tests do not qualify Bluetooth/USB switch, unplug/replug, sleep/wake or physical output on Windows. Test both default WebView2 and optional native engine. |
| Bundled helper paths | Compiled target triple, `.exe` suffix and resource root normalization are consistent. Helpers are launched directly with separate arguments, avoiding shell quoting of paths with spaces. | No proven packaged-path defect. Qualify installed non-ASCII user paths and a standard account on Windows. |
| In-app provider accounts | Windows native provider-session support is deliberately gated; this is a known feature limitation, not a general playback failure. | Public-page companion imports are separate. Do not claim Windows Spotify/YouTube account parity; see [install guide](install.md#spotify-and-youtube-accounts). |

Before claiming Windows runtime coverage: install the NSIS package as a standard user; verify actual WebView2 AAC and MP3 media clock progression; exercise a fresh public source and radio; verify seek/cancel and helper reaping; check no console flashes; test output-device switching and sleep/wake; repeat media checks on an N edition with/without the Media Feature Pack. The installed smoke now checks generated AAC in the default WebView2 engine on Windows CI, but it has not run on this uncommitted candidate. No Windows hardware is available in this Linux session.

Windows follow-up source validation: 1,166 native tests pass (25 ignored) on Linux, six installed-smoke Python tests and the WebView protocol regression pass; native debug build `f6cdec2a7a6c` succeeds. No frontend behavior changed in this follow-up. These checks do not execute Windows-only creation flags or a Windows audio device.
