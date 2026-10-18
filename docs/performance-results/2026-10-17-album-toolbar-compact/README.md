# Spotify album toolbar desktop regression

The album branch lacked the compact rules used by artist top tracks. Flex cross-axis stretching, column labels and inherited control padding produced the oversized sort/direction/save controls in the reported screenshot. Shared component rules now align labels horizontally and give sort, duration and save controls a34px border-box height. The component uses the existing theme background token rather than an undefined dark fallback.

An actual1914×515 Burrata capture after the first fix shows compact album controls and no footer overlap. Exact-component fixtures at1365×768 and954×515 show all controls34px high and no horizontal overflow. The954px dark/light fixture confirms the theme-token correction; its footer is a synthetic geometry boundary, not production-footer acceptance. The actual full queue/playback snapshot is exactly unchanged across the refresh (100 occurrences, index2, paused, position0).

Evidence: `.cache/qualification-album-toolbar-compact/album-after.png`, before/after private snapshots in that directory, and `.tmp/spotify-album-toolbar-visual/metrics-954x515.json` with reviewed dark/light captures. Svelte checks pass0 errors/0 warnings. This is bounded album-toolbar acceptance, not completion of every desktop view.
