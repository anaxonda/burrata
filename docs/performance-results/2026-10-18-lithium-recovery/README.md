# Lithium post-start recovery — 2026-10-18

## Observed state

The normal app retained Spotify Lithium (`2YodwKJnbPyNKe8XXSE9V7`), Nirvana, Nevermind (Remastered), duration 257.053s. Its selected source was `monochrome:154030613014253568`, and its paused position after failure was 42.112s. `lastMediaFailure` reported a twenty-second post-start stall; the visible message instead reported an expired audio-lookup deadline. No startup failure or native media error was retained. The normal launcher did not enable phase traces, and the audio node had already been detached. These observations establish a post-start stall, not a failed recording search. They do not establish the original network/CDN/decoder cause or the complete historical browser event sequence.

## Reproduced recovery bug and fix

Every browser `playing` event called the seek-baseline updater, including rebuffer-to-playing events without an actual seek. That overwrote earlier steady media progress. A subsequent freeze could therefore fail the five-second steady-progress gate, retain the expired original startup budget, and skip same-source refresh/fallback. The expired-budget branch also replaced the actionable stall reason with a generic lookup message.

The baseline now changes only when the seek serial changes. A new source already creates its own baseline. Rebuffer events on the same source preserve established progress, allowing the existing bounded recovery budget to be renewed. No startup timeout, recording/version match, two-attempt limit or short-source retry protection is relaxed. Expired budgets preserve the specific failure reason.

The regression extends four refresh/fallback/owner-cancellation cases with repeated playing events after established progress, requiring an exact 20,000 ms refresh budget, the same queue occurrence and the retained resume position. A fifth case verifies the expired-budget stall reason. All five fail before the fix and pass afterward; fifteen focused recovery/seek/ownership/cap cases pass. The complete frontend suite passes 994 tests in 82 files, Svelte reports zero errors/warnings, and the frontend production build passes. Native source/binary is unchanged.

## Runtime scope

A verified single-instance Vite/module/MCP relaunch loads the corrected frontend while preserving the full normal queue, occurrence IDs, selected index, paused 42.112s position, volume/repeat/shuffle and confirmed provider account revisions. [Preservation evidence](preservation.json) also records empty player-layout findings. No playback request or provider write was made during this investigation. This is deterministic reproduction and local rollout, not a live reproduction of the original transport stall or a new installer release. Published v0.1.3 installers predate this correction.
