# YouTube playlist unavailable entries

Ordinary playlist reads and fresh mutation preflight request `browseId: VL{playlistId}` with `params: wgYCCAA=` to include unavailable entries. This preserves private/deleted occurrences rather than silently omitting them.

Protocol evidence: yt-dlp commit [`897341241f2bfb2439fe0367f4c7c9ccd29a3828`](https://github.com/yt-dlp/yt-dlp/blob/897341241f2bfb2439fe0367f4c7c9ccd29a3828/yt_dlp/extractor/youtube/_tab.py), method `_reload_with_unavailable_videos`. The reviewed file has SHA-256 `736ac305841cf6f070cb44a2d7060771e9743f30b6ef38fe0304fc419877b970`. Only the observed request contract is used; no implementation was copied.

The earlier browseId-only experiment omitted this parameter without established semantics. Combined with the corrected command-wrapper cursor extraction, it collected 210 entries against a reported total of 240. The provider did not return unavailable rows in that read; it does not prove why the remaining 30 were absent. The restored request requires fresh live verification.

V2 playlist cursors retain the initial reported total and cumulative occurrence count. Completion requires an unambiguous terminal page and an exact count match. Missing totals and legacy V1 cursors remain unqualified. Fresh writeback preflight retains its separate validation.
