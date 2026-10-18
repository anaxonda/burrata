# Context-menu source hit-target refinement — 2026-10-17

The merged four-file frontend refinement places the optional SourceLink icon snippet inside the full-width padded button and preserves its opener, range action and callback wiring. Light-theme danger text uses `#a62c29`, with a 6.97:1 contrast ratio against white.

Verification passes: 52 focused tests across four frontend files (4.26s), Svelte check 0/0, and the production frontend build (4.42s). A bounded post-patch review opened the first visible Library row’s More menu in the normal dark-theme UI using PID/executable-guarded AT-SPI, then closed it through Close track actions. The Source icon and row aligned, Add to playlist was prominent, and the menu did not overlap the footer. At 1914×515, the footer was 98.5px high starting at y=416.5; layout inspection found no issues. The full post-UI snapshot exactly matched the pre-UI snapshot (13 occurrences, index 6, paused at 29.087175s). The private screenshot is not published.

This check covers the inspected dark-theme Library menu only; it does not establish live light-theme behavior or broad desktop UI acceptance. [Sanitized result](result.json).
