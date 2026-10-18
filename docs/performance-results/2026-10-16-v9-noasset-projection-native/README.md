# Isolated no-asset projection candidate native verification

This records source validation only; it is not a performance qualification and the candidate has not been promoted.

- Candidate source: isolated `/tmp/burrata-availability-native-prototype/src-tauri/src/storage.rs`
- Source SHA-256: `c4bd096756b78859524f30c01e6c2a2b38ec26285857947702ef63da45718944`
- Immutable test binary SHA-256: `87cc8decc976a2eebfddad4b9c89aa9548e3127f58220d7c837401fa10f8e7a7`
- Binary path: `/tmp/burrata-sorting-target/debug/verified-tests-v9-noasset-projection-c4bd0967/burrata_lib`
- Build: `cargo test --lib --no-run`, shared target, incremental disabled, `-j1`
- Focused regression: `storage::tests::no_asset_bpm_projection_index_covers_manual_values_and_asset_transitions` passed (1 passed, 733 filtered out)
- Full host suite: 718 passed, 0 failed, 16 ignored (734 total), 44.09 seconds; raw output in [host-tests.log](host-tests.log)

The candidate stores exact raw-plus-manual effective JSON as a sort projection for asset-free rows, allowing complete projection coverage and BPM-index eligibility. The regression covers manual-value parity, invalidation, null-last ordering, ties, both sort directions, first and later page plans, and sole-asset insertion/deletion. It has not been benchmarked after this change.
