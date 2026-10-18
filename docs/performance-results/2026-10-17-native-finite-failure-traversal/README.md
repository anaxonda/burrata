# Native finite failure traversal — private review checkpoint

The private patch replaces the native engine's global three-failure skip limit with a bounded traversal of exact queue occurrence IDs. Repeated track IDs remain separate occurrences. Repeat-all scans at most one cycle after uninterrupted failures; repeat-one does not retry the unavailable occurrence. Actual audio readiness, a manual selection or a structural queue change resets the traversal. Failure exhaustion does not trigger natural-end radio.

Root review required cancellation, stale generation, provider-global failure and malformed queue state to remain distinct from true exhaustion under the state lock. Those ineligible results do not append a failure or consume an occurrence. A cancellation regression initially dropped its watch receiver; that fixture was corrected before native execution.

Root independently compiled and ran the exact staged standard-library-only traversal helper: four tests passed for repeat-all termination, distinct occurrences, repeat-off bounds, a one-entry queue and malformed state. The full State/cancellation tests are added but not yet compiled or run. This checkpoint does not claim integrated native correctness, a new app build, or live album acceptance.

Private patch `/tmp/native-failure-occurrence.patch`, SHA-256 `b34699fb8d1e96eb46eba4ee4867ff2fce739db5f1e598b5d7da3ea66464f5fa`; helper SHA-256 `9ea4c7b0aa8a81b24c625fb321863875e0147e4aab5228d191fd28b82b55d599`. Integration with the queued-playback ownership patch is in progress in a private stage. The normal app remains unchanged.
