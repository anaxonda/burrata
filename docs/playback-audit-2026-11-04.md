# Playback parser follow-up, 2026-11-04

This follow-up records confirmed matcher defects and their source changes. Final native suites and packaged playback of the changed source remain required before release.

| Finding | Disposition | Regression coverage |
| --- | --- | --- |
| Spotify `I Will - Remastered 2009` conflicts with `I Will (Remastered 2009)` | Confirmed. Parse complete remaster labels consistently across delimiters, retaining the raw spelling and explicit year. Compare equivalent `Remaster`, `Remastered`, and digital-mastering spellings through one semantic key. | Captured request/provider fixture; Exact/Balanced/Flexible search and resolved assessments; wrong title, performer, performance, remix and year variants. |
| Whitespace inside square brackets changes version interpretation | Confirmed. Square brackets now use the same balanced, trimmed group reader as parentheses. | Padded delimiters, malformed groups, raw content spans and recording acceptance. |
| A live performance or named mix plus a mastering label conflicts with itself; reversing suffix order changes the title | Confirmed. Build comparison titles from recognized annotation spans and allow one recording label with one mastering label. Preserve unknown text and contradictory labels within the same dimension. | Annotation order/delimiter permutations, separate version fields, guests, neutral original labels, wrong venues/remixes/artists, contradictory mastering years. |
| A metadata assessment proves a delivered source is correctly bound | Unsupported. The pure matcher compares supplied evidence; fresh source binding belongs to playback verification. | The playback-boundary regression requires the exact resolved video ID and checks missing/wrong IDs, fresh wrong artists and wrong recording versions. |
| A singular artist field proves exhaustive credits | Intentional refusal to infer completeness. A singular structured artist can agree with a singleton request while complete-credit evidence remains false. Release alternatives retain their stronger evidence requirements. | Captured singleton metadata and separately labeled synthetic complete-credit variants; release-option checks. |

The saved-request refresh path uses the same remaster comparison key. It still rejects a different explicit year, identifier conflicts and conflicting recording versions. No stored library metadata is automatically rewritten. Parser/evaluator revisions advance to invalidate completed matching decisions; raw query results remain reusable.

The captured YouTube metadata was obtained separately from the sanitized runtime trace. It does not identify which runtime result row was returned and does not establish media availability. Synthetic full-info variants are explicitly separated from captured evidence.

Focused checks execute the production parser/matcher code and extracted regression bodies, with only the unused database alias boundary stubbed. They do not replace native Cargo, resolver integration, actual media delivery, PCM output, UI-route or physical-device qualification. Provider outages and blocked uploads remain separate failure categories.
