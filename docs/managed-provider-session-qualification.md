# Managed provider session qualification

## Current bounded checkpoint — 2026-10-17

Current normal-session reads return8/8 known Spotify artist releases and62 unique YouTube playlist entries through native managed HTTP, with OS-keyring storage and no companion connection seen. No login window was opened by the check. YouTube total/completeness remain unknown; expiry, account switching and writeback are not qualified. See [current evidence](performance-results/2026-10-17-managed-provider-restart-reads/README.md).

## Historical run sheet

The procedures and unqualified checkpoints below describe the earlier prototype.

This run sheet records the experimental native Spotify session on a real
account and the separately implemented, not-yet-qualified app-owned YouTube
session path. It does not qualify playlist writeback. The user reported one
successful local playlist import; its contents and correctness were not
independently verified. The separate Chromium transport remains opt-in with
`BURRATA_EXPERIMENTAL_MANAGED_PROVIDER_SESSION=1`; this run uses the native
WebView prototype instead.

## Launch and isolated sign-in

Use the freshness-checking launcher and a separate retained app-data profile.
The parent directory is private and must not be removed as an unneeded build
cache:

```sh
qualification_profile=$(mktemp -d /tmp/burrata-launch.XXXXXX)
chmod 700 "$qualification_profile"
printf 'burrata isolated memory run\n' > "$qualification_profile/.burrata-isolated-profile"
mkdir -p "$qualification_profile/fixture/Music"
BURRATA_APP_BINARY=/tmp/burrata-sorting-target/debug/burrata \
BURRATA_EXPERIMENTAL_NATIVE_PROVIDER_SESSION=1 \
BURRATA_ISOLATED_MUSIC_ROOT="$qualification_profile/fixture/Music" \
bash scripts/dev/launch-current.sh --isolated-profile "$qualification_profile" --keep-profile
```

The marker also enables the guarded Music-root override, keeping background
playlist reconciliation within the empty fixture directory. Do not set `HOME`.
Keep existing legacy Burrata processes alone. Run only one copy of this
qualification binary against the selected app-data directory at a time. In
Burrata, open **Settings → Connections → Experimental isolated provider
session** and choose **Sign in to Spotify**. Complete sign-in only in the
separate Spotify WebView opened by Burrata. The WebView uses incognito mode and
a stable, isolated context directory under `managed-provider-session/native-profile`;
its cookie store is ephemeral. Do not clear the parent managed-session
directory between steps because it holds the account/session scope used to
locate the OS-keyring entry.

Profile behavior differs by provider and platform. On Linux, the app-owned
YouTube WebKitGTK login uses a persistent dedicated directory at
`managed-provider-session/native-profile-youtube`, created with mode `0700` so
the WebKit context is configured to use that directory. Whether sign-in
cookies recover across an application restart remains a runtime qualification;
do not assume the directory setting alone proves it. Spotify remains
incognito. On macOS, both native sign-in windows remain incognito; persistent
WebKit profile isolation has not been verified. The Linux YouTube profile may
contain browser state and should be treated as sensitive: do not share or
delete it as routine cleanup during qualification.

Choose **Confirm Spotify account** only after the WebView shows the expected
account. Burrata reads only the `sp_dc` and `sp_key` session cookies from that
owned Spotify WebView, then performs a read-only status request and records the
verified account ID and revision. The cookie values are sent to a bounded,
read-only worker over stdin and are never written to logs or request files. The
credential is stored in the OS keyring when available; if keyring storage is
unavailable, the UI reports that the session is held in memory until Burrata
closes. Cookie credentials are not persisted in the WebView profile. Do not
capture or share cookies, tokens, or profile contents.

## Read operations, restart, and cleanup

Confirmation now auto-enables the Spotify background session, and the UI hides
the explicit Start action. Verify the transport label says
`native-webview-http`, the confirmed account remains active, and a read-only
operation such as account status or listing playlists succeeds. The
authenticated MCP session-start command is available to re-enable a previously
confirmed session after restart. The live evidence below qualifies Spotify
reads only. The YouTube native login and read path has deterministic fixtures
but has not been tested with a real account or WebView runtime. Provider writes
remain disabled.

Restart Burrata with the same feature flag and app-data directory. Run another
read-only check and confirm that the same account is still active. If secure
keyring storage was available, Burrata should recover the credential without
reopening the WebView. If it was memory-only, sign in again and confirm the
account after restart.

Use **Stop** to end the managed session, then verify the login window closes
and the isolated context directory remains. After account confirmation the
WebView also requests that its browsing data be cleared. The WebKit network process may
remain after its window closes, so this check does not qualify full process
release. Stop the app and check that no qualification Burrata app process
remains. Do not stop or alter legacy Burrata processes.

## Evidence to record

