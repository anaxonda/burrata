# Sasa: isolated metadata-shape playback check

Development executable SHA-256:
`500f90424ca5507b4996ea15b1669a93deb62cb23f3d57504f4ca1f32668da7b`.
The original main profile was closed. The retained synthetic fixture profile
contained the two public metadata shapes from the regression fixture, with
Monochrome first and YouTube/SoundCloud fallbacks. Volume was zero.

| Check | Result | Observed time |
| --- | --- | ---: |
| Local WAV before | native playback, correct 30-second duration | 442 ms |
| Sasa: separate title/version/duration | unavailable, meaningful reason | 10,325 ms |
| Sasa: merged legacy title | same unavailable reason | 7,809 ms |
| Local WAV after | native playback, correct duration | 343 ms |

Both Sasa attempts found zero Monochrome, 20 YouTube and 10 SoundCloud candidates;
zero candidates passed the recording/version gates. Neither attempted extraction
or played another version. Both finished inside 20 seconds and retained this
visible reason:

> No playable audio match was found for Timmy Regisford — Sasa (Main Version
> (4:41)). No available source matched the requested recording and version.
> Open the catalog source or choose another source.

The saved merged title was unchanged. Playback projected the same base title,
version and 281-second runtime for searching/matching. The check confirms correct
unavailability handling for these returned candidates, not universal source
unavailability or successful playback of this song. It is one request per shape,
not statistical qualification. The later shape may benefit from process caches;
the timings are not a cold/warm performance comparison.

See [redacted outcomes](report.json) and [count/timing source phases](source-phases.json).
The [initial harness report](attempt-1-tool-error-semantics.json) treated the
expected MCP unavailable-command error as a probe failure. The derived report
distinguishes that API result without repeating playback. The reusable driver is
`scripts/performance/verify_sasa_metadata_shapes.mjs`.

These requests also exposed missing native operation IDs for MCP-originated
playback. The following implementation checkpoint adds fallback IDs and separate
receipt-to-audio seek timing; this report predates that fix.
