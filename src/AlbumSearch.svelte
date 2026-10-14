<script lang="ts">
  import type { AlbumSearchPage, ArtistReleaseRef } from "./lib/artist-types";

  let { query: initialQuery = "", search }: {
    query?: string;
    search: (query: string, page: number) => Promise<AlbumSearchPage>;
  } = $props();

  let query = $state("");
  let submittedQuery = $state("");
  let results = $state<ArtistReleaseRef[]>([]);
  let nextPage = $state<number | null>(null);
  let warnings = $state<string[]>([]);
  let error = $state("");
  let busy = $state(false);
  let generation = 0;
  let lastPropQuery = $state("");

  $effect(() => {
    if (initialQuery !== lastPropQuery) {
      lastPropQuery = initialQuery;
      query = initialQuery;
      if (initialQuery.trim()) void runSearch();
    }
  });

  async function runSearch(page = 1, append = false) {
    const term = (append ? submittedQuery : query).trim();
    if (!term) return;
    const request = ++generation;
    busy = true;
    error = "";
    if (!append) { submittedQuery = term; results = []; nextPage = null; warnings = []; }
    try {
      const response = await search(term, page);
      if (request !== generation) return;
      const merged = append ? [...results, ...response.items] : response.items;
      const unique = new Map<string, ArtistReleaseRef>();
      for (const release of merged) unique.set(`${release.release.provider}:${release.sourceUrl}`, release);
      results = [...unique.values()];
      nextPage = response.nextPage ?? null;
      warnings = response.warnings;
    } catch (cause) {
      if (request === generation) error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      if (request === generation) busy = false;
    }
  }
</script>

<section class="album-search" aria-label="Album search">
  <header><div><p class="eyebrow">MUSICBRAINZ · DISCOGS</p><h2>Album search</h2><p class="muted">Search album and release titles across music catalogs.</p></div></header>
  <form onsubmit={(event) => { event.preventDefault(); void runSearch(); }}>
    <input bind:value={query} aria-label="Search albums" placeholder="Album title" />
    <button type="submit" disabled={busy || !query.trim()}>{busy ? "Searching…" : "Search albums"}</button>
  </form>
  {#if error}<p class="notice error" role="alert">{error} <button class="secondary" onclick={() => void runSearch()}>Retry</button></p>{/if}
  {#each warnings as warning}<p class="notice">{warning}</p>{/each}
  <div class="albums" aria-live="polite">
    {#each results as album (`${album.release.provider}:${album.sourceUrl}`)}
      <article>
        <div><small>{album.release.provider === "musicbrainz" ? "MusicBrainz release group" : "Discogs release"}</small>
          <h3>{album.title}</h3><p>{album.credits.map((credit) => credit.name).filter(Boolean).join(", ") || "Artist credit unavailable"}</p>
          <small>{album.primaryType ?? album.format ?? "Release type unavailable"}{#if album.year} · {album.year}{/if}{#if album.trackCount} · {album.trackCount} tracks{/if}</small>
          <details><summary>Show editions and details</summary><p class="muted">Edition and track details are available from the catalog source.</p><a href={album.sourceUrl} target="_blank" rel="noreferrer">Open source ↗</a></details>
        </div>
        <a class="source-link" href={album.sourceUrl} target="_blank" rel="noreferrer">Open source ↗</a>
      </article>
    {/each}
  </div>
  {#if submittedQuery && !results.length && !busy && !error}<p class="muted">No albums found.</p>{/if}
  {#if nextPage}<button class="secondary" disabled={busy} onclick={() => void runSearch(nextPage!, true)}>Load more albums</button>{/if}
</section>

<style>
  .album-search{display:flex;flex-direction:column;gap:1rem;min-width:0;min-height:0;overflow:auto;padding:1.25rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px}
  header,article,form{display:flex;align-items:center;justify-content:space-between;gap:1rem}
  form input{flex:1;min-width:0}button,input{font:inherit;padding:.65rem .8rem;border:1px solid var(--border,#454a50);border-radius:8px;background:var(--surface,#202328);color:inherit}button{cursor:pointer}button:disabled{opacity:.55;cursor:default}
  .albums{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,18rem),1fr));gap:.75rem}article{align-items:flex-start;border:1px solid var(--border,#454a50);border-radius:10px;padding:1rem}h2,h3,p{margin:.25rem 0}h2{font-size:1.4rem}h3{font-size:1rem}.eyebrow,small,.muted{color:var(--muted,#a3a8ae);font-size:.82rem}.eyebrow{font-weight:700;letter-spacing:.09em}.source-link{white-space:nowrap}a{color:var(--accent,#9dbfff)}details{margin-top:.6rem}
</style>
