# Key-estimation abstention diagnostics

This note explains why the three clips in the separate duration-qualified FSL10K subset produced no key estimate. It is a narrow diagnostic over those three preselected clips, not a general key-accuracy result.

The production route is in `src-tauri/src/media.rs`: FFmpeg decodes the file to mono 22,050 Hz `f32le`, and `estimate_key` calls the same `burrata_keyfinder_analyze` C ABI for the whole signal and five centered windows. The C++ wrapper in `src-tauri/keyfinder-native/keyfinder_ffi.cpp` returns LibKeyFinder's key enum and computes the best cosine-profile fit and its margin over the runner-up across 24 major/minor profiles. Current acceptance gates are fit >= 0.20 and margin >= 0.01 for the whole buffer and each window, at least three qualified windows, and agreement between the winning window key and the whole-track key.

All three whole-track fits are high (.7047–.8883), while all three margins fall below .01, so the first production failure is `whole_profile_margin`. The remaining measurements explain what would happen later in the pipeline:

- ID 78694 is annotated F major, but the whole classifier returns Am with margin .003715. Three qualified windows vote A, which disagrees with the whole Am key. The raw whole candidate is wrong for this clip. Lowering the whole margin threshold alone would still not make the unchanged production route accept it because whole/window consensus fails; weakening both confidence and consensus safeguards risks surfacing a wrong label. The abstention is useful here.
- ID 315848 is annotated D minor and all five windows classify Dm, but only one window clears the .01 margin. Whole-track margin is .007881, so the current result is still an abstention despite the label-consistent classifications.
- ID 418630 is annotated F# major (enharmonic to Gb major); the whole classifier returns Gb, the label-equivalent result, but with margin .001313. Only two windows qualify, and both call Gbm, disagreeing with the whole-track Gb. The current abstention reflects both weak whole-track separation and unstable section results.

The complete raw fit/margin values, exact window positions, first failing gates, input hashes, code hashes and native archive hashes are in [qualification.json](qualification.json).

## Reproduce the diagnostic

`scripts/qualification/keyfinder_diagnostic.cpp` reads a mono 22,050 Hz little-endian f32le file and calls the existing ABI. It requires a built Burrata FFI archive; it does not rebuild native code or modify application behavior. For example:

```sh
KEYFINDER_LIB_DIR=/path/to/cargo-out/lib
FFMPEG=/path/to/bundled/ffmpeg
RAW=/tmp/key.f32
WAV=/path/to/selected.wav

g++ -O2 -std=c++11 scripts/qualification/keyfinder_diagnostic.cpp \
  -o /tmp/keyfinder_diagnostic -L"$KEYFINDER_LIB_DIR" \
  -lburrata_keyfinder_ffi -lkeyfinder -lfftw3 -lstdc++ -lm
"$FFMPEG" -nostdin -v error -i "$WAV" -map 0:a:0 -vn -ac 1 -ar 22050 \
  -f f32le -y "$RAW"
/tmp/keyfinder_diagnostic "$RAW"
```

The decoder invocation matches the production `detect_key` arguments. The harness reports all windows even when the production function would already return at the whole-profile margin gate; its `first_production_failure` field preserves production's actual early-return order. It is diagnostic-only and writes no tags or app state. The exact binary and source hashes used for this run are recorded in the JSON.

The selected audio comes from the preselected FSL10K manifest at `.cache/bpm-key-real-corpus/fsl10k-duration-qualified-heldout/selection-manifest.json`; its archived item hashes and annotations are repeated in the evidence record. Attribution and archived licensing details remain in that selection manifest.
