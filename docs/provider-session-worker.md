# Experimental native provider session workers

The extension's `extension/provider-operations.js` is shared by its browser
companion and the native Spotify worker. The companion uses the signed-in
browser session; its YouTube operations remain page-backed in
`extension/background.js`, with an explicit YouTube session handoff available
to the desktop app. The desktop app has separate one-job workers:
`scripts/provider-session-worker.mjs` for Spotify and
`scripts/youtube-session-worker.mjs` for YouTube. Authenticated requests use
bounded native WebView HTTP after explicit account confirmation. YouTube also
has a one-shot pre-confirmation bootstrap that reads public page configuration
for identity review; it does not perform authenticated account reads.

The Spotify worker accepts only `sp_dc` and optional `sp_key` from the
app-owned sign-in window, requires `sp_dc`, and attaches that cookie only to
`open.spotify.com`. Its authenticated profile and playlist reads use the
already confirmed account credentials.

Native transport is opt-in. `BURRATA_EXPERIMENTAL_NATIVE_PROVIDER_SESSION=1`
requests it for a launch, and the app can persist the user's opt-in for later
launches when the environment override is absent. Windows remains disabled;
the native transport is enabled only on the supported Linux/macOS targets. The
source checkpoint passed 907 native tests (16 ignored) and built executable
`53f6e42ca4b8`. The restart check using the persisted opt-in with no environment
flag is still pending; do not describe that source behavior as live-verified.

The latest retained normal-profile read used `native-webview-http`, OS-keyring
credentials, and the already confirmed Spotify and YouTube accounts at revision
1, with no new login, confirmation, or companion connection. Its Spotify
profile response contained 30 rows of a reported 110 and was incomplete; its
YouTube own-library response contained 61 rows with a terminal response shape.
A separate bounded playlist listing found 113 unique Spotify IDs and 61
YouTube IDs. These are read observations, not completeness claims: Spotify
page metadata was mixed/incomplete, and the YouTube listing did not assert
`complete=true`. See the [provider read proof](performance-results/2026-10-17-native-provider-main/provider-read-proof.json)
and [navigation proof](performance-results/2026-10-17-native-provider-main/navigation-proof.json).

No track-writeback path is qualified for either provider. In a separate
read-only audit, both verification-playlist markers recorded an attempted
creation without a playlist ID or receipt, so their outcomes remain unknown;
absence from the bounded listing cannot prove that no playlist was created.
That audit performed no retry, deletion, import, or queue action. The current
read-only provider run records `providerWrites=false`; it does not resolve the
earlier creation attempts.

`scripts/provider-session-worker.mjs` accepts one JSON job on stdin with
`provider`, `operation`, `args`, and `cookieHeader`, then writes one JSON
response. It supports Spotify reads and a reviewed `applySyncPreview` path.
That path requires a Rust-stored preview descriptor, pinned account, exact
complete playlist snapshot, owned/editable playlist, and reviewed removals,
additions, and desired order. It performs a fresh read, applies bounded
operations once, and verifies the full resulting order. Ambiguous results
return `OUTCOME_UNKNOWN`; callers must refresh before another preview. Raw
add/remove/move operations remain rejected by the worker.

The worker allows HTTPS requests only to the exact Spotify token and server-time paths, Spotify Web API v1 reads, Spotify Pathfinder GraphQL, and the two existing public Meld JSON registries: the fixed GitHub Gist used by `extension/spotify-auth.js` for TOTP parameters and the fixed GitHub Pages GraphQL hash registry. Redirects are rejected. The cookie header is attached only to `open.spotify.com`; bearer authorization is attached only to Spotify API hosts. Each request has an eight-second cap, the whole job has a 20-second cap, and response/output sizes are bounded. Errors use fixed redacted messages, and normalized results are rejected if they contain credential-shaped fields or values.

Caller cancellation is checked before dispatch and forwarded to an in-flight
request. Header/body validation runs before installing timeout/listener resources,
so rejected requests do not leave those resources active. The direct worker test
suite includes both cancellation paths alongside profile browsing and reviewed
writeback fixtures; these tests do not authenticate against a live account.

Read deadlines and aborted HTTP reads return `REQUEST_TIMEOUT`; HTTP 429 returns `RATE_LIMITED`. These remain distinct when the worker error is converted to a Rust error string. A timeout therefore does not trigger the duration backfill’s throttle-stop path. An error after a playlist mutation may have been dispatched still returns `OUTCOME_UNKNOWN`, regardless of the underlying failure. The worker retains bounded phase/status/retry diagnostics, but the Rust bridge currently exposes them only where included in the fixed message; general structured cooldown propagation remains unfinished.

This remains experimental because Spotify's web token and persisted GraphQL operations are private interfaces that may change. The TOTP secret and operation hashes depend on the existing public Meld registries. If those sources or Spotify's web endpoints change, the worker should fail closed and the browser companion remains the supported path.

## YouTube playlist API reference comparison

## Native YouTube bootstrap

The native YouTube path can start from an app-owned isolated WebView rather
than requiring an extension handoff. After the user explicitly chooses
**I’ve signed in — continue**, Rust reads only 15 named cookies from that
WebView's cookie API: `SAPISID`, `__Secure-1PAPISID`, `__Secure-3PAPISID`,
`LOGIN_INFO`, `SID`, `HSID`, `SSID`, `APISID`, `__Secure-1PSID`,
`__Secure-3PSID`, `SIDCC`, `__Secure-1PSIDTS`, `__Secure-3PSIDTS`,
`__Secure-1PSIDCC`, and `__Secure-3PSIDCC`. It requires at least one of the
first three authentication cookies; the remaining allowlisted session cookies
are optional and are included in the handed-off cookie header when present.
Only the first three can supply authorization proofs: the worker creates
`SAPISIDHASH` from `SAPISID` (falling back to `__Secure-3PAPISID`),
`SAPISID1PHASH` from `__Secure-1PAPISID`, and `SAPISID3PHASH` from
`__Secure-3PAPISID`. It verifies the proof set against the exact allowlisted
cookie handoff. Other Google cookies are excluded.

