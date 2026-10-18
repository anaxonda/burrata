# Private startup PCM calibration: failed before play

Candidate: native `7a43ea0c8def`, App `da4309cd0191`; isolated fixture-only profile and an owned private null audio sink. The prototype required received silent PCM before dispatching one queued coded-local request. It timed out obtaining that baseline, so no playback request was dispatched and no startup latency is qualified.

The retained result reports `startup_pcm_silence_baseline_timeout`, `dispatchAttempted:false`, verified recorder cleanup, restored volume and paused state. Exact pinned prototype files remained unchanged through the run. Raw PCM was not retained. This is capture-tool evidence, not an application playback failure or a first-audio measurement. The baseline/sink behavior and guard overhead need investigation before another attempt.

Private result: `.cache/qualification-local-codecs/startup-pcm-da430-result-private.json`; fixture/tool pins and lifecycle evidence are retained under the local-codecs and fruit-unpinned-webview/startup-pcm qualification directories. The isolated app was stopped and its owned sink removed before restoring the normal app.

The source review found that the two-second baseline timer starts before the capture helper completes its pre-spawn ownership checks. Thus guard overhead consumes the baseline budget. Monitor suspension remains unproven: the failed report lacks first-data/byte diagnostics. A private fix is in progress to start the received-silence deadline after successful recorder spawn, separately bound preflight, and retain failure diagnostics. No timing threshold or received-zero requirement is being relaxed.

The normal app is restored with a mounted frontend and 99-tool authenticated handshake. Complete before/after app snapshots are exactly equal; the owned sink is removed. See `startup-pcm/restoration-proof.json` in retained qualification evidence.

The timer correction passes 17 offline protocol checks and a separate lifecycle review. Before live retry, a three-sample instrumentation measurement found one full read/SHA256 of the 360,807,552-byte debug binary costs 1,132/1,201/1,181 ms. Repeated full hashes in each capture guard would perturb PCM callback cadence. A private adapter refinement is therefore underway: hash at the boundaries and verify unchanged file identity/metadata during polling. This is instrumentation overhead evidence, not application playback timing.

## One retained retry after capture-tool fixes

The same isolated fixture profile was relaunched after timer, awaited-guard and file-pin fixes. It received 19,200 silent bytes before dispatch, with the first recorder data 38 ms after capture start. The one queued play was accepted. The 30-second capture retained 5,771,520 bytes in memory and cleaned up its recorder normally; raw PCM was not saved.

Qualification still fails: observed chunk drift reached 93 ms against the fixed 25 ms startup-calibration threshold. No startup latency is qualified. Guard snapshots were materially cheaper after removing repeated hashes, but one still took 303 ms; synchronous subprocess checks remain a possible source of callback disturbance, not proven audio-device delay.

Exact-operation cancellation reached terminal `cancelled`, volume restoration and final candidate hashes passed. The immediate report has `pausedFinally:false`; a separate later read confirms paused on the same track/session/source/entry with volume0.8. This exposes a missing bounded acknowledgement wait in the capture runner; it does not retroactively make the run pass. Both results are retained. The isolated app was then stopped and its owned sink removed; tool pins matched through the run.

Evidence: `startup-pcm-da430-retry-result-private.json` and derived `startup-pcm-retry-summary.json` in local-codecs qualification; lifecycle and later-state read are under `fruit-unpinned-webview/startup-pcm-retry`. These results do not qualify remote playback, first mixer audio, physical audibility or a release.

The next private revision uses asynchronous subprocess/proc reads and awaits up to three seconds for the exact owner to acknowledge pause after confirmed cancellation. Its offline checks and independent review pass. It retains the original 25ms capture threshold; live qualification remains contingent on the next retained result.

## Asynchronous guard run: still unqualified

One further retained run uses the asynchronous guard and exact-owner paused acknowledgement. It receives the 19,200-byte silent baseline, dispatches one queued play and captures 5,752,320 bytes without retaining raw PCM. Recorder first data arrives after 39ms. Guard median/worst is 68/135ms in this run, compared with 303ms worst in the previous run; these are single diagnostic runs, not a controlled performance comparison.

The first failing callback residual is 48ms against the unchanged 25ms limit, so startup timing remains unqualified. Exact-operation cancellation, terminal status, bounded paused acknowledgement, recorder closure, volume restoration and final candidate hashes all pass. The result retains `failureCategory:null` because the protocol finished but onset qualification failed; the runner exits nonzero. The authoritative failure reason is `onset.reason:capture_chunk_timing_unqualified` and its nested cadence reason. This reporting distinction should be made clearer before broader use.

The isolated app was stopped, its owned sink removed and tool hashes verified unchanged. Results and derived summary: `.cache/qualification-local-codecs/startup-pcm-da430-async-result-private.json` and `startup-pcm-async-summary.json`; lifecycle evidence is under `fruit-unpinned-webview/startup-pcm-async`.
