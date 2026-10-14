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

Automation runs only through the local stdio bridge to a running Burrata app; there is no remote HTTP MCP endpoint or embedded AI service. Read access is on by default. Playback/playlist control is on by default and can be disabled in Burrata's **Settings → AI control · MCP**. Download and audio-analysis management is off by default and must be enabled there. The bridge can export a playlist file for Mixxx from local audio; it does not control a live Mixxx deck. Browser chart/playlist imports are performed in Burrata's UI, not by MCP.

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

`get_release_tracks` is now available in READ scope. Pass a release object returned
by `get_artist_discography` and its artist identity; it returns saved, ordered tracks
with stable IDs for `play_tracks`, `queue_tracks` and playlist batches. Release groups
and Discogs masters resolve to a representative/main edition, recorded in provenance.
The current registry contains 37 tools. Actual ten-track MusicBrainz and Discogs album
lookups have been verified through the SDK client.

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