The worker accepts a one-shot `bootstrap` operation, performs a bounded GET of
only the selected YouTube origin's `/` page, and parses the static JSON object
passed to `ytcfg.set(...)`. It does not execute page scripts, follow redirects,
or send the cookie to another origin. The resulting client name/version,
optional API key and visitor data, locale, and session index are used for fresh
authenticated `account/accounts_list` and account-menu fallback checks, followed
by `navigation/resolve_url` to establish a stable channel ID. The worker
returns only verified account identity and nonsecret client context; Rust
requires the user to review the name and channel ID and explicitly confirm a
second time before keyring storage. These are implementation details of the
private InnerTube web interface, not a supported YouTube API contract.

This differs from the inspected [Meld login flow at commit
`e0af26953c315e6d8dfcef25b6436f7747fb34c6`](https://github.com/FrancescoGrazioso/Meld/tree/e0af26953c315e6d8dfcef25b6436f7747fb34c6),
which captures the full Music WebView cookie string and delegates auth to
InnerTubeX [`v0.5.2`](https://github.com/MetrolistGroup/innertubex/tree/v0.5.2).
That version selects one proof cookie (`SAPISID`, then 3PAPISID, then
1PAPISID) and emits one `SAPISIDHASH`; Burrata emits the separate matching
proof schemes above while keeping the cookie names bounded. Neither strategy
is a YouTube-supported authentication contract. Burrata's native YouTube
handoff, login compatibility, and live account path remain unqualified;
fixtures do not demonstrate a real sign-in or accepted live identity.

On Linux, both app-owned provider sign-in windows use separate persistent
WebKitGTK profiles under `managed-provider-session/native-profile-spotify` and
`managed-provider-session/native-profile-youtube`, each created with mode
`0700`. This preserves pre-confirmation sign-in state across restarts. After
confirmation, the app clears the WebView browsing data and stores the confirmed
credential in the OS keyring when available. On non-Linux platforms, both
sign-in windows request incognito mode; persistent-profile isolation has not
been verified there. Windows native transport remains disabled. The real
YouTube cookie handoff and post-confirmation credential recovery have not been
live-qualified.

The YouTube worker also supports a separately implemented, app-owned login
bootstrap as described above. The extension handoff remains an alternate
explicit session source. Both paths use bounded InnerTube requests. I compared
the account and playlist action assumptions with
the immutable `ytmusicapi` source revision
[`6a671182b3ddf606b4253e2780709c52e7a1c61b`](https://github.com/sigma67/ytmusicapi/tree/6a671182b3ddf606b4253e2780709c52e7a1c61b)
(reviewed source snapshot dated 2026-07-25). This is an unofficial client
implementation, not a YouTube-supported API contract.

At that revision, `get_account_info()` posts to
[`account/account_menu`](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/ytmusicapi/mixins/library.py#L459-L490)
and extracts the active account's display name, optional channel handle, and
avatar. It does not return a stable account/channel ID. This comparison alone
does not validate `account/accounts_list` or prove identity. Meld's pinned
InnerTubeX `v0.5.2` source separately confirms that it implements both
`account/accounts_list` and `account/account_menu`; Burrata's worker uses these
routes and requires a fresh stable channel identifier to match the selected
account before it accepts the handoff. Live acceptance remains unqualified. A
display name or browser handoff field alone is insufficient.

The playlist implementation confirms the prototype's three action names and
the `browse/edit_playlist` route: [add](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/ytmusicapi/mixins/playlists.py#L436-L483)
uses `ACTION_ADD_VIDEO` plus `addedVideoId`; [remove](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/ytmusicapi/mixins/playlists.py#L485-L515)
uses `ACTION_REMOVE_VIDEO` with both `setVideoId` and `removedVideoId`; and
[move](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/ytmusicapi/mixins/playlists.py#L321-L410)
uses `ACTION_MOVE_VIDEO_BEFORE`, `setVideoId`, and optionally
`movedSetVideoIdSuccessor`. Its [playlist parser](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/ytmusicapi/parsers/playlists.py)
exposes `setVideoId` as the unique occurrence ID needed for remove/move.
Therefore duplicate tracks must be addressed by occurrence ID, not by video ID
alone. The library's missing-`setVideoId` error text asks whether the user owns
the playlist, but that guard is not an ownership check; the native worker now
requires a fresh editable-playlist header tied to the authenticated account,
then checks the exact ordered snapshot before the edit and verifies the result.
This remains an unofficial protocol and has not been live-qualified. The
retained read-only audit does not establish whether its attempted verification
playlist creation succeeded; no YouTube edit or retry was performed as part of
that audit.

The pinned source declares MIT licensing, Python 3.10+, and `requests >= 2.22`
as its runtime dependency in [`pyproject.toml`](https://github.com/sigma67/ytmusicapi/blob/6a671182b3ddf606b4253e2780709c52e7a1c61b/pyproject.toml).
It is useful as a protocol reference, not a dependency recommendation: it
emulates YouTube Music's private web client, so endpoint and response changes
can break it without a supported API compatibility promise. No authenticated
YouTube traffic or playlist mutation was performed for this source review.
