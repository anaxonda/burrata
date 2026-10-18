# Expanded player UI verification

Linux development executable SHA-256 `8171563ac35f9b73a7a714f86f4a3a85b1a7dd60de8fc8b9f5648b41f282844e`, with Vite serving the current CSS. One isolated app instance, 11 synthetic records and a muted local WAV; main profile closed and preserved during verification.

App-specific MCP controls started and paused the fixture and opened the queue. `check-responsive-player.mjs` applied and verified 500×800, 360×640 and 1000×650 viewport sizes and restored the prior size. All three produced zero layout findings after the fix. Before the fix, 360×640 reported internal horizontal overflow in now-playing (81 px) and quality (83 px). Track-copy flex sizing and compact quality constraints resolve that overflow.

`queue-360.png` is a read-only capture of the verified isolated Burrata window, visually inspected: queue heading, Identify audio, close button and footer controls do not overlap. Track/quality text truncates within its available column. This covers the paused, populated local-file queue layout; it does not establish every busy, failure, chapter or platform layout.

The first screenshot showed onboarding covering the player; it was rejected as evidence of visible player layout. Setup was marked complete only in the synthetic profile before these checks. The scoped checkbox-size fix was subsequently visually checked at 360×640 on development executable `8e2bdfa41acc00986a543f9956aec44858ec1b5e7d3f3b5d88720602d3a46dab`; `onboarding-360.png` shows the compact checkbox and readable label.
