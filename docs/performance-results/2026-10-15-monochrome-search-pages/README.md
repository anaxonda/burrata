# Monochrome track-search paging evidence

This read-only investigation checked the pinned Monochrome client source and
unauthenticated responses from its public Tracks API. It did not use an account
or request audio. No credentials or full track listings are recorded here.

The reviewed client is `monochrome-music/monochrome` at commit
`a3c86df0436a3eedc341ab5163fc5ee6b1688b5d`, file `js/tracks-api.js`:

<https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/tracks-api.js#L504-L539>

Its `searchTracks` method documents
`GET /search/tracks?q=:query&limit=:limit`, defaults the requested limit to 20,
and sends no offset or cursor. It consumes `data.tracks`; it does not inspect
continuation fields. The normalized result sets `offset` to zero and reports
the number returned as `totalNumberOfItems`. Those normalized values are local
defaults, not evidence of a source-reported total.

The public root returned HTTP 200 and plain text `hello world`. Search returned
HTTP 200 JSON with top-level keys `tracks`, `releases`, `artists`, `topResults`,
`users`, and `playlists`. A track row contained `id`, `trackId`, `title`,
`artistIds`, `artistNames`, `releaseId`, `artwork`, `explicit`, `playable`,
`duration`, `isrc`, `recordingId`, and `searchPriority`. The search response
contained no total, next-page, cursor, or continuation metadata.

Unauthenticated request observations (counts only):

| Query | `limit` | `offset` | Returned tracks |
| --- | ---: | ---: | ---: |
| `Radiohead` | 2 | omitted | 2 |
| `Radiohead` | 2 | 2 | 2 |
| `Radiohead` | 12 | omitted | 8 |
| `Radiohead` | 30 | omitted | 8 |
| `Radiohead` | 30 | 30 | 8 |
| `the` | 12 | omitted | 4 |
| `the` | 30 | omitted | 4 |
| `the` | 30 | 30 | 4 |
| `love` | 12 | omitted | 7 |
| `love` | 30 | omitted | 7 |
| `love` | 30 | 30 | 7 |

For the repeated `Radiohead`/`limit=2` request, the offset-two response had the
same first two IDs as the request without an offset. The same held for the
other offset comparisons above. The offset parameter is ignored in these live
observations. The API does honor `limit` for the two-result request, but the
queries that returned fewer than 30 rows do not establish what happens when a
query has more than 30 matches. There is no source total to distinguish a
complete response from a truncated one.

Implementation consequence: the candidate lookup may retain its 12-result
bound, while interactive discovery may request up to 30 results through the
documented `limit` parameter. Do not synthesize an offset/cursor. Discovery
should state that coverage is unknown because the endpoint supplies no total or
continuation contract; a page that fills its requested limit should be marked
incomplete with a warning that the source offers no continuation mechanism.
