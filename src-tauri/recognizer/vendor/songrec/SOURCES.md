# Vendored SongRec fingerprinting source

Upstream: <https://github.com/marin-m/SongRec>
Pinned source commit: `eaa3ea3e1ce4da90e71881f9853564937db518a7` (SongRec 0.7.5)
Source archive: <https://github.com/marin-m/SongRec/archive/eaa3ea3e1ce4da90e71881f9853564937db518a7.tar.gz>
Source archive SHA-256: `44a31ba27953dab6efb2cac786d716a3e1aa70dfe2e50ca0a2c358a0a3526c8e`
License: GNU GPL version 3 or later; see `LICENSE` (copied from the same source archive).

`algorithm.rs`, `hanning.rs`, and `signature_format.rs` are derived from upstream `src/core/fingerprinting/`. The fingerprint algorithm and Shazam signature encoding are preserved. The file-decoding path and its Rodio/FFmpeg dependencies were removed because Burrata supplies WAV excerpts; the helper converts these to mono 16 kHz PCM. The gettext calls in signature parsing were replaced with plain error strings to avoid a system gettext dependency. The FFT grid allocation was moved to the heap to avoid large stack allocations.
