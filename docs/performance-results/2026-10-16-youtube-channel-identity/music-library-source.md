# YouTube Music playlist-library source evidence

Reviewed on **2026-10-16** using the public web retrieval tool. This records
upstream implementation evidence, not a private-session response capture.

The reviewed source is sigma67/ytmusicapi's
[`LibraryMixin.get_library_playlists`](https://github.com/sigma67/ytmusicapi/blob/main/ytmusicapi/mixins/library.py).
Its minimal request snippet is:

```python
body = {"browseId": "FEmusic_liked_playlists"}
endpoint = "browse"
```

The method checks authentication, sends that browse request, reads a library
grid, parses playlist cards, and follows grid continuations. This supports
Burrata's narrow change: a Music-origin session lists its own library through
the authenticated InnerTube browse feed. WWW-origin sessions retain the
existing HTML route. It does not establish that Music's `/feed/playlists`
contains WWW-style initial data.

The separately reviewed
[`get_library_contents` parser](https://github.com/sigma67/ytmusicapi/blob/main/ytmusicapi/parsers/library.py)
handles single-column browse tabs and nested library sections. Its grid
continuation handling is also visible in the library mixin. Burrata already
decodes `musicTwoRowItemRenderer` and `nextContinuationData` tokens; the added
synthetic fixture covers the initial grid and a `gridContinuation` response,
including an action tile without a playlist ID.

## Provenance limits

These are **moving `main` URLs, not pinned revisions**. The retrieved GitHub
page did not expose a commit SHA in the available text. Attempts to retrieve
the GitHub commits API and file history failed in the web tool; tag/short-SHA
pages and raw-source URLs also returned cache misses. A shell public-source
request failed DNS resolution. No exact reviewed commit is claimed.

The source establishes the initial Music browse ID and response family.
Burrata's existing continuation request uses a JSON continuation field;
ytmusicapi's wrapper passes continuation parameters separately. The fixture
checks Burrata's normalization and request construction, not live server
acceptance or completeness of a real account's library. Root-owned live
read-only verification remains a separate qualification step.

No private account identifiers, cookies, playlist titles, response bodies, or
credentials were accessed or saved for this source review.

## System collections

Also reviewed on 2026-10-16: ytmusicapi's
[`playlists.py`](https://github.com/sigma67/ytmusicapi/blob/main/ytmusicapi/mixins/playlists.py)
implements `get_liked_songs` by calling `get_playlist("LM", limit)` and
`get_saved_episodes` by calling `get_playlist("SE", limit)`. This provides
primary implementation evidence that these exact short IDs are legitimate
read targets. The same moving-revision limitation applies.

Burrata permits those two exact IDs in its read path, keeps its ordinary
playlist-ID validator for all other IDs, and always marks the resulting
collections non-editable. A separate mutation guard rejects both before a
playlist browse or edit request. The synthetic regression checks both reads,
the existing `VL` browse prefix, forced non-editability even with an owned
editable header, mutation refusal, and rejection of an unrelated short ID.
