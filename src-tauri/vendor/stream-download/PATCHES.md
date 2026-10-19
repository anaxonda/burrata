# Burrata patches to stream-download 0.24.4

Source: the crates.io `stream-download` 0.24.4 release, checksum
`3838c412a6dbb469e857b5148043dbb54d01dce49cace8d46709b795cc7697a3`.
Upstream: <https://github.com/aschey/stream-download-rs>.
The upstream MIT and Apache-2.0 licenses are included unchanged.

Only `src/source/mod.rs` differs from the published library source.

## Sparse seek gap selection

`Source::handle_seek` previously searched for the first missing range starting at
`min(writer_position, requested_position)`, then raised the range start to the
requested position while keeping that earlier gap's end. When a backward seek
left an earlier hole and a subsequent seek moved forward past it, this generated
a reversed HTTP range. Burrata's strict native HTTP client correctly rejected
that invalid request. A local WAV fixture reproduces the sequence 0 → 25 → 1 →
28 → 0 seconds while the earlier hole remains unfilled.

The patch searches for missing bytes at or after the requested position. Earlier
holes remain tracked for the existing background gap-filling logic. A queued
seek whose target is already cached, or is EOF, wakes its waiting reader without
changing the active writer or making a network request. The existing HTTP client,
byte-range validation and storage behavior are unchanged.

Regression tests exercise the sparse backward/forward sequence and cached/EOF
targets directly against `Source::handle_seek`. Burrata's native integration test
`unbuffered_http_seek_uses_ranges_and_keeps_recording_position` checks the same
sequence using a local HTTP origin that honors bounded ranges and validates the
decoded recording position. `native_http` tests continue to reject reversed
requests and incorrect responses.

## Cancellation while range response headers are pending

The original download loop checked its cancellation token between iterations.
Once its seek branch awaited `handle_seek`, cancellation could not interrupt the
pending HTTP range request. An established reader could therefore remain blocked
on an uncached seek when the origin accepted the connection but withheld its
response headers. A direct HTTP-client future cancellation test did not cover
this interaction with the download task and its blocked reader.

`Source::download` now races the entire `download_inner` future against the same
cancellation token, with cancellation checked first. Cancellation drops an
in-flight range or reconnect future, then runs the existing interrupted-download
finalization and notifies waiting readers. It does not mark missing bytes as
available or relax HTTP representation and byte-range validation.

The integration regression
`native_output::native_http::tests::stream_download_cancellation_interrupts_seek_waiting_for_range_headers`
uses an actual `HttpStream`, `StreamDownload`, its real cancellation token and a
blocking seek. Its local origin first makes the reader usable, then receives the
uncached range `32768-65535` and sends no response headers. The client has only a
connect timeout; the test therefore cannot pass through a short request timeout.
The confirmed pre-fix failure left the seek blocked beyond the test's 500 ms
bound. With the fix, cancellation completes that seek with an error within the
bound and closes the pending HTTP connection. Cleanup releases the fixture even
on failure, preventing a leaked test decoder thread. This is a local transport
and cancellation regression, not physical audio-device qualification.

Remove these patches after a pinned upstream release fixes both behaviors and
passes these regressions. No upstream issue has been submitted by this task.
