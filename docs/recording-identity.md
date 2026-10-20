# Recording identity and parsing

## Shared recording policy

Playback and downloads use the same typed evaluator and exactly three settings:

- **Exact** requires complete recording agreement or independently bound recording evidence.
- **Balanced** is the default, including for existing settings. It accepts supported credit and catalog variations while retaining performer and version constraints.
- **Flexible** allows supported partial credits, metadata gaps and moderate duration differences. An identified alternative performance, live version or remix can be selected when the song relationship is supported; the player labels the actual recording and preserves the original request.

Missing evidence, explicit conflicts and agreement are separate states. Legacy confidence labels do not decide acceptance. Fresh extraction replaces search metadata before the final decision. Recording-specific aliases and uniquely scoped MusicBrainz names feed the same evaluator; they do not rewrite saved artist names.

Identity observations and source locators are scoped to the requested identity, evaluator version and policy. Audio URL caching remains independent of policy: reusing a URL still requires current-policy recording validation. Explicit source selection applies to the chosen download only; it is removed from saved artifact provenance. Alternative downloads use a distinct actual-source identity and retain the requested recording in provenance.

Bandcamp track discovery is available as a public search provider. Search rows locate artist-hosted pages; stream availability is checked when opening the source. Automatic fallback retains its independent album/audio proof and submits the verified identity to the selected shared policy. Audio-reference verification remains a background operation.

The checkpoints below describe earlier builds. They are historical evidence, not qualification of the latest shared-policy candidate. Current release qualification must recheck the original troublesome entries in the final build and record its hash and results.

The shared Rust `recording_identity` module interprets identifiers, structured credits and album context from provider metadata. It does not turn a catalog identifier into proof that an arbitrary audio upload is the same recording.

## Resolution order

1. Reuse a playable local asset or a retained verified source binding.
2. Preserve and canonicalize exact recording identifiers at ingestion and library hydration. Accept canonical ISRCs and their standard hyphenated printed form. Preserve legacy raw values for display. Conflicting identifiers do not authorize an ISRC crosswalk; a manual ISRC override takes precedence over provider metadata.
3. For a radio song with an ISRC but no duration, use the bounded MusicBrainz identifier lookup before broad title search. A complete, uniquely supported recording response must agree on title, full artist credits and any known duration. An independently verified duration is applied to the playback copy, not saved tags.
4. Use recording-specific artist aliases and complete album crosswalks when providers present different credits. The Bandcamp/Apple album crosswalk binds album identity and every unique duration correspondence, checks titles and version conflicts, and binds the target recording. The timeline helper alone is not identity evidence.
5. Search and verify candidate audio. Accent folding can help search and an independently bound album comparison; generic identity normalization uses Unicode NFC and retains accents. Explicit guest, soundtrack and version labels are parsed separately without changing saved titles. Musical movements and ambiguous parentheses remain intact.
6. Retain successful source connections independently of expiring audio URLs. Failure classification already separates empty search, identity rejection, provider/access problems, deadlines and cancellation.

## Credits and classical recordings

Keep provider credit arrays intact within explicit bounds, or reject incomplete evidence. A malformed or oversized YouTube Topic artist list cannot be silently shortened to authorize a recording. SongRec's artist-array fallback preserves every valid supplied name.

The shared identity retains contributor roles and work IDs when supplied in recording relations. Composer/work identity is not performer/recording identity. A composition relationship cannot authorize another orchestra, conductor, cover or performance. This change does not add speculative same-name cross-links or infer missing classical roles.

## Saved radio source preparation

The optional **Prepare saved radio songs** setting defaults on. On launch, preparation rotates through at most 100 saved radio songs; new saves and favorites can also enqueue eligible tracks. There are at most 100 queued jobs and one active worker. Work waits 15 seconds after scheduling, defers queued jobs until 30 seconds after the latest foreground play request, re-reads saved state/settings, cancels active work when foreground playback starts, and never changes the queue, history, saved tags or external accounts.

Ordinary resolution is bounded to 20 seconds. A separate eligible Bandcamp verification gets at most 45 seconds; it may use audio-reference comparison only in this background phase. Successful preparation has a seven-day scheduling cooldown, unresolved work a one-hour cooldown. These are scheduling intervals, not expiry of verified recording-to-source evidence. Identity changes invalidate the scheduling key. A rotating scan prevents the newest page from permanently starving older songs.

Recognition preserves validated ISRC, Shazam key and explicit track-specific Apple Music URL. Old entries whose identifiers were discarded cannot recover those identifiers from the discarded response; background preparation uses remaining catalog/search evidence instead.

## Regression evidence

Tests cover printed/canonical ISRCs, conflicting and manual identifiers, legacy metadata compatibility, complete radio artist credits, missing-duration ISRC enrichment without saved-tag writes, malformed Topic lists, Unicode equivalence, explicit guest/version parsing, work versus performer identity, and complete/ambiguous album timelines.

`src-tauri/tests/fixtures/nighttime-album-crosswalk.json` retains only public catalog IDs, titles, artist credits and durations for the twelve-track Sem Nostalgia comparison. It contains no audio, account information or signed stream URLs. The regression binds Nighttime despite the different guest credit and rejects the Tostex artist substitution.

## Recording-specific locale credits

Recognized radio songs can query the retained ISRC directly. When Spotify omits the ISRC from its search row, an exact identifier-filter query and a terminal empty negative-control query are required before the result can provide a recording-specific credit presentation. Exact title/version, complete credits, account continuity and conflict guards remain required. Recognized ISRCs receive at most six seconds for the query, filter control and account fences; ordinary title lookup retains its four-second allowance. Both remain inside the original playback deadline with eight seconds reserved for audio startup. The observed `ฌอน` / `sean` presentation is not installed as a global artist alias; saved tags stay unchanged. Foreground lookup also supports unsaved history entries, while background preparation remains restricted to saved songs.

