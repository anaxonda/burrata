# Native encrypted MP4 integration status

Date: 2026-10-21

**Paused by the user:** extended lossless integration is on hold because monochrome.samidy.com is down. The optional service stays disabled. Continue unrelated playback/download fixes and release preparation; this paused feature is not a release gate.

## Implemented boundaries

`native_output::AudioSource` has an ephemeral `EncryptedMp4Audio` context. Its key constructor accepts exactly 16 bytes encoded as 32 hexadecimal characters. The key and context redact Debug, are not serializable, and clear owned key bytes on drop. Ordinary sources retain their existing decoder path.

Production CENC decoding requires an exact registered native relay lease. The separate bounded relay registry validates HTTPS destinations, pins public DNS, disables proxies and redirects, rejects resolver authorization/cookie headers, and supports Range/HEAD. Its leases survive ordinary WebView transport generation changes. Final lease drop revokes access and cancels in-flight transport. The decoder receives only the owned loopback input, with a restricted FFmpeg protocol list; the short ffconcat manifest carries the key through stdin rather than arguments or disk.

`monochrome_resolver` parses the typed resolver response and validates its selected recording/resource. `monochrome_playback` provides an injectable service with bounded authorization, cancellation and a single authorized refresh retry. Initial resolution uses the shared three-mode recording policy; fresh recovery preserves the same stable recording ID and selected provider. API credentials are separate from media requests. The acquisition service slot defaults to unset, so existing playback routes remain available and the new resolver route is inactive.

The native service/relay snapshot passed **1,624 tests, 37 ignored**, recorded in `/tmp/burrata-lossless-relay-service-native-tests.log`. This includes synthetic ordinary/fragmented CENC FLAC decode, seek, wrong-key rejection, cancellation, lease isolation and relay policy tests. The packaged Linux FFmpeg is 8.1.3. These results do not qualify macOS or Windows bundles or fresh provider streaming.

## Native-first handoff under qualification

The subsequent worktree adds a bounded, one-use, 30-second pending-source handle. Rust retains the resolved source and relay lease; `resolve_playback_track` returns only the opaque handle and generation for a native source. The UI transfers the exact queue/session/occurrence through `handoff_resolved_playback_source`, without sending the media URL or key to HTMLAudio. The backend checks the request generation, queue occurrence, identity evidence and current matching level before consuming the handle and starting native decoding without a second resolve. Expiry drops the retained resource.

Frontend tests cover accepted native handoff and a delayed result after Pause, alongside the native bridge envelope test. The completed frontend suite passes **1,167 tests in 93 files** (`/tmp/burrata-native-source-handoff-frontend-tests.log`); type checking reports zero errors/warnings, and the production frontend build passes. The latest native handoff and matching edits are still undergoing compilation/regression qualification; earlier passing counts do not qualify those later edits.

## Remaining dependency and acceptance

There is **no injected provider authorization source**. The documented browser observations do not establish a supported fresh credential issuance or refresh contract. No cookie extraction or manual bearer-token UI has been added, and the optional service remains disabled. Further provider/web research is paused by user instruction.

Fresh authorized full-length lossless playback for the reported tracks and held-out recordings, real seek/replay/expiry recovery, queue behavior and relevant platform bundles remain unverified. Captured bounded media probes and synthetic decoder tests establish feasibility, not completed Burrata lossless playback. See the [resolver plan](../../monochrome-broader-resolver-plan.txt) and [backend findings](../../monochrome-backend-alternatives-research.txt).
