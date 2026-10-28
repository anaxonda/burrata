<script lang="ts">
  import { untrack } from "svelte";
  import type { Collection, PlaylistMirrorOptions, PlaylistMirrorOptionsView } from "./lib/types";

  type Call = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  let {
    collections,
    initialCollectionId = null,
    call,
    onClose,
    onSaved,
  }: {
    collections: Collection[];
    initialCollectionId?: string | null;
    call: Call;
    onClose: () => void;
    onSaved?: () => void;
  } = $props();

  const specialCollections: Collection[] = [
    { id: "favorites", name: "Liked Songs", trackCount: 0 },
    { id: "history", name: "Listening history", trackCount: 0 },
  ];
  const choices = $derived.by(() => {
    const byId = new Map<string, Collection>();
    for (const collection of [...collections, ...specialCollections]) byId.set(collection.id, collection);
    return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
  });
  let selectedCollectionId = $state(untrack(() => initialCollectionId ?? choices[0]?.id ?? ""));
  let loaded = $state<PlaylistMirrorOptionsView | null>(null);
  let draft = $state<PlaylistMirrorOptions | null>(null);
  let loading = $state(false);
  let saving = $state(false);
  let error = $state("");
  let request = 0;

  async function loadOptions(collectionId: string) {
    if (!collectionId) return;
    const current = ++request;
    loading = true;
    error = "";
    loaded = null;
    draft = null;
    try {
      const result = await call("get_playlist_mirror_options", { collectionId }) as PlaylistMirrorOptionsView;
      if (current !== request || selectedCollectionId !== collectionId) return;
      loaded = result;
      draft = { ...result.options };
    } catch (failure) {
      if (current === request) error = String(failure);
    } finally {
      if (current === request) loading = false;
    }
  }

  $effect(() => {
    if (selectedCollectionId) void loadOptions(selectedCollectionId);
  });

  function resolved(key: "includeMediaLinks" | "includeYtdlpSearch"): boolean {
    if (!loaded || !draft) return false;
    return draft[key] ?? loaded.defaults[key];
  }

  function setResolved(key: "includeMediaLinks" | "includeYtdlpSearch", enabled: boolean) {
    if (!loaded || !draft) return;
    draft = { ...draft, [key]: enabled };
  }

  function useDefaults() {
    if (!draft) return;
    draft = { ...draft, includeMediaLinks: null, includeYtdlpSearch: null };
  }

  async function save() {
    if (!draft || !selectedCollectionId || saving) return;
    saving = true;
    error = "";
    try {
      const result = await call("set_playlist_mirror_options", {
        collectionId: selectedCollectionId,
        options: { ...draft },
      }) as PlaylistMirrorOptionsView;
      loaded = result;
      draft = { ...result.options };
      onSaved?.();
    } catch (failure) {
      error = String(failure);
    } finally {
      saving = false;
    }
  }
</script>

<dialog open class="playlist-mirror-dialog" aria-label="Local M3U copy settings" aria-modal="true">
  <header>
    <h2>Local M3U copy</h2>
    <button type="button" aria-label="Close local M3U settings" onclick={onClose}>×</button>
  </header>
  <label>Playlist
    <select aria-label="Playlist for local M3U copy" bind:value={selectedCollectionId} disabled={loading || saving}>
      {#each choices as collection (collection.id)}
        <option value={collection.id}>{collection.name}</option>
      {/each}
    </select>
  </label>
  {#if loading}<p role="status">Loading playlist settings…</p>
  {:else if loaded && draft}
    <label class="settings-option">
      <input type="checkbox" aria-label="Include this playlist in local M3U copies" bind:checked={draft.included} disabled={saving} />
      <span><strong>Keep a local copy of this playlist</strong><small>Include this playlist in automatic M3U8 updates.</small></span>
    </label>
    <label class="settings-option">
      <input type="checkbox" aria-label="Include media links in this playlist copy" checked={resolved("includeMediaLinks")} onchange={(event) => setResolved("includeMediaLinks", event.currentTarget.checked)} disabled={saving} />
      <span><strong>Include local and online media links</strong><small>Write media paths and links for local and online audio.</small></span>
    </label>
    <label class="settings-option">
      <input type="checkbox" aria-label="Include yt-dlp search fallback in this playlist copy" checked={resolved("includeYtdlpSearch")} onchange={(event) => setResolved("includeYtdlpSearch", event.currentTarget.checked)} disabled={saving} />
      <span><strong>Include yt-dlp search fallback</strong><small>Add a search fallback for playlist entries without an available media link.</small></span>
    </label>
    {#if draft.included && !resolved("includeMediaLinks") && !resolved("includeYtdlpSearch")}
      <p class="playlist-mirror-metadata-note">This copy will contain playlist metadata only. Enable media links or the search fallback to include audio references.</p>
    {/if}
    {#if draft.includeMediaLinks !== null || draft.includeYtdlpSearch !== null}
      <button type="button" class="secondary" disabled={saving} onclick={useDefaults}>Use current defaults</button>
    {/if}
    {#if !loaded.globallyEnabled}<p role="status">Global playlist mirroring is off. These choices are saved and will apply when it is enabled.</p>{/if}
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <footer>
    <button type="button" class="secondary" onclick={onClose}>Close</button>
    <button type="button" disabled={!loaded || !draft || loading || saving} onclick={() => void save()}>{saving ? "Saving…" : "Save playlist copy"}</button>
  </footer>
</dialog>

<style>
  .playlist-mirror-dialog { position: fixed; inset: 50% auto auto 50%; transform: translate(-50%, -50%); z-index: 120; width: min(480px, calc(100vw - 32px)); max-height: min(80vh, 680px); overflow: auto; border: 1px solid var(--line); border-radius: 14px; padding: 20px; color: var(--text); background: var(--panel); box-shadow: 0 20px 70px #0008; }
  header, footer { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  header h2 { margin: 0; font-size: 18px; }
  dialog > label:not(.settings-option) { display: grid; gap: 7px; margin: 16px 0; }
  select { min-height: 38px; }
  .settings-option { display: flex; gap: 10px; align-items: flex-start; padding: 12px 0; }
  .settings-option span { display: grid; gap: 4px; }
  .settings-option small, .playlist-mirror-metadata-note { color: var(--muted); }
  .playlist-mirror-metadata-note { padding: 10px; border-radius: 8px; background: var(--raised); }
  footer { justify-content: flex-end; margin-top: 18px; }
  button, select { color: inherit; background: var(--raised); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px; }
</style>
