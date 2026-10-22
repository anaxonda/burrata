# Bulgaria download metadata check — 2026-10-21

## Initial read-only check

The new read-only checker `scripts/qualification/download-metadata-check.py` compared the 13 Bulgaria FLACs in `bulgaria-downloads.json` against their saved library records and physical FLAC tags. It opened the library database with SQLite `mode=ro` and `query_only`; ffprobe only read the media files.

| Field | Library values available | Physical tags present | Compared values matching |
| --- | ---: | ---: | ---: |
| Artist | 13/13 | 13/13 | 13/13 |
| Title | 13/13 | 13/13 | 13/13 |
| Album | 13/13 | 13/13 | 13/13 |
| Album artist | 0/13 | 0/13 | Not comparable |
| Date | 0/13 | 0/13 | Not comparable |
| Genre | 0/13 | 0/13 | Not comparable |
| Label | 0/13 | 0/13 | Not comparable |
| ISRC | 1/13 | 1/13 | 1/1 |

All 13 files probe as FLAC, stereo, 44.1 kHz, 16-bit; their durations agree with the saved asset facts. The one ISRC present is `USNO18716501` on Pilentze Pee, matching the source-repair evidence in `README.md`. The other twelve library tracks have no ISRC value and no physical ISRC tag.

The saved asset payloads retain the acquisition provider, ID and URL, but do not retain a source-metadata snapshot. Thus the checker can establish that the optional tags are absent in these files; it cannot determine from the retained data whether album artist/date/genre/label were absent from the provider response or omitted by the tag writer. Likewise, the twelve blank ISRCs are not evidence that the source did not provide ISRCs. Reacquire or export the accepted source metadata before deciding which missing values are provider omissions versus tagging gaps.

This check confirms a narrower result than the existing “artist/title/album tags” claim: those three identity tags match on all 13 files, but the downloads are not demonstrated to have complete metadata. It did not change tags or PCM. The manifest has no PCM reference paths, so decoded-PCM equality was not evaluated here.

The surrounding UI/download gaps remain as described in `todo-audit.md`: the live Bulgaria run qualifies playlist batch acquisition and its helper wiring, while an end-to-end album-page download pass is not recorded. Real provider failure activating the configured fallback is also unqualified; the exercised Bulgaria tracks had acceptable preferred-source matches.

To rerun without changing the library or media:

```sh
python3 scripts/qualification/download-metadata-check.py \
  --manifest docs/performance-results/2026-10-21-ui-downloads/bulgaria-downloads.json \
  --db /path/to/burrata.sqlite --format text
```

For optional decoded-PCM comparison, add `pcmReferencePath` to each relevant manifest row and pass `--pcm`. The script streams decoded PCM through SHA-256 and does not write the decoded audio to disk.

## Final live recheck

The final app binary used for this check was `.cache/qualification-v4-resolver-20261021/bin/burrata`, SHA-256 `d8a1717fa7f36c043f4c95231087b68bc3394243d392b10e78a64c5d3afb2259`.

Retrying Pilentze Pee autotag returned `queued: true`. The resulting job for the current asset hash `f9bf6e961831b8b8d342645eb5e67ce6088100f6e4f5083f3be2608526ef6794` is `applied` with the message “Verified metadata is tagged from the acquisition source; no additional catalog metadata was found.” The older job for hash `0cb874542d3537c2028e6d4f562c11918e28609cd975ebb33c937c8dff23b9e4` remains in history as `no_match`. This confirms the retry creates current-hash work without rewriting the old result.

After the retry, the physical-tag/ffprobe check again found artist, title, and album matching on all 13 files. Pilentze Pee has the matching `USNO18716501` ISRC in its library record and physical FLAC tag; the other 12 have no library ISRC or physical ISRC tag. All 13 remain FLAC, stereo, 44.1 kHz, 16-bit. Album artist, date, genre, and label remain absent from library values and physical tags on all 13. Their acquisition source metadata was not retained, so availability of these optional fields from the original provider is unknown; absence does not prove a tagging omission.

The Pilentze Pee decoded-PCM SHA-256 was `d2af1f314b40ea524f5d58bbdf4c475dd6901a0b9a7dd633ce6ebd5126f352d9` both before and after the retag, confirming unchanged decoded audio for that file only.

The live catalog-album operation `662a8fc8-3f63-468f-8111-1da97b655253` finished with 13 requested, 13 skipped, zero queued jobs, and zero failed calls. This is a **failed live album-download qualification**. The sanitized MCP receipt omits per-track skip reasons, so it does not establish whether the cause was an empty provider search or match-policy rejection. The earlier playlist batch results and helper tests do not qualify this album-page route.

The compact archived evidence is [bulgaria-final-live-evidence.json](bulgaria-final-live-evidence.json). It records the binary hash, retry/current and historical job states, before/after PCM digests, 13-file metadata counts, and album-operation totals without storing full profile paths or provider credentials.
