# Current Kaufman album traversal: harness limitations retained

Two isolated runs used the exact19 catalog rows, in original order, on debug candidate `dd9474d3fdf9`. Seeding preserved metadata exactly, added no candidate/preview/source override to those rows, and passed SQLite integrity/FK checks. No remote write occurred.

The first run stopped after2.52s because the harness incorrectly required `engine: webview` in `get_app_status.playback`, which omits that field for WebView. Queue IDs and19 distinct occurrence IDs were correct. The guard already required the WebView process; media snapshots explicitly identify their engine. This DTO assertion was corrected, with regression checks for the actual missing-field response. A separate8s play-call timeout was corrected to the bounded overall traversal budget rather than undercutting the production20s per-occurrence startup deadline.

The second run observed indices0,1,2 in order, then stopped after7.88s because the first skipped-track notice had not been captured before the next failure replaced it. Its error `occurrence_transition_gap` describes the pending-notice assertion, not a numerical queue-index gap or proven application failure. Cleanup paused and restored volume in both runs. The failed reports remain untouched in `.cache/qualification-kaufman-full19-current/` and `.cache/qualification-kaufman-full19-current-v2/`.

Neither run qualifies album playback, global source unavailability, a working post-third-failure transition, recording identity, audibility or mixer consumption. No further blind retry is warranted: per-occurrence failure evidence must be observable independently of a rapidly replaced last-notice label, or the diagnostic must report missing reasons honestly. The original album acceptance remains open.


## Retained-history harness preparation

The private runner now consumes `recentTrackFailures` using exact queue revision, occurrence ID, track ID and bounded generation checks. Explicit final-occurrence queue exhaustion is supported. Index jumps require terminal evidence for every bypassed row; an index alone never proves a failure. Matching retained evidence clears a pending notice. Root review caught and corrected an unconditional index-gap rejection before any live run. Six failure-history cases and four transition cases pass offline; no new live album outcome is claimed. Files: `.tmp/kaufman-full19-reviewed-candidate/{failure-evidence,transition-evidence}.mjs`.


## New history-bearing live candidate

Candidate `ecc2c40a10e8` run (`.cache/qualification-kaufman-ledger-current/run-private.json`) stopped after10.628s with `owned_notice_unavailable`; no terminal occurrence was classified by the runner. The subsequent actual `playback_state` receipt retains two explicit failures with exact occurrence IDs: Vgradena Nevesta and Duda, both no matching requested recording/version. The helper incorrectly required top-level native `queueRevision`/`generation`; the WebView DTO omits these, and its source generation can be null after cleanup. This silently discarded valid history before the accessibility fallback failed. No full-album conclusion is supported. Reader correction is private/in progress; no blind retry was made. Cleanup and normal-state restoration are verified.


## Corrected-reader traversal on buffering-fix frontend

Native `ecc2c40a10e8` / frontend `9edf424726b2` traversed14 exact occurrences with explicit owned `no_playable_match` evidence in47.988s. The15th occurrence (index14) was resolving when the legacy `play_tracks` frontend-response waiter timed out at its fixed45-second limit and canceled the request; five final occurrences remain unqualified. The runner result is a failure, not full-album acceptance or proof of all-source unavailability. Both cleanup checks pass. Root verified the45s timeout in automation.rs; a recoverable queued-command contract is being implemented privately instead of extending the RPC indefinitely. Evidence: `.cache/qualification-kaufman-buffering-current/run-private.json`.
