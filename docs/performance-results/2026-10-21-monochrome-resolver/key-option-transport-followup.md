# FFmpeg CENC decryption-key transport follow-up

Date: 2026-10-21

## Result

The packaged Linux FFmpeg is n8.1.3. A synthetic-only fixture was generated locally (three seconds of 44.1 kHz mono sine, FLAC in ordinary and fragmented MP4, AES-CTR CENC with a test-only key and KID). The raw key was never used outside this local fixture/probe. Full PCM, input-side one-second seek, and output-side one-second seek all exactly matched the clear reference for both container forms while using an ffconcat manifest sent through FFmpeg stdin with `-nostdin`. The key appeared in the manifest bytes sent through the pipe, not in FFmpeg argv or on disk. The result is recorded in `transport-probe-result.json`; the reproducible script lives in `.cache/monochrome-protocol-prototype/transport-probe.py` and removes its temporary media/key-file fixtures on exit.

Three direct CLI alternatives failed with the same packaged build:

- `-decryption_key <0600-file-path>` treats the path as key text and fails option parsing.
- `-/decryption_key <file-path>` fails as `Unrecognized option '/decryption_key'` even though FFmpeg documents this general option-value-from-file syntax.
- `-decryption_key /proc/self/fd/N` treats the descriptor path as key text and fails.

Therefore the easiest native-output experiment is a single-file ffconcat script over the child process's stdin, with a per-file `option decryption_key <hex>` line, `-f concat -safe 0 -i pipe:0`, and the normal PCM pipe on stdout. Add `-nostdin` and a narrow `-protocol_whitelist`; stdin is still usable as the explicit `pipe:0` media input in this tested invocation. Feed the short script through the existing child stdin pipe and close it immediately after writing. Since FFmpeg and PCM output can run concurrently, production code should write stdin on a separate task/thread while continuing to drain stdout/stderr, and preserve current cancellation/kill/wait behavior. Do not put the key in command arguments, environment variables, logs, cache keys, track metadata, or diagnostics. The key remains in parent/child memory and transient pipe buffers until consumed; clear owned buffers on disposal where practical.

For a remote media URL, the ffconcat `file` entry must be correctly quoted; reject line breaks, NULs, and unsupported schemes before formatting. Apply a restrictive protocol whitelist that permits only the chosen stream protocol and the required transport subprotocols; do not automatically allow local `file:` resources for production. Do not forward the resolver API bearer or Turnstile JWT to the media CDN. Revalidate the CDN host and every redirect using the existing destination policy. Include any necessary media request headers as ffconcat per-file options only after verifying correct escaping and ensuring they are not resolver credentials.

## Native integration recommendation

`native_output.rs::AudioSource` (`src-tauri/src/native_output.rs:26`) currently carries URL, headers, and decoder hints. `decode_ffmpeg_with_diagnostics` (`native_output.rs:2656`) builds the CLI invocation and currently assigns stdin null at line 2732. Add a redacted ephemeral transport context to `AudioSource` (no `Debug`/serialization exposure), with the CENC key separated from ordinary headers. In that decoder function, route only encrypted MP4 inputs through a generated one-file ffconcat script sent through child stdin; preserve the current direct-URL branch. Spawn a writer thread/task for the small manifest, while the existing stdout/stderr drain and child wait/cancel logic runs. Close stdin after the script is written; let existing kill/wait/disposal own process cleanup. Keep the URL/key out of Debug and source caches, and drop the key after child disposal. Reject malformed CENC keys unless exactly 16 bytes represented as 32 hexadecimal characters. If the resource is inline DASH, first validate each referenced media URL, then apply the same per-file option policy; do not treat the manifest as a direct audio URL.

At the higher playback boundary, add the resolver branch within `playback_sources::resolve_inner` (`playback_sources.rs:929`) so it inherits shared deadline/cancellation/fallback logic; the entry point is `playback_sources::resolve` (`:543`). Update `fresh_candidate_audio_identity_matches` (`:868`) to consume independently returned resolver evidence through the common matching policy, without taking an echoed request ISRC as proof. The shared queue preparation call already uses `resolve_with_trace_context` in `playback.rs:2752`.

The native `libavformat` alternative is also viable: FFmpeg documents passing demuxer-private options through `AVDictionary` to `avformat_open_input`; the CENC option is a 16-byte hex key. A purpose-built linked decoder/helper could receive the key over stdin or another private IPC channel and set the option in memory, avoiding a generated ffconcat script. That path requires a new FFmpeg-linked target and substantially more build/platform work than reusing the bundled CLI.

## Portability and qualification limits

This checkout contains only `src-tauri/resources/tools/x86_64-unknown-linux-gnu/ffmpeg` (version 8.1.3); no macOS or Windows FFmpeg executable was available to test. The tested transport uses the standard child stdin pipe and FFmpeg `pipe:0`, not `/proc` or inherited numeric descriptors, so it avoids the Unix-only FD naming problem and should be more portable. The concat directive and per-file options are documented FFmpeg features, but macOS and Windows packaged builds still need the same synthetic full-decode/seek regression. Confirm those targets include concat, pipe, MOV CENC, and the selected network protocols. Do not claim cross-platform qualification from this Linux run.

This probe says nothing about live CDN URLs, resolver auth, production CENC compatibility, audio-device playback, or Alvafro. The Linux result is local synthetic feasibility only.

## Primary FFmpeg references

- [FFmpeg concat demuxer syntax](https://ffmpeg.org/ffmpeg-formats.html#concat): documents `option key value`, per-file directives, `safe`, and pipe-readable text scripts.
- [FFmpeg MOV/MP4 demuxer](https://ffmpeg.org/ffmpeg-formats.html#mov_002fmp4_002f3gp): documents `decryption_key` as a hex 16-byte CENC/AES-128 CTR key.
- [FFmpeg CLI option argument file syntax](https://www.ffmpeg.org/ffmpeg.html#Options): documents `-/option file` generally; the private MOV option rejected it in the exact packaged Linux build.
- [FFmpeg `avformat_open_input`](https://ffmpeg.org/doxygen/8.1/group__lavf__decoding.html): documents providing format and private demuxer options through an `AVDictionary`.
