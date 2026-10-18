# Import terminal outcome and unavailable-track notice — 2026-10-17

Two separate regressions were fixed and checked. First, a terminal native import result (`completed` or `partial`) remains authoritative when cancellation races with job completion, preventing the UI from treating a completed partial import as a cancellation and offering a misleading retry. Four new overview/detail cases cover completed-partial outcomes; existing explicit-cancellation coverage remains.

Second, a known no-match WebView skip notice now keeps its version reason together with one coherent action instead of truncating the action tail at the former 180-character boundary. Two App fixtures cover the notice and ensure sensitive URL material is redacted.

The focused App suites pass 311 tests across two files (48.42s); Svelte check is 0/0 and the production frontend build passes (6.39s). These are source-level regression checks. They do not establish successful live provider import/writeback or broad playback qualification. [Sanitized result](result.json).
