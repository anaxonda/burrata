# Desktop release readiness

Audit date: 2026-10-13. This document describes the release automation and its remaining distribution gates. It is not a release approval or legal opinion.

## Release automation

`.github/workflows/release.yml` builds only from an existing `vMAJOR.MINOR.PATCH` tag. The tag must resolve to the checked out commit, and its version must equal `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`. Native builders target macOS 15 arm64, Windows Server 2022 x64, and Ubuntu 22.04 x64. They run the source audit, contract checks, frontend checks and tests, extension tests, MCP tests, player tests, Rust formatting, and Rust tests before producing the installer.

The build outputs are a macOS DMG, Windows NSIS installer with the offline WebView2 installer embedded, and Linux `.deb`, `.rpm`, and AppImage packages. Linux AppImage packaging enables Tauri's media-framework option. Each platform artifact includes its source tag/commit, target, runner and compiler/runtime versions, SHA-256 inventory, vendored source inventory, and third-party notice file. A separate extension job builds a Chromium ZIP and obtains an **unlisted AMO-signed** Firefox XPI. It requires `AMO_JWT_ISSUER` and `AMO_JWT_SECRET`; unsigned Firefox output is rejected.

The final job verifies all three platform inventories match the same tag and commit and that both browser packages exist. It then creates a **draft prerelease** only. Maintainers review and accept the draft manually before publishing it. There is no automatic promotion, code signing, notarization, or in-app updater. Unsigned desktop downloads are an accepted product decision; macOS Gatekeeper warnings and Windows SmartScreen prompts remain expected for unsigned artifacts.

Each native job also builds the matching BgUtils provider/plugin 2.0.2 from its pinned upstream commit and emits a target-specific plugin ZIP with SHA-256 and SHA-512. Draft assembly verifies all three plugin assets and attaches them separately from the `yt-dlp` update binaries. `youtube-support.json` points to each matching `yt-dlp` executable and includes SHA-256/SHA-512; target filenames are preserved during assembly. The feed becomes reachable to clients only when maintainers manually publish the accepted release.

## Pinned runtime tools and source records

`scripts/release/locked-tools.json` records exact versions, upstream URLs, asset names, licenses, and upstream checksum manifests. `prepare-tools.py` stages native tools under `src-tauri/resources/tools/<target-triple>/` and rejects missing or mismatched upstream SHA-256 entries. The pinned shared JavaScript runtime is Node LTS 24. yt-dlp uses the tested official nightly PyInstaller binary with bundled EJS support. Deno is not part of the desktop runtime.

All three targets include FFmpeg/ffprobe 8.1.3. macOS builds natively from the pinned source archive with statically built LAME 3.100 and Opus 1.6.1, disables autodetection, and enables Secure Transport. Windows and Linux use named assets from the immutable BtbN FFmpeg-Builds release and verify them against its checksum file. Their exact BtbN autobuild recipe and all 119 unique dependency source-cache archives referenced by the successful Windows/Linux target Dockerfiles are preserved in the separate `runtime-sources-2026-10-07` source-only release. Native preparation validates the checked-in compact source manifest, so compilation can proceed while that release upload completes. Final artifact collection then verifies the published manifest bytes and both source-asset sizes against the lock; installers link to the separate source release without embedding its 2.1 GB of archives. The manifest inventories source hashes, license/notice paths, and embedded MIT header grants. The macOS bundle also includes fpcalc 1.5.1 from its pinned upstream asset with matching source archive and notices. Missing binaries, source manifest, or checksum matches fail preparation; missing published source assets fail artifact collection.

fpcalc 1.5.1 is included on all three targets. macOS builds the CLI from pinned source against the static FFmpeg prefix; Linux and Windows use the official upstream fpcalc archives after verifying the exact asset and extracted binary hashes. Each target retains the matching source archive and LGPL license text.

There is no `Deno` dependency: yt-dlp EJS is supplied by the official PyInstaller distribution, and the BgUtils YouTube support runtime uses the shared Node 24 resource. The bundled Node runtime and BgUtils helper/plugin must be staged by the release preparation path at the target-specific resource locations expected by the native resolver. Missing helper files or plugins must fail release preparation.

## Remaining distribution gates

- Confirm codecs and format coverage against Burrata's conversion, validation, playback, and analysis paths on fresh macOS, Windows, and Linux installations.
- Confirm every other bundled tool's license, notices, exact source/configuration, and any source-offer obligations. See [vendored native sources](../src-tauri/vendor/SOURCES.md) and the generated release `THIRD_PARTY_SOURCES.md`.
- Exercise install, upgrade, uninstall, first launch, playback, conversion, ffprobe validation, YouTube helper operation, and MCP behavior on clean supported machines. CI compilation/tests do not establish clean-machine qualification.
- Browser companion rollout still needs an intentional Chromium distribution route. The release workflow supplies a ZIP; it does not publish to the Chrome Web Store.
- Signing and notarization are not configured. They are not required for the currently accepted unsigned installer plan, but unsigned macOS and Windows downloads may trigger operating-system warnings.
- App updates remain manual. No automatic updater or update signing keys are configured.

## Other runtime integrations

slskd remains a separate, optional service configured by the user. Burrata connects to its HTTP API and does not install or supervise it. Browser capture requires the companion extension and native messaging host. Optional AcoustID fingerprinting requires `fpcalc` plus an AcoustID key. Provider credentials are stored in the operating system credential store where supported; they are not release build secrets. See [tool setup](tool-setup.md), [connections](connections.md), and [MCP bridge](mcp.md).

The repository's `audit_secrets.py` checks common private-key, cloud-token, and credential-assignment formats without printing matches. It is heuristic, does not scan Git history, and does not replace secret rotation if a credential was ever committed.
