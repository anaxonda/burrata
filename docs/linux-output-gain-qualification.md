# Linux native output gain qualification

`scripts/performance/linux_output_gain_qualification.py` captures candidate output from a temporary PulseAudio null sink. It does not set a default sink or change any sink volume. The candidate controller receives `PULSE_SINK`, `BURRATA_QUAL_SINK`, `BURRATA_QUAL_CASE=off|normalized`, and `BURRATA_QUAL_EXPECTED_GAIN_DB`; it must select Off or Track mode and play the same synthetic test track to natural end for both runs. Supply `--expected-gain-db` using the gain shown by the product for the normalized run. The harness records stereo 48 kHz PCM and reports sample-aligned first audible sample/window, per-channel peaks, clipping, and time-windowed RMS deltas.

Example, once an immutable candidate and its controller are available:

```sh
python3 scripts/performance/linux_output_gain_qualification.py \
  --output-dir /tmp/burrata-output-gain-qualification \
  --expected-gain-db -6 \
  --controller /path/to/candidate-controller --candidate /path/to/immutable-candidate
```

The capture/report path can be exercised without an app using this synthetic-only controller:

```sh
python3 scripts/performance/linux_output_gain_qualification.py \
  --output-dir /tmp/burrata-synthetic-output-gain-pair --expected-gain-db -6 \
  --controller python3 scripts/performance/linux_output_gain_synthetic_controller.py
```

This controller builds two generated WAV files, with the normalized case scaled in the source samples before playback. A host run captured 2.36 seconds, aligned the two PCM recordings, and measured −5.99 dB on the first audible sample, −6.00 dB over the first 100 ms, and −6.00 dB across both channels overall. This validates full-duration loopback and report math only; it does not exercise product gain code.

The controller owns candidate-specific playback setup. It must use an identical fixture, start from the same source position, let output settle only through the candidate's own behavior, and return after natural end. Compare the two WAV files and JSON report. For a known constant −6 dB gain, non-silent left and right channel RMS should each differ by approximately −6.02 dB from the first audible windows onward. A late-only drop indicates a startup burst; changing left/right deltas or unexpected peaks indicate channel or headroom problems. A GUI or physical-speaker listening check is separate evidence.

For a Tauri candidate, start at the profile's saved zero volume, then run `scripts/performance/verify_native_gain_routing.py` after the candidate has created an audio stream. It checks the frozen executable hash, marked synthetic profile, exact PulseAudio server/sink/module, and candidate-descendant sink-input PID. Raise the fixture volume through the private UI only when the command reports `status: verified`.

On Linux, some Rodio/CPAL builds open ALSA directly and ignore `PULSE_SINK`. In that case, create an `ALSA_CONFIG_PATH` file inside the marked synthetic profile for the candidate process only. Set `pcm.!default` to the Pulse PCM plugin with the exact `PULSE_SERVER` and owned sink name, and set `ctl.!default` to that server. Do not edit system ALSA/Pulse configuration or set a default sink. `linux_alsa_pulse_route_smoke.py` verifies this adapter using a generated one-second WAV and `aplay`; the candidate route guard must then verify the real Burrata stream before raising its private UI volume.

## Candidate measurement, 2026-10-25

The immutable candidate `f8d0067d…` was played through the native engine from a synthetic-only profile. The selected C Major WAV was analyzed by the app itself at −10.86 LUFS and −0.42 dBTP. Track mode reported −7.1 dB (−7.14 dB by the stored policy calculation), with the default −18 LUFS target and −1 dBTP ceiling. In same-volume private null-sink captures, the normalized output measured −7.10 dB relative to Off across the aligned capture windows on both channels, with no clipped frames. At app volumes 0.36 and 0.64, normalized output changed by +5.02 dB, matching the expected +5.00 dB from the volume ratio while the reported normalization remained −7.1 dB.

To close the onset alignment gap, a second pair was captured as float32 while Burrata was paused. The mode was set and saved before starting each run, then the finite private-profile helper loaded exactly the generated C Major track into a new generation from position zero. The Off and Track runs had distinct generations (54 and 57); Track reported −7.14 dB from `ffmpegAnalysis` at load. Cross-correlation between the decoded outputs was 0.99981 with only a three-frame relative offset. Source-aligned gain was −7.143 dB over the first 20 ms, −7.143 dB over 50 ms, −7.140 dB over 100 ms, and −7.140 dB over 500 ms on both channels. No clipped frames appeared in the first 100 ms. This tests onset envelopes from the first decoded content and avoids treating a single sample near a zero crossing as meaningful.

The result qualifies the Linux native path to an owned null sink only. It does not qualify physical output, other operating systems, or WebView/Opus paths. See `docs/performance-results/2026-10-25-first-four/native-output-linux-qualification.json` for the candidate hash, capture details, route checks, exact synthetic measurement, and limits.

The harness creates one uniquely named `module-null-sink` and unloads only the module ID it created, including on failure. It routes the controller through `PULSE_SINK` so the system default remains unchanged. Review and clean the output directory manually after retaining any desired report.

`linux_output_gain_qualification_test.py` is an offline synthetic analyzer check: it generates distinct left/right tones at unity and −6 dB and verifies the analyzer's frame, peak, and RMS math. `linux_pulse_null_sink_smoke.py` validates the real PulseAudio-compatible null-sink route with a generated stereo WAV and `paplay`; it does not involve Burrata. The recorder requests 20 ms latency/10 ms process time and waits for at least the fixture duration plus 150 ms of monitor frames, bounded by five seconds, before stopping and flushing raw PCM to WAV. This replaced a fixed sleep that produced a 1.024 second recording. The corrected host run captured 2.4 seconds, including the 2-second fixture and pre/post silence, at the expected left/right peaks (0.31998/0.16998 versus 0.32/0.17). The temporary sink was removed and the original default sink remained selected. This qualifies the loopback infrastructure only, not native mixer behavior, candidate playback, or physical output. The initial unprivileged sandbox could not connect to the host audio server; the smoke check succeeded with host audio access.
