# Small real-audio BPM and key validation set

## Purpose and limits

Use a small, reproducible set of real recordings to check Burrata's BPM/key path after the existing synthetic triad tests. This set is a smoke/behavior check, not a population benchmark: the source is user-uploaded loops rather than released songs, two annotators agreeing does not make a label infallible, and ten clips cannot estimate genre-wide accuracy. The Freesound Loop Dataset (FSLD) has 9,455 loops and tempo, key, genre, and instrumentation annotations. Its authors describe a mixture of user-supplied BPM and algorithmic key metadata plus human annotation; the per-annotator records let this small subset be restricted to clips where two human annotation files agree on BPM and key/mode. [Dataset record and data layout](https://zenodo.org/records/3967852), [Ramires et al., ISMIR 2020](https://repositori-api.upf.edu/api/core/bitstreams/01adb695-2967-4ba6-9570-377d65f0fc8c/content).

This set validates the application's estimates against source annotations for these specific recordings. It does not establish universal keys for ambiguous or changing-harmony music, resolve half/double-time conventions, or claim acoustic verification by a musicologist. A human score/performance may use another key spelling or perceive the beat at half/double tempo. Keep those results visible rather than silently treating every disagreement as a model defect.

## Frozen candidate manifest

Labels below were read from the two FSLD per-annotator JSON records `annotations/5/sound-<ID>.json` and `annotations/10/sound-<ID>.json` in the 1.6 MB `annotations.zip`. Both records mark each item `discard: false`, mark tempo as defined, and agree on the listed BPM, key and mode. Genre words are the annotators' tags, and may differ. Freesound pages were checked for the displayed title, uploader and item-level license; all selected licenses below permit redistribution with attribution or waive copyright, unlike several otherwise useful candidates carrying NonCommercial terms.

