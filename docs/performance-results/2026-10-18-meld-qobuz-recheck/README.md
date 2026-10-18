# Meld Qobuz: current source and fresh resolver check

Checked 2026-10-18. Recommendation: retain Burrata's current playback sources. A disabled-by-default, user-configured Qobuz adapter is technically feasible, but the anonymous public hosts are not qualified as a shipped playback backend. The useful next experiment is an authorized account-bound/self-hosted adapter with a bounded catalog contract, rather than default automatic traffic to the community mirrors. No application, Cargo, release, credentials or account state changed in this investigation.

## Revision and pipeline

GitHub's main-branch commit API returned [`1ded7122ee0bd166bd703bfe3b63d233af8b7dcd`](https://github.com/FrancescoGrazioso/Meld/tree/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd). The pinned [`QobuzAudioProvider.kt`](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/qobuz/QobuzAudioProvider.kt) is 67,823 bytes with SHA-256 `c61d8657fa23f218005389b9c02f87f8b885d8c5547b4247d3fc5fb50f9e4e21`: identical to the Oct 16 inspected provider. This is a fresh revision check, not evidence of a newly rewritten resolver. [Source paths and hashes](source-manifest.json) also pin the health checker, persistent match model, manual override model/dialog and GPL license.

Catalog lookup and stream resolution are separate phases. Catalog adapters use `/api/get-music?q=…&offset=0` for Kenny/Squid/TrypT and `/search?query=…&offset=0&limit=30&region=…` for Jumo. Squid/TrypT send `Token-Country`; Jumo supports a limited region set and normalizes other choices. Search tries multiple terms/backends, then scores returned track rows. A matching ISRC adds 1,000 points, but it remains a scored candidate selection with title/version/artist gates and a 25-second duration tolerance when both durations exist. It picks the maximum score; it does not require one unique same-ISRC row or prove a unique release edition. The README's “without ambiguity” wording should therefore not be copied as a Burrata identity guarantee.

After a match, resolver-specific requests obtain media URLs with a quality ladder `27 → 7 → 6 → 5`, starting at the selected tier. The client recognizes captcha/403 and preview failures; previews short-circuit the ladder. Per-backend captcha cooldown is five minutes and hard host failure cooldown three minutes. Runtime endpoint overrides permit a user-supplied host. Native requests use blocking OkHttp `execute()` with 8-second connect and 9-second read/call limits; the source explicitly notes that a coroutine timeout cannot interrupt these blocking calls. These limits do not establish a small total budget across the whole term/backend/quality matrix.

Resolved URLs have a separate short-lived cache: `etsp` expiry minus 15 seconds if present, otherwise five minutes. HEAD/one-byte Range probes estimate length/bitrate; they do not prove sustained decode, seeks or expiry refresh during an active stream. The Room match record stores Qobuz ID and quality facts keyed by YouTube ID; a manual override supplies a direct `qobuz:track:<id>` identity and skips fuzzy search. Those concepts transfer, but Burrata should retain source/account/market provenance and source metadata fingerprints rather than treating a media-ID-only cache as permanent proof.

The [`health checker`](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/qobuz/QobuzBackendHealthChecker.kt) labels any 2xx response ONLINE and Jumo's check uses `/`. That measures reachability, not catalog JSON or stream readiness; an HTML frontend can pass that test.

## Fresh bounded catalog checks

Four anonymous GETs at approximately 16:43:49 UTC used the actual pinned catalog paths, query **Sasa Timmy Regisford**, JSON Accept headers, a 10-second socket timeout and a 1 MiB read cap. They used no cookies, tokens, stream endpoint or audio bytes. [Machine-readable observations](catalog-checks.json) retain URLs, UTC time, response sizes/hashes and errors. The timeout is a socket bound, not an asserted hard process-wide deadline; Kenny elapsed 10.292 seconds.

| Host | Fresh observation |
| --- | --- |
| Squid | Local DNS lookup failed before HTTP. |
| TrypT | HTTP 503, 302-byte HTML response. |
| Kenny | Read timeout after 10.292 seconds. |
| Jumo | HTTP 200, 53,791-byte HTML response on the catalog `/search` path; not JSON. |

