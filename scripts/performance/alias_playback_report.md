Alias playback trace report
==========================

`alias_playback_report.py` reads captured `burrata_playback_phase {JSON}` log
lines offline. It emits summary JSON with sample counts, median, nearest-rank
p95, worst latency, cache/discovery categories, joined callers, metadata request
counts, and known response bytes. No live benchmarks or measured performance
claims are supplied by this change. Fixtures in the unit tests are synthetic.

```sh
python3 scripts/performance/alias_playback_report.py \
  --candidate /tmp/candidate.log --baseline /tmp/baseline.log \
  --label alias-playback --build release --warmup 3 \
  --output /tmp/alias-report.json
python3 -m unittest discover -s scripts/performance -p 'test_alias_playback_report.py'
```

Capture baseline and candidate in independent, comparable runs using the same
build type, machine, source mix and cache conditions. For a small investigation,
use three exploratory attempts followed by at least twenty measured attempts
per condition. `--warmup 3` excludes the first three distinct resolution IDs in
each file in log order; default is zero. Keep separate files for cold, warm,
concurrent and failure conditions, and for debug and release. A single trace
cannot establish exact causal overhead. Input paths, trace IDs, URLs, titles,
and arbitrary outcome strings are not copied into the summary; choose a
non-sensitive `--label` yourself.

Caller waits are measured from `alias_lookup_end.details.durationMs`. Worker
wall time comes from `alias_discovery_end.details.durationMs`, once per
`sharedLookupId` (falling back to resolution ID for older logs). Request timings
and bytes come from `alias_metadata_request_end`; exact repeated request events
within a shared worker are deduplicated by timestamp, elapsed time and details.
When request events are absent, discovery-work/end aggregate `metadataRequests` and
`bytes` are used instead. Unknown bytes are not inferred. Missing phase timings
produce zero samples and null statistics. Partial traces remain partial.

Worker intervals contain request time, and caller waits contain worker time;
these metrics overlap and must never be added as total overhead. Resolution
duration uses `resolution_end.durationMs`, falling back to envelope `elapsedMs`.
Candidate recheck duration is reported separately. Discovery category means a
discovery start/end was observed for the caller; warm means cached aliases were
present without a stale flag; skipped means neither was observed, which can
also reflect incomplete instrumentation. It does not prove zero alias work.
The report deliberately makes no first-audio claim: correlate first audio with
the playback qualification tools when collecting end-to-end evidence.

Provider-session hydration is reported separately from direct public-catalog
HTTP. Its elapsed time is included in worker/caller intervals, but internal
provider HTTP request counts and bytes are unavailable and are not reported as
zero. A hydration invocation can use cached evidence.
