<script lang="ts">
  import { modalDialog } from "./lib/modal-dialog";
  import type { Candidate, Track } from "./lib/types";

  let { track, candidate = null, chapterTitle = "", onClose, onCatalogSearch, onWebSearch, onOpenSource }:
    { track: Track; candidate?: Candidate | null; chapterTitle?: string; onClose: () => void; onCatalogSearch: (query: string) => void; onWebSearch: (query: string) => void; onOpenSource: (url: string) => void } = $props();
  const chapter = $derived(Boolean(track.playbackRange) || Boolean(chapterTitle));
  const sourceUrl = $derived(candidate?.url || track.sourceUrl || "");
  const initialQuery = $derived([chapterTitle || track.title, !(chapterTitle || track.title).toLocaleLowerCase().includes(track.artist.toLocaleLowerCase()) ? track.artist : "", track.version].filter(Boolean).join(" ").trim());
  let query = $state("");
  let previousSeed = "";
  $effect(() => { if (initialQuery !== previousSeed) { previousSeed = initialQuery; query = initialQuery; } });
  function focus(node: HTMLDialogElement) {
    const modal = modalDialog(node, onClose);
    node.querySelector<HTMLInputElement>("input")?.focus();
    return modal;
  }
  function submitCatalog() { const value = query.trim(); if (value) onCatalogSearch(value); }
  function submitWeb() { const value = query.trim(); if (value) onWebSearch(value); }
</script>

<dialog use:focus class="info-search" aria-labelledby="info-search-title" onclick={event => {
    if (event.target !== event.currentTarget) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose();
  }}>
    <header><div><p class="eyebrow">MORE INFORMATION</p><h2 id="info-search-title">Search music information</h2></div><button aria-label="Close information search" onclick={onClose}>×</button></header>
    <p class="context">{chapter ? `Chapter: ${chapterTitle || track.title} · from ${track.playbackRange?.parentTitle || track.title}` : `${track.artist || "Unknown artist"} · ${track.title}`}</p>
    {#if candidate?.provider}<p class="source-label">Audio source: {candidate.provider}{candidate.uploader ? ` · ${candidate.uploader}` : ""}</p>{/if}
    <form onsubmit={(event) => { event.preventDefault(); submitCatalog(); }}>
      <label for="info-query">Search query</label><input id="info-query" bind:value={query} autocomplete="off" />
      <div class="actions"><button class="primary" type="submit" disabled={!query.trim()}>Search catalogs</button><button type="button" disabled={!query.trim()} onclick={submitWeb}>Search the web ↗</button></div>
    </form>
    {#if sourceUrl && /^https?:\/\//i.test(sourceUrl)}<button class="source" onclick={() => onOpenSource(sourceUrl)}>Open audio source ↗</button>{/if}
    <p class="note">Search results are for reference. They do not change this track’s tags or catalog identity.</p>
  </dialog>

<style>
  .info-search{width:min(480px,calc(100vw - 32px));max-height:calc(100dvh - 32px);overflow:auto;padding:20px;border:1px solid var(--line);border-radius:14px;background:var(--surface);color:var(--text);box-shadow:0 18px 58px #0009}.info-search::backdrop{background:#0009}header{display:flex;align-items:start;justify-content:space-between;gap:12px}h2{margin:3px 0 0;font-size:18px}.eyebrow{margin:0;color:var(--muted);font-size:10px;letter-spacing:.12em}.context{margin:18px 0 3px;font-weight:600}.source-label,.note{color:var(--muted);font-size:12px}label{display:block;margin:16px 0 6px;font-size:12px}input{box-sizing:border-box;width:100%;padding:10px;border:1px solid var(--line);border-radius:7px;background:var(--base);color:var(--text)}.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:12px}.source{margin-top:10px}.note{margin:16px 0 0;line-height:1.45}header button{font-size:20px;line-height:1}
</style>
