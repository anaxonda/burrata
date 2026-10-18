# Streaming playlists and local files

Burrata's browser companion is version **0.2.1**. It handles Spotify and YouTube provider requests in the browser, so provider sign-in and session credentials stay there. The app does not ask for or store provider passwords or cookies.

## Connect the browser companion

Keep Burrata open, install or reload the companion, then open its popup and enable **app-driven imports**. The browser asks for optional access to supported provider sites. Grant access for the sites you plan to use; if it is declined, current-page capture can still be used where supported, but app-driven requests will fail until access is granted. The popup shows whether the companion is connected and app-driven mode is enabled.

For Spotify, sign in to Spotify in the browser. Burrata can then search tracks, artists, albums, playlists, and profiles, and browse your playlists. YouTube public playlist and channel search does not require an account. Sign in to YouTube in the browser to browse and import your own playlists. Provider pages may open or come to the foreground during connection or sign-in. Complete sign-in or verification in that browser tab and retry from Burrata.

## Browse, import and refresh playlists

Open a playlist to browse its tracks, filter or sort them, and play or queue tracks without adding the playlist to the library. **Add to library** is a separate action. **Select all** loads the remaining account-list pages before selecting; importing a selection starts one background job (up to 500 playlists) with progress and cancellation.

Settings → Connections has separate **Show Spotify account playlists** and **Show YouTube account playlists** options, both off by default. Enabling one lists remote playlists in its sidebar section without importing them.

Creator names link to official provider profiles where supplied. Missing counts or visibility are omitted rather than represented as zero or guessed.

Select the remote playlists to import. Burrata shows one editable playlist linked to the source, grouped under its provider. Source snapshots remain internal for refresh and sync comparisons. Reimporting the same provider/account/playlist reuses its linked playlist. Imports and local edits do not write changes back to a provider.

Refresh captures a new source snapshot. If the local working collection still matches its previous baseline, Burrata updates it to the refreshed entries. If you edited the working copy, refresh preserves those edits and records the latest remote state for review. An incomplete capture is not used to replace the local entries.

Provider writeback is a separate, explicit workflow under **Write changes**, shown only for editable linked playlists. Build a sync preview, review its additions, removals, moves, unresolved tracks, and conflict status, then apply that preview. Burrata checks the signed-in account, full playlist snapshot, edit permission, and stored preview before writing; if the remote playlist changes after review, refresh and create a new preview. Native provider writeback is experimental and remains disabled unless the user opts in; it has fixture coverage but has not been live-qualified. No live playlist mutations were performed during its implementation.

## Local files, quality, and replacement

A track can have multiple local files. **On this device** lists the linked files with codec, bitrate, sample rate, channels, bit depth when available, duration, and file size. Estimated bitrate is marked as approximate. Use **Use this file** to select the preferred file for that recording; the choice is saved and applies across its linked catalog entries.

**Replace** targets one managed local file for the same recording. Burrata verifies the selected file has not changed, prepares the replacement in its final codec, and preserves the old bytes in an adjacent `.burrata-backup-*` file. The backup path is recorded with the replacement. Burrata rejects occupied destinations and restores the old file if it cannot complete the replacement metadata update. External read-only files cannot be replacement targets.

## MCP tools

The local MCP bridge forwards the app's live registry. The relevant tools are:

- `discover_music` searches Spotify catalog types and YouTube playlists or channels when those capabilities are available.
- `get_remote_playlist` browses a page of tracks without creating a local import.
- `list_remote_playlists`, `import_remote_playlists`, `get_remote_playlist_import_job`, and `cancel_remote_playlist_import` browse, import, and track import jobs.
- `refresh_remote_playlist`, `get_playlist_sync_preview`, and `apply_playlist_sync_preview` refresh a binding and review or apply an editable provider sync.
- `list_track_files` lists the local files for a track; `set_preferred_track_file` changes its preferred file.

Read permission covers browsing, previews, and file listing. Import, refresh, and job cancellation use Control; applying a Spotify preview uses Manage. Choosing a preferred file uses Control. Provider connect and disconnect remain app controls and do not accept MCP credentials.

Provider playlist writes are disabled by default. Enable the per-provider write setting in Connections, then use Sync on an editable linked playlist to review the complete proposed diff before applying. Tracks without a verified provider identity block the write; Burrata does not guess matches or silently skip entries. If a write result is ambiguous, refresh the playlist and build a new preview before trying again. Open source remains available in the playlist menu.
