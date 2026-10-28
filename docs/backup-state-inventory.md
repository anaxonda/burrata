# Backup state inventory and v1 contract

Date: 2026-10-25. Status: inventory and v1 implementation contract. The backup state projection is implemented in `src-tauri/src/backup_state.rs`; archive/apply orchestration remains coordinated separately.

This document records the durable state found while designing library backup and restore. Paths and preference names below are grounded in the current source. It proposes a narrow archive boundary for the first implementation; it does not authorize copying the live profile or importing arbitrary database/configuration content.

## State inventory

### Native application data directory

The Tauri setup uses `BURRATA_APP_DATA` when set, otherwise Tauri's app data directory. That directory currently contains:

| State | Current location/owner | Backup treatment |
| --- | --- | --- |
| Main library and application preferences | `burrata.sqlite`, `Store` in `storage.rs` | Include a consistent SQLite snapshot, then sanitize/classify rows before publication. |
| Listening history | `playback-history.sqlite`, `PlaybackHistory` | Include a separate SQLite snapshot in the same archive revision. It is not in the main DB. |
| Cached artwork | `artwork/` | Exclude as regenerable cache by default. User-supplied artwork requires a separately identified ownership/reference path before it can be included safely. |
| Browser bridge discovery/configuration | `browser-bridge.json` | Exclude; it contains machine-local integration/runtime information. Recreate on this device. |
| Managed provider browser/session files | `managed-provider-session/` | Exclude session authorization, cookies, browser profiles and worker state. Sign in again after restore. |
| YouTube helper/runtime data | `youtube-support/` | Exclude installed helpers, temporary state, and runtime cache. |
| Automation configuration | `automation.json` | Do not archive wholesale. It can contain local endpoint/configuration data; inventory any user-authored, portable automation separately before adding a versioned allowlist. |
| Temporary acquisition/publication/verification files | task-specific staging/state files | Exclude. Restore must never resume or publish a pending job automatically. |

The main DB is opened with SQLite WAL mode and a process-wide `Store` mutex. A filesystem copy of `burrata.sqlite` alone can omit committed WAL pages. `playback-history.sqlite` is separately opened in WAL mode and has its own mutex. Both therefore require SQLite's online backup API (or a coordinated read transaction/copy mechanism with equivalent guarantees), not raw file copies.

### Main database contents

`schema.sql` is only the initial schema. `Store::open` migrates through `user_version=11`. The resulting schema includes:

| Tables / contents | Classification |
| --- | --- |
| `works`, `recordings`, `catalog_listings` | Library identities, durable recording corrections and catalog association. Include. Listing payloads and source URLs must be sanitized to remove expiring/private media URLs and credentials. |
| `collections`, `collection_members`, `playlist_ordered_collections`, `playlist_ordered_entries`, `browser_sources`, `browser_imports` | User collections, order, source binding/import definition. Include durable hierarchy/order and reviewed imports; strip transient browser tokens. Folder/section links may also live in JSON preferences and need component-aware serialization. |
| `local_assets` | Asset identity/hash/path and file metadata. Include records and content hash; treat absolute paths as device-specific locators to remap. Do not include audio bytes unless the user explicitly enables media inclusion. |
| `asset_analyses`, `asset_autotags` | Analysis/tagging results tied to specific asset hashes and versions. Include only records whose `file_hash` matches the registered asset hash; exclude stale analyses, progress and path-bearing values. This covers the current BPM/key/audio analysis rows and future resource-bound loudness/waveform rows under the same asset/hash/version identity. |
| `download_jobs` | Persisted acquisition job intent/status. Do not restart these jobs on restore. If retained, restore only as paused, reviewable records with paths, private URLs, and transient task state removed. Safer v1 default: omit and report count. |
| `settings` | Contains the application settings JSON plus many namespaced JSON preferences, including library/UI preferences, queue/session checkpoints, playlist sync metadata, source definitions and provider connection settings. Never export the table wholesale. Serialize an allowlist by key/type and sanitize nested values. |
| `chart_snapshots`, `chart_entries`, `provider_cache`, `preview_sources`, `acquisition_candidates`, `artist_mappings`, `verified_playback_sources` | Cached/provider-derived or expiring evidence. Exclude in v1 and recompute. In particular `verified_playback_sources` carries provider URLs and expiring timestamps; cached matching evidence is not user-authored truth. |

