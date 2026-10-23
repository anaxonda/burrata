# Next Burrata release — draft

These changes follow tagged v0.1.8 and are not part of that public installer.

- Extend fullscreen artwork upgrades to supported YouTube image assets. Keep the selected asset identity and request a larger size only when needed. Preserve the public artwork source after local caching so fullscreen upgrades can still find the same asset. A full App regression, 1,274 frontend tests and 1,786 native tests pass; live Linux renderer verification confirms the selected artwork upgrades from 120×120 to a loaded 600×600 image. Bounded artwork inspection now includes fullscreen image dimensions and load state without image URLs.

Continue the open requirements in [todo.md](../../todo.md), including remote Mac playback investigation, live Soulseek settings/wishlist checks and the remaining source/catalog cases. Extended Monochrome lossless work remains paused.

- Fix managed Soulseek settings loading and Apply-result refresh after a reactive-object cloning error. Preserve nondefault values and keep failed reads from presenting an editable default form.
- Keep wishlist checkboxes beside their query labels, replace internal artist-evidence paths with readable source labels, and correct the managed-folder guidance.

- Clear stale station codec and bitrate labels when its stream URL changes; retain station identity and saved listening history.

- Show short download-layout choices with accurate folder-path previews. Real-file checks cover Flat, Artist/Album, Playlist and playlist fallback, source-tag retention and preservation of already published files.
