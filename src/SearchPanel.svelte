<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import type { ArtistRef, ArtistReleaseRef, ArtistSearchResult } from "./lib/artist-types";
  import type { Candidate, Chapter, Track } from "./lib/types";
  import { validChapterRange } from "./lib/chapters";
  import { call, desktop } from "./lib/api";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";
  import { trackSortAvailabilityForSources, trackSortOptionLabelForSources } from "./lib/track-sort-availability";
  import { safeArtworkUrl } from "./lib/artwork-url";
  import TrackList from "./TrackList.svelte";
  import VirtualResults from "./VirtualResults.svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";
  import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";
  import { searchNavigationScopes, publicSearchNavigationScope } from "./lib/search-navigation-scope";
  import { takeCollectionRows } from "./lib/search-collection";
  import { searchUiControlExpired, searchUiControlFingerprint } from "./lib/search-ui-control-revision";
  import { addSearchHistoryEntry, readSearchHistory, removeSearchHistoryEntry, writeSearchHistory, type SearchHistoryEntry } from "./lib/search-history";
  import type { AutomationSearchErrorCode, AutomationSearchKind, AutomationSearchReport, AutomationSearchRequest, AutomationSearchSourceId } from "./lib/search-ui-automation";

  export type SearchKind = "all" | "tracks" | "artists" | "albums" | "videos" | "playlists" | "profiles" | "channels";
  export type SearchRow = { id: string; kind: string; sourceId: string; title: string; subtitle?: string | null; url?: string | null; artworkUrl?: string | null; track?: Track | null; data?: unknown };
  export type SearchPage = { sourceId?: string; results?: SearchRow[]; nextCursor?: string | null; complete?: boolean; progress?: string | null; warnings?: string[] };
  export type Source = { id: string; name: string; kinds?: string[]; enabled?: boolean; detail?: string };

  const DEFAULT_SOURCES: Source[] = [
    { id: "library", name: "My library" }, { id: "musicbrainz", name: "MusicBrainz" }, { id: "discogs", name: "Discogs" },
    { id: "youtube", name: "YouTube" }, { id: "soundcloud", name: "SoundCloud" }, { id: "soulseek", name: "Soulseek" }, { id: "bandcamp", name: "Bandcamp", kinds: ["track"] },
  ];
  const ONLINE_SOURCE_IDS = ["spotify", "musicbrainz", "discogs", "youtube", "soundcloud", "soulseek", "monochrome", "bandcamp"];
  const PREF_KEY = "burrata.discovery.search-scope.v2";
  const OLD_PREF_KEY = "burrata.discovery.sources.v1";
  const CONTINUE_PREF_KEY = "burrata.discovery.continue-search.v1";
  const kinds: { id: SearchKind; label: string }[] = [
    { id: "all", label: "All" }, { id: "tracks", label: "Tracks" }, { id: "artists", label: "Artists" }, { id: "albums", label: "Albums" }, { id: "videos", label: "Videos / Sets" }, { id:"playlists",label:"Playlists" }, { id:"profiles",label:"Profiles" }, { id:"channels",label:"Channels" },
  ];
  let {
    query: initialQuery = "", navigationSnapshotKey = null, initialSources, availableSources, sourceCapabilities = {}, searchSource, enrichSource, enrichTrack, searchPreset,
    onRemoteResult = () => {}, onAddRemotePlaylist, onPlaylistTracks, onQueueTracks, onArtist = () => {}, searchArtists, onArtistTrack = () => {}, onAlbum = () => {}, onTrackAlbum, onQueryChange = () => {}, onPlay = () => {}, onDownload = () => {}, onFavorite, onNext, onQueue, onContext,
    onPlayChapter = () => {}, onQueueChapter = () => {}, onDownloadChapter = () => {}, onInfoChapter = () => {},
    playingTrack = null, bufferingTrack = null, playingTrackId = "", bufferingTrackId = "", paused = false,
    automationSearch = null, automationSearchCancelled = null, onAutomationSearchComplete = () => {},
  }: {
    navigationSnapshotKey?: string | null;
    query?: string; initialSources?: string[]; availableSources?: { id: string; name: string; kinds?: string[]; enabled?: boolean; detail?: string }[]; sourceCapabilities?: Record<string, boolean | string>;
    searchSource?: (request: { query: string; sourceIds: string[]; kinds: string[]; cursor?: string | null; limit: number }) => Promise<SearchPage>;
    enrichSource?: (url: string) => Promise<Candidate>;
    enrichTrack?: (track: Track) => Promise<Track>;
    onArtist?: (artist: ArtistRef) => void; searchArtists?: (query: string) => Promise<ArtistSearchResult[]>; onArtistTrack?: (track: Track) => void; onAlbum?: (album: ArtistReleaseRef) => void; onTrackAlbum?: (track: Track) => void | Promise<void>;
    onRemoteResult?: (row: SearchRow) => void;
    onAddRemotePlaylist?: (row: SearchRow, options: { signal: AbortSignal; onProgress: (message: string) => void }) => Promise<{ partial?: boolean; cancelled?: boolean } | void> | void;
    onPlaylistTracks?: (tracks: Track[]) => Promise<void> | void;
    onQueueTracks?: (tracks: Track[], mode: "append" | "next") => void;
    onQueryChange?: (query: string) => void;
    onPlay?: (track: Track, displayedTracks?: Track[], index?: number) => void; onDownload?: (track: Track) => void; onFavorite?: (track: Track) => void; onNext?: (track: Track) => void; onQueue?: (track: Track) => void; onContext?: (track: Track, event: MouseEvent) => void;
    onPlayChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    onQueueChapter?: (parent: Track, candidate: Candidate, chapter: Chapter, mode: "next" | "append") => void;
    onDownloadChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    onInfoChapter?: (parent: Track, candidate: Candidate, chapter: Chapter) => void;
    playingTrack?: Track | null; bufferingTrack?: Track | null; playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
    automationSearch?: AutomationSearchRequest | null;
    automationSearchCancelled?: string | null;
    onAutomationSearchComplete?: (report: AutomationSearchReport) => void | Promise<void>;
    searchPreset?: { sourceId: string; kind: SearchKind; requestId: number };
  } = $props();
  const providerReads = new ProviderReadCancellation(call);

  let navigationReady = $state(false);
  let navigationDestroyed = false;
  let navigationRestoreEpoch = 0;
  let navigationAccountScopes: Record<string,string|null> = {};
  let navigationNotice = $state("");
  let searchElement: HTMLElement | null = null;
  let text = $state("");
  let artistSuggestions = $state<ArtistSearchResult[]>([]);
  let artistSuggestionsOpen = $state(false);
  let artistSuggestionIndex = $state(-1);
  let artistSuggestionRequest = 0;
  let artistSuggestionTimer: ReturnType<typeof setTimeout> | null = null;
  let searchHistory = $state<SearchHistoryEntry[]>(readSearchHistory());
  let searchHistoryOpen = $state(false);
  let searchHistoryIndex = $state(-1);
  let submitted = $state("");
  let kind = $state<SearchKind>("all");
  const sources = $derived(availableSources?.length ? availableSources : DEFAULT_SOURCES);
  let searchMode = $state<"online" | "library">("online");
  let sourceChoice = $state("all");
  let customSourceIds = $state<string[]>([]);
  let rowsBySource = $state<Record<string, SearchRow[]>>({});
  let selectedTrackRows = $state<SearchRow[]>([]);
  let searchSelectionMode = $state(false);
  let playlistImportBusy = $state<string[]>([]);
  let playlistImportDone = $state<string[]>([]);
  let playlistImportErrors = $state<Record<string, string>>({});
  let playlistImportProgress = $state<Record<string, string>>({});
  const playlistImportControllers = new Map<string, AbortController>();
  let statuses = $state<Record<string, string>>({});
  let errors = $state<Record<string, string>>({});
  let cursors = $state<Record<string, string | null>>({});
  let incompleteSources = $state<Record<string, boolean>>({});
  let overflowRows = $state<Record<string, SearchRow[]>>({});
  let warnings = $state<Record<string, string[]>>({});
  let expanded = $state<Record<string, boolean>>({});
  let collapsedSources = $state<Record<string, boolean>>({});
  let chapterExpanded = $state<Record<string, boolean>>({});
  let chaptersByRow = $state<Record<string, Candidate | "loading" | "none" | "unavailable" | "unknown">>({});
  let generation = 0;
  let loadingSourceRequests: Record<string, number> = {};
  let soulseekSessions: Record<string, string> = {};
  let consumedPageCursors: Record<string, Set<string>> = {};
  let enrichmentQueue: { sourceId: string; row: SearchRow; request: number }[] = [];
  let enrichmentRunning = 0;
  let enrichmentStarted = new Set<string>();
  let lastQueryProp = "";
  let initialized = $state(false);
  let lastPresetRequestId = 0;
  let presetRequestPending = $state(false);
  let continueSearch = $state(false);
  let continuePreferenceLoaded = $state(false);
  let collectingSearch = $state(false);
  let collectionProgress = $state("");
  let collectionRequest = 0;
  let collectionDeadlineTimer: ReturnType<typeof setTimeout> | null = null;
  let collectionDeadlineOwner = 0;
  let collectionSoulseekSource: string | null = null;
  let lastAutomationSearchRequestId = "";
  let activeAutomationSearchId = $state<string | null>(null);
  let activeAutomationSearchRequest = $state<AutomationSearchRequest | null>(null);
  const collectionReads = new ProviderReadCancellation(call);
  const MAX_COLLECTION_RESULTS_PER_ACTION = 10_000;
  const COLLECTION_DEADLINE_MS = 120_000;
  let collectedSort = $state<TrackSortField>("sourceOrder");
  let collectedDirection = $state<SortDirection>("asc");
  let minimumDuration = $state("");
  let maximumDuration = $state("");

  $effect(() => {
    const initial = initialSources;
    if (!initialized) {
      initialized = true;
      text = initialQuery;
      lastQueryProp = initialQuery;
      const available = new Set(sources.map(source => source.id));
      const validOnline = (ids: unknown) => Array.isArray(ids) ? [...new Set(ids.filter((id): id is string => typeof id === "string" && ONLINE_SOURCE_IDS.includes(id) && available.has(id) && sourceEnabled(id)))] : [];
      if (initial) {
        const initialOnline = validOnline(initial);
        if (initial.includes("library") && !initialOnline.length) searchMode = "library";
        else if (initialOnline.length > 1) { searchMode = "online"; sourceChoice = "custom"; customSourceIds = initialOnline; }
        else if (initialOnline.length) { searchMode = "online"; sourceChoice = initialOnline[0]!; }
      } else {
        let restored = false;
        try {
          const saved = JSON.parse(localStorage.getItem(PREF_KEY) ?? "null") as { mode?: unknown; sourceId?: unknown; sourceIds?: unknown } | null;
          if (saved && (saved.mode === "online" || saved.mode === "library")) {
            if (saved.mode === "library" && available.has("library") && sourceEnabled("library")) { searchMode = "library"; restored = true; }
            else if (saved.mode === "online" && saved.sourceId === "custom") {
              customSourceIds = validOnline(saved.sourceIds);
              sourceChoice = "custom"; searchMode = "online"; restored = true;
            } else if (saved.mode === "online" && (saved.sourceId === "all" || (typeof saved.sourceId === "string" && validOnline([saved.sourceId]).length))) {
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
      if (sourceChoice !== "all" && sourceChoice !== "custom" && !validOnline([sourceChoice]).length) sourceChoice = "all";
    }
    if (initialQuery !== lastQueryProp) { lastQueryProp = initialQuery; text = initialQuery; }
  });
  $effect(() => {
    if (!initialized) return;
    try { localStorage.setItem(PREF_KEY, JSON.stringify({ mode: searchMode, sourceId: sourceChoice, ...(sourceChoice === "custom" ? { sourceIds: customSourceIds } : {}) })); } catch { /* storage can be unavailable in private contexts */ }
  });
  $effect(() => {
    if (!continuePreferenceLoaded) return;
    try { localStorage.setItem(CONTINUE_PREF_KEY, String(continueSearch)); } catch { /* storage can be unavailable */ }
  });
  $effect(() => {
    const preset = searchPreset;
    if (!navigationReady || !initialized || !preset || preset.requestId === 0 || preset.requestId === lastPresetRequestId) return;
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
  onDestroy(() => { navigationDestroyed = true; navigationRestoreEpoch++; cancelAutomationSearch(); generation++; collectionRequest++; clearCollectionDeadline(); if (artistSuggestionTimer) clearTimeout(artistSuggestionTimer); artistSuggestionRequest++; providerReads.cancelAll(); collectionReads.cancelAll(); void cancelSoulseek(); for (const controller of playlistImportControllers.values()) controller.abort(); });

  function scheduleArtistSuggestions(query: string) {
    if (artistSuggestionTimer) clearTimeout(artistSuggestionTimer);
    const request = ++artistSuggestionRequest;
    artistSuggestions = [];
    artistSuggestionIndex = -1;
    artistSuggestionsOpen = false;
    searchHistoryOpen = !query.trim() && searchInputFocused && searchHistory.length > 0;
    if (!searchArtists || query.trim().length < 2) return;
    artistSuggestionTimer = setTimeout(async () => {
      try {
        const results = await searchArtists(query.trim());
        if (request !== artistSuggestionRequest) return;
        artistSuggestions = results.slice(0, 6);
        artistSuggestionsOpen = artistSuggestions.length > 0;
      } catch {
        if (request === artistSuggestionRequest) artistSuggestions = [];
      }
    }, 300);
  }

  function chooseArtistSuggestion(result: ArtistSearchResult) {
    if (artistSuggestionTimer) clearTimeout(artistSuggestionTimer);
    artistSuggestionRequest++;
    artistSuggestionsOpen = false;
    artistSuggestions = [];
    onArtist(result.artist);
  }

  let searchInputFocused = false;
  function rememberCompletedSearch(query: string) {
    searchHistory = addSearchHistoryEntry(searchHistory, { query, kind, mode: searchMode, sourceChoice, customSourceIds: [...customSourceIds], savedAt: Date.now() });
    writeSearchHistory(searchHistory);
  }
  function restoreSearchHistory(entry: SearchHistoryEntry) {
    searchHistoryOpen = false;
    searchHistoryIndex = -1;
    text = entry.query;
    onQueryChange(text);
    if (kinds.some(option => option.id === entry.kind)) kind = entry.kind as SearchKind;
    if (entry.mode === "library") {
      if (sourceEnabled("library")) searchMode = "library";
    } else {
      const configured = validConfiguredSources(entry.customSourceIds);
      if (entry.sourceChoice === "custom" && configured.length) { searchMode = "online"; sourceChoice = "custom"; customSourceIds = configured; }
      else if (entry.sourceChoice === "all") { searchMode = "online"; sourceChoice = "all"; }
      else if (sourceEnabled(entry.sourceChoice) && ONLINE_SOURCE_IDS.includes(entry.sourceChoice)) { searchMode = "online"; sourceChoice = entry.sourceChoice; }
    }
  }
  function removeHistoryEntry(entry: SearchHistoryEntry) {
    searchHistory = removeSearchHistoryEntry(searchHistory, entry.query);
    writeSearchHistory(searchHistory);
    searchHistoryIndex = Math.min(searchHistoryIndex, searchHistory.length - 1);
  }
  function clearSearchHistory() {
    searchHistory = [];
    searchHistoryOpen = false;
    searchHistoryIndex = -1;
    writeSearchHistory(searchHistory);
  }
  function closeSearchPopupsOnBlur(event: FocusEvent) {
    const input = event.currentTarget as HTMLInputElement;
    setTimeout(() => {
      if (input.parentElement?.contains(document.activeElement)) return;
      searchInputFocused = false;
      searchHistoryOpen = false;
      artistSuggestionsOpen = false;
      artistSuggestionRequest++;
      if (artistSuggestionTimer) clearTimeout(artistSuggestionTimer);
    }, 0);
  }

  function handleArtistSuggestionKeys(event: KeyboardEvent) {
    if (!artistSuggestionsOpen && searchHistoryOpen && !text.trim() && searchHistory.length) {
      if (event.key === "ArrowDown") { event.preventDefault(); searchHistoryIndex = (searchHistoryIndex + 1) % searchHistory.length; }
      else if (event.key === "ArrowUp") { event.preventDefault(); searchHistoryIndex = (searchHistoryIndex - 1 + searchHistory.length) % searchHistory.length; }
      else if (event.key === "Enter" && searchHistoryIndex >= 0) { event.preventDefault(); restoreSearchHistory(searchHistory[searchHistoryIndex]!); }
      else if (event.key === "Escape") { event.preventDefault(); searchHistoryOpen = false; }
      return;
    }
    if (!artistSuggestionsOpen || !artistSuggestions.length) {
      if (event.key === "Escape") artistSuggestionsOpen = false;
      return;
    }
    if (event.key === "ArrowDown") { event.preventDefault(); artistSuggestionIndex = (artistSuggestionIndex + 1) % artistSuggestions.length; }
    else if (event.key === "ArrowUp") { event.preventDefault(); artistSuggestionIndex = (artistSuggestionIndex - 1 + artistSuggestions.length) % artistSuggestions.length; }
    else if (event.key === "Escape") { event.preventDefault(); artistSuggestionsOpen = false; }
    else if (event.key === "Enter" && artistSuggestionIndex >= 0) { event.preventDefault(); chooseArtistSuggestion(artistSuggestions[artistSuggestionIndex]!); }
  }
  onMount(async () => {
    if (navigationSnapshotKey) return; // App owns the awaited history restoration.
    navigationReady = true;
    try { continueSearch = localStorage.getItem(CONTINUE_PREF_KEY) === "true"; } catch { continueSearch = false; }
    continuePreferenceLoaded = true;
    if (automationSearch?.requestId) return;
    if (!initialQuery.trim()) return;
    await tick();
    if (!submitted && text.trim() && !presetRequestPending) void runSearch();
  });

  // Capture only on explicit route navigation, never on playback/scroll ticks.
  function searchNavigationSnapshot() {
    return { controls:{text,submitted,kind,searchMode,sourceChoice,customSourceIds:[...customSourceIds],collectedSort,collectedDirection,minimumDuration,maximumDuration,continueSearch},
      rowsBySource:{...rowsBySource},overflowRows:{...overflowRows},cursors:{...cursors},
      consumed:Object.fromEntries(Object.entries(consumedPageCursors).map(([id,values])=>[id,[...values]])),
      statuses:{...statuses},errors:{...errors},warnings:{...warnings},incompleteSources:{...incompleteSources},
      expanded:{...expanded},collapsedSources:{...collapsedSources},chapterExpanded:{...chapterExpanded},chaptersByRow:Object.fromEntries(Object.entries(chaptersByRow).filter(([,value])=>value!=="loading")),
      selectedTrackRows:[...selectedTrackRows],searchSelectionMode,collectionProgress,navigationNotice,
      accountScopes:{...navigationAccountScopes},loadedCount:Object.values(rowsBySource).reduce((n,rows)=>n+rows.length,0),
      scroll:searchElement?.scrollTop??0,chapterMetadataEvicted:false,needsRefetch:false };
  }
  type SearchNavigationSnapshot = ReturnType<typeof searchNavigationSnapshot>;
  function stripSearchNavigationRows(saved: SearchNavigationSnapshot): SearchNavigationSnapshot {
    return {...saved,rowsBySource:{},overflowRows:{},cursors:{},consumed:{},statuses:{},errors:{},warnings:{},incompleteSources:{},expanded:{},collapsedSources:{},chapterExpanded:{},chaptersByRow:{},selectedTrackRows:[],searchSelectionMode:false,collectionProgress:"",needsRefetch:true};
  }
  function retainedSearchNavigationRows(saved: SearchNavigationSnapshot): number {
    return Object.values(saved.rowsBySource).reduce((n,rows)=>n+rows.length,0)
      +Object.values(saved.overflowRows).reduce((n,rows)=>n+rows.length,0)
      +Object.values(saved.chaptersByRow).reduce((n,value)=>n+1+(typeof value==="object"?(value.chapters?.length??0):0),0);
  }
  export function captureNavigationSnapshot(key:string):string {
    const wasCollecting=collectingSearch;
    cancelAutomationSearch();generation++;collectionRequest++;clearCollectionDeadline();
    providerReads.cancelAll();collectionReads.cancelAll();void cancelSoulseek();collectionSoulseekSource=null;
    collectingSearch=false;loadingSourceRequests={};enrichmentQueue=[];
    statuses=Object.fromEntries(Object.entries(statuses).map(([id,status])=>[id,["Searching…","Loading more…"].includes(status)?"Cancelled":status]));
    if(wasCollecting)collectionProgress="Collection paused while navigating. Collect more to continue.";
    let snapshot=searchNavigationSnapshot();
    const rowCount=Object.values(snapshot.rowsBySource).reduce((n,rows)=>n+rows.length,0)+Object.values(snapshot.overflowRows).reduce((n,rows)=>n+rows.length,0);
    //Never retain a cursor after truncating its preceding rows. Above the bounded
    //cache budget preserve controls and explicitly require a fresh search.
    if(rowCount>MAX_COLLECTION_RESULTS_PER_ACTION)snapshot=stripSearchNavigationRows(snapshot);
    else if(retainedSearchNavigationRows(snapshot)>MAX_COLLECTION_RESULTS_PER_ACTION){
      //Drop separately fetched heavy chapter metadata, not preceding search rows
      //or their continuation. Chapter details can be expanded again on return.
      snapshot={...snapshot,chaptersByRow:{},chapterExpanded:{},chapterMetadataEvicted:true};
    }
    providerNavigationSnapshots.put({kind:"search-panel",key,accountScope:"search-routes",snapshot,rows:retainedSearchNavigationRows(snapshot),stripRows:stripSearchNavigationRows});
    return key;
  }
  export async function restoreNavigationSnapshot(key:string):Promise<void> {
    const epoch=++navigationRestoreEpoch;navigationReady=false;
    cancelAutomationSearch();generation++;collectionRequest++;clearCollectionDeadline();
    providerReads.cancelAll();collectionReads.cancelAll();void cancelSoulseek();
    collectingSearch=false;loadingSourceRequests={};enrichmentQueue=[];
    //Suppress current parent request IDs, not a future global preset left by a
    //different route. A genuinely new request arriving during restore still runs.
    lastPresetRequestId=searchPreset?.requestId??0;
    lastAutomationSearchRequestId=automationSearch?.requestId??"";
    const request=generation;
    const current=()=>!navigationDestroyed&&epoch===navigationRestoreEpoch&&request===generation;
    try {
      const saved=providerNavigationSnapshots.get<SearchNavigationSnapshot>("search-panel",key,"search-routes");
      if(!saved){navigationNotice="Previous search snapshot expired. Search again to reload it.";return;}
      const ids=[...new Set([...Object.keys(saved.rowsBySource),...Object.keys(saved.overflowRows),...Object.keys(saved.cursors)])];
      const scopes=await searchNavigationScopes(call,ids);
      if(!current())return;
      ({text,submitted,kind,searchMode,sourceChoice,collectedSort,collectedDirection,minimumDuration,maximumDuration,continueSearch}=saved.controls);
      customSourceIds = validConfiguredSources(saved.controls.customSourceIds);
      lastQueryProp=initialQuery;continuePreferenceLoaded=true;
      const invalid=new Set(ids.filter(id=>!scopes[id]||!saved.accountScopes[id]||scopes[id]!==saved.accountScopes[id]));
      const retained=<T,>(values:Record<string,T>)=>Object.fromEntries(Object.entries(values).filter(([id])=>!invalid.has(id)));
      rowsBySource=retained(saved.rowsBySource);overflowRows=retained(saved.overflowRows);cursors=retained(saved.cursors);
      consumedPageCursors=Object.fromEntries(Object.entries(retained(saved.consumed)).map(([id,values])=>[id,new Set(values)]));
      statuses=retained(saved.statuses);errors=retained(saved.errors);warnings=retained(saved.warnings);incompleteSources=retained(saved.incompleteSources);
      //Per-row metadata/selection may reference account-owned results. Clear it
      //as a unit on account uncertainty rather than guessing row ownership.
      expanded=invalid.size?{}:saved.expanded;chapterExpanded=invalid.size?{}:saved.chapterExpanded;chaptersByRow=invalid.size?{}:saved.chaptersByRow;
      collapsedSources=invalid.size?{}:(saved.collapsedSources??{});
      selectedTrackRows=invalid.size?[]:saved.selectedTrackRows;searchSelectionMode=!invalid.size&&saved.searchSelectionMode;
      collectionProgress=invalid.size?"":saved.collectionProgress;navigationAccountScopes=scopes;
      navigationNotice=saved.needsRefetch?"Previous search rows were evicted from memory. Search again to reload them.":invalid.size?"Some search results were cleared because their account changed, could not be verified, or their session ended. Search again to reload them.":saved.chapterMetadataEvicted?"Some cached chapter details were dropped to bound memory. Expand chapters to inspect them again.":saved.navigationNotice;
      if(invalid.size){
        const cleared={...searchNavigationSnapshot(),scroll:saved.scroll,needsRefetch:saved.needsRefetch};
        const rows=retainedSearchNavigationRows(cleared);
        providerNavigationSnapshots.put({kind:"search-panel",key,accountScope:"search-routes",snapshot:cleared,rows,stripRows:stripSearchNavigationRows});
      }
      await tick();if(!current())return;
      requestAnimationFrame(()=>{if(current()&&searchElement)searchElement.scrollTop=saved.scroll;});
    } finally {if(epoch===navigationRestoreEpoch)navigationReady=true;}
  }

  function sourceEnabled(id: string) { const source = sources.find(item => item.id === id); return Boolean(source) && (id !== "spotify" || source?.enabled === true) && source?.enabled !== false && sourceCapabilities[id] !== false && sourceCapabilities[id] !== "disconnected"; }
  function validConfiguredSources(ids: unknown) { return Array.isArray(ids) ? [...new Set(ids.filter((id): id is string => typeof id === "string" && ONLINE_SOURCE_IDS.includes(id) && sourceEnabled(id)))] : []; }
  function kindsForKind(value: SearchKind): string[] {
    if (value === "all") return ["track", "artist", "album", "video", "set", "playlist", "profile", "channel"];
    if (value === "videos") return ["video", "set"];
    return [value.slice(0, -1)];
  }
  function sourceSupportsKind(sourceId: string, value: SearchKind) {
    const supported = sources.find(item => item.id === sourceId)?.kinds;
    return !supported?.length || kindsForKind(value).some(item => supported.includes(item));
  }
  function sourceSupports(id: string) {
    const supported = sources.find(item => item.id === id)?.kinds;
    return !supported?.length || requestedKinds().some(value => supported.includes(value));
  }
  const trackControlsApplicable = $derived(!["artists", "albums", "playlists", "profiles", "channels"].includes(kind));
  const enabledOnlineSources = $derived(sources.filter(source => ONLINE_SOURCE_IDS.includes(source.id) && sourceEnabled(source.id)));
  const selectedSource = $derived(searchMode === "library" || sourceChoice === "all" || sourceChoice === "custom" ? null : enabledOnlineSources.find(source => source.id === sourceChoice) ?? null);
  const visibleKinds = $derived(searchMode === "library" ? kinds.filter(item => item.id === "all" || item.id === "tracks") : kinds);
  const selectedSourceUnsupportedKinds = $derived(searchMode === "online" && sourceChoice !== "all" && sourceChoice !== "custom" && selectedSource
    ? visibleKinds.filter(item => !sourceSupportsKind(selectedSource.id, item.id))
    : []);
  const selectedSourceUnsupported = $derived(searchMode === "online" && sourceChoice !== "all" && sourceChoice !== "custom" && selectedSource && !sourceSupports(selectedSource.id) ? selectedSource : null);
  // This changes only provider section order. Each provider keeps its own relevance order;
  // loadedOccurrences also follows this same displayed order for continuation playback.
  const displayedSources = $derived.by(() => {
    const available = searchMode === "library"
      ? sources.filter(source => source.id === "library" && sourceEnabled(source.id))
      : (sourceChoice === "all" ? enabledOnlineSources : enabledOnlineSources.filter(source => sourceChoice === "custom" ? customSourceIds.includes(source.id) : source.id === sourceChoice)).filter(source => sourceSupports(source.id));
    return [...available].sort((left, right) => Number(left.id === "musicbrainz") - Number(right.id === "musicbrainz"));
  });
  function activeSourceIds() {
    if (searchMode === "library") return sourceEnabled("library") ? ["library"] : [];
    return (sourceChoice === "all" ? enabledOnlineSources.map(source => source.id) : sourceChoice === "custom" ? customSourceIds : [sourceChoice]).filter(id => sourceEnabled(id));
  }
  const automationSourceIds = new Set<AutomationSearchSourceId>(["all", "library", ...ONLINE_SOURCE_IDS as AutomationSearchSourceId[]]);
  const automationKinds = new Set<AutomationSearchKind>(["all", "tracks", "artists", "albums", "videos", "playlists", "profiles", "channels"]);
  function finishAutomationSearch(report: AutomationSearchReport) {
    if (activeAutomationSearchId !== report.requestId) return;
    activeAutomationSearchId = null;
    activeAutomationSearchRequest = null;
    void Promise.resolve(onAutomationSearchComplete(report)).catch(() => {});
  }
  function cancelAutomationSearch() {
    const requestId = activeAutomationSearchId;
    if (!requestId) return;
    const request = activeAutomationSearchRequest;
    finishAutomationSearch({
      requestId,
      status: "cancelled",
      sourceId: request?.requestId === requestId && automationSourceIds.has(request.sourceId) ? request.sourceId : "all",
      kind: request?.requestId === requestId && automationKinds.has(request.kind) ? request.kind : "all",
      resultCounts: {},
      errorCode: "cancelled",
    });
  }
  function automationReport(request: AutomationSearchRequest, status: AutomationSearchReport["status"], resultCounts: Record<string, number>, errorCode?: AutomationSearchErrorCode, sourceCoverage?: AutomationSearchReport["sourceCoverage"]): AutomationSearchReport {
    return { requestId: request.requestId, status, sourceId: request.sourceId, kind: request.kind, resultCounts, ...(errorCode ? { errorCode } : {}), ...(sourceCoverage ? { sourceCoverage } : {}) };
  }
  async function startAutomationSearch(request: AutomationSearchRequest) {
    if (activeAutomationSearchId) cancelAutomationSearch();
    activeAutomationSearchId = request.requestId;
    activeAutomationSearchRequest = request;
    const sourceId = automationSourceIds.has(request.sourceId) ? request.sourceId : "all";
    const kindValue = automationKinds.has(request.kind) ? request.kind : "all";
    const safeRequest = { ...request, sourceId, kind: kindValue } as AutomationSearchRequest;
    const query = typeof request.query === "string" ? request.query.trim() : "";
    if (!request.requestId || request.requestId.length > 128 || !automationSourceIds.has(request.sourceId) || !automationKinds.has(request.kind) || !query || Array.from(query).length > 500) {
      finishAutomationSearch(automationReport(safeRequest, "failed", {}, "invalid_request"));
      return;
    }
    text = query; submitted = query; kind = kindValue;
    if (sourceId === "library") searchMode = "library";
    else { searchMode = "online"; sourceChoice = sourceId; }
    await tick();
    if (activeAutomationSearchId !== request.requestId) return;
    const sourceIds = sourceId === "all"
      ? activeSourceIds().filter(id => sourceSupports(id))
      : activeSourceIds().filter(id => id === sourceId && sourceSupports(id));
    if (!sourceIds.length) {
      const hasAny = sourceId === "all" ? activeSourceIds().length > 0 : sourceEnabled(sourceId);
      finishAutomationSearch(automationReport(safeRequest, "failed", {}, hasAny ? "source_unavailable" : "no_sources"));
      return;
    }
    await runSearch(undefined, false, collectionRequest, request.requestId);
    if (activeAutomationSearchId !== request.requestId) return;
    const resultCounts = Object.fromEntries(sourceIds.map(id => [id, Math.min(10_000, Math.max(0, Math.floor(rowsBySource[id]?.length ?? 0)))]));
    const failures = sourceIds.filter(id => statuses[id] === "Unavailable");
    const status = failures.length ? "failed" : "complete";
    const sourceCoverage = Object.fromEntries(sourceIds.map(id => {
      const hasMore = Boolean(cursors[id] || overflowRows[id]?.length);
      return [id, { hasMore, incomplete: hasMore || Boolean(incompleteSources[id]) || statuses[id] === "Unavailable" }];
    }));
    finishAutomationSearch(automationReport(safeRequest, status, resultCounts, failures.length ? "request_failed" : undefined, sourceCoverage));
  }
  $effect(() => {
    const request = automationSearch;
    if (!navigationReady || !initialized || !request || !request.requestId || request.requestId === lastAutomationSearchRequestId) return;
    lastAutomationSearchRequestId = request.requestId;
    void startAutomationSearch(request);
  });
  $effect(() => {
    const cancelled = automationSearchCancelled;
    if (cancelled && activeAutomationSearchId === cancelled) {
      const request = activeAutomationSearchRequest;
      const sourceId = request?.requestId === cancelled && automationSourceIds.has(request.sourceId) ? request.sourceId : "all";
      const requestKind = request?.requestId === cancelled && automationKinds.has(request.kind) ? request.kind : "all";
      finishAutomationSearch({ requestId: cancelled, status: "cancelled", sourceId, kind: requestKind, resultCounts: {}, errorCode: "cancelled" });
      generation++; providerReads.cancelAll(); collectionReads.cancelAll();
      statuses = Object.fromEntries(Object.keys(statuses).map(id => [id, "Cancelled"]));
    }
  });
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
    providerReads.cancelAll();
    collectionReads.cancelAll();
    void cancelSoulseek(); collectionSoulseekSource = null;
    if (submitted.trim()) await runSearch();
    else { generation++; collectionRequest++; collectingSearch = false; collectionProgress = ""; cancelSearchTrackSelection(); rowsBySource = {}; statuses = {}; errors = {}; cursors = {}; incompleteSources = {}; warnings = {}; overflowRows = {}; }
  }
  function requestedKinds(): string[] {
    if (kind === "all") return ["track", "artist", "album", "video", "set", "playlist", "profile", "channel"];
    if (kind === "videos") return ["video", "set"];
    return [kind.slice(0, -1)];
  }
  function identity(row: SearchRow) { return `${row.sourceId}:${row.kind}:${row.id}`; }
  function toggleSelectedSearchTrack(row: SearchRow) {
    if (selectedTrackRows.includes(row)) { selectedTrackRows = selectedTrackRows.filter(item => item !== row); return; }
    const index = loadedSearchTrackRows.indexOf(row);
    const next = [...selectedTrackRows];
    const insertBefore = next.findIndex(item => { const existing = loadedSearchTrackRows.indexOf(item); return existing >= 0 && index >= 0 && existing > index; });
    next.splice(insertBefore < 0 ? next.length : insertBefore, 0, row);
    selectedTrackRows = next;
  }
  function cancelSearchTrackSelection() {
    selectedTrackRows = [];
    searchSelectionMode = false;
  }
  async function addPlaylistResult(row: SearchRow) {
    const key = identity(row);
    if (!onAddRemotePlaylist || playlistImportBusy.includes(key) || playlistImportDone.includes(key)) return;
    const controller = new AbortController(); playlistImportControllers.set(key, controller);
    playlistImportBusy = [...playlistImportBusy, key]; playlistImportErrors = { ...playlistImportErrors, [key]: "" };
    playlistImportProgress = { ...playlistImportProgress, [key]: "Starting playlist import…" };
    try {
      const outcome = await onAddRemotePlaylist(row, { signal: controller.signal, onProgress: message => { playlistImportProgress = { ...playlistImportProgress, [key]: message }; } });
      if (outcome?.cancelled) playlistImportProgress = { ...playlistImportProgress, [key]: "Import canceled. You can retry." };
      else { playlistImportDone = [...playlistImportDone, key]; playlistImportProgress = { ...playlistImportProgress, [key]: outcome?.partial ? "Added with missing items." : "Added to library." }; }
    }
    catch (cause) { playlistImportErrors = { ...playlistImportErrors, [key]: cause instanceof Error ? cause.message : String(cause) }; }
    finally { playlistImportControllers.delete(key); playlistImportBusy = playlistImportBusy.filter(item => item !== key); }
  }
  function cancelPlaylistImport(row: SearchRow) { playlistImportControllers.get(identity(row))?.abort(); }
  function mergeRows(sourceId: string, incoming: SearchRow[], append: boolean) {
    const merged = append ? [...(rowsBySource[sourceId] ?? []), ...incoming] : incoming;
    const unique = new Map<string, SearchRow>();
    for (const row of merged) unique.set(identity(row), row);
    rowsBySource = { ...rowsBySource, [sourceId]: [...unique.values()] };
  }
  async function requestSource(query: string, sourceId: string, cursor?: string | null, reads = providerReads, limit = 30): Promise<SearchPage> {
    if (searchSource) return searchSource({ query, sourceIds: [sourceId], kinds: requestedKinds(), cursor, limit });
    const args = { query, sourceIds: [sourceId], kinds: requestedKinds(), cursor: cursor ?? null, limit };
    // Soulseek creates a separately polled job; its dedicated cancellation command owns that lifecycle.
    if (sourceId === "soulseek") return call<SearchPage>("search_source", args);
    return reads.read<SearchPage>("search_source", args);
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
  async function runSearch(appendSource?: string, collection = false, collectionRun = collectionRequest, automationRequestId: string | null = null, collectionBudget?: { used: number }) {
    if (!appendSource) cancelSearchTrackSelection();
    if (activeAutomationSearchId && activeAutomationSearchId !== automationRequestId) cancelAutomationSearch();
    const term = (appendSource ? submitted : text).trim();
    if (!term) return;
    if (appendSource && loadingSourceRequests[appendSource] === generation) return;
    if (!appendSource) {
      providerReads.cancelAll();
      collectionReads.cancelAll();
      clearCollectionDeadline();
      collectionRequest++;
      collectingSearch = false; collectionProgress = "";
      void cancelSoulseek(); collectionSoulseekSource = null;
      generation++;
      submitted = term;
      rowsBySource = {}; statuses = {}; errors = {}; cursors = {}; incompleteSources = {}; warnings = {}; overflowRows = {}; expanded = {}; collapsedSources = {}; chaptersByRow = {};
      loadingSourceRequests = {}; consumedPageCursors = {};
      enrichmentQueue = []; enrichmentStarted = new Set();
    }
    const request = generation;
    if (collection && appendSource === "soulseek") collectionSoulseekSource = "soulseek";
    const activeIds = activeSourceIds();
    const sourceIds = appendSource ? [appendSource] : activeIds.filter(id => sourceSupports(id));
    for (const sourceId of activeIds.filter(id => !sourceSupports(id))) statuses = { ...statuses, [sourceId]: "Not available for this result type" };
    if (!sourceIds.length) return;
    if (!appendSource) {
      const scopes=sourceIds.some(id=>id==="spotify"||id==="youtube")?await searchNavigationScopes(call,sourceIds):Object.fromEntries(sourceIds.map(id=>[id,publicSearchNavigationScope(id)]));
      if(request!==generation||navigationDestroyed)return;
      navigationAccountScopes=scopes;navigationNotice="";
    }
    await Promise.all(sourceIds.map(async sourceId => {
      loadingSourceRequests = { ...loadingSourceRequests, [sourceId]: request };
      statuses = { ...statuses, [sourceId]: appendSource ? "Loading more…" : "Searching…" };
      errors = { ...errors, [sourceId]: "" };
      try {
        let page: SearchPage;
        const requestedCursor = appendSource ? cursors[sourceId] : null;
        const buffered = appendSource ? overflowRows[sourceId] ?? [] : [];
        if (buffered.length) {
          const actionAllowance = collection ? Math.max(0, MAX_COLLECTION_RESULTS_PER_ACTION - (collectionBudget?.used ?? 0)) : 30;
          const batch = buffered.slice(0, Math.min(30, actionAllowance));
          const left = buffered.slice(batch.length);
          overflowRows = { ...overflowRows, [sourceId]: left };
          if (collectionBudget) collectionBudget.used += batch.length;
          if (batch.length) { mergeRows(sourceId, batch, true); queueVisibleEnrichment(sourceId, request); }
          if (collection && (collectionBudget?.used ?? 0) >= MAX_COLLECTION_RESULTS_PER_ACTION) {
            statuses = { ...statuses, [sourceId]: "Collection action limit reached; collect more to continue" };
            return;
          }
          if (left.length) {
            statuses = { ...statuses, [sourceId]: "More results available" };
            return;
          }
          if (!cursors[sourceId]) {
            statuses = { ...statuses, [sourceId]: incompleteSources[sourceId] ? "Incomplete results; no continuation is available" : "Complete" };
            return;
          }
          page = await requestSource(term, sourceId, cursors[sourceId], collection ? collectionReads : providerReads, collection ? Math.max(1, Math.min(30, MAX_COLLECTION_RESULTS_PER_ACTION - (collectionBudget?.used ?? 0))) : 30);
        } else {
          const remaining = collection ? Math.max(1, Math.min(30, MAX_COLLECTION_RESULTS_PER_ACTION - (collectionBudget?.used ?? 0))) : 30;
          page = await requestSource(term, sourceId, appendSource ? cursors[sourceId] : null, collection ? collectionReads : providerReads, remaining);
        }
        if (request !== generation || (collection && collectionRun !== collectionRequest)) {
          // Soulseek's start request creates a server-side job before it returns its id,
          // so an earlier query change may have had nothing to cancel yet.
          if (sourceId === "soulseek" && page.nextCursor) {
            const searchId = page.nextCursor.split("@", 1)[0]!;
            void call("cancel_slskd_search", { searchId }).catch(() => {});
          }
          return;
        }
        // Soulseek polls one active session cursor until completion; it is not a page token.
        if (sourceId !== "soulseek") {
          const consumed = consumedPageCursors[sourceId] ?? new Set<string>();
          if (requestedCursor) consumed.add(requestedCursor);
          consumedPageCursors[sourceId] = consumed;
          if (page.nextCursor && consumed.has(page.nextCursor)) {
            page = { ...page, nextCursor: null, complete: false,
              progress: "Pagination stopped; loaded results kept.",
              warnings: [...(page.warnings ?? []), "The source repeated a page cursor. Loaded results are incomplete; start a new search to retry."] };
          }
        }
        let append = Boolean(appendSource);
        while (true) {
          incompleteSources = { ...incompleteSources, [sourceId]: page.complete === false };
          const pageResults = page.results ?? [];
          const actionRemaining = collection ? Math.max(0, MAX_COLLECTION_RESULTS_PER_ACTION - (collectionBudget?.used ?? 0)) : pageResults.length;
          const { accepted: actionRows, overflow: actionOverflow } = takeCollectionRows(pageResults, actionRemaining);
          if (collectionBudget) collectionBudget.used += actionRows.length;
          if (actionOverflow.length) overflowRows = { ...overflowRows, [sourceId]: [...actionOverflow, ...(overflowRows[sourceId] ?? [])] };
          mergeRows(sourceId, actionRows, append);
          queueVisibleEnrichment(sourceId, request);
          append = true;
          cursors = { ...cursors, [sourceId]: page.nextCursor ?? null };
          warnings = { ...warnings, [sourceId]: page.warnings ?? [] };
          if (collection && (collectionBudget?.used ?? 0) >= MAX_COLLECTION_RESULTS_PER_ACTION) break;
          if (sourceId !== "soulseek" || page.complete !== false || !page.nextCursor) break;
          soulseekSessions = { ...soulseekSessions, [sourceId]: page.nextCursor };
          statuses = { ...statuses, [sourceId]: page.progress || "Searching…" };
          await new Promise(resolve => setTimeout(resolve, 900));
          if (request !== generation || (collection && collectionRun !== collectionRequest)) { await cancelSoulseek(sourceId); return; }
          page = await pollSource(sourceId, page.nextCursor);
          if (request !== generation || (collection && collectionRun !== collectionRequest)) { await cancelSoulseek(sourceId); return; }
        }
        if (sourceId === "soulseek") soulseekSessions = { ...soulseekSessions, [sourceId]: "" };
        statuses = { ...statuses, [sourceId]: collection && (overflowRows[sourceId]?.length ?? 0) > 0
          ? "Collection limit reached; load more to continue"
          : page.complete === false ? (page.progress || (page.nextCursor ? "More results available" : "Incomplete results; no continuation is available"))
          : page.nextCursor ? "More results available" : "Complete" };
      } catch (error) {
        if (request !== generation || (collection && collectionRun !== collectionRequest)) return;
        const message = error instanceof Error ? error.message : String(error);
        if (sourceId === "soulseek" && /unknown or expired slskd search/i.test(message)) {
          // Keep already loaded rows, but discard the unusable native cursor.
          // Retry then starts a fresh Soulseek search and appends its results.
          cursors = { ...cursors, [sourceId]: null };
          soulseekSessions = { ...soulseekSessions, [sourceId]: "" };
          errors = { ...errors, [sourceId]: "This Soulseek search expired. Loaded results are kept; Retry starts a fresh search." };
          statuses = { ...statuses, [sourceId]: "Search expired" };
          return;
        }
        errors = { ...errors, [sourceId]: message };
        statuses = { ...statuses, [sourceId]: "Unavailable" };
      } finally {
        if (collection && collectionRun === collectionRequest && collectionSoulseekSource === sourceId) collectionSoulseekSource = null;
        if (loadingSourceRequests[sourceId] === request) {
          const next = { ...loadingSourceRequests }; delete next[sourceId]; loadingSourceRequests = next;
        }
      }
    }));
    if (!appendSource && !collection && !automationRequestId && request === generation && sourceIds.some(id => !errors[id] && statuses[id] !== "Unavailable")) rememberCompletedSearch(term);
  }
  function cancel() {
    cancelAutomationSearch();
    generation++; collectionRequest++; clearCollectionDeadline(); collectingSearch = false; collectionProgress = "";
    providerReads.cancelAll(); collectionReads.cancelAll(); void cancelSoulseek(); collectionSoulseekSource = null;
    statuses = Object.fromEntries(Object.keys(statuses).map(id => [id, "Cancelled"]));
  }
  const durationFilter = $derived({
    ...(parseDurationBound(minimumDuration ?? "") !== null ? { minDurationSeconds: parseDurationBound(minimumDuration ?? "")! } : {}),
    ...(parseDurationBound(maximumDuration ?? "") !== null ? { maxDurationSeconds: parseDurationBound(maximumDuration ?? "")! } : {}),
  });
  const loadedOccurrences = $derived.by(() => displayedSources.flatMap(source => {
    const rows = rowsBySource[source.id] ?? [];
    const candidates = rows.filter((row): row is SearchRow & { track: Track } => Boolean(row.track));
    // Keep each provider row attached to a unique sort object. A recording may
    // occur more than once (even with the exact same Track object reference),
    // and reconstructing rows by Track reference loses source-order reversals.
    const occurrences = candidates.map(row => ({ row, track: { ...row.track } }));
    const matching = new Set(filterTracks(occurrences.map(item => item.track), durationFilter));
    const filtered = occurrences.filter(item => matching.has(item.track));
    const rowBySortTrack = new Map(filtered.map(item => [item.track, item.row]));
    const ordered = sortTracks(filtered.map(item => item.track), collectedSort, collectedDirection);
    return ordered.map(track => ({ sourceId: source.id, row: rowBySortTrack.get(track)!, track: rowBySortTrack.get(track)!.track! }));
  }));
  // Playback continuation follows the visible provider groups while the loaded
  // dataset remains intact for collection, sorting, selection, and snapshots.
  const playableOccurrences = $derived(loadedOccurrences.filter(item => !collapsedSources[item.sourceId]));
  // Finite automation handles are tied to the exact loaded, filtered and sorted
  // playable snapshot. They are invalidated synchronously before every action.
  let searchControlRevision = crypto.randomUUID();
  let searchControlFingerprint = "";
  const searchControlRows = new Map<string, { revision: string; sourceId: string; row: SearchRow }>();
  function syncSearchControlRevision() {
    const fingerprint = searchUiControlFingerprint({
      text, submitted, mode: searchMode, source: sourceChoice, kind, sort: collectedSort,
      direction: collectedDirection, minimumDuration, maximumDuration, generation,
      collectionRequest, continueSearch,
      occurrences: playableOccurrences.map(item => ({ sourceId: item.sourceId, rowId: item.row.id, rowKind: item.row.kind, track: item.track })),
    });
    if (fingerprint !== searchControlFingerprint) {
      searchControlFingerprint = fingerprint;
      searchControlRevision = crypto.randomUUID();
      searchControlRows.clear();
    }
    return searchControlRevision;
  }
  function boundedAutomationText(value: unknown, max = 300) {
    if (typeof value !== "string") return "";
    const encoder = new TextEncoder();
    let output = "";
    let bytes = 0;
    for (const character of value) {
      const size = encoder.encode(character).byteLength;
      if (bytes + size > max) break;
      output += character;
      bytes += size;
    }
    return output;
  }
  export function inspectAutomationSearchUi(offset = 0, limit = 100, expiresAtMs = Date.now() + 5_000) {
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    const revision = syncSearchControlRevision();
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    const start = Math.max(0, Math.min(1_000_000, Math.floor(offset)));
    const count = Math.max(1, Math.min(200, Math.floor(limit)));
    const end = Math.min(playableOccurrences.length, start + count);
    const rows = playableOccurrences.slice(start, end).map(item => {
      const token = crypto.randomUUID();
      searchControlRows.set(token, { revision, sourceId: item.sourceId, row: item.row });
      return {
        occurrenceToken: token,
        sourceId: item.sourceId,
        title: boundedAutomationText(item.track.title),
        artist: boundedAutomationText(item.track.artist),
        durationSeconds: (() => { const value = item.track.playbackRange ? item.track.playbackRange.endTime - item.track.playbackRange.startTime : item.track.durationSeconds; return Number.isFinite(value) && Number(value) > 0 ? value : null; })(),
      };
    });
    // Keep the in-memory token table bounded even after repeated inspection.
    while (searchControlRows.size > 1000) searchControlRows.delete(searchControlRows.keys().next().value!);
    const observedAtMs = Date.now();
    if (searchUiControlExpired(expiresAtMs, observedAtMs)) return { status: "expired" };
    return {
      status: "ready", revision, observedAtMs, query: boundedAutomationText(submitted, 500),
      mode: searchMode, sourceId: searchMode === "library" ? "library" : sourceChoice,
      queryInputPending: text.trim() !== submitted.trim(),
      kind, continueSearch, total: playableOccurrences.length, offset: start,
      hasMore: displayedSources.some(source => Boolean(cursors[source.id] || overflowRows[source.id]?.length)),
      collecting: collectingSearch, rows,
    };
  }
  export async function setAutomationContinueSearch(expectedRevision: string, enabled: boolean, expiresAtMs: number) {
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    const revision = syncSearchControlRevision();
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    if (revision !== expectedRevision) return { status: "stale", revision };
    const appliedAtMs = Date.now();
    if (searchUiControlExpired(expiresAtMs, appliedAtMs)) return { status: "expired" };
    continueSearch = enabled;
    try { localStorage.setItem(CONTINUE_PREF_KEY, String(enabled)); } catch { /* preference remains active for this session */ }
    await tick();
    return { status: "complete", revision: syncSearchControlRevision(), enabled: continueSearch, appliedAtMs };
  }
  export function playAutomationSearchResult(expectedRevision: string, occurrenceToken: string, expiresAtMs: number) {
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    const revision = syncSearchControlRevision();
    if (searchUiControlExpired(expiresAtMs, Date.now())) return { status: "expired" };
    if (revision !== expectedRevision) return { status: "stale", revision };
    const target = searchControlRows.get(occurrenceToken);
    if (!target || target.revision !== revision) return { status: "stale", revision };
    const index = playableOccurrences.findIndex(item => item.sourceId === target.sourceId && item.row === target.row);
    if (index < 0) return { status: "stale", revision };
    const item = playableOccurrences[index];
    const dispatchedAtMs = Date.now();
    if (searchUiControlExpired(expiresAtMs, dispatchedAtMs)) return { status: "expired" };
    // Reuse the exact UI handler so the existing continuation/snapshot rules
    // and duplicate occurrence identity are preserved.
    void playSearchResult(item.track, item.sourceId, item.row);
    return { status: "dispatched", revision, sourceId: item.sourceId, dispatchedAtMs };
  }
  const collectedTracks = $derived(loadedOccurrences.map(item => item.track));
  const loadedTracks = $derived(collectedTracks);
  const loadedDatasetTracks = $derived(displayedSources.flatMap(source =>
    (rowsBySource[source.id] ?? []).flatMap(row => row.track ? [row.track] : [])));
  const loadedSortSources = $derived(displayedSources.map(source => source.id));
  const loadedSortIncomplete = $derived(Object.values(cursors).some(Boolean) || Object.values(overflowRows).some(rows => rows.length > 0));
  const loadedSortAvailability = $derived(trackSortAvailabilityForSources(loadedDatasetTracks, loadedSortSources, loadedSortIncomplete));
  const collectedTrackCount = $derived(displayedSources.reduce((total, source) => total + (rowsBySource[source.id] ?? []).filter(row => Boolean(row.track)).length, 0));
  const displayedRowsBySource = $derived.by(() => Object.fromEntries(displayedSources.map(source => {
    const rows = rowsBySource[source.id] ?? [];
    const tracksForSource = loadedOccurrences.filter(item => item.sourceId === source.id).map(item => item.row);
    return [source.id, [...rows.filter(row => !row.track), ...tracksForSource]];
  })));
  const loadedSearchTrackRows = $derived(Object.values(displayedRowsBySource).flat().filter(row => row.track));
  const selectedSearchTracks = $derived(loadedSearchTrackRows
    .filter(row => selectedTrackRows.includes(row))
    .flatMap(row => row.track ? [row.track] : []));
  async function playSearchResult(track: Track, sourceId?: string, row?: SearchRow) {
    if (!continueSearch) { onPlay?.(track); return; }
    let queue = playableOccurrences.map(item => item.track);
    let index = sourceId && row ? playableOccurrences.findIndex(item => item.sourceId === sourceId && item.row === row) : -1;
    if (index < 0) index = queue.indexOf(track);
    if (index < 0) { queue = [track, ...queue]; index = 0; }
    onPlay?.(track, queue.length ? queue : [track], index);
  }
  async function collectMoreSearchResults() {
    const request = ++collectionRequest;
    const started = Date.now();
    collectingSearch = true;
    collectionProgress = "Collecting more search results…";
    clearCollectionDeadline();
    collectionDeadlineOwner = request;
    collectionDeadlineTimer = setTimeout(() => {
      if (collectionDeadlineOwner === request) {
        collectionDeadlineTimer = null;
        collectionDeadlineOwner = 0;
        stopCollection(request, "Collection stopped at the two-minute limit; loaded results kept.");
      }
    }, COLLECTION_DEADLINE_MS);
    try {
      const budget = { used: 0 };
      let total = collectedTrackCount;
      while (request === collectionRequest && budget.used < MAX_COLLECTION_RESULTS_PER_ACTION && Date.now() - started < COLLECTION_DEADLINE_MS) {
        const pending = displayedSources.filter(source => (cursors[source.id] || overflowRows[source.id]?.length) && statuses[source.id] !== "Searching…" && statuses[source.id] !== "Loading more…");
        if (!pending.length) break;
        let advanced = false;
        for (const source of pending) {
          if (request !== collectionRequest || budget.used >= MAX_COLLECTION_RESULTS_PER_ACTION || Date.now() - started >= COLLECTION_DEADLINE_MS) break;
          const before = cursors[source.id];
          const beforeOverflow = overflowRows[source.id]?.length ?? 0;
          await runSearch(source.id, true, request, null, budget);
          if (request !== collectionRequest) break;
          if (cursors[source.id] !== before || (overflowRows[source.id]?.length ?? 0) < beforeOverflow) advanced = true;
          total = collectedTrackCount;
          collectionProgress = `Collected ${total.toLocaleString()} tracks…`;
        }
        if (!advanced) break;
      }
    } finally {
      if (collectionDeadlineOwner === request) clearCollectionDeadline();
      if (request === collectionRequest) { collectingSearch = false; collectionProgress = ""; }
    }
  }
  function clearCollectionDeadline() {
    if (collectionDeadlineTimer !== null) clearTimeout(collectionDeadlineTimer);
    collectionDeadlineTimer = null; collectionDeadlineOwner = 0;
  }
  function stopCollection(request: number, message: string) {
    if (request !== collectionRequest) return;
    collectionRequest++;
    clearCollectionDeadline();
    collectionReads.cancelAll();
    if (collectionSoulseekSource) { void cancelSoulseek(collectionSoulseekSource); collectionSoulseekSource = null; }
    collectingSearch = false;
    collectionProgress = message;
    statuses = Object.fromEntries(Object.entries(statuses).map(([id, status]) => [id, status === "Loading more…" ? "Collection stopped; loaded results kept" : status]));
  }
  function cancelCollection() { stopCollection(collectionRequest, "Collection cancelled; loaded results kept."); }
  function rowArtist(row: SearchRow): ArtistRef | null {
    const data = row.data as Record<string, unknown> | null;
    const ref = (data?.artist ?? data?.ref ?? data) as ArtistRef | undefined;
    return ref && (ref.provider === "musicbrainz" || ref.provider === "discogs") && typeof ref.id === "string" ? ref : null;
  }
  function rowAlbum(row: SearchRow): ArtistReleaseRef | null {
    const data = row.data as ArtistReleaseRef | null;
    return data && data.release && data.sourceUrl ? data : null;
  }
  function nonTrackAction(row: SearchRow): string | null {
    if (row.sourceId === "spotify") {
      if (row.kind === "artist") return "Explore artist";
      if (row.kind === "album") return "Open album";
      if (row.kind === "playlist") return "Open playlist";
      if (row.kind === "profile") return "Open profile";
    }
    if (row.kind === "artist") return rowArtist(row) ? "Open artist" : null;
    if (row.kind === "album") return rowAlbum(row) ? "Open album" : null;
    if (row.kind === "playlist") return "Open playlist";
    if (row.kind === "profile") return "Open profile";
    if (row.kind === "channel") return "Open channel";
    return null;
  }
  function openNonTrackResult(row: SearchRow) {
    if (row.sourceId === "spotify" || ["playlist", "profile", "channel"].includes(row.kind)) {
      onRemoteResult(row);
    } else if (row.kind === "artist") {
      const artist = rowArtist(row);
      if (artist) onArtist(artist);
    } else if (row.kind === "album") {
      const album = rowAlbum(row);
      if (album) onAlbum(album);
    }
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
    if (!audioCandidate || typeof audioCandidate !== "object" || !Array.isArray((audioCandidate as Candidate).chapters)) return null;
    const candidate = audioCandidate as Candidate;
    const chapters = usableChapters(candidate);
    return chapters.length ? { ...candidate, chapters } : { ...candidate, chapters: [] };
  }
  function usableChapters(candidate: Candidate): Chapter[] {
    return (Array.isArray(candidate.chapters) ? candidate.chapters : []).filter((chapter) =>
      Boolean(chapter && typeof chapter === "object" && typeof chapter.id === "string" && chapter.id.trim()
        && typeof chapter.title === "string" && chapter.title.trim() && validChapterRange(chapter, candidate.durationSeconds)));
  }
  function chapterState(row: SearchRow): Candidate | "loading" | "none" | "unavailable" | "unknown" | undefined {
    const state = chaptersByRow[identity(row)];
    if (state) return state;
    if (!row.track) return undefined;
    const candidate = knownCandidate(row.track);
    return candidate ? candidate.chapters?.length ? candidate : "none" : undefined;
  }
  async function inspectChapters(row: SearchRow) {
    const id = identity(row);
    if (chaptersByRow[id] === "loading" || !row.url || !enrichSource && !desktop()) return;
    const request = generation;
    chaptersByRow = { ...chaptersByRow, [id]: "loading" };
    try {
      const candidate = enrichSource ? await enrichSource(row.url) : await call<Candidate>("inspect_youtube_source", { url: row.url });
      if (request !== generation) return;
      const chapters = usableChapters(candidate);
      chaptersByRow = { ...chaptersByRow, [id]: Array.isArray(candidate.chapters) ? (chapters.length ? { ...candidate, chapters } : "none") : "unknown" };
    } catch { if (request === generation) chaptersByRow = { ...chaptersByRow, [id]: "unavailable" }; }
  }
</script>

<section class="search-panel" aria-label="Discovery search" bind:this={searchElement}>
  {#if navigationNotice}<p role="status" class="navigation-notice">{navigationNotice}</p>{/if}
  <form class="search-form" onsubmit={(event) => { event.preventDefault(); void runSearch(); }}>
    <div class="search-input-wrap">
      <input aria-label="Search music" role="combobox" aria-autocomplete="list" aria-expanded={artistSuggestionsOpen || searchHistoryOpen} aria-controls={artistSuggestionsOpen ? "artist-search-suggestions" : searchHistoryOpen ? "search-history-menu" : undefined} aria-activedescendant={artistSuggestionsOpen ? artistSuggestionIndex >= 0 ? `artist-search-suggestion-${artistSuggestionIndex}` : undefined : searchHistoryOpen && searchHistoryIndex >= 0 ? `search-history-item-${searchHistoryIndex}` : undefined} onfocus={() => { searchInputFocused = true; if (!text.trim() && searchHistory.length) searchHistoryOpen = true; }} onblur={closeSearchPopupsOnBlur} onkeydown={handleArtistSuggestionKeys} bind:value={text} oninput={(event) => { text = event.currentTarget.value; searchHistoryIndex = -1; scheduleArtistSuggestions(text); if (text.trim() !== submitted.trim()) { cancelSearchTrackSelection(); cancelAutomationSearch(); generation++; collectionRequest++; clearCollectionDeadline(); collectingSearch = false; collectionProgress = ""; providerReads.cancelAll(); collectionReads.cancelAll(); if (collectionSoulseekSource) { void cancelSoulseek(collectionSoulseekSource); collectionSoulseekSource = null; } statuses = Object.fromEntries(Object.entries(statuses).map(([id, status]) => [id, status === "Searching…" || status === "Loading more…" ? "Cancelled" : status])); } onQueryChange(text); }} placeholder="Track, artist, album, video or set" />
      {#if artistSuggestionsOpen}<ul id="artist-search-suggestions" class="artist-search-suggestions" role="listbox" aria-label="Artist suggestions">
        {#each artistSuggestions as result, index (result.artist.provider + ":" + result.artist.id)}<li role="option" aria-selected={artistSuggestionIndex === index}><button id={`artist-search-suggestion-${index}`} type="button" tabindex="-1" onclick={() => chooseArtistSuggestion(result)}><span>{result.name}</span>{#if result.disambiguation}<small>{result.disambiguation}</small>{:else if result.country}<small>{result.country}</small>{/if}</button></li>{/each}
      </ul>{/if}
      {#if searchHistoryOpen && !text.trim() && searchHistory.length}<div id="search-history-menu" class="artist-search-suggestions search-history-menu" role="listbox" aria-label="Recent searches">
        <div class="search-history-heading"><strong>Recent searches</strong><button type="button" class="clear-search-history" onclick={clearSearchHistory}>Clear history</button></div>
        {#each searchHistory as entry, index (entry.query.toLocaleLowerCase())}<div class="search-history-row" role="option" aria-selected={searchHistoryIndex === index}><button id={`search-history-item-${index}`} class="history-query" type="button" tabindex="-1" onclick={() => restoreSearchHistory(entry)}>{entry.query}<small>{entry.mode === "library" ? "My library" : entry.sourceChoice === "custom" ? "Selected sources" : entry.sourceChoice === "all" ? "All online sources" : sources.find(source => source.id === entry.sourceChoice)?.name ?? "Online"} · {kinds.find(option => option.id === entry.kind)?.label ?? "All"}</small></button><button class="remove-search-history" type="button" aria-label={`Remove ${entry.query} from search history`} title="Remove recent search" onclick={() => removeHistoryEntry(entry)}>×</button></div>{/each}
      </div>{/if}
    </div>
    <button class="primary" type="submit" disabled={!text.trim() || !activeSourceIds().length}>Search</button>
    {#if Object.values(statuses).some(status => status === "Searching…" || status === "Loading more…" || status.endsWith("…"))}<button type="button" onclick={cancel}>Cancel</button>{/if}
  </form>
  <div class="primary-controls-toolbar">
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
        <option value="custom">Choose sources…</option>
        {#each enabledOnlineSources as source (source.id)}
          <option value={source.id} disabled={!sourceSupports(source.id)}>{source.name}{sourceSupports(source.id) ? "" : " — not available for this result type"}</option>
        {/each}
      </select>
      {#if sourceChoice === "custom"}
        <fieldset class="custom-source-picker" aria-label="Choose online sources">
          <legend>Choose online sources</legend>
          {#each enabledOnlineSources as source (source.id)}
            <label><input type="checkbox" checked={customSourceIds.includes(source.id)} onchange={(event) => { customSourceIds = event.currentTarget.checked ? [...customSourceIds, source.id] : customSourceIds.filter(id => id !== source.id); void restartSearch(); }} />{source.name}</label>
          {/each}
          {#if !activeSourceIds().length}<p role="status">Choose at least one enabled source to search.</p>{/if}
        </fieldset>
      {/if}
    {/if}
  </div>
  <div class="filters" aria-label="Result type">
    {#each visibleKinds as item}
      {@const unsupported = selectedSource && searchMode === "online" && sourceChoice !== "all" && sourceChoice !== "custom" && !sourceSupportsKind(selectedSource.id, item.id)}
      <button class:active={kind === item.id} aria-pressed={kind === item.id} disabled={Boolean(unsupported)} aria-describedby={unsupported ? "unsupported-result-kinds" : undefined} title={unsupported ? `${item.label} search is not supported by ${selectedSource?.name}` : undefined} onclick={() => { if (kind !== item.id) cancelSearchTrackSelection(); kind = item.id; if (submitted) void runSearch(); }}>{item.label}</button>
    {/each}
  </div>
  </div>
  {#if selectedSourceUnsupportedKinds.length}<p id="unsupported-result-kinds" class="scope-hint" role="note">{selectedSource?.name} does not support {selectedSourceUnsupportedKinds.map(item => item.label.toLowerCase()).join(", ")} result types.</p>{/if}
  {#if trackControlsApplicable}<div class="track-toolbar">
    <fieldset class="track-controls">
      <legend class="sr-only">Loaded track controls</legend>
    <div class="collection-controls" role="group" aria-label="Loaded track ordering">
    <label>Sort <select aria-label="Sort loaded tracks" bind:value={collectedSort}><option value="sourceOrder">Source order</option><option value="title" disabled={!loadedSortAvailability.title}>{trackSortOptionLabelForSources(loadedSortAvailability, "title", "Title", loadedSortSources)}</option><option value="artist" disabled={!loadedSortAvailability.artist}>{trackSortOptionLabelForSources(loadedSortAvailability, "artist", "Artist", loadedSortSources)}</option><option value="album" disabled={!loadedSortAvailability.album}>{trackSortOptionLabelForSources(loadedSortAvailability, "album", "Album", loadedSortSources)}</option><option value="duration" disabled={!loadedSortAvailability.duration}>{trackSortOptionLabelForSources(loadedSortAvailability, "duration", "Duration", loadedSortSources)}</option><option value="releaseDate" disabled={!loadedSortAvailability.releaseDate}>{trackSortOptionLabelForSources(loadedSortAvailability, "releaseDate", "Release date", loadedSortSources)}</option><option value="dateAdded" disabled={!loadedSortAvailability.dateAdded}>{trackSortOptionLabelForSources(loadedSortAvailability, "dateAdded", "Date added", loadedSortSources)}</option><option value="bpm" disabled={!loadedSortAvailability.bpm}>{trackSortOptionLabelForSources(loadedSortAvailability, "bpm", "BPM", loadedSortSources)}</option><option value="key" disabled={!loadedSortAvailability.key}>{trackSortOptionLabelForSources(loadedSortAvailability, "key", "Key", loadedSortSources)}</option><option value="plays" disabled={!loadedSortAvailability.plays}>{trackSortOptionLabelForSources(loadedSortAvailability, "plays", "Plays", loadedSortSources)}</option><option value="views" disabled={!loadedSortAvailability.views}>{trackSortOptionLabelForSources(loadedSortAvailability, "views", "Views", loadedSortSources)}</option></select></label>
    <button type="button" aria-label="Toggle loaded track sort direction" onclick={() => collectedDirection = collectedDirection === "asc" ? "desc" : "asc"}>{collectedDirection === "asc" ? "Ascending" : "Descending"}</button>


    </div>
      <details class="loaded-track-filters"><summary aria-label={`Search filters and playback options${minimumDuration.trim() || maximumDuration.trim() ? ", active duration filters" : ""}`}>Filters &amp; playback{#if minimumDuration.trim() || maximumDuration.trim()} <span class="active-filter-indicator">· Active</span>{/if}</summary><div class="loaded-filter-fields">
    <label class="continue-setting"><input type="checkbox" aria-label="Play remaining loaded search results" bind:checked={continueSearch} /> Play remaining loaded search results</label>
    <label>Min duration <input aria-label="Minimum loaded track duration" type="text" inputmode="decimal" placeholder="seconds or mm:ss" value={minimumDuration} oninput={(event) => minimumDuration = event.currentTarget.value} /></label>
    <label>Max duration <input aria-label="Maximum loaded track duration" type="text" inputmode="decimal" placeholder="seconds or mm:ss" value={maximumDuration} oninput={(event) => maximumDuration = event.currentTarget.value} /></label>
      </div></details>
    </fieldset>
      {#if trackControlsApplicable && (Object.values(cursors).some(Boolean) || Object.values(overflowRows).some(rows => rows.length > 0))}<button type="button" class="collect-more" onclick={() => void collectMoreSearchResults()} disabled={collectingSearch}>Collect more tracks</button>{/if}
    {#if onPlaylistTracks && loadedSearchTrackRows.length}<div class="search-playlist-selection" role="group" aria-label="Search track selection">
      {#if searchSelectionMode}<span>{selectedSearchTracks.length ? `${selectedSearchTracks.length} tracks selected` : `${loadedSearchTrackRows.length} tracks loaded`}</span><button type="button" onclick={() => selectedTrackRows = loadedSearchTrackRows}>Select loaded</button>{#if selectedSearchTracks.length}<button type="button" onclick={() => selectedTrackRows = []}>Clear selection</button>{#if onQueueTracks}<button type="button" class="add-search-queue" onclick={() => onQueueTracks(selectedSearchTracks, "append")}>Add to queue</button>{/if}<button type="button" class="add-search-selected" onclick={() => void onPlaylistTracks?.(selectedSearchTracks)}>Add selected to playlist</button>{/if}<button type="button" class="cancel-search-selection" onclick={cancelSearchTrackSelection}>Cancel</button>
      {:else}<span>{loadedSearchTrackRows.length} tracks loaded</span><button type="button" class="start-search-selection" aria-label="Select search tracks" onclick={() => searchSelectionMode = true}>Select</button>{/if}
    </div>{/if}
  </div>{/if}
  {#if collectingSearch || collectionProgress}<div class="collection-status" role="status">{collectionProgress}{#if collectingSearch}<button type="button" onclick={cancelCollection}>Cancel collection</button>{/if}</div>{/if}
  {#if selectedSourceUnsupported}<p class="scope-hint" role="status">{selectedSourceUnsupported.name} does not support {kinds.find(item => item.id === kind)?.label.toLocaleLowerCase()}. Choose another source in Search in or change the result type.</p>{/if}

  <div class="source-results" aria-live="polite">
    {#each displayedSources as source (source.id)}
      {@const rows = displayedRowsBySource[source.id] ?? []}
      {@const groupId = `search-results-${source.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
      <section class="source-group" aria-label={`${source.name} results`}>
        <header><h3><button type="button" class="source-toggle" aria-expanded={!collapsedSources[source.id]} aria-controls={groupId} aria-label={`${collapsedSources[source.id] ? "Expand" : "Collapse"} ${source.name} results`} onclick={() => collapsedSources = { ...collapsedSources, [source.id]: !collapsedSources[source.id] }}>{collapsedSources[source.id] ? "▸" : "▾"} {source.name}</button></h3><span>{statuses[source.id] ?? "Ready"}</span></header>
        <div id={groupId} hidden={Boolean(collapsedSources[source.id])}>
        {#if errors[source.id]}<p class="notice error">{errors[source.id]} <button onclick={() => void runSearch(source.id)}>Retry</button></p>{/if}
        {#each warnings[source.id] ?? [] as warning}<p class="notice">{warning}</p>{/each}
        {#if rows.length}
          <VirtualResults items={rows} itemKey={identity} estimateHeight={(row) => row.track ? 56 : 76} scrollKey={`${searchMode}:${sourceChoice}:${kind}:${submitted}:${collectedSort}:${collectedDirection}:${minimumDuration}:${maximumDuration}`} regionKey={source.id}>
          {#snippet children(row, _index)}
            {@const track = row.track}
            {@const artwork = row.sourceId === "spotify" && ["artist", "profile"].includes(row.kind) ? safeArtworkUrl(row.artworkUrl) : null}
            <article class="result-row" class:expanded={expanded[identity(row)]} oncontextmenu={track && onContext ? (event) => { event.preventDefault(); onContext?.(track, event); } : undefined}>
              <div class="result-main">
                {#if !track}<div class="result-title">{#if artwork}<span class="spotify-identity-art" aria-hidden="true"><span>♫</span><img src={artwork} alt="" loading="lazy" onerror={(event) => { (event.currentTarget as HTMLImageElement).hidden = true; }} /></span>{/if}<span class="kind-badge">{row.kind === "video" || row.kind === "set" ? "Video / set" : row.kind}</span>{#if nonTrackAction(row)}<button class="open-result" aria-label={`${nonTrackAction(row)}: ${row.title}`} title={row.title} onclick={() => openNonTrackResult(row)}><strong>{row.title}</strong><span>{nonTrackAction(row)} →</span></button>{:else}<strong class="result-row-title" title={row.title}>{row.title}</strong>{/if}</div>{/if}
                {#if !track && row.subtitle}<p class="subtitle">{row.subtitle}</p>{/if}
                {#if track}
                  {#if onPlaylistTracks && searchSelectionMode}<label class="search-track-select"><input type="checkbox" aria-label={`Select ${track.title} for playlist`} checked={selectedTrackRows.includes(row)} onchange={() => toggleSelectedSearchTrack(row)} />Select</label>{/if}
                  <div class="compact-track"><TrackList tracks={[track]} presentation="playlist" position={() => rows.slice(0, _index + 1).filter(item => item.track).length} listLabel={`${source.name} track result`} onPlay={() => playSearchResult(track, source.id, row)} onDownload={onDownload} onFavorite={onFavorite} onNext={onNext} onQueue={onQueue} onArtist={onArtistTrack} onAlbum={onTrackAlbum} onContext={onContext} onInfo={() => { expanded = { ...expanded, [identity(row)]: !expanded[identity(row)] }; }} meta={() => [source.name, track.localAvailability === "available" ? "In library" : null, track.releaseDate, track.durationSeconds ? `${Math.floor(track.durationSeconds / 60)}:${String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}` : null].filter(Boolean).join(" · ")} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} /></div>
                  {@const cachedChapterState = chapterState(row)}
                  {#if row.url && (row.kind === "video" || row.kind === "set" || source.id === "youtube" || source.id === "soundcloud") && cachedChapterState && typeof cachedChapterState === "object"}
                    <details class="chapter-details" open={Boolean(chapterExpanded[identity(row)])} ontoggle={(event) => { chapterExpanded = { ...chapterExpanded, [identity(row)]: event.currentTarget.open }; }}><summary class="chapter-badge available">Chapters · {cachedChapterState.chapters?.length ?? 0}</summary>
                    {#if chapterExpanded[identity(row)]}<div class="chapter-list">
                      {#each cachedChapterState.chapters ?? [] as chapter (chapter.id)}<div class="chapter-row"><span>{Math.floor(chapter.startTime / 60)}:{String(Math.floor(chapter.startTime % 60)).padStart(2, "0")} · {chapter.suggestedTrack ? `${chapter.suggestedTrack.artist} — ${chapter.suggestedTrack.title}` : chapter.title}</span><button onclick={() => onPlayChapter(track, cachedChapterState, chapter)}>Play</button><button onclick={() => onQueueChapter(track, cachedChapterState, chapter, "next")}>Next</button><button onclick={() => onQueueChapter(track, cachedChapterState, chapter, "append")}>Queue</button><button onclick={() => onDownloadChapter(track, cachedChapterState, chapter)}>Download</button><button onclick={() => onInfoChapter(track, cachedChapterState, chapter)}>More info</button></div>{/each}
                    </div>{/if}
                    </details>
                  {/if}
                  {#if expanded[identity(row)]}<p class="result-details">{detailLines(track).join(" · ")}</p>
                    {#if row.url && (row.kind === "video" || row.kind === "set" || source.id === "youtube" || source.id === "soundcloud") && (cachedChapterState === "loading" || cachedChapterState === "unavailable" || cachedChapterState === "unknown" || !cachedChapterState)}
                      {#if cachedChapterState === "loading"}<span class="chapter-lookup-status" role="status">Checking chapters…</span>{:else}<button class="chapter-check" onclick={() => void inspectChapters(row)}>{cachedChapterState === "unavailable" ? "Retry chapter check" : "Check for chapters"}</button>{/if}
                    {/if}
                  {/if}
                {:else if ["playlist","profile","channel"].includes(row.kind) || (row.sourceId==="spotify" && ["album","artist"].includes(row.kind))}
                  {#if ["spotify", "youtube"].includes(row.sourceId) && row.kind === "playlist" && onAddRemotePlaylist}<button class="add-playlist-result" type="button" disabled={playlistImportDone.includes(identity(row))} onclick={() => void addPlaylistResult(row)}>{playlistImportBusy.includes(identity(row)) ? "Adding…" : playlistImportDone.includes(identity(row)) ? "Added to library" : "Add to library"}</button>{#if playlistImportBusy.includes(identity(row))}<span class="notice" role="status">{playlistImportProgress[identity(row)] ?? "Adding playlist…"}</span><button type="button" onclick={() => cancelPlaylistImport(row)}>Cancel import</button>{/if}{#if playlistImportProgress[identity(row)] && !playlistImportBusy.includes(identity(row))}<p class="notice" role="status">{playlistImportProgress[identity(row)]}</p>{/if}{#if playlistImportErrors[identity(row)]}<p class="notice error" role="alert">{playlistImportErrors[identity(row)]}</p>{/if}{/if}
                {:else if row.kind === "album"}
                  {#if row.url}<a href={row.url} target="_blank" rel="noreferrer">Open source ↗</a>{/if}
                {:else}
                  <button class="details-toggle" aria-expanded={Boolean(expanded[identity(row)])} onclick={() => expanded = { ...expanded, [identity(row)]: !expanded[identity(row)] }}>{expanded[identity(row)] ? "Hide details" : "Details"}</button>
                  {#if expanded[identity(row)]}
                    {@const metadata = row.data && typeof row.data === "object" ? row.data as Record<string, unknown> : {}}
                    <p class="result-details">{[metadata.description ?? row.subtitle, metadata.creator ?? metadata.uploader, metadata.uploadDate ? `Uploaded ${metadata.uploadDate}` : null, metadata.releaseDate ? `Released ${metadata.releaseDate}` : null].filter(Boolean).join(" · ")}</p>
                  {/if}
                  {#if row.url && row.kind !== "artist"}<a href={row.url} target="_blank" rel="noreferrer">Open source ↗</a>{/if}
                {/if}
              </div>
            </article>
          {/snippet}
          </VirtualResults>
        {:else if statuses[source.id] === "Complete"}<p class="search-empty">No results from {source.name}.</p>
        {:else if statuses[source.id] === "Not available for this result type"}<p class="search-empty">{source.name} does not support this result type. Choose another source or result type.</p>{/if}
        </div>
        {#if cursors[source.id] || overflowRows[source.id]?.length}<button class="load-more" disabled={statuses[source.id] === "Loading more…"} onclick={() => void runSearch(source.id)}>Load more</button>{/if}
      </section>
    {/each}
  </div>
</section>

<style>
  .source-toggle{border:0;padding:.15rem .25rem;background:transparent;color:inherit;font:inherit;font-weight:650;text-align:left;cursor:pointer}.source-toggle:focus-visible{outline:2px solid var(--accent,#9dbfff);outline-offset:2px}
  .search-panel{--panel:var(--surface,#17191c);--border:var(--line,#454a50);display:flex;flex-direction:column;gap:.85rem;min-width:0;min-height:0;height:100%;overflow-y:auto;overflow-x:hidden;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px;container-name:search-pane;container-type:inline-size}
  .search-form,.source-group>header{display:flex;align-items:center;justify-content:space-between;gap:.75rem;min-width:0}.search-input-wrap{position:relative;flex:1;min-width:0}.search-form .search-input-wrap input{box-sizing:border-box;width:100%;min-width:0}.artist-search-suggestions{position:absolute;z-index:60;top:calc(100% + 4px);left:0;right:0;max-height:300px;overflow:auto;margin:0;padding:4px;list-style:none;border:1px solid var(--border,#454a50);border-radius:8px;background:var(--surface,#202328);box-shadow:0 8px 24px #0008}.artist-search-suggestions li{margin:0}.artist-search-suggestions button{display:flex;width:100%;flex-direction:column;align-items:flex-start;gap:2px;padding:8px;text-align:left;border:0;background:transparent}.artist-search-suggestions button:hover,.artist-search-suggestions [aria-selected="true"] button,.search-history-row[aria-selected="true"] .history-query{background:var(--raised,#2a2e34)}.artist-search-suggestions small{color:var(--muted,#a3a8ae);font-size:.75rem}.search-history-heading,.search-history-row{display:flex;align-items:center;gap:4px}.search-history-heading{justify-content:space-between;padding:4px 6px 6px;color:var(--muted,#a3a8ae);font-size:.78rem}.search-history-heading .clear-search-history{padding:3px 6px;border:0;background:transparent;color:var(--accent,#9dbfff);font-size:.74rem}.search-history-row{border-radius:6px}.search-history-row .history-query{flex:1;min-width:0}.search-history-row .remove-search-history{flex:none;width:30px;height:30px;justify-content:center;align-items:center;padding:0;border:0;background:transparent;color:var(--muted,#a3a8ae);font-size:18px}.search-history-row .remove-search-history:hover{color:var(--text,#eee)}.source-group>header h3,.source-group>header span{min-width:0;overflow-wrap:anywhere}.source-group>header span,.subtitle,.search-empty,.result-details{color:var(--muted,#a3a8ae);font-size:.82rem}.track-controls{display:contents;border:0;margin:0;padding:0;min-width:0}.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}.continue-setting{display:flex;align-items:center;gap:.45rem;font-size:.8rem;color:var(--muted,#a3a8ae)}.continue-setting input{accent-color:var(--accent,#4c78c8)}.collection-controls{display:flex;align-items:end;gap:.5rem;flex-wrap:wrap}.collection-controls label{display:flex;flex-direction:column;gap:.2rem;font-size:.72rem;color:var(--muted,#a3a8ae)}.collection-controls select{max-width:100%;font-size:.78rem}.collection-status{display:flex;align-items:center;gap:.5rem;font-size:.8rem;color:var(--muted,#a3a8ae)}
  .continue-setting{flex-direction:row;margin:0}.continue-setting input{width:auto;flex:none;margin:0;padding:0}.collection-controls label{margin:0}.search-in-label{margin:0}
  button,input,select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}button{cursor:pointer}button:disabled,select:disabled{opacity:.55;cursor:default}.primary{background:var(--accent,#4c78c8);border-color:var(--accent,#4c78c8);color:var(--base,#fff)}.search-form input{flex:1;min-width:0;height:36px;padding:.35rem .6rem}.search-form button{min-height:36px;padding:.35rem .6rem}.filters{display:flex;gap:.4rem;flex-wrap:wrap}.filters button{font-size:.84rem}.filters button.active{border-color:var(--accent,#9dbfff);color:var(--accent,#9dbfff)}
  .scope-controls{display:grid;grid-template-columns:auto auto minmax(0,1fr);align-items:center;gap:.55rem;min-width:0}.search-mode{display:flex;min-width:0;padding:3px;gap:2px;border:1px solid var(--border,#454a50);border-radius:9px;background:var(--surface,#202328)}.search-mode button{min-width:0;padding:.4rem .65rem;border:0;background:transparent;font-size:.82rem;white-space:nowrap}.search-mode button.active{background:var(--accent,#4c78c8);color:var(--base,#fff)}.search-in-label{font-size:.8rem;color:var(--muted,#a3a8ae);white-space:nowrap}.scope-controls select{box-sizing:border-box;width:100%;min-width:0;max-width:100%;height:34px;padding:0 .55rem;line-height:normal;overflow:visible;font-size:.82rem}.scope-controls select option{background:var(--panel,#17191c);color:var(--text,#eee)}.library-helper{grid-column:2/-1;min-width:0;margin:0;color:var(--muted,#a3a8ae);font-size:.76rem;line-height:1.35}.scope-hint{margin:0;color:var(--muted,#a3a8ae);font-size:.8rem;line-height:1.4}
  .source-results{display:flex;flex-direction:column;gap:.65rem;min-width:0}.source-group{min-width:0;max-width:100%;border:1px solid var(--border,#383d43);border-radius:10px;padding:.55rem}.source-group>header{padding:0 .15rem .35rem}.source-group h3{margin:0;font-size:.93rem}.result-row{min-width:0;border-top:1px solid var(--border,#30343a);padding:.5rem .15rem}.result-main{min-width:0;overflow-wrap:anywhere}.result-title{display:flex;align-items:center;gap:.5rem;min-width:0}.result-title strong{font-size:.92rem;min-width:0;overflow-wrap:anywhere}.result-row-title{flex:1 1 auto}.spotify-identity-art{position:relative;display:grid;place-items:center;flex:0 0 32px;width:32px;height:32px;overflow:hidden;border-radius:50%;background:var(--surface,#202328);color:var(--muted,#a3a8ae)}.spotify-identity-art img{position:absolute;inset:0;width:100%;height:100%;object-fit:cover}.kind-badge{display:inline-flex;flex:none;border-radius:999px;background:var(--surface,#202328);padding:.18rem .48rem;color:var(--muted,#a3a8ae);font-size:.69rem}.subtitle{margin:.2rem 0 0 .1rem;overflow-wrap:anywhere}.open-result{display:inline-flex;align-items:baseline;flex:1 1 auto;gap:.45rem;max-width:100%;min-width:0;border:0;padding:.2rem 0;background:none;text-align:left;color:inherit;font-weight:600;white-space:normal;overflow-wrap:anywhere}.open-result strong{flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.open-result span{flex:none;color:var(--accent,#9dbfff);font-size:.76rem}.result-main>a{display:inline-block;margin-top:.35rem;color:var(--accent,#9dbfff);font-size:.78rem;overflow-wrap:anywhere}.details-toggle{margin-top:.4rem;padding:.3rem .45rem;font-size:.75rem}.result-details{margin:.35rem 0;overflow-wrap:anywhere}.chapter-details{margin-top:.4rem;max-width:100%}.chapter-badge{display:inline-flex;margin-top:.4rem;border-radius:999px;background:var(--surface,#202328);padding:.2rem .55rem;color:var(--muted,#a3a8ae);font-size:.72rem;white-space:normal;overflow-wrap:anywhere}.chapter-badge.available{color:var(--accent,#9dbfff);cursor:pointer}.chapter-check{margin:.25rem 0;padding:.2rem .4rem;font-size:.72rem;color:var(--muted,#a3a8ae)}.chapter-lookup-status{margin:.25rem;color:var(--muted,#a3a8ae);font-size:.72rem}.chapter-list{margin-top:.55rem;padding:.45rem;border-left:2px solid var(--accent,#9dbfff);min-width:0}.chapter-row{display:flex;align-items:center;gap:.3rem;margin-top:.35rem;min-width:0;flex-wrap:wrap}.chapter-row span{flex:1 1 10rem;min-width:0;font-size:.78rem;overflow-wrap:anywhere}.chapter-row button{font-size:.76rem;padding:.35rem .5rem}.notice{margin:.35rem 0;font-size:.8rem;color:var(--muted,#a3a8ae);overflow-wrap:anywhere}.error{color:#ff9b9b}.load-more{margin:.4rem}.search-empty{padding:.25rem;overflow-wrap:anywhere}.source-group a{color:var(--accent,#9dbfff)}
  .search-playlist-selection{display:flex;align-items:center;gap:.5rem;padding:.4rem .55rem;border:1px solid var(--border,#454a50);border-radius:8px;margin:.4rem 0}.search-playlist-selection span{margin-right:auto}.search-track-select{display:inline-flex;align-items:center;gap:.25rem;font-size:.75rem;color:var(--muted,#a3a8ae)}.add-playlist-result{margin:.4rem .45rem 0 0}
  .result-row:has(.compact-track){padding:.15rem}
  @media(max-width:640px){.search-panel{padding:.7rem}.search-form{flex-wrap:wrap}.search-form .search-input-wrap{flex:1 1 100%}.search-form input{flex-basis:100%}.scope-controls{grid-template-columns:auto minmax(0,1fr)}.search-mode{grid-column:1/-1;justify-self:start}.library-helper{grid-column:1/-1}.search-mode button{padding:.4rem .5rem}.chapter-row{flex-wrap:wrap}.chapter-row span{flex-basis:100%}}
  @media(max-width:640px){.result-row:has(.compact-track){padding:0 .15rem}}
  @container search-pane (max-width:640px){.search-form{align-items:flex-start;flex-wrap:wrap}.search-form input{flex:1 1 100%}.scope-controls{grid-template-columns:minmax(0,1fr) minmax(0,2fr)}.search-mode{grid-column:1/-1;justify-self:start}.search-in-label{grid-column:1}.scope-controls select{grid-column:2}.library-helper{grid-column:1/-1}.source-group>header{align-items:flex-start;flex-wrap:wrap}.result-title{align-items:flex-start}.chapter-row span{flex-basis:100%}}
  @media(min-width:641px){.search-panel{gap:.45rem;padding:.75rem}.scope-controls{display:flex;justify-content:flex-start;gap:.45rem;min-width:0;flex-wrap:wrap}.scope-controls select{width:260px;max-width:min(260px,45vw);flex:0 1 260px}.library-helper{margin:0}.filters{gap:.3rem}.filters button{padding:.32rem .55rem;font-size:.78rem}.track-toolbar{display:flex;align-items:center;gap:.5rem;flex-wrap:wrap;min-width:0}.track-toolbar .collection-controls{align-items:center;gap:.4rem}.track-toolbar .collection-controls label{display:flex;flex-direction:row;align-items:center;gap:.35rem;font-size:.76rem;white-space:nowrap}.track-toolbar .collection-controls select{box-sizing:border-box;height:34px;min-height:34px;padding:0 .55rem;font-size:.78rem;line-height:normal;overflow:visible}.track-toolbar .collection-controls button{min-height:32px;padding:.25rem .55rem;font-size:.76rem}.loaded-track-filters{flex:none;min-width:0}.loaded-track-filters[open]{flex:1 1 100%}.loaded-track-filters>summary{list-style:none;cursor:pointer;white-space:nowrap;border:1px solid var(--border,#454a50);border-radius:7px;padding:.35rem .55rem;font-size:.78rem;color:var(--muted,#a3a8ae)}.loaded-track-filters>summary::-webkit-details-marker{display:none}.loaded-track-filters>summary::after{content:" ▾"}.loaded-track-filters[open]>summary::after{content:" ▴"}.active-filter-indicator{color:var(--accent,#9dbfff)}.loaded-filter-fields{display:flex;align-items:end;gap:.6rem;flex-wrap:wrap;max-width:100%;margin-top:.45rem;padding:.65rem;border:1px solid var(--border,#454a50);border-radius:8px;background:var(--panel,#17191c)}.loaded-filter-fields>label:not(.continue-setting){display:flex;flex-direction:column;gap:.2rem;font-size:.72rem;color:var(--muted,#a3a8ae)}.loaded-filter-fields input:not([type="checkbox"]){box-sizing:border-box;width:132px;min-width:0;height:32px;padding:.25rem .45rem;font-size:.76rem}.loaded-filter-fields .continue-setting{max-width:250px;line-height:1.3}.track-toolbar .collect-more{flex:none;margin:0;min-height:32px;padding:.3rem .6rem;font-size:.78rem}.collection-status{flex-wrap:wrap}}
  @media(min-width:641px){.primary-controls-toolbar{display:flex;align-items:center;gap:.5rem;flex-wrap:wrap;min-width:0}.primary-controls-toolbar .scope-controls{flex:0 1 auto}.primary-controls-toolbar .filters{flex:1 1 auto;align-items:center}.track-toolbar .search-playlist-selection{flex:0 1 auto;min-width:0;max-width:100%;margin:0;padding:0;gap:.3rem;flex-wrap:wrap;border:0;font-size:.76rem}.track-toolbar .search-playlist-selection span{margin-right:.2rem}.track-toolbar .search-playlist-selection button{min-height:30px;padding:.25rem .45rem;font-size:.76rem}}
</style>
