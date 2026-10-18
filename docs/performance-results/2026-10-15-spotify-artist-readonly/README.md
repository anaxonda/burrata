# Spotify artist read qualification

The live first artist-tool request reported `RATE_LIMITED` after 1806 ms; it returned no artist or album pages. No automatic retry was made. This report does not identify which nested account/catalog request was rate-limited, and it does not establish that artist browsing works. Sanitized status subsequently showed the existing Spotify session confirmed and native-ready; YouTube remained unconfirmed. These flags are session state, not a fresh provider account validation. The driver `scripts/performance/verify_spotify_artist_readonly.mjs` uses only the retained isolated confirmed session and public artist/release IDs. It records counts, timings, artwork availability and fixed failure flags; credentials, account IDs, raw errors and source URLs are excluded. There are no imports or remote writes.

`public-poso-search-ui.json` separately verifies the reported track search through the actual main-app search UI: `poso s’agapo` returned six Monochrome results. It does not verify playback or authenticated Spotify artist browsing.

The new read-only MCP commands use the same artist/album service as the UI. Exact 22-character Spotify IDs and bounded canonical decimal-string page cursors are validated, and returned cursors can be passed unchanged. This adds observability without replacing provider account or response identity checks.

Development executable SHA-256: `c32e0ad889e690594b1c0b8d12a01b544585b3d2e1447e0c37874bbdb699c2e5`. The full native suite passed 633 tests with 13 ignored. Four MCP transport tests and seven contracts pass. The isolated app mounted successfully and advertised 81 tools. This is Linux-only, single-attempt evidence, with no playback or writeback qualification. A later attempt should observe provider cooldown rather than loop on rate limits.

A subsequent code audit found that the tested worker revision also used `RATE_LIMITED` for internal read timeouts. The retained report contains no HTTP status or phase, so it cannot retrospectively prove HTTP 429. The later `529e1c47…` rebuild separates `REQUEST_TIMEOUT` and genuine 429 errors, with deterministic regression coverage. No live artist request was repeated for this correction; live artist paging remains unqualified.

## Diagnostic follow-up

`diagnostic-followup/report.json` records one later request on `529e1c47…`: 1596 ms, phase `catalog_rest`, HTTP 429, retry-after 29 seconds, `requestTimedOut:false`. This time the diagnostic evidence proves an actual REST throttle. No retry loop, account confirmation, import or write followed. The isolated provider window was closed and the same current executable restored as the sole main-profile instance, with the mounted frontend and 81 tools verified. The original ambiguous artifact remains unchanged. This does not qualify artist metadata, album pages or complete native artist browsing.
