<script lang="ts">
  import { onMount, tick } from "svelte";
  import SourceLink from "./SourceLink.svelte";
  import SourcePresets from "./SourcePresets.svelte";
  import {playlistTitle} from "./lib/playlist-title";
  import RadioView, {type RadioStation,type RadioTrackMetadata} from "./RadioView.svelte";
  import PlaybackQuality from "./PlaybackQuality.svelte";
  import type {PlaylistSection} from "./lib/playlist-sections";
  const DOWNLOADS_PLAYLIST_ID="00000000-0000-4000-8000-000000000001";
  let sidebarLayout=$state<{sections:PlaylistSection[];assignments:Record<string,string>;names?:Record<string,string>}>({sections:[],assignments:{}});
  let radioStations=$state<RadioStation[]>([]);
  let selectedRadioStationId=$state("main");
  let showAddRadio=$state(false);
  let renamePlaylistName=$state(""),renamingPlaylist=$state(false),importTargetCollectionId=$state<string|null>(null);
  let selectionAnchorId=$state<string|null>(null);

  import TrackMenu from "./TrackMenu.svelte";
  import PlaylistMenu from "./PlaylistMenu.svelte";
  let radioActions=$state<{station:RadioStation;anchor:{x:number;y:number}}|null>(null);
  let showReleaseSetup=$state(false);
  let renameTarget=$state<{kind:"playlist"|"radio";id:string;name:string}|null>(null);
  let sidebarRenameValue=$state("");
  let playlistActions = $state<{ collection: Collection; anchor: { x: number; y: number } } | null>(null);
  let seekSerial = $state(0);
  let downloadSubmitting = $state(false);
  let systemPausedTrack = $state(false);
  let actionTrack = $state<Track | null>(null);
  let actionAnchor = $state<{ x: number; y: number } | null>(null);
  let deletedPlaylist = $state<Collection | null>(null);
  let deletedCollections = $state<Collection[] | null>(null);
  let showTrackDetails = $state(false);
  let showCreatePlaylist = $state(false);
  import ReleaseView from "./ReleaseView.svelte";
  import SearchPanel from "./SearchPanel.svelte";
  import AutotagSetup from "./AutotagSetup.svelte";
  import AutotagReview, { type AutotagJob } from "./AutotagReview.svelte";
  import { virtualChapterTrack, chapterAbsolutePosition, chapterRelativePosition } from "./lib/chapters";
  import ReleaseSetup from "./ReleaseSetup.svelte";
  import ToolSetup from "./ToolSetup.svelte";
  import ConnectionsSetup from "./ConnectionsSetup.svelte";
  import LocalPlaylistFiles from "./LocalPlaylistFiles.svelte";
  import MetadataReview from "./MetadataReview.svelte";
  import ArtistPage from "./ArtistPage.svelte";
  import AlbumSearch from "./AlbumSearch.svelte";
  import FallbackSettings from "./FallbackSettings.svelte";
  import { streamingProviderOrder, downloadProviderOrder, type StreamingProvider } from "./lib/source-fallback";
  import PlayingInfo from "./PlayingInfo.svelte";
  import TrackInfoSearch from "./TrackInfoSearch.svelte";
  import PlaybackQueuePanel from "./PlaybackQueuePanel.svelte";
  import DownloadActivity from "./DownloadActivity.svelte";
  import { downloadStateLabel, downloadSourceLabel, downloadPeer, downloadTitle, downloadTransferDetail } from "./lib/download-display";
  import LibrarySidebar from "./LibrarySidebar.svelte";
  import { PlaybackQueue, savePlaybackQueue, loadPlaybackQueue } from "./lib/playback-queue";
  import { artistPlaybackTracks } from "./lib/artist-playback";
  import type { ArtistRef, ArtistSearchPage, ArtistProfile, ArtistReleasePage, ArtistReleaseRef, ArtistTrackPage, ArtistCollaborationPage, MetadataReview as MetadataReviewData, MetadataFieldKey } from "./lib/artist-types";
  import { playbackAction } from "./lib/playback-action";
  import { verifiedBatchSource } from "./lib/batch";
  import { LatestRequest } from "./lib/latest-request";
  import { choosePreviewCandidate, isLinkedAudioSearchCandidate, plausiblePreviewCandidate, playlistUrlCandidate } from "./lib/preview-source";
  import { sourcePosition, playlistPosition } from "./lib/source-position";
  import { rowWindow } from "./lib/table";
  import { call, desktop, playable, listen } from "./lib/api";
  import type {
    Track, Chapter,
    Genre,
    Chart,
    Preview,
    Candidate,
    Collection,
    DownloadJob,
    Settings,
    Dependency,
    LocalAsset,
    ChartMembership,
    BrowserStatus, BrowserImportResult, CandidatePreviewStatus,
  } from "./lib/types";
  type View =
    | "charts"
    | "source-browser"
    | "radio"
    | "search"
    | "favorites"
    | "collections"
    | "downloads"
    | "library"
    | "settings"
    | "artist"
    | "history";
  let view = $state<View>("library"),
    provider = $state("traxsource"),
    genres = $state<Genre[]>([]),
    genreUrl = $state(""),
    chart = $state<Chart | null>(null),
    snapshots = $state<Chart[]>([]);
  let tracks = $state<Track[]>([]),
    selected = $state<Track | null>(null),
    filter = $state(""),
    localFilter = $state("all" as "all" | "local" | "not-local"),
    query = $state(""),
    sort = $state("rank"),
    descending = $state(false);
  let searchMode = $state<"audio" | "youtube" | "soundcloud" | "soulseek">("audio"),
    soulseekSearchId = $state(""),
    soulseekStatus = $state(""),
    slskdEnabled = $state(false),
    audioSearchTrackIds = $state<Set<string>>(new Set()),
    downloadTrack = $state<Track | null>(null),
    downloadCandidates = $state<Candidate[]>([]),
    downloadCandidate = $state<Candidate | null>(null),
    metadataReviewOpen = $state(false),
    downloadMetadata = $state({ artist: "", title: "", version: "", album: "", label: "", genre: "", bpm: "", key: "", releaseDate: "", isrc: "", catalogNumber: "" }),
    downloadLoading = $state(false),
    rowDownloadBusy = $state<Record<string, boolean>>({}),
    rowDownloadFeedback = $state<Record<string, string>>({}),
    downloadRequest = 0;
  let busy = $state(0),
    error = $state(""),
    notice = $state(""),
    collections = $state<Collection[]>([]),
    collectionId = $state(""),
    newCollection = $state(""),
    renamedCollection = $state(""),
    jobs = $state<DownloadJob[]>([]),
    assets = $state<LocalAsset[]>([]),
    dependencies = $state<Dependency[]>([]);
  let checked = $state<string[]>([]),
    mergeConfirmed = $state(false),
    sourceUrl = $state(""),
    sourceProvider = $state("direct"),
    outputProfile = $state("source");
  let candidateFormats = $state<Record<string, string>>({}),
    batchRunning = $state(false),
    batchStatus = $state(""),
    analysisStatuses = $state<{ assetId: string; inputHash?: string; version?: number; status: string; bpm?: number | null; key?: string | null; error?: string; payload?: Record<string, unknown> | null }[]>([]),
    analysisBusy = $state(false);
  let batchCancelled = false;
  const candidateKey = (c: Candidate) => `${c.provider}:${c.id}`;
  let memberships = $state<ChartMembership[]>([]),
    membershipTrackId = $state(""),
    membershipLoading = $state(false),
    membershipError = $state("");
  let candidates = $state<Candidate[]>([]),
    importText = $state(""),
    importKind = $state("tracklist"),
    showImport = $state(false),
    localPath = $state("");
  let browserUrl = $state(""), browserMessage = $state(""), browserRequest = $state(""),
    browserStatus = $state<BrowserStatus | null>(null), browserImports = $state<BrowserImportResult[]>([]),
    activeBrowserImport = $state<BrowserImportResult | null>(null),
    activeArtistTrack = $state<Track | null>(null),
    activeArtistName = $state(""),
    activeArtistRef = $state<ArtistRef | null>(null),
    playlistTrack = $state<Track | null>(null);
  let settings = $state<Settings>({
    musicDirectory: "",
    autoPreview: false,
    previewDelayMs: 250,
    theme: "dark",
    preferredDownloadSource: "youtube",
    preferredStreamingSource: "youtube",
    overridePlaylistTrackUrls: false,
    streamingFallbackEnabled: true,
    streamingFallbackSources: ["youtube", "soundcloud"],
    downloadFallbackEnabled: false,
    downloadFallbackSources: ["monochrome", "slskd", "youtube", "soundcloud"],
    downloadFallbackAllowLossy: false,
    ytDlpPath: "yt-dlp",
    ffmpegPath: "ffmpeg",
    ffprobePath: "ffprobe",
    jsRuntimePath: "deno",
  });
  let persistedMusicDirectory = $state("");
  let automationPermissions = $state({ control: true, manage: false });
  let playerHeight = $state(96);
  let sidebarWidth = $state(200), infoPanelWidth = $state(340), screenWidth = $state(1440);
  let sourceCapabilities = $state<Record<string, boolean>>({ soulseek: false, discogs: false });
  let availableSources = $state<{id:string;name:string;kinds?:string[];enabled?:boolean;detail?:string}[]>([]);
  let queueCandidates = $state<Record<string, Candidate>>({});
  let autotagJobs = $state<AutotagJob[]>([]), autotagReviewId = $state<string | null>(null);
  let autotagSettings = $state({enabled:true,shazamFallback:true,fpcalcAvailable:false,songrecAvailable:false,acoustidConfigured:false});
  let crossedChapterSession = "";
  function saveLayoutWidth(kind: "sidebar"|"info", width:number) {
    if (kind === "sidebar") sidebarWidth=Math.max(160,Math.min(320,width)); else infoPanelWidth=Math.max(280,Math.min(420,width));
    try {localStorage.setItem(`burrata.layout.${kind}.v1`,String(kind === "sidebar" ? sidebarWidth : infoPanelWidth));}catch{}
  }
  function updatePlaybackPosition(absolute:number) {
    position=absolute;
    const range=playingTrack?.playbackRange;
    if (range && !paused && absolute>=range.endTime && crossedChapterSession!==playbackSession) {
      crossedChapterSession=playbackSession;
      audio?.pause();
      if (preview?.sessionId) void call("candidate_preview_control",{action:"pause",value:null}).catch(()=>{});
      void advanceQueue(1,true);
    }
    persistPlaybackPosition();
  }
  async function seekPlayer(relative:number) {
    const value=playingTrack ? chapterAbsolutePosition(playingTrack,relative) : relative;
    if (preview?.sessionId) await call("candidate_preview_control",{action:"seek",value}); else if(audio) audio.currentTime=value;
    position=value;seekSerial++;void publishAutomationPlayback();
  }
  function rememberQueueCandidate(parent:Track,candidate:Candidate) {queueCandidates={...queueCandidates,[parent.id]:candidate};}
  function playChapter(parent:Track,candidate:Candidate,chapter:Chapter) {
    rememberQueueCandidate(parent,candidate);const track=virtualChapterTrack(parent,candidate,chapter);
    if(track) void startPreview(track,false,false,[track],0);
  }
  function searchMoreInfo(track:Track,candidate:Candidate|null=null,chapterTitle?:string) {
    infoSearchTarget={track,candidate,chapterTitle};
  }
  async function openCatalogInfoSearch(value:string) {
    infoSearchTarget=null;query=value;catalogSearchRequest++;
    await navigate("search");
  }
  function openWebInfoSearch(value:string) {
    void call("open_external_url",{url:`https://www.google.com/search?q=${encodeURIComponent(value)}`});
  }
  function openInfoSource(url:string) {
    if(/^https?:\/\//i.test(url))void call("open_external_url",{url});
  }
  async function queueChapter(parent:Track,candidate:Candidate,chapter:Chapter,mode:"next"|"append") {
    rememberQueueCandidate(parent,candidate);const track=virtualChapterTrack(parent,candidate,chapter);
    if(track) { try {const registered=desktop()?await call<Track>("register_chapter_track",{track}):track;queueArtistTracks([registered],mode);}catch(failure){error=String(failure);} }
  }
  async function downloadChapter(parent:Track,candidate:Candidate,chapter:Chapter) {
    rememberQueueCandidate(parent,candidate);
    await openDownloadChooser(parent,candidate,chapter.id);
  }
  function candidateForQueue(parent:Track):Candidate|null {return queueCandidates[parent.id] ?? importedAudioCandidate(parent);}
  async function inspectQueueChapters(parent:Track) {
    if(parent.playbackRange || queueCandidates[parent.id]?.chapters) return;
    const candidate=importedAudioCandidate(parent);
    if(!candidate || !["youtube","soundcloud"].includes(candidate.provider))return;
    try {rememberQueueCandidate(parent,await call<Candidate>("inspect_youtube_source",{url:candidate.url}));}catch{}
  }
  async function queueAllChapters(parent:Track,chapters:Chapter[],mode:"next"|"append"="append") {
    const candidate=candidateForQueue(parent);if(!candidate)return;
    const list=chapters.map(ch=>virtualChapterTrack(parent,candidate,ch)).filter((t):t is Track=>Boolean(t));
    try {const registered=desktop()?await Promise.all(list.map(track=>call<Track>("register_chapter_track",{track}))):list;queueArtistTracks(registered,mode);}catch(failure){error=String(failure);}
  }
  async function loadAutotagState() {
    if(!desktop())return;
    [autotagSettings,autotagJobs]=await Promise.all([call<typeof autotagSettings>("get_autotag_settings"),call<AutotagJob[]>("get_autotag_jobs")]);
  }
  async function openSearchAlbum(release:ArtistReleaseRef) {
    searchRelease=release;searchReleaseTracks=[];searchReleaseError="";searchReleaseBusy=true;
    try {
      let artist=release.credits.find(credit=>credit.artist)?.artist;
      if(!artist) {
        const found=await searchArtists(release.credits.map(c=>c.name).join(" "),1);
        artist=found.results.find(a=>a.name.toLowerCase()===release.credits[0]?.name.toLowerCase())?.artist;
      }
      if(!artist)throw new Error("This release has no unambiguous artist identity. Open its source to inspect it.");
      const rows=await getReleaseTracks(release,artist);
      if(searchRelease?.sourceUrl===release.sourceUrl)searchReleaseTracks=rows;
    }catch(failure){searchReleaseError=String(failure);}finally{searchReleaseBusy=false;}
  }
  let importDateFilter = $state("");
  let showPlayingInfo = $state(false);
  let infoTrack = $state<Track | null>(null);
  let infoSearchTarget = $state<{ track: Track; candidate: Candidate | null; chapterTitle?: string } | null>(null);
  let catalogSearchRequest = $state(0);
  let showDownloads = $state(false);
  let pausedMirrors = $state<string[]>([]), localPlaylistIds = $state<string[]>([]);
  let albumQuery = $state("");
  let searchRelease = $state<ArtistReleaseRef | null>(null), searchReleaseTracks = $state<Track[]>([]), searchReleaseBusy = $state(false), searchReleaseError = $state("");
  let librarySource = $state("all"), searchCategory = $state("tracks"), showQueue = $state(false);
  let historyItems = $state<{ id: string; track: Track; provider: string; sessionId: string; playedAt: string; chapter?: unknown }[]>([]);
  let queueIndex = $state(-1);
  let queueTracks = $state<Track[]>([]), repeatMode = $state<"off" | "all" | "one">("off"), shuffleEnabled = $state(false);
  const playbackQueue = new PlaybackQueue();
  let playbackSession = "", recordedSession = "";
  const PLAYBACK_POSITION_KEY = "burrata.playback-position.v1";
  const MAX_AUTOMATIC_QUEUE_SKIPS = 3;
  let resumeTrackId = "", resumePosition = 0, pendingResumePosition = 0;
  let playingCandidate = $state<Candidate | null>(null), splitChapters = $state(false), selectedChapterIds = $state<string[]>([]);
  let focusedDownloadChapterId = $state<string | null>(null);
  let mirrorStatus = $state("");
  let artistScroll = $state(0);
  let artistSection = $state<"overview" | "releases" | "tracks" | "collaborations" | "local">("overview");
  type Route = { view: View; collectionId: string; provider: string; genreUrl: string; query: string; filter: string; localFilter: "all" | "local" | "not-local"; sort: string; descending: boolean; librarySource: string; searchCategory: string; searchMode: typeof searchMode; importId?: string; artistName: string; artistRef: ArtistRef | null; artistTrack: Track | null; artistSection: typeof artistSection; artistScroll: number; chartId?: string; selected: Track | null; scroll: number };
  let backRoutes = $state<Route[]>([]), forwardRoutes = $state<Route[]>([]);
  let restoringRoute = false;
  const artworkCache = new Map<string, { path: string; url: string }>();
  const artworkFilePaths = new Map<string, string>();
  let radioBusy = $state<string | null>(null);
  let radioRequest = 0;
  let failedArtwork = $state<Record<string, boolean>>({});
  let playingTrack = $state<Track | null>(null),
    preview = $state<Preview | null>(null),
    previewStatus = $state(""),
    audio = $state<HTMLAudioElement>(),
    paused = $state(true),
    position = $state(0),
    duration = $state(0),
    volume = $state(0.8),
    table = $state<HTMLDivElement>();
  let scrollTop = $state(0),
    viewportHeight = $state(600);
  const rowHeight = 60,
    overscan = 8;
  const previewRequests = new LatestRequest();
  let audioRecovery: Promise<void> | null = null;
  let activeAutomationPlayback: string | null = null;
  let lastMediaFailure: string | null = null;
  const warmedTrackIds = new Map<string, number>();
  let automaticPreviewRecovery: {
    request: number;
    track: Track;
    tried: Set<string>;
    attempts: number;
    exactSourceOnly?: boolean;
    failureReason?: string;
  } | undefined;
  let selectionRequest = 0,
    navigationRequest = 0,
    searchRequest = 0,
    autoTimer: ReturnType<typeof setTimeout> | undefined;
  let activeContext = "charts:";
  const contexts = new Map<
    string,
    {
      tracks: Track[];
      selected: Track | null;
      filter: string;
      sort: string;
      descending: boolean;
      scroll: number;
    }
  >();
  const views: Exclude<View, "artist" | "history">[] = [
    "charts",
    "search",
    "favorites",
    "collections",
    "downloads",
    "library",
    "settings",
  ];
  function displayPosition(track: Track) { return view === "collections" ? playlistPosition(track) ?? sourcePosition(track) : sourcePosition(track); }
  const sorted = $derived.by(() => {
    const q = filter.toLowerCase();
    const list = tracks.filter((t) => {
      const availability = (t as Track & { localAvailability?: string }).localAvailability;
      const isLocal = availability ? availability === "available" : Boolean(t.localPath);
      const matchesLocal = localFilter === "all" || (localFilter === "local" ? isLocal : !isLocal);
      const matchesSource = view !== "library" || librarySource === "all" || (librarySource === "local" ? isLocal : librarySource === "personal" ? true : t.provider === librarySource);
      return matchesLocal && matchesSource && `${t.artist} ${t.title} ${t.version ?? ""} ${t.label ?? ""} ${t.genre ?? ""}`
        .toLowerCase().includes(q);
    });
    return list.sort((a, b) => {
      const x = sort === "rank" ? displayPosition(a) : a[sort as keyof Track],
        y = sort === "rank" ? displayPosition(b) : b[sort as keyof Track];
      let d =
        x == null
          ? 1
          : y == null
            ? -1
            : typeof x === "number" && typeof y === "number"
              ? x - y
              : String(x).localeCompare(String(y));
      return descending ? -d : d;
    });
  });
  const virtualized = $derived(sorted.length > 1000);
  const windowRows = $derived(
    rowWindow(sorted.length, scrollTop, viewportHeight, rowHeight, overscan),
  );
  const startRow = $derived(windowRows.start);
  const endRow = $derived(windowRows.end);
  const visibleTracks = $derived(sorted.slice(startRow, endRow));
  const currentCollection = $derived(
    collections.find((c) => c.id === collectionId),
  );
  function sourceUrlForCollection(id: string): string | null {
    const imported = latestImports.find((item) => item.collectionId === id);
    const collection = collections.find((item) => item.id === id) as (Collection & { sourceUrl?: string | null; sourceProvider?: string | null; provenance?: unknown }) | undefined;
    const provenance = collection?.provenance && typeof collection.provenance === "object" ? collection.provenance as Record<string, unknown> : {};
    const source = imported?.sourceUrl || collection?.sourceUrl || (typeof provenance.sourceUrl === "string" ? provenance.sourceUrl : "");
    try { const parsed = new URL(source); return ["https:", "http:"].includes(parsed.protocol) ? parsed.href : null; } catch { return null; }
  }
  const currentCollectionSourceUrl = $derived(view === "collections" ? sourceUrlForCollection(collectionId) : null);
  const currentChartSourceUrl = $derived(view === "charts" ? (activeBrowserImport?.sourceUrl || chart?.sourceUrl || null) : null);
  async function openSourceToRefresh(url: string) {
    if (!url) return;
    await run(async () => {
      await call("open_external_url", { url });
      notice = "Opened the source page. Refresh it, then use the browser extension to capture a new import.";
    });
  }
  $effect(() => {
    const message = notice;
    if (!message) return;
    const timer = setTimeout(() => { if (notice === message) notice = ""; }, 5500);
    return () => clearTimeout(timer);
  });
  async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
    busy++;
    error = "";
    try {
      return await fn();
    } catch (e) {
      error = String(e);
    } finally {
      busy--;
    }
  }
  const time = (s?: number | null) =>
    s == null || !Number.isFinite(s)
      ? "—"
      : `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  function captureRoute(): Route {
    return { view, collectionId, provider, genreUrl, query, filter, localFilter, sort, descending, librarySource, searchCategory, searchMode, importId: view === "charts" ? activeBrowserImport?.id : undefined, artistName: activeArtistName, artistRef: activeArtistRef, artistTrack: activeArtistTrack, artistSection, artistScroll, chartId: view === "charts" ? chart?.id : undefined, selected, scroll: table?.scrollTop ?? 0 };
  }
  function rememberRoute() {
    if (!restoringRoute) { backRoutes = [...backRoutes.slice(-99), captureRoute()]; forwardRoutes = []; }
  }
  async function travelRoute(direction: "back" | "forward") {
    const stack = direction === "back" ? backRoutes : forwardRoutes;
    const route = stack[stack.length - 1]; if (!route) return;
    const current = captureRoute();
    if (direction === "back") { backRoutes = backRoutes.slice(0, -1); forwardRoutes = [...forwardRoutes, current]; }
    else { forwardRoutes = forwardRoutes.slice(0, -1); backRoutes = [...backRoutes, current]; }
    restoringRoute = true;
    try {
      collectionId = route.collectionId; provider = route.provider; genreUrl = route.genreUrl; query = route.query;
      activeArtistName = route.artistName; activeArtistRef = route.artistRef; activeArtistTrack = route.artistTrack; artistSection = route.artistSection; artistScroll = route.artistScroll; librarySource = route.librarySource; searchCategory = route.searchCategory; searchMode = route.searchMode; albumQuery = route.query;
      await navigate(route.view);
      if (route.importId) { const saved = browserImports.find((item) => item.id === route.importId); if (saved) await openBrowserImport(saved); }
      else if (route.view === "charts" && route.chartId) await loadSnapshot(route.chartId);
      if (route.view === "charts") await loadGenres(false);
      filter = route.filter; localFilter = route.localFilter; sort = route.sort; descending = route.descending; selected = route.selected;
      requestAnimationFrame(() => { if (table) table.scrollTop = route.scroll; });
    } finally { restoringRoute = false; }
  }
  const latestImports = $derived.by(() => {
    const seen = new Set<string>();
    return browserImports.filter((item) => !seen.has(item.sourceUrl) && Boolean(seen.add(item.sourceUrl)));
  });
  const sourceCollections = $derived(collections.filter((item) => {
    const source = latestImports.find((entry) => entry.collectionId === item.id)?.provider;
    return librarySource === "all" || (librarySource === "local" ? localPlaylistIds.includes(item.id) : librarySource === "personal" ? !source && !localPlaylistIds.includes(item.id) : source === librarySource);
  }).map((item) => ({ ...item, trackCount: latestImports.find((entry) => entry.collectionId === item.id)?.capturedCount ?? item.trackCount })));
  async function openPlaylist(id: string) {
    if (id === "favorites" || id === "history") { await navigate(id); return; }
    const imported = latestImports.find((item) => item.collectionId === id);
    if (imported) { await openBrowserImport(imported); return; }
    rememberRoute(); collectionId = id;
    restoringRoute = true; try { await navigate("collections"); } finally { restoringRoute = false; }
  }
  async function importPreset(url:string) {
    browserUrl=url;showImport=false;
    await requestBrowserImport();
    if(view==="source-browser")notice=browserMessage;
  }
  async function browseProviderCharts(source: string) {
    if(["shazam","lastfm","bandcamp"].includes(source)) {
      await navigate("source-browser");provider=source;activeBrowserImport=null;chart=null;tracks=[];collectionId="";return;
    }
    await navigate("charts"); provider = source; activeBrowserImport = null; chart = null; tracks = []; genreUrl = ""; genres = []; await loadGenres();
  }
  async function chooseLibrarySource(source: string) {
    rememberRoute(); restoringRoute = true;
    try { await navigate("library"); librarySource = source; } finally { restoringRoute = false; }
  }
  async function syncMirrors() {
    if (!desktop()) return;
    try {
      const result = await call<{ synchronized: number; warnings: string[]; mirrorsDirectory?: string; pausedMirrors?: string[]; sources?: {collectionId?: string | null}[] }>("sync_playlists");
      mirrorStatus = result.warnings?.join(" · ") ?? "";
      pausedMirrors = result.pausedMirrors ?? []; localPlaylistIds = result.sources?.map((source) => source.collectionId).filter((id): id is string => Boolean(id)) ?? [];
      collections = await call<Collection[]>("list_collections");
    } catch (failure) { mirrorStatus = String(failure); }
  }
  function refreshQueue() { queueIndex = playbackQueue.currentIndex; queueTracks = [...playbackQueue.tracks]; repeatMode = playbackQueue.repeat; savePlaybackQueue(playbackQueue); }
  async function advanceQueue(direction: number, ended = false, failedSkips = 0, requestId: string | null = null) {
    if (!ended && direction > 0 && repeatMode === "one") { playbackQueue.setRepeat("off"); }
    const next = direction < 0 ? playbackQueue.previous() : playbackQueue.next();
    playbackQueue.setRepeat(repeatMode); refreshQueue();
    if (next) { selected = next; await startPreview(next, true, ended, undefined, undefined, failedSkips, requestId); } else paused = true;
  }
  function persistPlaybackPosition() {
    if (!playingTrack || typeof localStorage === "undefined") return;
    resumeTrackId = playingTrack.id;
    resumePosition = Math.max(0, Number.isFinite(position) ? position : 0);
    try { localStorage.setItem(PLAYBACK_POSITION_KEY, JSON.stringify({ trackId: resumeTrackId, position: resumePosition, savedAt: Date.now() })); } catch { /* Storage can be unavailable or full. */ }
  }
  function restorePlaybackPosition() {
    if (typeof localStorage === "undefined") return;
    try {
      const value = JSON.parse(localStorage.getItem(PLAYBACK_POSITION_KEY) ?? "null") as { trackId?: unknown; position?: unknown; savedAt?: unknown } | null;
      if (!value || typeof value.trackId !== "string" || !Number.isFinite(value.position) || Number(value.position) <= 0 ||
          !Number.isFinite(value.savedAt) || Date.now() - Number(value.savedAt) > 30 * 24 * 60 * 60 * 1000 ||
          playbackQueue.current?.id !== value.trackId) return;
      resumeTrackId = value.trackId;
      resumePosition = Number(value.position);
      position = resumePosition;
      previewStatus = `Ready to resume from ${time(resumePosition)}`;
    } catch { /* Ignore corrupt or inaccessible saved state. */ }
  }
  async function seekPendingResume(request: number) {
    const target = pendingResumePosition;
    const player = audio;
    if (!target || !player) return;
    if (player.readyState < 1) {
      await new Promise<void>((resolve) => {
        const finish = () => { player.removeEventListener("loadedmetadata", finish); player.removeEventListener("error", finish); resolve(); };
        player.addEventListener("loadedmetadata", finish, { once: true });
        player.addEventListener("error", finish, { once: true });
        setTimeout(finish, 2000);
      });
    }
    if (!previewRequests.isCurrent(request) || player !== audio) return;
    const limit = Number.isFinite(player.duration) ? player.duration : 0;
    if (target > 0 && (!limit || target < limit - 1)) {
      try { player.currentTime = target; position = target; } catch { /* Some remote streams cannot seek yet. */ }
    }
  }
  async function recordSuccessfulPlayback() {
    if (!playingTrack || !playbackSession || recordedSession === playbackSession) return;
    recordedSession = playbackSession;
    if (playingCandidate) void rememberAudioSource(playingTrack, playingCandidate);
    const chapter = playingCandidate?.chapters?.find((item) => position >= item.startTime && position < item.endTime);
    await call("record_playback", { sessionId: playbackSession, provider: playingCandidate?.provider ?? preview?.provider ?? playingTrack.provider, track: playingTrack, chapter: chapter ?? null }).catch((failure) => { mirrorStatus = `History: ${String(failure)}`; recordedSession = ""; });
  }
  async function rememberAudioSource(track: Track, candidate: Candidate) {
    try {
      const updated = await call<Track>("save_audio_match", { track, candidate });
      const remembered = updated.provenance as Record<string, unknown>;
      const merge = (item: Track) => ({ ...item, provenance: { ...(item.provenance as Record<string, unknown>), audioCandidate: remembered.audioCandidate, candidateUrl: remembered.candidateUrl, audioMatchStatus: remembered.audioMatchStatus } });
      if (playingTrack?.id === track.id) playingTrack = merge(playingTrack);
      tracks = tracks.map((item) => item.id === track.id ? merge(item) : item);
    } catch { /* A source can play even when its metadata cannot be saved. */ }
  }
  async function prepareCandidateDetails(track: Track, candidate: Candidate, request: number) {
    let inspected = candidate;
    if (candidate.provider === "youtube" && !candidate.chapters) {
      try { inspected = await call<Candidate>("inspect_candidate", { track, candidate }); } catch { /* Playback can continue without extra metadata. */ }
    }
    if (!previewRequests.isCurrent(request)) return;
    playingCandidate = inspected;
    if (recordedSession === playbackSession) void rememberAudioSource(track, inspected);
    if (!track.artworkUrl && inspected.artworkUrl) {
      const updated = { ...track, artworkUrl: inspected.artworkUrl };
      if (playingTrack?.id === track.id) playingTrack = updated;
      tracks = tracks.map((item) => item.id === track.id ? updated : item);
    }
    if (playingTrack?.artworkUrl) void cacheCover(playingTrack);
  }
  function updateArtwork(track: Track, artworkUrl: string) {
    if (artworkUrl.startsWith("https://")) artworkFilePaths.delete(track.id);
    if (playingTrack?.id === track.id) { playingTrack = { ...playingTrack, artworkUrl }; void publishAutomationPlayback(); }
    if (selected?.id === track.id) selected = { ...selected, artworkUrl };
  }
  async function cacheCover(track: Track) {
    const assetId = assetIdForTrack(track);
    if (assetId) {
      try {
        const embedded = await call<string | null>("cache_asset_artwork", { assetId });
        if (embedded) { artworkFilePaths.set(track.id, embedded); updateArtwork(track, playable(embedded)); return; }
      } catch { /* Fall through to public artwork. */ }
    }
    const url = track.artworkUrl; if (!url || !url.startsWith("https://")) return;
    try {
      let cached = artworkCache.get(url);
      if (!cached) { const path = await call<string>("cache_artwork", { url }); cached = { path, url: playable(path) }; artworkCache.set(url, cached); }
      artworkFilePaths.set(track.id, cached.path);
      updateArtwork(track, cached.url);
    } catch { /* The image error handler can try a source thumbnail. */ }
  }
  function recoverArtwork(track: Track) {
    const failed = track.artworkUrl; if (!failed) return;
    failedArtwork = { ...failedArtwork, [failed]: true };
    const candidate = playingTrack?.id === track.id ? playingCandidate : importedAudioCandidate(track);
    if (candidate?.provider !== "youtube") return;
    try {
      const url = new URL(candidate.url);
      const id = url.hostname === "youtu.be" ? url.pathname.slice(1) : url.searchParams.get("v");
      if (!id || !/^[A-Za-z0-9_-]+$/.test(id)) return;
      const alternate = ["hqdefault", "mqdefault"].map((size) => `https://i.ytimg.com/vi/${id}/${size}.jpg`).find((value) => value !== failed && !failedArtwork[value]);
      if (alternate) updateArtwork(track, alternate);
    } catch { /* Keep the placeholder when the source has no artwork. */ }
  }
  const playerPosition = $derived(playingTrack ? chapterRelativePosition(playingTrack, position) : position);
  const playerDuration = $derived(playingTrack?.playbackRange ? playingTrack.playbackRange.endTime-playingTrack.playbackRange.startTime : duration);
  const chaptersByTrack = $derived(Object.fromEntries(queueTracks.map(track=>[track.id,candidateForQueue(track)?.chapters ?? (track.id===playingTrack?.id ? playingCandidate?.chapters ?? [] : [])])));
  const currentTagJob = $derived(autotagJobs.find(job=>job.assetId===autotagReviewId));
  const currentTagAsset = $derived(assets.find(asset=>asset.id===autotagReviewId));
  const downloadTagStatuses = $derived.by(()=>Object.fromEntries(jobs.flatMap(job=>{
    const asset=assets.find(item=>item.path===job.outputPath);if(!asset)return[];
    const tagging=autotagJobs.find(item=>item.assetId===asset.id);if(!tagging)return[];
    const status=tagging.status;
    const review=["review","approved","no_match","failed","retry"].includes(status);
    const label=["applied","completed"].includes(status)?"Tagged":review?status==="review"||status==="approved"?"Needs review":"Recognition unavailable":status==="disabled"?"Automatic tagging off":"Tagging…";
    return [[job.id,{label,assetId:asset.id,review}]];
  })));
  const activeChapter = $derived(playingCandidate?.chapters?.find((item) => position >= item.startTime && position < item.endTime));
  const playerAudioSource = $derived.by(() => {
    if (!preview || !["audio", "local", "direct"].includes(preview.kind)) return "";
    if (preview.kind === "local") return "Local file";
    const providerId = preview.provider || (preview.isExcerpt ? playingTrack?.provider : playingCandidate?.provider);
    const names: Record<string, string> = { youtube: "YouTube", soundcloud: "SoundCloud", monochrome: "Monochrome", beatport: "Beatport", traxsource: "Traxsource", apple: "Apple Music", itunes: "Apple Music" };
    let source = providerId ? names[providerId] : undefined;
    if (!source) { try { source = new URL(preview.url).hostname; } catch { /* No source identity available. */ } }
    return source ? `${preview.isExcerpt ? "Preview from" : "Streaming from"} ${source}` : "Streaming audio";
  });
  const playingTitle = $derived(playingTrack?.playbackRange ? playingTrack.title : activeChapter?.title ?? playingTrack?.title ?? preview?.title ?? "Nothing playing");
  function seekChapter(start: number) { void seekPlayer(playingTrack ? chapterRelativePosition(playingTrack,start) : start); }
  async function executeSearch() {
    if (searchCategory === "artists") await searchArtistDiscography();
    else if (searchCategory === "albums") albumQuery = query;
    else if (searchCategory === "catalog") await search();
    else await findAudio();
  }
  async function searchArtistDiscography() {
    if (!query.trim()) return;
    activeArtistName = query.trim(); activeArtistRef = null; activeArtistTrack = null;
    artistSection = "releases"; artistScroll = 0;
    await navigate("artist");
  }
  const contextKey = () =>
    `${view}:${view === "collections" ? collectionId : ""}`;
  async function navigate(next: View, recordRoute = true) {
    if (recordRoute) rememberRoute();
    notice = "";
    contexts.set(activeContext, {
      tracks,
      selected,
      filter,
      sort,
      descending,
      scroll: table?.scrollTop ?? 0,
    });
    if (next === "collections" && !collectionId && collections.length) {
      collectionId = [...collections].sort((a, b) => b.trackCount - a.trackCount)[0].id;
    }
    view = next;
    activeContext = contextKey();
    selectionRequest++;
    candidates = [];
    clearTimeout(autoTimer);
    const cached = contexts.get(contextKey());
    checked = [];
    mergeConfirmed = false;
    tracks = cached?.tracks ?? [];
    selected = cached?.selected ?? null;
    memberships = [];
    membershipTrackId = "";
    membershipLoading = false;
    membershipError = "";
    if (selected) void loadMemberships(selected, selectionRequest);
    filter = cached?.filter ?? "";
    localFilter = "all";
    sort = cached?.sort ?? "rank";
    descending = cached?.descending ?? false;
    const request = ++navigationRequest;
    if (["favorites", "collections", "library"].includes(next))
      await run(async () => {
        const result = await call<Track[]>("list_tracks", {
          view: next,
          collectionId: next === "collections" ? collectionId : null,
        });
        if (request === navigationRequest) {
          tracks = result;
          warmPreviewTracks(result.slice(0, 3));
        }
      });
    if (next === "history") await run(async () => { historyItems = await call<typeof historyItems>("list_playback_history", { limit: 100, offset: 0 }); });
    if (next === "downloads") await refreshJobs();
    if (next === "library")
      await run(async () => {
        const [savedAssets, statuses] = await Promise.all([
          call<LocalAsset[]>("list_assets"), call<typeof analysisStatuses>("get_analysis_status"),
        ]);
        assets = savedAssets; analysisStatuses = statuses;
      });
    if (next === "settings")
      await run(
        async () =>
          (dependencies = await call<Dependency[]>("check_dependencies")),
      );
    requestAnimationFrame(() => {
      if (table) table.scrollTop = cached?.scroll ?? 0;
    });
  }
  async function loadGenres(selectDefault = true) {
    const p = provider;
    if(!["beatport","traxsource"].includes(p)){genres=[];return;}
    await run(async () => {
      const result = await call<Genre[]>("list_genres", { providerId: p });
      if (p === provider) {
        genres = result;
        if (selectDefault) {
          genreUrl =
            result.find((g) => g.name.toLowerCase() === "deep house")?.chartUrl ??
            result[0]?.chartUrl ??
            "";
        }
      }
    });
  }
  async function loadChart(refresh = false) {
    if (!genreUrl) return;
    selectionRequest++;
    clearTimeout(autoTimer);
    const request = ++navigationRequest,
      p = provider,
      url = genreUrl;
    await run(async () => {
      const result = await call<Chart>("get_chart", {
        providerId: p,
        url,
        refresh,
      });
      if (request !== navigationRequest) return;
      chart = result;
      activeBrowserImport = null;
      checked = [];
      mergeConfirmed = false;
      tracks = result.tracks;
      warmPreviewTracks(result.tracks.slice(0, 3));
      selected = null;
      candidates = [];
      notice = result.warning ?? (result.stale ? "Showing cached chart." : "");
      snapshots = await call<Chart[]>("list_snapshots", { providerId: p, url });
    });
  }
  async function loadSnapshot(id: string, expectedNavigation?: number) {
    await run(async () => {
      const loaded = await call<Chart>("load_snapshot", { id });
      if (expectedNavigation !== undefined &&
          (expectedNavigation !== navigationRequest || view !== "charts")) return;
      chart = loaded;
      activeBrowserImport = null;
      tracks = chart.tracks;
      warmPreviewTracks(chart.tracks.slice(0, 3));
      selected = null;
      candidates = [];
      checked = [];
      mergeConfirmed = false;
      selectionRequest++;
      clearTimeout(autoTimer);
      scrollTop = 0;
      if (table) table.scrollTop = 0;
    });
  }
  async function loadMemberships(t: Track, request: number) {
    memberships = [];
    membershipTrackId = t.id;
    membershipLoading = true;
    membershipError = "";
    try {
      const result = await call<ChartMembership[]>("get_chart_memberships", {
        trackId: t.id,
      });
      if (request === selectionRequest && selected?.id === t.id)
        memberships = result;
    } catch (e) {
      if (request === selectionRequest && selected?.id === t.id)
        membershipError = String(e);
    } finally {
      if (request === selectionRequest && selected?.id === t.id)
        membershipLoading = false;
    }
  }
  function select(t: Track, source: "click" | "navigation" = "click") {
    if (radioBusy && radioBusy !== t.id) { radioRequest++; radioBusy = null; }
    selected = t;
    candidates = [];
    const request = ++selectionRequest;
    void loadMemberships(t, request);
    clearTimeout(autoTimer);
    if (source === "click") {
      void startPreview(t);
    } else if (settings.autoPreview) {
      autoTimer = setTimeout(
        () => void startPreview(t),
        settings.previewDelayMs,
      );
    }
    const index = sorted.findIndex((track) => track.id === t.id);
    if (index >= 0) warmPreviewTracks(sorted.slice(index + 1, index + 3));
  }

  function warmPreviewTracks(items: Track[]) {
    const now = Date.now();
    for (const [id, warmedAt] of warmedTrackIds) {
      if (now - warmedAt > 4 * 60_000) warmedTrackIds.delete(id);
    }
    while (warmedTrackIds.size > 128) {
      const oldest = warmedTrackIds.keys().next().value;
      if (!oldest) break;
      warmedTrackIds.delete(oldest);
    }
    const fresh = items.filter((track) => !warmedTrackIds.has(track.id)).slice(0, 3);
    if (!fresh.length) return;
    fresh.forEach((track) => warmedTrackIds.set(track.id, now));
    while (warmedTrackIds.size > 128) {
      const oldest = warmedTrackIds.keys().next().value;
      if (!oldest) break;
      warmedTrackIds.delete(oldest);
    }
    void call<void>("warm_track_previews", { tracks: fresh }).catch(() => {
      fresh.forEach((track) => {
        if (warmedTrackIds.get(track.id) === now) warmedTrackIds.delete(track.id);
      });
    });
  }

  function importedAudioCandidate(track: Track): Candidate | undefined {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown> : {};
    const value = provenance.audioCandidate;
    if (!value || typeof value !== "object") return undefined;
    const candidate = value as Candidate;
    return ["youtube", "soundcloud", "monochrome"].includes(candidate.provider) &&
      typeof candidate.id === "string" && typeof candidate.url === "string" &&
      typeof candidate.title === "string" ? candidate : undefined;
  }
  function linkedSearchCandidate(track: Track): Candidate | undefined {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown> : {};
    const raw = provenance.audioCandidate && typeof provenance.audioCandidate === "object"
      ? provenance.audioCandidate as Candidate : undefined;
    if (raw?.provider === "slskd" && raw.source?.kind === "soulseek" && raw.source.username?.trim() && raw.source.filename?.trim()) return raw;
    const web = importedAudioCandidate(track);
    return web && isLinkedAudioSearchCandidate(track, web) ? web : undefined;
  }
  function requiresDownloadReview(track: Track, candidate: Candidate): boolean {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown> : {};
    return candidate.confidence !== "high" || provenance.sourceKind === "audioSearch" || audioSearchTrackIds.has(track.id);
  }
  function trackSourceLabel(track: Track): string {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown> : {};
    return provenance.sourceKind === "audioSearch" ? "Search result source" : "Catalog source";
  }
  function bpmAnnotation(track: Track): string {
    return analysisAnnotation(track, "bpm");
  }
  function downloadBpmAnnotation(): string {
    const enteredBpm = String(downloadMetadata.bpm ?? "").trim();
    if (!downloadTrack || !bpmAnnotation(downloadTrack) || !enteredBpm) return "";
    return Math.abs(Number(enteredBpm) - Number(downloadTrack.bpm)) <= 0.1
      ? bpmAnnotation(downloadTrack) : "";
  }
  function downloadKeyAnnotation(): string {
    if (!downloadTrack || !analysisAnnotation(downloadTrack, "key") || !downloadMetadata.key) return "";
    return downloadMetadata.key.trim().toLowerCase() === String(downloadTrack.key ?? "").trim().toLowerCase()
      ? analysisAnnotation(downloadTrack, "key") : "";
  }

  function safePreviewError(reason: unknown): string {
    const message = reason instanceof Error ? reason.message : String(reason ?? "");
    return message.replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, 180) || "No playable audio stream was returned.";
  }
  function mediaAudioFailureReason(): string {
    const code = audio?.error?.code;
    if (code === 2) return "The source could not be loaded because of a network error.";
    if (code === 3) return "The source audio could not be decoded.";
    if (code === 4) return "The source did not provide a supported audio format.";
    return audio?.error?.message || "The selected source did not provide playable audio.";
  }

  async function resolveCandidateAudio(t: Track, candidate: Candidate, request: number) {
    if (!previewRequests.isCurrent(request)) return false;
    if (candidate.provider === "monochrome") {
      const inspected = await previewRequests.resolve(request, call<Candidate>("inspect_candidate", { track: t, candidate }));
      if (!inspected || !previewRequests.isCurrent(request)) return false;
      candidate = inspected;
    }
    const result = await previewRequests.resolve(
      request,
      call<Preview>("resolve_candidate_audio", { candidate }),
    );
    if (!result || result.kind !== "audio" || !result.url || !previewRequests.isCurrent(request)) {
      if (previewRequests.isCurrent(request) && automaticPreviewRecovery?.request === request && automaticPreviewRecovery.track.id === t.id) {
        automaticPreviewRecovery.failureReason = "The resolver returned no playable audio stream.";
      }
      return false;
    }
    preview = result;
    playingTrack = t;
    playingCandidate = candidate;
    void prepareCandidateDetails(t, candidate, request);
    paused = true;
    position = pendingResumePosition;
    duration = 0;
    previewStatus = `Loading ${candidate.provider} audio…`;
    await tick();
    if (!previewRequests.isCurrent(request) || playingTrack?.id !== t.id || !audio) return false;
    audio.volume = volume;
    try {
      await seekPendingResume(request);
      await audio.play();
      if (!previewRequests.isCurrent(request)) return false;
      pendingResumePosition = 0;
      paused = false;
      previewStatus = `Playing ${candidate.provider} audio`;
      return true;
    } catch (failure) {
      if (previewRequests.isCurrent(request)) {
        if (automaticPreviewRecovery?.request === request && automaticPreviewRecovery.track.id === t.id) {
          automaticPreviewRecovery.failureReason = `Playback could not start: ${safePreviewError(failure)}`;
        }
        preview = null;
        paused = true;
      }
      return false;
    }
  }

  async function findAndStartPreviewSource(t: Track, request: number, recovering = false) {
    const found: Candidate[] = [];
    const provenance = t.provenance && typeof t.provenance === "object"
      ? t.provenance as Record<string, unknown> : {};
    const isExactAudioSearch = Boolean(t.playbackRange) || ["audioSearch", "youtubeRadio"].includes(String(provenance.sourceKind));
    const previous = recovering && automaticPreviewRecovery?.track.id === t.id ? automaticPreviewRecovery : undefined;
    automaticPreviewRecovery = { request, track:t, tried:previous?.tried ?? new Set(), attempts:previous?.attempts ?? 0, exactSourceOnly:isExactAudioSearch };
    const cached = playlistUrlCandidate(t, importedAudioCandidate(t));
    const pinnedSource = isExactAudioSearch && !(t.provider === "import" && settings.overridePlaylistTrackUrls && !t.playbackRange && provenance.sourceKind !== "youtubeRadio");
    automaticPreviewRecovery.exactSourceOnly = pinnedSource;
    if (pinnedSource) {
      if (!cached || (t.playbackRange ? cached.url !== t.playbackRange.sourceUrl : !isLinkedAudioSearchCandidate(t, cached))) {
        previewStatus = "This search result has no valid linked audio source.";
        return;
      }
      found.push(cached); candidates = [...found];
      automaticPreviewRecovery.tried.add(candidateKey(cached));
      automaticPreviewRecovery.attempts++;
      try { if (await resolveCandidateAudio(t, cached, request)) return; }
      catch (failure) { if (automaticPreviewRecovery?.request === request) automaticPreviewRecovery.failureReason = safePreviewError(failure); }
      if (!previewRequests.isCurrent(request)) return;
      preview = null; paused = true;
      previewStatus = `The selected ${cached.provider} source could not play: ${automaticPreviewRecovery?.failureReason ?? "The source did not provide playable audio."}`;
      return;
    }
    const primary = (["monochrome", "soundcloud"].includes(settings.preferredStreamingSource ?? "") ? settings.preferredStreamingSource : "youtube") as StreamingProvider;
    const providers = streamingProviderOrder(primary, settings.streamingFallbackEnabled !== false, settings.streamingFallbackSources ?? ["youtube", "soundcloud"]);
    for (const providerId of providers) {
      if (!previewRequests.isCurrent(request)) return;
      const label = providerId === "youtube" ? "YouTube" : providerId === "monochrome" ? "Monochrome" : "SoundCloud";
      let providerAttempts = [...automaticPreviewRecovery.tried].filter(key => key.startsWith(`${providerId}:`)).length;
      if (providerAttempts >= (providerId === "monochrome" ? 1 : 2)) continue;
      if (cached?.provider === providerId && !automaticPreviewRecovery.tried.has(candidateKey(cached)) && (t.provider === "import" && isLinkedAudioSearchCandidate(t, cached) || choosePreviewCandidate(t, [cached], providerId))) {
        found.push(cached); candidates = [...found];
        automaticPreviewRecovery.tried.add(candidateKey(cached));
        automaticPreviewRecovery.attempts++; providerAttempts++;
        previewStatus = `Loading ${label} audio…`;
        try { if (await resolveCandidateAudio(t, cached, request)) return; } catch { /* Search another match in this source. */ }
        if (!previewRequests.isCurrent(request)) return;
      }
      previewStatus = `Searching ${label}…`;
      let results: Candidate[] | undefined;
      try { results = await previewRequests.resolve(request, call<Candidate[]>("find_candidates", { track:t, providerIds:[providerId], requestGeneration:request })); }
      catch { continue; }
      if (!results || !previewRequests.isCurrent(request)) return;
      found.push(...results); candidates = [...found];
      const eligible = t.provider === "import" && isExactAudioSearch ? results.filter(candidate => candidate.confidence === "high") : results;
      while (previewRequests.isCurrent(request) && providerAttempts < (providerId === "monochrome" ? 1 : 2)) {
        const match = choosePreviewCandidate(t, eligible, providerId, automaticPreviewRecovery.tried);
        if (!match) break;
        automaticPreviewRecovery.tried.add(candidateKey(match)); automaticPreviewRecovery.attempts++; providerAttempts++;
        previewStatus = `Loading ${label} audio…`;
        try { if (await resolveCandidateAudio(t, match, request)) return; } catch { /* Try the next configured source. */ }
        if (!previewRequests.isCurrent(request)) return;
      }
    }
    if (!previewRequests.isCurrent(request)) return;
    preview = null;
    paused = true;
    previewStatus = found.length
      ? "No playable audio match found. Open a source link to listen on its site."
      : "No audio preview is available. Open a source link to listen on its site.";
  }

  async function startPreview(t: Track, fromQueue = false, automaticContinuation = false, displayedTracks?: Track[], displayedIndex?: number, failedSkips = 0, automationRequestId: string | null = null) {
    if (radioBusy && radioBusy !== t.id) { radioRequest++; radioBusy = null; }
    activeAutomationPlayback = automationRequestId;
    systemPausedTrack = false;
    audioRecovery = null;
    if (!fromQueue) {
      const list = displayedTracks ?? (view === "history" ? historyItems.map((item) => item.track) : sorted.some((item) => item.id === t.id) ? [...sorted] : [t]);
      const index = displayedIndex ?? list.findIndex((item) => item === t);
      playbackQueue.setTracks(list, index >= 0 ? index : Math.max(0, list.findIndex((item) => item.id === t.id)));
      playbackQueue.setRepeat(repeatMode); if (shuffleEnabled) playbackQueue.shuffle(); refreshQueue();
    }
    pendingResumePosition = !automaticContinuation && resumeTrackId === t.id ? resumePosition : t.playbackRange?.startTime ?? 0;
    crossedChapterSession="";
    playbackSession = crypto.randomUUID(); recordedSession = ""; playingCandidate = null;
    clearTimeout(autoTimer);
    const request = previewRequests.next();
    automaticPreviewRecovery = { request, track: t, tried: new Set(), attempts: 0 };
    audio?.pause();
    if (preview?.sessionId) await call("stop_candidate_preview", { sessionId: preview.sessionId }).catch((e) => (error = String(e)));
    if (!previewRequests.isCurrent(request)) return;
    paused = true;
    preview = null;
    if(t.playbackRange && desktop()) {try {t=await call<Track>("register_chapter_track",{track:t});}catch(failure){previewStatus=String(failure);return;}}
    playingTrack = t;
    position = pendingResumePosition;
    duration = 0;
    previewStatus = "Loading preview…";
    const linked = importedAudioCandidate(t); if (linked) { playingCandidate = linked; void prepareCandidateDetails(t, linked, request); }
    if (t.artworkUrl || assetIdForTrack(t)) void cacheCover(t);
    if(t.provider==="radioparadise" || t.provider==="internet-radio" || (t.provider==="radio" && (t.provenance as {sourceKind?:string})?.sourceKind==="radio")) {
      const station=radioStations.find(item=>item.id===t.providerId);if(!station){previewStatus="Unknown radio station";return;}
      playingTrack={...t,title:station.name,artist:station.provider==="radio_paradise"?"Radio Paradise":"Internet radio",sourceUrl:station.homepageUrl??station.sourceUrl};
      preview={kind:"audio",url:station.streamUrl,canSeek:false,canVolume:true,canAutoplay:true,isExcerpt:false,provider:t.provider,title:station.name,audioCodec:station.codec??undefined,audioBitrateKbps:station.bitrateKbps??undefined};position=0;pendingResumePosition=0;
      await tick();if(!previewRequests.isCurrent(request)||!audio)return;
      try{audio.volume=volume;await audio.play();paused=false;previewStatus=`Playing ${station.name}`;}catch(failure){preview=null;paused=true;previewStatus=`Radio could not play: ${safePreviewError(failure)}`;}return;
    }
    await run(async () => {
      let result: Preview | undefined;
      try {
        result = await previewRequests.resolve(
          request,
          call<Preview>("resolve_preview", { track: t }),
        );
      } catch {
        if (previewRequests.isCurrent(request)) await findAndStartPreviewSource(t, request);
        return;
      }
      if (!result || !previewRequests.isCurrent(request)) return;
      if (["audio", "local", "direct"].includes(result.kind)) {
        preview = result;
        await tick();
        if (!previewRequests.isCurrent(request)) return;
        if (audio) {
          audio.volume = volume;
          try {
            await seekPendingResume(request);
            await audio.play();
            if (previewRequests.isCurrent(request)) {
              paused = false;
              pendingResumePosition = 0;
              previewStatus = result.isExcerpt ? "Playing preview" : "Playing";
              return;
            }
          } catch {
            if (!previewRequests.isCurrent(request)) return;
          }
        }
      }
      if (previewRequests.isCurrent(request)) await findAndStartPreviewSource(t, request);
    });
    if (automaticContinuation && previewRequests.isCurrent(request) && (!preview || paused)) {
      pendingResumePosition = 0;
      const next = playbackQueue.nextAfterFailedStart(failedSkips, MAX_AUTOMATIC_QUEUE_SKIPS);
      if (next) {
        selected = next;
        refreshQueue();
        await startPreview(next, true, true, undefined, undefined, failedSkips + 1);
      } else {
        paused = true;
        previewStatus = `Could not start ${t.title}; stopped after skipping ${MAX_AUTOMATIC_QUEUE_SKIPS} unavailable tracks.`;
      }
    }
  }
  function recoverAudioFailure(event: Event) {
    if (event.currentTarget !== audio || !playingTrack || !preview ||
        !["audio", "local", "direct"].includes(preview.kind)) return;
    const track = playingTrack, oldRequest = automaticPreviewRecovery;
    const request = previewRequests.next();
    audio?.pause();
    preview = null;
    paused = true;
    previewStatus = "Audio preview failed. Trying another match…";
    lastMediaFailure = mediaAudioFailureReason();
    audioRecovery = run(async () => {
      if (!oldRequest || oldRequest.track.id !== track.id) {
        automaticPreviewRecovery = { request, track, tried: new Set(), attempts: 0 };
        await findAndStartPreviewSource(track, request);
        return;
      }
      oldRequest.request = request;
      if (oldRequest.exactSourceOnly) {
        oldRequest.failureReason = lastMediaFailure ?? mediaAudioFailureReason();
        previewStatus = `The selected ${importedAudioCandidate(track)?.provider ?? "audio"} source could not play: ${oldRequest.failureReason}`;
        return;
      }
      await findAndStartPreviewSource(track, request, true);
    });
  }
  function togglePlayback() {
    const action = playbackAction(
      selected?.id,
      playingTrack?.id,
      Boolean(preview && (audio || preview.sessionId)),
    );
    if (action === "start-selected" && selected) {
      void startPreview(selected);
      return;
    }
    if (action === "toggle-current" && preview?.sessionId) {
      void run(() => call("candidate_preview_control", { action: paused ? "play" : "pause", value: null }));
    } else if (action === "toggle-current" && audio && preview) {
      if (audio.paused) void audio.play().catch((e) => (error = String(e)));
      else audio.pause();
    } else if (selected) void startPreview(selected);
    else if (playbackQueue.current) void startPreview(playbackQueue.current, true);
  }
  function toggleCurrentPlayback() {
    if (preview?.sessionId) void run(() => call("candidate_preview_control", { action: paused ? "play" : "pause", value: null }));
    else if (audio && preview) { if (audio.paused) void audio.play().catch((failure) => (error = String(failure))); else audio.pause(); }
    else if (playingTrack) void startPreview(playingTrack, true);
    else if (playbackQueue.current) void startPreview(playbackQueue.current, true);
    else if (selected) void startPreview(selected);
  }
  async function showPlayingTrackInfo() {
    const track = playingTrack;
    if (!track) return;
    if (view !== "library") await navigate("library");
    selected = tracks.find((item) => item.id === track.id) ?? track;
    const request = ++selectionRequest;
    void loadMemberships(selected, request);
  }
  function adjacent(delta: number) {
    const index = sorted.findIndex((t) => t.id === selected?.id);
    const t = sorted[Math.max(0, Math.min(sorted.length - 1, index + delta))];
    if (t) {
      select(t, "navigation");
      requestAnimationFrame(() => scrollToTrack(t.id));
    }
  }
  function scrollToTrack(id: string) {
    if (!table) return;
    if (!virtualized) {
      table.querySelector("tr.selected")?.scrollIntoView({ block: "nearest" });
      return;
    }
    const index = sorted.findIndex((t) => t.id === id),
      top = index * rowHeight;
    if (top < table.scrollTop) table.scrollTop = top;
    else if (top + rowHeight > table.scrollTop + table.clientHeight - 38)
      table.scrollTop = top + rowHeight - table.clientHeight + 38;
    scrollTop = table.scrollTop;
  }
  async function favorite(t: Track) {
    await run(async () => {
      await call("set_favorite", { track: t, value: !t.favorite });
      t.favorite = !t.favorite;
      tracks = [...tracks];
    });
  }
  async function disposition(t: Track, value: string) {
    await run(async () => {
      await call("set_disposition", { track: t, value });
      t.disposition = value;
      tracks = [...tracks];
    });
  }
  function automationPlaybackSnapshot() {
    return { trackId: playingTrack?.id ?? null, track: playingTrack ? { id: playingTrack.id, artist: playingTrack.artist, title: playingTitle, album: playingTrack.album, artworkUrl: playingTrack.artworkUrl, artworkFilePath: artworkFilePaths.get(playingTrack.id) ?? null, localPath: playingTrack.localPath, durationSeconds: playingTrack.durationSeconds, sourceUrl: playingTrack.sourceUrl } : null,
      paused, position:playerPosition, duration:playerDuration, volume, seekSerial, canSeek: Boolean(preview?.canSeek), canVolume: Boolean(preview?.canVolume), shuffle: shuffleEnabled, repeat: repeatMode, previewKind: preview?.kind ?? null, playerMessage: previewStatus, readyState: audio?.readyState ?? null, mediaError: audio?.error?.message ?? null, lastMediaFailure, status: preview ? (paused ? "paused" : "playing") : systemPausedTrack ? "paused" : "idle", queue: queueTracks.map((track) => ({ id: track.id, artist: track.artist, title: track.title })), currentIndex: queueIndex };
  }
  async function publishAutomationPlayback() {
    if (desktop()) await call("automation_playback_state", { playback: automationPlaybackSnapshot() }).catch(() => {});
  }
  async function handleAutomationPlayback(request: { requestId: string; action: string; trackIds?: string[]; mode?: string | null; position?: number | null; volume?: number | null }) {
    let failure: string | null = null;
    if (["play", "resume", "next", "previous", "openUri"].includes(request.action)) activeAutomationPlayback = request.requestId;
    try {
      if (request.action === "play" || request.action === "queue") {
        const library = await call<Track[]>("list_tracks", { view: "library", collectionId: null });
        const requested = (request.trackIds ?? []).map((id) => library.find((track) => track.id === id));
        if (!requested.length || requested.some((track) => !track)) throw new Error("Unknown or empty track IDs");
        const list = requested as Track[];
        if (request.action === "play") {
          if (activeAutomationPlayback !== request.requestId) throw new Error("Playback request was cancelled or superseded");
          playbackQueue.setTracks(list, 0); refreshQueue(); await startPreview(list[0]!, true, false, undefined, undefined, 0, request.requestId);
          if (audioRecovery) await audioRecovery;
          if (playingTrack?.id !== list[0]!.id || !preview || paused) throw new Error(previewStatus || "No playable audio found");
        } else {
          if (request.mode === "replace" || !queueTracks.length) playbackQueue.setTracks(list, 0);
          else if (request.mode === "next") for (const track of [...list].reverse()) playbackQueue.playNext(track);
          else playbackQueue.restore({ ...playbackQueue.snapshot(), tracks: [...queueTracks, ...list] });
          refreshQueue();
        }
      } else if (request.action === "pause" || request.action === "resume") {
        const action = request.action === "pause" ? "pause" : "play";
        if (preview?.sessionId) await call("candidate_preview_control", { action, value: null });
        else if (audio && preview) { if (action === "pause") audio.pause(); else await audio.play(); }
        else if (action === "play" && (playingTrack || playbackQueue.current)) await startPreview((playingTrack || playbackQueue.current)!, true, false, undefined, undefined, 0, request.requestId);
        else if (action === "play") throw new Error("Nothing is queued to play");
        if (action === "pause") { previewRequests.next(); activeAutomationPlayback = null; }
        paused = action === "pause";
      } else if (request.action === "seek") {
        const seconds = playingTrack ? chapterAbsolutePosition(playingTrack, Number(request.position)) : Number(request.position);
        if (!Number.isFinite(seconds) || seconds < 0 || !preview?.canSeek) throw new Error("This audio cannot seek");
        if (preview.sessionId) await call("candidate_preview_control", { action: "seek", value: seconds });
        else if (audio) audio.currentTime = seconds;
        else throw new Error("Player is not ready");
        position = seconds; seekSerial++;
      } else if (request.action === "volume") {
        const value = Number(request.volume);
        if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error("Volume must be from 0 to 1");
        volume = value;
        if (audio) audio.volume = value;
        if (preview?.sessionId) await call("candidate_preview_control", { action: "volume", value });
      } else if (request.action === "stop") {
        await closePreview(); position = 0; persistPlaybackPosition();
      } else if (request.action === "shuffle") {
        const enabled = request.mode === "true";
        if (enabled !== shuffleEnabled) { shuffleEnabled = enabled; if (enabled) playbackQueue.shuffle(); refreshQueue(); }
      } else if (request.action === "repeat") {
        if (!["off", "all", "one"].includes(request.mode ?? "")) throw new Error("Invalid repeat mode");
        repeatMode = request.mode as "off" | "all" | "one"; playbackQueue.setRepeat(repeatMode); refreshQueue();
      } else if (request.action === "openUri") {
        const uri = new URL(request.mode ?? "");
        const library = await call<Track[]>("list_tracks", { view: "library", collectionId: null });
        let track = library.find(item => item.sourceUrl === uri.href || (uri.protocol === "file:" && item.localPath === decodeURIComponent(uri.pathname)));
        if (!track) {
          const youtube = ["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"].includes(uri.hostname);
          const soundcloud = ["soundcloud.com", "www.soundcloud.com"].includes(uri.hostname);
          if (!["http:", "https:"].includes(uri.protocol) || (!youtube && !soundcloud) || uri.username || uri.password) throw new Error("OpenUri supports saved library items and YouTube/SoundCloud audio links");
          const results = await call<Track[]>("search_audio_tracks", { query: uri.href, providerIds: [youtube ? "youtube" : "soundcloud"] });
          track = results.find(item => item.sourceUrl === uri.href) ?? (results.length === 1 ? results[0] : undefined);
          if (!track) throw new Error("No exact audio source found for this URI");
        }
        if (activeAutomationPlayback !== request.requestId) throw new Error("Playback request was cancelled or superseded");
        playbackQueue.setTracks([track], 0); refreshQueue();
        await startPreview(track, true, false, undefined, undefined, 0, request.requestId);
      } else if (request.action === "next" || request.action === "previous") {
        if (paused) {
          const keepPaused = Boolean(preview) || systemPausedTrack;
          // Manual Next exits repeat-one before moving to another queue item. Keep
          // paused automation navigation consistent without starting a preview.
          if (request.action === "next" && repeatMode === "one") {
            repeatMode = "off";
            playbackQueue.setRepeat(repeatMode);
          }
          const next = request.action === "next" ? playbackQueue.next() : playbackQueue.previous();
          if (next) {
            await closePreview(); refreshQueue(); playingTrack = next; selected = next;
            playingCandidate = null; position = 0; duration = next.durationSeconds ?? 0;
            systemPausedTrack = keepPaused; previewStatus = keepPaused ? "Paused" : "Ready";
            persistPlaybackPosition();
          }
        } else await advanceQueue(request.action === "next" ? 1 : -1, false, 0, request.requestId);
      }
      else throw new Error("Unknown playback action");
    } catch (error) { failure = String(error); }
    await call("automation_playback_complete", { requestId: request.requestId, playback: automationPlaybackSnapshot(), error: failure }).catch(() => {});
  }
  async function saveAutomationPermissions() {
    await run(async () => { const result = await call<{permissions: {control: boolean; manage: boolean}}>("set_automation_permissions", { ...automationPermissions }); automationPermissions = result.permissions; });
  }
  function openPlayerPanel(panel: "info" | "queue" | "downloads" | null, track?: Track) {
    infoTrack = panel === "info" ? track ?? null : null;
    showPlayingInfo = panel === "info"; showQueue = panel === "queue"; showDownloads = panel === "downloads";
  }
  function keyboard(e: KeyboardEvent) {
    if (e.key === "Escape" && deletedCollections) { deletedCollections = null; return; }
    if (e.key === "Escape" && (showDownloads || showQueue || showPlayingInfo)) { openPlayerPanel(null); return; }
    if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { e.preventDefault(); void travelRoute(e.key === "ArrowLeft" ? "back" : "forward"); return; }
    if (downloadTrack || playlistTrack) return;
    if (
      (e.target as HTMLElement)?.closest(
        "input,textarea,select,button,[contenteditable]",
      ) ||
      e.ctrlKey ||
      e.metaKey ||
      e.altKey
    )
      return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      adjacent(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.code === "Space") {
      e.preventDefault();
      togglePlayback();
    } else if (selected && e.key.toLowerCase() === "s")
      void disposition(selected, "shortlist");
    else if (selected && e.key.toLowerCase() === "r")
      void disposition(selected, "reject");
  }
  function activateRenameDialog(node:HTMLDialogElement){
    const close=(event:Event)=>{event.preventDefault();renameTarget=null;};node.addEventListener("cancel",close);
    if(typeof node.showModal==="function")node.showModal();else node.setAttribute("open","");
    return {destroy(){node.removeEventListener("cancel",close);if(node.open&&typeof node.close==="function")node.close();}};
  }
  function activateDownloadDialog(node: HTMLDialogElement) {
    const restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const closeChooser = () => {
      downloadRequest++;
      downloadTrack = null;
      downloadCandidates = [];
      downloadCandidate = null;
    };
    const onCancel = (event: Event) => {
      event.preventDefault();
      closeChooser();
    };
    node.addEventListener("cancel", onCancel);
    if (typeof node.showModal === "function") node.showModal();
    else node.setAttribute("open", "");
    return {
      destroy() {
        node.removeEventListener("cancel", onCancel);
        if (node.open && typeof node.close === "function") node.close();
        if (restoreFocus?.isConnected) restoreFocus.focus();
      },
    };
  }
  async function searchSoulseek() {
    if (!query.trim()) return;
    const staleSearchId = soulseekSearchId;
    soulseekSearchId = "";
    if (staleSearchId) void call("cancel_slskd_search", { searchId: staleSearchId }).catch(() => {});
    const request = ++searchRequest;
    selectionRequest++;
    clearTimeout(autoTimer);
    await run(async () => {
      soulseekStatus = "Starting peer search…";
      const started = await call<{ id: string }>("start_slskd_search", { query: query.trim() });
      if (request !== searchRequest) {
        await call("cancel_slskd_search", { searchId: started.id }).catch(() => {});
        return;
      }
      soulseekSearchId = started.id;
      while (request === searchRequest) {
        const result = await call<{ id: string; state: string; tracks: Track[]; complete: boolean; warnings: string[] }>(
          "get_slskd_search", { searchId: started.id },
        );
        if (request !== searchRequest) return;
        tracks = result.tracks ?? [];
        selected = null;
        checked = [];
        if (result.complete) {
          soulseekStatus = `${tracks.length} results${result.warnings?.length ? ` · ${result.warnings.join(" · ")}` : ""}`;
          soulseekSearchId = "";
          warmPreviewTracks(tracks.slice(0, 3));
          return;
        }
        soulseekStatus = result.state || "Searching peers…";
        await new Promise((resolve) => setTimeout(resolve, 900));
      }
    });
  }
  async function cancelSoulseekSearch() {
    const id = soulseekSearchId;
    if (!id) return;
    searchRequest++;
    soulseekSearchId = "";
    soulseekStatus = "Search cancelled.";
    await call("cancel_slskd_search", { searchId: id }).catch((failure) => (error = String(failure)));
  }
  async function discoverArtist(track: Track) {
    if (!track.artist.trim()) return;
    const provenance=track.provenance as {catalog?:{artists?:Array<{provider:string;id:string;name?:string}>}} | undefined;
    const credited=provenance?.catalog?.artists?.filter(artist=>artist.name?.toLowerCase()===track.artist.trim().toLowerCase());
    if(credited?.length===1 && ["musicbrainz","discogs"].includes(credited[0].provider))return discoverArtistRef({provider:credited[0].provider as ArtistRef["provider"],id:credited[0].id});
    rememberRoute();
    artistSection = "overview"; artistScroll = 0;
    activeArtistTrack = track;
    activeArtistName = track.artist;
    activeArtistRef = null;
    await navigate("artist", false);
  }
  async function discoverArtistRef(artist: ArtistRef) {
    rememberRoute();
    activeArtistTrack = null;
    activeArtistName = "";
    artistSection = "overview"; artistScroll = 0;
    activeArtistRef = artist;
    await navigate("artist", false);
  }
  function playArtistTrack(track: Track, displayed: Track[] = [track], index = 0) {
    selected = track;
    playbackQueue.setTracks(displayed ?? [track], index ?? 0);
    playbackQueue.setRepeat(repeatMode);
    refreshQueue();
    return startPreview(track, true);
  }
  function queueArtistTracks(items: Track[], mode: "append" | "next") {
    if (!items.length) return;
    if (!queueTracks.length) playbackQueue.setTracks(items, 0);
    else if (mode === "next") for (const track of [...items].reverse()) playbackQueue.playNext(track);
    else playbackQueue.restore({ ...playbackQueue.snapshot(), tracks: [...queueTracks, ...items] });
    refreshQueue();
    notice = `${items.length === 1 ? items[0].title : `${items.length} tracks`} added ${mode === "next" ? "next" : "to queue"}.`;
  }
  async function addArtistTrack(track: Track) {
    openPlayerPanel(null);
    playlistTrack = track;
    if (!collections.length) collections = await call<Collection[]>("list_collections");
    if (!collectionId && collections.length) collectionId = collections[0].id;
  }
  async function addArtistTrackToPlaylist() {
    const track = playlistTrack;
    if (!track || !collectionId) return;
    await run(async () => {
      await call("add_to_collection", { collectionId, tracks: [track] });
      collections = await call<Collection[]>("list_collections");
      playlistTrack = null;
      notice = "Track added to playlist.";
    });
  }
  async function createPlaylistForArtistTrack() {
    const track = playlistTrack;
    if (!track || !newCollection.trim()) return;
    await run(async () => {
      const created = await call<Collection>("create_collection", { name: newCollection.trim() });
      await call("add_to_collection", { collectionId: created.id, tracks: [track] });
      collections = await call<Collection[]>("list_collections");
      collectionId = created.id;
      newCollection = "";
      playlistTrack = null;
      notice = `Created ${created.name} and added ${track.title}.`;
    });
  }
  async function searchArtists(query: string, page: number) {
    return call<ArtistSearchPage>("search_artists", { query, page, pageSize: 20 });
  }
  async function getArtistProfile(artist: ArtistRef) {
    const profile = await call<ArtistProfile>("get_artist_profile", { artist });
    return { ...profile, topTracks: artistPlaybackTracks(profile.topTracks, assets) };
  }
  async function getArtistReleases(artist: ArtistRef, cursor?: string | null) {
    return call<ArtistReleasePage>("get_artist_releases", { artist, cursor });
  }
  async function getReleaseTracks(release: ArtistReleaseRef, artist: ArtistRef) {
    return artistPlaybackTracks(await call<Track[]>("get_release_tracks", { release, artist }), assets);
  }
  async function getArtistTracks(artist: ArtistRef, cursor?: string | null) {
    const page = await call<ArtistTrackPage>("get_artist_tracks", { artist, cursor });
    return { ...page, items: artistPlaybackTracks(page.items, assets) };
  }
  async function getArtistCollaborations(artist: ArtistRef, cursor?: string | null) {
    return call<ArtistCollaborationPage>("get_artist_collaborations", { artist, cursor });
  }
  async function resolveRecordingMetadata(track: Track, release?: ArtistReleaseRef | null, recording?: ArtistRef | null) {
    return call<MetadataReviewData>("resolve_recording_metadata", { track, releaseRef: release, recordingRef: recording });
  }
  async function applyDownloadMetadata(track: Track, review: MetadataReviewData, acceptedFieldKeys: MetadataFieldKey[]) {
    const accepted = new Set(acceptedFieldKeys);
    const changes = Object.fromEntries(review.fields.filter((field) => accepted.has(field.key)).map((field) => [field.key, field.value]));
    if (!Object.keys(changes).length) return;
    const updated = await call<Track>("update_track", { track: { ...track, ...changes } });
    tracks = tracks.map((item) => sameTrack(item, track) ? { ...item, ...updated } : item);
    if (selected && sameTrack(selected, track)) selected = { ...selected, ...updated };
    if (playingTrack && sameTrack(playingTrack, track)) playingTrack = { ...playingTrack, ...updated };
    downloadTrack = { ...track, ...updated };
    downloadMetadata = { ...downloadMetadata,
      artist: updated.artist, title: updated.title, version: updated.version ?? "",
      album: updated.album ?? "", label: updated.label ?? "", genre: updated.genre ?? "",
      bpm: updated.bpm == null ? "" : String(updated.bpm), key: updated.key ?? "",
      releaseDate: updated.releaseDate ?? "", isrc: updated.isrc ?? "", catalogNumber: updated.catalogNumber ?? "",
    };
    notice = assetIdForTrack(track) || track.localAvailability === "available"
      ? "Metadata saved; local tags are updating."
      : "Accepted metadata fields were saved.";
  }
  function sameTrack(a: Track, b: Track): boolean {
    return a.id === b.id || (a.provider === b.provider && a.providerId === b.providerId) || (Boolean(a.sourceUrl && b.sourceUrl) && a.sourceUrl === b.sourceUrl);
  }
  function assetIdForTrack(track: Track): string | undefined {
    const explicit = (track as Track & { assetId?: string | null }).assetId;
    return explicit ?? assets.find((asset) => asset.recordingId === track.id || sameTrack(asset.track, track))?.id;
  }
  function analysisStatusForTrack(track: Track) {
    const assetId = assetIdForTrack(track);
    return assetId ? analysisStatuses.find((status) => status.assetId === assetId) : undefined;
  }
  function analysisAnnotation(track: Track, field: "bpm" | "key"): string {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const analysis = provenance[field === "bpm" ? "bpmAnalysis" : "keyAnalysis"];
    if (!analysis || typeof analysis !== "object") return "";
    const record = analysis as Record<string, unknown>;
    const value = field === "bpm"
      ? record.bpm ?? record.estimatedBpm
      : record.key ?? record.estimatedKey ?? record.wholeTrackKey;
    const matches = field === "bpm" ? typeof value === "number" && track.bpm != null && Math.abs(value - track.bpm) <= 0.1
      : typeof value === "string" && track.key != null && value.trim().toLowerCase() === track.key.trim().toLowerCase();
    const status = String(record.status ?? "").replace(/[_-]+/g, " ");
    if (!matches || /failed|error|unavailable|no estimate|no confident estimate/i.test(status)) return "";
    return `Estimated from audio; not verified catalog metadata${record.method ? ` · ${record.method}` : ""}${record.confidence != null ? ` · confidence ${record.confidence}` : ""}.`;
  }
  async function analyzeLibraryAssets() {
    if (analysisBusy) return;
    const targeted = checked.length ? assets.filter((asset) => checked.includes(asset.recordingId) || checked.includes(asset.track.id))
      : selected ? assets.filter((asset) => sameTrack(asset.track, selected!)) : assets.filter((asset) => asset.track.bpm == null || !asset.track.key);
    const targetIds = targeted.map((asset) => asset.id);
    if (!targetIds.length) { notice = "No local recordings need BPM or key analysis."; return; }
    analysisBusy = true;
    try {
      const result = await call<{ queued: number }>("analyze_local_assets", { assetIds: targetIds, force: false });
      notice = `Queued analysis for ${result.queued} local recording${result.queued === 1 ? "" : "s"}.`;
      analysisStatuses = await call<typeof analysisStatuses>("get_analysis_status");
    } catch (failure) { error = String(failure); }
    finally { analysisBusy = false; }
  }
  function mergeLibraryTrack(updated: Track) {
    const merge = (track: Track) => sameTrack(track, updated) ? { ...track, ...updated } : track;
    tracks = tracks.map(merge);
    if (selected && sameTrack(selected, updated)) selected = merge(selected);
    if (playingTrack && sameTrack(playingTrack, updated)) playingTrack = merge(playingTrack);
    if (chart) chart = { ...chart, tracks: chart.tracks.map(merge) };
    for (const [key, context] of contexts) {
      contexts.set(key, { ...context, tracks: context.tracks.map(merge), selected: context.selected && sameTrack(context.selected, updated) ? merge(context.selected) : context.selected });
    }
    assets = assets.map((asset) => asset.recordingId === updated.id || sameTrack(asset.track, updated) ? { ...asset, track: merge(asset.track) } : asset);
  }
  async function handleLibraryUpdated(updated: Track) {
    mergeLibraryTrack(updated);
    if (!["library", "favorites", "collections"].includes(view)) return;
    const currentView = view, currentCollectionId = collectionId, currentContext = activeContext;
    try {
      const refreshed = await call<Track[]>("list_tracks", { view: currentView, collectionId: currentView === "collections" ? currentCollectionId : null });
      if (view !== currentView || activeContext !== currentContext) return;
      tracks = refreshed.map((track) => sameTrack(track, updated) ? { ...track, ...updated } : track);
      if (selected) selected = tracks.find((track) => sameTrack(track, selected!)) ?? selected;
    } catch (failure) { notice = `Saved metadata updated; the current list could not refresh. ${String(failure)}`; }
  }
  async function applyArtistMetadata(track: Track, review: MetadataReviewData, acceptedFieldKeys: MetadataFieldKey[]) {
    const accepted = new Set(acceptedFieldKeys);
    const changes = Object.fromEntries(review.fields.filter((field) => accepted.has(field.key)).map((field) => [field.key, field.value]));
    if (!Object.keys(changes).length) return;
    const updated = await call<Track>("update_track", { track: { ...track, ...changes } });
    tracks = tracks.map((item) => sameTrack(item, track) ? { ...item, ...updated } : item);
    if (selected && sameTrack(selected, track)) selected = { ...selected, ...updated };
    if (activeArtistTrack && sameTrack(activeArtistTrack, track)) activeArtistTrack = { ...activeArtistTrack, ...updated };
    notice = assetIdForTrack(track) || track.localAvailability === "available"
      ? "Metadata saved; local tags are updating."
      : "Accepted metadata fields were saved.";
  }
  async function search() {
    const staleSearchId = soulseekSearchId;
    soulseekSearchId = "";
    if (staleSearchId) void call("cancel_slskd_search", { searchId: staleSearchId }).catch(() => {});
    selectionRequest++;
    clearTimeout(autoTimer);
    const request = ++searchRequest;
    await run(async () => {
      const result = await call<Track[]>("search_tracks", {
        query,
        providerIds: ["beatport", "traxsource"],
      });
      if (request === searchRequest) {
        tracks = result;
        audioSearchTrackIds = new Set();
        warmPreviewTracks(result.slice(0, 3));
        selected = null;
        candidates = [];
      }
    });
  }
  async function searchAudio() {
    const staleSearchId = soulseekSearchId;
    soulseekSearchId = "";
    if (staleSearchId) void call("cancel_slskd_search", { searchId: staleSearchId }).catch(() => {});
    const providers = searchMode === "audio" ? ["youtube", "soundcloud"] : [searchMode];
    if (!query.trim()) return;
    selectionRequest++;
    clearTimeout(autoTimer);
    const request = ++searchRequest;
    await run(async () => {
      const result = await call<Track[]>("search_audio_tracks", {
        query: query.trim(), providerIds: providers,
      });
      if (request === searchRequest) {
        tracks = result;
        audioSearchTrackIds = new Set(result.map((track) => track.id));
        warmPreviewTracks(result.slice(0, 3));
        selected = null;
        candidates = [];
      }
    });
  }
  function findAudio() {
    if (searchMode === "soulseek") void searchSoulseek();
    else void searchAudio();
  }
  function changeAudioSearchSource() {
    searchRequest++;
    selectionRequest++;
    clearTimeout(autoTimer);
    const staleSearchId = soulseekSearchId;
    soulseekSearchId = "";
    soulseekStatus = "";
    if (staleSearchId) void call("cancel_slskd_search", { searchId: staleSearchId }).catch(() => {});
    tracks = [];
    selected = null;
    checked = [];
    candidates = [];
    audioSearchTrackIds = new Set();
    contexts.set(activeContext, { tracks: [], selected: null, filter, sort, descending, scroll: 0 });
  }
  async function findSources() {
    if (!selected) return;
    const t = selected,
      request = selectionRequest;
    await run(async () => {
      const result = await call<Candidate[]>("find_candidates", {
        track: t,
        providerIds: ["youtube", "soundcloud"],
      });
      if (request === selectionRequest) candidates = result;
    });
  }
  async function mergeSelected() {
    if (checked.length < 2 || !mergeConfirmed) return;
    await run(async () => {
      await call("merge_recordings", {
        primaryId: checked[0],
        otherIds: checked.slice(1),
      });
      checked = [];
      mergeConfirmed = false;
      notice =
        "Verified recordings merged. Provider chart rankings are retained.";
      if (view === "charts") await loadChart();
      else await navigate(view);
    });
  }
  async function addSource() {
    if (!selected || !sourceUrl.trim()) return;
    const t = selected,
      request = selectionRequest;
    const c: Candidate = {
      id: sourceUrl,
      provider: sourceProvider,
      url: sourceUrl.trim(),
      title: t.title,
      uploader: null,
      durationSeconds: null,
      artworkUrl: null,
      formats: [],
      permission: "unknown",
      confidence: "review",
      score: 0,
      reasons: [
        "User supplied URL; verify recording identity and download permission.",
      ],
      official: false,
    };
    await run(async () => {
      const result = await call<Candidate>("inspect_candidate", {
        track: t,
        candidate: { ...c, id: sourceProvider === "direct" ? sourceUrl : "" },
      });
      if (request === selectionRequest) {
        candidates = [
          result,
          ...candidates.filter((x) => x.url !== result.url),
        ];
        sourceUrl = "";
      }
    });
  }
  async function inspect(c: Candidate) {
    if (!selected) return;
    const t = selected,
      request = selectionRequest;
    await run(async () => {
      const result = await call<Candidate>("inspect_candidate", {
        track: t,
        candidate: c,
      });
      if (request === selectionRequest)
        candidates = candidates.map((x) => (x.id === c.id ? result : x));
    });
  }
  async function download(c: Candidate) {
    if (!selected) return;
    await openDownloadChooser(selected, c);
  }
  async function openDownloadChooser(track: Track, initialCandidate?: Candidate, initialChapterId?: string) {
    if(track.playbackRange) {
      const provenance=track.provenance as {parentTrack?:Track};const candidate=importedAudioCandidate(track);
      const chapter=candidate?.chapters?.find(ch=>ch.id===track.playbackRange!.chapterId);
      if(provenance.parentTrack && candidate && chapter)return downloadChapter(provenance.parentTrack,candidate,chapter);
    }
    const request = ++downloadRequest;
    const typedDirectory = settings.musicDirectory;
    const cached = initialCandidate ?? linkedSearchCandidate(track);
    openPlayerPanel(null);
    downloadTrack = track;
    focusedDownloadChapterId = initialChapterId ?? null;
    splitChapters = Boolean(initialChapterId); selectedChapterIds = initialChapterId ? [initialChapterId] : [];
    downloadCandidates = cached ? [cached] : [];
    downloadCandidate = cached ?? null;
    downloadMetadata = {
      artist: track.artist, title: track.title, version: track.version ?? "", album: track.album ?? "",
      label: track.label ?? "", genre: track.genre ?? "", bpm: track.bpm == null ? "" : String(track.bpm),
      key: track.key ?? "", releaseDate: track.releaseDate ?? "", isrc: track.isrc ?? "", catalogNumber: track.catalogNumber ?? "",
    };
    downloadLoading = true;
    await tick();
    await run(async () => {
      try {
        const freshSettings = await call<Settings>("get_settings");
        if (request !== downloadRequest || downloadTrack?.id !== track.id) return;
        persistedMusicDirectory = freshSettings.musicDirectory;
        settings = { ...freshSettings, musicDirectory: typedDirectory || freshSettings.musicDirectory };
        const searchSource = view === "search" && searchMode !== "audio" ? searchMode === "soulseek" ? "slskd" : searchMode : undefined;
        const preferred = searchSource ?? preferredSource(freshSettings);
        const providers = slskdEnabled ? ["monochrome", "youtube", "soundcloud", "slskd"] : ["monochrome", "youtube", "soundcloud"];
        const inspect = async (candidate: Candidate) => {
          if (candidate.provider === "slskd" || (candidate.formats?.length && (candidate.provider !== "youtube" || candidate.chapters))) return;
          try {
            const inspected = await call<Candidate>("inspect_candidate", { track, candidate });
            if (request !== downloadRequest || downloadTrack?.id !== track.id) return;
            downloadCandidates = downloadCandidates.map(item => candidateKey(item) === candidateKey(candidate) ? inspected : item);
            if (downloadCandidate && candidateKey(downloadCandidate) === candidateKey(candidate)) {
              downloadCandidate = inspected;
              if (focusedDownloadChapterId) {
                splitChapters = true;
                selectedChapterIds = inspected.chapters?.some(chapter => chapter.id === focusedDownloadChapterId) ? [focusedDownloadChapterId] : [];
              } else selectedChapterIds = inspected.chapters?.map(chapter => chapter.id) ?? [];
            }
          } catch { /* Best available remains usable; the job validates the source. */ }
        };
        await Promise.allSettled([
          ...(cached ? [inspect(cached)] : []),
          ...providers.map(async provider => {
            const found = await call<Candidate[]>("find_candidates", { track, providerIds: [provider] });
            if (request !== downloadRequest || downloadTrack?.id !== track.id) return;
            const existing = new Set(downloadCandidates.map(candidateKey));
            downloadCandidates = [...downloadCandidates, ...found.filter(item => !existing.has(candidateKey(item)))];
            if (!downloadCandidate && provider === preferred) {
              const recommended = provider === "slskd" ? found.filter(item => item.source?.kind === "soulseek").sort((a,b) => b.score-a.score)[0] : choosePreviewCandidate(track, found, provider);
              if (recommended) { downloadCandidate = recommended; await inspect(recommended); }
            }
          }),
        ]);
      } finally { if (request === downloadRequest) downloadLoading = false; }
    });
  }
  async function startSongRadio(track: Track) {
    const request = ++radioRequest;
    radioBusy = track.id;
    await run(async () => {
      try {
        let seed = playingTrack?.id === track.id && playingCandidate?.provider === "youtube" ? playingCandidate : linkedSearchCandidate(track);
        if (seed?.provider !== "youtube") {
          const found = await call<Candidate[]>("find_candidates", { track, providerIds: ["youtube"] });
          seed = choosePreviewCandidate(track, found, "youtube");
        }
        if (!seed || seed.provider !== "youtube") throw new Error("Find a playable YouTube source for this song before starting radio.");
        const result = await call<{ tracks: Track[]; total: number }>("get_song_radio", { track, candidate: seed, limit: 25 });
        if (request !== radioRequest) return;
        if (!result.tracks.length) throw new Error("YouTube returned no radio recommendations for this song.");
        const keepPlaying = playingTrack?.id === track.id && preview && !paused;
        const seedTrack = keepPlaying ? track : { ...track, provenance: { ...(track.provenance && typeof track.provenance === "object" ? track.provenance : {}), sourceKind: "youtubeRadio", audioCandidate: seed } };
        const queue = [seedTrack, ...result.tracks];
        playbackQueue.setTracks(queue, 0); refreshQueue();
        if (!keepPlaying) await startPreview(seedTrack, true);
        if (request !== radioRequest) return;
        openPlayerPanel("queue");
        notice = `Song radio · ${result.total} YouTube recommendations.`;
      } finally { if (request === radioRequest) radioBusy = null; }
    });
  }
  async function saveFallbackSettings(kind: "streaming" | "download", enabled: boolean, sources: string[]) {
    const changes = kind === "streaming" ? { streamingFallbackEnabled:enabled, streamingFallbackSources:sources } : { downloadFallbackEnabled:enabled, downloadFallbackSources:sources };
    settings = { ...settings, ...changes };
    await run(async () => { const fresh = await call<Settings>("get_settings"); await call("save_settings", { settings:{ ...fresh, ...changes } }); });
  }
  function preferredSource(settingsValue: Settings): "youtube" | "soundcloud" | "slskd" | "monochrome" {
    const source = settingsValue.preferredDownloadSource;
    return source === "soundcloud" || source === "slskd" || source === "monochrome" ? source : "youtube";
  }
  async function downloadTopMatch(track: Track) {
    if (rowDownloadBusy[track.id]) return;
    rowDownloadBusy = { ...rowDownloadBusy, [track.id]: true };
    rowDownloadFeedback = { ...rowDownloadFeedback, [track.id]: "Finding a source…" };
    try {
      const latest = await call<Settings>("get_settings");
      const linked = importedAudioCandidate(track);
      const provenance = track.provenance && typeof track.provenance === "object"
        ? track.provenance as Record<string, unknown> : {};
      const rawLinked = provenance.audioCandidate && typeof provenance.audioCandidate === "object"
        ? provenance.audioCandidate as Candidate : undefined;
      const linkedSoulseek = rawLinked?.provider === "slskd" && rawLinked.source?.kind === "soulseek" &&
        Boolean(rawLinked.source.username?.trim() && rawLinked.source.filename?.trim());
      const linkedCandidate = linkedSoulseek ? rawLinked
        : linked && isLinkedAudioSearchCandidate(track, linked) ? linked : undefined;
      const searchSource = view === "search" && searchMode !== "audio"
        ? searchMode === "soulseek" ? "slskd" : searchMode : undefined;
      let source = linkedCandidate?.provider === "slskd" ? "slskd"
        : ["youtube", "soundcloud", "monochrome"].includes(linkedCandidate?.provider ?? "") ? linkedCandidate!.provider as "youtube" | "soundcloud" | "monochrome"
          : searchSource ?? preferredSource(latest);
      let sourceLabel = source === "slskd" ? "Soulseek" : source === "youtube" ? "YouTube" : source === "monochrome" ? "Monochrome" : "SoundCloud";
      const typedDirectory = settings.musicDirectory;
      settings = { ...latest, ...settings, musicDirectory: typedDirectory || latest.musicDirectory, preferredDownloadSource: preferredSource(latest) };
      let candidate: Candidate | undefined;
      const order = downloadProviderOrder(source, latest.downloadFallbackEnabled === true, latest.downloadFallbackSources ?? ["monochrome","slskd","youtube","soundcloud"]);
      for (const providerId of order) {
        try {
          const linkedChoice = linkedCandidate?.provider === providerId ? linkedCandidate : undefined;
          const found = linkedChoice ? [linkedChoice] : await call<Candidate[]>("find_candidates", { track, providerIds:[providerId] });
          let selected = linkedChoice ?? (providerId === "slskd"
            ? found.filter(item => item.provider === providerId && item.source?.kind === "soulseek" && item.source.username?.trim() && item.source.filename?.trim() && item.score >= 0.55 && item.confidence !== "low").sort((a,b)=>b.score-a.score)[0]
            : choosePreviewCandidate(track, found, providerId as StreamingProvider));
          if (!selected || (providerId !== source && selected.confidence !== "high")) continue;
          if (selected.provider !== "slskd" && !selected.formats?.length) selected = await call<Candidate>("inspect_candidate", { track, candidate:selected });
          if (selected.permission === "restricted") continue;
          candidate = selected;
          source = providerId as typeof source;
          sourceLabel = source === "slskd" ? "Soulseek" : source === "youtube" ? "YouTube" : source === "monochrome" ? "Monochrome" : "SoundCloud";
          break;
        } catch { /* Try only the configured fallback providers. */ }
      }
      if (!candidate) {
        rowDownloadFeedback = { ...rowDownloadFeedback, [track.id]: "No usable recording match · More to review" };
        notice = `No usable match in the selected download sources for ${track.title}. Use More to review alternatives.`;
        return;
      }
      const selectedDirectory = settings.musicDirectory.trim();
      const targetDirectory = selectedDirectory || latest.musicDirectory.trim();
      if (!targetDirectory) {
        rowDownloadFeedback = { ...rowDownloadFeedback, [track.id]: "Choose music folder in Settings" };
        error = "Choose and save a music folder in Settings before downloading.";
        return;
      }
      if (selectedDirectory && selectedDirectory !== latest.musicDirectory) {
        latest.musicDirectory = selectedDirectory;
        await call("save_settings", { settings: latest });
      } else latest.musicDirectory = targetDirectory;
      settings = { ...latest, preferredDownloadSource: preferredSource(latest) };
      persistedMusicDirectory = targetDirectory;
      const job = await call<DownloadJob>("enqueue_download", {
        request: { track, candidate, formatId: null, outputProfile },
      });
      jobs = [job, ...jobs.filter((item) => item.id !== job.id)];
      const qualityLabel = candidate.provider === "slskd"
        ? candidate.source?.bitrate_kbps ? `${Math.round(candidate.source.bitrate_kbps)} kbps peer file` : "peer file"
        : "best available quality";
      rowDownloadFeedback = { ...rowDownloadFeedback, [track.id]: `Queued · ${sourceLabel} · ${qualityLabel}` };
      notice = `Queued ${track.title} from ${sourceLabel}. The source match is unverified; tags use saved recording metadata.`;
    } catch (failure) {
      const message = String(failure);
      rowDownloadFeedback = { ...rowDownloadFeedback, [track.id]: `Could not queue · More to review` };
      error = message;
    } finally {
      rowDownloadBusy = { ...rowDownloadBusy, [track.id]: false };
    }
  }
  async function inspectDownloadCandidate(candidate: Candidate) {
    if (!downloadTrack) return;
    const track = downloadTrack, request = downloadRequest;
    downloadCandidate = candidate;
    if (focusedDownloadChapterId && !candidate.chapters?.some((chapter) => chapter.id === focusedDownloadChapterId)) focusedDownloadChapterId = null;
    splitChapters = Boolean(focusedDownloadChapterId); selectedChapterIds = focusedDownloadChapterId ? [focusedDownloadChapterId] : candidate.chapters?.map((chapter) => chapter.id) ?? [];
    if (candidate.provider === "slskd" || (candidate.formats?.length && (candidate.provider !== "youtube" || candidate.chapters))) return;
    await run(async () => {
      const inspected = await call<Candidate>("inspect_candidate", { track, candidate });
      if (request === downloadRequest && downloadTrack?.id === track.id) {
        downloadCandidates = downloadCandidates.map((item) => candidateKey(item) === candidateKey(candidate) ? inspected : item);
        downloadCandidate = inspected;
        if (focusedDownloadChapterId) {
          splitChapters = true;
          selectedChapterIds = inspected.chapters?.some((chapter) => chapter.id === focusedDownloadChapterId) ? [focusedDownloadChapterId] : [];
        } else selectedChapterIds = inspected.chapters?.map((chapter) => chapter.id) ?? [];
      }
    });
  }
  function focusDownloadChapter(chapterId: string) {
    focusedDownloadChapterId = chapterId;
    splitChapters = true;
    selectedChapterIds = [chapterId];
  }
  async function downloadSingleChapter(chapter: Chapter) {
    if (!downloadCandidate) return;
    focusDownloadChapter(chapter.id);
    await queueDownloadChoice();
  }
  async function queueDownloadChoice() {
    const track = downloadTrack, candidate = downloadCandidate;
    if (!track || !candidate || downloadSubmitting) return;
    if (splitChapters && !selectedChapterIds.length) { error = "Choose at least one chapter."; return; }
    const bpmText = String(downloadMetadata.bpm ?? "").trim();
    if (bpmText && (!Number.isFinite(Number(bpmText)) || Number(bpmText) <= 0 || Number(bpmText) > 1000)) {
      error = "Enter a BPM value from 0 to 1000, or leave the tag blank.";
      return;
    }
    downloadSubmitting = true;
    await run(async () => {
      const latestSettings = await call<Settings>("get_settings");
      const selectedDirectory = settings.musicDirectory.trim();
      const targetDirectory = selectedDirectory || latestSettings.musicDirectory.trim();
      if (!targetDirectory) {
        settings = latestSettings;
        persistedMusicDirectory = latestSettings.musicDirectory;
        throw new Error("Choose a music folder and save it before queuing downloads.");
      }
      if (selectedDirectory && selectedDirectory !== latestSettings.musicDirectory) {
        latestSettings.musicDirectory = selectedDirectory;
        await call("save_settings", { settings: latestSettings });
        persistedMusicDirectory = selectedDirectory;
      } else {
        latestSettings.musicDirectory = targetDirectory;
        persistedMusicDirectory = targetDirectory;
      }
      settings = latestSettings;
      const job = await call<DownloadJob>("enqueue_download", {
        request: {
          chapterIds: splitChapters ? selectedChapterIds : null,
          track: {
            ...track,
            artist: downloadMetadata.artist,
            title: downloadMetadata.title,
            version: downloadMetadata.version || null,
            album: downloadMetadata.album || null,
            label: downloadMetadata.label || null,
            genre: downloadMetadata.genre || null,
            bpm: bpmText ? Number(bpmText) : null,
            key: downloadMetadata.key || null,
            releaseDate: downloadMetadata.releaseDate || null,
            isrc: downloadMetadata.isrc || null,
            catalogNumber: downloadMetadata.catalogNumber || null,
          },
          candidate,
          formatId: candidateFormats[candidateKey(candidate)] || null,
          outputProfile,
        },
      });
      jobs = [job, ...jobs.filter((item) => item.id !== job.id)];
      notice = "Added to download queue.";
      downloadRequest++;
      downloadTrack = null;
      downloadCandidate = null;
      downloadCandidates = [];
      focusedDownloadChapterId = null;
    });
    downloadSubmitting = false;
  }
  async function queueSelected(items?: Track[]) {
    if (batchRunning || !(items?.length ?? checked.length)) return;
    const typedDirectory = settings.musicDirectory.trim();
    let latestSettings: Settings;
    try {
      latestSettings = await call<Settings>("get_settings");
    } catch (failure) {
      error = `Could not refresh the saved music folder: ${String(failure)}`;
      return;
    }
    const targetDirectory = typedDirectory || latestSettings.musicDirectory.trim();
    if (!targetDirectory) {
      settings = latestSettings;
      persistedMusicDirectory = latestSettings.musicDirectory;
      error = "Configure a music directory in Settings before batch acquisition.";
      return;
    }
    if (typedDirectory && typedDirectory !== latestSettings.musicDirectory) {
      latestSettings.musicDirectory = typedDirectory;
      try {
        await call("save_settings", { settings: latestSettings });
      } catch (failure) {
        error = `Could not save the music folder: ${String(failure)}`;
        return;
      }
    } else {
      latestSettings.musicDirectory = targetDirectory;
    }
    settings = latestSettings;
    persistedMusicDirectory = targetDirectory;
    const batch = items ?? tracks.filter((t) => checked.includes(t.id));
    const source = preferredSource(latestSettings);
    const profile = outputProfile;
    batchRunning = true;
    batchCancelled = false;
    let queued = 0,
      skipped = 0,
      failed = 0;
    const failures: string[] = [];
    try {
      for (let i = 0; i < batch.length && !batchCancelled; i++) {
        const track = batch[i];
        batchStatus = `Matching ${i + 1}/${batch.length}: ${track.title}`;
        try {
          const found = await call<Candidate[]>("find_candidates", {
            track,
            providerIds: [source],
          });
          const chosen = source === "slskd" ? found.filter(item => item.source?.kind === "soulseek" && item.score >= 0.9 && item.confidence !== "low").sort((a,b) => b.score-a.score)[0] : await verifiedBatchSource(
            found,
            (candidate) =>
              call<Candidate>("inspect_candidate", { track, candidate }),
            () => batchCancelled,
          );
          if (batchCancelled) break;
          if (!chosen) {
            skipped++;
            continue;
          }
          const job = await call<DownloadJob>("enqueue_download", {
            request: {
              track,
              candidate: chosen,
              formatId: null,
              outputProfile: profile,
            },
          });
          jobs = [job, ...jobs.filter((j) => j.id !== job.id)];
          queued++;
        } catch (e) {
          failed++;
          failures.push(`${track.artist} — ${track.title}: ${String(e)}`);
        }
      }
      notice = `Batch ${batchCancelled ? "stopped" : "complete"}: ${queued} queued; ${skipped} need manual review; ${failed} failed. Matches that need closer review were skipped.`;
      if (failures.length) error = failures.slice(0, 3).join(" · ");
    } finally {
      batchRunning = false;
      batchStatus = "";
    }
  }
  async function downloadPlaylist(id: string, useDisplayed = false) {
    const items = useDisplayed ? [...tracks] : await call<Track[]>("list_tracks", { view: "collections", collectionId: id });
    await queueSelected(items);
  }
  async function refreshJobs() {
    await run(async () => (jobs = await call<DownloadJob[]>("list_jobs")));
  }
  const namedCollections=$derived(collections.map(item=>({...item,name:sidebarLayout.names?.[item.id]??item.name})));
  async function persistSidebar(layout:typeof sidebarLayout) {
    await run(async()=>{sidebarLayout=await call<typeof sidebarLayout>("save_sidebar_layout",{layout});});
  }
  function createSection(name:string) {void persistSidebar({...sidebarLayout,sections:[...sidebarLayout.sections,{id:crypto.randomUUID(),name}]});}
  function renameSection(id:string,name:string) {void persistSidebar({...sidebarLayout,sections:sidebarLayout.sections.map(section=>section.id===id?{...section,name}:section)});}
  function deleteSection(id:string) {void persistSidebar({...sidebarLayout,sections:sidebarLayout.sections.filter(section=>section.id!==id),assignments:Object.fromEntries(Object.entries(sidebarLayout.assignments).filter(([,section])=>section!==id))});}
  function movePlaylistToSection(playlistId:string,sectionId:string|null) {
    const assignments={...sidebarLayout.assignments};if(sectionId)assignments[playlistId]=sectionId;else delete assignments[playlistId];void persistSidebar({...sidebarLayout,assignments});
  }
  async function createNewPlaylist(sectionId?:string) {
    let name="New playlist",n=2;while(collections.some(c=>c.name===name))name=`New playlist ${n++}`;
    await run(async()=>{const created=await call<Collection>("create_collection",{name});collections=[...collections,created];
      if(sectionId)sidebarLayout=await call<typeof sidebarLayout>("save_sidebar_layout",{layout:{...sidebarLayout,assignments:{...sidebarLayout.assignments,[created.id]:sectionId}}});
      showImport=false;importTargetCollectionId=null;await openPlaylist(created.id);renamePlaylistName=name;renamingPlaylist=true;});
  }
  async function renameCurrentPlaylist(){if(!currentCollection||!renamePlaylistName.trim())return;await run(async()=>{await call("rename_collection",{collectionId:currentCollection!.id,name:renamePlaylistName.trim()});if(sidebarLayout.names?.[currentCollection!.id])sidebarLayout=await call<typeof sidebarLayout>("save_sidebar_layout",{layout:{...sidebarLayout,names:{...sidebarLayout.names,[currentCollection!.id]:renamePlaylistName.trim()}}});collections=await call<Collection[]>("list_collections");renamingPlaylist=false;});}
  async function removeSidebarStation(station:RadioStation){
    await run(async()=>{
      radioStations=await call<RadioStation[]>("remove_radio_station",{stationId:station.id});
      if(selectedRadioStationId===station.id)selectedRadioStationId=radioStations[0]?.id??"";
      notice=`Removed station “${station.name}”.`;
    });
  }
  function startSidebarRename(kind:"playlist"|"radio",id:string,name:string){renameTarget={kind,id,name};sidebarRenameValue=name;}
  async function saveSidebarRename(){
    const target=renameTarget;const name=sidebarRenameValue.trim();if(!target||!name)return;
    await run(async()=>{
      if(target.kind==="radio")radioStations=await call<RadioStation[]>("rename_radio_station",{stationId:target.id,name});
      else {
        if(collections.some(item=>item.id===target.id))await call("rename_collection",{collectionId:target.id,name});
        sidebarLayout=await call<typeof sidebarLayout>("save_sidebar_layout",{layout:{...sidebarLayout,names:{...sidebarLayout.names,[target.id]:name}}});
        collections=await call<Collection[]>("list_collections");
      }
      renameTarget=null;
    });
  }
  function openPlaylistImport(){importTargetCollectionId=view==="collections"?collectionId:null;showImport=true;void refreshBrowser().catch(()=>{});}
  $effect(()=>{if(!showImport)return;const interval=setInterval(()=>{void call<BrowserStatus>("get_browser_status").then(status=>browserStatus=status).catch(()=>{});},5000);return()=>clearInterval(interval);});
  async function finishBrowserImport(imported:BrowserImportResult){
    await refreshBrowser();
    if(importTargetCollectionId){const target=importTargetCollectionId;const importedChart=await call<Chart>("load_browser_import",{id:imported.id});await call("add_to_collection",{collectionId:target,tracks:importedChart.tracks});collections=await call<Collection[]>("list_collections");showImport=false;importTargetCollectionId=null;await openPlaylist(target);}
    else await openBrowserImport(imported);
  }
  function selectTrackRow(track:Track,event:MouseEvent){
    if(event.shiftKey || event.ctrlKey || event.metaKey){
      const at=sorted.findIndex(t=>t.id===track.id);const anchor=sorted.findIndex(t=>t.id===(selectionAnchorId??selected?.id));
      if(event.shiftKey&&anchor>=0&&at>=0){checked=[...new Set([...checked,...sorted.slice(Math.min(at,anchor),Math.max(at,anchor)+1).map(t=>t.id)])];}
      else {checked=checked.includes(track.id)?checked.filter(id=>id!==track.id):[...checked,track.id];selectionAnchorId=track.id;}
      selected=track;mergeConfirmed=false;return;
    }
    selectionAnchorId=track.id;if(checked.length){checked=[track.id];selected=track;return;}select(track);
  }
  const playingRadioStation=$derived((playingTrack?.provider==="radioparadise" || playingTrack?.provider==="internet-radio" || (playingTrack?.provider==="radio" && (playingTrack?.provenance as {sourceKind?:string})?.sourceKind==="radio")) ? radioStations.find(station=>station.id===(playingTrack?.provenance as {stationId?:string})?.stationId)??null:null);
  function radioNowPlaying(station:RadioStation,item:RadioTrackMetadata|null){
    if(playingTrack?.id!==`radio:${station.id}`)return;
    playingTrack={...playingTrack,artist:item?.artist??(station.provider==="radio_paradise"?"Radio Paradise":"Internet radio"),title:item?.title??station.name,album:item?.album??undefined,artworkUrl:item?.artworkUrl??(item?undefined:station.faviconUrl??undefined),provenance:{...(playingTrack.provenance as Record<string,unknown>),radioMetadata:item}};
    if(playingTrack.artworkUrl)void cacheCover(playingTrack);void publishAutomationPlayback();
  }
  function playRadioStation(station:RadioStation){
    selectedRadioStationId=station.id;showAddRadio=false;
    const track:Track={id:`radio:${station.id}`,provider:"radio",providerId:station.id,title:station.name,artist:station.provider === "radio_paradise" ? "Radio Paradise" : "Internet radio",artworkUrl:station.faviconUrl??undefined,sourceUrl:station.homepageUrl??station.sourceUrl,favorite:false,provenance:{stationId:station.id,radioStreamUrl:station.streamUrl,sourceKind:"radio"}};
    void startPreview(track,false,false,[track],0);
  }
  $effect(()=>{const station=playingRadioStation;if(!station||(view==="radio"&&selectedRadioStationId===station.id)||paused)return;let stopped=false;let timer:ReturnType<typeof setTimeout>;
    async function poll(){try{const result=await call<{nowPlaying:RadioTrackMetadata|null}>("poll_radio_station",{stationId:station!.id,recognize:true});if(!stopped)radioNowPlaying(station!,result.nowPlaying);}catch{if(!stopped)radioNowPlaying(station!,null);}finally{if(!stopped)timer=setTimeout(()=>void poll(),30000);}}
    void poll();return()=>{stopped=true;clearTimeout(timer);};
  });
  async function createCollection() {
    if (!newCollection.trim()) return;
    await run(async () => {
      const c = await call<Collection>("create_collection", {
        name: newCollection.trim(),
      });
      collections = [...collections, c];
      collectionId = c.id;
      newCollection = "";
      if (view === "collections") {
        tracks = await call<Track[]>("list_tracks", { view: "collections", collectionId: c.id });
        selected = null;
        checked = [];
        activeContext = contextKey();
        contexts.set(activeContext, { tracks, selected: null, filter: "", sort: "rank", descending: false, scroll: 0 });
      }
    });
  }
  async function createAndAddPlaylist() {
    const toAdd = checked.length ? tracks.filter((track) => checked.includes(track.id)) : selected ? [selected] : [];
    if (!newCollection.trim() || !toAdd.length) return;
    await run(async () => {
      const created = await call<Collection>("create_collection", { name: newCollection.trim() });
      await call("add_to_collection", { collectionId: created.id, tracks: toAdd });
      collections = await call<Collection[]>("list_collections");
      collectionId = created.id;
      newCollection = "";
      checked = [];
      notice = `Created ${created.name} and added ${toAdd.length} recording${toAdd.length === 1 ? "" : "s"}.`;
      if (view === "collections") {
        tracks = await call<Track[]>("list_tracks", { view: "collections", collectionId: created.id });
        activeContext = contextKey();
        contexts.set(activeContext, { tracks, selected, filter, sort, descending, scroll: 0 });
      }
    });
  }
  async function renameCollection() {
    if (!collectionId || !renamedCollection.trim()) return;
    await run(async () => {
      const updated = await call<Collection>("rename_collection", {
        collectionId, name: renamedCollection.trim(),
      });
      collections = collections.map((collection) => collection.id === updated.id ? updated : collection);
      renamedCollection = "";
      notice = "Playlist renamed.";
    });
  }
  function selectedPlaylistIndex() {
    if (!selected) return -1;
    const exact = tracks.indexOf(selected);
    if (exact >= 0) return exact;
    const occurrence = (playlistPosition(selected) ?? 0) - 1;
    return tracks[occurrence]?.id === selected.id ? occurrence : tracks.findIndex((track) => track.id === selected?.id);
  }
  async function moveSelectedPlaylistEntry(direction: number) {
    if (view !== "collections" || !selected || !collectionId) return;
    const from = selectedPlaylistIndex(), to = from + direction;
    if (from < 0 || to < 0 || to >= tracks.length) return;
    await run(async () => {
      collectionId = await call<string>("move_playlist_entry", { collectionId, from, to });
      tracks = await call<Track[]>("list_tracks", { view: "collections", collectionId });
      selected = tracks[to] ?? null;
      collections = await call<Collection[]>("list_collections");
      sort = "rank"; descending = false;
    });
  }
  async function removePlaylistTrack(track: Track) {
    if (view !== "collections" || !collectionId) return;
    const targetId = collectionId;
    await run(async () => {
      const editedId = await call<string | null>("remove_from_collection", { collectionId: targetId, trackIds: [track.id] }) || targetId;
      if (view === "collections" && collectionId === targetId) { collectionId = editedId; tracks = await call<Track[]>("list_tracks", { view: "collections", collectionId: editedId }); }
      collections = await call<Collection[]>("list_collections");
      checked = checked.filter(id => id !== track.id);
      if (selected?.id === track.id) selected = null;
      notice = "Removed from playlist.";
    });
  }
  async function deletePlaylist(target = currentCollection) {
    if (!target) return;
    await run(async () => {
      await call("delete_collection", { collectionId: target.id });
      deletedPlaylist = target;
      collections = await call<Collection[]>("list_collections");
      browserImports = await call<typeof browserImports>("list_browser_imports");
      if ((view === "collections" || view === "charts") && collectionId === target.id) await navigate("library");
      notice = `Deleted playlist “${target.name}”. Audio files are kept.`;
    });
  }
  async function undoDeletePlaylist(target = deletedPlaylist) {
    if (!target) return;
    await run(async () => {
      await call("restore_collection", { collectionId: target.id });
      collections = await call<Collection[]>("list_collections");
      browserImports = await call<typeof browserImports>("list_browser_imports");
      deletedPlaylist = null;
      deletedCollections = null;
      await openPlaylist(target.id);
      notice = "Playlist restored.";
    });
  }
  async function removeSelectedFromPlaylist() {
    if (view !== "collections" || !collectionId || !checked.length) return;
    await run(async () => {
      collectionId = await call<string | null>("remove_from_collection", { collectionId, trackIds: checked }) || collectionId;
      tracks = await call<Track[]>("list_tracks", { view: "collections", collectionId });
      collections = await call<Collection[]>("list_collections");
      checked = [];
      selected = null;
      notice = "Removed selected recordings from this playlist.";
    });
  }
  async function addSelected() {
    if ((!selected && !checked.length) || !collectionId) return;
    await run(async () => {
      await call("add_to_collection", {
        collectionId,
        tracks: checked.length
          ? tracks.filter((t) => checked.includes(t.id))
          : [selected!],
      });
      collections = await call<Collection[]>("list_collections");
      notice = "Track added to playlist.";
    });
  }
  async function importTracks() {
    selectionRequest++;
    clearTimeout(autoTimer);
    await run(async () => {
      tracks = await call<Track[]>("import_tracks", {
        text: importText,
        kind: importKind,
      });
      warmPreviewTracks(tracks.slice(0, 3));
      selected = null;
      showImport = false;
      importText = "";
      notice = "Imported tracks are ready for review.";
    });
  }
  async function importM3uFile(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    selectionRequest++;
    clearTimeout(autoTimer);
    importKind = "m3u";
    await run(async () => {
      if (file.size > 2 * 1024 * 1024) throw new Error("Imports are limited to 2 MB");
      importText = await file.text();
      tracks = await call<Track[]>("import_tracks", { text: importText, kind: "m3u" });
      warmPreviewTracks(tracks.slice(0, 3));
      selected = null;
      showImport = false;
      importText = "";
      notice = "Playlist metadata imported for review.";
    });
    input.value = "";
  }
  async function previewCandidate(c: Candidate) {
    if (!selected) return;
    activeAutomationPlayback = null;
    clearTimeout(autoTimer);
    const request = previewRequests.next(), track = selected;
    playbackSession = crypto.randomUUID(); recordedSession = "";
    audio?.pause(); paused = true; position = 0; duration = 0;
    preview = null;
    playingTrack = track;
    automaticPreviewRecovery = { request, track, tried: new Set([candidateKey(c)]), attempts: 1, exactSourceOnly: true };
    await run(async () => {
      previewStatus = `Loading ${c.provider} audio…`;
      try {
        if (await resolveCandidateAudio(track, c, request)) return;
      } catch (failure) {
        if (automaticPreviewRecovery?.request === request) {
          automaticPreviewRecovery.failureReason = safePreviewError(failure);
        }
      }
      if (previewRequests.isCurrent(request)) {
        previewStatus = `The selected ${c.provider} source could not play: ${automaticPreviewRecovery?.failureReason ?? "No playable audio stream was returned."}`;
      }
    });
  }
  async function closePreview() {
    activeAutomationPlayback = null;
    systemPausedTrack = false;
    previewRequests.next(); audio?.pause();
    const candidate = preview?.sessionId;
    preview = null; paused = true;
    if (candidate) await run(() => call("stop_candidate_preview", { sessionId: candidate }));
  }
  async function refreshBrowser() {
    const [status, imports] = await Promise.all([
      call<BrowserStatus>("get_browser_status"), call<BrowserImportResult[]>("list_browser_imports"),
    ]);
    browserStatus = status; browserImports = imports;
  }
  async function openBrowserImport(result: BrowserImportResult, revealImportDetails = false) {
    rememberRoute();
    const request = ++navigationRequest;
    const loaded = await call<Chart>("load_browser_import", { id: result.id });
    if (request !== navigationRequest) return;
    if(!collections.some(item=>item.id===result.collectionId))collections=await call<Collection[]>("list_collections");
    if (request !== navigationRequest) return;
    contexts.set(activeContext, {
      tracks,
      selected,
      filter,
      sort,
      descending,
      scroll: table?.scrollTop ?? 0,
    });
    selectionRequest++; clearTimeout(autoTimer);
    view = "charts"; activeContext = "charts:"; chart = loaded; tracks = loaded.tracks; selected = null;
    warmPreviewTracks(loaded.tracks.slice(0, 3));
    checked = []; candidates = []; memberships = []; filter = ""; sort = "rank"; descending = false;
    contexts.set(activeContext, { tracks, selected: null, filter, sort, descending, scroll: 0 });
    if (result.kind === "chart" && ["beatport", "traxsource"].includes(result.provider.toLowerCase())) {
      provider = result.provider.toLowerCase();
      genreUrl = loaded.sourceUrl;
      void loadGenres(false);
    } else if (!genres.length) {
      void loadGenres();
    }
    activeBrowserImport = result; importDateFilter = result.id; collectionId = result.collectionId; showImport = revealImportDetails;
    notice = revealImportDetails ? `${result.title}: ${result.capturedCount} tracks imported${result.complete ? "." : " (partial capture)."}` : "";
  }
  async function copyCurrentImport() {
    if (!activeBrowserImport) return;
    const date = importDateFilter === "all" ? "all dates" : new Date(activeBrowserImport.fetchedAt).toLocaleDateString();
    const copy = await call<Collection>("create_collection", { name: `${activeBrowserImport.title} (${date})`.slice(0, 160) });
    await call("add_to_collection", { collectionId: copy.id, tracks });
    collections = await call<Collection[]>("list_collections"); await openPlaylist(copy.id);
  }
  async function exportMixxxPlaylist(id: string) {
    await run(async () => { const result = await call<{path: string; exported: number; missing: unknown[]}>("export_mixxx_playlist", { collectionId: id }); notice = `Exported ${result.exported} local tracks for Mixxx${result.missing.length ? ` · ${result.missing.length} still need local audio` : ""}: ${result.path}`; });
  }
  async function showAllImportDates() {
    if (!activeBrowserImport) return;
    const sourceUrl = activeBrowserImport.sourceUrl, request = ++navigationRequest;
    const versions = browserImports.filter((item) => item.sourceUrl === sourceUrl);
    const captures = await Promise.all(versions.map((item) => call<Chart>("load_browser_import", { id: item.id })));
    if (request !== navigationRequest || activeBrowserImport?.sourceUrl !== sourceUrl) return;
    const recordings = new Map<string, Track>();
    for (const capture of captures) for (const track of capture.tracks) {
      const previous = recordings.get(track.id);
      const dates = previous ? ((previous.provenance as Record<string, unknown>).importDates as string[]) : [];
      recordings.set(track.id, { ...(previous ?? track), provenance: { ...((previous ?? track).provenance as Record<string, unknown>), importDates: [...new Set([...dates, capture.fetchedAt])] } });
    }
    tracks = [...recordings.values()]; selected = null; checked = []; importDateFilter = "all";
    notice = ""; filter = "";
  }
  async function requestBrowserImport() {
    await run(async () => {
      const url=browserUrl.trim();browserRequest="direct";browserMessage="Reading the public source…";
      try {
        const imported=await call<BrowserImportResult>("import_collection_url",{url});
        browserRequest="";browserMessage=`Imported ${imported.capturedCount} tracks directly${imported.complete?"":" · partial"}.`;
        await finishBrowserImport(imported);
      }catch(failure){
        browserRequest="";
        const reason=String(failure);
        browserRequest=await call<string>("request_browser_import",{url});
        browserMessage=`Direct import unavailable: ${reason}. Sent to the connected browser; use the companion to capture the latest tracklist.`;
        await refreshBrowser();
      }
    });
  }
  async function importBrowserJson() {
    await run(async () => {
      const parsed = JSON.parse(importText);
      const result = await call<BrowserImportResult>("import_browser_snapshot", { snapshot: parsed.snapshot ?? parsed });
      await finishBrowserImport(result); importText = "";
    });
  }
  async function saveSettingsFields(keys: (keyof Settings)[], message: string) {
    const changes = Object.fromEntries(keys.map(key=>[key,settings[key]]));
    await run(async()=>{
      const fresh = await call<Settings>("get_settings");
      await call("save_settings",{settings:{...fresh,...changes}});
      if(keys.includes("musicDirectory")) persistedMusicDirectory=settings.musicDirectory;
      if(keys.includes("playlistDirectories")) void syncMirrors();
      notice=message;
    });
  }
  async function saveSettings() {
    await run(async () => {
      await call("save_settings", { settings });
      persistedMusicDirectory = settings.musicDirectory;
      notice = "Settings saved.";
      void syncMirrors();
    });
  }
  async function savePreferredDownloadSource() {
    const requested = settings.preferredDownloadSource ?? "youtube";
    await run(async () => {
      const fresh = await call<Settings>("get_settings");
      const next = { ...fresh, preferredDownloadSource: requested };
      await call("save_settings", { settings: next });
      settings = { ...settings, preferredDownloadSource: requested, musicDirectory: settings.musicDirectory || fresh.musicDirectory };
      persistedMusicDirectory = fresh.musicDirectory;
      const label = requested === "slskd" ? "Soulseek" : requested === "monochrome" ? "Monochrome" : requested === "soundcloud" ? "SoundCloud" : "YouTube";
      notice = `Preferred download source saved: ${label}.`;
    });
  }
  onMount(() => {
    if(desktop())void call<{setupComplete:boolean}>("get_release_setup").then(status=>{if(status&&!status.setupComplete&&!settings.musicDirectory)showReleaseSetup=true;}).catch(()=>{});

    void call<typeof sidebarLayout>("get_sidebar_layout").then(layout=>{if(layout?.sections&&layout?.assignments)sidebarLayout=layout;}).catch(()=>{});
    void call<RadioStation[]>("get_radio_stations").then(stations=>{radioStations=stations??[];}).catch(()=>{});
    const restored = loadPlaybackQueue();
    if (restored) { playbackQueue.restore(restored.snapshot()); repeatMode = playbackQueue.repeat; refreshQueue(); playingTrack = playbackQueue.current; if (playingTrack) void cacheCover(playingTrack); }
    restorePlaybackPosition();
    try {const left=Number(localStorage.getItem("burrata.layout.sidebar.v1"));if(left>=160&&left<=320)sidebarWidth=left;const right=Number(localStorage.getItem("burrata.layout.info.v1"));if(right>=280&&right<=420)infoPanelWidth=right;}catch{}
    void loadAutotagState().catch(()=>{});
    if(desktop())void call<typeof availableSources>("list_search_sources").then(sources=>availableSources=sources).catch(()=>{});
    const automationTimer = setInterval(() => void publishAutomationPlayback(), 1000);
    const syncTimer = setInterval(() => { if (desktop()) void syncMirrors(); }, 60000);
    if (desktop()) { void syncMirrors(); void navigate("library", false); }
    let unlisten: (() => void) | undefined;
    let unlistenWarning: (() => void) | undefined;
    let disposed = false;
    const extraListeners: (() => void)[] = [];
    const register = <T,>(event: string, handler: (payload: T) => void) => {
      void listen<T>(event, (e) => handler(e.payload)).then((fn) => { if (disposed) fn(); else extraListeners.push(fn); }).catch((e) => (error = String(e)));
    };
    const browserTimer = setInterval(() => {
      if (desktop() && showImport) void refreshBrowser().catch((e) => (browserMessage = String(e)));
    }, 5000);
    if (desktop()) {
      const startupNavigationRequest = navigationRequest;
      register<Parameters<typeof handleAutomationPlayback>[0]>("automation-playback", (request) => { void handleAutomationPlayback(request); });
      register<{requestId: string}>("automation-playback-cancelled", (request) => { if (activeAutomationPlayback === request.requestId) { previewRequests.next(); audio?.pause(); paused = true; activeAutomationPlayback = null; if (preview?.sessionId) void call("stop_candidate_preview", {sessionId: preview.sessionId}); } });
      register<{stations:RadioStation[]}>("radio-stations-changed",({stations})=>{
        radioStations=stations;
        if(!stations.some(station=>station.id===selectedRadioStationId))selectedRadioStationId=stations[0]?.id??"";
      });
      register<unknown>("automation-library-changed", () => { void syncMirrors(); if (["library", "collections", "favorites"].includes(view)) { const currentView = view, currentCollection = collectionId; void call<Track[]>("list_tracks", { view: currentView, collectionId: currentView === "collections" ? currentCollection : null }).then((items) => { if (view === currentView && collectionId === currentCollection) tracks = items; }).catch(() => {}); } });
      void call<{permissions: {control: boolean; manage: boolean}}>("get_automation_status").then((status) => { if (status?.permissions) automationPermissions = status.permissions; }).catch(() => {});
      void publishAutomationPlayback();
      register<{ assetId: string; inputHash?: string; version?: number; status: string; bpm?: number | null; key?: string | null; error?: string; payload?: Record<string, unknown> | null }>("analysis-progress", (status) => {
        analysisStatuses = [...analysisStatuses.filter((item) => item.assetId !== status.assetId), status];
      });
      register<Track>("library-updated", (updated) => { void handleLibraryUpdated(updated); });
      register<{ok: boolean; status?: {warnings: string[]; pausedMirrors: string[]; sources: {collectionId?: string | null}[]}; error?: string; watcherWarnings?: string[]}>("playlist-sync", (event) => {
        if (event.status) { mirrorStatus = [...event.status.warnings, ...(event.watcherWarnings ?? [])].join(" · "); pausedMirrors = event.status.pausedMirrors; localPlaylistIds = event.status.sources.map((source) => source.collectionId).filter((id): id is string => Boolean(id)); }
        else if (event.error) mirrorStatus = event.error;
        void call<Collection[]>("list_collections").then((result) => { collections = result; }).catch(() => {});
        if (view === "collections" || view === "library") {
          const expectedView = view, expectedCollection = collectionId;
          void call<Track[]>("list_tracks", { view, collectionId: view === "collections" ? collectionId : null }).then((result) => { if (view === expectedView && collectionId === expectedCollection) tracks = result; }).catch(() => {});
        }
      });
      register("autotag-status", ()=>void (async()=>{
        await loadAutotagState();assets=await call<LocalAsset[]>("list_assets");
        const latest=await call<Track[]>("list_tracks",{view:"all",collectionId:null});
        const byId=new Map(latest.map(track=>[track.id,track]));
        if(playingTrack)playingTrack=byId.get(playingTrack.id) ?? playingTrack;
        playbackQueue.restore({...playbackQueue.snapshot(),tracks:queueTracks.map(track=>byId.get(track.id) ?? track)});refreshQueue();
        tracks=await call<Track[]>("list_tracks",{view:view === "collections"?"collection":view,collectionId:collectionId || null});
      })().catch(()=>{}));
      register<CandidatePreviewStatus>("candidate-preview-status", (status) => {
        if (preview?.sessionId !== status.sessionId) return;
        paused = status.paused; updatePlaybackPosition(status.position); duration = status.duration;
        preview = { ...preview, canSeek: status.canSeek, canVolume: status.canVolume };
        if (status.error) {
          previewStatus = status.error;
        }
        else if (status.closed) { preview = null; paused = true; previewStatus = "Preview window closed"; }
        else previewStatus = status.paused
          ? `Paused · ${preview.provider === "youtube" ? "YouTube" : "SoundCloud"}`
          : `Playing via ${preview.provider}`;
      });
      register<BrowserStatus>("browser-status",(status)=>{browserStatus=status;});
      register<BrowserImportResult>("browser-imported", (result) => {
        browserMessage = "Import received."; browserRequest = "";
        browserImports = [result, ...browserImports.filter((item) => item.sourceUrl !== result.sourceUrl)];
        void run(async () => { collections = await call<Collection[]>("list_collections"); await finishBrowserImport(result); });
      });
      register<{requestId: string; state: string; message: string}>("browser-import-status", (status) => {
        if (!browserRequest || browserRequest === status.requestId) browserMessage = status.message;
      });
      void call<{ settings: { slskd: { enabled: boolean } } }>("get_connections")
        .then((status) => {slskdEnabled = status.settings.slskd.enabled;sourceCapabilities={soulseek:slskdEnabled,discogs:Boolean((status.settings as {discogsEnabled?:boolean}).discogsEnabled)};})
        .catch(() => {});
      void call<Settings>("get_settings").then((value) => {
        persistedMusicDirectory = value.musicDirectory;
        // Keep a directory the user has typed while startup settings are still resolving.
        if (!settings.musicDirectory || settings.musicDirectory === persistedMusicDirectory) settings = value;
      }).catch((e) => (error = String(e)));
      void call<string[]>("get_diagnostics").then((diagnostics) => {
        if (diagnostics.length) notice = diagnostics.join(" · ");
      }).catch((e) => (error = String(e)));
      void run(async () => {
        const [, savedCollections] = await Promise.all([
          refreshBrowser().catch((e) => (browserMessage = String(e))),
          call<Collection[]>("list_collections"),
        ]);
        collections = savedCollections;
        if (!collectionId && collections.length) {
          collectionId = [...collections].sort((a, b) => b.trackCount - a.trackCount)[0].id;
        }
        if (startupNavigationRequest === navigationRequest && view === "charts" && browserImports.length) {
          await openBrowserImport(browserImports[0]);
        } else if (startupNavigationRequest === navigationRequest && view === "charts") {
          await loadGenres();
          if (genreUrl) {
            const savedSnapshots = await call<Chart[]>("list_snapshots", { providerId: provider, url: genreUrl });
            if (startupNavigationRequest === navigationRequest && view === "charts") {
              snapshots = savedSnapshots;
              if (snapshots.length) await loadSnapshot(snapshots[0].id, startupNavigationRequest);
            }
          }
        }
      });
      void listen<string>(
        "provider-warning",
        (e) => (notice = String(e.payload)),
      )
        .then((fn) => {
          if (disposed) fn();
          else unlistenWarning = fn;
        })
        .catch((e) => (error = String(e)));
      void listen<DownloadJob>(
        "download-progress",
        (e) => {
          const index = jobs.findIndex((job) => job.id === e.payload.id);
          jobs = index < 0
            ? [e.payload, ...jobs]
            : jobs.map((job, i) => i === index ? e.payload : job);
        },
      )
        .then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        })
        .catch((e) => (error = String(e)));
    }
    return () => {
      disposed = true;
      batchCancelled = true;
      unlisten?.();
      unlistenWarning?.();
      extraListeners.forEach((fn) => fn()); clearInterval(browserTimer); clearInterval(syncTimer); clearInterval(automationTimer);
      if (preview?.sessionId) void call("stop_candidate_preview", { sessionId: preview.sessionId }).catch(() => {});
      clearTimeout(autoTimer);
      previewRequests.next();
      audio?.pause();
    };
  });
