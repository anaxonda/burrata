# Native Spotify session: live read qualification

This was a user-driven check of the isolated provider-session prototype. The
user completed Spotify sign-in and explicit account confirmation in its login
WebView; the login window then closed. A sanitized app-status response
confirmed Spotify connected, revision 1, and the `native-webview-http`
transport. No account ID, cookies, tokens, profile URL, or other
credential-bearing values are retained here.

With the login window closed, `list_remote_playlists` returned 48 items and a
continuation cursor in 2.438 seconds. The user also reported that importing a
playlist succeeded. A later restart recovered the OS-keyring credential without
interactive reauthentication. The previously confirmed session was re-enabled
with the authenticated MCP session-start command, which returned success. This
verifies live login, playlist listing, and one restart-recovery path, plus a
user-reported import; it does not establish import correctness, repeatability,
account switching, or playlist writeback. Sanitized MCP status exposes the
credential storage mode without exposing account IDs or secrets.

The attempted developer Web API route did not qualify profile browsing. The worker's bearer
header forwarding was corrected while preserving cookie/header routing
boundaries, but the attempted public REST owner-playlists request returned
HTTP 429 with `Retry-After: 22s`. After that cooldown, the single permitted
retry remained rate-limited with an eight-second retry hint, so requests
stopped. Spotify's [February 2026 migration
guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
later clarifies the public API limit: `GET /users/{id}/playlists` has no
replacement, and `GET /me/playlists` is scoped to the authorized user. The
changes apply to Development Mode apps; Extended Quota Mode apps are
unaffected, and Burrata's quota classification is not established by this
record. The 429 remains the observed response for the attempted request, but it
does not qualify a retryable public-profile route, and no supported public
Developer API profile-browse success is claimed. A private interface candidate,
`GET https://spclient.wg.spotify.com/user-profile-view/v3/profile/{id}` with
bounded playlist and zero artist/episode limits, is described by the pinned
[Spotifly implementation](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/index.ts)
and [response type](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/types/user.ts).
It is undocumented and has no cursor/offset contract in that source. One
read-only call to `list_remote_playlists` succeeded against a public official
Spotify account using the confirmed session: 46 entries returned in 2,188 ms,
all with valid 22-character playlist IDs and artwork URLs, with no cursor or
error. Root hot-staged only the Spotify worker and provider-operations JS into
the retained private app profile; the running app (build `0e782`) remained on
the WebView transport and session credentials were untouched. This qualifies
one bounded profile-ID read only. It does not qualify arbitrary accounts,
profile search, pagination, UI behavior, or long-term endpoint availability.
The running binary did not include the new Rust `total`/`truncated` fields, so
no completeness metadata is claimed. The private worker's 67-test suite is
deterministic coverage only.

An imported Spotify track reached MCP-reported `playing` status, with position
advancing by 5.48 seconds and a reported duration of 246.36 seconds. This
confirms status/playhead progress only. The launch did not set
`BURRATA_PLAYBACK_ENGINE=native`; source configuration confirms that this used
the default WebView path, though playback snapshots did not expose the engine.
No first-mixer-audio or physical-audibility evidence was collected. Repeating
the same track ID did not restart playback because `play_tracks` is idempotent
for the current track in this WebView provider-session run; the failure artifact
was retained with that reason. A separate native local playback sequence did
start fresh generations, but it is not accepted as qualification: first mixer
audio arrived after 7,132 ms. A warm 20-start sequence then reported 170.079
seconds against a verified 250-second source, so qualification halted before
seeking and no seek measurements were collected. A later identity report of
251 seconds against 245.48 seconds was also rejected. Details
and the resulting hold on the 30-minute memory capture are in the
[playback performance record](../../playback-performance.md).

One identified packaging issue was an incomplete bundled `managedBgUtils`; the
resolver now falls back to a system executable when available. A separate
system-CLI check decoded 0.5 seconds (96,000 PCM bytes) in 4.623 seconds. That
is not Burrata application playback and is not qualification evidence. No
engine-qualified playback, seek, recording-identity, or end-to-end performance
qualification is claimed from this session.

A later shared-profile attempt used the default WebView resolver for one
imported recording. The MCP client timed out at 25 seconds, then the app exited
with SIGSEGV (139) at `resolution_start`, before provider work. The redacted
[crash summary](../2026-10-15-shared-webview-resolver/crash-summary.json)
records the function-only stack through the resolver deadline wrapper. A large
unboxed future's stack footprint is suspected, not confirmed; the artifact
contains no SQLite lock evidence. Thus this later attempt failed and default
live playback remains unverified. The playback implementation is under repair.

The updated app mounted its frontend and completed the authenticated handshake
with 69 MCP tools. Five session-control MCP operations exposed sanitized
`credentialStorage` status without account identifiers. The UI now auto-enables
the background session after confirmation and hides the explicit Start action;
the authenticated MCP start command was used successfully after restart to
recover the previously confirmed session from the OS keyring without another
interactive login. This does not qualify long-running background behavior.
