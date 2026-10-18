# Queue menu visual review

Synthetic fixture only; no real queue, provider, or playback state was used. The fixture renders 24 synthetic queue rows, two synthetic chapters, and a fixed 90px footer. The latest chapter captures include **Add chapter to playlist**.

| Viewport | Track overflow menu | Chapter overflow menu |
| --- | --- | --- |
| 1280×720 | `queue-track-1280.png` | `queue-chapter-1280.png` |
| 707×480 | `queue-track-707.png` | `queue-chapter-707.png` |

The chapter menu stays within the viewport and above the footer at both sizes. At 1280×720 it spans x=1032–1239 and y=447–624, leaving 41px to the right edge and 6px above the footer. At 707×480 it spans x=463–670 and y=207–384, leaving 37px to the right edge and 6px above the footer. All four actions are 36px high; each 20px icon aligns with its label. Document scroll width equals the viewport width at both sizes. The longer queue remains scrollable behind the popup.

Fixture Vite and the isolated Chromium session were stopped after capture. The actual application and playback state were not opened or changed.

Source hashes reviewed for the latest chapter-action UI:
- `src/PlaybackQueuePanel.svelte`: `6054b183ad5b725d87e31c7646b7736c306e062a75a580bfd88d3b9a92296906`
- `src/MenuIcon.svelte`: `c5a359beff835bdb5a4b7637515b3fab0715cde877cfc46ff28d7498ff467565`
- `src/PlaybackQueuePanel.test.ts`: `b91532498ff11d177ce150d0f58e2b09df258be441a608ef2ef6a77551bef3ed`
