# Third-party notices

This release bundles the following pinned tools. Exact versions, URLs, and archive checksums are in `locked-tools.json`; platform-specific binary/source checksums are in each `LOCKED_TOOLS-*.json` and `THIRD_PARTY_SOURCES.md`.

- Node.js 24.21.0 — MIT; the upstream `LICENSE` file is bundled beside the runtime.
- yt-dlp nightly 2026.10.10.233141 — the core source is Unlicense; the PyInstaller binary also contains GPLv3+ code and other third-party components. The matching source commit's `LICENSE` and `THIRD_PARTY_LICENSES.txt` are bundled.
- FFmpeg/ffprobe 8.1.3 — GPL build. macOS is built from the pinned FFmpeg source with static LAME and Opus and Secure Transport. Windows and Linux use the pinned BtbN FFmpeg-Builds release. Its exact build recipe and all 119 dependency source-cache archives selected by the successful Linux/Windows target builds are available in the separate `runtime-sources-2026-10-07` source-only release; its compact hash/license-path manifest is included with each installer. The 15 ffnvcodec headers each retain the full embedded MIT grant and copyright notice; paths and hashes are recorded in the source manifest. See `FFMPEG_LICENSE.txt`, `FFMPEG_BUILD_INFO.txt`, and `THIRD_PARTY_SOURCES.md` for build and source records.
- LAME 3.100 — LGPL-2.0-or-later; source archive and upstream license accompany the macOS tool bundle.
- Opus 1.6.1 — BSD-3-Clause; source archive and upstream license accompany the macOS tool bundle.
- Chromaprint/fpcalc 1.5.1 — official pinned upstream fpcalc assets are used on all three targets, with archive and extracted-binary hashes verified. Each platform tool bundle also carries the matching Chromaprint source archive, its upstream `LICENSE.md`, and the locally pinned LGPL-2.1 text.
- BgUtils POT Provider 2.0.2 — provider server and matching yt-dlp plugin from commit `26475e9d3665b972c4aeb83b4dbaafcf64b88438`; source and package lock files are retained in the resource bundle. Its plugin download is attached as a separately checksummed release asset.

The Burrata source tree is distributed under GPL-3.0-or-later. See `THIRD_PARTY_SOURCES.md` for vendored native source inventory. Browser companion packages retain their own manifest and licensing metadata; Firefox is signed by AMO as unlisted and Chromium is supplied as a ZIP.
