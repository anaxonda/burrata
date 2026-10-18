# Disconnected Downloads UI fixture

Renders the real `DownloadActivity.svelte` and `VirtualResults.svelte` with synthetic 500-job data. The main Downloads toolbar and job-card markup mirror the current App view; the app shell, route, and neutral reserved player area are fixture content. The fixture does not mount `App.svelte`, Tauri APIs, production storage, or real action handlers. Use `?mode=activity` for the activity component and `?mode=all` for the virtualized route fixture. These captures are limited to component/layout review, not App/native integration.
