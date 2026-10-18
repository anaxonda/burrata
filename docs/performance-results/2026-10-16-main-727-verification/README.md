# Recorder cleanup and targeted asset queries

The main Linux development native suite passes 727 tests, zero failures, 14 ignored on the host in 8.39 seconds. The exact compiled sandbox run has 20 failures (19 localhost fixtures and the deadline/process-reaping fixture); all pass on the same-binary host run. Seven contracts pass. The frontend checkpoint remains 508 passing tests across 56 files with clean Svelte checks and a passing production build.

This covers explicit kill/await on Linux recorder EOF and read timeout, and existing cancellation/deadline paths. Cancellation uses a producer-release handshake and asserts the cancellation reason; the EOF test asserts its specific reason. An earlier full host attempt failed three fixtures whose 30ms fake-Python read budget was too small. Those failures are retained in local logs, not counted as passes. Normal fake reads now allow 2 seconds, while the dedicated timeout case remains 500ms.

Autotag, tag recovery, selected-recording file operations and file-manager validation use scoped indexed lookups while retaining error/not-found behavior. Initial Library full-asset loading and broader analysis/artist paging remain separate open work.

The normal development GUI executable builds successfully and is retained unlaunched; its hash is in aggregate.json. The existing 472b58f8 GUI and owned login window remain untouched. These tests do not qualify live Burrata recognition, provider sessions, release performance, or macOS/Windows behavior.
