# Native Spotify auth request duplication

Historical baseline: the request counts below precede the promoted native memory-only token coordinator. That coordinator is present in main and the running `472b58f8` executable; see [the later implementation checkpoint](../../meld-session-investigation.md#isolated-implementation-follow-up-2026-10-16). These baseline counts are retained for comparison and must not be presented as current token-refresh behavior. Public-registry reuse and live refresh/resource qualification remain separate checks.

The existing mocked two-page artist-release cursor fixture was instrumented in a temporary test copy. Its assertions passed. Two provider jobs made two requests each to the public TOTP registry, server-time, public query-hash registry and token endpoint, plus two profile queries and two artist-release page queries. This is a request-count result, not live latency, CPU or account qualification. The adjacent JSON contains no credentials or request bodies.

The parent currently starts a new Node worker per job, and each job initializes an empty registry store and a new provider context. This releases worker memory after completion but prevents expiry-aware token reuse across pages. Meld's inspected SpotifyTokenManager instead retains token expiry and serializes refresh.

A safe replacement should scope in-memory state to provider, confirmed account, credential fingerprint and session generation; honor provider expiry with skew; cache nothing when expiry is absent or invalid; coalesce refresh while preserving independent caller cancellation; and clear state on stop, account switch or credential change. Tokens must remain inside the native session boundary and never appear in logs, public tool results or disk caches. Public registry metadata can have a separate bounded TTL. This is an implementation gap, not a completed optimization.

The full relocated test copy also had one staging-fixture failure because moving the file changed its relative repository root. That harness-location failure is retained as a limitation and does not establish a product regression. Production worker test results remain separate.
