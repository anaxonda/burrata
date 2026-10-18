# Burrata MCP bridge

Burrata includes a local stdio MCP server that exposes the app's automation tools to MCP clients. It reads the authenticated loopback endpoint from the app's `automation.json`, loads the current tool registry, then forwards calls to Burrata. The bearer token stays in the local discovery file and authorization header; it is not included in MCP tool metadata or stdout.

The bridge uses the official TypeScript SDK stable v2 packages `@modelcontextprotocol/server` and `@modelcontextprotocol/client` (2.3.1). The server uses the SDK's `serveStdio` transport and imports each advertised JSON Schema from the app registry. See the SDK's [stdio guide](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/serving/stdio.md), [tools guide](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/tools.md), and [package/version listing](https://www.npmjs.com/package/@modelcontextprotocol/server).

## Start and test

Start Burrata first, then launch the bridge from the project root:

```sh
npm install
npm run mcp:start
```

The app writes `automation.json` into its Tauri app-data directory (`$XDG_DATA_HOME/org.burrata.discovery/automation.json` on Linux when `XDG_DATA_HOME` is set; otherwise `~/.local/share/org.burrata.discovery/automation.json`). The bridge reads the `url` and `token` properties from that file. `BURRATA_AUTOMATION_FILE` can point to another discovery file when running the bridge directly, mainly for tests.

The registry is read when the stdio process starts, with a five-second timeout. If discovery is missing, Burrata refuses the connection, or the registry cannot be reached, the process remains protocol-safe and exposes an empty tool list. Start Burrata and restart the MCP process to load tools. Calls allow up to 65 seconds for longer local operations; a refused connection returns `PLAYER_NOT_RUNNING`, while a timeout or other network failure returns `REQUEST_TIMEOUT` or `NETWORK_UNAVAILABLE`. For timed-out calls, check the operation's status before retrying because the app may still be processing it.

## Available tools and permissions

The live app registry is authoritative. It exposes library and playlist reads, catalog/artist search, unified music discovery, chapter inspection and playback, autotag review, audio-source search and `inspect_audio_source`, playback and playlist controls, playback-history search and lookup, download-job management, analysis, and local Mixxx playlist export. The audio workflow is explicit: search, inspect the chosen candidate and its formats/chapters, then pass that candidate to `download_candidate`; chapter IDs can be supplied when selecting chapters. For example, import a current Beatport Top 50 in Burrata, read its saved playlist, compare exact audio candidates for selected tracks, then export the resulting collection's local files for Mixxx. Track search supports the library, MusicBrainz, YouTube, SoundCloud, Soulseek and Monochrome. Beatport and Traxsource are imported chart sources rather than searchable catalog providers. See [discovery, chapters, and autotagging](discovery-chapters-autotag.md) for the added tools and their boundaries.

`inspect_spotify_profile_page` is a read-only account-scoped diagnostic accepting a validated `ownerId`. Default `mode: "paged"` accepts `offset` (0–10,000,000) and `limit` (1–30); `mode: "summary"` instead accepts `playlistLimit` (1–200, default 200) and rejects page bounds. Its strict DTO returns bounded counts, structural enums and ordered domain-separated playlist-ID hashes, without raw IDs or provider response bodies. The summary mode passes native regression tests and is available in the current development binary. It does not establish profile completeness or perform provider writes.

`inspect_youtube_playlist_page` is a read-only diagnostic with validated `playlistId` and optional continuation `cursor` inputs. Requests remain scoped to the confirmed YouTube account. It returns bounded structural counts and pagination diagnostics, excluding raw provider bodies, provider IDs and tokens. Its response-shape diagnostic flag is off by default for ordinary playlist reads; this tool does not import tracks, edit playlists or establish content completeness.

`inspect_youtube_library_page` is a read-only tool with no arguments. It inspects
only the initial own-library page for the confirmed YouTube account, returning
bounded response-shape counters and pagination flags. It excludes raw responses,
account/provider IDs and continuation tokens. It does not collect subsequent
pages, import playlists or prove whole-library completeness or absence.

The optional native YouTube sign-in status includes a host-only `youtubeLoginPage` classification (`music`, `google-accounts`, `youtube-www`, `other`, or `unavailable`); it never returns the current URL or page contents. If Google sign-in finishes on its own origin, call the Control tool `return_youtube_login_to_music` explicitly after signing in. It requires the owned window to still be on the exact HTTPS Google accounts origin and a valid allowlisted YouTube authentication cookie, then navigates only to the fixed Music home page. Call `confirm_managed_provider_account` after Music loads. Bounded Linux verification has confirmed one YouTube account, an own-playlist listing and a selected playlist with 240 occurrences; this does not qualify other accounts, completeness, expiry or writeback.

Remote playlist tools use the app's configured provider transport; the browser companion is an optional browser-based path, not a universal requirement. The app-managed provider session supports Spotify and YouTube account requests after the user signs in and confirms the account in Burrata. On builds with the native transport implemented (currently Linux and macOS), its native WebView HTTP transport can be opted into in Settings → Connections; the choice persists, and `BURRATA_EXPERIMENTAL_NATIVE_PROVIDER_SESSION=1` is a development override. When that transport is enabled, requests run on demand without keeping an external browser open. The native provider session requires confirmed credentials in the OS keyring; if secure storage is unavailable, confirmation fails rather than routing credentials through MCP. MCP never asks for the streaming password. When app-managed native access is not selected or supported, the configured browser-based transport remains available, including the browser companion path.

Implementation support and live qualification are separate. The current implementation routes Spotify and YouTube provider requests through the configured session/transport. Live Spotify reads through the app-managed path are qualified to one Linux account. Bounded Linux verification also confirmed one YouTube account, an own-playlist listing and a selected playlist with 240 occurrences; this does not qualify other accounts, completeness, expiry or writeback. These results do not establish broad account, platform or write qualification. With an available Spotify session, `discover_music` searches Spotify tracks, artists, albums, playlists and profiles. `get_remote_artist` reads an artist and a page of releases, while `get_remote_album_tracks` reads album tracks; both accept validated Spotify IDs and bounded decimal-string cursors that can be passed back unchanged from `nextCursor`. YouTube discovery can search public playlists and channels, and the existing track and video searches remain available. `list_remote_playlists` browses a provider's playlists; `import_remote_playlists` starts an import job, which can be checked or cancelled with `get_remote_playlist_import_job` and `cancel_remote_playlist_import`. `refresh_remote_playlist` updates an imported snapshot. Playlist write sync is separately opt-in for each provider in Connections and remains review-gated. `search_remote_playlist_writeback_candidates({bindingId,localTrackId})` searches the bound provider for one local track and returns expiring review tokens. `get_playlist_sync_preview({bindingId,reviewedMatches?})` returns proposed changes; optional `reviewedMatches` contains explicit `{localTrackId,candidateToken}` pairs, never raw provider URLs or IDs. The service rechecks token expiry, account, binding and local metadata. `apply_playlist_sync_preview` applies a saved preview after checking any supplied reviewed change set against the plan. Imports and refreshes require Control permission; preview is Read, and applying a preview is Manage. Playlist write implementation does not itself establish live write qualification.

`get_remote_track` is a read-only Spotify lookup that requires an available Spotify session and an exact 22-character alphanumeric track ID. `get_remote_artist` and `get_remote_album_tracks` use the same session and validate the Spotify ID and numeric cursor. Burrata checks the returned account and catalog identity, then returns normalized metadata without importing or saving it to the local library.

`repair_spotify_durations_public` is an explicit Control action that fills missing durations in local Spotify catalog metadata using public Spotify track metadata. It does not require a Spotify account and does not write to remote playlists. Each call processes at most 300 rows or 60 seconds and returns only `changed`, `processed` and `complete` counts; call it again to continue an unfinished batch. Use `public_spotify_duration_repair_running` to check status and `cancel_public_spotify_duration_repair` to stop an active run. The MCP bridge allows 75 seconds for the repair call; other calls retain the 65-second default. Public endpoint rate limits may delay retrying.

`discover_music.cursor` can contain buffered results from earlier pages. Its shared maximum is 2 MiB at the tool schema, request validation and cursor decoder. The authenticated automation request body allows a bounded 64 KiB for the JSON envelope around a maximum-size cursor.

`list_track_files` returns local files linked to one track, and `set_preferred_track_file` selects the preferred file for playback. File selection requires Control permission. Local file paths are returned only by the explicit track-files tool.

Automation runs only through the local stdio bridge to a running Burrata app; there is no remote HTTP MCP endpoint or embedded AI service. Read access is on by default. Playback/playlist control is on by default and can be disabled in Burrata's **Settings → AI control · MCP**. Download and audio-analysis management is off by default and must be enabled there. The bridge can export a playlist file for Mixxx from local audio; it does not control a live Mixxx deck. Browser chart imports are performed in Burrata's UI. Remote Spotify and YouTube playlist tools use the configured provider transport described above; an external browser companion is only one available path.

Run the real-client transport and authentication regression tests with:

```sh
npm run test:mcp
```

## Connect a client

The snippets below show configuration only. They do not edit a client's configuration. Replace `/absolute/path/to/burrata` with the repository path where `npm install` has been run.

For Codex CLI, the [official MCP guide](https://developers.openai.com/codex/mcp) documents stdio servers through `codex mcp add` and `~/.codex/config.toml`:

```sh
codex mcp add burrata -- node /absolute/path/to/burrata/mcp/server.js
codex mcp list
```

Or use this TOML entry:

```toml
[mcp_servers.burrata]
command = "node"
args = ["/absolute/path/to/burrata/mcp/server.js"]
cwd = "/absolute/path/to/burrata"
```

For Claude Code, the [official MCP guide](https://code.claude.com/docs/en/mcp) documents a local stdio server with `claude mcp add`:

```sh
claude mcp add --transport stdio burrata -- node /absolute/path/to/burrata/mcp/server.js
claude mcp list
```

Other MCP clients can use this generic stdio configuration shape:

```json
{
  "mcpServers": {
    "burrata": {
      "command": "node",
      "args": ["/absolute/path/to/burrata/mcp/server.js"],
      "cwd": "/absolute/path/to/burrata"
    }
  }
}
```

Keep the server command local. Do not add the app's bearer token to the client configuration; the bridge reads it from Burrata's discovery file.

`playback_state` returns a bounded queue page: use `offset` and `limit` (default 50,
maximum 200), then inspect `queueTotal`, `queueOffset`, `queueLimit` and
`queueHasMore`. The current track and playback position are returned independently
of the requested page. This keeps a large library queue from exceeding the local
bridge response limit.

`identify_playing_track` requires Control permission. Existing clients may call it
with no arguments. To make cancellation possible, pass a caller-generated UUID as
`captureId` and pass the exact same value to
`cancel_playing_track_identification`; without a supplied ID, the request remains
compatible with older callers but cannot be cancelled through MCP. The cancellation
tool requires Control permission and accepts only that UUID. Both native and
WebView identification paths use the same request ID; cancellation does not accept
an arbitrary command or provider payload. Cancellation is scoped to that request and
is remembered briefly if it reaches the backend just before its matching identify
call.
It identifies a short excerpt of the currently playing native or WebView track
and returns the result separately; it does not change saved tags. The app chooses
the validated source and current position, including chapter offsets. Pausing,
seeking, or changing the track cancels or rejects an outdated result. Identification
requires FFmpeg and SongRec and can take up to 60 seconds, so clients should allow
that response time. Default-WebView capture has deterministic coverage and one successful live
Poso excerpt; broader recognition qualification remains pending.

`get_release_tracks` is now available in READ scope. Pass a release object returned
by `get_artist_discography` and its artist identity; it returns saved, ordered tracks
with stable IDs for `play_tracks`, `queue_tracks` and playlist batches. Release groups
and Discogs masters resolve to a representative/main edition, recorded in provenance.
Actual ten-track MusicBrainz and Discogs album lookups have been verified through the SDK client.

## Radio station control

Use `search_radio_stations` with a name, country or tag, then pass one returned
station object to `add_radio_station`. For a known stream URL, use
`add_custom_radio_station` with `name` and `url`. `list_radio_stations`,
`rename_radio_station`, `remove_radio_station`, `get_station_details` and
`play_radio_station` operate on stable station IDs. Station mutations require
Control permission and update the running GUI immediately. Station details include
observed listening history when available. Automatic recognition follows GUI
playback; MCP does not need to trigger each identification.

## Playback history

`search_playback_history` searches the saved playback history and returns paginated
entries with stable session IDs and stable track IDs. Use `list_playback_history`
for the same page format without filters, or pass a `query`, `provider`, `stationId`,
inclusive `from`/`to` bounds (`YYYY-MM-DD` or RFC3339), `sort` (`newest` or `oldest`),
`limit` (1–100), and `offset`. `get_playback_history_entry` retrieves one entry
using its `sessionId`. These tools require Read permission; they only inspect
history and do not alter playback or delete saved entries. Set `provider` to
`radio` to search radio songs or `non-radio` to exclude them; `stationId` narrows
radio history to one station. Local file paths are removed from returned track and
chapter metadata.

## Current gaps

The registry still lacks playlist rename/delete/reorder/remove-track operations,
queue removal/reordering and shuffle/repeat controls, chart/browser import or
refresh tools, and reusable missing-track/quality-upgrade comparisons. Source
selection and download jobs exist, but there is no transactional bulk acquisition
workflow. Mixxx support exports local playlist files; it does not operate decks or
mix live. The stdio bridge requires the running desktop app and Node; headless and
remote MCP transports are not implemented.

`inspect_playback_layout` is a read-only, no-argument tool for checking the
running desktop player's layout. It reports geometry only for a fixed set of
roles: footer, now-playing, transport, scrubber, right-side controls, quality
and error indicators, plus the queue panel, header, title, identify button,
queue-close button and panel-close button. Each role includes presence,
visibility, viewport position and element/client/scroll dimensions. Findings
identify missing or hidden required roles, out-of-viewport elements,
horizontal overflow, overlaps, and queue-header clipping. The tool returns no
track titles, source URLs, account details or arbitrary DOM. The frontend must
be ready, and the request times out after five seconds. For an explicitly
selected instance, save the geometry with
`node scripts/dev/inspect-playback-layout.mjs AUTOMATION_FILE OUTPUT_JSON`.
This complements visual inspection; it does not establish physical audibility
or verify every responsive state from a single window size.

`inspect_webview_media` is a read-only tool with no arguments and a five-second
response bound. It samples the active audio element's current time, duration,
paused/ready/network/error state and source-presence/stability booleans. Playback,
session, generation, seek and queue-occurrence checks reject a sample if the
player changes during inspection. It returns no media URL or local path. Native
playback returns `applicable: false`. Repeated time progression is evidence that
the WebView media element advances; it does not establish physical audibility or
native mixer consumption.

Diagnostic builds launched with `BURRATA_WEBVIEW_MEDIA_EVENT_TRACE=1` also return
an optional bounded event history: up to64 play/playing/waiting/stalled/canplay/
canplaythrough/seeking/seeked/ended/error samples, each with monotonic timestamp,
media state and at most eight buffered ranges. The history resets with session,
source generation, queue occurrence or seek serial. It records no source URL,
path or title, performs no work on `timeupdate`, and leaves playback/readiness
policy unchanged. The flag must be set before launching the isolated app; it
uses a static startup script rather than an arbitrary-script MCP interface.
The feature is disabled by default. Opted-in diagnostics also project bounded codec,
MIME, format ID, video flag, expected byte length and direct/HLS stream kind.
Missing or unsupported optional metadata becomes null; URLs, headers and paths
are omitted. `BURRATA_PLAYBACK_TRACE=1` separately emits redacted upcoming-search
completion and observed candidate-cache hit/miss/waited-hit events, allowing
preparation to be distinguished from actual foreground reuse.

`navigate_ui` is a Control-permission tool for finite app routes (`library`, `favorites`, `history`, `downloads`, `settings`) or an exact Spotify artist/profile/playlist identifier, or Back/Forward history direction. Artist and playlist IDs must be 22 ASCII alphanumeric characters. It confirms route entry, with a 15-second bound and cancellation/stale/unmount guards; provider data may continue loading. It accepts no arbitrary URL, script or selector. See [the command contract and qualification limits](automation/navigate_ui.md).

`set_player_panel` is a Control-permission tool that sets the visible player
panel to `queue`, `info` or `closed`. It changes only the panel view; it does
not change playback or the queue. It requires the frontend-ready handshake,
allows one outstanding request, and waits up to five seconds for the UI to
apply the change. To run the same check through the local stdio MCP bridge and
write a sanitized result containing only the requested panel and completion
status, use `node scripts/dev/set-player-panel.mjs AUTOMATION_FILE queue|info|closed OUTPUT_JSON`.

`search_ui` runs the visible app's search flow for a query (up to 500
characters), a selected source or `all`, and a result kind such as `tracks`,
`playlists` or `profiles`. It requires Control permission because it changes
the visible search state. It waits for actual source-search completion for up
to 30 seconds and returns only source IDs, result counts, a fixed status or
error code, and optional per-source `sourceCoverage` (`hasMore` and
`incomplete` booleans). `status: "complete"` means the UI search operation
finished; it does not promise that every provider result was loaded. Use
`sourceCoverage` to distinguish an available continuation from a partial result
with no continuation. Older clients may omit this optional field. The result
never includes the query, result rows, URLs or provider errors. Only one UI
search can run through MCP at a time. A timed-out or cancelled MCP call signals
the frontend to cancel its outstanding reads.

The visible search can also be inspected and controlled through three bounded
tools. `inspect_search_ui` reads a page of currently loaded, filtered and sorted
playable track occurrences from the Search view (up to 200 rows), with a fresh
UUID revision and short-lived opaque occurrence tokens. `set_search_continuation`
changes whether playing a displayed result snapshots the loaded search results
into the queue; it requires the current revision and the preference remains
off by default. `play_search_result` requires the same revision and one token
from the latest inspected rows. Any query, source, kind, sort, duration-filter
or result-page change makes old tokens stale; old revisions also remain stale
after the Search panel is remounted. UI control requests expire after a bounded
deadline. The play response reports that
the existing UI action was dispatched, not that audio playback succeeded.
These tools do not expose provider URLs or arbitrary UI selectors.

`resize_app_window` uses the control permission to resize the main window in
logical pixels (width 320–7680, height 240–4320). It returns the previous and
resulting client-area sizes and maximized states. Set `restoreMaximized: false`
for a responsive-layout check, then restore the returned `before.innerSize`
and `before.maximized` values. Omission preserves the prior maximized state.

`list_tracks_page` reads library, favorites, shortlist or a saved collection
through the same database query as the UI. Filtering and sorting run before
pagination; `limit` is bounded to 200 and the response includes `total` and
`hasMore`. It accepts the shared sort fields, duration bounds, availability,
provider and text query. It does not change collection membership or order.

The `preview_verification_playlist_intent({provider})` and `apply_verification_playlist_intent({provider,intentId,reviewToken,contractHash})` tools support reviewed creation of an additional private empty verification playlist with a UUID title. Preview uses control scope and local persistence; apply uses manage scope. Approval expires after ten minutes and on process/account/revision/generation changes. The contract includes bounded prior-creation counts and warnings, including unresolved outcomes; changed history requires a fresh preview. Dispatch is one-shot, failures are never retried automatically, and a returned ID is durably retained before readback. The reviewed-intent native build passed its verification suite. Two explicitly approved exact creations were each dispatched once: Spotify returned HTTP 429 with an unknown outcome; YouTube returned `PLJHq5kM_QbX8` with a matching title, but readback reported null owner, privacy, and total, with `complete: false`. Do not retry either attempt. Successful track writeback remains unqualified.

One bounded live native Spotify test is now verified: exact Spotify-ID addition to the user-created test playlist, independent one-track readback, zero-diff follow-up and write-sync restored off. This is not full writeback qualification; cross-catalog/reorder/removal/YouTube and creation remain open. [Evidence](performance-results/2026-10-17-spotify-one-track-writeback/README.md).

### Queued playback acceptance and recovery

`play_tracks` retains its existing behavior: it waits for playback start or an error.
For an exact queue that may require more than 45 seconds of source matching, use:

```json
{"name":"play_tracks_queued","arguments":{"operationId":"b9ee9fe8-a935-4efa-80b0-a04467df337e","trackIds":["track-a","track-a","track-b"]}}
```

This separate control tool validates 1–500 track IDs, preserves supplied order and
repeated occurrences, and returns an operation record once that exact queue is
installed and startup has been dispatched. Its immutable `receipt` contains the
engine, queue revision, every queue occurrence ID, requested IDs/count, and initial
accepted generation. Acceptance does not imply that sound has started.

Supply a new caller-generated UUID for each new queue intent. If the response is
lost, use `get_playback_operation` with the same `operationId`, or repeat the exact
original `play_tracks_queued` call. Exact retries return the existing record without
restarting playback; reusing an ID for a different ordered track sequence fails.
Operation states are `preparing`, `accepted`, `playing`, `paused`, `failed`,
`exhausted`, `cancelled`, and `superseded`. Existing `playback_state` supplies the
current player projection and bounded, sanitized per-occurrence failure ledger.
The root operation ID remains stable across failed-source attempts and queue
advancement, while attempt generations change. Both engines traverse unavailable
occurrences finitely, including duplicate track IDs; exhausted failures do not
wrap indefinitely under repeat modes or authorize queue-end radio continuation.

`cancel_playback_operation` targets only that operation ID, including cancellation
before dispatch. A late cancellation cannot stop a newer playback owner. Stop and
replacement end ownership; pause, seek, and resume preserve the root operation.
RPC completion, disconnected response delivery, and lost acceptance acknowledgments
do not cancel accepted playback. Preparation still has a finite 45-second deadline.
WebView status delivery uses a bounded outbox with retries on automation pulses.

The operation registry retains at most 128 records and evicts terminal records
first. Active records are retained; new preparation fails if capacity is occupied
by active work. `unknown_or_expired` means the ID is absent from this process's
bounded registry, including after restart. Receipt recovery applies while the
record is retained. A fixed-size seen-ID filter rejects reuse of evicted IDs for the rest of this process, preventing a late
retry from restarting playback. The filter may conservatively reject a fresh UUID
as unknown/expired; generate a new UUID in that case. Status records and the
frontend retry outbox are process/session state, not durable jobs.
