# Nighttime in the Backyard: catalog enrichment and playback qualification

## Current checkpoint (2026-10-20)

The saved library row now plays through automatic Bandcamp fallback. The normal-profile WebView resolved `bandcamp:2065435071` in 7.123 seconds and advanced past 24 seconds with no media error. An isolated live regression independently refreshed access, verified the complete twelve-track Apple/Bandcamp album crosswalk, decoded five seconds of PCM, and confirmed saved tags were unchanged. Its cached foreground resolution took 667 ms; first PCM after resolution took 344 ms. These are separate observations, not a combined end-to-end benchmark.

Explicitly selecting the public label YouTube source `yK-GAynXCok` now also plays in the normal WebView: resolution took 7.805 seconds and media advanced beyond ten seconds. The direct-resolver experiment fell back to the ordinary extractor; this does not qualify the direct route or establish an automatic connection from the saved two-artist row to that upload. Bandcamp remains a fallback, not a preferred source installed for this recording.

The following sections preserve earlier investigation evidence and limitations **at those earlier checkpoints**. Their statements that playback/fallback was not implemented have been superseded by the checkpoint above. General macOS/Windows, adverse-network and release-installer qualification remain outstanding.

## Earlier catalog investigation

The reported saved radio row has no duration, album, ISRC or provider recording ID. A bounded independent Spotify lookup now supplies 197.76 seconds for the exact title and both credits, Lucas Santtana and Arto Lindsay. It changes an ephemeral playback copy and caches catalog evidence; it does not save tags or change the queue.

The first two-second diagnostics timed out. Free-text catalog search also returned unrelated recommendations and further pages. A quoted Spotify `track:` title query returns five rows with explicit terminal paging and complete parsing coverage; the exact full-credit row is distinct from Tostex credits and remixes. The opportunity is now at most four seconds inside the original playback deadline, with eight seconds reserved for normal resolution. All page/artist/version/account/cancellation checks remain required. The real public response is covered by a regression test.

The corrected query initially still returned no enrichment: a stale packaged `provider-operations.js` overrode the newly embedded script and omitted `rawCount`/`unrecognizedCount`. Safe stage diagnostics identified that discrepancy. Refreshing generated resources produces successful live enrichment in 2457 ms. The development launcher now refreshes packaged provider assets after validating executable freshness. This was a development resource mismatch, not proof of a published installer defect.

Playback is **not yet fixed for this recording**. An isolated resolver check using the independently retrieved duration still rejects its extracted candidate, which is the Tostex recording on Release - Topic, YouTube `r38JuTiyFu8` (198 seconds). Its fresh full credits differ, so rejection is correct. The likely original label upload `yK-GAynXCok` has title “Lucas Santtana - Night Time In The Backyard”, 198 seconds, no structured artist/track metadata and no verified-channel flag. Its description credits Lucas Santtana and Arto Lindsay as writers and names Sem Nostalgia. That is a research lead, not a verified automatic recording binding. No manual mapping or relaxed guest check was installed.

The full feature-enabled native suite passed 1336 tests, 0 failed, 33 ignored after rerunning with loopback permission; the restricted run had 74 fixture permission failures. Provider adapter tests passed 18/18. The final diagnostic-only logging addition was compiled and exercised live after that native suite. Build and test logs remain in `/tmp/burrata-catalog-duration-*`; the checked-in summaries exclude credentials, signed URLs, account identities and the user's library. The isolated resolver check ran during compilation and is not latency qualification. No device-audibility or successful full-playback claim is made.


## Follow-up: a verified public audio reference

A fresh public Apple catalog search returns track `1677838124`, “Nighttime in the Backyard”, Lucas Santtana & Arto Lindsay, album Sem Nostalgia, duration 197.760 seconds. It independently distinguishes the Tostex performance and Deerhoof remix. The artist's Bandcamp album lists track `2065435071`, “Night-time in the backyard”, also 197.760 seconds, with an ordinary public MP3 stream.

Both streams decoded successfully to mono 8 kHz PCM. Removing one second from either edge of Apple's public preview leaves 27.929125 seconds. An FFT cross-correlation against the entire Bandcamp stream peaks at offset 55.9845 seconds, normalized correlation **0.9920719478490855**. This is strong empirical evidence that the differently credited/titled Bandcamp stream contains the catalog recording; it is a single-case research measurement, not a calibrated general acceptance threshold or full-length identity proof. Audio, signed URLs and raw responses remain outside the repository.

References: [Apple exact recording](https://music.apple.com/us/album/nighttime-in-the-backyard/1677838121?i=1677838124), [artist's Bandcamp album](https://lucassanttana.bandcamp.com/album/sem-nostalgia-2021-repress).

A fresh label-upload extraction succeeded, but its three tested audio formats failed to open through the standalone FFmpeg probe. No successful label-upload audio comparison is claimed. A fresh exact Spotify REST read again returned HTTP 429; no repeated polling was performed.

The practical implementation path is a bounded, on-demand audio-reference verification fallback after ordinary identity matching fails. Resolve the saved row to the exact full-credit catalog entry; use a permitted public preview as the reference; compare an aligned candidate audio sample; reject insufficient, ambiguous, remix/live/cover or poorly correlated evidence; and cache a recording-scoped connection including both identities, reference provenance and comparison method/version. Revalidate stream access independently on replay. Discovery may search harmless title spellings and reduced artist queries, but those queries never authorize dropping guest checks. Bandcamp is a demonstrated usable candidate here; automatic discovery and production playback promotion are not implemented (Bandcamp playback remains default-off). A YouTube candidate still needs the same audio verification or another independent recording binding.

Also remove the optional exact Spotify hydration when a complete search result already establishes the sole full-credit recording and valid duration: lack of an optional ISRC alone should not force a rate-limited REST read. Preserve hydration when required identity/duration evidence is missing or the saved row has an ISRC constraint.

No library tags, queue entries, manual source pins or production identity gates were changed by this investigation. The reported library row is not yet automatically playable.
