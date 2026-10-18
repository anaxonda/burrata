# Meld session and provider architecture

Fresh Qobuz recheck (2026-10-18): [current pinned source, new host observations and opt-in architecture recommendation](performance-results/2026-10-18-meld-qobuz-recheck/README.md). Meld HEAD is now `1ded7122ee0bd166bd703bfe3b63d233af8b7dcd`, but the provider file hash is unchanged from the Oct 16 review. Four fresh metadata-only checks supplied no catalog JSON; this is a time/location observation, not proof of permanent host failure. A pinned MIT API implementation is available for a separately authorized self-hosted experiment; no live deployment or playback is qualified.

Follow-up: the newer Meld Qobuz provider is separately pinned to commit `dc7a27224acee33fa475ab70e1935dc03d3ef4ef`, with its source hash and four bounded catalog outcomes in [the Qobuz investigation](performance-results/2026-10-16-meld-qobuz-source/README.md). It relies on community resolver hosts rather than a Qobuz login renderer. None of those four checks returned catalog JSON, so no new playback source is qualified. The session architecture review below retains its original, separate revision pin.

This review is pinned to [Meld commit `e0af26953c315e6d8dfcef25b6436f7747fb34c6`](https://github.com/FrancescoGrazioso/Meld/tree/e0af26953c315e6d8dfcef25b6436f7747fb34c6), confirmed as the repository HEAD during this investigation (2026-10-15). The conclusions below concern the inspected Android Spotify implementation; they do not establish that Meld's private-provider techniques are stable or supported by Spotify.

Meld's Spotify WebView is a login surface. It clears cookies when constructing the login view, enables the browser features needed by the sign-in page, and detects a redirect to `open.spotify.com`. On that redirect it reads `sp_dc` (and optional `sp_key`) from Android's `CookieManager`, stops loading, navigates the WebView to `about:blank`, and runs token acquisition in an IO coroutine. The captured cookies are stored in DataStore, then the login screen stores the returned access token/expiry and navigates back. See [`SpotifyLoginScreen.kt`](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/ui/screens/SpotifyLoginScreen.kt#L120) and [cookie extraction and handoff](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/ui/screens/SpotifyLoginScreen.kt#L690).

The token exchange and ongoing Spotify data calls do not require a live WebView. `SpotifyAuth` fetches a community-maintained TOTP secret, obtains Spotify server time, and uses `HttpURLConnection` with the captured cookies to call the web player's internal `/api/token` endpoint. [`SpotifyTokenManager`](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/utils/SpotifyTokenManager.kt#L20) caches the access token and expiry, serializes refresh attempts with a mutex, and reacquires an expired token using the stored cookies; it clears the stored session and requests re-login when the cookie appears expired. [`SpotifyAuth.kt`](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/spotify/SpotifyAuth.kt#L14) documents that the token route and TOTP method are internal.

The app-side Spotify client uses Ktor with OkHttp for GraphQL persisted-query calls to Spotify Pathfinder and REST fallback reads, sending the bearer token. It maintains current/previous query hashes and signals a remote hash refresh when both are rejected. See [`Spotify.kt` client construction](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/spotify/Spotify.kt#L48), [persisted-query retry](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/spotify/Spotify.kt#L165), and [bearer-authenticated GraphQL/REST calls](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/spotify/Spotify.kt#L245).

YouTube Music has a similar separation with different session material. Its login view reads the YouTube cookie plus `visitorData`, `dataSyncId`, and `authUser` from the logged-in page, persists them, clears the view's cache/history/form data, and restarts the app process after login. [Login extraction and persistence](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/ui/screens/LoginScreen.kt#L119). The InnerTube compatibility facade owns a Ktor/OkHttp client and delegates request construction, session handling, retries and authenticated mutations to InnerTubeX; the session contains the cookie and those identifiers, and player calls return parsed player responses. [InnerTube transport and session](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/innertube/src/main/kotlin/com/metrolist/innertube/InnerTube.kt#L39) · [player response models](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/innertube/src/main/kotlin/com/metrolist/innertube/models/response/PlayerResponse.kt#L12). The player response's video details carry `lengthSeconds`; parsed song/search entries also normalize duration into the app's model. These are direct, native HTTP calls against YouTube's private InnerTube interface, not a persistent YouTube player page.

The source reviewed here explicitly blanks the page after cookie capture and clears cookies at the start of a new login. I found no normal-success-path call to `WebView.destroy()` or post-login cookie clearing in this screen. Thus it supports the login-only-WebView architecture, but it does not prove that the WebView process or cookie store is torn down immediately after success. The resource conclusion is an architectural inference from the source, not a measured Android process/RSS result.

For Burrata, the transferable pattern is a short-lived provider-login surface followed by native session/token management and native API/playback clients. Keep browser-provider tokens/cookies in the appropriate credential store, bound the login surface lifetime explicitly, and measure renderer/process release on target platforms. Do not treat Meld's Spotify TOTP secret, web-player token endpoint, GraphQL hashes, or YouTube InnerTube interface as stable provider APIs; they are undocumented dependencies that can change independently.

The Linux lifecycle has an additional source-level constraint: the locked `tauri-runtime-wry 2.12.1` implementation retains WebContexts for the app lifetime, with a comment explaining that WebKit does not close its network process when no WebViews remain (`src/lib.rs`, lines 2392–2414 in the inspected local dependency). Closing Burrata's provider window therefore does not prove all browser resources have exited. Meld's process restart is relevant here, but neither code review establishes measured retained memory. Measure after successful confirmation and during idle HTTP jobs before adding any restart behavior. Burrata's confirmed YouTube path already clears/closes the login view, saves the session to keyring, and uses a one-shot HTTP worker that does not depend on the view; restart recovery still needs live verification.

## Artist browsing source follow-up (2026-10-15)

The earlier REST artist/release/album adapter returned a confirmed HTTP 429 at `catalog_rest` in an isolated live read; that observation does not qualify paging or establish a permanent denial. The source now includes a session-native Pathfinder path, described below; it has deterministic coverage but no authenticated runtime qualification.

The exact pinned Spotifly request and type files were fetched and hashed in [the source manifest](performance-results/2026-10-15-spotify-artist-readonly/public-source-manifest.json). They establish web-session overview/album query shapes, but not a complete artist-release pager. The overview artist envelope is `data.artistUnion`; album details use `data.albumUnion`. Category arrays with totals alone do not establish pagination.

The public [desktop client snapshot](https://open.spotifycdn.com/cdn/build/web-player/web-player.6b816267.js), SHA-256 `1b9dcc42aeb9c4a7abfc673da946478b100d9fb62acb8bf7f27aa63ef8769602`, contains `queryArtistOverview` (module 22516). Its hash matches the [Meld registry](https://francescograzioso.github.io/Meld/spotify-gql-hashes.json) reviewed in this pass. Modules 8272/23339 expose `getArtistNameAndTracks` with URI/offset/limit and top-track next-offset handling. Modules 70554/98385 expose `getAlbumNameAndTracks` with URI/offset/limit and album next-offset handling. Those adapters consume track URIs; they do not prove full track metadata hydration or a release pager.

A subsequent source review located the current artist route chunk and verified `queryArtistDiscographyAll` with URI/offset/limit and the exact `DATE_DESC` wire value, plus detailed album-track metadata. These request shapes are implemented with passing adapter/worker fixtures. See [the pinned follow-up and limits](performance-results/2026-10-15-spotify-artist-readonly/session-native-followup.md). Actual account reads and UI validation remain required, and detailed album pages beyond offset 50 are explicitly experimental.

## Native worker request-count follow-up (2026-10-16)

At the historical baseline below, Burrata's native worker repeated token acquisition and registry setup for each provider job; the subsequent implementation checkpoint describes the promoted token coordinator. An instrumented mocked two-page artist-release fixture observed two requests to each auth/registry endpoint and two profile reads, in addition to the two page queries. [The request-count evidence and safe cache requirements](performance-results/2026-10-16-spotify-worker-auth-requests/README.md) distinguish this measured duplication from Meld's expiry-aware token manager. These are mocked counts, not live provider latency or a completed cache implementation.

## Isolated implementation follow-up (2026-10-16)

Later checkpoint: the memory-only token coordinator, private auth parser, and successful-confirmation Spotify login blanking described below have landed in main source and the verified `472b58f8` executable. The 696-test isolated checkpoint below records an earlier prototype, not the promoted implementation. The Linux build and deterministic tests establish code behavior, but do not establish live token refresh or post-login resource release. A read-only MCP check of the running executable reports `native-login:youtube`, `youtubeLoginPage: google-accounts`, and neither provider confirmed; leave that owned sign-in surface intact until login finishes. Main source subsequently passed 718 native tests before additional small cancellation guards; that count is not the running executable's test checkpoint (698).

The promoted implementation separates auth refresh from ordinary one-shot provider jobs. Its native, process-memory coordinator shares a bounded refresh among callers, scopes entries to confirmed account/session generation and an ephemeral keyed credential fingerprint, applies expiry skew, and preserves independent caller cancellation. Missing or invalid provider expiry permits a non-cacheable current-job token lease. Ordinary provider jobs still verify account identity. A dedicated private IPC parser extracts auth material without exposing it through public provider results; successful confirmation blanks the Spotify login page while retaining a generation-bound retry cookie if verification fails.

The initial isolated prototype checkpoint passed 696 native tests, including cache and private-parser fixtures, with 19 existing worker and 2 new private-protocol Node fixtures passing separately. Later revisions added exact-token invalidation after authentication rejection/account mismatch and preserved provider-specific diagnostic/error behavior; those revisions were promoted into the `472b58f8` executable. Live token refresh, resource release, and writeback compatibility remain unqualified.

## YouTube login surface and session restoration follow-up (2026-10-16)

The pinned Meld login screen polls the Music WebView for its cookie header and
`VISITOR_DATA`, `DATASYNC_ID`, and `SESSION_INDEX`, verifies account information
through its native InnerTube client, persists those values, clears the view's
cache/history/form state, flushes the cookie manager, and restarts the app
process. Its subsequent InnerTube calls use the persisted session and HTTP
client; the login WebView is not part of ordinary provider requests. This is
source evidence for separating login from provider work, not a measurement of
renderer teardown or proof of durable cookie recovery.

Burrata follows the same separation after confirmation. On Linux,
`confirm_native_youtube_account` in
[`managed_provider_session.rs`](../src-tauri/src/managed_provider_session.rs)
first requires the owned login window to be at the exact Music origin, then
captures only page config fields through a bounded WebKit callback and reads
the allowlisted cookies. It bootstraps and verifies the channel, presents a
second confirmation, stores the session in the OS keyring, clears browsing data,
and closes the login window. Later native provider operations load the
keyring-backed session and launch the bounded HTTP worker; they do not look up
or use the login renderer. After restart, a confirmed session resumes without
that renderer when persisted native access is enabled. If access was stopped,
or keyring storage was unavailable and the credential existed only in memory,
the corresponding resume or sign-in step is still required.

The remaining login handoff difference is observable and actionable: Linux
confirmation rejects a window still at Google Accounts because the page context
must come from Music. Burrata now exposes the explicit Continue action in the
normal settings login flow. When the owned page is at `accounts.google.com`, the
action checks for the bounded authentication-cookie family and navigates only
to the fixed Music homepage; after Music loads, the user continues again to
verify the channel. It does not automatically redirect while Google sign-in
may still be in progress. No live account or credential was inspected for this
comparison.

Linux login persistence remains a separate qualification from confirmed
session restoration. The app assigns YouTube a provider-specific WebKit data
directory with incognito disabled; source inspection establishes directory
separation, while the stored confirmed credential is in the OS keyring and the
browser data is cleared after confirmation. Neither this implementation nor
Meld's process restart establishes measured renderer release or successful
pre-confirmation cookie recovery across an application restart. Those behaviors
still need runtime qualification.

After confirmation, both implementations separate ordinary provider requests
from the login renderer. Burrata's successful confirmation clears browsing data
and closes the owned login window without restarting the app; its later calls
load the confirmed session from memory or the OS keyring. Meld clears the view
and restarts the process. The source does not establish that either approach
releases the underlying WebKit/WebView process immediately. Burrata's bounded
HTTP workers and Spotify auth refresh are stopped/cancelled on app exit, but
renderer memory and process release have not been measured. The follow-up now
transitions a current YouTube `SIGN_IN_REQUIRED` response into a persisted
reconnect-required state and blocks ordinary jobs before loading the rejected
credential. It preserves the saved account and keyring credential until explicit
replacement confirmation, including a reviewed channel switch. This differs
from Meld's Spotify token manager, which clears its expired Spotify cookie
session and requests login again; it is not evidence that Meld has equivalent
YouTube expiry handling. The combined native suite and focused UI/worker tests
pass, but live expiry recovery remains unqualified. See the
[744-test checkpoint](performance-results/2026-10-16-youtube-reconnect-verification/README.md).

## Browsing and writeback scope follow-up (2026-10-16)

The read-only source comparison did not reveal a justified cursor fix for the incomplete Spotify profile. The reviewed Spotifly `getUser(id, {playlistLimit, ...})` interface issues one `user-profile-view/v3/profile` request with a prefix limit and no offset/cursor loop. Burrata's retained live prefix evidence reports 30, 29, 18, then zero new rows, while the exposed total remains 136. This supports the current no-growth stop and incomplete label; it does not explain the unseen rows or prove that another undocumented continuation is impossible. See the [prefix evidence](performance-results/2026-10-15-spotify-profile-prefix/README.md).

Pinned Meld's album facade requests detailed `getAlbum` tracks at offset 0 / limit 50. The separately reviewed web `getAlbumNameAndTracks` continuation carries track URIs rather than complete metadata. Neither establishes that detailed album pages beyond 50 work; Burrata retains the experimental offset label described in the [source-pinned follow-up](performance-results/2026-10-15-spotify-artist-readonly/session-native-followup.md).

Meld's YouTube `library(browseId)` reads music-library shelves with continuation. Its owned-library add/remove flow fetches a fresh token through `next(WatchEndpoint(videoId))`, then sends `feedback(tokens)`. This differs from `/browse/edit_playlist`. Burrata's present experimental API deliberately covers playlist reads/edits with exact ownership, complete-snapshot and preview gates. Owned-library mutations are not implemented and must not be inferred from playlist write support. They are outside the current playlist-sync request.

## Playlist creation scope checked on 2026-10-17

In the reviewed `e0af26953c315e6d8dfcef25b6436f7747fb34c6` source, the YouTube adapter exposes `createPlaylist(title)` through `WEB_REMIX` and InnerTubeX. The reviewed Spotify client supports adding/removing/reordering entries and editing playlist attributes, but contains no playlist-creation function or mutation. This establishes YouTube Music creation code and Spotify existing-playlist editing in the inspected client; it does not establish live success or support in an unverified newer Meld revision. [YouTube source](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/innertube/src/main/kotlin/com/metrolist/innertube/YouTube.kt), [Spotify source](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/spotify/Spotify.kt).

Meld's existing-playlist support is not evidence that Burrata's Spotify creation attempt succeeded or that a different working Spotify create endpoint has been identified. Burrata's dispatched HTTP 429 remains unknown, with no retry. Newer source retrieval was unavailable in this check; no unsupported latest-version claim is made.