`recordings.favorite` and some collection membership are overlapping representations. Restore must preserve the current invariant by using existing storage APIs/migrations or a validated import transaction, not by applying selected table rows independently. `manual_metadata` is JSON and may contain correction provenance; preserve known user fields but do not blindly retain arbitrary blobs.

The generic `settings` table also has user-created records outside the top-level `Settings` struct. The typed allowlist covers `playlistPreferences.v1`, `playlistHierarchy.v1`, `playlistSync.v1` (projected to a backup-only inert schema), `sidebarLayout.v1`, `radioStations.v1`, `hiddenRadioStations.v1`, `radioStationNames.v1`, `radioHistory.v1`, `includeRadioSongsInPlaybackHistory.v1`, `autotag.settings`, `radioRecognitionEnabled.v1`, `loudness.v1`, `backupSchedule.v1`, `playingTrackDetectionCache.v1`, and `playingTrackDetectionPolicy.v1`. Full private backups retain bounded detected-track observations keyed by an opaque active-resource hash; the separate per-resource opt-in policy is restored disabled so importing a backup never starts recognition. Detected-track observations and policy are private library state and are omitted from portable playlist shares. Full private backups also preserve explicit remembered LRCLIB choices under a well-formed lowercase 64-hex scope key: the canonical candidate ID, bounded metadata, and bounded lyric body are retained so the remembered selection and its source hash remain usable. Lyric bodies remain opaque text, including URL-looking lines; the body is never interpreted as a source URL. Candidate URLs are reconstructed from the numeric LRCLIB ID; arbitrary source URL fields, headers, credentials, and other provider state are excluded. Standalone settings exports omit these lyric-choice bodies and report them as omitted private settings. Frontend preferences are independently allowlisted and validated; the portable lyric-timing preference remains keyed to the same provider/track/source hash, so timing adjustments continue to match the restored body. `radioHistory.v1` preserves bounded per-station artist/title/album/year/time metadata, but excludes stream and artwork URLs. Playlist mirror source/mirror/protected-copy paths are reduced to basename-only relink hints; snapshots are reserialized through the share-safe playlist serializer, retain their original source SHA-256 as a labeled historical identifier, and include a separate hash for sanitized contents. They sit under `backupProjection` with empty runtime path maps, so restore cannot activate the source or overwrite files. Restored schedules retain interval/retention preferences but are disabled with no destination. Radio stream URL preferences, directory roots, download destinations, provider connections, lyric-choice arbitrary source URL fields, remote account IDs, and raw remote state remain excluded.

### History and playback checkpoint

`playback-history.sqlite` has one `playback_history` row per session (`session_id`, timestamp, provider, serialized track, optional chapter). It preserves repeated plays. Include history as private personal data; do not turn it into a playlist during full backup or merge history rows with deduplication by recording. The separate `radioHistory.v1` preference is also personal listening history and is included through the bounded metadata projection above.

Queue/session checkpoints, selected playback engine and related preferences are stored in `settings` (`nativePlaybackQueue.v1`, `nativePlaybackSession.v1`, `playbackEngine.v1`). The WebView also persists a queue snapshot and resume cursor in localStorage (`burrata.playback-queue.v1`, `burrata.playback-position.v1`). Treat these as one logical resumable playback component: prefer the latest internally consistent native checkpoint; serialize one canonical queue/session snapshot and restore it paused. Do not export engine-switch transient controls as authoritative user preference.

### Frontend localStorage

The observed keys fall into three classes:

