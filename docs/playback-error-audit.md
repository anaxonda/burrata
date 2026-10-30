# Playback error audit — 2026-10-29

This audit follows two reported production failures: a linked Bandcamp import rejected before playback, and a native HTTP startup failure that reported only an unexpected response status. Passing installer startup and synthetic-audio smoke tests did not qualify these provider paths.

## Fixes

- Explicit Bandcamp pages bypass automatic discovery eligibility. Discovery deliberately rejects pinned requests; applying that gate to an already selected page prevented extraction entirely. Bandcamp extraction also retains the page’s structured artist and track fields instead of relying on a flattened caption. Pinned Bandcamp audio still requires fresh artist, numeric provider ID, title, duration and version validation, including on recovery. A contradictory fresh page artist cannot be overridden by matching words in the caption.
- Native initial and range failures retain only the HTTP status and phase. URLs, authentication headers and response bodies are excluded. Status is checked before successful-response metadata is parsed.
- Native request/body failures retain transport retry semantics even when reqwest provides no HTTP status or useful nested cause. Empty media responses fail as transport errors; byte validation remains strict.
- Decoder transport failures between successful source preflight and first audio may refresh the source. These attempts share the original startup deadline and the existing two-recovery cap. An already exhausted resolver does not gain a fresh startup budget. Cancellation and output-device failures do not enter this startup transport path.
- Failed native snapshots are treated as paused by the compatibility bridge. Track rows distinguish **Unavailable** from **Paused**. The player owns one current error presentation, with technical detail in an information disclosure and existing retry/source-information actions. A previous skipped-recording notice is cleared when a current playback failure takes priority.
- Background source-preparation events may retain at most one quarter of the bounded diagnostic ring. They cannot crowd out foreground failure evidence. Bandcamp is retained as a distinct diagnostic provider rather than appearing as “other”.

- A missing or changed local-only file cannot become an online source search after library hydration removes its path. Playback stops with a file-specific storage message. Catalog recordings with remote source identities retain their existing fallback policy.

- Waiting-source scheduling selects the oldest-due request rather than the first eligible row. A 100-request regression crosses the hourly retry boundary and verifies that every request gets a turn.

## Verification matrix

The native transport fixtures exercise real loopback responses: valid 200/206 bodies; 400, 401, 403, 404, 410, 416, 429, 500 and 503 initial responses; rejected 200 range responses; mismatched ranges and validators; truncated/overflow bodies; and cancellation before headers/seek handshakes complete. They retain strict byte-range validation rather than accepting incompatible content after a seek.

Recovery tests cover startup versus resolver failures, original deadlines, exhausted retry caps, cancellation, output failures, same-candidate URL-cache invalidation and queue failure classification. Existing identity suites retain wrong-artist, version, provider-ID, duration and stale-mapping rejection cases. Frontend tests cover failure/recovery/success transitions, queued occurrence retries, privacy redaction, notice priority and source controls.

The original “500 Miles” HTTP status was discarded by the old transport, so it cannot be reconstructed retrospectively. Controlled status fixtures verify the corrected error and recovery path.

A separate ignored **live_bandcamp_sketches_import_resolves_without_changing_identity** test runs the complete resolver against the reported public track, using an isolated database and no user playback session. It reproduced the production error before the routing fix. The Linux release workflow runs it explicitly before packaging. A provider outage fails that qualification step; a green synthetic smoke alone is insufficient to publish.

## Release qualification limits

Deterministic fixtures verify behavior under controlled failures; they do not establish current provider availability. Live resolution verifies source identity and accessible media bytes, not physical audio output. Installed-artifact audio smoke remains a separate required check. No claim of live Soulseek peer-transfer qualification should be made without a configured daemon and an available consenting peer/test fixture. Cross-platform CI and installed-artifact checks must pass for the exact release commit before publishing another public release.

Local verification of the frozen implementation passed **2,285 native tests** (56 explicit environment/live qualifications ignored in the ordinary suite), **1,728 frontend tests**, **12 MCP tests**, and **8 repository contract tests**. The live Bandcamp resolver qualification passed separately. Release packaging and cross-platform results are recorded by the workflow for the exact tagged source commit.

## Unreleased follow-up: first WebView start on the reported YouTube mix

The original production report for `youtube:mDtud5fLgFQ` shows a 20,142 ms first attempt with a configured 20-second limit. Its attachment stopped after 5,052 ms in the play phase, still at `readyState=0`, `networkState=2`, time zero, and without a recorded media error. Source identity and both range probes had already passed. Replay completed in 6,208 ms, with media reaching playing about 5,788 ms after play. The timeout identifies the failure boundary, not a reason to increase the configured budget.

The missing startup work includes the synchronous first-use media capability probe. In a fresh isolated WebView using the release's bundled media runtime, the first Ogg/Vorbis `canPlayType()` call blocked for 6,189 ms; its immediate repetition took effectively zero time. Previously this call ran while constructing resolver arguments, before source lookup was dispatched. It therefore serialized media-registry initialization and provider work inside the shared startup budget.

