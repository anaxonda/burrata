# Burrata 0.1.2 release acceptance — 2026-10-18

> Publication update (verified directly on GitHub 2026-10-18): [v0.1.2](https://github.com/anaxonda/burrata/releases/tag/v0.1.2) was published at 16:16:27 UTC as a public prerelease (`isDraft: false`). Supporting downloads remain a draft. The draft descriptions below record the earlier qualification stage; main-release publication is complete. Later local playback/extension fixes are outside the released commit.

The practical playback/UI/final-bug-check effort was qualified as a draft, then published as the [v0.1.2 prerelease](https://github.com/anaxonda/burrata/releases/tag/v0.1.2); [platform CI](https://github.com/anaxonda/burrata/actions/runs/39106398365); [tagged release workflow](https://github.com/anaxonda/burrata/actions/runs/39106996579). Source tag `v0.1.2` resolves to `34507720aa0d1362af3abee062a5a0cc3d033087`. The earlier failed v0.1.1 tag is unchanged.

## Platform and artifact evidence

- Linux 1,092, macOS 1,058 and Windows 1,018 native library tests pass (21 intentionally ignored on each platform), plus deterministic WAV decoder, frontend/provider/companion/MCP/player, source audit and command/permission checks. See `native-test-counts.json`.
- The installed DEB, DMG and NSIS apps pass packaged-tool versions, MP3/Opus encode/decode/tag/cover fixtures, recognizer startup, browser-host heartbeat and authenticated frontend/MCP startup (103 tools). See the three `smoke-*.json` records (line endings normalized in these documentation copies; original artifacts remain attached to the workflow/release).
- RPM and AppImage are generated and checksummed, not independently installation-smoked.
- All five installers match their platform inventories by SHA-256 and size. The original 92-entry manifest was verified before the download-list cleanup; the current main/support manifests cover 93 unchanged payloads and match GitHub's asset digests. The checksum file and acceptance note themselves were separately downloaded and digest-verified. See `asset-proof.json`.
- Both companions report 0.2.1; the Firefox XPI has its signature block and matches the AMO-qualified job artifact. Its SHA-256 is `ad4914197080a0d9259004d69b094ddf3b7baa267bc016c4a7e99e32f6baa08a`.

## Download-list cleanup

The [main release](https://github.com/anaxonda/burrata/releases/tag/v0.1.2) now contains five installers with clean filenames, the Chromium extension, signed Firefox extension and `SHA256SUMS.txt` (eight assets total). [Supporting downloads](https://github.com/anaxonda/burrata/releases/tag/untagged-26f63c0ec9ce9d663846) contain optional helper updates, matching sources, licenses and acceptance records (87 assets including their own checksum manifest). Ordinary installation does not require these supporting packages.

All 93 original non-checksum payloads retain their SHA-256 and size. Main checksums cover seven downloads; supporting checksums cover 86 files. The helper feed and its referenced binaries stay together in the supporting release. Both drafts remain non-public and target the same qualified commit. Future release assembly uses verified installer/companion inventories rather than publishing every build artifact in the main list. The packaging fixture checks this split, both manifests, helper-feed consistency and fail-closed rejection of a damaged installer.

README was reduced from 286 to 72 lines, with current installer/extension guidance, bundled-tool behavior, concise source-development instructions and links to detailed documentation. All local README links resolve.

## Local deployment and UI

The immutable native debug build from the exact tag is `957eac158d87a4c1a90216657a6e32f70f7ccd4d4281d76b6a3ce0ec354159b5`. All 5,645 pinned inputs were unchanged during the successful build. The verified launcher checked transformed Vite modules, then confirmed frontend mounting and all 103 MCP tools. Exactly one Burrata GUI is running. All 30 queue occurrences, occurrence IDs/order, selected index, track ID, position, paused state, volume, repeat and shuffle are preserved across restart; see `preservation-proof.json`.

The final 1914×515 desktop screenshot and the playback geometry inspection show no footer overlap. Earlier narrow/wide checks are in the [desktop report](../2026-10-17-release-desktop-pass/README.md). Screenshots and full user-state snapshots remain private in `.cache/release-final-playback/`.

## Corrections found during qualification

- Use Tauri's own async-task handle type for macOS/Windows system media workers.
- Retain Windows canonical path prefixes without querying an incomplete drive-prefix path; keep each real component and symlink check.
- Link the existing Common Controls v6 resource into the Windows library test harness so its TaskDialogIndirect import can load. Production manifest handling is unchanged.
- Correct fixture assumptions about macOS canonical temporary paths and accepted-socket blocking state, Linux-only ETXTBSY behavior, Windows dotfile scope, refused-connect timing and native path separators. The final child-directory test also exercises roundtrip browsing.

The first final local rebuild exhausted disk space at link time. After confirming no compiler was active, inactive generated incremental cache was removed; a retry with incremental compilation disabled passed. No user library, credentials, provider profile or saved queue was deleted.

## Limits retained

This is a draft, not a public promotion. Gatekeeper/SmartScreen prompts, signing/notarization and fresh-machine product qualification are outside the installed-smoke reports. Live playback evidence remains bounded rather than a statistical latency/CPU qualification. Source crosswalk checks fail closed for absent or ambiguous exact edition relationships and do not guarantee playable audio. Spotify playlist work is paused; YouTube writeback remains off awaiting the reviewed preview; codec/storage experiments remain off. See [playback](../2026-10-17-webview-stall-recovery/README.md) and [crosswalk](../2026-10-17-verified-source-crosswalk/README.md) evidence.
