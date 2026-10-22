# Host-network Bacc smoke

One smoke diagnostic with the frozen alias-cache BPM v3 binary, Bacc saved fixture, public settings, Balanced matching, and trace enabled ran through `exec_command` with `require_escalated`. The cold and warm attempts both succeeded and resolved the same YouTube recording (`rpCswezRrLs`). This verifies that the fixture/provider path can work from the host network; it is only one paired observation, not a performance cohort or general availability claim.

| Scenario | Outcome | Resolve time |
|---|---|---:|
| Cold | success | 10.207 s |
| Warm | success | 0.602 s |

Cold trace observed one `extract_and_validate_start`, one logical `extraction_start` and `extraction_end`, and two preview lookup starts/ends. Warm trace used the mapped direct YouTube path. Trace values are logical events/counters, not HTTP request totals. With one sample per scenario, no p95 is reported.

Binary SHA-256: `7a95ec034871cfbd205ed022fd634e7ae8c97d71e43f0a985fbf49eb25726f6a`; example source SHA-256: `02abc81d239dc70f713a61d06f170964cad6bf06bc05ee0432bbc680b00e85fe`. The [sanitized aggregate](bacc-v3-hostnet-smoke.json) contains the input manifest hash and trace summary. Raw reports and logs remain private in `.cache/matching-stage5-hostnet-smoke-20261021/`.
