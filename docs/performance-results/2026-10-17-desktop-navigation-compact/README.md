# Desktop Downloads, Settings and repeated navigation

The current normal app (native `7a43ea0c8def`, App `33260ec39305`) was checked at 1914×515 and 956×513 in dark theme. Downloads controls wrap compactly, duration hints fit, Settings has one page heading, and neither route overlaps the playback bar. Opening Settings twice and going Back returns to Downloads at both widths. Screenshots and layout inspections are retained in `.cache/qualification-desktop-downloads-settings/*-after*` and `*-half-final*`; layout findings are empty. The earlier `downloads-half.png` actually captured Settings and is retained as defect evidence, not Downloads acceptance.

The fix skips duplicate history entries only for unchanged plain top-level pages. The integration regression also checks that changing Spotify to YouTube within source browsing still records history. Negative controls reject both the original duplicate-entry behavior and suppressing all same-view history.

The full frontend suite passed 922 tests. After three test-only provenance type corrections, the three affected focused tests passed, Svelte reported zero errors/warnings, and contracts and the production build passed. Logs are in `.cache/qualification-desktop-navigation-compact/`. The complete 30-occurrence queue and playback snapshot stayed exactly equal after live checks; original tiling and window geometry were restored, and the app was returned to Library. See `ui-restoration-proof.json` in that evidence directory.

This closes these specific desktop and navigation defects. Broader routes, keyboard/scroll behavior, light-theme native UI and loading/error states remain open. The separate light DownloadActivity component fixture is not full-app acceptance.
