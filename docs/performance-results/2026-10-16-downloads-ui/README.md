# Disconnected Downloads UI evidence

Captured 2026-10-16 with Chromium 154.0.8037.97 (Arch Linux), using a fresh headless Chromium profile and a Vite fixture under `/tmp`. No native Burrata window or regular browser was launched or controlled.

The fixture mounts the real `DownloadActivity.svelte` and `VirtualResults.svelte`. Its main Downloads toolbar and job-card markup mirror the current App view; the app shell, route, synthetic data, and neutral reserved player area are fixture content. Activity's View all action switches between fixture modes. The action handlers are inert. This is disconnected component/layout evidence only: it does not mount `App.svelte` or Tauri and does not exercise route wiring, persistence, actual downloads, the real player, or native window sizing.

Visual review at 360, 500, and 1000 px found the activity header's View all, Refresh, and Close controls remain visible. At 360 px they wrap below the title without overlap. Long titles and the synthetic error message wrap within activity cards. In the main Downloads fixture, the sort, direction, duration bounds, and Refresh controls wrap into available rows at 360 px and align more compactly at 500 and 1000 px. Job details and actions stack below 600 px, giving titles the full row width; the existing side-by-side layout remains at 1000 px. The 360 px screenshot starts at the top of the scrollable queue, so the first long row extends below the visible viewport and its actions require scrolling. These observations are limited to the Download Activity header, toolbar, and job rows rendered in the fixture; the full App shell/player geometry is outside scope.

The initial captures from before the narrow-row CSS and neutral player placeholder are preserved with `-initial` filenames. The current matrix is below.

## Screenshot matrix

| File | Viewport | State |
|---|---:|---|
| [activity-360.png](screenshots/activity-360.png) | 360 × 800 | View all / Refresh / Close, long title, progress and error rows |
| [activity-500.png](screenshots/activity-500.png) | 500 × 800 | Same activity states and actions |
| [activity-1000.png](screenshots/activity-1000.png) | 1000 × 800 | Activity overlay over Downloads page |
| [all-360.png](screenshots/all-360.png) | 360 × 800 | Full Downloads controls; stacked, scrollable job row |
| [all-500.png](screenshots/all-500.png) | 500 × 800 | Full Downloads controls, stacked long title and error row |
| [all-1000.png](screenshots/all-1000.png) | 1000 × 800 | Full Downloads controls and wide side-by-side job rows |

## Source and harness SHA-256

| Source | SHA-256 |
|---|---|
| `src/App.svelte` | `a50dde025b2eae7f02ad928dffdd6b7cbbb1e33c890e02aea3b1f20c809e868d` |
| `src/DownloadActivity.svelte` | `469b157c3489ed71d836963f96e0a16f1dfb55c39644c9938e84de86c4f6e5cd` |
| `src/VirtualResults.svelte` | `99ffb67356c4e84f55fe5f1a75cc77cfca5ccd6f3c12daf87fdebc652060afee` |
| `src/style.css` | `b0d7a1a507e21ff7684dacf0ce3338158d13be6f5180c817cb582ec40ae3a66d` |
| `src/desktop-layout.css` | `361b540fbbf653522277446fa0dc8cb9688e8fe80ec72de6716d387373a9810c` |
| `harness/Fixture.svelte` | `3ca3c756b44d65237295e12de502c4e9accd7bd39adea8a15ebf75b467d91e77` |
| `harness/index.html` | `2b73c0da63a6b1de8b8cdb530c1f23e398eab4da68be9689053fb65f7b8b9720` |
| `harness/main.ts` | `af12bac065f3a4359204f654ce8f7162d5545c77a2b5cb79dd9520661d2639b2` |
| `harness/vite.config.ts` | `32c2117006bef598c17f34b33ea714f34439f491d7aa6cb42b908b2010dbb27a` |
| `harness/README.md` | `1e0e0502643c3fe454eb3594d75368814403f86bb79ae753163d07320ef8602c` |

The fixture sources are archived in [harness](harness/); they import repository components and CSS by their current absolute workspace paths.
