# `navigate_ui` MCP tool

`navigate_ui` uses Burrata's authenticated automation control permission and the native event/ack path to change the visible app route. It accepts exactly one of:

- `view`: `library`, `favorites`, `history`, `downloads`, or `settings`.
- `spotifyArtistId`: exactly 22 ASCII letters or digits.
- `spotifyProfileId`: a validated Spotify profile identifier.
- `spotifyPlaylistId`: exactly 22 ASCII letters or digits.
- `historyDirection`: `back` or `forward`; empty history returns an unavailable result.

The frontend uses its existing route navigation function. A Spotify artist ID is assigned before opening the artist view. The frontend acknowledges only after the requested route has been entered and the navigation generation still matches; provider-backed artist data may continue loading afterward. It returns only a correlated completion status and the allowlisted destination. Requests serialize through one active slot, time out after 15 seconds, and emit a cancellation event when the native request is dropped. The frontend ignores cancelled or superseded requests. No URLs, arbitrary scripts, selectors, result rows, or remote write actions are accepted.

The playlist target uses the same playlist-opening path as the sidebar. History restores its saved route state. Completion explicitly reports `completionScope: "route_transition"`; it does not prove provider content finished loading. Cancellation suppresses a stale acknowledgment and does not roll back an already changed route.

The playlist/history extension passes five mounted App regressions and Svelte check. Its parser/schema fixtures pass in the 894-test native suite (16 ignored). The running `9782d1a6dfb4` build predates this extension; live playlist/history acceptance remains pending.
