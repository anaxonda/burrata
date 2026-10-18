# Experimental Opus decoder comparison

This debug-profile comparison used the generated five-minute Opus tone fixture. It ran three warmups per path, then 20 seeks each at 1%, 50%, and 95%. At each position it compared a fresh FFmpeg seek with the retained experimental Opus decoder. Order alternated between repetitions.

Median first-PCM times were 69.6–74.6 ms for FFmpeg and 38.9–45.3 ms for retained Opus. Per-position p95 values were 92.8–101.8 ms for FFmpeg and 49.3–57.1 ms for Opus. The Opus path reported worker-thread CPU; FFmpeg subprocess CPU is recorded separately. The run used Cargo's default `src-tauri/target` directory.

These figures measure decoder-to-first-PCM only. They do not measure app startup, resolver or network latency, mixer/device output, renderer behavior, real recordings, memory, or supported-platform behavior. The feature remains off by default and is not promoted based on this fixture result.
