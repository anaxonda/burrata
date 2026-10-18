<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";

  type Provider = "spotify" | "youtube";
  type RemotePlaylist = {
    id: string;
    title: string;
    url: string;
    ownerName?: string | null;
    trackCount?: number | null;
    imported?: boolean;
  };
  type PlaylistPage = { items?: RemotePlaylist[]; nextCursor?: string | null };

  let { provider, enabled, call, importedUrls = [], filter = "", onOpen }: {
    provider: Provider;
    enabled: boolean;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    importedUrls?: string[];
    filter?: string;
    onOpen: (url: string) => void;
  } = $props();
  const providerReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));

  let items = $state<RemotePlaylist[]>([]);
  let nextCursor = $state<string | null>(null);
  let busy = $state(false);
  let error = $state("");
  let requestGeneration = 0;

  $effect(() => {
    if (!enabled) {
      providerReads.cancelAll();
      requestGeneration++;
      busy = false;
      items = [];
      nextCursor = null;
      error = "";
      return;
    }
    const generation = ++requestGeneration;
    untrack(() => { void loadPage(null, false, generation); });
    return () => { if (requestGeneration === generation) requestGeneration++; providerReads.cancelAll(); };
  });

  onDestroy(() => { requestGeneration++; providerReads.cancelAll(); });
  const visibleItems = $derived(items.filter(item =>
    !item.imported && !importedUrls.some(url => normalizePlaylistUrl(url) === normalizePlaylistUrl(item.url)) &&
    (!filter.trim() || `${item.title} ${item.ownerName ?? ""}`.toLocaleLowerCase().includes(filter.trim().toLocaleLowerCase()))
  ));
  let providerName = $derived(provider === "spotify" ? "Spotify" : "YouTube");

  function normalizePlaylistUrl(value: string) {
    try {
      const url = new URL(value);
      url.hash = "";
      url.hostname = url.hostname.toLowerCase().replace(/^www\./, "");
      if (url.hostname === "youtube.com" || url.hostname === "music.youtube.com") {
        const list = url.searchParams.get("list");
        url.search = "";
        if (list) url.searchParams.set("list", list);
      } else {
        url.search = "";
      }
      url.pathname = url.pathname.replace(/\/$/, "");
      return url.toString().replace(/\/$/, "");
    } catch { return value.trim(); }
  }

  function mergeRows(current: RemotePlaylist[], incoming: RemotePlaylist[]) {
    const seen = new Set(current.flatMap(item => [item.id, normalizePlaylistUrl(item.url)]));
    const merged = [...current];
    for (const item of incoming) {
      if (!item || typeof item.id !== "string" || typeof item.title !== "string" || typeof item.url !== "string") continue;
      const url = normalizePlaylistUrl(item.url);
      if (seen.has(item.id) || seen.has(url)) continue;
      seen.add(item.id);
      seen.add(url);
      merged.push(item);
    }
    return merged;
  }

  async function loadPage(cursor: string | null, append: boolean, generation = requestGeneration) {
    if (!enabled || busy) return;
    busy = true;
    error = "";
    try {
      const args: Record<string, unknown> = { provider };
      if (cursor) args.cursor = cursor;
      const page = await providerReads.read<PlaylistPage>("list_remote_playlists", args);
      if (generation !== requestGeneration) return;
      if (!page || !Array.isArray(page.items)) throw new Error("invalid page");
      items = mergeRows(append ? items : [], page.items);
      nextCursor = typeof page.nextCursor === "string" && page.nextCursor ? page.nextCursor : null;
    } catch {
      if (generation === requestGeneration) error = `Could not load ${providerName} playlists. Check the browser connection and site permission.`;
    } finally {
      if (generation === requestGeneration) busy = false;
    }
  }

  function refresh() {
    const generation = ++requestGeneration;
    void loadPage(null, false, generation);
  }
</script>

<section class="account-playlists" aria-label={`${providerName} playlists`}>
  <header>
    <span class="scope-label">Loaded results</span>
    {#if enabled}<button class="quiet" disabled={busy} onclick={refresh}>Refresh</button>{/if}
  </header>
  {#if !enabled}
    <p class="hint">Turn on playlists in Settings to browse here.</p>
  {:else}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if visibleItems.length}
      <ul>
        {#each visibleItems as item (`${item.id}:${normalizePlaylistUrl(item.url)}`)}
          <li>
            <button class="playlist-title" onclick={() => onOpen(item.url)}>{item.title}</button>
            <span class="playlist-detail">
              {#if item.ownerName}{item.ownerName}{/if}
              {#if typeof item.trackCount === "number"}{#if item.ownerName} · {/if}{item.trackCount} tracks{/if}
            </span>
          </li>
        {/each}
      </ul>
    {:else if !busy && !error}
      <p class="hint">{filter.trim() ? "No loaded playlists match this filter." : "No unimported playlists to show."}</p>
    {/if}
    {#if busy}<p class="hint" role="status">Loading playlists…</p>{/if}
    {#if nextCursor}<button class="load-more" disabled={busy} onclick={() => void loadPage(nextCursor, true)}>Load more</button>{/if}
  {/if}
</section>

<style>
  .account-playlists { display:grid; gap:7px; min-width:0; }
  header { display:flex; align-items:center; justify-content:space-between; gap:8px; }
  .scope-label { color:var(--muted); font-size:10px; }
  button { min-height:28px; }
  .quiet { padding:4px 8px; background:transparent; font-size:10px; }
  .load-more { width:100%; padding:5px 8px; font-size:10px; }
  ul { display:grid; gap:3px; margin:0; padding:0; list-style:none; }
  li { display:grid; min-width:0; gap:1px; padding:4px 0; }
  .playlist-title { overflow:hidden; min-width:0; padding:2px 0; border:0; background:transparent; color:var(--text); text-align:left; text-overflow:ellipsis; white-space:nowrap; font-size:11px; }
  .playlist-detail,.hint { margin:0; color:var(--muted); font-size:10px; }
  .playlist-detail { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .error { margin:0; color:var(--warning,#d39c45); font-size:10px; }
</style>
