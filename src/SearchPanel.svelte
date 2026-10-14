<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import type { ArtistRef, ArtistReleaseRef } from "./lib/artist-types";
  import type { Candidate, Chapter, Track } from "./lib/types";
  import { call, desktop } from "./lib/api";
  import TrackList from "./TrackList.svelte";

  export type SearchKind = "all" | "tracks" | "artists" | "albums" | "videos";
  export type SearchRow = { id: string; kind: string; sourceId: string; title: string; subtitle?: string | null; url?: string | null; track?: Track | null; data?: unknown };
  export type SearchPage = { sourceId?: string; results?: SearchRow[]; nextCursor?: string | null; complete?: boolean; progress?: string | null; warnings?: string[] };
  export type Source = { id: string; name: string; kinds?: string[]; enabled?: boolean; detail?: string };

  const DEFAULT_SOURCES: Source[] = [
    { id: "library", name: "My library" }, { id: "musicbrainz", name: "MusicBrainz" }, { id: "discogs", name: "Discogs" },
    { id: "youtube", name: "YouTube" }, { id: "soundcloud", name: "SoundCloud" }, { id: "soulseek", name: "Soulseek" },
  ];
  const ONLINE_SOURCE_IDS = ["musicbrainz", "discogs", "youtube", "soundcloud", "soulseek"];
  const PREF_KEY = "burrata.discovery.search-scope.v2";
  const OLD_PREF_KEY = "burrata.discovery.sources.v1";
  const kinds: { id: SearchKind; label: string }[] = [
    { id: "all", label: "All" }, { id: "tracks", label: "Tracks" }, { id: "artists", label: "Artists" }, { id: "albums", label: "Albums" }, { id: "videos", label: "Videos / Sets" },
  ];
  let {
    query: initialQuery = "", initialSources, availableSources, sourceCapabilities = {}, searchSource, enrichSource, enrichTrack, searchPreset,
    onArtist = () => {}, onArtistTrack = () => {}, onAlbum = () => {}, onQueryChange = () => {}, onPlay = () => {}, onDownload = () => {}, onFavorite, onNext, onQueue, onContext,
    onPlayChapter = () => {}, onQueueChapter = () => {}, onDownloadChapter = () => {}, onInfoChapter = () => {},
    playingTrackId = "", bufferingTrackId = "", paused = false,
  }: {
    query?: string; initialSources?: string[]; availableSources?: { id: string; name: string; kinds?: string[]; enabled?: boolean; detail?: string }[]; sourceCapabilities?: Record<string, boolean | string>;
    searchSource?: (request: { query: string; sourceIds: string[]; kinds: string[]; cursor?: string | null; limit: number }) => Promise<SearchPage>;
    enrichSource?: (url: string) => Promise<Candidate>;
    enrichTrack?: (track: Track) => Promise<Track>;
    onArtist?: (artist: ArtistRef) => void; onArtistTrack?: (track: Track) => void; onAlbum?: (album: ArtistReleaseRef) => void;
    onQueryChange?: (query: string) => void;
    onPlay?: (track: Track) => void; onDownload?: (track: Track) => void; onFavorite?: (track: Track) => void; onNext?: (track: Track) => void; onQueue?: (track: Track) => void; onContext?: (track: Track, event: MouseEvent) => void;
    onPlayChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    onQueueChapter?: (parent: Track, candidate: Candidate, chapter: Chapter, mode: "next" | "append") => void;
    onDownloadChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    onInfoChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
    searchPreset?: { sourceId: string; kind: SearchKind; requestId: number };
  } = $props();

  let text = $state("");
  let submitted = $state("");
  let kind = $state<SearchKind>("all");
  const sources = $derived(availableSources?.length ? availableSources : DEFAULT_SOURCES);
  let searchMode = $state<"online" | "library">("online");
  let sourceChoice = $state("all");
  let rowsBySource = $state<Record<string, SearchRow[]>>({});
  let statuses = $state<Record<string, string>>({});
  let errors = $state<Record<string, string>>({});
  let cursors = $state<Record<string, string | null>>({});
  let warnings = $state<Record<string, string[]>>({});
  let expanded = $state<Record<string, boolean>>({});
  let chaptersByRow = $state<Record<string, Candidate | "loading" | "none" | "unavailable" | "unknown">>({});
  let generation = 0;
  let soulseekSessions: Record<string, string> = {};
  let enrichmentQueue: { sourceId: string; row: SearchRow; request: number }[] = [];
  let enrichmentRunning = 0;
  let enrichmentStarted = new Set<string>();
  let lastQueryProp = "";
  let initialized = $state(false);
  let lastPresetRequestId = 0;
  let presetRequestPending = $state(false);

  $effect(() => {
    const initial = initialSources;
    if (!initialized) {
      initialized = true;
      text = initialQuery;
      lastQueryProp = initialQuery;
      const available = new Set(sources.map(source => source.id));
      const validOnline = (ids: unknown) => Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string" && ONLINE_SOURCE_IDS.includes(id) && available.has(id) && sourceEnabled(id)) : [];
      if (initial) {
        const initialOnline = validOnline(initial);
        if (initial.includes("library") && !initialOnline.length) searchMode = "library";
        else if (initialOnline.length) { searchMode = "online"; sourceChoice = initialOnline[0]!; }
      } else {
        let restored = false;
        try {
          const saved = JSON.parse(localStorage.getItem(PREF_KEY) ?? "null") as { mode?: unknown; sourceId?: unknown } | null;
          if (saved && (saved.mode === "online" || saved.mode === "library")) {
            if (saved.mode === "library" && available.has("library") && sourceEnabled("library")) { searchMode = "library"; restored = true; }
            else if (saved.mode === "online" && (saved.sourceId === "all" || (typeof saved.sourceId === "string" && validOnline([saved.sourceId]).length))) {
              searchMode = "online"; sourceChoice = saved.sourceId as string; restored = true;
            }
          }
        } catch { /* fall back to the older source selection or online default */ }
        if (!restored) {
          let old: unknown = [];
          try { old = JSON.parse(localStorage.getItem(OLD_PREF_KEY) ?? "[]"); } catch { old = []; }
          const migratedOnline = validOnline(old);
          if (Array.isArray(old) && old.includes("library") && !migratedOnline.length) searchMode = "library";
          else { searchMode = "online"; sourceChoice = "all"; }
        }
      }
      if (searchMode === "library" && (!available.has("library") || !sourceEnabled("library"))) searchMode = "online";
      if (sourceChoice !== "all" && !validOnline([sourceChoice]).length) sourceChoice = "all";
    }
    if (initialQuery !== lastQueryProp) { lastQueryProp = initialQuery; text = initialQuery; }
  });
  $effect(() => {
    if (!initialized) return;
    try { localStorage.setItem(PREF_KEY, JSON.stringify({ mode: searchMode, sourceId: sourceChoice })); } catch { /* storage can be unavailable in private contexts */ }
  });
  $effect(() => {
    const preset = searchPreset;
    if (!initialized || !preset || preset.requestId === 0 || preset.requestId === lastPresetRequestId) return;
    lastPresetRequestId = preset.requestId;
    if (!sourceEnabled(preset.sourceId)) return;
    presetRequestPending = true;
    if (preset.sourceId === "library") searchMode = "library";
    else { searchMode = "online"; sourceChoice = preset.sourceId; }
    kind = preset.kind;
    void tick().then(() => {
      if (lastPresetRequestId !== preset.requestId) return;
      submitted = "";
      presetRequestPending = false;
      if (text.trim()) void runSearch();
    });
  });
  onDestroy(() => { generation++; void cancelSoulseek(); });
  onMount(async () => {
    if (!initialQuery.trim()) return;
    await tick();
    if (!submitted && text.trim() && !presetRequestPending) void runSearch();
  });

  function sourceEnabled(id: string) { const source = sources.find(item => item.id === id); return Boolean(source) && source?.enabled !== false && sourceCapabilities[id] !== false && sourceCapabilities[id] !== "disconnected"; }
  function sourceSupports(id: string) {
    const supported = sources.find(item => item.id === id)?.kinds;
    return !supported?.length || requestedKinds().some(value => supported.includes(value));
  }
  const enabledOnlineSources = $derived(sources.filter(source => ONLINE_SOURCE_IDS.includes(source.id) && sourceEnabled(source.id)));
  const selectedSource = $derived(searchMode === "library" ? sources.find(source => source.id === "library" && sourceEnabled(source.id)) : sourceChoice === "all" ? null : enabledOnlineSources.find(source => source.id === sourceChoice) ?? null);
  const visibleKinds = $derived(searchMode === "library" ? kinds.filter(item => item.id === "all" || item.id === "tracks") : kinds);
  const selectedSourceUnsupported = $derived(searchMode === "online" && sourceChoice !== "all" && selectedSource && !sourceSupports(selectedSource.id) ? selectedSource : null);
  const displayedSources = $derived(searchMode === "library"
    ? sources.filter(source => source.id === "library" && sourceEnabled(source.id))
    : (sourceChoice === "all" ? enabledOnlineSources : enabledOnlineSources.filter(source => source.id === sourceChoice)).filter(source => sourceSupports(source.id)));
  function activeSourceIds() {
    if (searchMode === "library") return sourceEnabled("library") ? ["library"] : [];
    return (sourceChoice === "all" ? enabledOnlineSources.map(source => source.id) : [sourceChoice]).filter(id => sourceEnabled(id));
  }
  function changeMode(mode: "online" | "library") {
    if (searchMode === mode) return;
    searchMode = mode;
    if (mode === "library" && !["all", "tracks"].includes(kind)) kind = "tracks";
    if (mode === "online" && !enabledOnlineSources.length) return;
    void restartSearch();
  }
  function changeSource(sourceId: string) {
    if (sourceChoice === sourceId) return;
    sourceChoice = sourceId;
    void restartSearch();
  }
  async function restartSearch() {
    void cancelSoulseek();
    if (submitted.trim()) await runSearch();
    else { generation++; rowsBySource = {}; statuses = {}; errors = {}; cursors = {}; warnings = {}; }
  }
  function requestedKinds(): string[] {
    if (kind === "all") return ["track", "artist", "album", "video", "set"];
    if (kind === "videos") return ["video", "set"];
    return [kind.slice(0, -1)];
  }
  function identity(row: SearchRow) { return `${row.sourceId}:${row.kind}:${row.id}`; }
  function mergeRows(sourceId: string, incoming: SearchRow[], append: boolean) {
    const merged = append ? [...(rowsBySource[sourceId] ?? []), ...incoming] : incoming;
    const unique = new Map<string, SearchRow>();
    for (const row of merged) unique.set(identity(row), row);
    rowsBySource = { ...rowsBySource, [sourceId]: [...unique.values()] };
  }
  async function requestSource(query: string, sourceId: string, cursor?: string | null): Promise<SearchPage> {
    if (searchSource) return searchSource({ query, sourceIds: [sourceId], kinds: requestedKinds(), cursor, limit: 30 });
    return call<SearchPage>("search_source", { query, sourceIds: [sourceId], kinds: requestedKinds(), cursor: cursor ?? null, limit: 30 });
  }
  async function pollSource(sourceId: string, searchId: string): Promise<SearchPage> {
    return call<SearchPage>("poll_search_source", { searchId, sourceId });
  }
  async function cancelSoulseek(sourceId = "soulseek") {
    const id = soulseekSessions[sourceId];
    if (!id) return;
    soulseekSessions = { ...soulseekSessions, [sourceId]: "" };
    await call("cancel_slskd_search", { searchId: id }).catch(() => {});
  }
  async function runSearch(appendSource?: string) {
    const term = (appendSource ? submitted : text).trim();
    if (!term) return;
    if (!appendSource) {
      void cancelSoulseek();
      generation++;
      submitted = term;
      rowsBySource = {}; statuses = {}; errors = {}; cursors = {}; warnings = {}; expanded = {}; chaptersByRow = {};
      enrichmentQueue = []; enrichmentStarted = new Set();
    }
    const request = generation;
    const activeIds = activeSourceIds();
    const sourceIds = appendSource ? [appendSource] : activeIds.filter(id => sourceSupports(id));
    for (const sourceId of activeIds.filter(id => !sourceSupports(id))) statuses = { ...statuses, [sourceId]: "Not available for this result type" };
    if (!sourceIds.length) return;
    await Promise.all(sourceIds.map(async sourceId => {
      statuses = { ...statuses, [sourceId]: appendSource ? "Loading more…" : "Searching…" };
      errors = { ...errors, [sourceId]: "" };
      try {
        let page = await requestSource(term, sourceId, appendSource ? cursors[sourceId] : null);
        if (request !== generation) return;
        let append = Boolean(appendSource);
        while (true) {
          mergeRows(sourceId, page.results ?? [], append);
          queueVisibleEnrichment(sourceId, request);
          append = true;
          cursors = { ...cursors, [sourceId]: page.nextCursor ?? null };
          warnings = { ...warnings, [sourceId]: page.warnings ?? [] };
          if (sourceId !== "soulseek" || page.complete !== false || !page.nextCursor) break;
          soulseekSessions = { ...soulseekSessions, [sourceId]: page.nextCursor };
          statuses = { ...statuses, [sourceId]: page.progress || "Searching…" };
          await new Promise(resolve => setTimeout(resolve, 900));
          if (request !== generation) { await cancelSoulseek(sourceId); return; }
          page = await pollSource(sourceId, page.nextCursor);
          if (request !== generation) { await cancelSoulseek(sourceId); return; }
        }
        if (sourceId === "soulseek") soulseekSessions = { ...soulseekSessions, [sourceId]: "" };
        statuses = { ...statuses, [sourceId]: page.complete === false ? (page.progress || "More results available") : "Complete" };
      } catch (error) {
        if (request !== generation) return;
        errors = { ...errors, [sourceId]: error instanceof Error ? error.message : String(error) };
        statuses = { ...statuses, [sourceId]: "Unavailable" };
      }
    }));
  }
  function cancel() { generation++; void cancelSoulseek(); statuses = Object.fromEntries(Object.keys(statuses).map(id => [id, "Cancelled"])); }
  function rowArtist(row: SearchRow): ArtistRef | null {
    const data = row.data as Record<string, unknown> | null;
    const ref = (data?.artist ?? data?.ref ?? data) as ArtistRef | undefined;
    return ref && (ref.provider === "musicbrainz" || ref.provider === "discogs") && typeof ref.id === "string" ? ref : null;
  }
  function rowAlbum(row: SearchRow): ArtistReleaseRef | null {
    const data = row.data as ArtistReleaseRef | null;
    return data && data.release && data.sourceUrl ? data : null;
  }
  function detailLines(track: Track) {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const metadata = provenance.sourceMetadata && typeof provenance.sourceMetadata === "object" ? provenance.sourceMetadata as Record<string, unknown> : {};
    const genreValues = metadata.sourceGenres ?? metadata.genres;
    const genres = Array.isArray(genreValues) ? genreValues.filter(Boolean).join(", ") : genreValues;
    const releaseDate = track.releaseDate ? `Released ${track.releaseDate}` : null;
    const uploadDate = metadata.uploadDate ? `Uploaded ${metadata.uploadDate}` : null;
    return [track.album, track.label, track.genre ?? (typeof genres === "string" ? genres : null), releaseDate, metadata.description, metadata.creator ?? metadata.channel, uploadDate, track.durationSeconds ? `${Math.floor(track.durationSeconds / 60)}:${String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}` : null].filter(Boolean);
  }
  function mergedEnrichment(original: Track, updated: Track): Track {
    const merged = { ...original, ...updated } as Track;
    for (const key of ["artist", "title", "version", "album", "label", "genre", "normalizedGenre", "releaseDate", "durationSeconds", "bpm", "key", "artworkUrl", "isrc", "catalogNumber"] as const) {
      const value = updated[key];
      if (value == null || value === "") (merged as unknown as Record<string, unknown>)[key] = original[key];
    }
    merged.provenance = { ...(original.provenance && typeof original.provenance === "object" ? original.provenance as Record<string, unknown> : {}), ...(updated.provenance && typeof updated.provenance === "object" ? updated.provenance as Record<string, unknown> : {}) };
    return merged;
  }
  function queueVisibleEnrichment(sourceId: string, request: number) {
    for (const row of (rowsBySource[sourceId] ?? []).slice(0, 20)) {
      if (!row.track) continue;
      const key = `${request}:${identity(row)}`;
      if (enrichmentStarted.has(key)) continue;
      enrichmentStarted.add(key);
      enrichmentQueue.push({ sourceId, row, request });
    }
    void pumpEnrichment();
  }
  async function pumpEnrichment() {
    if (enrichmentRunning >= 2) return;
    const task = enrichmentQueue.shift();
    if (!task) return;
    enrichmentRunning++;
    try {
      const enriched = enrichTrack ? await enrichTrack(task.row.track!) : await call<Track>("enrich_search_result", { track: task.row.track });
      if (task.request === generation && enriched) {
        const rows = rowsBySource[task.sourceId] ?? [];
        const key = identity(task.row);
        rowsBySource = { ...rowsBySource, [task.sourceId]: rows.map(row => identity(row) === key && row.track ? { ...row, track: mergedEnrichment(row.track, enriched) } : row) };
      }
    } catch { /* enrichment is supplemental; search results stay usable */ }
    finally { enrichmentRunning--; void pumpEnrichment(); }
  }
  function knownCandidate(track: Track): Candidate | null {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const audioCandidate = provenance.audioCandidate ?? provenance.candidate;
    return audioCandidate && typeof audioCandidate === "object" && Array.isArray((audioCandidate as Candidate).chapters) ? audioCandidate as Candidate : null;
  }
  function chapterState(row: SearchRow): Candidate | "loading" | "none" | "unavailable" | "unknown" | undefined {
    return chaptersByRow[identity(row)] ?? (row.track ? knownCandidate(row.track) ?? undefined : undefined);
  }
  async function inspectChapters(row: SearchRow) {
    const id = identity(row);
    if (chaptersByRow[id] === "loading" || !row.url || !enrichSource && !desktop()) return;
    chaptersByRow = { ...chaptersByRow, [id]: "loading" };
    try {
      const candidate = enrichSource ? await enrichSource(row.url) : await call<Candidate>("inspect_youtube_source", { url: row.url });
      chaptersByRow = { ...chaptersByRow, [id]: Array.isArray(candidate.chapters) ? (candidate.chapters.length ? candidate : "none") : "unknown" };
    } catch { chaptersByRow = { ...chaptersByRow, [id]: "unavailable" }; }
  }
