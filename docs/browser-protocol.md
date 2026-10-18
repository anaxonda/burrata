# Burrata browser bridge v1

Native host name: `org.burrata.browser`. Firefox extension ID:
`burrata-browser@burrata.local`. Native host uses Burrata's executable with
`--native-messaging`; host manifests must launch a wrapper because native host
manifest `path` has no argument support. Installer creates that wrapper.

Native messages are UTF-8 JSON prefixed by a four-byte little-endian length.
Maximum message size: 1 MiB. Replies have `ok: boolean`, with `error` on failure.
No account cookies, authorization tokens, audio files, or arbitrary filesystem
commands are transferred by the extension.

## Import request

```json
{
  "type": "import",
  "requestId": "optional app request UUID",
  "snapshot": {
    "schemaVersion": 1,
    "provider": "beatport",
    "kind": "chart",
    "title": "House Top 100",
    "sourceUrl": "https://www.beatport.com/genre/house/5/top-100",
    "capturedAt": "2026-10-11T23:00:00Z",
    "complete": false,
    "expectedTotal": 100,
    "warnings": [],
    "entries": [{
      "providerId": "12345",
      "sourceUrl": "https://www.beatport.com/track/example/12345",
      "artist": "Example Artist",
      "title": "Example Track",
      "version": "Extended Mix",
      "position": 1,
      "rank": 1,
      "durationSeconds": 360,
      "bpm": 124,
      "key": "Am",
      "album": null,
      "label": null,
      "genre": null,
      "releaseDate": null,
      "artworkUrl": null,
      "isrc": null,
      "catalogNumber": null
    }]
  }
}
```

Providers include `beatport`, `traxsource`, `spotify`, `shazam`, `lastfm`, and `bandcamp`. Kinds are `chart`, `playlist`, `album`, and Bandcamp-only `collection`.
Spotify playlist and album captures use their ordered track position, never a chart
rank. Album captures apply the album title and header artwork to each entry. IDs and URLs are validated
by the native app. Missing optional fields stay unknown. Captured entry positions
must be positive; chart ranks are retained only when exposed explicitly. Duplicate
recordings retain first position and all original occurrence positions as provenance.
Captures are never labeled complete solely because DOM extraction stopped producing
new rows. Known totals must match the capture; otherwise completion is unknown.

Import reply: `{ "ok": true, "result": BrowserImportResult }`. Result fields:
`id`, `provider`, `kind`, `title`, `sourceUrl`, `fetchedAt`, `collectionId`,
`capturedCount`, `importedCount`, `expectedTotal`, `complete`, `warnings`.
The app emits `browser-imported` with that result after transactional persistence.

## App-driven requests

Extension connects native port while browser is running, polls at most once per two
seconds while enabled: `{ "type": "poll", "providerProtocolVersion": 2 }`. Reply:
`{ "ok": true, "requests": [{ "id": "UUID", "url": "...", "provider": "..." }] }`.
The extension opens a dedicated inactive provider tab. A normal verification/login gate
is presented by bringing that tab forward; never solve challenges automatically.

When app-driven requests are disabled, the extension sends
`{ "type": "heartbeat", "providerProtocolVersion": 2 }`
over native messaging every two seconds. This keeps manual companion status current
without polling or opening provider pages. The popup status check also heartbeats.
The protocol version gates provider operations; older companions can still import
browser snapshots but cannot receive remote playlist operations.

## Provider operations (protocol version 2)

The app may queue a fixed `providerOperation` request through the same poll response:

```json
{
  "id": "request UUID",
  "kind": "providerOperation",
  "operationId": "correlation UUID",
  "provider": "spotify",
  "operation": "search",
  "args": { "query": "Example", "kind": "playlist", "limit": 25 }
}
```

Provider values are `spotify` and `youtube`. Operations are limited to `connect`,
`status`, `search`, `listPlaylists`, `getPlaylist`, and Spotify-only
`addTracks`, `removeTracks`, and `moveTracks`. The extension performs the operation
using the matching provider tab's existing session and returns a native
`providerResult` message with both `requestId` and `operationId`, `ok`, and either
`result` or `{ "code": "...", "message": "..." }` in `error`. The native app
correlates both IDs, bounds the wait and response size, validates provider IDs and
URLs, and rejects unsupported operations. Provider credentials and arbitrary
endpoints or scripts are never sent to the native app.

Bandcamp Discover does not expose song rows. The extension collects at most 20
rendered artist-hosted `/album/{slug}` links from the current `/discover/{tag}`
page and sends `importAlbumCollection` with the Discover URL, page title, and album
URLs. The native host validates each URL and reads each album's explicit public
tracklist with at most four concurrent requests, a 110-second overall bound, and
a 1,000-track cap. Failed albums become partial-import warnings. Album cards are
never converted into song entries, and the result remains incomplete.

Request status:
`{ "type": "status", "requestId": "UUID", "state": "loading|verification|failed", "message": "..." }`.
The app emits `browser-import-status` with `requestId`, `state`, `message`.
An import with `requestId` completes that request. Failures leave useful cached data.

## Desktop commands

- `get_browser_status()` → `{connected, pendingCount, lastSeen, hostName}`.
- `request_browser_import({url})` → request ID string.
- `list_browser_imports()` → BrowserImportResult[] newest first.
- `load_browser_import({id})` → Chart (snapshot). Playlist tracks have null ranks.
- `import_browser_snapshot({snapshot})` → BrowserImportResult (also JSON file/paste fallback).

Loopback bridge binds 127.0.0.1 on an ephemeral port. Discovery file
`browser-bridge.json` in the Tauri app-data directory contains `url` and a fresh
random `token`, readable only by the current user. Native host discovers this file;
HTTP requests require `Authorization: Bearer <token>`. No CORS is enabled.
Routes: `POST /browser/import` accepts native import requests; `POST /browser/poll`,
`POST /browser/status`, and `POST /browser/heartbeat` accept the corresponding
native requests. Player routes
are merged into this server by the desktop app and validate a separate player-only
token. Provider scripts never receive the extension bridge token.
