# Playback parser follow-up, 2026-11-04

This follow-up records confirmed matcher defects and their source changes. Final native suites and packaged playback of the changed source remain required before release.

| Finding | Disposition | Regression coverage |
| --- | --- | --- |
| Spotify `I Will - Remastered 2009` conflicts with `I Will (Remastered 2009)` | Confirmed. Parse complete remaster labels consistently across delimiters, retaining the raw spelling and explicit year. Compare equivalent `Remaster`, `Remastered`, and digital-mastering spellings through one semantic key. | Captured request/provider fixture; Exact/Balanced/Flexible search and resolved assessments; wrong title, performer, performance, remix and year variants. |
| Whitespace inside square brackets changes version interpretation | Confirmed. Square brackets now use the same balanced, trimmed group reader as parentheses. | Padded delimiters, malformed groups, raw content spans and recording acceptance. |
| A live performance or named mix plus a mastering label conflicts with itself; reversing suffix order changes the title | Confirmed. Build comparison titles from recognized annotation spans and allow one recording label with one mastering label. Preserve unknown text and contradictory labels within the same dimension. | Annotation order/delimiter permutations, separate version fields, guests, neutral original labels, wrong venues/remixes/artists, contradictory mastering years. |
| Version fields and title labels use different vocabularies or Flexible permissions | Confirmed. Share spelling keys and category classification for recognized edits, extended versions, dub, a cappella, mono/stereo and tempo variants. Only changed version components require alternative permissions. | Same-version field/title pairs; options enabled/disabled; same live performance with another remaster versus another venue; typed alternative classification/ranking. |
| An artist alias consumes the second query before a verified title alias is used | Confirmed. Combine the verified artist/title alternatives in one slot, preserving the canonical first query and two-query cap. Use parsed version components to avoid duplicate query labels. | Both aliases together, equivalent title spelling, manual artist/title fields, presentation suffixes, runtime labels and retained live context. |
| A metadata assessment proves a delivered source is correctly bound | Unsupported. The pure matcher compares supplied evidence; fresh source binding belongs to playback verification. | The playback-boundary regression requires the exact resolved video ID and checks missing/wrong IDs, fresh wrong artists and wrong recording versions. |
| A singular artist field proves exhaustive credits | Intentional refusal to infer completeness. A singular structured artist can agree with a singleton request while complete-credit evidence remains false. Release alternatives retain their stronger evidence requirements. | Captured singleton metadata and separately labeled synthetic complete-credit variants; release-option checks. |

The saved-request refresh path uses the shared version keys and all parsed semantic components. It still rejects a different explicit year, identifier conflicts and conflicting recording versions. No stored library metadata is automatically rewritten. Parser/evaluator revisions advance to invalidate completed matching decisions; raw query results remain reusable.

Original/album labels remain neutral alongside mastering labels. An explicit Original Mix observation alongside Live, a named remix or Dub remains contradictory. A source with such contradictory input is not accepted merely because all Flexible categories are enabled.

The reviewed existing-corpus differential covers 32 fixture pairs and 256 assessments. Its only eight changed decisions refuse the now-recognized, unbound Slowed + Reverb alternative more explicitly; the other 248 assessments are unchanged and there is no new acceptance in that existing corpus. Captured and generated positive regressions are tracked separately.

The captured YouTube metadata was obtained separately from the sanitized runtime trace. It does not identify which runtime result row was returned and does not establish media availability. Synthetic full-info variants are explicitly separated from captured evidence.

Focused checks execute the production parser/matcher code and extracted regression bodies, with only the unused database alias boundary stubbed. They do not replace native Cargo, resolver integration, actual media delivery, PCM output, UI-route or physical-device qualification. Provider outages and blocked uploads remain separate failure categories.

Bare `- Live` suffixes and ordinary hyphenated titles remain intentionally ambiguous. Compositions beyond one recording label plus one mastering label require corroborated examples and paired negatives. The two-query budget does not guarantee every provider result can be retrieved.