| Keys | Treatment |
| --- | --- |
| `burrata.playback-queue.v1`, `burrata.playback-position.v1` | Include the canonicalized queue and paused resume cursor under the playback component; validate track references and finite/bounded position. |
| `burrata.discovery.search-scope.v2`, `burrata.discovery.sources.v1`, `burrata.discovery.continue-search.v1`, `burrata.queue-end-radio.v1`, `burrata.waveform-enabled.v1`, `burrata.layout.sidebar.v1`, `burrata.layout.info.v1`, `burrata.lyrics-timing.v1` | Portable UI/search preferences. Include only through explicit versioned keys and type validation; layout may be device-specific and should be optional. |
| `burrata.lastfmUsername` | Account identifier. Exclude from generic settings export unless account identifiers are an explicit separately reviewed option. It is not a credential, but can identify a user. |
| `burrata.discovery.search-history.v1`, `burrata.soulseek.wishlist.v1` | Full private backup includes typed, bounded search history and wishlist entries. Restored wishlist entries are disabled until reviewed so restore cannot start searches. A separate settings-only export may omit query history. |
| `burrata.engine-switch-controls.v1`, `burrata.native-playback-migrated.v1` | Transient engine handoff/migration state. Exclude. |
| `burrata.backup-frontend-preference-receipt.v1` | Local restore-delivery receipt (pending nonce and writer process-session ID). Exclude from backups/shares; it is not a user preference. |
| `burrata.backup-playback-restore-receipt.v1` | Local WebView delivery receipt for the selected playback handoff (pending nonce and writer process-session ID). Exclude from backups/shares; native queue/session rows remain the playback component’s durable state. |
| `sessionStorage` diagnostics toggle and all in-memory frontend state | Session/runtime state. Exclude. |

Search history and the Soulseek wishlist remain private query data. Full backups retain them through the typed frontend allowlist; unknown browser storage keys stay excluded, even if their name starts with `burrata.`.

### Imported frontend preference handoff

`backupFrontendPreferences.v1` is an internal pending-delivery envelope: schema version, fresh nonce, and allowlisted imported preferences. It is excluded as an internal settings row; portable preference values are backed up through the explicit frontend projection instead. Imports/restores create a fresh handoff, and older bare maps migrate using a compare-and-swap. A new process-session ID is minted in memory at startup. The WebView writes its local receipt after applying the values; the handoff is acknowledged only when that receipt is observed in a later process session. Same-session retries do not replay values over later user edits, and nonce comparison prevents an old acknowledgment from clearing a newer import. The local receipt is disposable and is never exported/restored. Runtime qualification of this protocol is separate from the previously observed candidate47 silent-storage failure.

`backupPlaybackHydration.v1` remains a version-1 native pending handoff. Its read command adds the current in-memory process-session ID without persisting it. In WebView mode, the frontend writes the restored queue, position, and controls, then writes a separate local receipt. A same-process acknowledgment is deferred; a later process acknowledges a matching nonce without replaying the imported values, preserving later user changes. Missing receipts leave the handoff available for retry. Native-engine restores use the staged SQLite queue/session directly and do not need this frontend handoff. The playback receipt is excluded from backup and portable preference projections. These source changes still require runtime qualification against silent WebView storage failures.

### Restored section mirror review

The restored playlist-sync preview may include `sectionMirrorRelinks`, listing a playlist name, section name, and relative path for each exact-content-verified section mirror already present under the current mirror folder. The review UI shows these paths before the existing resume acknowledgment. Only listed section mirrors are rebound; other candidate files remain unbound and protected. This describes the source contract and does not by itself establish runtime qualification.

### Detected tracks and observed waveform retention

`playingTrackDetectionCache.v1` stores bounded, resource-scoped recognition observations with approximate source timestamps. Full private backups include its validated metadata; portable playlist exports omit it. `playingTrackDetectionPolicy.v1` is separate: restored detection policies are disabled, so restoring a backup cannot start sending playback samples for recognition.

`burrata.observed-waveform-cache.v1` is a bounded, regenerable local browser cache of partial amplitude summaries, excluded from the backup frontend allowlist. It stores opaque resource digests and known bins, with unknown regions left empty; it does not contain PCM audio. Durable reuse requires verified local content or a stable stream representation. Unknown stream encodings remain session-only. Restoring a library does not imply restoring this cache or complete waveform coverage.

### Credentials and local media

Provider secrets are held in the operating-system keyring by `connections.rs`; session-only credentials are held in memory. Neither belongs in an archive. The `connections` preference record contains non-secret toggles/configuration and a persisted list of session-only suppression markers. Its fields still require allowlisting: do not export account IDs, private endpoints, cookies, or credentials; restore provider enabled-state only where it is safe and let the user sign in again.

