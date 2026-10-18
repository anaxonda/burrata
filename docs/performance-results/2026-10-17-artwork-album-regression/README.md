# Artwork cache and album-route regression — 2026-10-17

The focused App/artwork and album-route suite passes 289 tests across two files in 40.83s. Svelte check reports zero errors and warnings, and the production frontend build passes. An earlier run passed 288 tests and failed one assertion because it counted an album-navigation call; after the test captured calls before Play, the focused regression and full suite passed. This is a test assertion correction, not an application failure.

Nine cache cases and three App cases verify the bounded image LRU: with a three-image/read cap, the active footer and Details URLs (at most two total) remain protected, with no metadata mutation. The reviewer inspected the actual 1914×515 main window: footer artwork rendered via the data fallback, footer height 98.5px, and zero layout findings. The private screenshot is not included.

The native source is unchanged from source pin 513; four frontend cache-related files changed. The underlying asset-protocol failure is not established as fixed. The actual A Portrait of Nikolai Kaufman live album playback issue and same-case default-WebView Sasa check remain open. [Sanitized metrics and safe artifact hashes](result.json).

## Follow-up exact-URL diagnostic

On normal build `f0ece0d9f984`, the fresh probe's `src` and `currentSrc` both match the expected owned asset URL, but it fires `onerror` with zero image dimensions. The WebKit observer is attached and sees five other resource events, with zero recognized asset requests. CSP-blocked remains false; footer data fallback loads52×52 with no layout findings. Independently, the actual cached file is a regular non-symlink1,415-byteJPEG,52×52; Pillow verification passes. This rules out missing/corrupt image bytes for this case, but does not establish whether the built-in asset handler ran or returned an error. The next proposed diagnostic observes only fixed error categories from Tauri's existing handler; it must not replace its scope/path checks. Private receipt: `.cache/qualification-shared-deadline-http-artwork/layout-private.json`.

## Built-in handler logger and offscreen WebKit control

On candidate `7c8c46e64110`, the opt-in path-redacted logger installs successfully. The real artwork probe still errors with0×0 dimensions, no CSP block, five other resource events and zero asset resource events; all fixed built-in-handler error counters stayzero. The visible footer fallback continues to load the verified52×52 artwork. Zero selected error logs do not establish zero handler invocations: successful responses and generic500 errors are not counted.

A separate ephemeral offscreen WebKitGTK4.1 control serves an embedded1×1 PNG through temporary custom schemes. All five cases (asset simple, encoded absolute, encoded with query, double-slash absolute, separate custom-scheme control) record one handler entry, one resource-start event and a loaded1×1 image, with no resource failures, callback errors or timeout. No production handler, scope, user file or network request is used. This narrows the issue to app integration/response rather than generic unsupported WebKit custom schemes. It does not prove what the real Tauri asset handler returns. Private output: `.cache/qualification-combined-desktop-playback/webkit-scheme-probe.json`; disposable harness: `.tmp/webkit-scheme-probe.py`.


## Current normal-app read-only check

On `dd9474d3fdf9`, the current cached-file probe still fails while the file exists and both file/cache directory grants are present. Config, Cargo feature and URL form remain consistent with Tauri asset support. Wry GTK marks custom schemes secure; `displayIsolated=false` alone does not establish a defect. Zero observer resource events do not prove the handler was never entered. No registration or permission change was made. The next narrow diagnostic is redacted handler-entry/status/MIME/body-length evidence, including generic responder errors. Raw receipt: `.cache/qualification-track-failure-history/normal-playback-layout-private.json`.

## Current half-width desktop follow-up

On current native `ecc2c40a10e8` / frontend `9edf424726b2`, a read-only authenticated layout inspection and app-owned screenshot at 954×1036 show the footer artwork loaded at 300×300 through the data fallback. The footer is 94.5px high, all visible transport/scrubber/right controls remain inside the viewport, and layout findings are empty. Visual review confirms no footer overlap in this Library state. This does not qualify other views, open panels, or themes.

The exact cached asset probe still fails with zero image dimensions, while the cached file and directory grants are present and CSP-blocked remains false. The fallback display check passes; the underlying asset transport remains unresolved. Existing protocol counters are wired through `artwork_resource_probe`; their zero selected-error counts do not establish whether the built-in handler returned a response. A separate private handler-wrapper instrumentation patch is prepared but has not been built or run. Private evidence: `.cache/qualification-buffering-ui-recovery/current-layout-followup-private.json` and `current-half-width-private.png`.
