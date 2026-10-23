<script lang="ts">
  import { untrack } from "svelte";
  export type UnsortedLibraryEntry = {
    id: string;
    provider: string;
    providerId: string;
    artist: string;
    title: string;
    album?: string | null;
    selectionToken: string;
    classification: "unsortedLibraryEntry";
  };
  export type UnsortedLibraryPage = {
    entries: UnsortedLibraryEntry[];
    total: number;
    limit: number;
    offset: number;
  };
  export type UnsortedLibraryRemoval = {
    removedIds: string[];
    skipped: { id: string; reason: string }[];
  };
  type NativeCall = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;

  let { open = false, call, onClose = () => {}, onRemoved = () => {}, pageSize = 25 }: {
    open?: boolean;
    call: NativeCall;
    onClose?: () => void;
    onRemoved?: (ids: string[]) => void;
    pageSize?: number;
  } = $props();

  let offset = $state(0);
  let page = $state<UnsortedLibraryPage>({ entries: [], total: 0, limit: 25, offset: 0 });
  let selected = $state<Record<string, UnsortedLibraryEntry>>({});
  let busy = $state(false);
  let applying = $state(false);
  let error = $state("");
  let confirmOpen = $state(false);
  let resultMessage = $state("");
  let skipped = $state<{ id: string; reason: string }[]>([]);
  let requestEpoch = 0;

  $effect(() => {
    if (!open) { requestEpoch++; confirmOpen = false; return; }
    untrack(() => {
      offset = 0;
      selected = {};
      resultMessage = "";
      skipped = [];
      void loadPage(0);
    });
  });

  async function loadPage(nextOffset: number) {
    const epoch = ++requestEpoch;
    offset = Math.max(0, Math.trunc(nextOffset));
    busy = true;
    error = "";
    try {
      const response = await call<UnsortedLibraryPage>("preview_unsorted_library_entries", { limit: pageSize, offset });
      if (epoch !== requestEpoch) return;
      const entries = Array.isArray(response?.entries) ? response.entries.filter(entry =>
        entry && typeof entry.id === "string" && typeof entry.selectionToken === "string" &&
        entry.classification === "unsortedLibraryEntry" && typeof entry.provider === "string" && typeof entry.providerId === "string" &&
        typeof entry.artist === "string" && typeof entry.title === "string") : [];
      const total = Number.isFinite(response?.total) ? Math.max(0, response.total) : 0;
      if (offset >= total && total > 0) { void loadPage(Math.max(0, Math.floor((total - 1) / pageSize) * pageSize)); return; }
      page = { entries, total, limit: pageSize, offset };
    } catch (failure) {
      if (epoch === requestEpoch) error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      if (epoch === requestEpoch) busy = false;
    }
  }

  function toggleEntry(entry: UnsortedLibraryEntry) {
    const next = { ...selected };
    if (next[entry.id]?.selectionToken === entry.selectionToken) delete next[entry.id];
    else next[entry.id] = entry;
    selected = next;
    confirmOpen = false;
  }
  function deselectAll() {
    selected = {};
    confirmOpen = false;
  }
  const selectedEntries = $derived(Object.values(selected));
  const pageStart = $derived(page.total ? page.offset + 1 : 0);
  const pageEnd = $derived(Math.min(page.total, page.offset + page.entries.length));

  async function confirmRemoval() {
    if (!selectedEntries.length || applying) return;
    applying = true;
    error = "";
    resultMessage = "";
    skipped = [];
    const submitted = selectedEntries.map(({ id, selectionToken }) => ({ id, selectionToken }));
    try {
      const result = await call<UnsortedLibraryRemoval>("remove_unsorted_library_entries", { selections: submitted });
      const removedIds = Array.isArray(result?.removedIds) ? result.removedIds.filter(id => typeof id === "string") : [];
      const rejected = Array.isArray(result?.skipped) ? result.skipped.filter(item => item && typeof item.id === "string") : [];
      const next = { ...selected };
      for (const id of removedIds) delete next[id];
      selected = next;
      skipped = rejected;
      resultMessage = removedIds.length
        ? `Removed ${removedIds.length} selected entr${removedIds.length === 1 ? "y" : "ies"} from the library. Source cache and files were not deleted.`
        : "No entries were removed. The library may have changed since this review.";
      onRemoved(removedIds);
      confirmOpen = false;
      await loadPage(offset);
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      applying = false;
    }
  }
</script>

