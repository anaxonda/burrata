# Track matching stage 5: corpus and measurement audit

This is an audit and harness contract for stage 5 of
[`track-matching-improvement-plan.txt`](track-matching-improvement-plan.txt).
It adds a separate policy-contract matrix and a measurement summarizer alongside
the now-updated 18-case Resolved corpus. The stage5 work does not register its
matrix in `lib.rs` or claim measured accuracy or latency.

## What the existing corpus establishes

`recording-match-corpus.json` contains 18 Resolved-stage examples. Its current
top-level labels count as follows:

| Current label | Cases | What the record supports |
|---|---:|---|
| `sourceBackedRecordingFixture` | 8 | Public/provider-shaped metadata examples, with varying amounts of source evidence. This label does not itself prove audio identity. |
| `hardNegativeFixture` | 6 | A mix of synthetic policy contrasts and public-shaped candidates with an expected rejection. Several are not independently verified source recordings. |
| `needsEvidence` | 2 | The corpus explicitly avoids promoting the candidate to a verified recording. |
| `sourceBackedAlternativeFixture` | 1 | A documented Live contrast, but its evidence text explicitly calls the alternative fixture synthetic. |
| `unresolvedAnnotationFixture` | 1 | An unrequested annotation lacks independent evidence and is expected to remain unresolved and request enrichment. |

Every expected case has Exact, Balanced, and Flexible relationship/action values,
but the wired test evaluates **Resolved only**. The fixture has no Search-stage
expectations or structured proof-strength field. It is a regression suite, not a
sample from a labeled population, so its counts cannot estimate precision,
recall, or per-provider accuracy.

The labels deserve these qualifications before they are used as ground truth:

| Case(s) | Audit finding |
|---|---|
| `everybody-wants-cowboy-primary-credits`, `baba-ralmm-emil-extended-294`, `nighttime-santtana-lindsay`, `chart-mike-dunn-dj-tai-494`, `masaki-untitled-dub-347`, `larry-labirt-corey-holmes-330` | The records cite public provider items/captions and metadata. They support source-shaped regression inputs; none of those citations alone verifies decoded audio or a recording-to-source binding. The last three mark candidate permission `unknown`. |
| `diaraby-ry-cooder-446826` | The strongest identity evidence in this corpus: a Monochrome recording identifier, track ID, ISRC, and credits are carried. This supports catalog identity evidence; source playback and audible equivalence remain separate questions. |
| `velib-bleu-sean-original-203` | The evidence describes a public catalog regression, but its candidate URL is `example.invalid` and candidate ID is fixture-named. Treat it as provider-shaped/synthetic until an independently reviewable source is attached. |
| `miroloi-tis-manas-lakis-chalkias-ensemble-127626` | Correctly labeled `needsEvidence`; the candidate URL is `example.invalid`, and the evidence text says the likely Topic source lacks a verified binding. |
| `cowboy-wrong-guest`, `diaraby-composer-only-wrong-orchestra`, `conflicting-isrc-despite-caption`, `larry-wrong-cover`, `cowboy-preview` | Policy contrasts. Their names and metadata do not show that a live provider returned these exact objects. Keep their synthetic status explicit. The `conflicting-id` Spotify URL is not, by itself, proof of a real item. |
| `nighttime-artist-overlap-remix` | A public Spotify-shaped different remix is documented. It is useful as a close negative; it does not establish a general rate of false matches. |
| `cowboy-live-flexible-alternative` | The evidence says “synthetic hard negative/alternative,” while the corpus category says `sourceBackedAlternativeFixture`. Keep it classified as a synthetic policy probe. |
| `larry-wrong-remix` | Reclassified as `unresolvedAnnotationFixture`: the `[Digital Generation]` annotation has no corresponding request label or independent label evidence. All Resolved levels now expect `Unresolved/Enrich`. |

Case-by-case disposition:

