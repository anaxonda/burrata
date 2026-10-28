# Portable playlist format, version 1

Status: frozen v1 contract, implemented in the native playlist pipeline on 2026-10-25. Version 1 uses UTF-8 M3U8, standard `#EXTM3U` / `#EXTINF`, and single-line JSON comments in the `BURRATA-*` namespace. It does not define the full backup format.

## Existing behavior and boundaries

- `playlist_sync.rs` owns managed mirrors, source bindings, immutable snapshots, atomic writes, external-edit reconciliation, and deletion pause/resume behavior. New mirrors use v1; existing legacy mirrors stay unchanged until a reviewed, hash-guarded migration. History uses the same sync pass and caller-owned `PlaybackHistory` handle.
- `playlist_files.rs` discovers local M3U/M3U8 files (2 MiB, 10,000-entry, depth 8, and 5,000-file limits). It avoids symlinks, filters generated mirrors and exports, matches only registered local assets, and retains unresolved metadata and duplicate occurrences.
- `imports.rs` preserves every M3U occurrence, including repeated tracks and EXTINF-only entries. Playlist file locations remain untrusted data.
- Ordered playlist occurrences have stable UUIDs in `playlist_ordered_entries`. `playlistHierarchy.v1` stores folders, playlist placement, sections, and occurrence annotations keyed to those IDs. The hierarchy preference does not replace ordered-entry storage.
- Playback history is a separate WAL SQLite database (`playback_history`) keyed by session ID, with UTC/RFC3339 `played_at`, provider, a track JSON snapshot, and optional chapter JSON. Listing is newest first and paged; there is no folder/section relation. Export must call the history service, not infer history from a collection.
- Portable share export, preview, file export, import preview/apply, hierarchy editing, history export, and legacy-mirror migration are exposed through narrow native commands. Portable import currently binds only unique exact local-asset matches; other entries remain unverified metadata. It does not query providers, download, favorite, or write listening history.

The format is independent of backup archive structure. Backup state uses a separate strict projection; restoring a path-stripped playlist sync projection does not activate source bindings or mirror writes.

## Wire grammar

The grammar is line-oriented. Each JSON comment contains exactly one compact JSON object encoded as UTF-8, with no literal CR/LF. JSON strings escape control characters normally. Unknown optional object members may be preserved as inert data; a reader must never execute, resolve, or interpret them as commands. Local mirror round trips retain supported-v1 extension data within the limits below. Share exports serialize only the typed public schema and omit arbitrary extension members at every level (playlist, folders, sections, tracks and entries): an unknown string cannot be assumed free of private paths, provider links or credentials. This omission is deliberate and also applies when re-sharing an imported playlist; it does not imply that a future extension has been understood.

```text
file         = header playlist-meta? entry*
header       = "#EXTM3U" LF
playlist-meta= "#BURRATA-PLAYLIST:" json LF
entry        = burrata-entry / standard-entry
burrata-entry= track-meta entry-meta extinf resource?
standard-entry= extinf resource?
track-meta   = "#BURRATA-TRACK:" json LF
entry-meta   = "#BURRATA-ENTRY:" json LF
extinf       = "#EXTINF:" integer "," display-name LF
resource     = m3u-location LF
```

For each Burrata occurrence, emit its `TRACK`, `ENTRY`, `EXTINF`, and resource in that order. A duplicate occurrence repeats all four lines, reuses the same track `id`, and uses a distinct entry `id`. A file may contain an empty playlist (header plus playlist metadata and no entries). Burrata readers require exactly one playlist record, one track and one entry record per complete occurrence, a consistent track description whenever a track ID repeats, and unique entry IDs within the file. Unknown `#BURRATA-*` records are ignored/preserved as extensions. Reject malformed known records with a line-numbered preview error; do not partially commit. Standard entries may contain an `EXTINF` with no following location; retain its metadata as an unresolved occurrence.

`#EXTINF` duration is an integer number of seconds or `-1`; use rounded nonnegative duration, otherwise `-1`. The display name is `artist - title` with ` (version)` appended when present. It is a fallback for non-Burrata readers and for legacy import. Burrata uses structured metadata as the identity description. Commas in the display name are allowed because M3U splits at the first comma. CR/LF and other C0 controls in display fields are replaced with spaces.

The Burrata records all require `schemaVersion: 1`.

### Playlist record

Required fields:

```json
{"schemaVersion":1,"id":"p1","purpose":"share","name":"Late set","description":null,"folderAncestry":[],"sections":[],"createdAt":null,"updatedAt":null}
```

