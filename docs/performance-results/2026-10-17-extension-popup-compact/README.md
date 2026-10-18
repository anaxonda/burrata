# Compact browser companion popup

The source popup now uses the existing Burrata icon, a compact primary action row, grouped YouTube controls and collapsible explanatory sections. Existing control IDs, event script, two-step account confirmation and optional permission behavior are preserved. Explicit consent, diagnostic exclusions, one-use code lifetime and cookie-handoff disclosures remain present.

Verification: `node --test extension/tests/popup.test.mjs` passed (one behavior suite); the complete extension suite also passed on the host (76 tests). The sandbox full-suite attempt hit `spawnSync python3 EPERM` in the installer fixture; the host fixture writes only temporary manifests and does not install them. Independent source review checked IDs, disabled/hidden states and unchanged `popup.js`/manifests. Root reviewed `.tmp/browser-popup-stage/popup-layout-only.png`, rendered at 400×850 with the script deliberately omitted: no overlap or clipped controls. The initial script-enabled file render showed an expected missing-extension-API error; it is not an installed-extension result.

Installed-extension smoke verification is still open. This source/resource change is not yet in the running Burrata binary. Evidence: `.cache/qualification-track-failure-history/extension-popup-tests.log` and `intentional-source-changes.json`.

A read-only check of the sole normal app (`dd9474d3fdf9`) found no playback-bar geometry findings at 954×1036; footer height 112.39 pixels. This is one window-size check, not the complete desktop matrix. The same tool still reports the existing failed artwork probe, so artwork qualification remains open. Raw evidence: `.cache/qualification-track-failure-history/normal-playback-layout-private.json`.