{#if open}
  <div class="cleanup-backdrop">
    <dialog open class="library-cleanup" aria-modal="true" aria-labelledby="library-cleanup-title">
      <header>
        <div><h2 id="library-cleanup-title">Review unsorted library entries</h2><p>These entries may include old search results. The label does not establish that an item is unwanted.</p></div>
        <button type="button" aria-label="Close cleanup review" onclick={onClose}>Close</button>
      </header>
      <p class="cleanup-scope">Review exact entries before removing their library membership. Source cache, downloaded files, favorites, playlists, and assets are kept.</p>
      {#if resultMessage}<p class="cleanup-result" role="status">{resultMessage}</p>{/if}
      {#if error}<p class="cleanup-error" role="alert">{error}</p>{/if}
      {#if busy}<p role="status">Loading review page…</p>
      {:else if page.total === 0}<p class="cleanup-empty">No unsorted entries are available for review.</p>
      {:else}
        <p class="cleanup-count" aria-live="polite">Showing {pageStart}–{pageEnd} of {page.total} possible old search results · {selectedEntries.length} selected</p>
        <ul class="cleanup-entries" aria-label="Unsorted library entries">
          {#each page.entries as entry (entry.id)}
            <li>
              <label>
                <input type="checkbox" aria-label={`Select ${entry.artist} — ${entry.title}`} checked={selected[entry.id]?.selectionToken === entry.selectionToken} onchange={() => toggleEntry(entry)} />
                <span class="cleanup-track"><strong>{entry.title}</strong><span>{entry.artist}{entry.album ? ` · ${entry.album}` : ""}</span><small>Possible old search result · review individually</small></span>
              </label>
            </li>
          {/each}
        </ul>
        <nav class="cleanup-pages" aria-label="Cleanup review pages">
          <button type="button" disabled={page.offset <= 0 || busy} onclick={() => void loadPage(Math.max(0, page.offset - pageSize))}>Previous</button>
          <button type="button" disabled={page.offset + page.entries.length >= page.total || busy} onclick={() => void loadPage(page.offset + pageSize)}>Next</button>
          <button type="button" disabled={!selectedEntries.length || applying} onclick={deselectAll}>Clear selection</button>
        </nav>
      {/if}
      {#if selectedEntries.length}
        {#if confirmOpen}
          <section class="cleanup-confirm" aria-label="Confirm selected removals">
            <h3>Remove {selectedEntries.length} selected entr{selectedEntries.length === 1 ? "y" : "ies"} from the library?</h3>
            <p>This removes library membership only. Cache entries and audio files will remain.</p>
            <ul>{#each selectedEntries as entry (entry.id)}<li>{entry.artist} — {entry.title}</li>{/each}</ul>
            <button type="button" disabled={applying} onclick={() => (confirmOpen = false)}>Cancel</button>
            <button type="button" class="remove-confirm" disabled={applying} onclick={() => void confirmRemoval()}>{applying ? "Removing…" : "Remove selected from library"}</button>
          </section>
        {:else}
          <button type="button" class="review-removal" onclick={() => (confirmOpen = true)}>Review removal of {selectedEntries.length} selected</button>
        {/if}
      {/if}
      {#if skipped.length}<section class="cleanup-skipped" role="status"><h3>Entries kept</h3><ul>{#each skipped as item (item.id)}<li>{page.entries.find(entry => entry.id === item.id)?.title ?? item.id}: {item.reason}</li>{/each}</ul></section>{/if}
    </dialog>
  </div>
{/if}

<style>
  .cleanup-backdrop{position:fixed;inset:0;z-index:1200;display:grid;place-items:center;padding:1rem;background:#0009}
  .library-cleanup{box-sizing:border-box;width:min(720px,100%);max-height:min(88vh,900px);overflow:auto;padding:1rem;border:1px solid var(--line,#454a50);border-radius:12px;background:var(--surface,#17191c);color:var(--text,#eee);box-shadow:0 12px 42px #0008}
  .library-cleanup header{display:flex;justify-content:space-between;gap:1rem;align-items:start}
  .library-cleanup h2{margin:.1rem 0 .35rem;font-size:1.2rem}.library-cleanup header p,.cleanup-scope{margin:.25rem 0;color:var(--muted,#a3a8ae);font-size:.88rem;line-height:1.45}
  .cleanup-scope{padding:.55rem;border-left:3px solid var(--accent,#9dbfff);background:#ffffff08}
  .cleanup-count{font-size:.82rem;color:var(--muted,#a3a8ae)}
  .cleanup-entries,.cleanup-confirm ul,.cleanup-skipped ul{list-style:none;margin:.5rem 0;padding:0}
  .cleanup-entries li{padding:.6rem 0;border-bottom:1px solid var(--line,#454a50)}
  .cleanup-entries label{display:flex;align-items:start;gap:.65rem;cursor:pointer}.cleanup-entries input{margin-top:.25rem}
  .cleanup-track{display:grid;gap:.15rem}.cleanup-track>span{color:var(--muted,#a3a8ae);font-size:.86rem}.cleanup-track small{color:var(--muted,#a3a8ae);font-size:.72rem}
  .cleanup-pages{display:flex;gap:.45rem;flex-wrap:wrap;margin:.75rem 0}
  .library-cleanup button{border:1px solid var(--line,#454a50);border-radius:7px;padding:.4rem .65rem;color:inherit;background:#ffffff0a;cursor:pointer}
  .library-cleanup button:disabled{opacity:.55;cursor:not-allowed}.review-removal,.remove-confirm{background:var(--accent,#9dbfff)!important;color:#111!important}
  .cleanup-confirm,.cleanup-skipped{margin:.75rem 0;padding:.7rem;border:1px solid var(--line,#454a50);border-radius:8px;background:#ffffff08}
  .cleanup-confirm h3,.cleanup-skipped h3{margin:.1rem 0 .4rem;font-size:1rem}.cleanup-confirm li,.cleanup-skipped li{padding:.15rem 0;font-size:.85rem}
  .cleanup-result{color:var(--success,#75d49a)}.cleanup-error{color:var(--danger,#ff8c8c)}.cleanup-empty{color:var(--muted,#a3a8ae)}
</style>
