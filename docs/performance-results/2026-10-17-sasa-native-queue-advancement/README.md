# Sasa native queue advancement — 2026-10-17

Three isolated native runs passed a queue progression case with a known-version-unavailable entry followed by a distinct duplicate local occurrence. The next local occurrence produced five seconds of both wall time and decoded consumption in every run. Observed transition upper bounds were 4,735ms, 133ms and 113ms; playback ended paused in all three.

A separate private capture review showed the unavailable reason and an Open catalog action while the following local occurrence was playing. The existing notice expires after about 5.5 seconds, so this confirms short-lived visibility rather than persistent history. No private capture or track identifier is published.

This is native-path evidence only. Fresh profiles did not control provider/preparation warmth, so these times are not cold startup or p95. Duplicate-occurrence advancement does not verify exact recording correctness. The same case remains to be checked on default WebView, and the separate album issue remains open. [Sanitized result](result.json).
