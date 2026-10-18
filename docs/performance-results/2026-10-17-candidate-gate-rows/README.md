# Candidate-level gate diagnostics

The source now preserves per-candidate conjunction evidence: actual canonical/verified-alias recording, Topic and provisional predicate results, plus the existing first-accepting-path outcome. Metadata phrase observations and raw five-second duration comparison are explicitly descriptive, not substitutes for provider-specific gates. No acceptance decision is changed.

Tracing-disabled execution does not allocate these diagnostics. Tracing captures at most200 rows and emits up to12 rows per event; the aggregate summary event remains compatible. Rows contain only bounded enums, booleans and ordinals. Tests extend redaction, same-candidate evidence, row limits and a worst-case serialized chunk-size assertion.

Independent static review and rustfmt check pass. Seven obsolete, single-link Cargo leaf executables were verified unused and removed, reclaiming1,714,683,904 allocated bytes. Current Cargo outputs, immutable/rollback executables, user data, credentials and qualification evidence were retained. Ledger: `.cache/obsolete-cargo-leaf-executable-cleanup.json`. The completed tests/build and live diagnostic are recorded below. The sole normal app now runs native2bb09a9b0ce9; no matching rules or provider data were changed.

## First compiled regression run

The first native test build completed in2m01s. Eight selected tests passed, including row caps, redaction size bounds, tracing-disabled allocation/laziness and existing gate behavior; one new assertion failed. It incorrectly expected catalog-title equality between requested “Vgradena Nevesta (Built in Bride)” and candidate catalog title “Vgradena Nevesta”. The diagnostic correctly returned false, consistent with the existing aggregate counter. The assertion was corrected to false, with the failed log retained as `focused-native-tests-before-fixture-fix.log`; the corrected rerun passes as recorded below. This is not evidence of a production gate defect or verified recording equivalence.

## Corrected regression and full-suite result

The corrected focused run passes9 tests. The complete native library suite passes1,067 tests with21 ignored in50.13s. The subsequent executable build and live rollout pass as recorded below. No matching gates were changed.

Two bounded exact Spotify track metadata attempts returned HTTP429, including one after the first cooldown; no further attempt is scheduled. This is a provider rate limit, not proof that the recording is unavailable or that its source identity matches the MusicBrainz recording. No recording-to-source mapping was added.

The development executable build passes in2m06s with all542 pinned inputs unchanged. A read-only immutable candidate is frozen as `2bb09a9b0ce9`; source/build hashes are in `candidate.json` and `build-validation.json`. The subsequent live collection and normal-profile rollout are recorded below.

## Live isolated album diagnostic and normal restoration

The new native2bb09a9b0ce9/App33260ec39305 ran a fresh credential-free, empty-queue profile containing all19 exact catalog occurrences. Four exact public MusicBrainz keys were warmed through app APIs; payload changes mean this is not a causal latency comparison to the older run. Exact metadata/preflight and queue ownership pass. All19 occurrences finish unavailable in128.073s; no eligible media is found. This remains playback failure, not global unavailability proof.

The frozen trace validates57 provider groups and184 candidate rows with zero malformed lines or gate-summary mismatches. Monochrome3, YouTube65 and SoundCloud116 candidates are all rejected;181 are low confidence. No row has an accepting canonical/verified-alias recording, Topic or provisional predicate. No candidate has the descriptive title+artist-phrase+five-second-duration conjunction. These observations do not justify weakening confidence, artist, title or duration protections.

Exact owned terminal exhaustion and isolated volume restoration pass. An initial stop-snapshot attempt omitted the isolated endpoint and failed before signaling; retry with the explicit isolated endpoint snapshots and stops the correct launcher. The normal profile is restored through the verified launcher on2bb09a9b0ce9; the complete30-occurrence queue and entire playback snapshot compare exactly equal to the immediate pre-test snapshot. The normal launch uses tracing/default experimental flags off. Only one Burrata window remains.

Evidence: `.cache/qualification-candidate-gate-rows/live-kaufman/`, including `full19-current-private.json`, frozen app log (owner-only), bounded `inspect_rows.py`, sanitized `gate-rows-final.json`/`provider-aggregate.json`, launcher records and `restoration-proof.json`. The original failed fixture assertion and older unavailable runs remain retained. No provider writes or mapping changes were made.

The final integrated frontend suite also passes930 tests across77 files in116.19s (`frontend-full-tests.log`). This validates the combined frontend source, including the candidate-review UI and radio regression; it does not establish live feature completeness.
