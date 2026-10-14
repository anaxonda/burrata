# Burrata

Local-first desktop music discovery and acquisition for DJs. Tauri 2, Svelte 5,
TypeScript, Rust, and SQLite. Licensed under GPL-3.0-or-later.

**Development status:** the discovery/acquisition app now includes a browser companion
for Beatport/Traxsource charts and Spotify playlists, plus YouTube/SoundCloud audio playback, Soulseek via a local slskd service,
and artist discovery. These integrations are experimental;
real website extraction, playback, and the complete desktop/Mixxx workflow require
live qualification.

Direct Beatport and Traxsource chart requests in this workspace returned HTTP 403
with provider verification pages. The companion uses your normal browser session;
verification or sign-in may require your attention. No API credentials are required.
Synthetic parser tests do not establish current website compatibility.

## Implemented workflow

- Beatport and Traxsource genre chart adapters, lazy track details, catalog search,
  original ranks and genres, local chart caching, and dated snapshots.
- Dense chart/search/library views with filtering, sorting, artwork, dark/light themes,
  and virtualized large tables.
- Persistent capability-aware preview player, keyboard navigation, favorites,
  shortlist/reject actions, and optional delayed auto-preview.
- Independent playlists, CSV/tracklist/M3U imports, local-file import, and explicit verified recording
  merges. Different remixes and edits remain separate until you verify a merge.
- YouTube/SoundCloud source discovery and inspection through `yt-dlp`; explicit
  Bandcamp/artist/label source URLs; source/version/duration comparison.
- Persistent acquisition queue, structured progress, cancellation during acquisition,
  explicit retry after failures, automatic Soulseek reconciliation after restart, and duplicate-source checks.
- ffprobe validation, selected metadata tags/artwork through Lofty, audio hashing,
  publication journals, and atomic publication without overwriting existing files.
- Original codec preservation, separate MP3 compatibility exports, portable M3U8
  playlists, and Open in File Manager.

Click a song to play its local audio or stored preview. If neither is usable,
Burrata automatically searches YouTube, then SoundCloud for a plausible artist/title
match. Playback does not fetch blocked storefront pages. Preview searches inspect
three results instead of the manual search's ten. Burrata prepares the first three
tracks and nearby tracks in the background, sharing cached lookups and audio-stream
resolution with clicks. Obsolete search requests are cancelled when switching songs.
YouTube and SoundCloud audio streams play in the main audio player without opening
a video window. Stream URLs are temporary and kept only in memory. A preview match
does not verify the recording's identity or version, and does not add audio files to
your library. Output uses system audio routing. The ↗ action on each track opens its
original catalog page; track details show the same action with the provider's name.

Search includes direct YouTube and SoundCloud queries, with separate catalog search.
Each row has Play, Download and More controls. Download queues the best plausible
match from the chosen source. A linked audio result uses its own exact source;
a single-source Search selection applies to other search results; Charts and
Library use the preferred source in Settings. That preference saves immediately.
More opens source/mix
review, available codec/bitrate, output profile, destination, and editable metadata.
Automatic matches can be wrong; matching scores are heuristics rather than proof
of identity. Use More when you want to verify the mix before acquisition. Selecting a preview does not start an acquisition.

Create separate playlists, add current selections to a new or existing playlist,
rename them, or remove members. The now-playing title opens that recording's details
without starting playback again. Downloads preserve selected metadata and estimate
missing BPM and musical key from audio; estimates are labeled and do not replace
catalog values. Library analysis also updates existing downloaded files without
changing their paths or re-encoding their audio. Accepted metadata corrections
update local tags through a recoverable background job.
Artist names open MusicBrainz identity search and paginated recordings, releases,
and collaborations. Optional Discogs and Last.fm connections add release metadata
and popularity-ranked tracks. Metadata suggestions require explicit field review;
release dates belong to the selected edition, never a video upload date.

Soulseek search/downloads use a separately running local slskd service.
Playlist folders in Settings discover M3U/M3U8 files recursively; the music
directory is the default. Import reconnects registered files, or retains explicitly
unverified EXTINF/filename metadata.

See [connections setup](docs/connections.md), [local slskd setup](deploy/slskd/README.md)
and [tool setup](docs/tool-setup.md) for tool purposes and platform installation links.

## Development

Requires Node >=22.12, npm, Rust, and the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/).
The frontend is a Vite SPA; no hosted backend or user account is required.

```sh
npm install
npm run check
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri -- dev
```

For a local app that keeps working without the Vite development server:

```sh
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --features custom-protocol
./src-tauri/target/debug/burrata
```

This launch embeds the built frontend. A plain `cargo run` development launch
instead requires Vite on localhost:1420 for the entire app session.

Build Linux packages after the real-provider acceptance gate passes:

```sh
npm run tauri -- build
```

