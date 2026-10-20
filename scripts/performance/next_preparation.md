# Native next-track preparation diagnostic

This opt-in Linux driver compares the existing debug preparation windows, 30 and
45 seconds, without changing production policy. It does not build Burrata,
publish a release, load a normal profile, or connect to an account. The default
command prints a plan and does not launch anything:

```sh
python3 scripts/performance/next_preparation.py
```

An exploratory run is three alternating pairs: 30/45, 45/30, 30/45. After reviewing
the retained failures and protocol evidence, a separately requested metric run
plans twenty transitions per arm. Neither phase automatically starts another
phase or promotes a result. The whole-run limits are 600 seconds for exploration
and 3600 seconds for metrics, followed by bounded cleanup. Unstarted rows remain
`not_run` if the budget expires; output files are never silently overwritten.

The following commands are examples for a later explicit run. They have not been
executed as part of implementing this driver. Close the current Burrata GUI
first: the driver refuses an existing instance and does not stop someone else's
app. Use a current debug executable; a release build ignores the diagnostic flag.

```sh
python3 scripts/performance/next_preparation.py --run-isolated \
  --binary .cache/verification-target/debug/burrata \
  --output /tmp/burrata-next-preparation-exploratory.json

python3 scripts/performance/next_preparation.py --phase metric --run-isolated \
  --binary .cache/verification-target/debug/burrata \
  --output /tmp/burrata-next-preparation-metric.json
```

Each arm creates one fresh `/tmp/burrata-launch.*` profile with the existing
isolation marker and empty Music root. It seeds a 90-second local WAV and a direct
YouTube track using the public manifest's exact video ID and independently
recorded duration. It does not copy cookies, credentials, mappings, queues, or
expiring stream URLs. The production resolver pins this explicit source; actual
formats are observed and pair deltas require equal selected formats. A default
Matthew run refers to the manifest's explicit video, rather than inferring that
it is acoustically equivalent to the Spotify catalog recording.

The verified launcher compiles the fixed frontend modules before app startup.
The child environment selects the native engine, protected frontend, phase
tracing, and the requested diagnostic window. The MCP observer requires the exact
isolated endpoint and `engine: native` before sending any playback command. It
mutes volume, selects the two-track queue once, and seeks the local control once
to 30 seconds. It then sends read-only state polls and waits for a natural next
occurrence plus five seconds of advancing native playhead. There is no later
seek, manual skip, automatic source change, or harness retry.

Every attempt stops only its owned observer and launcher process groups. The
next arm cannot start until cleanup is confirmed; abandoned readers/helpers
cannot overlap the following trial. The temporary profile is then removed. A
failed cleanup stops the run and retains the profile rather than deleting files
under a live app. A per-user lock prevents concurrent copies of this driver.

A failed setup stops the run immediately; the remaining scheduled arms stay
`not_run`. In particular, a stale executable is a launcher freshness failure,
not a measured playback attempt. The report retains a typed launcher category,
exit status before cleanup, readiness milestones, and a repository-relative
stale-source path where available. Bounded log fingerprints, partial redacted
traces, observer reports, resource samples, and separate stage outcomes survive
profile removal. Raw log text, signed URLs, and credentials are not copied into
the report. Cleanup failures are appended without replacing the original
failure. The original result file is never rewritten by a later run.

The report retains every planned attempt, original terminal failures, probe
denials, cleanup failures, source/format differences, and redacted preparation
events. The post-seek generation and operation IDs must correlate with the
30/45 policy, resolve, queue, and adoption phases. A missing trace, ignored flag,
or foreground fallback without preparation adoption makes the attempt
ineligible. Preparation remaining time is estimated from adjacent local
playhead polls. Reader queue age is reported separately; it is not called URL
age or remaining URL lifetime. Actual URL expiry remains unknown, and existing
production expiration/access checks and bounded refresh behavior are unchanged.

Future reports identify resource attribution as
`root-start-executable-basename-v2`. Only the selected app root's PID/start
identity is `app`. The sampler verifies identity around `/proc/PID/exe` reads,
caches each executable basename by PID/start identity, and recognizes explicit
WebKit and helper basenames. An unrecognized or inaccessible executable remains
`unknown`; its `comm` name cannot turn it into `app`. In particular,
`comm=MainThread` with a proven `node` executable is a helper. A recycled PID does
not inherit the previous process's executable role or CPU baseline. Loss of the
selected root identity fails the observation rather than adopting another root.

CPU and RSS sampling sleeps 200 ms plus collection overhead. Tree totals include
all four components: app, WebView, helper and unknown. Safe metadata records the
classification version, counts of seen identities and recognized basenames,
unknown counts and sampling limitations, without arguments or arbitrary paths.
Failed executable reads are cached as unknown. An exec within the same PID/start
identity can leave a cached basename stale; classification is a sampled process
role, not verified job/script attribution.

The report distinguishes control setup, steady control playback, preparation,
transition, and sustained next playback. CPU 100% means one logical core. Vite
is outside the application total; short-lived or reparented helpers can be
missed, and summed RSS can double-count shared pages. A transition interval
includes the estimated end of the control through the first second after
observed next playback. These samples can locate a preparation CPU spike, but do
not establish complete kernel child CPU usage.

The [archived six-arm exploratory run](../../docs/performance-results/2026-10-19-next-preparation/README.md)
used the legacy comm-only classifier. Its app/helper split is explicitly
unreliable because a Node executable named `MainThread` was counted as app. Its
original bytes and conclusions remain unchanged. The new classifier does not
retroactively repair those component histories; only their combined sampled
tree totals are usable within the documented limits.

Handoff delay is a 100 ms polling estimate against the control's expected end,
not a speaker waveform measurement or proof of sample-level gaplessness. Actual
network wire bytes are explicitly unknown: decoder reads and downloaded-range
high-water marks are not substituted. Network/expiry fault attribution needs a
separate controlled origin with byte counts; the driver does not manufacture
fresh URLs or weaken access checks. Descriptive p95 is withheld for exploration
and until twenty eligible observations exist in an explicitly requested metric
arm; failure rates continue to use all planned/attempted rows.

Offline tests cover natural progression, source/duration/occurrence ownership,
reorder and cancellation invalidation, held playheads, buffering, failure
retention, redaction, seed isolation, bounded plans, source changes, owned group
cleanup, and resource phase classification. A source-derived native contract
fixture checks the Rust snapshot, quality and MCP pagination field names; it is
not labeled as a captured runtime result. No GUI or provider is needed:

```sh
node --test --test-isolation=none --test-timeout=10000 scripts/performance/next_preparation_observer.test.mjs
PYTHONPATH=scripts/performance python3 -m unittest scripts/performance/test_next_preparation.py
```
