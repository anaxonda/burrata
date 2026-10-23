# Next Burrata release — draft

These changes follow tagged v0.1.8 and are not part of that public installer.

- Extend fullscreen artwork upgrades to supported YouTube image assets. Keep the selected asset identity and request a larger size only when needed. Preserve the public artwork source after local caching so fullscreen upgrades can still find the same asset. A full App regression, 1,274 frontend tests and 1,786 native tests pass; live Linux renderer verification confirms the selected artwork upgrades from 120×120 to a loaded 600×600 image. Bounded artwork inspection now includes fullscreen image dimensions and load state without image URLs.

Continue the open requirements in [todo.md](../../todo.md), including remote Mac playback investigation, live Soulseek settings/wishlist checks and the remaining source/catalog cases. Extended Monochrome lossless work remains paused.

- Fix managed Soulseek settings loading and Apply-result refresh after a reactive-object cloning error. Preserve nondefault values and keep failed reads from presenting an editable default form.
- Keep wishlist checkboxes beside their query labels, replace internal artist-evidence paths with readable source labels, and correct the managed-folder guidance.

- Clear stale station codec and bitrate labels when its stream URL changes; retain station identity and saved listening history.

- Show short download-layout choices with accurate folder-path previews. Real-file checks cover Flat, Artist/Album, Playlist and playlist fallback, source-tag retention and preservation of already published files.

- Add bounded LRCLIB fallbacks using verified title and performer aliases, including non-Latin scripts supplied by the catalog. Keep recording, credit and duration checks; preserve guest credits.
- Offer a keyless Google Translate adapter alongside an OpenAI-compatible provider. Local response/line-identity checks pass, but live Google requests returned HTTP 429 and real API qualification remains pending; do not treat this as verified live availability.
- Explain download recording-credit conflicts and keep inspection/connection errors distinct from recording-policy rejections.

- Read same-name `.lrc` sidecars and embedded lyric text from the selected registered local audio file before online lookup, retaining source hashes and rejecting changed audio or unrelated recording bindings.

- Honor bounded LRC file timing offsets in the display clock while keeping the original lyric timestamps intact. Resetting manual timing preserves the file offset.

- Apply verified recording aliases to lyrics already cached from Shazam recognition, without requesting another identification.

- Preserve successful SongRec recognition for tag review when a later catalog lookup fails, while keeping catalog identity unverified.

- Continue configured tag-identification fallbacks after catalog lookup outages, retain total-failure errors and require review for incomplete release context.

- Separate two-excerpt recognition from optional catalog enrichment and keep recognition excerpts in request-owned private temporary directories. Unix cancellation cleanup is covered by a real subprocess fixture.
