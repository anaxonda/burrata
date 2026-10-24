# Next Burrata release — draft

These changes follow tagged v0.1.8 and are not part of that public installer.

- Extend fullscreen artwork upgrades to supported YouTube image assets. Keep the selected asset identity and request a larger size only when needed. Preserve the public artwork source after local caching so fullscreen upgrades can still find the same asset. A full App regression, 1,274 frontend tests and 1,786 native tests pass; live Linux renderer verification confirms the selected artwork upgrades from 120×120 to a loaded 600×600 image. Bounded artwork inspection now includes fullscreen image dimensions and load state without image URLs.

Continue the open requirements in [todo.md](../../todo.md), including remote Mac playback investigation, live Soulseek settings/wishlist checks and the remaining source/catalog cases. Extended Monochrome lossless work remains paused.

- Fix managed Soulseek settings loading and Apply-result refresh after a reactive-object cloning error. Preserve nondefault values and keep failed reads from presenting an editable default form.
- Keep wishlist checkboxes beside their query labels, replace internal artist-evidence paths with readable source labels, and correct the managed-folder guidance.

- Clear stale station codec and bitrate labels when its stream URL changes; retain station identity and saved listening history.

- Show short download-layout choices with accurate folder-path previews. Real-file checks cover Flat, Artist/Album, Playlist and playlist fallback, source-tag retention and preservation of already published files.

- Add bounded LRCLIB fallbacks using verified title and performer aliases, including non-Latin scripts supplied by the catalog. Keep recording, credit and duration checks; preserve guest credits.
- Offer a keyless Google Translate adapter alongside an OpenAI-compatible provider. Free-first mode falls back once to a configured API key. Live OpenRouter GPT-6 Luna translation passes, including the reported choir lyric document; keyless Google remains best effort after rate limiting.
- Explain download recording-credit conflicts and keep inspection/connection errors distinct from recording-policy rejections.

- Read same-name `.lrc` sidecars and embedded lyric text from the selected registered local audio file before online lookup, retaining source hashes and rejecting changed audio or unrelated recording bindings.

- Honor bounded LRC file timing offsets in the display clock while keeping the original lyric timestamps intact. Resetting manual timing preserves the file offset.

- Apply verified recording aliases to lyrics already cached from Shazam recognition, without requesting another identification.

- Preserve successful SongRec recognition for tag review when a later catalog lookup fails, while keeping catalog identity unverified.

- Continue configured tag-identification fallbacks after catalog lookup outages, retain total-failure errors and require review for incomplete release context.

- Separate two-excerpt recognition from optional catalog enrichment and keep recognition excerpts in request-owned private temporary directories. Unix cancellation cleanup is covered by a real subprocess fixture.

- Retain which recording identifiers agree in both recognized excerpts. Show explicit ISRC agreement in tag review without inferring it for older recognition records.

- Expose fullscreen lyrics through the finite player-panel API. Live isolated Linux UI checks cover `.lrc` timing offsets and embedded plain lyrics; the normal 200-entry queue and paused position were restored exactly.

- Discover several LRCLIB lyric candidates using structured titles, broader title queries and verified artist names. Choose related performances explicitly; those display as plain text with source credits and timing disabled. The reported choir lookup returns seven selectable results.
- Add the OpenRouter GPT-6 Luna preset and actual translation-provider attribution, preserving line IDs and blank paragraphs. Existing provider choices remain intact.
- Explain live BPM availability in Track info and keep routine position updates from restarting its polling. Native audio remains required; uncertain rhythm produces no fabricated estimate.

- Display compact playback quality as one readable codec/resolution or codec/bitrate label, with full details in a tooltip. Avoid shrinking every field into separate ellipses.
- Let listeners pair alternating bilingual source lines into aligned columns without changing the source or invoking translation. Generated translations retain one original/translated pair per row.

- Default paired source lyrics to side-by-side presentation without row dividers. Rename Translate to Show translation and allow cached generated translations to be hidden and restored without another provider request. Add a translation-language button with language presets and custom entry in Settings → Connections.

- Make lyrics controls immersive: a translation icon toggles the second column; timing offsets, source choices and credits live in a compact options menu. Persist manually chosen lyric sources across restarts, scoped to the same recording metadata.
- Add Copy lyrics and Save lyrics as text file, plus a searchable standard-language picker in the lyrics menu and Settings. Keep explicit embedding for managed downloaded tracks; external indexed audio remains read-only.
- Add the opt-in Embed lyrics in downloads setting. Use remembered choices or verified matching lyrics before publication; retain plain or LRC text in format-native lyric tags and read those offline. Preserve lyric tags when exporting MP3 copies.
- Display MusicBrainz release territory XW as Worldwide.

- Resolve exact MusicBrainz-linked Bandcamp albums for live recordings without requiring an Apple studio-catalog reference. Verify the entire album sequence and recording runtimes, preserve explicit group credits containing ampersands, and keep shared matching checks. The reported Gizzard live recording passes source resolution and native PCM qualification.
