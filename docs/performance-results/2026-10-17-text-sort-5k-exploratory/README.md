# Text sort expression experiment — 5k exploratory run

The in-memory 5,000-row experiment completed all four title/artist ascending/descending cases in 9.351s. Each case used one warmup and one timed pair. Candidate medians were 54–79ms versus 143–164ms for the reference, but every query plan still performed a full temporary sort. The benchmark excludes whole-Store/effective-metadata preparation.

These single-pair measurements are exploratory, not production evidence. This original expression-index plan still used full temporary sorts. A later reverse-binary 50k benchmark completed with exact ordering parity and no temp sort in its isolated fixture; its separate 5k run had only one timed pair per case. That integration proposal is staged default-off and not merged, and no production index is installed. [Later reverse-index results](../2026-10-17-text-sort-reverse-index/README.md). [Sanitized case data](result.json).
