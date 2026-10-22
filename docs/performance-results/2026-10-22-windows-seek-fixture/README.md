# Windows seek-fixture hang investigation

CI run [39750327939](https://github.com/anaxonda/burrata/actions/runs/39750327939), source `110edb9769e86b87fc4beddd10d22bfe2cfd5631`, was deliberately canceled after its second Windows library-test step exceeded a 45-minute diagnostic budget. It is not a passing qualification. Linux and macOS checks passed; the dependent Linux bundle job did not run.

The ordinary Windows suite passed 1,599 tests with 37 ignored in 96 seconds. The compatibility-feature pass compiled in 2 minutes 28 seconds and began running tests at 08:59:53 UTC. At 09:01:12 it reported `native_output::native_http::tests::stream_download_cancellation_interrupts_seek_waiting_for_range_headers` running for over 60 seconds. Other tests continued to finish; the job never emitted a final library result before cancellation at 09:43 UTC. Thus compilation was not the long-running phase.

Source inspection found an unbounded failure-cleanup path in this regression. Its two-second setup timeout could panic before canceling the reader. Tokio runtime teardown then waited for an unabortable `spawn_blocking` seek while the fixture origin could remain blocked in its second `accept()`. The log does not expose the exact first failing setup assertion, so this is a demonstrated source-level hang path, not proof of which setup event was missed in that attempt.

The production vendored `stream-download` cancellation patch already races the full download/seek future against cancellation. It was retained. The correction is confined to the test harness:

- Dedicated decoder thread with a completion channel, avoiding an unabortable Tokio blocking task during test teardown.
- Deadline-bounded origin accepts that observe cleanup; accepted sockets explicitly use blocking mode with read timeouts.
- Cleanup before reporting failed setup, with bounded completion waits.
- A forced missing-handshake regression that enters the actual cleanup branch and confirms both seek and origin completion.
- The positive test still requires cancellation within 500 ms after the origin confirms the real uncached range request, before fixture release can complete that request.

Native CI and release test steps now have 45-minute upper bounds. Their commands, runtime-off environment and test coverage are retained. A fresh complete cross-platform CI run is required; canceling the diagnostic run does not satisfy that gate. Local and final CI results are recorded in the release qualification audit.

## Local correction qualification

The full Linux native library suite passed **1,689 tests, 0 failed, 37 ignored**. The affected twelve-test HTTP fixture module then passed ten consecutive bounded executions (four test threads per execution), including the forced missed-handshake cleanup case. [Source/binary hashes and repetition results](local-validation.json). These are local regression checks, not Windows qualification or an estimated flake rate.
