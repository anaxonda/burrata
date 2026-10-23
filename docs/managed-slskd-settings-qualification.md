# Managed Soulseek download settings qualification

The public settings command accepts only the repository-managed local slskd connection. It keeps the connection and platform checks before entering the file/restart transaction. The GUI clones the plain command response before assigning reactive state, so nondefault settings survive both loading and Apply-result refresh.

The opt-in Linux test exercises that same transaction against a uniquely named disposable Docker Compose project:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib \
  managed_download_settings_disposable_daemon -- --ignored
```

Docker Compose and the already-cached pinned slskd image are required. The test specifies `pull_policy: never`, `network_mode: none`, an empty private data directory and a generated test-only API key. It mounts no user library or real daemon configuration, publishes no ports, and disables Soulseek connection, share scanning, configuration watching and version checks. The effective options are read through the container's own loopback API using its bundled `wget`.

The test applies defaults, changes slots, speed, retry count, initial delay, maximum delay and partial-file policy to nondefault values, and verifies all six through the running daemon's options API. It checks that Apply changes the container ID, then restores defaults and checks another recreation. Cleanup removes only the randomly named fixture project; normal slskd transfers are not involved.

This test complements the UI regression and live read-only inspection. It does not claim a live Apply click against a user's normal daemon, interrupted-transfer recovery, peer availability or successful acquisition of every album track. The normal daemon must not be restarted for qualification while unrelated queued transfers are present.

Recorded Linux result: the disposable integration passed in 12.40 seconds; all 35 related native Soulseek regressions passed. The normal daemon start time and transfer-state counts remained unchanged. [Sanitized evidence](performance-results/2026-10-22-integrated-followup/managed-slskd-download-settings-docker-runtime.json).
