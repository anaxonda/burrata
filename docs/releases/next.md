# Next Burrata release — draft

These changes follow tagged v0.1.8 and are not part of that public installer.

- Extend fullscreen artwork upgrades to supported YouTube image assets. Keep the selected asset identity and request a larger size only when needed. Preserve the public artwork source after local caching so fullscreen upgrades can still find the same asset. A full App regression, 1,274 frontend tests and 1,786 native tests pass; live renderer verification of the cache-source fix remains. Bounded artwork inspection now includes fullscreen image dimensions and load state without image URLs.

Continue the open requirements in [todo.md](../../todo.md), including remote Mac playback investigation, live Soulseek settings/wishlist checks and the remaining source/catalog cases. Extended Monochrome lossless work remains paused.
