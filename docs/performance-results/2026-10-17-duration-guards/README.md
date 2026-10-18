# Duration response guards — 2026-10-17

The public-metadata duration worker now accepts only scalar duration values: numbers and numeric strings. Booleans, arrays and objects are rejected. Row eligibility is applied per item, preserving manual, chapter-derived and already-known durations even when the provider returns the same recording ID.

The direct worker suite passes 55 tests. The pinned native source suite passes 942 tests, with 0 failures and 21 ignored (16.75s). The source pin is `513`; the application build was still running at report time, so no built-candidate or live-runtime claim is made. [Sanitized result](result.json).