Release packaging stages yt-dlp, Node.js, FFmpeg/ffprobe, the native recognition
helper, and the browser/MCP companion tools with the app. Node.js is shared by
yt-dlp's packaged JavaScript challenge support and local MCP services; Deno and a
separately installed EJS package are not required for the packaged setup. Managed
YouTube support can check the configured Recommended release feed or official yt-dlp
nightlies, verify checksums, probe compatibility, and roll back to the prior version
or bundled baseline. The Recommended feed is only available after a release publishes
its manifest. The tagged release workflow creates a draft prerelease, while a manual
build without a tag produces native qualification packages only. Native package jobs
install and launch the macOS DMG, Windows NSIS installer, or Linux DEB in isolated app
data, probe bundled tools, query the authenticated MCP status endpoint, and test the
native-host heartbeat. They attach checksummed smoke evidence to the packages and
preserve failure evidence. The Linux RPM and AppImage are built but do not receive the
installation smoke; OS security prompts and full fresh-machine behavior remain manual
qualification. The Windows/Linux FFmpeg corresponding-source archives are published
separately in the
[runtime source release](https://github.com/anaxonda/burrata/releases/tag/runtime-sources-2026-10-07).
The anonymous BgUtils provider runs locally on loopback when needed; Burrata does not
use account cookies or sign-in. Preview streams are temporary and are not retained.
Configure custom executable paths and the absolute music directory in Settings when
needed. `slskd` remains an optional, separately installed service for Soulseek.
See [tool setup and YouTube support](docs/tool-setup.md) for details. The application
does not run a continuously active Python service; a custom yt-dlp installation may
itself use Python.

Choose the music directory before acquisition. Published files can be imported through
Mixxx's normal music-directory workflow. Other DJ software can import supported files
through its normal library interface. There is no proprietary DJ database mutation.

### Keyboard

| Key | Action |
| --- | --- |
| Up / Down | Select previous / next filtered track |
| Space | Play / pause; start selected track if no current preview |
| S | Shortlist selected recording |
| R | Reject selected recording |

Shortcuts do not capture typing inside inputs. Selection and currently playing audio
are distinct. Clicking a song starts playback. Playing songs selected with arrow
keys is optional and off by default; its default delay is 250 ms. **Playlists** are
saved lists of tracks (called collections in the storage model).

### Imports and file formats

Tracklists use `Artist - Title (Version)`, optionally with numbered rows. CSV requires
`artist,title`; optional columns include `version`, `label`, `genre`, `bpm`, `key`,
`duration` (seconds or `minutes:seconds`), and `url`. Import validation is all-or-nothing.
Spotify playlist CSV exports are also accepted through Import → CSV / Spotify export.
Supported columns include Track URI, Track Name, Artist Name(s), Album Name, Album
Release Date, Album Image URL, Track Duration (ms), and ISRC. Spotify track IDs and
source links are retained; milliseconds are converted to seconds. Unique recordings
retain their first playlist position; repeated IDs are deduplicated. Numeric Spotify
pitch-class values are preserved as provenance rather than written as musical-key tags.
Live Spotify playlist URL import is supported through the browser companion. This imports
track metadata for subsequent matching; it does not download Spotify audio.
M3U/M3U8 files can be selected in Import. Extended playlists must contain
`#EXTINF` names in `Artist - Title (Version)` form; entry paths and URLs are
discarded. This imports playlist metadata, while local audio files use the
separate local-file import.

Imported names are unverified identities, not automatic matches to provider listings.

Source output preserves codec and quality, remuxing to a supported container when
necessary. MP3 exports are separately identified compatibility derivatives. Encoding a
lossy source at a higher bitrate does not restore missing audio information. Actual
codec/bitrate and processing history are stored separately. Formats supported by the
media pipeline include MP3, FLAC, Opus, M4A, WAV, AIFF, and Vorbis; compatibility with a
specific DJ program or hardware player must be checked separately.

Downloads are always user initiated. Source permissions remain `unknown` unless
established, and restricted sources are rejected. Public availability is not recorded
as permission to download. Authentication and access restrictions are not bypassed.

## Structure

- `src/`: Svelte UI, supported global Tauri bridge, playback race control and virtualization.
- `src-tauri/src/providers/`: independent chart/search/preview capabilities and site adapters.
- `src-tauri/src/storage.rs` and `schema.sql`: versioned SQLite schema, snapshots,
  source listings, recording identity, collections, settings, assets and jobs.
- `src-tauri/src/acquisition.rs`, `matching.rs`, `media.rs`: source inspection,
  subprocess lifecycle, conservative identity matching, tagging and publication.
- `src-tauri/tests/fixtures/`: synthetic provider parser specimens.
- `scripts/check_contracts.py`: dependency-free IPC/capability and schema checks.

SQLite lives in Tauri's platform application-data directory. Audio lives in the chosen
music directory. Staging is `.burrata-staging/<job-id>` on the destination filesystem;
publication journals survive a filesystem/SQLite commit gap and are reconciled at
startup. Publication requires filesystem hard-link support and fails explicitly if it
is unavailable. Already imported local audio is read without being rewritten.

Catalog defaults: one concurrent request per provider, at least one second between
requests, `Retry-After` respected, six-hour chart cache, seven-day detail cache. Refresh
failures retain cached charts with a warning. These are application defaults, not
provider-granted quotas. Manual metadata takes precedence over refreshed catalog data.

No telemetry, mandatory account, cloud storage, or downloaded audio belongs in this
repository. Provider HTTP parsing is unofficial and experimental.

## Browser companion

See the [chart/playlist import implementation summary and guide](docs/chart-playlist-imports.md)
for setup, current validation, and the Beatport 0.1.3 repair.

See [installation and capture limitations](extension/README.md). Build the desktop
binary, register its native host, and load the Firefox or Chromium extension. Manual
**Import current page** works without persistent website permissions. Enabling
**app-driven imports** lets Burrata request a chart/Spotify playlist URL automatically;
verification pages are brought forward when needed.

Imports create dated snapshots and a saved collection. Partial captures add tracks
without removing previous members. A verified complete capture synchronizes that
source collection, including removing missing members. Use a separate personal
collection for independently curated additions. Spotify repeated occurrences retain
position provenance while collection membership deduplicates recordings.

Charts restores the most recent saved browser import on startup and exposes saved
imports directly in the chart view. Collections defaults to the most populated
saved collection. Library includes saved recording metadata as well as local audio;
an imported recording appears there before audio is acquired.

The bridge is local and authenticated; extension metadata cannot execute downloads
or import local paths. `BURRATA_APP_DATA` optionally selects a separate app-data directory
for development/portable use; the app and native host must use the same directory.

## Validation

```sh
./scripts/verify.sh
# Local socket forwarding test needs permission to bind a loopback listener:
cargo test --manifest-path src-tauri/Cargo.toml native_forwarding_authenticates_exact_request -- --ignored
```

Provider/parser and player tests use synthetic fixtures, a real sanitized Beatport
Afro House row, or mocked official APIs.
Validation completed in this workspace:

- Latest playback/UI checks: 101 Rust tests passed, with the existing opt-in loopback
  forwarding test ignored; 46 frontend tests, five mocked player tests, and seven
  contract checks passed. The browser-companion work passed 26 tests separately.
- Svelte/TypeScript reported zero diagnostics; the production frontend build passed.
- Synthetic IPC smoke exercised the compiled UI, including imports, candidate controls,
  keyboard switching, stale responses, and dark/light layouts at 960/1440 pixels.
- Live YouTube and SoundCloud `yt-dlp` searches returned public results; YouTube format
  inspection returned AAC and Opus audio. A search for the imported Spotify song
  Jamaican (Bam Bam) returned a plausible review candidate; the imported Traxsource
  song Tonite had no plausible result in the checked searches.
- Resolved YouTube AAC and SoundCloud progressive MP3 streams played successfully
  in Chrome with advancing playback time and no media error. Native Linux WebKitGTK
  still needs a live check. Tag round trips cover all selected fields across MP3,
  FLAC, Opus, M4A, WAV, and AIFF; a generated 120 BPM track verifies tempo analysis
  and the written BPM tag. ID3 formats now use v2.4 to preserve full release dates.
- The Linux desktop binary built and started with isolated data. Persistent native-host
  framing imported a partial playlist through the authenticated bridge into SQLite
  and rejected unsafe fields.
- Both official embedded players played actual audio/video in Chromium using the
  production player assets and CSP. Tauri WebKitGTK playback remains unqualified.

Full acquisition, real extension website extraction, and Mixxx acceptance still need
live verification. No DJ database is modified by Burrata.

## Release acceptance still required

1. Retrieve and correctly rank real Traxsource Deep House and Beatport genre charts.
2. Validate real preview playback in Linux WebKitGTK and rapid keyboard switching,
   including stale requests, seeking, provider failures and no overlapping audio.
3. Save a collection, restart, and verify persistence and cached offline browsing.
4. Search and inspect YouTube/SoundCloud candidates, retaining uncertain versions for review.
5. Acquire an authorized test recording, cancel/retry another, and inspect the final
   audio and tags in every supported format.
6. Import the completed directory into Mixxx and load the recording onto a deck.
7. Combine two charts, verify overlapping recordings, merge them explicitly, and
   confirm one collection/playlist entry with both provider ranks preserved.
8. Review native installed-package smoke evidence and separately install/qualify Linux
   RPM and AppImage packages; validate startup recovery before publishing a release.

Hype/DJ/historical charts, starter-pack automation, native drag-out, and full
fresh-machine Windows/macOS qualification remain later milestones. The generated
lockfiles should be used during release qualification.
