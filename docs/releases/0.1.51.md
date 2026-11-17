# Burrata 0.1.51

Artist pages now put biography and top tracks first, radio listening history avoids redundant song entries, and public Bandcamp fan playlists can be imported directly.

- Direct playback of an explicitly selected YouTube upload follows the verified video ID even if the upload caption differs from Burrata’s display title. For unpinned results, the title check also accepts an exact artist prefix. Stream access and duration checks still apply.
- Search uses a compact Sources menu with checkboxes, Clear, Select all, and Apply. The closed button summarizes the selected services.
- Artist overviews show formatted biographies above top tracks. Genres, aliases, profile links, and provider coverage are grouped under collapsed Artist details. Repeated genre/tag names share one attributed label.
- Top tracks retain Last.fm's popularity order, with a clearer sort label. MusicBrainz supplies artist identity. Last.fm supplies the popularity ranking.
- Radio artist biographies can use matching song credits to distinguish artists with the same name. Song changes refresh the lookup, and delayed results from an earlier song are discarded.
- Radio Paradise's advancing program countdown keeps fresh station metadata from being replaced by redundant fingerprint checks that could create a second history entry under alternate artist or title wording. Stalled or missing metadata still permits audio recognition. Existing history is preserved.
- Queue selections can be removed with Delete. Clear queue becomes Clear selection while rows are selected, retaining the currently playing occurrence.
- Public Bandcamp fan playlists support paginated direct import without account login. Browser Companion 0.2.16 can capture the initial public page and clearly marks it partial when more pages exist. Direct imports are bounded to 1,000 tracks; unavailable later pages produce an explicit partial result.
- Ask AI answers hide internal citation handles while retaining returned public source links.

## Verification and limits

The native resolver decoded audio from the reported Rubén González upload at the start and near the end. An explicit selection with a changed display title retained the same YouTube video ID. Focused frontend and native regression tests pass, including a live signed-out import of all 267 tracks across six pages from the reported Bandcamp playlist. Artist layout checks pass at wide and narrow widths. The live “Morning — Beck” lookup resolved the artist from matching song credits, and the artist profile returned a Discogs biography. MusicBrainz intermittently returned HTTP 503 with global rate-limit headers. Provider availability can still affect profile lookups and playlist imports.

Desktop release builds and installer checks cover macOS arm64, Windows x64, and Linux x64. Windows physical audio output remains unverified under the same hosted-runner deferral used for the preceding release. Desktop downloads remain unsigned; see the [installation instructions](https://github.com/anaxonda/burrata/blob/main/docs/install.md).
