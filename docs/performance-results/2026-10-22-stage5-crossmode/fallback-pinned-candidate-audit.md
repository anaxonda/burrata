# Bounded fallback pinned-candidate audit

Date: 2026-10-22. This is a read-only follow-up to the final preferred-provider
fallback run for the 13-track Bulgarian release.

The run processed all 13 tracks and queued 11. The two skipped requests were
“Messetschinko lio Greïlivko” and “Schopska Pesen.” I reassessed each previously
qualified Soulseek candidate against the current saved MusicBrainz request using
the shared download assessor. Both returned **Probable / Accept** with
metadata-only proof:

| Request | Request runtime | Prior candidate runtime | Current assessment |
| --- | ---: | ---: | --- |
| Messetschinko lio Greïlivko | 154.0 s | 154.0 s | Title, artist, and runtime Agree; ISRC USNO18716506; no version conflict |
| Schopska Pesen | 88.52 s | 90.0 s | Title, artist, and runtime Agree; no ISRC or version conflict |

Both candidates have validated Soulseek filename-caption evidence and a
request-scoped MusicBrainz artist-credit name equivalence. The caption evidence
does not raise the result above Probable. Candidate references are represented
only by SHA-256 hashes: `ae3a6c4b4d425d510d8c258078a581c19cd2de6e3f9f0d323d36bcdddf1f3f0f`
and `5d9676d0a54ca56a8aefade048c04ba1aa33606746a6ecbdfccb02ceae9ec9e7`.

Because the previously qualified candidates still pass the current assessor,
these two misses indicate retrieval or candidate-selection limits in this
bounded fallback run. They do not establish that the peers were offline or that
the matching policy rejected the correct files. The observed fallback result is
**11 of 13 queued**, not 13 of 13. Separately, the original 13-track album
assets had already been verified complete in the prior album run; this fallback
observation does not invalidate that result. It also does not establish
exhaustive provider coverage or completion of every source query.
