# macOS v0.1.7 playback diagnostics analysis

## Scope and privacy

This analysis is derived from the local v0.1.7 diagnostics export. The export itself remains outside this report. These files contain phase counts, aggregate timings, safe status codes, and encounter-order pseudonyms for fingerprints. They do not contain raw resolution IDs, fingerprint hashes, event timestamps, titles, artist names, URLs, or error text.

## Capture

- Build: Burrata 0.1.7 on macos aarch64 (aarch64-apple-darwin).
- Capture: 1682 events; 0 dropped of 2000 capacity; 48 resolution operations and 17 standalone events without a `resolution_start` phase.
- Runtime snapshot at export: webview engine, status `idle`, 0 buffering events, 0 recoveries.
- Settings: primary youtube; fallbacks youtube, soundcloud; matching balanced; fallback enabled True with 1500 ms delay.
- Helpers: yt-dlp 2026.10.10.233141; ffmpeg 8.1.3; ffprobe 8.1.3; JavaScript runtime 24.21.0; yt-dlp-ejs unavailable.

## Findings

- The 48 traced resolution operations ended with 26 successes, 15 failures, and 7 cancellations. Failed operations all used category `unknown`; this capture therefore does not disclose their underlying error cause.
- By the `resolution_start` provider field, `soundcloud` operations were 0 successful and 12 failed; `youtube` operations were 9 successful and 1 failed; `other` operations ended 17 successful, 2 failed, and 7 cancelled. This field describes the trace event's provider label, not a per-track final provider.
- `extraction_end` recorded 17 SoundCloud attempts: 1 returned `ok=true` and 16 returned `ok=false`; duration n=17, median 4634 ms, p90 6141 ms, range 3912–6836 ms. YouTube had 13 extraction attempts, 13 with `ok=true`; duration n=13, median 5986 ms, p90 8728 ms, range 5346–10441 ms.
- The direct-YouTube phase (`direct_youtube_end`) completed with `ok=true` in 9/10 events; duration n=10, median 325.5 ms, p90 822 ms, range 206–840 ms. This phase is distinct from the slower extraction phase above.
- HTTP response evidence is limited to `stream_probe_end`: 110 response events, 52 status 206 and 58 status 403, all labeled YouTube. For attempt 1, 75 responses occurred; 37 were 403. For attempt 2, 21 remained 403 and 14 returned 206. There were 37 retry-wait events, all status 403 with an expected 1500 ms delay.
- Some operations with a 403 probe response still ended successfully after additional candidate work, while others failed or were cancelled. A 403 is proven; whether it reflects expired authorization, source restrictions, a transient edge issue, or another cause is unknown here.
- Identity evidence: 13 full checks passed and 0 mapping checks were recorded; no failed identity-check event appears. Candidate-gate evidence covered 273 rows (84 accepted, 189 rejected), mostly low/review confidence. These are discovery/ranking decisions, not a count of pinned-provider identity failures.
- Resolution-end latency: overall n=48, median 4347.5 ms, p90 10720 ms, range 194–12278 ms; successful n=26, median 357.5 ms, p90 10810 ms, range 194–12278 ms; failed n=15, median 4879 ms, p90 8991 ms, range 4281–9621 ms; cancelled n=7, median 3259 ms, p90 6276 ms, range 279–6276 ms. Search start-to-end elapsed medians: YouTube n=17, median 1 ms, p90 870 ms, range 0–881 ms; SoundCloud n=14, median 4401.0 ms, p90 6502 ms, range 0–8641 ms.
- Fingerprint correlation is available in `aggregate.json` under `fingerprintCorrelation`: raw SHA-256 values are replaced with `track-NN` and `source-NN`. One track pseudonym recurs across multiple resolution attempts; the table correlates track/provider/outcome and explicitly paired track/source fingerprints. Probe statuses are operation-level and cannot always be assigned to an individual candidate when multiple candidates were examined.

## What this capture establishes—and what it does not

It establishes repeated SoundCloud extraction failures, a mixture of 403 and 206 HTTP probe responses for YouTube, retries after 403, and passing identity checks wherever those checks were emitted. It does not expose the actual yt-dlp error, the denied host, the redirect chain, whether the 403 target URL was stale or account-restricted, or a request-level network count. It also cannot compare helper versions: the report contains only one yt-dlp build (`2026.10.10.233141`), one ffmpeg/ffprobe build (`8.1.3`), and no yt-dlp-ejs helper. The WebView snapshot is idle at export and does not reconstruct prior playback state.

## Interpretation

The clearest actionable signal is split-stage: SoundCloud extraction is consistently slow and usually fails, while YouTube’s direct candidate phase is often quick but some extracted sources receive repeated 403 probe responses. The trace does not establish that either issue caused a specific user-visible failure because errors and stream hosts are absent. The next diagnostic capture should retain sanitized extractor error classes and redirect destination host class, plus a safe final playback-result event, to distinguish extraction failure from source-access denial without exporting URLs or raw error text.

## Integration follow-up

The development build now emits enum-only extraction failure categories and explicit HTTP statuses before stderr redaction. It also exports bounded WebView media/startup history and the actual UI playback context, addressing the idle Rust snapshot gap in this v0.1.7 report. Raw helper output and media URLs remain excluded.

Local comparison: public SoundCloud search for the reported Kendrick Lamar recording `luther` and exact-candidate format inspection both succeeded, returning AAC HLS formats at reported 96/160 kbps. This establishes local metadata/extractor access only; it does not prove source playback, matching helper environments, or the cause of the remote failures. Track Info now separates metadata provenance from the selected audio provider, and resolves generic catalog provenance where known, so its label no longer reads “Catalog catalog”.

A stronger same-version Linux comparison used bundled yt-dlp `2026.10.10.233141`, matching the report: exact-source extraction succeeded in 3,127 ms, selected `hls_aac_160k` / AAC HLS, retained the expected provider ID, and a separate three-second remote HLS decode succeeded in 1,164 ms. [Extraction evidence](../2026-10-22-integrated-followup/same-version-soundcloud-extraction.json) and [decode evidence](../2026-10-22-integrated-followup/same-version-soundcloud-decode.json). This rules out universal failure of that helper/source combination; it does not reproduce the Mac network, location, architecture or WebView execution. The development diagnostics now retain a fixed `extractorReason` label for geographic, subscription, provider-client, format and access errors, while preserving existing matching and queue policy.
