# Lithium post-start recovery — 2026-10-18

## Observed state

The normal app retained Spotify Lithium (`2YodwKJnbPyNKe8XXSE9V7`), Nirvana, Nevermind (Remastered), duration 257.053s. Its selected source was `monochrome:154030613014253568`, and its paused position after failure was 42.112s. `lastMediaFailure` reported a twenty-second post-start stall; the visible message instead reported an expired audio-lookup deadline. No startup failure or native media error was retained. The normal launcher did not enable phase traces, and the audio node had already been detached. These observations establish a post-start stall, not a failed recording search. They do not establish the original network/CDN/decoder cause or the complete historical browser event sequence.

## Reproduced recovery bug and fix

Every browser `playing` event called the seek-baseline updater, including rebuffer-to-playing events without an actual seek. That overwrote earlier steady media progress. A subsequent freeze could therefore fail the five-second steady-progress gate, retain the expired original startup budget, and skip same-source refresh/fallback. The expired-budget branch also replaced the actionable stall reason with a generic lookup message.

The baseline now changes only when the seek serial changes. A new source already creates its own baseline. Rebuffer events on the same source preserve established progress, allowing the existing bounded recovery budget to be renewed. No startup timeout, recording/version match, two-attempt limit or short-source retry protection is relaxed. Expired budgets preserve the specific failure reason.

The regression extends four refresh/fallback/owner-cancellation cases with repeated playing events after established progress, requiring an exact 20,000 ms refresh budget, the same queue occurrence and the retained resume position. A fifth case verifies the expired-budget stall reason. All five fail before the fix and pass afterward; fifteen focused recovery/seek/ownership/cap cases pass. The complete frontend suite passes 994 tests in 82 files, Svelte reports zero errors/warnings, and the frontend production build passes. Native source/binary is unchanged.

## Runtime scope

A verified single-instance Vite/module/MCP relaunch loads the corrected frontend while preserving the full normal queue, occurrence IDs, selected index, paused 42.112s position, volume/repeat/shuffle and confirmed provider account revisions. [Preservation evidence](preservation.json) also records empty player-layout findings. No playback request or provider write was made during this investigation. This is deterministic reproduction and local rollout, not a live reproduction of the original transport stall or a new installer release. Published v0.1.3 installers predate this correction.

## Repeated failure: source delivery and automatic resume

A later user retry remained at 42.112s with readyState 2, unpaused and no media error. A public source probe returned a valid FLAC prefix (24-bit/96kHz stereo, total response length 93,250,039 bytes), but a separate bounded 20 MiB request reset after 129,819 bytes; subsequent probes returned HTTP 429. These are direct source-delivery failures observed during the investigation, not proof that every earlier browser stall had the same transport cause.

Automatic reattachment also accepted fulfilled `play()` plus a position assignment as successful resumed audio. Automatic seeks did not use the manual seek watchdog, while a seeking event could reset the stall watchdog. The resumed-attachment path now independently requires owner-bound media-clock progress, unpaused readyState>=2 and seeking=false inside the original shared deadline. It accepts verified clock progress even if the browser leaves `play()` pending, and rejects stale same-URL sources. Zero-position startup behavior and recording/source identity guards are unchanged. Four regressions cover seeking/frozen, settled/frozen, pending-play progress and stale-owner progress; recovery-success fixtures now simulate real advancement.

HTTP 429 now starts a shared bounded in-memory Monochrome cooldown (Retry-After seconds 1–120, default 30). Catalog/probe retries and remembered source attempts respect it, allowing an eligible provider to use the remaining foreground budget without retrying the rate-limited service. Explicit source pinning, the two-attempt cap and shared deadlines are unchanged. A real local HTTP fixture verifies one 429 probe suppresses a second request, caps the advertised cooldown, and permits requests after expiry. The first restricted run failed at loopback socket binding; the same compiled test then passed with local sockets enabled, and the complete final native suite passes. No transport spoofing or rate-limit bypass is used.

The read-only media inspector now exposes bounded seeking/buffered/seekable fields without URLs. Numeric range bounds and maximum eight intervals are checked in both frontend and native projection.

## Current live result

The corrected frozen binary `b36940b3cbad` and frontend `ac945691b9c5` are running through the verified one-GUI launcher. The original normal queue occurrence, paused 42.112s position, volume and repeat/shuffle state were preserved before the explicit resume. A first automation request was sent before frontend registration, timed out and did not start playback; the ready-window retry is the qualification below. This was a launcher/test timing error, not a source failure.

The [live retry proof](stream-retry-proof.json) shows the same Monochrome recording and queue occurrence advancing through 54.624s and 73.953s, seek complete, readyState 4, no retained startup/media failure. The service delivered audio successfully on this retry; this does not prove that its transient reset/429 condition can never recur, nor that YouTube fallback played in this run. Monitoring stopped when the current recording/queue changed; that newer playback was left untouched. No full-recording, acoustic or statistical performance claim is made.

All 1,114 native tests pass (21 ignored); all 998 frontend tests pass in 82 files; Svelte reports zero errors/warnings and both builds pass. The installed public v0.1.3 binaries still predate these local corrections.
