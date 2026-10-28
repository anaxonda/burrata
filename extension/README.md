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

First [install Burrata for your system](../docs/install.md) and open it. Download
an extension from the **same main release's Assets** on
[Burrata releases](https://github.com/anaxonda/burrata/releases).
The extension version is separate from the app version: app 0.1.3 ships extension 0.2.2.

### Firefox

1. Download `firefox-extension-<version>.xpi`. Keep it as an XPI; do not extract it.
2. In Firefox open `about:addons`, choose **Extensions**, open the gear menu and
   choose **Install Add-on From File…**. Select the downloaded XPI and approve
   Firefox's installation prompt. This is the Mozilla-signed release add-on,
   which stays installed after Firefox restarts.
3. Pin Burrata to the toolbar if desired, then [connect it to the app](#connect-to-burrata).

### Chrome, Chromium and Edge

1. Download `chrome-extension-<version>.zip` and extract it into a permanent folder.
   Do not delete or move this folder after loading it.
2. Open `chrome://extensions` (Chrome/Chromium) or `edge://extensions` (Edge).
3. Enable **Developer mode**, select **Load unpacked**, and choose the extracted
   folder containing `manifest.json`, not the ZIP or its parent directory.
4. Open the browser's Extensions menu and pin Burrata if desired, then
   [connect it to the app](#connect-to-burrata).

There is no Chrome Web Store installation in this release; Developer mode and
manual updates are required. A managed work/school browser may block this option.
See [Chrome's instructions](https://developer.chrome.com/docs/extensions/get-started/tutorial/hello-world#load-unpacked).

### Connect to Burrata

1. In Burrata choose **Settings → Connections → Browser companion → Connect
   browser companion**. This installs the bundled local bridge for your current
   user; you do not need to download a separate host or run a terminal command.
2. Fully quit and reopen the browser after connecting. Keep Burrata running.
3. Open a supported page, then the Burrata extension popup and choose **Import
   current page**. Reload a provider tab that was already open when you installed
   or updated the extension. Check the returned count and any incomplete warning.

**Browser host registered** means the local helper was registered; it does not
prove the extension is installed or connected. If the popup says **Native host
disconnected**, repeat these steps. Install the app in its permanent location
before connecting (Applications on macOS; a fixed folder for AppImage).
Registration explicitly covers Firefox, Chrome, Chromium and Edge. Other browser
variants and Snap/Flatpak browser packages are not qualified. See
[system-specific troubleshooting](../docs/install.md#updates-and-common-problems).

### Connections and account access

Manual page import needs no provider pairing code. In the popup's **Connections**,
**Browser-assisted access** is an optional alternative for app-driven provider
work; it asks for additional site permissions. Complete any provider sign-in or
verification page yourself. The app's preferred experimental Linux/macOS account
connection is in **Settings → Connections**; Windows does not have that in-app
session transport in this release. See [account setup](../docs/install.md#spotify-and-youtube-accounts).

The extension's YouTube session handoff is a separate, explicitly consented
one-time pairing flow. It does not pair Spotify and is not required to import a
public YouTube video/playlist from the current tab. Spotify public-page capture
is also independent of an account connection, but only the rows the site makes
available can be imported.

### Updates

- Firefox: install the newer signed XPI through **Install Add-on From File…**.
- Chrome/Edge: replace the files in the same permanent extracted folder with the
  newer ZIP contents, then choose **Reload** on Burrata's extension card.
- After either update, reload provider tabs. After an app update or move, use
  **Connect browser companion** again and restart the browser if disconnected.

The local bridge is a compiled console helper and does not open another Burrata
window. Windows registration uses per-user registry entries. Page imports do not
transfer cookies; the separately approved YouTube session handoff has different
access and consent.

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
- **Preview changes** reads the current page without saving tracks and shows additions,
  removals, moves and warnings. **Apply reviewed import** commits that exact preview;
  changed sources require a fresh preview. **Import current page** remains available.
- Bandcamp imports now retain artwork, page links and available public release
  descriptions/credits/tags. Other parsers retain available artwork, source links,
  separate remix credits, explicit availability and public counts. Missing values
  remain missing; source evidence does not prove recording identity.
- Import receipts distinguish captured occurrences from unique recordings. Failed
  Bandcamp album pages can be retried separately from Burrata's import details.
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

Source-only setup: Firefox can use **Load Temporary Add-on** in
`about:debugging#/runtime/this-firefox` with `extension/manifest.json`; that install
is removed on restart. For Chromium, run `npm run extension:chrome` in the source
checkout and load `.cache/browser-companion-chromium`. Release users do not need
Node.js, npm or a source checkout.


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
