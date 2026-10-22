# Stage 5 held-out catalog identity check

Current fixture: [`recording-match-stage5-heldout-catalog-v2.json`](fixtures/recording-match-stage5-heldout-catalog-v2.json). The original [`v1 fixture`](fixtures/recording-match-stage5-heldout-catalog-v1.json) is retained with its initial expectations and was not rewritten after its first evaluator run.

The captured v1 output is retained in [`recording-match-stage5-heldout-v1-initial-run.txt`](fixtures/recording-match-stage5-heldout-v1-initial-run.log). The v1 run produced 12/12 matching outcomes for the two source-backed cases. Its six-cell synthetic missing-identifier probe differed: the evaluator returned `Exact/Inspect` at Search and `Exact/Accept` at Resolved with `metadataOnly` proof. We had incorrectly labeled absent identifier evidence as requiring unresolved/enrichment, overlooking the documented Exact policy that permits matching duration as a metadata anchor. Metadata-only proof does not itself require enrichment. This is preserved as the v1 result, not retroactively counted as a pass.

In v2, the independently sourced positive and negative remain the held-out set, with six outcomes per case. The synthetic missing-identifier example is separated under `policyProbes` and its expectation is revised to the documented contract. Runner summaries report held-out cases and policy probes separately. This is a contract check, not an estimate of population accuracy.

The source rows are MusicBrainz public recording-series metadata: [Indie 88](https://musicbrainz.org/series/33061f6b-6553-4d61-851b-71b790bf1460) lists Seven Nation Army — The White Stripes — USVT10300001 — 3:52; [Rolling Stone 500](https://musicbrainz.org/series/355b26c9-001e-4728-852e-82b4379adb82) lists Smells Like Teen Spirit — Nirvana — USGF19942501 — 5:01 and separately lists Seven Nation Army. Both request/candidate wrappers are synthetic fixture carriers populated from these catalog facts; they are not observed Burrata provider items. The shared ISRC supports `recordingIdentifier` proof for the positive case; the conflicting and missing identifiers target `metadataOnly`. No case claims `audioFingerprint`, an audio-source binding, or decoded audio equivalence.

Run the structural check:

```sh
python3 scripts/validate_stage5_heldout.py
```

The production-module runner is ready for the separately approved Cargo run:

```sh
cargo run --manifest-path scripts/heldout-matching/Cargo.toml
```

The current held-out sample has one positive and one conflict pair. It cannot establish precision, recall, or general accuracy. We could not independently label close same-title/different-performer, guest-credit, edit/remix, or source-bound audio cases from the checked primary metadata.