- `id` is an export-scoped token matching `[A-Za-z0-9_-]{1,64}`. Do not write the sender's database UUID as this ID.
- `purpose` is `share`, `mirror`, or `history`.
- `name` is required, valid UTF-8, and at most 512 bytes. `description` is nullable, at most 4 KiB.
- `folderAncestry` is an ordered array of `{ "id": token, "name": string }`, root first; `sections` is an ordered array of `{ "id": token, "name": string }`. Arrays may be empty. IDs are local to this export. Names are not identity.
- timestamps are RFC3339 UTC strings or `null`. Shared playlist timestamps are omitted/null by default.
- `extensions`, if present, is an object limited to 16 KiB. It must not contain a credential, raw provenance blob, private filesystem path, or executable payload.

### Track record

```json
{"schemaVersion":1,"id":"t1","title":"Night Drive","artists":[{"name":"Aster Vale","role":"primary"},{"name":"Mira North","role":"featured"}],"album":"After Hours","albumArtist":null,"version":"Extended Mix","durationSeconds":245.4,"isrc":"USAAA2400001","catalogReferences":[{"namespace":"musicbrainz.recording","id":"recording-id","url":"https://musicbrainz.org/recording/recording-id"}],"bpm":{"value":124.0,"source":"manual"},"key":{"value":"8A","source":"analysis"},"tags":[]}
```

- `id` is export-scoped (`t1`, `t2`, etc.), unique per distinct serialized track description, and reused for repeated occurrences. Assign IDs by first occurrence; do not derive or expose sender IDs. Resource URLs and internal listing/asset IDs are not track identity.
- `title` and `artists` are required; artist order is significant. `album`, `albumArtist`, `version`, duration, and ISRC are nullable. Durations must be finite and positive, ISRCs normalized uppercase when valid.
- `catalogReferences` contains only public, durable, canonical catalog references with a documented namespace. It excludes private provider URLs, signed/expiring media URLs, access tokens, session credentials, and device-specific IDs. A reference is a matching hint, not proof of equivalence.
- BPM/key use `{value,source}` where `source` is `manual`, `tag`, `catalog`, or `analysis`; absent provenance means omit the measurement. Do not serialize arbitrary source provenance.
- `tags` is an array of plain strings. It is empty in share mode by default. Shared ratings/notes are excluded by default. If a later option includes them, each field must be explicit, bounded, and shown in preview.
- Full track metadata is capped at 32 KiB per record. All JSON values are inert.

### Entry record

```json
{"schemaVersion":1,"id":"e1","track":"t1","order":1,"sectionId":null,"note":null,"cuePoints":[],"playbackRange":null,"history":{"sessionId":null,"playedAt":null}}
```

- `id` is a unique export-scoped token. `track` must refer to a declared track ID. `order` is one-based and equals the physical occurrence order.
- `sectionId` references a playlist `sections[].id` or is `null`. `note`, `cuePoints`, and `playbackRange` are occurrence-level data, distinct from recording metadata. Share defaults omit notes, cue points, and playback range unless explicitly selected for inclusion.
- Cue points are finite nonnegative seconds. Playback ranges have finite `startSeconds < endSeconds`. Bounds are 16 KiB per entry.
- `history` is `null` except for history exports. History exports include an opaque export-scoped occurrence ID and, only when the user selects “include dates”, a UTC `playedAt`. Never include the local session ID. Entries are not deduplicated, even when the same recording repeats.

## Resource line and identity placeholder

The resource line is data, never a command. Burrata v1 recognizes a placeholder only when it exactly matches `burrata://track/<token>` (case-sensitive scheme and host, no query, fragment, user info, or port; token grammar above) and maps it to the corresponding `TRACK` record in the same file. It is not a network URL. A missing/unknown placeholder reference remains an unresolved entry with its metadata intact.

In **share** mode, emit a Burrata placeholder for every occurrence unless the user explicitly enables yt-dlp search links. This avoids leaking local paths, media URLs, and provider-private references. Public catalog page URLs stay in `catalogReferences` metadata unless a separately documented M3U player resolver can consume that scheme. Search links are best-effort player-specific fallback only, default off; when enabled, query full ordered artist credit, title, and version. Never append a generic `mix` suffix and never treat a search result as a verified match.

