# Native general YouTube search feasibility

Four anonymous WEB videos-only search requests (two per script version) use the pinned yt-dlp videos filter and the checked-in youtubei.js 18.1.0 WEB client version. Fixed HTTPS endpoint, default certificate validation, no redirects, no account/cookies, eight-second timeout and 1MiB body bound. Only allowlisted public result fields, response hashes and counts are retained; no raw response or tokens. The scripts are the executed probes, not application integration.

All four requests return HTTP 200 and parse candidates. Version 1 caps parsed rows at ten despite responses containing twenty video renderers; its limited absence finding remains retained. Version 2 inspects all twenty:

- `John Denver Matthew`: `ps9X-DXUFGY` is row 12, title `JOHN DENVER - MATTHEW.`, 225 seconds. Another complete-credit discovery title is row 13. The top title-only `Matthew` result remains row 1.
- `Pengus Victory Lap`: `kzrZXghhy1k` is row 1, title `Pengus - Victory Lap`, 261 seconds. Earlier Music songs search did not find this ID in its first ten parsed rows.

The matching title/ID discovery is promising because a **fresh** direct-player response might satisfy the existing complete-credit identity gate without an extractor. Search facts are provisional: they cannot substitute for fresh player identity, establish edition equivalence or authorize audio. The captured title-only Matthew Music result still needs structured proof that the current direct player does not supply; no artist/title is synthesized from search metadata.

Requests took approximately 0.87–1.34 seconds during concurrent optimized Rust compilation. This establishes protocol feasibility, not startup performance, p95, France/macOS coverage or a matched comparison with the helper. A helper-free native general-search → fresh direct verifier experiment remains to be implemented and measured. All misses, ambiguity, identity rejection or direct-access failures must preserve ordinary search/extractor fallback within the original deadline and two-attempt budget.

[Version 1 report](report-v1.json), [version 2 report](report-v2.json), [artifact hashes](manifest.json). The probes write fixed `/tmp` report filenames using exclusive creation; reproductions must intentionally choose a new output path to preserve earlier results.
