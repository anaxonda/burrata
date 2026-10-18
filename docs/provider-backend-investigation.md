# Provider backend investigation

Meld/Qobuz follow-up (2026-10-16): a separately pinned provider client and four anonymous catalog checks are recorded in [the source/host evidence](performance-results/2026-10-16-meld-qobuz-source/README.md). Community resolver source orchestration is inspectable, deployed server source remains unidentified, and no checked host returned usable catalog data. Separate credentialed MIT Qobuz clients were located but not run without their required access. These findings extend the backend research; they do not qualify a new default or fallback.

Research snapshot: 2026-10-15. Monochrome source references below are pinned
to `a3c86df0436a3eedc341ab5163fc5ee6b1688b5d` (the commit reported by the
currently deployed `monochrome.st` page and already used in Burrata's source
review). A source revision describes that code; it cannot establish which
services a live host currently runs.

## Monochrome architecture and Burrata's adapter

At the pinned revision, Burrata's adapter is a narrow client for the public
Tracks API described in Monochrome's [`tracks-api.js`](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/tracks-api.js).
It sends catalog search to `https://tracks.monochrome.st/search/tracks` and
resolves audio from `https://tracks.monochrome.st/track/{id}`. It does not call
the separate unified frontend API in
[`api.js`](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/api.js),
which handles bearer tokens, Turnstile JWTs, and `/api/v2/track/` requests. The
two integrations must not be treated as aliases for a single service.

The implementation in [`monochrome.rs`](../src-tauri/src/monochrome.rs)
confirms the boundary: playback candidate search asks for at most 12 entries,
while interactive catalog search requests 30. The observed server ignores offset,
so Burrata labels catalog coverage as unknown rather than inventing a next cursor.
It accepts track IDs of 17–20 digits; candidate inspection requests the catalog track URL and reads
at most 64 bytes to verify the returned container. The adapter recognizes FLAC
and MP4/M4A, does not trust a frontend quality label, and does not claim that a
FLAC container is high-resolution. Requests use a 15-second timeout and do not
follow redirects. A response must be from the configured server origin and
match `/track/{numeric-id}`; query strings and fragments are rejected. Burrata
adds no Monochrome credentials or origin-spoofing headers.

