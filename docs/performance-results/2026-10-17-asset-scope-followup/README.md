# Asset-scope startup follow-up — 2026-10-17

The built candidate `45eca63f814d` passed the 921-test native suite (0 failures, 17 ignored), the nine startup-harness tests, and the development build. It was restored as the normal GUI; a private-free comparison confirms the 13-entry queue order, current index, paused position and controls are unchanged.

The fresh 50k synthetic startup-only run took 15,758.5ms to register. Its trace records 50,000 explicit grant calls, zero reused grants, 6,933ms in asset grants and endpoint registration at 11,257ms. The prior optimized run recorded a 15,257ms harness wait, 6,305ms grants and endpoint registration at 10,930ms. This pair shows no benefit on the external fixture; the candidate was slower on all three comparisons. The run is not apples-to-apples with the prior page-matrix workload: it has zero page reads and no projection-readiness wait, and is only a single run. Refine this scope candidate before performance promotion; do not infer broad startup performance.

[Sanitized startup-only result](startup-only-result.json) · [queue preservation summary](main-preservation-proof.json)
