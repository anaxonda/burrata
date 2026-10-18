# Current-build coded seek output

The isolated Linux debug WebView candidate is native `97597ce4c7fd`, frontend `a2f1055c10d8`. A distinct 60-second stereo PCM fixture encodes each media second as a different frequency. Capture reads only the owned app-routed null-sink monitor, analyzes PCM in memory and persists code/frequency metadata, not raw audio. Candidate/process/profile/track/fixture/metadata bindings and recorder chunk timing are checked before correlation.

The first direct attempt passed its seek protocol and completed capture, but correlation rejected `coded_fixture_binding_mismatch`: the private seek report omitted its redundant stimulus-binding track ID. The original reports remain unchanged. The private emitter was corrected; application code did not change.

The repeated direct attempt passes seek protocol and output correlation. Forty stereo post-seek windows show progressing media bins 49–53 after a target of 48 seconds, within the correlator's one-second-bin tolerance and in coarse agreement with the media trace. Chunk coverage is complete with a 35 ms first-data delay and maximum cadence residual 208 ms. Final pause and original volume restoration pass.

This is one functional content-position control. It does not establish exact sample landing, onset/p95 latency, physical speaker output, catalog recording identity, repeated reliability, other codecs or platforms, or the intermittent buffering-recovery case. Paused-seek verification and the accepted repetition matrix remain open.

Private evidence is under `.cache/qualification-local-codecs/`: `publication-coded-direct-*`, the corrected `*-02-*` reports, and `seek-binding-correction-private.json`. The candidate is bound to the retained 541-input build snapshot in `.cache/qualification-queued-publication/`.

### Paused-seek control

The first ordinary 2-second resume-control attempt failed with a sanitized transport error at `resume_playback`; final pause and volume restoration still passed. Capture completed, but protocol failure correctly prevented output qualification. The private diagnostic now retains typed numeric MCP timeout metadata without raw exception text.

A separately labeled 8-second acknowledgement diagnostic (still inside the original 45-second total) passed protocol and coded-output correlation: 41 stereo windows show media bins 48–52. Actual resume acknowledgement took 173 ms, below the previous 2-second limit. This does not reproduce or explain the earlier intermittent failure and does not promote the longer deadline or establish repeated/default-budget reliability. Both attempts remain retained.

### Current desktop capture

An app resize request was ignored by the tiling window manager; its 1914px result is not half-width evidence. A reversible resize targeted only the exact isolated app container, then an actual 954×515 capture reported no overlaps and a 112.39px footer. The original tiled 1914×515 viewport was restored and verified. The view also exposed a stale skipped-track banner after a fresh queue replacement; that issue is tracked separately.

### Repetition in progress on the notice-fix frontend

The existing direct-seek batch is running on native `97597ce4c7fd`, frontend `3943b23cfb37`, with the default 2-second resume budget and 17 pinned diagnostic source files. At the inspected checkpoint, 11 of the planned 23 cases (three exploratory plus 20 measured) had completed: ten passed and one measured case was unqualified. This is a partial checkpoint, not a completed reliability result.

Case 6 passed playback controls and cleanup, but output correlation rejected capture cadence drift: maximum residual 284 ms exceeded the unchanged 250 ms limit. No capture error or cleanup failure was reported. This prevents a content-position qualification for that case; it does not by itself establish an application playback failure. The original report and remaining batch are retained unchanged. Final counts must come from the completed batch report.

### Completed default-budget direct batch

The notice-fix frontend batch completed all 23 scheduled cases: all three exploratory controls and 19 of 20 measured controls qualified. The one measured rejection remains case 6's capture-cadence residual (284 ms versus the unchanged 250 ms limit). All 23 completed their playback controls, pause/volume restoration and child-process cleanup without reported cleanup failures. The runner exits nonzero because qualification was not unanimous. All 17 pinned diagnostic source hashes remained unchanged through completion.

This establishes repeated coarse coded-PCM direct-seek checks within that isolated Linux debug WebView fixture, with one unqualified capture. It does not establish a 95% application playback success rate: the rejected capture cannot distinguish playback correctness. Paused seeks, other codecs, physical output, latency p95, resource use and other platforms remain unqualified. The complete private report is `notice-coded-direct-batch-private.json`, with per-case reports retained alongside it.
