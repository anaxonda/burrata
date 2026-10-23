# Chart and playlist imports — implementation summary

Current browser companion version: **0.1.7**. This document covers the import work
completed so far, setup, observed failures, and remaining validation.

## Supported paths

| Input | Import path | Current evidence |
| --- | --- | --- |
| Beatport charts | Browser companion; direct Rust adapter where accessible | Actual sanitized Afro House row parses correctly; full browser import after the latest fix still needs confirmation |
| Traxsource charts | Browser companion; direct Rust adapter where accessible | Live import saved 100 tracks with continuous ranks 1–100; original capture had unknown total |
| Spotify playlists | Browser companion | Live import saved positions 1–31 of 96; updated scrolling needs live revalidation |
| Spotify albums | Browser companion | Captures ordered album tracks and applies the album title and cover art |
| Spotify CSV exports | App import panel | Column aliases, track IDs, durations, order, and deduplication tested |
| Other CSVs and pasted tracklists | App import panel | Validated local import implementation |
| M3U / M3U8 playlists | Local playlist discovery and app import panel | Discovery opens local playlists as Burrata collections; entries can include local paths or streaming URLs |
| Browser snapshot JSON | App import panel | Accepts the same typed metadata snapshot as the browser bridge |

SoundCloud and YouTube are supported for source search and candidate previews;
the companion does not currently import their playlists. Importing a Spotify
playlist imports metadata, not Spotify audio.

## Additional public collection imports (October 13, 2026)

The URL import tries a direct public tracklist first, then sends an import request
to the connected companion browser. Existing imported playlist headers and context
menus have **Open source to refresh**, which opens the saved URL in the OS default
browser for manual companion capture. These actions preserve dated import versions.

| Source | Direct path | Browser path and limits |
| --- | --- | --- |
| Shazam | Top 200/Top 50 country or world chart CSV | Rendered chart rows; live world CSV parser and in-memory persistence passed for 200 tracks |
| Bandcamp | Artist album URLs with structured tracklists | Rendered album tracklist; live Björk Debut parse/persistence passed. Discover/fan album cards are not tracklists |
| Last.fm | Public chart rows when readable; official chart/user top tracks API with configured key | Rendered chart/library rows. Live public request returned a client challenge; the configured API key successfully fetched and validated 1,000 tracks (capped, partial). Public visible slices remain partial |
| Spotify | Browser companion | Albums/playlists; no direct anonymous scraper |

Last.fm user imports preserve supported overall, weekly, monthly, quarterly,
half-year and yearly periods. Custom date ranges fall back to browser capture.
Shazam CSV rows without recording URLs remain unverified metadata; no catalog ID
is invented. These imports do not download provider audio.

Chrome's loadable copy is rebuilt by `npm run extension:chrome`; reload the existing
companion to pick up new parsers and optional provider permissions. Firefox uses
`extension/manifest.json` directly; reload the temporary add-on after source changes.

## Provider genre Top 100 links

The browser companion’s genre picker uses explicit routes returned by each Rust
provider. Traxsource currently exposes 28 genre links in its official homepage
menu; all 28 are represented, including chart routes for Pop / Dance, R&B / Hip
Hop, and Efx / DJ Tools. Beatport’s current genre menu has 47 entries across
Electronic and Open Format; all 47 are represented with their observed IDs and
slugs, including Open Format Rock.

