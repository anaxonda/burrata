# Context menu visual verification

The fixture mounts the real `TrackMenu.svelte` or `PlaylistMenu.svelte` component against the app styles. It uses synthetic track and playlist data and no live app state.

Start Vite from the project root on an unused local port:

```sh
./node_modules/.bin/vite --config vite.config.ts --host 127.0.0.1 --port 5187 --strictPort
```

Open `/docs/performance-results/2026-10-16-context-menu/harness/` for track actions. Add `?menu=playlist` for playlist actions. For screenshot capture, use `--window-size=1280,720` or `--window-size=707,480`. To collect the page title's JSON geometry with Chromium's `--dump-dom` in this environment, pass `--window-size=1280,863` or `--window-size=707,623`; that mode reported a CSS viewport 143px shorter than the requested window. The title contains the measurements after two animation frames.

The captured screenshots and measured geometry are recorded in `context-menu-visual-report.json`.
