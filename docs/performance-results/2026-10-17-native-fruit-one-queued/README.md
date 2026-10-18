# One queued native Fruit observation

Linux development/debug native `7a43ea0c8def`, App `da4309cd0191`. The native suite passes 1,066 tests (21 ignored); the fresh executable build is byte-identical to the prior binary, with 541 recorded inputs unchanged during the build. The reused isolated fixture profile has Monochrome preferred and fallback disabled. No cold-start claim is made.

One UUID-bound queued request starts the exact Fruit Bats source, without a setup play or seek. The 57 retained samples include normal startup buffering followed by playing. Native-reported source readiness is 691ms and first mixer audio 1,372ms; the final playhead is 30.279s. These are one-run native timings at muted volume on a private sink, not physical audibility, acoustic recording identity or p95 evidence.

No post-progress interruption trigger appears in the 30-second window: neither sampled buffering nor later buffering/recovery counter or last-buffering-event changes. This is a no-reproduction control, not recovery of the earlier interrupted operation and not a fix for it. If buffering had occurred, the runner would have retained the same generation/seek for 5.5s; that branch was not exercised live.

The exact owned pause acknowledgement and UUID cancellation pass, but private volume restoration fails closed after cancellation clears the queued-owner marker. Thus the runner exits nonzero and cleanup qualification fails. No retroactive pass is claimed. The raw result is retained; the isolated app was stopped, its owned sink removed, and tool pins matched through the run. The normal profile was never used for playback testing.

Private result and lifecycle evidence: `.cache/qualification-queued-playback/fruit-native-recovery-current/`; the preliminary launch was rejected as stale before dispatch, recorded separately under `fruit-native-recovery`. Diagnostics labeled `lastBufferingStreamSnapshot` are event snapshots, not continuously updated network counters. Original underflow remains open.

After the isolated test shut down, the fresh normal app passed the Vite module and authenticated mounted-frontend handshake. Complete before/after normal app snapshots compare exactly equal, including all 30 queue entries/occurrence IDs, paused3.287602s, volume0.8, repeat off and shuffle off. Proof is in `.cache/qualification-cache-owner-expiry/restoration-proof.json`. The private cleanup adjustment is staged offline, but this failed result has not been rewritten or rerun.
