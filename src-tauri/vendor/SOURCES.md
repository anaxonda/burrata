# Bundled key analysis dependencies

The application statically links LibKeyFinder and FFTW into the Burrata native library. These sources are built locally at build time; users do not install either analyzer.

| Project | Pinned version | Upstream | Source archive SHA-256 | License |
| --- | --- | --- | --- | --- |
| LibKeyFinder | 2.2.8, release commit `b33b5a8` | https://github.com/mixxxdj/libkeyfinder | `a54fc6c5ff435bb4b447f175bc97f9081fb5abf0edd5d125e6f5215c8fff4d11` | GPL-3.0-or-later (`libkeyfinder/LICENSE`) |
| FFTW | 3.3.11 | https://www.fftw.org/fftw-3.3.11.tar.gz | `5630c24cdeb33b131612f7eb4b1a9934234754f9f388ff8617458d0be6f239a1` | GPL-2.0-or-later (`fftw/COPYING`) |

The source is redistributed here under the upstream licenses. No upstream source changes have been made.
