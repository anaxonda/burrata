# Desktop release readiness

## Active release effort — 2026-10-18

[Burrata v0.1.2](https://github.com/anaxonda/burrata/releases/tag/untagged-2951018c6630cf30afde) is a verified **draft prerelease**, pinned to `34507720aa0d1362af3abee062a5a0cc3d033087`. [Full platform CI](https://github.com/anaxonda/burrata/actions/runs/39106398365) and [tagged release workflow](https://github.com/anaxonda/burrata/actions/runs/39106996579) pass. Linux DEB, macOS DMG and Windows NSIS installed-app smokes pass; RPM/AppImage are generated and checksummed. All five installer inventories match uploaded hashes/sizes; all 92 checksum entries match GitHub asset digests. Companion 0.2.1 includes the verified AMO-signed Firefox XPI. [Acceptance evidence](performance-results/2026-10-18-platform-release/README.md).

The sole local app runs exact tagged-source native `957eac158d87`, with the complete 30-occurrence queue and nine protected playback fields preserved. The verified launcher checks Vite-transformed modules and frontend/MCP connectivity. Final playback-bar geometry has no findings. Keep this release non-public as requested; OS security prompts, public signing/notarization and fresh-machine product qualification remain outside the recorded smoke checks.

Spotify playlist work remains paused; YouTube writeback remains disabled pending exact-preview approval. Experimental codec/storage changes remain off. Older exhaustive performance gates below are follow-up qualification, not blockers for this completed draft effort. [Current todo](../todo.md).

## Prior audit and distribution evidence

The dated checkpoints below remain evidence within their stated scope; their old runtime names and holds do not describe the active release effort.

Reachable-history follow-up: a bounded read-only scan covers 35 commits and 5,471 UTF-8 blobs, with one historical client-key pattern match and 22 binary omissions. It is not a clean-history or artifact qualification; see the [audit scope](performance-results/2026-10-17-history-pattern-audit/README.md).

Current checkpoint: the worktree secret-pattern audit now passes (10 scanner tests); history/artifact review and the remaining distribution gates stay open. The release storage benchmark compile ended with exit 143, then completed from 206 verified unchanged staged inputs. Its frozen release candidate passed the isolated actual-Store 50k benchmark, including 160 paired page reads and full pre/post-update parity. Indexing remains experimental and default-off; this Store result does not close whole-app or platform release gates. The first optimized attempt failed during fixture backfill with a SQLite disk I/O error; its log remains preserved, and the cause is unproven. See the [release Store report](performance-results/2026-10-17-text-index-release-store/README.md). One explicitly approved native Spotify-ID addition now has independent remote readback and a zero-diff/off preview; broader writeback remains unqualified. Creation status: approved Spotify empty creation returned HTTP 429/unknown outcome, and YouTube creation returned an ID but failed private/empty readback. No creation retry is permitted.

No release has been created. The full todo and platform qualification remain open. Earlier credential-pattern findings were corrected and the current worktree audit rerun passed; this does not establish a clean history or packaged artifact. The release-mode 50k Store harness passed its bounded actual-Store scope; it does not qualify the full application or promote the experimental indexes.

Audit date: 2026-10-14. This document describes the release automation and its remaining distribution gates. It is not a release approval or legal opinion.

## Release automation

`.github/workflows/release.yml` supports two paths. A `workflow_dispatch` with no tag builds the current `main` commit as a build-only qualification run, uploads native qualification packages, and skips the extension and draft-release jobs. A pushed `vMAJOR.MINOR.PATCH` tag must resolve to the checked out commit, and its version must equal `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`; that path creates a draft prerelease after validation. Native builders target macOS 15 arm64, Windows Server 2022 x64, and Ubuntu 22.04 x64. They run the source audit, contract checks, frontend checks and tests, extension tests, MCP tests, player tests, Rust formatting, and Rust tests before producing the installer.

The build outputs are a macOS DMG, Windows NSIS installer with the offline WebView2 installer embedded, and Linux `.deb`, `.rpm`, and AppImage packages. Linux AppImage packaging enables Tauri's media-framework option. Each platform artifact includes its source tag/commit, target, runner and compiler/runtime versions, SHA-256 inventory, vendored source inventory, third-party notices, and installed-smoke evidence. A separate extension job builds a Chromium ZIP and obtains an **unlisted AMO-signed** Firefox XPI. It requires `AMO_JWT_ISSUER` and `AMO_JWT_SECRET`; unsigned Firefox output is rejected. A blank-tag dispatch with `extensions_only=true` qualifies browser packaging without desktop builds or draft creation. Repeat signing reuses an existing AMO version only after verifying its published SHA-256 and exact extension payload; changed payloads require a new extension version.

Before collection, each native job installs and launches the produced desktop installer in isolated temporary app data. The smoke checks packaged Node, yt-dlp, FFmpeg/ffprobe, fpcalc, and native helpers; waits for authenticated GUI discovery; queries MCP `get_app_status`; and sends a native-host heartbeat. It writes `smoke-evidence.json`, which is included and checksummed with successful package artifacts and also uploaded when a smoke step fails. Linux currently installs the DEB for this smoke; RPM and AppImage are built and attached but are not installation-smoked. The scripted smoke does not assess macOS Gatekeeper or Windows SmartScreen prompts, and does not replace fresh-machine product qualification.

When a native release or Linux CI job fails, it retains only the built installer files as a seven-day `debug-installer-*` artifact. The manually dispatched `smoke-installed.yml` workflow can download one platform's retained installer by source run ID and rerun the current smoke code without rebuilding Burrata or its packaged runtimes. Its evidence records the source run and source commit; it is diagnostic and does not replace or relax the release workflow's required tests and smoke gate.

The final job verifies all three platform inventories match the same tag and commit and that both browser packages exist. It then creates a **draft prerelease** only. Maintainers review and accept the draft manually before publishing it. There is no automatic promotion, code signing, notarization, or in-app updater. Unsigned desktop downloads are an accepted product decision; macOS Gatekeeper warnings and Windows SmartScreen prompts remain expected for unsigned artifacts.

Each native job also builds the matching BgUtils provider/plugin 2.0.2 from its pinned upstream commit and emits a target-specific plugin ZIP with SHA-256 and SHA-512. Draft assembly verifies all three plugin assets and attaches them separately from the `yt-dlp` update binaries. `youtube-support.json` points to each matching `yt-dlp` executable and includes SHA-256/SHA-512; target filenames are preserved during assembly. The feed becomes reachable to clients only when maintainers manually publish the accepted release.

## Pinned runtime tools and source records

`scripts/release/locked-tools.json` records exact versions, upstream URLs, asset names, licenses, and upstream checksum manifests. `prepare-tools.py` stages native tools under `src-tauri/resources/tools/<target-triple>/` and rejects missing or mismatched upstream SHA-256 entries. The pinned shared JavaScript runtime is Node LTS 24. yt-dlp uses the tested official nightly PyInstaller binary with bundled EJS support. Deno is not part of the desktop runtime.

All three targets build FFmpeg/ffprobe 8.1.3 from the same pinned upstream source archive and shared GPL-3.0-or-later feature profile. The build statically includes LAME 3.100 and Opus 1.6.1, enables zlib (pinned static zlib 1.3.2 on Linux/Windows and SDK zlib on macOS), disables dependency autodetection, and retains broad built-in audio decoding, common containers and transports, PNG artwork encoding, metadata, and chapters. TLS uses Apple Secure Transport on macOS, pinned static OpenSSL 3.5.9 on Linux, and Windows Schannel on Windows; Windows native CI supplies an MSYS2 MINGW64 compiler for FFmpeg's MinGW build. Each target stages the exact corresponding source archives, upstream licenses, target build scripts, effective configure arguments, and hash inventory with the runtime. CI caches prepared runtime bundles by target, lock file, and builder source; every restored bundle is checked against its source inventory, recipe hashes, binary hashes, and versions before use. Native CI rehashes cached sources before use and exercises MP3, Opus, FLAC, PNG, and Matroska metadata/chapters against generated fixtures while checking HTTPS and common transport support. In the first local Linux build, the FFmpeg/ffprobe binary pair is 78,760,672 bytes versus 344,424,144 bytes in the previously staged Linux bundle, a 77.1% reduction for those two binaries; this does not estimate full application or installer size. The Windows and Linux BtbN binaries and their separate 2.1 GB source-cache bundle are no longer used by release preparation. `prepare-tools.py` and artifact collection verify the built binaries, source archives, build scripts, configuration, and notices against `locked-tools.json`. A successful clean-run qualification across the new builders is still required before release. The macOS bundle also includes fpcalc 1.5.1 from its pinned upstream asset with matching source archive and notices.

fpcalc 1.5.1 is included on all three targets from the official pinned Chromaprint release assets. Preparation verifies each archive and extracted binary hash, retains the matching source archive and upstream `LICENSE.md`, and adds the locally checked-in LGPL-2.1 text after verifying its locked SHA-256.

There is no `Deno` dependency: yt-dlp EJS is supplied by the official PyInstaller distribution, and the BgUtils YouTube support runtime uses the shared Node 24 resource. The bundled Node runtime and BgUtils helper/plugin must be staged by the release preparation path at the target-specific resource locations expected by the native resolver. Missing helper files or plugins must fail release preparation.

## Remaining distribution gates

- Native playback is behind an internal qualification gate and must remain off by default until its macOS, Windows, and Linux manual matrix passes. The planned Rodio/Symphonia/stream-download stack keeps FFmpeg as the HLS and HE-AAC fallback; see [native playback qualification](native-playback.md) for the build-only decode check, exact manual cases, device-removal and sleep/wake requirements, and measurements. Public distribution remains on hold until those runs are reviewed. Do not treat a build-only dispatch or CI decode test as manual qualification.
- Confirm codecs and format coverage against Burrata's conversion, validation, playback, and analysis paths on fresh macOS, Windows, and Linux installations.
- Confirm every other bundled tool's license, notices, exact source/configuration, and any source-offer obligations. See [vendored native sources](../src-tauri/vendor/SOURCES.md) and the generated release `THIRD_PARTY_SOURCES.md`.
- Exercise upgrade, playback, conversion, ffprobe validation, YouTube helper operation, and MCP behavior on fresh supported machines. The automated smoke covers basic install, launch, bundled-tool probes, MCP status, native-host heartbeat, and uninstall; it does not qualify operating-system security prompts or full product behavior.
- Browser companion rollout still needs an intentional Chromium distribution route. The release workflow supplies a ZIP; it does not publish to the Chrome Web Store.
- Signing and notarization are not configured. They are not required for the currently accepted unsigned installer plan, but unsigned macOS and Windows downloads may trigger operating-system warnings.
- App updates remain manual. No automatic updater or update signing keys are configured.

## Other runtime integrations

slskd remains a separate, optional service configured by the user. Burrata connects to its HTTP API and does not install or supervise it. Browser capture requires the companion extension and native messaging host. Optional AcoustID fingerprinting requires `fpcalc` plus an AcoustID key. Provider credentials are stored in the operating system credential store where supported; they are not release build secrets. See [tool setup](tool-setup.md), [connections](connections.md), and [MCP bridge](mcp.md).

The repository's `audit_secrets.py` checks common private-key, cloud-token, and credential-assignment formats without printing matches. It is heuristic, does not scan Git history, and does not replace secret rotation if a credential was ever committed.

## Release and usability follow-through

The 2026-10-14 implementation adds the player, playlist preferences, shared track views, searchable history/MCP tools, and public-documentation changes below. Shared source-build scripts and stronger installer media checks are implemented; native release qualification and fresh-machine manual checks remain separate distribution gates.

### Player layout and loading feedback

- Model resolving, buffering, playing, paused, and failed playback explicitly. Show an animated ring around the main play button while resolving or buffering, including after playback stalls; stop it on pause, success, cancellation, or failure. Preserve a working pause/cancel action and provide an accessible loading announcement and reduced-motion treatment.
- Give track information, transport, and trailing controls separate space in the bottom bar. Keep volume and queue controls from overlapping at half-screen widths, large text, and varying sidebar widths. Use a recognizable queue icon, an adequate click target, a tooltip, and a visible active state; show a text label when space permits. Move lower-priority details into the currently-playing pane at narrow widths.
- Validate local, YouTube/SoundCloud, chapter, and radio playback, including slow resolution, a stalled stream, and quick track changes. Loading from an old request must not overwrite the current track's state.

### Consistent track display and interactions

- Use one shared responsive track row/list and action menu for playlists, library, search, artist/discography tracks, album tracks, and radio/listening history. Keep typography, spacing, metadata separation, selection, playback state, and action placement consistent; views choose relevant columns rather than introducing independent row designs.
- Show title, artist, and essential availability first. At narrower widths, place artist beneath title and collapse secondary columns. Keep album track numbers, discography release/year, and history playback timestamps where relevant. Long text must truncate cleanly without concatenating artist/title or covering controls.
- Keep play, heart, download, and overflow as the main row actions. Put play-next, queue, playlist, source, and information actions in the shared overflow/right-click menu. Artist and album links use verified identities when present and offer a search when identity is unresolved.
- Share play/pause/buffering feedback and highlight the current track accessibly. Support Shift range selection, Ctrl/Cmd toggling, and a compact bulk-action toolbar across applicable lists. Surface downloads on affected rows and keep codec/provenance/match diagnostics in track information.
- Present discography albums separately from recordings; opening an album uses the shared track list. Standardize loading, empty, and provider-failure states. Keep Queue and Now playing in a responsive resizable side panel with clear tabs and preserve browsing position.

### Favorites and playlist shortcuts

`playlist_preferences.rs` persists a mutable Favorites playlist ID, nine shortcut destinations and configurable key combinations. It migrates legacy liked tracks, clears deleted shortcut targets, and synchronizes heart state with the selected playlist. The settings categories, player/pane hearts, shortcut assignment flow, and GUI/MCP playlist synchronization are wired to these services.

- Add matching heart controls to the playback bar and currently-playing pane. Their state comes from membership in the configured Favorites playlist and stays synchronized with track menus and playlist edits.
- Let users choose or create an editable Favorites playlist in Settings. Preserve existing liked tracks when introducing this setting; imported immutable snapshots cannot be the mutable target. Changing the setting must not silently move or remove tracks from the old playlist.
- Add up to nine configurable playlist shortcut slots. Invoking an unassigned slot opens a playlist chooser with a create-playlist action, then remembers the chosen playlist ID. Assigned slots add the currently playing track without changing playback; repeated additions do not duplicate it. Settings allow rebinding, clearing, and selecting another playlist, with conflict detection and explicit feedback for deleted destinations or no current track.
- Default shortcuts to the focused Burrata application and suppress them in text inputs, editable content, and modal workflows. Global shortcuts are a separate future decision. Make chapter actions target the current chapter's stable track identity when one is available.
- Reuse playlist services for the GUI and MCP. Heart removal affects only the chosen Favorites playlist membership, never the recording or downloaded file.

### Settings organization

Replace the long mixed settings page with a responsive category navigation and one active category:

1. General: theme and basic application preferences.
2. Playback: source preference, streaming fallbacks, preview behavior, radio recognition, and inclusion of radio songs in listening history.
3. Library & playlists: music folder, playlist folders, Favorites destination, and shortcut mappings.
4. Downloads & tagging: download sources/fallbacks, quality, and metadata recognition preferences.
5. Connections: browser companion, optional Soulseek/slskd, and catalog accounts.
6. Integrations: MCP permissions/setup and supported player/system integrations.
7. Advanced & diagnostics: bundled/managed/system tool paths and versions, yt-dlp updates/rollback, and troubleshooting.

Keep onboarding separate from everyday settings, provide a consistent save/status model, and show advanced paths only on demand. Check keyboard navigation and narrow layouts as well as full-width desktop use.

### Radio history as reusable tracks

- Replace station history's bespoke title/artist cards with the shared playlist track list, including standard artwork, row numbering, selection, context menus, play, download, Favorites, queue, playlist, artist, and information actions.
- Persist stable track identities for history entries, with station, playback date/time, and identification provenance. Show the local date/time for each listening occurrence in the list, with an explicit timezone in its full timestamp; allow newest/oldest sorting and date filtering. Distinguish actual user listening times from provider-supplied broadcast-history times, and retain the listening timestamp when a delayed recognition result arrives. Preserve distinct listening occurrences in history without duplicating the underlying track. Recognition results remain provisional unless reconciled to a catalog identity.
- Playing an earlier history item resolves the song through the normal audio-source pipeline; never reuse the live station stream URL as if it were that past song. Indicate when the resolved recording is uncertain or unavailable. This does not imply archived access to the original radio broadcast.
- Keep station history and overall listening history clearly distinguished, using the same list component and consistent actions. Previously heard entries remain visible when current station metadata expires or speech replaces music.
- Add **Include radio songs in playback history** under Playback settings, enabled by default for new and existing profiles without an explicit choice. Identified songs actually heard while listening also appear in overall history with a visible **Via [station name]** attribution, playback timestamp, and station link. Do not insert unplayed provider-history entries. Use one occurrence per heard song/session transition, reconcile delayed metadata/recognition without duplicates, and distinguish repeated listens. Turning this off stops future radio-song additions without removing saved history or disabling station history/recognition.
- Make history searchable by title, artist, album, and station, combined with date range, radio/non-radio/station filters, and newest/oldest sorting. Search the full persisted history on the backend with bounded pagination; do not limit matching to the currently loaded page. Apply the same behavior to station-history lists where relevant.

### History through MCP

- Read-scoped `search_playback_history` and `get_playback_history_entry` tools are exposed through the authenticated service/stdio bridge. `list_playback_history` is available as a search alias. Live SDK discovery, metadata enrichment, search and entry lookup have been verified against the running app.
- Share the GUI's history query service, filters, sort order, and pagination. Return stable history-entry and track IDs, listening timestamps, source kind, station ID/name, recognition provenance, and bounded pagination metadata; allow station-specific queries and distinguish broadcast timestamps from listening occurrences.
- History tracks should work with existing play, queue, playlist, and audio-source/download tools through their stable track IDs. Keep history lookup read-only and subject to the existing read scope; do not expose deletion merely to enable search.

### Shared FFmpeg build and fresh-machine qualification

- Shared source builds and platform TLS backends are implemented in release preparation. Keep bundled tools as the default; explicit system/custom overrides remain advanced settings.
- The build keeps broad audio decoding, common containers and streaming protocols, MP3/Opus encoders, metadata/chapters, and PNG artwork support while disabling optional dependency autodetection. Source/configuration, hashes, notices, and matching source archives are recorded for each target.
- Native CI caches source archives and prepared tools. Archives are checked against locked hashes; prepared tools are checked against build-recipe fingerprints, recorded binary hashes and expected versions before reuse. Measure installed size and installer download size against the current baseline; do not set a savings promise before measurement.
- Qualify exact installer downloads in fresh OS VMs with clean app data and no preinstalled development/media tools. Verify actual executable paths during playback, download, conversion, tagging/artwork, chapter operations, BPM/key analysis, recognition, MCP, yt-dlp update/rollback, restart, and uninstall. Include AppImage/RPM coverage and a separate downloaded-file Gatekeeper/SmartScreen check. Existing CI smoke remains a prerequisite, not a substitute.

### Public documentation and privacy

- Rewrite Soulseek instructions for a general audience: optional service setup, login/API key, host/container download-path mapping, sharing visibility, connection versus transfer troubleshooting, and supported OS options. Update settings labels and remove machine-specific success claims.
- Audit tracked files, Git history, release assets/logs, and Git author metadata separately for personal paths/usernames, local infrastructure, and credentials. Replace personal path examples with portable placeholders; keep legitimate upstream attribution intact. Confirm ignored credential/config files are absent from tracked history.
- Document recognition accurately: releases include the portable `burrata-recognize` helper derived from SongRec fingerprinting code; fingerprints are generated locally, but Shazam matching uses a remote service. Remove stale instructions implying a separately installed SongRec application is required.
- Review historical exposure before choosing any history rewrite; source-tree cleanup does not remove older commits or already uploaded release artifacts.