## Navigation and queue behavior

The normal Track Info panel no longer asks users to select a verification method. Verification remains internal to source resolution. Library row playback explicitly snapshots the displayed list and occurrence index; it does not load the entire library into the queue. Same-occurrence metadata refresh during a failed start receives at most one fresh-source retry, while replacing or reordering the queue invalidates stale requests.

Discography ordering fills bounded subsequent release pages before treating the visible chronology as complete. Dates use the earliest known edition date, including month/day, and partial results remain labeled. This fixes sorting a first page of older editions as if it represented the whole catalog.

## Captured versions and structured YouTube mixes

Chart captures sometimes repeat a full title in the version field before a mix label and runtime. Matching removes that duplicate title and runtime without discarding the actual remix identity. Title-only Live/Remix qualifiers and named guests remain constraints.

An exact, known-duration versioned Topic result can reach fresh extraction and is retained ahead of weaker flat-title uploads when extraction attempts are capped. Acceptance still requires fresh complete structured performer credits, the exact requested mix title, bounded duration, and valid Topic/channel and description-credit evidence. Finding a different shorter edit is not enough to satisfy an Extended Mix request.

Audio-provider search tracks with a real album and artist hint use the existing catalog album search. A unique exact title/artist match opens the release, ambiguous results open existing album choices. Tracks with no album hint still require an explicitly labeled discography search rather than pretending an album was identified.

## Three chart-track live acceptance checkpoint

All three original saved Traxsource entries now play automatically in the normal-profile WebView: `15057930` (DJ Tai Vocal Remix) → YouTube `mVvwOJJRwXo`; `15019328` (Untitled Dub 1) → `Se4-OPD-nEs`; `14798487` (It's All About The Love For You) → `iHugvFBKbF0`. Both queue transitions were exercised by seeking near the preceding track's end. Media progress and error-free ready state were verified for each. No source pins or saved-tag changes were used. [Public-only acceptance evidence](performance-results/2026-10-20-three-chart-playback/live-qualification.json).

The fixes parse an exact known catalog-label suffix, recognize a remixer named in the exact requested mix caption, and normalize title-only internal apostrophes for an exact Original Vocal caption with complete artist credits and bounded duration. These changes do not implement a user-selectable permissive policy. The final native suite passes 1,394 tests; this is local Linux WebView qualification, not physical macOS/Windows or statistical startup qualification.

## External matcher comparison (2026-10-20 source snapshots)

[Meld's Spotify-to-YouTube mapper](https://github.com/FrancescoGrazioso/Meld/blob/e3caa6eee4d2f9045fa0048390faa0e4621c7a5a/app/src/main/kotlin/com/metrolist/music/playback/SpotifyYouTubeMapper.kt) considers YouTube Music songs and ranks fuzzy title/first-artist/duration scores. Its raw acceptance floor is 0.35; variant penalties affect selection rather than forbidding a live/cover replacement. [Its scorer](https://github.com/FrancescoGrazioso/Meld/blob/e3caa6eee4d2f9045fa0048390faa0e4621c7a5a/app/src/main/kotlin/com/metrolist/spotify/SpotifyMapper.kt) weighs title 45%, first artist 35%, duration 20%, stripping feature/bracket and some remix/remaster text. It caches matches and supports persistent manual overrides. This policy can accept differences Burrata currently rejects; the source does not prove match accuracy.

[Spotube's current bundled audio plugin](https://github.com/KRTirtho/spotube-plugin-youtube-audio/blob/94d87c573ef253afa330646f866af2943e6bbbca/src/segments/audio_source.ht) searches ISRC first, falls back to title/artists only for an empty result, and leaves result interpretation to the host. [The host source flow](https://github.com/team-spotube/spotube/blob/69a310c78f5ceaf4eab7dfee98f187d38211c9ba/lib/services/sourced_track/sourced_track.dart) preserves search order for English titles, applies a small title/artist/official-marker ranking otherwise, and caches a selected source before stream resolution. Manual sibling selection is supported. Borrow identifier-first discovery, source/metadata separation and understandable candidate selection; preserve validation after nonempty searches, successful-playback caching, freshness and matching-policy provenance. A search query containing an ISRC does not mean every returned video has that ISRC.

## Alias coverage follow-up

An artist-name alias is distinct from a recording credit change. MusicBrainz identifies Lakis Chalkias and Lakis Halkias as names for the same artist, while the matching To Miroloi Tis Manas source also credits an ensemble. Older Spotify captures can lack the verified artist IDs needed to reach the alias crosswalk. An exact-ID Spotify search was observed to return those IDs even when the separate track-details endpoint was rate-limited; an exact-ID search fallback now supplies that metadata under the original account, cancellation and deadline checks. Altered metadata, incomplete credits and conflicting identifiers are rejected; native regression qualification is in progress. No global string substitution or manual source binding qualifies this recording yet.

Spotify negative alias results now require complete hydrated artist identity and are keyed to its verified fingerprint, rather than only visible saved tags. Missing metadata cannot be cached as a completed no-match, and subsequent artist-ID or ISRC enrichment changes the key. Five focused alias-lookup tests pass. This follow-up is not yet qualified in the running binary.

Search confidence can also be stale after a recording-specific credit is verified: a result scored against a former spelling can remain `low` and be rejected before extraction. Low-confidence rows are now re-scored against only validated complete alias variants before candidate gating. Candidate metadata is retained, and all version, permission, duration and fresh-source identity checks still apply. This does not create an optional partial-credit policy.