| Case ID | Current label | Evidence class to use in analysis |
|---|---|---|
| `matthew-john-denver` | `needsEvidence` | Public YouTube ID/caption; duration differs by 12 seconds; no source binding. Preserve as unresolved evidence, not as a confirmed wrong match. |
| `everybody-wants-cowboy-primary-credits` | `sourceBackedRecordingFixture` | Public YouTube ID/caption plus public request identity; metadata regression only, no audio proof. |
| `cowboy-wrong-guest` | `hardNegativeFixture` | Synthetic `example.invalid` contrast. |
| `cowboy-live-flexible-alternative` | `sourceBackedAlternativeFixture` | Synthetic Live-caption policy variation over a cited public ID; the source citation does not support the edited alternative claim. |
| `nighttime-santtana-lindsay` | `sourceBackedRecordingFixture` | Public Spotify catalog metadata for matching artist/title/runtime; no audio binding is asserted. |
| `nighttime-artist-overlap-remix` | `hardNegativeFixture` | Public Spotify catalog item with a different remix/performer credit; useful close negative, not a population sample. |
| `baba-ralmm-emil-extended-294` | `sourceBackedRecordingFixture` | Public Topic result/caption and runtime; metadata evidence only. |
| `chart-mike-dunn-dj-tai-494` | `sourceBackedRecordingFixture` | Public YouTube caption/runtime paired with Traxsource request; candidate permission is `unknown`. |
| `masaki-untitled-dub-347` | `sourceBackedRecordingFixture` | Public YouTube caption/runtime paired with Traxsource request; candidate permission is `unknown`. |
| `larry-labirt-corey-holmes-330` | `sourceBackedRecordingFixture` | Public YouTube caption/runtime paired with Traxsource request; candidate permission is `unknown`. |
| `velib-bleu-sean-original-203` | `sourceBackedRecordingFixture` | Synthetic placeholder candidate (`example.invalid`, fixture ID); do not use as public source observation without replacement evidence. |
| `miroloi-tis-manas-lakis-chalkias-ensemble-127626` | `needsEvidence` | Reported likely Topic variant with `example.invalid` candidate; unresolved and correctly not accepted. |
| `diaraby-ry-cooder-446826` | `sourceBackedRecordingFixture` | Strong catalog identity fields (Monochrome track ID/ISRC and complete credits); not proof of audio transport or decoded source. |
| `diaraby-composer-only-wrong-orchestra` | `hardNegativeFixture` | Synthetic placeholder YouTube item; composer/title contrast only. |
| `conflicting-isrc-despite-caption` | `hardNegativeFixture` | Synthetic provider-shaped identifier conflict; URL uses a placeholder ID and should not be counted as a live provider result. |
| `larry-wrong-cover` | `hardNegativeFixture` | Synthetic placeholder cover caption. |
| `cowboy-preview` | `hardNegativeFixture` | Synthetic placeholder preview, explicitly useful for preview policy only. |
| `larry-wrong-remix` | `unresolvedAnnotationFixture` | Synthetic placeholder remix with an unverified `[Digital Generation]` annotation; `Unresolved/Enrich` across modes. |

The former `/tmp/burrata-matching-audit` results and resolved-only reproducer
have been preserved at
[`results.txt`](performance-results/2026-10-21-stage5-matching-audit/results.txt),
[`isolated-test-output.txt`](performance-results/2026-10-21-stage5-matching-audit/isolated-test-output.txt),
and [`legacy-resolved-reproducer.rs`](../scripts/matching-audit/legacy-resolved-reproducer.rs).
The retained results are a historical implementation audit snapshot, not a new
source of labels. They record these
behaviors across all three modes at Resolved: `[432Hz]` and `[Slowed + Reverb]`
were accepted as Exact; a Live caption could be hidden by clean structured title
fields; an internally conflicting Live/Original Mix request was accepted at
Balanced/Flexible; punctuation-only titles compared as Exact; Unicode `İ feat. Å`
panicked; a nested request ISRC conflict was accepted; and Balanced acceptance
was not cumulative into Flexible for the bound-catalog/missing-runtime example.
The output also records that an Alice Remix example had a legacy score of
`0.49/low` despite a Flexible Alternative/Accept assessment. Under the reviewed
policy table that is only a Flexible Search `Alternative/Inspect` and Resolved
`Alternative/Accept` when the candidate identity is actually bound; conflicting
IDs resolve to Reject. These are evaluator
reproductions, not claims that those titles were wrongly played from a live
provider. The accompanying `main.rs` shows the fixture construction.

## Independent matrix

[`recording-match-stage5-matrix-v1.json`](../src-tauri/tests/fixtures/recording-match-stage5-matrix-v1.json)
is a new contract fixture, deliberately separate from the existing corpus. It
has 14 cases and six expected cells per case: three modes at Search and three at
Resolved. Its labels describe evidence basis, candidate proof, uncertainty, and
whether the input is synthetic. Every metadata-only case says explicitly that it
does not prove acoustic identity. The matrix covers:

- metadata agreement without a recording binding, plus a bound catalog with
  missing runtime;
- unknown tuning, speed, and remaster annotations;
- clean structured title versus contradictory Live caption, and conflicting
  request title/version observations;
- Unicode guest parsing, punctuation-only title identity, and nested ISRC
  conflicts;
- an extra complete performer credit, an explicit preview, and a Flexible
  remix-alternative policy probe.

The matrix expresses the reviewed stage-1 policy table; it is not an account of
the current production evaluator's outputs. Each cell records proof strength
separately from relationship/action: all cases target `metadataOnly`. The
Monochrome candidate has a self-consistent catalog ID, but it is not cross-bound
to the request and therefore does not earn `recordingIdentifier`. Its synthetic
examples remain synthetic even if the matcher accepts them. Before wiring it into
Rust tests, confirm the expected proof-strength field against the production
enum, then add non-overlapping fixture registration with the owners of
`recording_match.rs` and `lib.rs`.

Validate coverage without Cargo:

```sh
python3 scripts/matching_stage5.py validate-matrix
python3 -m unittest scripts/test_matching_stage5.py
```

