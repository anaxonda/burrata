# Recover the WebView buffering indicator after progress resumes

The isolated PCM control demonstrated a concrete UI defect: after seek, WebKit reported readyState2 while its media clock and private sink output continued, leaving the app's waiting/stalled flag set without another playing event.

The frontend now clears that UI flag after two consecutive plausible near-1× media/wall progress intervals from the current audio node, source generation and seek serial. It requires current media data (readyState≥2), an unpaused, non-seeking, non-ended, error-free element. Source changes, seeking/seeked, waiting/stalled, play/pause, ending and errors reset the evidence. Stale WebView events cannot update the native engine's position. The helper stores one sample and a capped interval count; it does not scan the queue or recording lists.

This change does not claim clock progress always proves decoded output, change decoder behavior or weaken source/error/retry gates. Genuine halted clocks and metadata-only state keep the indicator. Broad playback/codec/provider qualification remains open.

Focused private tests pass. Main frontend validation passes879 tests across75 files, Svelte zero errors/warnings and eight contract checks under `.cache/qualification-buffering-ui-recovery/`: helper cases cover progress, halted clocks, seek jumps, implausible fast/slow clocks and paused/error/stale events; App regressions compare readyState1 and readyState2 after waiting/seeked and check no candidate retry. Native source/resource inputs are unchanged from the 999-pass candidate; the existing binary can host this frontend through protected Vite after verification. The currently running normal window still has the prior protected frontend until explicit handoff.

Source evidence for the defect: [isolated output controls](../2026-10-17-isolated-seek-output/README.md). Root-reviewed patch: `/tmp/webview-buffering-ui-recovery.patch`, plus the readyState1 guard/regression. Source pins record all531 inputs and the four-file frontend-only change.


Frontend production build passes. The source remains pinned at531 inputs; native code/resources are unchanged. Three actual new-frontend direct-seek controls pass clock/ownership/cleanup checks, all at readyState4. Captures2 and3 completed with nonzero output; capture1 hit its initial10-second stream-appearance wait before the stream was observed (the driver first trace was13.158s), so no PCM conclusion is drawn from that attempt. The intermittent readyState2 case was not reproduced on this frontend; the regression tests cover it, but live recovery qualification remains open. Normal app restored with native `ecc2c40a10e8` / frontend `9edf424726b2` and exact fresh30-entry queue/settings/position3.287602s unchanged. See `.cache/qualification-kaufman-buffering-current/normal-launch/restoration.json`.

## Repeated-seek harness preparation

The private batch harness now has independent child process groups, bounded TERM/KILL/close observation, explicit cleanup uncertainty that stops the next case, and a bounded 20-second initial owned-stream wait retaining each no-stream observation. Root reran 28 offline tests successfully, covering child termination, stream ownership/wait, recorder cleanup, batch failure retention/cancellation, and measured baseline/post-seek PCM correlation. A sandbox launch failed before execution with a temporary-directory quota error; the same tests passed on the host. This is harness qualification only: no new live seek repetitions, latency statistics, codec qualification or readyState2 recovery reproduction are claimed.

A read-only current normal-app check at 954×1036 additionally shows loaded footer fallback artwork and no layout findings; the reviewed screenshot has no playback-bar overlap in Library. See the [artwork follow-up](../2026-10-17-artwork-album-regression/README.md).
