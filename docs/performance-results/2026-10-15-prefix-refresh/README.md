# Saved-view refresh and helper reliability checkpoint

Development binary SHA-256 `8e2bdfa41acc00986a543f9956aec44858ec1b5e7d3f3b5d88720602d3a46dab` passed the launcher’s Vite/module freshness and authenticated frontend/MCP handshake (76 tools). Main profile was restored after sequential isolated UI checks. Exactly one Burrata GUI process remained. No live provider writes were performed.

Saved-view refreshes now use the same SQL ordering/filtering plan as paged reads to retrieve the exact loaded ID/occurrence prefix. Hydration is bounded to 200 IDs per call, four concurrent calls. Revision checks reject stale query/context responses; metadata events coalesce without dropping previous updates; pending pagination settles before prefix reconciliation. A missing hydrated ID triggers one prefix requery instead of reviving a deleted cached row. Exact listing IDs take precedence over source aliases when overlaying event metadata.

Default native suite: 594 passed, 13 ignored. Custom-protocol suite: 593 passed, 13 ignored (before the final test-only 1,105-row fixture addition). That added fixture passes in the default suite and verifies uncapped prefixes, nullable duration order in both directions and global filtering against page continuation. Known zero duration remains unknown under the existing duration rules.

Frontend: 155 App integration tests plus 256 tests in the other 49 files passed. Coverage includes sorted/filtered event refreshes, stale queries, coalesced updates, metadata arriving during load-more and deletion between prefix selection and hydration. Svelte check: zero errors/warnings. Frontend build and seven IPC/schema contracts pass.

Helper spawn retries only Unix ETXTBSY once, with cancellable backoff inside the existing absolute operation deadline. Actual write-locked executable fixtures verify transient recovery, persistent failure, deadline and cancellation without launching unwanted subprocesses. Cancellation remains distinct from source failure.

Actual rebuilt main SearchPanel smoke check of `poso s'agapo`, YouTube tracks: complete, 20 results, no error. This is a search check, not a new live playback or writeback qualification.
