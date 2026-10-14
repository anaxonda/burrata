# Automatic live-performance tagging

YouTube downloads automatically normalize explicit performance titles before
creating the download job. Supported forms include:

- `Big Thief Perform “Mary” | Pitchfork Music Festival 2018`
- `Big Thief — Mary (Live at Pitchfork Music Festival 2018)`
- `Big Thief - Mary (Live on KEXP)`

Burrata stores artist, song title and live version separately. Exported audio
uses `Mary (Live at Pitchfork Music Festival 2018)` as its title so ordinary
players display the performance distinction. A comment preserves the original
title, source URL, and the fact that the performance identity is inferred and
not catalog-verified.

When a Spotify, MusicBrainz, Discogs, Beatport or Traxsource track is acquired
from an explicitly identified YouTube live performance, Burrata creates a
separate YouTube recording identity. It does not overwrite the selected catalog
record. Studio album, release date, ISRC, label, catalog number, BPM and key
are not copied to the new performance. The original selection is retained in
provenance for review. Audio analysis can estimate BPM/key from the acquired
file rather than borrowing the studio recording's values.

Artist, song or version conflicts stop acquisition for review. Bare `(Live)`,
uploader names and ambiguous video titles are not enough to infer the event.
Event years remain part of the version/source evidence; the current model does
not have a separate structured recording-date field. They are not written as
release dates. Existing downloaded files are not bulk-retagged automatically.

This is conservative title-based automation, not acoustic verification of the
exact recording. Future catalog reconciliation should require evidence for the
specific performance before applying release identifiers or album metadata.

## Reliability and comparison

The automated checks cover supported title forms, artist/song/version conflicts,
separation from studio metadata, and actual MP3/FLAC tag round trips using
synthetic audio. They do not establish a success rate on arbitrary uploads.

spotDL searches YouTube Music using Spotify track metadata, tries ISRC searches
when available, scores artist/title/album/duration matches, and penalizes
unexpected live/cover/remix labels. It writes the selected Spotify song's
metadata to the acquired audio. This improves catalog matching, but its source
selection remains heuristic rather than acoustic proof of recording identity.
See its [search implementation](https://github.com/spotDL/spotify-downloader/blob/master/spotdl/providers/audio/base.py),
[matching implementation](https://github.com/spotDL/spotify-downloader/blob/master/spotdl/utils/matching.py),
and [tag writer](https://github.com/spotDL/spotify-downloader/blob/master/spotdl/utils/metadata.py).

Seal delegates metadata extraction/embedding to yt-dlp. Its audio options use
release year or upload date for the date, and album or playlist/video title for
the album. Those fallbacks are convenient for downloads but do not establish
an actual music release. See [Seal's download options](https://github.com/JunkFood02/Seal/blob/main/app/src/main/java/com/junkfood/seal/util/DownloadUtil.kt)
and [yt-dlp metadata defaults](https://github.com/yt-dlp/yt-dlp#modifying-metadata).

A broader Burrata pipeline should prioritize structured music metadata, check
version and duration compatibility, distinguish song identity from recording
identity, and retain per-field evidence. Acoustic fingerprints can corroborate
catalogued recordings; unmatched live recordings still need source evidence or
review. A score must not be displayed as an empirically measured probability.

References: [MusicBrainz recording guidelines](https://musicbrainz.org/doc/Style/Recording)
and [ID3 recording/release date definitions](https://id3.org/id3v2.4.0-frames).
