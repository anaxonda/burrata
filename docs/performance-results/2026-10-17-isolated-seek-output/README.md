# WebView seek readiness versus isolated PCM output

Linux debug candidate `ecc2c40a10e8`, protected frontend `20381f78de0a…`, synthetic 60-second stereo 48kHz WAV. The sole test app was routed via its own `PULSE_SINK` to a uniquely named temporary null sink. Capture verified PID/start time/profile, descendant stream ownership, module index/name/arguments and monitor ownership; any foreign stream would reject capture. No default sink or existing stream was moved. Only PCM level metrics were saved, not raw audio or desktop audio. A one-second paused control contained no nonzero samples.

| Control | Post-seek media progress | Readiness/status | Isolated PCM after seek |
| --- | ---: | --- | --- |
| Direct v4 | 5.195 s | readyState 4, playing | 29/29 selected windows nonzero, RMS about 1638 |
| Pause/seek/resume v1, from active source | 5.005 s | readyState 4, playing | 19/19 selected windows nonzero, RMS about 1638 |
| Direct v5 | 5.317 s | readyState 2, buffering throughout | 29/29 selected windows nonzero, RMS about 1638 |

Direct v5 reproduces the readiness/UI buffering symptom while the private sink continues carrying output at the same level as before the seek (24/24 selected pre-seek windows nonzero, RMS about1638). Thus readyState2 alone is not evidence of silent output for this observed path. It does not establish that all prior stalls were false positives. The paused comparison passed once; no causal workaround or production decoder change is claimed.

These are three exploratory correctness/output controls, not startup/seek latency benchmarks or a p95. Recorder and driver have separate wall clocks within the same host; first-data delays were52–99ms and device latency was not measured. Correlation windows exclude the immediate seek boundary. Constant-tone output presence does not prove the acoustic seek position or physical audibility. Native Rust mixer consumption remains unqualified. AAC/Opus controls and repeated matched recordings remain open.

Earlier attempts remain retained: capture guard/schema errors (module JSON lacks indexes; short output has multiline unrelated arguments), an over-short play receipt deadline, a playing target tolerance that missed48.752 by0.002s, and an inappropriate diagnostic rejection of buffering itself. Those attempts do not count as playback acceptance or silent-audio results. The corrected diagnostic allows observing buffering while retaining exact source/occurrence/seek ownership, error checks, bounded plausible landing and five-second progress. Production behavior is unchanged.

Evidence: `.cache/qualification-local-codecs/{direct-seek-v4-private.json,direct-output-v4-private.json,paused-seek-v1-private.json,paused-seek-output-v1-private.json,direct-seek-v5-private.json,direct-output-v5-private.json,paused-output-control-v2-private.json}`; safe correlation summaries and pinned inputs under `.cache/qualification-seek-output-current/`. All protocols confirmed pause/volume cleanup. The isolated app was stopped and its exact temporary sink removed before normal-profile restoration.
