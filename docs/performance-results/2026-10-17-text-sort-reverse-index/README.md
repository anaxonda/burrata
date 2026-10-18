# Reverse-binary text sort benchmark — 2026-10-17

The isolated 50,000-row in-memory benchmark completed in 151.79s. Across title and artist, ascending and descending, each case used three warmups and 20 paired timed reads. All four candidate orderings matched the full 50k reference order. Their query plans scanned the expression index and used neither a full nor right-part temporary sort. The retained result includes every timing pair and candidate/reference median, p95 and worst value.

A separate 5k run completed all four cases with one warmup and one timed pair each; it is exploratory only. The earlier 5k experiment with the original expression-index plan did use full temporary sorts and remains preserved in its own report.

These measurements cover only in-memory expression sorting. They exclude Store filtering/hydration, index creation and update costs, resource use, and development/release behavior. The production code does not install these indexes; a default-off integration proposal is staged but not merged. This is not a whole-application or production performance claim. [Sanitized benchmark data](result.json).
