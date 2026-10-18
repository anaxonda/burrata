<script lang="ts">
  import { playlistWriteCapabilities, hasPlaylistWriteCapability, playlistWritePlanAllowed, type PlaylistWriteCapabilities } from "./lib/remote-playlist-capabilities";
  import PlaylistWritebackMatch from "./PlaylistWritebackMatch.svelte";
  type MoveChange = { entryIds: string[]; beforeEntryId?: string | null };
  type SyncPreview = {
    previewId: string; bindingId: string; provider: string; playlistId: string; localCollectionId: string;
    baselineRevision?: string | null; remoteRevision?: string | null; conflict?: boolean;
    applied?: boolean; canEdit?: boolean; writeCapabilities?: PlaylistWriteCapabilities; writeSyncEnabled?: boolean;
    accountId?: string | null;
    changes: {
      add: { trackUri: string; entryId?: string }[];
      remove: { entryIds: string[] }[];
      move: MoveChange[];
      unresolved?: { entryId?: string; localTrackId?: string; title?: string; artist?: string; reason?: string }[];
    };
    summary?: string; error?: string;
  };

  let { bindingId, enabled, call, onApplied = () => {} }: {
    bindingId: string;
    enabled: boolean;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onApplied?: () => void | Promise<void>;
  } = $props();

  let loading = $state(false);
  let applying = $state(false);
  let preview = $state<SyncPreview | null>(null);
  let error = $state("");
  let notice = $state("");
  let requestVersion = 0;
  let reviewedMatches = $state<Record<string,string>>({});
  let reviewedLabels = $state<Record<string,string>>({});

  $effect(() => {
    bindingId; enabled;
    requestVersion += 1;
    loading = false; applying = false; preview = null; error = ""; notice = ""; reviewedMatches = {}; reviewedLabels = {};
  });

  async function sync() {
    if (!enabled || !bindingId) return;
    const requestedBindingId = bindingId;
    const request = ++requestVersion;
    loading = true; error = ""; notice = ""; preview = null;
    try {
      const matches = Object.entries(reviewedMatches).map(([localTrackId,candidateToken]) => ({localTrackId,candidateToken}));
      const result = await call<SyncPreview>("get_remote_playlist_sync_preview", { bindingId: requestedBindingId, ...(matches.length ? {reviewedMatches:matches} : {}) });
      if (request !== requestVersion || requestedBindingId !== bindingId || !enabled) return;
      if (result.bindingId !== requestedBindingId) { error = "The service returned a preview for a different playlist."; return; }
      preview = result;
      if (preview.error) error = preview.error;
    } catch (failure) { if (request === requestVersion && requestedBindingId === bindingId) error = `Could not prepare sync preview: ${String(failure)}`; }
    finally { if (request === requestVersion) loading = false; }
  }

  function selectMatch(localTrackId: string, candidateToken: string, label: string) {
    if (loading || applying) return;
    reviewedMatches = {...reviewedMatches,[localTrackId]:candidateToken};
    reviewedLabels = {...reviewedLabels,[localTrackId]:label};
    void sync();
  }

  async function apply() {
    const current = preview;
    if (!enabled || !current || !canApply()) return;
    const requestedBindingId = bindingId;
    const request = ++requestVersion;
    applying = true; error = ""; notice = "";
    try {
      const result = await call<SyncPreview>("apply_remote_playlist_sync_preview", { previewId: current.previewId });
      if (request !== requestVersion || requestedBindingId !== bindingId || !enabled) return;
      if (result.bindingId && result.bindingId !== requestedBindingId) { error = "The service returned a result for a different playlist."; return; }
      if (result.conflict) { preview = result; error = "This preview is stale. Refresh it before applying."; }
      else if (result.error) { error = result.error; }
      else {
        const syncedProvider = current.provider === "youtube" ? "YouTube" : "Spotify";
        preview = null;
        reviewedMatches = {}; reviewedLabels = {};
        notice = result.summary || `${syncedProvider} playlist synced.`;
        await onApplied();
      }
    } catch (failure) { if (request === requestVersion && requestedBindingId === bindingId) error = `Could not apply sync changes: ${String(failure)}`; }
    finally { if (request === requestVersion) applying = false; }
  }

  function canApply() {
    return enabled && !!preview && ["spotify", "youtube"].includes(preview.provider)
      && preview.bindingId === bindingId && /^[A-Za-z0-9-]{8,128}$/.test(preview.previewId)
      && typeof preview.baselineRevision === "string" && preview.baselineRevision.length > 0 && preview.baselineRevision.length <= 256
      && typeof preview.remoteRevision === "string" && preview.remoteRevision.length > 0 && preview.remoteRevision.length <= 256
      && playlistWritePlanAllowed(preview.canEdit, preview.writeCapabilities, preview.changes) && preview.writeSyncEnabled === true && preview.conflict === false
      && preview.applied !== true && !preview.error && !!preview.changes
      && Array.isArray(preview.changes.add) && Array.isArray(preview.changes.remove) && Array.isArray(preview.changes.move)
      && !(preview.changes.unresolved?.length);
  }

  function providerName() { return preview?.provider === "youtube" ? "YouTube" : "Spotify"; }

  function label(group: string, raw: unknown) {
    const row = raw as Record<string, unknown>;
    const entryIds = Array.isArray(row.entryIds) ? row.entryIds.join(", ") : "";
    if (group === "add") return String(row.trackUri || "Track");
    if (group === "remove") return `Entries ${entryIds}`;
    if (group === "move") return `Entries ${entryIds} → ${row.beforeEntryId || "end"}`;
    const name = `${row.title || "Unresolved track"}${row.artist ? ` · ${row.artist}` : ""}`;
    return `${name} · ${row.reason || `unavailable or unresolved on ${providerName()}`}`;
  }
