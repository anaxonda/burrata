# Stage 5 isolated matching runs, 2026-10-22

These sanitized logs retain evaluator rows and final counts, without Cargo build
warnings or environment-specific command lines. Fixture IDs and synthetic
carrier labels are preserved in each row.

- v2: 12 held-out cells and 6 policy-probe cells, zero differences.
- v3 initial run: 30 held-out cells with 2 Flexible radio-edit expectation
differences; preserved separately at
  [the initial v3 output](../2026-10-22-stage5-heldout-v3/v3-initial-evaluator-output.log).
- v4: 30 held-out cells and 6 policy-probe cells, zero differences after the
  documented radio-edit policy expectations were corrected. This is a
  policy-corrected rerun after observing v3's differences, not a first-pass
  held-out result.
- Existing stage 5 policy matrix: 84 cells and 3 metamorphic checks, zero
  differences.

The v2/v4 carriers are synthetic metadata derived from cited public catalog
records; these results do not verify provider retrieval, audio identity, or
population accuracy. The evaluator artifact was
`.cache/verification-target/debug/burrata-heldout-matching`, SHA-256
`d9e63768da9e527810647a260f0ce2c12d8e133832038fc0fe51a1271d004316`. Its
runner source SHA-256 was
`8e6376c12f297eec4f8fff90fa75efb40be92bdc808a379548edcee0dc2c858f`.
The v2 fixture SHA-256 is
`f5ca32fa30cb541b5b1e3771a0c39251836b544a8f9829540738b7bba0876154`; the v4
fixture SHA-256 is
`dda3b1e4ff9f4c0f9e4d60fee36409655a2c4e92f1c369de752b5d0ac49db87b`.
