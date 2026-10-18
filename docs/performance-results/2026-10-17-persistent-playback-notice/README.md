# Persistent unavailable-track notice and opener advancement

The current frontend passes 307 App integration tests, Svelte check (0 errors/0 warnings), and the production frontend build. Native candidate `297a78ff5c54` previously passed 944 tests (21 ignored). The structured notice expiry distinguishes persistent playback failures from transient success messages; dismissal and replacement remain supported.

A fresh isolated WebView run passed `terminal_opener_to_exact_local_successor`: exact Sasa / Timmy Regisford / Main Version (281 seconds) was unavailable, the original two-occurrence play request was accepted, and the verified synthetic local successor advanced 5.378 seconds over 5.419 seconds wall time (7 actual media observations). The guarded accessibility reader found one actionable matching notice with zero extraction errors; finally Pause succeeded. Root reviewed the 954×1036 screenshot: the reason and source-action guidance remain visible and wrap correctly, with no overlapping playbar controls. This is one exploratory run, not p95, physical audibility, native mixer, acoustic identity, or successful Sasa playback qualification.

The preceding run remains failed: playback advanced but the notice reader returned false despite a visible banner. PyGI Accessible.get_text shadows Text.get_text; the reader now invokes the unbound Text API. Four offline reader tests and runner syntax check pass. The new passing run is separate; no failed result was overwritten.

Evidence: `.cache/qualification-persistent-notice/opener-live.json` (failed), `.cache/qualification-notice-reader-fixed/opener-live.json` (passed) and adjacent private screenshots. Product App SHA `25e77d9153cb402bc77cbcceaa06f2a5544c8b258a916783a8570cec5a1a5023`; immutable native SHA `297a78ff5c5425b5270f57dce292aa9ad373be1458358832ae2983ad1af91251`. No provider writes occurred. The first fresh launch failed before app start when temporary quota was exhausted; only rebuildable inactive-test caches were removed, preserving profile data and fixtures.

Cancellation, actual Kaufman album advancement, matched-recording repetitions, full performance and platform qualification remain open.