</script>

<section class="search-panel" aria-label="Discovery search">
  <header class="heading"><div><p class="eyebrow">DISCOVERY</p><h2>Search music</h2></div><span class="source-count">{searchMode === "library" ? "My library" : sourceChoice === "all" ? "All online sources" : selectedSource?.name ?? "All online sources"}</span></header>
  <form class="search-form" onsubmit={(event) => { event.preventDefault(); void runSearch(); }}>
    <input aria-label="Search music" bind:value={text} oninput={(event) => { text = event.currentTarget.value; onQueryChange(text); }} placeholder="Track, artist, album, video or set" />
    <button class="primary" type="submit" disabled={!text.trim() || !activeSourceIds().length}>Search</button>
    {#if Object.values(statuses).some(status => status === "Searching…" || status === "Loading more…" || status.endsWith("…"))}<button type="button" onclick={cancel}>Cancel</button>{/if}
  </form>
  <div class="filters" aria-label="Result type">
    {#each visibleKinds as item}<button class:active={kind === item.id} aria-pressed={kind === item.id} onclick={() => { kind = item.id; if (submitted) void runSearch(); }}>{item.label}</button>{/each}
  </div>
  <div class="scope-controls">
    <div class="search-mode" role="group" aria-label="Search scope">
      <button aria-label="Online" aria-pressed={searchMode === "online"} class:active={searchMode === "online"} disabled={!enabledOnlineSources.length} onclick={() => changeMode("online")}>Online</button>
      <button aria-label="My library" aria-pressed={searchMode === "library"} class:active={searchMode === "library"} disabled={!sourceEnabled("library")} onclick={() => changeMode("library")}>My library</button>
    </div>
    {#if searchMode === "library"}
      <p class="library-helper">Saved tracks, imports and downloads</p>
    {:else}
      <label class="search-in-label" for="discovery-source-select">Search in</label>
      <select id="discovery-source-select" aria-label="Search in" value={sourceChoice} onchange={(event) => changeSource(event.currentTarget.value)}>
        <option value="all">All online sources</option>
        {#each enabledOnlineSources as source (source.id)}
          <option value={source.id} disabled={!sourceSupports(source.id)}>{source.name}{sourceSupports(source.id) ? "" : " — not available for this result type"}</option>
        {/each}
      </select>
    {/if}
  </div>
  {#if selectedSourceUnsupported}<p class="scope-hint" role="status">{selectedSourceUnsupported.name} does not support {kinds.find(item => item.id === kind)?.label.toLocaleLowerCase()}. Choose another source in Search in or change the result type.</p>{/if}

  <div class="source-results" aria-live="polite">
    {#each displayedSources as source (source.id)}
      {@const rows = rowsBySource[source.id] ?? []}
      <section class="source-group" aria-label={`${source.name} results`}>
        <header><h3>{source.name}</h3><span>{statuses[source.id] ?? "Ready"}</span></header>
        {#if errors[source.id]}<p class="notice error">{errors[source.id]} <button onclick={() => void runSearch(source.id)}>Retry</button></p>{/if}
        {#each warnings[source.id] ?? [] as warning}<p class="notice">{warning}</p>{/each}
        {#if rows.length}
          {#each rows as row (identity(row))}
            {@const track = row.track}
            <article class="result-row" class:expanded={expanded[identity(row)]} oncontextmenu={track && onContext ? (event) => { event.preventDefault(); onContext?.(track, event); } : undefined}>
              <div class="result-main">
                {#if !track}<div class="result-title"><span class="kind-badge">{row.kind === "video" || row.kind === "set" ? "Video / set" : row.kind}</span><strong>{row.title}</strong></div>{/if}
                {#if row.subtitle}<p class="subtitle">{row.subtitle}</p>{/if}
                {#if track}
                  <TrackList tracks={[track]} listLabel={`${source.name} track result`} onPlay={(item) => onPlay(item)} onDownload={onDownload} onFavorite={onFavorite} onNext={onNext} onQueue={onQueue} onArtist={onArtistTrack} onContext={onContext} {playingTrackId} {bufferingTrackId} {paused} />
                  <div class="badges"><span>{source.name}</span>{#if track.localAvailability === "available"}<span>In library</span>{/if}{#if track.releaseDate}<span>{track.releaseDate}</span>{/if}{#if track.durationSeconds}<span>{Math.floor(track.durationSeconds / 60)}:{String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}</span>{/if}</div>
                  {#if row.url && (row.kind === "video" || row.kind === "set" || source.id === "youtube" || source.id === "soundcloud")}
                    {@const cachedChapterState = chapterState(row)}
                    {#if cachedChapterState === "loading"}<span class="chapter-badge loading">Chapters checking…</span>
                    {:else if cachedChapterState === "unavailable"}<button class="chapter-badge unavailable" onclick={() => void inspectChapters(row)}>Chapter lookup unavailable · Retry</button>
                    {:else if cachedChapterState === "unknown"}<button class="chapter-badge unknown" onclick={() => void inspectChapters(row)}>Chapters unknown · Check</button>
                    {:else if cachedChapterState === "none"}<span class="chapter-badge">No chapters</span>
                    {:else if cachedChapterState}
                      <details class="chapter-details"><summary class="chapter-badge available">Chapters available · {cachedChapterState.chapters?.length ?? 0}</summary>
                      <div class="chapter-list">
                        {#each cachedChapterState.chapters ?? [] as chapter (chapter.id)}<div class="chapter-row"><span>{Math.floor(chapter.startTime / 60)}:{String(Math.floor(chapter.startTime % 60)).padStart(2, "0")} · {chapter.suggestedTrack ? `${chapter.suggestedTrack.artist} — ${chapter.suggestedTrack.title}` : chapter.title}</span><button onclick={() => onPlayChapter(track, cachedChapterState, chapter)}>Play</button><button onclick={() => onQueueChapter(track, cachedChapterState, chapter, "next")}>Next</button><button onclick={() => onQueueChapter(track, cachedChapterState, chapter, "append")}>Queue</button><button onclick={() => onDownloadChapter(track, cachedChapterState, chapter)}>Download</button><button onclick={() => onInfoChapter(track, cachedChapterState, chapter)}>More info</button></div>{/each}
                      </div>
                      </details>
                    {:else}<button class="chapter-badge unknown" onclick={() => void inspectChapters(row)}>Chapters unknown · Check</button>{/if}
                  {/if}
                  <button class="details-toggle" aria-expanded={Boolean(expanded[identity(row)])} onclick={() => expanded = { ...expanded, [identity(row)]: !expanded[identity(row)] }}>{expanded[identity(row)] ? "Hide details" : "Details"}</button>
                  {#if expanded[identity(row)]}<p class="details">{detailLines(track).join(" · ")}</p>{/if}
                {:else if row.kind === "artist"}
                  {#if rowArtist(row)}<button class="open-result" onclick={() => onArtist(rowArtist(row)!)}>{row.title} <span>Open artist →</span></button>{/if}
                {:else if row.kind === "album"}
                  {#if rowAlbum(row)}<button class="open-result" onclick={() => onAlbum(rowAlbum(row)!)}>{row.title} <span>Open album →</span></button>{:else}<p>{row.title}</p>{/if}
                  {#if row.url}<a href={row.url} target="_blank" rel="noreferrer">Open source ↗</a>{/if}
                {:else}
                  <button class="details-toggle" aria-expanded={Boolean(expanded[identity(row)])} onclick={() => expanded = { ...expanded, [identity(row)]: !expanded[identity(row)] }}>{expanded[identity(row)] ? "Hide details" : "Details"}</button>
                  {#if expanded[identity(row)]}
                    {@const metadata = row.data && typeof row.data === "object" ? row.data as Record<string, unknown> : {}}
                    <p class="details">{[metadata.description ?? row.subtitle, metadata.creator ?? metadata.uploader, metadata.uploadDate ? `Uploaded ${metadata.uploadDate}` : null, metadata.releaseDate ? `Released ${metadata.releaseDate}` : null].filter(Boolean).join(" · ")}</p>
                  {/if}
                  {#if row.url}<a href={row.url} target="_blank" rel="noreferrer">Open source ↗</a>{/if}
                {/if}
              </div>
            </article>
          {/each}
        {:else if statuses[source.id] === "Complete"}<p class="empty">No results from {source.name}.</p>
        {:else if statuses[source.id] === "Not available for this result type"}<p class="empty">{source.name} does not support this result type. Choose another source or result type.</p>{/if}
        {#if cursors[source.id]}<button class="load-more" disabled={statuses[source.id] === "Loading more…"} onclick={() => void runSearch(source.id)}>Load more</button>{/if}
      </section>
    {/each}
  </div>
</section>

<style>
  .search-panel{display:flex;flex-direction:column;gap:.85rem;min-width:0;min-height:0;height:100%;overflow:auto;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px}
  .heading,.search-form,.source-group>header{display:flex;align-items:center;justify-content:space-between;gap:.75rem}.heading h2{margin:0;font-size:1.35rem}.eyebrow{margin:0 0 .2rem;color:var(--muted,#a3a8ae);font-size:.7rem;font-weight:700;letter-spacing:.1em}.source-count,.source-group>header span,.subtitle,.empty,.details{color:var(--muted,#a3a8ae);font-size:.82rem}
  button,input,select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}button{cursor:pointer}button:disabled,select:disabled{opacity:.55;cursor:default}.primary{background:var(--accent,#4c78c8);border-color:var(--accent,#4c78c8);color:var(--base,#fff)}.search-form input{flex:1;min-width:0}.filters{display:flex;gap:.4rem;flex-wrap:wrap}.filters button{font-size:.84rem}.filters button.active{border-color:var(--accent,#9dbfff);color:var(--accent,#9dbfff)}
  .scope-controls{display:grid;grid-template-columns:auto auto minmax(0,1fr);align-items:center;gap:.55rem;min-width:0}.search-mode{display:flex;min-width:0;padding:3px;gap:2px;border:1px solid var(--border,#454a50);border-radius:9px;background:var(--surface,#202328)}.search-mode button{min-width:0;padding:.4rem .65rem;border:0;background:transparent;font-size:.82rem;white-space:nowrap}.search-mode button.active{background:var(--accent,#4c78c8);color:var(--base,#fff)}.search-in-label{font-size:.8rem;color:var(--muted,#a3a8ae);white-space:nowrap}.scope-controls select{width:100%;min-width:0;max-width:100%;overflow:hidden;text-overflow:ellipsis;font-size:.84rem}.scope-controls select option{background:var(--panel,#17191c);color:var(--text,#eee)}.library-helper{grid-column:2/-1;min-width:0;margin:0;color:var(--muted,#a3a8ae);font-size:.76rem;line-height:1.35}.scope-hint{margin:0;color:var(--muted,#a3a8ae);font-size:.8rem;line-height:1.4}
  .source-results{display:flex;flex-direction:column;gap:.9rem}.source-group{border:1px solid var(--border,#383d43);border-radius:10px;padding:.65rem}.source-group>header{padding:0 .15rem .45rem}.source-group h3{margin:0;font-size:.93rem}.result-row{border-top:1px solid var(--border,#30343a);padding:.7rem .15rem}.result-title{display:flex;align-items:center;gap:.5rem}.result-title strong{font-size:.92rem}.kind-badge,.badges span{display:inline-flex;border-radius:999px;background:var(--surface,#202328);padding:.18rem .48rem;color:var(--muted,#a3a8ae);font-size:.69rem}.subtitle{margin:.28rem 0}.badges{display:flex;flex-wrap:wrap;align-items:center;gap:.35rem;margin-top:.45rem}.open-result{border:0;padding:0;background:none;text-align:left;color:inherit;font-weight:600}.open-result span{color:var(--accent,#9dbfff);font-size:.78rem;margin-left:.3rem}.result-main>a{display:inline-block;margin-top:.45rem;color:var(--accent,#9dbfff);font-size:.78rem}.details-toggle{margin-top:.4rem;padding:.3rem .45rem;font-size:.75rem}.details{margin:.35rem 0}.chapter-details{margin-top:.4rem}.chapter-badge{display:inline-flex;margin-top:.4rem;border-radius:999px;background:var(--surface,#202328);padding:.2rem .55rem;color:var(--muted,#a3a8ae);font-size:.72rem}.chapter-badge.available{color:var(--accent,#9dbfff);cursor:pointer}.chapter-badge.unknown,.chapter-badge.unavailable{border:0;cursor:pointer}.chapter-badge.loading{color:var(--muted,#a3a8ae)}.chapter-list{margin-top:.55rem;padding:.45rem;border-left:2px solid var(--accent,#9dbfff)}.chapter-row{display:flex;align-items:center;gap:.3rem;margin-top:.35rem}.chapter-row span{flex:1;font-size:.78rem}.chapter-row button{font-size:.76rem;padding:.35rem .5rem}.notice{margin:.35rem 0;font-size:.8rem;color:var(--muted,#a3a8ae)}.error{color:#ff9b9b}.load-more{margin:.4rem}.empty{padding:.25rem}.source-group a{color:var(--accent,#9dbfff)}
  @media(max-width:640px){.search-panel{padding:.7rem}.search-form{flex-wrap:wrap}.search-form input{flex-basis:100%}.scope-controls{grid-template-columns:auto minmax(0,1fr)}.search-mode{grid-column:1/-1;justify-self:start}.library-helper{grid-column:1/-1}.search-mode button{padding:.4rem .5rem}.chapter-row{flex-wrap:wrap}.chapter-row span{flex-basis:100%}}
</style>