The visible names and routes come from the official [Traxsource genre
menu](https://www.traxsource.com/) and current [Beatport genre
taxonomy](https://greenroomsupport.beatport.com/hc/en-us/articles/41043520429076-Beatport-Genres-Including-NEW-Open-Format-Genres).
Beatport’s [Top 100 chart guide](https://greenroomsupport.beatport.com/hc/en-us/articles/49464019907092-What-are-Top-100-Charts)
confirms genre Top 100 tracks are available. The route pattern and category
IDs were checked against official chart links, including [Amapiano](https://www.beatport.com/genre/amapiano/98/top-100),
[140 / Deep Dubstep / Grime](https://www.beatport.com/genre/140-deep-dubstep-grime/95/top-100),
[Breaks / Breakbeat / UK Bass](https://www.beatport.com/genre/breaks-breakbeat-uk-bass/9/top-100),
[Rock](https://www.beatport.com/genre/rock/109/top-100), and [Traxsource Deep House](https://www.traxsource.com/genre/13/deep-house/top).

## Why a browser companion

Direct Beatport and Traxsource requests may return HTTP 403 verification pages.
API access is not available for this project. The companion reads
public track metadata from pages loaded in the user's normal browser session.

Manual capture uses temporary access to the active tab. Optional app-driven mode
lets Burrata request a supported URL and open an inactive browser tab. A browser
must remain running; provider sign-in or verification can still require user
interaction. Verification pages are brought forward rather than solved automatically.

## Linux / Firefox setup

From the project directory:

```sh
cd /path/to/burrata
npm run tauri -- dev
```

After the desktop executable has built, use another terminal to register the host:

```sh
cd /path/to/burrata
python3 scripts/install_browser_host.py \
  --browser firefox \
  --executable "$PWD/src-tauri/target/debug/burrata"
```

The Python script creates a launcher and a Firefox native-messaging registration
under `~/.mozilla/native-messaging-hosts/`. The launcher executes the Burrata binary
with `--native-messaging`. The script does not install the extension, read cookies,
or download audio. Python is only needed for this registration step; the host itself
is Burrata's Rust executable.

1. Open `about:debugging#/runtime/this-firefox` in Firefox.
2. Choose **Load Temporary Add-on** and select `extension/manifest.json`.
3. Keep Burrata running and open a supported chart or playlist.
4. Click the extension icon, then **Import current page**.

Temporary Firefox installations disappear on browser restart. For packaged
extension distribution, see the
[companion instructions](../extension/README.md) for Chromium and other platform setup.

If startup reports that port 1420 is occupied, another Vite development server is
already running. Stop the duplicate Burrata development server before relaunching;
do not terminate unrelated processes.

## Import workflows

### From the browser

Open the actual chart or playlist, complete any provider verification, and let the
tracks render. **Import current page** captures supported public fields and forwards
the snapshot to the running app through native messaging.

### From Burrata

Enable **app-driven imports** in the companion and grant its provider-site permissions.
In Burrata, open **+ Import**, paste the chart or Spotify playlist URL, and choose
**Import URL**. The extension polls for app requests while enabled. If verification
is necessary, finish it in the browser and click **Import current page** to complete
the pending request.

### Without the companion

In **+ Import**, choose **Tracklist**, **CSV / Spotify export**, or **Browser snapshot
JSON**, paste the content, and select **Import tracks**. Use **Import M3U / M3U8 file**
to select an extended playlist file. Entries need `#EXTINF` names in `Artist - Title`
form; mix/version text in parentheses is retained. M3U entry paths and URLs are not
opened or saved as local-file references. Tracklists use
`Artist - Title (Version)`. Standard CSV requires `artist,title`.

Spotify CSV aliases include Track URI, Track Name, Artist Name(s), Album Name,
Album Release Date, Album Image URL, Track Duration (ms), and ISRC. Milliseconds
are converted to seconds. Numeric Spotify key values remain provenance rather
than being written as musical-key names.

### Library sources and import dates

The Library sidebar groups imported Beatport, Traxsource, and Spotify captures by
source. Open a group to revisit a saved chart, playlist, or album; Beatport and
Traxsource groups also link to chart browsing. Personal collections remain in the
Playlists section. When viewing an imported capture, **Import date** switches
between saved versions of that same source URL or shows all dates together.

The Library's **Local** source displays discovered M3U/M3U8 files directly in the
page. It scans folders saved under **Settings → Playlist folders**, your saved
music directory, and `~/Music` when opened, when settings change, and when the app
regains focus. Use **Scan folders** for a manual refresh. Root availability and
scan warnings are shown with the results. Each playlist shows its path and entry
count, including URL entries. **Open playlist** opens or creates its Burrata
collection idempotently; **Refresh import** imports a newer version, and the
original file stays in place.

## Saved data and completeness

Browser imports create an immutable dated snapshot and a saved collection associated
with the source URL. **Saved browser imports** reopens captures; collections remain
available after restarting the app.

- Charts retain explicit source ranks; ranks are not invented from arbitrary array order.
- Spotify uses playlist positions, separate from chart ranks.
- Repeated recording IDs are deduplicated, with occurrence positions retained in provenance.
- Captured-entry count, unique-recording count, expected total, and warnings are shown separately.
- Completeness requires a known total and continuous captured positions matching that total.
- Missing totals, stopped scrolling, or absent rows do not prove a complete capture.
- Partial reimports add members without removing previously imported members.
- Complete reimports synchronize the source collection, including removing missing members.

Use a separate personal collection for independently curated additions. Explicit
manual recording merges survive reimport; automatic matching across different
providers is not established by a similar title alone. Missing metadata stays missing.
Browser-captured identities remain unverified until checked against reliable metadata.

Capture waits up to eight seconds for supported rows. Spotify capture scrolls for at
most 25 seconds and restores the original position. Large playlists, localization,
pagination, or website changes can still produce partial captures.

## Beatport failure and latest repair

The reported page was:

`https://www.beatport.com/genre/afro-house/89/top-100`

1. The initial parser returned **No supported track rows found**.
2. Version 0.1.1 expanded selectors and loading handling, but the live page still failed:
   101 track links, 199 row candidates, and one metadata script were detected.
3. Version 0.1.2 added **Save parser diagnostic**, a local sanitized-row export.
4. The user's actual row showed the source rank inside artwork controls, the mix name
   nested inside the title, and BPM/key combined in one cell.
5. Version 0.1.3 parses those fields and the `.cell.date` release date.

The real sanitized fixture now yields rank **1**, title **Jamaican (Bam Bam)**,
version **Extended Mix**, artists **HUGEL, SOLTO (FR)**, label **MoBlack Records**,
BPM **122**, key **A Major**, and release date **2025-11-07**. Duration is absent
from that row and remains unknown.

The saved diagnostic showed 100 track identities in the surrounding table. The
latest parser passes against the actual row and a multiple-row regression variant;
a successful full 100-track browser import has **not yet been confirmed**.

To use the latest fix, click **Reload** for the companion in `about:debugging`,
refresh the Beatport tab, then select **Import current page**. No desktop rebuild or
native-host registration rerun is required for these extension-only updates.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Native host missing/disconnected | Keep Burrata running; check that host registration points to an existing executable; reopen the extension after registration |
| App-driven import waits indefinitely | Enable app-driven mode, check site permissions, then use **Refresh connection** in the app |
| Sign-in or verification shown | Complete it in the normal provider tab, then import that page |
| Tracks visible but capture finds none | Reload the companion and provider tab; save a parser diagnostic if it still fails |
| Import marked partial | Review counts/warnings, allow more rows to load, then reimport; previous collection members are retained |
| Temporary Firefox add-on missing | Load `extension/manifest.json` again after browser restart |

**Save parser diagnostic** saves `burrata-<provider>-diagnostic.json` locally. It
contains one sanitized public row and structural selectors, excluding cookies,
scripts, form fields, arbitrary attributes, and URL query strings. The export sends
nothing to the native host or a server. Share that file when reporting a parser
failure so the actual layout can become a regression fixture.

## Backend and security work completed

- Persistent, bounded native-messaging framing and authenticated loopback forwarding.
- Strict typed metadata validation; unknown privileged fields such as `localPath` are rejected.
- Browser metadata cannot initiate downloads or filesystem imports.
- Private bridge discovery file and no browser CORS access.
- SQLite v2 migration, transactional imports, collection updates, and historical captures.
- UI connection state, URL/JSON imports, saved captures, counts, and warnings.
- Versioned capture listeners, stale-capture cancellation, and fresh parser loading.

The protocol is described in [browser-protocol.md](browser-protocol.md). Main files:
[parsers.js](../extension/parsers.js), [content.js](../extension/content.js),
[background.js](../extension/background.js),
[browser_bridge.rs](../src-tauri/src/browser_bridge.rs), and
[storage.rs](../src-tauri/src/storage.rs).

## Validation and remaining work

Version 0.1.4 fixes Spotify positions when accessibility row indexes include a
column header, recognizes singular playlist totals, and detects verification
headings without mistaking ordinary track titles for challenges. Chrome pending
verification requests survive service-worker restarts through session storage.
App-driven imports open dedicated tabs; closing one reports failure to Burrata.
`npm run extension:chrome` prepares the loadable Chrome variant.

The latest checks passed 21 companion tests, 73 Rust tests including the separately
run loopback forwarding test, 18 frontend tests, seven contract checks, type checking,
the production frontend build, and the desktop build. A live YouTube search using the backend's
configured flags returned ten public candidates with yt-dlp 2026.08.19 and Deno.
YouTube search is accessed from a selected track's **Find YouTube & SoundCloud
matches** action.

Live chart and Spotify validation remains outstanding: this chat's computer-use
inventory exposed only the in-app browser and MCP Apps, and requesting Chrome
reported `Browser is not available: chrome`, including after refreshing the tool.
No source-site challenges were opened or completed during this run.
Chrome native-host registration was initially rejected by automatic approval review.
After the user explicitly approved its scope, registration succeeded at
`~/.config/google-chrome/NativeMessagingHosts/org.burrata.browser.json` for companion
ID `gmiknopfepcnfapmogmmemghmcfhdgcd` and the rebuilt local Burrata executable.
This enables the Burrata bridge; it does not connect Chrome to Codex computer use.
Chrome remained absent from computer use's browser inventory after registration.

After the user explicitly selected **@Chrome**, Chrome became available. The
example Beatport chart rendered all ranks 1–100 and the example Spotify playlist
displayed a total of 96 songs with public track rows. Traxsource initially showed
verification, then reached the chart page without an automated challenge action.
These observations confirm page access, not a completed companion import.
Chrome's URL policy blocked `chrome://extensions/`, so companion installation
could not be inspected or changed through browser control. Subsequent source-page
inspection reported an unattached debugger or timed out; the three original source
tabs were marked for continuation. The user was asked to check/load companion
0.1.4 manually from `.cache/browser-companion-chromium`.

Completed checks include 13 extension/parser/installer tests, the real sanitized
Beatport row fixture, synthetic compiled-UI import checks, storage migration/reimport
tests, and a Linux desktop/native-host smoke import into SQLite. The existing Rust
suite passed 70 tests, plus the separately run loopback forwarding test.

Still required:

- Confirm the full live Beatport chart import after version 0.1.3.
- Validate complete Traxsource and real Spotify captures, including larger playlists.
- Check metadata coverage, repeated imports, rankings, and refresh behavior on real providers.
- Qualify Chromium capture and permanent extension installation/distribution.
- Continue the imported-track → source preview → authorized acquisition → tags → Mixxx acceptance workflow.

## Track and media qualification follow-up

After companion 0.1.4 was loaded, Burrata was started from the existing built
binary with the built frontend served locally. The saved `browser_imports` table
was still empty at the verification checkpoint: no successful live import from
any of the three providers has been established. Chrome source-page control was
timing out, so the user was asked to trigger **Import current page** in each
companion popup for database-level verification.

Live yt-dlp checks with the configured Deno runtime returned ten YouTube search
candidates and inspected an individual official audio URL. Inspection returned
five AAC/Opus audio-only formats. These were metadata-only checks; acquisition of
remote audio has not been qualified by this run.

Stronger tagging tests exposed and repaired a real MP3 artwork failure: with
Lofty's ID3v2.3 writer, an absent picture description produced a malformed UTF-16
terminator. Explicitly setting an empty description preserves ID3v2.3 and fixes
reopening. The actual Rust round-trip test now passes artist, title/version, album,
genre, BPM, key, and exact embedded artwork bytes across MP3, FLAC, Opus, M4A,
WAV, and AIFF, using generated audio and artwork. M4A retains picture bytes while
reporting the generic picture type.

Candidate previews now reject obsolete start generations before replacing a
newer session. Stale responses stop only their own session, preventing a cancelled
start from leaving a player active or stopping a newer preview. Pure ordering and
session-matching tests, the complete Rust suite (74 passed, one separately opt-in
loopback test), 19 frontend tests, three player tests, type checking, and the
frontend build passed. Actual audible playback in Linux WebKitGTK is still an
outstanding live acceptance check.
The desktop binary was subsequently rebuilt successfully with one compiler job
and restarted with these fixes. Chrome was rediscovered under a new browser ID,
but claiming the live Beatport tab still reported an unattached debugger. The
three source tabs were marked for continuation; live companion import and audible
preview results still need user-triggered checks.

## Downloads diagnostics and version 0.1.5

The user's Spotify diagnostic row parses correctly: position 1, Jamaican (Bam Bam),
HUGEL and SOLTO (FR), album, and 156-second duration. The actual saved import has
positions 1–31 of the provider's 96 songs; its title was incorrectly “Your Library.”
The row-only diagnostic does not identify the live scroll container. The previous
capture excluded `overflow:hidden` viewports, a likely cause of reading only the
initial virtualized window. Version 0.1.5 probes programmatically scrollable
ancestors, including OverlayScrollbars viewports, selects a track-linked row in
main content rather than a sidebar/header, and waits for quiet samples at the
actual bottom. It retains the 25-second limit, restores scroll position, and never
labels a partial capture complete. Diagnostics now include bounded scrollbar
dimensions/overflow metadata for diagnosing any remaining partial capture.

The newer Beatport diagnostic (`burrata-beatport-diagnostic (1).json`) uses
`tracks-list-item` and `__ItemName-`, unlike the previously supported table layout.
Version 0.1.5 supports that layout and explicit numeric rank buttons (text or
numeric accessible label), with separate title/mix extraction. The old sanitized
diagnostic omitted buttons, so it cannot establish the rank by itself; ranks are
not synthesized from order. New diagnostics preserve numeric rank buttons while
excluding action attributes and nonnumeric buttons.

The actual Traxsource import saved all 100 tracks with ranks 1–100. Its old snapshot
remains marked partial because the capture did not expose a total. The parser now
also reads an explicit “TOP 100” from the provider document title, allowing a new
capture to establish completeness when every rank is present.

The playlist heading now comes from main content, and Spotify's presentation row
and its ARIA wrapper are parsed once. Regression coverage includes both supplied
sanitized rows and a simulated 96-track virtualized playlist with an unrelated
scrollable sidebar; it verifies capture and position restoration. The Chrome
package is regenerated in `.cache/browser-companion-chromium`. Reload the
companion in Chrome, refresh provider tabs, and retry imports. Full live Spotify
and Beatport results remain outstanding; no browser was connected to computer
use during this diagnostic investigation.

## Saved tracks appearing empty in the app

The user's database contained 138 canonical recordings and two collections with
131 members, while local audio assets were empty. Library previously required a
local audio asset, and startup did not restore any saved chart. This made existing
metadata imports hard to find. Library now lists canonical saved recordings
without requiring acquired audio. The regression test verifies metadata-only
recordings appear and explicit recording merges still deduplicate them.

Charts now restores the newest browser import on startup and exposes a saved-import
selector in its normal view. Collections defaults to the most populated collection.
Opening an import updates the active navigation context, and delayed startup loads
do not replace a view chosen by the user. Settings and dependency diagnostics load
independently from saved content so they cannot hold the track table empty.

## One-click playback and UI follow-up

Clicking a song now starts playback immediately; optional delayed playback applies
to arrow-key selection. Selection no longer requests storefront details. Preview
resolution checks verified local assets and safe cached audio URLs, then discovers
a plausible YouTube match with SoundCloud fallback. It does not call Traxsource
or Beatport, so their HTTP 403 pages cannot block this playback path.

The official embed opens in a compact Burrata Player window and attempts playback
on readiness. Linux disables the user-gesture media requirement only in that
isolated window. Provider autoplay blocks retain a usable Play button; unavailable
embeds try another plausible result, with at most two YouTube attempts and three
total attempts so SoundCloud can be tried. Previewing does not acquire audio or
verify a recording's version. Older requests cannot replace newer playback.

Preview search cancels superseded subprocesses, caches successful results (including
empty results) for five minutes, and has a 20-second deadline per provider. Trailing
display durations are removed from version search terms; an empty version-specific
query retries artist/title while preserving version-conflict checks.

A live metadata-only search for the saved Spotify song HUGEL, SOLTO (FR) — Jamaican
(Bam Bam) returned a plausible MoBlack Records YouTube result at 157 seconds versus
the catalog's 156 seconds. Its score is 0.8 / review because the catalog version is
unknown. The imported Traxsource song Tonite returned no plausible match; unrelated
results are rejected rather than played. No remote audio was downloaded.

The UI calls saved collections **Playlists**, gives native dropdowns explicit
dark/light option colors, and reduces sidebar, toolbar, and detail spacing while
preserving virtualized row dimensions. Playlists are saved track lists; browser
imports also create source-associated playlists.

Validation: 87 Rust tests passed (one existing opt-in loopback test ignored), 33
frontend tests, five player tests, seven contract checks, zero Svelte/TypeScript
diagnostics, and successful frontend/desktop builds. Coverage includes one-click
preview without catalog details, cached audio, stale song switches, unavailable
YouTube-to-SoundCloud fallback, autoplay blocks, cache reuse, and search cancellation.
Actual audible Linux WebKitGTK playback and visual dropdown inspection remain live
acceptance checks: computer use exposed no connected browser during this follow-up.

## Faster audio-only playback and original source links

The user confirmed the previous automatic embed played, but started slowly and
opened a video window. Automatic previews and candidate **Play audio** buttons now
resolve an audio-only stream through yt-dlp and use the main audio element. They
do not launch the embedded video player. AAC/M4A is preferred, with an audio-only
fallback. Signed stream URLs remain in memory rather than the library or provenance.

Preview searches now request three candidates; manual source searches retain ten.
Chart, playlist, snapshot, search, and saved-import loads prepare the first three
tracks, and selection prepares the next two. Candidate searches and stream resolution
share per-key locks and bounded caches, avoiding duplicate extraction when a click
overlaps preparation. Frontend preparation expires after four minutes; backend caches
expire within five minutes. Only one background preparation batch runs at a time.

Each track row has an **↗** source action, with an **Open on Beatport / Traxsource /
Spotify** button in details. It uses the original track URL and falls back to the
captured chart/playlist URL when a track URL is unavailable. Preview and download
sources do not overwrite the catalog destination. Clicking source does not play the
track. Entries with no web source do not invent a provider link.

The live Jamaican (Bam Bam) check took 2.728 seconds for search and 3.554 seconds for
audio stream resolution. Its resolved stream accepted a range request with HTTP 206
and audio/mp4 content. This qualifies stream access; cold clicks still depend on
provider latency, while prepared clicks reuse the completed metadata work.

The same AAC stream played in Chrome through a temporary audio-only test page:
43.9 seconds of advancing playback, 156.48-second duration, readyState 4, and no
media error. The test tab was closed and the page removed. Native WebKitGTK remains
a live acceptance check. Final checks passed 92 Rust tests (one existing opt-in
loopback test ignored), 36 frontend tests, seven contract checks, type checking,
and frontend/desktop builds. The development watcher restarted the desktop app.

## Search, download review, playlists, and metadata

Search now supports the user's query directly on YouTube and SoundCloud, with
provider toggles and a separate catalog-search action. Each source returns up to
ten unranked tracks. Exact source candidates remain in provenance for playback,
inspection, and downloads, avoiding another artist/title lookup. Uploader names
do not become artist tags, and upload dates do not become musical release dates.
Source search identities stay unverified.

Each row has left-side Play and Download controls. Download opens a modal chooser
with source title/uploader/duration, heuristic match scores/reasons, available
audio codec/bitrate/sample rate, output profile, destination, and the exact outgoing
tags. Scores are not presented as probabilities. Uncertain source/mix matches
require explicit confirmation. Both row and detail download buttons use this flow;
opening the chooser does not enqueue a job. The chooser supports Escape, keyboard
focus containment, and restoring prior focus.

The chooser refreshes persisted settings and saves a typed destination on explicit
queue confirmation. Batch downloads also refresh settings instead of blocking on
stale frontend defaults. The normal app database had no persisted settings row at
the investigation checkpoint; destination status now distinguishes saved paths
from values that will be saved when queued.

Personal playlists can be created independently, including creating and adding the
current selected/checked tracks without leaving their view. Playlist names and
memberships can be edited separately; storage tests prove independent memberships
after reopening and preservation of original import snapshots. Clicking the
now-playing title opens its details in Library without restarting playback.

Downloaded files receive selected recording metadata rather than stale source
upload tags. Generated-file tests read back artist, title/mix, album, label, genre,
BPM, musical key, full release date, ISRC, catalog number, and artwork bytes across
MP3, FLAC, Opus, M4A/AAC, WAV, and AIFF. A real writer gap dropped MP3 release dates
with ID3v2.3; ID3 formats now use v2.4 for complete dates. Very old v2.3-only players
may need separate compatibility qualification. These tests verify storage, not
the identity of the downloaded recording.

When BPM is missing, FFmpeg-decoded PCM is analyzed for a regular onset pattern.
Successful estimates and confidence are recorded in `bpmAnalysis`; missing or
unreliable results do not invent BPM. Existing catalog BPM stays unchanged.
Estimated values are labeled in the UI and are not key/beat-grid analysis.

Settings now explains tools, checks their availability, links official installation
guides for Windows/macOS/Linux, and hides executable paths under advanced options.
Tool discovery supports common desktop-launch locations. See [tool setup](tool-setup.md).
The release packages include the supported media and recognition tools. See
[tool setup](tool-setup.md) for their roles and troubleshooting.

Live direct searches returned ten YouTube and ten SoundCloud results; individual
source inspection returned audio-only formats. The progressive SoundCloud stream
played in Chrome to 13.208 seconds of its 249.208-second duration with no error.
No acquired remote audio files were published during these checks. Final checks:
101 Rust tests passed (one existing loopback test ignored), 42 frontend tests,
seven contract checks, clean type checking, and frontend/desktop builds. Native
desktop playback and a complete remote acquisition into the user's library remain
live acceptance checks.

## Explicit YouTube result rejected by catalog matching

The reported Bedouin, Hiya — Salaam (Francis Mercier Remix) result includes extra
title separators, hashtags, and emojis. Its flat source metadata has no artist,
so the general search parser correctly leaves artist unknown. The frontend then
incorrectly applied artist/title catalog matching to that explicitly selected
source and rejected it before trying playback.

Audio-search results now play their attached valid source directly, regardless of
catalog match score or missing artist. This does not verify recording metadata.
Failures preserve the resolver/media reason and do not silently substitute another
recording. Catalog previews continue using conservative automatic matching.
Regression coverage includes unknown artists, decorated titles, low scores,
invalid links, playback errors, and stale requests. The frontend suite passed
46 tests with zero Svelte/TypeScript diagnostics.

The exact reported YouTube result (`lVAccRyVoJI`) resolved to progressive M4A/AAC,
returned HTTP 206 for a byte range, and played in Chrome to 19.347 seconds of a
343.399-second duration with readyState 4 and no media error. The temporary test
page was removed. Native desktop playback remains a live acceptance check.

## Library and playback update — October 12, 2026

Search and Library are the main destinations. Library filters by Spotify, Beatport,
Traxsource, Local, or Personal; Liked Songs is the favorites playlist. Downloads and
playing-track information open panels without replacing the current page. Back and
forward restore artist selection, section, import version, and list position.

Spotify playlist and album imports support localized URLs and group captures by
canonical source URL. The newest dated capture opens by default; older versions
remain selectable. Repeated playlist entries retain their occurrence positions.
Imported snapshots are immutable; editing creates a personal copy.

Artist releases default to Albums, with source links, scrollable tracklists and
provider-specific pagination. MusicBrainz release-group types and Discogs formats
are mapped consistently. Artist section tabs keep their height with long lists.
Optional Last.fm top tracks still require a configured key.

Playlist discovery checks configured folders, the music folder, and ~/Music.
It does not automatically scan OneTagger's configuration directory. Importing an
existing local file is idempotent; refresh creates a new retained version. Local
files are registered read-only. All playlists mirror to music-folder/Playlists as
M3U8 files, with dated source archives. Entries prefer registered local audio,
then a remembered public audio URL, then a ytdl://ytsearch1: search expression.
These search expressions target mpv/yt-dlp-compatible players.

Mirrors synchronize in both directions through a filesystem watcher plus a
periodic fallback. Concurrent app/file changes preserve a conflict copy. Deleted
mirrors pause until explicitly restored. Order and repeated entries are retained.

Playback queues continue automatically, support next/previous, play-next,
shuffle/repeat and reordering, and retain position without autoplay after restart.
Unavailable automatic starts are skipped with a bounded retry count. Listening
history records successful audio starts and deduplicates pause/resume events.
Successfully played YouTube/SoundCloud candidates are remembered for playback and
playlist mirrors, without promoting the recording match to verified metadata.

YouTube chapter downloads are optional. Selected chapters become separate tagged
files and a personal playlist; the unsplit parent is not also published by default.
Chapter labels are unverified unless a unique compatible catalog suggestion is
found. Missing BPM/key analysis runs on published files, and estimates remain
labeled. Cover art prefers registered embedded pictures, then cached catalog or
source artwork, with alternate YouTube thumbnails on failure.

Automated verification includes generated audio/tag readback, chapter publication
and retry, provider cursor exhaustion, history, queue persistence, source snapshots,
ordered duplicates, mirror conflicts, embedded artwork and extension album parsing.
Chrome UI checks use a clearly labeled isolated fixture with public cached Big Thief
provider data; those layout checks are separate from live native import/playback.
Direct Traxsource requests can still return HTTP 403, requiring browser capture.

Consolidated checks for this update: 159 native tests (including the loopback
fixture), 73 frontend tests, 28 extension tests, and 5 player contract tests passed.
Svelte/TypeScript reports zero errors or warnings.

Live restart verification: the embedded desktop build opened the existing library
with 338 catalog entries and four local assets. The reloaded Chrome companion
captured Spotify's Big Thief — Masterpiece album and the native bridge stored a
complete 12/12 album snapshot; its Spotify M3U mirror contains 12 entries. Existing
complete Beatport and Traxsource imports retained 100 entries each. The older
31-entry Spotify playlist capture remains partial, rather than being relabeled.
FFprobe readback of all four registered audio files found artist/title, BPM and key
fields; estimated BPM/key values do not establish recording identity or beat-grid
accuracy. The pre-update SQLite backup is /tmp/burrata-before-update.sqlite.

### Source groups and local control follow-up

The sidebar now nests imports under collapsible Beatport, Traxsource, Spotify and
Local groups. Source filter dropdowns, the Personal heading, Local first label and
import-count banner were removed. An imported playlist selects one import date or
All import dates; copying it preserves the displayed version. Queue, playing-info
and download panels are exclusive and clear the measured player height.

The missing `sync_playlists` command was an IPC permission/registration inventory
bug. The build manifest and capability checks now cover every invoked command.
Startup/sync automatically registers new readable local playlists, keeping order,
repeats and exact validated YouTube/SoundCloud URIs. The actual four ~/Music/playlists
files are present with 3, 37, 14 and 25 entries. Optional provider failures no longer
hide successful artist results; live Big Thief searches and album links work.

Local M3U discovery also reads ordinary `# Artist - Title` comments immediately
before an entry, with standard `#EXTINF` metadata taking precedence. These labels
remain unverified playlist metadata. An unchanged source can repair existing
URL-only placeholders offline without changing their IDs, memberships or order;
manual edits and already enriched metadata are retained. A later bare-URL import
does not erase known metadata for the same URL.

The local Opus player uses authenticated loopback HTTP instead of relying on
WebKit's custom asset-scheme media handling. Live MCP checks played all four local
recordings directly; single-run start times were 551–872 ms. Playback recovery
is awaited before MCP acknowledgment, and timed-out requests cannot start later.
The application is rebuilt with embedded assets, independent of the Vite server.

Current verification: 176 native tests including actual loopback range/auth checks,
79 frontend tests, 28 extension tests, 5 isolated player tests, 7 contract checks and
3 SDK MCP tests passed. MCP Inspector and the live SDK acceptance workflow passed.
The pre-MCP database backup is /tmp/burrata-before-mcp.sqlite.

### Artist-name search pagination correction

Clicking Gillian Welch from an audio result reached artist search page 2 because
the UI sent one-based page 1 and the desktop command incremented it again.
The saved GUI provider cache confirmed `gillian welch:2:20`, containing unrelated
Gillians, while the MCP path correctly returned Gillian Welch on page 1.
The desktop adapter now forwards one-based pages unchanged and defaults to page 1;
Load more no longer skips alternating pages. The artist Releases tab is now labeled
Discography, and explicit artist searches start there. Artist headings use the
resolved provider profile name even when navigation begins with just a provider ID.
A native command-adapter regression and the frontend artist paging/discography
regression passed; all 80 frontend tests and command capability checks passed.

### Repeated release-track rendering correction

Gillian Welch still failed after the paging correction because the release UI used
`position + title` as a Svelte keyed-list identity. A real live release repeats
Caleb Meyer without positions, producing duplicate `nullCaleb Meyer` keys and
aborting the discography render. Release and metadata-review tracklists now use
ordered occurrence indices, retaining repeated songs and repeated disc positions.
A Chrome reproduction using the cached public Gillian Welch profile confirmed the
original exception, then the fixed artist-name → profile → Discography path rendered
49 album entries. The fixture remains available with `?artist=gillian`. The duplicate
track regression and all 81 frontend tests passed; the desktop build was restarted.

### Playable discography and queue controls

Artist discographies now group same-title/type editions into album cards with an
explicit edition selector. Album titles expand ordered track rows; Play album starts
at the first track, clicking a row starts at that album offset, Queue appends, and
Play next inserts the whole album after the current item in order. Artist Tracks,
Top tracks and Local views also expose playback and queue actions. Less-used source,
download and playlist actions sit under More.

Filters cover loaded release titles/track names, type and provider. Album chronology
uses the earliest known loaded edition year, with unknown dates last. Tracks support
search, availability and source/title/date/duration sorting. Load more retains provider
cursors; filter counts describe loaded metadata, not the complete remote catalog.

The new shared get_release_tracks service resolves exact MusicBrainz/Discogs editions,
release-group representatives and master main releases. It retains ordered discs,
repeated occurrences, per-track credits, durations and source metadata. Complete cached
MusicBrainz release lists seed detail caches. Catalog IDs satisfy Store's canonical
identity rule and persist for playlist/queue/MCP use. The get_release_tracks MCP tool
exposes the same service. Unique exact local artist/title/version/duration matches can
supply playback audio without merging catalog identities; ambiguous matches stay remote.

Verification: 180 backend tests, 90 frontend tests, seven IPC/schema contracts and SDK
transport tests passed; generated release tracks also pass Store save/reload checks.
Live Time (The Revelator) lookups returned ten ordered tracks from each provider. Native
source discovery returned twenty YouTube/SoundCloud candidates for Revelator. Chrome
used real public provider fixtures with a mocked desktop transport to verify filtering,
expansion and album queue order. A real playback test that would replace the user's queue
was rejected by automatic review and was not run; isolated UI/automated playback tests
were used instead. No native queue mutation was issued by these checks.

### Compact desktop layout (2026-10-12)

The left sidebar remains beside content at every width: 200px on large windows,
168px at intermediate widths, and a 64px navigation rail below 800px. The rail's
Menu drawer exposes playlists and source groups and stops above the actual player
height. Track lists use one virtualized table at every width, with optional album
and BPM/key columns disappearing as space shrinks. Download remains directly
available; queue, source, playlist, selection, and information actions live in a
keyboard-dismissible track menu. Playing a row does not automatically open details
or expose selection/playlist editing controls. Library maintenance controls are
collapsed under Library options.

The player uses artwork and track identity on the left, transport and seek in the
center, and queue/volume/information on the right, with a compact two-row layout
below 650px. Removed conflicting legacy rules that hid the table and replaced the
player grid on narrow windows. Chrome browser fixture QA checked 390, 760, 960,
and 1440px widths without horizontal overflow, rail playlist access, Escape menu
dismissal, and Play next. Browser fixture QA uses mocked desktop services; it does
not verify real audio playback or downloads.

Validation: Svelte check has zero errors/warnings; all 90 frontend tests pass,
including adapted menu interactions retaining their underlying service assertions.
Production assets build successfully. Actual desktop playback was not changed by
Chrome fixture QA.

### Playlist actions and numbering (2026-10-12)

Track row numbers follow the displayed list order, independent of provider chart
ranks or playlist-position metadata retained on library tracks. This fixes library
rows all showing 1 after combining tracks from multiple sources. Right-click a
track for the same actions as its ellipsis button. Right-click a playlist in the
sidebar to open or delete it. Context menus stay within the viewport above the
player and close with Escape.

Editable playlists support direct track removal and batch removal. Editing a
protected local/source playlist creates an editable copy and switches to that
copy, preserving the original source. Playlist deletion hides the collection and
source imports without deleting recordings, audio files, or original playlist
files. Restore with Undo deletion or Library options → Deleted playlists. Deleted
source bindings remain recorded so routine sync does not recreate a deleted
playlist; restoration retains the same ID and ordered entries.

Validation: 98 frontend tests; backend suite 185 passed, one existing ignored
local-listener test. Browser fixture QA verified numbering, anchored track and
playlist context menus, soft deletion, and restoration without changing real user
playlists. Linux MPRIS implementation is documented in `docs/mpris.md`.

Last.fm read-only catalog and chart requests use the API key configured in
Burrata's Connections settings. They do not require the shared secret used for
write operations. Provider responses and chart availability can vary over time.

## Preset sidebar browsers

Shazam, Last.fm and Bandcamp now have permanent collapsible sidebar sections,
even before the first import. Choose **Browse charts** (Shazam/Last.fm) or
**Browse music** (Bandcamp) to open preset controls; URLs are generated for you.

- Shazam: Global/country Top 200, Viral, and 18 genre categories. Region and genre
  routes were read from the official Chrome dropdowns. Top 200 is imported directly;
  unsupported direct chart variants use the companion browser for capture.
- Last.fm: global tracks, ten genre/tag presets, and profile top tracks for six
  periods. Profile username is remembered locally. Tag charts call the documented
  `tag.getTopTracks` API, whose JSON response uses `tracks` (user charts use
  `toptracks`). Tags describe chart membership and are not assigned as track genre.
  Saved profile links use the browser's `date_preset` query for the chosen period.
- Bandcamp: curated official Discover genres, including electronic, jazz, R&B/soul,
  funk, folk, world and Latin. Browse in the OS default browser, choose an album,
  then capture its tracklist with the companion. Discover album cards aren't
  imported as songs. The R&B/soul route uses `r-b-soul` from Bandcamp's own menu.

Preset imports retain the same dated versions, sidebar grouping and source-refresh
links as URL imports. Provider browsing and import navigation support Back/Forward.

Live validation of the configured Last.fm Afro House tag preset returned 200
tracks through the production parser; its unknown/unfetched remainder is marked
partial. Preset UI checks passed at 390 px without horizontal page overflow.

## Playlist workflow, Downloads and internet radio (2026-10-13)

The sidebar + creates an editable playlist immediately and opens its name field.
Imports are available from Library options or Import tracks inside a playlist.
Personal playlist sections can be created, renamed and removed without deleting
playlists; drag playlists into a section, or use its accessible Move control.
Shift-click selects a contiguous track range; Ctrl/Cmd-click toggles tracks.
Selections can be added to the queue, downloaded or added to another playlist.
Album and playlist download actions use the configured preferred audio source;
uncertain recording/remix matches are reported for review rather than silently
acquired.

All newly published downloads, reused downloaded sources and chapter outputs are
added to the stable Downloads playlist. Startup backfills existing managed assets
only when their file exists and their acquisition provenance is present. Existing
chapter playlists remain, but new sets do not automatically create another one.

New downloads follow the selected **Settings → Downloads → Folder layout**:

- Flat: `Music/Artist - Title [version].extension`.
- Artist and album: `Music/Album Artist/Album/Artist - Title [version].extension`,
  using the track artist if album artist is absent and `Singles` if album is absent.
- Playlist first: downloads started from a saved local playlist use
  `Music/Playlists/Playlist Name/Artist - Title [version].extension`; other downloads
  fall back to Artist and album.

Folder names are sanitized and publication remains collision-safe. The layout is
captured when a job is created. Changing the preference does not move existing
files or reorganize older downloads. The Downloads playlist remains a database/M3U
view rather than another file copy. Chapters follow the same chosen layout; MP3
exports carry an `[MP3 export]` suffix. Temporary job files live in
`.burrata-staging`, and source audio for chapter reuse in `.chapter-source-cache`.

Further layout proposals remain: catalog albums could use verified year and
track-order tags for `Music/Artist/Album (Year)/01 - Title.ext`; continuous sets
could use `Music/DJ Sets/DJ/Date - Set.ext`, with chapter exports in a sibling set
folder retaining DJ/set identity and chapter timing. These proposals require
separate implementation and qualification. Album identity and track-order tags
remain independent of folder names.

Tags and persistent IDs should remain authoritative; paths alone are insufficient
for compilations, collaborations and tracks that belong to several playlists.
A future migration needs a dry run, collision handling and updates to assets and
M3U paths, rather than moving existing files implicitly.

Radio stations are saved separately from music playlists and appear in a
collapsible sidebar subsection. Radio Paradise includes official FLAC stream
links and current program metadata/artwork; recently heard items are those
observed by the open view, not a fetched full station history. Additional stations
can be found with Radio Browser or added using a direct HTTP(S), M3U or PLS URL.
Station-directory codec/bitrate fields are station-reported, not a measurement of
current audio quality. Arbitrary stations do not yet expose ICY current-song
metadata or artwork. Saved station removal does not delete music files.

References: [Transistor](https://codeberg.org/y20k/transistor),
[Radio Browser API](https://docs.radio-browser.info/),
[Radio Paradise streams](https://radioparadise.com/listen/stream-links).

## Station details, history and identification

Station pages now use the selected station's name, a short homepage description
when available, its website and stream details. Known Radio Paradise channels and
The North 103.3 have station-specific description fallbacks. Metadata-confidence
notes are under Details. The player information panel shows the actual streaming
URL separately from the station website; the compact quality label is secondary
text.

Radio Browser search ranks normalized query tokens independently of order and
folds diacritics. It makes at most three bounded directory requests and expands
`MPR` to Minnesota Public Radio and its official service names. Search `MPR News`,
`The Current`, `YourClassical MPR` or `Radio Heartland` for a specific MPR stream.
The North 103.3's official stream is
`https://thenorth1033.streamguys1.com/live`.

Burrata persists identified song history per station. Only polling an actively
playing station records an observation; opening its details does not. Repeated
consecutive observations are deduplicated, while a song heard again later can
appear again. This is the user's observed history, not a full historical station
playlist. The latest identified song remains in history; a temporarily missing
match shows Last identified instead of clearing the station's track card. Stations without ICY or other supported metadata may have no entries
until a song can be identified.

Recognition runs automatically while a station is playing and supported stream
metadata is missing, including when browsing another screen. Attempts and cached
matches are spaced by 90 seconds; failed recognition does not interrupt playback.
Identify current song remains available. Release packages include Burrata's
portable recognition helper, which captures a bounded 12-second stream sample
locally and generates a fingerprint. Shazam matching sends the fingerprint to a
remote service. Recognition does not use the microphone. A no-match does not
create invented artist/title tags or history entries. Recognition results remain
separate from verified catalog identities.

Sidebar context menus support renaming playlists (including imported and built-in
entries) and radio stations. Radio context menus also offer Remove station:
saved entries are deleted and built-in stations are hidden persistently.
Persistent label overrides preserve stable IDs and
source snapshot titles; a refreshed import cannot erase a user-chosen label.

References: [The North Listen Live](https://www.thenorth1033.org/listen-live) and
[MPR streams](https://www.mpr.org/listen/streams).
