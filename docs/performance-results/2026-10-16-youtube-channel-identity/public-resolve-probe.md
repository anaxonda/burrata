# Public YouTube `resolve_url` probe

This is a bounded unauthenticated probe of the fixed public handle `@YouTubeMusic` using the WEB client version currently pinned by Burrata's resolver (`2.20260708.00.00`). The script fetched the public API key from each host's public HTML config in memory, made four `navigation/resolve_url` POSTs, and recorded only response endpoint kind, redirect host/path kind, HTTP status, and whether the browse ID matched the UCID shape. No cookies or authorization headers were sent; the key was not written to the artifact.

- Probe time: `2026-10-16T15:01:58Z`
- Script command: `python3 /tmp/youtube-channel-identity-patch/public-resolve-probe.py`
- Script SHA-256: `80dab704120402cc311ac8ac1f4590d2ed4a8e5471ecd6590993da18edd48b6e`
- Redacted raw JSON: [`public-resolve-probe.json`](public-resolve-probe.json)

| Public page config | Request host | Client/version | Status | Response shape | Redirect host/path | UCID shape |
| --- | --- | --- | ---: | --- | --- | --- |
| www.youtube.com | www.youtube.com | WEB / 2.20260708.00.00 | 200 | browse | none | yes |
| www.youtube.com | music.youtube.com | WEB / 2.20260708.00.00 | 200 | browse | none | yes |
| music.youtube.com | www.youtube.com | WEB / 2.20260708.00.00 | 200 | browse | none | yes |
| music.youtube.com | music.youtube.com | WEB / 2.20260708.00.00 | 200 | browse | none | yes |

Primary source evidence for the endpoint shape is the [Invidious `resolve_url` implementation and documented example](https://github.com/iv-org/invidious/blob/master/src/invidious/yt_backend/youtube_api.cr), which shows a channel URL resolving to `endpoint.browseEndpoint.browseId`. The [pinned ytmusicapi account-info implementation](https://github.com/sigma67/ytmusicapi/blob/c412d7ce346304e339712b1737044733a5264b5e/ytmusicapi/mixins/library.py) documents the Music `account_menu` active header and treats `channelHandle` as optional. The WEB client matrix above is a live public observation, not a guarantee of future undocumented endpoint behavior.
