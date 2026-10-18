# Monochrome backend source follow-up (2026-10-17)

## Result

No newly identified source closes the actual Tracks or unified playback server/deployment provenance gap, and no documented hosted endpoint is qualified for a Burrata prototype. The existing frontend pin remains `a3c86df0436a3eedc341ab5163fc5ee6b1688b5d`; its client contracts and the absence of their server handlers are already documented in `docs/provider-backend-investigation.md`.

## New lead checked: `hifi-api-workers`

- The Monochrome organization exposed [`monochrome-music/hifi-api-workers`](https://github.com/monochrome-music/hifi-api-workers), described in its public PR index as a “Cloudflare Workers port of binimum/hifi-api”. The index shows PR #2, “Add caching middleware and small refactors”, opened 2026-08-07: [repository pull requests](https://github.com/monochrome-music/hifi-api-workers/pulls).
- The repository URL currently returns 404 in direct primary-source retrieval; `/tree/main` also returns 404. Neither a source tree, exact commit SHA, license, Worker entrypoint/configuration, secrets schema, nor deployment mapping could be inspected. A public PR listing/repository description is not enough to pin or deploy it.
- Even if recoverable, this is explicitly a port of the *legacy* `binimum/hifi-api` design. Monochrome’s current [`INSTANCES.md`](https://github.com/monochrome-music/monochrome/blob/main/INSTANCES.md) says (April 30, 2026) it switched away from hifi-api “kinda”; it is an outdated instance list, not deployment provenance. The current upstream [`README.md`](https://github.com/monochrome-music/monochrome/blob/main/README.md#self-hosting) says streaming on a domain-hosted self-host requires a working, paid HiFi API endpoint, and the public API is configured to accept official Monochrome instances. This makes the worker-port lead unsuitable as evidence for the current `api.monochrome.tf`, Samidy, unified or Tracks deployment.
- The precise backend source/deployment revision, license, required Tidal credentials and runtime/deployment cost of this worker port remain unknown. Do not prototype it based on the repository name or inferred compatibility.

## Primary-source locations reviewed

- Existing exact frontend source pin and client contracts: [`js/tracks-api.js`](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/tracks-api.js), [`js/api.js`](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/api.js), and the repository's [`functions/api` directory at that pin](https://github.com/monochrome-music/monochrome/tree/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/functions/api). These are frontend source references; no Track stream service implementation or unified playback handler is present there.
- Current upstream migration and self-hosting statements: [`INSTANCES.md` on `main`](https://github.com/monochrome-music/monochrome/blob/main/INSTANCES.md) and [`README.md` on `main`](https://github.com/monochrome-music/monochrome/blob/main/README.md#self-hosting). These are mutable docs, not pinned deployment manifests.
- New lead: [`hifi-api-workers` pull index](https://github.com/monochrome-music/hifi-api-workers/pulls); it exposes PR metadata but not enough source to recover an exact server revision or deploy contract.

This follow-up made no playback-endpoint calls, requested no credentials and changed no application source. The research note was subsequently retained in repository documentation.
