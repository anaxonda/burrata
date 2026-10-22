# Real-audio BPM/key baseline — 2026-10-21

This report records the first production-path run over the frozen ten-item Freesound Loop Dataset (FSLD) subset. It is a smoke check on these clips, not a genre/population benchmark. The exact per-item result, source-page and archive license observations, and full audio SHA-256 values are in [the sanitized JSON evidence](bpm-key-real-music-validation-results-2026-10-21.json). No audio is included in the repository.

## Initial v2 baseline run and aggregate result

The run used production `media::detect_bpm` / `media::detect_key` with the packaged FFmpeg binary (SHA-256 `e17253d50522cf5f9b52aabd9faba42cac164c1682a61029b53c957c6f2b1f33`) and analysis generation `burrata-media-analysis-v1-keyfinder-2.2.8-bpm-v2`. The frozen FSLD annotation archive MD5 is `3920ee437802cf047a990b2968fa066c`.

| Measure | Result |
|---|---:|
| Clips decoded | 10/10 |
| BPM estimates | 5/10 |
| BPM abstentions | 5/10 |
| BPM within 1 BPM | 1/10 (1/5 of estimates) |
| Octave-equivalent within 1 BPM | 1/10 |
| Key estimates | 0/10 |
| Key abstentions | 10/10 |

Only clip 424396 produced a BPM within 1 BPM of its agreed label (103.8 vs 104). Four other returned estimates were far from their labels. For clip 157779, the reported related tempo 68.1 and primary tempo 136.2 both miss the 102 BPM label; for 242555, 187.2 and its related 93.6 miss 140 BPM. The reported BPM confidence is normalized onset-autocorrelation strength, not a probability.

## Per-item observations

`—` means the analyzer abstained; it does not mean a measured error of zero. The JSON evidence contains the full clip hashes and exact source details.

| FSLD ID | Duration | Expected BPM | BPM estimate | Expected key | Key estimate |
|---|---:|---:|---:|---|---|
| 200873 | 20.872 s | 92 | — | D minor | — |
| 188785 | 7.239 s | 132 | 194.4 | D major | — |
| 4604 | 2.001 s | 60 | — | C-sharp minor | — |
| 193754 | 13.707 s | 140 | 187.6 | C major | — |
| 390879 | 24.000 s | 80 | — | C major | — |
| 424396 | 18.462 s | 104 | 103.8 | C minor | — |
| 429065 | 18.625 s | 120 | — | C major | — |
| 474984 | 20.000 s | 120 | — | C minor | — |
| 157779 | 18.825 s | 102 | 136.2 (related 68.1) | C minor | — |
| 242555 | 8.562 s | 140 | 187.2 (related 93.6) | B minor | — |

## Interpreting abstentions and limits

The key analyzer requires five distributed windows, each at least eight seconds long. Because these clips are all shorter than 40 seconds, none can pass that window-length guard. Two clips are also shorter than the analyzer's eight-second whole-input minimum. The zero key estimates therefore indicate that this corpus did not exercise key accuracy; they are not evidence of decode or keyfinder infrastructure failure. A future short-clip policy needs its own validated excerpt length and agreement rule, rather than simply relaxing the existing guards.

The BPM analyzer builds a 10 ms positive-RMS-change envelope and evaluates periodic autocorrelation. Its current minimum-period-coverage guard rejects the 2.001-second clip. Other no-estimate results are recorded as abstentions, but this result format does not expose which confidence/periodicity guard rejected each one. Keep the abstention count visible; do not treat only returned estimates as a full-coverage accuracy result.

The sample is too small to justify threshold or weight tuning. A general improvement should expose candidate tempo evidence and rejection reasons, test a richer onset representation against a larger fixed, duration/genre-stratified set, and preserve strict and half/double-time scores separately. Keep a held-out subset for assessment and retain every abstention and mismatch.

## Provenance and license observations

The run's audio hashes match the selected items recorded in the local FSLD archive evidence. Five archive metadata license URIs differ from the frozen Freesound-page license in the source manifest: IDs 188785, 4604, 390879, 429065, and 474984. For each, the page manifest records CC BY 4.0 while the archived metadata records CC BY 3.0. The evidence preserves both values; resolve that provenance discrepancy before making a redistribution claim. This report contains no audio binaries.