This means the public adapter is **not** a direct client for Monochrome's
current Tidal/HiFi or Deezer playback paths. The deployed Monochrome interface
currently exposes a Dev Mode URL for a Tidal HiFi API server, a separately
configurable Deezer fallback API, and API-instance management; see the
[deployed settings page](https://monochrome.st/settings) and the pinned
[`api.js`](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/api.js).
Those are different interfaces and backend dependencies from the Tracks API
adapter in Burrata. Monochrome's current README also says domain-hosted
self-hosts cannot stream through its configured API unless a compatible paid
HiFi API endpoint is supplied. See the pinned
[README self-hosting section](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/README.md#self-hosting).

The source therefore supports these conclusions:

- Burrata's supported Monochrome integration is specifically the public Tracks
  catalog/direct-track API, as implemented at the pinned source revision.
- The unified frontend integration and Tidal HiFi/Deezer fallback chain are
  separate backend designs. Adopting them would require a separate protocol,
  access, playback, identity, and failure-mode investigation.
- Self-hosting the web application does not imply self-hosting its audio
  provider. The upstream README documents a domain allowlist restriction and
  a paid-compatible-HiFi endpoint exception; that is evidence against assuming
  a self-hosted frontend gives Burrata a private, interchangeable audio backend.
- A source tree and a working public endpoint do not prove Samidy's deployment
  is compatible. A request to `https://monochrome.samidy.com/` returned HTTP
  502 during this review. That only records the observed failure at that time;
  it does not establish a permanent outage or the site's deployed revision.

The deployed page identifies the `a3c86df0` build and exposes the frontend
settings described above. The repository README and instances document have
operational caveats: the README limits domain-hosted streaming, while the
instances file says its API listing is outdated after a service migration.
Treat those documents as descriptions of their own revisions, not an uptime or
compatibility guarantee. No hostname substitution, backend emulation, private
credentials, or new provider path is justified by this evidence.

## Backend implementation and deployment map

The client imports reveal three separate paths, but the repository does not
contain the server implementation for all three:

| Path | Client entry and wire shape | Server source/deployment evidence | Assessment |
| --- | --- | --- | --- |
| Tracks | `tracks-api.js`; catalog search on `/search/tracks` and direct stream object `/track/{id}` at `tracks.monochrome.st` | The frontend names the hosted origin, but no server package or deployment manifest for that origin is identified. The repository does have `functions/track/[id].js`; inspection shows that route renders bot/social metadata and tries Tidal API plus older HiFi instance lookups. It is not the `tracks.monochrome.st` search/stream service. | Burrata's existing client is scoped to the hosted Tracks route. Keep it only as the current public-provider path and verify bytes/codec at playback time. |
| Unified playback | `api.js` imports `HiFi.ts`, storage settings, and the Tracks streamer. It keeps unified playback state and uses bearer/Turnstile material for the unified request path (including `/api/v2/track/`). | The frontend source documents the client protocol, but the backend handler and its deployment configuration are not present in the pinned frontend repository. `INSTANCES.md` lists API origins, but it is not a deployment manifest and now explicitly warns that its Tidal/HiFi listing is outdated after a service migration. | A UI setting or endpoint path does not identify the running server implementation. There is no reproducible backend revision to pin for this path. |
| Legacy HiFi/Tidal API | The older API-instance client sends `/track/`, `/trackManifests/`, `/info/`, and catalog-style routes to configured instance origins. | The closest concrete implementation is [binimum/hifi-api at `bf46988`](https://github.com/binimum/hifi-api/tree/bf46988), with [`main.py`](https://github.com/binimum/hifi-api/blob/bf46988/main.py), API examples in its README, and an MIT license. The repository describes itself as a fork of `sachinsenal0x64/hifi`; its docs cover `/track/` and `/trackManifests/`, signed Tidal manifests, token setup, serialized playback-account requests, and Tidal account-blocking risk. | This is a legacy protocol implementation, not proof of the current Monochrome deployment. The upstream instances document says it is outdated and says Monochrome switched away from hifi-api. Don't label this source as the backend behind `monochrome-api.samidy.com` or `api.monochrome.tf` without deployment provenance. |

The official Monochrome repository is Apache-2.0. That covers the frontend
and included functions in the source tree, but does not establish the code or
license of the separately hosted `tracks.monochrome.st` server. The included
[`functions/track/[id].js` route](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/functions/track/%5Bid%5D.js)
is a social-preview handler, not that service: it renders metadata for bot
requests and consults Tidal/older instance APIs for that purpose. The nearby
[`functions/api` directory](https://github.com/monochrome-music/monochrome/tree/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/functions/api)
contains no unified playback handler. These routes are useful negative
deployment evidence, but do not prove that no private or separately hosted
server exists.

The follow-up source review found no additional credential-free server path.
At the pinned revision, the unified client's default base URL and token are
empty, and it returns no unified response until configured. Its default-key
flow exchanges a Turnstile response at `/api/auth/turnstile` using the configured
bearer token before requesting `/api/v2/track/`. The legacy HiFi server's
[`main.py`](https://github.com/binimum/hifi-api/blob/bf46988/main.py) loads
`token.json` or Tidal client/refresh credentials from the environment and cannot
serve playback without them. Neither a localhost Monochrome frontend nor its
social-preview function supplies those missing backend components.

These are separate outcomes: the frontend architecture offers reusable cache,
expiry and fallback ideas; reproducible self-hosting of the current Tracks or
unified service remains unsupported by the located public source; direct
Tracks use remains an existing, unqualified playback candidate. The bounded
host-network request returned HTTP 403 before catalog data or audio bytes
([sanitized result](performance-results/2026-10-15-monochrome-bounded/host-network.json)).
That request establishes neither a permanent outage nor the reason for denial.
No recording match, codec, range seek or backend performance claim follows
from it. No paid infrastructure was provisioned.
The `monochrome-legacy` repository is only a small browser-compatibility
front-end (two commits; Flash/HTML5 files), not a provider server, and its
repository listing has no license file. It is not a backend source to adopt.
There is no separately identified `tidal-api` repository in the pinned
Monochrome source; the concrete legacy Tidal HTTP server located here is
`binimum/hifi-api`.

The deployment trail does not close the gap. The official
[`INSTANCES.md` on current main](https://github.com/monochrome-music/monochrome/blob/main/INSTANCES.md)
labels its API-instance list outdated after the service migration; the
current README likewise describes the older public HiFi endpoint as no longer
the default path. The instances page is a mutable operational note, not a
pinned deployment manifest. That is positive evidence that blindly adopting the old
HiFi server would target the wrong deployment, rather than a reason to stop
the investigation. It also means the implementation behind the newer unified
and Tracks origins cannot be established from public frontend sources. A live
hostname's availability, including Samidy's mirror, would not reveal its
deployment revision or prove wire compatibility.

For a credential-free local prototype, a fixture parser could be written for
the documented HiFi response shapes (including the base64 BTS manifest and
the newer `trackManifests` response), but that would test only parsing. It
would not validate entitlement, signed URL access, CDN range requests, FLAC
content, decoder continuity, or correctness of a recording match. No backend
was provisioned and no account material is available here. A direct Tracks or
HiFi prototype therefore remains opt-in for an explicitly configured,
user-owned endpoint and should not be promoted to a default/fallback until an
authorized sample is actually fetched, decoded, and matched through the
existing playback path. The unified endpoint is not a practical target until
its backend implementation and deployment contract are identified.

## Performance evidence audit

The existing artifacts provide useful component-level measurements, but do
not qualify the requested end-to-end provider or application performance:

- The release seek run is a Linux x86_64 build from a dirty checkout, with
  generated five-minute sine fixtures and 20 seeks per position. It times
  decoder-to-first-PCM and decode worker/process CPU. Its metadata explicitly
  excludes resolver, mixer, output device, renderer, network access, and
  recording correctness. It is not a historical-build comparison.
- The debug seek run has the same fixture, platform, and scope limitations.
  The results can support a claim about decoder reuse on those synthetic
  fixtures only. They cannot support a claim about multi-second user-visible
  seek latency or a provider/backend speedup.
- The running-app sample metadata identifies the older pre-change development
  application and uncontrolled playback state. It records no controlled seek
  and explicitly marks qualification false. It is diagnostic evidence only.
- The InnerTube smoke artifacts are a small provider prototype exercise, not
  a Monochrome comparison. They include failures and one explicit clip, and
  the documentation says they ran alongside optimized compilation; the timings
  are exploratory. They do not compare equivalent Burrata/provider paths.
- `scripts/check_monochrome_live.mjs` describes a live playback, seek, and
  optional download acceptance procedure. A procedure is not a result: no
  checked-in run report demonstrates repeated, correctly matched Monochrome
  playback or a controlled performance comparison.
- [`release-qualification.md`](release-qualification.md) requires clean-machine
  checks of public release installers. Hosted CI and the current Linux decoder
  microbenchmark do not satisfy that checklist.

Accordingly, the current performance evidence does not establish a Rust
cutover regression, that SoundCloud is faster than YouTube, that Samidy and
Burrata use interchangeable Monochrome backends, or that a proposed backend
would meet the p95/CPU targets. `playback-performance.md` correctly keeps those
questions open.

To qualify an actual provider change, the comparison needs the same named
recordings and expected IDs, equivalent cold/warm/replay and seek scenarios,
recording correctness and access verification, failures and retries retained,
at least 20 attempts on relevant paths, and correlated resolver/source-ready,
first-audio, seek-first-audio, process/thread CPU, and buffering measurements.
It must identify the provider endpoint and build/dependency revisions while
keeping URLs and credentials out of reports. Remote provider runs should be
separate from local decoder microbenchmarks. The existing benchmark runner
requires a real playback adapter; it cannot infer success from extraction
metadata.

## Primary references

- [Monochrome source at pinned revision](https://github.com/monochrome-music/monochrome/tree/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d)
- [Tracks API implementation at pinned revision](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/tracks-api.js)
- [Unified API implementation at pinned revision](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/api.js)
- [Upstream README self-hosting guidance at pinned revision](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/README.md#self-hosting)
- [Monochrome API instances document](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/INSTANCES.md)
- [Current API instances migration note](https://github.com/monochrome-music/monochrome/blob/main/INSTANCES.md)
- [Pinned social-preview route (not the Tracks backend)](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/functions/track/%5Bid%5D.js)
- [Pinned functions/api directory](https://github.com/monochrome-music/monochrome/tree/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/functions/api)
- [Currently deployed Monochrome settings](https://monochrome.st/settings)
- [Legacy HiFi/Tidal API source at `bf46988`](https://github.com/binimum/hifi-api/tree/bf46988)
- [Legacy HiFi/Tidal API endpoint examples](https://github.com/binimum/hifi-api/blob/bf46988/README.md#api-schema)
- [Legacy HiFi/Tidal API license](https://github.com/binimum/hifi-api/blob/bf46988/LICENSE)
- [Monochrome legacy browser-only repository](https://github.com/monochrome-music/monochrome-legacy)

## Spotify public profile playlist browsing

Spotify's former `api.spotify.com/v1/users/{id}/playlists` route is not used
for arbitrary public profiles. The bounded adapter instead follows the
profile-view request documented by the pinned Spotifly client at commit
`4fc289abc02e29c2c60f9fce94c87ab1cfa58982`: GET
`spclient.wg.spotify.com/user-profile-view/v3/profile/{id}` with playlist,
artist, episode, and market query parameters. See the [pinned Spotifly
implementation](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/index.ts)
and [profile response types](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/types/user.ts).
This is an undocumented Spotify web endpoint, not a supported public API.
Burrata restricts the worker to that exact HTTPS host and route, forwards only
the web-session bearer token, omits cookies, disables redirects, and reports
known counts and truncation without inventing cursor pagination. The user's
own playlist library continues to use its separate Pathfinder operation.

## Bounded public hosted check (2026-10-15)

A no-new-credentials Tracks catalog request for the public Get Lucky reference
was attempted before any audio fetch. The ordinary sandbox first failed DNS;
that result describes the execution environment, not provider availability.
Repeating the same bounded request with host networking reached the server and
returned HTTP 403. No catalog response body or stream bytes were retained, and
no stream request or decoder run occurred. This is one forbidden request, not
a claim that the service is permanently unavailable. The sanitized
[host-network record](performance-results/2026-10-15-monochrome-bounded/host-network.json)
and separate [sandbox record](performance-results/2026-10-15-monochrome-bounded/sandbox-network.json)
retain the failure and reviewed source revisions. Matching, expiry, range seeking,
actual decoding and repeated latency remain unqualified. The default stays Tracks;
this result does not qualify a new provider or justify bypassing access controls.

## Later successful Tracks playback checkpoint

The earlier Get Lucky HTTP 403 did not generalize to every request. Later on
2026-10-15, the repaired main app played the exact public Poso recording through
Monochrome at a decoded duration of 245.48 seconds. An isolated Linux native run
then used the same sanitized Spotify recording metadata, Monochrome first and
YouTube/SoundCloud fallback enabled. Its first mixer-consumed audio arrived at
2,613 ms (source ready at 1,406 ms). A clean repetition passed three warmups,
20 starts, 60 seeks and paused seek/resume with zero failures. Warm observed
startup p95 was 922 ms; seek-to-first-audio p95 was 126 ms. See the
[gate](performance-results/2026-10-15-monochrome-primary-native/isolated-playback-gate.json)
and [clean repetition](performance-results/2026-10-15-monochrome-primary-native/repeated-qualification-clean-escalated.json).

This establishes usable direct Tracks playback for one correctly selected
recording on this build, not universal coverage or a hosted-backend comparison.
Fallback providers were configured but not exercised in that successful run.
The contended exploratory run and two rejected harness/environment attempts
remain separate. The source/self-hosting conclusions above are unchanged:
working hosted playback does not publish the Tracks service implementation or
supply the unified API's missing credentials and backend source. No alternative
backend was promoted and no paid infrastructure was provisioned.


## Exact-recording follow-up (2026-10-16)

The primary [Sasa track page](https://www.traxsource.com/track/15044160/sasa-main-version) and [Shelter Lab Tracks Vol 2 release](https://www.traxsource.com/title/2875589/shelter-lab-tracks-vol-2) identify Timmy Regisford's Main Version at 4:41, on Access Records, with a reported release date of 2026-10-16. This establishes catalog identity and duration, not full-stream availability. No matching permitted full-length source was verified by the bounded public searches; the existing unavailable regression fixture now retains these exact catalog IDs.

The official [Ergo track page](https://cuneiformrecords.bandcamp.com/track/sorrows-of-the-moon-featuring-mary-halvorson-2) exposes track ID 292222166, title Sorrows Of The Moon (featuring Mary Halvorson), artist Ergo and duration 310.254 seconds. Its album is [If Not Inertia](https://cuneiformrecords.bandcamp.com/album/if-not-inertia-2). A bounded credential-free page fetch succeeded without a challenge interstitial. Installed yt-dlp 2026.08.19 failed to decode the HTML-entity-escaped `data-tralbum` attribute; Burrata's existing native album parser decoded that static JSON correctly. No provider JavaScript or login material was used.

Exactly one bounded media GET to the observed Bandcamp audio host succeeded: 5,098,066 bytes of MP3, 44.1 kHz stereo, with redirects disabled. Local FFmpeg probing reported 310.253333 seconds and complete decoding returned exit code 0. This proves full-length delivery and external decoder acceptance for that request, not Burrata mixer startup, seeking, signed-URL recovery or platform qualification. The [redacted fixture](../tests/fixtures/playback/ergo-bandcamp-source.json) contains metadata and result summaries; live media URLs, query tokens and the copyrighted audio are excluded.

A direct, pinned Bandcamp track-URL playback prototype is being implemented behind an internal opt-in flag; broad Bandcamp search and default-provider changes are out of its scope. [Bandcamp's own help](https://get.bandcamp.help/en/articles/15263228-does-bandcamp-support-30-second-snippets) describes full public plays followed by a purchase prompt. Qualification must honor that gating rather than repeatedly fetching or bypassing it. Current Tracks remains the default, and no new backend is promoted by the single request.

### Bounded range checkpoint (2026-10-16)

A fresh anonymous fetch of the same official Ergo track page resolved the expected public mp3-128 descriptor. First, middle, and final bounded range requests each returned HTTP 206 with valid `audio/mpeg` ranges and a consistent 5,098,066-byte object length. The previously retained full MP3 was reused and decoded completely; the middle and final range fragments also decoded. The first fragment was checked at the response level but was not retained for local decoding. Details are in the [redacted aggregate](performance-results/2026-10-16-bandcamp-ergo-ranges/aggregate-redacted.json) and [checkpoint notes](performance-results/2026-10-16-bandcamp-ergo-ranges/README.md).

This supports explicit-page transport feasibility only. A previously captured signed URL also returned 206, but its timestamp was not expired, so expiry and refresh behavior remain unverified. Burrata playback, seeking, cancellation and cross-platform behavior remain unqualified. The backend still requires `BURRATA_EXPERIMENTAL_BANDCAMP_PLAYBACK=1`, and Bandcamp remains absent from the default source fallback; no general catalog/search integration or default availability follows from this checkpoint.


### Later actual mixer checkpoint (2026-10-17)

The [isolated Ergo native proof](performance-results/2026-10-17-directory-identification/ergo-native-proof.json) supersedes the earlier playback-only open gate above: one opt-in exact-source start reached the mixer in1793ms, agreed with the independent310.254-second duration, consumed five seconds, preserved the occurrence through paused seek10, then consumed another5.01seconds. This is a single exploratory functional pass; late95% seek, backward seek, expiry/refresh, acoustic recording identity and cross-platform behavior remain unqualified. The experiment remains default-off, with no broad Bandcamp catalog/fallback promotion.

The scripts-only `--one-shot-bandcamp` mode is implemented with 26 merged helper/imported-case/sustained-playback checks passing. On the isolated53 build, one fresh source attachment reached the mixer in1495ms, then95% (294.7413s) and1% (3.10254s) seeks each passed five seconds of mixer-consumed progress with the same occurrence; measured decoder seek times were48ms and44ms. Final pause passed. [Late/backseek proof](performance-results/2026-10-17-storage-recognition/ergo-late-backseek.json). This reused an isolated fixture profile, not a fresh database, and does not qualify expiry, acoustic identity, twenty-repeat reliability, resource use or other platforms. The existing3+20matrix default remains unchanged.


## Credentialed Qobuz server source pin (2026-10-17)

The previously located alternative now has a pinned server implementation:
[Qobuz-DL-API at `1134bc6662572afec514c7b3a88f50af94e0fd6c`](https://github.com/audio-music-streamer/Qobuz-DL-API/tree/1134bc6662572afec514c7b3a88f50af94e0fd6c),
under its [MIT license](https://github.com/audio-music-streamer/Qobuz-DL-API/blob/1134bc6662572afec514c7b3a88f50af94e0fd6c/LICENSE).
Its Next.js [search handler](https://github.com/audio-music-streamer/Qobuz-DL-API/blob/1134bc6662572afec514c7b3a88f50af94e0fd6c/app/api/get-music/route.ts)
and [media-URL handler](https://github.com/audio-music-streamer/Qobuz-DL-API/blob/1134bc6662572afec514c7b3a88f50af94e0fd6c/app/api/download-music/route.ts)
provide the get-music/download-music paths and success/data envelopes read by
Meld's pinned client. This is a concrete self-hostable server source, not
proof that any of Meld's anonymous hosts runs this revision.

Its [direct client](https://github.com/audio-music-streamer/Qobuz-DL-API/blob/1134bc6662572afec514c7b3a88f50af94e0fd6c/lib/qobuz-dl.tsx)
requires Qobuz app/account credentials and returns a media URL; it does not
establish decoded identity, duration, codec, preview exclusion or range seeking.
The separate [Qobuz-DL revision `ada30543c28286a3ec3d55e6ccc444166f5420b4`](https://github.com/QobuzDL/Qobuz-DL/tree/ada30543c28286a3ec3d55e6ccc444166f5420b4)
is also MIT and includes server code/environment configuration and Docker
examples. Its [README](https://github.com/QobuzDL/Qobuz-DL/blob/ada30543c28286a3ec3d55e6ccc444166f5420b4/README.md)
requires a paying-member token for full-length files. The Docker example uses
a floating Node image and development server; it is not a qualified production
deployment. No backend was installed or run, and no provider credentials or
audio were requested. This resolves source identification for a separate
credentialed alternative while leaving current Tracks/unified and anonymous-host
deployment provenance open. An offline envelope/error/preview/cancellation
contract fixture is the next step available without login; no default changes
or credential-free playback claim follows.


The [October 17 source follow-up](provider-backend-source-followup-2026-10-17.md) records the unpinnable `hifi-api-workers` legacy-port lead. It supplies no inspectable deployment or new qualified hosted backend; the current Tracks/unified/Samidy source gaps remain.