Record the OS and WebView runtime version, confirmation success, displayed
account ID (or a locally redacted form), credential-storage mode, read-operation
success, account continuity after restart, whether Stop closed the login window,
and whether WebKit child processes remained. Note any MFA prompt, cookie
extraction error, keyring limitation, or provider API denial. A single
successful session recovery after restart using the OS keyring and the
authenticated MCP session-start command is recorded below; this does not qualify
long-running background behavior. Account switching, provider writes, YouTube,
or other systems remain outside this qualification.

## App-owned YouTube sign-in (not live-qualified)

The native prototype now has an independent YouTube sign-in window and profile
directory. With the same native-session flag, choose **Sign in to YouTube** in
Settings → Connections. Complete sign-in only in Burrata's isolated WebView,
then choose **I’ve signed in — continue**. On Linux, the YouTube WebKitGTK
context is configured to use `managed-provider-session/native-profile-youtube`
with mode `0700`. Recovery of sign-in cookies across development restarts has
not been verified. Spotify remains incognito; macOS YouTube sign-in is also
incognito because persistent profile isolation has not been verified there.

Burrata collects only 15 named YouTube cookies from its owned WebView:
`SAPISID`, `__Secure-1PAPISID`, `__Secure-3PAPISID`, `LOGIN_INFO`, `SID`,
`HSID`, `SSID`, `APISID`, `__Secure-1PSID`, `__Secure-3PSID`, `SIDCC`,
`__Secure-1PSIDTS`, `__Secure-3PSIDTS`, `__Secure-1PSIDCC`, and
`__Secure-3PSIDCC`. At least one of the first three authentication cookies is
required; the remaining session cookies are optional and pass in the cookie
header when present. Only the three authentication-cookie variants generate
proofs: `SAPISIDHASH` from SAPISID (falling back to 3PAPISID), `SAPISID1PHASH`
from 1PAPISID, and `SAPISID3PHASH` from 3PAPISID. The worker checks those
proofs against the exact allowlisted handoff.