The standalone Rust package imports `matching.rs`, `models.rs`,
`recording_identity.rs`, and `recording_match.rs` by path, so it evaluates the
production matching modules without editing or registering a test in `lib.rs`.
Its direct dependencies match those imports (`serde`, `serde_json`,
`icu_normalizer`, `uuid`, and `url`). Run `cargo run --manifest-path
scripts/matching-audit/Cargo.toml --bin stage5-matrix` from the repository root to
evaluate the 84 target cells; it prints `PASS`/`DIFF` rows and returns nonzero
while a reviewed policy target differs. This is the isolated native-suite route
for stage5 matrix integration; wiring it into the app crate would require a
production `lib.rs` test registration, which is intentionally outside this
work. `legacy-resolved-reproducer` retains the original focused probes. Both
binaries read current production source, while preserved `.txt` outputs remain
the captured historical baseline. The standalone harness has not been run while
production matching files are changing.

## Cold/warm trace contract

`scripts/matching_stage5.py summarize <observations.jsonl>` summarizes
instrumented observations. The JSONL field contract is checked by the script and
its tests. Each row includes:

- a `pairId`, `caseId`, operation, mode, stage, and `condition` (`cold` or
  `warm`);
- the frozen build identity: git commit, artifact SHA-256, evaluator/evidence/
  policy versions, and platform;
- actual request identity (`provider`, provider ID, title, artist) and candidate
  identity (`provider`, stable source ID), without signed/transient URLs;
- success or failure and its failure stage, plus measured phase durations;
- cache hits, external requests, and extraction attempts.

Record `parseMs`, `comparisonMs`, `aliasLookupMs`, `providerSearchMs`,
`extractionMs`, `verificationMs`, `accessValidationMs`, `resolutionMs`, and
`firstAudioMs` when instrumented. Missing phases stay missing; failures remain in
the denominator and in any available phase samples. The summarizer emits p50 per condition and p95 only for phases with at least 20
observations, plus success/failure counts, request/extraction/cache counts, and
pairing issues. It reports `firstAudioMs` separately; resolver timing, media
start, and verified audible playback must not be presented as interchangeable
claims.

Cold and warm rows with a shared `pairId` must have identical case, request,
candidate, operation, mode, stage, and frozen build identity. A mismatch returns
status 2 and appears in the report. Do not compare different source identities,
policy versions, or builds as a cache-speedup pair. Retain failed attempts and
resource-use observations alongside successful runs; the current summarizer
does not infer a speedup or accuracy improvement.

## Practical qualification sequence

1. Once the implementation owners freeze parsed evidence/proof fields, reconcile
   this matrix's expected cells with those public enums and the six Search/Resolved
   combinations. Keep uncertain metadata as unresolved evidence.
2. The standalone matrix runner now exercises source-metadata artist-field
   relocation, composed/decomposed Unicode preservation, contradictory ISRC
   monotonicity, and cumulative eligibility over all cases. Add remaining
   annotation, malformed-input boundedness, and fresh/cache equivalence checks.
   Run against the same evaluator and
   provider-shaped adapters; keep synthetic property inputs labeled as such.
3. Classify every fixture as independently source-observed, source-bound,
   metadata-only, unresolved, or synthetic policy probe. Split training/tuning
   examples from a held-out set before interpreting precision/recall.
4. Only after tracing is available, collect matched cold/warm playback and
   download observations for frozen builds and fixed request/source identities.
   Report search, extraction, verification, resolver, and first-audio phases
   separately. Do not infer audible-start performance from resolver completion.
5. Publish remaining blind spots and failed cases with the results. Neither the
   old 18 examples nor this 14-case contract matrix is a representative accuracy
   sample.

## Expanded source-backed catalog set, v3

The separate [`v3 catalog fixture`](fixtures/recording-match-stage5-heldout-catalog-v3.json)
adds five primary-source-supported metadata cases and one synthetic missing-
duration policy mutation. It covers same-title/different-artist identity,
credited guests, an explicit live performance, a radio edit, a scoped artist
spelling alias, and a shared identifier with missing duration. Every carrier is
synthetic; this expands evaluator regression coverage but does not measure
provider retrieval, audio identity, or population precision/recall. See
[`v3 provenance and label notes`](track-matching-stage5-catalog-v3.md).

The first v3 evaluator run is retained at
[`v3-initial-evaluator-output.log`](performance-results/2026-10-22-stage5-heldout-v3/v3-initial-evaluator-output.log).
It evaluated 30 held-out cells and differed in two Flexible cells for the U2
radio edit. The fixture generator had generalized “identified alternative” to
all Flexible alternatives, while the plan's current eligibility rule names only
live, remix, and cover alternatives. This was a policy-label overgeneralization,
not an audio-identity finding. The source metadata continues to identify a
different catalog recording. No v3 label or result was overwritten.

The [`v4 catalog fixture`](fixtures/recording-match-stage5-heldout-catalog-v4.json)
keeps the same source metadata and ground-truth label: the U2 radio edit is a
distinct catalog edit/alternative recording. Its separate
`flexiblePolicyEligible=false` policy field records that the current written
policy does not admit radio edits, so expected actions remain
`Unresolved/Reject` at all levels. Expectations were corrected after observing
v3's two differences; v4 is a policy-corrected rerun, not a first-pass held-out
result. It still uses synthetic carriers and does not verify audio identity or
estimate population accuracy. The v3 fixture and its initial output remain
unchanged.
