# Caption parsing regression matrix — 2026-10-21

This matrix describes the current shared recording evaluator, not a guarantee about arbitrary uploads or macOS audio output. Native parser changes passed the reported full run (**1,520 passed; 37 ignored**). No additional build or network research was performed for this document.

Implementation: [recording_match.rs](../../../src-tauri/src/recording_match.rs), [recording_identity.rs](../../../src-tauri/src/recording_identity.rs). Test names below are searchable in those files.

## Covered behavior

Caption acceptance rows below describe **Balanced** unless another level is stated. Flexible has separate alternative-recording rules; these rows do not assert identical decisions across all three policy levels.

| Input or failure mode | Current protection | Regression |
| --- | --- | --- |
| Literal **Vibe With Me**, Come With Me, Dancing With Myself | Bare `with` does not invent a guest named “Me” or “Myself.” Explicit `Song - with Guest` and bracketed feature credits still work. | `ordinary_with_in_a_title_does_not_invent_a_featured_performer` |
| Actual `Nipsey Hussle x Bino Rideaux - Vibe With Me (feat. Leon Thomas) (Official Audio)` | Known trailing presentation labels are peeled before title/guest comparison. Prefix performers plus the explicit guest must cover the complete requested performer set. Search returns **Inspect**; fresh resolved evidence can return **Accept / Probable** at Balanced. Exact does not accept missing recording anchors. | `captured_vibe_official_caption_has_complete_credits_at_search_and_resolution` |
| Collaboration separators `&`, `and`, `x`, `X`, `×`, commas; bracketed `featuring` | Separators in the artist prefix are interpreted only against complete requested credits. Exact whole-act spelling is checked first. Artist aliases are not guessed from text. | `explicit_feature_and_collaboration_punctuation_is_equivalent_without_alias_guessing` |
| Missing/wrong guest, an extra artist, or a substring such as a tribute-act name | The union must equal the requested normalized performer set; partial overlap cannot complete the caption. | `incomplete_wrong_extra_or_substring_credits_cannot_complete_a_caption` |
| Live, Instrumental, Remix or 432Hz around feature/presentation labels | Presentation cleanup does not erase these distinctions or authorize an unrequested version. | `presentation_stripping_does_not_hide_semantic_version_conflicts`; `unrequested_live_mix_is_rejected_and_caption_prefix_is_not_uploader_evidence` |
| Punctuation inside a single act: Earth, Wind & Fire | A complete structured single-act credit remains one identity. This is distinct from interpreting collaboration punctuation in a caption. | `comma_in_single_band_name_is_not_split_into_people` |
| Geographic/work parentheses and explicit `w/` | Existing tests preserve Turtledove (Epiros), Symphony No. 5 (II. Andante), and Ripple (Para Nana); explicit bracketed `w/ Ry Cooder` remains a feature annotation. | `reported_titles_preserve_guests_versions_and_work_names` |
| Null credits, unknown artist evidence, or conflicting identifiers | Null artist arrays can use independent complete caption evidence; missing resolved artist evidence requests enrichment; contradictory ISRC/mix/artist evidence remains a blocker. | `null_artist_arrays_allow_independent_caption_prefix_evidence`; `unknown_artist_evidence_at_resolution_requests_enrichment`; `wrong_isrc_and_mix_and_artist_are_hard_conflicts` |

## Concrete remaining coverage gaps

These are bounded future regression targets, **not verified catalogue identities or requests to broaden acceptance**.

| Gap | What remains uncertain / safe follow-up |
| --- | --- |
| Literal `ft.`, `feat.`, `featuring`, or `w/` inside a title | Only bare `with` was corrected. For example, synthetic `12 ft. Tall` could be misread as a guest annotation. Add lexical-preservation cases with explicit source-title evidence; do not assume every occurrence means a performer. |
| Literal `and` / `x` in titles or act names | The new collaboration test covers artist-prefix separators, not ordinary title text or a single act containing `x`. Add explicit title-preservation and whole-act cases before extending splitting. |
| More than four nested labels, malformed brackets, or several feature groups | New cleanup loops are bounded to four passes. Current tests cover the reported two-label layout and semantic-version orderings, not every deeper/malformed arrangement. Retain uncertainty rather than stripping arbitrary text. |
| Localized feature/presentation words | `con`, `avec`, `mit`, and translated “official audio” labels are not general identity rules. Unsupported wording may cause false negatives; recognition needs explicit, tested syntax and authority-backed names. |
| Spelling/transliteration and roman-number name suffixes | The parser does not equate Leon Thomas with Leon Thomas III by itself. Alias evidence must come through the existing verified context; no general suffix deletion or cross-product alias guessing is justified. |
| Malformed non-null credits plus an otherwise complete caption | The null-array regression is not exhaustive coverage of malformed objects, inconsistent counts, or mixed valid/invalid credits. Add table cases for each shape; do not claim complete metadata merely because some rows parsed. |
| A misleading upload carrying correct-looking credits | Caption matching is metadata evidence, not an audio fingerprint. A copied title, “Official Audio” suffix, or audible five-second excerpt cannot establish that the entire file is the intended recording. Keep identity checks separate from transport and content-window qualification. |

## Grounded live evidence and limits

The captured official Vibe search candidate was `youtube:ExdT24gJngk`, flat duration **232s**; fresh extraction reported **231s**. After the fix, one isolated cold request resolved in **8.56s**, with first PCM at **8.69s**, and retained complete Discogs credit evidence. The fresh caption carried all three requested performers; no saved duration or ISRC was invented.

The production harness decoded the initial five seconds. A separate **115s** window contained substantial audio; direct window requests at **0s and 30s returned HTTP 403 before decoding**. Those failures were retained without retries and are not evidence of encoded silence. This is Linux qualification, not reproduction of the exact macOS library/profile or proof of the complete media timeline.

Local artifacts: `.cache/playback-v015-mac-20261021/vibe-official-search-candidate.json`, `vibe-post-caption-resolver.json`, `vibe-post-caption-phases.json`, and `vibe-window-summary.json`. Raw signed media URLs and credentials are intentionally omitted here.
