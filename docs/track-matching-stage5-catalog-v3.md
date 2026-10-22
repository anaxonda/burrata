# Stage 5 source-backed catalog regression set, v3

[`recording-match-stage5-heldout-catalog-v3.json`](fixtures/recording-match-stage5-heldout-catalog-v3.json)
adds five held-out metadata comparisons and one separately counted synthetic
mutation. Expected labels are frozen before the first v3 evaluator run. The
dataset broadens category coverage; it remains a selected, small regression set
and cannot estimate population accuracy.

All source records are MusicBrainz catalog pages. Their fact roles are:

| Source | Supports |
|---|---|
| [Crazy in Love work](https://musicbrainz.org/work/e526fb3d-f0e7-325e-a934-546a9e82b5ce) | The album version, its guest credit, the no-rap version without Jay-Z and its distinct ISRC/length, plus cataloged live and cover recordings. |
| [Rolling Stone recording series](https://musicbrainz.org/series/355b26c9-001e-4728-852e-82b4379adb82) | The Beyoncé feat. Jay-Z album version, ISRC USSM10305425, and 3:56 duration. |
| [Beyoncé aliases](https://musicbrainz.org/artist/859d0860-d480-4efd-970c-c05d5f1776b8/aliases) | `Beyonce` as an alias and catalog artist-credit spelling of the Beyoncé entity. |
| [Metallica’s …And Justice for All release](https://musicbrainz.org/release/da53e497-8c61-4d4a-a29b-a5b53d86ccb7) and [U2’s Achtung Baby release](https://musicbrainz.org/release/9b746946-202a-39b5-9964-9c578ad1c373) | Distinct One recordings with different performers/composer credits and lengths (Metallica 7:27; U2 4:36). |
| [Mary J. Blige & U2’s One single](https://musicbrainz.org/release/b81a4987-6cd5-4b97-b3c4-9f22f5cedf65) | `One (radio edit)`, the Mary J. Blige & U2 artist credit, 4:04 length, and its link to the One work. |
| [Beyoncé recording list](https://musicbrainz.org/artist/859d0860-d480-4efd-970c-c05d5f1776b8/recordings?page=6) | The 2012 Brooklyn Crazy in Love live recording, its distinct ISRC USJZ11200006, and 3:06 length. |

The five catalog cases cover a shared-ISRC spelling alias, same-title/different-
artist contradiction, a named-guest and ISRC conflict, a live-performance
alternative, and a radio-edit alternative with an additional catalog credit.
The spelling-alias case supplies a scoped alias fact validated against the
MusicBrainz artist profile to the evaluator; it tests comparison once evidence
is known, not alias discovery or its latency. The single policy probe removes
the candidate duration from the shared-ISRC positive. It is separated from the
held-out set because its carrier is a deliberate mutation, not a provider item.

Each candidate/request object is a fixture carrier populated from primary
catalog metadata. It does not claim that a provider returned the object, that
any audio was decoded or listened to, or that an arbitrary URL is bound to the
catalog recording. A matching ISRC supports catalog identity only. The test
target for Flexible alternatives measures the documented alternative policy;
it does not convert a live or radio edit into a same-recording claim.

Check labels and source references with:

```sh
python3 scripts/validate_stage5_catalog_v3.py
```

After the fixture is frozen, the production-module evaluator can be run without
changing the application crate’s test registration:

```sh
cargo run --manifest-path scripts/heldout-matching/Cargo.toml -- \
  docs/fixtures/recording-match-stage5-heldout-catalog-v3.json
```

The evaluator output is a contract comparison, not an accuracy score. Report
each difference and retain it; do not relabel expected behavior to fit a run.