These observations independently reproduce the earlier outcomes from this machine at this time. They do not prove global/permanent host failure, absence of the recording, or that every endpoint/region is unusable. They supply no catalog candidate to qualify. No signed media URL was requested, retained or decoded, so playback/seek/expiry behavior remains untested.

## Backend source, credentials and licensing

A concrete self-hostable API candidate exists: [`audio-music-streamer/Qobuz-DL-API` at `1134bc6662572afec514c7b3a88f50af94e0fd6c`](https://github.com/audio-music-streamer/Qobuz-DL-API/tree/1134bc6662572afec514c7b3a88f50af94e0fd6c), under MIT. Its README documents the same `/api/get-music` and `/api/download-music` family. Its `lib/qobuz-dl.tsx` performs `catalog/search` and signed `track/getFileUrl` requests against Qobuz with configured app ID, secret and user-auth tokens. It randomly selects a configured token per call. This establishes inspectable API implementation and credential requirements, not that any Meld host deploys it or that it works against today's upstream API. Do not copy random token rotation into an account-bound Burrata session.

The upstream [`QobuzDL/Qobuz-DL` pin `ada30543c28286a3ec3d55e6ccc444166f5420b4`](https://github.com/QobuzDL/Qobuz-DL/tree/ada30543c28286a3ec3d55e6ccc444166f5420b4) is also MIT; its README says default credentials do not work and a paying member token is needed beyond 30 seconds. [Pinned source and license hashes](backend-source-manifest.json) distinguish these public projects from the unidentified live deployments. The former `squid-wtf/squid-qobuz-dl` GitHub repository returned 404 from GitHub's repository API in this fresh check; that does not prove its code is unavailable elsewhere.

Meld is GPL-3.0, with notices retained in source; copying its implementation requires preserving its applicable license and attribution. MIT source reuse also retains its notice. Source licensing does not grant Qobuz service credentials or music-distribution rights. Qobuz's publicly hosted [API Terms of Use](https://static.qobuz.com/apps/api/QobuzAPI-TermsofUse.pdf) is explicitly dated September 2011 and describes Qobuz-issued app identifiers/secrets and user/region restrictions; it is historical primary documentation, not confirmation of today's developer enrollment or a reviewed agreement for Burrata. No approved Burrata app credentials or authorized test account were supplied or used. Self-hosting removes dependency on an anonymous mirror, but does not remove the upstream account, entitlement and service-access requirements.

## Concrete opt-in integration recommendation

1. Start with an isolated adapter accepting a user-configured HTTPS endpoint and an explicit authorized account/market scope. Keep credentials in the existing OS-keyring/session path, never a shared built-in token or public feed. Require schema-valid catalog JSON for health; distinguish reachable HTML, auth rejection, rate limit, unavailable catalog and stream readiness.
2. Separate durable match evidence from expiring stream leases. Store exact provider IDs, source fingerprint, ISRC evidence, version, account/market, backend and schema version. Multiple same-ISRC candidates or conflicting editions require explicit selection; text scoring produces review candidates. Never rewrite user tags/Track identity with resolver guesses.
3. Use one absolute caller deadline and drop-cancellable native requests, bounded response bodies and a global attempt budget. Reuse host circuit breakers, optional prewarming and quality downgrade; preserve a user's selected quality ceiling. Keep signed URLs redacted and in memory with bounded expiry. A custom endpoint must not receive Spotify/YouTube credentials.
4. Only after catalog qualification, run a small authorized stream trial through Burrata's existing native decoder: verify non-preview duration/format, initial audio, sustained playback, seek/range behavior, cancellation, expired-lease refresh and fallback without duplicate account requests. Expose the selected source and fallback cause rather than promise instant/silent lossless playback.

The implementation shape is feasible and useful; release qualification remains blocked on a working authorized backend and measured stream behavior. Nothing in this investigation changes the current release or promotes an anonymous resolver.
