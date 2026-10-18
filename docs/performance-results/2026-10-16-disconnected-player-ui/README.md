# Disconnected player UI layout evidence

Captured 2026-10-16 with Chromium 154.0.8037.97 (Arch Linux) using the isolated Vite harness in `harness/`. Each screenshot is a fresh headless Chromium render at the viewport shown; no native Burrata window or regular browser was controlled.

The fixture imports current `style.css`, `desktop-layout.css`, and `player.css`, and mounts the real `TrackList`, `PlaybackQueuePanel`, `PlayingInfo`, `PlaybackQuality`, and `SeekSlider` components. Its footer mirrors the current App markup, classes, control ordering, labels, and source/quality/seek areas. Its static page and synthetic data are **not** `App.svelte` or Tauri integration. These captures support component/layout review only; they do not prove application boot, persistence, audio behavior, native resizing, or computed geometry through App's runtime state.

Visual review found the player footer remains visible in the captured states; player title, quality, and source text stay within their compact area; queue Identify and active Cancel/progress labels fit; the queue/details panels end above the footer; and no obvious control overlap or horizontal overflow appears at the selected widths. Unavailable playback expands the player error into a second footer row without covering the transport. This was visual inspection, with no DOM geometry assertions.

The first fixture mount attempt used `new Fixture()` and failed under Svelte 5; switching to `mount()` fixed it. A few later captures were discarded after the temporary server had been stopped or parallel headless processes exhausted `/tmp`; the archived matrix below contains only the successful sequential recaptures. The Vite server and temporary Chromium profile were stopped/removed after capture.

## Screenshot matrix

| File | Viewport | Synthetic state |
|---|---:|---|
| [360-playing.png](screenshots/burrata-ui-360-playing-final.png) | 360 × 800 | Long title, playing, collapsed panel |
| [360-queue-idle.png](screenshots/burrata-ui-360-queue-idle-final.png) | 360 × 800 | Buffering, expanded queue, idle Identify audio, active chapter |
| [360-queue-active.png](screenshots/burrata-ui-360-queue-active-final.png) | 360 × 800 | Buffering, expanded queue, Cancel capture progress, active chapter |
| [360-details-active.png](screenshots/burrata-ui-360-details-active-final.png) | 360 × 800 | Paused, expanded details, Cancel capture progress and PlayingInfo actions |
| [500-playing.png](screenshots/burrata-ui-500-playing-final.png) | 500 × 900 | Long title, playing, collapsed panel |
| [500-queue-active.png](screenshots/burrata-ui-500-queue-active-final.png) | 500 × 900 | Buffering, expanded queue, Cancel capture progress, active chapter |
| [500-details-active.png](screenshots/burrata-ui-500-details-active-final.png) | 500 × 900 | Paused, expanded details, Cancel capture progress and PlayingInfo actions |
| [1000-unavailable-details.png](screenshots/burrata-ui-1000-unavailable-details-final.png) | 1000 × 800 | Unavailable source/error, expanded details |
| [1000-queue-active.png](screenshots/burrata-ui-1000-queue-active-final.png) | 1000 × 800 | Buffering, expanded queue, Cancel capture progress, active chapter |
| [1440-playing.png](screenshots/burrata-ui-1440-playing-final.png) | 1440 × 900 | Long title, playing, collapsed panel |
| [1440-details-active.png](screenshots/burrata-ui-1440-details-active-final.png) | 1440 × 900 | Paused, expanded details, Cancel capture progress and PlayingInfo actions |

## Source and harness SHA-256

Hashes identify the current source used for this visual pass and the archived fixture source.

| Source | SHA-256 |
|---|---|
| `src/style.css` | `04c8473bb5fd64d24184722b4269588a500e9510e1993b0d5cbf7dd755b829ea` |
| `src/desktop-layout.css` | `361b540fbbf653522277446fa0dc8cb9688e8fe80ec72de6716d387373a9810c` |
| `src/player.css` | `b3e6257290c2d34aa1382199d6cc01d91ba00d6ae306b91dd7cb9f34fd48c20c` |
| `src/TrackList.svelte` | `1730fc110b4b7116deaa6031c022fe11d1484aa2e1736d470df7d2faa62dcfd4` |
| `src/PlaybackQueuePanel.svelte` | `3624aa8395c65cefba9cc6f51681c1df34f030fd77ad0ffa9d7807e6021d7010` |
| `src/PlayingInfo.svelte` | `5c8c810e3109340eadb6f7ea3104acb283c8c62bd2a453991501e3a23eb6e459` |
| `src/PlaybackQuality.svelte` | `e26b31cb25efce4b4f24b591b65aa53886c10b3041261d865f66cabb96ac0b36` |
| `src/SeekSlider.svelte` | `199a500e9c61cb8abaa641393df17373bd2e9704b9d62f8a2d81570a69510e58` |
| `src/App.svelte` | `85283e8c993a589b019cb3daa87fd9aa428b3bf5f134c17d93af0e54fba6eb67` |
| `harness/Fixture.svelte` | `799bfbcc7f49843df229f9a8adf74cfa0837b00eb616c60466a057921bb0e018` |
| `harness/main.ts` | `2371183b5edae4c8541f5b83b0e040c57064c794a651e25cece06614cb44609b` |
| `harness/vite.config.ts` | `f49bf3fc7a1231d1e2c61708449daa3464eba54045dcd1a2ef2680ce885d4bd9` |
| `harness/index.html` | `c214c158e96145cbc73c96b070f15a7715b0192358587fce516e14e5683890a1` |
| `harness/README.md` | `9f68d8809bab20d6d64a73fce39eb1a0f11c961a8b62f438dbb8a4b6a5a30076` |
