# Retained Spotify session: live read-only verification

Development executable SHA-256:
`db663c5abb2d10364a6e28fcdfa3a6eaaf821d2d88a4881b454d0cc6c8b413a1`.
The original main instance was closed before launching the retained isolated
provider verification profile. The app reported the native WebView/HTTP transport
ready with a previously confirmed Spotify account. No new confirmation, login,
credential migration, import or remote playlist write was performed.

Results through Burrata MCP:

- Library page: 48 playlists in 2,406 ms, with a next cursor.
- Own public profile by ID: 49 playlists in 2,518 ms; provider total 108,
  explicitly truncated/incomplete and no next cursor.
- The same profile by official URL: 49 playlists in 1,738 ms, same incomplete scope.
- A separate two-page library check returned 48 and 49 rows, 97 distinct playlist
  IDs, no duplicates, and another cursor. This does not prove full collection.
- The actual app profile-search UI returned 30 Spotify profiles for the public
  query `Spotify`, without an error.

See [sanitized counts/timings](report.json) and
[actual UI search completion](profile-search-ui.json). No account identifiers,
playlist names or credentials are retained in these reports. The profile endpoint's
partial result remains an unresolved pagination limitation. This verifies reads
for one retained account on Linux, not all accounts, expired-session recovery,
complete profile pagination, YouTube sign-in or either provider's writeback.

The isolated app was closed after testing. Its existing session remains in its
own profile, separate from the main library.
