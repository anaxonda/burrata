# Bound prolonged native HTTP buffering

## Defect and change

Native startup and explicit seek readiness have deadlines. Once first audio starts, the startup watchdog exits. The HTTP downloader can repeatedly time out and reconnect without returning an error to the decoder, leaving a finite track buffering indefinitely. `stream-download`0.24.4 handles a next-chunk timeout by calling `handle_reconnect` and continuing its loop; its reconnect handler returns `Ok(())` even when the reconnect itself fails or times out. Burrata's native HTTP client sets a connect timeout, which does not bound an established stream's repeated no-data episodes.

A reviewed change in `src-tauri/src/playback.rs` adds a20-second watchdog for a continuous post-start buffering episode. The timer is tied to generation, seek serial and buffering count. Resume, pause, seek, replacement, another buffering episode or loss of readiness invalidates it. It exits early on updates, waits without holding the command lock, and rechecks ownership under that lock before invoking existing bounded source recovery. Recovery uses the consumed position and existing retry limit; exhaustion keeps an actionable error visible.

Eligibility is derived from the actual resolved audio-input HTTP(S) scheme and known finite duration, propagated through next-track preloading, and retained only in memory. Radio/station playback, local paths, unknown durations, startup and pending seeks are excluded. No source URL, new setting or provider credential is exposed. This does not change download reconnect timing or startup buffering policy.

The change addresses an independently identified indefinite-buffering path. It is not established as the cause of the previously captured short Fruit Bats underflow, and it does not prevent every temporary network interruption.

## Verification status

Two independent source reviews cover timer ownership, source classification and radio/local exclusions. All five focused tests pass. Full native library regression passes1,074 tests,21 ignored,zero failures in58.95 seconds, with545 source pins unchanged. The fresh executable build passes in2m25s, SHA-256 `26cd66cc0d9530740f9f32a136ca0ed9bc617b10bc6d24f9d9a8b3e25b14ed41`, with545 inputs unchanged during the build. The verified executable is now running as the sole normal app with the related WebView stall/playhead fixes. The full normal queue and settings are preserved, and resumed playback progresses. Evidence and source pins are retained in `.cache/qualification-steady-buffer-watch/`.
