<script lang="ts">
  import {playlistTitle} from "./lib/playlist-title";
  import type {RadioStation} from "./RadioView.svelte";
  import ProviderLogo from "./ProviderLogo.svelte";
  import PlaylistSections from "./PlaylistSections.svelte";
  import {PLAYLIST_SECTION_DRAG_TYPE,type PlaylistSection} from "./lib/playlist-sections";
  const downloadsId="00000000-0000-4000-8000-000000000001";

  type Collection = { id: string; name: string; trackCount: number; sourceProvider?: string; sourceUrl?: string; provenance?: unknown };
  type BrowserImport = { id: string; provider: string; title: string; collectionId: string; sourceUrl: string };

  let {
    view, favoritesPlaylistId = "",
    playlistNames={}, onRadioContext,
    radioStations=[], activeRadioStationId="", onRadioStation=()=>{}, onAddRadio=()=>{},
    sections=[], assignments={}, onCreateSection=()=>{}, onRenameSection=()=>{}, onDeleteSection=()=>{}, onMovePlaylist=()=>{}, onCreatePlaylist=()=>{},
    collections,
    browserImports,
    localPlaylistIds = [],
    activeCollectionId,
    activeSource = "all",
    onNavigate,
    onPlaylist,
    onPlaylistContext,
    onSource,
    onImport,
    onSettings,
    onDownloads,
    activeJobs,
    width = 200,
    onWidthChange,
  }: {
    favoritesPlaylistId?: string;
    view: string;playlistNames?:Record<string,string>;onRadioContext?:(station:RadioStation,event:MouseEvent)=>void;
    radioStations?:RadioStation[];activeRadioStationId?:string;onRadioStation?:(station:RadioStation)=>void;onAddRadio?:()=>void;
    sections?:PlaylistSection[];assignments?:Record<string,string>;
    onCreateSection?:(name:string)=>void;onRenameSection?:(id:string,name:string)=>void;onDeleteSection?:(id:string)=>void;onMovePlaylist?:(id:string,sectionId:string|null)=>void;onCreatePlaylist?:(sectionId?:string)=>void;
    collections: Collection[];
    browserImports: BrowserImport[];
    localPlaylistIds?: string[];
    activeCollectionId: string;
    activeSource?: string;
    onNavigate: (view: string) => void;
    onPlaylist: (id: string) => void;
    onPlaylistContext?: (id: string, event: MouseEvent) => void;
    onSource: (provider: string) => void;
    onImport: () => void;
    onSettings: () => void;
    onDownloads: () => void;
    activeJobs: number;
    width?: number;
    onWidthChange?: (width: number) => void;
  } = $props();

  let drawerOpen = $state(false);
  let resizing = $state(false);
  const minWidth = 160;
  const maxWidth = 320;
  function setWidth(value: number) { onWidthChange?.(Math.max(minWidth, Math.min(maxWidth, value))); }
  function startResize(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    resizing = true;
    const move = (moveEvent: PointerEvent) => setWidth(startWidth + moveEvent.clientX - startX);
    const end = () => { resizing = false; window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", end); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end, { once: true });
  }
  function toggleDrawer() { drawerOpen = !drawerOpen; }
  function closeDrawer() { drawerOpen = false; }

  const knownSources = [
    { id: "beatport", label: "Beatport", mark: "B" },
    { id: "traxsource", label: "Traxsource", mark: "T" },
    { id: "spotify", label: "Spotify", mark: "S" },
    { id: "shazam", label: "Shazam", mark: "Z" },
    { id: "lastfm", label: "Last.fm", mark: "L" },
    { id: "bandcamp", label: "Bandcamp", mark: "b" },
  ];
  const sourceGroups = $derived.by(() => {
    const knownIds = new Set([...knownSources.map((source) => source.id), "local"]);
    const providerIds = new Set([
      ...browserImports.map((item) => normalizeProvider(item.provider)).filter(Boolean),
      ...collections.map((item) => normalizeProvider(item.sourceProvider ?? "")).filter(Boolean),
    ]);
    const additional = [...providerIds].filter((id) => !knownIds.has(id)).sort().map((id) => ({ id, label: sourceLabel(id), mark: sourceLabel(id).slice(0, 1).toUpperCase() }));
    const sources = [...knownSources, ...additional, { id: "local", label: "Local", mark: "▧" }];
    return sources.map((source) => {
    const sourceImports = source.id === "local" ? [] : browserImports.filter((item) => normalizeProvider(item.provider) === source.id && !hasSection(item.collectionId));
    const importedIds = new Set(sourceImports.map((item) => item.collectionId));
    const sourceCollections = source.id === "local"
      ? collections.filter((item) => localPlaylistIds.includes(item.id) && !hasSection(item.id))
      : collections.filter((item) => normalizeProvider(item.sourceProvider ?? "") === source.id && !hasSection(item.id));
    return {
      ...source,
      imports: sourceImports,
      collections: sourceCollections.filter((item) => !importedIds.has(item.id)),
      count: sourceImports.length + sourceCollections.filter((item) => !importedIds.has(item.id)).length,
    };
  });
  });
  const importedCollectionIds = $derived(new Set([
    ...browserImports.map((item) => item.collectionId),
    ...localPlaylistIds,
  ]));
  const personalCollections = $derived(collections.filter((item) => !importedCollectionIds.has(item.id) && !hasSection(item.id) && item.id!==downloadsId));
  function hasSection(id:string){return sections.some(section=>section.id===assignments[id]);}
  function dragPlaylist(event:DragEvent,id:string){if(event.dataTransfer){event.dataTransfer.setData(PLAYLIST_SECTION_DRAG_TYPE,id);event.dataTransfer.effectAllowed="move";}}
  function normalizeProvider(provider: string) { return provider.trim().toLowerCase().replace(/[^a-z0-9_-]/g, "-"); }
  function sourceLabel(id: string) {
    if (id === "lastfm") return "Last.fm";
    return id.split(/[-_]/).filter(Boolean).map((part) => part[0]!.toUpperCase() + part.slice(1)).join(" ") || "Other";
  }
</script>

<svelte:window onkeydown={(event) => { if (event.key === "Escape" && drawerOpen) closeDrawer(); }} />

<aside class:drawer-open={drawerOpen} class:resizing class="library-sidebar" style={`--sidebar-width:${Math.max(minWidth, Math.min(maxWidth, width))}px`} aria-label="Library navigation">
  <div class="library-brand" aria-label="Burrata">
    <span class="brand-mark">b</span><span>burrata</span>
  </div>

  <button class="rail-toggle" aria-label={drawerOpen ? "Close library menu" : "Expand library menu"} aria-expanded={drawerOpen} aria-controls="library-sidebar-content" title={drawerOpen ? "Close library menu" : "Expand library menu"} onclick={toggleDrawer}>
    <span aria-hidden="true">{drawerOpen ? "×" : "☰"}</span><span class="rail-toggle-label">Menu</span>
  </button>

  <nav class="library-primary" aria-label="Main navigation">
    <button class:active={view === "search"} title="Search" aria-label="Search" onclick={() => { onNavigate("search"); closeDrawer(); }}>
      <span class="primary-icon" aria-hidden="true">⌕</span><span class="nav-label">Search</span>
    </button>
    <button class:active={view === "library" || view === "collections" || view === "favorites"} title="Library" aria-label="Library" onclick={() => { onNavigate("library"); closeDrawer(); }}>
      <span class="primary-icon" aria-hidden="true">♫</span><span class="nav-label">Library</span>
    </button>

  </nav>

  {#if drawerOpen}<button class="sidebar-scrim" aria-label="Close library menu" onclick={closeDrawer}></button>{/if}
  <div id="library-sidebar-content" class="library-scroll" aria-label="Playlists and sources">
    <div class="drawer-heading"><strong>Library navigation</strong><button aria-label="Close library menu" onclick={closeDrawer}>×</button></div>
    <details class="source-group radio-section" open>
      <summary class="source-heading"><span class="source-mark" aria-hidden="true">◉</span><span class="source-name">Radio</span><small>{radioStations.length}</small><span class="disclosure" aria-hidden="true">⌄</span></summary>
      <div class="source-playlists">
        {#each radioStations as station (station.id)}
          <button class="playlist-card" class:active={view==="radio" && activeRadioStationId===station.id} onclick={()=>{onRadioStation(station);closeDrawer();}} oncontextmenu={(event)=>{if(onRadioContext){event.preventDefault();onRadioContext(station,event);closeDrawer();}}} title={station.name}>
            <span class="playlist-glyph" aria-hidden="true">◉</span><span class="playlist-copy"><strong>{station.name}</strong><small>{station.country || "Live radio"}</small></span>
          </button>
        {/each}
        <button class="browse-source" aria-label="Add radio station" onclick={()=>{onAddRadio();closeDrawer();}}>＋ Add station</button>
      </div>
    </details>
    <section class="playlist-section">
      <div class="sidebar-section-title playlist-heading">
        <span>Playlists</span>
        <button class="icon-button" aria-label="Create playlist" title="Create playlist" onclick={() => { onImport(); closeDrawer(); }}>＋</button>
      </div>
      <div class="playlist-list built-in-playlists">
        <button class="playlist-card" class:active={activeCollectionId===downloadsId} onclick={()=>{onPlaylist(downloadsId);closeDrawer();}} oncontextmenu={(event)=>{event.preventDefault();onPlaylistContext?.(downloadsId,event);}}><span class="playlist-glyph" aria-hidden="true">↓</span><span class="playlist-copy"><strong>{playlistNames[downloadsId]??collections.find(item=>item.id===downloadsId)?.name??"Downloads"}</strong><small>{collections.find(item=>item.id===downloadsId)?.trackCount??0} tracks</small></span></button>
        {#if !favoritesPlaylistId}<button class="playlist-card" class:active={view === "favorites"} onclick={() => { onPlaylist("favorites"); closeDrawer(); }} oncontextmenu={(event)=>{event.preventDefault();onPlaylistContext?.("favorites",event);}}>
          <span class="playlist-glyph liked" aria-hidden="true">♥</span>
          <span class="playlist-copy"><strong>{playlistNames.favorites??"Liked Songs"}</strong><small>Your favorites</small></span>
        </button>{/if}
        <button class="playlist-card" onclick={() => { onPlaylist("history"); closeDrawer(); }} oncontextmenu={(event)=>{event.preventDefault();onPlaylistContext?.("history",event);}}>
          <span class="playlist-glyph history" aria-hidden="true">◷</span>
          <span class="playlist-copy"><strong>{playlistNames.history??"History"}</strong><small>Recently played</small></span>
        </button>
        {#each personalCollections as collection (collection.id)}
          <button draggable="true" ondragstart={(event)=>dragPlaylist(event,collection.id)} class="playlist-card collection-card" class:active={activeCollectionId === collection.id} onclick={() => { onPlaylist(collection.id); closeDrawer(); }} oncontextmenu={(event) => { if (onPlaylistContext) { event.preventDefault(); onPlaylistContext(collection.id, event); closeDrawer(); } }}>
            <span class="playlist-glyph collection-glyph" class:liked={collection.id===favoritesPlaylistId} aria-hidden="true">{collection.id===favoritesPlaylistId?"♥":"▤"}</span>
            <span class="playlist-copy"><strong>{collection.name}</strong><small>{collection.trackCount} {collection.trackCount === 1 ? "track" : "tracks"}</small></span>
          </button>
        {/each}
      </div>
      <PlaylistSections showUnsorted={false} {sections} {assignments} collections={collections.filter(item=>item.id!==downloadsId)} {activeCollectionId} onPlaylist={(id)=>{onPlaylist(id);closeDrawer();}} onPlaylistContext={(id,event)=>onPlaylistContext?.(id,event)} {onCreateSection} {onRenameSection} {onDeleteSection} {onMovePlaylist} {onCreatePlaylist}/>
      <div class="source-groups" aria-label="Imported playlists by source">
        {#each sourceGroups as source (source.id)}
          <details class="source-group" open={source.count > 0}>
            <summary class={`source-heading ${source.id}`}>
              <span class={`source-mark ${source.id}`} aria-hidden="true"><ProviderLogo provider={source.id}/></span>
              <span class="source-name">{source.label}</span>
              {#if source.count}<small>{source.count}</small>{/if}
              <span class="disclosure" aria-hidden="true">⌄</span>
            </summary>
            <div class="source-playlists">
              {#each source.imports as imported (imported.id)}
                {@const collection = collections.find((item) => item.id === imported.collectionId)}
                <button draggable="true" ondragstart={(event)=>dragPlaylist(event,imported.collectionId)} class="playlist-card collection-card imported-card" class:active={activeCollectionId === imported.collectionId} onclick={() => { onPlaylist(imported.collectionId); closeDrawer(); }} oncontextmenu={(event) => { if (onPlaylistContext) { event.preventDefault(); onPlaylistContext(imported.collectionId, event); closeDrawer(); } }}>
                  <span class="playlist-glyph collection-glyph" aria-hidden="true">▤</span>
                  <span class="playlist-copy"><strong>{playlistTitle(playlistNames[imported.collectionId] || imported.title || collection?.name || "Imported playlist",imported.provider)}</strong><small>{collection?.trackCount ?? 0} {(collection?.trackCount ?? 0) === 1 ? "track" : "tracks"}</small></span>
                </button>
              {/each}
              {#each source.collections as collection (collection.id)}
                <button draggable="true" ondragstart={(event)=>dragPlaylist(event,collection.id)} class="playlist-card collection-card imported-card" class:active={activeCollectionId === collection.id} onclick={() => { onPlaylist(collection.id); closeDrawer(); }} oncontextmenu={(event) => { if (onPlaylistContext) { event.preventDefault(); onPlaylistContext(collection.id, event); closeDrawer(); } }}>
                  <span class="playlist-glyph collection-glyph" aria-hidden="true">▤</span>
                  <span class="playlist-copy"><strong>{collection.name}</strong><small>{collection.trackCount} {collection.trackCount === 1 ? "track" : "tracks"}</small></span>
                </button>
              {/each}
              {#if ["beatport", "traxsource", "shazam", "lastfm", "bandcamp"].includes(source.id)}<button class="browse-source" aria-label={`Browse ${source.label} ${source.id === "bandcamp" ? "music" : "charts"}`} onclick={() => { onSource(source.id); closeDrawer(); }}>{source.id === "bandcamp" ? "Browse music" : "Browse charts"} ↗</button>{/if}
              {#if source.count === 0}<p class="source-empty">No playlists yet</p>{/if}
            </div>
          </details>
        {/each}
      </div>
    </section>
  </div>

  <footer class="library-footer">
    <button class:active={view === "downloads"} title="Downloads" aria-label="Downloads" onclick={() => { onDownloads(); closeDrawer(); }}>
      <span aria-hidden="true">↓</span><span class="nav-label">Downloads</span>
      {#if activeJobs > 0}<span class="jobs-count">{activeJobs}</span>{/if}
    </button>
    <button class:active={view === "settings"} title="Settings" aria-label="Settings" onclick={() => { onSettings(); closeDrawer(); }}>
      <span aria-hidden="true">⚙</span><span class="nav-label">Settings</span>
    </button>
  </footer>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (Focusable window splitter provides keyboard resizing.) -->
  <div tabindex="0" class="sidebar-resize" role="separator" aria-label="Resize library sidebar" aria-orientation="vertical" aria-valuemin={minWidth} aria-valuemax={maxWidth} aria-valuenow={width} onpointerdown={startResize} onkeydown={(event) => { if (event.key === "ArrowLeft") { event.preventDefault(); setWidth(width - 10); } else if (event.key === "ArrowRight") { event.preventDefault(); setWidth(width + 10); } }}></div>
</aside>

<style>
  .library-sidebar {
    position: relative;
    grid-row: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 22px 13px 13px;
    overflow: hidden;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }
  .sidebar-resize { position:absolute; z-index:3; top:0; right:-4px; width:8px; height:100%; padding:0; border:0; background:transparent; cursor:col-resize; touch-action:none; }
  .sidebar-resize:hover,.sidebar-resize:focus-visible { background:color-mix(in srgb,var(--accent) 35%,transparent); outline:0; }
  .resizing { user-select:none; }
  .library-brand {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 8px 26px;
    color: var(--text);
    text-decoration: none;
    font-size: 20px;
    font-weight: 760;
    letter-spacing: -0.8px;
  }
  .brand-mark {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 10px 10px 10px 3px;
    color: var(--base);
    background: var(--accent);
    font-size: 24px;
    font-weight: 900;
    line-height: 1;
  }
  .library-primary { display: grid; gap: 4px; }
  .library-primary button,
  .library-footer > button {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    min-height: 42px;
    padding: 0 11px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    text-align: left;
    font-size: 13px;
    font-weight: 570;
  }
  .library-primary button:hover,
  .library-footer > button:hover { background: var(--raised); color: var(--text); }
  .library-primary button.active,
  .library-footer > button.active { background: var(--raised); color: var(--accent); }
  .primary-icon { width: 20px; text-align: center; font-size: 20px; line-height: 1; }
  .library-scroll { min-height: 0; flex: 1; overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
  .sidebar-section-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 8px;
    color: var(--muted);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1.25px;
    text-transform: uppercase;
  }
  .source-mark { display: grid; place-items: center; width: 19px; height: 19px; border-radius: 5px; background: var(--raised); color: var(--text); font-size: 11px; font-weight: 750; }
  .source-mark.spotify { color: #8fdf9b; }
  .source-mark.beatport { color: #d3ee75; }
  .source-mark.traxsource { color: #f5a16d; }
  .source-mark.local { color: #95bff4; }
  .playlist-section { margin-top: 27px; }
  .icon-button { width: 26px; height: 26px; padding: 0; border: 0; background: transparent; color: var(--muted); font-size: 19px; line-height: 1; }
  .icon-button:hover { color: var(--accent); background: var(--raised); }
  .playlist-list { display: grid; gap: 3px; margin-top: 9px; }
  .source-groups { display: grid; gap: 2px; margin-top: 14px; }
  .source-group { min-width: 0; }
  .source-heading { display: flex; align-items: center; gap: 9px; min-height: 36px; padding: 4px 8px; border-radius: 7px; color: var(--muted); cursor: pointer; list-style: none; }
  .source-heading::-webkit-details-marker { display: none; }
  .source-heading:hover { background: var(--raised); color: var(--text); }
  .source-name { flex: 1; font-size: 11px; font-weight: 620; }
  .source-heading small { color: var(--muted); font-size: 9px; font-variant-numeric: tabular-nums; }
  .disclosure { color: var(--muted); font-size: 14px; transition: transform 140ms ease; }
  .source-group:not([open]) .disclosure { transform: rotate(-90deg); }
  .source-playlists { display: grid; gap: 2px; padding: 1px 0 5px 10px; }
  .imported-card { min-height: 38px; }
  .browse-source { background:transparent;color:var(--muted);border:0;text-align:left;padding:7px 10px;font-size:11px; }
  .browse-source:hover { color:var(--accent); }
  .source-empty { margin: 3px 8px 7px 29px; color: var(--muted); font-size: 10px; }
  .playlist-card { display: flex; align-items: center; gap: 10px; width: 100%; min-width: 0; padding: 6px 8px; border: 1px solid transparent; border-radius: 8px; background: transparent; color: var(--text); text-align: left; }
  .playlist-card:hover { background: var(--raised); }
  .playlist-card.active { border-color: color-mix(in srgb, var(--accent) 25%, transparent); background: color-mix(in srgb, var(--raised) 78%, var(--accent) 8%); }
  .playlist-glyph { display: grid; place-items: center; flex: 0 0 34px; width: 34px; height: 34px; border-radius: 7px; background: var(--raised); color: var(--muted); font-size: 15px; }
  .playlist-glyph.liked { background: linear-gradient(145deg, #6546a6, #b8499b); color: #fff; }
  .playlist-glyph.history { background: color-mix(in srgb, var(--accent) 16%, var(--raised)); color: var(--accent); font-size: 20px; }
  .collection-glyph { color: var(--muted); }
  .playlist-copy { display: grid; min-width: 0; gap: 3px; }
  .playlist-copy strong { overflow: hidden; color: var(--text); font-size: 11px; font-weight: 590; text-overflow: ellipsis; white-space: nowrap; }
  .playlist-copy small { color: var(--muted); font-size: 10px; }
  .library-footer { display: grid; gap: 3px; margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--line); }
  .library-footer > button { min-height: 36px; font-size: 12px; }
  .library-footer > button > span:first-child { width: 20px; text-align: center; font-size: 17px; }
  .jobs-count { display: grid; place-items: center; min-width: 19px; height: 19px; margin-left: auto; padding: 0 5px; border-radius: 10px; background: color-mix(in srgb, var(--accent) 18%, var(--raised)); color: var(--accent); font-size: 10px; }
  .rail-toggle,.sidebar-scrim { display: none; }
  .drawer-heading { display:none; }
  @media (min-width: 901px) {
    .library-sidebar { width:var(--sidebar-width,200px); }
  }
  @media (min-width: 801px) and (max-width: 1099px) {
    .library-sidebar { padding: 18px 9px 10px; }
    .library-brand { margin: 0 5px 18px; font-size: 18px; }
    .playlist-card { gap: 7px; padding: 5px 5px; }
    .playlist-glyph { flex-basis: 29px; width: 29px; height: 29px; }
    .library-primary button,.library-footer > button { gap: 8px; padding: 0 8px; }
    .source-heading { gap: 6px; padding-inline: 5px; }
    .source-playlists { padding-left: 4px; }
  }
  @media (max-width: 900px) {
    .library-sidebar { grid-row: 1; grid-column: 1; min-width: 0; min-height: 0; padding: 12px 6px; overflow: visible; position: relative; z-index: 10; }
    .sidebar-resize { display:none; }
    .library-brand { justify-content: center; margin: 0 0 12px; }
    .library-brand > span:last-child,.nav-label,.rail-toggle-label { display: none; }
    .brand-mark { width: 34px; height: 34px; }
    .rail-toggle { display: grid; place-items: center; width: 48px; height: 40px; margin: 0 auto 8px; border: 1px solid var(--line); border-radius: 9px; background: var(--raised); color: var(--text); font-size: 19px; }
    .library-primary { grid-template-columns: 1fr; gap: 5px; }
    .library-primary button { justify-content: center; width: 48px; min-height: 44px; margin: auto; padding: 0; }
    .primary-icon { width: auto; font-size: 22px; }
    .library-scroll { display: none; }
    .library-footer { gap: 5px; margin-top: auto; padding-top: 8px; }
    .library-footer > button { justify-content: center; width: 48px; min-height: 42px; margin: auto; padding: 0; }
    .library-footer > button > span:first-child { width: auto; font-size: 18px; }
    .jobs-count { position: absolute; min-width: 16px; height: 16px; margin: -29px 0 0 26px; padding: 0 3px; font-size: 9px; }
    .sidebar-scrim { display: block; position: fixed; inset: 0 0 var(--player-height, 100px) 64px; z-index: 1000; width: auto; height: auto; padding: 0; border: 0; background: rgb(0 0 0 / 48%); }
    .drawer-open .library-scroll { display: block; position: fixed; z-index: 1001; top: 0; bottom: var(--player-height, 100px); left: 64px; width: min(300px, calc(100vw - 64px)); max-height: none; padding: 18px 14px 12px; overflow-y: auto; overscroll-behavior: contain; background: var(--surface); border-right: 1px solid var(--line); box-shadow: 14px 0 34px rgb(0 0 0 / 28%); }
    .drawer-open .playlist-section { margin-top: 0; }
    .drawer-open .library-brand { display: none; }
    .drawer-heading { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; color: var(--text); font-size: 13px; }
    .drawer-heading button { width: 30px; height: 30px; border: 1px solid var(--line); border-radius: 7px; background: transparent; color: var(--text); font-size: 20px; }
  }
</style>
