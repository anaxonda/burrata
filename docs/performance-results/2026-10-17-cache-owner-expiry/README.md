# Native shared lookup cancellation and expiry regressions

Two test-only additions in `acquisition.rs` pass against the real native cache implementation:

- A barrier confirms the first PreviewAudioCache resolver started and a second caller joined its shared flight before explicit cancellation. The first returns `Cancelled`; the waiting caller runs its own resolver, returns its result and caches it for a subsequent read.
- A candidate-search entry seeded older than its TTL is missed and replaced by one resolver invocation. The following read hits the refreshed entry without another invocation.

Each targeted Cargo run passes one test (1,086 filtered out). Compilation took 1m57s; the second command reused the build. Logs and the test-source pin are retained in `.cache/qualification-cache-owner-expiry/`. A subsequent full native run passes 1,066 tests with 21 ignored; the fresh development/debug executable build also passes, with all 541 recorded inputs unchanged. Its SHA256 is byte-identical to the prior executable (`7a43ea0c8def`), consistent with these test-only additions. The first sandboxed full run was denied loopback fixture access; its failed log remains retained separately.

These close deterministic owner-cancellation handoff and candidate-cache expiry/refresh gaps. They do not qualify live signed-URL expiry, network/probe stalls, alternate providers or platforms. Production cache behavior and the running immutable executable are unchanged.
