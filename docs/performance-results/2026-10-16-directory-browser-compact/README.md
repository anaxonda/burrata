# DirectoryBrowser compact chrome visual check

This fixture mounts the real `DirectoryBrowser.svelte` with synthetic native-call responses and the app’s global styles. It uses a synthetic sidebar, `Folders` header, and player footer; it does not call native code or open the live app.

Start Vite from the repository root on port 5195, then open `/docs/performance-results/2026-10-16-directory-browser-compact/harness/`. Use `?metrics=1` to add viewport geometry to the document title and `?duration=1` to open the duration popover. Chromium screenshot mode uses `--window-size=1914,515` or `--window-size=707,515`; for `--dump-dom` geometry, use window height 658 to obtain a 515px CSS viewport.

The `visual-report.json` records source hashes, screenshot paths, geometry artifacts, and the focused 13-test command.
