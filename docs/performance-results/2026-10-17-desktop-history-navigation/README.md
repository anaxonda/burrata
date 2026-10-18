# Bounded desktop History navigation check

On the normal `f0ece0d9f984` app, navigated Library → History → Downloads, Back twice, Forward once and Back once through the owned MCP route API. Route-transition acknowledgements completed. Root reviewed the loaded History and final Library captures at954×1036; final Library controls and visible rows are restored, with no visible playback-bar overlap. The full playback snapshot is identical before and after.

This verifies this route sequence and final rendered state only. Intermediate Back/Forward acknowledgements do not independently prove each loaded destination or every filter/scroll state, and the test does not qualify global navigation. History's bulky filter area remains a UI improvement task. Captures and private state receipts are under `.cache/qualification-desktop-history-navigation/`.

## Compact History toolbar checkpoint

Candidate `7c8c46e64110` replaces the always-expanded History filters with a compact query/Search/Filters row. Advanced controls retain their existing bindings behind a bounded disclosure, with outside-pointer and Escape dismissal. Two focused integration tests pass;309 unrelated App tests were skipped by that focused invocation. Full frontend check0/0 and build pass.

Actual app-owned captures at1914×515 and954×1036 show aligned closed-toolbar controls and readable player controls; both native layout inspections report no findings. The half-screen check temporarily floated only the hash-verified Burrata compositor container, then restored its original tiled layout and Library route. Full playback snapshots are exactly equal before and after all checks (100 occurrences, index2, paused). Private captures/reports are under `.cache/qualification-combined-desktop-playback/`.

Eight separate exact-component Chromium captures cover open/closed filters in both themes at707×515 and954×1036. They verify bounded popover geometry and aligned buttons; their synthetic footer is not production-footer evidence. The actual native disclosure was subsequently opened/closed through its exact app-owned accessibility action, reviewed at1914×515 and954×1036, and reported no playback-layout findings. Original tiled layout and Library route were restored. Native keyboard/outside dismissal remains covered only by component tests.

The full frontend rerun passes846 tests across71 files. Its preceding run passed845 and failed one globally-last IPC assertion because a playback heartbeat followed the relevant history request. The test now checks the last history request with every original filter/pagination assertion intact; runtime code did not change. Both runs are retained under `.cache/qualification-combined-desktop-playback/`.
