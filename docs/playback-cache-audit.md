# Playback resolver cache audit

This is an implementation snapshot of the resolver paths in `acquisition.rs` and `playback_sources.rs`, compared with the investigation goals in [playback-performance.md](playback-performance.md). It does not measure latency or establish that a persistent client would improve playback.

## Existing reuse

- Candidate searches are cached in memory for five minutes (128 entries), keyed by normalized track fields, providers, result limit, and yt-dlp/runtime settings. Per-key async gates coalesce concurrent misses (`acquisition.rs:78-80, 1391-1394, 1725-1807`). Raw audio search results use a separate two-minute, 64-entry cache (`acquisition.rs:80-81, 1472-1475`).
- Candidate inspections use a process-global `OnceLock` cache with a 90-second TTL and 128-entry cap, keyed by provider, candidate URL, executable profile, and runtime settings. It also uses per-key gates (`acquisition.rs:87-89, 1396-1414`).
- Resolved preview audio is cached in the `AcquisitionManager` for at most five minutes and 64 entries. The key is provider + candidate URL + yt-dlp/runtime paths. Its single-flight gate is per key; the second caller waits for the first, then reads its cached result (`acquisition.rs:83-84, 1466-1469, 1603-1690, 1809-1817`).
- Chapter-source files have a separate bounded, expiring cache (15 minutes, 16 entries; `acquisition.rs:85-86`). In the output path, decoder/reader and downloaded ranges are reused across supported native seeks; this is separate from resolver URL/client reuse (`playback-performance.md`, “Implemented behavior”).

## Cancellation and coalescing

Preview-audio callers carry their own `watch::Receiver`. A canceled waiter exits while waiting for the per-key gate; it does not cancel the caller that currently owns the gate. If the active resolver's own receiver is canceled, its resolution future is dropped, the guard is released, and another waiter can resolve. This gives callers independent cancellation, but coalescing is serialized work rather than a shared result future (`acquisition.rs:1612-1662`). The same keyed-gate pattern is used for candidate search, inspection, and raw audio search.

The resolver's foreground provider race also has a shared child cancellation signal: when that race completes, it signals the sibling attempt and drops unfinished futures (`playback_sources.rs:83-110`; described in `playback-performance.md`). That is cancellation within one playback request, distinct from cross-request cache coalescing.

## Identity and expiring URLs

Search candidates are not treated as verified recording mappings: raw results are explicitly marked as review/unverified (`acquisition.rs:980-987, 1050-1058`). During playback, candidates that did not already match the requested recording are checked again after extraction metadata is attached; a mismatch is rejected before returning the source (`playback_sources.rs:272-280, 289-305, 389-401`). Access probing checks actual bytes before a preview audio result is cached (`acquisition.rs:1967-1992`).

Verified recording-to-provider mappings now have a separate SQLite table. They store only stable YouTube/SoundCloud IDs and validated permanent public URLs, never signed stream URLs. A fingerprint includes title, artist, album artist, version, duration, ISRC, and chapter identity/range. Changed fingerprints invalidate remembered mappings; merge/deletion cleanup removes obsolete recording mappings.

A mapping is saved only after accessible extraction supplies a title and the exact expected provider ID, with recording matching applied. Reuse performs extraction and recording/ID validation again. A remembered-source failure consumes one of the provider's two foreground candidate slots and is deduplicated from subsequent search results; another eligible same-provider candidate can still be tried. Definitive unavailability or an identity mismatch invalidates the mapping, while cancellation and transient failures preserve it. New storage/resolver regression fixtures cover these conditions; final native test qualification is recorded separately.


The preview cache reads case-insensitive absolute `expire`/`expires` timestamps and AWS/Google `X-Amz-Date`/`X-Goog-Date` plus `Expires` lifetimes. It chooses the earliest recognized expiry, subtracts a 30-second safety margin, and caps the TTL at five minutes. Expired or incomplete AWS/Google signatures are not cached; unknown signing schemes still use the five-minute cap (`acquisition.rs:1694-1732`). Deterministic tests cover signing-time anchoring, expiry, incomplete signatures, and multiple expiry limits. Fresh retry explicitly invalidates the same candidate key before resolution, while later transport failures also invalidate it (`acquisition.rs:2990-3013`, `playback.rs:1214-1221`). Invalidation uses one global epoch, so an unrelated in-flight lookup may be prevented from caching after any invalidation; this conservative behavior is documented in `playback-performance.md`.


## Client reuse gap

The default public Monochrome adapter now shares a process-wide `reqwest::Client`; its no-proxy, no-redirect, 15-second timeout policy is retained, while custom-base clients remain separately constructed for tests (`monochrome.rs:8-44`). The preview-access probe still constructs a new client for each resolved URL because it pins DNS resolution to that URL's validated public addresses (`acquisition.rs:1967-1989`), and yt-dlp extraction still runs through the resolver's process path rather than a persistent extraction session. The playback output layer retains its network runtime/reader for supported seeks, but does not reuse the resolver's extraction or preflight client. Whether keeping those remaining resolver clients or a provider session alive would help remains unmeasured; DNS pinning and provider session semantics would need to remain part of any comparison.

The practical distinction is: current code reuses search results, inspected candidates, accessible stream URLs, the default Monochrome HTTP client, and some playback decoder state; it persists verified recording mappings separately from expiring URLs, but does not maintain a general long-lived extraction session. URL expiry handling is bounded and safety-margined for recognized absolute and AWS/Google signatures, not universal across signing schemes.

## Follow-up probe review (2026-10-15)

The resolver's head/tail range probes and native decoder HTTP open are separate operations. Preflight requires real nonempty audio bytes, correct 206/Content-Range behavior for known lengths, and a tail check for larger streams; it also retains transient-denial retries and public-address pinning. Its response is not a decoder-ready reader, so deleting the probe would remove protections rather than demonstrate redundant work. Existing native tests verify buffered seeks do not reopen the stream and unbuffered seeks retain range semantics. Current source-ready observations do not isolate pinned-client setup or probe cost. No probe removal or client reuse change is justified by this review; measure those costs separately before adopting reader-integrated validation.

## Independent candidate-search waiters — 2026-10-15

A canceled preview/playback lookup could wait on the shared candidate-cache mutex until the current same-key owner finished. The gate wait is now cancellation-aware, including the playback prefetch entrypoint. The regression blocks an owner, cancels only its waiter, requires the waiter to return within 100 ms while the owner remains live, then verifies the owner result remains cached. The full native checkpoint passes 637 tests (13 ignored). This establishes cancellation behavior; it does not establish a startup latency percentile or steady-playback CPU improvement.
