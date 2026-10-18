# Search history and occurrence-boundary fixes — 2026-10-17

The frontend now snapshots search controls, loaded page/cursor state and scroll position for bounded account-scoped Back/Forward restoration without refetching. Search-to-artist history is covered; search-to-album remains staged and untested, so this does not resolve the global Back/Forward request. Chapter-heavy lists cap the displayed child-row budget and mount collapsed chapter rows lazily.

Occurrence navigation now distinguishes explicit duplicate/chapter selection from Resume: selecting an occurrence resets to its range start, while Resume retains the saved position. A reproduced chapter-boundary plus media-ended race that advanced twice was fixed. A 1914×515 app-owned capture was reviewed with no layout findings. These are source and focused UI checks, not live route-flow or new audio qualification.

The App suite passes 289 tests (52.44s); SearchPanel/cache/scope suites pass 50 tests across three files (7.19s); Svelte check is 0/0 and the production build passes (5.67s). The native/provider inputs are unchanged from this frontend source pin. A separate default-off text-index prototype remains isolated and is not performance-promoted while its actual Store benchmark is pending. [Sanitized result](result.json).
