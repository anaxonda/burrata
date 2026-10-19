# Bundled native dependencies

The application statically links LibKeyFinder and FFTW into the Burrata native library. These sources are built locally at build time; users do not install either analyzer.

| Project | Pinned version | Upstream | Source archive SHA-256 | License |
| --- | --- | --- | --- | --- |
| LibKeyFinder | 2.2.8, release commit `b33b5a8` | https://github.com/mixxxdj/libkeyfinder | `a54fc6c5ff435bb4b447f175bc97f9081fb5abf0edd5d125e6f5215c8fff4d11` | GPL-3.0-or-later (`libkeyfinder/LICENSE`) |
| FFTW | 3.3.11 | https://www.fftw.org/fftw-3.3.11.tar.gz | `5630c24cdeb33b131612f7eb4b1a9934234754f9f388ff8617458d0be6f239a1` | GPL-2.0-or-later (`fftw/COPYING`) |
| stream-download | 0.24.4, with Burrata sparse-seek and pending-request cancellation patches | https://github.com/aschey/stream-download-rs | crates.io package: `3838c412a6dbb469e857b5148043dbb54d01dce49cace8d46709b795cc7697a3` | MIT OR Apache-2.0 (`stream-download/LICENSE-MIT`, `stream-download/LICENSE-APACHE`) |

The source is redistributed here under the upstream licenses. LibKeyFinder and FFTW have no upstream source changes. `stream-download` is built from the checked-in source through Cargo's path patch; its upstream manifests, licenses and [patch record](stream-download/PATCHES.md) are retained. Its patches correct HTTP seek gap selection and interrupt pending seek/reconnect requests on cancellation without relaxing byte-range validation. Regression evidence includes sparse backward/forward seeks and cancellation of a real blocked `StreamDownload` seek while its local origin withholds range response headers. Each release's supporting downloads include the exact patched source archive, its SHA-256, both license texts and patch record.
