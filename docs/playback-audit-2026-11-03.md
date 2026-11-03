# Playback audit — in progress

The audit covers saved Library, search, album, artist, and Spotify-imported entry paths; provider selection; matching options; and delivered native audio. A previous-version comparison is diagnostic when useful, not a required explanation for every failure. No normal-library writes or host audio changes are authorized by these tests.

## Confirmed same-package comparison

Package source a164c991477a5930dc0a9d51d97e14bb805d668d; ELF SHA-256 fcdb160fd82a89039f5b30071465c86636887a75b8277bde94d180348f9bb883. These runs use isolated saved-row/automation playback, not actual renderer row clicks. The native output gate requires 4.5 active seconds and at least 5 seconds of observation after first audio, with a separate 20-second startup deadline.

| Input | Flexible duration policy | Result | Active output |
| --- | --- | --- | --- |
| Bacc, fresh YouTube search entry, duration 198 | Bounded, shorter/longer 30 seconds | Pass | 5.2 s |
| Bacc, saved Discogs entry, duration absent | Ordinary existing duration rule | Pass | 5.1 s |
| Same saved Bacc entry | Bounded, shorter/longer 30 seconds | Resolver identity refusal | 0 s |
| Lady Soul (Oscar P Dub), fresh YouTube search entry | Bounded 30/30 | Pass | 5.1 s |
| Fly Down, fresh Bandcamp search entry | Bounded 30/30 | Pass through YouTube | 4.8 s |

Every completed run reported hostInventoryChanged=false. Fly Down is a Bandcamp entry-path result, not proof of delivered Bandcamp audio. Evidence and fixture/source hashes are retained privately under /tmp/burrata-playback-audit; the Bacc comparison is bacc-policy-comparison.json.

Bacc's missing request duration cannot satisfy the configured duration delta check. Title and artist agree on the freshly resolved candidate, and the ordinary Flexible policy plays it. This is a settings/metadata interaction on the same binary; it does not establish a version regression. A production fix has not yet been applied. Explicit duration bounds must not be falsely reported as verified against an absent duration, and wrong-artist/version/identifier/source-lock protections must remain intact.

## Retrieval finding

The saved Lady Soul request renders `Envie, Oscar P Lady Soul Oscar P Dub (6:07)` and a precision query. Earlier playback WEB-video searches returned two valid empty pages. Fresh Music search without the display clock finds youtube:RJG1NSWNJN4, and that entry now plays. These are different retrieval contexts; this does not prove that the clock alone caused the empty results. A retrieval-only display-runtime cleanup is being prepared, preserving Oscar P Dub and the original request metadata.

## Validation and outstanding work

The matching-policy capture controls passed 37 Python tests. The new renderer-entrypoint helper passed its focused Node test. The Bacc native matcher characterization test is written but has not run: local Cargo dependency compilation exhausted storage before a test binary was produced. That failure is not a playback result. One initial pre-launch capture failed because its harness was edited while the process was starting; it is retained separately and excluded from playback results.

Actual renderer Library/search/album/artist clicks remain unqualified. Wider source delivery (including SoundCloud), Spotify entry paths, more policy combinations, useful previous-version checks, final fixes, and rerun evidence remain required. Monochrome is unavailable and must be reported as such rather than replaced by fixture success. Do not publish on the strength of this partial audit.
