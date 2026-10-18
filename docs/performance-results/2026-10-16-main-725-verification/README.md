# Main development verification, 2026-10-16

The same compiled native library test binary passes 725 tests, with zero failures and 14 ignored, on the host in 8.60 seconds. Its sandbox run passes 703 and fails 22: 19 localhost fixture restrictions and three owned-process capture fixture restrictions. All 22 pass in the same-binary host run. The protected binary SHA-256 is recorded in aggregate.json.

This checkpoint covers restricted Linux app-output capture fixtures, native/shared key parsing, queue metadata, and indexed single-asset reads for local audio requests, artwork and export. The lookup preserves existing hydration and error behavior. It does not remove the separate full asset/status reads at Library startup.

The frontend full suite passes 507 tests across 56 files. The subsequently extended shared key fixture passes all nine focused sorting tests. Svelte checks, the production frontend build and seven contracts pass. These are development/deterministic checks, not playback performance, live provider, release-build or cross-platform qualification. The GUI holding the provider login remains the earlier 472b58f8 executable.

The normal Linux GUI development executable builds successfully (1m44s), with SHA-256 `0d008f9c3a7d1550822e96752f0f6be138515280fdc1553b7a9cbb6345bb40f8`. It is protected in a separate immutable build-cache directory and has not been launched. The existing sign-in GUI remains intact.

The subsequent connection-control UI change passes the full 508-test frontend suite (56 files), zero-error/zero-warning Svelte checks and production frontend build. Resume is hidden during native sign-in/account review and retained after Stop or for a legacy opt-in. Native behavior and login state are unchanged.

Cache retention update: after the 727-test normal executable built and was protected, this superseded, never-launched 725-test GUI executable was removed to recover 329,438,000 bytes. Its manifest/hash remains recorded. The protected 725 test executable used by the decoder recheck remains available. No profile, credentials or data backup was removed.
