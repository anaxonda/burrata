# Public Spotify duration repair: live main-library verification

Development executable SHA-256 `25ac105af5f45c6678f63d3022ce0933ffac3f6e80b6c58b6fdfc8461b324471` passed the verified launcher handshake with 79 authenticated tools and its frontend mounted. One Burrata GUI instance remained on the preserved main profile. Native provider status before and after reported neither Spotify nor YouTube confirmed; the repair used no account, external browser, companion, login or remote playlist mutation.

An online SQLite backup and known-duration baseline were saved privately with owner-only permissions before repair. Two explicit MCP actions filled all 396 missing Spotify durations among 578 Spotify listings:

| Action | Checked | Filled | Scan complete | Elapsed including MCP setup |
| --- | ---: | ---: | --- | ---: |
| First bounded pass | 300 | 300 | no | 55,099 ms |
| Second bounded pass | 96 | 96 | yes | 17,907 ms |
| Exhausted repeat | 0 | 0 | yes | 354 ms |

All 182 previously known durations remained exactly unchanged. Of 396 newly filled values, 389 retained fractional seconds. Raw duration gaps fell from 396 to zero. A later actual-app read traversed all 578 Spotify rows in three 200-record pages: every duration was present and global ascending whole-second order held after background reconciliation (`paged-read.json`). Comparison against the backup found no changes to 4,898 manual-metadata rows, 5,073 ordered playlist occurrences, 7,357 collection memberships, or the duration fields of 15 local assets. Aggregate evidence is in `preservation.json`; track IDs, names, cookies and response HTML are excluded.

The repair uses only a fixed public HTTPS embed URL and accepts a duration after matching exact Spotify ID, URI and track entity type. It disables redirects, sends no cookies or bearer credentials, bounds bodies to 2 MiB, and uses bounded timeouts. One action handles at most 300 IDs under a 60-second lookup budget, with five concurrent requests. It shares the authenticated repair’s generation/commit gate, keeps a separate public cursor, and never clears known values. Explicit cancellation drops the pending lookup; RAII cleanup prevents abandoned requests from leaving repair permanently busy. Rate limits stop the batch without cursor advancement and impose a bounded cooldown. Verified unavailable entities are skipped without a write; malformed or mismatched schemas stop honestly.

Settings → Connections contains an explicit repair control, status refresh and Stop. It reports partial slices separately from exhausted scans, warns that unavailable durations may remain, and does not start network repair on mount/import. Newly eligible imports rearm the public cursor without starting anonymous work. Existing authenticated repair remains account-checked and automatic after confirmation.

Default and custom-protocol native suites each passed 611 tests / 13 ignored. The 51-file frontend suite passed 417 tests after mounting the control. Svelte check found no errors/warnings; frontend build, seven IPC/schema contracts and three MCP bridge tests passed. Tests cover exact identity/units, body/redirect bounds, cancellation, dropped futures, deferred account work, cooldown, unavailable-versus-schema cursor behavior and guarded persistence. This proves this library’s public metadata repair path, not supported-API stability, all provider imports, live writeback or playback performance. Spotify’s embedded-page schema is undocumented and may change; source mismatches fail closed.