In **local mirror** mode, prefer a verified local asset, using a path relative to the mirror root where valid and otherwise an absolute path. Metadata still contains no path. If no local asset exists, use only a durable, public, supported playback URL when it is safe to share in a local mirror; otherwise use the Burrata placeholder. The selected resource is a playback hint, never identity. When import sees a path that cannot be matched to a registered local asset, keep the entry unresolved; do not open arbitrary playlist paths or import local files implicitly.

## Burrata read and conflict rules

1. Physical M3U entry order and count define occurrences. Metadata `order` mismatch is a preview warning; Burrata normalizes the order to the file order and retains each occurrence. It never deduplicates equal tracks.
2. A valid `TRACK` record is the intended identity description. `EXTINF` display text is a fallback only when no valid track record is available. If the two differ, preserve structured metadata and show the display mismatch in preview; do not silently rewrite either source file.
3. `ENTRY.track` must reference the track record attached to that physical occurrence. Invalid/missing references, duplicate occurrence IDs, or truncated groups produce a preview error and no commit. A valid unknown optional member is preserved as inert data where round-trip import/export is supported.
4. Current import preview matches a unique exact local asset by title, artist credit, version, and compatible ISRC/duration. Ambiguous or unavailable entries remain unverified. Catalog references and sender-scoped IDs are evidence only; neither overrides conflicting version/performer evidence.
5. Unavailable or ambiguous tracks remain in original order with metadata and occurrence identity. No import automatically downloads, favorites, records listening history, or executes links. Imported history defaults to a normal playlist.
6. Current import preview creates a new playlist after review. Export-scoped IDs never bind to a recipient's existing playlist; update and merge policies are not enabled by this v1 UI.
7. Malformed JSON, invalid encoding, oversized records, too many records, or a future unsupported `schemaVersion` fail preview with a clear error. Never partially apply. Unknown optional members from supported v1 may be retained, subject to size limits.

## Share privacy and local mirror compatibility

Share mode excludes filesystem paths/roots, asset IDs, database IDs, private provider URLs, cached or signed media URLs, credentials, history dates/session IDs, personal notes, ratings, tags, and arbitrary provenance by default. User opt-ins must name each personal-data class and be reflected in preview. No exporter should serialize `Track.provenance` wholesale.

Existing `#BURRATA-MIRROR`, `#PLAYLIST:<text>`, ordinary EXTINF M3U, and prior generated files remain readable. Readers treat old location lines as paths/playback hints under existing safety rules. A file without Burrata records follows existing import behavior. `#BURRATA-MIRROR` remains an ownership marker for managed mirror discovery; it is orthogonal to the v1 metadata and may precede `#PLAYLIST` for legacy output. `#PLAYLIST` remains the human-readable fallback name.

Migration is lazy and non-destructive:

- Do not eagerly rewrite existing mirror files, rename their directories, or convert their content simply because the app starts with v1 support.
- Preserve the `mirror_hashes`, `mirror_paths`, `mirror_snapshots`, source snapshots, protected copies, source bindings, and paused-deletion state. The current on-disk SHA-256 baseline is authoritative for external-edit detection until the user explicitly resumes a deleted mirror or elects to migrate/export it.
- Keep external edit parsing/reconciliation and snapshot archival in the existing `playlist_sync.rs` flow. Do not add another watcher or sync loop. Before updating an old mirror as part of an already-triggered content write, compare its bytes to the saved hash. If the user changed it, reconcile/archive under current rules; if reconciliation is ambiguous, preserve the edited file and report a conflict. If it is unchanged, archive the prior complete bytes and atomically publish v1.
- Deleted mirrors remain paused until explicit resume. Missing or unavailable destinations are not deletion evidence. A resumed mirror can be written in v1 after preview/confirmation by the caller.
- Keep old `#PLAYLIST` and EXTINF semantics available for external players. V1 JSON comments are ignored by ordinary M3U readers.
- Do not silently change existing yt-dlp behavior for all exports. New share exports default search off. Any legacy mirror policy conversion is an explicit migration setting/preview; no unconditional `mix` suffix is carried into the v1 search query.

## Native API boundaries

The native commands expose preview/render/export for collections and history, hash-guarded import preview/apply, hierarchy read/write, mirror migration preview/apply, and explicit mirror resume. The serialization/parser module is side-effect free; sync and storage own persistence and reconciliation.

