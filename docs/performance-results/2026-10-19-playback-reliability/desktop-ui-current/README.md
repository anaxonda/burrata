# Desktop inspection, 2026-10-19

Current debug runtime from `eeb19fb`, using the current Vite frontend and one disposable profile. Normal library, credentials and queue were not used. The launcher verified compiled `main.ts`, `App.svelte` and `style.css` before starting the executable; authenticated MCP confirmed the frontend mounted.

At the actual 954 × 1036 desktop viewport, the playback timeout control became visible and focused, its text was fully readable, and settings/playback controls did not overlap. [Settings capture](timeout-settings.png), [navigation completion](timeout-navigation.json), [geometry](settings-layout.json).

The muted six-minute synthetic local WAV advanced in the WebView media clock. With the information panel open, library titles remained visible and the playback bar retained separate metadata, transport, seek and volume controls. [Active capture](active-info.png), [geometry](active-info-layout.json), [progression samples](local-webview-progression.json). Playback was paused after inspection.

This verifies the captured desktop states only. It is not audible-output, native-engine, larger-display, macOS/Windows, remote-provider or complete-recording qualification. The timeout setting remained at its default 20 seconds.
