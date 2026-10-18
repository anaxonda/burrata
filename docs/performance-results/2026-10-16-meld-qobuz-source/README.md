# Meld Qobuz source and bounded catalog checks

The provider was fetched from [Meld commit dc7a27224acee33fa475ab70e1935dc03d3ef4ef](https://github.com/FrancescoGrazioso/Meld/blob/dc7a27224acee33fa475ab70e1935dc03d3ef4ef/app/src/main/kotlin/com/metrolist/music/qobuz/QobuzAudioProvider.kt): 67,823 bytes, SHA-256 `c61d8657fa23f218005389b9c02f87f8b885d8c5547b4247d3fc5fb50f9e4e21`. The exact source URL/hash is retained in `source-manifest.txt`; no provider code was copied into Burrata.

Meld separates community-resolver catalog searches from media URL resolution. It tries four hosts with host cooldowns and a quality ladder. Its matcher gives ISRC a strong bonus after title, artist and duration checks; conflicting remix/live/instrumental variants are rejected, but “Main Version” is not an exact-edition guarantee. Stream cache expiry uses `etsp` when present and otherwise a short TTL. HEAD and one-byte Range requests infer length/bitrate; they do not establish actual player seeking or active-stream refresh. These are observed client contracts, not official Qobuz API guarantees.

Four bounded anonymous metadata-only GETs used the pinned client's search paths for **Sasa — Timmy Regisford — Main Version, 281 seconds**:

| Host | Observed result |
| --- | --- |
| Squid | Local DNS resolution failed before HTTP. |
| TrypT | HTTP 503, 302 bytes of HTML. |
| Jumo | HTTP 200, 53,791 bytes of HTML for its web UI instead of catalog JSON. |
| Kenny | Ten-second timeout, zero response bytes. |

Requests were capped at ten seconds and one MiB each. The two JSON evidence files preserve the results. No credentials, stream endpoint, signed media URL, audio download or decoding were used. These failures establish neither recording absence nor a usable matching source; direct-provider playback remains unqualified.

Public source discovery found a [Squid frontend](https://github.com/squid-wtf/squid-qobuz-dl), but no identified backend implementation for these four hosts. Separate MIT projects [QobuzDL](https://github.com/QobuzDL/Qobuz-DL) and [Qobuz-DL-API](https://github.com/audio-music-streamer/Qobuz-DL-API) document official-API clients requiring app credentials and a paying-member token for full tracks. They do not establish the deployment behind Meld's anonymous resolver hosts. No such credentials were available, so no credentialed local backend was run. Meld's GPL-3 source and Burrata's GPL-3.0-or-later metadata are compatible license families; any future code reuse must preserve attribution and license notices.

Architecture reuse, backend source/self-hosting and direct-provider use are separate conclusions: the orchestration is inspectable, the deployed anonymous backend source remains unidentified, and the four catalog checks supplied no usable playback candidate. Existing Tracks remains the default; no new backend was promoted.
