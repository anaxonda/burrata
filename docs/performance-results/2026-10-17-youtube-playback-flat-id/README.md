# YouTube playback flat-search ID handling

Playback candidate parsing previously filled an absent URL but rejected a flat-search row whose URL was the matching bare video ID. The integrated change canonicalizes that exact 11-character ID representation to a YouTube watch URL. Explicit video URLs must identify the same candidate; external hosts, credentials, mismatched IDs, channels and playlists remain rejected. Recording, version, duration and permission checks are unchanged.

Two native regressions pass: a direct admission matrix and a fake-tool search containing bare-ID, explicit-URL, missing-URL and rejected rows. The focused test command completed successfully with two passes. The full native suite also passes: 1,035 passed, zero failed and 21 ignored, in 19.48 seconds. Its log is retained under `.cache/qualification-youtube-bare-id/native-full.log`.

The earlier album trace does not retain raw rejected rows, so it cannot establish that bare IDs caused those failures. Actual album audio, source coverage and latency remain unqualified. This change has not yet been rebuilt into a GUI candidate or exercised against live provider results.

Private integration evidence retains the pre-change source, before/after hashes and frozen patch hash under `.cache/qualification-youtube-bare-id/`.