Local audio files are user-owned data outside app data. A metadata-only backup includes asset identity, hash, and original path as a remapping hint only if the privacy UI discloses that absolute paths are included. Preferred v1 behavior is to store relative-to-root paths where possible plus root labels, and keep original absolute roots in an optional local mapping section. Media inclusion must copy only verified registered assets, estimate size first, stream to archive staging, and report each missing/changed asset. Never overwrite audio during metadata replacement.

## Proposed archive/settings contract

### Full archive: `.burrata-backup`

ZIP container, format identifier `org.burrata.backup`, `schemaVersion: 1`. Keep the manifest first and small; reject duplicate paths, absolute paths, `..`, symlinks, unsupported compression methods, oversized entries, excessive total uncompressed size, and unsupported future schema versions before extraction.

```json
{
  "format": "org.burrata.backup",
  "schemaVersion": 1,
  "appVersion": "<version>",
  "createdAt": "<RFC3339 UTC>",
  "snapshotId": "<UUID>",
  "components": [
    {"id":"library","path":"data/library.sqlite","encoding":"sqlite","bytes":0,"sha256":"..."},
    {"id":"history","path":"data/playback-history.sqlite","encoding":"sqlite","bytes":0,"sha256":"..."},
    {"id":"settings","path":"data/settings.json","encoding":"json","schemaVersion":1,"bytes":0,"sha256":"..."}
  ],
  "media": {"included":false,"assets":0,"missing":0,"changed":0}
}
```

The manifest is not a trust signature. Checksums detect corruption only. Each component is independently versioned and size/hash checked. The canonical `library.sqlite` snapshot must have volatile/cache-only tables purged from the staged copy (or be projected to a backup DB schema); do not mutate the live DB. Document whether component hashes cover compressed or uncompressed bytes; recommended: uncompressed component bytes.

For a portable library state, first implementation may use a sanitized SQLite snapshot plus versioned JSON preferences. Keep schemas/migrations explicit. Preserve SQLite schema/user_version in the component; future archive migration code must open a disposable staged DB and migrate there before any live write.

### Separate settings JSON

`burrata-settings.json`, JSON object `{format:"org.burrata.settings", schemaVersion:1, createdAt, application, preferences, frontendPreferences, assetPathHints}`. Build from named allowlisted fields and preferences, not `Settings` serialization as an unrestricted dump and never `SELECT * FROM settings` into the file. The allowlist covers safe playback/matching/download preferences, playlist hierarchy/shortcuts, paused mirror snapshot projections, safe radio definitions and radio history metadata, loudness policy, frontend search scope/history, wishlist, queue-end behavior, panel layout and lyric timing. Restore wishlists disabled. Exclude filesystem executable paths, `musicDirectory`/playlist roots, account identifiers, credentials, authorization state, saved raw URLs with embedded auth/query secrets, diagnostic payloads and task/job state. Device-specific outputs remain excluded. Only the typed `frontendPreferences` map may be handed back to localStorage.

`playback.json` uses `org.burrata.playback`, `schemaVersion:1`, queue track identity/metadata, occurrence IDs, current index/entry, queue revision, paused position, repeat/shuffle/volume, and `restorePaused:true`. It may preserve `queueLoudnessContexts` only when a binding names an entry in the queue and the typed album resource identity recomputes to its declared `resourceKey`; malformed resources and orphan entry bindings are dropped. It omits source URLs, provider provenance, paths, engine handoff and runtime diagnostics. Frontend capture supplies the current localStorage queue/session to the backup command because native code cannot inspect WebView localStorage.

### Proposed Rust-facing types and commands (names provisional)

```rust
struct BackupCreateOptions {
    include_media: bool, // false by default
    include_history: bool, // true for full backup; false only for settings-only export
}
struct BackupInspection {
    schema_version: u32,
    created_at: String,
    components: Vec<BackupComponentSummary>,
    media: MediaSummary,
    warnings: Vec<String>,
}
enum RestoreMode { Merge, Replace }
struct RestoreOptions {
    mode: RestoreMode,
    components: Vec<String>,
    path_mappings: Vec<PathMapping>,
    settings_policy: SettingsRestorePolicy,
}
```