The packaged-FFmpeg result JSON had SHA-256 `445514579ed949fd53baa2f8693bc0b998dc1e2a73f535aebab37572447b4957`; archive evidence and extracted-audio manifest hashes are recorded in the JSON report. The earlier direct-download attempt that evaluated zero clips is superseded by this verified local-archive run.

## Post-fix v3 run on the same ten clips

The v3 run used the same ten audio SHA-256 inputs as the v2 baseline and the structural faster-peak guard: a faster local autocorrelation peak is eligible only when its period is an approximately integral 2× or 3× harmonic of the conventional period within the lag quantization bound. This constrains the relationship between candidates; it does not tune score thresholds to these clips. The production app binary used for the run has SHA-256 `ed0564326b3724f7a14e4f971bc0ccb7a19ddb13320d0bc7488aad5ca7424755`; the corpus analyzer executable has SHA-256 `9786aa5abe65293d4c06ab4127c8e8f4532c1381822662ca460f7e488c04d4ea`.

| Measure | v2 baseline | v3 same inputs |
|---|---:|---:|
| BPM estimates | 5/10 | 5/10 |
| BPM within 1 BPM | 1/10 | 3/10 |
| Octave-equivalent within 1 BPM | 1/10 | 3/10 |
| Key estimates | 0/10 | 0/10 |

The three v3 BPM estimates within 1 BPM were 193754 (139.8 vs 140), 424396 (103.8 vs 104), and 242555 (139.7 vs 140). For the same input hashes, 193754 and 242555 changed from approximately 187 BPM to the annotated tempo. 188785 moved from 194.4 to 150.1 BPM but remains wrong by 18.1 BPM; 157779 remains 136.2 BPM against 102. Coverage stayed at five estimates and five abstentions. This paired ten-clip result is evidence that the structural guard addresses these observed substitutions; it does not establish general accuracy.

The machine-readable per-item comparison is [bpm-key-real-music-v3-results-2026-10-21.json](bpm-key-real-music-v3-results-2026-10-21.json). It includes all ten input hashes and the raw v3 result artifact hash.

## Separate duration-qualified held-out subset

A separate three-item subset was selected before analyzer results using the recorded criteria: both FSLD annotators agree on nonempty BPM/key/mode, the archive license is CC BY or CC0, and decoded duration is 40–120 seconds. The selector screened 327 paired annotation IDs, found 56 items meeting annotation/license requirements, then found all three duration-qualified items; all three were included. The subset is separate from the original ten and its selection manifest remains frozen.

| FSLD ID | Duration | Expected BPM | BPM estimate | BPM result | Expected key | Key estimate |
|---|---:|---:|---:|---|---|---|
| 78694 | 80.000 s | 60 | 120 (related 60.1) | octave-equivalent, not strict | F major | — |
| 315848 | 48.000 s | 120 | 60 | octave-equivalent, not strict | D minor | — |
| 418630 | 56.250 s | 120 | 80 | neither strict nor octave-equivalent | F-sharp major | — |

All three BPM estimates miss strict ±1 BPM; two are octave-equivalent. No key was returned even though all three clips meet the 40-second length implied by the current five-window policy. This means duration alone does not explain these key abstentions. The corpus output has no profile-fit, profile-margin, or window-consensus diagnostics, so these results do not identify the rejection gate or justify calling it an infrastructure failure. Expose those diagnostics before drawing that conclusion.

See [the separate held-out evidence](bpm-key-duration-qualified-heldout-2026-10-21.json) for full clip and two-annotator record hashes, attribution, archive license, selection criteria, and result hashes. All selected archive-member CRCs are verified, but the 8.84 GB archive's full MD5 was not verified; the full archive was not downloaded. The report contains no audio files.

Keep the original ten and the duration-qualified subset separate in every score: the latter was chosen to exercise key eligibility, not to tune the algorithm, and only three items met its frozen criteria. Neither run supports a population-level accuracy claim.
