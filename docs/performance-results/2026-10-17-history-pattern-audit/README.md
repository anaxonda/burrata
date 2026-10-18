# Bounded reachable-history pattern audit

A read-only audit enumerated 35 commits and 5,820 objects reachable from local refs. Of 5,493 blobs, 5,471 UTF-8 blobs were searched with the existing worktree scanner's patterns (27,989,301 UTF-8 bytes). Twenty-two non-UTF-8 blobs were skipped; no objects exceeded the configured size or total scan budgets.

One historical Google API-key pattern match appears in a YouTube Music request's client-key query parameter. Its call-site context is consistent with an embedded public client key, rather than evidence of a session credential, but does not establish validity, intended restrictions or account-secret status. The matched value is not reproduced. No history rewrite, credential change or provider request was made.

Coverage remains incomplete: this heuristic audit excludes binary blobs, unreachable and reflog-only objects, remote objects absent locally and packaged/hosted artifacts. It does not establish secret-free history or close the release artifact audit. Private metadata-only reports and the one-shot scanner remain under `.tmp/`.
