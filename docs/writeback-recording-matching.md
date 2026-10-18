# Reviewed recording matching for playlist writeback

An unresolved local recording in the Spotify or YouTube sync preview can search
the destination catalog. The user sees candidate title, artist, version and
duration, chooses a recording, then reviews a newly generated complete playlist
diff. Choosing a candidate never applies playlist changes. The chosen recording
labels stay visible and all choices can be cleared. Local tags and source IDs
are preserved.

The search command is `search_remote_playlist_writeback_candidates` with
`bindingId` and `localTrackId`. It returns at most 30 candidates from the first
provider page. Known edition-marker or duration mismatches and explicitly
unavailable candidates are excluded. Other title/artist differences require the
user's explicit judgment; a candidate is not an automatic identity assertion.
Chapter excerpts are not mapped to full destination recordings. All recognized edition markers are compared together: a shared Remix label cannot hide a Live mismatch, and a shared Radio Edit label cannot hide an additional Live marker.

The service issues opaque tokens scoped to binding, provider, account and local
recording fingerprint. Tokens expire after 15 minutes and the cache is capped at
1,000 entries. The existing preview command accepts optional `reviewedMatches`
containing only local recording IDs and tokens. It rejects arbitrary provider
IDs, wrong scope, duplicate choices, expired tokens and changed local identities.
Repeated local occurrences reuse the selected recording while preserving their
separate playlist positions.

Before applying, the stored preview checks current local recording fingerprints
and ordered occurrences as well as the existing remote revision/account checks.
It repeats the local check after the awaited remote/account validation and before
dispatch, so local edits made while a slow provider is being checked invalidate
the preview too.
Changed local metadata or membership requires a new preview. Matching is held for
the current review session and is not written into saved tags or persisted as a
permanent provider mapping.

Fixture coverage exercises the actual candidate search, serialized UI fields,
availability/edition/duration/ID rejection, returned-token use with repeated local
occurrences, and a local edit made while remote validation is deliberately blocked.
The latter must dispatch no playlist edits. The current full native library suite passes 673 tests, including candidate-search regressions for combined edition markers. These fixtures do not prove live provider compatibility.

Live account matching and writeback remain unqualified. No remote playlist was
modified while implementing this flow. Search pagination, richer edition
classification and reusable reviewed mappings are further work.