Ordinary source lookup and refresh now dispatch their backend request first, then initialize the WebView media capability cache while the lookup is in flight. Each WebView reuses its probe result. Radio retains actual Vorbis/Opus capability information for live Ogg conversion. The 20/30/45-second settings, identity checks, cancellation ownership and existing recovery bounds are unchanged.

A fresh private run reproduced the old timeout. A controlled check of the patched build used a 14-second synthetic lookup plus real AAC decoding through the packaged WebKit runtime and a silent virtual sink. Its 4,955 ms cold probe began after lookup dispatch and overlapped that lookup; media reached playing in 14,462 ms and the full startup completed in 14,473 ms, with the original 20-second deadline and a progressing media clock. This controls the scheduling variable without relying on changing provider availability; it does not measure physical audibility or guarantee live YouTube access.

The existing `inspect_webview_media` and `get_playback_diagnostics_report` read-only tools also expose bounded startup history when normal playback diagnostics are enabled, including failures retained after replay. They use the same redacted projection as export: no signed audio URLs, headers, titles or paths. A missing frontend leaves backend events available. Future inspection does not require a hidden session flag or a manual export dialog.

Verification: all **529 app integration and codec tests**, Svelte checks with zero errors/warnings, the native startup-history bounds/redaction regression, the native application build, and the real-WebView controlled check passed. These fixes are local follow-up changes and are not in the published v0.1.23 build.

## Unreleased follow-up: playback lifecycle audit — 2026-10-30

This pass examined first start versus resuming an attached stream, source ownership during HLS failures, native bridge initialization/disposal, optional waveform routing, seek/stall ownership, and native/relay HTTP response validation. Four additional defects were reproduced with deterministic regressions:

- **A frozen resume could report success.** Resuming an existing WebView source raced its `play()` promise against observed progress. A resolved promise won even when the media clock never advanced. Resume now requires at least 250 ms of owned media-clock progress while unpaused, ready, not seeking, and without a media error. The existing startup deadline still bounds both pending and resolved-but-frozen promises. Replay of an ended source measures progress from zero. Polling exists only during the bounded resume attempt and is cleared on completion or cancellation.
- **An obsolete HLS failure could reach the active player.** Fatal callbacks checked whether the HLS instance was destroyed, but not whether the caller still owned that source. The adapter now checks current-source ownership before forwarding the failure, including after the error callback in case it changes ownership. Obsolete transports are cleaned up without emitting a current media error.
- **Cancelled native initialization could retain callbacks or overwrite a replacement.** Listener registration and initial snapshot retrieval could complete after disposal/reinitialization. Late registrations are now released, stale state/position events and snapshots are ignored, and an obsolete initialization cannot dispose its successor. Tests exercise both delayed subscription and delayed snapshot completion.
- **An unsuccessful optional waveform lookup could disable stall recovery.** The lookup replaced recovery state with a speculative new state, then cleared it if no route was attached. The still-playing source consequently lost its watchdog. Its existing recovery state, successful-attachment evidence, retry history, and deadline now transfer to the new request generation while the optional lookup runs. Only an actual route replacement starts the new attachment state. The regression starts real application logic, fails the optional lookup, then verifies that a subsequent stall still requests recovery.

The new failure fixtures were run against the pre-fix behavior and failed, then passed with the fixes. Some older integration fixtures required correction: resume-success fixtures now advance media time, and an engine-transition fixture supplies a successful native bootstrap instead of delivering events through a disposed subscription.

The native HTTP/relay/storage/failure-classification selection passed **36 tests**, including loopback HTTP and finite HLS decode/seek fixtures. These changes do not alter source matching, startup timeout settings, or the published/running v0.1.23 application. Deterministic tests establish behavior under controlled failures; this pass does not claim new live-provider or physical-speaker qualification.

Final verification: **154 frontend test files / 1,738 tests** passed across the broad run and corrected affected-suite reruns. The complete app/native-bridge/HLS-adapter selection passed **548 tests**, and the final resume/UI-focused rerun passed **29 tests**. Svelte reported zero errors and zero warnings; the production frontend build passed. The existing large-chunk build advisory remains. Logs and before/after source hashes are retained under `.cache/playback-lifecycle-audit-20261030/`.

## Unreleased follow-up: fallback starvation and stall evidence — 2026-10-30

An observed catalog album attempt rejected YouTube candidates after about 2.3 seconds, but its simultaneous Monochrome lookup occupied almost the whole 20-second deadline. The old scheduler waited for both before trying the remaining configured fallbacks. The fallback task now contains the remaining serial chain, allowing it to advance while the preferred provider is still pending. This keeps at most two provider lookups active and preserves matching, priority grace, cancellation, and the shared deadline. The existing special handling when Bandcamp is the first fallback remains unchanged.

A separate “After You've Gone” source started successfully, then stopped making observable clock progress after a seek. Its eight-second fresh-source recovery also timed out. Existing evidence cannot distinguish a transport/decoder stall from an unobserved clock problem. Watchdog expiry now records one bounded, redacted media sample before recovery discards the element, improving the evidence for a recurrence without adding periodic sampling. This is a diagnostic improvement, not a claim that the original stall is fixed.
