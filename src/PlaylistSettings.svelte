<script lang="ts">
  type Collection = { id: string; name: string; trackCount: number };
  type Preferences = { favoritesPlaylistId: string; shortcuts: (string | null)[]; shortcutBindings?: string[] };
  let { preferences, collections, call, onchange, oncollections, includeYtdlpSearchDefault = false, onYtdlpSearchDefaultChange = () => {} }: {
    preferences: Preferences;
    collections: Collection[];
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onchange: (value: Preferences) => void;
    oncollections?: () => void | Promise<void>;
    includeYtdlpSearchDefault?: boolean;
    onYtdlpSearchDefaultChange?: (enabled: boolean) => void;
  } = $props();
  let newName = $state("");
  let bindings = $state<string[]>([]);
  let saving = $state(false);
  let error = $state("");

  async function assign(slot: number, collectionId: string) {
    await run(async () => {
      const updated = await call<Preferences>("set_playlist_shortcut", {
        slot, collectionId: collectionId || null,
      });
      onchange(updated);
    });
  }

  async function saveBinding(slot: number) {
    await run(async () => {
      const updated = await call<Preferences>("set_playlist_shortcut_binding", {
        slot, binding: bindings[slot],
      });
      onchange(updated);
      bindings = [...(updated.shortcutBindings ?? [])];
    });
  }

  async function setFavoritesPlaylist(collectionId: string) {
    await run(async () => {
      const updated = await call<Preferences>("set_favorites_playlist", { collectionId });
      onchange(updated);
    });
  }

  async function createAndAssign() {
    const name = newName.trim();
    if (!name) return;
    await run(async () => {
      const created = await call<Collection>("create_collection", { name });
      const current = [...preferences.shortcuts];
      const slot = current.findIndex(value => !value);
      if (slot >= 0) {
        const updated = await call<Preferences>("set_playlist_shortcut", { slot, collectionId: created.id });
        onchange(updated);
      }
      newName = "";
      await onchangeCollections();
    });
  }

  async function run(action: () => Promise<void>) {
    saving = true;
    error = "";
    try { await action(); }
    catch (failure) { error = String(failure); }
    finally { saving = false; }
  }

  async function onchangeCollections() { await oncollections?.(); }
  $effect(() => { bindings = [...(preferences.shortcutBindings ?? Array.from({ length: 9 }, (_, index) => `Alt+${index + 1}`))]; });
</script>

<section class="playlist-settings" aria-labelledby="playlist-settings-title">
  <header>
    <h3 id="playlist-settings-title">Favorites playlist</h3>
    <p>Choose the editable playlist used by the heart control. Existing playlists keep their tracks when you switch.</p>
  </header>
  <label class="favorites-target">
    <span>Favorites</span>
    <select aria-label="Favorites playlist" value={preferences.favoritesPlaylistId} disabled={saving}
      onchange={event => void setFavoritesPlaylist(event.currentTarget.value)}>
      {#if !collections.some(collection => collection.id === preferences.favoritesPlaylistId)}
        <option value={preferences.favoritesPlaylistId} disabled>Missing playlist · choose a new target</option>
      {/if}
      {#each collections as collection}<option value={collection.id}>{collection.name}</option>{/each}
    </select>
  </label>
  <header>
    <h3>Playlist shortcuts</h3>
    <p>Assign playlists and keys to the nine shortcut slots. Empty slots stay unassigned.</p>
  </header>
  <div class="slots">
    {#each Array.from({ length: 9 }, (_, index) => index) as slot}
      <div class="slot-card">
        <label>
          <span class="key">{slot + 1}</span>
          <select aria-label={`Playlist shortcut ${slot + 1}`} value={preferences.shortcuts[slot] ?? ""}
            disabled={saving} onchange={event => void assign(slot, event.currentTarget.value)}>
            <option value="">Unassigned</option>
            {#each collections as collection}<option value={collection.id}>{collection.name}</option>{/each}
          </select>
        </label>
        <label class="binding">
          <span>Key</span>
          <input aria-label={`Shortcut key ${slot + 1}`} value={bindings[slot] ?? ""} disabled={saving}
            onchange={event => { bindings[slot] = event.currentTarget.value; void saveBinding(slot); }} />
        </label>
      </div>
    {/each}
  </div>
  <form onsubmit={event => { event.preventDefault(); void createAndAssign(); }}>
    <input bind:value={newName} maxlength="120" placeholder="New playlist name" aria-label="New playlist name" disabled={saving} />
    <button type="submit" disabled={saving || !newName.trim()}>Create playlist</button>
  </form>
  <header>
    <h3>Playlist exports</h3>
    <p>Choose whether optional search links are included by default. Each export can override this.</p>
  </header>
  <label class="export-search-default">
    <input type="checkbox" aria-label="Include search links by default" checked={includeYtdlpSearchDefault}
      onchange={event => onYtdlpSearchDefaultChange(event.currentTarget.checked)} />
    <span>Include search links by default</span>
  </label>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  .playlist-settings { display:grid; gap:14px; }
  h3 { margin:0 0 4px; font-size:15px; }
  p { margin:0; color:var(--muted); font-size:12px; }
  .slots { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:8px; }
  .slot-card { min-width:0; display:grid; gap:6px; padding:8px; border:1px solid var(--line); border-radius:8px; }
  .favorites-target { display:flex; align-items:center; gap:10px; max-width:480px; }
  .favorites-target > span { width:90px; flex:none; }
  .binding { margin-left:34px; }
  .binding > span { width:25px; color:var(--muted); font-size:10px; }
  label { display:flex; align-items:center; gap:8px; min-width:0; }
  .key { display:grid; place-items:center; width:26px; height:26px; flex:none; border:1px solid var(--line); border-radius:6px; color:var(--muted); font-size:11px; }
  select,input { min-width:0; width:100%; }
  form { display:flex; gap:8px; }
  form button { flex:none; }
  .error { color:var(--danger,#d66); }
  @media(max-width:600px) { .slots { grid-template-columns:repeat(2,minmax(0,1fr)); } }
</style>
