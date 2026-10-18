# Protected 725 decoder recheck

This record uses the already-built, protected native test executable; it did
not invoke Cargo or the running GUI. The local fixture is a generated 300-second
48 kHz stereo WAV, independently measured at 300.000 seconds by ffprobe.

The ignored `external_audio_fixture_reports_duration_and_decodes_after_late_seek`
test passed. It verified the decoder duration within one second, sought to 95%
of the fixture and decoded the remaining tail to within one second of the
expected 15 seconds. This is decoder correctness evidence for that test binary
and synthetic WAV only. It excludes source resolution, mixing, audio-device
output, renderer and whole-application behavior.

The ignored `seek_decoder_benchmark` test also emitted 120 decoder-to-first-PCM
timing samples (20 per strategy at 1%, 50% and 95%). Those timings overlapped a
separate global-candidate Cargo compilation and are therefore marked
**confounded and not qualification**. They must not be used as a CPU or latency
comparison. The raw samples are retained in `benchmark-attempts.json` for
traceability only.

The test executable SHA-256 is
`03c9d1ffc51d6448b48ee79e8549d36b2a138027904f17c6c0f7c77ad5d97701`; its
protected manifest checkpoint reports 725 passed and 14 ignored tests. The
fixture SHA-256 is
`67d6654cd4cbcecb42985334153c9349e83c5898fe70c4491e9b398a38f3dd58` and its
size is 57,600,078 bytes.