</script>

{#if enabled}
  <section class="playlist-sync" aria-label={`Sync ${preview?.provider === "youtube" ? "YouTube" : "Spotify"} playlist`}>
    <button class="sync-trigger" disabled={loading || applying || !bindingId} onclick={() => void sync()}>{loading ? "Checking…" : preview ? "Refresh" : "Sync"}</button>
    {#if error}<p class="sync-error" role="alert">{error}</p>{/if}
    {#if notice}<p class="sync-notice" role="status">{notice}</p>{/if}
    {#if Object.keys(reviewedLabels).length}
      <section class="sync-change-group" aria-label="Chosen recordings">
        <h4>Chosen recordings</h4>
        <ul>{#each Object.values(reviewedLabels) as label}<li>{label}</li>{/each}</ul>
        <button disabled={loading || applying} onclick={() => { reviewedMatches = {}; reviewedLabels = {}; void sync(); }}>Clear matches and refresh</button>
      </section>
    {/if}
    {#if preview}
      <div class="sync-review">
        <strong>{preview.summary || `Review ${providerName()} changes`}</strong>
        <span>{preview.conflict ? `The ${providerName()} playlist changed. Refresh before applying.` : "Review the complete change list before applying."}</span>
      </div>
      {#each [{ key: "add", title: `Add to ${providerName()}`, rows: preview.changes.add }, { key: "remove", title: `Remove from ${providerName()}`, rows: preview.changes.remove }, { key: "move", title: `Move in ${providerName()}`, rows: preview.changes.move }, { key: "unresolved", title: "Unavailable or unresolved", rows: preview.changes.unresolved ?? [] }] as group}
        {#if group.rows.length}
          <section class="sync-change-group">
            <h4>{group.title} <span>{group.rows.length}</span></h4>
            <ul>{#each group.rows as row}<li>{label(group.key, row)}
              {#if group.key === "unresolved" && "localTrackId" in row && row.localTrackId}
                <PlaylistWritebackMatch {bindingId} localTrackId={row.localTrackId} provider={preview.provider} disabled={loading || applying} {call} onSelect={selectMatch} />
              {/if}
            </li>{/each}</ul>
          </section>
        {/if}
      {/each}
      {#if preview.conflict}<p class="sync-blocked" role="status">This preview is stale. Refresh it before applying.</p>{/if}
      {#if preview.changes.unresolved?.length}<p class="sync-blocked" role="status">{preview.changes.unresolved.length} track{preview.changes.unresolved.length === 1 ? " is" : "s are"} unavailable or unresolved on {providerName()}. Choose a matching recording, or replace or remove the item in the working playlist; nothing will be guessed or skipped.</p>{/if}
      {#if !hasPlaylistWriteCapability(preview.canEdit, preview.writeCapabilities)}<p class="sync-blocked" role="status">This {providerName()} playlist is read-only.</p>{:else if !preview.canEdit && playlistWriteCapabilities(preview.canEdit, preview.writeCapabilities).add && !playlistWriteCapabilities(preview.canEdit, preview.writeCapabilities).remove && !playlistWriteCapabilities(preview.canEdit, preview.writeCapabilities).reorder}<p class="sync-blocked" role="status">Additions only. Removing or reordering tracks is not available.</p>{/if}
      {#if preview.writeSyncEnabled === false}<p class="sync-blocked" role="status">{providerName()} write sync is turned off. Enable it in settings before applying changes.</p>{/if}
      {#if preview.provider === "youtube" && preview.writeSyncEnabled === true && canApply()}<p class="sync-blocked" role="status">YouTube writes use an experimental private interface. Review the exact change list and refresh if anything has changed.</p>{/if}
      {#if canApply()}<button class="sync-apply" disabled={applying || loading} onclick={() => void apply()}>{applying ? "Applying…" : "Apply changes"}</button>{/if}
    {/if}
  </section>
{/if}

<style>
  .playlist-sync { min-width:0; display:grid; justify-items:start; gap:8px; }
  .sync-trigger,.sync-apply { padding:7px 11px; font-size:11px; }
  .sync-review { min-width:0; display:grid; gap:3px; padding:8px; border-radius:7px; background:var(--raised); font-size:11px; }
  .sync-review span { color:var(--muted); }
  .sync-change-group { min-width:0; width:100%; display:grid; gap:5px; }
  .sync-change-group h4 { display:flex; justify-content:space-between; margin:0; font-size:11px; }
  .sync-change-group h4 span { color:var(--muted); }
  .sync-change-group ul { display:grid; gap:4px; max-height:160px; overflow:auto; margin:0; padding:0; list-style:none; }
  .sync-change-group li { min-width:0; padding:6px; border-radius:5px; background:var(--raised); font-size:10px; overflow-wrap:anywhere; }
  .sync-error,.sync-notice,.sync-blocked { margin:0; font-size:11px; line-height:1.4; overflow-wrap:anywhere; }
  .sync-error { color:var(--danger,#d66); }
  .sync-notice { color:var(--success,#5bab8a); }
  .sync-blocked { padding:7px; border-radius:6px; background:color-mix(in srgb,var(--warning,#d39c45) 10%,transparent); color:var(--warning,#d39c45); }
</style>
