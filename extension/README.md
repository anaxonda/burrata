# Burrata Browser Companion

Firefox first, with a Chromium Manifest V3 variant. Page imports extract selected
public chart, playlist, and album metadata from supported sites: Beatport,
Traxsource, Spotify, public YouTube videos and playlists, Shazam, Last.fm, and
Bandcamp. Page imports do not send cookies, tokens, audio, or the complete page.
Provider operations and the separate, explicitly consented YouTube session handoff
have their own access flows. Provider access restrictions require your interaction.

For the implementation history, latest Beatport fix, and validation status, see the
[chart and playlist import summary](../docs/chart-playlist-imports.md).

## Install

1. Open Burrata's browser integration settings and choose **Register native host**.
   Burrata installs its bundled stdio host and registers it for Firefox, Chrome,
   Chromium, and Edge for the current user. Reopen the browser after changing the
   registration. The app's integration status reports which registrations are ready.
2. Firefox: open `about:debugging#/runtime/this-firefox`, choose **Load Temporary
   Add-on**, and select `extension/manifest.json`. Temporary installs disappear when
   Firefox restarts. A permanent installation needs a Mozilla-signed release.
3. Chrome, Chromium, or Edge: run `npm run extension:chrome` to prepare
   `.cache/browser-companion-chromium`, open `chrome://extensions`,
   enable Developer mode, and choose **Load unpacked** (Edge uses
   `edge://extensions`). The committed public manifest key gives a stable extension
   ID; the preparation command prints it. No signing private key is included. Select
   the prepared directory. After source changes, rerun the preparation command and
   reload the extension.
4. Keep Burrata running. Open a supported chart, playlist, or album, click the extension icon, and
   choose **Import current page**. This grants temporary access to that tab only.
5. Optionally click **Enable app-driven imports** and grant the requested provider site
   permissions. Burrata can now ask the extension to load charts, playlists, or albums in inactive tabs.
   The extension brings verification pages forward. Complete verification, then
   click **Import current page** to finish that pending import. No challenge solving
   or authentication bypass is attempted.

The host is a compiled Burrata console executable with inherited standard input and
output, so native messaging does not open the desktop window. Windows uses per-user
native messaging registry entries. The Chromium extension ID comes from the committed
manifest key and remains stable across builds.

## Capture limits

- Capture waits up to eight seconds for rendered rows before reporting safe diagnostic
  counts. After updating the companion, reload it and the provider tab.
- Traxsource reads public track rows, preserving explicit ranks and excluding remixers
  from the main artist credits. Beatport reads explicit ranked track metadata in page
  JSON, or supported DOM rows. Unranked recommendation data is discarded.
- Spotify reads track rows and original positions, scrolling for at most 25 seconds
  and restoring your scroll position. Repeated track occurrences retain their original
  positions for app-side deduplication. Large, slow, or changed playlists and albums may need a
  repeat import and are reported incomplete.
- YouTube imports one public watch page as a single-track playlist, or captures the
  rendered rows of a public playlist while scrolling for at most 25 seconds. It uses
  visible title/channel/duration/artwork fields only; signing in is not required for
  current-page capture. Playlist occurrence order and repeated videos are retained.
  A missing count, unrendered row, or stopped scroll leaves the snapshot incomplete.
- Shazam chart capture reads ranked song records from page data or rendered chart rows.
  Last.fm supports its Top Tracks chart and a user's track library; chart ranks come from
  explicit row positions, while a paginated library remains incomplete unless all tracks
  and an explicit total are present.
- Bandcamp album capture reads the album's explicit embedded tracklist. Collection pages
  are imported only when they render actual song rows; album cards are not converted into
  tracks. Bandcamp Discover imports up to 20 rendered album URLs and then reads each album’s explicit public tracklist; failed albums produce partial-import warnings.
- Completeness requires an explicit provider total and every position from 1 through
  that total. A stopped scroll, absent total, or absent rows never proves completeness.
- Parsers depend on public website structure and English count labels. Localized or
  redesigned pages can fail or return partial metadata. Re-import after the page loads.
- Parser tests use synthetic fixtures. They establish parsing behavior for those shapes,
  not full live compatibility across providers. Real extraction and verification behavior
  still needs validation in a browser.

App-driven mode checks for requests at most every two seconds while enabled. Native
host errors appear in the popup and extension badge. Disabling it stops app-driven polling; a lightweight heartbeat keeps the manual companion connection status current.

## Development

```sh
node --test extension/tests/*.test.mjs
```

The protocol is documented in [browser-protocol.md](../docs/browser-protocol.md). Provider parsers are pure
ES modules; tests use jsdom, synthetic DOM fixtures, and a real sanitized Beatport chart row. After editing background
or manifest files, reload the extension in your browser's extension developer UI.

## Reporting a parser failure

If tracks are visible but capture finds none, click **Save parser diagnostic**.
This saves a JSON file locally containing one sanitized public row and its structural
selectors. It excludes cookies, scripts, form data, arbitrary attributes, and URL
query strings. Nothing is sent to Burrata or any server by this action. Attach the
file when reporting the failure so the actual provider layout can become a fixture.
