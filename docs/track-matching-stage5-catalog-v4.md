# Stage 5 source-backed catalog regression set, v4

[`recording-match-stage5-heldout-catalog-v4.json`](fixtures/recording-match-stage5-heldout-catalog-v4.json)
retains the five source-backed metadata comparisons and the separate synthetic
missing-duration probe from v3. MusicBrainz sources and metadata are unchanged.
The radio edit retains its `identifiedAlternativeRecording` ground-truth label:
it is a distinct catalog recording and alternative edition. A separate
`flexiblePolicyEligible=false` field records that its policy expectation is now
`Unresolved/Reject` at all three levels because the written Flexible alternative
policy currently names live, remix, and cover recordings. The catalog
relationship does not assert audio identity.

This correction follows the first v3 evaluator run, which produced 30 held-out
cells with two Flexible radio-edit differences. The initial output is archived
at [`v3-initial-evaluator-output.log`](performance-results/2026-10-22-stage5-heldout-v3/v3-initial-evaluator-output.log).
The v3 fixture and that output are retained unchanged. V4 is explicitly a
policy-corrected rerun after observing the differences, not a clean first-pass
held-out result. All carriers are synthetic catalog metadata, not provider
items or decoded audio; the set makes no population accuracy claim.

Validate the frozen labels with:

```sh
python3 scripts/validate_stage5_catalog_v3.py
```

Evaluate the production matching modules with:

```sh
cargo run --manifest-path scripts/heldout-matching/Cargo.toml -- \
  docs/fixtures/recording-match-stage5-heldout-catalog-v4.json
```

Do not revise v4 expected cells to make a run pass. Report any differences as
policy or implementation questions.

## Evaluator outcome

The v4 run evaluated 30 held-out cells and 6 separate policy-probe cells with
zero differences. This is a policy-corrected rerun after v3's two Flexible
radio-edit expectation differences, not a first-pass held-out result. The
existing v2 run also had zero differences across 12 held-out and 6 probe cells;
the separate stage 5 policy matrix had zero differences across 84 cells and
three metamorphic checks. See the [archived outputs and artifact hashes](performance-results/2026-10-22-stage5-matching-final/README.md).

These results compare metadata policy expectations to the evaluator. They do
not verify decoded audio identity, source retrieval, or population accuracy.
