# Search continuation queue snapshot integration

A new App integration regression enables the off-by-default continuation option on a YouTube results page, filters out a short track and sorts the remaining rows by title. Clicking Beta queues the displayed `[Alpha, Beta]` snapshot with Beta current. Appending another page, reversing sort direction, and submitting a replacement query leave that queue unchanged while search results change.

The focused main-worktree test passes (1 passed, 339 skipped). This checks the real App/SearchPanel/queue wiring with a fake backend and media element. It does not qualify live provider pagination, audible playback, duplicate occurrences or queue-end radio. Existing full-suite evidence predates this added test; no new full-suite count is claimed.

Command: `npx vitest run src/App.integration.test.ts -t 'queues visible sorted loaded rows' --maxWorkers=1 --fileParallelism=false`.

A private negative control removed only the continuation toggle interaction. It failed at the intended queue assertion: expected `[Alpha, Beta]`, received `[Beta]`; setup succeeded. The temporary negative-control file was removed and the integrated test was left unchanged.

A mixed-route App integration test now passes alongside the queue test (2 passed, 339 skipped). It covers Directory → Search → pending album → Back, rejects the late album response, then restores folder filters/search query/album tracks through Back and Forward to the artist route without refetching cached album tracks. This does not qualify live keyboard/scroll/provider behavior.