The worker fetches the same-origin homepage and parses static JSON client
configuration without executing downloaded scripts, then checks the selected
channel through authenticated account identity operations. For comparison,
the exact pinned [Meld login source](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/ui/screens/LoginScreen.kt)
captures the whole Music WebView cookie header; its pinned
[InnerTubeX `v0.5.2` implementation](https://github.com/MetrolistGroup/innertubex/blob/v0.5.2/src/commonMain/kotlin/com/metrolist/innertubex/InnerTube.kt)
sanitizes but does not name-allowlist the header and emits one `SAPISIDHASH`
from the first available proof cookie. Burrata's bounded cookie allowlist and
three proof schemes differ. Both rely on private InnerTube behavior without a
supported YouTube authentication contract.

Review the displayed channel name and verified channel ID before clicking the
second **Confirm** action. The app rechecks the active channel immediately
before keeping the session in the OS keyring (or reports memory-only storage)
and closes the sign-in window. This flow never reads a regular Chrome profile
or sends a credential in a URL. If Google blocks sign-in inside the WebView,
an app-owned Chromium fallback has not been implemented or qualified yet; the
explicit one-time YouTube companion handoff is an alternate path.

This path has fixture coverage for the static bootstrap parser, fresh account
identity, allowed-cookie filtering, and worker request bounds. The complete
app-owned cookie handoff, real login challenge, persistent Linux profile
recovery, and live account reads have not been qualified with a real YouTube
account. Do not treat the implementation as qualified until those checks pass.

## Live evidence recorded 2026-10-15

The user completed Spotify sign-in and account confirmation in the isolated
native WebView prototype. The login window closed. Sanitized app status
confirmed Spotify connected at revision 1 using `native-webview-http`. With
that window closed, a read-only remote-playlist request returned 48 items and
a continuation cursor in 2.438 seconds. After restart, the OS-keyring
credential recovered without interactive sign-in; the authenticated MCP
session-start command returned success. The user reported a successful
playlist import; import contents and correctness were not independently
verified. See the [redacted live-read record](performance-results/2026-10-15-native-provider-session/live-read-qualification.md).

Public profile playlist browsing through Spotify's developer Web API is
unsupported for apps affected by the February 2026 Development Mode changes:
Spotify's [migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
documents no replacement for `GET /users/{id}/playlists`, and `GET /me/playlists`
returns the currently authorized user's playlists. Spotify says Extended Quota
Mode apps are unaffected; Burrata's API quota classification is not established
here. The attempted request returned HTTP 429 with a 22-second retry hint; one
retry after that cooldown remained rate-limited with an eight-second hint, then
requests stopped. The 67-test extension/worker suite passes. No success of public
arbitrary-user browsing through the developer API is claimed. A
candidate private web endpoint is
`GET https://spclient.wg.spotify.com/user-profile-view/v3/profile/{id}` with
bounded `playlist_limit`, `artist_limit=0`, `episode_limit=0`, and
`market=from_token`. The pinned [Spotifly source](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/index.ts)
and [response type](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/types/user.ts)
describe this undocumented interface. One bounded read-only `list_remote_playlists`
call succeeded for a public official Spotify account: 46 entries with valid
22-character IDs and artwork, no cursor or error, in 2,188 ms. It used the
already confirmed session in the retained WebView app profile; only the worker
and provider-operations JS were hot-staged. This validates one profile-ID read,
not arbitrary accounts, profile search, pagination, UI behavior, result
completeness or long-term availability. Rust `total` and `truncated` metadata
were not present in that running binary.
An imported Spotify track reached MCP-reported `playing` status and its position
advanced by 5.48 seconds; reported duration was 246.36 seconds. This does not
prove audio output. Source configuration confirms that, with
`BURRATA_PLAYBACK_ENGINE` unset, this launch used the default WebView playback
path; it was not a native playback attempt. No first-mixer-audio evidence was
collected in this Spotify provider-session run. Repeating the same track ID did
not restart it because the WebView command is idempotent for the current track;
that failure artifact was retained.

Separate explicit native local-playback attempts now have mixer-audio
measurements, but do not qualify playback: the first successful mixer audio
arrived after 7,132 ms, exceeding the 5-second threshold. A warm 20-start
sequence produced new generations but reported 170.079 seconds for a verified
250-second source, so qualification stopped before seeks. A later identity
sample reported 251 seconds against 245.48 seconds and was rejected. The
earlier session's default WebView path was the engine used for status and
position progress, but this does not qualify audio. A later shared-profile
WebView resolver attempt timed out at the MCP client after 25 seconds and the
app exited with SIGSEGV (139) before provider work. The function-only stack
points through `tokio::time::timeout<playback_sources::resolve_inner future>`
and `resolve_playback_track`; large unboxed-future stack exhaustion is
suspected, not confirmed. The [redacted crash summary](performance-results/2026-10-15-shared-webview-resolver/crash-summary.json)
contains details. The default live playback path is not verified, and the
native path has not been promoted. The 30-minute isolated memory run remains
held until the resolver crash and duration identity mismatch are fixed and a
stable build is available.

A root cause was identified in the incomplete `managedBgUtils` bundle, and the
resolver now falls back to a system executable when available. A separate CLI
decode of 0.5 seconds of audio completed in 4.623 seconds; that is not an
application playback result. The authenticated app handshake exposed 69 MCP
tools, including five session-control operations with sanitized
credential-storage status and no account identifiers; the status field is
`credentialStorage`. Login,
playlist reading, and one keyring recovery path are verified; public profile
browsing, engine-qualified playback, seeks, long-running
background behavior, account switching, provider writes, and end-to-end
performance remain unqualified.

## Companion update for one-time existing YouTube login

The existing Chrome native-messaging wrapper was updated to the immutable,
verified `ced6cc73…6394` debug executable. Its former wrapper is backed up in
the workspace cache. The native-host manifest and extension permission set
were unchanged. The cached companion popup/background files match the current
source. Chrome’s internal Extensions page could not be controlled through
CUA; the user was asked to reload the installed companion, then explicitly
review and confirm the existing YouTube account in its pairing flow. No cookie
handoff or YouTube connection is claimed until the user completes that flow
and the app independently verifies the channel.

A separate authenticated public Spotify profile-name search returned 20 profile
rows with 20 valid official profile URLs in 3,386 ms. The sanitized record is
[profile-search.json](performance-results/2026-10-15-webview-identify/profile-search.json).
This used the singular `profile` kind; the destination browser UI was found
to send `profiles`, which the backend does not accept. The corresponding UI
fix is separate from this direct MCP evidence.

The corresponding public official profile URL also returned 46 playlists with
valid IDs and artwork in 2,016 ms. The current binary preserved `totalCount:1545`,
`complete:false`, and `truncated:true`, with no continuation cursor. This proves
the bounded URL path and honest incomplete metadata, not access to all 1,545
reported playlists. See [profile-url.json](performance-results/2026-10-15-webview-identify/profile-url.json).

## Subsequent qualification limits

The user now requests YouTube writeback in addition to Spotify writeback. Both
must remain explicitly enabled and previewed. No live remote write has been
performed. Pending backend concurrency protection and live YouTube session
qualification prevent a complete-support claim.

The native Spotify session restored from secure storage and reached the exact
track catalog operation, but the service returned HTTP 429 twice. The second
attempt waited over 105 seconds after a 36-second Retry-After response. Live
retries stopped; the safe evidence is under
[Spotify exact-track qualification](performance-results/2026-10-15-spotify-exact-track/qualification.json).
The retained imports already have durations, so the empty duration-repair pass
is not evidence of a successful live backfill.
