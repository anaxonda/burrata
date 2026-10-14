# Discovery, chapters, and automatic tagging

Search has two scopes: **Online** and **My library**. Online uses one compact
selector for a single source or all configured online sources: MusicBrainz,
Discogs, YouTube, SoundCloud and Soulseek. Discogs needs configured credentials;
Soulseek needs configured slskd. These sources use real remote searches, and
provider errors are shown without substituting results from another catalog.

Beatport, Traxsource and Spotify are not online search options. Their saved imports
are searchable under My library along with saved tracks and downloads. Library
search matches artist, title, album, version and remixer credits, with pagination.

Scope and source preferences are saved locally. Previous checkbox selections are
migrated to the new selector; removed providers cannot remain selected. Changing
scope or source cancels stale requests and reruns a submitted query. Result types
unsupported by a source are skipped.

Track rows can be enriched with supplemental source metadata. This can include a
description, creator/channel, upload date, and source genres. An upload date is
shown as an upload date; it is not treated as a music release date. Source metadata
is evidence from that provider and does not by itself verify recording identity.

Soulseek keeps polling while peers respond. Completed results use bounded pages with
Load more; the completed response stays cached for five minutes so paging does
not restart the peer search.

## Chapter inspection and playback

Chapter inspection is tied to an explicit audio candidate. Burrata can expose
available chapters inline, register stable chapter tracks, and play, queue, or
download a selected chapter. Chapter tracks refer to a bounded time range in the
parent source. Playback seeks to the range start and advances at its end, so the
parent audio does not continue beyond the selected chapter. Chapter boundaries are
checked against the inspected candidate before registration.

During chapter-split downloads, Burrata keeps a temporary copy of the acquired
parent audio under `.burrata-staging/.chapter-source-cache` to avoid retrieving the
same source again for each selected chapter. Cache entries are keyed by source,
URL, format, and music directory, expire after their configured lifetime, and are
used only for chapter extraction. Entries expire after 15 minutes and are not
published as library tracks; they do not replace source inspection or
chapter-boundary validation. Soulseek downloads do not use this source cache.

Chapter titles and inferred artist/title metadata can be uncertain. Review chapter
labels before treating them as catalog metadata. For chapter excerpts, automatic
tagging review is limited to artist, title, and genre; it does not apply album,
release-date, or recording identifiers to an excerpt.

## Automatic tagging of new downloads

Automatic tagging is queued for managed downloads after they complete. It does not
scan or bulk-retag an existing library, and external read-only assets are excluded.
Matching checks source text and catalog candidates, with optional fingerprint
lookups. A high-confidence unambiguous match can fill supported metadata
automatically; uncertain or conflicting candidates go to the review screen. A
reviewer chooses a candidate and explicitly approves fields before those fields are
written. Unapproved fields are preserved. Live-performance identity conflicts
require review rather than applying studio recording metadata to a live version.

Fingerprint matching needs local `fpcalc` and an AcoustID application key for the
AcoustID lookup. Burrata generates fingerprints locally and sends the lookup to
AcoustID. Shazam can be enabled as a fallback and requires the `songrec` command
(or `SongRec`) plus FFmpeg for sampled audio excerpts; SongRec is not bundled.
Release packages include `fpcalc` for fingerprinting. In this development
environment, `songrec` and `fpcalc` are installed, but an AcoustID application
key is not configured, so AcoustID lookup is unavailable until one is saved in
**Settings → Connections → AcoustID**. Provider connectivity and matching success
depend on the user's setup and network access.

## MCP tools

The local MCP registry includes these discovery and metadata tools:

- `discover_music` searches selected sources with result-kind filters, per-source
  pages, and bounded result limits; `poll_music_search` polls an active Soulseek
  search.
- `inspect_source_chapters` inspects a supplied source candidate and returns
  registered chapter tracks; `play_chapter` plays a registered chapter track.
- `get_autotag_status` reads paginated tagging jobs. `retry_autotag`,
  `review_autotag_candidate`, and `apply_autotag_review` retry a job, select one of
  its candidates, and apply explicitly approved fields. These use the Manage
  permission; discovery/inspection/status use Read, and chapter playback uses
  Control.

The MCP registry and tool schemas are authoritative for the running app and may
change. The implementation and isolated tests cover these flows, but this document
does not claim that every provider or end-to-end live workflow has been verified.

## Verification on this machine

On 2026-10-13, the frontend suite passed 129 tests, the backend suite passed 224
(with one ignored), and the MCP transport suite passed three. Chrome layout
checks covered 390px, 1024px and 1440px, chapter controls and sidebar resizing.
The rebuilt desktop app was exercised through the official MCP client: native
YouTube Music returned Big Thief’s “Mary” with album “Capacity,” duration and
artwork, and canonical track lookup succeeded. Live Soulseek discovery returned
completed results in separate 50-item pages without overlap; the returned row ID
also worked with `get_track` after the peer search had completed.

These checks do not establish recognition accuracy across a real music corpus.
AcoustID remains unconfigured on this machine until an application key is supplied.

The online selector excludes Beatport, Traxsource and Spotify. The live revised
API returned MusicBrainz and Discogs artist references from the respective
provider, paged saved-library results, and rejected Beatport as an online source.
Chrome checks confirmed the scope switch and selector at desktop and narrow widths.

## October 13 search and chapter updates

YouTube mixed results rank by query matches in titles and structured artist metadata,
rather than promoting all YouTube Music tracks ahead of videos. Videos / Sets requests
skip YouTube Music tracks; Tracks requests skip raw video search.

The download chooser no longer requires the recording/mix confirmation checkbox.
Individual chapter download shows the chosen chapter and time range, with a Change
chapter action. Multi-chapter checkboxes appear only after selecting bulk mode.
Candidate identity and confidence are unchanged by downloading.

Track context menus, playing-track information, search chapter rows and queue chapter
rows provide Search for more info. Queries can be edited and searched in catalogs or
on the web without a catalog match. Catalog lookup selects Online / MusicBrainz / All
for that action, overriding remembered source filters. Lookup never changes tags.

SoundCloud preview resolution accepts audio-only HLS as well as progressive audio.
Exact source-ID checks and public HTTPS provider CDN restrictions still apply;
YouTube video streams and arbitrary media hosts remain rejected.

Live search verification after rebuild returned ten relevant Gia Fu salsa videos
in the top ten for both All and Videos / Sets, with no unrelated YouTube Music songs.