Keep the first API narrow: `inspect_backup(path) -> BackupInspection`, `create_backup(options, destination) -> BackupJob`, `restore_backup(path, options) -> RestoreJob`, plus progress/status/cancel by opaque job ID. Inspection is read-only and completes validation/checksum/limits before restore is enabled. `RestoreOptions` must not expose arbitrary SQL, table names, filesystem paths inside the archive, or executable task directives. Settings import should have a separate typed allowlist path and preview.

## Restore transaction and quiescing proposal

There is no current cross-store transaction boundary: main DB and history each have their own mutex, while playback, analysis, imports, playlist synchronization, download publication and frontend writes can continue. Replacing the DB file beneath open connections is unsafe. A complete restore therefore needs a process-wide backup/restore coordinator before writing commands/UI.

1. **Inspect and stage:** validate ZIP structure and archive bounds; verify all component hashes; migrate copies in a private staging directory; validate SQLite integrity, foreign keys, supported schema versions, row/count limits, JSON bounds and component references. Resolve media mappings in preview only. No live profile changes.
2. **Recovery point:** obtain a coordinated SQLite online backup of both live DBs and copy the allowlisted local preferences/checkpoints into a recovery archive. Verify this recovery archive before continuing. Keep it until the restore and first reopen succeed.
3. **Quiesce:** coordinator blocks new mutating commands and requests safe cancellation/pause from playback checkpoint writes, history recording, library imports, playlist watchers/sync, analysis, and acquisition/publication. Drain in-flight operations with a deadline and report the blocker instead of racing. Flush checkpoint/history writes and take the final consistent snapshots only after this drain.
4. **Apply:** for merge, apply validated component changes through one transaction per DB with stable IDs and explicit per-component conflict policy; history uses session ID idempotence. For replace, prepare new DB files from staged+validated content, close all DB-owning services under coordinator control, atomically swap the files while the app is at a controlled restart/reopen boundary, then reopen and validate. Avoid a claim that two filesystem renames are atomic together: write a recovery journal with prior/next filenames and startup recovery logic, or use a backup DB plus fully transactional row replacement across stores while connections are held closed.
5. **Commit/reopen:** verify schema/integrity and cross-component references; reopen stores and resume normal app operation with queue/jobs paused. Only then mark success and release the recovery snapshot according to retention policy.
6. **Rollback:** on error/interruption, use the journal to restore the previous verified DB pair and preferences, reopen the old stores, and retain recovery artifacts for diagnosis. Startup must detect an incomplete journal and restore the last complete pair before opening ordinary services. Never delete the only known-good copy during rollback.

The current architecture makes this a necessary boundary: `Store` and `PlaybackHistory` hide their SQLite connections behind independent mutexes and services own cloned `Arc`s. A coordinator should be added around mutation entry points/services before replace mode is offered. A narrower initial delivery can ship consistent backup plus inspect and merge-only restore; do not expose replace mode until all writers can be quiesced and restart recovery is implemented.

## Explicit exclusions and limits for v1

- Passwords, API tokens, cookies, browser profiles, keyring data, session-only credentials, media keys and authorization state.
- Expiring media/stream URLs, process-local tokens, provider cache and disposable artwork/metadata caches.
- Temporary files, live in-flight jobs, active playback engine state, pending acquisitions/publications, browser bridge tasks and OS integration state.
- Audio bytes unless `include_media` was selected and each asset was verified and reported. Metadata-only archives cannot restore missing bytes.
- Arbitrary frontend storage keys, raw `settings` rows, diagnostic dumps, raw provenance JSON or configuration objects without field-level sanitization.
- Automatic restart of downloads, purchases, provider writeback, scans or playback after restore.

The archive is private user data even without credentials. v1 is not encrypted; UI and docs must say so plainly. A successful checksum does not establish that a backup is trustworthy or safe.

## Safest implementation boundary

Start with read-only state inventory/serialization contracts and isolated fixtures. Next add a coordinator and online snapshot APIs that copy both DBs without touching the live profile. Add inspect and metadata-only archive creation before restore. Implement merge-only restore using staging and explicit conflict rules; gate replacement on proven app-wide writer quiescence, journaled DB-pair swap/recovery, and tests using disposable profiles. Do not fold provider-cache restoration or credentials into the format. Do not update shared Rust settings/models/commands until the backup component list and these contracts are agreed across the concurrent playlist and UI inventory work.