```rust
enum PlaylistExportPurpose { Share, LocalMirror, History }
struct PlaylistExportOptions {
    purpose: PlaylistExportPurpose,
    include_ytdlp_search: bool,
    include_notes: bool,
    include_tags: bool,
    include_ratings: bool,
    include_history_dates: bool,
}
struct PlaylistExportPreview {
    entry_count: usize,
    local_entries: usize,
    identity_placeholders: usize,
    search_links: usize,
    unresolved_entries: usize,
    excluded_private_fields: Vec<String>,
    destination: Option<String>,
    warnings: Vec<String>,
}
fn preview_playlist_export(id: &str, options: PlaylistExportOptions) -> Result<PlaylistExportPreview>;
fn render_playlist_export(id: &str, options: PlaylistExportOptions) -> Result<String>;
fn preview_playlist_import(input: PlaylistInput, destination: PlaylistDestination,
                           match_level: MatchLevel) -> Result<PlaylistImportPreview>;
fn apply_playlist_import(preview_id: &str, policy: ImportPolicy) -> Result<PlaylistImportResult>;
```

The renderer/parser should be a small shared format module. `playlist_sync.rs` retains mirror state, source reconciliation, archive snapshots, atomic writes, external edits, deletion pause/resume, and invokes the renderer. `playlist_files.rs` retains safe discovery and local asset lookup, invoking the common parser for preview/import. `imports.rs` should keep generic CSV/tracklist support and expose/consume parsed M3U track descriptions without deduplicating occurrences. `playback_history.rs` owns history retrieval and the history export input. `storage.rs` owns any future folder/section/occurrence schema and transactional persistence. `lib.rs` only registers narrow commands once contracts are approved. Frontend preview/results own user choices and display, not serialization or privacy filtering.

Preview output must disclose the purpose, output mode, counts by resource type, matching uncertainty, history/date behavior, excluded personal fields, and any legacy-mirror migration. Apply must be based on the reviewed immutable preview or reject if its source revision changed.

## Round-trip fixtures

### Portable share, duplicate track, section, non-Latin text

```m3u
#EXTM3U
#BURRATA-PLAYLIST:{"schemaVersion":1,"id":"p1","purpose":"share","name":"夜の道","description":null,"folderAncestry":[{"id":"f1","name":"Sets"}],"sections":[{"id":"s1","name":"Warm up"}],"createdAt":null,"updatedAt":null}
#BURRATA-TRACK:{"schemaVersion":1,"id":"t1","title":"Nuit d'été","artists":[{"name":"Élodie","role":"primary"}],"album":"Échos","albumArtist":null,"version":"Extended Mix","durationSeconds":245.4,"isrc":null,"catalogReferences":[],"bpm":{"value":124.0,"source":"manual"},"key":null,"tags":[]}
#BURRATA-ENTRY:{"schemaVersion":1,"id":"e1","track":"t1","order":1,"sectionId":"s1","note":null,"cuePoints":[],"playbackRange":null,"history":null}
#EXTINF:245,Élodie - Nuit d'été (Extended Mix)
burrata://track/t1
#BURRATA-TRACK:{"schemaVersion":1,"id":"t1","title":"Nuit d'été","artists":[{"name":"Élodie","role":"primary"}],"album":"Échos","albumArtist":null,"version":"Extended Mix","durationSeconds":245.4,"isrc":null,"catalogReferences":[],"bpm":{"value":124.0,"source":"manual"},"key":null,"tags":[]}
#BURRATA-ENTRY:{"schemaVersion":1,"id":"e2","track":"t1","order":2,"sectionId":"s1","note":null,"cuePoints":[],"playbackRange":null,"history":null}
#EXTINF:245,Élodie - Nuit d'été (Extended Mix)
burrata://track/t1
```

Expected read: playlist and folder/section names retained; two ordered entry occurrences (`e1`, `e2`) reference one track description (`t1`); neither is matched by sender ID; no date, local path, media URL, or personal note leaks.

### Ordinary EXTINF-only text remains importable

```m3u
#EXTM3U
#EXTINF:180,Artist - Song (Club Mix)
https://example.invalid/opaque
```

Expected read: one unverified `Artist` / `Song` / `Club Mix` occurrence; the location is not executed or trusted as identity. If the input contains only an EXTINF with no location, preserve it as an entry with no playable resource. Generic import APIs may still return a track list, but playlist import must not drop that occurrence.

### Existing managed mirror remains compatible

```m3u
#EXTM3U
#BURRATA-MIRROR
#PLAYLIST:Legacy Mixxx export
#EXTINF:180,Artist - Song
../Artist/Song.flac
```

Expected read: old name/location rules and current mirror snapshots remain in force; no v1 metadata is required until an explicit migration/write publishes it.
