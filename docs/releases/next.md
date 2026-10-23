# Burrata v0.1.8 — draft

These notes describe the current development checkpoint. They are not a published release or a claim that all runtime and installer checks have passed.

- Expand playback diagnostic reports with WebView startup and media-event history, extraction failure categories, helper versions and the macOS version. Enable diagnostics in Settings → Advanced, reproduce the issue, and export the report before closing Burrata. [Capture instructions](https://github.com/anaxonda/burrata/blob/main/docs/install.md#capture-a-playback-diagnostic-report).
- Preserve explicitly linked SoundCloud recordings during resolution. Open YouTube and SoundCloud URLs directly instead of treating them as text searches.
- Add on-demand synced and plain lyrics. Maximize the lyrics panel, or use the microphone button in the player to open fullscreen lyrics directly. Escape returns to the previous screen.
- Add optional side-by-side lyric translation using an OpenAI-compatible provider, model and your own API key. Translation starts only when selected. Reuse lyric text included in an existing Shazam identification when available; this does not start an audio upload or provide a general Shazam web lookup.
- Add Downloaded and Not downloaded filters throughout track browsing, including search, releases, artist tracks, radio history, queue and playback history. Playback history uses current file availability across the full filtered history.
- Add Soulseek artist and album-folder discovery, exact directory browsing and explicit folder downloads. Preserve source-file tags while keeping unverified peer metadata distinct from catalog identity.
- Add a Soulseek wishlist in Settings → Downloads. Saved searches check periodically while the app is open, with pause/remove controls and candidate review; they do not download automatically.
- Add managed local slskd controls for download slots, speed, retries and partial files. Separately configured daemons remain read-only. Improve configuration preservation and restart rollback reporting.
- Add download layouts for artist/album folders and playlist folders, plus cover-art embedding. Existing files are not reorganized automatically.
- Improve combined artist-and-album MusicBrainz searches, artist portraits, fullscreen artwork and alternative stream-URL discovery for saved radio stations.
- Add an optional live BPM estimate. Choose Native audio in Settings → Playback and select Apply and restart to preserve the current queue and playhead, then enable Show live BPM. Estimates can remain blank when confidence is insufficient and do not change saved tags. Compatibility playback remains the default.

## Qualification and limitations

The Mac report identified repeated SoundCloud extraction failures and mixed YouTube access responses; its underlying device/network cause is still unproven. The expanded diagnostics are intended to narrow that investigation. Linux source and seek checks do not establish physical Mac behavior.

Provider availability, peer queues and catalog coverage vary. Secret Garden's conflicting catalog performer credit remains unresolved. Shazam does not always return lyrics; the reported choir recording has not been qualified through a current recognition response. Translation has synthetic integration coverage but no paid-provider qualification.

Extended Monochrome lossless integration remains paused and excluded. Spotify playlist work and YouTube writeback remain outside this release work.

Install: [macOS, Windows and Linux instructions](https://github.com/anaxonda/burrata/blob/main/docs/install.md).
