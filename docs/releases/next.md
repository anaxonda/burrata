# Burrata 0.1.37

This release improves recording metadata, playback discovery, album navigation, lyrics requests and internet-radio setup.

- Playback can verify slash-separated collaboration captions and accented artist spellings against the complete requested performer set in Balanced mode. Fresh extractor metadata must remain bound to the selected source ID; extra performers and conflicting versions retain their checks.
- Playback searches retain up to 20 source metadata rows so correctly credited recordings below covers and live uploads remain available for verification. Extraction attempts and playback deadlines stay bounded, and older ten-row search caches are invalidated.
- YouTube Music search and imports preserve individual artist credits, album browse IDs and album-track durations. Browser Companion 0.2.11 carries this metadata through authenticated search and playlist imports. Beatport, Traxsource, Monochrome and restored tracks preserve structured credits more consistently.
- Find album resolves recording membership and opens a verified release or offers specific release choices. Complete ordered album tracklists can establish alternate-title relationships, including a bounded shorter provider label. MusicBrainz release artists remain separate from recording-line credits. Authenticated Spotify is preferred when album membership is verified; MusicBrainz and Discogs provide catalog destinations. Exact known release IDs are preserved, and an exact YouTube album endpoint remains available when catalog lookup cannot resolve it.
- Cold album lookup uses complete validated artist editions to avoid duplicate group lookups and checks possible matching programmes first. Unknown counts and partial catalogues retain their fallback paths; the lookup deadline remains bounded.
- Album lookup and cancellation use native-valid UUID request IDs. Navigation changes discard stale lookup results.
- YouTube autoplay fills a missing displayed artist from the exact saved video, including library entries beyond the loaded page. The playbar, Now Playing and lyrics use that display metadata while the autoplay source binding remains intact.
- Lyrics requests can recover title and artist from supported explicit live-performance captions when the artist is missing. Unknown uploaders are not promoted into artist metadata.
- Add station opens its own focused dialog with Search and Stream URL tabs, preserving the selected station or other page underneath.
- Playback errors distinguish a DRM-protected source from other rejected recording matches.

## Qualification

The Linux candidate passed real Spotify search playback of “La Nave del Olvido” in Balanced mode, with fresh source verification, native decoding and advancing position. Its 2,770 native tests, 1,639 frontend tests and installed DEB/AppImage/RPM checks passed. Search continuation queued 30 loaded results and advanced past failed sources to the playable recording. A subsequent candidate passed all three platform builds and the warm album-navigation check, but failed the cold-cache navigation deadline and was withheld. The cold-lookup correction and exact final-artifact checks remain in progress.

Linux, macOS and Windows installer results will be recorded separately. Hosted-runner smoke tests do not establish fresh-customer-machine behavior. Physical macOS and Windows audio output remain unverified; Windows output testing may be explicitly deferred as in the preceding release.

Provider availability, DRM and incomplete recording identity can still prevent playback. These changes do not claim universal provider access or automatic repair of ambiguous legacy library entries.

The live “Amiga” and “Turista” source attempts still failed verification in this qualification. Continuation skipped them successfully. This release does not claim every Spotify result has an available verified audio source.