</script>

<svelte:window onkeydown={keyboard} bind:innerWidth={screenWidth} />
<div class:light={settings.theme === "light"} class:has-player-panel={showPlayingInfo || showQueue} class="app-shell" style={`--player-height: ${playerHeight}px; --library-sidebar-width:${Math.min(sidebarWidth,Math.max(160,screenWidth-360))}px; --info-panel-width:${infoPanelWidth}px`}>
  <LibrarySidebar playlistNames={sidebarLayout.names??{}} onRadioContext={(station,event)=>radioActions={station,anchor:{x:event.clientX,y:event.clientY}}} {radioStations} activeRadioStationId={selectedRadioStationId} onRadioStation={(station)=>{showAddRadio=false;selectedRadioStationId=station.id;void navigate("radio");playRadioStation(station);}} onAddRadio={()=>{showAddRadio=true;void navigate("radio");}} width={sidebarWidth} onWidthChange={(width)=>saveLayoutWidth("sidebar",width)} {view} collections={namedCollections} browserImports={latestImports} {localPlaylistIds} activeSource="all" activeCollectionId={view === "collections" || view === "charts" ? collectionId : ""}
    onNavigate={(next) => { if (next === "library") { if (view !== "library" || librarySource !== "all") void chooseLibrarySource("all"); } else if (view !== next) void navigate(next as View); }}
    onPlaylist={(id) => void openPlaylist(id)} onPlaylistContext={(id, event) => { const collection = namedCollections.find(item => item.id === id)??(["favorites","history"].includes(id)?{id,name:sidebarLayout.names?.[id]??(id==="favorites"?"Liked Songs":"History"),trackCount:0}:null); if (collection) playlistActions = { collection, anchor: { x: event.clientX, y: event.clientY } }; }} onSource={(source) => void browseProviderCharts(source)}
    sections={sidebarLayout.sections} assignments={sidebarLayout.assignments} onCreateSection={createSection} onRenameSection={renameSection} onDeleteSection={deleteSection} onMovePlaylist={movePlaylistToSection} onCreatePlaylist={(sectionId)=>void createNewPlaylist(sectionId)}
    onImport={() => void createNewPlaylist()} onSettings={() => void navigate("settings")} onDownloads={() => { openPlayerPanel("downloads"); void refreshJobs(); }}
    activeJobs={jobs.filter((job) => !["completed", "failed", "cancelled"].includes(job.state)).length} />
  <main>
    <header>
      <div class="navigation-controls"><button aria-label="Go back" disabled={!backRoutes.length} onclick={() => void travelRoute("back")}>←</button><button aria-label="Go forward" disabled={!forwardRoutes.length} onclick={() => void travelRoute("forward")}>→</button></div>
      <div>
        <div class="eyebrow">DISCOVER · AUDITION · COLLECT</div>
        <h1>
          {view === "source-browser" ? (provider === "lastfm" ? "Last.fm" : provider === "shazam" ? "Shazam" : "Bandcamp") : view === "charts"
            ? (activeBrowserImport ? (sidebarLayout.names?.[activeBrowserImport.collectionId]??playlistTitle(activeBrowserImport.title,activeBrowserImport.provider)) : "Top 100")
            : view === "collections"
              ? (sidebarLayout.names?.[collectionId]??currentCollection?.name ?? "Playlists")
              : view === "favorites" ? (sidebarLayout.names?.favorites??"Liked Songs") : view === "history" ? (sidebarLayout.names?.history??"Listening history") : view[0].toUpperCase() + view.slice(1)}
        </h1>
      </div>
      <div class="header-actions">
        {#if busy}<span class="busy">Working…</span>{/if}{#if currentCollectionSourceUrl}<button title="Open the original source, then refresh it and capture a new import with the browser extension." onclick={() => void openSourceToRefresh(currentCollectionSourceUrl!)}>Open source to refresh ↗</button>{/if}<button
          onclick={() => void createNewPlaylist()}>＋ New playlist</button
        >
      </div>
    </header>
    {#if !desktop()}<div class="banner">
        Desktop connection unavailable. Run the Tauri app to browse real charts
        and access local files.
      </div>{/if}
    {#if error}<div class="banner error" role="alert">
        {error}<button aria-label="Dismiss error" onclick={() => (error = "")}
          >×</button
        >
      </div>{/if}
    {#if notice}<div class="banner notice" role="status">
        {notice}<button
          aria-label="Dismiss notice"
          onclick={() => (notice = "")}>×</button
        >
      </div>{/if}
    {#if deletedPlaylist}<div class="banner notice"><span>Deleted playlist: {deletedPlaylist.name}</span><button onclick={() => void undoDeletePlaylist()}>Undo deletion</button></div>{/if}
    {#if deletedCollections}<div class="modal-backdrop"><dialog open aria-label="Deleted playlists" class="playlist-trash"><h2>Deleted playlists</h2><p>Restore a playlist with its tracks. Audio files are kept when playlists are deleted.</p>{#each deletedCollections as item (item.id)}<div class="trash-row"><span>{item.name}<small>{item.trackCount} tracks</small></span><button aria-label={`Restore ${item.name}`} onclick={() => void undoDeletePlaylist(item)}>Restore</button></div>{:else}<p>No deleted playlists.</p>{/each}<button onclick={() => (deletedCollections = null)}>Close</button></dialog></div>{/if}
    {#if showImport}<section class="import-panel">
        <h3>Import charts, playlists, or albums</h3>
        <p class="browser-help">Public tracklists are imported directly when available. Otherwise Burrata sends the source to your connected companion browser; use the companion to import the latest version. Your browser handles sign-in or verification.</p>
        <div class="toolbar">
          <input aria-label="Chart, playlist or album URL" bind:value={browserUrl} placeholder="Chart, Spotify playlist, Bandcamp album, Shazam or Last.fm URL" />
          <button disabled={!browserUrl.trim() || Boolean(browserRequest)} onclick={() => void requestBrowserImport()}>Import URL</button>
          <button onclick={() => void run(refreshBrowser)}>Refresh connection</button>
        </div>
        <p role="status">{browserStatus?.connected ? "Browser extension connected" : "Companion not seen yet · open its popup to connect"}{browserStatus?.pendingCount ? ` · ${browserStatus.pendingCount} pending` : ""}{browserMessage ? ` · ${browserMessage}` : ""}</p>
        {#if browserRequest}<button onclick={() => { browserRequest = ""; browserMessage = "The browser request may still finish. You can submit another URL."; }}>Allow another request</button>{/if}
        {#if browserImports.length}<details open><summary>Saved browser imports</summary><div class="browser-imports">
          {#each browserImports as imported}<button onclick={() => void run(() => openBrowserImport(imported))}>
            {imported.provider} · {imported.title} · {imported.importedCount}{imported.expectedTotal != null ? ` / ${imported.expectedTotal}` : ""} recordings · {imported.complete ? "complete" : "partial"}
          </button>{/each}
        </div></details>{/if}
        {#if activeBrowserImport && !importTargetCollectionId}<div class="browser-capture">
          <strong>{activeBrowserImport.title}</strong> · {activeBrowserImport.kind} · {activeBrowserImport.capturedCount} entries captured, {activeBrowserImport.importedCount} unique recordings{activeBrowserImport.expectedTotal != null ? ` of ${activeBrowserImport.expectedTotal} expected entries` : " · total unknown"} · {activeBrowserImport.complete ? "complete" : "partial capture"}
          {#each activeBrowserImport.warnings as warning}<p>{warning}</p>{/each}
        </div>{/if}

        <div class="toolbar">
          <label class="button" for="m3u-import-file">Import M3U / M3U8 file</label>
          <input id="m3u-import-file" type="file" accept=".m3u,.m3u8,audio/x-mpegurl,application/vnd.apple.mpegurl" onchange={importM3uFile} />
          <select aria-label="Import format" bind:value={importKind}>
            <option value="tracklist">Tracklist</option>
            <option value="csv">CSV / Spotify export</option>
            <option value="m3u">M3U / M3U8 metadata</option>
            <option value="browser-json">Browser snapshot JSON</option>
          </select>
          <button
            onclick={() => void (importKind === "browser-json" ? importBrowserJson() : importTracks())}
            disabled={!importText.trim()}>Import tracks</button
          >
          <button onclick={() => (showImport = false)}>Close</button>
        </div>
        <textarea
          bind:value={importText}
          placeholder="Paste an artist — title tracklist, CSV, or Spotify playlist CSV export here"
          rows="5"
        >
        </textarea>
        <div class="toolbar">
          <input
            bind:value={localPath}
            placeholder="Full path to a local audio file"
          />
          <button
            disabled={!localPath.trim()}
            onclick={() =>
              void run(async () => {
                const t = await call<Track>("import_local_file", {
                  path: localPath,
                });
                tracks = [t, ...tracks];
                showImport = false;
              })}>Import local file</button
          >
        </div>
      </section>{/if}
    {#if autotagReviewId && currentTagJob && currentTagAsset}<section class="autotag-page"><button onclick={()=>autotagReviewId=null}>← Back</button><AutotagReview job={currentTagJob} asset={currentTagAsset} onReview={(assetId,providerId)=>call<AutotagJob>("review_autotag_candidate",{assetId,providerId})} onApply={async(assetId,fieldKeys)=>{await call("apply_autotag_review",{assetId,fieldKeys});assets=await call<LocalAsset[]>("list_assets");await loadAutotagState();}} onRetry={async(assetId)=>{await call("retry_autotag",{assetId});await loadAutotagState();}} /></section>
    {:else if view === "artist"}
      <section class="artist-page-host">
        <ArtistPage
          activeTrack={activeArtistTrack}
          artistName={activeArtistName}
          initialArtistRef={activeArtistRef}
          initialSection={artistSection} initialScroll={artistScroll} onScroll={(position) => (artistScroll = position)} onSectionChange={(section) => { if (artistSection !== section) { rememberRoute(); if (!restoringRoute) artistScroll = 0; } artistSection = section; }}
          onArtistSelected={(artist, name) => { if (activeArtistRef?.id === artist?.id && activeArtistRef?.provider === artist?.provider) return; rememberRoute(); if (!restoringRoute) artistScroll = 0; activeArtistRef = artist; if (name) activeArtistName = name; }}
          localTracks={assets.map((asset) => asset.track)}
          {searchArtists}
          {getArtistProfile}
          {getArtistReleases}
          {getReleaseTracks}
          {getArtistTracks}
          {getArtistCollaborations}
          {resolveRecordingMetadata}
          onApplyMetadata={applyArtistMetadata}
          onPlayTrack={playArtistTrack}
          onDownloadAlbum={(items)=>void queueSelected(items)}
          onQueueTracks={queueArtistTracks}
          onTrackContext={(track, event) => { actionAnchor = { x: event.clientX, y: event.clientY }; actionTrack = artistPlaybackTracks([track], assets)[0] ?? track; }}
          onDownloadTrack={(track) => openDownloadChooser(track)}
          onAddTrackToPlaylist={addArtistTrack}
          onArtistNavigate={discoverArtistRef}
        />
      </section>
    {:else if view === "settings"}
      <section class="settings" aria-label="Settings">
        <ReleaseSetup onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings=value)}/>
        <header class="settings-heading"><p>Choose your audio sources, library folders, and playback preferences.</p></header>
        <div class="settings-section-heading"><h2>Audio sources</h2><span>Changes save automatically</span></div>
        <div class="settings-source-grid">
          <section class="settings-source-card" aria-labelledby="streaming-settings-title">
            <h3 id="streaming-settings-title">Streaming</h3><p>Play audio without downloading a file.</p>
            <label>Primary source<select aria-label="Preferred streaming source" bind:value={settings.preferredStreamingSource} onchange={() => void run(async () => { const fresh = await call<Settings>("get_settings"); await call("save_settings", { settings: { ...fresh, preferredStreamingSource: settings.preferredStreamingSource } }); })}>
            <option value="youtube">YouTube</option><option value="soundcloud">SoundCloud</option><option value="monochrome">Monochrome</option>
          </select><small>Local files play first. Monochrome requires a strong recording match.</small></label>
            <FallbackSettings label="Streaming" enabled={settings.streamingFallbackEnabled !== false} providers={settings.streamingFallbackSources ?? ["youtube", "soundcloud"]} primary={settings.preferredStreamingSource ?? "youtube"} sources={[{id:"monochrome",name:"Monochrome"},{id:"youtube",name:"YouTube"},{id:"soundcloud",name:"SoundCloud"}]} onchange={(enabled,sources)=>void saveFallbackSettings("streaming",enabled,sources)} />
            <label class="settings-option"><input type="checkbox" aria-label="Override playlist track URLs" bind:checked={settings.overridePlaylistTrackUrls} onchange={() => void run(async () => { const fresh = await call<Settings>("get_settings"); await call("save_settings", { settings: { ...fresh, overridePlaylistTrackUrls: settings.overridePlaylistTrackUrls } }); })} /><span><strong>Override playlist track URLs</strong><small>Try the preferred source and configured streaming fallbacks. Off keeps the specified playlist URL.</small></span></label>
          </section>
          <section class="settings-source-card" aria-labelledby="download-settings-title">
            <h3 id="download-settings-title">Downloads</h3><p>Save a recording to your music folder.</p>
            <label>Primary source<select aria-label="Preferred download source" bind:value={settings.preferredDownloadSource} onchange={() => void savePreferredDownloadSource()}>
            <option value="youtube">YouTube</option>
            <option value="soundcloud">SoundCloud</option>
            <option value="slskd">Soulseek</option>
            <option value="monochrome">Monochrome</option>
          </select><small>Default for Charts and Library. A source you explicitly select takes priority.</small></label>
            <FallbackSettings label="Download" enabled={settings.downloadFallbackEnabled === true} providers={settings.downloadFallbackSources ?? ["monochrome","slskd","youtube","soundcloud"]} primary={settings.preferredDownloadSource ?? "youtube"} sources={[{id:"monochrome",name:"Monochrome"},{id:"slskd",name:"Soulseek"},{id:"youtube",name:"YouTube"},{id:"soundcloud",name:"SoundCloud"}]} onchange={(enabled,sources)=>void saveFallbackSettings("download",enabled,sources)} />
            <p class="settings-source-note">Soulseek is currently available for downloads only.</p>
            <label class="settings-option"><input type="checkbox" aria-label="Allow lossy download fallback" bind:checked={settings.downloadFallbackAllowLossy} onchange={() => void run(async()=>{ const fresh=await call<Settings>("get_settings"); await call("save_settings",{settings:{...fresh,downloadFallbackAllowLossy:settings.downloadFallbackAllowLossy}}); })} /><span><strong>Allow a lossy fallback</strong><small>Otherwise, a lossless download only falls back to another lossless source. Chapter downloads keep their selected source.</small></span></label>
          </section>
        </div>
        <section class="settings-basic-card" aria-labelledby="library-settings-title">
          <h2 id="library-settings-title">Music folder</h2><p>Completed downloads are saved here after validation and tagging.</p>
          <label>Download destination<input bind:value={settings.musicDirectory} placeholder="/home/you/Music/DJ" /></label>
          <button onclick={()=>void saveSettingsFields(["musicDirectory"],"Music folder saved.")}>Save music folder</button>
        </section>
        <LocalPlaylistFiles mode="settings" {settings} onChange={(value) => (settings = value)} onSave={() => void saveSettingsFields(["playlistDirectories"],"Playlist folders saved.")} />
        <section class="settings-basic-card" aria-labelledby="playback-settings-title">
          <h2 id="playback-settings-title">Appearance & playback</h2>
          <div class="settings-grid"><label>Theme<select bind:value={settings.theme}><option value="dark">Dark</option><option value="light">Light</option></select></label>
          <label>Preview delay (ms)<input type="number" min="0" max="5000" bind:value={settings.previewDelayMs} /></label></div>
          <label class="settings-option"><input type="checkbox" bind:checked={settings.autoPreview} /><span><strong>Play songs selected with arrow keys</strong><small>Wait for the preview delay before starting audio.</small></span></label>
          <button onclick={()=>void saveSettingsFields(["theme","autoPreview","previewDelayMs"],"Playback preferences saved.")}>Save playback preferences</button>
        </section>
        <AutotagSetup {...autotagSettings} acoustIdConfigured={autotagSettings.acoustidConfigured} onChanged={(change)=>autotagSettings={...autotagSettings,...change}} onSave={()=>void run(async()=>{autotagSettings=await call("save_autotag_settings",{enabled:autotagSettings.enabled,shazamFallback:autotagSettings.shazamFallback});})} />
        <ConnectionsSetup onChanged={(status) => { slskdEnabled = status.settings.slskd.enabled; sourceCapabilities={soulseek:slskdEnabled,discogs:status.settings.discogsEnabled}; void call<typeof availableSources>("list_search_sources").then(sources=>availableSources=sources).catch(()=>{}); void loadAutotagState().catch(()=>{}); }} />
        <details class="settings-advanced"><summary><strong>Tools & diagnostics</strong><span>Executable paths and dependency checks</span></summary>
          <ToolSetup {settings} {dependencies} onChange={(value)=>(settings=value)} onCheck={()=>void run(async()=>(dependencies=await call<Dependency[]>("check_dependencies")))} onSave={()=>void saveSettingsFields(["ytDlpPath","ffmpegPath","ffprobePath","jsRuntimePath"],"Tool paths saved.")} checking={busy>0} />
        </details>
        <details class="settings-advanced"><summary><strong>AI control</strong><span>Local MCP connections and permissions</span></summary>
          <section class="automation-settings"><p>Allow a connected assistant to use the player and library.</p><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.control} onchange={()=>void saveAutomationPermissions()} /><span>Allow playback and playlist changes</span></label><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.manage} onchange={()=>void saveAutomationPermissions()} /><span>Allow downloads and audio analysis</span></label><details><summary>Connection instructions</summary><p>Start the local MCP bridge with <span>Use the installed MCP command shown in YouTube & integrations above.</span>.</p></details></section>
        </details>
      </section>
    {:else if view === "search"}
      {#if searchRelease}<ReleaseView release={searchRelease} tracks={searchReleaseTracks} loading={searchReleaseBusy} error={searchReleaseError} onBack={()=>searchRelease=null} onPlay={playArtistTrack} onQueue={queueArtistTracks} onDownload={(track)=>void openDownloadChooser(track)} onDownloadAlbum={(items)=>void queueSelected(items)} onArtist={(track)=>void discoverArtist(track)} onPlaylist={(track)=>void addArtistTrack(track)} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} />{:else}
      <SearchPanel {availableSources} {query} searchPreset={catalogSearchRequest ? {sourceId:"musicbrainz",kind:"all",requestId:catalogSearchRequest} : undefined} onQueryChange={(value)=>query=value} {sourceCapabilities} onArtist={(artist)=>void discoverArtistRef(artist)} onArtistTrack={(track)=>void discoverArtist(track)} onAlbum={(release)=>void openSearchAlbum(release)} onPlay={(track)=>void startPreview(track,false,false,[track],0)} onDownload={(track)=>void openDownloadChooser(track)} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onContext={(track,event)=>{actionTrack=track;actionAnchor=event ? {x:event.clientX,y:event.clientY}:null;}} onPlayChapter={playChapter} onQueueChapter={queueChapter} onDownloadChapter={(parent,candidate,chapter)=>void downloadChapter(parent,candidate,chapter)} onInfoChapter={(parent,candidate,chapter)=>{const virtual=virtualChapterTrack(parent,candidate,chapter);if(virtual)searchMoreInfo(virtual,candidate,chapter.title);}} />{/if}
    {:else if view === "radio"}
      <RadioView savedStations={radioStations} stationNames={Object.fromEntries(radioStations.map(station=>[station.id,station.name]))} onAddClosed={()=>showAddRadio=false} selectedStationId={selectedRadioStationId} showAdd={showAddRadio} onStationsChanged={(stations)=>{const added=stations.find(station=>!radioStations.some(existing=>existing.id===station.id));radioStations=stations;if(added)selectedRadioStationId=added.id;else if(!stations.some(station=>station.id===selectedRadioStationId))selectedRadioStationId=stations[0]?.id??"";}} onPlayStation={playRadioStation} onArtist={(track)=>void discoverArtist(track)} playingStationId={!paused ? playingRadioStation?.id??"" : ""} onNowPlaying={radioNowPlaying} />
    {:else if view === "source-browser"}
      <SourcePresets {provider} onImport={importPreset} onOpen={(url)=>void run(()=>call("open_external_url",{url}))} />
    {:else if view === "history"}
      <section class="history-page" aria-label="Listening history">
        <div class="detail-actions"><button disabled={!historyItems.length} onclick={() => void run(async () => { await call("clear_playback_history"); historyItems = []; })}>Clear history</button></div>
        {#each historyItems as item, index (item.sessionId)}<article class="history-row"><button class="history-play" aria-label={`Play ${item.track.title}`} onclick={() => void startPreview(item.track, false, false, historyItems.map((entry) => entry.track), index)}>▶</button><button class="history-title" onclick={() => { selected = item.track; void navigate("library").then(() => (selected = item.track)); }}><strong>{item.track.title}</strong><small>{item.track.artist} · {item.provider}</small></button><time>{new Date(item.playedAt).toLocaleString()}</time><button aria-label={`Download ${item.track.title}`} onclick={() => void downloadTopMatch(item.track)}>↓</button></article>{:else}<div class="empty"><h2>Nothing played yet</h2><p>Songs appear here when audio starts.</p></div>{/each}
        {#if historyItems.length && historyItems.length % 100 === 0}<button onclick={() => void run(async () => { const more = await call<typeof historyItems>("list_playback_history", { limit: 100, offset: historyItems.length }); historyItems = [...historyItems, ...more]; })}>Show earlier plays</button>{/if}
      </section>
    {:else if view === "downloads"}
      <div class="toolbar">
        <span>{jobs.length} jobs</span>
        <button onclick={() => void refreshJobs()}>Refresh queue</button>
      </div>
      <section class="queue">
        {#each jobs as job (job.id)}<article class="job">
            <div>
              <strong>{downloadTitle(job)}</strong>
              <p>{job.track.artist || "Artist not identified"}{#if job.track.version} · {job.track.version}{/if}</p>
              <p>{downloadSourceLabel(job)}{#if downloadPeer(job)} · {downloadPeer(job)}{/if}</p>
              {#if downloadTransferDetail(job)}<p>{downloadTransferDetail(job)}</p>{/if}
              <small>{downloadStateLabel(job.state)}{#if job.state === "downloading"} · {Math.round(job.progress)}%{/if}</small>
              {#if job.state === "downloading"}<progress max="100" value={job.progress} aria-label={`${downloadTitle(job)} download progress`}></progress>{/if}
              {#if job.error}<p class="error-text">{job.error}</p>{/if}
            </div>
            <div>
              {#if ["failed", "cancelled", "interrupted"].includes(job.state)}<button
                  onclick={() =>
                    void run(async () => {
                      await call("retry_download", { jobId: job.id });
                      await refreshJobs();
                    })}>Retry</button
                >{:else if job.state !== "completed"}<button
                  onclick={() =>
                    void run(async () => {
                      await call("cancel_download", { jobId: job.id });
                      await refreshJobs();
                    })}>Cancel</button
                >{/if}{#if job.outputPath}<button
                  onclick={() =>
                    void run(() =>
                      call("open_in_file_manager", { path: job.outputPath }),
                    )}>Show file</button
                >{/if}
            </div>
          </article>{:else}<div class="empty">
            <span>↓</span>
            <h2>Your queue is clear</h2>
            <p>
              Choose a recording, compare its sources, then start a download.
            </p>
          </div>{/each}
      </section>
    {:else}
      <div class="toolbar" class:chart-toolbar={view === "charts"} class:library-toolbar={view === "library"}>
        {#if view === "charts" && !activeBrowserImport}<select
            aria-label="Chart provider"
            bind:value={provider}
            onchange={() => {
              genres = [];
              genreUrl = "";
              void loadGenres();
            }}
          >
            <option value="traxsource">Traxsource</option>
            <option value="beatport">Beatport</option>
          </select>
          <select aria-label="Chart genre" bind:value={genreUrl}>
            <option value="">Choose genre</option>
            {#each genres as g}<option value={g.chartUrl}>{g.name}</option
              >{/each}</select
          >
          <button disabled={!genreUrl} onclick={() => void run(async () => { await call("open_external_url", { url: genreUrl }); })}>Open Top 100 in browser ↗</button>
          <button
            class="primary"
            onclick={() => void loadChart()}
            disabled={!genreUrl}>Browse Top 100</button
          >
          <button onclick={() => void loadChart(true)} disabled={!genreUrl}
            >↻</button
          >
        {:else if view === "library"}

          {#if showCreatePlaylist}<input bind:value={newCollection} placeholder="New playlist name" aria-label="New playlist name" /><button disabled={!newCollection.trim()} onclick={() => void createCollection().then(() => (showCreatePlaylist = false))}>Create</button>{/if}
          <details class="library-tools"><summary>Library options</summary><div><button onclick={openPlaylistImport}>Import charts or playlists</button>
            <button onclick={() => void syncMirrors()}>Sync M3U files</button>
            <button onclick={() => void run(async () => { deletedCollections = await call<Collection[]>("list_deleted_collections"); })}>Deleted playlists</button>
            {#if assets.length}<button disabled={analysisBusy} onclick={() => void analyzeLibraryAssets()}>{analysisBusy ? "Analyzing…" : "Analyze missing BPM / key"}</button>{/if}
          </div></details>
        {:else if view === "collections"}<button onclick={()=>{renamePlaylistName=currentCollection?.name??"";renamingPlaylist=true;}}>Rename</button><button onclick={openPlaylistImport}>Import tracks</button>
          {#if renamingPlaylist}<input aria-label="Playlist name" bind:value={renamePlaylistName} onkeydown={(event)=>{if(event.key==="Enter")void renameCurrentPlaylist();if(event.key==="Escape")renamingPlaylist=false;}} /><button onclick={()=>void renameCurrentPlaylist()}>Save name</button>{/if}
          <button disabled={!collectionId || batchRunning} onclick={() => void downloadPlaylist(collectionId, true)}>↓ Download playlist</button><details class="playlist-import-actions"><summary>Playlist actions</summary><button disabled={!collectionId} onclick={() => void exportMixxxPlaylist(collectionId)}>Export local tracks for Mixxx</button><select
            aria-label="Playlist"
            bind:value={collectionId}
            onchange={() => void navigate("collections")}
          >
            <option value="">Choose playlist</option>
            {#each collections as c}<option value={c.id}
                >{c.name} ({c.trackCount})</option
              >{/each}</select
          >
          <button disabled={!currentCollection} onclick={() => void deletePlaylist()}>Delete playlist</button>
          <button
            disabled={!collectionId}
            onclick={() =>
              void run(
                async () =>
                  (notice = `Playlist exported: ${await call<string>("export_playlist", { collectionId })}`),
              )}>Export M3U8</button
          ></details>{/if}
      </div>
      {#if view === "charts"}<div class="chart-context">
          <span
            >{activeBrowserImport ? "" : chart?.title ?? "Genre charts"}{#if chart?.stale}<span class="tag"
                >Cached</span
              >{/if}</span
          >
          <div class="chart-source-actions">
            {#if activeBrowserImport}<button disabled={batchRunning} onclick={() => void downloadPlaylist(activeBrowserImport!.collectionId, true)}>↓ Download playlist</button><details class="playlist-import-actions"><summary>Playlist actions</summary>{#if currentCollection}<button onclick={() => void deletePlaylist()}>Delete this import</button>{/if}<button onclick={() => void run(copyCurrentImport)}>Make editable copy</button><button onclick={() => void exportMixxxPlaylist(activeBrowserImport!.collectionId)}>Export local tracks for Mixxx</button></details>{/if}
            {#if currentChartSourceUrl}<button class="chart-open-source" title="Open the original source, then refresh it and capture a new import with the browser extension." onclick={() => void openSourceToRefresh(currentChartSourceUrl!)}>Open source to refresh ↗</button>{/if}
            {#if activeBrowserImport}<label>Import date<select aria-label="Imported playlist version" value={importDateFilter || activeBrowserImport.id} onchange={(event) => { if (event.currentTarget.value === "all") void run(showAllImportDates); else { const version = browserImports.find((item) => item.id === event.currentTarget.value); if (version) void run(() => openBrowserImport(version)); } }}><option value="all">All import dates</option>{#each browserImports.filter((item) => item.sourceUrl === activeBrowserImport?.sourceUrl) as version}<option value={version.id}>{new Date(version.fetchedAt).toLocaleString()} · {version.capturedCount} tracks{version.complete ? "" : " · partial"}</option>{/each}</select></label>{/if}
            {#if chart}<small class="chart-date"
                >{new Date(chart.fetchedAt).toLocaleString()}</small
              >{/if}{#if snapshots.length}<select class="desktop-chart-snapshot"
                aria-label="Saved chart snapshots"
                value={chart?.id ?? ""}
                onchange={(e) => void loadSnapshot(e.currentTarget.value)}
              >
                {#each snapshots as s}<option value={s.id}
                    >{new Date(s.fetchedAt).toLocaleString()}</option
                  >{/each}</select
              >{/if}
          </div>
        </div>
        {#if !activeBrowserImport}<details class="mobile-chart-tools">
          <summary>Browse charts · {provider === "beatport" ? "Beatport" : "Traxsource"}</summary>
          <div class="mobile-chart-tool-fields">
            <label>Provider<select aria-label="Mobile chart provider" bind:value={provider} onchange={() => { genres = []; genreUrl = ""; void loadGenres(); }}><option value="traxsource">Traxsource</option><option value="beatport">Beatport</option></select></label>
            <label>Genre<select aria-label="Mobile chart genre" bind:value={genreUrl}><option value="">Choose genre</option>{#each genres as g}<option value={g.chartUrl}>{g.name}</option>{/each}</select></label>
            <button disabled={!genreUrl} onclick={() => void run(async () => { await call("open_external_url", { url: genreUrl }); })}>Open Top 100 in browser ↗</button>
        <button class="primary" onclick={() => void loadChart()} disabled={!genreUrl}>Browse Top 100</button>
            <button onclick={() => void loadChart(true)} disabled={!genreUrl}>Refresh chart</button>
            {#if snapshots.length}<label>Saved snapshots<select aria-label="Mobile saved chart snapshots" value={chart?.id ?? ""} onchange={(e) => void loadSnapshot(e.currentTarget.value)}>{#each snapshots as s}<option value={s.id}>{new Date(s.fetchedAt).toLocaleString()}</option>{/each}</select></label>{/if}
            <label class="mobile-chart-url">Chart URL<input aria-label="Mobile chart URL" bind:value={genreUrl} placeholder="Paste a supported chart URL" /></label>
            <button class="mobile-chart-open" disabled={!genreUrl} onclick={() => void loadChart()}>Open chart URL</button>
          </div>
        </details>
        <div class="url-row desktop-chart-url">
          <input
            aria-label="Chart URL"
            bind:value={genreUrl}
            placeholder="Or paste a supported chart URL"
          />
          <button disabled={!genreUrl} onclick={() => void loadChart()}
            >Open chart</button
          >
        </div>{/if}{/if}
      {#if view === "library"}
        <section class="library-overview" aria-label="Playlist file status">
          {#if mirrorStatus}<p class="sync-warning" role="status">{mirrorStatus}</p>{/if}
          {#each pausedMirrors as id}<button onclick={() => void run(async () => { await call("resume_playlist_mirror", { collectionId: id }); await syncMirrors(); })}>Restore M3U · {id === "favorites" ? "Liked Songs" : collections.find((item) => item.id === id)?.name ?? "playlist"}</button>{/each}
        </section>
        {#if librarySource === "local"}<div class="local-playlist-region"><LocalPlaylistFiles {settings} onSettings={() => void navigate("settings")} onImported={(created) => { void run(async () => { collections = await call<Collection[]>("list_collections"); await openPlaylist(created.id); }); }} /></div>{/if}
      {/if}
      <div class="workspace" class:track-info-open={showTrackDetails} class:other-panel={showPlayingInfo || showQueue || showDownloads}>
        <section class="track-area">
          <div class="table-tools">
            <input
              bind:value={filter}
              placeholder="Filter this view…"
              aria-label="Filter tracks"
            />
            <label class="local-filter">Availability
              <select aria-label="Filter local recordings" bind:value={localFilter}>
                <option value="all">All recordings</option>
                <option value="local">On this device</option>
                <option value="not-local">Not on this device</option>
              </select>
            </label>
            <select class="track-sort" aria-label="Sort tracks" bind:value={sort} onchange={() => (descending = false)}>
              <option value="rank">Playlist order</option><option value="title">Title</option><option value="artist">Artist</option><option value="bpm">BPM</option><option value="key">Key</option><option value="durationSeconds">Duration</option><option value="releaseDate">Release date</option>
            </select>
            <span>{sorted.length} tracks</span>
            {#if view === "collections" && checked.length && selected}<button disabled={selectedPlaylistIndex() <= 0} onclick={() => void moveSelectedPlaylistEntry(-1)}>Move up</button><button disabled={selectedPlaylistIndex() < 0 || selectedPlaylistIndex() >= tracks.length - 1} onclick={() => void moveSelectedPlaylistEntry(1)}>Move down</button>{/if}
            {#if checked.length}<input aria-label="New playlist for selection" bind:value={newCollection} placeholder="New playlist name" />
              <button disabled={!newCollection.trim()} onclick={() => void createAndAddPlaylist()}>Create & add</button>{/if}
            {#if checked.length}<button onclick={()=>queueArtistTracks(tracks.filter(track=>checked.includes(track.id)),"append")}>Add {checked.length} to queue</button><button aria-label="Select all filtered tracks" onclick={() => { checked = sorted.map(t => t.id); mergeConfirmed = false; }}>Select all</button><button
                onclick={() => void addSelected()}
                disabled={!collectionId}
                >Add {checked.length} to playlist</button
              ><button
                disabled={batchRunning}
                onclick={() => void queueSelected()}
                >Download selected</button
              >
              {#if view === "collections"}<button onclick={() => void removeSelectedFromPlaylist()}>Remove {checked.length} from playlist</button>{/if}
              <button
                onclick={() => {
                  checked = [];
                  mergeConfirmed = false;
                }}>Clear selection</button
              >
              {#if checked.length > 1}<label class="merge-check">
                  <input type="checkbox" bind:checked={mergeConfirmed} /> Same recording
                  & version</label
                >
                <button
                  disabled={!mergeConfirmed}
                  onclick={() => void mergeSelected()}
                  >Merge {checked.length}</button
                >{/if}
            {/if}{#if batchRunning}<span role="status">{batchStatus}</span
              ><button onclick={() => (batchCancelled = true)}
                >Stop batch matching</button
              >{/if}<small
              >↑ ↓ select · Space play · S shortlist · R reject</small
            >
          </div>
          <div
            class="track-scroll desktop-track-list"
            bind:this={table}
            bind:clientHeight={viewportHeight}
            onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
          >
            <table>
              <thead>
                <tr>
                  <th class="track-number"><button aria-label="Sort by position" onclick={() => { descending = sort === "rank" ? !descending : false; sort = "rank"; }}>#</button></th>
                  <th class="track-name"><button onclick={() => { descending = sort === "title" ? !descending : false; sort = "title"; }}>Title{sort === "title" ? descending ? " ↓" : " ↑" : ""}</button></th>
                  <th class="track-album">Album / label</th>
                  <th class="track-tempo"><button onclick={() => { descending = sort === "bpm" ? !descending : false; sort = "bpm"; }}>BPM / key</button></th>
                  <th class="track-time"><button aria-label="Sort by duration" onclick={() => { descending = sort === "durationSeconds" ? !descending : false; sort = "durationSeconds"; }}>Time</button></th>
                  <th class="track-options"><span class="sr-only">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {#if virtualized && startRow > 0}<tr
                    class="spacer"
                    aria-hidden="true"
                    ><td colspan="6" style={`height:${startRow * rowHeight}px`}
                    ></td></tr
                  >{/if}
                {#each visibleTracks as t, rowIndex (`${t.id}:${startRow + rowIndex}`)}<tr
                    class:selected={selected?.id === t.id || checked.includes(t.id)}
                    class:rejected={t.disposition === "reject"}
                    onclick={(event) => selectTrackRow(t,event)}
                    oncontextmenu={(e) => { e.preventDefault(); actionAnchor = { x: e.clientX, y: e.clientY }; actionTrack = t; }}
                  >
                    <td class="track-number">
                      <span class="row-number">{playingTrack?.id === t.id && !paused ? "♫" : startRow + rowIndex + 1}</span>
                      <button class="row-play" aria-label={`Play ${t.title}`} title="Play audio" onclick={(e) => { e.stopPropagation(); select(t); }}>▶</button>
                    </td>
                    <td class="track-name">
                      <div class="track-title" title={bpmAnnotation(t) || undefined}>
                        {#if playingTrack?.id === t.id && !paused}<span
                            class="playing-indicator">▸</span
                          >{/if}{t.title}{#if t.localAvailability === "available" || (!t.localAvailability && t.localPath)}<span class="downloaded-badge" title="Downloaded · available on this device" aria-label="Downloaded"
                            >↓</span
                          >{/if}
                      </div>
                      <div class="track-subtitle">
                        <button class="artist-link" aria-label={`Find recordings by ${t.artist}`} onclick={(e) => { e.stopPropagation(); void discoverArtist(t); }}>{t.artist}</button>{t.version
                          ? ` · ${t.version}`
                          : ""}{t.disposition === "shortlist"
                          ? " · Shortlisted"
                          : ""}
                        {#if rowDownloadFeedback[t.id]}<small class="row-download-feedback" role="status">{rowDownloadFeedback[t.id]}</small>{/if}
                      </div>
                    </td>
                    <td class="track-album" title={t.album ?? t.label ?? ""}>{t.album ?? t.label ?? "—"}</td>
                    <td class="track-tempo">{t.bpm ?? "—"}{#if t.bpm != null && analysisAnnotation(t, "bpm")}<span title={analysisAnnotation(t, "bpm")}> ≈</span>{/if}<small title={analysisAnnotation(t, "key") || undefined}>{t.key ?? ""}{#if t.key && analysisAnnotation(t, "key")} ≈{/if}</small></td>
                    <td class="track-time">{time(t.durationSeconds)}</td>
                    <td class="track-options">
                      <div class="compact-track-actions">
                        {#if checked.length}<input type="checkbox" aria-label={`Select ${t.title}`} checked={checked.includes(t.id)} onclick={(e) => {e.stopPropagation();if(e.shiftKey){e.preventDefault();selectTrackRow(t,e);}}} onchange={(e) => { checked = e.currentTarget.checked ? [...checked, t.id] : checked.filter((id) => id !== t.id); mergeConfirmed = false; }} />{/if}
                        <button class="row-download" aria-label={`Download ${t.title}`} aria-busy={rowDownloadBusy[t.id]} title={rowDownloadFeedback[t.id] || "Download best match"} disabled={rowDownloadBusy[t.id]} onclick={(e) => { e.stopPropagation(); void downloadTopMatch(t); }}>{rowDownloadBusy[t.id] ? "…" : rowDownloadFeedback[t.id]?.startsWith("Queued") ? "✓" : "↓"}</button>
                        <button class="row-more" aria-label={`Actions for ${t.title}`} title="Track actions" onclick={(e) => { e.stopPropagation(); actionAnchor = null; actionTrack = t; }}>⋯</button>
                      </div>
                    </td>
                  </tr>{/each}{#if virtualized && endRow < sorted.length}<tr
                    class="spacer"
                    aria-hidden="true"
                    ><td
                      colspan="6"
                      style={`height:${(sorted.length - endRow) * rowHeight}px`}
                    ></td></tr
                  >{/if}</tbody
              >
            </table>
            {#if !sorted.length}<div class="empty">
                <span>♫</span>
                <h2>
                  {tracks.length
                    ? "No matching recordings"
                    : view === "charts"
                      ? "Find your next record"
                      : view === "collections"
                        ? collectionId
                          ? "This playlist is empty"
                          : "Choose a playlist"
                        : view === "library"
                          ? "No saved recordings yet"
                          : view === "favorites"
                            ? "No favorites yet"
                            : "Nothing here yet"}
                </h2>
                <p>
                  {view === "charts"
                    ? browserImports.length
                      ? "Choose a provider and genre, or open a saved import from the menu above."
                      : "Choose a provider and genre to browse a real chart."

                      : view === "collections"
                        ? collectionId
                          ? "Add recordings from Charts or Search to this playlist."
                          : "Choose a saved playlist above, or create one to collect recordings."
                        : view === "library"
                          ? "Import a playlist, album, or chart to start your library. Completed downloads appear with their local audio files."
                          : view === "favorites"
                            ? "Mark a recording with the star to keep it here."
                      : "Save recordings or import local files to build your library."}
                </p>
              </div>{/if}
          </div>
          {#if view === "library" && selected && showTrackDetails}{@const asset = assets.find(
              (a) => a.recordingId === selected?.id,
            )}{#if asset}<div class="toolbar">
                <span>{asset.finalCodec} · {asset.processing}</span>
                <button
                  onclick={() =>
                    void run(() =>
                      call("open_in_file_manager", { path: asset.path }),
                    )}>Show file</button
                >
                <button
                  onclick={() =>
                    void run(async () => {
                      const result = await call<LocalAsset>("export_mp3", {
                        assetId: asset.id,
                      });
                      notice = `MP3 exported: ${result.path}`;
                    })}>Export MP3</button
                >
              </div>{/if}
          {/if}
        </section>
        <aside class="details" class:drawer-open={Boolean(selected) && showTrackDetails}>
          {#if selected}<button class="drawer-close" aria-label="Close recording details" onclick={() => (showTrackDetails = false)}>Close</button>{#if selected.artworkUrl && !failedArtwork[selected.artworkUrl]}<img
                class="artwork"
                src={selected.artworkUrl}
                onerror={() => { if (selected) recoverArtwork(selected); }}
                alt="Release artwork"
                loading="lazy"
              />{:else}<div class="artwork artwork-empty">♫</div>{/if}
            <div class="eyebrow">{selected.provider} · RECORDING</div>
            <h2>{selected.title}</h2>
            <p class="artist"><button class="artist-link" aria-label={`Find recordings by ${selected.artist}`} onclick={() => void discoverArtist(selected!)}>{selected.artist}</button></p>
            {#if selected.version}<span class="tag">{selected.version}</span
              >{/if}
            <div class="detail-actions">
              <button
                class="primary"
                onclick={() => void startPreview(selected!)}>▶ Preview</button
              >
              <SourceLink track={selected} />
              <button onclick={() => void favorite(selected!)}
                >{selected.favorite ? "★ Saved" : "☆ Save"}</button
              >
            </div>
            <dl>
              {#each [["Label", selected.label], ["Release", selected.album], ["Genre", selected.genre], ["BPM", selected.bpm], ["Key", selected.key], ["Duration", time(selected.durationSeconds)], ["Released", selected.releaseDate], ["ISRC", selected.isrc], ["Catalog", selected.catalogNumber]] as [key, value]}<dt
                >
                  {key}
                </dt>
                <dd>{value ?? "—"}{#if key === "BPM" && analysisAnnotation(selected, "bpm")} <small title={analysisAnnotation(selected, "bpm")}>Estimated</small>{/if}{#if key === "Key" && analysisAnnotation(selected, "key")} <small title={analysisAnnotation(selected, "key")}>Estimated</small>{/if}</dd>{/each}
            </dl>
            <div class="detail-actions">
              <button onclick={() => void disposition(selected!, "shortlist")}
                >Shortlist</button
              >
              <button onclick={() => void disposition(selected!, "reject")}
                >Reject</button
              >
            </div>
            <div class="detail-actions"><button onclick={() => void addArtistTrack(selected!)}>Add to playlist…</button></div>
            <h3>Cached chart appearances</h3>
            {#if membershipTrackId === selected.id}
              {#if membershipLoading}<p class="membership-note">
                  Loading saved appearances…
                </p>
              {:else if membershipError}<p class="membership-note">
                  {membershipError}
                </p>
              {:else if !memberships.length}<p class="membership-note">
                  No saved chart appearances for this recording.
                </p>
              {:else}{#each memberships as membership}<article
                    class="chart-membership"
                  >
                    <div class="eyebrow">
                      {membership.provider} · CACHED SNAPSHOT
                    </div>
                    <strong>{membership.title}</strong>
                    <p>
                      {membership.rank != null
                        ? `Rank ${membership.rank} · `
                        : ""}{new Date(membership.fetchedAt).toLocaleString()}
                    </p>
                    <button
                      onclick={() =>
                        void run(() =>
                          call("open_external_url", {
                            url: membership.sourceUrl,
                          }),
                        )}>Open source chart</button
                    >
                  </article>{/each}{/if}
            {/if}
            <h3>Full recording sources</h3>
            <label
              >Output profile<select bind:value={outputProfile}>
                <option value="source">Preserve source audio</option>
                <option value="mp3">Source + MP3 320 kbps export</option>
              </select>
            </label>
            <button class="full" onclick={() => void findSources()}
              >Find YouTube & SoundCloud matches</button
            >
            <details>
              <summary>Add an authorized source URL</summary>
              <select aria-label="Source provider" bind:value={sourceProvider}>
                <option value="direct">Direct audio download</option>
                <option value="bandcamp">Bandcamp</option>
                <option value="youtube">YouTube</option>
                <option value="soundcloud">SoundCloud</option>
              </select>
              <label
                >URL<input bind:value={sourceUrl} placeholder="https://…" />
              </label>
              <button
                disabled={!sourceUrl.trim()}
                onclick={() => void addSource()}>Inspect source</button
              >
            </details>
            {#each candidates as c}<article class="candidate">
                <div class="eyebrow">{c.provider} · {c.confidence}</div>
                <strong>{c.title}</strong>
                <p>
                  {c.uploader ?? "Unknown uploader"} · {time(c.durationSeconds)}
                </p>
                <span class="tag">Permission: {c.permission}</span>
                {#each c.reasons as reason}<small>{reason}</small>{/each}
                {#if c.formats.length}<label
                    >Audio format<select
                      value={candidateFormats[candidateKey(c)] ?? ""}
                      onchange={(e) =>
                        (candidateFormats = {
                          ...candidateFormats,
                          [candidateKey(c)]: e.currentTarget.value,
                        })}
                    >
                      <option value=""
                        >Best available source audio (automatic)</option
                      >
                      {#each c.formats as f}<option value={f.id}
                          >{f.codec} · {f.extension}{f.bitrate
                            ? ` · ${Math.round(f.bitrate)} kbps`
                            : ""}{f.sampleRate
                            ? ` · ${f.sampleRate} Hz`
                            : ""}{f.lossless ? " · lossless" : ""}</option
                        >{/each}
                    </select></label
                  >{/if}
                <div class="detail-actions">
                  {#if ["youtube", "soundcloud"].includes(c.provider)}<button onclick={() => void previewCandidate(c)}>▶ Play audio</button>{/if}
                  <button onclick={() => void inspect(c)}>Inspect</button>
                  <button
                    disabled={c.permission === "restricted"}
                    onclick={() => void download(c)}
                    >Download selected profile</button
                  >
                </div>
              </article>{/each}
            <details>
              <summary>Edit metadata</summary>
              <label>Artist<input bind:value={selected.artist} /> </label>
              <label>Title<input bind:value={selected.title} /> </label>
              <label>Version<input bind:value={selected.version} /> </label>
              <label>Label<input bind:value={selected.label} /> </label>
              <label
                >BPM<input type="number" bind:value={selected.bpm} />
              </label>
              <label>Key<input bind:value={selected.key} /> </label>
              <button
                onclick={() =>
                  void run(async () => {
                    await call("update_track", { track: selected });
                    notice = "Metadata saved.";
                  })}>Save correction</button
              >
            </details>{:else}<div class="details-empty">
              <span>◉</span>
              <h3>Listen closer</h3>
              <p>
                Select a recording to listen and compare sources.
              </p>
            </div>{/if}
        </aside>
      </div>
    {/if}
  </main>
  {#if playlistTrack}<div class="download-backdrop">
    <dialog class="download-dialog playlist-picker-dialog" use:activateDownloadDialog aria-labelledby="playlist-picker-title" tabindex="-1">
      <header><div><div class="eyebrow">PLAYLIST</div><h2 id="playlist-picker-title">Add recording</h2><p>{playlistTrack.artist} — {playlistTrack.title}</p></div>
        <button aria-label="Close playlist picker" onclick={() => (playlistTrack = null)}>✕</button></header>
      <label>Existing playlist
        <select aria-label="Choose playlist for track" bind:value={collectionId}>
          <option value="">Choose playlist</option>
          {#each collections as collection}<option value={collection.id}>{collection.name} ({collection.trackCount})</option>{/each}
        </select>
      </label>
      <button class="primary" disabled={!collectionId} onclick={() => void addArtistTrackToPlaylist()}>Add to playlist</button>
      <label>Or create a new playlist<input aria-label="New playlist for track" bind:value={newCollection} placeholder="Playlist name" /></label>
      <footer><button onclick={() => (playlistTrack = null)}>Cancel</button><button class="primary" disabled={!newCollection.trim()} onclick={() => void createPlaylistForArtistTrack()}>Create & add</button></footer>
    </dialog>
  </div>{/if}
  {#if downloadTrack}<div class="download-backdrop">
      <dialog class="download-dialog download-sheet" use:activateDownloadDialog aria-labelledby="download-title" tabindex="-1">
        <header>
          <div><div class="eyebrow">DOWNLOAD REVIEW</div><h2 id="download-title">Download recording</h2>
            <p>{downloadTrack.artist} — {downloadTrack.title}</p></div>
          <button aria-label="Close download chooser" onclick={() => { downloadRequest++; downloadTrack = null; downloadCandidates = []; downloadCandidate = null; }}>✕</button>
        </header>
        {#if downloadLoading}<p role="status">{downloadCandidate ? "Checking quality and additional sources…" : "Finding a source…"}</p>{/if}
        {#if !downloadCandidates.length}{#if !downloadLoading}<p>No source found. Search audio to find a recording.</p>{/if}
        {:else if !downloadCandidate}<section class="download-unselected-source" aria-label="Other audio sources">
            <p>No match from the selected source. Choose another source to review it.</p>
            <div class="download-candidates">
              {#each downloadCandidates as candidate (candidateKey(candidate))}<button class="candidate-choice" aria-label={`Select ${candidate.provider} source ${candidate.title}`} onclick={() => void inspectDownloadCandidate(candidate)}>
                <span class="eyebrow">{candidate.provider} · {candidate.confidence === "high" ? "strong match" : "needs review"}</span>
                <strong>{candidate.title}</strong><small>{candidate.uploader ?? candidate.source?.username ?? "Unknown uploader"} · {time(candidate.durationSeconds)} · {candidate.permission}</small>
              </button>{/each}
            </div>
          </section>
        {:else if downloadCandidate}
          <section class="download-source-primary" aria-label="Selected audio source">
            <span class="eyebrow">{downloadCandidate.provider} source · {downloadCandidate.confidence === "high" ? "strong match" : "needs review"}</span>
            <strong>{downloadCandidate.title}</strong>
            <small>{downloadCandidate.uploader ?? "Unknown uploader"} · {time(downloadCandidate.durationSeconds)} · {downloadCandidate.permission === "restricted" ? "Download restricted" : "Audio quality: " + (downloadCandidate.formats?.[0] ? `${downloadCandidate.formats[0].codec} ${downloadCandidate.formats[0].extension}${downloadCandidate.formats[0].bitrate ? ` · ${Math.round(downloadCandidate.formats[0].bitrate)} kbps` : ""}` : "best available")}</small>
            {#if requiresDownloadReview(downloadTrack, downloadCandidate)}<small class="identity-note">This audio source is unverified. Download tags use the track details; chapter labels remain unverified.</small>{/if}
            {#each downloadCandidate.reasons as reason}<small class="match-reason">{reason}</small>{/each}
          </section>
          {#if downloadCandidates.length > 1}<details class="download-alternatives">
            <summary>Choose another source ({downloadCandidates.length - 1} alternatives)</summary>
            <div class="download-candidates">
              {#each downloadCandidates as candidate (candidateKey(candidate))}
                {#if candidateKey(candidate) !== candidateKey(downloadCandidate)}<article>
                  <button class="candidate-choice" aria-label={`Select ${candidate.provider} source ${candidate.title}`} onclick={() => void inspectDownloadCandidate(candidate)}>
                    <span class="eyebrow">{candidate.provider} · match score {candidate.score.toFixed(2)} · {candidate.confidence === "high" ? "strong match" : "needs review"}</span>
                    <strong>{candidate.title}</strong>
                    <small>{candidate.uploader ?? "Unknown uploader"} · {time(candidate.durationSeconds)} · Permission: {candidate.permission}</small>
                  </button>
                  {#each candidate.reasons as reason}<small class="match-reason">{reason}</small>{/each}
                </article>{/if}
              {/each}
            </div>
          </details>{/if}
          {#if downloadCandidate.chapters?.length}
            <details class="chapter-download-options" open={Boolean(focusedDownloadChapterId)}>
              <summary>{focusedDownloadChapterId ? "Selected chapter" : `Download chapters · ${downloadCandidate.chapters.length}`}</summary>
              {#if focusedDownloadChapterId}
                {@const focusedChapter = downloadCandidate.chapters.find((chapter) => chapter.id === focusedDownloadChapterId)}
                {#if focusedChapter}<div class="focused-chapter">
                  <div><strong>{focusedChapter.suggestedTrack ? `${focusedChapter.suggestedTrack.artist} — ${focusedChapter.suggestedTrack.title}` : focusedChapter.title}</strong><small>{time(focusedChapter.endTime - focusedChapter.startTime)} · {time(focusedChapter.startTime)}–{time(focusedChapter.endTime)}</small></div>
                  <button type="button" class="change-chapter" onclick={() => { focusedDownloadChapterId = null; splitChapters = false; selectedChapterIds = []; }}>Change chapter</button>
                </div>{/if}
              {:else}
                <div class="chapter-tools">
                  <button type="button" disabled={busy > 0} onclick={() => void run(async () => { if (!downloadCandidate) return; const enriched = await call<Candidate>("enrich_chapters", { candidate: downloadCandidate }); downloadCandidate = enriched; })}>Find chapter metadata</button>
                  <label class="bulk-chapters"><input type="checkbox" checked={splitChapters} onchange={(event) => { splitChapters = event.currentTarget.checked; selectedChapterIds = []; }} />Select multiple</label>
                </div>
                {#each downloadCandidate.chapters as chapter (chapter.id)}
                  <div class="chapter-download-row">
                    {#if splitChapters}<input class="chapter-select-checkbox" type="checkbox" aria-label={`Select ${chapter.title} for bulk download`} checked={selectedChapterIds.includes(chapter.id)} onchange={(event) => { selectedChapterIds = event.currentTarget.checked ? [...selectedChapterIds, chapter.id] : selectedChapterIds.filter((id) => id !== chapter.id); }} />{/if}
                    <button type="button" class="chapter-select" aria-label={`Select chapter ${chapter.title}`} onclick={() => focusDownloadChapter(chapter.id)}><strong>{chapter.suggestedTrack ? `${chapter.suggestedTrack.artist} — ${chapter.suggestedTrack.title}` : chapter.title}</strong><small>{time(chapter.endTime - chapter.startTime)} · {time(chapter.startTime)}–{time(chapter.endTime)}{#if chapter.suggestionConfidence} · {chapter.suggestionConfidence === "exact_unique_catalog_match" ? "Clear match" : "Needs review"}{/if}</small></button>
                    <button type="button" class="chapter-download-icon" aria-label={`Download chapter ${chapter.title}`} title="Download chapter" disabled={busy > 0 || downloadSubmitting} onclick={() => void downloadSingleChapter(chapter)}>↓</button>
                  </div>
                {/each}
                {#if splitChapters}<p class="chapter-bulk-note">Chapter labels are separate track tags. Review each selected source before it downloads.</p><button type="button" class="download-selected-chapters" disabled={!selectedChapterIds.length || downloadSubmitting || downloadCandidate.permission === "restricted"} onclick={() => void queueDownloadChoice()}>Download {selectedChapterIds.length || "selected"} chapter{selectedChapterIds.length === 1 ? "" : "s"}</button>{/if}
              {/if}
            </details>
          {/if}
          <details class="download-tags">
            <summary>Edit tags · {downloadMetadata.artist} — {downloadMetadata.title}</summary>
            <p>These fields come from the track details. Audio-source and chapter labels may be unverified.</p>
            <label>Artist<input aria-label="Download tag artist" bind:value={downloadMetadata.artist} /></label>
            <label>Title<input aria-label="Download tag title" bind:value={downloadMetadata.title} /></label>
            <label>Version or mix<input aria-label="Download tag version" bind:value={downloadMetadata.version} /></label>
            <label>Label<input aria-label="Download tag label" bind:value={downloadMetadata.label} /></label>
            <label>BPM<input aria-label="Download tag BPM" type="number" min="0" max="1000" step="0.01" inputmode="decimal" bind:value={downloadMetadata.bpm} /></label>
            {#if downloadBpmAnnotation()}<small class="estimated-bpm-note" title={downloadBpmAnnotation()}>The shown BPM is an audio analysis estimate, not verified catalog metadata.</small>{/if}
            <label>Key<input aria-label="Download tag key" bind:value={downloadMetadata.key} /></label>
            {#if downloadKeyAnnotation()}<small class="estimated-bpm-note" title={downloadKeyAnnotation()}>The shown musical key is an audio analysis estimate, not verified catalog metadata.</small>{/if}
            <button onclick={() => (metadataReviewOpen = true)}>Find metadata</button>
            {#if metadataReviewOpen}<MetadataReview track={downloadTrack} resolve={async (track, recording) => resolveRecordingMetadata(track, null, recording)} onApply={applyDownloadMetadata} onClose={() => (metadataReviewOpen = false)} />{/if}
            <details class="more-download-tags"><summary>More metadata tags</summary>
              <label>Album<input aria-label="Download tag album" bind:value={downloadMetadata.album} /></label>
              <label>Genre<input aria-label="Download tag genre" bind:value={downloadMetadata.genre} /></label>
              <label>Release date<input aria-label="Download tag release date" bind:value={downloadMetadata.releaseDate} /></label>
              <label>ISRC<input aria-label="Download tag ISRC" bind:value={downloadMetadata.isrc} /></label>
              <label>Catalog number<input aria-label="Download tag catalog number" bind:value={downloadMetadata.catalogNumber} /></label>
            </details>
          </details>
          <div class="download-catalog-source"><span>{trackSourceLabel(downloadTrack)}</span><SourceLink track={downloadTrack} /></div>
          {#if downloadCandidate.formats?.length}<details class="download-advanced"><summary>Advanced audio options</summary>
            <label>Audio quality
              <select value={candidateFormats[candidateKey(downloadCandidate)] ?? ""} onchange={(e) => candidateFormats = { ...candidateFormats, [candidateKey(downloadCandidate!)]: e.currentTarget.value } }>
                <option value="">Best available source audio</option>
                {#each downloadCandidate.formats as format}<option value={format.id}>{format.codec} · {format.extension}{format.bitrate ? ` · ${Math.round(format.bitrate)} kbps` : ""}{format.sampleRate ? ` · ${format.sampleRate} Hz` : ""}{format.lossless ? " · lossless" : ""}</option>{/each}
              </select>
            </label>
            <label>Output profile
              <select bind:value={outputProfile}><option value="source">Preserve source audio</option><option value="mp3">Source + MP3 320 kbps export</option></select>
            </label>
          </details>{:else if downloadCandidate}<button onclick={() => void inspectDownloadCandidate(downloadCandidate!)}>Check available audio quality</button>{/if}
        {/if}
        <label class="download-directory">Music folder
          <input aria-label="Download folder" bind:value={settings.musicDirectory} placeholder="Choose a folder in Settings" />
          <small>{settings.musicDirectory.trim() && settings.musicDirectory.trim() === persistedMusicDirectory.trim() ? "Saved destination" : settings.musicDirectory.trim() ? "Will be saved when you queue" : "Not configured"}</small>
        </label>
        {#if !settings.musicDirectory}<button onclick={() => { downloadTrack = null; void navigate("settings"); }}>Open Settings</button>{/if}
        <footer>
          <button onclick={() => { downloadRequest++; downloadTrack = null; downloadCandidates = []; downloadCandidate = null; }}>Cancel</button>
          <button class="primary" disabled={!downloadCandidate || downloadSubmitting || downloadCandidate.permission === "restricted" || (splitChapters && !focusedDownloadChapterId && !selectedChapterIds.length)} onclick={() => void queueDownloadChoice()}>Download</button>
        </footer>
      </dialog>
    </div>{/if}
  {#if showDownloads}<DownloadActivity onFindSource={(job) => void openDownloadChooser(job.track)} tagStatuses={downloadTagStatuses} onReviewTags={(id)=>{autotagReviewId=id;showDownloads=false;}} {jobs} onClose={() => (showDownloads = false)} onRefresh={() => void refreshJobs()} onCancel={(jobId) => void run(async () => { await call("cancel_download", {jobId}); await refreshJobs(); })} onRetry={(jobId) => void run(async () => { await call("retry_download", {jobId}); await refreshJobs(); })} onShowFile={(path) => void run(() => call("open_in_file_manager", {path}))} />{/if}
  {#if showPlayingInfo || showQueue}<section class="player-panel-host" aria-label="Player panel"><nav class="player-panel-tabs"><button class:active={showPlayingInfo} onclick={()=>openPlayerPanel("info")}>Now playing</button><button class:active={showQueue} onclick={()=>openPlayerPanel("queue")}>Queue</button><button aria-label="Close player panel" onclick={()=>openPlayerPanel(null)}>×</button></nav>
  {#if showPlayingInfo && (infoTrack || playingTrack)}{@const shownTrack = infoTrack || playingTrack!}<PlayingInfo docked={true} activeChapter={shownTrack.id===playingTrack?.id ? activeChapter?.title : undefined} panelWidth={infoPanelWidth} onPanelWidthChange={(width)=>saveLayoutWidth("info",width)} onPlay={()=>void toggleCurrentPlayback()} track={shownTrack} asset={assets.find(asset=>asset.recordingId===shownTrack.id||asset.track.id===shownTrack.id)??null} streamQuality={shownTrack.id===playingTrack?.id && preview?.kind==="audio" ? {codec:preview.audioCodec,bitrateKbps:preview.audioBitrateKbps,live:Boolean(playingRadioStation)} : undefined} onUpgrade={()=>void openDownloadChooser(shownTrack)} onRadio={() => { showPlayingInfo = false; void startSongRadio(shownTrack); }} radioBusy={radioBusy === shownTrack.id} candidate={shownTrack.id === playingTrack?.id ? playingCandidate : null} onClose={() => { showPlayingInfo = false; infoTrack = null; }} onArtist={() => { showPlayingInfo = false; void discoverArtist(shownTrack); }} onDownload={() => { showPlayingInfo = false; void openDownloadChooser(shownTrack); }} onPlaylist={() => { showPlayingInfo = false; void addArtistTrack(shownTrack); }} onMoreInfo={() => searchMoreInfo(shownTrack,shownTrack.id===playingTrack?.id?playingCandidate:null,shownTrack.id===playingTrack?.id?activeChapter?.title:undefined)} />{/if}
  {#if showQueue}<PlaybackQueuePanel {chaptersByTrack} onPlayChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)playChapter(parent,candidate,chapter);}} onQueueChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)queueChapter(parent,candidate,chapter,"append");}} onDownloadChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)void downloadChapter(parent,candidate,chapter);}} onInfoChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate){const virtual=virtualChapterTrack(parent,candidate,chapter);if(virtual)searchMoreInfo(virtual,candidate,chapter.title);}}} onQueueAllChapters={(parent,chapters)=>queueAllChapters(parent,chapters)} onInspectChapters={(parent)=>void inspectQueueChapters(parent)} tracks={queueTracks} currentIndex={queueIndex} chapters={playingCandidate?.chapters ?? []} {position} {repeatMode} {shuffleEnabled} onClose={() => openPlayerPanel(null)} onPlay={(index) => { playbackQueue.restore({ ...playbackQueue.snapshot(), currentIndex: index }); refreshQueue(); const track = queueTracks[index]; if (track) void startPreview(track, true); }} onMove={(from, to) => { playbackQueue.reorder(from, to); refreshQueue(); }} onRemove={(index) => { playbackQueue.removeAt(index); refreshQueue(); }} onSeek={seekChapter} onToggleShuffle={() => { shuffleEnabled = !shuffleEnabled; if (shuffleEnabled) playbackQueue.shuffle(); refreshQueue(); }} onCycleRepeat={() => { repeatMode = repeatMode === "off" ? "all" : repeatMode === "all" ? "one" : "off"; playbackQueue.setRepeat(repeatMode); refreshQueue(); }} />{/if}
  </section>{/if}
  {#if showReleaseSetup}<dialog open class="download-dialog release-onboarding" aria-label="Welcome to Burrata"><ReleaseSetup onboarding onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings=value)} onComplete={()=>showReleaseSetup=false}/></dialog>{/if}
  {#if renameTarget}<dialog class="download-dialog playlist-picker-dialog" use:activateRenameDialog aria-label={`Rename ${renameTarget.kind}`} tabindex="-1"><form onsubmit={(event)=>{event.preventDefault();void saveSidebarRename();}}><h2>Rename {renameTarget.kind==="radio"?"station":"playlist"}</h2><label>Name<input aria-label="New name" maxlength="160" bind:value={sidebarRenameValue}/></label><footer><button type="button" onclick={()=>renameTarget=null}>Cancel</button><button class="primary" disabled={!sidebarRenameValue.trim()}>Save</button></footer></form></dialog>{/if}
  {#if radioActions}{@const menu=radioActions}<PlaylistMenu onDelete={()=>void removeSidebarStation(menu.station)} kind="radio station" name={menu.station.name} anchor={menu.anchor} {playerHeight} onRename={()=>startSidebarRename("radio",menu.station.id,menu.station.name)} onOpen={()=>{selectedRadioStationId=menu.station.id;void navigate("radio");}} onOpenSource={()=>void openSourceToRefresh(menu.station.homepageUrl??menu.station.sourceUrl)} onClose={()=>radioActions=null}/>{/if}
  {#if playlistActions}{@const menu = playlistActions}<PlaylistMenu onRename={()=>startSidebarRename("playlist",menu.collection.id,menu.collection.name)} onDownload={collections.some(item=>item.id===menu.collection.id)?()=>void downloadPlaylist(menu.collection.id):undefined} onOpenSource={sourceUrlForCollection(menu.collection.id) ? () => void openSourceToRefresh(sourceUrlForCollection(menu.collection.id)!) : undefined} name={menu.collection.name} anchor={menu.anchor} {playerHeight} onClose={() => (playlistActions = null)} onOpen={() => void openPlaylist(menu.collection.id)} onDelete={menu.collection.id!==DOWNLOADS_PLAYLIST_ID && collections.some(item=>item.id===menu.collection.id)?()=>void deletePlaylist(menu.collection):undefined} />{/if}
  {#if actionTrack}{@const menuTrack = actionTrack}<TrackMenu track={menuTrack} onRadio={() => void startSongRadio(menuTrack)} radioBusy={radioBusy === menuTrack.id} anchor={actionAnchor} {playerHeight} onRemove={view === "collections" ? () => void removePlaylistTrack(menuTrack) : undefined} onClose={() => (actionTrack = null)} onNext={() => queueArtistTracks([menuTrack], "next")} onQueue={() => queueArtistTracks([menuTrack], "append")} onPlaylist={() => void addArtistTrack(menuTrack)} onFavorite={() => void favorite(menuTrack)} onDownload={() => void openDownloadChooser(menuTrack)} onUpgrade={()=>void openDownloadChooser(menuTrack)} onMoreInfo={() => searchMoreInfo(menuTrack,importedAudioCandidate(menuTrack))} onDetails={() => { if (view === "artist") { openPlayerPanel("info", menuTrack); return; } selected = menuTrack; showTrackDetails = true; void loadMemberships(menuTrack, ++selectionRequest); }} onSelect={() => { if (!checked.includes(menuTrack.id)) checked = [...checked, menuTrack.id]; }} />{/if}
  {#if infoSearchTarget}<TrackInfoSearch track={infoSearchTarget.track} candidate={infoSearchTarget.candidate} chapterTitle={infoSearchTarget.chapterTitle} onClose={() => (infoSearchTarget=null)} onCatalogSearch={openCatalogInfoSearch} onWebSearch={openWebInfoSearch} onOpenSource={openInfoSource} />{/if}
  <footer class="player" bind:clientHeight={playerHeight}>
    <button class="now-playing click-info" aria-label="Show playing track information" disabled={!playingTrack} onclick={() => { infoTrack = null; openPlayerPanel("info"); }}>
      <div class="mini-art">
        {#if playingTrack?.artworkUrl && !failedArtwork[playingTrack.artworkUrl]}<img
          src={playingTrack.artworkUrl}
          onerror={() => { if (playingTrack) recoverArtwork(playingTrack); }}
          alt=""
        />{:else}♫{/if}
      </div>
      <div class="player-track-copy">
        <strong class="player-track-title">{playingTitle}</strong>
        <small class="player-track-artist">{playingTrack?.playbackRange?.parentTitle ?? (activeChapter ? playingTrack?.title : playingTrack?.artist) ?? (playingTrack ? "Artist unknown" : "Select a recording")}</small>
        <small class="player-status" role="status" title={previewStatus}>
          {!playingTrack ? (previewStatus || "Press Space to play") : !preview ? (previewStatus === "Ready" ? "Ready" : previewStatus ? "Unavailable" : "Ready") : paused ? "Paused" : previewStatus.startsWith("Searching") ? "Finding audio" : previewStatus.startsWith("Loading") ? "Loading" : preview.isExcerpt ? "Playing preview" : "Playing"}
        </small>
        {#if playingTrack}<PlaybackQuality compact track={playingTrack} asset={preview?.kind==="local" ? assets.find(asset=>asset.recordingId===playingTrack?.id||asset.track.id===playingTrack?.id)??null : null} streamQuality={preview?.kind==="audio" ? {codec:preview.audioCodec,bitrateKbps:preview.audioBitrateKbps,live:Boolean(playingRadioStation)} : undefined} />{/if}
        {#if playerAudioSource}<small class="player-source" title={preview?.kind === "local" ? playingTrack?.localPath ?? playerAudioSource : playerAudioSource}>{playerAudioSource}</small>{/if}
      </div>
    </button>

    <div class="player-center">
      <div class="player-transport">
        <button class:active={shuffleEnabled} aria-label="Shuffle" aria-pressed={shuffleEnabled} onclick={() => { shuffleEnabled = !shuffleEnabled; if (shuffleEnabled) playbackQueue.shuffle(); refreshQueue(); }}>⤨</button>
        <button aria-label="Previous recording" onclick={() => { void advanceQueue(-1); }}>⏮</button>
        <button class="play-button" aria-label={paused ? "Play" : "Pause"} onclick={toggleCurrentPlayback}>{paused ? "▶" : "Ⅱ"}</button>
        <button aria-label="Next recording" onclick={() => { void advanceQueue(1); }}>⏭</button>
        <button aria-label={`Repeat: ${repeatMode}`} aria-pressed={repeatMode !== "off"} onclick={() => { repeatMode = repeatMode === "off" ? "all" : repeatMode === "all" ? "one" : "off"; playbackQueue.setRepeat(repeatMode); refreshQueue(); }}>{repeatMode === "one" ? "↻1" : repeatMode === "all" ? "↻" : "↻"}</button>
      </div>

      <div class="player-scrubber">
        <span class="time">{time(playerPosition)}</span>
        <input
          class="seek"
          type="range"
          min="0"
          max={playerDuration || 1}
          step="0.1"
          value={playerPosition}
          disabled={!preview?.canSeek || (!audio && !preview?.sessionId)}
          aria-label="Playback position"
          oninput={(e) => {
            void seekPlayer(Number(e.currentTarget.value));
          }}
        />
        <span class="time">{time(playerDuration)}</span>
      </div>
    </div>

    <div class="player-right">
      <button aria-label="Show playback queue and chapters" aria-pressed={showQueue} onclick={() => openPlayerPanel(showQueue ? null : "queue")}>☷</button>
      <label class="player-volume-control">
        <span aria-hidden="true">◖))</span>
        <input
          class="volume"
          type="range"
          min="0"
          max="1"
          step="0.01"
          bind:value={volume}
          disabled={!preview?.canVolume || (!audio && !preview?.sessionId)}
          aria-label="Volume"
          oninput={(e) => {
            const value = Number(e.currentTarget.value);
            if (preview?.sessionId) void run(() => call("candidate_preview_control", { action: "volume", value }));
            else if (audio) audio.volume = value;
          }}
        />
      </label>
      <button aria-label="Show track details" onclick={() => openPlayerPanel("info")} disabled={!playingTrack}>ⓘ</button>
    </div>
    {#if preview && ["audio", "local", "direct"].includes(preview.kind)}<audio
        bind:this={audio}
        src={playable(preview.url)}
        ontimeupdate={() => updatePlaybackPosition(audio?.currentTime ?? 0)}
        onseeked={() => { position = audio?.currentTime ?? 0; seekSerial++; void publishAutomationPlayback(); }}
        onloadedmetadata={() => (duration = audio?.duration ?? 0)}
        onplay={() => (paused = false)}
        onpause={() => { paused = true; persistPlaybackPosition(); }}
        onplaying={() => { paused = false; void recordSuccessfulPlayback(); }}
        onended={() => void advanceQueue(1, true)}
        onerror={recoverAudioFailure}
      >
      </audio>{:else if preview?.kind === "external"}<div class="embed-player">
        <p class="external-notice">
          In-app preview unavailable for this recording.
        </p>
        <button
          onclick={() =>
            void run(() => call("open_external_url", { url: preview!.url }))}
          >Open provider page</button
        >
        <button
          onclick={() => {
            previewRequests.next();
            preview = null;
            paused = true;
          }}>Close</button
        >
      </div>{:else if preview?.sessionId}<div class="embed-player">
        <p class="external-notice">Playing in the provider preview window. Use its Play button if playback is blocked.</p>
        <button onclick={() => void closePreview()}>Close player</button>
      </div>{:else if preview}<div class="embed-player">
        <p class="external-notice">This preview source is unavailable in this player.</p>
        <button onclick={() => void closePreview()}>Close player</button>
      </div>{/if}
  </footer>
</div>
