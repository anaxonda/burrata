# Matching performance cohort coordinator

`scripts/performance/matching_cohort.py` runs the built `playback_reliability`
diagnostic sequentially for each public fixture and each matching level. It
copies the fixture settings while changing only `matchingLevel`, and invokes the
diagnostic in `paired` mode so each measured cold attempt is immediately
followed by its warm attempt. Production defaults are 20 measured pairs after
three warmup pairs; `--smoke` runs one measured pair and zero warmups for a
quick harness qualification. Smoke output is diagnostic only.

By default it runs `exact`, `balanced`, and `flexible`. Use `--levels Balanced
Flexible` to run only selected policies for a bounded same-build comparison;
selected levels are listed in `frozen.levels` and under each case's `modes`.
Level names are case-insensitive, and duplicates or unknown names are rejected.

Create a JSON manifest with paths relative to the manifest:

```json
{
  "cases": [
    {
      "caseId": "bacc-saved",
      "track": "../../.cache/playback-v015-mac-20261021/bacc-saved-fixture.json",
      "settings": "../../.cache/playback-v015-mac-20261021/public-settings.json",
      "expectedSourceId": "optional-public-provider-id"
    }
  ]
}
```

Run it with:

```sh
python3 scripts/performance/matching_cohort.py \
  --binary target/release/examples/playback_reliability \
  --manifest /path/to/public-cohort.json \
  --outputdir /path/to/cohort-output
```

Add `--smoke` for the fast check or specify `--reps 20 --warmups 3` explicitly.
The coordinator records the binary, manifest, track, settings, and diagnostic
source hashes, the source revision, matching mode, run settings, process exit
codes, per-attempt failures and timing/resource observations. A failure remains
in its scenario denominator. It withholds p95 unless at least 20 measured
timing observations exist. It records cold/warm resolved provider-ID changes
within each pair and optional expected-ID match/mismatch totals.

Add `--trace` to set `BURRATA_PLAYBACK_TRACE=1` only in the diagnostic child.
The example records its unique `nativeOperationId` on every attempt; the
coordinator joins phase events to that ID. Events with no native operation ID
can join through a shared lookup ID only when one diagnostic operation owns that
lookup; unknown IDs and lookups shared by multiple operations are left
unassigned. It retains only an allowlist of logical phase names, numeric
duration observations, and the maximum observed resolver extraction-budget
counter. The `extraction_start` phase-event count is reported
separately. A zero counter snapshot does not mean no extraction when
`extraction_start` events are present; these measure different things. Warmup
events are excluded from measured summaries. These counters are not HTTP request
totals, and concurrent phase duration
samples must not be added as if the phases ran serially. Per-attempt RSS is
omitted because the available child RSS measurement is a process-lifetime high
water mark.

The diagnostic's raw JSON and stdout/stderr logs are retained as dotfiles with
owner-only permissions because they can contain URLs or provider details. The
shared `aggregate.json` contains an allowlisted subset and no URLs, request
headers, or credentials. The input settings are rejected when they contain
credential/profile/library fields; this coordinator does not read normal
application settings or the application profile. Supply only curated public
fixtures and a public settings JSON.

Without `--trace`, no phase data is included. The diagnostic emits a stable
native-operation key when trace mode is enabled, which the coordinator uses for
correlation. It never derives network request totals from phase-event counts.