| Freesound ID | Recording title / uploader | Agreed labels (BPM; key) | Annotation genre tags (combined) | Item license |
|---|---|---|---|---|
| [200873](https://freesound.org/s/200873/) | `hiphop-23.wav` / LoopPacks | 92; D minor | hip hop, electronic, live sounds | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| [188785](https://freesound.org/s/188785/) | `distant bass.wav` / tim.kahn | 132; D major | bass music, live sounds, house / techno | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| [4604](https://freesound.org/s/4604/) | `convoluted bare bone loop 60 bpm 158.wav` / Jovica | 60; C-sharp minor | bass music, global, electronic | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| [193754](https://freesound.org/s/193754/) | `Oblong - DOT STEM.wav` / 24thelement | 140; C major | electronic | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| [390879](https://freesound.org/s/390879/) | `Inca Spa loop 01.wav` / CarlosCarty | 80; C major | cinematic, global | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| [424396](https://freesound.org/s/424396/) | `a c poppy tune fizzy lead 104bpm-08.wav` / @realdavidfloat | 104; C minor | global, electronic, house / techno, other dance music | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) |
| [429065](https://freesound.org/s/429065/) | `DIZI FLUTE LOOP 18 120 BPM.wav` / CarlosCarty | 120; C major | global, live sounds | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| [474984](https://freesound.org/s/474984/) | `Native American Flute Canyon Loopable Cm 120 bpm` / CarlosCarty | 120; C minor | global | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| [157779](https://freesound.org/s/157779/) | `bass looper 102.wav` / Snapper4298 | 102; C minor | bass music, electronic, house / techno, other dance music | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) |
| [242555](https://freesound.org/s/242555/) | `Psy Trance Loop 140 Bpm 06.wav` / zgump | 140; B minor | house / techno, other dance music | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) |

The table is the ten-item corpus; do not replace items based on Burrata's predictions. The annotation archive, DOI `10.5281/zenodo.3967852`, is the frozen label source. The per-item Freesound pages are the provenance and license sources. Preserve each source page, title, uploader, license URI, dataset version, and the corresponding two raw annotation JSON records in any local test manifest. Do not copy the 8.8 GB complete FSLD audio archive into the repository.

## Acquisition and fixture handling

1. Download only `annotations.zip` from the [FSLD Zenodo record](https://zenodo.org/records/3967852) and verify its published MD5 (`3920ee437802cf047a990b2968fa066c`). For each ID, retain the exact two JSON annotation records cited above. This archive is 1.6 MB; the audio archive is 8.8 GB.
2. Fetch the ten individual audio items from their Freesound pages or through Freesound's authenticated API. The audio is legally available under the per-item CC licenses in the manifest; comply with each license's attribution terms if audio is copied or redistributed. Freesound's API documentation describes item metadata and authenticated access: [API resources](https://freesound.org/docs/api/resources_apiv2.html). Keep any API token out of scripts, logs, and the repository.
3. Store downloaded audio outside version control by default. Record the downloaded file's SHA-256, sample rate, channel count and duration locally. If the project later needs checked-in samples, retain the page URL, uploader, title, license name and URI alongside every clip; obtain a separate review of attribution and repository distribution before adding binaries.
4. Before an evaluation run, verify that every local file's ID matches the manifest, can be decoded by the same production analyzer path, and is not empty/corrupt. Never infer an ID or label from an analyzer output.

## Evaluation

Run each item through the production audio-analysis path, recording the raw estimate, any confidence/quality indicators, and the exact input file hash. Compare BPM against the agreed BPM and report both strict error and musically tolerant alternatives:

- exact BPM match;
- within 1 BPM;
- octave-equivalent tempo match (`estimate`, `2 × estimate`, or `estimate / 2` within 1 BPM of the annotation), reported separately from strict match.

Compare key by pitch class and mode separately; normalize enharmonic spellings (for example, C-sharp and D-flat) before scoring. Report per-item outcomes as well as aggregate counts for this ten-item sample. Do not fold half/double-time tolerance into the strict score or report a single percentage as general accuracy. A key or tempo mismatch is evidence to inspect against the audio and annotations; two agreeing annotations are useful ground truth for a smoke test, not an absolute adjudication.

Run after the synthetic tests and keep the outputs as a dated local or CI artifact that names the code revision, dataset version, manifest hash and all ten file hashes. If an item is unavailable or fails decoding, mark it missing and its reason; do not swap in a model-favorable replacement without a new manifest version.

## Dataset alternatives considered

The [FMA Key and Mode Annotations](https://zenodo.org/records/10719860) project has expert song-level key/mode labels, but its audio is distributed in multi-gigabyte FMA archives and it does not provide corresponding human BPM labels. FMA's Spotify/Echonest tempo field is an algorithmic feature, not suitable as tempo ground truth. The [GiantSteps Tempo/Key dataset](https://github.com/GiantSteps/giantsteps-tempo-dataset) supplies tempo/key annotations for Beatport electronic previews, but its repository offers scripts to download commercial previews rather than openly licensed audio. FSLD is therefore the practical small joint-label source for a redistributable test subset, subject to item-level license tracking.


## Current execution status

Earlier direct Freesound/preview retrieval attempts returned HTTP 403, so the first manifest run had no usable audio and evaluated zero clips. A later local extraction of the ten selected items from the FSLD archive was verified against archive members and supplied the inputs for the production-path run with packaged FFmpeg. The frozen set was not changed based on analyzer predictions. The resulting dated baseline is [archived here](bpm-key-real-music-validation-results-2026-10-21.md), with a machine-readable summary and per-item audio hashes in [the JSON evidence](bpm-key-real-music-validation-results-2026-10-21.json).

The run decoded all ten clips. BPM produced an estimate for five and abstained for five; one of ten estimates was within 1 BPM of the agreed annotation (one of five estimates). Half/double-time equivalence also matches one of ten. This small smoke set is not a population accuracy claim. The BPM result format does not identify the internal abstention gate for each item; the 2.0-second clip is below the estimator's minimum periodic-coverage length, while the other abstentions need diagnostic evidence before attributing a cause.

Key produced no estimate for any of the ten clips. Each clip is under 40 seconds, while the current policy requires five distributed excerpts of at least eight seconds each. This is expected short-input abstention, not a decode/runtime failure, and means this corpus does not measure key accuracy. Do not describe these rows as ten wrong keys or lower the minimum excerpt length solely to make this set return labels.

The archive evidence confirms the selected item IDs, titles, uploaders, and hashes, but its embedded license metadata differs from the frozen source-page manifest for five items (IDs 188785, 4604, 390879, 429065, and 474984). Preserve both observations and reconcile them before making an audio-redistribution claim. No audio files were added to the repository as part of this qualification.

The tempo algorithm generation is now `burrata-media-analysis-v1-keyfinder-2.2.8-bpm-v3`. On the same frozen ten audio hashes, v3 improved the within-1-BPM count from 1/10 to 3/10 while retaining five estimates and five abstentions; see the [bounded comparison](bpm-key-real-music-validation-results-2026-10-21.md). A separate, preselected duration-qualified subset contains three clips of 48–80 seconds: all BPM estimates miss strict tolerance (two are octave-equivalent), and no keys are returned. Those longer clips pass the minimum duration gate, but existing corpus output does not include profile/consensus diagnostics to explain their key abstentions. This is a small paired algorithm check plus a three-item held-out smoke set, not a population accuracy result. Existing populated estimates remain intact until explicit recalculation; startup continues to fill missing nonmanual fields only. Manual values, including intentionally blank fields, remain protected. The v3 production app binary SHA-256 is `ed0564326b3724f7a14e4f971bc0ccb7a19ddb13320d0bc7488aad5ca7424755`; its corpus analyzer SHA-256 is `9786aa5abe65293d4c06ab4127c8e8f4532c1381822662ca460f7e488c04d4ea`.
