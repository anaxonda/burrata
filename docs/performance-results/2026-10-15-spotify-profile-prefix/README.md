# Bounded Spotify profile prefix collection

The reviewed [Spotifly getUser implementation](https://github.com/tr1ckydev/spotifly/blob/4fc289abc02e29c2c60f9fce94c87ab1cfa58982/src/index.ts#L47-L49)
uses a configurable playlist prefix length. It does not expose continuation in
that method. Burrata now requests growing prefixes, checks the earlier URI order
against a SHA-256 cursor fingerprint, and returns only newly collected rows.
Cursors bind to the normalized profile ID. Requests are capped at 200 raw rows;
no growth, changed earlier entries and malformed cursors stop collection.
Unknown, inconsistent or unrecognized results never establish completeness.

Live read-only Linux check using a retained isolated native Spotify session:

| Page | Reference | New rows | Time | Reported total | More |
| --- | --- | ---: | ---: | ---: | --- |
| 1 | ID | 30 | 1,805 ms | 89 | yes |
| 2 | official URL | 29 | 1,633 ms | 118 | yes |
| 3 | ID | 18 | 1,728 ms | 136 | yes |
| 4 | official URL | 0 | 1,734 ms | 136 | no |

The collector retrieved 77 distinct playlists, with zero duplicates, in 6,901 ms.
The prior single-request implementation returned 49. The provider's reported
total changed with these requests and remained greater than the exposed rows;
the result remains explicitly incomplete. This does not prove that the missing
rows are accessible, explain their absence, or qualify complete profile browsing.
See [sanitized live report](report.json).

The [first exploratory attempt](attempt-1-short-prefix.json) stopped at 59 because
it treated a shorter-than-requested prefix as terminal. The corrected rule
continues while rows grow and a known reported total still indicates more.
Fixtures cover short growth followed by no growth, changed prefixes, different
owners, missing/inconsistent totals, omitted rows and the cap. Legacy playlist
URIs are normalized to canonical playlist IDs/URLs.

The final development executable is
`500f90424ca5507b4996ea15b1669a93deb62cb23f3d57504f4ca1f32668da7b`.
Adapter tests pass 10/10 and worker tests 14/14 after the growth correction.
The preceding full checkpoint passed 629 native tests (13 ignored), 428 frontend
tests and seven contracts, with clean Svelte diagnostics/frontend build.
No account confirmation, credential migration, import or remote write occurred.
The isolated app was closed before restoring the original main profile. Its
playbar inspector reported zero findings at 954×1036.
