<script lang="ts">
  import { onDestroy } from "svelte";
  import { call } from "./lib/api";
  import type { RadioStation } from "./RadioView.svelte";
  let {onStationsChanged, onClose}: {onStationsChanged: (stations: RadioStation[]) => void; onClose: () => void} = $props();
  let addMode = $state<"search" | "url">("search");
  let searchQuery = $state(""), countryFilter = $state(""), tagFilter = $state("");
  let searchResults = $state<RadioStation[]>([]), searching = $state(false), searched = $state(false);
  let addingStationId = $state(""), stationName = $state(""), stationUrl = $state(""), saving = $state(false), error = $state("");
  let disposed = false, searchRequest = 0;
  onDestroy(() => { disposed = true; searchRequest++; });
  function stationTags(station: RadioStation, max = 3) { return (station.tags ?? "").split(/[;,]/).map(tag => tag.trim()).filter(Boolean).slice(0,max); }
  function activate(node: HTMLDialogElement) {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (typeof node.showModal === "function") node.showModal(); else node.setAttribute("open", "");
    node.querySelector<HTMLInputElement>("input")?.focus();
    return {destroy() { if (node.open && typeof node.close === "function") node.close(); if (previous?.isConnected) previous.focus(); }};
  }
  async function searchStations() {
    const query = searchQuery.trim(), country = countryFilter.trim(), tag = tagFilter.trim();
    if (!query && !country && !tag) return;
    const request = ++searchRequest;
    searching = true; searched = false; error = "";
    try {
      const results = await call<RadioStation[]>("search_radio_stations", {query, country: country || null, tag: tag || null, limit: 20});
      if (!disposed && request === searchRequest) { searchResults = results; searched = true; }
    } catch (failure) { if (!disposed && request === searchRequest) error = String(failure); }
    finally { if (!disposed && request === searchRequest) searching = false; }
  }
  async function addSearchStation(station: RadioStation) {
    if (saving) return;
    saving = true; addingStationId = station.id; error = "";
    try { const stations = await call<RadioStation[]>("add_radio_station", {station}); onStationsChanged(stations); if (!disposed) onClose(); }
    catch (failure) { if (!disposed) error = String(failure); }
    finally { saving = false; addingStationId = ""; }
  }
  async function addCustomStation() {
    if (saving || !stationName.trim() || !stationUrl.trim()) return;
    saving = true; error = "";
    try { const stations = await call<RadioStation[]>("add_custom_radio_station", {name:stationName.trim(), url:stationUrl.trim()}); onStationsChanged(stations); if (!disposed) onClose(); }
    catch (failure) { if (!disposed) error = String(failure); }
    finally { saving = false; }
  }
</script>

<dialog class="add-radio-dialog" aria-labelledby="add-radio-title" aria-modal="true" use:activate oncancel={(event) => {event.preventDefault(); onClose();}}>
  <header><h2 id="add-radio-title">Add radio station</h2><button type="button" aria-label="Close add radio station" onclick={onClose}>×</button></header>
      <div class="add-tabs" role="tablist" aria-label="Add station method"><button role="tab" aria-selected={addMode === "search"} class:active={addMode === "search"} onclick={() => (addMode = "search")}>Search</button><button role="tab" aria-selected={addMode === "url"} class:active={addMode === "url"} onclick={() => (addMode = "url")}>Stream URL</button></div>
      {#if addMode === "search"}
        <form class="station-search" onsubmit={(event) => { event.preventDefault(); void searchStations(); }}><label>Search internet radio <span><input aria-label="Search internet radio" bind:value={searchQuery} placeholder="Station name or keyword" /><button disabled={searching || (!searchQuery.trim() && !countryFilter.trim() && !tagFilter.trim())}>{searching ? "Searching…" : "Search"}</button></span></label><div class="radio-filters"><label>Country<input aria-label="Station country filter" bind:value={countryFilter} placeholder="Any country" /></label><label>Genre<input aria-label="Station genre filter" bind:value={tagFilter} placeholder="Any genre" /></label></div></form>
        {#if searchResults.length}<ul class="search-results" aria-label="Station search results">{#each searchResults as station (station.id)}<li><div><strong>{station.name}</strong><small>{[station.country, stationTags(station, 3).join(" · "), station.codec, (station.bitrateKbps ?? station.bitrate) ? `${station.bitrateKbps ?? station.bitrate} kbps` : ""].filter(Boolean).join(" · ") || "Internet radio"}</small></div><button disabled={Boolean(addingStationId)} onclick={() => void addSearchStation(station)}>{Boolean(addingStationId) ? "Adding…" : "Add"}</button></li>{/each}</ul>{:else if searched && !searching}<p class="radio-empty">No matching stations yet. Try a country or genre.</p>{/if}
      {:else}
        <form class="station-search custom-station" onsubmit={(event) => { event.preventDefault(); void addCustomStation(); }}><label>Station name<input aria-label="Station name" bind:value={stationName} /></label><label>Stream URL<input aria-label="Stream URL" type="url" bind:value={stationUrl} placeholder="https://…" /></label><button disabled={saving || !stationName.trim() || !stationUrl.trim()}>Add stream</button><small>Use a direct audio stream URL. HLS playlists are supported when the app can play them.</small></form>
      {/if}
      {#if error}<p role="alert" class="error">{error}</p>{/if}

</dialog>

<style>
  .add-radio-dialog{box-sizing:border-box;width:min(620px,calc(100vw - 32px));max-width:calc(100vw - 32px);max-height:calc(100svh - 32px);overflow:auto;padding:22px;border:1px solid var(--line);border-radius:12px;background:var(--surface);color:var(--text)}
  .add-radio-dialog::backdrop{background:#0009}header{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:18px}h2{margin:0;font-size:20px}button{border:1px solid var(--line);border-radius:7px;padding:8px 12px;background:var(--raised);color:var(--text)}button:disabled{opacity:.5}.add-tabs{display:flex;gap:6px;margin-bottom:16px}.add-tabs button{background:transparent}.add-tabs button.active{background:var(--raised);color:var(--accent)}.station-search label{display:grid;gap:6px;min-width:0;color:var(--muted);font-size:12px}.station-search label span{display:flex;gap:8px;min-width:0}.station-search label span input{flex:1;width:0}.station-search button{flex-shrink:0}.station-search input{box-sizing:border-box;min-width:0;width:100%;padding:9px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);font:inherit}.radio-filters{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:10px;margin-top:12px}.custom-station{display:grid;gap:14px}.custom-station>button{justify-self:start}.custom-station small,.radio-empty{color:var(--muted);font-size:12px;line-height:1.5}.search-results{list-style:none;padding:0;margin:16px 0 0}.search-results li{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:12px 0;border-top:1px solid var(--line)}.search-results li div{display:grid;gap:4px;min-width:0;overflow-wrap:anywhere}.search-results small{color:var(--muted);font-size:12px}.error{padding:12px;border:1px solid #a44;border-radius:8px;color:#ff9c9c}@media(max-width:420px){.add-radio-dialog{padding:16px}.radio-filters{grid-template-columns:1fr}}
</style>
