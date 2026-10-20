// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserImportResult, Candidate, Chart, Collection, DownloadJob, LocalAsset, NativePlaybackSnapshot, Preview, Settings, Track } from "./lib/types";
import type { ArtistReleaseRef } from "./lib/artist-types";
import type { TrackSortAvailability } from "./lib/track-sort-availability";
import { WebviewStallWatchdog } from "./webview-stall-watchdog";
import { WebviewSeekCompletionWatchdog } from "./webview-seek-completion-watchdog";

const mocks = vi.hoisted(() => ({ call: vi.fn(), listen: vi.fn(), captureRadioPcm: vi.fn(), isDesktop: true }));
vi.mock("./lib/webview-radio-pcm", () => ({ captureWebviewRadioWavBase64: mocks.captureRadioPcm }));
vi.mock("./lib/api", () => ({
  call: mocks.call,
  listen: mocks.listen,
  desktop: () => mocks.isDesktop,
  playable: (url: string) => url,
}));
vi.mock("svelte", async () => {
  // @ts-expect-error Match the browser runtime used by the direct import above.
  return await import("../node_modules/svelte/src/index-client.js");
});

import App from "./App.svelte";

const savedTrack: Track = {
  id: "recording-1", provider: "beatport", providerId: "1",
  sourceUrl: "https://example.test/track/1", artist: "Visible Artist",
  title: "Saved Recording", favorite: false, provenance: {},
};
const latestImport: BrowserImportResult = {
  id: "snapshot-new", provider: "beatport", kind: "chart",
  title: "Latest Saved Import", sourceUrl: "https://example.test/chart",
  fetchedAt: "2026-10-12T00:00:00Z", collectionId: "collection-full",
  capturedCount: 1, importedCount: 1, expectedTotal: 1, complete: true, warnings: [],
};
const oldImport = { ...latestImport, id: "snapshot-old", title: "Older Import" };
const chart: Chart = {
  id: latestImport.id, provider: "beatport", title: latestImport.title,
  sourceUrl: latestImport.sourceUrl, fetchedAt: latestImport.fetchedAt,
  tracks: [savedTrack], stale: false,
};
const collections: Collection[] = [
  { id: "collection-empty", name: "Empty Collection", trackCount: 0 },
  { id: "collection-full", name: "Saved Collection", trackCount: 131 },
];
let mutableFavoritesCollection: Collection = { id: "favorites", name: "Liked Songs", trackCount: 0 };
const collectionTrack: Track = {
  ...savedTrack, id: "collection-recording", title: "Existing Collection Member",
};
const secondTrack: Track = {
  ...savedTrack, id: "recording-2", artist: "Another Artist", title: "Another Recording",
};
function makeNativeSnapshot(overrides: Partial<NativePlaybackSnapshot> = {}): NativePlaybackSnapshot {
  return {
    engine:"native", revision:1, generation:1, status:"idle", trackId:null, track:null, candidate:null,
    queue:[], currentIndex:-1, paused:true, position:0, duration:0, volume:.8, canSeek:false, canVolume:true,
    shuffle:false, repeat:"off", playerMessage:"", quality:{}, chapter:null, radioNowPlaying:null, radioHistory:[],
    ...overrides,
  };
}
const eventHandlers = new Map<string, (event: { event: string; id: number; payload: unknown }) => void>();
let failNextCollectionLoad = false;
let unmountApp: (() => void) | undefined;
let holdStartupCalls = false;
let delayedBrowserImports: Promise<BrowserImportResult[]> | undefined;
let directBrowserImport: BrowserImportResult | null = null;
let directBrowserImportFailure = "";
let previewMode: "idle" | "fallback" | "race" | "direct" | "alternates" = "idle";
let directPreviewExcerpt = true;
let playbackResolverFailure = "";
let playbackResolverCandidate: Candidate | null | undefined;
let playbackResolverCandidates: (Candidate | Error)[] = [];
let delayPlaybackTrackId: string | undefined;
let finishPlaybackResolve: ((value: { preview: Preview; candidate: Candidate | null }) => void) | undefined;
let afterPlaybackResolver: (() => void) | undefined;
let afterPreviewResolve: (() => void) | undefined;
let finishOldSearch: ((value: Candidate[]) => void) | undefined;
let finishAudioResolve: ((value: Preview) => void) | undefined;
let delayAudioCandidateId: string | undefined;
let candidateAudioFailure = "";
let finishCandidateInspect: ((value: Candidate) => void) | undefined;
let delayCandidateInspectId: string | undefined;
let delayedCandidateProviders = new Map<string, (value: Candidate[]) => void>();
let failedCandidateProviders = new Set<string>();
let deferNativePause = false;
let finishNativePause: (() => void) | undefined;
let deferDependencyCheck = false;
let finishDependencyCheck: (() => void) | undefined;
let songRadioTracks: Track[] = [];
let songRadioFailure = "";
let holdSongRadio = false;
let finishSongRadio: ((value: { tracks: Track[]; total: number }) => void) | undefined;
let batchInspectionHigh = false;
let artistProfileTopTracks: Track[] = [];
let unsupportedArtistCommands = false;
let artistCommandFailure: string | null = null;
let artistProfileLinks: { label: string; url: string; provider?: string | null }[] = [];
let savedChartTracks: Track[] = [savedTrack];
let automationTrackLookup: Track[] | null = null;
let automationPlaybackAck: {protocolVersion:number;accepted:boolean;needFull:boolean} | undefined;
let audioSearchResult: Track[] = [];
let partialSoulseekSearch = false;
let discoveryAlbum: ArtistReleaseRef | null = null;
let discoveryAlbumTracks: Track[] = [];
let configuredMusicDirectory = "";
let configuredPreferredDownloadSource = "youtube";
let configuredPreferredStreamingSource = "youtube";
let configuredPauseOnSourceOpen = false;
let configuredSourceOpenAtPosition = false;
let identifyPlayingTrackResult: { trackId: string; title?: string | null; artist?: string | null; note?: string } = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist" };
let holdNativeIdentify = false;
let finishNativeIdentify: ((value: typeof identifyPlayingTrackResult) => void) | undefined;
let webviewIdentifyResult: { trackId: string; title?: string | null; artist?: string | null; artworkUrl?: string | null; note?: string } = { trackId: savedTrack.id, title: "Identified Recording", artist: "Identified Artist" };
let holdWebviewIdentify = false;
let finishWebviewIdentify: ((value: typeof webviewIdentifyResult) => void) | undefined;
let holdRadioIdentify = false;
let finishRadioIdentify: ((value: { nowPlaying: typeof radioNowPlayingState; note?: string }) => void) | undefined;
let webviewSourceGeneration = 1;
let overridePlaylistTrackUrls = false;
let monochromeCandidates: Candidate[] = [];
let streamingFallbackEnabled = true;
let streamingFallbackSources = ["youtube", "soundcloud"];
let downloadFallbackEnabled = false;
let downloadFallbackSources = ["monochrome", "slskd", "youtube", "soundcloud"];
let slskdConnectionEnabled = false;
let discogsConnectionEnabled = false;
let playlistRemoved = false;
let mockCollectionRows: Track[] | null = null;
let deletedCollectionIds = new Set<string>();
let deletedCollectionRecords: Collection[] = [];
let savedAssets: LocalAsset[] = [];
let directoryBrowserRoots: { id: string; name: string; path: string }[] = [];
let directoryBrowserPage: Record<string, unknown> | null = null;
let directoryBrowserNestedPage: Record<string, unknown> | null = null;
let trackFilesByRequestedId = new Map<string, LocalAsset[]>();
let unsupportedScopedAssetCommands = false;
let preferredAssetChoice = "";
let analysisStatusResult: { assetId: string; inputHash?: string; version?: number; status: string; payload?: Record<string, unknown> | null }[] = [];
let savedSnapshots: Chart[] = [];
let historyEntries: { id: string; track: Track; provider: string; sessionId: string; playedAt: string; chapter?: unknown }[] = [];
let deferHistoryRestorePage = false;
let finishHistoryRestorePage: ((page: { items: typeof historyEntries; total: number; hasMore: boolean }) => void) | undefined;
let radioHistoryEnabled = true;
let favoriteTrackIds = new Set<string>();
let playlistPreferences = { favoritesPlaylistId: "favorites", shortcuts: Array<string | null>(9).fill(null), shortcutBindings: Array.from({ length: 9 }, (_, index) => `Alt+${index + 1}`) };
let chapterResults: Candidate["chapters"] = null;
let downloadJobs: DownloadJob[] = [];
let sidebarLayoutState: { sections: { id: string; name: string }[]; assignments: Record<string, string>; names?: Record<string, string> } = { sections: [], assignments: {} };
let sidebarLayoutSaves: typeof sidebarLayoutState[] = [];
type MockRadioStation = { codec?: string; id: string; name: string; streamUrl: string; sourceUrl: string; provider?: string; description?: string; homepageUrl?: string; faviconUrl?: string; isCustom?: boolean };
let radioStationState: MockRadioStation[] = [];
let radioNowPlayingState: { stationId: string; artist: string; title: string; album?: string | null; year?: string | null; sourceUrl: string; metadataConfidence: string; observedAt?: string | null; artworkUrl?: string | null } | null = null;
let remoteBindingState: {bindingId: string; provider: string; collectionId: string; sourceCollectionId: string; url: string; canEdit: boolean; ownerName?: string; ownerId?: string}[] = [];
let mpvStatusAvailable = false;
let nativePlaybackSnapshot: NativePlaybackSnapshot | null = null;
let playlistWatcherHealthy = true;
let periodicPlaylistSyncCallbacks: (() => void)[] = [];

function youtubeCandidate(track: Track): Candidate {
  return {
    id: `youtube:${track.id}`, provider: "youtube",
    url: `https://youtube.com/watch?v=${track.id.padEnd(11, "x").slice(0, 11)}`,
    title: `${track.artist} - ${track.title} (Official Audio)`, uploader: track.artist,
    durationSeconds: 240, artworkUrl: null, formats: [], permission: "unknown",
    confidence: "review", score: 0.82, reasons: [], official: true,
  };
}
function soulseekCandidate(track: Track): Candidate {
  return {
    id: `peer-hash-${track.id}`, provider: "slskd", url: "", title: `${track.artist} - ${track.title}.flac`,
    uploader: "peer-user", durationSeconds: 240, artworkUrl: null, formats: [], permission: "unknown",
    confidence: "low", score: 0, reasons: [], official: false,
    source: { kind: "soulseek", username: "peer-user", filename: `${track.artist} - ${track.title}.flac`, size_bytes: 28_000_000, bitrate_kbps: 933 },
  };
}
function withChapters(candidate: Candidate): Candidate {
  return { ...candidate, chapters: chapterResults };
}

async function settle() {
  for (let i = 0; i < 12; i++) await Promise.resolve();
  if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(0);
  else await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
}
function lastCallOf(command: string) { const calls = mocks.call.mock.calls.filter(([name]) => name === command); return calls[calls.length - 1]; }
function collectionCard(name = "Saved Collection"): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>(".collection-card")]
    .find((item) => item.textContent?.includes(name))!;
}
function sourceGroup(provider: string): HTMLDetailsElement {
  const summary = document.querySelector<HTMLElement>(`.source-heading.${provider}`)!;
  return summary.parentElement as HTMLDetailsElement;
}
function importedPlaylistCard(title = latestImport.title): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>(".imported-card")]
    .find((item) => item.textContent?.includes(title))!;
}
function rightClickPlaylist(name: string) {
  const playlist = collectionCard(name);
  playlist.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 340, clientY: 200 }));
}
function searchButton(): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>('.search-form button[type="submit"]')!;
}
function submitSearch() {
  document.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
}
async function selectSearchSources(sourceNames: string[]) {
  const mode = sourceNames.some((name) => name.toLocaleLowerCase() === "library" || name.toLocaleLowerCase() === "my library") ? "library" : "online";
  const modeButton = document.querySelector<HTMLButtonElement>(`[aria-label="${mode === "library" ? "My library" : "Online"}"]`);
  if (modeButton?.getAttribute("aria-pressed") !== "true") modeButton?.click();
  if (mode === "online") {
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    const requested = sourceNames[0]?.toLocaleLowerCase();
    const value = sourceNames.length !== 1 || !requested || requested === "all online sources" || requested === "all" ? "all" : ({
      "spotify": "spotify", "musicbrainz": "musicbrainz", "discogs": "discogs", "youtube": "youtube", "soundcloud": "soundcloud", "soulseek": "soulseek",
    } as Record<string, string>)[requested] ?? "all";
    if (picker.value !== value) {
      picker.value = value;
      picker.dispatchEvent(new Event("change", { bubbles: true }));
    }
  }
  await settle();
}
async function searchPlaylistOverview(queryText: string, provider: string) {
  await selectSearchSources([provider]);
  const playlistKind = [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Playlists");
  if (!playlistKind) throw new Error("Could not find playlist result filter");
  playlistKind.click(); await settle();
  const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
  query.value = queryText; query.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  submitSearch(); await settle();
}
async function searchDiscovery(queryText: string, sources: string[]) {
  await selectSearchSources(sources);
  const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
  query.value = queryText;
  query.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  submitSearch();
  await settle();
}
async function openLibraryImport() {
  const options = document.querySelector<HTMLDetailsElement>(".library-tools")!;
  if (!options.open) options.querySelector("summary")!.click();
  await settle();
  [...options.querySelectorAll<HTMLButtonElement>("button")]
    .find(button => button.textContent?.trim() === "Import charts or playlists")!.click();
  await settle();
}
async function refreshSidebarBrowserImports() {
  await openLibraryImport();
  [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find((button) => button.textContent?.trim() === "Close")!.click();
  await settle();
}
async function saveSidebarRename(name: string) {
  const input = document.querySelector<HTMLInputElement>('[aria-label="New name"]')!;
  input.value = name;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  document.querySelector<HTMLFormElement>('dialog[aria-label^="Rename "] form')!
    .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await settle();
}
async function trackActions(trackTitle = "Saved Recording"): Promise<HTMLDialogElement> {
  const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes(trackTitle));
  const button = row?.querySelector<HTMLButtonElement>(".more-trigger");
  if (!button) throw new Error(`Could not find actions for ${trackTitle}`);
  button.click();
  await settle();
  const menu = document.querySelector<HTMLDialogElement>("dialog.track-menu");
  if (!menu) throw new Error(`Track actions menu did not open for ${trackTitle}`);
  return menu;
}
function menuActionText(button: HTMLElement): string {
  const visibleLabel = button.cloneNode(true) as HTMLElement;
  visibleLabel.querySelectorAll('[aria-hidden="true"]').forEach((icon) => icon.remove());
  return visibleLabel.textContent?.replace(/\s+/g, " ").trim() ?? "";
}
async function chooseTrackAction(action: string, trackTitle = "Saved Recording") {
  const menu = await trackActions(trackTitle);
  const button = [...menu.querySelectorAll<HTMLButtonElement>("button")].find((item) => menuActionText(item) === action);
  if (!button) throw new Error(`Could not find '${action}' for ${trackTitle}`);
  button.click();
  await settle();
}
async function selectDownloadSource(match?: string) {
  const dialog = document.querySelector<HTMLDialogElement>("dialog.download-dialog");
  if (!dialog) throw new Error("Download chooser did not open");
  const rows = [...dialog.querySelectorAll<HTMLButtonElement>(".candidate")];
  const needle = match?.toLocaleLowerCase();
  const row = (needle ? rows.find((item) => `${item.textContent ?? ""} ${item.closest(".provider-group")?.getAttribute("aria-label") ?? ""}`.toLocaleLowerCase().includes(needle)) : undefined)
    ?? rows.find((item) => item.getAttribute("aria-pressed") === "true")
    ?? rows[0];
  if (!row) throw new Error("Download chooser has no source to select");
  row.click();
  await settle();
}
async function confirmReviewedDownload(sourceMatch?: string) {
  await selectDownloadSource(sourceMatch);
  const dialog = document.querySelector<HTMLDialogElement>("dialog.download-dialog")!;
  dialog.querySelector<HTMLButtonElement>("footer .primary")!.click();
  await settle();
}
async function automationAction(requestId: string, action: string, fields: Record<string, unknown> = {}) {
  const handler = eventHandlers.get("automation-playback");
  if (!handler) throw new Error("Automation playback event handler is not registered");
  handler({ event: "automation-playback", id: Date.now(), payload: { requestId, action, ...fields } });
  await settle();
  const completion = mocks.call.mock.calls.find(([command, args]) => command === "automation_playback_complete" && (args as { requestId?: string }).requestId === requestId);
  if (!completion) throw new Error(`Automation request '${requestId}' was not acknowledged`);
  return completion[1] as { error: string | null; playback: Record<string, unknown> };
}
async function startWebviewIdentify(track = savedTrack) {
  document.querySelector<HTMLButtonElement>(`[aria-label="Play ${track.title}"]`)!.click();
  await settle();
  document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
  await settle();
  const identify = [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")]
    .find((button) => button.textContent?.trim() === "Identify song");
  if (!identify) throw new Error("Identify action is not available for the current WebView playback");
  return identify;
}

beforeEach(() => {
  document.body.innerHTML = "";
  const localStorageValues = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => localStorageValues.get(key) ?? null,
    setItem: (key: string, value: string) => localStorageValues.set(key, value),
    removeItem: (key: string) => localStorageValues.delete(key),
  });
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  Object.defineProperty(HTMLDialogElement.prototype, "show", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value: function(this: HTMLDialogElement) { this.removeAttribute("open"); this.dispatchEvent(new Event("close")); } });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    callback(0);
    return 1;
  });
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
  const setIntervalReal = globalThis.setInterval.bind(globalThis);
  vi.spyOn(globalThis, "setInterval").mockImplementation(((callback: TimerHandler, timeout?: number, ...args: unknown[]) => {
    if (timeout === 60000) {
      periodicPlaylistSyncCallbacks.push(() => { if (typeof callback === "function") callback(); });
      return 0 as ReturnType<typeof setInterval>;
    }
    return setIntervalReal(callback, timeout, ...args);
  }) as typeof setInterval);
  mocks.listen.mockResolvedValue(() => {});
  mocks.captureRadioPcm.mockImplementation(async (_audio: HTMLAudioElement, _current: () => boolean, _signal: AbortSignal, progress: (seconds: number) => void, options?: { onSample: (wav: string, seconds: number) => Promise<boolean> }) => {
    const wav = "captured-wav-base64";
    progress(options ? 3 : 8);
    if (options?.onSample) await options.onSample(wav, 3);
    return wav;
  });
  mocks.isDesktop = true;
  eventHandlers.clear();
  failNextCollectionLoad = false;
  holdStartupCalls = false;
  delayedBrowserImports = undefined;
  directBrowserImport = null;
  directBrowserImportFailure = "";
  previewMode = "idle";
  directPreviewExcerpt = true;
  playbackResolverFailure = "";
  playbackResolverCandidate = undefined;
  playbackResolverCandidates = [];
  delayPlaybackTrackId = undefined;
  finishPlaybackResolve = undefined;
  afterPlaybackResolver = undefined;
  afterPreviewResolve = undefined;
  finishOldSearch = undefined;
  finishAudioResolve = undefined;
  delayAudioCandidateId = undefined;
  candidateAudioFailure = "";
  finishCandidateInspect = undefined;
  delayCandidateInspectId = undefined;
  delayedCandidateProviders = new Map();
  failedCandidateProviders = new Set();
  deferNativePause = false;
  finishNativePause = undefined;
  deferDependencyCheck = false;
  finishDependencyCheck = undefined;
  songRadioTracks = [];
  songRadioFailure = "";
  holdSongRadio = false;
  finishSongRadio = undefined;
  batchInspectionHigh = false;
  artistProfileTopTracks = [];
  unsupportedArtistCommands = false;
  artistCommandFailure = null;
  artistProfileLinks = [];
  savedChartTracks = [savedTrack];
  automationTrackLookup = null;
  automationPlaybackAck = undefined;
  audioSearchResult = [];
  partialSoulseekSearch = false;
  discoveryAlbum = null;
  discoveryAlbumTracks = [];
  configuredMusicDirectory = "";
  configuredPreferredDownloadSource = "youtube";
  configuredPreferredStreamingSource = "youtube";
  configuredPauseOnSourceOpen = false;
  configuredSourceOpenAtPosition = false;
  identifyPlayingTrackResult = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist" };
  holdNativeIdentify = false;
  finishNativeIdentify = undefined;
  webviewIdentifyResult = { trackId: savedTrack.id, title: "Identified Recording", artist: "Identified Artist" };
  holdWebviewIdentify = false;
  finishWebviewIdentify = undefined;
  holdRadioIdentify = false;
  finishRadioIdentify = undefined;
  webviewSourceGeneration = 1;
  overridePlaylistTrackUrls = false;
  monochromeCandidates = [];
  streamingFallbackEnabled = true;
  streamingFallbackSources = ["youtube", "soundcloud"];
  downloadFallbackEnabled = false;
  downloadFallbackSources = ["monochrome", "slskd", "youtube", "soundcloud"];
  slskdConnectionEnabled = false;
  discogsConnectionEnabled = false;
  playlistRemoved = false;
  mockCollectionRows = null;
  deletedCollectionIds = new Set();
  deletedCollectionRecords = [];
  savedAssets = [];
  directoryBrowserRoots = [{ id: "folder-root", name: "Music", path: "/music" }];
  directoryBrowserPage = {
    rootId: "folder-root", path: "/music", relativePath: "", breadcrumbs: [{ name: "Music", relativePath: "" }],
    directories: [{ name: "Jazz", relativePath: "Jazz" }], files: [], total: 0, offset: 0, limit: 200,
  };
  directoryBrowserNestedPage = null;
  trackFilesByRequestedId = new Map();
  unsupportedScopedAssetCommands = false;
  preferredAssetChoice = "";
  analysisStatusResult = [];
  savedSnapshots = [];
  historyEntries = [];
  deferHistoryRestorePage = false;
  finishHistoryRestorePage = undefined;
  radioHistoryEnabled = true;
  favoriteTrackIds = new Set<string>();
  playlistPreferences = { favoritesPlaylistId: "favorites", shortcuts: Array<string | null>(9).fill(null), shortcutBindings: Array.from({ length: 9 }, (_, index) => `Alt+${index + 1}`) };
  chapterResults = null;
  downloadJobs = [];
  sidebarLayoutState = { sections: [], assignments: {} };
  sidebarLayoutSaves = [];
  radioStationState = [];
  mutableFavoritesCollection = { id: "favorites", name: "Liked Songs", trackCount: 0 };
  radioNowPlayingState = null;
  remoteBindingState = [];
  mpvStatusAvailable = false;
  nativePlaybackSnapshot = null;
  playlistWatcherHealthy = true;
  periodicPlaylistSyncCallbacks = [];
  mocks.listen.mockImplementation(async (event: string, handler: (event: { event: string; id: number; payload: unknown }) => void) => {
    eventHandlers.set(event, handler);
    return () => {};
  });
  const localMockRows = (args?: Record<string, unknown>) => {
    let rows = args?.view === "collections" ? (playlistRemoved ? [] : [...(mockCollectionRows ?? [collectionTrack])]) : args?.view === "favorites" ? savedChartTracks.filter(track=>favoriteTrackIds.has(track.id)).map(track=>({...track,favorite:true})) : [...savedChartTracks];
    const query=String(args?.query??"").trim().toLocaleLowerCase();
    if(query)rows=rows.filter(track=>`${track.title} ${track.artist} ${track.album??""} ${track.version??""} ${track.genre??""}`.toLocaleLowerCase().includes(query));
    if(args?.availability && args.availability!=="all")rows=rows.filter(track=>{const local=track.localAvailability?track.localAvailability==="available":Boolean(track.localPath);return args.availability==="local"?local:!local;});
    const field=String(args?.sortField??"sourceOrder"), direction=args?.direction==="desc"?-1:1;
    if(field==="title")rows.sort((a,b)=>a.title.localeCompare(b.title)*direction);
    else if(field==="artist")rows.sort((a,b)=>a.artist.localeCompare(b.artist)*direction);
    else if(field==="sourceOrder" && args?.direction==="desc")rows.reverse();
    return rows;
  };
  const filesForRequestedTrack = (trackId: string) => trackFilesByRequestedId.get(trackId)
    ?? savedAssets.filter(asset => asset.recordingId === trackId || asset.track.id === trackId);
  mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "public_spotify_duration_repair_running": return false;
      case "playlist_watcher_running": return playlistWatcherHealthy;
      case "get_playback_snapshot": return nativePlaybackSnapshot;
      case "check_dependencies": if (deferDependencyCheck) return new Promise<void>((resolve) => { finishDependencyCheck = () => resolve(); }); return [];
      case "playback_command": {
        if (deferNativePause && (args?.command as {action?:string})?.action === "pause") return new Promise<void>((resolve) => { finishNativePause = resolve; });
        return undefined;
      }
      case "get_connections": return { settings: { slskd: { enabled: slskdConnectionEnabled }, discogsEnabled: discogsConnectionEnabled }, slskdCredential: { present: slskdConnectionEnabled, sessionOnly: false }, discogsCredential: { present: discogsConnectionEnabled, sessionOnly: false }, lastfmCredential: { present: false, sessionOnly: false } };
      case "get_automation_status": return { permissions: { control: true, manage: false }, playback: { status: "idle", queue: [] } };
      case "automation_playback_state": return automationPlaybackAck;
      case "automation_layout_complete": return undefined;
      case "automation_playback_complete": return automationPlaybackAck ?? { acknowledged: true };
      case "search_artists": return { query: args?.query, page: 0, pageSize: 20, hasMore: false, results: [{ artist: { provider: "musicbrainz", id: "artist-1" }, name: "Visible Artist", sourceUrl: "https://musicbrainz.org/artist/artist-1" }], warnings: [] };
      case "get_artist_profile": {
        const artist = args?.artist as { provider: string; id: string };
        return { artist: { artist, name: "Visible Artist", sourceUrl: "https://musicbrainz.org/artist/artist-1" }, genres: [], tags: [], sourceTags: [], aliases: [], links: artistProfileLinks, topTracks: artistProfileTopTracks, releases: { artist, items: [], warnings: [] }, discogsAvailable: false, lastfmAvailable: false, sourceCoverage: [], warnings: [] };
      }
      case "list_artist_local_assets_page": {
        if (unsupportedArtistCommands) throw new Error("unknown Tauri command list_artist_local_assets_page");
        if (artistCommandFailure) throw new Error(artistCommandFailure);
        return { items: savedAssets, total: savedAssets.length, limit: args?.limit ?? 200, offset: args?.offset ?? 0, sortAvailability: { title: true, sourceOrder: true } };
      }
      case "match_artist_playback_sources": {
        if (unsupportedArtistCommands) throw new Error("unknown Tauri command match_artist_playback_sources");
        if (artistCommandFailure) throw new Error(artistCommandFailure);
        return ((args?.sourceTracks ?? []) as Track[]).map(track => {
          const matches = savedAssets.filter(asset => asset.track.localAvailability === "available"
            && asset.track.artist.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase() === track.artist.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase()
            && asset.track.title.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase() === track.title.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase()
            && (asset.track.version ?? "").normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase() === (track.version ?? "").normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase());
          return matches.length === 1 ? matches[0] : null;
        });
      }
      case "get_artist_tracks": return { artist: args?.artist, items: [], source: "MusicBrainz", warnings: [] };
      case "get_artist_releases": return { artist: args?.artist, items: [], warnings: [] };
      case "get_release_tracks": return discoveryAlbumTracks;
      case "get_artist_collaborations": return { artist: args?.artist, items: [], warnings: [] };
      case "resolve_recording_metadata": return {
        sourceTrackId: savedTrack.id, recordingCandidates: [],
        fields: [{ key: "label", value: "Reviewed Label", source: "MusicBrainz", sourceUrl: "https://musicbrainz.org/release/1", reviewRequired: true }],
        credits: [], warnings: [],
      };
      case "get_settings":
        if (holdStartupCalls) return new Promise(() => {});
        return { musicDirectory: configuredMusicDirectory, preferredDownloadSource: configuredPreferredDownloadSource, preferredStreamingSource: configuredPreferredStreamingSource, overridePlaylistTrackUrls, streamingFallbackEnabled, streamingFallbackSources, downloadFallbackEnabled, downloadFallbackSources, pauseOnSourceOpen: configuredPauseOnSourceOpen, sourceOpenAtPosition: configuredSourceOpenAtPosition, autoPreview: false, previewDelayMs: 250, theme: "dark", ytDlpPath: "yt-dlp", ffmpegPath: "ffmpeg", ffprobePath: "ffprobe", jsRuntimePath: "deno" };
      case "get_sidebar_layout": return sidebarLayoutState;
      case "save_sidebar_layout": {
        sidebarLayoutState = args?.layout as typeof sidebarLayoutState;
        sidebarLayoutSaves.push(sidebarLayoutState);
        return sidebarLayoutState;
      }
      case "get_radio_stations": return radioStationState;
      case "resolve_radio_station_playback_url": return radioStationState.find(station=>station.id===args?.stationId)?.streamUrl;
      case "resolve_radio_station_artwork": return null;
      case "get_mpv_status": return { available: mpvStatusAvailable };
      case "open_in_mpv": return undefined;
      case "identify_webview_playing_pcm": return args?.stationId
        ? holdRadioIdentify ? new Promise((resolve) => { finishRadioIdentify = resolve; }) : { nowPlaying: radioNowPlayingState, note: "No song identified." }
        : holdWebviewIdentify ? new Promise((resolve) => { finishWebviewIdentify = resolve; }) : webviewIdentifyResult;
      case "identify_webview_playing_output": return webviewIdentifyResult;
      case "identify_radio_station": return holdRadioIdentify
        ? new Promise((resolve) => { finishRadioIdentify = resolve; })
        : { nowPlaying: radioNowPlayingState, note: "No song identified." };
      case "identify_playing_track": return holdNativeIdentify ? new Promise((resolve) => { finishNativeIdentify = resolve; }) : identifyPlayingTrackResult;
      case "get_radio_history_setting": return radioHistoryEnabled;
      case "set_radio_history_setting": radioHistoryEnabled = Boolean(args?.enabled); return undefined;
      case "get_radio_song_track": {
        const metadata = args?.metadata as { artist?: string; title?: string; album?: string | null; stationId?: string; station_id?: string; sourceUrl?: string; artworkUrl?: string | null };
        const stationId = String(args?.stationId ?? metadata.stationId ?? metadata.station_id ?? "station");
        return { ...savedTrack, id: `radio_song:${stationId}:${metadata.title ?? "song"}`, provider: "radio_song", artist: metadata.artist ?? "Unknown artist", title: metadata.title ?? "Unknown title", album: metadata.album ?? undefined, artworkUrl: metadata.artworkUrl ?? undefined, sourceUrl: "", provenance: { radioStationId: stationId, radioStationName: radioStationState.find((station) => station.id === stationId)?.name ?? "Example station", radioMetadata: metadata } };
      }
      case "record_radio_history": return true;
      case "get_station_details": return { stationId: args?.stationId, history: [], historyAvailable: false };
      case "poll_radio_station": return { stationId: args?.stationId, nowPlaying: radioNowPlayingState, history: [], historyAvailable: false, note: "" };
      case "remove_radio_station": {
        radioStationState=radioStationState.filter(station=>station.id!==args?.stationId);
        return radioStationState;
      }
      case "rename_radio_station": {
        radioStationState = radioStationState.map((station) => station.id === args?.stationId ? { ...station, name: String(args?.name) } : station);
        return radioStationState;
      }
      case "get_song_radio":
        if (songRadioFailure) throw new Error(songRadioFailure);
        if (holdSongRadio) return new Promise((resolve) => { finishSongRadio = resolve; });
        return { tracks: songRadioTracks, total: songRadioTracks.length };
      case "get_diagnostics":
        if (holdStartupCalls) return new Promise(() => {});
        return [];
      case "list_remote_playlist_bindings": return remoteBindingState;
      case "list_collections": return [...collections, ...remoteBindingState.map(binding=>({id:binding.collectionId,name:"Linked Spotify (remote copy)",trackCount:1})), mutableFavoritesCollection].filter((collection) => !deletedCollectionIds.has(collection.id));
      case "list_deleted_collections": return deletedCollectionRecords;
      case "sync_playlists": return { scanned: 0, synchronized: 0, warnings: [], mirrorsDirectory: null, sources: [] };
      case "scan_local_playlists": return { files: [], roots: [], warnings: [] };
      case "list_genres": return [{ id: "deep-house", name: "Deep House", chartUrl: "https://example.test/chart" }];
      case "get_browser_status": return { connected: false, pendingCount: 0, lastSeen: null, hostName: "test" };
      case "list_browser_imports": return delayedBrowserImports ?? (directBrowserImport ? [directBrowserImport, latestImport, oldImport] : [latestImport, oldImport]);
      case "import_collection_url":
        if (directBrowserImportFailure) throw new Error(directBrowserImportFailure);
        if (directBrowserImport) return directBrowserImport;
        throw new Error("Direct source import unavailable");
      case "request_browser_import": return "browser-request-direct";
      case "load_browser_import": {
        const imported = [directBrowserImport, latestImport, oldImport].find((item) => item?.id === args?.id) ?? latestImport;
        return { ...chart, id: imported.id, title: imported.title, tracks: savedChartTracks };
      }
      case "list_snapshots": return savedSnapshots;
      case "load_snapshot": return savedSnapshots.find((snapshot) => snapshot.id === args?.id) ?? chart;
      case "search_playback_history": {
        const options = (args?.options ?? {}) as { query?: string; provider?: string | null; stationId?: string | null; from?: string | null; to?: string | null; sort?: string; limit?: number; offset?: number };
        let matching = [...historyEntries];
        if (options.query) {
          const query = options.query.toLocaleLowerCase();
          matching = matching.filter((item) => `${item.track.title} ${item.track.artist} ${item.track.album ?? ""} ${(item.track.provenance as Record<string,unknown> | null)?.radioStationName ?? ""}`.toLocaleLowerCase().includes(query));
        }
        if (options.provider) matching = matching.filter((item) => options.provider === "non-radio" ? item.provider !== "radio" : item.provider.toLocaleLowerCase() === options.provider!.toLocaleLowerCase());
        if (options.stationId) matching = matching.filter((item) => (item.track.provenance as Record<string, unknown> | null)?.radioStationId === options.stationId);
        if (options.from) matching = matching.filter((item) => item.playedAt >= options.from!);
        if (options.to) matching = matching.filter((item) => item.playedAt.slice(0, 10) <= options.to!.slice(0, 10));
        matching.sort((a, b) => options.sort === "oldest" ? a.playedAt.localeCompare(b.playedAt) : b.playedAt.localeCompare(a.playedAt));
        const total = matching.length;
        const offset = options.offset ?? 0;
        const items = matching.slice(offset, offset + (options.limit ?? 100));
        const page = { items, total, hasMore: offset + items.length < total };
        if (deferHistoryRestorePage && offset === 200) {
          deferHistoryRestorePage = false;
          return await new Promise<typeof page>((resolve) => { finishHistoryRestorePage = resolve; });
        }
        return page;
      }
      case "get_playlist_preferences": return playlistPreferences;
      case "set_playlist_shortcut": {
        const shortcuts = [...playlistPreferences.shortcuts];
        shortcuts[Number(args?.slot)] = args?.collectionId ? String(args.collectionId) : null;
        playlistPreferences = { ...playlistPreferences, shortcuts };
        return playlistPreferences;
      }
      case "set_playlist_shortcut_binding": {
        const shortcutBindings = [...playlistPreferences.shortcutBindings];
        shortcutBindings[Number(args?.slot)] = String(args?.binding);
        playlistPreferences = { ...playlistPreferences, shortcutBindings };
        return playlistPreferences;
      }
      case "set_favorites_playlist": playlistPreferences = { ...playlistPreferences, favoritesPlaylistId: String(args?.collectionId) }; return playlistPreferences;
      case "set_favorite": {
        const track = args?.track as Track;
        if (args?.value) favoriteTrackIds.add(track.id); else favoriteTrackIds.delete(track.id);
        return undefined;
      }
      case "record_playback": {
        const sessionId = String(args?.sessionId);
        const track = args?.track as Track;
        historyEntries = [...historyEntries.filter((item) => item.sessionId !== sessionId), {
          id: `history-${sessionId}`, track, provider: String(args?.provider), sessionId,
          playedAt: "2026-10-12T12:00:00Z", chapter: args?.chapter,
        }];
        return undefined;
      }
      case "cache_artwork": return "/cached-artwork.jpg";
      case "list_tracks_page": {
        if (args?.view === "collections" && failNextCollectionLoad) { failNextCollectionLoad=false; throw new Error("Offline"); }
        const rows=localMockRows(args), offset=Number(args?.offset??0),limit=Number(args?.limit??200);
        return {tracks:rows.slice(offset,offset+limit),total:rows.length,offset,limit};
      }
      case "list_directory_browser_roots": return directoryBrowserRoots;
      case "list_directory_browser_children": {
        const rootId = String(args?.rootId ?? "");
        const relativePath = String(args?.relativePath ?? "");
        return { rootId, relativePath, directories: rootId === "folder-root" && relativePath === "" ? (directoryBrowserPage?.directories as unknown[] | undefined) ?? [] : [] };
      }
      case "choose_directory_browser_root": return { path: "/archive" };
      case "add_directory_browser_root": {
        const root = { id: "archive-root", name: "Archive", path: String(args?.path ?? "/archive") };
        directoryBrowserRoots = [...directoryBrowserRoots.filter(item => item.id !== root.id), root];
        return root;
      }
      case "remove_directory_browser_root":
        directoryBrowserRoots = directoryBrowserRoots.filter(item => item.id !== String(args?.rootId ?? ""));
        return undefined;
      case "browse_directory_browser": {
        const relativePath = String(args?.relativePath ?? "");
        if (relativePath === "Jazz" && directoryBrowserNestedPage) return { ...directoryBrowserNestedPage, offset: args?.offset ?? 0, limit: args?.limit ?? 200 };
        return { ...directoryBrowserPage, offset: args?.offset ?? 0, limit: args?.limit ?? 200 };
      }
      case "list_track_prefix": {
        const rows=localMockRows(args),limit=Number(args?.limit??0);
        return {rows:rows.slice(0,limit).map((track,index)=>({listingId:track.id,playlistPosition:args?.view==="collections"?((track.provenance as Record<string,unknown>)?.playlistPosition as number ?? index+1):null})),total:rows.length,limit};
      }
      case "get_tracks": {
        const rows = automationTrackLookup ?? savedChartTracks;
        return ((args?.ids ?? []) as string[]).flatMap((id) => {
          const track = rows.find((item) => item.id === id);
          return track ? [track] : [];
        });
      }
      case "list_favorite_track_ids": return savedChartTracks.filter(track=>favoriteTrackIds.has(track.id)).map(track=>track.id);
      case "list_tracks":
        if (args?.view === "collections" && failNextCollectionLoad) {
          failNextCollectionLoad = false;
          throw new Error("Offline");
        }
        if (args?.view === "collections") return playlistRemoved ? [] : [...(mockCollectionRows ?? [collectionTrack])];
        if (args?.view === "favorites") return savedChartTracks.filter((track) => favoriteTrackIds.has(track.id)).map((track) => ({ ...track, favorite: true }));
        return savedChartTracks;
      case "search_tracks": return [savedTrack];
      case "search_albums": return { items: [], nextPage: null, warnings: [] };
      case "list_search_sources": return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"], enabled: true },
        { id: "discogs", name: "Discogs", kinds: ["artist", "album"], enabled: true },
        { id: "youtube", name: "YouTube", kinds: ["track", "video"], enabled: true },
        { id: "soundcloud", name: "SoundCloud", kinds: ["track", "set"], enabled: true },
        { id: "soulseek", name: "Soulseek", kinds: ["track"], enabled: slskdConnectionEnabled },
      ];
      case "search_source": {
        const sourceId = String((args?.sourceIds as string[] | undefined)?.[0] ?? "library");
        const requestedKinds = args?.kinds as string[] | undefined;
        if (sourceId === "musicbrainz" && requestedKinds?.includes("album") && discoveryAlbum) {
          return { sourceId, complete: true, results: [{ id: discoveryAlbum.release.id, kind: "album", sourceId, title: discoveryAlbum.title, url: discoveryAlbum.sourceUrl, data: discoveryAlbum }] };
        }
        if (sourceId === "soulseek") {
          const track = { ...savedTrack, id: "peer-result", provider: "slskd", providerId: "peer-hash", title: "Peer Shared Recording", provenance: { sourceKind: "audioSearch", audioCandidate: soulseekCandidate(savedTrack) } };
          if (partialSoulseekSearch) return { sourceId, complete: false, progress: "searching", nextCursor: "slskd-search-1", results: [] };
          return { sourceId, complete: true, results: [{ id: track.id, kind: "track", sourceId, title: track.title, url: track.sourceUrl, track }] };
        }
        const audioSource = ["youtube", "soundcloud"].includes(sourceId);
        const rows = audioSource ? audioSearchResult.filter((track) => {
          const candidate = (track.provenance as { audioCandidate?: Candidate } | undefined)?.audioCandidate;
          return !candidate || candidate.provider === sourceId;
        }) : [savedTrack];
        return { sourceId, complete: true, results: rows.map((item) => {
          const track = audioSource ? { ...item, provenance: { ...(item.provenance as Record<string, unknown> ?? {}), sourceKind: "audioSearch" } } : item;
          const candidate = (track.provenance as { audioCandidate?: Candidate } | undefined)?.audioCandidate;
          return { id: track.id, kind: audioSource ? "video" : "track", sourceId, title: track.title, subtitle: track.artist, url: candidate?.url ?? track.sourceUrl, track };
        }) };
      }
      case "poll_search_source": {
        const track = { ...savedTrack, id: "peer-result", provider: "slskd", providerId: "peer-hash", title: "Peer Shared Recording", provenance: { sourceKind: "audioSearch", audioCandidate: soulseekCandidate(savedTrack) } };
        return { sourceId: "soulseek", complete: true, results: [{ id: track.id, kind: "track", sourceId: "soulseek", title: track.title, url: track.sourceUrl, track }] };
      }
      case "enrich_search_result": return args?.track;
      case "inspect_youtube_source": return args?.candidate ?? { ...youtubeCandidate(savedTrack), chapters: chapterResults ?? [] };
      case "register_chapter_track": return args?.track;
      case "get_autotag_settings": return { enabled: true, shazamFallback: true, fpcalcAvailable: false, songrecAvailable: false, acoustidConfigured: false };
      case "get_autotag_jobs": return [];
      case "get_chart_memberships": return [];
      case "get_chart": return chart;
      case "list_assets": return savedAssets;
      case "list_assets_for_paths": if (unsupportedScopedAssetCommands) throw new Error("unknown Tauri command list_assets_for_paths"); return savedAssets.filter(asset => ((args?.paths ?? []) as string[]).includes(asset.path));
      case "list_assets_by_ids": if (unsupportedScopedAssetCommands) throw new Error("unknown Tauri command list_assets_by_ids"); return savedAssets.filter(asset => ((args?.assetIds ?? []) as string[]).includes(asset.id));
      case "list_track_files_for_recordings": if (unsupportedScopedAssetCommands) throw new Error("unknown Tauri command list_track_files_for_recordings"); return ((args?.recordingIds ?? []) as string[]).map(trackId => ({ trackId, files: filesForRequestedTrack(trackId) }));
      case "list_track_files": {
        const files = filesForRequestedTrack(String(args?.trackId ?? ""));
        const replacement = savedAssets.find(asset => asset.id === "preferred-after-change");
        return preferredAssetChoice && replacement ? [files[0], replacement].filter((asset): asset is LocalAsset => Boolean(asset)) : files;
      }
      case "count_assets": if (unsupportedScopedAssetCommands) throw new Error("unknown Tauri command count_assets"); return savedAssets.length;
      case "get_analysis_status_for_assets": if (unsupportedScopedAssetCommands) throw new Error("unknown Tauri command get_analysis_status_for_assets"); return analysisStatusResult.filter(status => ((args?.assetIds ?? []) as string[]).includes(status.assetId));
      case "list_asset_artist_matches_page": return { items: [], total: 0, limit: args?.limit ?? 200, offset: args?.offset ?? 0 };
      case "get_analysis_status": return analysisStatusResult;
      case "analyze_local_assets": return { queued: (args?.assetIds as string[] | undefined)?.length ?? 0 };
      case "update_track": return args?.track;
      case "search_audio_tracks": return audioSearchResult;
      case "save_settings": {
        const value = args?.settings as Settings;
        configuredMusicDirectory = value.musicDirectory ?? configuredMusicDirectory;
        configuredPreferredDownloadSource = value.preferredDownloadSource ?? configuredPreferredDownloadSource;
        streamingFallbackEnabled = value.streamingFallbackEnabled ?? streamingFallbackEnabled;
        streamingFallbackSources = value.streamingFallbackSources ?? streamingFallbackSources;
        downloadFallbackEnabled = value.downloadFallbackEnabled ?? downloadFallbackEnabled;
        downloadFallbackSources = value.downloadFallbackSources ?? downloadFallbackSources;
        return undefined;
      }
      case "save_connections": return { settings: { slskd: { enabled: false } }, slskdCredential: { present: false, sessionOnly: false }, discogsCredential: { present: false, sessionOnly: false }, lastfmCredential: { present: false, sessionOnly: false } };
      case "test_connections": return [];
      case "enqueue_download": return { id: "job-1" };
      case "list_jobs": return downloadJobs;
      case "rename_collection": {
        if (args?.collectionId === mutableFavoritesCollection.id) {
          mutableFavoritesCollection = { ...mutableFavoritesCollection, name: String(args.name) };
          return mutableFavoritesCollection;
        }
        return { id: args?.collectionId, name: args?.name, trackCount: 131 };
      }
      case "remove_from_collection": {
        if (args?.collectionId === "collection-full") {
          if (mockCollectionRows) {
            const removed = new Set((args?.trackIds as string[] | undefined) ?? []);
            mockCollectionRows = mockCollectionRows.filter(track => !removed.has(track.id));
            playlistRemoved = mockCollectionRows.length === 0;
          } else playlistRemoved = true;
        }
        return undefined;
      }
      case "move_playlist_entry": {
        if (mockCollectionRows && args?.collectionId === "collection-full") {
          const from=Number(args.from),to=Number(args.to),rows=[...mockCollectionRows];
          if(from>=0&&to>=0&&from<rows.length&&to<rows.length){const [track]=rows.splice(from,1);rows.splice(to,0,track);mockCollectionRows=rows;}
        }
        return args?.collectionId;
      }
      case "delete_collection": {
        const collectionId = String(args?.collectionId);
        deletedCollectionIds.add(collectionId);
        const deleted = collections.find((collection) => collection.id === collectionId);
        if (deleted && !deletedCollectionRecords.some((item) => item.id === collectionId)) deletedCollectionRecords.push(deleted);
        return undefined;
      }
      case "restore_collection": {
        const collectionId = String(args?.collectionId);
        deletedCollectionIds.delete(collectionId);
        deletedCollectionRecords = deletedCollectionRecords.filter((item) => item.id !== collectionId);
        return undefined;
      }
      case "create_collection": return { id: "collection-new", name: args?.name, trackCount: 0 };
      case "add_to_collection": return undefined;
      case "append_playlist_entries": return undefined;
      case "inspect_candidate": {
        const candidate = args?.candidate as Candidate;
        if (candidate.id === delayCandidateInspectId) return new Promise((resolve) => { finishCandidateInspect = resolve; });
        return { ...candidate, ...(batchInspectionHigh ? { confidence: "high" } : {}), formats: [{ id: "audio-best", codec: "Opus", extension: "webm", bitrate: 192, lossless: false }] };
      }
      case "get_track_details": return { ...(args?.track as Track), assetId: preferredAssetChoice || (args?.track as Track | undefined)?.assetId };
      case "set_preferred_track_file": preferredAssetChoice = String(args?.assetId ?? ""); return undefined;
      case "resolve_preview": {
        const track = args?.track as Track;
        if (previewMode === "direct") { const result = { kind: "audio", url: "/cached-preview.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: directPreviewExcerpt, sourceGeneration: webviewSourceGeneration, audioMimeType: "audio/mp4", audioCodec: "mp4a.40.2", audioFormatId: "140", audioFormatHasVideo: false, audioExpectedContentLength: 123456 } satisfies Preview; afterPreviewResolve?.(); return result; }
        if (previewMode === "race" && track.id === savedTrack.id) throw new Error("Cached source unavailable");
        if (previewMode === "fallback") throw new Error("No cached preview");
        return { kind: "external", url: track.sourceUrl, canSeek: false, canVolume: false, canAutoplay: false, isExcerpt: false } satisfies Preview;
      }
      case "resolve_playback_track": {
        const track = args?.track as Track;
        if (track.id === delayPlaybackTrackId) return new Promise((resolve) => { finishPlaybackResolve = resolve; });
        if (playbackResolverFailure) throw new Error(playbackResolverFailure);
        const hinted = (args?.selectedCandidate as Candidate | null | undefined)
          ?? (track.provenance as Record<string, unknown> | undefined)?.audioCandidate as Candidate | undefined;
        const queued = playbackResolverCandidates.shift();
        if (queued instanceof Error) throw queued;
        const candidate = queued ?? (playbackResolverCandidate === undefined ? hinted ?? withChapters(youtubeCandidate(track)) : playbackResolverCandidate);
        afterPlaybackResolver?.();
        return {
          candidate,
          preview: { kind: "audio", url: `/resolved-${candidate?.id ?? track.id}.mp3`, canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false, durationSeconds: track.durationSeconds, provider: candidate?.provider, title: candidate?.title, sourceGeneration: webviewSourceGeneration, audioMimeType: "audio/mp4", audioCodec: "mp4a.40.2", audioFormatId: "140", audioFormatHasVideo: false, audioExpectedContentLength: 123456 } satisfies Preview,
        };
      }
      case "cancel_playback_resolution": return undefined;
      case "find_candidates": {
        const track = args?.track as Track;
        const requestedProvider = (args?.providerIds as string[] | undefined)?.[0];
        if (requestedProvider && failedCandidateProviders.has(requestedProvider)) throw new Error(`${requestedProvider} unavailable`);
        if (requestedProvider && delayedCandidateProviders.has(requestedProvider)) {
          return new Promise((resolve) => delayedCandidateProviders.set(requestedProvider, resolve as (value: Candidate[]) => void));
        }
        if ((args?.providerIds as string[])?.includes("monochrome")) return monochromeCandidates;
        if ((args?.providerIds as string[])?.includes("slskd")) {
          const candidate = soulseekCandidate(track);
          return [{ ...candidate, confidence: "review", score: 0.85 }];
        }
        if ((args?.providerIds as string[])?.includes("youtube") && (args?.providerIds as string[])?.includes("soundcloud")) {
          return [withChapters(youtubeCandidate(track)), { ...withChapters(youtubeCandidate(track)), id: `soundcloud:${track.id}`, provider: "soundcloud", url: "https://soundcloud.com/visible-artist/saved-recording" }];
        }
        if ((args?.providerIds as string[])?.includes("soundcloud")) {
          return [{ ...withChapters(youtubeCandidate(track)), id: `soundcloud:${track.id}`, provider: "soundcloud",
            url: "https://soundcloud.com/visible-artist/saved-recording" }];
        }
        if (previewMode === "race" && track.id === savedTrack.id) {
          return new Promise((resolve) => { finishOldSearch = resolve; });
        }
        if (previewMode === "alternates" && args?.providerIds && (args.providerIds as string[]).includes("youtube")) {
          return [withChapters(youtubeCandidate(track)), { ...withChapters(youtubeCandidate(track)), id: `youtube-alt:${track.id}`, url: `https://youtube.com/watch?v=${track.id.padEnd(10, "y")}z`, title: `${track.artist} - ${track.title} (Live)` }];
        }
        return [withChapters(youtubeCandidate(track))];
      }
      case "warm_track_previews": return undefined;
      case "start_slskd_search": return { id: "slskd-search-1" };
      case "get_slskd_search": {
        const track = { ...savedTrack, id: "peer-result", provider: "slskd", providerId: "peer-hash", title: "Peer Shared Recording", provenance: { sourceKind: "audioSearch" } };
        return { id: args?.searchId, state: "complete", tracks: [{ ...track, provenance: { sourceKind: "audioSearch", audioCandidate: soulseekCandidate(track) } }], complete: true, warnings: [] };
      }
      case "cancel_slskd_search": return undefined;
      case "open_external_url": return undefined;
      case "resolve_candidate_audio": {
        const candidate = args?.candidate as Candidate;
        if (candidate.id === delayAudioCandidateId) return new Promise((resolve) => { finishAudioResolve = resolve; });
        if (candidateAudioFailure) throw new Error(candidateAudioFailure);
        return { kind: "audio", url: `/candidate-${candidate.id}.mp3`, canSeek: true, canVolume: true, canAutoplay: true,
          isExcerpt: false, provider: candidate.provider, sourceGeneration: webviewSourceGeneration,
          title: candidate.title } satisfies Preview;
      }
      case "start_candidate_preview": {
        const candidate = args?.candidate as Candidate;
        return { kind: "candidate", url: candidate.url, canSeek: true, canVolume: true, canAutoplay: false,
          isExcerpt: false, sessionId: `session-${candidate.id}`, provider: candidate.provider,
          title: candidate.title } satisfies Preview;
      }
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
});

afterEach(() => {
  unmountApp?.();
  unmountApp = undefined;
  sessionStorage.removeItem("burrata.webviewMediaEventDiagnostics");
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

afterEach(() => { if (vi.isFakeTimers()) vi.useRealTimers(); });

describe("periodic playlist sync fallback wiring", () => {
  it.each([[true, false], [false, true]] as const)("watcher healthy=%s triggers sync=%s", async (healthy, shouldSync) => {
    playlistWatcherHealthy = healthy;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const before = mocks.call.mock.calls.filter(([command]) => command === "sync_playlists").length;
    expect(periodicPlaylistSyncCallbacks.length).toBeGreaterThan(0);
    for (const callback of periodicPlaylistSyncCallbacks) callback();
    await settle();
    const after = mocks.call.mock.calls.filter(([command]) => command === "sync_playlists").length;
    expect(mocks.call).toHaveBeenCalledWith("playlist_watcher_running");
    expect(after - before).toBe(shouldSync ? 1 : 0);
  });
});

describe("read-only WebView media inspection", () => {
  async function latestPublishedPlayback() {
    const call = [...mocks.call.mock.calls].reverse().find(([command]) => command === "automation_playback_state");
    if (!call) throw new Error("No playback snapshot has been published");
    return (call[1] as { playback: Record<string, unknown> }).playback;
  }
  async function latestPublishedPlaybackFor(trackId: string) {
    let latest: Record<string, unknown> = {};
    for (let attempt = 0; attempt < 40; attempt += 1) {
      latest = await latestPublishedPlayback();
      if (latest.trackId === trackId && typeof latest.sessionId === "string" && latest.sessionId.length > 0 &&
          Number.isSafeInteger(latest.sourceGeneration) && Object.prototype.hasOwnProperty.call(latest, "entryId")) return latest;
      await settle();
    }
    throw new Error(`Current WebView playback identity was not published: ${JSON.stringify({trackId:latest.trackId,sessionId:latest.sessionId,sourceGeneration:latest.sourceGeneration,entryId:latest.entryId})}`);
  }
  async function inspectWithRequest(request: Record<string, unknown>) {
    const handler = eventHandlers.get("automation-inspect-webview-media");
    if (!handler) throw new Error("WebView media inspection listener is not registered");
    handler({ event: "automation-inspect-webview-media", id: Date.now(), payload: request });
    await settle();
    return [...mocks.call.mock.calls].reverse().find(([command]) => command === "automation_layout_complete")?.[1] as { requestId: string; layout: Record<string, unknown> } | undefined;
  }
  it("samples direct media-element getters and returns only bounded fields", async () => {
    previewMode = "direct";
    savedChartTracks = [savedTrack];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click(); await settle();
    const element = document.querySelector<HTMLAudioElement>("audio")!;
    Object.defineProperties(element, {
      currentTime: { configurable: true, get: () => 10.25 }, duration: { configurable: true, get: () => 131 },
      paused: { configurable: true, get: () => false }, readyState: { configurable: true, get: () => 4 },
      networkState: { configurable: true, get: () => 2 }, currentSrc: { configurable: true, get: () => "https://secret.invalid/audio?token=never-return" },
      error: { configurable: true, get: () => null },
      seeking: { configurable: true, get: () => true },
      buffered: { configurable: true, get: () => ({ length: 2, start: (index: number) => [0, 10][index], end: (index: number) => [5, 18][index] }) },
      seekable: { configurable: true, get: () => ({ length: 1, start: () => 0, end: () => 131 }) },
    });
    const playback = await latestPublishedPlayback();
    const response = await inspectWithRequest({ requestId: "webview-media:test", trackId: playback.trackId, sessionId: playback.sessionId, sourceGeneration: playback.sourceGeneration, seekSerial: playback.seekSerial, currentIndex: playback.currentIndex, queueTotal: (playback.queue as unknown[]).length, entryId: null, queueTrackId: playback.trackId });
    expect(response?.layout).toMatchObject({ engine: "webview", applicable: true, media: { present: true, currentTime: 10.25, duration: 131, paused: false, seeking: true, buffered: [[0, 5], [10, 18]], seekable: [[0, 131]], readyState: 4, networkState: 2, errorCode: null, sourcePresent: true, sourceStable: true } });
    expect(JSON.stringify(response)).not.toContain("secret.invalid");
    expect(JSON.stringify(response)).not.toContain("never-return");
  });
  it("includes opted-in bounded event diagnostics with source ownership and no source URLs", async () => {
    sessionStorage.setItem("burrata.webviewMediaEventDiagnostics", "1");
    previewMode = "direct";
    savedChartTracks = [savedTrack];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click(); await settle();
    const element = document.querySelector<HTMLAudioElement>("audio")!;
    Object.defineProperties(element, {
      currentTime: { configurable: true, get: () => 12.5 }, readyState: { configurable: true, get: () => 2 },
      networkState: { configurable: true, get: () => 2 }, paused: { configurable: true, get: () => false },
      buffered: { configurable: true, get: () => ({ length: 1, start: () => 12, end: () => 18 }) },
      currentSrc: { configurable: true, get: () => "https://secret.invalid/audio?token=never-return" },
    });
    element.dispatchEvent(new Event("play"));
    element.dispatchEvent(new Event("waiting"));
    // `seeked` publishes a playback snapshot immediately; the publisher's regular
    // heartbeat is one second, so a single generic settle can leave the idle snapshot latest.
    element.dispatchEvent(new Event("seeked"));
    await settle();
    const playback = await latestPublishedPlaybackFor(savedTrack.id);
    const entryId = playback.entryId ?? null;
    const response = await inspectWithRequest({ requestId: "webview-media:events", trackId: playback.trackId, sessionId: playback.sessionId, sourceGeneration: playback.sourceGeneration, seekSerial: playback.seekSerial, currentIndex: playback.currentIndex, queueTotal: (playback.queue as unknown[]).length, entryId, queueTrackId: playback.trackId });
    expect(response?.layout).toMatchObject({ trackId: savedTrack.id, sessionId: playback.sessionId, sourceGeneration: playback.sourceGeneration, entryId, seekSerial: playback.seekSerial });
    expect(response?.layout.diagnostics).toMatchObject({ owner: { sessionId: playback.sessionId, sourceGeneration: playback.sourceGeneration, entryId, seekSerial: playback.seekSerial }, sourceFormat: { codec: "mp4a.40.2", mimeType: "audio/mp4", formatId: "140", hasVideo: false, expectedContentLength: 123456, streamKind: "direct" }, events: [{ type: "play" }, { type: "waiting", readyState: 2, currentTime: 12.5, buffered: [{ start: 12, end: 18 }] }, { type: "seeked" }] });
    expect(JSON.stringify(response)).not.toContain("secret.invalid");
    expect(JSON.stringify(response)).not.toContain("never-return");
    sessionStorage.removeItem("burrata.webviewMediaEventDiagnostics");
  });
  it("returns actual identity on a stale request and reports an absent element without inventing media fields", async () => {
    const mediaListenerCleanup=vi.fn();
    mocks.listen.mockImplementation(async (event: string, handler: (event: { event: string; id: number; payload: unknown }) => void) => {
      eventHandlers.set(event,handler);
      return () => { if(event==="automation-inspect-webview-media")mediaListenerCleanup(); eventHandlers.delete(event); };
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    const playback = await latestPublishedPlayback();
    const stale = await inspectWithRequest({ requestId: "webview-media:stale", trackId: "old-track", sessionId: playback.sessionId, sourceGeneration: playback.sourceGeneration, seekSerial: playback.seekSerial, currentIndex: playback.currentIndex, queueTotal: 0, entryId: null, queueTrackId: null });
    expect(stale?.layout.trackId).toBeNull();
    expect(stale?.layout.trackId).not.toBe("old-track");
    expect(stale?.layout.media).toMatchObject({ present: false, currentTime: null, duration: null, paused: null, readyState: null, networkState: null, errorCode: null, sourcePresent: false, sourceStable: false });
    const lateHandler=eventHandlers.get("automation-inspect-webview-media");
    unmountApp();unmountApp=undefined;await settle();
    expect(mediaListenerCleanup).toHaveBeenCalledOnce();
    const completionCount=mocks.call.mock.calls.filter(([command])=>command==="automation_layout_complete").length;
    lateHandler?.({event:"automation-inspect-webview-media",id:900,payload:{requestId:"webview-media:late"}});
    await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="automation_layout_complete")).toHaveLength(completionCount);
  });
});

describe("directory browser navigation and playback", () => {
  it("restores distinct folder locations across Library, Back and Forward",async()=>{
    directoryBrowserPage={rootId:"folder-root",path:"/music",relativePath:"",breadcrumbs:[{name:"Music",relativePath:""}],directories:[{name:"Jazz",relativePath:"Jazz"}],files:[],total:0,offset:0,limit:200};
    directoryBrowserNestedPage={...directoryBrowserPage,path:"/music/Jazz",relativePath:"Jazz",breadcrumbs:[{name:"Music",relativePath:""},{name:"Jazz",relativePath:"Jazz"}],directories:[]};
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const root=document.querySelector<HTMLButtonElement>('[role="treeitem"][aria-label="Music"]')!;
    root.querySelector(".folder-tree-chevron")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    document.querySelector<HTMLButtonElement>('[role="treeitem"][aria-label="Jazz"]')!.click();await settle();
    const folderFilter=document.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')!;
    folderFilter.value="saved-folder-query";folderFilter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click();await settle();
    root.click();await settle();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).not.toContain("Jazz");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Library");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Jazz");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')?.value).toBe("saved-folder-query");
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Library");
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();await settle();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).not.toContain("Jazz");
    expect(mocks.call.mock.calls.some(([command])=>["play_tracks","resolve_preview","resolve_audio"].includes(command))).toBe(false);
  });

  it("restores separate Spotify artist controls for repeated history entries",async()=>{
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    const original=mocks.call.getMockImplementation()!;const id="0123456789ABCDEFGHIJKL";
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="get_managed_provider_session_status")return {spotifyAccountId:"fixture",accountRevisions:{spotify:1}};
      if(command==="browse_remote_artist")return {artist:{id,name:"Fixture Artist",url:`https://open.spotify.com/artist/${id}`,genres:[]},albums:[{id:"album-fixture",title:"Fixture release",url:"https://open.spotify.com/album/album-fixture",artists:["Fixture Artist"]}],topTracksUnavailable:"None"};
      if(command==="browse_remote_artist_tracks")return {tracks:[],total:0,nextCursor:null};
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    let request=0;
    const go=async(target:unknown)=>{eventHandlers.get("automation-ui-navigation")?.({event:"automation-ui-navigation",id:1,payload:{requestId:`snapshot-${++request}`,target}});await settle();await settle();};
    await go({kind:"spotify_artist",spotifyArtistId:id});
    const setSort=async(value:string)=>{const select=document.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!;select.value=value;select.dispatchEvent(new Event("change",{bubbles:true}));await settle();};
    await setSort("title");await go({kind:"view",view:"library"});
    await go({kind:"spotify_artist",spotifyArtistId:id});await setSort("releaseDate");await go({kind:"view",view:"library"});
    const back=async()=>{document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();};
    await back();expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("releaseDate");
    await back();expect(document.querySelector("main h1")?.textContent).toBe("Library");
    await back();expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("title");
    providerNavigationSnapshots.clear();
  });

  it("restores catalog artist discography controls through the App history binding",async()=>{
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>(".artist-link")!.click();await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();await settle();
    [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Artist sections"] button')].find(button=>button.textContent==="Discography")!.click();await settle();
    const query=document.querySelector<HTMLInputElement>('[aria-label="Search discography"]')!;
    query.value="saved-release-query";query.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    const releaseType=document.querySelector<HTMLSelectElement>('[aria-label="Release type"]')!;
    releaseType.value="All";releaseType.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click();await settle();
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");
    expect(providerNavigationSnapshots.maxEntries).toBe(20);
    for(let index=0;index<21;index++)providerNavigationSnapshots.put({kind:"artist-page",key:`history-pressure-${index}`,accountScope:"public-catalog",snapshot:{},rows:0,stripRows:()=>({})});
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search discography"]')?.value).toBe("saved-release-query");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Release type"]')?.value).toBe("All");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector('[aria-label="Artist sections"] button.active')?.textContent).toBe("Overview");
    expect(document.querySelector('[aria-label="Search discography"]')).toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search discography"]')?.value).toBe("saved-release-query");
    providerNavigationSnapshots.clear();
    expect(mocks.call.mock.calls.some(([command])=>["play_tracks","resolve_preview","resolve_audio"].includes(command))).toBe(false);
  });

  it("does not apply a stale Back restoration after a newer navigation",async()=>{
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const filter=document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value="old-library-filter";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    eventHandlers.get("automation-ui-navigation")?.({event:"automation-ui-navigation",id:1,payload:{requestId:`history-${mocks.call.mock.calls.length}`,target:{kind:"view",view:"history"}}});await settle();await settle();
    const original=mocks.call.getMockImplementation()!;let finish!:(value:unknown)=>void;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="list_tracks_page"&&args?.view==="library")return new Promise(resolve=>finish=resolve);
      return original(command,args);
    });
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();
    eventHandlers.get("automation-ui-navigation")?.({event:"automation-ui-navigation",id:1,payload:{requestId:`history-${mocks.call.mock.calls.length}`,target:{kind:"view",view:"history"}}});await settle();await settle();
    const history=document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!;
    history.value="new-history-filter";history.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    finish({tracks:[],total:0,offset:0,limit:200});await settle();await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Listening history");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')?.value).toBe("new-history-filter");
    expect(document.querySelector('[aria-label="Filter tracks"]')).toBeNull();
  });

  it("keeps the browsed folder and plays the exact indexed path asset", async () => {
    const pathTrack: Track = {
      id: `local:${"a".repeat(64)}`, provider: "local", providerId: "a".repeat(64), sourceUrl: `local:${"a".repeat(64)}`,
      artist: "Folder Artist", title: "Jazz Cut", favorite: false, localAvailability: "available",
      localPath: "/music/Jazz/Cut.flac", assetId: "asset-jazz-cut", provenance: {},
    };
    directoryBrowserPage = {
      rootId: "folder-root", path: "/music", relativePath: "", breadcrumbs: [{ name: "Music", relativePath: "" }],
      directories: [{ name: "Jazz", relativePath: "Jazz" }], files: [], total: 0, offset: 0, limit: 200,
    };
    directoryBrowserNestedPage = {
      rootId: "folder-root", path: "/music/Jazz", relativePath: "Jazz",
      breadcrumbs: [{ name: "Music", relativePath: "" }, { name: "Jazz", relativePath: "Jazz" }],
      directories: [], files: [{ path: pathTrack.localPath!, name: "Cut.flac", assetId: pathTrack.assetId, track: pathTrack }],
      total: 1, offset: 0, limit: 200,
    };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const folderRoot = [...document.querySelectorAll<HTMLButtonElement>('[role="treeitem"]')].find(button => button.getAttribute("aria-label") === "Music")!;
    folderRoot.querySelector(".folder-tree-chevron")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector<HTMLButtonElement>('[role="treeitem"][aria-label="Jazz"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Music/Jazz");

    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('button[aria-label="Folders"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Music/Jazz");
    expect(document.querySelector('[aria-label="Play Jazz Cut"]')).not.toBeNull();

    document.querySelector<HTMLButtonElement>('[aria-label="Play Jazz Cut"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_preview", expect.objectContaining({ track: pathTrack }));
    expect(mocks.call.mock.calls.some(([command, args]) => command === "resolve_preview" && (args as { track?: Track })?.track?.localPath === "/music/Jazz/Cut.flac" && (args as { track?: Track })?.track?.assetId === "asset-jazz-cut")).toBe(true);
  });

  it("registers a folder from the sidebar and updates the controlled browser root", async () => {
    directoryBrowserRoots = [{ id: "folder-root", name: "Music", path: "/music" }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Add folder"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("choose_directory_browser_root");
    expect(mocks.call).toHaveBeenCalledWith("add_directory_browser_root", { path: "/archive" });
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Archive");
    expect(document.querySelector('[role="treeitem"][aria-label="Archive"]')?.getAttribute("aria-current")).toBe("location");

    document.querySelector<HTMLButtonElement>('[aria-label="Unregister folder Archive"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("remove_directory_browser_root", { rootId: "archive-root" });
    expect(document.querySelector('[role="treeitem"][aria-label="Archive"]')).toBeNull();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Music");
  });
});

describe("saved recording navigation", () => {
  it("searches for more information from track actions without requiring a catalog match", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button).includes("Search for more info"))!.click();
    await settle();
    const dialog = document.querySelector<HTMLDialogElement>("dialog.info-search")!;
    expect(dialog).not.toBeNull();
    const query = dialog.querySelector<HTMLInputElement>("#info-query")!;
    expect(query.value).toContain("Visible Artist");
    query.value = "An intentionally unmatched recording title";
    query.dispatchEvent(new Event("input", { bubbles: true }));
    dialog.querySelector<HTMLButtonElement>('button[type="submit"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Discovery search"]')).not.toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "An intentionally unmatched recording title" }));
    expect(mocks.call.mock.calls.some(([command]) => command === "update_track" || command === "apply_autotag_review")).toBe(false);
  });

  it("uses MusicBrainz all-results preset even when Discovery remembers YouTube", async () => {
    const local = new Map([["burrata.discovery.search-scope.v2", JSON.stringify({ mode: "online", sourceId: "youtube" })]]);
    vi.stubGlobal("localStorage", { getItem: (key: string) => local.get(key) ?? null, setItem: (key: string, value: string) => local.set(key, value), removeItem: (key: string) => local.delete(key) });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button).includes("Search for more info"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('dialog.info-search button[type="submit"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({
      query: "Saved Recording Visible Artist", sourceIds: ["musicbrainz"], kinds: expect.arrayContaining(["track", "artist", "album", "video", "set"]),
    }));
    expect(mocks.call.mock.calls.some(([command]) => ["set_favorite", "update_track", "import_browser_snapshot"].includes(String(command)))).toBe(false);
  });

  it("opens a public web search and source link from the more information panel", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button).includes("Search for more info"))!.click();
    await settle();
    const dialog = document.querySelector<HTMLDialogElement>("dialog.info-search")!;
    dialog.querySelector<HTMLButtonElement>(".source")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: savedTrack.sourceUrl });
    dialog.querySelector<HTMLButtonElement>('button[type="button"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://www.google.com/search?q=Saved%20Recording%20Visible%20Artist" });
    expect(mocks.call.mock.calls.some(([command]) => command === "update_track")).toBe(false);
  });

  it("starts in Library without auto-opening a saved Spotify playlist", async () => {
    delayedBrowserImports = Promise.resolve([{ ...latestImport, provider: "spotify", kind: "playlist" }]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelector(".shared-track-list")?.textContent).toContain("Saved Recording");
    expect(mocks.call).not.toHaveBeenCalledWith("load_browser_import", { id: "snapshot-new" });
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });
  it("hydrates only the visible library page without a startup inventory scan", async () => {
    const localTrack = { ...savedTrack, localAvailability: "available" as const };
    savedChartTracks = [localTrack];
    savedAssets = [{ id: "visible-asset", recordingId: localTrack.id, path: "/music/visible.flac", hash: "hash-visible",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: localTrack }];
    analysisStatusResult = [{ assetId: "visible-asset", status: "completed" }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("list_track_files_for_recordings", { recordingIds: [savedTrack.id] });
    expect(mocks.call).not.toHaveBeenCalledWith("list_assets", expect.anything());
    expect(mocks.call.mock.calls.some(([command]) => command === "list_assets")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "get_analysis_status")).toBe(false);
  });

  it("falls back to legacy inventory commands when the installed native host lacks scoped asset IPC", async () => {
    unsupportedScopedAssetCommands = true;
    const localTrack = { ...savedTrack, localAvailability: "available" as const };
    savedChartTracks = [localTrack];
    savedAssets = [{ id: "legacy-visible", recordingId: localTrack.id, path: "/music/legacy.flac", hash: "legacy-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: localTrack }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("list_assets");
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows every local file returned for a catalog alias track ID", async () => {
    const aliasTrack = { ...savedTrack, id: "catalog-alias", title: "Alias Recording" };
    const canonicalTrack = { ...aliasTrack, id: "canonical-recording" };
    savedChartTracks = [aliasTrack];
    const files = [
      { id: "alias-file-a", recordingId: canonicalTrack.id, path: "/music/alias-a.flac", hash: "hash-a", sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: canonicalTrack },
      { id: "alias-file-b", recordingId: canonicalTrack.id, path: "/music/alias-b.mp3", hash: "hash-b", sourceCodec: "mp3", finalCodec: "mp3", processing: "complete", track: canonicalTrack },
    ] satisfies LocalAsset[];
    savedAssets = files;
    trackFilesByRequestedId.set(aliasTrack.id, files);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    await chooseTrackAction("Download options…", aliasTrack.title);
    const dialog = document.querySelector<HTMLDialogElement>("dialog.download-dialog")!;
    expect(dialog.textContent).toContain("alias-a.flac");
    expect(dialog.textContent).toContain("alias-b.mp3");
    expect(mocks.call).toHaveBeenCalledWith("list_track_files_for_recordings", { recordingIds: [aliasTrack.id] });
  });

  it("imports a supported public URL directly, saves it, and opens the returned snapshot", async () => {
    const url = "https://www.shazam.com/charts/top-200/world";
    directBrowserImport = {
      ...latestImport, id: "snapshot-direct-url", provider: "shazam", kind: "chart",
      title: "Direct Shazam Import", sourceUrl: url, collectionId: "collection-direct-url",
    };
    delayedBrowserImports = Promise.resolve([directBrowserImport, latestImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await openLibraryImport();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Chart, playlist or album URL"]')!;
    input.value = url;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")]
      .find((button) => button.textContent?.trim() === "Import URL")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("import_collection_url", { url });
    expect(mocks.call).toHaveBeenCalledWith("list_browser_imports");
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: "snapshot-direct-url" });
    expect(mocks.call.mock.calls.some(([command]) => command === "request_browser_import")).toBe(false);
    expect(document.body.textContent).toContain("Direct Shazam Import");
  });

  it("falls back to the connected companion when direct URL import fails", async () => {
    const url = "https://example.test/chart";
    directBrowserImportFailure = "Provider parser unavailable";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await openLibraryImport();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Chart, playlist or album URL"]')!;
    input.value = url;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")]
      .find((button) => button.textContent?.trim() === "Import URL")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("import_collection_url", { url });
    expect(mocks.call).toHaveBeenCalledWith("request_browser_import", { url });
    expect(document.body.textContent).toContain("Sent to the connected browser; use the companion");
    expect(document.querySelector('[aria-label="Chart, playlist or album URL"]')).not.toBeNull();
  });

  it("organizes imported playlists under source groups without a Personal or status row", async () => {
    const shazamImport = { ...latestImport, id: "snapshot-shazam", provider: "shazam", title: "Shazam Mix", sourceUrl: "https://example.test/shazam", collectionId: "collection-shazam" };
    const bandcampImport = { ...latestImport, id: "snapshot-bandcamp", provider: "bandcamp", title: "Bandcamp Picks", sourceUrl: "https://example.test/bandcamp", collectionId: "collection-bandcamp" };
    const lastfmImport = { ...latestImport, id: "snapshot-lastfm", provider: "lastfm", title: "Last.fm Mix", sourceUrl: "https://example.test/lastfm", collectionId: "collection-lastfm" };
    delayedBrowserImports = Promise.resolve([latestImport, oldImport, shazamImport, bandcampImport, lastfmImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const sidebar = document.querySelector(".library-sidebar")!;
    for (const provider of ["beatport", "traxsource", "spotify", "local"]) {
      expect(sidebar.querySelector(`.source-heading.${provider}`)).not.toBeNull();
    }
    expect(sidebar.querySelector(`.source-heading.shazam .source-name`)?.textContent).toContain("Shazam");
    expect(sidebar.querySelector(`.source-heading.bandcamp .source-name`)?.textContent).toContain("Bandcamp");
    expect(sidebar.querySelector(`.source-heading.lastfm .source-name`)?.textContent).toContain("Last.fm");
    expect(sidebar.textContent).not.toContain("Local first");
    expect(sidebar.textContent).not.toMatch(/\d+ browser imports? saved/);
    expect(sidebar.textContent).not.toContain("Personal");
    expect(collectionCard("Empty Collection").textContent).toContain("Empty Collection");
    const beatport = sourceGroup("beatport");
    beatport.open = false;
    (beatport.querySelector("summary") as HTMLElement).click();
    expect(beatport.open).toBe(true);
    expect(beatport.querySelectorAll(".imported-card")).toHaveLength(1);
  });
  it("shows one editable linked playlist instead of a snapshot and remote copy", async () => {
    delayedBrowserImports = Promise.resolve([{...latestImport, provider:"spotify", title:"Linked Spotify", sourceUrl:"https://open.spotify.com/playlist/example"}]);
    remoteBindingState = [{bindingId:"binding",provider:"spotify",collectionId:"linked-working",sourceCollectionId:"collection-full",url:"https://open.spotify.com/playlist/example",canEdit:true}];
    const instance = mount(App, {target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    await refreshSidebarBrowserImports();
    const cards=[...document.querySelectorAll<HTMLButtonElement>(".collection-card")].filter(card=>card.textContent?.includes("Linked Spotify"));
    expect(cards).toHaveLength(1);
    expect(cards[0].textContent).not.toContain("remote copy");
    expect(document.querySelector('.library-sidebar')?.textContent).not.toContain("Saved Collection");
    cards[0].click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page",expect.objectContaining({collectionId:"linked-working",limit:200,offset:0}));
    expect(document.querySelector("h1")?.textContent).toBe("Linked Spotify");
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')!.click(); await settle();
    expect(document.querySelector(".playlist-menu")?.textContent).toContain("Refresh playlist");
    expect(document.querySelector(".playlist-menu")?.textContent).toContain("Open source");
  });

  it("filters imported sidebar playlists by their already-loaded binding owner", async () => {
    delayedBrowserImports = Promise.resolve([{...latestImport, provider:"spotify", title:"Linked Spotify", sourceUrl:"https://open.spotify.com/playlist/example"}]);
    remoteBindingState = [{bindingId:"binding",provider:"spotify",collectionId:"linked-working",sourceCollectionId:"collection-full",url:"https://open.spotify.com/playlist/example",canEdit:true,ownerName:"DJ North",ownerId:"north-owner-id"}];
    const instance = mount(App, {target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    await refreshSidebarBrowserImports();
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    const bindingReads = mocks.call.mock.calls.filter(([command]) => command === "list_remote_playlist_bindings").length;
    for (const query of ["DJ NORTH", "north-owner-id"]) {
      filter.value = query; filter.dispatchEvent(new Event("input", {bubbles:true})); await settle();
      expect(sourceGroup("spotify").querySelector(".imported-card")?.textContent).toContain("Linked Spotify");
    }
    filter.value = "different owner"; filter.dispatchEvent(new Event("input", {bubbles:true})); await settle();
    expect(sourceGroup("spotify").querySelector(".imported-card")).toBeNull();
    expect(sourceGroup("spotify").textContent).toContain("No imported playlists match this filter.");
    expect(mocks.call.mock.calls.filter(([command]) => command === "list_remote_playlist_bindings")).toHaveLength(bindingReads);
  });

  it("opens a saved browser import only after selecting its Library playlist", async () => {
    delayedBrowserImports = Promise.resolve([latestImport, oldImport]);
    holdStartupCalls = true;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await refreshSidebarBrowserImports();
    expect(document.body.textContent).toContain("Saved Recording");
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.body.textContent).toContain("Latest Saved Import");
    expect(mocks.call).not.toHaveBeenCalledWith("load_browser_import", { id: "snapshot-new" });
    const beatport = sourceGroup("beatport");
    beatport.open = false;
    (beatport.querySelector("summary") as HTMLElement).click();
    expect(beatport.open).toBe(true);
    importedPlaylistCard("Latest Saved Import").click();
    await settle();
    expect(document.body.textContent).toContain("Latest Saved Import");
    expect(document.querySelector('[aria-label="Chart provider"]')).toBeNull();
    expect(document.querySelector('[aria-label="Chart genre"]')).toBeNull();
    const dateFilter = document.querySelector('[aria-label="Imported playlist version"]') as HTMLSelectElement;
    expect([...dateFilter.options].map((option) => option.textContent)).toContain("All import dates");
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: "snapshot-new" });
    expect(document.querySelector(".chart-source-actions")?.textContent).not.toContain("Open source");
    document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-menu button")].find(button => menuActionText(button).includes("Open source to refresh"))!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: latestImport.sourceUrl });
  });

  it("opens an imported collection source from its sidebar menu without triggering a reimport", async () => {
    delayedBrowserImports = Promise.resolve([latestImport, oldImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await refreshSidebarBrowserImports();
    const imported = importedPlaylistCard("Latest Saved Import");
    imported.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 160 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Saved Collection"]')!;
    const openSource = [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button).includes("Open source to refresh"))!;
    expect(openSource).toBeTruthy();
    openSource.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: latestImport.sourceUrl });
    expect(mocks.call.mock.calls.some(([command]) => command === "request_browser_import" || command === "import_browser_snapshot")).toBe(false);
  });

  it("opens the original URL for a directly browsed chart snapshot", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const beatportBrowse = document.querySelector<HTMLDetailsElement>(".source-heading.beatport")?.parentElement?.querySelector<HTMLButtonElement>(".browse-source");
    beatportBrowse!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".chart-toolbar button")].find((button) => button.textContent?.includes("Browse Top 100"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')!.click(); await settle();
    const openSource = [...document.querySelectorAll<HTMLButtonElement>(".playlist-menu button")].find(button => menuActionText(button).includes("Open source to refresh"))!;
    expect(menuActionText(openSource)).toContain("Open source to refresh");
    openSource.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: chart.sourceUrl });
    expect(mocks.call.mock.calls.some(([command]) => command === "request_browser_import" || command === "import_browser_snapshot")).toBe(false);
  });

  it("opens Shazam presets from the sidebar without querying chart-only commands", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    mocks.call.mockClear();
    const group = sourceGroup("shazam");
    group.open = true;
    group.querySelector<HTMLButtonElement>('[aria-label="Browse Shazam charts"]')!.click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Shazam");
    expect(document.querySelector('[aria-label="Shazam presets"]')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => ["list_genres", "get_chart"].includes(String(command)))).toBe(false);
  });

  it("imports the default Shazam world chart and Back returns to its provider presets", async () => {
    const url = "https://www.shazam.com/charts/top-200/world";
    directBrowserImport = {
      ...latestImport, id: "snapshot-shazam-preset", provider: "shazam", kind: "chart",
      title: "Shazam World Top 200", sourceUrl: url, collectionId: "collection-shazam-preset",
    };
    delayedBrowserImports = Promise.resolve([directBrowserImport, latestImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const group = sourceGroup("shazam");
    group.open = true;
    group.querySelector<HTMLButtonElement>('[aria-label="Browse Shazam charts"]')!.click();
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('.presets button.primary')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("import_collection_url", { url });
    expect(mocks.call).toHaveBeenCalledWith("list_browser_imports");
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: "snapshot-shazam-preset" });
    expect(mocks.call.mock.calls.some(([command]) => command === "import_chart" || command === "request_browser_import")).toBe(false);
    expect(document.querySelector("h1")?.textContent).toBe("Shazam World Top 200");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Shazam");
    expect(document.querySelector('[aria-label="Shazam presets"]')).not.toBeNull();
  });

  it("opens Bandcamp discovery from its preset without treating the destination as an import", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const group = sourceGroup("bandcamp");
    group.open = true;
    group.querySelector<HTMLButtonElement>('[aria-label="Browse Bandcamp music"]')!.click();
    await settle();
    mocks.call.mockClear();
    [...document.querySelectorAll<HTMLButtonElement>('.presets button')]
      .find(button => button.textContent?.includes("Electronic"))!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://bandcamp.com/discover/electronic" });
    expect(mocks.call.mock.calls.some(([command]) => ["import_chart", "import_collection_url"].includes(String(command)))).toBe(false);
  });

  it("requeries an exact prefix when a cached row was deleted during refresh", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    mocks.call.mockClear();
    const original = mocks.call.getMockImplementation()!;
    let refreshPrefixes = 0;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_track_prefix" && refreshPrefixes++ === 0) {
        return {rows:[
          {listingId:savedTrack.id,playlistPosition:null},
          {listingId:secondTrack.id,playlistPosition:null},
        ],total:2,limit:2};
      }
      return original(command, args);
    });
    // The cached UI still contains savedTrack, but the database no longer does.
    savedChartTracks = [secondTrack];
    eventHandlers.get("automation-library-changed")?.({event:"automation-library-changed",id:7,payload:{}});
    await settle();
    const rows = [...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean);
    expect(refreshPrefixes).toBeGreaterThanOrEqual(2);
    expect(rows.join(" ")).toContain("Another Recording");
    expect(rows.join(" ")).not.toContain("Saved Recording");
    expect(mocks.call.mock.calls.some(([name])=>name==="list_tracks")).toBe(false);
  });

  it("keeps the filtered sorted page exact after library and playlist events", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "recording";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("Another Recording"),expect.stringContaining("Saved Recording")]));
    mocks.call.mockClear();
    const updated={...secondTrack,title:"Aardvark Recording"};
    savedChartTracks=[savedTrack,updated];
    eventHandlers.get("library-updated")?.({event:"library-updated",id:1,payload:updated});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix",expect.objectContaining({view:"library",query:"recording",sortField:"title",direction:"asc",limit:2}));
    expect(mocks.call.mock.calls.some(([name])=>name==="list_tracks")).toBe(false);
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean)[0]).toContain("Aardvark Recording");
    mocks.call.mockClear();
    eventHandlers.get("playlist-sync")?.({event:"playlist-sync",id:2,payload:{ok:true}});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix",expect.objectContaining({view:"library",query:"recording",sortField:"title",direction:"asc",limit:2}));
    expect(mocks.call.mock.calls.some(([name])=>name==="list_tracks")).toBe(false);
    expect(mocks.call.mock.calls.filter(([name])=>name==="get_tracks").every(([,args])=>(args as {ids:string[]}).ids.length<=200)).toBe(true);
  });

  it("rejects an exact-prefix response after the active filter changes", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "recording";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const original = mocks.call.getMockImplementation()!;
    let finishPrefix: ((value:{rows:{listingId:string;playlistPosition:number|null}[];total:number;limit:number})=>void)|undefined;
    mocks.call.mockImplementation(async (command,args) => {
      if(command==="list_track_prefix") return await new Promise(resolve=>{finishPrefix=resolve;});
      return original(command,args);
    });
    eventHandlers.get("automation-library-changed")?.({event:"automation-library-changed",id:3,payload:{}});
    await settle();
    filter.value = "another";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    finishPrefix?.({rows:[{listingId:savedTrack.id,playlistPosition:null},{listingId:secondTrack.id,playlistPosition:null}],total:2,limit:2});
    await settle();
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean).join(" ")).toContain("Another Recording");
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean).join(" ")).not.toContain("Saved Recording");
  });

  it("carries earlier dirty tracks into a coalesced newer library refresh", async () => {
    savedChartTracks=[savedTrack,secondTrack];
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    const filter=document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value="recording";filter.dispatchEvent(new Event("input",{bubbles:true}));
    const sort=document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value="title";sort.dispatchEvent(new Event("change",{bubbles:true}));
    await settle();
    mocks.call.mockClear();
    const original=mocks.call.getMockImplementation()!;
    let finishFirst:((value:{rows:{listingId:string;playlistPosition:number|null}[];total:number;limit:number})=>void)|undefined;
    let prefixCalls=0;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="list_track_prefix"&&prefixCalls++===0)return await new Promise(resolve=>{finishFirst=resolve;});
      return original(command,args);
    });
    const updatedFirst={...savedTrack,title:"Zebra Recording"};
    savedChartTracks=[updatedFirst,secondTrack];
    eventHandlers.get("library-updated")?.({event:"library-updated",id:4,payload:updatedFirst});
    await settle();
    const updatedSecond={...secondTrack,title:"Aardvark Recording"};
    savedChartTracks=[updatedFirst,updatedSecond];
    eventHandlers.get("library-updated")?.({event:"library-updated",id:5,payload:updatedSecond});
    await settle();
    finishFirst?.({rows:[{listingId:savedTrack.id,playlistPosition:null},{listingId:secondTrack.id,playlistPosition:null}],total:2,limit:2});
    await settle();
    const visible=[...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean);
    expect(prefixCalls).toBe(2);
    expect(visible[0]).toContain("Aardvark Recording");
    expect(visible[1]).toContain("Zebra Recording");
    expect(mocks.call.mock.calls.some(([name])=>name==="list_tracks")).toBe(false);
  });

  it("rebuilds the exact loaded prefix when metadata changes during load more", async () => {
    const longTracks:Track[]=Array.from({length:201},(_,index)=>({...savedTrack,id:`recording-page-${index}`,providerId:String(index),sourceUrl:`https://example.test/track/${index}`,title:`Track ${String(index).padStart(4,"0")}`}));
    savedChartTracks=longTracks;
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    const sort=document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value="title";sort.dispatchEvent(new Event("change",{bubbles:true}));
    await settle();
    mocks.call.mockClear();
    const original=mocks.call.getMockImplementation()!;
    let finishPage:((value:{tracks:Track[];total:number;offset:number;limit:number})=>void)|undefined;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="list_tracks_page"&&args?.offset===200)return await new Promise(resolve=>{finishPage=resolve;});
      return original(command,args);
    });
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.trim()==="Load more tracks")!.click();
    await settle();
    const updated={...longTracks[200],title:"0000 Updated Track"};
    savedChartTracks=[...longTracks.slice(0,200),updated];
    eventHandlers.get("library-updated")?.({event:"library-updated",id:6,payload:updated});
    await settle();
    finishPage?.({tracks:[longTracks[200]],total:201,offset:200,limit:200});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix",expect.objectContaining({view:"library",sortField:"title",limit:201}));
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent?.trim()).filter(Boolean)[0]).toContain("0000 Updated Track");
    expect([...document.querySelectorAll<HTMLButtonElement>("button")].some(button=>button.textContent?.trim()==="Load more tracks")).toBe(false);
    expect(mocks.call.mock.calls.some(([name])=>name==="list_tracks")).toBe(false);
  });

  it("uses global sort availability across pages and rejects stale query availability", async () => {
    const longTracks: Track[] = Array.from({ length: 201 }, (_, index) => ({
      ...savedTrack,
      id: `sort-page-${index}`,
      providerId: String(index),
      sourceUrl: `https://example.test/track/${index}`,
      title: `Track ${String(index).padStart(4, "0")}`,
      ...(index === 200 ? { bpm: 128 } : {}),
    }));
    savedChartTracks = longTracks;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    const original = mocks.call.getMockImplementation()!;
    const availability = (overrides: Partial<TrackSortAvailability> = {}): TrackSortAvailability => ({
      sourceOrder: true, title: true, artist: true, album: true, duration: true,
      releaseDate: false, dateAdded: false, bpm: false, key: false, plays: false, views: false,
      ...overrides,
    });
    let finishStale: ((value: { tracks: Track[]; total: number; offset: number; limit: number; sortAvailability: TrackSortAvailability }) => void) | undefined;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_tracks_page") {
        const query = String(args?.query ?? "");
        if (query === "needle") return await new Promise(resolve => { finishStale = resolve; });
        if (query === "other") {
          const result = await original(command, args) as { tracks: Track[]; total: number; offset: number; limit: number };
          return { ...result, sortAvailability: availability({ bpm: false, plays: true }) };
        }
        const result = await original(command, args) as { tracks: Track[]; total: number; offset: number; limit: number };
        return { ...result, sortAvailability: availability({ bpm: true, plays: false }) };
      }
      return original(command, args);
    });
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    const bpm = sort.querySelector<HTMLOptionElement>('option[value="bpm"]')!;
    const plays = sort.querySelector<HTMLOptionElement>('option[value="plays"]')!;
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({ offset: 0, limit: 200 }));
    expect(bpm.disabled).toBe(false);
    expect(plays.disabled).toBe(true);
    expect(plays.textContent).toContain("unavailable");

    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "needle";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(plays.disabled).toBe(false); // The prior query's map is cleared while the new page is pending.
    filter.value = "other";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(bpm.disabled).toBe(true);
    expect(plays.disabled).toBe(false);
    finishStale?.({
      tracks: [], total: 0, offset: 0, limit: 200,
      sortAvailability: availability({ bpm: true, plays: false }),
    });
    await settle();
    expect(bpm.disabled).toBe(true);
    expect(plays.disabled).toBe(false);
  });

  it("holds the prior App rows while BPM sort is pending, then refetches and replaces them after readiness", async () => {
    const priorRows: Track[] = [
      { ...savedTrack, id: "prior-a", title: "Needle prior A" },
      { ...savedTrack, id: "prior-b", title: "Needle prior B" },
    ];
    const bpmRows: Track[] = [
      { ...savedTrack, id: "bpm-low", title: "Needle low BPM", bpm: 90 },
      { ...savedTrack, id: "bpm-high", title: "Needle high BPM", bpm: 130 },
    ];
    savedChartTracks = priorRows;
    const original = mocks.call.getMockImplementation()!;
    let bpmRequests = 0;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_tracks_page" && args?.view === "library") {
        const page = { offset: Number(args.offset ?? 0), limit: Number(args.limit ?? 200) };
        if (args.query === "needle" && args.sortField === "bpm") {
          bpmRequests++;
          if (bpmRequests === 1) {
            return { tracks: [], total: 0, ...page, sortPending: true, sortAvailabilityPending: true };
          }
          return {
            tracks: bpmRows, total: bpmRows.length, ...page,
            sortPending: false, sortAvailabilityPending: false,
            sortAvailability: { sourceOrder:true,title:true,artist:true,album:true,duration:true,releaseDate:true,dateAdded:true,bpm:true,key:true,plays:true,views:true },
          };
        }
        if (args.query === "needle") {
          return {
            tracks: priorRows, total: priorRows.length + 1, ...page,
            sortAvailabilityPending: false,
            sortAvailability: { sourceOrder:true,title:true,artist:true,album:true,duration:true,releaseDate:true,dateAdded:true,bpm:true,key:true,plays:true,views:true },
          };
        }
      }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "needle";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "bpm";
    sort.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(bpmRequests).toBe(1);
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ")).toContain("Needle prior A");
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ")).toContain("Needle prior B");
    expect(document.body.textContent).toContain("Preparing results… Showing the previous view.");
    const loadMore = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Preparing sort…"));
    expect(loadMore?.disabled).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({ view:"library",query:"needle",sortField:"bpm",limit:200,offset:0 }));

    eventHandlers.get("sort-availability-updated")?.({ event:"sort-availability-updated", id:41, payload:{} });
    await settle();
    expect(bpmRequests).toBe(2);
    expect(lastCallOf("list_tracks_page")).toEqual(["list_tracks_page", expect.objectContaining({ view:"library",query:"needle",sortField:"bpm",limit:200,offset:0 })]);
    const visible = [...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ");
    expect(visible).toContain("Needle low BPM");
    expect(visible).toContain("Needle high BPM");
    expect(visible.indexOf("Needle low BPM")).toBeLessThan(visible.indexOf("Needle high BPM"));
    expect(visible).not.toContain("Needle prior A");
    expect(document.body.textContent).not.toContain("Preparing results… Showing the previous view.");
    expect(document.body.contains(loadMore!)).toBe(false);
    expect([...document.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.includes("Preparing sort…"))).toBe(false);
  });

  it("discards an old pending-sort refresh after the App query changes", async () => {
    const initial: Track[] = [{ ...savedTrack, id:"query-old", title:"Needle original" }];
    const newer: Track[] = [{ ...savedTrack, id:"query-new", title:"Other filter result" }];
    savedChartTracks = initial;
    const original = mocks.call.getMockImplementation()!;
    let finishOldReady: ((value: unknown) => void) | undefined;
    let bpmRequests = 0;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_tracks_page" && args?.view === "library") {
        const page = { offset:Number(args.offset ?? 0), limit:Number(args.limit ?? 200) };
        if (args.sortField === "bpm" && args.query === "needle") {
          bpmRequests++;
          if (bpmRequests === 1) return { tracks:[], total:0, ...page, sortPending:true, sortAvailabilityPending:true };
          return await new Promise(resolve => { finishOldReady = resolve; });
        }
        if (args.query === "needle") return { tracks:initial,total:initial.length,...page,sortAvailabilityPending:false,sortAvailability:{sourceOrder:true,title:true,artist:true,album:true,duration:true,releaseDate:true,dateAdded:true,bpm:true,key:true,plays:true,views:true} };
        if (args.query === "other") return { tracks:newer,total:newer.length,...page,sortAvailabilityPending:false,sortAvailability:{sourceOrder:true,title:true,artist:true,album:true,duration:true,releaseDate:true,dateAdded:true,bpm:true,key:true,plays:true,views:true} };
      }
      return original(command,args);
    });
    const instance = mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    const filter=document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value="needle";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    const sort=document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value="bpm";sort.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(bpmRequests).toBe(1);
    eventHandlers.get("sort-availability-updated")?.({event:"sort-availability-updated",id:42,payload:{}});
    await settle();
    expect(finishOldReady).toBeTruthy();
    filter.value="other";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    finishOldReady!({tracks:[{...savedTrack,id:"stale-bpm",title:"Stale needle BPM",bpm:100}],total:1,offset:0,limit:200,sortAvailabilityPending:false,sortAvailability:{sourceOrder:true,title:true,artist:true,album:true,duration:true,releaseDate:true,dateAdded:true,bpm:true,key:true,plays:true,views:true}});
    await settle();
    const visible=[...document.querySelectorAll(".shared-track-list > li")].map(row=>row.textContent??"").join(" ");
    expect(visible).toContain("Other filter result");
    expect(visible).not.toContain("Stale needle BPM");
    expect(lastCallOf("list_tracks_page")).toEqual(["list_tracks_page",expect.objectContaining({view:"library",query:"other",limit:200,offset:0})]);
  });

  it("refreshes only pending availability for the active query and preserves its loaded rows and cursor", async () => {
    const needleRows: Track[] = [
      { ...savedTrack, id: "needle-1", title: "Needle One", bpm: undefined },
      { ...savedTrack, id: "needle-2", title: "Needle Two", bpm: undefined },
    ];
    const otherRows: Track[] = [
      { ...savedTrack, id: "other-1", title: "Other One", bpm: undefined },
      { ...savedTrack, id: "other-2", title: "Other Two", bpm: undefined },
      { ...savedTrack, id: "other-3", title: "Other Three", bpm: undefined },
    ];
    savedChartTracks = [...needleRows, ...otherRows];
    const original = mocks.call.getMockImplementation()!;
    const availability = (overrides: Partial<TrackSortAvailability> = {}): TrackSortAvailability => ({
      sourceOrder: true, title: true, artist: true, album: false, duration: false,
      releaseDate: false, dateAdded: false, bpm: false, key: false, plays: false, views: false,
      ...overrides,
    });
    let finishNeedleAvailability: ((page: {tracks:Track[];total:number;offset:number;limit:number;sortAvailabilityPending?:boolean;sortAvailability?:TrackSortAvailability|null}) => void) | undefined;
    let finishOtherAvailability: ((page: {tracks:Track[];total:number;offset:number;limit:number;sortAvailabilityPending?:boolean;sortAvailability?:TrackSortAvailability|null}) => void) | undefined;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_tracks_page") {
        const query = String(args?.query ?? ""), offset = Number(args?.offset ?? 0), limit = Number(args?.limit ?? 200);
        if (query === "needle" && limit === 200) return { tracks: needleRows, total: needleRows.length, offset, limit, sortAvailabilityPending: true };
        if (query === "other" && limit === 200 && offset === 0) return { tracks: otherRows.slice(0, 2), total: otherRows.length, offset, limit, sortAvailabilityPending: true };
        if (query === "needle" && limit === 1) return await new Promise(resolve => { finishNeedleAvailability = resolve; });
        if (query === "other" && limit === 1) return await new Promise(resolve => { finishOtherAvailability = resolve; });
      }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect(mocks.call.mock.calls.some(([command, args]) => command === "list_tracks_page" && (args as Record<string, unknown>)?.limit === 1)).toBe(false);
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "needle";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    eventHandlers.get("sort-availability-updated")?.({ event: "sort-availability-updated", id: 1, payload: {} });
    await settle();
    expect(finishNeedleAvailability).toBeTruthy();

    filter.value = "other";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const table = document.querySelector<HTMLDivElement>(".track-scroll")!;
    table.scrollTop = 48;
    eventHandlers.get("sort-availability-updated")?.({ event: "sort-availability-updated", id: 2, payload: {} });
    await settle();
    expect(finishOtherAvailability).toBeTruthy();

    finishNeedleAvailability!({ tracks: [], total: 0, offset: 0, limit: 1, sortAvailabilityPending: false, sortAvailability: availability({ bpm: true }) });
    await settle();
    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);

    finishOtherAvailability!({ tracks: [{ ...savedTrack, id: "must-not-replace-rows", title: "Metadata response row" }], total: 999, offset: 0, limit: 1, sortAvailabilityPending: false, sortAvailability: availability({ bpm: false, plays: true }) });
    await settle();
    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(true);
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ")).toContain("Other One");
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ")).toContain("Other Two");
    expect(document.body.textContent).not.toContain("Metadata response row");
    expect(document.querySelectorAll(".shared-track-list > li")).toHaveLength(2);
    expect(table.scrollTop).toBe(48);
    expect([...document.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.trim() === "Load more tracks")).toBe(true);
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Load more tracks")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({ query: "other", offset: 2, limit: 200 }));
  });

  it("keeps a pending availability refresh valid while loading the next page of the same query", async () => {
    const rows: Track[] = Array.from({ length: 3 }, (_, index) => ({
      ...savedTrack, id: `pending-page-${index + 1}`, title: `Pending Page ${index + 1}`, bpm: undefined,
    }));
    savedChartTracks = rows;
    const original = mocks.call.getMockImplementation()!;
    const availability: TrackSortAvailability = {
      sourceOrder: true, title: true, artist: true, album: false, duration: false,
      releaseDate: false, dateAdded: false, bpm: false, key: false, plays: false, views: false,
    };
    let finishAvailability: ((page: {tracks:Track[];total:number;offset:number;limit:number;sortAvailabilityPending?:boolean;sortAvailability?:TrackSortAvailability|null}) => void) | undefined;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "list_tracks_page" && args?.view === "library") {
        const offset = Number(args?.offset ?? 0), limit = Number(args?.limit ?? 200);
        if (limit === 1) return await new Promise(resolve => { finishAvailability = resolve; });
        if (offset === 0) return { tracks: rows.slice(0, 2), total: 3, offset, limit, sortAvailabilityPending: true };
        if (offset === 2) return { tracks: rows.slice(2), total: 3, offset, limit };
      }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    eventHandlers.get("sort-availability-updated")?.({ event: "sort-availability-updated", id: 3, payload: {} });
    await settle();
    expect(finishAvailability).toBeTruthy();

    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Load more tracks")!.click();
    await settle();
    expect(document.querySelectorAll(".shared-track-list > li")).toHaveLength(3);
    const table = document.querySelector<HTMLDivElement>(".track-scroll")!;
    table.scrollTop = 34;
    finishAvailability!({ tracks: [], total: 0, offset: 0, limit: 1, sortAvailabilityPending: false, sortAvailability: availability });
    await settle();

    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(true);
    expect([...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "").join(" ")).toContain("Pending Page 3");
    expect(document.querySelectorAll(".shared-track-list > li")).toHaveLength(3);
    expect(table.scrollTop).toBe(34);
    expect([...document.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.trim() === "Load more tracks")).toBe(false);
  });

  it("opens the most populated collection by default and shows its recordings", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    collectionCard().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.body.textContent).toContain("Existing Collection Member");
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({
      view: "collections", collectionId: "collection-full",limit:200,offset:0,
    }));
  });

  it("loads and persists sidebar section assignments when creating a playlist in a section", async () => {
    sidebarLayoutState = {
      sections: [{ id: "section-mood", name: "Mood" }],
      assignments: { "collection-empty": "section-mood" },
    };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_sidebar_layout");
    const sections = document.querySelector('[aria-label="Playlist sections"]')!;
    expect(sections.textContent).toContain("Mood");
    expect(sections.textContent).toContain("Empty Collection");
    sections.querySelector<HTMLButtonElement>('[aria-label="Create playlist in Mood"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "New playlist" });
    expect(mocks.call).toHaveBeenCalledWith("save_sidebar_layout", {
      layout: { sections: [{ id: "section-mood", name: "Mood" }], assignments: { "collection-empty": "section-mood", "collection-new": "section-mood" } },
    });
    expect(sidebarLayoutSaves[sidebarLayoutSaves.length - 1]?.assignments["collection-new"]).toBe("section-mood");
    expect(document.querySelector("h1")?.textContent).toBe("New playlist");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Playlist name"]')?.value).toBe("New playlist");
  });

  it("renames an imported playlist from its sidebar context menu and persists the header name", async () => {
    delayedBrowserImports = Promise.resolve([latestImport, oldImport]);
    sidebarLayoutState = { sections: [], assignments: {}, names: {} };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await openLibraryImport();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find((button) => button.textContent?.trim() === "Close")!.click();
    await settle();
    const sourcePlaylist = importedPlaylistCard("Latest Saved Import") ?? collectionCard("Saved Collection");
    sourcePlaylist.click();
    await settle();
    const renameTarget = importedPlaylistCard("Latest Saved Import") ?? collectionCard("Saved Collection");
    renameTarget.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Saved Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Rename")!.click();
    await settle();
    await saveSidebarRename("Sunset Charts");
    expect(mocks.call).toHaveBeenCalledWith("rename_collection", { collectionId: "collection-full", name: "Sunset Charts" });
    expect(sidebarLayoutState.names?.["collection-full"]).toBe("Sunset Charts");
    expect(sidebarLayoutSaves[sidebarLayoutSaves.length - 1]?.names?.["collection-full"]).toBe("Sunset Charts");
    expect(document.querySelector("main > header h1")?.textContent).toBe("Sunset Charts");
    expect([...document.querySelectorAll(".playlist-card")].some((button) => button.textContent?.includes("Sunset Charts"))).toBe(true);
  });

  it("renames the mutable favorites playlist through its sidebar context menu", async () => {
    sidebarLayoutState = { sections: [], assignments: {}, names: {} };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const favorite = [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find((button) => button.textContent?.includes("Liked Songs"))!;
    favorite.click();
    await settle();
    const activeFavorite = [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find((button) => button.textContent?.includes("Liked Songs"))!;
    activeFavorite.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 310, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Liked Songs"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Rename")!.click();
    await settle();
    await saveSidebarRename("Starred Tracks");
    expect(mutableFavoritesCollection.name).toBe("Starred Tracks");
    expect(document.querySelector("main > header h1")?.textContent).toBe("Starred Tracks");
    expect([...document.querySelectorAll(".playlist-card")].some((button) => button.textContent?.includes("Starred Tracks"))).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("rename_collection", { collectionId: "favorites", name: "Starred Tracks" });
  });

  it("removes a built-in radio station through the sidebar context menu", async()=>{
    radioStationState=[{id:"main",name:"Main Mix",streamUrl:"https://radio.example/main",sourceUrl:"https://radio.example",provider:"radio_paradise",isCustom:false}];
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    const station=[...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Main Mix"))!;
    station.dispatchEvent(new MouseEvent("contextmenu",{bubbles:true,cancelable:true,clientX:320,clientY:190}));
    await settle();
    const menu=document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for radio station Main Mix"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find(button=>menuActionText(button)==="Remove station")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("remove_radio_station",{stationId:"main"});
    expect(radioStationState).toHaveLength(0);
    expect(document.querySelector(".radio-section")?.textContent).not.toContain("Main Mix");
  });

  it("renames a radio station from the sidebar context menu and updates its saved station record", async () => {
    radioStationState = [{
      id: "custom:night", name: "Night Drive", streamUrl: "https://radio.example/night", sourceUrl: "https://radio.example/night", provider: "custom", isCustom: true,
    }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const station = [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find((button) => button.textContent?.includes("Night Drive"))!;
    station.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 320, clientY: 190 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for radio station Night Drive"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Rename")!.click();
    await settle();
    await saveSidebarRename("After Hours");
    expect(mocks.call).toHaveBeenCalledWith("rename_radio_station", { stationId: "custom:night", name: "After Hours" });
    expect(radioStationState[0]?.name).toBe("After Hours");
    expect([...document.querySelectorAll(".radio-section .playlist-card")].some((button) => button.textContent?.includes("After Hours"))).toBe(true);
  });

  it("creates a playlist immediately from the sidebar create action and opens its rename field", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Create playlist"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "New playlist" });
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({ view: "collections", collectionId: "collection-new",limit:200,offset:0 }));
    expect(document.querySelector("h1")?.textContent).toBe("New playlist");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Playlist name"]')?.value).toBe("New playlist");
  });

  it("imports a chart URL into the already-open playlist when using Import tracks", async () => {
    const url = "https://www.shazam.com/charts/top-200/world";
    directBrowserImport = {
      ...latestImport, id: "snapshot-into-playlist", provider: "shazam", kind: "chart",
      title: "Shazam tracks", sourceUrl: url, collectionId: "collection-imported-source",
    };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard("Empty Collection").click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Empty Collection");
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Import tracks")!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Chart, playlist or album URL"]')!;
    input.value = url; input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button => button.textContent?.trim() === "Import URL")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("import_collection_url", { url });
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: "snapshot-into-playlist" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-empty", tracks: savedChartTracks });
    expect(document.querySelector("h1")?.textContent).toBe("Empty Collection");
    expect(document.querySelector(".browser-capture")).toBeNull();
  });

  it("uses modifier clicks for range and toggle selection without starting extra previews", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const thirdTrack: Track = { ...savedTrack, id: "recording-3", title: "Third Recording" };
    savedChartTracks = [savedTrack, secondTrack, thirdTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")];
    expect(rows).toHaveLength(3);
    rows[0].dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const previews = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    rows[2].dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    await settle();
    expect([...document.querySelectorAll(".shared-track-list > li:not(.virtual-spacer).selected")]).toHaveLength(3);
    expect([...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].some(button => button.textContent?.trim() === "Merge 3")).toBe(true);
    rows[1].dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await settle();
    expect([...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].some(button => button.textContent?.trim() === "Merge 2")).toBe(true);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previews);
  });

  it("numbers multiple recordings by row position when every chart rank is one", async () => {
    savedChartTracks = [
      { ...savedTrack, id: "rank-one-a", title: "First Ranked One", rank: 1 },
      { ...savedTrack, id: "rank-one-b", title: "Second Ranked One", rank: 1 },
      { ...savedTrack, id: "rank-one-c", title: "Third Ranked One", rank: 1 },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect([...document.querySelectorAll(".shared-track-list > li:not(.virtual-spacer) .track-position")].map((node) => node.textContent?.trim())).toEqual(["1", "2", "3"]);
  });

  it("scrolls the selected shared row into view during keyboard navigation", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollIntoView");
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: scrollIntoView });
    try {
      document.querySelector<HTMLElement>(".shared-track-list > li")!.click();
      await settle();
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
      await settle();
      expect(document.querySelector(".shared-track-list > li.selected")?.textContent).toContain(secondTrack.title);
      expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" });
    } finally {
      if (original) Object.defineProperty(HTMLElement.prototype, "scrollIntoView", original);
      else delete (HTMLElement.prototype as unknown as { scrollIntoView?: () => void }).scrollIntoView;
    }
  });

  it("opens the row context menu without playing or changing the selected recording", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")];
    rows[0].dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const startCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    expect(rows[0].classList.contains("selected")).toBe(true);
    rows[1].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 420, clientY: 220 }));
    await settle();
    expect(document.querySelector("dialog.track-menu.context-menu")).not.toBeNull();
    expect(rows[0].classList.contains("selected")).toBe(true);
    expect(rows[1].classList.contains("selected")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(startCount);
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(1);
  });

  it("opens row More at the button edge without changing playback or selection", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")];
    const menuButton = rows[1].querySelector<HTMLButtonElement>(".more-trigger")!;
    vi.spyOn(menuButton, "getBoundingClientRect").mockReturnValue({
      x: 444, y: 260, left: 444, top: 260, right: 472, bottom: 288, width: 28, height: 28,
      toJSON: () => ({}),
    } as DOMRect);
    const selectedBefore = [...document.querySelectorAll(".shared-track-list > li.selected")];
    const previewCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    const playCount = vi.mocked(HTMLMediaElement.prototype.play).mock.calls.length;
    menuButton.click();
    await settle();
    const menu = document.querySelector<HTMLDialogElement>("dialog.track-menu.context-menu")!;
    expect(menu).not.toBeNull();
    expect(menu.style.left).toBe("472px");
    expect(menu.style.top).toBe("288px");
    expect([...document.querySelectorAll(".shared-track-list > li.selected")]).toEqual(selectedBefore);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewCount);
    expect(vi.mocked(HTMLMediaElement.prototype.play).mock.calls).toHaveLength(playCount);
  });

  it("renames a playlist and removes checked tracks from that playlist only", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("button")].find((item) => item.textContent === "Rename")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const rename = document.querySelector('[aria-label="Playlist name"]') as HTMLInputElement;
    rename.value = "Weekend Set";
    rename.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll("button")].find((item) => item.textContent === "Save name")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("rename_collection", { collectionId: "collection-full", name: "Weekend Set" });
    await chooseTrackAction("Select for batch actions", "Existing Collection Member");
    expect(document.querySelector('[aria-label="Select occurrence 1: Existing Collection Member for playlist"]')).not.toBeNull();
    [...document.querySelectorAll("button")].find((item) => item.textContent?.includes("Remove 1 from playlist"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("remove_from_collection", {
      collectionId: "collection-full", trackIds: [collectionTrack.id],
    });
    expect(document.querySelector(".shared-track-list")?.textContent).not.toContain("Existing Collection Member");
  });

  it("removes one recording directly from its active playlist only", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click(); await settle();
    mocks.call.mockClear();
    await chooseTrackAction("Remove from playlist", "Existing Collection Member");
    expect(mocks.call).toHaveBeenCalledWith("remove_from_collection", { collectionId: "collection-full", trackIds: [collectionTrack.id] });
    expect(mocks.call.mock.calls.filter(([command]) => command === "remove_from_collection")).toHaveLength(1);
    expect(document.querySelector(".shared-track-list")?.textContent).not.toContain("Existing Collection Member");
    expect(collectionCard("Empty Collection")).not.toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix", expect.objectContaining({view:"collections",collectionId:"collection-full",limit:1}));
    expect(mocks.call.mock.calls.some(([command]) => command === "list_tracks")).toBe(false);
  });

  it("moves a selected playlist occurrence by its original position under title sorting and filtering", async () => {
    delayedBrowserImports = Promise.resolve([]);
    mockCollectionRows = [
      {...collectionTrack,id:"playlist-z",title:"Zebra Recording",provenance:{playlistPosition:1}},
      {...collectionTrack,id:"playlist-middle",title:"Middle Recording",provenance:{playlistPosition:2}},
      {...collectionTrack,id:"playlist-a",title:"Aardvark Recording",provenance:{playlistPosition:3}},
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click();
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change", { bubbles: true }));
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "Aardvark";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    mocks.call.mockClear();
    const selectedRow = [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")]
      .find(row => row.textContent?.includes("Aardvark Recording"))!;
    selectedRow.click();
    await settle();
    await chooseTrackAction("Select for batch actions", "Aardvark Recording");
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.trim()==="Move up")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("move_playlist_entry", {collectionId:"collection-full",from:2,to:1});
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix", expect.objectContaining({view:"collections",collectionId:"collection-full",query:"Aardvark",sortField:"title",limit:1}));
    expect(mocks.call.mock.calls.some(([command]) => command === "list_tracks")).toBe(false);
    expect(document.querySelector(".shared-track-list")?.textContent).toContain("Aardvark Recording");
  });

  it("deletes and restores a playlist by stable ID without changing queued or playing audio", async () => {
    delayedBrowserImports = Promise.resolve([]);
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click(); await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((row) => row.textContent?.includes("Existing Collection Member"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Add to queue", "Existing Collection Member");
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    const queueCount = document.querySelectorAll(".queue-list article").length;
    const previewCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    expect(queueCount).toBeGreaterThan(0);
    expect(previewCount).toBe(1);
    expect(play).toHaveBeenCalledTimes(1);
    document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')!.click(); await settle();
    [...document.querySelectorAll("button")].find((button) => menuActionText(button) === "Delete playlist")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("delete_collection", { collectionId: "collection-full" });
    expect(document.body.textContent).toContain("Deleted playlist: Saved Collection");
    expect([...document.querySelectorAll("button")].some((button) => button.textContent?.trim() === "Undo deletion")).toBe(true);
    if (!document.querySelector(".playback-queue-panel")) { document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle(); }
    expect(document.querySelectorAll(".queue-list article")).toHaveLength(queueCount);
    expect(document.querySelector(".playback-queue-panel")?.textContent).toContain("Existing Collection Member");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Existing Collection Member");
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewCount);
    expect(play).toHaveBeenCalledTimes(1);
    [...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === "Undo deletion")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("restore_collection", { collectionId: "collection-full" });
    expect(mocks.call).toHaveBeenCalledWith("list_tracks_page", expect.objectContaining({ view: "collections", collectionId: "collection-full",limit:200,offset:0 }));
    expect(document.querySelector("h1")?.textContent).toBe("Saved Collection");
    expect(document.body.textContent).toContain("Existing Collection Member");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Existing Collection Member");
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewCount);
    expect(play).toHaveBeenCalledTimes(1);
  });

  it("deletes an inactive playlist from its sidebar menu without changing view or queue", async () => {
    delayedBrowserImports = Promise.resolve([]);
    savedChartTracks = [savedTrack, secondTrack];
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((row) => row.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Add to queue", "Another Recording");
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    const queueBeforeDelete = [...document.querySelectorAll(".queue-list article .track-copy strong")].map((node) => node.textContent?.trim());
    expect(queueBeforeDelete.length).toBeGreaterThan(0);
    const previewCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;

    rightClickPlaylist("Empty Collection"); await settle();
    expect(document.querySelector('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')).not.toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.playlist-menu button")].find((button) => menuActionText(button) === "Delete playlist")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("delete_collection", { collectionId: "collection-empty" });
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Saved Recording");
    expect([...document.querySelectorAll(".queue-list article .track-copy strong")].map((node) => node.textContent?.trim())).toEqual(queueBeforeDelete);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewCount);
    expect(play).toHaveBeenCalledTimes(1);

    [...document.querySelectorAll<HTMLElement>(".library-tools summary")].find((summary) => summary.textContent?.trim() === "Library options")!.click();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Deleted playlists")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_deleted_collections");
    expect(document.querySelector('[aria-label="Deleted playlists"]')?.textContent).toContain("Empty Collection");
    document.querySelector<HTMLButtonElement>('[aria-label="Restore Empty Collection"]')!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("restore_collection", { collectionId: "collection-empty" });
    expect([...document.querySelectorAll(".queue-list article .track-copy strong")].map((node) => node.textContent?.trim())).toEqual(queueBeforeDelete);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Saved Recording");
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewCount);
    expect(play).toHaveBeenCalledTimes(1);
  });

  it("creates a playlist from a selected chart track without changing playback or view", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Select for batch actions");
    const name = document.querySelector('[aria-label="New playlist for selection"]') as HTMLInputElement;
    name.value = "Friday Set";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll("button")].find((item) => item.textContent === "Create & add")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "Friday Set" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", {
      collectionId: "collection-new", tracks: [savedTrack],
    });
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
  });

  it("opens the active recording information without changing the view or restarting playback", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const resolveCalls = () => mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    expect(resolveCalls()).toBe(1);
    (document.querySelector(".click-info") as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    const info = document.querySelector(".playing-info");
    expect(info?.textContent).toContain("Saved Recording");
    expect(info?.textContent).toContain("Visible Artist");
    expect(info?.textContent).toContain("beatport");
    expect(resolveCalls()).toBe(1);
  });

  it("labels only matching analyzed BPM values and drops the estimate note after tag edits", async () => {
    const analyzedTrack: Track = {
      ...savedTrack,
      bpm: 124,
      provenance: { bpmAnalysis: { bpm: 124, source: "decoded audio", method: "10ms onset autocorrelation", confidence: 0.9 } },
    };
    savedChartTracks = [analyzedTrack];
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    expect(row.textContent).toContain("≈124 BPM");
    expect(row.querySelector(".track-title")?.getAttribute("title")).toContain("Estimated from audio");
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Track information");
    const bpmInfo = [...document.querySelectorAll<HTMLElement>(".playing-info dl div")].find((item) => item.querySelector("dt")?.textContent === "Tempo")!;
    expect(bpmInfo.textContent).toContain("estimated");
    await chooseTrackAction("Download options…");
    expect(document.querySelector("dialog.download-dialog .provider-group .candidate")).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector(".estimated-bpm-note")?.textContent).toContain("audio analysis estimate");
    const bpm = document.querySelector('[aria-label="Download tag BPM"]') as HTMLInputElement;
    bpm.value = "128";
    bpm.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(document.querySelector(".estimated-bpm-note")).toBeNull();
  });

  it("queues missing local analysis and merges progress/results without restarting playback", async () => {
    const localTrack: Track = { ...savedTrack, assetId: "asset-1", localAvailability: "available" };
    savedAssets = [{ id: "asset-1", recordingId: localTrack.id, path: "/music/saved.mp3", hash: "hash-1",
      sourceCodec: "mp3", finalCodec: "mp3", processing: "ready", track: localTrack }];
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const resolveCount = () => mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    expect(resolveCount()).toBe(1);

    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Library"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("button")].find((item) => item.textContent?.includes("Analyze missing BPM / key"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("analyze_local_assets", { assetIds: ["asset-1"], force: false });

    eventHandlers.get("analysis-progress")?.({ event: "analysis-progress", id: 2,
      payload: { assetId: "asset-1", status: "running" } });
    await settle();
    expect(resolveCount()).toBe(1);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");

    const analyzed: Track = { ...localTrack, bpm: 124, key: "Am", provenance: {
      bpmAnalysis: { bpm: 124, source: "decoded audio", method: "10ms onset autocorrelation", confidence: 0.9 },
      keyAnalysis: { wholeTrackKey: "Am", status: "complete", source: "decoded audio", method: "libkeyfinder distributed-window agreement", confidence: 0.88, version: "libkeyfinder-2.2.8" },
    } };
    eventHandlers.get("library-updated")?.({ event: "library-updated", id: 3, payload: analyzed });
    await settle();
    const updatedRow = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    expect(updatedRow.textContent).toContain("≈124 BPM");
    expect(updatedRow.textContent).toContain("≈Am");
    expect(updatedRow.querySelector(".track-title")?.getAttribute("title")).toContain("libkeyfinder distributed-window agreement");
    expect(updatedRow.textContent).toContain("124");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
    expect(resolveCount()).toBe(1);
  });

  it("requires an explicit field selection before applying catalog metadata in the download sheet", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Download options…");
    [...document.querySelectorAll("dialog summary")].find((item) => item.textContent?.includes("Edit tags"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    (document.querySelector(".download-tags button") as HTMLButtonElement).click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_recording_metadata", {
      track: savedTrack, releaseRef: null, recordingRef: undefined,
    });
    expect(document.body.textContent).toContain("Find catalog metadata");
    expect(mocks.call.mock.calls.some(([command]) => command === "update_track")).toBe(false);
    const field = document.querySelector('.metadata-review input[type="checkbox"]') as HTMLInputElement;
    field.checked = true;
    field.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect((document.querySelector(".metadata-review button.primary") as HTMLButtonElement).disabled).toBe(false);
    (document.querySelector(".metadata-review button.primary") as HTMLButtonElement).click();
    await settle();
    const update = mocks.call.mock.calls.find(([command]) => command === "update_track");
    expect(update?.[1]).toEqual({ track: { ...savedTrack, label: "Reviewed Label" } });
    expect(document.querySelector('[aria-label="Download tag label"]')).not.toBeNull();
  });

  it("keeps the collection context when a browser import changes the current view", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const nav = () => [...document.querySelectorAll("nav button")];
    collectionCard("Empty Collection").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.body.textContent).toContain("Existing Collection Member");

    const imported = { ...latestImport, id: "snapshot-event", title: "Event Import" };
    eventHandlers.get("browser-imported")?.({ event: "browser-imported", id: 1, payload: imported });
    await settle();
    expect(document.body.textContent).toContain("Event Import");

    nav().find((item) => item.textContent?.includes("Search"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    failNextCollectionLoad = true;
    collectionCard("Empty Collection").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.body.textContent).toContain("Existing Collection Member");
    expect(document.querySelector(".shared-track-list")?.textContent).not.toContain("Event Import");
  });

  it("does not restore saved Charts over a view chosen while startup requests are pending", async () => {
    let finishImports!: (value: BrowserImportResult[]) => void;
    delayedBrowserImports = new Promise((resolve) => { finishImports = resolve; });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const libraryButton = [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Library"));
    libraryButton!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    finishImports([latestImport, oldImport]);
    await settle();

    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelector(".shared-track-list")?.textContent).toContain("Saved Recording");
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(mocks.call).not.toHaveBeenCalledWith("load_browser_import", { id: "snapshot-new" });
  });

  it("restores playlist filters and Search queries with back and forward navigation", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const playlistFilter = document.querySelector('[aria-label="Filter tracks"]') as HTMLInputElement;
    playlistFilter.value = "Existing";
    playlistFilter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("nav button")].find((button) => button.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const query = document.querySelector('[aria-label="Search music"]') as HTMLInputElement;
    query.value = "late night";
    query.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch();
    await settle();
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Saved Collection");
    expect((document.querySelector('[aria-label="Filter tracks"]') as HTMLInputElement).value).toBe("Existing");
    (document.querySelector('[aria-label="Go forward"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Search");
    expect((document.querySelector('[aria-label="Search music"]') as HTMLInputElement).value).toBe("late night");
  });

  it.each(["back", "forward"] as const)("ignores %s while a route restoration is pending and preserves search navigation state", async (secondDirection) => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click();
    await settle();
    await selectSearchSources(["MusicBrainz"]);
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "deferred navigation";
    query.dispatchEvent(new Event("input", { bubbles: true }));
    const original = mocks.call.getMockImplementation()!;
    let deferSources = false;
    let finishSources: ((value: { id: string; name: string; kinds: string[]; enabled: boolean }[]) => void) | undefined;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources" && deferSources) {
        deferSources = false;
        return await new Promise<{ id: string; name: string; kinds: string[]; enabled: boolean }[]>(resolve => { finishSources = resolve; });
      }
      return original(command, args);
    });
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find(button => button.textContent?.includes("Settings"))!.click();
    await settle();
    deferSources = true;
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();
    await Promise.resolve();
    const back = document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!;
    const forward = document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!;
    expect(back.disabled).toBe(true);
    expect(forward.disabled).toBe(true);
    (secondDirection === "back" ? back : forward).click();
    expect(finishSources).toBeDefined();
    finishSources!([{ id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"], enabled: true }]);
    await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("deferred navigation");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("musicbrainz");
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();
    await settle();
    expect(document.querySelector(".settings")).not.toBeNull();
  });

  it("offers only supported online sources and can explicitly search My library", async () => {
    slskdConnectionEnabled = true;
    discogsConnectionEnabled = true;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find((button) => button.textContent?.includes("Search"))!.click();
    await settle();

    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    const options = [...picker.options];
    expect(options.map((option) => option.value)).toEqual(["all", "musicbrainz", "discogs", "youtube", "soundcloud", "soulseek"]);
    expect(options.map((option) => option.textContent).join(" ")).not.toMatch(/Beatport|Traxsource|Spotify/i);
    expect(options[0]?.textContent).toBe("All online sources");
    expect(picker.value).toBe("all");
    expect(document.querySelector('[aria-label="Online"]')?.getAttribute("aria-pressed")).toBe("true");

    const onlineQuery = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    onlineQuery.value = "Online test";
    onlineQuery.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch();
    await settle();
    const onlineIds = mocks.call.mock.calls
      .filter(([command, args]) => command === "search_source" && (args as { query?: string }).query === "Online test")
      .map(([, args]) => (args as { sourceIds: string[] }).sourceIds[0]);
    expect(onlineIds).toEqual(expect.arrayContaining(["musicbrainz", "discogs", "youtube", "soundcloud", "soulseek"]));

    await selectSearchSources(["My library"]);
    expect(document.querySelector('[aria-label="My library"]')?.getAttribute("aria-pressed")).toBe("true");
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Saved Recording";
    query.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Saved Recording", sourceIds: ["library"] }));
    expect(document.querySelector(".source-results")?.textContent).toContain("Saved Recording");
    expect(mocks.call.mock.calls.some(([command, args]) => command === "search_source" && (args as { query?: string; sourceIds?: string[] }).query === "Saved Recording" && (args as { sourceIds?: string[] }).sourceIds?.some((id) => id !== "library"))).toBe(false);
  });


  it("returns directly to the prior filtered Library when a sidebar account playlist is opened", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const playlistUrl = "https://open.spotify.com/playlist/11223344556677889900aa";
    const playlistTrack = { ...savedTrack, id: "sidebar-remote-track", provider: "spotify", providerId: "sidebar-remote-track", title: "Sidebar remote track" };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_sidebar_layout") return { sections: [], assignments: {}, showAccountPlaylists: { spotify: true } };
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "sidebar-account", accountRevisions: { spotify: 1 } };
      if (command === "list_remote_playlists" && args?.provider === "spotify") return { items: [{ id: "sidebar-list-item", title: "Sidebar remote mix", url: playlistUrl }], nextCursor: null };
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "sidebar-remote-mix", title: "Sidebar remote mix", tracks: [playlistTrack], nextCursor: null, total: 1, complete: true };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "preserve-this-library-query";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    sourceGroup("spotify").open = true;
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Spotify playlists"] .playlist-title')!.click();
    await settle();
    expect(document.querySelector(".remote-playlist-detail")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();
    await settle();
    await settle();

    expect(document.querySelector("main h1")?.textContent).toBe("Library");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')?.value).toBe("preserve-this-library-query");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.value).toBe("title");
    expect(document.querySelector(".remote-playlist-detail")).toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => ["play_tracks", "resolve_preview", "resolve_audio"].includes(String(command)))).toBe(false);
  });

  it("routes provider playlist browsing from the sidebar into the matching main Search scope and kind", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "playlist"], enabled: true },
        { id: "youtube", name: "YouTube", kinds: ["track", "playlist"], enabled: true },
      ];
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    for (const [source, label] of [["spotify", "Spotify"], ["youtube", "YouTube"]]) {
      if (document.querySelector("main h1")?.textContent !== "Library") {
        document.querySelector<HTMLButtonElement>('[aria-label="Library"]')?.click();
        await settle();
      }
      sourceGroup(source).open = true;
      await settle();
      document.querySelector<HTMLButtonElement>(`[aria-label="Search ${label} playlists"]`)!.click();
      await settle();
      expect(document.querySelector("main h1")?.textContent).toBe("Search");
      expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe(source);
      expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Playlists")?.getAttribute("aria-pressed")).toBe("true");
    }
  });

  it("restores a Spotify owner playlist browser through detail Back and app Forward without refetching", async () => {
    const { providerNavigationSnapshots } = await import("./lib/provider-navigation-snapshots");
    providerNavigationSnapshots.clear();
    const originalCall = mocks.call.getMockImplementation()!;
    const owner = "fixture-owner";
    const ownerUrl = "https://open.spotify.com/user/fixture-owner";
    const playlistUrl = "https://open.spotify.com/playlist/1234567890123456789012";
    const status = { accountRevisions: { spotify: 31 }, spotifyAccountId: "fixture-spotify-account" };
    const playlistTrack = (id: string, title: string): Track => ({
      ...savedTrack, id, title, provider: "spotify", providerId: `spotify:${id}`,
      sourceUrl: `https://open.spotify.com/track/${id}`, provenance: { remoteEntryId: `entry-${id}` },
    });
    const ownerRows = (args?: Record<string, unknown>) => args?.cursor
      ? { items: [{ id: "owner-playlist-2", title: "Owner page two", url: "https://open.spotify.com/playlist/2234567890123456789012", ownerName: "DJ Fixture", ownerId: owner, ownerUrl }], nextCursor: null, complete: true }
      : { items: [{ id: "owner-playlist-1", title: "Owner page one", url: playlistUrl, ownerName: "DJ Fixture", ownerId: owner, ownerUrl }], nextCursor: "owner-page-2", complete: false };
    const trackRows = (args?: Record<string, unknown>) => args?.cursor
      ? { provider: "spotify", playlistId: "fixture-playlist", title: "Owner mix", ownerName: "DJ Fixture", ownerUrl, tracks: [playlistTrack("owner-track-2", "Orbit Beta")], nextCursor: null, total: 2, complete: true }
      : { provider: "spotify", playlistId: "fixture-playlist", title: "Owner mix", ownerName: "DJ Fixture", ownerUrl, tracks: [playlistTrack("owner-track-1", "Orbit Alpha")], nextCursor: "tracks-page-2", total: 2 };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.ownerId === owner || args?.ownerId === ownerUrl ? ownerRows(args) : { items: [], nextCursor: null, complete: true };
      if (command === "browse_remote_playlist") return trackRows(args);
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "profile", "playlist"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.[0] === "spotify" && (args?.kinds as string[] | undefined)?.includes("profile")) return {
        sourceId: "spotify", complete: true,
        results: [{ id: "spotify:user:fixture-owner", kind: "profile", sourceId: "spotify", title: "DJ Fixture", url: ownerUrl }],
      };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const spotify = sourceGroup("spotify"); spotify.open = true;
    spotify.querySelector<HTMLButtonElement>('[aria-label="Search Spotify playlists"]')!.click(); await settle();
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("spotify");
    const profilesKind = [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Profiles")!;
    profilesKind.click(); await settle();
    const profileQuery = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    profileQuery.value = "DJ Fixture"; profileQuery.dispatchEvent(new Event("input", { bubbles: true })); await settle(); submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Open profile: DJ Fixture"]')!.click(); await settle();
    expect(document.querySelector(".remote-profile-heading strong")?.textContent).toBe(ownerUrl);
    expect(document.querySelector(".remote-source-tabs")).toBeNull();
    expect(document.querySelector(".remote-profile-scope")?.textContent).toContain("playlist pages stay scoped to this profile");
    document.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(document.body.textContent).toContain("Owner page two");
    const listCallsBeforeDetail = mocks.call.mock.calls.filter(([command, args]) => command === "list_remote_playlists" && (args as Record<string, unknown>)?.ownerId === owner).length;
    document.querySelector<HTMLButtonElement>('[aria-label="Browse Owner page one"]')!.click(); await settle();
    expect(document.querySelector(".remote-playlist-detail h2")?.textContent).toBe("Owner mix");
    document.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    const detailFilter = document.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')!;
    detailFilter.value = "Orbit"; detailFilter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const remoteBrowseCalls = () => mocks.call.mock.calls.filter(([command]) => command === "browse_remote_playlist").length;
    const remoteBrowseCallsBeforeBack = remoteBrowseCalls();
    document.querySelector<HTMLButtonElement>(".remote-detail-back")!.click(); await settle();
    expect(document.querySelector(".remote-profile-heading strong")?.textContent).toBe(ownerUrl);
    expect(document.querySelector(".remote-source-tabs")).toBeNull();
    expect(document.body.textContent).toContain("Owner page two");
    expect(mocks.call.mock.calls.filter(([command, args]) => command === "list_remote_playlists" && (args as Record<string, unknown>)?.ownerId === owner)).toHaveLength(listCallsBeforeDetail);
    expect(remoteBrowseCalls()).toBe(remoteBrowseCallsBeforeBack);
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')?.value).toBe("Orbit");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')?.value).toBe("title");
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    expect(remoteBrowseCalls()).toBe(remoteBrowseCallsBeforeBack);
    document.querySelector<HTMLButtonElement>(".remote-detail-owner")!.click(); await settle();
    expect(document.querySelector(".remote-profile-heading strong")?.textContent).toBe(ownerUrl);
    expect(document.querySelector(".remote-source-tabs")).toBeNull();
    expect(document.body.textContent).toContain("Owner page one");
    const callsBeforeOwnerBack = remoteBrowseCalls();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')?.value).toBe("Orbit");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')?.value).toBe("title");
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    expect(remoteBrowseCalls()).toBe(callsBeforeOwnerBack);
  });

  it("saves the full loaded Spotify album in source order despite filtering, without playback or remote writes", async () => {
    const artistId = "1234567890123456789012";
    const album = { id: "save-album-app", title: "Save Album", url: "https://open.spotify.com/album/save-album-app", artists: ["Save Artist"], totalTracks: 3 };
    const first = { ...savedTrack, id: "spotify-save-first", provider: "spotify", providerId: "spotify-save-first", title: "First source track", artist: "Save Artist", sourceUrl: "https://open.spotify.com/track/first-source", durationSeconds: 180 };
    const second = { ...first, id: "spotify-save-second", providerId: "spotify-save-second", title: "Second source track", sourceUrl: "https://open.spotify.com/track/second-source", durationSeconds: 240 };
    const status = { accountRevisions: { spotify: 44 }, spotifyAccountId: "save-app-account" };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist", "album"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) return {
        sourceId: "spotify", complete: true, results: [{ id: `spotify:artist:${artistId}`, kind: "artist", sourceId: "spotify", title: "Save Artist", url: `https://open.spotify.com/artist/${artistId}`, data: { id: artistId, kind: "artist" } }],
      };
      if (command === "browse_remote_artist") return { artist: { id: artistId, name: "Save Artist", url: `https://open.spotify.com/artist/${artistId}`, genres: [] }, albums: [album], totalAlbums: 1 };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId, tracks: [], total: 0 };
      if (command === "browse_remote_album_tracks") return { album, tracks: [first, second], nextCursor: "album-page-2", total: 3 };
      if (command === "match_artist_playback_sources") return args?.sourceTracks;
      if (command === "save_library_tracks") return { savedCount: 2, savedTrackIds: [first.id, second.id] };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!; picker.value = "spotify"; picker.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!.value = "Save Artist";
    document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click(); submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click(); await settle();
    const minimum = document.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    minimum.value = "200"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    document.querySelector<HTMLButtonElement>('[aria-label="Save loaded tracks to library"]')!.click(); await settle();
    const saveCall = mocks.call.mock.calls.find(([command]) => command === "save_library_tracks");
    expect(saveCall?.[1]).toMatchObject({ tracks: [expect.objectContaining({ id: first.id }), expect.objectContaining({ id: second.id })] });
    expect(((saveCall?.[1] as { tracks: Track[] }).tracks).map(track => track.id)).toEqual([first.id, second.id]);
    expect(mocks.call.mock.calls.some(([command]) => command === "playback_command")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => ["add_remote_playlist_tracks", "write_remote_playlist", "sync_remote_playlist"].includes(String(command)))).toBe(false);
  });

  it("clears an old Spotify artist entry snapshot when following an exact track artist credit", async () => {
    const firstArtistId = "1234567890123456789012", secondArtistId = "2234567890123456789012";
    const album = { id: "snapshot-source-album", title: "Snapshot Source Album", url: "https://open.spotify.com/album/snapshot-source-album", artists: ["First Artist"] };
    const creditedTrack: Track = { ...savedTrack, id: "spotify:track:5555555555555555555555", provider: "spotify", providerId: "5555555555555555555555", artist: "Second Artist", title: "Credited track", sourceUrl: "https://open.spotify.com/track/5555555555555555555555", provenance: { catalog: { provider: "spotify", providerId: "5555555555555555555555", artistCount: 1, artists: [{ provider: "spotify", id: secondArtistId, name: "Second Artist" }] } } };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 47 }, spotifyAccountId: "snapshot-account" };
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true }];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) {
        const query = String(args?.query ?? "");
        return query === "First Artist"
          ? { sourceId: "spotify", complete: true, results: [{ id: `spotify:artist:${firstArtistId}`, kind: "artist", sourceId: "spotify", title: "First Artist", url: `https://open.spotify.com/artist/${firstArtistId}`, data: { id: firstArtistId, kind: "artist" } }] }
          : { sourceId: "spotify", complete: true, results: [{ id: creditedTrack.id, kind: "track", sourceId: "spotify", title: creditedTrack.title, track: creditedTrack }] };
      }
      if (command === "browse_remote_artist") {
        const id = String(args?.artistId ?? "");
        return { artist: { id, name: id === secondArtistId ? "Second Artist" : "First Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: id === firstArtistId ? [album] : [], totalAlbums: id === firstArtistId ? 1 : 0 };
      }
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: String(args?.artistId ?? ""), tracks: [], total: 0 };
      if (command === "browse_remote_album_tracks") return { album, tracks: [creditedTrack], total: 1 };
      if (command === "match_artist_playback_sources") return args?.sourceTracks;
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!; picker.value = "spotify"; picker.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!; query.value = "First Artist"; query.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click(); submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toContain("Snapshot Source Album");
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    await searchDiscovery("Second track", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId: secondArtistId, cursor: null }));
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Second Artist");
  });

  it("restores Spotify artist album browsing through app Back and Forward without provider refetch", async () => {
    const { providerNavigationSnapshots } = await import("./lib/provider-navigation-snapshots");
    providerNavigationSnapshots.clear();
    const originalCall = mocks.call.getMockImplementation()!;
    const artistId = "1234567890123456789012";
    const searchArtwork = "https://i.scdn.co/image/route-search-artist-art";
    const album = { id: "artist-route-album", title: "Route Album", url: "https://open.spotify.com/album/artist-route-album", artists: ["Route Artist"] };
    const track = { ...savedTrack, id: "route-artist-track", title: "Route Artist Song", provider: "spotify", providerId: "spotify:route-artist-track", sourceUrl: "https://open.spotify.com/track/route-artist-track" };
    const status = { accountRevisions: { spotify: 32 }, spotifyAccountId: "fixture-spotify-account" };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist", "album", "playlist", "profile"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) return {
        sourceId: "spotify", complete: true, results: [{ id: `spotify:artist:${artistId}`, kind: "artist", sourceId: "spotify", title: "Route Artist", url: `https://open.spotify.com/artist/${artistId}`, data: { id: artistId, kind: "artist", artworkUrl: searchArtwork } }],
      };
      if (command === "browse_remote_artist") return { artist: { id: artistId, name: "Route Artist", url: `https://open.spotify.com/artist/${artistId}`, genres: [] }, albums: [album], totalAlbums: 1, topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") return { album, tracks: [track], total: 1, complete: true };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    picker.value = "spotify"; picker.dispatchEvent(new Event("change", { bubbles: true }));
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Route Artist"; query.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click();
    submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    expect((document.querySelector(".spotify-artist .hero img") as HTMLImageElement)?.src).toBe(searchArtwork);
    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click(); await settle();
    const remoteCalls = () => mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist" || command === "browse_remote_album_tracks").length;
    const callsBeforeBack = remoteCalls();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Route Album");
    expect(document.body.textContent).toContain("Route Artist Song");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Route Artist");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector(".search-panel")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Route Artist");
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Route Album");
    expect(document.body.textContent).toContain("Route Artist Song");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect((document.querySelector(".spotify-artist .hero img") as HTMLImageElement)?.src).toBe(searchArtwork);
    const directNavigationId = "navigate-same-artist-without-search-artwork";
    eventHandlers.get("automation-ui-navigation")?.({ event: "automation-ui-navigation", id: 75,
      payload: { requestId: directNavigationId, target: { kind: "spotify_artist", spotifyArtistId: artistId } } });
    await settle();
    expect(document.querySelector(".spotify-artist .hero img")).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete", {
      requestId: directNavigationId, destination: artistId, status: "complete",
    });
    expect(remoteCalls()).toBe(callsBeforeBack);
  });

  it("opens the exact Spotify album identity from a track context menu and restores the prior route", async () => {
    const albumId = "aaaaaaaaaaaaaaaaaaaaaa", trackId = "bbbbbbbbbbbbbbbbbbbbbb";
    const album = { id: albumId, title: "Exact Album Edition", url: `https://open.spotify.com/album/${albumId}`, artists: ["Exact Artist"] };
    const track: Track = { ...savedTrack, id: `spotify:track:${trackId}`, provider: "spotify", providerId: trackId, sourceUrl: `https://open.spotify.com/track/${trackId}`, album: album.title, title: "Exact Album Song", provenance: { recordingContext: { albumId, albumUri: `spotify:album:${albumId}` } } };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 14 }, spotifyAccountId: "fixture-account" };
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "album"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) return {
        sourceId: "spotify", complete: true, results: [{ id: trackId, kind: "track", sourceId: "spotify", title: track.title, track }],
      };
      if (command === "browse_remote_album_tracks") return { provider: "spotify", accountId: "fixture-account", album, tracks: [{ ...track, id: "album-occurrence", provenance: { albumId, albumPosition: 0 } }], total: 1, complete: true };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    picker.value = "spotify"; picker.dispatchEvent(new Event("change", { bubbles: true }));
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = track.title; query.dispatchEvent(new Event("input", { bubbles: true })); submitSearch(); await settle();
    const row = [...document.querySelectorAll<HTMLElement>(".result-row")].find(item => item.textContent?.includes(track.title))!;
    row.querySelector<HTMLElement>(".track-list-viewport li")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 360, clientY: 220 })); await settle();
    const goToAlbum = [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button => button.textContent?.trim() === "Go to album")!;
    expect(goToAlbum).toBeTruthy(); goToAlbum.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_album_tracks", expect.objectContaining({ albumId, cursor: null }));
    expect(mocks.call.mock.calls.some(([command]) => command === "browse_remote_artist")).toBe(false);
    expect(document.querySelector(".spotify-artist")?.getAttribute("aria-label")).toBe("Spotify album");
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe(album.title);
    expect(document.body.textContent).toContain(track.title);
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector(".search-panel")).not.toBeNull();
  });

  it("opens the exact MusicBrainz release version from a track context menu", async () => {
    const releaseId = "12345678-1234-4234-8234-123456789abc", artistId = "abcdefab-1234-4234-8234-123456789abc";
    const track: Track = { ...savedTrack, id: "mb-edition-track", provider: "catalog", providerId: "mb-recording", sourceUrl: `https://musicbrainz.org/release/${releaseId}`, album: "Selected Release Edition", title: "Release Track", provenance: {
      catalogProvider: "musicbrainz", releaseId, artistCredits: [{ name: "Exact Credited Artist", role: "artist credit", artist: { provider: "musicbrainz", id: artistId } }],
    } };
    const albumTrack = { ...track, id: "mb-release-row-1", title: "Release Track", provenance: { catalogProvider: "musicbrainz", releaseId } };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "album"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("musicbrainz")) return {
        sourceId: "musicbrainz", complete: true, results: [{ id: track.id, kind: "track", sourceId: "musicbrainz", title: track.title, track }],
      };
      if (command === "get_release_tracks") return [albumTrack];
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    picker.value = "musicbrainz"; picker.dispatchEvent(new Event("change", { bubbles: true }));
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = track.title; query.dispatchEvent(new Event("input", { bubbles: true })); submitSearch(); await settle();
    const row = [...document.querySelectorAll<HTMLElement>(".result-row")].find(item => item.textContent?.includes(track.title))!;
    row.querySelector<HTMLElement>(".track-list-viewport li")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 360, clientY: 220 })); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button => button.textContent?.trim() === "Go to album")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_release_tracks", expect.objectContaining({
      release: expect.objectContaining({ release: { provider: "musicbrainz", id: releaseId }, title: "Selected Release Edition" }),
      artist: { provider: "musicbrainz", id: artistId },
    }));
    expect(document.querySelector(".release-view h1")?.textContent).toBe("Selected Release Edition");
    expect(document.body.textContent).toContain("Release Track");
  });

  it("finds a noncatalog track's exact album by title and parsed catalog artist identity", async () => {
    const artistId="11111111-1111-4111-8111-111111111111", releaseId="22222222-2222-4222-8222-222222222222";
    const track:Track={...savedTrack,id:"youtube-album-track",provider:"youtube",providerId:"fixture-video",sourceUrl:"https://youtube.test/watch?v=fixture",title:"Equitoreal",artist:"Dubtribe Sound System",album:"Bryant Street",provenance:{sourceKind:"audioSearch",catalog:{provider:"musicbrainz",artists:[{provider:"musicbrainz",id:artistId,name:"Dubtribe Sound System"}],artistCount:1}}};
    const release:ArtistReleaseRef={release:{provider:"musicbrainz",id:releaseId},title:"Bryant Street",sourceUrl:`https://musicbrainz.org/release/${releaseId}`,credits:[{name:"Dubtribe Sound System",artist:{provider:"musicbrainz",id:artistId},role:"artist credit",sourceUrl:`https://musicbrainz.org/release/${releaseId}`}],tracks:[]};
    const wrongArtist={...release,release:{...release.release,id:"33333333-3333-4333-8333-333333333333"},credits:[{...release.credits[0]!,name:"Different Artist",artist:{provider:"musicbrainz" as const,id:"44444444-4444-4444-8444-444444444444"}}]};
    const wrongTitle={...release,release:{...release.release,id:"55555555-5555-4555-8555-555555555555"},title:"Different Album"};
    audioSearchResult=[track];
    discoveryAlbumTracks=[{...track,id:"album-track-occurrence",provider:"catalog",providerId:"recording",title:"Equitoreal",sourceUrl:release.sourceUrl,provenance:{catalogProvider:"musicbrainz",releaseId}}];
    const originalCall=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="search_albums")return {items:[wrongArtist,wrongTitle,release],nextPage:null,warnings:[]};
      return originalCall(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await searchDiscovery("Equitoreal",["YouTube"]);
    const row=[...document.querySelectorAll<HTMLElement>(".result-row")].find(item=>item.textContent?.includes(track.title))!;
    row.querySelector<HTMLElement>(".track-list-viewport li")!.dispatchEvent(new MouseEvent("contextmenu",{bubbles:true,cancelable:true,clientX:360,clientY:220}));await settle();
    const albumAction=[...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button=>button.textContent?.trim()==="Go to album")!;
    expect(albumAction.disabled).toBe(false);albumAction.click();await settle();await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_albums",{query:"Bryant Street",page:1,pageSize:20});
    expect(mocks.call).toHaveBeenCalledWith("get_release_tracks",expect.objectContaining({release:expect.objectContaining({release:{provider:"musicbrainz",id:releaseId}}),artist:{provider:"musicbrainz",id:artistId}}));
    expect(document.querySelector(".release-view h1")?.textContent).toBe("Bryant Street");
    expect(audioSearchResult[0]?.album).toBe("Bryant Street");
  });

  it("opens album search choices when a noncatalog album hint matches multiple releases", async () => {
    const artistId="66666666-6666-4666-8666-666666666666";
    const track:Track={...savedTrack,id:"soundcloud-album-track",provider:"soundcloud",providerId:"fixture-track",sourceUrl:"https://soundcloud.test/track",title:"A Song",artist:"An Artist",album:"Shared Album",provenance:{sourceKind:"audioSearch"}};
    const release=(id:string):ArtistReleaseRef=>({release:{provider:"musicbrainz",id},title:"Shared Album",sourceUrl:`https://musicbrainz.org/release/${id}`,credits:[{name:"An Artist",artist:{provider:"musicbrainz",id:artistId},role:"artist credit",sourceUrl:`https://musicbrainz.org/release/${id}`}],tracks:[]});
    audioSearchResult=[track];
    const originalCall=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="search_albums")return {items:[release("77777777-7777-4777-8777-777777777777"),release("88888888-8888-4888-8888-888888888888")],nextPage:null,warnings:[]};
      return originalCall(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await searchDiscovery("A Song",["SoundCloud"]);
    const row=[...document.querySelectorAll<HTMLElement>(".result-row")].find(item=>item.textContent?.includes(track.title))!;
    row.querySelector<HTMLElement>(".track-list-viewport li")!.dispatchEvent(new MouseEvent("contextmenu",{bubbles:true,cancelable:true,clientX:360,clientY:220}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button=>button.textContent?.trim()==="Go to album")!.click();await settle();await settle();
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("musicbrainz");
    expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Albums")?.getAttribute("aria-pressed")).toBe("true");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Shared Album");
    expect(mocks.call.mock.calls.some(([command,args])=>command==="search_source"&&(args?.kinds as string[]|undefined)?.includes("album"))).toBe(true);
  });

  it("handles a failed noncatalog album lookup without leaking provider URLs",async()=>{
    const track:Track={...savedTrack,id:"album-lookup-error-track",provider:"youtube",providerId:"fixture-track",sourceUrl:"https://youtube.test/watch?v=fixture",title:"Song",artist:"Artist",album:"Album",provenance:{sourceKind:"audioSearch"}};
    audioSearchResult=[track];
    const originalCall=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="search_albums")throw new Error("Catalog lookup failed https://private.invalid/?token=secret-token");
      return originalCall(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await searchDiscovery("Song",["YouTube"]);
    const row=[...document.querySelectorAll<HTMLElement>(".result-row")].find(item=>item.textContent?.includes(track.title))!;
    row.querySelector<HTMLElement>(".track-list-viewport li")!.dispatchEvent(new MouseEvent("contextmenu",{bubbles:true,cancelable:true,clientX:360,clientY:220}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button=>button.textContent?.trim()==="Go to album")!.click();await settle();
    const notice=document.querySelector(".banner.notice")?.textContent??"";
    expect(notice).toContain("Catalog lookup failed");expect(notice).not.toContain("private.invalid");expect(notice).not.toContain("secret-token");
  });

  it("opens a Spotify track's exact single-artist credit at Spotify and preserves the search route", async () => {
    const artistId = "aaaaaaaaaaaaaaaaaaaaaa";
    const spotifyTrack: Track = { ...savedTrack, id: "spotify:track:cccccccccccccccccccccc", provider: "spotify", providerId: "cccccccccccccccccccccc", artist: "Exact Spotify Artist", title: "Spotify search track", sourceUrl: "https://open.spotify.com/track/cccccccccccccccccccccc", provenance: { catalog: { provider: "spotify", providerId: "cccccccccccccccccccccc", verified: true, artistCount: 1, artists: [{ provider: "spotify", id: artistId, name: "Exact Spotify Artist" }] } } };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true }];
      if (command === "search_source") return { sourceId: "spotify", complete: true, results: [{ id: spotifyTrack.id, kind: "track", sourceId: "spotify", title: spotifyTrack.title, track: spotifyTrack }] };
      if (command === "browse_remote_artist") return { artist: { id: artistId, name: "Exact Spotify Artist", url: `https://open.spotify.com/artist/${artistId}`, genres: [] }, albums: [], totalAlbums: 0, topTracksUnavailable: "No tracks" };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery("Spotify search track", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId, cursor: null }));
    expect(document.querySelector(".spotify-artist")).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "search_artists")).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector('[aria-label="Search music"]')).not.toBeNull();
  });

  it("asks which Spotify artist to open when a track has multiple exact Spotify artist credits", async () => {
    const primaryId = "aaaaaaaaaaaaaaaaaaaaaa", featuredId = "bbbbbbbbbbbbbbbbbbbbbb";
    const spotifyTrack: Track = { ...savedTrack, id: "spotify:track:dddddddddddddddddddddd", provider: "spotify", providerId: "dddddddddddddddddddddd", artist: "Primary Artist, Featured Artist", title: "Collaboration", sourceUrl: "https://open.spotify.com/track/dddddddddddddddddddddd", provenance: { catalog: { provider: "spotify", providerId: "dddddddddddddddddddddd", verified: true, artistCount: 2, artists: [{ provider: "spotify", id: primaryId, name: "Primary Artist" }, { provider: "spotify", id: featuredId, name: "Featured Artist" }] } } };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true }];
      if (command === "search_source") return { sourceId: "spotify", complete: true, results: [{ id: spotifyTrack.id, kind: "track", sourceId: "spotify", title: spotifyTrack.title, track: spotifyTrack }] };
      if (command === "browse_remote_artist") { const id = String(args?.artistId ?? ""); return { artist: { id, name: id === featuredId ? "Featured Artist" : "Primary Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [], totalAlbums: 0, topTracksUnavailable: "No tracks" }; }
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery("Collaboration", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    const chooser = document.querySelector<HTMLDialogElement>('[aria-label="Choose Spotify artist"]');
    expect(chooser).not.toBeNull(); expect(chooser?.textContent).toContain("Primary Artist"); expect(chooser?.textContent).toContain("Featured Artist");
    expect(mocks.call.mock.calls.some(([command]) => command === "browse_remote_artist")).toBe(false);
    chooser?.querySelector<HTMLButtonElement>('[aria-label="Open Spotify artist Featured Artist"]')?.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId: featuredId, cursor: null }));
  });


  it("deduplicates exact Spotify credits and searches Spotify when credits are incomplete or mismatched", async () => {
    const artistId = "aaaaaaaaaaaaaaaaaaaaaa";
    const duplicate: Track = { ...savedTrack, id: "spotify:track:eeeeeeeeeeeeeeeeeeeeee", provider: "spotify", providerId: "eeeeeeeeeeeeeeeeeeeeee", artist: "Repeated Artist, Repeated Artist", title: "Duplicate credit", sourceUrl: "https://open.spotify.com/track/eeeeeeeeeeeeeeeeeeeeee", provenance: { catalog: { provider: "spotify", providerId: "eeeeeeeeeeeeeeeeeeeeee", artistCount: 2, artists: [{ provider: "spotify", id: artistId, name: "Repeated Artist" }, { provider: "spotify", id: artistId, name: "Repeated Artist" }] } } };
    const incomplete: Track = { ...duplicate, id: "spotify:track:ffffffffffffffffffffff", providerId: "ffffffffffffffffffffff", artist: "Incomplete Artist", title: "Incomplete credit", sourceUrl: "https://open.spotify.com/track/ffffffffffffffffffffff", provenance: { catalog: { provider: "spotify", providerId: "ffffffffffffffffffffff", artistCount: 2, artists: [{ provider: "spotify", id: artistId, name: "Incomplete Artist" }, { provider: "spotify", id: "not-a-spotify-id", name: "Incomplete Artist" }] } } };
    const mismatched: Track = { ...duplicate, id: "spotify:track:gggggggggggggggggggggg", providerId: "gggggggggggggggggggggg", artist: "Edited Artist Name", title: "Edited metadata credit", sourceUrl: "https://open.spotify.com/track/gggggggggggggggggggggg", provenance: { catalog: { provider: "spotify", providerId: "gggggggggggggggggggggg", artistCount: 1, artists: [{ provider: "spotify", id: artistId, name: "Original Spotify Credit" }] } } };
    const staleCatalogIdentity: Track = { ...duplicate, id: "spotify:track:hhhhhhhhhhhhhhhhhhhhhh", providerId: "hhhhhhhhhhhhhhhhhhhhhh", artist: "Repeated Artist", title: "Stale catalog identity", sourceUrl: "https://open.spotify.com/track/hhhhhhhhhhhhhhhhhhhhhh", provenance: { catalog: { provider: "spotify", providerId: "different-spotify-track", artistCount: 1, artists: [{ provider: "spotify", id: artistId, name: "Repeated Artist" }] } } };
    let current = duplicate;
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist"], enabled: true },
      ];
      if (command === "search_source") {
        const sourceId = String((args?.sourceIds as string[] | undefined)?.[0] ?? "");
        const kinds = args?.kinds as string[] | undefined;
        if (sourceId === "spotify" && kinds?.includes("artist")) return { sourceId, complete: true, results: [{ id: `spotify:artist:${artistId}`, kind: "artist", sourceId, title: String(args?.query ?? "Artist"), url: `https://open.spotify.com/artist/${artistId}`, data: { id: artistId } }] };
        return { sourceId: "spotify", complete: true, results: [{ id: current.id, kind: "track", sourceId: "spotify", title: current.title, track: current }] };
      }
      if (command === "browse_remote_artist") return { artist: { id: String(args?.artistId ?? ""), name: "Repeated Artist", url: `https://open.spotify.com/artist/${String(args?.artistId ?? "")}`, genres: [] }, albums: [], totalAlbums: 0, topTracksUnavailable: "No tracks" };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!.click(); await settle();
    await searchDiscovery("Duplicate credit", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(document.querySelector('[aria-label="Choose Spotify artist"]')).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId, cursor: null }));
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();

    current = incomplete;
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!.click(); await settle();
    await searchDiscovery("Incomplete credit", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(document.querySelector('[aria-label="Choose Spotify artist"]')).toBeNull();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Incomplete Artist");
    expect((document.querySelector<HTMLSelectElement>('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("spotify");
    expect([...document.querySelectorAll<HTMLButtonElement>('.filters button')].find(button => button.textContent === "Artists")?.getAttribute("aria-pressed")).toBe("true");
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Incomplete Artist", sourceIds: ["spotify"], kinds: ["artist"] }));
    expect(mocks.call.mock.calls.some(([command]) => command === "search_artists")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist")).toHaveLength(1);

    current = mismatched;
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!.click(); await settle();
    await searchDiscovery("Edited metadata credit", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Edited Artist Name");
    expect((document.querySelector<HTMLSelectElement>('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("spotify");
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Edited Artist Name", sourceIds: ["spotify"], kinds: ["artist"] }));
    expect(mocks.call.mock.calls.some(([command]) => command === "search_artists")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist")).toHaveLength(1);

    current = staleCatalogIdentity;
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!.click(); await settle();
    await searchDiscovery("Stale catalog identity", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Repeated Artist");
    expect((document.querySelector<HTMLSelectElement>('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("spotify");
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Repeated Artist", sourceIds: ["spotify"], kinds: ["artist"] }));
    expect(mocks.call.mock.calls.some(([command]) => command === "search_artists")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist")).toHaveLength(1);
  });

  it("opens an explicit MusicBrainz artist search from Spotify and restores the Spotify page with Back", async () => {
    const spotifyId = "aaaaaaaaaaaaaaaaaaaaaa";
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "cross-provider-account", accountRevisions: { spotify: 1 } };
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true },
      ];
      if (command === "search_source") {
        const sourceId = String((args?.sourceIds as string[] | undefined)?.[0] ?? "");
        if (sourceId === "spotify" && (args?.kinds as string[] | undefined)?.includes("artist")) return { sourceId, complete: true, results: [{ id: `spotify:artist:${spotifyId}`, kind: "artist", sourceId, title: "Cross Provider Artist", url: `https://open.spotify.com/artist/${spotifyId}`, data: { id: spotifyId } }] };
        if (sourceId === "musicbrainz" && (args?.kinds as string[] | undefined)?.includes("artist")) return { sourceId, complete: true, results: [{ id: "mb-result-1", kind: "artist", sourceId, title: "Cross Provider Artist", url: "https://musicbrainz.org/artist/mb-result-1", data: { id: "mb-result-1" } }] };
      }
      if (command === "browse_remote_artist") return { artist: { id: spotifyId, name: "Cross Provider Artist", url: `https://open.spotify.com/artist/${spotifyId}`, genres: [] }, albums: [], totalAlbums: 0 };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click(); await settle();
    await searchDiscovery("Cross Provider Artist", ["spotify"]);
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Cross Provider Artist");
    const browseCallsBeforeSwitch = mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist").length;
    document.querySelector<HTMLButtonElement>('[aria-label="Find Cross Provider Artist on MusicBrainz"]')!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Cross Provider Artist");
    expect((document.querySelector<HTMLSelectElement>('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("musicbrainz");
    expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")?.getAttribute("aria-pressed")).toBe("true");
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Cross Provider Artist", sourceIds: ["musicbrainz"], kinds: ["artist"] }));
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist")).toHaveLength(browseCallsBeforeSwitch);
    expect(mocks.call.mock.calls.some(([command]) => command === "search_artists")).toBe(false);
    expect(document.querySelector(".artist-result")).toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Cross Provider Artist");
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_artist")).toHaveLength(browseCallsBeforeSwitch);
  });

  it("opens a Spotify artist result only after an explicit selection from a MusicBrainz artist page", async () => {
    const spotifyId = "bbbbbbbbbbbbbbbbbbbbbb";
    const mbArtistId = "11111111-2222-4333-8444-555555555555";
    savedChartTracks = [{ ...savedTrack, provenance: { catalog: { provider: "musicbrainz", providerId: "recording-1", artistCount: 1, artists: [{ provider: "musicbrainz", id: mbArtistId, name: "Visible Artist" }] } } }];
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.[0] === "spotify" && (args?.kinds as string[] | undefined)?.includes("artist")) return {
        sourceId: "spotify", complete: true, results: [{ id: `spotify:artist:${spotifyId}`, kind: "artist", sourceId: "spotify", title: "Visible Artist", url: `https://open.spotify.com/artist/${spotifyId}`, data: { id: spotifyId } }],
      };
      if (command === "browse_remote_artist") return { artist: { id: spotifyId, name: "Visible Artist", url: `https://open.spotify.com/artist/${spotifyId}`, genres: [] }, albums: [], totalAlbums: 0 };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click(); await settle();
    expect(document.querySelector(".artist-page")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Find Visible Artist on Spotify"]')!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Visible Artist");
    expect((document.querySelector<HTMLSelectElement>('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("spotify");
    expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")?.getAttribute("aria-pressed")).toBe("true");
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Visible Artist", sourceIds: ["spotify"], kinds: ["artist"] }));
    expect(mocks.call.mock.calls.some(([command]) => command === "browse_remote_artist")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "playback_command" || command === "save_library_tracks" || command === "append_playlist_entries" || command === "add_remote_playlist_tracks")).toBe(false);
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Visible Artist");
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId: spotifyId, cursor: null }));
    expect(mocks.call.mock.calls.some(([command]) => command === "playback_command" || command === "save_library_tracks" || command === "append_playlist_entries" || command === "add_remote_playlist_tracks")).toBe(false);
  });

  it("keeps the current artist page and explains when the requested provider is disabled", async () => {
    savedChartTracks = [{ ...savedTrack, provenance: { catalog: { provider: "musicbrainz", providerId: "recording-1", artistCount: 1, artists: [{ provider: "musicbrainz", id: "11111111-2222-4333-8444-555555555555", name: "Visible Artist" }] } } }];
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist"], enabled: false, detail: "Connect Spotify in Settings" },
      ];
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Find Visible Artist on Spotify"]')!.click(); await settle();
    expect(document.querySelector(".artist-page")).not.toBeNull();
    expect(document.body.textContent).toContain("Connect Spotify in Settings");
    expect(mocks.call.mock.calls.some(([command, args]) => command === "search_source" && (args as { sourceIds?: string[] }).sourceIds?.includes("spotify"))).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "browse_remote_artist")).toBe(false);
  });

  it("opens Spotify artist results and pre-matches bounded track pages before playback", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const spotifyArtistId = "0123456789012345678901";
    const artistArtwork = "https://i.scdn.co/image/fixture-artist-art";
    const album = { id: "spotify-album-1", title: "Fixture Album", url: "https://open.spotify.com/album/spotify-album-1", albumType: "album", totalTracks: 501, artists: ["Fixture Spotify Artist"] };
    const remoteTracks = Array.from({ length: 501 }, (_, index) => ({
      ...savedTrack, id: `spotify-row-${String(index).padStart(3, "0")}`, provider: "spotify", providerId: "spotify-track-shared",
      title: `Spotify Song ${String(index).padStart(3, "0")}`, localPath: undefined, localAvailability: undefined,
      provenance: { remoteEntryId: `spotify-entry-${index}`, positionScope: "album", position: index + 1 },
    } satisfies Track));
    remoteTracks[500] = { ...remoteTracks[0]!, provenance: { remoteEntryId: "spotify-entry-duplicate-occurrence", positionScope: "album", position: 501 } };
    let delayedArtistTrackId: string | null = null;
    let finishDelayedArtistMatcher: ((value: (LocalAsset | null)[]) => void) | undefined;
    let delayedNoMatchTrackId: string | null = null;
    let finishDelayedNoMatch: ((value: (LocalAsset | null)[]) => void) | undefined;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist", "album", "playlist", "profile"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) return {
        sourceId: "spotify", complete: true, results: [{
          id: `spotify:artist:${spotifyArtistId}`, kind: "artist", sourceId: "spotify",
          title: "Fixture Spotify Artist", url: `https://open.spotify.com/artist/${spotifyArtistId}`,
          data: { id: "9999999999999999999999", kind: "artist", artworkUrl: artistArtwork },
        }],
      };
      if (command === "browse_remote_artist") return {
        artist: { id: spotifyArtistId, name: "Fixture Spotify Artist", url: `https://open.spotify.com/artist/${spotifyArtistId}`, genres: [] },
        albums: [album], totalAlbums: 1,
        topTracksUnavailable: "Spotify does not provide artist top tracks through this route.",
      };
      if (command === "browse_remote_album_tracks") return { album, tracks: remoteTracks, total: remoteTracks.length };
      if (command === "match_artist_playback_sources") {
        const sourceTracks = (args?.sourceTracks ?? []) as Track[];
        if (sourceTracks.length === 1 && sourceTracks[0]?.id === delayedArtistTrackId) return new Promise(resolve => { finishDelayedArtistMatcher = resolve; });
        if (sourceTracks.length === 1 && sourceTracks[0]?.id === delayedNoMatchTrackId) return new Promise(resolve => { finishDelayedNoMatch = resolve; });
        return sourceTracks.map(track => track.id === remoteTracks[0]!.id ? ({
          id: `asset-${track.id}`, recordingId: track.id, path: `/music/${track.id}.flac`, hash: `hash-${track.id}`,
          sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: { ...track, localAvailability: "available" },
        } satisfies LocalAsset) : null);
      }
      return originalCall(command, args);
    });

    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click();
    await settle();
    const picker = document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    picker.value = "spotify"; picker.dispatchEvent(new Event("change", { bubbles: true }));
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Fixture Spotify Artist"; query.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Artists")!.click();
    submitSearch(); await settle();

    const openArtist = document.querySelector<HTMLButtonElement>(".open-result")!;
    expect(openArtist.textContent).toContain("Explore artist");
    expect(openArtist.closest('[aria-label="Spotify results"]')).not.toBeNull();
    openArtist.click(); await settle();

    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("Fixture Spotify Artist");
    expect(document.querySelector(".spotify-artist .hero img")).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_artist", expect.objectContaining({ artistId: spotifyArtistId, cursor: null }));
    expect(mocks.call).not.toHaveBeenCalledWith("get_artist_profile", expect.anything());
    expect(mocks.call).not.toHaveBeenCalledWith("search_artists", expect.anything());

    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click();
    await settle();
    const matchingCalls = () => mocks.call.mock.calls.filter(([command]) => command === "match_artist_playback_sources");
    expect(matchingCalls().map(([, args]) => (args as { sourceTracks: Track[] }).sourceTracks.length)).toEqual([200, 200, 101]);
    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 000"]')!.click();
    await settle();
    expect(matchingCalls().map(([, args]) => (args as { sourceTracks: Track[] }).sourceTracks.length)).toEqual([200, 200, 101]);
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: expect.objectContaining({
      id: remoteTracks[0]!.id, provider: "spotify", providerId: "spotify-track-shared", localPath: `/music/${remoteTracks[0]!.id}.flac`,
    }) }));
    await new Promise(resolve => setTimeout(resolve, 1100));
    await settle();
    const playbackStates = mocks.call.mock.calls.filter(([command]) => command === "automation_playback_state");
    const lastPlayback = playbackStates[playbackStates.length - 1]?.[1] as { playback?: { queue?: Track[] } } | undefined;
    expect(lastPlayback?.playback?.queue?.map(track => track.id)).toEqual(remoteTracks.map(track => track.id));

    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 001"]')!.click();
    await settle();
    expect(matchingCalls().map(([, args]) => (args as { sourceTracks: Track[] }).sourceTracks.length)).toEqual([200, 200, 101]);

    eventHandlers.get("library-updated")?.({ event: "library-updated", id: 90, payload: remoteTracks[0] });
    delayedArtistTrackId = remoteTracks[2]!.id;
    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 002"]')!.click();
    await settle();
    expect(finishDelayedArtistMatcher).toBeDefined();
    expect((matchingCalls()[matchingCalls().length - 1]?.[1] as { sourceTracks: Track[] }).sourceTracks.map(track => track.id)).toEqual([remoteTracks[2]!.id]);
    eventHandlers.get("automation-playback")?.({ event: "automation-playback", id: 91, payload: { requestId: "stop-during-artist-match", action: "stop" } });
    await settle();
    finishDelayedArtistMatcher!([{
      id: `asset-${remoteTracks[2]!.id}`, recordingId: remoteTracks[2]!.id, path: `/music/${remoteTracks[2]!.id}.flac`, hash: `hash-${remoteTracks[2]!.id}`,
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: { ...remoteTracks[2]!, localAvailability: "available" },
    }]);
    await settle();
    expect(mocks.call.mock.calls.some(([command, args]) => command === "resolve_playback_track" && (args as { track?: Track }).track?.id === remoteTracks[2]!.id)).toBe(false);

    eventHandlers.get("library-updated")?.({ event: "library-updated", id: 92, payload: remoteTracks[3] });
    delayedNoMatchTrackId = remoteTracks[3]!.id;
    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 003"]')!.click();
    await settle();
    expect(finishDelayedNoMatch).toBeDefined();
    const matcherCallsBeforeInvalidatedReply = matchingCalls().length;
    eventHandlers.get("library-updated")?.({ event: "library-updated", id: 93, payload: remoteTracks[3] });
    finishDelayedNoMatch!([null]);
    delayedNoMatchTrackId = null;
    await settle();

    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 003"]')!.click();
    await settle();
    expect(matchingCalls()).toHaveLength(matcherCallsBeforeInvalidatedReply + 1);
  });

  it("changes the loaded saved browser snapshot when its version picker changes", async () => {
    delayedBrowserImports = Promise.resolve([latestImport, oldImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await refreshSidebarBrowserImports();
    importedPlaylistCard("Latest Saved Import").click();
    await settle();
    const picker = document.querySelector('[aria-label="Imported playlist version"]') as HTMLSelectElement;
    expect(picker).not.toBeNull();
    expect([...picker.options].map((option) => option.textContent)).toContain("All import dates");
    expect([...picker.options].map((option) => option.value)).toEqual(["all", latestImport.id, oldImport.id]);
    picker.value = oldImport.id;
    picker.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: oldImport.id });
    expect(document.querySelector("h1")?.textContent).toBe("Older Import");
  });

  it("shows active duration filters in the compact toolbar and clears them", async () => {
    savedChartTracks = [savedTrack, { ...secondTrack, durationSeconds: 240 }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const filterMenu = document.querySelector<HTMLDetailsElement>(".duration-filter-tools")!;
    expect(filterMenu.open).toBe(false);
    filterMenu.querySelector<HTMLElement>("summary")!.click();
    await settle();
    const minimum = document.querySelector<HTMLInputElement>('[aria-label="Minimum duration"]')!;
    minimum.value = "90";
    minimum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(filterMenu.querySelector("summary")?.getAttribute("aria-label")).toBe("Duration filters, 1 active");
    filterMenu.querySelector<HTMLButtonElement>(".clear-duration-filters")!.click();
    await settle();
    expect(minimum.value).toBe("");
    expect(filterMenu.querySelector("summary")?.getAttribute("aria-label")).toBe("Duration filters");
  });

  it("filters non-paged chart tracks by album and intersects the duration bound", async () => {
    savedChartTracks = [
      { ...savedTrack, id: "short-blue", title: "Short Song", album: "Blue Record", durationSeconds: 60 },
      { ...savedTrack, id: "long-blue", title: "Long Song", album: "Blue Record", durationSeconds: 180 },
      { ...savedTrack, id: "other", title: "Other Song", album: "Other Record", durationSeconds: 300 },
    ];
    delayedBrowserImports = Promise.resolve([latestImport, oldImport]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await refreshSidebarBrowserImports();
    importedPlaylistCard("Latest Saved Import").click();
    await settle();

    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "Blue Record";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    let rows = [...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "");
    expect(rows).toHaveLength(2);
    expect(rows.join(" ")).toContain("Short Song");
    expect(rows.join(" ")).toContain("Long Song");
    expect(rows.join(" ")).not.toContain("Other Song");

    const minimum = document.querySelector<HTMLInputElement>('[aria-label="Minimum duration"]')!;
    minimum.value = "2:00";
    minimum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    rows = [...document.querySelectorAll(".shared-track-list > li")].map(row => row.textContent ?? "");
    expect(rows).toHaveLength(1);
    expect(rows.join(" ")).toContain("Long Song");
    expect(rows.join(" ")).not.toContain("Short Song");
    expect(rows.join(" ")).not.toContain("Other Song");
  });

  it("keeps duration bounds collapsed in download and chart toolbars until Filters is opened", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const handler = eventHandlers.get("automation-ui-navigation")!;
    const go = async (requestId: string, target: unknown) => {
      handler({ event: "automation-ui-navigation", id: 1, payload: { requestId, target } });
      await settle(); await settle();
    };
    for (const [requestId, view, label] of [["downloads-duration", "downloads", "Download minimum duration"], ["charts-duration", "charts", "Minimum duration"]] as const) {
      await go(requestId, { kind: "view", view });
      const filters = document.querySelector<HTMLButtonElement>(".duration-filter .filter-toggle")!;
      const minimum = document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
      const fields = minimum.parentElement?.parentElement as HTMLDivElement;
      expect(filters.getAttribute("aria-expanded")).toBe("false");
      expect(fields.hidden).toBe(true);
      filters.click(); await settle();
      expect(fields.hidden).toBe(false);
      minimum.value = "2:00";
      minimum.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
      filters.click(); await settle();
      expect(fields.hidden).toBe(true);
      expect(minimum.value).toBe("2:00");
      expect(filters.textContent).toContain("1");
    }
  });

  it.each(["button.position-play", "button.track-title"])("rebuilds the library queue when its current track is played with %s", async (selector) => {
    savedChartTracks = [{...savedTrack,rank:1},{...secondTrack,rank:2}];
    previewMode="direct";
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    await automationAction("single-track-before-library","play",{trackIds:[savedTrack.id]});await settle();
    const single=(await automationAction("single-track-state","volume",{volume:0.8})).playback;
    expect(single.queue).toHaveLength(1);
    document.querySelector<HTMLButtonElement>(`[aria-label="Recordings"] ${selector}`)!.click();await settle();
    const rebuilt=(await automationAction("library-queue-state","volume",{volume:0.8})).playback;
    expect(rebuilt.queue).toHaveLength(2);
    expect(rebuilt.currentIndex).toBe(0);
    document.querySelector<HTMLButtonElement>('[aria-label="Next recording"]')!.click();await settle();
    const advanced=(await automationAction("library-next-state","volume",{volume:0.8})).playback;
    expect(advanced.trackId).toBe(secondTrack.id);
    expect(advanced.currentIndex).toBe(1);
  });

  it("keeps the playback queue stable through navigation and advances on ended", async () => {
    savedChartTracks = [{ ...savedTrack, rank: 1 }, { ...secondTrack, rank: 2 }];
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    (document.querySelector('[aria-label="Play Saved Recording"]') as HTMLButtonElement).click();
    await settle();
    (document.querySelector('[aria-label="Show playback queue and chapters"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelectorAll(".playback-queue-panel .queue-list article")).toHaveLength(2);
    [...document.querySelectorAll("nav button")].find((button) => button.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("ended", { bubbles: true }));
    await settle();
    expect(document.querySelectorAll(".playback-queue-panel .queue-list article")).toHaveLength(2);
    expect(document.querySelector(".playback-queue-panel .queue-list article.current strong")?.textContent).toBe("Another Recording");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
  });

  it("refreshes autotagged current and queued tracks without reordering playback", async () => {
    savedChartTracks = [{ ...savedTrack, rank: 1 }, { ...secondTrack, rank: 2 }];
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    (document.querySelector('[aria-label="Play Saved Recording"]') as HTMLButtonElement).click();
    await settle();
    (document.querySelector('[aria-label="Show playback queue and chapters"]') as HTMLButtonElement).click();
    await settle();

    const before = [...document.querySelectorAll(".playback-queue-panel .queue-list article")]
      .map((row) => row.querySelector("strong")?.textContent?.trim());
    expect(before).toEqual(["Saved Recording", "Another Recording"]);

    savedChartTracks = [
      { ...savedTrack, title: "Tagged Recording", album: "Reviewed Album", rank: 1 },
      { ...secondTrack, album: "Second Album", rank: 2 },
    ];
    mocks.call.mockClear();
    const status = eventHandlers.get("autotag-status");
    expect(status).toBeDefined();
    status!({ event: "autotag-status", id: 1, payload: { assetId: "asset-1", status: "applied" } });
    await settle();

    const rows = [...document.querySelectorAll(".playback-queue-panel .queue-list article")];
    expect(rows.map((row) => row.querySelector("strong")?.textContent?.trim()))
      .toEqual(["Tagged Recording", "Another Recording"]);
    expect(rows[0]?.querySelector("small")?.textContent).toContain("Reviewed Album");
    expect(rows[1]?.querySelector("small")?.textContent).toContain("Second Album");
    expect(rows[0]?.getAttribute("aria-current")).toBe("true");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Tagged Recording");
    expect(mocks.call).toHaveBeenCalledWith("get_tracks", { ids: ["recording-1", "recording-2"] });
    expect(mocks.call).toHaveBeenCalledWith("list_track_prefix", expect.objectContaining({ view: "library", limit: 2 }));
    expect(mocks.call.mock.calls.some(([command]) => command === "list_tracks")).toBe(false);
  });

  it("shows full-set chapter titles, seeks chapter audio relative to its bounds over MPRIS, and advances at the chapter end", async () => {
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), title: "Visible Artist — Full Set", durationSeconds: 100,
      chapters: [
        { id: "intro", title: "Intro", startTime: 0, endTime: 20, confidence: "review" },
        { id: "song-a", title: "Song A", startTime: 20, endTime: 60, confidence: "review" },
        { id: "outro", title: "Outro", startTime: 60, endTime: 100, confidence: "review" },
      ],
    };
    const fullSet = { ...savedTrack, id: "full-set", title: "Full Set", durationSeconds: 100, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [fullSet];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Full Set"]')!.click(); await settle();
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("play"));
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 25 });
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Song A");
    expect(document.querySelector(".player-track-artist")?.textContent).toBe("Full Set");

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    document.querySelector<HTMLElement>('[aria-label="More actions for Full Set"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button=>button.textContent==="Show chapters")!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play chapter Song A"]')!.click(); await settle();
    const chapterAudio = document.querySelector("audio")!;
    // The browser cannot apply the saved chapter seek until the media duration is known.
    chapterAudio.dispatchEvent(new Event("loadedmetadata")); await settle();
    expect(mocks.call).toHaveBeenCalledWith("register_chapter_track", expect.objectContaining({ track: expect.objectContaining({ playbackRange: expect.objectContaining({ startTime: 20, endTime: 60 }) }) }));
    const seek = await automationAction("chapter-relative-seek", "seek", { position: 900 });
    expect(seek.error).toBeNull();
    expect(seek.playback).toMatchObject({ position: 40, duration: 40 });
    chapterAudio.dispatchEvent(new Event("play"));
    Object.defineProperty(chapterAudio, "currentTime", { configurable: true, value: 60 });
    chapterAudio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Song A");
    expect(document.querySelector(".player-status")?.textContent).toContain("Paused");
    expect(play).toHaveBeenCalled();
  });

  it("adds a chapter through its own Queue menu using the inspected parent source and exact range", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const parent = { ...savedTrack, id: "playlist-chapter-parent", title: "Long Set", durationSeconds: 100 };
    const candidate: Candidate = {
      ...youtubeCandidate(parent), title: "Chapter Artist — Long Set", durationSeconds: 100,
      chapters: [{ id: "chapter-song", title: "Chapter Artist — Natural Song", startTime: 20, endTime: 60, confidence: "review" }],
    };
    const fullSet = { ...parent, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [fullSet];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Long Set"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    document.querySelector<HTMLElement>('[aria-label="More actions for Long Set"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Show chapters")!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="More actions for chapter Chapter Artist — Natural Song"]')!.click();
    await settle();
    const beforePlaybackCalls = mocks.call.mock.calls.filter(([command]) => ["playback_command", "resolve_candidate_audio", "play_tracks"].includes(String(command))).length;
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Add chapter to playlist")!.click();
    await settle();

    const registration = [...mocks.call.mock.calls].reverse().find(([command]) => command === "register_chapter_track");
    expect(registration).toBeDefined();
    expect((registration![1] as { track: Track }).track).toMatchObject({
      id: "chapter:playlist-chapter-parent:chapter-song", provider: "chapter", title: "Natural Song", artist: "Chapter Artist",
      playbackRange: { parentTrackId: parent.id, chapterId: "chapter-song", sourceUrl: candidate.url, startTime: 20, endTime: 60, parentTitle: candidate.title, parentDurationSeconds: 100 },
      provenance: { parentTrack: expect.objectContaining({ id: parent.id }), audioCandidate: expect.objectContaining({ id: candidate.id, url: candidate.url }) },
    });
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).toContain("Chapter Artist — Natural Song");
    expect(mocks.call.mock.calls.filter(([command]) => ["playback_command", "resolve_candidate_audio", "play_tracks"].includes(String(command)))).toHaveLength(beforePlaybackCalls);
  });

  it("does not let a late chapter registration replace a newer playlist picker", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const parent = { ...savedTrack, id: "picker-race-parent", title: "Long Set", durationSeconds: 100 };
    const candidate: Candidate = { ...youtubeCandidate(parent), title: "Chapter Artist — Long Set", durationSeconds: 100,
      chapters: [{ id: "chapter-song", title: "Chapter Artist — Natural Song", startTime: 20, endTime: 60, confidence: "review" }] };
    const fullSet = { ...parent, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [fullSet];
    const previousCall = mocks.call.getMockImplementation()!;
    let finishRegistration!: (track: Track) => void;
    let registeredChapter!: Track;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "register_chapter_track") { registeredChapter = args?.track as Track; return new Promise<Track>(resolve => { finishRegistration = resolve; }); }
      return previousCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Long Set"]')!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    document.querySelector<HTMLElement>('[aria-label="More actions for Long Set"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Show chapters")!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="More actions for chapter Chapter Artist — Natural Song"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Add chapter to playlist")!.click();
    await settle();
    expect(finishRegistration).toBeTypeOf("function");

    document.querySelector<HTMLElement>('[aria-label="More actions for Long Set"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Add to playlist")!.click();
    await settle();
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).toContain("Long Set");
    finishRegistration(registeredChapter);
    await settle();
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).toContain("Long Set");
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).not.toContain("Natural Song");
  });

  it("opens the full download history and virtualizes later job actions with stable identity", async () => {
    downloadJobs = Array.from({ length: 500 }, (_, index) => {
      const track = { ...savedTrack, id: `history-track-${index}`, title: `Historical job ${index}` };
      return {
        id: `job-${index}`, track, candidate: youtubeCandidate(track),
        state: index === 250 ? "failed" : "completed", progress: 0,
        createdAt: `2026-10-${String((index % 28) + 1).padStart(2, "0")}T12:00:00Z`,
        outputProfile: "source", formatId: null,
        ...(index === 250 ? { outputPath: "/music/history-250.flac" } : {}),
      } satisfies DownloadJob;
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    document.querySelector<HTMLButtonElement>('[aria-label="Downloads"]')!.click();
    await settle();
    const viewAll = document.querySelector<HTMLButtonElement>('[aria-label="View all downloads"]');
    expect(viewAll).not.toBeNull();
    viewAll!.click();
    await settle();

    const queue = document.querySelector<HTMLElement>(".queue")!;
    const host = queue.querySelector<HTMLElement>(".virtual-results")!;
    Object.defineProperty(queue, "clientHeight", { configurable: true, value: 500 });
    Object.defineProperty(queue, "clientWidth", { configurable: true, value: 700 });
    Object.defineProperty(host, "clientWidth", { configurable: true, value: 700 });
    queue.getBoundingClientRect = () => ({ top: 0, bottom: 500, left: 0, right: 700, width: 700, height: 500, x: 0, y: 0, toJSON() {} });
    host.getBoundingClientRect = () => ({ top: -queue.scrollTop, bottom: 56_000 - queue.scrollTop, left: 0, right: 700, width: 700, height: 56_000, x: 0, y: -queue.scrollTop, toJSON() {} });
    await settle();
    expect(queue.querySelectorAll(".job").length).toBeLessThan(30);
    expect(queue.querySelector("[data-virtual-index='0'] strong")?.textContent).toBe("Historical job 0");

    queue.scrollTop = 28_000;
    queue.dispatchEvent(new Event("scroll"));
    await settle();
    const laterRow = queue.querySelector<HTMLElement>("[data-virtual-index='250']");
    expect(laterRow?.querySelector("strong")?.textContent).toBe("Historical job 250");
    laterRow!.querySelector<HTMLButtonElement>("button")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("retry_download", { jobId: "job-250" });
    const refreshedRow = queue.querySelector<HTMLElement>("[data-virtual-index='250']")!;
    [...refreshedRow.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Show file")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_in_file_manager", { path: "/music/history-250.flac" });
  });

  it("keeps player information, queue, and download activity mutually exclusive", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    downloadJobs = [{
      id: "download-active", track: savedTrack,
      candidate: { ...youtubeCandidate(savedTrack), confidence: "high", score: 92,
        formats: [{ id: "flac", codec: "FLAC", extension: "flac", bitrate: 900, sampleRate: 44100, lossless: true }] },
      state: "downloading", progress: 61, createdAt: "2026-10-12T12:00:00Z", outputProfile: "source", formatId: "flac",
    }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    (document.querySelector('[aria-label="Play Saved Recording"]') as HTMLButtonElement).click();
    await settle();

    (document.querySelector('[aria-label="Show playing track information"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".playing-info")).not.toBeNull();
    expect(document.querySelector(".playback-queue-panel, .download-activity")).toBeNull();

    (document.querySelector('[aria-label="Show playback queue and chapters"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".playing-info")).toBeNull();
    expect(document.querySelector(".playback-queue-panel")).not.toBeNull();
    expect(document.querySelector(".download-activity")).toBeNull();

    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")]
      .find((button) => button.textContent?.includes("Downloads"))!.click();
    await settle();
    expect(document.querySelector(".playing-info, .playback-queue-panel")).toBeNull();
    expect(document.querySelector(".download-activity")?.textContent).toContain("Saved Recording");
    expect(document.querySelector(".download-activity progress")?.getAttribute("value")).toBe("61");

    const waitingJob: DownloadJob = {
      ...downloadJobs[0], id: "peer-wait", track: { ...savedTrack, title: "Mercy Of Time", artist: "Louie Vega, Msaki", version: "Louie Vega Mix" },
      candidate: { ...downloadJobs[0].candidate, provider: "slskd", source: { kind: "soulseek", username: "music-peer", filename: "Mercy.flac", size_bytes: 100 } },
      state: "waiting_for_peer", progress: 0,
      soulseek: { bytesReceived: 0, speedBytesPerSecond: 0, queuePosition: 884, attempts: 1, daemonState: "Queued, Remotely" },
    };
    const update = (job: DownloadJob) => eventHandlers.get("download-progress")?.({ event: "download-progress", id: 1, payload: job });
    update(waitingJob); await settle();
    const row = document.querySelector(".activity-job")!;
    expect(row.textContent).toContain("Mercy Of Time");
    expect(row.textContent).toContain("Louie Vega, Msaki");
    expect(row.textContent).toContain("Louie Vega Mix");
    expect(row.textContent).toContain("Soulseek");
    expect(row.textContent).toContain("music-peer");
    expect(row.textContent).toContain("Waiting for source");
    expect(row.textContent).toContain("Position 884 in source queue");
    expect(row.querySelector("progress, .progress-label")).toBeNull();
    update({ ...downloadJobs[0], progress: 65 }); await settle();
    expect(document.querySelector(".activity-job")).toBe(row);
    expect(document.querySelector(".download-activity progress")?.getAttribute("value")).toBe("65");
  });

  it("opens and closes the player queue through the bounded automation panel event", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const openRequestId = "queue-panel-request";
    eventHandlers.get("automation-player-panel")?.({ event: "automation-player-panel", id: 8,
      payload: { requestId: openRequestId, panel: "queue" } });
    await settle();
    expect(document.querySelector(".playback-queue-panel")).not.toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_player_panel_complete", {
      requestId: openRequestId, panel: "queue", status: "complete",
    });
    const closeRequestId = "close-panel-request";
    eventHandlers.get("automation-player-panel")?.({ event: "automation-player-panel", id: 9,
      payload: { requestId: closeRequestId, panel: "closed" } });
    await settle();
    expect(document.querySelector(".playback-queue-panel")).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_player_panel_complete", {
      requestId: closeRequestId, panel: "closed", status: "complete",
    });
  });

  it("finite playlist navigation acknowledges route separately from contents and restores Library Back Forward",async()=>{
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    const base=mocks.call.getMockImplementation()!;const id="11223344556677889900aa";
    let finish!:(value:unknown)=>void;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="get_managed_provider_session_status")return {spotifyAccountId:"navigation-fixture",accountRevisions:{spotify:1}};
      if(command==="browse_remote_playlist")return new Promise(resolve=>finish=resolve);
      return base(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const filter=document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;filter.value="preserved-navigation-filter";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    const go=async(requestId:string,target:unknown)=>{eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{requestId,target}});await settle();await settle();};
    await go("exact-playlist",{kind:"spotify_playlist",spotifyPlaylistId:id});
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_playlist",expect.objectContaining({provider:"spotify",playlistId:`https://open.spotify.com/playlist/${id}`}));
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"exact-playlist",destination:id,status:"complete"});
    expect(document.querySelector("#remote-playlist-detail-title")?.textContent).toBe("Remote playlist");
    finish({provider:"spotify",playlistId:id,title:"Verified navigation fixture",tracks:[savedTrack],nextCursor:null,total:1,complete:true});await settle();
    expect(document.querySelector("#remote-playlist-detail-title")?.textContent).toBe("Verified navigation fixture");
    await go("history-back",{kind:"history",historyDirection:"back"});
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"history-back",destination:"back",status:"complete"});
    expect(document.querySelector("main h1")?.textContent).toBe("Library");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')?.value).toBe("preserved-navigation-filter");
    await go("history-forward",{kind:"history",historyDirection:"forward"});
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"history-forward",destination:"forward",status:"complete"});
    expect(document.querySelector("#remote-playlist-detail-title")?.textContent).toBe("Verified navigation fixture");
    expect(mocks.call.mock.calls.some(([command])=>["open_external_url","import_remote_playlists","resolve_playback_track","playback_command"].includes(String(command)))).toBe(false);
  });
  it("exact playlist navigation from an already active source browser still acknowledges the requested playlist",async()=>{
    const base=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="get_managed_provider_session_status")return {spotifyAccountId:"same-view-fixture",accountRevisions:{spotify:1}};
      if(command==="browse_remote_playlist")return {provider:"spotify",playlistId:String(args?.playlistId),title:String(args?.playlistId),tracks:[],nextCursor:null,total:0,complete:true};
      return base(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    for(const id of ["11223344556677889900aa","11223344556677889900bb"]) {
      eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{requestId:id,target:{kind:"spotify_playlist",spotifyPlaylistId:id}}});await settle();await settle();
      expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:id,destination:id,status:"complete"});
      expect(document.querySelector("#remote-playlist-detail-title")?.textContent).toBe(`https://open.spotify.com/playlist/${id}`);
    }
  });
  it("history with no destination fails and cancelled playlist navigation never acknowledges success",async()=>{
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const handler=eventHandlers.get("automation-ui-navigation")!;
    handler({event:"automation-ui-navigation",id:1,payload:{requestId:"empty-history",target:{kind:"history",historyDirection:"back"}}});await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"empty-history",destination:"back",status:"failed"});
    handler({event:"automation-ui-navigation",id:2,payload:{requestId:"cancel-playlist",target:{kind:"spotify_playlist",spotifyPlaylistId:"11223344556677889900aa"}}});
    eventHandlers.get("automation-ui-navigation-cancelled")!({event:"automation-ui-navigation-cancelled",id:3,payload:{requestId:"cancel-playlist"}});await settle();await settle();
    expect(mocks.call.mock.calls.some(([command,args])=>command==="automation_ui_navigation_complete"&&args?.requestId==="cancel-playlist")).toBe(false);
  });

  it("does not add a duplicate Back destination when automation revisits the current view", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["playlist"], enabled: true },
        { id: "youtube", name: "YouTube", kinds: ["playlist"], enabled: true },
      ];
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const handler = eventHandlers.get("automation-ui-navigation")!;
    const go = async (requestId: string, target: unknown) => {
      handler({event:"automation-ui-navigation", id:1, payload:{requestId,target}});
      await settle(); await settle();
    };
    await go("downloads", {kind:"view",view:"downloads"});
    expect(document.querySelector('[aria-label="Sort download jobs"]')).not.toBeNull();
    await go("settings", {kind:"view",view:"settings"});
    expect(document.querySelector('section.settings[aria-label="Settings"]')).not.toBeNull();
    await go("settings-again", {kind:"view",view:"settings"});
    expect(document.querySelector('section.settings[aria-label="Settings"]')).not.toBeNull();
    await go("back", {kind:"history",historyDirection:"back"});
    expect(document.querySelector('[aria-label="Sort download jobs"]')).not.toBeNull();
    expect(document.querySelector('section.settings[aria-label="Settings"]')).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"back",destination:"back",status:"complete"});

    sourceGroup("spotify").querySelector<HTMLButtonElement>('[aria-label="Search Spotify playlists"]')!.click();
    await settle(); await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Search");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("spotify");
    sourceGroup("youtube").querySelector<HTMLButtonElement>('[aria-label="Search YouTube playlists"]')!.click();
    await settle(); await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Search");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("youtube");
    expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Playlists")?.getAttribute("aria-pressed")).toBe("true");
    await go("back-to-spotify", {kind:"history",historyDirection:"back"});
    expect(document.querySelector("main h1")?.textContent).toBe("Search");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("spotify");
    await go("back-to-downloads", {kind:"history",historyDirection:"back"});
    expect(document.querySelector('[aria-label="Sort download jobs"]')).not.toBeNull();
    expect(document.querySelector("main h1")?.textContent).not.toBe("Search");
  });

  it("opens Playback settings through finite category navigation and exposes the startup timeout", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{
      requestId:"navigate-playback-settings",target:{kind:"view",view:"settings",settingsCategory:"playback"},
    }});await settle();await settle();
    expect(document.querySelector('.settings-nav [aria-current="page"]')?.textContent).toBe("Playback");
    const timeout=[...document.querySelectorAll<HTMLLabelElement>(".settings-grid label")].find(label=>label.textContent?.startsWith("Playback startup timeout"))!.querySelector("select")!;
    expect([...timeout.options].map(option=>option.textContent)).toEqual(["20 seconds","30 seconds","45 seconds"]);
    expect(timeout.value).toBe("20");
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"navigate-playback-settings",destination:"settings",status:"complete"});
  });

  it("defaults legacy settings to Balanced and exposes exactly three recording match levels", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{
      requestId:"matching-level-playback",target:{kind:"view",view:"settings",settingsCategory:"playback"},
    }});
    await settle(); await settle();

    const control = document.querySelector<HTMLSelectElement>('[aria-label="Recording match level"]')!;
    expect(control.value).toBe("balanced");
    expect([...control.options].map(option => [option.value, option.textContent])).toEqual([
      ["exact", "Exact"], ["balanced", "Balanced"], ["flexible", "Flexible"],
    ]);
    expect(document.body.textContent).toContain("Allows metadata and credit variations while preserving the requested recording.");
  });

  it("saves Flexible matching with fresh settings and shares the selection with Downloads", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{
      requestId:"matching-level-playback-save",target:{kind:"view",view:"settings",settingsCategory:"playback"},
    }});
    await settle(); await settle();

    const control = document.querySelector<HTMLSelectElement>('[aria-label="Recording match level"]')!;
    control.value = "flexible";
    control.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const getSettingsIndex = mocks.call.mock.calls.findIndex(([command]) => command === "get_settings");
    const saveIndex = mocks.call.mock.calls.findIndex(([command]) => command === "save_settings");
    expect(getSettingsIndex).toBeGreaterThanOrEqual(0);
    expect(saveIndex).toBeGreaterThan(getSettingsIndex);
    expect((mocks.call.mock.calls[saveIndex]![1] as { settings: Settings }).settings).toMatchObject({
      matchingLevel: "flexible",
      preferredDownloadSource: "youtube",
      preferredStreamingSource: "youtube",
      streamingFallbackEnabled: true,
      streamingFallbackSources: ["youtube", "soundcloud"],
      downloadFallbackEnabled: false,
    });

    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")]
      .find(button => button.textContent?.trim() === "Downloads")!.click();
    await settle();
    const downloadsControl = document.querySelector<HTMLSelectElement>('[aria-label="Recording match level"]')!;
    expect(downloadsControl.value).toBe("flexible");
    expect(document.body.textContent).toContain("May use another live version, cover or remix; substitutions are labeled.");
  });

  it("scrolls to and focuses only the fixed startup timeout control before completing navigation", async () => {
    const previousScrollIntoView = HTMLElement.prototype.scrollIntoView;
    const previousRect = HTMLElement.prototype.getBoundingClientRect;
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype,"scrollIntoView",{configurable:true,value:scrollIntoView});
    Object.defineProperty(HTMLElement.prototype,"getBoundingClientRect",{configurable:true,value:()=>({x:20,y:30,top:30,right:220,bottom:70,left:20,width:200,height:40,toJSON:()=>({})})});
    try {
      const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
      eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{
        requestId:"focus-playback-timeout",target:{kind:"view",view:"settings",settingsCategory:"playback",settingsControl:"playbackStartupTimeout"},
      }});await settle();await settle();
      const control=document.getElementById("playback-startup-timeout") as HTMLSelectElement;
      expect(scrollIntoView).toHaveBeenCalledWith({block:"center",inline:"nearest"});
      expect(document.activeElement).toBe(control);
      const completionCall = mocks.call.mock.calls.findIndex(([name,args]) => name === "automation_ui_navigation_complete" && args?.requestId === "focus-playback-timeout");
      expect(scrollIntoView.mock.invocationCallOrder[0]).toBeLessThan(mocks.call.mock.invocationCallOrder[completionCall]);
      expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"focus-playback-timeout",destination:"settings",status:"complete"});
    } finally {
      if(previousScrollIntoView)Object.defineProperty(HTMLElement.prototype,"scrollIntoView",{configurable:true,value:previousScrollIntoView});
      else delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
      Object.defineProperty(HTMLElement.prototype,"getBoundingClientRect",{configurable:true,value:previousRect});
    }
  });

  it.each([
    {kind:"view",view:"settings",settingsCategory:"arbitrary"},
    {kind:"view",view:"library",settingsCategory:"playback"},
  ])("rejects invalid or mixed settings category navigation in the frontend: %j", async target => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{requestId:"invalid-settings-category",target}});await settle();
    expect(document.querySelector('section.settings')).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete",{requestId:"invalid-settings-category",destination:target.view,status:"failed"});
  });

  it("navigates the rendered app through bounded automation and rejects stale requests", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const successId = "navigate-settings-ok";
    eventHandlers.get("automation-ui-navigation")?.({ event:"automation-ui-navigation", id:71,
      payload:{requestId:successId,target:{kind:"view",view:"settings"}} });
    await settle(); await settle();
    expect(document.querySelector('section.settings[aria-label="Settings"]')).not.toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete", {
      requestId:successId,destination:"settings",status:"complete",
    });

    const staleId = "navigate-settings-stale";
    eventHandlers.get("automation-ui-navigation")?.({ event:"automation-ui-navigation", id:72,
      payload:{requestId:staleId,target:{kind:"view",view:"settings"}} });
    (document.querySelector('[aria-label="Library"]') as HTMLButtonElement).click();
    eventHandlers.get("automation-ui-navigation-cancelled")?.({ event:"automation-ui-navigation-cancelled", id:73,
      payload:{requestId:staleId} });
    await settle(); await settle();
    expect(document.querySelector('[aria-label="Library navigation"]')).not.toBeNull();
    expect(mocks.call.mock.calls.filter(([command,args]) => command === "automation_ui_navigation_complete" && (args as {requestId?:string})?.requestId === staleId)).toHaveLength(0);
  });

  it("opens an exact Spotify profile through finite automation and retains incomplete overlapping pages", async () => {
    const { providerNavigationSnapshots } = await import("./lib/provider-navigation-snapshots");
    providerNavigationSnapshots.clear();
    const originalCall = mocks.call.getMockImplementation()!;
    const owner = "dkkelsh1", ownerUrl = `https://open.spotify.com/user/${owner}`;
    const item = (id: string) => ({ id, title: `Profile list ${id}`, url: `https://open.spotify.com/playlist/${id}`, ownerId: owner, ownerUrl });
    const first = item("1234567890123456789012"), second = item("2234567890123456789012");
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 32 }, spotifyAccountId: "fixture-profile-account" };
      if (command === "list_remote_playlists") {
        expect(args?.provider).toBe("spotify"); expect(args?.ownerId).toBe(ownerUrl);
        return args?.cursor === "tail"
          ? { items: [], nextCursor: null, totalCount: 4, truncated: true, complete: false, snapshotConsistency: "progressive" }
          : args?.cursor === "second"
          ? { items: [first, second], nextCursor: "tail", totalCount: 4, truncated: true, complete: false, snapshotConsistency: "progressive" }
          : { items: [first], nextCursor: "second", totalCount: 4, complete: false, snapshotConsistency: "progressive" };
      }
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const requestId = "navigate-public-profile";
    eventHandlers.get("automation-ui-navigation")?.({ event: "automation-ui-navigation", id: 76, payload: { requestId, target: { kind: "spotify_profile", spotifyProfileId: owner } } });
    await settle(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete", { requestId, destination: owner, status: "complete" });
    expect(document.querySelector(".remote-profile-heading strong")?.textContent).toBe(ownerUrl);
    expect(document.querySelector(".remote-source-tabs")).toBeNull();
    expect(document.querySelectorAll(".remote-result-row")).toHaveLength(1);
    document.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(document.querySelectorAll(".remote-result-row")).toHaveLength(2);
    document.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(document.querySelectorAll(".remote-result-row")).toHaveLength(2);
    expect(document.querySelector(".remote-more")).toBeNull();
    expect(document.body.textContent).toContain("bounded or incomplete");
    expect(mocks.call.mock.calls.some(([command]) => command === "open_external_url" || command === "import_remote_playlists" || command === "playback_command")).toBe(false);
  });

  it("does not acknowledge UI navigation after its app unmounts", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    deferDependencyCheck = true;
    const requestId = "navigate-unmount-pending";
    eventHandlers.get("automation-ui-navigation")?.({ event:"automation-ui-navigation", id:74,
      payload:{requestId,target:{kind:"view",view:"settings"}} });
    await Promise.resolve();
    expect(finishDependencyCheck).toBeDefined();

    await unmount(instance);
    unmountApp = undefined;
    finishDependencyCheck!();
    await settle();
    expect(mocks.call.mock.calls.filter(([command,args]) => command === "automation_ui_navigation_complete" && (args as {requestId?:string})?.requestId === requestId)).toHaveLength(0);
  });

  it("refreshes newly confirmed Spotify capabilities before an automated profile search", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    let spotifyConfirmed = false;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["profile"], enabled: spotifyConfirmed },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.includes("spotify")) return {
        sourceId: "spotify", complete: true, results: [{ id: "spotify:fixture-profile", kind: "profile", sourceId: "spotify", title: "Fixture Profile", url: "https://open.spotify.com/user/fixture-profile" }],
      };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    spotifyConfirmed = true;
    const requestId = "fresh-spotify-profile-search";
    eventHandlers.get("automation-search-ui")?.({ event: "automation-search-ui", id: 81,
      payload: { requestId, query: "Fixture", sourceId: "spotify", kind: "profiles" } });
    await settle(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ sourceIds: ["spotify"], kinds: ["profile"] }));
    expect(mocks.call).toHaveBeenCalledWith("automation_search_ui_complete", expect.objectContaining({ requestId, status: "complete", resultCounts: { spotify: 1 } }));
    expect(document.body.textContent).toContain("Fixture Profile");
  });

  it("runs an app-specific search request without keyboard focus and returns counts only", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const requestId = "df680a60-2e12-44c9-98b5-7d4c04887c2a";
    eventHandlers.get("automation-search-ui")?.({ event: "automation-search-ui", id: 8,
      payload: { requestId, query: "Saved Recording", sourceId: "library", kind: "tracks" } });
    await settle();
    await settle();
    const calls = mocks.call.mock.calls.filter(([command]) => command === "automation_search_ui_complete");
    expect(calls).toHaveLength(1);
    expect(calls[0]?.[1]).toEqual({ requestId, status: "complete", sourceId: "library", kind: "tracks", resultCounts: { library: 1 }, sourceCoverage: { library: { hasMore: false, incomplete: false } } });
    expect(mocks.call.mock.calls.filter(([command]) => command === "search_source")).toHaveLength(1);
    expect(document.querySelector<HTMLInputElement>('input[placeholder="Track, artist, album, video or set"]')?.value).toBe("Saved Recording");
  });

  it("starts automation playback while animation frames are suspended in a background window", async () => {
    previewMode = "direct";
    vi.stubGlobal("requestAnimationFrame", () => 1);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")?.({ event: "automation-playback", id: 9,
      payload: { requestId: "background-play", action: "play", trackIds: [savedTrack.id] } });
    await settle();
    await settle();
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({requestId: "background-play", error: null, playback: expect.objectContaining({ paused: false, trackId: savedTrack.id })}));
  });

  it("waits for audio recovery before acknowledging automation playback success", async () => {
    previewMode = "direct";
    let rejectDirectPlay!: (reason: Error) => void;
    let resolveRecoveredPlay!: () => void;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => {
      if (play.mock.calls.length === 1) return new Promise((_resolve, reject) => { rejectDirectPlay = reject; });
      if (play.mock.calls.length === 2) return new Promise((resolve) => { resolveRecoveredPlay = () => resolve(); });
      return Promise.resolve();
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")?.({ event: "automation-playback", id: 10,
      payload: { requestId: "recover-play", action: "play", trackIds: [savedTrack.id] } });
    await settle();
    expect(play).toHaveBeenCalledTimes(1);

    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    expect(play).toHaveBeenCalledTimes(2);
    rejectDirectPlay(new Error("direct audio play failed"));
    await settle();
    expect(mocks.call.mock.calls.some(([command, args]) => command === "automation_playback_complete" && (args as { requestId: string }).requestId === "recover-play")).toBe(false);

    resolveRecoveredPlay();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({
      requestId: "recover-play", error: null,
      playback: expect.objectContaining({ trackId: savedTrack.id, paused: false }),
    }));
  });

  it("does not start a candidate returned after an automation playback request is cancelled", async () => {
    previewMode = "race";
    delayPlaybackTrackId = savedTrack.id;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")?.({ event: "automation-playback", id: 11,
      payload: { requestId: "cancel-pending-play", action: "play", trackIds: [savedTrack.id] } });
    await settle();
    expect(finishPlaybackResolve).toBeDefined();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: savedTrack }));

    eventHandlers.get("automation-playback-cancelled")?.({ event: "automation-playback-cancelled", id: 12,
      payload: { requestId: "cancel-pending-play" } });
    finishPlaybackResolve!({ candidate: youtubeCandidate(savedTrack), preview: { kind: "audio", url: "/late-cancelled.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false } });
    await settle();

    expect(mocks.call.mock.calls.some(([command]) => command === "start_candidate_preview")).toBe(false);
    expect(play).not.toHaveBeenCalled();
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({
      requestId: "cancel-pending-play", error: expect.any(String),
      playback: expect.objectContaining({lastStartupFailure:null}),
    }));
  });

  it("acknowledges an automation queue request and shares its queue with the player", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")?.({
      event: "automation-playback", id: 7,
      payload: { requestId: "request-queue-1", action: "queue", trackIds: [savedTrack.id], mode: "replace" },
    });
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({
      requestId: "request-queue-1", error: null,
      playback: expect.objectContaining({ queue: [expect.objectContaining({ id: savedTrack.id })] }),
    }));
    (document.querySelector('[aria-label="Show playback queue and chapters"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelectorAll(".playback-queue-panel .queue-list article")).toHaveLength(1);
    expect(document.querySelector(".playback-queue-panel .track-copy strong")?.textContent).toBe("Saved Recording");
  });

  it("hydrates automation queues by ID in request order, including repeated catalog tracks outside the current view", async () => {
    const importedTrack = { ...savedTrack, id: "imported-catalog-track", title: "Imported Catalog Track" };
    savedChartTracks = [];
    automationTrackLookup = [secondTrack, importedTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    mocks.call.mockClear();

    const result = await automationAction("catalog-queue-hydration", "queue", {
      trackIds: [importedTrack.id, secondTrack.id, importedTrack.id], mode: "replace",
    });

    expect(result.error).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("get_tracks", { ids: [importedTrack.id, secondTrack.id, importedTrack.id] });
    expect(mocks.call.mock.calls.some(([command, args]) => command === "list_tracks" && (args as { view?: string })?.view === "library")).toBe(false);
    expect(result.playback.queue).toEqual([
      expect.objectContaining({ id: importedTrack.id, title: importedTrack.title }),
      expect.objectContaining({ id: secondTrack.id, title: secondTrack.title }),
      expect.objectContaining({ id: importedTrack.id, title: importedTrack.title }),
    ]);
  });

  it("rejects automation requests when get_tracks omits any requested ID", async () => {
    automationTrackLookup = [savedTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const result = await automationAction("missing-automation-track", "queue", {
      trackIds: [savedTrack.id, "missing-track-id"], mode: "replace",
    });

    expect(result.error).toContain("Unknown or empty track IDs");
    expect(result.playback.queue).toEqual([]);
  });

  it("keeps playback queue changes when browser persistence fails and clears the warning after a successful save", async () => {
    const thirdTrack: Track = { ...savedTrack, id: "recording-3", title: "Third Recording" };
    const stored = new Map<string, string>();
    let failWrites = false;
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => {
        if (failWrites) throw new Error("quota exceeded");
        stored.set(key, value);
      },
      removeItem: (key: string) => stored.delete(key),
    });
    automationTrackLookup = [savedTrack, secondTrack, thirdTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const seeded = await automationAction("queue-save-seed", "queue", {
      trackIds: [savedTrack.id], mode: "replace",
    });
    expect(seeded.playback.queue).toEqual([expect.objectContaining({ id: savedTrack.id })]);
    expect(JSON.parse(stored.get("burrata.playback-queue.v1")!).tracks).toHaveLength(1);

    failWrites = true;
    const failedSave = await automationAction("queue-save-failure", "queue", {
      trackIds: [secondTrack.id], mode: "append",
    });
    expect((failedSave.playback.queue as { id: string }[]).map((track) => track.id)).toEqual([savedTrack.id, secondTrack.id]);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Your playback queue couldn't be saved");
    expect(JSON.parse(stored.get("burrata.playback-queue.v1")!).tracks).toHaveLength(1);

    await automationAction("queue-save-failure-repeat", "queue", {
      trackIds: [thirdTrack.id], mode: "append",
    });
    expect(document.querySelectorAll('[role="alert"]')).toHaveLength(1);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("changes may be lost after restarting Burrata");

    failWrites = false;
    const savedAgain = await automationAction("queue-save-recovered", "queue", {
      trackIds: [thirdTrack.id], mode: "append",
    });
    expect((savedAgain.playback.queue as { id: string }[]).map((track) => track.id)).toEqual([
      savedTrack.id, secondTrack.id, thirdTrack.id, thirdTrack.id,
    ]);
    expect(document.querySelector('[role="alert"]')?.textContent ?? "").not.toContain("Your playback queue couldn't be saved");
    expect(JSON.parse(stored.get("burrata.playback-queue.v1")!).tracks).toHaveLength(4);
  });

  it("applies MPRIS repeat, shuffle, and stop requests and acknowledges updated playback snapshots", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const started = await automationAction("mpris-start", "play", { trackIds: [savedTrack.id] });
    expect(started.error).toBeNull();
    const queued = await automationAction("mpris-queue", "queue", { trackIds: [secondTrack.id], mode: "append" });
    expect(queued.error).toBeNull();
    expect((queued.playback.queue as { id: string }[]).map((item) => item.id)).toEqual([savedTrack.id, secondTrack.id]);
    const repeated = await automationAction("mpris-repeat", "repeat", { mode: "one" });
    expect(repeated.error).toBeNull();
    expect(repeated.playback.repeat).toBe("one");
    const shuffled = await automationAction("mpris-shuffle", "shuffle", { mode: "true" });
    expect(shuffled.error).toBeNull();
    expect(shuffled.playback.shuffle).toBe(true);
    expect((shuffled.playback.queue as { id: string }[]).map((item) => item.id).sort()).toEqual([savedTrack.id, secondTrack.id].sort());
    const stopped = await automationAction("mpris-stop", "stop");
    expect(stopped.error).toBeNull();
    expect(stopped.playback.status).toBe("idle");
    expect(stopped.playback.position).toBe(0);
    expect((stopped.playback.queue as { id: string }[])).toHaveLength(2);
    expect(mocks.call.mock.calls.filter(([command]) => command === "automation_playback_complete").map(([, args]) => (args as { requestId: string }).requestId)).toContain("mpris-stop");
  });

  it("moves Next and Previous through a paused queue without resolving or replaying audio", async () => {
    previewMode = "direct";
    savedChartTracks = [savedTrack, secondTrack];
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const started = await automationAction("paused-queue-play", "play", { trackIds: [savedTrack.id, secondTrack.id] });
    expect(started.error).toBeNull();
    const paused = await automationAction("paused-queue-pause", "pause");
    expect(paused.error).toBeNull();
    expect(paused.playback.paused).toBe(true);
    const previewsBeforeSkip = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    const searchesBeforeSkip = mocks.call.mock.calls.filter(([command]) => command === "find_candidates").length;

    const next = await automationAction("paused-queue-next", "next");
    expect(next.error).toBeNull();
    expect(next.playback.currentIndex).toBe(1);
    expect(next.playback.trackId).toBe(secondTrack.id);
    expect(next.playback.repeat).toBe("off");
    expect(next.playback.paused).toBe(true);
    expect(next.playback.status).toBe("paused");
    expect(play).toHaveBeenCalledTimes(1);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewsBeforeSkip);
    expect(mocks.call.mock.calls.filter(([command]) => command === "find_candidates")).toHaveLength(searchesBeforeSkip);

    const previous = await automationAction("paused-queue-previous", "previous");
    expect(previous.error).toBeNull();
    expect(previous.playback.currentIndex).toBe(0);
    expect(previous.playback.trackId).toBe(savedTrack.id);
    expect(previous.playback.paused).toBe(true);
    expect(previous.playback.status).toBe("paused");
    expect(play).toHaveBeenCalledTimes(1);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview")).toHaveLength(previewsBeforeSkip);
    expect(mocks.call.mock.calls.filter(([command]) => command === "find_candidates")).toHaveLength(searchesBeforeSkip);
  });

  it("rejects an unsupported MPRIS OpenUri without searching or importing it", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const completion = await automationAction("unsafe-uri", "openUri", { mode: "file:///etc/passwd" });
    expect(completion.error).toContain("OpenUri supports saved library items and YouTube/SoundCloud audio links");
    expect(mocks.call.mock.calls.filter(([command]) => command === "search_audio_tracks")).toHaveLength(0);
    expect(mocks.call.mock.calls.filter(([command]) => ["find_candidates", "search_tracks", "import_local_file", "load_browser_import"].includes(String(command)))).toHaveLength(0);
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({ requestId: "unsafe-uri", error: expect.stringContaining("OpenUri supports") }));
  });

  it("records one successful listening session across pause and resume", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    (document.querySelector('[aria-label="Play Saved Recording"]') as HTMLButtonElement).click();
    await settle();
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("playing"));
    await settle();
    audio.dispatchEvent(new Event("pause"));
    audio.dispatchEvent(new Event("playing"));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "record_playback")).toHaveLength(1);
    [...document.querySelectorAll(".playlist-card")].find((button) => button.textContent?.includes("History"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).toContain("Saved Recording");
    expect(document.querySelector('.history-page .track-list-viewport')?.classList.contains("playlist")).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("search_playback_history", { options: expect.objectContaining({ limit: 200, offset: 0 }) });
  });

  it("restores paged history filters and virtual scroll through artist back and forward", async () => {
    historyEntries = Array.from({ length: 350 }, (_, index) => {
      const track = { ...savedTrack, id: `history-navigation-${index}`, title: `History recording ${index}`, durationSeconds: 180 };
      return { id: `history-navigation-${index}`, track, provider: "youtube", sessionId: `history-navigation-session-${index}`, playedAt: new Date(Date.UTC(2026, 0, 2, 0, index)).toISOString() };
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("History"))!.click();
    await settle();
    expect(document.querySelectorAll('.shared-track-list li:not(.virtual-spacer)').length).toBeLessThan(50);
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!;
    query.value = "History recording";
    query.dispatchEvent(new Event("input", { bubbles: true }));
    const source = document.querySelector<HTMLSelectElement>('[aria-label="History source"]')!;
    source.value = "youtube";
    source.dispatchEvent(new Event("change", { bubbles: true }));
    const order = document.querySelector<HTMLSelectElement>('[aria-label="History sort order"]')!;
    order.value = "oldest";
    order.dispatchEvent(new Event("change", { bubbles: true }));
    const trackSort = document.querySelector<HTMLSelectElement>('[aria-label="Sort playback history tracks"]')!;
    trackSort.value = "title";
    trackSort.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const direction = document.querySelector<HTMLSelectElement>('[aria-label="Playback history sort direction"]')!;
    direction.value = "desc";
    direction.dispatchEvent(new Event("change", { bubbles: true }));
    const from = document.querySelector<HTMLInputElement>('[aria-label="History from date"]')!;
    from.value = "2026-01-01";
    from.dispatchEvent(new Event("input", { bubbles: true }));
    const minimum = document.querySelector<HTMLInputElement>('[aria-label="History minimum duration"]')!;
    minimum.value = "60";
    minimum.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".history-filters")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const earlier = [...document.querySelectorAll<HTMLButtonElement>(".history-page button")].find(button => button.textContent?.trim() === "Show earlier plays")!;
    earlier.click();
    await settle();
    const viewport = document.querySelector<HTMLDivElement>('[aria-label=\"Playback history tracks\"]')!.parentElement as HTMLDivElement;
    viewport.scrollTop = 240 * 60;
    viewport.dispatchEvent(new Event("scroll"));
    await settle();
    (document.querySelector(".artist-link") as HTMLButtonElement).click();
    await settle();
    (document.querySelector(".artist-result") as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-heading")?.textContent).toContain("Visible Artist");

    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector<HTMLSelectElement>('[aria-label="History source"]')!.value).toBe("youtube");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!.value).toBe("History recording");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="History sort order"]')!.value).toBe("oldest");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort playback history tracks"]')!.value).toBe("title");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Playback history sort direction"]')!.value).toBe("desc");
    expect(document.querySelector<HTMLInputElement>('[aria-label="History from date"]')!.value).toBe("2026-01-01");
    expect(document.querySelector<HTMLInputElement>('[aria-label="History minimum duration"]')!.value).toBe("60");
    expect(document.querySelector<HTMLDivElement>('[aria-label=\"Playback history tracks\"]')!.parentElement!.scrollTop).toBe(240 * 60);
    expect(lastCallOf("search_playback_history")).toEqual(["search_playback_history", { options: expect.objectContaining({ query: "History recording", provider: "youtube", sort: "oldest", sortField: "title", direction: "desc", minDurationSeconds: 60, from: expect.stringContaining("2026-01-01"), offset: 200, limit: 200 }) }]);

    (document.querySelector('[aria-label="Go forward"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go forward"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-heading")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector<HTMLDivElement>('[aria-label=\"Playback history tracks\"]')!.parentElement!.scrollTop).toBe(240 * 60);

    (document.querySelector(".artist-link") as HTMLButtonElement).click();
    await settle();
    (document.querySelector(".artist-result") as HTMLButtonElement).click();
    await settle();
    deferHistoryRestorePage = true;
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(finishHistoryRestorePage).toBeDefined();
    const changedQuery = document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!;
    changedQuery.value = "History recording 1";
    changedQuery.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".history-filters")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    finishHistoryRestorePage!({ items: historyEntries.slice(200), total: historyEntries.length, hasMore: false });
    await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!.value).toBe("History recording 1");
    expect(document.querySelector<HTMLDivElement>('[aria-label=\"Playback history tracks\"]')!.parentElement!.scrollTop).toBe(0);
  // This functional scenario traverses several routes and restores two pages
  // repeatedly. Its harness timeout is separate from application latency gates.
  }, 10_000);

  it("keeps history search on a compact row and exposes advanced filters on demand", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("History"))!.click();
    await settle();

    const form = document.querySelector<HTMLFormElement>(".history-filters")!;
    const advanced = form.querySelector<HTMLDetailsElement>(".history-advanced-filters")!;
    expect(advanced.open).toBe(false);
    expect(form.querySelector('[aria-label="Search playback history"]')).not.toBeNull();
    expect(form.querySelector('[aria-label="History from date"]')?.closest("details")).toBe(advanced);
    expect(advanced.querySelector("summary")?.getAttribute("aria-label")).toBe("Show history filters");

    const summary = advanced.querySelector<HTMLElement>("summary")!;
    summary.click();
    expect(advanced.open).toBe(true);
    const source = advanced.querySelector<HTMLSelectElement>('[aria-label="History source"]')!;
    source.value = "youtube";
    source.dispatchEvent(new Event("change", { bubbles: true }));
    const from = advanced.querySelector<HTMLInputElement>('[aria-label="History from date"]')!;
    from.value = "2026-01-01";
    from.dispatchEvent(new Event("input", { bubbles: true }));
    advanced.querySelector<HTMLButtonElement>(".history-apply-filters")!.click();
    await settle();
    expect(mocks.call).toHaveBeenLastCalledWith("search_playback_history", {
      options: expect.objectContaining({ provider: "youtube", from: expect.stringContaining("2026-01-01") }),
    });
    from.focus();
    from.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(advanced.open).toBe(false);
    expect(document.activeElement).toBe(summary);
    summary.click();
    expect(advanced.open).toBe(true);
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(advanced.open).toBe(false);
  });

  it("searches history across the full result set before displaying its page", async () => {
    historyEntries = Array.from({ length: 105 }, (_, index) => {
      const track = { ...savedTrack, id: `history-track-${index}`, title: index === 0 ? "Lágrimas Negras" : `Unrelated recording ${index}` };
      return { id: `history-${index}`, track, provider: "youtube", sessionId: `session-${index}`, playedAt: new Date(Date.UTC(2026, 0, 1, 0, index)).toISOString() };
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find((button) => button.textContent?.includes("History"))!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!;
    input.value = "lágrimas";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".history-filters")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();

    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).toContain("Lágrimas Negras");
    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).not.toContain("Unrelated recording");
    expect(mocks.call).toHaveBeenCalledWith("search_playback_history", { options: expect.objectContaining({ query: "lágrimas", offset: 0, limit: 200 }) });
  });

  it("restarts history pagination when unsaved search edits are applied from the earlier button", async () => {
    historyEntries = Array.from({ length: 350 }, (_, index) => {
      const track = { ...savedTrack, id: `history-scope-${index}`, title: index < 250 ? `Match recording ${index}` : `Other recording ${index}` };
      return { id: `history-scope-${index}`, track, provider: "youtube", sessionId: `history-scope-session-${index}`, playedAt: new Date(Date.UTC(2026, 0, 1, 0, index)).toISOString() };
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("History"))!.click();
    await settle();
    expect([...document.querySelectorAll<HTMLButtonElement>(".history-page button")].some(button=>button.textContent?.trim()==="Show earlier plays")).toBe(true);

    const input = document.querySelector<HTMLInputElement>('[aria-label="Search playback history"]')!;
    input.value = "Match";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const apply = [...document.querySelectorAll<HTMLButtonElement>(".history-page button")].find(button => button.textContent?.trim() === "Apply filters and reload")!;
    apply.click();
    await settle();

    const rows = document.querySelector('[aria-label="Playback history tracks"]')?.textContent ?? "";
    expect(rows).toContain("Match recording");
    expect(rows).not.toContain("Other recording");
    expect(lastCallOf("search_playback_history")).toEqual(["search_playback_history", { options: expect.objectContaining({ query: "Match", offset: 0, limit: 200 }) }]);
    expect(document.querySelector(".history-page")?.textContent).toContain("Show earlier plays");
  });

  it("keeps history sort choices available while matching plays remain on later pages", async () => {
    historyEntries = Array.from({ length: 201 }, (_, index) => {
      const track = { ...savedTrack, id: `history-paged-${index}`, title: `Paged recording ${index}`,
        ...(index === 200 ? { bpm: 121 } : {}) };
      return { id: `history-paged-${index}`, track, provider: "youtube", sessionId: `history-paged-session-${index}`,
        playedAt: new Date(Date.UTC(2026, 0, 1, 0, index)).toISOString() };
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("History"))!.click();
    await settle();

    const bpm = document.querySelector<HTMLSelectElement>('[aria-label="Sort playback history tracks"] option[value="bpm"]')!;
    expect(bpm.disabled).toBe(false);
    const earlier = [...document.querySelectorAll<HTMLButtonElement>(".history-page button")].find(button => button.textContent?.trim() === "Show earlier plays")!;
    earlier.click();
    await settle();
    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).toContain("Paged recording 200");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort playback history tracks"] option[value="bpm"]')!.disabled).toBe(false);
  });

  it("opens full listening history for one station and clears that station when changing source", async () => {
    radioStationState = [{ id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    historyEntries = [
      { id: "main-play", sessionId: "main-play", provider: "radio", playedAt: "2026-10-14T18:30:00Z", track: { ...savedTrack, id: "radio-main", title: "Main Station Song", provenance: { radioStationId: "main" } } },
      { id: "other-play", sessionId: "other-play", provider: "radio", playedAt: "2026-10-14T18:31:00Z", track: { ...savedTrack, id: "radio-other", title: "Other Station Song", provenance: { radioStationId: "other" } } },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find((button) => button.textContent?.includes("Main Mix"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".full-history")!.click();
    await settle();

    expect(document.querySelector('[aria-label="Listening history"]')).not.toBeNull();
    const station = document.querySelector<HTMLSelectElement>('[aria-label="History station"]')!;
    expect(station.value).toBe("main");
    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).toContain("Main Station Song");
    expect(document.querySelector('[aria-label="Playback history tracks"]')?.textContent).not.toContain("Other Station Song");
    expect(mocks.call).toHaveBeenCalledWith("search_playback_history", { options: expect.objectContaining({ provider: "radio", stationId: "main" }) });

    const source = document.querySelector<HTMLSelectElement>('[aria-label="History source"]')!;
    source.value = "non-radio";
    source.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(document.querySelector('[aria-label="History station"]')).toBeNull();
    const historySearch = mocks.call.mock.calls.filter(([command]) => command === "search_playback_history").slice(-1)[0]!;
    expect((historySearch[1] as { options: Record<string, unknown> }).options).toMatchObject({ provider: "non-radio", stationId: null });
  });

  it("shows radio history enabled by default and persists the user's opt-out", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find((button) => button.textContent?.includes("Settings"))!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find((button) => button.textContent?.trim() === "Playback")!.click();
    await settle();
    const toggle = [...document.querySelectorAll<HTMLLabelElement>("label")]
      .find((label) => label.textContent?.includes("Include radio songs in playback history"))!
      .querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(toggle.checked).toBe(true);
    toggle.click();
    await settle();
    expect(radioHistoryEnabled).toBe(false);
    expect(mocks.call).toHaveBeenCalledWith("set_radio_history_setting", { enabled: false });
  });

  it("sends only explicitly selected chapter IDs in a bulk chapter download", async () => {
    configuredMusicDirectory = "/saved/music";
    chapterResults = [
      { id: "chapter-one", title: "Intro", startTime: 0, endTime: 30, confidence: "review" },
      { id: "chapter-two", title: "Main Set", startTime: 30, endTime: 180, confidence: "review" },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Download options…");
    const chapterOptions = document.querySelector<HTMLDetailsElement>(".chapter-download-options")!;
    chapterOptions.open = true;
    await settle();
    const split = chapterOptions.querySelector<HTMLInputElement>(".bulk-chapters input")!;
    expect(split).toBeDefined();
    split.click();
    await settle();
    const chapterChecks = [...chapterOptions.querySelectorAll<HTMLInputElement>(".chapter-select-checkbox")];
    expect(chapterChecks.map((input) => input.checked)).toEqual([false, false]);
    chapterChecks[0]!.click();
    await settle();
    const queueDownloadButton = chapterOptions.querySelector<HTMLButtonElement>(".download-selected-chapters")!;
    expect(queueDownloadButton.disabled, document.querySelector(".download-dialog")?.textContent).toBe(false);
    queueDownloadButton.click();
    await settle();
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { chapterIds: string[] } }).request.chapterIds).toEqual(["chapter-one"]);
  });

  it("shows one chapter at a time and downloads a chapter from its compact row", async () => {
    configuredMusicDirectory = "/saved/music";
    chapterResults = [
      { id: "chapter-one", title: "Intro", startTime: 0, endTime: 30, confidence: "review" },
      { id: "chapter-two", title: "Main Set", startTime: 30, endTime: 180, confidence: "review" },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Download options…");

    const chapters = document.querySelector<HTMLDetailsElement>(".chapter-download-options")!;
    chapters.open = true;
    await settle();
    chapters.querySelector<HTMLButtonElement>('[aria-label="Select chapter Main Set"]')!.click();
    await settle();
    expect(chapters.textContent).toContain("Main Set");
    expect(chapters.querySelectorAll(".chapter-download-row")).toHaveLength(0);
    expect(chapters.querySelector(".focused-chapter small")?.textContent).toContain("2:30");

    chapters.querySelector<HTMLButtonElement>(".change-chapter")!.click();
    await settle();
    expect(chapters.querySelectorAll(".chapter-download-row")).toHaveLength(2);
    chapters.querySelector<HTMLButtonElement>('[aria-label="Download chapter Intro"]')!.click();
    await settle();
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { chapterIds: string[] } }).request.chapterIds).toEqual(["chapter-one"]);
  });

  it("shows track artwork in the universal information panel and falls back when it fails", async () => {
    savedChartTracks = [{ ...savedTrack, artworkUrl: "https://img.example.test/cover.jpg" }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Track information");
    const image = document.querySelector(".playing-info .track-hero img") as HTMLImageElement;
    expect(image).not.toBeNull();
    image.dispatchEvent(new Event("error"));
    await settle();
    expect(document.querySelector(".playing-info .cover-placeholder")?.textContent).toBe("♫");
  });

  it("filters recordings using hydrated local availability and keeps all as the default", async () => {
    savedChartTracks = [
      { ...savedTrack, id: "local-recording", title: "Local Recording", localPath: "/music/local.mp3", localAvailability: "available" } as Track,
      { ...secondTrack, id: "missing-recording", localPath: "/removed/missing.mp3", localAvailability: "missing" } as Track,
      { ...savedTrack, id: "not-downloaded", title: "Not Downloaded", localAvailability: "none" } as Track,
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const localSelect = document.querySelector('[aria-label="Filter local recordings"]') as HTMLSelectElement;
    expect(document.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(3);
    localSelect.value = "local";
    localSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect([...document.querySelectorAll(".shared-track-list > li")].map((row) => row.textContent).join(" ")).toContain("Local Recording");
    expect([...document.querySelectorAll(".shared-track-list > li")].map((row) => row.textContent).join(" ")).not.toContain("Not Downloaded");
    localSelect.value = "not-local";
    localSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(document.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(2);
  });

  it("keeps the unified track list and rail navigation usable with track actions and playlist creation", async () => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector('[aria-label="Search"]')).not.toBeNull();
    expect(document.querySelector('[aria-label="Expand library menu"]')).not.toBeNull();
    expect(document.querySelector(".track-cards")).toBeNull();
    await chooseTrackAction("Add to playlist…");
    expect(document.querySelector("dialog.playlist-picker-dialog")).not.toBeNull();
    const name = document.querySelector('[aria-label="New playlist for track"]') as HTMLInputElement;
    name.value = "Morning Set";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    const createButton = [...document.querySelectorAll<HTMLButtonElement>("dialog.playlist-picker-dialog button")].find((button) => button.textContent?.includes("Create & add"))!;
    await settle();
    expect(createButton.disabled).toBe(false);
    createButton.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "Morning Set" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-new", tracks: [savedTrack] });
    expect(document.querySelector("h1")?.textContent).toBe("Library");
  });

  it("shows main-table batch checkboxes only after explicit selection and Cancel clears them without playback", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelectorAll(".track-list-viewport .select-track")).toHaveLength(0);
    document.querySelector<HTMLButtonElement>('[aria-label="Select tracks for batch actions"]')!.click();
    await settle();
    const checkbox = document.querySelector<HTMLInputElement>('.track-list-viewport input[aria-label="Select Saved Recording"]')!;
    expect(checkbox).not.toBeNull();
    checkbox.click();
    await settle();
    expect(checkbox.checked).toBe(true);
    [...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].find(button => button.textContent?.trim() === "Cancel selection")!.click();
    await settle();
    expect(document.querySelectorAll(".track-list-viewport .select-track")).toHaveLength(0);
    expect(document.querySelector('[aria-label="Select tracks for batch actions"]')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => ["play_tracks", "resolve_playback", "resolve_preview", "resolve_audio", "playback_command", "record_playback"].includes(String(command)))).toBe(false);
  });

  it("adds selected local playlist occurrences without collapsing duplicate tracks or playing audio", async () => {
    mockCollectionRows = [savedTrack, {...savedTrack}, {...secondTrack, id:"chapter-fixture", provider:"chapter", title:"Chapter", provenance:{chapterId:"chapter-one"}}];
    const instance = mount(App, {target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    const playlist = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Empty Collection"))!;
    playlist.click(); await settle();
    expect(document.querySelectorAll('input[aria-label^="Select occurrence "]')).toHaveLength(0);
    document.querySelector<HTMLButtonElement>('[aria-label="Select playlist tracks"]')!.click(); await settle();
    const select = (position:number) => { const input=document.querySelector<HTMLInputElement>(`[aria-label^="Select occurrence ${position}:"]`)!; input.click(); };
    const submit = async () => {
      [...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].find(button=>/^Add .* to playlist$/.test(button.textContent??""))!.click(); await settle();
      const destination=document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
      destination.value="collection-full"; destination.dispatchEvent(new Event("change",{bubbles:true}));await settle();
      [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button=>button.textContent==="Add to playlist")!.click();await settle();
    };
    select(1); await settle(); await submit();
    let writes=mocks.call.mock.calls.filter(([command])=>command==="append_playlist_entries");
    expect(writes[writes.length-1]?.[1]?.tracks).toHaveLength(1);
    // Choosing a destination must leave the source playlist and its rows in place.
    expect(document.querySelectorAll('input[aria-label^="Select occurrence "]')).toHaveLength(3);
    expect(document.querySelector<HTMLInputElement>('[aria-label^="Select occurrence 1:"]')!.checked).toBe(true);
    select(1); await settle();
    select(1);select(2);await settle();await submit();
    writes=mocks.call.mock.calls.filter(([command])=>command==="append_playlist_entries");
    const appended=writes[writes.length-1]?.[1]?.tracks as Track[];
    expect(appended).toHaveLength(2);
    expect(appended.map(track=>track.id)).toEqual([savedTrack.id,savedTrack.id]);
    select(1);select(2);await settle();select(3);await settle();
    const filter=document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value="Chapter";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    expect(document.querySelectorAll('input[aria-label^="Select occurrence "]')).toHaveLength(0);
    document.querySelector<HTMLButtonElement>('[aria-label="Select playlist tracks"]')!.click(); await settle();
    const bulk=[...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].find(button=>/^Add .* to playlist$/.test(button.textContent??""))!;
    expect(bulk.disabled).toBe(true);
    select(1); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".table-tools button")].find(button=>button.textContent==="Cancel selection")!.click(); await settle();
    expect(document.querySelectorAll('input[aria-label^="Select occurrence "]')).toHaveLength(0);
    expect(mocks.call.mock.calls.some(([command])=>["play_tracks","resolve_playback","resolve_preview","resolve_audio"].includes(command))).toBe(false);
  });

  it("closes the playlist picker on native Escape cancel and restores focus without changing a playlist", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const returnTarget=document.querySelector<HTMLButtonElement>("main > header .header-actions > button")!;
    returnTarget.focus();await chooseTrackAction("Add to playlist…");
    const dialog=document.querySelector<HTMLDialogElement>(".playlist-picker-dialog")!;
    const before=mocks.call.mock.calls.filter(([command])=>command==="add_to_collection").length;
    const cancel=new Event("cancel",{cancelable:true});dialog.dispatchEvent(cancel);await settle();
    expect(cancel.defaultPrevented).toBe(true);expect(document.querySelector(".playlist-picker-dialog")).toBeNull();
    expect(document.activeElement).toBe(returnTarget);
    expect(mocks.call.mock.calls.filter(([command])=>command==="add_to_collection")).toHaveLength(before);
  });
  it("cannot add to a hidden playlist destination from either the button or Enter", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    await chooseTrackAction("Add to playlist…");
    const select=document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    select.value="collection-full";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    const search=document.querySelector<HTMLInputElement>('[aria-label="Search playlist destinations"]')!;
    search.value="no destination matches";search.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    const add=[...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button=>button.textContent==="Add to playlist")!;
    expect(add.disabled).toBe(true);const before=mocks.call.mock.calls.filter(([command])=>command==="add_to_collection").length;
    // Enter calls the handler directly even when the hidden ID remains bound.
    select.dispatchEvent(new KeyboardEvent("keydown",{key:"Enter",bubbles:true}));await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="add_to_collection")).toHaveLength(before);
    expect(document.querySelector(".playlist-picker-dialog")).not.toBeNull();
    search.value="";search.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    select.value="collection-full";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(add.disabled).toBe(false);
  });

  it("keeps playlist creation available for empty search results and supports keyboard destination selection", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Add to playlist…");
    const search = document.querySelector<HTMLInputElement>('[aria-label="Search playlist destinations"]')!;
    expect(document.activeElement).toBe(search);
    search.value = "no playlist matches this";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(document.querySelector('[role="status"]')?.textContent).toContain("No matching playlists");
    expect(document.querySelector('[aria-label="New playlist for track"]')).not.toBeNull();
    const createName = document.querySelector<HTMLInputElement>('[aria-label="New playlist for track"]')!;
    createName.value = "Quiet Morning";
    createName.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    document.querySelector<HTMLButtonElement>("dialog.playlist-picker-dialog footer .primary")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "Quiet Morning" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-new", tracks: [expect.objectContaining({ id: savedTrack.id })] });

    await chooseTrackAction("Add to playlist…");
    const destinationSearch = document.querySelector<HTMLInputElement>('[aria-label="Search playlist destinations"]')!;
    destinationSearch.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    const destination = document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    expect(document.activeElement).toBe(destination);
    destination.value = "collection-full";
    destination.dispatchEvent(new Event("change", { bubbles: true }));
    destination.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-full", tracks: [expect.objectContaining({ id: savedTrack.id })] });
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
  });

  it("opens the universal track information panel and adds its selected track to a playlist", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Track information");
    const details = document.querySelector<HTMLElement>(".playing-info")!;
    expect(details.textContent).toContain("Saved Recording");
    expect(details.textContent).toContain("Visible Artist");
    expect(document.querySelectorAll(".playing-info")).toHaveLength(1);
    expect(document.querySelector(".details")).toBeNull();
    const add = [...details.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Add to playlist"))!;
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
    add.click();
    await settle();
    expect(document.querySelector("dialog.playlist-picker-dialog")).not.toBeNull();
  });

  it("downloads a playlist through its context menu using the preferred source without navigating or starting playback", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    batchInspectionHigh = true;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    rightClickPlaylist("Empty Collection");
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Download playlist")!.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "collections", collectionId: "collection-empty" });
    expect(mocks.call).toHaveBeenCalledWith("find_candidates", { track: collectionTrack, providerIds: ["soundcloud"] });
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.provider).toBe("soundcloud");
    expect(mocks.call.mock.calls.filter(([command]) => command === "enqueue_download")).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => ["resolve_preview", "resolve_candidate_audio"].includes(String(command)))).toBe(false);
    expect(play).not.toHaveBeenCalled();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
  });

  it("opens standalone artist track information with one playlist action and no inline collection selector", async () => {
    artistProfileTopTracks = [savedTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    const topTrackPlay = document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!;
    const topTrack = topTrackPlay.closest("li")!;
    topTrack.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>("dialog.track-menu")!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Track information")!.click();
    await settle();

    const info = document.querySelector<HTMLElement>(".playing-info")!;
    const playlistActions = [...info.querySelectorAll<HTMLButtonElement>("button")].filter((button) => button.textContent?.trim() === "Add to playlist");
    expect(playlistActions).toHaveLength(1);
    expect(info.querySelector("select")).toBeNull();
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
    playlistActions[0]!.click();
    await settle();
    expect(document.querySelector("dialog.playlist-picker-dialog")).not.toBeNull();
  });

  it("navigates exact catalog artist sections without search, playback or external navigation", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const handler = eventHandlers.get("automation-ui-navigation")!;
    for (const [index, artist, section] of [
      [1, { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000" }, "releases"],
      [2, { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000" }, "tracks"],
      [3, { provider: "discogs", id: "12345" }, "overview"],
    ] as const) {
      handler({ event: "automation-ui-navigation", id: index, payload: {
        requestId: `catalog-section-${index}`, target: { kind: "catalog_artist", catalogArtist: artist, catalogArtistSection: section },
      } });
      await settle(); await settle();
      expect(mocks.call).toHaveBeenCalledWith("get_artist_profile", { artist });
      expect(document.querySelector(".artist-page .tabs button.active")?.textContent).toBe(section === "releases" ? "Discography" : section === "tracks" ? "Tracks" : "Overview");
      expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete", {
        requestId: `catalog-section-${index}`, destination: artist.id, status: "complete",
      });
    }
    expect(mocks.call).toHaveBeenCalledWith("get_artist_tracks", expect.objectContaining({ artist: { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000" } }));
    expect(mocks.call.mock.calls.some(([command]) => ["search_artists", "open_external_url", "resolve_playback_track", "playback_command"].includes(String(command)))).toBe(false);
  });

  it("rejects invalid catalog navigation bridge targets before changing route", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const handler = eventHandlers.get("automation-ui-navigation")!;
    for (const [index, artist, section] of [
      [1, { provider: "musicbrainz", id: "not-a-uuid" }, "tracks"],
      [2, { provider: "discogs", id: "18446744073709551616" }, "overview"],
      [3, { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000" }, "local"],
      [4, { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000", script: "ignored" }, "overview"],
    ] as const) {
      handler({ event: "automation-ui-navigation", id: index, payload: { requestId: `invalid-catalog-${index}`,
        target: { kind: "catalog_artist", catalogArtist: artist, catalogArtistSection: section } } });
      await settle();
      expect(mocks.call).toHaveBeenCalledWith("automation_ui_navigation_complete", {
        requestId: `invalid-catalog-${index}`, destination: artist.id, status: "failed",
      });
    }
    expect(document.querySelector(".artist-page")).toBeNull();
    expect(mocks.call).not.toHaveBeenCalledWith("get_artist_profile", expect.anything());
  });

  it("opens ArtistPage source and provider anchors externally on desktop, including keyboard and middle clicks", async () => {
    artistProfileLinks = [
      { label: "Discogs", provider: "discogs", url: "https://www.discogs.com/artist/123" },
      { label: "Unsafe", provider: "test", url: "javascript:alert(1)" },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar button[aria-label="Library"]')?.click();
    await settle();
    [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li")].find((row) => row.textContent?.includes("Saved Recording"))!
      .querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();

    const sourceProfile = [...document.querySelectorAll<HTMLAnchorElement>(".artist-page a[target='_blank']")]
      .find((anchor) => anchor.textContent?.includes("Open source profile"))!;
    const keyboardClick = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, detail: 0 });
    sourceProfile.dispatchEvent(keyboardClick);
    await settle();
    expect(keyboardClick.defaultPrevented).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://musicbrainz.org/artist/artist-1" });

    const beforeMusicBrainzMiddle = mocks.call.mock.calls.filter(([command, args]) =>
      command === "open_external_url" && (args as { url?: string } | undefined)?.url === sourceProfile.href).length;
    const musicBrainzMiddle = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    sourceProfile.dispatchEvent(musicBrainzMiddle);
    await settle();
    expect(musicBrainzMiddle.defaultPrevented).toBe(true);
    expect(mocks.call.mock.calls.filter(([command, args]) =>
      command === "open_external_url" && (args as { url?: string } | undefined)?.url === sourceProfile.href))
      .toHaveLength(beforeMusicBrainzMiddle + 1);

    const providerLink = [...document.querySelectorAll<HTMLAnchorElement>(".artist-page a[target='_blank']")]
      .find((anchor) => anchor.textContent?.includes("Discogs"))!;
    const middleClick = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    providerLink.dispatchEvent(middleClick);
    await settle();
    expect(middleClick.defaultPrevented).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://www.discogs.com/artist/123" });

    const unsafe = [...document.querySelectorAll<HTMLAnchorElement>(".artist-page a[target='_blank']")]
      .find((anchor) => anchor.textContent?.includes("Unsafe"))!;
    const unsafeClick = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    unsafe.dispatchEvent(unsafeClick);
    await settle();
    expect(unsafeClick.defaultPrevented).toBe(true);
    expect(mocks.call.mock.calls.some(([, args]) => (args as { url?: string } | undefined)?.url?.startsWith("javascript:"))).toBe(false);

    const handled = document.createElement("a");
    handled.href = "https://example.org/handled";
    handled.target = "_blank";
    handled.addEventListener("click", (event) => event.preventDefault());
    document.body.append(handled);
    const ownHandlerClick = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    handled.dispatchEvent(ownHandlerClick);
    await settle();
    expect(ownHandlerClick.defaultPrevented).toBe(true);
    expect(mocks.call).not.toHaveBeenCalledWith("open_external_url", { url: "https://example.org/handled" });
    handled.remove();
  });

  it("leaves ArtistPage external anchors to browser preview navigation", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li")].find((row) => row.textContent?.includes("Saved Recording"))!
      .querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();

    const link = [...document.querySelectorAll<HTMLAnchorElement>(".artist-page a[target='_blank']")]
      .find((anchor) => anchor.textContent?.includes("Open source profile"))!;
    mocks.isDesktop = false;
    const click = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    link.dispatchEvent(click);
    await settle();
    expect(click.defaultPrevented).toBe(false);
    expect(mocks.call).not.toHaveBeenCalledWith("open_external_url", { url: "https://musicbrainz.org/artist/artist-1" });
  });

  it("searches Soulseek files through the cancellable polling API", async () => {
    configuredMusicDirectory = "/saved/music";
    slskdConnectionEnabled = true;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    partialSoulseekSearch = true;
    await searchDiscovery("Peer Shared Recording", ["Soulseek"]);
    await new Promise((resolve) => setTimeout(resolve, 950));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Peer Shared Recording", sourceIds: ["soulseek"] }));
    expect(mocks.call).toHaveBeenCalledWith("poll_search_source", { searchId: "slskd-search-1", sourceId: "soulseek" });
    expect(document.querySelector(".source-results")?.textContent).toContain("Peer Shared Recording");
    document.querySelector('[aria-label="Download Peer Shared Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector('[aria-label="slskd sources"] .candidate')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    await confirmReviewedDownload("slskd");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.source?.kind).toBe("soulseek");
    expect(mocks.call.mock.calls.some(([command, args]) => command === "inspect_candidate" && (args as { candidate?: Candidate }).candidate?.provider === "slskd")).toBe(false);
  });

  it("uses Soulseek search selection instead of the saved YouTube preference for catalog results", async () => {
    configuredMusicDirectory = "/saved/music";
    slskdConnectionEnabled = true;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await searchDiscovery("Visible Artist Saved Recording", ["Soulseek"]);
    const row = [...document.querySelectorAll(".result-row")].find((item) => item.textContent?.includes("Peer Shared Recording"))!;
    row.querySelector('[aria-label="Download Peer Shared Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ sourceIds: ["soulseek"] }));
    expect(document.querySelector('[aria-label="slskd sources"] .candidate')).not.toBeNull();
    await confirmReviewedDownload("slskd");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.provider).toBe("slskd");
  });

  it("clears stale YouTube results when changing Search source to Soulseek", async () => {
    configuredMusicDirectory = "/saved/music";
    slskdConnectionEnabled = true;
    audioSearchResult = [{ ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: youtubeCandidate(savedTrack) } }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await searchDiscovery("Visible Artist Saved Recording", ["YouTube"]);
    expect(document.querySelector(".source-results")?.textContent).toContain("Saved Recording");
    await selectSearchSources(["Soulseek"]);
    expect(document.querySelector(".source-results")?.textContent).not.toContain("Saved Recording");
    expect(document.querySelector('[aria-label="Download Saved Recording"]')).toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);

    submitSearch();
    await settle();
    expect(document.querySelector(".source-results")?.textContent).toContain("Peer Shared Recording");
    document.querySelector('[aria-label="Download Peer Shared Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector('[aria-label="slskd sources"] .candidate')).not.toBeNull();
    await confirmReviewedDownload("slskd");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.source?.kind).toBe("soulseek");
  });

  it("saves independent fallback switches and source order without unrelated edits", async () => {
    configuredPreferredStreamingSource="monochrome";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find(item=>item.textContent?.includes("Settings"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const source=document.querySelector('[aria-label="Streaming fallback 1"]') as HTMLSelectElement;
    source.value="soundcloud";source.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackSources).toEqual(["soundcloud","youtube"]);
    const off=document.querySelector('[aria-label="Enable streaming fallback"]') as HTMLInputElement;
    off.checked=false;off.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackEnabled).toBe(false);expect(source.disabled).toBe(true);
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Downloads")!.click();await settle();
    const downloads=document.querySelector('[aria-label="Enable download fallback"]') as HTMLInputElement;
    downloads.checked=true;downloads.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(downloadFallbackEnabled).toBe(true);expect(streamingFallbackEnabled).toBe(false);
    expect((mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]![1] as {settings:Settings}).settings.streamingFallbackSources).toEqual(["soundcloud","youtube"]);
  });

  it("can disable saved-radio preparation without changing streaming fallback", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find(item=>item.textContent?.includes("Settings"))!.click();await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const preparation=document.querySelector<HTMLInputElement>('[aria-label="Prepare saved radio songs"]')!;
    expect(preparation.checked).toBe(true);
    preparation.checked=false;preparation.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    const save=mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]!;
    expect((save[1] as {settings:Settings}).settings).toMatchObject({backgroundSourcePreparation:false,streamingFallbackEnabled:true});
  });

  it("saves playback preferences without saving an unfinished music-folder edit", async () => {
    configuredMusicDirectory="/saved/music";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find(item=>item.textContent?.includes("Settings"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Library")!.click();await settle();
    const folder=document.querySelector('input[placeholder="/home/you/Music/DJ"]') as HTMLInputElement;
    folder.value="/unsaved/music";folder.dispatchEvent(new Event("input",{bubbles:true}));
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const delay=document.querySelector('input[type="number"][max="5000"]') as HTMLInputElement;
    delay.value="500";delay.dispatchEvent(new Event("input",{bubbles:true}));
    [...document.querySelectorAll("button")].find(item=>item.textContent==="Save playback preferences")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    const save=mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]!;
    expect((save[1] as {settings:Settings}).settings).toMatchObject({musicDirectory:"/saved/music",previewDelayMs:500});
    expect(folder.value).toBe("/unsaved/music");
  });

  it("saves preferred source immediately without persisting unrelated typed settings", async () => {
    configuredMusicDirectory = "/saved/music";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-footer button")].find((item) => item.textContent?.includes("Settings"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find((item) => item.textContent?.trim()==="Library")!.click();
    await settle();
    const folder = document.querySelector('input[placeholder="/home/you/Music/DJ"]') as HTMLInputElement;
    folder.value = "/unsaved/music";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find((item) => item.textContent?.trim()==="Downloads")!.click();
    await settle();
    const source = document.querySelector('[aria-label="Preferred download source"]') as HTMLSelectElement;
    source.value = "soundcloud";
    source.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const save = mocks.call.mock.calls.find(([command]) => command === "save_settings");
    expect((save?.[1] as { settings: Settings }).settings).toMatchObject({ musicDirectory: "/saved/music", preferredDownloadSource: "soundcloud" });
    expect(folder.value).toBe("/unsaved/music");
    expect(document.body.textContent).toContain("Preferred download source saved: SoundCloud.");
  });

  it("queues an artist album without interrupting playback, then plays its full ordered tracklist", async () => {
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const originalCall = mocks.call.getMockImplementation()!;
    const albumTracks = [{...savedTrack,id:"album-first",title:"First album song"},{...savedTrack,id:"album-second",title:"Second album song"}];
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_release_tracks") return albumTracks;
      if (command === "get_artist_profile") {
        const result = await originalCall(command,args) as Record<string,unknown>;
        return {...result,releases:{artist:args?.artist,sourceCoverage:[],warnings:[],items:[{
          release:{provider:"musicbrainz",id:"album"},title:"Test Album",year:2001,primaryType:"Album",tracks:[],credits:[],sourceUrl:"https://musicbrainz.org/release/album",
        }]}};
      }
      return originalCall(command,args);
    });
    const instance = mount(App, {target:document.body}); unmountApp=()=>{void unmount(instance);};
    await settle();
    document.querySelector(".shared-track-list > li")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));
    await settle();
    const beforePlayCount=play.mock.calls.length;
    (document.querySelector(".artist-link") as HTMLButtonElement).click(); await settle();
    (document.querySelector(".artist-result") as HTMLButtonElement).click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".tabs button")].find(button=>button.textContent==="Discography")!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button=>button.textContent==="Queue")!.click(); await settle();
    expect(play.mock.calls.length).toBe(beforePlayCount);
    expect(document.querySelector(".now-playing")?.textContent).toContain(savedTrack.title);
    (document.querySelector('[aria-label="Show playback queue and chapters"]') as HTMLButtonElement).click(); await settle();
    expect(document.querySelector('[aria-label="Playback queue"]')?.textContent).toContain("First album song");
    expect(document.querySelector('[aria-label="Playback queue"]')?.textContent).toContain("Second album song");
    [...document.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button=>button.textContent==="Play album")!.click(); await settle();
    expect(document.querySelector(".now-playing")?.textContent).toContain("First album song");
    expect(play.mock.calls.length).toBe(beforePlayCount+1);
    const rows=document.querySelectorAll('[aria-label="Queued tracks"] article');
    expect(rows.length).toBe(2);
    expect(rows[0].textContent).toContain("First album song");
    expect(rows[1].textContent).toContain("Second album song");
  });

  it("continues the ArtistPage album queue after a middle recording has no WebView source", async () => {
    previewMode = "fallback";
    const album = {
      release: { provider: "musicbrainz", id: "4f7f6950-29cc-448d-a25b-445e383612f1" },
      title: "A Portrait of Nikolai Kaufman", year: 1981, primaryType: "Album", trackCount: 3,
      sourceUrl: "https://musicbrainz.org/release/4f7f6950-29cc-448d-a25b-445e383612f1", tracks: [], credits: [],
    };
    const albumTracks = [
      { ...savedTrack, id: "kaufman-playable-first", title: "Playable album opener", album: album.title },
      { ...secondTrack, id: "catalog:musicbrainz:4f7f6950-29cc-448d-a25b-445e383612f1:1:2:1:d0b8d1b1-05c3-4422-ae75-a5aae859d8dd", title: "Duda E Bolna Legnada (Duda Lays Sick)", artist: "Le Mystère Des Voix Bulgares", durationSeconds: 162.32, album: album.title },
      { ...savedTrack, id: "kaufman-playable-following", title: "Playable following recording", album: album.title },
    ];
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_release_tracks") return albumTracks;
      if (command === "get_artist_profile") {
        const result = await originalCall(command, args) as Record<string, unknown>;
        return { ...result, releases: { artist: args?.artist, sourceCoverage: [], warnings: [], items: [album] } };
      }
      if (command === "resolve_playback_track" && (args?.track as Track | undefined)?.id === albumTracks[1]!.id) {
        throw new Error("No playable audio match was found for Duda E Bolna Legnada (Duda Lays Sick)");
      }
      return originalCall(command, args);
    });

    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector(".shared-track-list > li")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    (document.querySelector(".artist-link") as HTMLButtonElement).click(); await settle();
    (document.querySelector(".artist-result") as HTMLButtonElement).click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".tabs button")].find(button => button.textContent === "Discography")!.click(); await settle();
    const beforeAlbumCalls = mocks.call.mock.calls.length;
    [...document.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button => button.textContent === "Play album")!.click(); await settle(); await settle();

    expect(document.querySelector(".now-playing")?.textContent).toContain("Playable album opener");
    document.querySelector<HTMLAudioElement>("audio")!.dispatchEvent(new Event("ended"));
    for (let index = 0; index < 6; index++) await settle();

    const resolvedIds = mocks.call.mock.calls.slice(beforeAlbumCalls)
      .filter(([command]) => command === "resolve_playback_track")
      .map(([, args]) => (args.track as Track).id);
    expect(resolvedIds).toEqual(albumTracks.map(track => track.id));
    const snapshot = (await automationAction("album-webview-skip-inspect", "volume", { volume: 0.8 })).playback;
    expect((snapshot.queue as Track[]).map(track => track.id)).toEqual(albumTracks.map(track => track.id));
    expect(snapshot.currentIndex).toBe(2);
    expect(snapshot.trackId).toBe(albumTracks[2]!.id);
    expect(snapshot.paused).toBe(false);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playable following recording");
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Skipped Duda E Bolna Legnada");
    expect(play).toHaveBeenCalled();
  });

  it("opens a searched album into a playable release view and keeps selected sources across result types", async () => {
    const storage = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    });
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    discoveryAlbum = {
      release: { provider: "musicbrainz", id: "search-album-1" }, title: "Searched Album", year: 2021,
      primaryType: "Album", trackCount: 2, sourceUrl: "https://musicbrainz.org/release/search-album-1", tracks: [],
      credits: [{ name: "Visible Artist", role: "artist", artist: { provider: "musicbrainz", id: "artist-1" }, sourceUrl: "https://musicbrainz.org/artist/artist-1" }],
    };
    discoveryAlbumTracks = [
      { ...savedTrack, id: "release-track-1", title: "Release Song One", album: "Searched Album" },
      { ...secondTrack, id: "release-track-2", title: "Release Song Two", album: "Searched Album" },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find((item) => item.textContent?.includes("Search"))!.click();
    await settle();
    await selectSearchSources(["MusicBrainz"]);
    expect(JSON.parse(localStorage.getItem("burrata.discovery.search-scope.v2") ?? "{}"))
      .toMatchObject({ mode: "online", sourceId: "musicbrainz" });
    const albumsFilter = [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Albums")!;
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Albums")!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Tracks")!.click(); await settle();
    expect((document.querySelector('[aria-label="Search in"]') as HTMLSelectElement).value).toBe("musicbrainz");
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Albums")!.click(); await settle();
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Searched Album"; query.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();

    expect(document.querySelector(".release-view h1")?.textContent).toBe("Searched Album");
    expect(mocks.call).toHaveBeenCalledWith("get_release_tracks", expect.objectContaining({ release: discoveryAlbum, artist: discoveryAlbum.credits[0].artist }));
    expect(document.body.textContent).toContain("Release Song One");
    document.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click(); await settle();
    expect(document.querySelector(".now-playing")?.textContent).toContain("Release Song One");
    expect(play).toHaveBeenCalled();
  });

  it("opens artist discovery from a track without interrupting the current preview", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    row.querySelector('[aria-label="Find recordings by Visible Artist"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".artist-page")?.textContent).toContain("Artist discovery");
    expect(mocks.call).toHaveBeenCalledWith("search_artists", { query: "Visible Artist", page: 1, pageSize: 20 });
    expect(document.querySelector(".now-playing")?.textContent).toContain("Saved Recording");
    document.querySelector(".artist-result")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_artist_profile", { artist: { provider: "musicbrainz", id: "artist-1" } });
    expect(document.querySelector(".artist-heading")?.textContent).toContain("Visible Artist");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
  });

  it("uses bounded native artist commands without scanning the asset inventory", async () => {
    artistProfileTopTracks = [savedTrack];
    savedAssets = [{ id: "artist-local", recordingId: savedTrack.id, path: "/music/saved.flac", hash: "artist-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: { ...savedTrack, localAvailability: "available" } }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("match_artist_playback_sources", { artistNames: ["Visible Artist"], sourceTracks: [savedTrack] });
    expect(mocks.call.mock.calls.some(([command]) => command === "list_assets")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Artist sections"] button')].find(button => button.textContent === "Local")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_artist_local_assets_page", {
      names: ["Visible Artist"], limit: 200, offset: 0, query: "", sortField: "sourceOrder", direction: "asc", availability: "all",
    });
    expect(document.querySelector("[aria-label=\"Local artist recordings\"]")?.textContent).toContain("Saved Recording");
    expect(mocks.call.mock.calls.some(([command]) => command === "list_assets")).toBe(false);
  });

  it("keeps old native artist fallback cached and globally sorts before paging only after an unknown command", async () => {
    unsupportedArtistCommands = true;
    artistProfileTopTracks = [savedTrack];
    const artistAsset = (id: string, title: string, artist = "Visible Artist") => {
      const track = { ...savedTrack, id, title, artist, localAvailability: "available" as const };
      return { id: `asset-${id}`, recordingId: id, path: `/music/${id}.flac`, hash: `hash-${id}`, sourceCodec: "flac", finalCodec: "flac", processing: "complete", track } satisfies LocalAsset;
    };
    savedAssets = [artistAsset("zulu", "Zulu Recording"), artistAsset("alpha", "Alpha Recording"), artistAsset("saved", "Saved Recording"), artistAsset("other", "Alpha Recording", "Other Artist")];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "list_assets")).toHaveLength(1);
    const artistPlay = document.querySelector<HTMLButtonElement>('.artist-page [aria-label="Play Saved Recording"]')!;
    artistPlay.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: expect.objectContaining({
      id: savedTrack.id, provider: savedTrack.provider, providerId: savedTrack.providerId, localPath: "/music/saved.flac", assetId: "asset-saved",
    }) }));
    [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Artist sections"] button')].find(button => button.textContent === "Local")!.click();
    await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort local artist tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const titles = [...document.querySelectorAll('[aria-label="Local artist recordings"] li')].map(item => item.textContent ?? "");
    expect(titles).toHaveLength(3);
    expect(titles[0]).toContain("Alpha Recording");
    expect(titles[1]).toContain("Saved Recording");
    expect(titles[2]).toContain("Zulu Recording");
    expect(mocks.call.mock.calls.filter(([command]) => command === "list_assets")).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "list_asset_artist_matches_page")).toBe(false);
  });

  it("does not join or publish a stale legacy artist inventory read after invalidation", async () => {
    unsupportedArtistCommands = true;
    artistProfileTopTracks = [savedTrack];
    const makeAsset = (id: string, title: string) => {
      const track = { ...savedTrack, id, title, artist: "Visible Artist", localAvailability: "available" as const };
      return { id: `asset-${id}`, recordingId: id, path: `/music/${id}.flac`, hash: `hash-${id}`, sourceCodec: "flac", finalCodec: "flac", processing: "complete", track } satisfies LocalAsset;
    };
    const staleAssets = [makeAsset("old-inventory-row", "Old Inventory Recording")];
    const freshAssets = [makeAsset("fresh-inventory-row", "Fresh Inventory Recording")];
    savedAssets = staleAssets;
    let finishStaleInventory: ((items: LocalAsset[]) => void) | undefined;
    let inventoryCalls = 0;
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_assets") {
        inventoryCalls++;
        if (inventoryCalls === 1) return new Promise(resolve => { finishStaleInventory = resolve; });
        return freshAssets;
      }
      return originalCall(command, args);
    });

    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Artist sections"] button')].find(button => button.textContent === "Local")!.click();
    await settle();
    expect(finishStaleInventory).toBeDefined();

    savedAssets = freshAssets;
    eventHandlers.get("asset-facts-updated")?.({ event: "asset-facts-updated", id: 94, payload: null });
    const query = document.querySelector<HTMLInputElement>('[aria-label="Filter local artist recordings"]')!;
    query.value = "Fresh"; query.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(inventoryCalls).toBe(2);
    expect(document.querySelector('[aria-label="Local artist recordings"]')?.textContent).toContain("Fresh Inventory Recording");

    finishStaleInventory!(staleAssets);
    await settle();
    expect(document.querySelector('[aria-label="Local artist recordings"]')?.textContent).toContain("Fresh Inventory Recording");
    expect(document.querySelector('[aria-label="Local artist recordings"]')?.textContent).not.toContain("Old Inventory Recording");
  });

  it("does not downgrade real artist command failures to an inventory scan", async () => {
    artistCommandFailure = "database busy while matching artist recordings";
    artistProfileTopTracks = [savedTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("database busy");
    expect(mocks.call.mock.calls.some(([command]) => command === "list_assets")).toBe(false);
  });

  it("restores the selected artist profile through back and forward without adding a route loop", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Find recordings by Visible Artist"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector(".artist-result")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".artist-heading")?.textContent).toContain("Visible Artist");
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_artist_profile")).toHaveLength(1);

    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    (document.querySelector('[aria-label="Go forward"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-heading")?.textContent).toContain("Visible Artist");
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_artist_profile")).toHaveLength(2);

    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector(".artist-result")?.textContent).toContain("Visible Artist");
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_artist_profile")).toHaveLength(2);
    (document.querySelector('[aria-label="Go back"]') as HTMLButtonElement).click();
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
  });

  it("starts an official match from one row click without waiting for catalog details", async () => {
    previewMode = "fallback";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"));
    row!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("get_track_details", expect.anything());
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: savedTrack, budgetMs: expect.any(Number) }));
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates" || command === "resolve_candidate_audio")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "start_candidate_preview")).toHaveLength(0);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
  });

  it("plays an exact low-score audio-search source with unknown artist and hashtag title", async () => {
    previewMode = "fallback";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Bedouin, Hiya - Salaam (Francis Mercier Remix) - #afrohouse #thesacredtribe - 🔥🧡🔥",
      uploader: "Unknown artist", confidence: "low", score: 0.12,
    };
    const searchTrack: Track = {
      ...savedTrack, artist: "Unknown artist", title: "Salaam",
      provenance: { sourceKind: "audioSearch", identity: "unverified source search result", audioCandidate: candidate },
    };
    savedChartTracks = [searchTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Salaam"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: expect.objectContaining({ provenance: expect.objectContaining({ audioCandidate: candidate }) }) }));
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
  });

  it("plays the exact linked YouTube radio seed without catalog identity matching", async () => {
    previewMode = "fallback";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "radio-seed-123", url: "https://youtu.be/radio-seed-123",
      title: "Different uploader title", confidence: "low", score: 0.03,
    };
    const radioTrack: Track = { ...savedTrack, provenance: { sourceKind: "youtubeRadio", audioCandidate: candidate } };
    savedChartTracks = [radioTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: expect.objectContaining({ provenance: expect.objectContaining({ audioCandidate: candidate }) }) }));
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it.each([false,true])("stream fallback enabled=%s respects its configured source list", async enabled => {
    previewMode = "fallback";
    configuredPreferredStreamingSource = "monochrome";
    streamingFallbackEnabled = enabled;
    streamingFallbackSources = ["soundcloud"];
    playbackResolverCandidates = enabled
      ? [{ ...youtubeCandidate(savedTrack), id: "soundcloud:recording-1", provider: "soundcloud", url: "https://soundcloud.com/visible-artist/saved-recording" }]
      : [new Error("Configured playback sources unavailable")];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(1);
    if(enabled) expect(document.querySelector(".player-source")?.textContent).toContain("SoundCloud");
    else expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("Configured playback sources unavailable");
  });

  it.each([false,true])("midstream recovery enabled=%s excludes unconfigured sources", async enabled => {
    previewMode="fallback";configuredPreferredStreamingSource="monochrome";streamingFallbackEnabled=enabled;streamingFallbackSources=["soundcloud"];
    monochromeCandidates=[{...youtubeCandidate(savedTrack),id:"monochrome:174208180224667648",provider:"monochrome",url:"https://tracks.monochrome.st/track/174208180224667648",confidence:"high",score:0.97}];
    playbackResolverCandidates = [monochromeCandidates[0]!, ...(enabled ? [{ ...youtubeCandidate(savedTrack), id: "soundcloud:recording-1", provider: "soundcloud", url: "https://soundcloud.com/visible-artist/saved-recording" } as Candidate] : [new Error("Configured playback sources unavailable")])];
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(2);
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain(enabled ? "soundcloud audio" : "Configured playback sources unavailable");
  });

  it.each([[false, true, "youtube"], [true, true, "monochrome"], [true, false, "youtube"]] as const)("playlist override=%s with match=%s plays %s", async (override, hasMatch, provider) => {
    previewMode = "fallback";
    configuredPreferredStreamingSource = "monochrome";
    overridePlaylistTrackUrls = override;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const linked = youtubeCandidate(savedTrack);
    const imported: Track = { ...savedTrack, provider:"import", sourceUrl:linked.url, provenance:{ sourceKind:"audioSearch", playlistEntry:linked.url, audioCandidate:linked } };
    savedChartTracks = [imported];
    monochromeCandidates = hasMatch ? [{ ...linked, id:"monochrome:174208180224667648", provider:"monochrome", url:"https://tracks.monochrome.st/track/174208180224667648", confidence:"high", score:0.97 }] : [];
    playbackResolverCandidate = provider === "monochrome" ? monochromeCandidates[0]! : linked;
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row => row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click", { bubbles:true }));
    await settle();
    const resolutions = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(resolutions).toHaveLength(1);
    expect((resolutions[0]![1] as {track:Track}).track.id).toBe(imported.id);
    expect(document.querySelector(".player-source")?.textContent).toContain(provider === "monochrome" ? "Monochrome" : "YouTube");
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it("shows the exact source resolver failure without silently substituting another recording", async () => {
    previewMode = "fallback";
    playbackResolverFailure = "The uploader disabled this video’s audio stream.";
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Salaam (Francis Mercier Remix) - #afrohouse", confidence: "low", score: 0.1,
    };
    savedChartTracks = [{ ...savedTrack, title: "Salaam", provenance: { sourceKind: "audioSearch", audioCandidate: candidate } }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Salaam"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toContain("Unavailable");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("uploader disabled this video’s audio stream");
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it("does not let a late explicit-source resolution replace a newer preview", async () => {
    previewMode = "fallback";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Exact searched source", confidence: "low", score: 0.1,
    };
    const explicitTrack = { ...savedTrack, title: "Explicit source", provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [explicitTrack, secondTrack];
    delayPlaybackTrackId = explicitTrack.id;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll(".shared-track-list > li")];
    rows.find((item) => item.textContent?.includes("Explicit source"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    rows.find((item) => item.textContent?.includes("Another Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    finishPlaybackResolve?.({ candidate, preview: { kind: "audio", url: "/late-explicit.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false } });
    await settle();
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing");
  });

  it("lets the user cancel delayed source resolution and ignores the late result", async () => {
    previewMode = "fallback";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Explicit source", confidence: "low", score: 0.1,
    };
    const delayedTrack = { ...savedTrack, title: "Explicit source", provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [delayedTrack];
    delayPlaybackTrackId = delayedTrack.id;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((row) => row.textContent?.includes("Explicit source"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(finishPlaybackResolve).toBeDefined();
    const cancel = document.querySelector<HTMLButtonElement>('[aria-label="Pause loading playback"]');
    expect(cancel).not.toBeNull();
    cancel!.click();
    await settle();
    expect(document.querySelector<HTMLButtonElement>('.play-button')?.getAttribute("aria-label")).toBe("Play");

    finishPlaybackResolve?.({ candidate, preview: { kind: "audio", url: "/late-cancelled.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false } });
    await settle();
    expect(document.querySelector<HTMLButtonElement>('.play-button')?.getAttribute("aria-label")).toBe("Play");
    expect(play).not.toHaveBeenCalled();
    expect(document.querySelector("audio")?.getAttribute("src") ?? "").not.toContain("late-cancelled");
  });

  it("synchronizes favorites from the saved membership after toggling a track", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Add to Liked Songs")!.click();
    await settle();

    expect(favoriteTrackIds.has(savedTrack.id)).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("set_favorite", { track: expect.objectContaining({ id: savedTrack.id }), value: true });
    expect(mocks.call).toHaveBeenCalledWith("list_favorite_track_ids");
  });

  it("assigns the first unassigned playlist shortcut, reuses it, and ignores focused inputs", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();
    await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("loadedmetadata"));
    await settle();

    const focused = document.createElement("input");
    document.body.append(focused);
    focused.focus();
    focused.dispatchEvent(new KeyboardEvent("keydown", { key: "1", altKey: true, bubbles: true }));
    await settle();
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
    focused.remove();

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "1", altKey: true, bubbles: true }));
    await settle();
    expect(document.querySelector("#playlist-picker-title")?.textContent).toBe("Assign playlist shortcut 1");
    const name = document.querySelector<HTMLInputElement>('[aria-label="New playlist for track"]')!;
    name.value = "Late Night";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const create = [...document.querySelectorAll<HTMLButtonElement>("dialog.playlist-picker-dialog button")].find((button) => button.textContent?.includes("Create & add"))!;
    expect(create.disabled).toBe(false);
    create.click();
    await settle();

    expect(playlistPreferences.shortcuts[0]).toBe("collection-new");
    expect(mocks.call).toHaveBeenCalledWith("set_playlist_shortcut", { slot: 0, collectionId: "collection-new" });
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
    mocks.call.mockClear();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "1", altKey: true, bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-new", tracks: [expect.objectContaining({ id: savedTrack.id })] });
    expect(document.querySelector("dialog.playlist-picker-dialog")).toBeNull();
  });

  it("uses a verified HTTPS radio transport without rewriting the saved station URL", async () => {
    const originalUrl="http://stream.example/live.mp3";
    radioStationState=[{id:"transport",name:"Transport fixture",streamUrl:originalUrl,sourceUrl:"https://radio.example",provider:"custom",isCustom:true}];
    const previous=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>command==="resolve_radio_station_playback_url"?"https://stream.example/live.mp3":previous(command,args));
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){return this.src.startsWith("http:")?Promise.reject(new DOMException("The operation is not supported.","NotSupportedError")):Promise.resolve();});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Transport fixture"))!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_radio_station_playback_url",expect.objectContaining({stationId:"transport",requestGeneration:expect.any(Number),detailed:true}));
    expect(document.querySelector("audio")?.src).toBe("https://stream.example/live.mp3");
    expect(radioStationState[0]?.streamUrl).toBe(originalUrl);
    expect(play).toHaveBeenCalled();
  });

  it("uses compatibility transport for HTTPS radio and preserves its MIME and CORS", async () => {
    radioStationState=[{id:"ogg-live",name:"Ogg compatibility fixture",streamUrl:"https://stream.example/live.ogg",sourceUrl:"https://radio.example",codec:"VORBIS",provider:"custom",isCustom:true}];
    const previous=mocks.call.getMockImplementation()!;
    const url="http://127.0.0.1:4567/webview-radio-mp3/fixture?token=fixture";
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>command==="resolve_radio_station_playback_url"?{url,audioMimeType:"audio/mpeg",audioCodec:"mp3",sourceTransport:"loopback-transcode"}:previous(command,args));
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Ogg compatibility fixture"))!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_radio_station_playback_url",expect.objectContaining({stationId:"ogg-live",detailed:true,webviewCapabilities:{oggVorbis:expect.any(Boolean),oggOpus:expect.any(Boolean)}}));
    expect(document.querySelector("audio")?.src).toBe(url);
    expect(document.querySelector("audio")?.crossOrigin).toBe("anonymous");
    expect(play).toHaveBeenCalled();
  });

  it("keeps a rejected radio transport error visible rather than retrying the unsafe original", async () => {
    radioStationState=[{id:"rejected",name:"Rejected transport",streamUrl:"http://stream.example/live",sourceUrl:"https://radio.example",provider:"custom",isCustom:true}];
    const previous=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{if(command==="resolve_radio_station_playback_url")throw new Error("Station redirect resolves to a nonpublic address");return previous(command,args);});
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Rejected transport"))!.click();await settle();
    expect(document.body.textContent).toContain("Station redirect resolves to a nonpublic address");
    expect(document.querySelector("audio")).toBeNull();
    expect(play).not.toHaveBeenCalled();
  });

  it("discards an old radio transport lookup after another station starts", async () => {
    radioStationState=[{id:"transport",name:"Old transport",streamUrl:"http://stream.example/old.mp3",sourceUrl:"https://radio.example",provider:"custom",isCustom:true},{id:"backup",name:"Current transport",streamUrl:"https://stream.example/current.mp3",sourceUrl:"https://radio.example",provider:"custom",isCustom:true}];
    const previous=mocks.call.getMockImplementation()!;
    let finish!:(url:string)=>void;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>command==="resolve_radio_station_playback_url" && args?.stationId==="transport"?new Promise<string>(resolve=>{finish=resolve;}):previous(command,args));
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const station=(name:string)=>[...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes(name))!;
    station("Old transport").click();await settle();expect(finish).toBeTypeOf("function");
    station("Current transport").click();await settle();
    expect(document.querySelector("audio")?.src).toBe("https://stream.example/current.mp3");
    finish("https://stream.example/old.mp3");await settle();
    expect(document.querySelector("audio")?.src).toBe("https://stream.example/current.mp3");
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Current transport");
    expect(play).toHaveBeenCalledTimes(1);
  });

  it("records identified radio songs with station and listen-time attribution per transition", async () => {
    radioStationState = [{ id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    radioNowPlayingState = {
      stationId: "main", artist: "Cesária Évora", title: "Sodade", album: "Miss Perfumado",
      sourceUrl: "https://radio.example/live", metadataConfidence: "station-reported metadata", observedAt: "2026-10-14T18:30:00Z",
    };
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function (this: HTMLMediaElement) {
      Object.defineProperty(this, "paused", { configurable: true, value: false });
      return Promise.resolve();
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find((button) => button.textContent?.includes("Main Mix"))!.click();
    await settle();
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "paused", { configurable: true, value: false });
    audio.dispatchEvent(new Event("playing"));
    await settle();
    audio.dispatchEvent(new Event("playing"));
    await settle();

    const radioRecords = mocks.call.mock.calls.filter(([command]) => command === "record_radio_history").map(([, args]) => args as Record<string, unknown>);
    expect(radioRecords.length).toBeGreaterThanOrEqual(1);
    expect(radioRecords.every((record) => record.stationId === "main" && record.listenedAt === radioRecords[0]?.listenedAt && record.occurrenceKey === radioRecords[0]?.occurrenceKey)).toBe(true);
    expect(radioRecords[0]).toMatchObject({ stationId: "main", listenedAt: expect.any(String), metadata: expect.objectContaining({ artist: "Cesária Évora", title: "Sodade" }) });
    expect(Number.isNaN(Date.parse(String(radioRecords[0]?.listenedAt)))).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "record_playback")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "get_radio_song_track")).toBe(true);
  });

  it("discards a delayed radio identification after the same station is restarted", async () => {
    previewMode = "direct";
    radioStationState = [
      { id: "main", name: "Main Mix", streamUrl: "https://radio.example/main", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false },
      { id: "backup", name: "Backup Mix", streamUrl: "https://radio.example/backup", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false },
    ];
    holdRadioIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const stationButton = (name: string) => [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")]
      .find((button) => button.textContent?.includes(name))!;
    stationButton("Main Mix").click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".radio-current .identify")!.click();
    await settle();
    expect(finishRadioIdentify).toBeDefined();
    expect(mocks.captureRadioPcm).toHaveBeenCalledTimes(1);
    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm", expect.objectContaining({
      trackId: "radio:main", stationId: "main", sessionId: expect.any(String), generation: expect.any(Number),
      seekSerial: 0, wavBase64: "captured-wav-base64",
    }));
    expect(mocks.call.mock.calls.some(([command]) => command === "identify_radio_station")).toBe(false);

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    stationButton("Backup Mix").click();
    await settle();
    stationButton("Main Mix").click();
    await settle();

    finishRadioIdentify!({
      nowPlaying: {
        stationId: "main", artist: "Stale Artist", title: "Stale Song",
        sourceUrl: "https://radio.example/song", metadataConfidence: "station-reported metadata",
      },
    });
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Radio currently playing"]')?.textContent).not.toContain("Stale Song");
  });

  it("does not fall back to reopening a WebView radio stream when current-output capture is unavailable", async () => {
    previewMode = "direct";
    radioStationState = [{ id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    mocks.captureRadioPcm.mockRejectedValueOnce(new Error("This WebView cannot capture the currently playing radio audio."));
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find((button) => button.textContent?.includes("Main Mix"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".radio-current .identify")!.click();
    await settle();
    expect(mocks.captureRadioPcm).toHaveBeenCalledTimes(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "identify_webview_playing_pcm" || command === "identify_radio_station")).toBe(false);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("cannot capture the currently playing radio audio");
  });

  it("preserves current radio evidence when a background station lookup is rejected as stale", async () => {
    radioStationState = [{id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false}];
    radioNowPlayingState = {stationId: "main", artist: "Ruthee", title: "Zoblazo", sourceUrl: "", metadataConfidence: "SongRec fingerprint"};
    const original = mocks.call.getMockImplementation()!;
    let rejectBackground = false, staleRejections = 0, countSuccessfulBackground = false, backgroundCalls = 0;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "poll_radio_station" && countSuccessfulBackground) {
        backgroundCalls++;
        if (backgroundCalls > 1) throw new Error("Playback changed during station metadata lookup");
      }
      if (command === "poll_radio_station" && rejectBackground) {staleRejections++; throw new Error("Playback changed during station metadata lookup");}
      return original(command, args);
    });
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, {target: document.body}); unmountApp = () => {void unmount(instance);};
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button => button.textContent?.includes("Main Mix"))!.click(); await settle();
    expect(document.querySelector("footer.player")?.textContent).toContain("Zoblazo");
    rejectBackground = true;
    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click(); await settle();
    expect(staleRejections).toBeGreaterThan(0);
    expect(document.querySelector("footer.player")?.textContent).toContain("Zoblazo");
    expect(document.querySelector('[role="alert"]')?.textContent ?? "").not.toContain("Playback changed during station metadata lookup");
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button => button.textContent?.includes("Main Mix"))!.click(); await settle();
    rejectBackground = false; countSuccessfulBackground = true;
    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click(); await settle(); await settle();
    expect(backgroundCalls).toBe(1);
    expect(document.querySelector("footer.player")?.textContent).toContain("Zoblazo");
  });

  it("keeps identified radio songs in the queue when playback-history saving is off", async () => {
    radioHistoryEnabled = false;
    radioStationState = [{ id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    radioNowPlayingState = { stationId: "main", artist: "Cesária Évora", title: "Sodade", album: "Miss Perfumado", sourceUrl: "https://radio.example/live", metadataConfidence: "station-reported metadata" };
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function (this: HTMLMediaElement) {
      Object.defineProperty(this, "paused", { configurable: true, value: false });
      return Promise.resolve();
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find((button) => button.textContent?.includes("Main Mix"))!.click();
    await settle();
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "paused", { configurable: true, value: false });
    audio.dispatchEvent(new Event("playing"));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Radio currently playing"]')?.textContent).toContain("Sodade");
    expect(document.querySelector('[aria-label="Radio currently playing"]')?.textContent).toContain("Main Mix");

    radioNowPlayingState = { stationId: "main", artist: "Buena Vista Social Club", title: "Chan Chan", sourceUrl: "https://radio.example/live", metadataConfidence: "station-reported metadata" };
    document.querySelector<HTMLButtonElement>(".radio-current .identify")!.click();
    await settle();
    expect(document.querySelector('[aria-label="Radio currently playing"]')?.textContent).toContain("Chan Chan");
    document.querySelector<HTMLButtonElement>(".history-toggle")!.click();
    await settle();
    expect(document.querySelector(".history-list")?.textContent).toContain("Sodade");
    expect(document.querySelector(".history-list")?.textContent).toContain("Main Mix");
    expect(mocks.call.mock.calls.some(([command]) => command === "record_radio_history")).toBe(false);
  });

  it("shows an explicit unavailable album action for a radio history track without an album link", async () => {
    radioStationState = [{ id: "main", name: "Main Mix", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    const metadata = { stationId: "main", artist: "Ruthee", title: "Zoblazo", sourceUrl: "https://radio.example/live", metadataConfidence: "station-reported metadata" };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_station_details") return { stationId: "main", history: [metadata], historyAvailable: true, note: "" };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button => button.textContent?.includes("Main Mix"))!.click();
    await settle(); await settle();
    const row = [...document.querySelectorAll<HTMLElement>(".radio-page .track-list-viewport li")].find(item => item.textContent?.includes("Zoblazo"))!;
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
    await settle(); await settle();
    const albumAction = [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find(button => button.textContent?.trim() === "Go to album")!;
    expect(albumAction.disabled).toBe(true);
    expect(albumAction.title).toBe("Album link unavailable for this track.");
    expect(albumAction.getAttribute("aria-label")).toContain("Album link unavailable for this track");
    expect(mocks.call.mock.calls.some(([command]) => command === "get_release_tracks")).toBe(false);
  });

  it("opens selected search-track information and plays that track instead of the current one", async () => {
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const searchedTrack: Track = {
      ...savedTrack, id: "search-selected", provider: "soundcloud", providerId: "search-selected", title: "Search Selected Song", artist: "Search Artist",
      provenance: { audioCandidate: { ...youtubeCandidate(savedTrack), id: "soundcloud:search-selected", provider: "soundcloud", url: "https://soundcloud.com/search-artist/search-selected", title: "Search Selected Song" } },
    };
    audioSearchResult = [searchedTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find((item) => item.textContent?.includes("Search"))!.click();
    await settle();
    await searchDiscovery("Search Selected Song", ["SoundCloud"]);
    const result = document.querySelector<HTMLElement>(".result-row")!;
    result.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.track-menu button")].find((button) => menuActionText(button) === "Track information")!.click();
    await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Search Selected Song");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Search Artist");
    const resolveCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    document.querySelector<HTMLButtonElement>(".playing-info .actions .primary")!.click();
    await settle();
    expect(mocks.call.mock.calls.some(([command, args]) => command === "resolve_preview" && (args as { track?: Track }).track?.id === "search-selected")).toBe(true);
    expect(play).toHaveBeenCalled();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length).toBeGreaterThan(resolveCount);
  });

  it("offers mpv only when available and uses local files before remote candidates", async () => {
    mpvStatusAvailable = true;
    const localTrack: Track = { ...savedTrack, localPath: "/music/saved.flac", localAvailability: "available" };
    const candidateTrack: Track = { ...savedTrack, id: "candidate-song", title: "Candidate Song", provenance: { audioCandidate: { ...youtubeCandidate(savedTrack), id: "youtube:candidate-song", url: "https://youtube.com/watch?v=abcdefghi12" } } };
    savedChartTracks = [localTrack, candidateTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions("Saved Recording");
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Track information")!.click();
    await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Open in mpv");
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info .actions button")].find((button) => button.textContent?.trim() === "Open in mpv")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_in_mpv", { source: "/music/saved.flac", startSeconds: null, endSeconds: null, video: true });

    const candidateMenu = await trackActions("Candidate Song");
    [...candidateMenu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Track information")!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info .actions button")].find((button) => button.textContent?.trim() === "Open in mpv")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_in_mpv", { source: "https://youtube.com/watch?v=abcdefghi12", startSeconds: null, endSeconds: null, video: true });
  });

  it("hides the mpv action when the executable is unavailable", async () => {
    remoteBindingState = [];
  mpvStatusAvailable = false;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Track information")!.click();
    await settle();
    expect([...document.querySelectorAll<HTMLButtonElement>(".playing-info .actions button")].some((button) => button.textContent?.trim() === "Open in mpv")).toBe(false);
  });

  it("opens a recording's catalog page without starting preview playback", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    const source = menu.querySelector('[aria-label="Open Saved Recording on source"]') as HTMLButtonElement;
    source.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: savedTrack.sourceUrl });
    expect(mocks.call.mock.calls.some(([command]) => command === "resolve_preview")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it("searches a selected audio provider and opens a reviewed download without auto-queueing", async () => {
    const candidate = youtubeCandidate(savedTrack);
    audioSearchResult = [{ ...savedTrack, provenance: { audioCandidate: candidate } }];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await searchDiscovery("Visible Artist Saved Recording", ["YouTube"]);
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Visible Artist Saved Recording", sourceIds: ["youtube"] }));
    const row = [...document.querySelectorAll(".result-row")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Play Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("find_candidates", expect.anything());
    expect(mocks.call.mock.calls.some(([command]) => command === "resolve_playback_track")).toBe(true);

    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector('[role="dialog"]')).toBeNull(); // Native dialog semantics
    expect(document.querySelector("dialog.download-dialog")?.textContent).toContain("Download recording");
    expect(document.querySelector("dialog.download-dialog .candidate")?.textContent).toContain("Saved Recording");
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    const folder = document.querySelector('[aria-label="Download folder"]') as HTMLInputElement;
    folder.value = "/tmp/burrata-music";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    const tagTitle = document.querySelector('[aria-label="Download tag title"]') as HTMLInputElement;
    tagTitle.value = "Saved Recording (Club Mix)";
    tagTitle.dispatchEvent(new Event("input", { bubbles: true }));
    expect(document.querySelector(".download-review")).toBeNull();
    await selectDownloadSource("youtube");
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    document.querySelector<HTMLButtonElement>(".download-dialog footer .primary")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("save_settings", expect.objectContaining({ settings: expect.objectContaining({ musicDirectory: "/tmp/burrata-music" }) }));
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { track: Track } }).request.track.title).toBe("Saved Recording (Club Mix)");
  });

  it("opens the chooser from row Download and queues only after source selection and confirmation", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("find_candidates", { track: savedTrack, providerIds: ["soundcloud"] });
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector("dialog.download-dialog")).not.toBeNull();
    await selectDownloadSource("soundcloud");
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    await confirmReviewedDownload("soundcloud");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { candidate: Candidate } }).request.candidate.provider).toBe("soundcloud");
    expect((queued![1] as { request: { formatId: string | null } }).request.formatId).toBeNull();
    expect(document.body.textContent).toContain("Added to download queue.");
    expect(document.querySelector("dialog.download-dialog")).toBeNull();

    await chooseTrackAction("Download options…");
    expect(document.querySelector("dialog.download-dialog")?.textContent).toContain("Download recording");
  });

  it("shows a linked source immediately while quality inspection and other providers are still pending", async () => {
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), id: "linked-youtube", confidence: "low", score: 0.2 };
    const linkedTrack: Track = { ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [linkedTrack];
    delayCandidateInspectId = candidate.id;
    slskdConnectionEnabled = true;
    delayedCandidateProviders.set("slskd", () => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Download options…");

    const dialog = document.querySelector<HTMLDialogElement>("dialog.download-dialog")!;
    expect(dialog.querySelector(".candidate")?.textContent).toContain(candidate.title);
    expect(dialog.querySelector<HTMLButtonElement>('footer .primary')?.disabled).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_candidates" && (args as { providerIds?: string[] }).providerIds?.[0] === "soundcloud")).toBe(true);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_candidates" && (args as { providerIds?: string[] }).providerIds?.[0] === "slskd")).toBe(true);
    expect(finishCandidateInspect).toBeDefined();

    // Closing invalidates the request; a late inspect result cannot recreate the chooser.
    dialog.querySelector<HTMLButtonElement>('[aria-label="Close download chooser"]')!.click();
    finishCandidateInspect?.({ ...candidate, formats: [{ id: "best", codec: "AAC", extension: "m4a", bitrate: 256, lossless: false }] });
    delayedCandidateProviders.get("slskd")?.([soulseekCandidate(linkedTrack)]);
    await settle();
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("keeps successful provider results when another provider fails and queues only once", async () => {
    configuredMusicDirectory = "/saved/music";
    failedCandidateProviders.add("soundcloud");
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Download options…");
    const dialog = document.querySelector<HTMLDialogElement>("dialog.download-dialog")!;
    expect(dialog.textContent).toContain("Saved Recording");
    expect(dialog.querySelector(".download-review")).toBeNull();
    const downloadButton = [...dialog.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Download")!;
    expect(downloadButton.disabled).toBe(false);
    await selectDownloadSource();
    downloadButton.click();
    downloadButton.click();
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "enqueue_download")).toHaveLength(1);
  });

  it("queues a linked Soulseek audioSearch result with an empty URL from its typed peer source", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "slskd";
    const peerCandidate: Candidate = {
      id: "peer-hash", provider: "slskd", url: "", title: "Saved Recording.flac", uploader: "peer-user",
      durationSeconds: 240, artworkUrl: null, formats: [], permission: "unknown", confidence: "low",
      score: 0, reasons: [], official: false,
      source: { kind: "soulseek", username: "peer-user", filename: "Saved Recording.flac", size_bytes: 28_000_000, bitrate_kbps: 933 },
    };
    const peerTrack: Track = {
      ...savedTrack, id: "peer-track", provider: "slskd", providerId: "peer-hash", title: "Peer Saved Recording",
      provenance: { sourceKind: "audioSearch", audioCandidate: peerCandidate },
    };
    savedChartTracks = [peerTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Peer Saved Recording"))!;
    row.querySelector('[aria-label="Download Peer Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector("dialog.download-dialog")).not.toBeNull();
    await confirmReviewedDownload("slskd");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { candidate: Candidate } }).request.candidate).toEqual(peerCandidate);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_candidates" && (args as { providerIds?: string[] }).providerIds?.includes("slskd"))).toBe(false);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "inspect_candidate" && (args as { candidate?: Candidate }).candidate?.id === peerCandidate.id)).toBe(false);
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("honors a linked SoundCloud search result even when YouTube is the saved default", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "youtube";
  configuredPreferredStreamingSource = "youtube";
  overridePlaylistTrackUrls = false;
  monochromeCandidates = [];
  streamingFallbackEnabled = true;
  streamingFallbackSources = ["youtube", "soundcloud"];
  downloadFallbackEnabled = false;
  downloadFallbackSources = ["monochrome", "slskd", "youtube", "soundcloud"];
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), provider: "soundcloud",
      id: "sc-exact", url: "https://soundcloud.com/visible-artist/saved-recording" };
    audioSearchResult = [{ ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("nav button")].find((item) => item.textContent?.includes("Search"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await searchDiscovery("Visible Artist Saved Recording", ["SoundCloud"]);
    const row = [...document.querySelectorAll(".result-row")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(document.querySelector('[aria-label="soundcloud sources"] .candidate')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    await confirmReviewedDownload("soundcloud");
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.provider).toBe("soundcloud");
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("starts song radio with the exact seed and 25 recommendations without restarting or downloading", async () => {
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const recommendations: Track[] = [
      { ...secondTrack, id: "radio-one", title: "Radio One" },
      { ...secondTrack, id: "radio-two", title: "Radio Two" },
    ];
    songRadioTracks = recommendations;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();
    await settle();
    const startsBefore = mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio").length;
    const playCallsBefore = play.mock.calls.length;
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Start song radio")!.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("get_song_radio", { track: savedTrack, candidate: expect.objectContaining({ provider: "youtube" }), limit: 25 });
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio")).toHaveLength(startsBefore);
    expect(play.mock.calls).toHaveLength(playCallsBefore);
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector(".playback-queue-panel")?.textContent).toContain("Saved Recording");
    expect(document.querySelector(".playback-queue-panel")?.textContent).toContain("Radio One");
    expect(document.querySelector(".playback-queue-panel")?.textContent).toContain("Radio Two");
  });

  it("preserves the current playback and queue when song radio fails", async () => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    songRadioFailure = "YouTube radio is unavailable";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Start song radio")!.click();
    await settle();
    expect(document.querySelector(".now-playing")?.textContent).toContain("Saved Recording");
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("YouTube radio is unavailable");
  });

  it("refreshes a saved music folder before download even when startup settings were blank", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    configuredMusicDirectory = "/saved/music";
    await chooseTrackAction("Download options…");
    expect((document.querySelector('[aria-label="Download folder"]') as HTMLInputElement).value).toBe("/saved/music");
    expect(document.querySelector("dialog.download-dialog .candidate")).not.toBeNull();
    expect(document.querySelector(".download-review")).toBeNull();
    await selectDownloadSource();
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    document.querySelector<HTMLButtonElement>(".download-dialog footer .primary")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call.mock.calls.some(([command]) => command === "save_settings")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(true);
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

  it("does not open a stale A result after the user selects B", async () => {
    previewMode = "race";
    delayPlaybackTrackId = savedTrack.id;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll(".shared-track-list > li")];
    rows.find((item) => item.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    rows.find((item) => item.textContent?.includes("Another Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    finishPlaybackResolve?.({ candidate: youtubeCandidate(savedTrack), preview: { kind: "audio", url: "/late-A.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false } });
    await settle();
    const starts = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(starts).toHaveLength(2);
    expect(starts.some(([, args]) => (args as { track: Track }).track.id === secondTrack.id)).toBe(true);
    expect(document.querySelector("audio")?.getAttribute("src")).not.toContain("late-A");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
  });

  it.each([1, 2])("recovers WebView buffering only with current data after a seek (readyState %i)", async (readyState) => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(readyState);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li")].find(row => row.textContent?.includes("Saved Recording"))!.click();
    await settle();
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("playing"));
    audio.dispatchEvent(new Event("waiting"));
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toContain("Buffering");
    audio.dispatchEvent(new Event("seeking"));
    audio.currentTime = 30;
    audio.dispatchEvent(new Event("seeked"));
    for (let index = 0; index < 3; index++) {
      clock += 500;
      audio.dispatchEvent(new Event("timeupdate"));
    }
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toContain("Buffering");
    for (let index = 0; index < 3; index++) {
      audio.currentTime += 0.5;
      clock += 500;
      audio.dispatchEvent(new Event("timeupdate"));
    }
    await settle();
    if (readyState >= 2) expect(document.querySelector(".player-status")?.textContent).not.toContain("Buffering");
    else expect(document.querySelector(".player-status")?.textContent).toContain("Buffering");
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it.each(["fresh-success", "fresh-failure-fallback", "fresh-and-fallback-failure", "owner-cancel"] as const)("recovers one owned finite WebView stall near its current position without changing the queue occurrence (%s)", async outcome => {
    vi.useFakeTimers();
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }, secondTrack];
    const firstCandidate = { ...youtubeCandidate(savedTrack), id: "monochrome:239910648984977408", provider: "monochrome", url: "https://tracks.monochrome.st/track/239910648984977408" };
    const secondCandidate = { ...youtubeCandidate(savedTrack), id: "youtube:second-stall-source" };
    playbackResolverCandidates = [firstCandidate, secondCandidate];
    const baseCall = mocks.call.getMockImplementation()!;
    let finishFreshResolve: ((value: unknown) => void) | undefined;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "resolve_playback_track" && (args as { refreshCandidateFresh?: boolean }).refreshCandidateFresh) {
        if (outcome === "owner-cancel") return new Promise(resolve => { finishFreshResolve = resolve; });
        if (outcome !== "fresh-success") throw new Error("Signed stream expired");
        const candidate = (args as { refreshCandidate: Candidate }).refreshCandidate;
        return { candidate, preview: { kind: "audio", url: "/refreshed-same-candidate.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false, provider: candidate.provider, sourceGeneration: webviewSourceGeneration } };
      }
      if (command === "resolve_playback_track" && (args as { attemptLimit?: number }).attemptLimit === 1 && outcome === "fresh-and-fallback-failure") throw new Error("No other verified source available");
      return baseCall(command, args);
    });
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    const startedAt: number[] = [];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function(this: HTMLMediaElement) {
      startedAt.push(this.currentTime);
      return Promise.resolve();
    });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const initial = await automationAction("finite-stall-start", "play", { trackIds: [savedTrack.id] });
    expect(initial.error).toBeNull();
    const started = mocks.call.mock.calls.find(([command, args]) => command === "automation_playback_complete" && (args as { requestId?: string }).requestId === "finite-stall-start")?.[1] as { playback: { queueEntryIds: string[] } };
    const initialOccurrence = started.playback.queueEntryIds[0];
    const audio = document.querySelector("audio")!;
    clock += 36_000;
    audio.currentTime = 36;
    await vi.advanceTimersByTimeAsync(36_000);
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    // A rebuffer/playing event on the same source must retain its proven steady baseline.
    audio.dispatchEvent(new Event("playing"));
    audio.dispatchEvent(new Event("waiting"));
    clock += 500;
    await vi.advanceTimersByTimeAsync(500);
    audio.currentTime = 36.5;
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    clock += 15_000;
    await vi.advanceTimersByTimeAsync(15_000);
    audio.dispatchEvent(new Event("stalled")); // repeated same-owner events do not postpone the deadline
    clock += 4_999;
    await vi.advanceTimersByTimeAsync(4_999);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    clock += 1;
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    if (outcome === "owner-cancel") {
      expect(finishFreshResolve).toBeTypeOf("function");
      await automationAction("cancel-stalled-refresh", "pause");
      await settle();
      finishFreshResolve!({ candidate: firstCandidate, preview: { kind: "audio", url: "/late-stale-refresh.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false } });
      await settle();
      expect(mocks.call.mock.calls.filter(([command, args]) => command === "resolve_playback_track" && (args as { attemptLimit?: number }).attemptLimit === 1)).toHaveLength(0);
      const cancelled = await automationAction("owner-cancel-after", "volume", { volume: 0.8 });
      expect(cancelled.playback.trackId).toBe(savedTrack.id);
      expect(cancelled.playback.paused).toBe(true);
      return;
    }

    if (outcome === "fresh-success" || outcome === "fresh-failure-fallback") {
      document.querySelector("audio")!.currentTime += 0.3;
      clock += 25;
      await vi.advanceTimersByTimeAsync(25);
      await settle();
    }
    const resolutionCalls = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(resolutionCalls).toHaveLength(outcome === "fresh-success" ? 2 : 3);
    expect(resolutionCalls[1][1]).toEqual(expect.objectContaining({
      refreshCandidateFresh: true, refreshCandidate: firstCandidate, selectedCandidate: null, budgetMs: expect.any(Number),
    }));
    expect((resolutionCalls[1][1] as { budgetMs: number }).budgetMs).toBe(20_000);
    expect((resolutionCalls[1][1] as { budgetMs: number }).budgetMs).toBeGreaterThan(0);
    if (outcome !== "fresh-success") {
      expect(resolutionCalls[2][1]).toEqual(expect.objectContaining({ attemptLimit: 1, budgetMs: expect.any(Number) }));
      expect((resolutionCalls[2][1] as { budgetMs: number }).budgetMs).toBeLessThanOrEqual((resolutionCalls[1][1] as { budgetMs: number }).budgetMs);
    }
    expect(startedAt).toHaveLength(outcome === "fresh-and-fallback-failure" ? 1 : 2);
    if (startedAt.length > 1) expect(startedAt[1]).toBeCloseTo(36.5, 1);
    const recovered = await automationAction("finite-stall-after", "volume", { volume: 0.8 });
    expect(recovered.error).toBeNull();
    expect(recovered.playback.queueEntryIds).toEqual([initialOccurrence]);
    expect(recovered.playback.trackId).toBe(savedTrack.id);
    expect(recovered.playback.paused).toBe(outcome === "fresh-and-fallback-failure");
    if (outcome === "fresh-and-fallback-failure") {
      expect(recovered.playback.lastMediaFailure).toContain("stalled for 20 seconds");
      expect(document.querySelector(".player-error")?.textContent).toContain("stalled for 20 seconds");
    }
  });

  it("preserves the stall cause when its existing recovery deadline has expired", async () => {
    vi.useFakeTimers();
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await automationAction("expired-stall-start", "play", { trackIds: [savedTrack.id] });
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    const failure = await automationAction("expired-stall-after", "volume", { volume: 0.8 });
    expect(failure.playback.playerMessage).toContain("stalled for 20 seconds");
    expect(failure.playback.playerMessage).toContain("recovery deadline expired");
    expect(failure.playback.playerMessage).not.toContain("Audio lookup exceeded");
  });

  it("does not exceed the two-attempt cap when the fallback source stalls before steady progress", async () => {
    vi.useFakeTimers();
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:early-first" },
      { ...youtubeCandidate(savedTrack), id: "youtube:early-second" },
    ];
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await automationAction("early-stall-start", "play", { trackIds: [savedTrack.id] });
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("error"));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(2);

    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(2);
    const failure = await automationAction("early-stall-after", "volume", { volume: 0.8 });
    expect(failure.playback.lastMediaFailure).toContain("stalled for 20 seconds");
  });

  it("starts the steady-stall watchdog only after a pending initial play fulfills", async () => {
    vi.useFakeTimers();
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "direct";
    directPreviewExcerpt = false;
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    const armWatchdog = vi.spyOn(WebviewStallWatchdog.prototype, "arm");
    let finishPlay!: () => void;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => new Promise<void>(resolve => { finishPlay = resolve; }));
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")!({ event: "automation-playback", id: 8101, payload: {
      requestId: "pending-start-stall", action: "play", trackIds: [savedTrack.id],
    } });
    await settle();
    const audio = document.querySelector("audio")!;
    expect(finishPlay).toBeTypeOf("function");
    audio.dispatchEvent(new Event("waiting"));
    audio.dispatchEvent(new Event("playing")); // No media-clock advance accompanies this event.
    expect(armWatchdog).not.toHaveBeenCalled();
    clock += 2_000;
    await vi.advanceTimersByTimeAsync(2_000);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(0);

    finishPlay();
    await settle();
    expect(armWatchdog).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".player-status")?.textContent).toMatch(/^Playing/);
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();

    expect(document.querySelector(".player-status")?.textContent).not.toContain("Buffering");
    expect(document.querySelector(".player-error")?.textContent).toMatch(/stalled|deadline/i);
  });

  it.each(["seek", "pause", "owner", "unmount"])("cancels steady-stall recovery after %s changes the owner state", async (change) => {
    vi.useFakeTimers();
    let clock = 1000, mediaPaused = false;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }, { ...secondTrack, durationSeconds: 240 }];
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:owner-test-first" },
      { ...youtubeCandidate(secondTrack), id: "youtube:owner-test-second" },
    ];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockImplementation(() => mediaPaused);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect((await automationAction("stall-owner-first", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    let audio = document.querySelector("audio")!;
    audio.currentTime = 10;
    audio.dispatchEvent(new Event("waiting"));
    if (change === "seek") audio.dispatchEvent(new Event("seeking"));
    if (change === "pause") { mediaPaused = true; audio.dispatchEvent(new Event("pause")); }
    if (change === "owner") {
      mediaPaused = false;
      expect((await automationAction("stall-owner-second", "play", { trackIds: [secondTrack.id] })).error).toBeNull();
      expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(2);
    }
    if (change === "unmount") { unmountApp?.(); unmountApp = undefined; }
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(change === "owner" ? 2 : 1);
  });

  it("does not report recovery as playing if play resets the required resume position", async () => {
    vi.useFakeTimers();
    let clock = 1000, plays = 0;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:resume-proof-first" },
      { ...youtubeCandidate(savedTrack), id: "youtube:resume-proof-second" },
    ];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function(this: HTMLMediaElement) {
      if (++plays === 2) this.currentTime = 0; // Simulate a source that accepted the seek but reset at play().
      return Promise.resolve();
    });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect((await automationAction("resume-proof-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    const audio = document.querySelector("audio")!;
    clock += 36_000;
    audio.currentTime = 36;
    await vi.advanceTimersByTimeAsync(36_000);
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    audio.dispatchEvent(new Event("waiting"));
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    clock += 25; await vi.advanceTimersByTimeAsync(25); await settle();
    expect(plays).toBe(2);
    expect(document.querySelector(".player-status")?.textContent).not.toMatch(/^Playing/);
    expect(document.querySelector(".player-error")?.textContent).toContain("could not resume at its previous playback position");
  });

  it.each(["seeking-frozen", "settled-frozen", "pending-progress", "stale-progress"] as const)("requires owned media-clock progress for automatic resume (%s)", async mode => {
    vi.useFakeTimers();
    let clock = 1000, plays = 0;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:resume-proof-first" },
      { ...youtubeCandidate(savedTrack), id: "youtube:resume-proof-second" },
    ];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "seeking", "get").mockImplementation(() => plays >= 2 && mode === "seeking-frozen");
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function(this: HTMLMediaElement) {
      ++plays;
      return plays >= 2 && mode === "pending-progress" ? new Promise<void>(() => {}) : Promise.resolve();
    });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect((await automationAction("resume-proof-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    const audio = document.querySelector("audio")!;
    clock += 36_000;
    audio.currentTime = 36;
    await vi.advanceTimersByTimeAsync(36_000);
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    audio.dispatchEvent(new Event("waiting"));
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    expect(plays).toBe(2);
    expect(document.querySelector(".player-status")?.textContent).not.toMatch(/^Playing/);
    if (mode === "pending-progress") {
      const recovered = document.querySelector("audio")!;
      recovered.currentTime += 0.3;
      clock += 25; await vi.advanceTimersByTimeAsync(25); await settle();
      expect(document.querySelector(".player-status")?.textContent).toMatch(/^Playing/);
      return;
    }
    if (mode === "stale-progress") {
      await automationAction("cancel-resume-proof", "pause");
      audio.currentTime += 0.3;
      audio.dispatchEvent(new Event("timeupdate"));
    }
    clock += 20_000; await vi.advanceTimersByTimeAsync(20_000); await settle();
    expect(document.querySelector(".player-status")?.textContent).not.toMatch(/^Playing/);
    if (mode !== "stale-progress") expect(document.querySelector(".player-error")?.textContent).toContain("20-second");
  });

  it("bounds a held active seek and keeps its requested recovery target", async () => {
    vi.useFakeTimers();
    let clock=1000;
    vi.spyOn(performance,"now").mockImplementation(()=>clock);
    previewMode="direct"; directPreviewExcerpt=false; savedChartTracks=[{...savedTrack,durationSeconds:240}];
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    let mediaSeeking=false;
    vi.spyOn(HTMLMediaElement.prototype,"seeking","get").mockImplementation(()=>mediaSeeking);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const descriptor=Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype,"currentTime")!;
    const seekWrites:number[]=[];
    vi.spyOn(HTMLMediaElement.prototype,"currentTime","set").mockImplementation(function(this: HTMLMediaElement,value:number){seekWrites.push(value);descriptor.set?.call(this,value);});
    const begin=vi.spyOn(WebviewSeekCompletionWatchdog.prototype,"begin");
    const instance=mount(App,{target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    expect((await automationAction("held-seek-start","play",{trackIds:[savedTrack.id]})).error).toBeNull();
    const playing=document.querySelector("audio")!; playing.currentTime=10; clock+=6_000; playing.dispatchEvent(new Event("timeupdate")); await settle();
    mediaSeeking=true;
    expect((await automationAction("held-seek","seek",{position:44})).error).toBeNull();
    expect(begin).toHaveBeenCalledWith(expect.objectContaining({target:44,paused:false}));
    clock+=20_000; await vi.advanceTimersByTimeAsync(20_000); await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(0);
    expect(seekWrites.filter(value=>value===44)).toHaveLength(1);
    expect(document.querySelector(".player-error")?.textContent).toContain("choose Play to retry from the requested position");
    expect(document.querySelector(".player-scrubber .time")?.textContent).toBe("0:44");
  });

  it("uses the resolver duration instead of a growing WebView estimate and clamps seeks to it", async () => {
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 347 }];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(35.5);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const writes: number[] = [];
    const descriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "currentTime")!;
    vi.spyOn(HTMLMediaElement.prototype, "currentTime", "set").mockImplementation(function(this: HTMLMediaElement, value: number) {
      writes.push(value);
      descriptor.set?.call(this, value);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect((await automationAction("stable-duration-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    const slider = document.querySelector<HTMLInputElement>('[aria-label="Playback position"]')!;
    expect(slider.max).toBe("347");
    expect(document.querySelectorAll(".player-scrubber .time")[1]?.textContent).toBe("5:47");

    expect((await automationAction("stable-duration-seek", "seek", { position: 400 })).error).toBeNull();
    expect(writes).toContain(346.95);
    expect(document.querySelectorAll(".player-scrubber .time")[1]?.textContent).toBe("5:47");
  });

  it("keeps excerpt duration independent from the full track duration", async () => {
    previewMode = "direct";
    directPreviewExcerpt = true;
    savedChartTracks = [{ ...savedTrack, durationSeconds: 347 }];
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(30);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect((await automationAction("excerpt-duration-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Playback position"]')!.max).toBe("30");
  });

  it.each([0, Number.NaN, Number.POSITIVE_INFINITY])("bounds a seek until an unknown duration can be validated (%s)", async (mediaDuration) => {
    vi.useFakeTimers();
    previewMode="direct"; directPreviewExcerpt=false; savedChartTracks=[{...savedTrack,durationSeconds:240}];
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(mediaDuration);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype,"seeking","get").mockReturnValue(true);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const descriptor=Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype,"currentTime")!;
    const writes:number[]=[];
    vi.spyOn(HTMLMediaElement.prototype,"currentTime","set").mockImplementation(function(this: HTMLMediaElement,value:number){writes.push(value);descriptor.set?.call(this,value);});
    const begin=vi.spyOn(WebviewSeekCompletionWatchdog.prototype,"begin");
    const instance=mount(App,{target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    expect((await automationAction("unknown-duration-start","play",{trackIds:[savedTrack.id]})).error).toBeNull();
    const audio=document.querySelector("audio")!;
    audio.currentTime=10; audio.dispatchEvent(new Event("timeupdate")); await settle();
    writes.length=0;
    const sourceCalls=mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track"||command==="resolve_preview").length;
    expect((await automationAction("unknown-duration-seek","seek",{position:44})).error).toBeNull();
    expect(writes).toContain(44);
    expect(audio.currentTime).toBe(44);
    expect(begin).toHaveBeenCalledWith(expect.objectContaining({target:44,paused:false}));
    await vi.advanceTimersByTimeAsync(20_000); await settle();
    expect(document.querySelector(".player-scrubber .time")?.textContent).toBe("0:44");
    expect(document.querySelector(".player-error")?.textContent).toContain("choose Play to retry from the requested position");
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track"||command==="resolve_preview")).toHaveLength(sourceCalls);
  });

  it("clears the seek deadline only after a current seeked event lands near target", async () => {
    vi.useFakeTimers();
    previewMode="direct"; directPreviewExcerpt=false; savedChartTracks=[{...savedTrack,durationSeconds:240}];
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const complete=vi.spyOn(WebviewSeekCompletionWatchdog.prototype,"complete");
    const instance=mount(App,{target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    expect((await automationAction("landed-seek-start","play",{trackIds:[savedTrack.id]})).error).toBeNull();
    expect((await automationAction("landed-seek","seek",{position:44})).error).toBeNull();
    const previewCallsBefore=mocks.call.mock.calls.filter(([command])=>command==="resolve_preview").length;
    const audio=document.querySelector("audio")!; audio.currentTime=30; audio.dispatchEvent(new Event("seeked")); await settle();
    expect(complete).not.toHaveBeenCalled();
    audio.currentTime=44; audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(complete).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(19_999); await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_preview")).toHaveLength(previewCallsBefore);
  });

  it("defers a paused seek deadline until resume and replaces an older seek owner", async () => {
    vi.useFakeTimers();
    let mediaPaused=false, clock=1000;
    vi.spyOn(performance,"now").mockImplementation(()=>clock);
    previewMode="direct"; directPreviewExcerpt=false; savedChartTracks=[{...savedTrack,durationSeconds:240}];
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockImplementation(()=>mediaPaused);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const begin=vi.spyOn(WebviewSeekCompletionWatchdog.prototype,"begin");
    const resume=vi.spyOn(WebviewSeekCompletionWatchdog.prototype,"resume");
    const instance=mount(App,{target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    expect((await automationAction("paused-seek-start","play",{trackIds:[savedTrack.id]})).error).toBeNull();
    const audio=document.querySelector("audio")!; audio.currentTime=10; clock+=6_000; audio.dispatchEvent(new Event("timeupdate")); await settle();
    mediaPaused=true; document.querySelector("audio")!.dispatchEvent(new Event("pause")); await settle();
    expect((await automationAction("paused-seek","seek",{position:30})).error).toBeNull();
    expect(begin).toHaveBeenLastCalledWith(expect.objectContaining({target:30,paused:true}));
    expect((await automationAction("paused-seek-new-target","seek",{position:60})).error).toBeNull();
    expect(begin).toHaveBeenLastCalledWith(expect.objectContaining({target:60,paused:true}));
    await vi.advanceTimersByTimeAsync(30_000); expect(resume).not.toHaveBeenCalled();
    mediaPaused=false; expect((await automationAction("paused-seek-resume","resume")).error).toBeNull();
    expect(resume).toHaveBeenCalledWith(expect.objectContaining({node:audio,seekSerial:expect.any(Number)}),20_000);
    await vi.advanceTimersByTimeAsync(20_000); await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(0);
  });

  it("refreshes the steady-playback attachment on a completed seek for an initially resolved full source", async () => {
    vi.useFakeTimers();
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "direct";
    directPreviewExcerpt = false;
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const initial = await automationAction("initial-full-source", "play", { trackIds: [savedTrack.id] });
    expect(initial.error).toBeNull();
    const started = mocks.call.mock.calls.find(([command, args]) => command === "automation_playback_complete" && (args as { requestId?: string }).requestId === "initial-full-source")?.[1] as { playback: { queueEntryIds: string[] } };
    const occurrence = started.playback.queueEntryIds[0];
    const audio = document.querySelector("audio")!;
    // Request the actual target; an uninitialized slider clamps its value to its temporary max=1.
    expect((await automationAction("initial-full-source-seek", "seek", { position: 40 })).error).toBeNull();
    audio.currentTime = 40;
    audio.dispatchEvent(new Event("seeked"));
    clock += 6_000;
    audio.currentTime = 46;
    audio.dispatchEvent(new Event("timeupdate"));
    audio.dispatchEvent(new Event("waiting"));
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();

    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    document.querySelector("audio")!.currentTime += 0.3;
    clock += 25; await vi.advanceTimersByTimeAsync(25); await settle();
    const recovered = await automationAction("initial-full-source-after-stall", "volume", { volume: 0.8 });
    expect(recovered.error).toBeNull();
    expect(recovered.playback.queueEntryIds).toEqual([occurrence]);
    expect(recovered.playback.trackId).toBe(savedTrack.id);
    expect(recovered.playback.paused).toBe(false);
  });

  it("allows delayed play after recovery to advance from its verified position", async () => {
    vi.useFakeTimers();
    let clock = 1000, plays = 0;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [{ ...savedTrack, durationSeconds: 240 }];
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:zero-resume-first" },
      { ...youtubeCandidate(savedTrack), id: "youtube:zero-resume-second" },
    ];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(function(this: HTMLMediaElement) {
      if (++plays === 2) { clock += 2_000; this.currentTime = 8; }
      return Promise.resolve();
    });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect((await automationAction("zero-resume-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    const audio = document.querySelector("audio")!;
    audio.currentTime = 6;
    clock += 6_000;
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    audio.dispatchEvent(new Event("waiting"));
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();
    clock += 25; await vi.advanceTimersByTimeAsync(25); await settle();
    expect(plays).toBe(2);
    const recovered = await automationAction("zero-resume-after-stall", "volume", { volume: 0.8 });
    expect(recovered.error).toBeNull();
    expect(recovered.playback.paused).toBe(false);
  });

  it("fails closed instead of restarting near-end or unknown-catalog-duration audio at zero", async () => {
    vi.useFakeTimers();
    let clock = 1000, plays = 0;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    previewMode = "fallback";
    savedChartTracks = [savedTrack]; // Catalog duration is absent; the finite media duration remains authoritative.
    playbackResolverCandidates = [
      { ...youtubeCandidate(savedTrack), id: "youtube:near-end-first" },
      { ...youtubeCandidate(savedTrack), id: "youtube:near-end-second" },
    ];
    automationPlaybackAck = { protocolVersion: 1, accepted: true, needFull: false };
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => { plays++; return Promise.resolve(); });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect((await automationAction("near-end-start", "play", { trackIds: [savedTrack.id] })).error).toBeNull();
    const audio = document.querySelector("audio")!;
    for (let position = 5; position <= 230; position += 5) {
      clock += 5_000;
      audio.currentTime = position;
      await vi.advanceTimersByTimeAsync(5_000);
      audio.dispatchEvent(new Event("timeupdate"));
    }
    audio.currentTime = 235;
    clock += 5_000;
    await vi.advanceTimersByTimeAsync(5_000);
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    audio.currentTime = 239.5;
    clock += 4_500;
    await vi.advanceTimersByTimeAsync(4_500);
    audio.dispatchEvent(new Event("timeupdate"));
    audio.dispatchEvent(new Event("waiting"));
    clock += 20_000;
    await vi.advanceTimersByTimeAsync(20_000);
    await settle();

    // The stall path now performs one bounded fresh-candidate refresh and one fallback
    // resolution. Both must fail closed without starting a new playback at zero.
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(3);
    expect(plays).toBe(1);
    expect(audio.currentTime).toBe(239.5);
    expect(document.querySelector(".player-error")?.textContent).toContain("could not resume at its previous playback position");
  });

  it.each([
    ["http://127.0.0.1:43210/webview-audio/session?token=secret", "anonymous"],
    ["https://audio.example/stream.mp3", null],
  ])("configures relay-only CORS before media attachment (%s)", async (url, expected) => {
    previewMode = "direct";
    const baseCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command, args) => {
      const result = await baseCall(command, args);
      return command === "resolve_preview" ? { ...result, url } : result;
    });
    const atAttach: (string | null)[] = [];
    const originalLoad = HTMLMediaElement.prototype.load;
    vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(function(this: HTMLMediaElement) {
      if (this.getAttribute("src") === url) atAttach.push(this.getAttribute("crossorigin"));
      originalLoad.call(this);
    });
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row => row.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(atAttach).toContain(expected);
    expect(document.querySelector("audio")?.getAttribute("crossorigin")).toBe(expected);
  });

  it("plays a cached direct preview before searching provider candidates", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.querySelector(".player-error")).toBeNull();
    await settle();
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
    expect(document.querySelector(".player-error")).toBeNull();
  });

  it("routes a low-confidence imported source hint through the shared verified resolver", async () => {
    previewMode = "fallback";
    const hint: Candidate = { ...youtubeCandidate(savedTrack), confidence: "low", score: 0.1 };
    savedChartTracks = [{ ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: hint } }];
    playbackResolverCandidate = { ...hint, confidence: "high", score: 0.99 };
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({
      track: expect.objectContaining({ provenance: expect.objectContaining({ audioCandidate: hint }) }),
      requestGeneration: expect.any(Number), budgetMs: expect.any(Number),
    }));
    expect(play).toHaveBeenCalled();
    expect(document.querySelector(".now-playing")?.textContent).toContain("Saved Recording");
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
  });

  it("reports held manual resume startup failure without buffering, advancing or unpausing its retained queue", async () => {
    previewMode="fallback";
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const local=new Map<string,string>([
      ["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack,secondTrack],currentIndex:0,repeat:"off"})],
      ["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position:89,savedAt:Date.now()})],
    ]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    let clock=1000;vi.spyOn(performance,"now").mockImplementation(()=>clock);
    afterPlaybackResolver=()=>{clock=20900;};
    vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(()=>new Promise<void>(()=>{}));
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-playback")!({event:"automation-playback",id:1,payload:{requestId:"manual-resume-deadline",action:"resume"}});
    await settle();
    const audio=document.querySelector("audio")!;Object.defineProperty(audio,"paused",{configurable:true,value:false});audio.dispatchEvent(new Event("waiting"));
    await new Promise(resolve=>setTimeout(resolve,150));await settle();
    const completion=mocks.call.mock.calls.find(([command,args])=>command==="automation_playback_complete"&&(args as {requestId?:string}).requestId==="manual-resume-deadline")?.[1] as {error:string|null;playback:{paused:boolean;status:string;position:number;currentIndex:number;queue:Track[];playerMessage:string}};
    expect(completion).toBeDefined();expect(completion.error).toContain("20-second playback deadline");
    expect(completion.playback.paused).toBe(true);expect(completion.playback.status).not.toBe("buffering");
    expect(completion.playback.position).toBe(89);expect(completion.playback.currentIndex).toBe(0);
    expect(completion.playback.queue.map(track=>track.id)).toEqual([savedTrack.id,secondTrack.id]);
    expect(completion.playback.playerMessage).toContain("20-second playback deadline");
  });

  it("retains requested resume context when play fails after the resume seek", async () => {
    previewMode="fallback";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack],currentIndex:0,repeat:"off"})],["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position:89,savedAt:Date.now()})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(1);
    let soughtPosition:number|null=null;
    vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){soughtPosition=this.currentTime;return Promise.reject(new Error("fixture startup decode failed"));});
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const completion=await automationAction("resume-seek-play-failure","resume");
    expect(soughtPosition).toBe(89);expect(completion.error).toContain("fixture startup decode failed");
    expect(completion.playback.lastStartupFailure).toMatchObject({phase:"play",resumeRequested:true,readyState:1});
    expect(completion.playback.paused).toBe(true);
  });

  it("publishes finite pre-cleanup phases for held metadata versus held play startup", async () => {
    for(const [position,phase]of [[89,"resume_seek"],[0,"play"]] as const){
      previewMode="fallback";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
      const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack,secondTrack],currentIndex:0,repeat:"off"})],["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position,savedAt:Date.now()})]]);
      vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
      let clock=1000;vi.spyOn(performance,"now").mockImplementation(()=>clock);afterPlaybackResolver=()=>{clock=20900;};
      vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(()=>new Promise<void>(()=>{}));vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
      const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
      const requestId=`phase-${phase}`;
      eventHandlers.get("automation-playback")!({event:"automation-playback",id:1,payload:{requestId,action:"resume"}});
      await settle();await new Promise(resolve=>setTimeout(resolve,150));await settle();
      const completion=mocks.call.mock.calls.find(([command,args])=>command==="automation_playback_complete"&&(args as {requestId?:string}).requestId===requestId)?.[1] as {error:string|null;playback:{lastStartupFailure:{phase:string;readyState:number;resumeRequested:boolean};playerMessage:string;paused:boolean;queue:Track[]}};
      expect(completion.error).toContain("20-second playback deadline");
      expect(completion.playback.lastStartupFailure.phase).toBe(phase);expect(completion.playback.lastStartupFailure.readyState).toBe(0);expect(completion.playback.lastStartupFailure.resumeRequested).toBe(position>0);
      expect(completion.playback.paused).toBe(true);expect(completion.playback.queue.map(track=>track.id)).toEqual([savedTrack.id,secondTrack.id]);
      await unmount(instance);unmountApp=undefined;await settle();mocks.call.mockClear();
    }
  });

  it("bounds automatic startup by the resolver deadline and keeps late play completion off newer audio", async () => {
    previewMode = "fallback";
    savedChartTracks = [savedTrack, secondTrack];
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    afterPlaybackResolver = () => { clock = 20_900; };
    let finishPlay: (() => void) | undefined;
    let playCalls = 0;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => {
      playCalls++;
      return playCalls === 1
        ? new Promise<void>((resolve) => { finishPlay = resolve; })
        : Promise.resolve();
    });
    const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(play).toHaveBeenCalledTimes(1);
    await new Promise((resolve) => setTimeout(resolve, 150));
    await settle();

    expect(document.querySelector(".player-status")?.textContent).toContain("Unavailable");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("20-second playback deadline");
    expect(document.querySelector(".player-error")?.textContent).toContain("20-second playback deadline");
    expect(pause).toHaveBeenCalled();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Another Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-error")).toBeNull();
    const newAudio = document.querySelector("audio")!;
    const pausesWhileNewTrackPlays = pause.mock.contexts.filter(node => node === newAudio).length;
    finishPlay?.();
    await settle();
    expect(pause.mock.contexts.filter(node => node === newAudio)).toHaveLength(pausesWhileNewTrackPlays);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-error")).toBeNull();
  });

  it("applies the shared startup deadline to a cached preview before it can start stale audio", async () => {
    previewMode = "direct";
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    afterPreviewResolve = () => { clock = 21_000; };
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(play).not.toHaveBeenCalled();
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("20-second playback deadline");
    expect(document.querySelector(".player-error")?.textContent).toContain("20-second playback deadline");
  });

  it("does not start audio when the shared resolver has already consumed the startup deadline", async () => {
    previewMode = "fallback";
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    afterPlaybackResolver = () => { clock = 21_000; };
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(play).not.toHaveBeenCalled();
    expect(document.querySelector(".player-status")?.textContent).toContain("Unavailable");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("20-second playback deadline");
    expect(document.querySelector(".player-error")?.textContent).toContain("20-second playback deadline");
  });

  it("shows an actionable shared-resolver failure with source secrets redacted", async () => {
    previewMode = "fallback";
    playbackResolverFailure = "No eligible audio source at https://private.example/resolve?token=secret-value";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    const status = document.querySelector<HTMLElement>(".player-status")!;
    expect(status.textContent).toContain("Unavailable");
    expect(status.title).toContain("No eligible audio source");
    expect(status.title).not.toContain("private.example");
    expect(status.title).not.toContain("secret-value");
    const errorDisclosure = document.querySelector<HTMLElement>(".player-error")!;
    expect(errorDisclosure.textContent).toContain("No eligible audio source");
    expect(errorDisclosure.textContent).not.toContain("private.example");
    expect(errorDisclosure.textContent).not.toContain("secret-value");
    expect(errorDisclosure.closest("button")).toBeNull();
    const retry = errorDisclosure.querySelector<HTMLButtonElement>('[aria-label="Retry playback"]')!;
    expect(retry.tagName).toBe("BUTTON");
    expect(retry.tabIndex).toBe(0);
    retry.focus();
    expect(document.activeElement).toBe(retry);

    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    playbackResolverFailure = "";
    retry.click();
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(2);
    expect(play).toHaveBeenCalled();
    expect(document.querySelector(".player-error")).toBeNull();
  });

  it("cancels a pending shared playback resolution before it can start stale audio", async () => {
    previewMode = "fallback";
    delayPlaybackTrackId = savedTrack.id;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(finishPlaybackResolve).toBeDefined();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: savedTrack }));

    (document.querySelector('[aria-label="Pause loading playback"]') as HTMLButtonElement).click();
    await settle();
    finishPlaybackResolve!({
      candidate: youtubeCandidate(savedTrack),
      preview: { kind: "audio", url: "/stale-resolved-audio.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false },
    });
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("cancel_playback_resolution", expect.objectContaining({ requestGeneration: expect.any(Number) }));
    expect(play).not.toHaveBeenCalled();
  });

  it.each(["short","steady","seek"] as const)("bounds short recovery sources and renews only after steady progress=%s", async mode => {
    const steady=mode==="steady";
    previewMode = "alternates";
    playbackResolverCandidates = [0,1,2].map(index => ({ ...youtubeCandidate(savedTrack), id: `youtube:recovery-${index}` }));
    let clock = 1000;
    vi.spyOn(performance,"now").mockImplementation(() => clock);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance = mount(App,{target:document.body}); unmountApp=()=>{void unmount(instance);}; await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click",{bubbles:true})); await settle();
    let audio=document.querySelector("audio")!;
    clock += mode!=="short" ? 6000 : 2000;
    audio.currentTime += mode!=="short" ? 6 : 2;
    if(mode==="seek") await automationAction("recovery-new-seek","seek",{position:audio.currentTime});
    audio.dispatchEvent(new Event("error")); await settle();
    let calls=mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track");
    expect(calls).toHaveLength(2);
    expect(calls[1][1].budgetMs).toBe(steady ? 20000 : mode==="seek" ? 14000 : 18000);
    audio=document.querySelector("audio")!;
    clock += 2000; audio.currentTime += 2; audio.dispatchEvent(new Event("error")); await settle();
    calls=mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track");
    expect(calls).toHaveLength(steady ? 3 : 2);
    if(steady) { audio=document.querySelector("audio")!;clock+=2000;audio.currentTime+=2;audio.dispatchEvent(new Event("error"));await settle();expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(3); }
  });

  it("uses the selected 30-second deadline for an explicit pinned source whose play never settles", async () => {
    vi.useFakeTimers();let clock=1_000;vi.spyOn(performance,"now").mockImplementation(()=>clock);
    previewMode="fallback";const candidate=youtubeCandidate(savedTrack);
    savedChartTracks=[{...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:candidate}}];
    const base=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command,args)=>{
      const result=await base(command,args);
      return command==="get_settings" ? {...result,playbackStartupTimeoutSeconds:30} : result;
    });
    let finish!:()=>void;
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
    const pause=vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();await settle();
    const oldAudio=document.querySelector("audio")!;expect(play).toHaveBeenCalledTimes(1);
    const resolution=mocks.call.mock.calls.find(([command])=>command==="resolve_playback_track")![1];
    expect(resolution.budgetMs).toBeGreaterThan(0);
    expect(resolution.budgetMs).toBeLessThanOrEqual(30_000);
    clock+=29_999;await vi.advanceTimersByTimeAsync(29_999);await settle();
    expect(document.querySelector('[aria-label="Pause loading playback"]')).not.toBeNull();
    clock+=1;await vi.advanceTimersByTimeAsync(1);await settle();
    expect(document.querySelector(".player-error")?.textContent).toContain("30-second playback deadline");
    expect(document.querySelector('[aria-label="Pause loading playback"]')).toBeNull();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(1);
    finish();oldAudio.dispatchEvent(new Event("playing"));await settle();
    expect(pause.mock.contexts).toContain(oldAudio);
    expect(document.querySelector(".play-button")?.getAttribute("aria-label")).toBe("Play");
  });

  async function liveRecoveryFixture() {
    vi.useFakeTimers();
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    radioStationState = [{ id:"main",name:"Main Mix",streamUrl:"https://radio.example/live",sourceUrl:"https://radio.example",provider:"radio_paradise",isCustom:false }];
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(Infinity);
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    const play = vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const pause = vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Main Mix"))!.click();await settle();
    const advance = async (milliseconds:number) => {clock+=milliseconds;await vi.advanceTimersByTimeAsync(milliseconds);await settle();};
    return {play,pause,advance,setClock:(value:number)=>{clock=value;}};
  }

  it.each(["error","stall","ended"])("reconnects a live radio %s after minutes using a fresh station budget without a finite seek", async trigger => {
    const fixture=await liveRecoveryFixture();
    const oldAudio=document.querySelector("audio")!;
    fixture.setClock(301_000);oldAudio.currentTime=300;
    if(trigger!=="stall") {oldAudio.dispatchEvent(new Event(trigger));await settle();}
    else {oldAudio.dispatchEvent(new Event("timeupdate"));oldAudio.dispatchEvent(new Event("waiting"));await fixture.advance(5_000);}
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_radio_station_playback_url")).toHaveLength(2);
    expect(mocks.call.mock.calls.some(([command])=>command==="resolve_playback_track")).toBe(false);
    expect(fixture.play).toHaveBeenCalledTimes(2);
    expect(document.querySelector("audio")!.currentTime).toBe(0);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    oldAudio.dispatchEvent(new Event("error"));await settle();
    expect(fixture.play).toHaveBeenCalledTimes(2);
  });

  it("bounds radio no-progress reconnects and renews retry allowance after five seconds of playback", async () => {
    const fixture=await liveRecoveryFixture();
    await fixture.advance(5_000);
    expect(fixture.play).toHaveBeenCalledTimes(2);
    let audio=document.querySelector("audio")!;
    // Five continuous seconds of playable clock progress earns a new recovery episode.
    for(let second=1;second<=5;second++) {
      audio.currentTime=second;audio.dispatchEvent(new Event("timeupdate"));await fixture.advance(1_000);
    }
    audio.dispatchEvent(new Event("error"));await settle();
    expect(fixture.play).toHaveBeenCalledTimes(3);
    audio=document.querySelector("audio")!;
    await fixture.advance(5_000);
    expect(fixture.play).toHaveBeenCalledTimes(4);
    await fixture.advance(5_000);
    expect(fixture.play).toHaveBeenCalledTimes(4);
    expect(document.querySelector(".player-error")?.textContent).toContain("Radio reconnect failed");
  });

  it("cancels an owned radio reconnect lookup on pause and ignores its late transport", async () => {
    const fixture=await liveRecoveryFixture();
    const base=mocks.call.getMockImplementation()!;
    let finish!: (value:string)=>void;
    mocks.call.mockImplementation((command,args)=>command==="resolve_radio_station_playback_url" ? new Promise(resolve=>{finish=resolve;}) : base(command,args));
    fixture.setClock(61_000);const oldAudio=document.querySelector("audio")!;oldAudio.currentTime=60;
    oldAudio.dispatchEvent(new Event("error"));await settle();
    expect(finish).toBeTypeOf("function");
    await automationAction("pause-owned-radio-reconnect","pause");
    finish("https://radio.example/late-transport");await settle();
    oldAudio.dispatchEvent(new Event("playing"));await settle();
    expect(fixture.play).toHaveBeenCalledTimes(1);
    expect(document.querySelector("audio")).toBeNull();
    expect(document.querySelector(".play-button")?.getAttribute("aria-label")).toBe("Play");
  });

  it("refreshes a pinned candidate on network failure at the same playhead and never asks for alternatives", async () => {
    vi.useFakeTimers();let clock=1_000;vi.spyOn(performance,"now").mockImplementation(()=>clock);
    previewMode="fallback";const candidate=youtubeCandidate(savedTrack);
    savedChartTracks=[{...savedTrack,durationSeconds:240,provenance:{sourceKind:"audioSearch",audioCandidate:candidate}}];
    const base=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command,args)=>{
      if(command==="resolve_playback_track" && args.refreshCandidateFresh) return Promise.resolve({candidate,preview:{kind:"audio",url:"/fresh-pinned.mp3",canSeek:true,canVolume:true,canAutoplay:true,isExcerpt:false,sourceGeneration:2}});
      return base(command,args);
    });
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const starts:number[]=[];vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){starts.push(this.currentTime);return Promise.resolve();});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Saved Recording"]')!.click();await settle();
    const original=document.querySelector("audio")!;clock+=60_000;original.currentTime=60;
    Object.defineProperty(original,"error",{configurable:true,value:{code:2,message:"Network error"}});
    original.dispatchEvent(new Event("error"));await settle();
    const refreshed=document.querySelector("audio")!;expect(refreshed.currentTime).toBe(60);
    refreshed.currentTime=60.3;clock+=25;await vi.advanceTimersByTimeAsync(25);await settle();
    expect(starts).toEqual([0,60]);
    const calls=mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track");
    expect(calls).toHaveLength(2);expect(calls[1][1]).toMatchObject({refreshCandidateFresh:true,refreshCandidate:expect.objectContaining({id:candidate.id,provider:candidate.provider}),failedCandidates:[]});
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
  });

  it("refreshes a catalog source after an expired transport deadline without reopening search", async () => {
    vi.useFakeTimers();
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    const track: Track = { ...savedTrack, id: "spotify:312Jqtrb8ZGiFYEsZlz751", provider: "spotify", providerId: "312Jqtrb8ZGiFYEsZlz751", durationSeconds: 240, provenance: {} };
    savedChartTracks = [track];
    const candidate = youtubeCandidate(track);
    playbackResolverCandidates = [candidate];
    const base = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "resolve_playback_track" && (args as { refreshCandidateFresh?: boolean }).refreshCandidateFresh) {
        return Promise.resolve({ candidate, preview: { kind: "audio", url: "/fresh-catalog-url.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false, sourceGeneration: 2 } });
      }
      return base(command, args);
    });
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li")].find(row => row.textContent?.includes("Saved Recording"))!.click();
    await settle();
    const original = document.querySelector("audio")!;
    clock += 60_000; // the first lookup deadline has elapsed, with no steady media progress
    Object.defineProperty(original, "error", { configurable: true, value: { code: 2, message: "Network error" } });
    original.dispatchEvent(new Event("error"));
    await settle();
    let resolutions = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(resolutions).toHaveLength(2);
    expect(resolutions[1][1]).toMatchObject({ refreshCandidateFresh: true, refreshCandidate: expect.objectContaining({ id: candidate.id, provider: candidate.provider }), failedCandidates: [], budgetMs: 8_000 });
    const refreshed = document.querySelector("audio")!;
    Object.defineProperty(refreshed, "error", { configurable: true, value: { code: 2, message: "Network error" } });
    refreshed.dispatchEvent(new Event("error"));
    await settle();
    resolutions = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(resolutions).toHaveLength(2);
  });

  it("pinned playback media failure cannot switch to an eligible alternative", async () => {
    previewMode="alternates";
    const candidate=youtubeCandidate(savedTrack);
    savedChartTracks=[{...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:candidate}}];
    playbackResolverCandidates=[candidate,{...candidate,id:"youtube:alternative"}];
    const base=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command,args)=>{
      const result=await base(command,args);
      if(command==="resolve_playback_track") (result as any).preview.sourceHeaderPolicy={headerCount:2,authorization:true,cookie:false,referer:false,origin:false,userAgent:false,other:true,unsafe:"Bearer secret-value"};
      return result;
    });
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(1);
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("selected");
    const completion=await automationAction("inspect-failed-source","pause");
    expect(completion.playback.lastResolvedSource).toEqual({provider:"youtube",id:candidate.id,headerPolicy:{headerCount:2,authorization:true,cookie:false,referer:false,origin:false,userAgent:false,other:true}});
    expect(JSON.stringify(completion.playback.lastResolvedSource)).not.toContain("secret-value");
    expect(completion.playback.lastMediaFailureSource).toEqual(completion.playback.lastResolvedSource);

  });

  it("keeps the attached relay registration alive through automation pause and paused seek", async () => {
    savedChartTracks=[savedTrack];previewMode="fallback";
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const original=mocks.call.getMockImplementation()!;
    let registeredGeneration:number|null=null, relayRetired=false;
    mocks.call.mockImplementation((command,args)=>{
      if(command==="cancel_playback_resolution"&&registeredGeneration!==null&&Number(args?.requestGeneration)>registeredGeneration)relayRetired=true;
      const result=original(command,args);
      if(command==="resolve_playback_track")return Promise.resolve(result).then((value:any)=>{
        registeredGeneration=Number(args?.requestGeneration);
        return {...value,preview:{...value.preview,url:"http://127.0.0.1:12345/webview-audio/fixture?token=fixture",sourceGeneration:registeredGeneration}};
      });
      return result;
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();await settle();
    expect(registeredGeneration).not.toBeNull();
    const node=document.querySelector("audio")!;const source=node.src;
    const paused=await automationAction("retain-relay-pause","pause");
    expect(paused.error).toBeNull();expect(paused.playback.paused).toBe(true);
    const seek=await automationAction("retain-relay-seek","seek",{position:60});
    expect(seek.error).toBeNull();expect(node.currentTime).toBe(60);
    expect(node.src).toBe(source);expect(relayRetired).toBe(false);
  });

  async function loadedResumeFixture() {
    savedChartTracks=[savedTrack,secondTrack];
    previewMode="fallback";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const pause=vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    await automationAction("loaded-pause","pause");
    const audio=document.querySelector("audio")!;
    await automationAction("loaded-seek","seek",{position:10});
    const begin=(id:string)=>{eventHandlers.get("automation-playback")!({event:"automation-playback",id:1,payload:{requestId:id,action:"resume"}});};
    const completion=(id:string)=>mocks.call.mock.calls.find(([command,args])=>command==="automation_playback_complete"&&(args as {requestId?:string}).requestId===id)?.[1] as {error:string|null;playback:Record<string,unknown>}|undefined;
    return {play,pause,audio,begin,completion};
  }
  it("bounds a hung existing WebView resume at twenty seconds and retains paused seek state",async()=>{
    const fixture=await loadedResumeFixture();
    // A ready decoder alone is not resume proof: without media-time progress,
    // this held play promise must still hit the finite deadline.
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    fixture.play.mockImplementation(()=>new Promise<void>(()=>{}));
    const realTimeout=setTimeout;
    const timeout=vi.spyOn(globalThis,"setTimeout").mockImplementation(((callback:any,ms?:number,...args:any[])=>realTimeout(callback,ms===20000?40:ms,...args)) as typeof setTimeout);
    vi.spyOn(performance,"now").mockReturnValue(1000);
    fixture.begin("hung-resume");await settle();await new Promise(resolve=>realTimeout(resolve,70));await settle();
    const result=fixture.completion("hung-resume")!;
    expect(timeout.mock.calls.some(([,ms])=>ms===20000)).toBe(true);
    expect(result.error).toContain("20-second playback deadline");
    expect(result.playback).toMatchObject({paused:true,status:"paused",position:10,lastMediaFailure:null});
    expect(result.playback.playerMessage).toContain("could not resume");
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track")).toHaveLength(1);
  });
  it("accepts owner-bound media progress when WebView play() remains pending", async () => {
    const fixture = await loadedResumeFixture();
    let mediaPaused = true, finishPlay!: () => void;
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockImplementation(() => mediaPaused);
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    fixture.play.mockImplementationOnce(() => new Promise<void>(resolve => { finishPlay = () => resolve(); }));
    fixture.begin("progress-resume"); await settle();
    expect(fixture.completion("progress-resume")).toBeUndefined();
    mediaPaused = false;
    fixture.audio.currentTime = 10.3;
    fixture.audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    const result = fixture.completion("progress-resume")!;
    expect(result.error).toBeNull();
    expect(result.playback).toMatchObject({ paused: false, status: "playing", seekSerial: 1 });
    finishPlay(); await settle();
  });
  it.each(["pause","seek","new-source"] as const)("cancels held existing WebView resume on %s without source failure",async mode=>{
    const fixture=await loadedResumeFixture();let finish!:()=>void;
    fixture.play.mockImplementationOnce(()=>new Promise<void>(resolve=>finish=resolve));
    fixture.begin("cancel-resume");await settle();
    if(mode==="pause")await automationAction("explicit-pause","pause");
    else if(mode==="seek")await automationAction("seek-during-resume","seek",{position:15});
    else { [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes("Another Recording"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle(); }
    await new Promise(resolve=>setTimeout(resolve,40));await settle();
    const cancelled=fixture.completion("cancel-resume")!;
    expect(cancelled.error).toContain("cancelled or superseded");
    expect(cancelled.playback.lastMediaFailure).toBeNull();
    expect(cancelled.playback.lastStartupFailure).toBeNull();
    const pauses=fixture.pause.mock.calls.length;
    finish();await settle();
    if(mode==="new-source") { expect(fixture.pause.mock.calls.length).toBe(pauses);expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording"); }
    else expect(document.querySelector(".play-button")?.getAttribute("aria-label")).toBe("Play");
  });
  it("ignores an obsolete pause event while a newer resume owns the same node",async()=>{
    const fixture=await loadedResumeFixture();let finish!:()=>void;
    fixture.play.mockImplementationOnce(()=>new Promise<void>(resolve=>finish=resolve));
    fixture.begin("pause-event-resume");await settle();
    Object.defineProperty(fixture.audio,"paused",{configurable:true,value:false});
    fixture.audio.dispatchEvent(new Event("pause"));await settle();
    finish();await settle();
    const result=fixture.completion("pause-event-resume")!;
    expect(result.error).toBeNull();expect(result.playback.paused).toBe(false);
  });
  it("late seeked completion does not cancel a valid resume for that seek",async()=>{
    const fixture=await loadedResumeFixture();let finish!:()=>void;
    fixture.play.mockImplementationOnce(()=>new Promise<void>(resolve=>finish=resolve));
    const sought=await automationAction("seek-before-resume","seek",{position:15});
    const serial=sought.playback.seekSerial;
    fixture.begin("resume-after-seek");await settle();
    fixture.audio.dispatchEvent(new Event("seeked"));await settle();
    expect(fixture.completion("resume-after-seek")).toBeUndefined();
    finish();await settle();
    const result=fixture.completion("resume-after-seek")!;
    expect(result.error).toBeNull();
    expect(result.playback).toMatchObject({paused:false,position:15,seekSerial:serial,lastStartupFailure:null,lastMediaFailure:null});
    // Repeated late completions publish position but cannot create a new seek.
    fixture.audio.dispatchEvent(new Event("seeked"));await settle();
    const published=lastCallOf("automation_playback_state")![1] as {playback:Record<string,unknown>};
    expect(published.playback.seekSerial).toBe(serial);
  });
  it("late cancelled resume cannot pause a newer successful resume on the same node",async()=>{
    const fixture=await loadedResumeFixture();let finish!:()=>void;
    fixture.play.mockImplementationOnce(()=>new Promise<void>(resolve=>finish=resolve));
    fixture.begin("old-resume");await settle();await automationAction("cancel-old-resume","pause");
    const resumed=await automationAction("new-resume","resume");expect(resumed.error).toBeNull();expect(resumed.playback.paused).toBe(false);
    const pauses=fixture.pause.mock.calls.length;finish();await settle();
    expect(fixture.pause.mock.calls.length).toBe(pauses);
    expect(document.querySelector(".play-button")?.getAttribute("aria-label")).toBe("Pause");
  });

  it("tries another plausible audio result when the first source fails", async () => {
    previewMode = "alternates";
    playbackResolverCandidates = [youtubeCandidate(savedTrack), { ...youtubeCandidate(savedTrack), id: `youtube-alt:${savedTrack.id}` }];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    const started = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(started).toHaveLength(2);
    expect(started[0][1].failedCandidates).toEqual([]);
    expect(started[1][1].failedCandidates).toEqual([{ provider: "youtube", id: youtubeCandidate(savedTrack).id }]);
    expect(mocks.call.mock.calls.filter(([command]) => command === "start_candidate_preview")).toHaveLength(0);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
    const state=await automationAction("capture-recovery-source","pause");
    expect(state.playback.lastMediaFailureSource).toMatchObject({provider:"youtube",id:youtubeCandidate(savedTrack).id});
    expect(state.playback.lastResolvedSource).toMatchObject({provider:"youtube",id:`youtube-alt:${savedTrack.id}`});

  });

  it("discovers an alternative when cached audio fails after playback starts", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track")).toHaveLength(1);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

});

  it("imports Spotify and YouTube playlist search results through the overview import job", async () => {
    const original = mocks.call.getMockImplementation()!;
    let provider = "spotify";
    const commands: [string, Record<string, unknown> | undefined][] = [];
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      commands.push([command, args]);
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "spotify", name: "Spotify", kinds: ["track", "artist", "album", "playlist"], enabled: true },
        { id: "youtube", name: "YouTube", kinds: ["track", "video", "playlist"], enabled: true },
      ];
      if (command === "search_source") return { sourceId: provider, complete: true, results: [{ id: `${provider}-playlist`, kind: "playlist", sourceId: provider, title: `${provider} set`, url: provider === "spotify" ? "https://open.spotify.com/playlist/abc" : "https://www.youtube.com/playlist?list=abc" }] };
      if (command === "import_remote_playlists") return { jobId: "import-1", state: "completed", results: [{ complete: true }] };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click();
    await settle();
    for (provider of ["spotify", "youtube"]) {
      await searchPlaylistOverview(`${provider} playlist`, provider);
      const add = document.querySelector<HTMLButtonElement>(".add-playlist-result");
      expect(add).not.toBeNull();
      add!.click();
      await settle();
      expect(commands.some(([command, args]) => command === "import_remote_playlists" && args?.provider === provider && (args.playlistIds as string[])?.[0]?.includes(provider === "spotify" ? "open.spotify.com" : "youtube.com"))).toBe(true);
      expect([...document.querySelectorAll(".add-playlist-result")].some(button => button.textContent?.includes("Added to library"))).toBe(true);
    }
  });

  it("shows import progress and cancels an overview playlist import through the native job command", async () => {
    const original = mocks.call.getMockImplementation()!;
    let finishStart: ((value: { jobId: string; state: string }) => void) | undefined;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "youtube", name: "YouTube", kinds: ["track", "video", "playlist"], enabled: true }];
      if (command === "search_source") return { sourceId: "youtube", complete: true, results: [{ id: "yt-list", kind: "playlist", sourceId: "youtube", title: "Test playlist", url: "https://www.youtube.com/playlist?list=test" }] };
      if (command === "import_remote_playlists") return new Promise(resolve => { finishStart = resolve; });
      if (command === "cancel_remote_playlist_import") return undefined;
      if (command === "get_remote_playlist_import_job") return { jobId: "cancel-me", state: "cancelled", results: [] };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click();
    await settle();
    await searchPlaylistOverview("youtube list", "youtube");
    document.querySelector<HTMLButtonElement>(".add-playlist-result")!.click();
    await settle();
    expect(document.querySelector('[role="status"]')?.textContent).toMatch(/Starting playlist import/i);
    const cancel = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Cancel import");
    expect(cancel).toBeDefined();
    cancel!.click(); await settle();
    finishStart?.({ jobId: "cancel-me", state: "running" });
    await new Promise(resolve => setTimeout(resolve, 850)); await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_remote_playlist_import", { jobId: "cancel-me" });
  });

  it("reports a completed overview import as added when cancellation races with its terminal status", async () => {
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "youtube", name: "YouTube", kinds: ["track", "video", "playlist"], enabled: true }];
      if (command === "search_source") return { sourceId: "youtube", complete: true, results: [{ id: "race-list", kind: "playlist", sourceId: "youtube", title: "Race playlist", url: "https://www.youtube.com/playlist?list=race" }] };
      if (command === "import_remote_playlists") return { jobId: "race-overview", state: "running", completed: 0, total: 1 };
      if (command === "cancel_remote_playlist_import") return undefined;
      if (command === "get_remote_playlist_import_job") return { jobId: "race-overview", state: "completed", completed: 1, total: 1, results: [{ playlistId: "race", collectionId: "local-race", complete: true }], errors: [] };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchPlaylistOverview("race playlist", "youtube");
    document.querySelector<HTMLButtonElement>(".add-playlist-result")!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Cancel import")!.click(); await settle();
    await new Promise(resolve => setTimeout(resolve, 850)); await settle();
    expect(document.querySelector<HTMLButtonElement>(".add-playlist-result")?.textContent).toContain("Added to library");
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Added to library");
  });

  it("keeps a partial overview import authoritative when cancellation races with its terminal status", async () => {
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "youtube", name: "YouTube", kinds: ["track", "video", "playlist"], enabled: true }];
      if (command === "search_source") return { sourceId: "youtube", complete: true, results: [{ id: "partial-race-list", kind: "playlist", sourceId: "youtube", title: "Partial race playlist", url: "https://www.youtube.com/playlist?list=partial-race" }] };
      if (command === "import_remote_playlists") return { jobId: "partial-race-overview", state: "running", completed: 0, total: 1 };
      if (command === "cancel_remote_playlist_import") return undefined;
      if (command === "get_remote_playlist_import_job") return { jobId: "partial-race-overview", state: "partial", completed: 1, total: 1, results: [{ playlistId: "partial-race", collectionId: "local-partial", complete: false }], errors: [] };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchPlaylistOverview("partial race playlist", "youtube");
    document.querySelector<HTMLButtonElement>(".add-playlist-result")!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Cancel import")!.click(); await settle();
    await new Promise(resolve => setTimeout(resolve, 850)); await settle();
    expect(document.querySelector<HTMLButtonElement>(".add-playlist-result")?.textContent).toContain("Added to library");
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Added with missing items.");
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

  it("keeps zero-result partial playlist imports as failures instead of reporting them added", async () => {
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "spotify", name: "Spotify", kinds: ["playlist"], enabled: true }];
      if (command === "search_source") return { sourceId: "spotify", complete: true, results: [{ id: "zero-list", kind: "playlist", sourceId: "spotify", title: "Unavailable playlist", url: "https://open.spotify.com/playlist/zero" }] };
      if (command === "import_remote_playlists") return { state: "partial", results: [], errors: [{ message: "Provider returned no playlist" }] };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchPlaylistOverview("zero playlist", "spotify");
    document.querySelector<HTMLButtonElement>(".add-playlist-result")!.click(); await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Provider returned no playlist");
    expect(document.querySelector(".add-playlist-result")?.textContent).toBe("Add to library");
  });

  it("single-flights a bulk playlist append and keeps the picker open until the write finishes", async () => {
    const original = mocks.call.getMockImplementation()!;
    let finishAppend: (() => void) | undefined;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "append_playlist_entries") return new Promise<void>(resolve => { finishAppend = resolve; });
      return original(command, args);
    });
    audioSearchResult = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery("two tracks", ["youtube"]);
    expect(document.querySelectorAll('.search-track-select input')).toHaveLength(0);
    document.querySelector<HTMLButtonElement>('[aria-label="Select search tracks"]')!.click(); await settle();
    const selects = [...document.querySelectorAll<HTMLInputElement>('.search-track-select input')];
    expect(selects).toHaveLength(2);
    selects.forEach(input => input.click()); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add selected to playlist")!.click(); await settle();
    const destination = document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    destination.value = "collection-empty"; destination.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const add = [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button => button.textContent?.trim() === "Add to playlist")!;
    add.click(); await settle(); add.click(); await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "append_playlist_entries")).toHaveLength(1);
    expect(add.disabled).toBe(true);
    finishAppend?.(); await settle();
  });

  it("ignores stale collection hydration when a newer bulk picker selection opens", async () => {
    const original = mocks.call.getMockImplementation()!;
    let holdLoads = false;
    const finishes: ((value: Collection[]) => void)[] = [];
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_collections" && holdLoads) return new Promise(resolve => { finishes.push(resolve); });
      if (command === "list_collections" && !holdLoads) return [];
      return original(command, args);
    });
    audioSearchResult = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle(); holdLoads = true;
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery("two tracks", ["youtube"]);
    document.querySelector<HTMLButtonElement>('[aria-label="Select search tracks"]')!.click(); await settle();
    const checkboxes = [...document.querySelectorAll<HTMLInputElement>(".search-track-select input")];
    checkboxes[0]!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add selected to playlist")!.click(); await settle();
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).toContain("Saved Recording");
    document.querySelector<HTMLButtonElement>('[aria-label="Close playlist picker"]')!.click(); await settle();
    checkboxes[0]!.click(); checkboxes[1]!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add selected to playlist")!.click(); await settle();
    expect(finishes).toHaveLength(2);
    finishes[1]!([...collections]); await settle();
    finishes[0]!([{ id: "stale-only", name: "Stale result", trackCount: 0 }]); await settle();
    expect(document.querySelector(".playlist-picker-dialog")).not.toBeNull();
    expect(document.querySelector(".playlist-picker-dialog header p")?.textContent).toContain("Another Recording");
    expect(document.querySelector(".playlist-picker-dialog h2")?.textContent).toBe("Add recording");
  });

  it("retries a failed create-and-add by appending to the already-created playlist", async () => {
    const original = mocks.call.getMockImplementation()!;
    let appendCount = 0;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "append_playlist_entries") { appendCount++; if (appendCount === 1) throw new Error("Write interrupted"); }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle(); await chooseTrackAction("Add to playlist…");
    const name = document.querySelector<HTMLInputElement>('[aria-label="New playlist for track"]')!;
    name.value = "Retry destination"; name.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const create = [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button => button.textContent?.includes("Create & add"))!;
    create.click(); await settle();
    expect(document.querySelector(".playlist-picker-dialog")).not.toBeNull();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Write interrupted");
    create.click(); await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "create_collection")).toHaveLength(1);
    expect(mocks.call.mock.calls.filter(([command]) => command === "append_playlist_entries")).toHaveLength(2);
    const appendCalls = mocks.call.mock.calls.filter(([command]) => command === "append_playlist_entries");
    expect(appendCalls[appendCalls.length-1]).toEqual(["append_playlist_entries", { collectionId: "collection-new", tracks: [savedTrack] }]);
    expect(document.querySelector(".playlist-picker-dialog")).toBeNull();
  });

describe("native playback frontend", () => {
  function mountNative(snapshot:NativePlaybackSnapshot) {
    nativePlaybackSnapshot=snapshot;
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
  }
  function emitNative(snapshot:NativePlaybackSnapshot) {
    eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:snapshot.revision,payload:snapshot});
  }
  function expectNoPlaybackStart() {
    const starts = new Set(["play", "select", "resume", "next", "seek"]);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command"
      && starts.has(String((args as { command?: { action?: string } } | undefined)?.command?.action)))).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "play_tracks" || command === "resolve_preview")).toBe(false);
  }

  it("keeps the canonical paused track marked in catalog artist recordings", async () => {
    artistProfileTopTracks = [savedTrack];
    const paused = makeNativeSnapshot({ status: "paused", trackId: savedTrack.id, track: savedTrack, queue: [savedTrack], currentIndex: 0, paused: true });
    mountNative(paused); await settle();
    document.querySelector<HTMLButtonElement>(".artist-link")!.click(); await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click(); await settle();
    const row = document.querySelector<HTMLElement>('[aria-label="Artist top tracks"] li[aria-current="true"]')!;
    expect(row).not.toBeNull();
    expect(row.classList.contains("paused")).toBe(true);
    expect(row.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe("Paused Saved Recording");
    expectNoPlaybackStart();
  });

  it("keeps paused and playing state in a searched album release view", async () => {
    discoveryAlbum = {
      release: { provider: "musicbrainz", id: "paused-search-album" }, title: "Paused Search Album", year: 2025,
      primaryType: "Album", trackCount: 1, sourceUrl: "https://musicbrainz.org/release/paused-search-album", tracks: [],
      credits: [{ name: "Visible Artist", role: "artist", artist: { provider: "musicbrainz", id: "artist-1" }, sourceUrl: "https://musicbrainz.org/artist/artist-1" }],
    };
    discoveryAlbumTracks = [savedTrack];
    const paused = makeNativeSnapshot({ status: "paused", trackId: savedTrack.id, track: savedTrack, queue: [savedTrack], currentIndex: 0, paused: true });
    mountNative(paused); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button => button.textContent?.includes("Search"))!.click(); await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Albums")!.click(); await settle();
    await searchDiscovery("Paused Search Album", ["MusicBrainz"]);
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    const row = document.querySelector<HTMLElement>('[aria-label="Release tracks"] li[aria-current="true"]')!;
    expect(row).not.toBeNull();
    expect(row.classList.contains("paused")).toBe(true);
    expect(row.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe("Paused Saved Recording");
    emitNative({ ...paused, revision: 2, status: "playing", paused: false }); await settle();
    expect(row.classList.contains("paused")).toBe(false);
    expect(row.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe("Playing Saved Recording");
    emitNative({ ...paused, revision: 3, status: "buffering", paused: false }); await settle();
    expect(row.classList.contains("buffering")).toBe(true);
    expect(row.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe("Buffering Saved Recording");
    expectNoPlaybackStart();
  });

  it("restores blacklisted artwork after display-cache eviction without changing track metadata",async()=>{
    const original=mocks.call.getMockImplementation()!,data="data:image/jpeg;base64,/9j/AA==";
    mocks.call.mockImplementation((command,args)=>command==='read_cached_artwork_display'?Promise.resolve(data):original(command,args));
    const artwork=(n:number)=>`asset://localhost/%2Fown%2Fartwork%2F${n}.jpg`;
    const snapshot=(n:number,revision:number)=>{const track={...savedTrack,id:`artwork-${n}`,artworkUrl:artwork(n)};return makeNativeSnapshot({revision,status:'paused',trackId:track.id,track,queue:[track],currentIndex:0,position:7,paused:true});};
    mountNative(snapshot(1,1));await settle();
    for(let n=1;n<=4;n++){if(n>1){emitNative(snapshot(n,n));await settle();}document.querySelector<HTMLImageElement>('footer.player .mini-art img')!.dispatchEvent(new Event('error'));await settle();expect(document.querySelector<HTMLImageElement>('footer.player .mini-art img')?.getAttribute('src')).toBe(data);}
    emitNative(snapshot(1,5));await settle();
    expect(document.querySelector<HTMLImageElement>('footer.player .mini-art img')?.getAttribute('src')).toBe(data);
    expect(mocks.call.mock.calls.filter(([command,args])=>command==='read_cached_artwork_display'&&(args as {path?:string})?.path==='/own/artwork/1.jpg')).toHaveLength(2);
    emitNative(snapshot(1,6));await settle();expect(mocks.call.mock.calls.filter(([command,args])=>command==='read_cached_artwork_display'&&(args as {path?:string})?.path==='/own/artwork/1.jpg')).toHaveLength(2);
    expect(nativePlaybackSnapshot?.track?.artworkUrl).toBe(artwork(1));expect(mocks.call.mock.calls.some(([command])=>command==='playback_command')).toBe(false);
  });
  it("recovers current artwork rejected by pending capacity without publishing stale images",async()=>{
    const original=mocks.call.getMockImplementation()!,releases=new Map<string,(value:string)=>void>(),data="data:image/jpeg;base64,/9j/AA==";
    mocks.call.mockImplementation((command,args)=>command==='read_cached_artwork_display'?new Promise<string>(resolve=>releases.set((args as {path:string}).path,resolve)):original(command,args));
    const snapshot=(n:number)=>{const track={...savedTrack,id:`pending-art-${n}`,artworkUrl:`asset://localhost/%2Fown%2Fartwork%2F${n}.jpg`};return makeNativeSnapshot({revision:n,status:'paused',trackId:track.id,track,queue:[track],currentIndex:0,paused:true});};
    mountNative(snapshot(1));await settle();for(let n=1;n<=4;n++){if(n>1){emitNative(snapshot(n));await settle();}document.querySelector<HTMLImageElement>('footer.player .mini-art img')!.dispatchEvent(new Event('error'));await settle();}
    expect(releases.size).toBe(3);releases.get('/own/artwork/1.jpg')!(data);await settle();expect(releases.has('/own/artwork/4.jpg')).toBe(true);expect(document.querySelector('footer.player .mini-art img')).toBeNull();
    releases.get('/own/artwork/4.jpg')!(data);await settle();expect(document.querySelector<HTMLImageElement>('footer.player .mini-art img')?.getAttribute('src')).toBe(data);
    for(const n of [2,3])releases.get(`/own/artwork/${n}.jpg`)!(data);await settle();expect(document.querySelector<HTMLImageElement>('footer.player .mini-art img')?.getAttribute('src')).toBe(data);
  });
  it("keeps a persistent footer cover while details and stale artwork completions change",async()=>{
    const original=mocks.call.getMockImplementation()!,releases=new Map<string,(value:string)=>void>();
    const data=(n:number)=>`data:image/jpeg;base64,/9j/A${n}==`;
    const tracks=Array.from({length:5},(_,i)=>({...savedTrack,id:`surface-art-${i+1}`,title:`Surface artwork ${i+1}`,artworkUrl:`asset://localhost/%2Fown%2Fartwork%2F${i+1}.jpg`}));savedChartTracks=tracks;
    mocks.call.mockImplementation((command,args)=>command==='read_cached_artwork_display'?((args as {path:string}).path==='/own/artwork/1.jpg'?Promise.resolve(data(1)):new Promise<string>(resolve=>releases.set((args as {path:string}).path,resolve))):original(command,args));
    mountNative(makeNativeSnapshot({revision:1,status:'paused',trackId:tracks[0]!.id,track:tracks[0],queue:[tracks[0]!],currentIndex:0,paused:true}));await settle();document.querySelector<HTMLImageElement>('footer.player .mini-art img')!.dispatchEvent(new Event('error'));await settle();
    for(let n=2;n<=5;n++){await chooseTrackAction('Track information',tracks[n-1]!.title);document.querySelector<HTMLImageElement>('.playing-info .track-hero img')!.dispatchEvent(new Event('error'));await settle();}
    expect(releases.size).toBe(3);releases.get('/own/artwork/2.jpg')!(data(2));await settle();expect(releases.has('/own/artwork/5.jpg')).toBe(true);releases.get('/own/artwork/5.jpg')!(data(5));await settle();
    for(const n of [3,4])releases.get(`/own/artwork/${n}.jpg`)!(data(n));await settle();
    // Re-publish cache after stale completions: a fixed footer must still be present.
    await chooseTrackAction('Track information',tracks[3]!.title);await settle();
    expect(document.querySelector<HTMLImageElement>('footer.player .mini-art img')?.getAttribute('src')).toBe(data(1));expect(document.querySelector<HTMLImageElement>('.playing-info .track-hero img')?.getAttribute('src')).toBe(data(4));
    expect(mocks.call.mock.calls.filter(([command,args])=>command==='read_cached_artwork_display'&&(args as {path?:string})?.path==='/own/artwork/1.jpg')).toHaveLength(1);expect(mocks.call.mock.calls.some(([command])=>command==='playback_command')).toBe(false);
  });
  it("announces the canonical main-table row as playing, paused, and buffering", async () => {
    const sameTitleOtherRecording = { ...savedTrack, id: "same-title-other-recording", providerId: "other-provider-id" };
    savedChartTracks = [savedTrack, sameTitleOtherRecording, secondTrack];
    const initial = makeNativeSnapshot({ revision: 70, generation: 24, status: "playing", trackId: savedTrack.id, track: savedTrack,
      queue: savedChartTracks, currentIndex: 0, paused: false, position: 4, duration: 240 });
    mountNative(initial);
    await settle();

    const currentRows = () => [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li[aria-current='true']")];
    const status = () => currentRows()[0]?.querySelector<HTMLElement>(".table-playback-state");
    expect(currentRows()).toHaveLength(1);
    expect(currentRows()[0]?.textContent).toContain(savedTrack.title);
    expect(status()?.textContent).toBe("♫");
    expect(status()?.getAttribute("aria-label")).toBe(`Playing ${savedTrack.title}`);
    const playControl = () => currentRows()[0]?.querySelector<HTMLButtonElement>(".position-play");
    expect(playControl()?.getAttribute("aria-label")).toBe(`Play ${savedTrack.title}`);
    expect(playControl()?.getAttribute("aria-busy")).toBe("false");
    expect(currentRows()[0]?.classList.contains("buffering")).toBe(false);

    const paused = { ...initial, revision: 71, status: "paused" as const, paused: true };
    emitNative(paused); await settle();
    expect(currentRows()).toHaveLength(1);
    expect(currentRows()[0]?.classList.contains("paused")).toBe(true);
    expect(status()?.textContent).toBe("Ⅱ");
    expect(status()?.getAttribute("aria-label")).toBe(`Paused ${savedTrack.title}`);
    expect(playControl()?.classList.contains("paused")).toBe(true);
    expect(playControl()?.getAttribute("aria-busy")).toBe("false");

    const buffering = { ...initial, revision: 72, status: "buffering" as const, paused: false, playerMessage: "Buffering audio…" };
    emitNative(buffering); await settle();
    expect(currentRows()).toHaveLength(1);
    expect(status()?.textContent).toBe("…");
    expect(status()?.getAttribute("aria-label")).toBe(`Buffering ${savedTrack.title}`);
    expect(playControl()?.classList.contains("buffering")).toBe(true);
    expect(playControl()?.getAttribute("aria-busy")).toBe("true");
    const unrelatedSameTitle = [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")].find(row => row.getAttribute("aria-current") !== "true");
    expect(unrelatedSameTitle?.querySelector<HTMLButtonElement>(".position-play")?.getAttribute("aria-busy")).toBe("false");

    const unknownRange = { parentTrackId: "same-id-parent", chapterId: "part", sourceUrl: "https://example.test/mix", startTime: 30, endTime: 60, parentTitle: "Unknown range" };
    const unknownPlaybackIdentity = { ...savedTrack, playbackRange: unknownRange };
    emitNative({ ...initial, revision: 73, status: "playing", paused: false, track: unknownPlaybackIdentity, trackId: unknownPlaybackIdentity.id, queue: [unknownPlaybackIdentity] }); await settle();
    expect(currentRows()).toHaveLength(0);
  });

  it("does not refetch detail files for position-only native snapshots", async () => {
    const track = { ...savedTrack, localAvailability: "available" as const };
    savedAssets = [{ id: "native-asset", recordingId: track.id, path: "/music/native.flac", hash: "native-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track }];
    trackFilesByRequestedId.set(track.id, savedAssets);
    const initial = makeNativeSnapshot({ revision: 1, generation: 1, status: "playing", trackId: track.id, track,
      queue: [track], currentIndex: 0, paused: false, position: 0 });
    mountNative(initial);
    await settle();
    const fileCalls = () => mocks.call.mock.calls.filter(([command]) => command === "list_track_files_for_recordings").length;
    const statusCalls = () => mocks.call.mock.calls.filter(([command]) => command === "get_analysis_status_for_assets").length;
    const initialFileCalls = fileCalls(), initialStatusCalls = statusCalls();
    expect(initialFileCalls).toBeGreaterThan(0);
    expect(initialStatusCalls).toBeGreaterThan(0);

    emitNative({ ...initial, revision: 2, position: 1 });
    await settle();
    emitNative({ ...initial, revision: 3, position: 2 });
    await settle();
    expect(fileCalls()).toBe(initialFileCalls);
    expect(statusCalls()).toBe(initialStatusCalls);
  });

  it("loads parent and explicit asset files for a virtual chapter without changing its excerpt duration", async () => {
    const parent = { ...savedTrack, id: "full-set", assetId: "chapter-parent-asset" };
    const parentAsset = { id: "chapter-parent-asset", recordingId: parent.id, path: "/music/full-set.flac", hash: "parent-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track: parent } satisfies LocalAsset;
    const alternate = { ...parentAsset, id: "chapter-alternate", path: "/music/full-set-copy.mp3", hash: "alternate-hash" } satisfies LocalAsset;
    savedAssets = [parentAsset, alternate];
    trackFilesByRequestedId.set(parent.id, savedAssets);
    const chapter: Track = { ...parent, id: "chapter:full-set:verse", provider: "chapter", durationSeconds: 30, playbackRange: {
      parentTrackId: parent.id, chapterId: "verse", sourceUrl: parent.sourceUrl, startTime: 72, endTime: 102, parentTitle: "Full Set",
    } };
    mountNative(makeNativeSnapshot({ revision: 70, generation: 20, status: "playing", trackId: chapter.id, track: chapter,
      queue: [chapter], currentIndex: 0, paused: false, position: 0, duration: 30 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const info = document.querySelector(".playing-info")!;
    expect(info.textContent).toContain("full-set.flac");
    expect(info.textContent).toContain("full-set-copy.mp3");
    expect(mocks.call).toHaveBeenCalledWith("list_assets_by_ids", { assetIds: ["chapter-parent-asset"] });
    expect(chapter.durationSeconds).toBe(30);
  });

  it("refreshes the mounted details file group after choosing a preferred recording file", async () => {
    const track = { ...savedTrack, localAvailability: "available" as const };
    const first = { id: "preferred-first", recordingId: track.id, path: "/music/preferred-first.flac", hash: "first-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "complete", track } satisfies LocalAsset;
    const second = { ...first, id: "preferred-second", path: "/music/preferred-second.mp3", hash: "second-hash", sourceCodec: "mp3", finalCodec: "mp3" } satisfies LocalAsset;
    const replacement = { ...first, id: "preferred-after-change", path: "/music/preferred-after-change.ogg", hash: "replacement-hash", sourceCodec: "ogg", finalCodec: "ogg" } satisfies LocalAsset;
    savedAssets = [first, second, replacement];
    trackFilesByRequestedId.set(track.id, savedAssets);
    mountNative(makeNativeSnapshot({ revision: 71, generation: 21, status: "playing", trackId: track.id, track,
      queue: [track], currentIndex: 0, paused: false, position: 0 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const alternateButton = [...document.querySelectorAll<HTMLButtonElement>(".playing-info .file-row button")]
      .find(button => button.textContent?.trim() === "Use this file");
    expect(alternateButton).toBeDefined();
    alternateButton!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("set_preferred_track_file", { trackId: track.id, assetId: "preferred-first" });
    expect(document.querySelector(".playing-info")?.textContent).toContain("preferred-after-change.ogg");
    expect(document.querySelector(".playing-info")?.textContent).not.toContain("preferred-second.mp3");
  });

  it("shows a safe native source failure through recovery and clears it on successful audio", async () => {
    const failed = makeNativeSnapshot({
      revision: 44, generation: 8, status: "buffering", trackId: savedTrack.id, track: savedTrack,
      queue: [savedTrack], currentIndex: 0, paused: false, playerMessage: "Unavailable",
      diagnostics: { lastSourceFailure: { trackId: savedTrack.id, title: savedTrack.title, artist: savedTrack.artist, reason: "No eligible audio source at https://private.example/resolve token=secret-value" } },
    });
    mountNative(failed);
    await settle();
    const playerStatus = document.querySelector<HTMLElement>(".player-status")!;
    expect(playerStatus.textContent).toBe("Buffering…");
    expect(document.querySelector(".player-error")).toBeNull();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("No eligible audio source");
    expect(document.body.textContent).not.toContain("private.example");
    expect(document.body.textContent).not.toContain("secret-value");

    emitNative({ ...failed, revision: 45, status: "failed", paused: false });
    await settle();
    expect(document.querySelector(".player-error")?.textContent).toContain("No eligible audio source");
    expect(document.querySelector(".player-status")?.textContent).toContain("Could not play Visible Artist — Saved Recording");
    expect(document.querySelector<HTMLElement>(".player-status")?.title).not.toContain("private.example");
    expect(document.querySelector<HTMLElement>(".player-status")?.title).not.toContain("secret-value");

    emitNative({ ...failed, revision: 46, status: "buffering", paused: false });
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toBe("Buffering…");
    expect(document.querySelector(".player-error")).toBeNull();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("No eligible audio source");

    emitNative({ ...failed, revision: 47, status: "playing", paused: false, playerMessage: "Playing audio", diagnostics: { lastSourceFailure: null } });
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toBe("Playing audio");
    expect(document.querySelector(".player-error")).toBeNull();

    emitNative({ ...failed, revision: 48, status: "paused", paused: true, playerMessage: "Paused" });
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toBe("Paused");
    expect(document.querySelector(".player-error")).toBeNull();
  });

  it("clears a playback-skip notice for explicit MCP native next and previous", async () => {
    const skipped = { trackId: savedTrack.id, title: "Old native skip", artist: "Old artist", reason: "No playable audio match" };
    mountNative(makeNativeSnapshot({
      status: "playing", trackId: savedTrack.id, track: savedTrack,
      queue: [savedTrack, secondTrack], currentIndex: 0, paused: false,
      diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: skipped },
    }));
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Old native skip");
    expect((await automationAction("explicit-native-next", "next")).error).toBeNull();
    expect(document.querySelector(".banner.notice")).toBeNull();

    const nextSkipped = { ...skipped, title: "Another native skip" };
    emitNative(makeNativeSnapshot({
      revision: 2, status: "playing", trackId: secondTrack.id, track: secondTrack,
      queue: [savedTrack, secondTrack], currentIndex: 1, paused: false,
      diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: nextSkipped },
    }));
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Another native skip");
    expect((await automationAction("explicit-native-previous", "previous")).error).toBeNull();
    expect(document.querySelector(".banner.notice")).toBeNull();
  });

  it("clears a skip notice for paused WebView MCP navigation, not failed play hydration", async () => {
    await mountQueueFailureFixture("No playable audio match was found for the skipped recording");
    const skipMessage = document.querySelector(".banner.notice")?.textContent ?? "";
    expect(skipMessage).toContain("Skipped Unavailable middle album recording");
    expect((await automationAction("paused-webview-pause", "pause")).error).toBeNull();
    expect((await automationAction("paused-webview-previous", "previous")).error).toBeNull();
    expect(document.querySelector(".banner.notice")).toBeNull();

    const skipped = { trackId: savedTrack.id, title: "Retain after invalid play", artist: "Fixture", reason: "No playable audio match" };
    emitNative(makeNativeSnapshot({
      revision: 70, status: "playing", trackId: secondTrack.id, track: secondTrack,
      queue: [savedTrack, secondTrack], currentIndex: 1, paused: false,
      diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: skipped },
    }));
    await settle();
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) =>
      command === "get_tracks" ? [] : original(command, args));
    const invalid = await automationAction("invalid-play-hydration", "play", { trackIds: ["missing-track"] });
    expect(invalid.error).toContain("Unknown or empty track IDs");
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Retain after invalid play");
  });

  it("clears only playback-skip notices when an explicit track play starts", async () => {
    const skipped = { trackId: savedTrack.id, title: "Old skipped recording", artist: "Old artist", reason: "No playable audio match" };
    mountNative(makeNativeSnapshot({
      status: "playing", trackId: secondTrack.id, track: secondTrack,
      queue: [secondTrack], currentIndex: 0, paused: false,
      diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: skipped },
    }));
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Old skipped recording");
    for (let attempt = 0; attempt < 3; attempt++) {
      document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();
      await settle();
    }
    expect(document.querySelector(".banner.notice")).toBeNull();

    const warning = eventHandlers.get("provider-warning");
    warning!({ event: "provider-warning", id: 900, payload: "An unrelated provider notice" });
    await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("An unrelated provider notice");
  });

  it("shows a skipped recording failure without marking the next track failed and honors dismissal/new queue", async () => {
    const skipped = {
      trackId: savedTrack.id,
      title: savedTrack.title,
      artist: savedTrack.artist,
      reason: "No playable audio match was found for this recording",
    };
    const next = makeNativeSnapshot({
      revision: 1,
      queueRevision: 1,
      generation: 2,
      status: "playing",
      trackId: secondTrack.id,
      track: secondTrack,
      queue: [savedTrack, secondTrack],
      currentIndex: 1,
      paused: false,
      playerMessage: "Playing audio",
      diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: skipped },
    });
    mountNative(next);
    await settle();
    expect(document.querySelector(".player-error")).toBeNull();
    expect(document.querySelector(".player-status")?.textContent).toBe("Playing audio");
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Could not play Visible Artist — Saved Recording");
    expect(document.querySelector(".banner.notice")?.textContent).toContain("No playable audio match");
    vi.useFakeTimers();
    try {
      const another={...skipped,title:"Another failed recording"};
      emitNative({...next,revision:2,diagnostics:{lastSourceFailure:another,lastSkippedTrackFailure:null}});await settle();
      await vi.advanceTimersByTimeAsync(6500);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Another failed recording");
      emitNative({...next,revision:3,diagnostics:{lastSourceFailure:null,lastSkippedTrackFailure:{...skipped,title:"Latest skipped recording"}}});await settle();
      await vi.advanceTimersByTimeAsync(6500);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Latest skipped recording");
    } finally {vi.useRealTimers();}

    document.querySelector<HTMLButtonElement>('[aria-label="Dismiss notice"]')!.click();
    emitNative({ ...next, revision: 4, diagnostics:{lastSourceFailure:null,lastSkippedTrackFailure:{...skipped,title:"Latest skipped recording"}} });
    await settle();
    expect(document.querySelector(".banner.notice")).toBeNull();

    emitNative({ ...next, revision: 5, queueRevision: 2, diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: null } });
    await settle();
    emitNative({ ...next, revision: 6, queueRevision: 2, diagnostics: { lastSourceFailure: null, lastSkippedTrackFailure: skipped } });
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent).toContain("No playable audio match");
    expect(document.querySelector(".player-error")).toBeNull();
  });

  it("retries the active duplicate occurrence in a native queue", async () => {
    const duplicateQueue = [savedTrack, secondTrack, savedTrack];
    mountNative(makeNativeSnapshot({
      revision: 49, generation: 10, status: "failed", trackId: savedTrack.id, track: savedTrack,
      queue: duplicateQueue, currentIndex: 2, paused: false, playerMessage: "Unavailable",
      diagnostics: { lastSourceFailure: { trackId: savedTrack.id, title: savedTrack.title, artist: savedTrack.artist, reason: "No eligible audio source" } },
    }));
    await settle();

    document.querySelector<HTMLButtonElement>('[aria-label="Retry playback"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({ action: "select", index: 2 }) });
    const selectedIndexes = mocks.call.mock.calls
      .filter(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "select")
      .map(([, args]) => (args as { command: { index: number } }).command.index);
    expect(selectedIndexes).toEqual([2]);
  });

  it("opens an active source without pausing or seeking when source-open preferences are off", async () => {
    const candidate = youtubeCandidate(savedTrack);
    mountNative(makeNativeSnapshot({ revision: 45, generation: 9, status: "playing", trackId: savedTrack.id, track: savedTrack, candidate, queue: [savedTrack], currentIndex: 0, paused: false, position: 37, duration: 240 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find((button) => button.textContent?.includes("Open youtube source"))?.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: candidate.url });
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "pause")).toBe(false);
  });

  it("uses current-track source preferences from shared history rows but leaves other recordings untouched", async () => {
    configuredPauseOnSourceOpen=true;configuredSourceOpenAtPosition=true;
    const chapter:Track={...savedTrack,id:"shared-source-chapter",sourceUrl:"https://youtube.com/watch?v=abcdefghi12",playbackRange:{parentTrackId:"full-set",chapterId:"shared-source-chapter",sourceUrl:"https://youtube.com/watch?v=abcdefghi12",startTime:72,endTime:102,parentTitle:"Full Set"}};
    const other:Track={...secondTrack,sourceUrl:"https://youtube.com/watch?v=other123456"};
    historyEntries=[chapter,other].map((track,index)=>({id:`source-history-${index}`,track,provider:"youtube",sessionId:`source-session-${index}`,playedAt:"2026-10-12T00:00:00Z"}));
    mountNative(makeNativeSnapshot({revision:46,generation:9,status:"playing",trackId:chapter.id,track:chapter,queue:[chapter],currentIndex:0,paused:false,position:17,duration:30}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button=>button.textContent?.includes("History"))!.click();await settle();
    const rows=[...document.querySelectorAll<HTMLElement>('.history-page .shared-track-list li')];expect(rows).toHaveLength(2);
    rows[1]!.querySelector<HTMLButtonElement>('.more-trigger')!.click();await settle();
    document.querySelector<HTMLButtonElement>('.track-menu .source-link')!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url",{url:other.sourceUrl});
    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args as {command?:{action?:string}})?.command?.action==="pause")).toBe(false);
    rows[0]!.querySelector<HTMLButtonElement>('.more-trigger')!.click();await settle();
    document.querySelector<HTMLButtonElement>('.track-menu .source-link')!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"pause"}});
    expect(mocks.call).toHaveBeenCalledWith("open_external_url",{url:"https://youtube.com/watch?v=abcdefghi12&t=89s"});
    expect(mocks.call.mock.calls.filter(([command,args])=>command==="open_external_url"&&(args as {url?:string})?.url===other.sourceUrl)).toHaveLength(1);
    expect(mocks.call.mock.calls.filter(([command,args])=>command==="open_external_url"&&(args as {url?:string})?.url==="https://youtube.com/watch?v=abcdefghi12&t=89s")).toHaveLength(1);
  });

  it("pauses and opens the current YouTube position including a chapter offset when enabled", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    const chapter: Track = { ...savedTrack, id: "chapter-active", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", playbackRange: { parentTrackId: "full-set", chapterId: "chapter-active", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", startTime: 72, endTime: 102, parentTitle: "Full Set" } };
    const candidate = { ...youtubeCandidate(chapter), url: "https://youtube.com/watch?v=abcdefghi12" };
    mountNative(makeNativeSnapshot({ revision: 46, generation: 9, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find((button) => button.textContent?.includes("Open youtube source"))?.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "pause" } });
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://youtube.com/watch?v=abcdefghi12&t=89s" });
  });

  it("keeps the source timestamp from click time while a native pause is pending", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    deferNativePause = true;
    const chapter: Track = { ...savedTrack, id: "chapter-deferred-pause", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", playbackRange: { parentTrackId: "full-set", chapterId: "chapter-deferred-pause", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", startTime: 72, endTime: 102, parentTitle: "Full Set" } };
    const candidate = { ...youtubeCandidate(chapter), url: "https://youtube.com/watch?v=abcdefghi12" };
    mountNative(makeNativeSnapshot({ revision: 50, generation: 11, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find(button => button.textContent?.includes("Open youtube source"))!.click();
    await settle();
    expect(finishNativePause).toBeDefined();
    emitNative(makeNativeSnapshot({ revision: 51, generation: 11, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 105, duration: 200 }));
    finishNativePause!();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://youtube.com/watch?v=abcdefghi12&t=89s" });
  });

  it("routes the Details source link through active-track pause and timestamp preferences", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    const chapter: Track = { ...savedTrack, id: "chapter-details-active", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", playbackRange: { parentTrackId: "full-set", chapterId: "chapter-details-active", sourceUrl: "https://youtube.com/watch?v=abcdefghi12", startTime: 72, endTime: 102, parentTitle: "Full Set" } };
    const candidate = { ...youtubeCandidate(chapter), url: "https://youtube.com/watch?v=abcdefghi12" };
    mountNative(makeNativeSnapshot({ revision: 49, generation: 10, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find(button => button.textContent?.includes("Search for more info"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>("dialog.info-search .source")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "pause" } });
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: "https://youtube.com/watch?v=abcdefghi12&t=89s" });
  });

  it("treats the exact parent source as active while its native chapter is playing", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    const sourceUrl = "https://youtube.com/watch?v=abcdefghi12";
    const parent = { ...savedTrack, sourceUrl };
    const chapter: Track = { ...parent, id: "chapter-parent-source", provider: "chapter", playbackRange: {
      parentTrackId: parent.id, chapterId: "part", sourceUrl, startTime: 72, endTime: 102, parentTitle: "Full Set",
    } };
    const candidate = { ...youtubeCandidate(parent), url: sourceUrl };
    savedChartTracks = [parent];
    mountNative(makeNativeSnapshot({ revision: 52, generation: 12, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();

    const menu = await trackActions(parent.title);
    menu.querySelector<HTMLButtonElement>("button.source-link")!.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "pause" } });
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: `${sourceUrl}&t=89s` });
  });

  it("does not match a parent track when its chosen source differs from the playing chapter source", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    const sourceUrl = "https://youtube.com/watch?v=parentvideo1";
    const chapterUrl = "https://youtube.com/watch?v=chaptervid1";
    const parent = { ...savedTrack, sourceUrl };
    const chapter: Track = { ...parent, id: "chapter-different-source", provider: "chapter", playbackRange: {
      parentTrackId: parent.id, chapterId: "part", sourceUrl: chapterUrl, startTime: 72, endTime: 102, parentTitle: "Full Set",
    } };
    const candidate = { ...youtubeCandidate(parent), url: chapterUrl };
    savedChartTracks = [parent];
    mountNative(makeNativeSnapshot({ revision: 53, generation: 13, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();

    const menu = await trackActions(parent.title);
    menu.querySelector<HTMLButtonElement>("button.source-link")!.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: sourceUrl });
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "pause")).toBe(false);
  });

  it("does not open a parent source after its active chapter changes during the pause", async () => {
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    deferNativePause = true;
    const sourceUrl = "https://youtube.com/watch?v=abcdefghi12";
    const parent = { ...savedTrack, sourceUrl };
    const chapter: Track = { ...parent, id: "chapter-parent-pause-race", provider: "chapter", playbackRange: {
      parentTrackId: parent.id, chapterId: "part", sourceUrl, startTime: 72, endTime: 102, parentTitle: "Full Set",
    } };
    const candidate = { ...youtubeCandidate(parent), url: sourceUrl };
    savedChartTracks = [parent];
    mountNative(makeNativeSnapshot({ revision: 54, generation: 14, status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false, position: 17, duration: 30 }));
    await settle();

    const menu = await trackActions(parent.title);
    menu.querySelector<HTMLButtonElement>("button.source-link")!.click();
    await settle();
    expect(finishNativePause).toBeDefined();

    emitNative(makeNativeSnapshot({ revision: 55, generation: 15, status: "playing", trackId: secondTrack.id, track: secondTrack, queue: [secondTrack], currentIndex: 0, paused: false, position: 8, duration: 240 }));
    await settle();
    finishNativePause!();
    await settle();

    expect(mocks.call.mock.calls.some(([command]) => command === "open_external_url")).toBe(false);
    expect(document.querySelector(".player-track-title")?.textContent).toBe(secondTrack.title);
  });

  it("keeps WebView parent playback time absolute when opening its source", async () => {
    previewMode = "direct";
    configuredSourceOpenAtPosition = true;
    const sourceUrl = "https://youtube.com/watch?v=abcdefghi12";
    const parent = { ...savedTrack, title: "Full Set", sourceUrl, durationSeconds: 240 };
    playbackResolverCandidate = {
      ...youtubeCandidate(parent), url: sourceUrl, durationSeconds: 240,
      chapters: [{ id: "part", title: "Middle", startTime: 72, endTime: 102, confidence: "review" }],
    };
    savedChartTracks = [parent];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Full Set"]')!.click();
    await settle();
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 89 });
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    const menu = await trackActions(parent.title);
    menu.querySelector<HTMLButtonElement>("button.source-link")!.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: `${sourceUrl}&t=89s` });
  });

  it("opens a parent source at absolute time during WebView chapter playback", async () => {
    previewMode = "direct";
    configuredPauseOnSourceOpen = true;
    configuredSourceOpenAtPosition = true;
    const sourceUrl = "https://youtube.com/watch?v=abcdefghi12";
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), url: sourceUrl, durationSeconds: 240,
      chapters: [{ id: "middle", title: "Middle", startTime: 72, endTime: 102, confidence: "review" }],
    };
    const parent = { ...savedTrack, title: "Full Set", sourceUrl, durationSeconds: 240, provenance: { audioCandidate: candidate } };
    playbackResolverCandidate = candidate;
    savedChartTracks = [parent];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Full Set"]')!.click();
    await settle();

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    document.querySelector<HTMLElement>('[aria-label="More actions for Full Set"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find((button) => button.textContent === "Show chapters")!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play chapter Middle"]')!.click();
    await settle();
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 89 });
    Object.defineProperty(audio, "paused", { configurable: true, value: false });
    audio.dispatchEvent(new Event("play"));
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();

    const menu = await trackActions(parent.title);
    menu.querySelector<HTMLButtonElement>("button.source-link")!.click();
    await settle();

    expect(pause).toHaveBeenCalled();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: `${sourceUrl}&t=89s` });
  });

  it("does not pause active playback when opening a different recording's source link", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const candidate = youtubeCandidate(savedTrack);
    mountNative(makeNativeSnapshot({ revision: 47, generation: 9, status: "playing", trackId: savedTrack.id, track: savedTrack, candidate, queue: [savedTrack], currentIndex: 0, paused: false, position: 37, duration: 240 }));
    await settle();
    const menu = await trackActions(secondTrack.title);
    const sourceLink = [...menu.querySelectorAll<HTMLButtonElement>("button.source-link")].find((button) => button.getAttribute("aria-label")?.includes(secondTrack.title));
    sourceLink?.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url", { url: secondTrack.sourceUrl });
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "pause")).toBe(false);
  });

  it("identifies an ordinary active native track without saving identification as tags", async () => {
    identifyPlayingTrackResult = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist", note: "Matched by the active player." };
    mountNative(makeNativeSnapshot({ revision: 48, generation: 9, status: "playing", trackId: savedTrack.id, track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack], currentIndex: 0, paused: false }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find((button) => button.textContent?.trim() === "Identify song")?.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("identify_playing_track", { captureId: expect.stringMatching(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i) });
    expect(mocks.call.mock.calls.some(([command]) => ["update_track", "save_audio_match", "apply_autotag_review"].includes(String(command)))).toBe(false);
  });

  it("identifies active native audio directly from the expanded queue", async () => {
    identifyPlayingTrackResult = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist" };
    mountNative(makeNativeSnapshot({ revision: 49, generation: 9, status: "playing", trackId: savedTrack.id, track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack], currentIndex: 0, paused: false }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();

    const identify = document.querySelector<HTMLButtonElement>('[aria-label="Identify currently playing audio"]');
    expect(identify).toBeTruthy();
    identify!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("identify_playing_track", { captureId: expect.stringMatching(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i) });
    expect(mocks.call.mock.calls.some(([command]) => ["update_track", "save_audio_match", "apply_autotag_review"].includes(String(command)))).toBe(false);
  });

  it("cancels deferred native identification with its capture ID and ignores the late result", async () => {
    holdNativeIdentify = true;
    mountNative(makeNativeSnapshot({ revision: 50, generation: 9, status: "playing", trackId: savedTrack.id, track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack], currentIndex: 0, paused: false }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Identify currently playing audio"]')!.click();await settle();
    const identifyCalls=mocks.call.mock.calls.filter(([command])=>command==="identify_playing_track");
    expect(identifyCalls).toHaveLength(1);
    const captureId=(identifyCalls[0]![1] as {captureId:string}).captureId;
    expect(captureId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
    expect(finishNativeIdentify).toBeDefined();
    document.querySelector<HTMLButtonElement>('[aria-label="Cancel current audio capture"]')!.click();await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="cancel_playing_track_identification")).toEqual([["cancel_playing_track_identification",{captureId}]]);
    expect(mocks.call.mock.calls.some(([command])=>command==="cancel_webview_identification")).toBe(false);
    finishNativeIdentify!({trackId:savedTrack.id,title:"Late Cancelled Native Result",artist:"Stale Artist"});await settle();
    expect(document.querySelector(".banner.notice")?.textContent??"").not.toContain("Late Cancelled Native Result");
    expect(document.querySelector('[aria-label="Identify currently playing audio"]')).toBeTruthy();
    expect(mocks.call.mock.calls.some(([command])=>["update_track","save_audio_match","apply_autotag_review","playback_command"].includes(String(command)))).toBe(false);
  });

  it("identifies WebView audio from the expanded queue and discards a stale seek result", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 82;
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await startWebviewIdentify();
    const audio = document.querySelector("audio");
    expect(audio).toBeTruthy();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();

    let currentTime = 91.25;
    Object.defineProperty(audio!, "currentTime", { configurable: true, get: () => currentTime, set: (value: number) => { currentTime = value; } });
    document.querySelector<HTMLButtonElement>('[aria-label="Identify currently playing audio"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm", expect.objectContaining({
      generation: 82, trackId: savedTrack.id, sessionId: expect.any(String), seekSerial: 0, wavBase64: "captured-wav-base64",
    }));
    expect(finishWebviewIdentify).toBeDefined();

    const seek = await automationAction("identify-queue-stale-seek", "seek", { position: 32 });
    expect(seek.error).toBeNull();
    finishWebviewIdentify!({ trackId: savedTrack.id, title: "Stale Queue Result", artist: "Old Artist" });
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Stale Queue Result");
  });

  it("identifies ordinary WebView playback from its captured source generation and current audio position", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 73;
    webviewIdentifyResult = { trackId: savedTrack.id, title: "Live Performance", artist: "Recognized Artist" };
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const identify = await startWebviewIdentify();
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 91.25 });
    identify.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm", expect.objectContaining({
      generation: 73, trackId: savedTrack.id, sessionId: expect.any(String), seekSerial: 0, wavBase64: "captured-wav-base64",
    }));
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Identified: Recognized Artist — Live Performance");
    expect(mocks.call.mock.calls.some(([command]) => ["update_track", "save_audio_match", "apply_autotag_review"].includes(String(command)))).toBe(false);
  });

  it("identifies progressive WebView samples and stops after the first match", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 73;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const originalCall = mocks.call.getMockImplementation()!;
    let attempts = 0;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "identify_webview_playing_pcm") {
        attempts++;
        return attempts === 1 ? {trackId: savedTrack.id, note: "No match yet"} : {trackId: savedTrack.id, title: "Progressive Match", artist: "Recognized Artist"};
      }
      return originalCall(command, args);
    });
    const sampledSeconds: number[] = [];
    mocks.captureRadioPcm.mockImplementation(async (_audio: HTMLAudioElement, isCurrent: () => boolean, signal: AbortSignal, progress: (seconds: number) => void, options: {onSample: (wav: string, seconds: number) => Promise<boolean>}) => {
      for (const seconds of [3, 6, 12]) {
        expect(isCurrent()).toBe(true);
        expect(signal.aborted).toBe(false);
        sampledSeconds.push(seconds); progress(seconds);
        if (await options.onSample(`captured-${seconds}`, seconds)) break;
      }
      return "captured-6";
    });
    const instance = mount(App, {target: document.body});
    unmountApp = () => {void unmount(instance);};
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(sampledSeconds).toEqual([3, 6]);
    expect(mocks.captureRadioPcm).toHaveBeenCalledTimes(1);
    const calls = mocks.call.mock.calls.filter(([command]) => command === "identify_webview_playing_pcm");
    expect(calls).toHaveLength(2);
    expect(calls.map(([, args]) => args.wavBase64)).toEqual(["captured-3", "captured-6"]);
    const identity = expect.objectContaining({generation: 73, trackId: savedTrack.id, sessionId: expect.any(String), seekSerial: 0});
    for (const [, args] of calls) expect(args).toEqual(identity);
    expect(calls[0]![1].captureId).toBe(calls[1]![1].captureId);
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Recognized Artist — Progressive Match");
  });

  it("discards WebView identification when playback crosses a natural chapter boundary", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 84;
    holdWebviewIdentify = true;
    const chapterTrack: Track = { ...savedTrack, id: "natural-chapter-track", provenance: { sourceKind: "audioSearch" } };
    const chapterCandidate: Candidate = {
      ...youtubeCandidate(chapterTrack),
      chapters: [
        { id: "chapter-a", title: "Opening song", startTime: 0, endTime: 30, confidence: "high" },
        { id: "chapter-b", title: "Next song", startTime: 30, endTime: 60, confidence: "high" },
      ],
    };
    const track: Track = { ...chapterTrack, provenance: { sourceKind: "audioSearch", audioCandidate: chapterCandidate } };
    savedChartTracks = [track];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const identify = await startWebviewIdentify(track);
    const audio = document.querySelector<HTMLAudioElement>("audio")!;
    let currentTime = 12;
    Object.defineProperty(audio, "currentTime", { configurable: true, get: () => currentTime, set: (value: number) => { currentTime = value; } });
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    const firstChapter = (await automationAction("chapter-identify-before", "volume", { volume: 0.8 })).playback;
    expect(firstChapter).toMatchObject({ trackId: track.id, chapterId: "chapter-a", sourceGeneration: 84, seekSerial: 0 });

    identify.click();
    await settle();
    expect(finishWebviewIdentify).toBeDefined();

    currentTime = 35;
    audio.dispatchEvent(new Event("timeupdate"));
    await settle();
    const nextChapter = (await automationAction("chapter-identify-after", "volume", { volume: 0.8 })).playback;
    expect(nextChapter).toMatchObject({
      trackId: firstChapter!.trackId, sessionId: firstChapter!.sessionId, chapterId: "chapter-b",
      sourceGeneration: firstChapter!.sourceGeneration, seekSerial: firstChapter!.seekSerial,
    });
    expect(nextChapter?.position).toBe(35);

    finishWebviewIdentify!({ trackId: track.id, title: "Stale Opening Song Result", artist: "Old Artist" });
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Stale Opening Song Result");
    expect(mocks.call.mock.calls.some(([command]) => ["update_track", "save_audio_match", "apply_autotag_review"].includes(String(command)))).toBe(false);
  });

  it("passes absolute source time when identifying a WebView chapter track", async () => {
    previewMode = "direct";
    const chapterTrack: Track = {
      ...savedTrack, id: "webview-chapter", title: "Song From Concert",
      sourceUrl: "https://youtube.com/watch?v=concert12345",
      playbackRange: { parentTrackId: "concert-set", chapterId: "song-from-concert", sourceUrl: "https://youtube.com/watch?v=concert12345", startTime: 120, endTime: 158, parentTitle: "Concert Set" },
    };
    savedChartTracks = [chapterTrack];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    const identify = await startWebviewIdentify(chapterTrack);
    Object.defineProperty(document.querySelector("audio")!, "currentTime", { configurable: true, value: 137 });
    identify.click();
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm", expect.objectContaining({
      generation: webviewSourceGeneration, trackId: chapterTrack.id, sessionId: expect.any(String), seekSerial: 0, wavBase64: "captured-wav-base64",
    }));
  });

  it("explicit duplicate queue selection starts the new occurrence at zero instead of the old playhead", async () => {
    previewMode="direct";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack,savedTrack],currentIndex:0,repeat:"off"})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(1);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    const starts:number[]=[];vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){starts.push(this.currentTime);return Promise.resolve();});
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    await automationAction("duplicate-first-resume","resume");await settle();
    const first=document.querySelector("audio")!;first.currentTime=30;first.dispatchEvent(new Event("timeupdate"));await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();await settle();
    const rows=document.querySelectorAll<HTMLElement>('.queue-list article');
    expect(rows).toHaveLength(2);rows[1]!.querySelector<HTMLButtonElement>('.track-select')!.click();await settle();
    expect(starts[starts.length - 1]).toBe(0);
    const snapshot=(await automationAction("duplicate-selected-inspect","volume",{volume:0.8})).playback;
    expect(snapshot.currentIndex).toBe(1);expect(snapshot.queue).toHaveLength(2);expect(snapshot.position).toBe(0);
  });

  it("explicit chapter occurrence selection starts at its range start rather than a saved interior position", async () => {
    previewMode="direct";
    const chapter:Track={...savedTrack,id:"repeat-chapter",playbackRange:{parentTrackId:"parent-set",chapterId:"chapter-a",sourceUrl:"https://example.test/set",startTime:10,endTime:25,parentTitle:"Set"}};
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[chapter,chapter],currentIndex:0,repeat:"off"})],["burrata.playback-position.v1",JSON.stringify({trackId:chapter.id,position:20,savedAt:Date.now()})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(1);vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    const starts:number[]=[];vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){starts.push(this.currentTime);return Promise.resolve();});vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();await settle();
    const rows=document.querySelectorAll<HTMLElement>('.queue-list article');expect(rows).toHaveLength(2);rows[1]!.querySelector<HTMLButtonElement>('.track-select')!.click();await settle();
    expect(starts[starts.length - 1]).toBe(10);
  });

  it("restored Resume still starts at its own saved playhead", async () => {
    previewMode="direct";
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack],currentIndex:0,repeat:"off"})],["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position:61.25,savedAt:Date.now()})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(1);vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    const starts:number[]=[];vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(function(this:HTMLMediaElement){starts.push(this.currentTime);return Promise.resolve();});vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('.play-button')!.click();await settle();expect(starts[starts.length - 1]).toBe(61.25);
  });

  it("advances once when a terminal chapter emits both boundary timeupdate and ended", async () => {
    previewMode="direct";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const chapter:Track={...savedTrack,id:"terminal-chapter",playbackRange:{parentTrackId:"parent-set",chapterId:"terminal",sourceUrl:"https://example.test/set",startTime:10,endTime:25,parentTitle:"Set"}};
    const third={...secondTrack,id:"third-after-chapter",title:"Third after chapter"};
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[chapter,secondTrack,third],currentIndex:0,repeat:"off"})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(1);vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(25);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    await automationAction("terminal-chapter-start","resume");await settle();
    const audio=document.querySelector("audio")!;audio.currentTime=25;
    audio.dispatchEvent(new Event("timeupdate"));audio.dispatchEvent(new Event("ended"));await settle();
    const snapshot=(await automationAction("terminal-chapter-after","volume",{volume:0.8})).playback;
    expect(snapshot.currentIndex).toBe(1);expect(snapshot.trackId).toBe(secondTrack.id);
  });

  it("shows a restored playback position as ready rather than unavailable", async () => {
    const local=new Map<string,string>([
      ["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack],currentIndex:0,repeat:"off"})],
      ["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position:61,savedAt:Date.now()})],
    ]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    const status=document.querySelector<HTMLElement>(".player-status")!;
    expect(status.textContent).toContain("Ready");
    expect(status.textContent).not.toContain("Unavailable");
    expect(status.title).toContain("Ready to resume from 1:01");
  });

  it("publishes a queue-free position delta and sends the full duplicate queue after a queue edit", async () => {
    previewMode="direct";
    automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();

    const started=await automationAction("publisher-start","play",{trackIds:[savedTrack.id]});
    expect(started.error).toBeNull();
    const startCompletion=mocks.call.mock.calls.find(([command,args])=>command==="automation_playback_complete" && (args as {requestId?:string}).requestId==="publisher-start");
    const initial=(startCompletion?.[1] as {playback?:Record<string,unknown>}|undefined)?.playback;
    expect(initial?.queue).toEqual([expect.objectContaining({id:savedTrack.id})]);
    expect(initial?.queueEntryIds).toEqual([expect.any(String)]);
    expect(new Set(initial?.queueEntryIds as string[]).size).toBe(1);

    mocks.call.mockClear();
    document.querySelector("audio")!.dispatchEvent(new Event("seeked"));
    await settle();
    const firstPulse=mocks.call.mock.calls.find(([command])=>command==="automation_playback_state");
    expect(firstPulse).toBeDefined();
    expect((firstPulse![1] as {playback:Record<string,unknown>}).playback.queueUpdate).toMatchObject({kind:"unchanged"});
    expect("queue" in (firstPulse![1] as {playback:Record<string,unknown>}).playback).toBe(false);

    automationTrackLookup=[savedTrack];
    await automationAction("publisher-duplicates","queue",{trackIds:[savedTrack.id,savedTrack.id],mode:"append"});
    const completion=mocks.call.mock.calls.find(([command,args])=>command==="automation_playback_complete" && (args as {requestId?:string}).requestId==="publisher-duplicates");
    const completedPlayback=(completion?.[1] as {playback:Record<string,unknown>}).playback;
    expect(completedPlayback.queueUpdate).toMatchObject({kind:"full"});
    expect((completedPlayback.queue as {id:string}[]).map(track=>track.id)).toEqual([savedTrack.id,savedTrack.id,savedTrack.id]);
    const occurrenceIds=completedPlayback.queueEntryIds as string[];
    expect(occurrenceIds).toHaveLength(3);
    expect(new Set(occurrenceIds).size).toBe(3);
    expect(occurrenceIds[0]).toBe((initial?.queueEntryIds as string[])[0]);

    mocks.call.mockClear();
    document.querySelector("audio")!.dispatchEvent(new Event("seeked"));
    await settle();
    const changedPulse=mocks.call.mock.calls.find(([command])=>command==="automation_playback_state");
    expect(changedPulse).toBeDefined();
    const changedPlayback=(changedPulse![1] as {playback:Record<string,unknown>}).playback;
    expect(changedPlayback.queueUpdate).toMatchObject({kind:"unchanged"});
    expect("queue" in changedPlayback).toBe(false);
    expect("queueEntryIds" in changedPlayback).toBe(false);
  });

  it("keeps sending full queues when an older native bridge has no playback-state acknowledgement", async () => {
    previewMode="direct";
    automationPlaybackAck=undefined;
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    await automationAction("legacy-publisher-start","play",{trackIds:[savedTrack.id]});
    mocks.call.mockClear();
    const audio=document.querySelector("audio")!;
    audio.dispatchEvent(new Event("seeked"));
    await settle();
    audio.dispatchEvent(new Event("seeked"));
    await settle();
    const pulses=mocks.call.mock.calls.filter(([command])=>command==="automation_playback_state");
    expect(pulses.length).toBeGreaterThanOrEqual(2);
    for(const [,args] of pulses) {
      const playback=(args as {playback:Record<string,unknown>}).playback;
      expect(playback.queueUpdate).toMatchObject({kind:"full"});
      expect(playback.queue).toEqual([expect.objectContaining({id:savedTrack.id})]);
    }
  });

  it("routes the registered automation identify event through captured PCM and completes a generic result", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 73;
    webviewIdentifyResult = { trackId: savedTrack.id, title: "Live Performance", artist: "Recognized Artist", artworkUrl: "https://images.shazam.com/matched-cover.jpg", note: "Matched" };
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await startWebviewIdentify();
    await new Promise(resolve => setTimeout(resolve, 1050));
    await settle();
    const playbackCall = [...mocks.call.mock.calls].reverse().find(([command]) => command === "automation_playback_state");
    const playback = playbackCall?.[1] && (playbackCall[1] as {playback?:Record<string,unknown>}).playback;
    expect(playback).toBeDefined();
    const handler = eventHandlers.get("automation-identify-playing-audio");
    expect(handler).toBeDefined();
    const captureId = "123e4567-e89b-12d3-a456-426614174000";
    handler!({event:"automation-identify-playing-audio",id:1,payload:{captureId,trackId:savedTrack.id,sessionId:playback!.sessionId,generation:73,seekSerial:0}});
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm", expect.objectContaining({captureId,trackId:savedTrack.id,generation:73,sessionId:playback!.sessionId,seekSerial:0,wavBase64:"captured-wav-base64"}));
    expect(mocks.call).toHaveBeenCalledWith("automation_identify_complete", {captureId,result:{trackId:savedTrack.id,title:"Live Performance",artist:"Recognized Artist",artworkUrl:"https://images.shazam.com/matched-cover.jpg",note:"Matched"},error:null});
    expect(mocks.call.mock.calls.some(([command])=>command==="identify_webview_track" || command==="identify_playing_track")).toBe(false);
  });

  it("uses owned Linux output capture when browser media capture is unavailable", async () => {
    vi.spyOn(navigator,"userAgent","get").mockReturnValue("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15");
    previewMode="direct";webviewSourceGeneration=73;
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};
    await settle();(await startWebviewIdentify()).click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_output",expect.objectContaining({trackId:savedTrack.id,generation:73,seekSerial:0}));
    expect(mocks.captureRadioPcm).not.toHaveBeenCalled();
  });

  it("cancels only the matching automation identify nonce and completes cancellation before a late result", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 73;
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await startWebviewIdentify();
    await new Promise(resolve => setTimeout(resolve, 1050));
    await settle();
    const snapshotCall=[...mocks.call.mock.calls].reverse().find(([command])=>command==="automation_playback_state");
    const snapshot=snapshotCall?.[1] && (snapshotCall[1] as {playback?:Record<string,unknown>}).playback;
    const handler=eventHandlers.get("automation-identify-playing-audio")!;
    const captureId="123e4567-e89b-12d3-a456-426614174002";
    handler({event:"automation-identify-playing-audio",id:3,payload:{captureId,trackId:savedTrack.id,sessionId:snapshot!.sessionId,generation:73,seekSerial:snapshot!.seekSerial}});
    await settle();
    expect(finishWebviewIdentify).toBeDefined();
    const busyId = "123e4567-e89b-12d3-a456-426614174099";
    handler({event:"automation-identify-playing-audio",id:30,payload:{captureId:busyId,trackId:savedTrack.id,sessionId:snapshot!.sessionId,generation:73,seekSerial:snapshot!.seekSerial}});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_identify_complete", {captureId:busyId,result:{},error:"Audio identification is already in progress"});
    eventHandlers.get("automation-identify-cancelled")!({event:"automation-identify-cancelled",id:4,payload:{captureId:"old-capture"}});
    await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("cancel_webview_identification",expect.anything());
    eventHandlers.get("automation-identify-cancelled")!({event:"automation-identify-cancelled",id:5,payload:{captureId}});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_webview_identification",{captureId});
    const completions = () => mocks.call.mock.calls.filter(([command,args])=>command==="automation_identify_complete" && (args as {captureId?:string}).captureId===captureId);
    expect(completions()).toEqual([["automation_identify_complete",{captureId,result:{},error:"Audio identification cancelled"}]]);
    finishWebviewIdentify!({trackId:savedTrack.id,title:"Late Song",artist:"Late Artist"});
    await settle();
    expect(completions()).toHaveLength(1);
    expect(document.querySelector(".banner.notice")?.textContent??"").not.toContain("Late Song");
  });

  it("completes MCP identification promptly when the user cancels capture and accepts the next request", async () => {
    previewMode="direct";webviewSourceGeneration=73;holdWebviewIdentify=true;
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};
    await settle();await startWebviewIdentify();
    await new Promise(resolve=>setTimeout(resolve,1050));await settle();
    const stateCall=[...mocks.call.mock.calls].reverse().find(([command])=>command==="automation_playback_state");
    const state=(stateCall![1] as {playback:Record<string,unknown>}).playback;
    const handler=eventHandlers.get("automation-identify-playing-audio")!;
    const captureId="123e4567-e89b-12d3-a456-426614174003";
    const payload={captureId,trackId:savedTrack.id,sessionId:state.sessionId,generation:73,seekSerial:state.seekSerial};
    handler({event:"automation-identify-playing-audio",id:6,payload});await settle();
    const finishCancelled=finishWebviewIdentify!;
    const cancel=[...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find(button=>button.textContent?.startsWith("Cancel capture"));
    expect(cancel).toBeDefined();cancel!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_webview_identification",{captureId});
    const completions=()=>mocks.call.mock.calls.filter(([command,args])=>command==="automation_identify_complete"&&(args as {captureId?:string}).captureId===captureId);
    // The recognizer remains unresolved: completion must come from cancellation.
    expect(completions()).toEqual([["automation_identify_complete",{captureId,result:{},error:"Audio identification cancelled"}]]);
    expect([...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].some(button=>button.textContent==="Identify song"&&!button.disabled)).toBe(true);
    const nextId="123e4567-e89b-12d3-a456-426614174004";
    handler({event:"automation-identify-playing-audio",id:7,payload:{...payload,captureId:nextId}});await settle();
    expect(mocks.call).toHaveBeenCalledWith("identify_webview_playing_pcm",expect.objectContaining({captureId:nextId}));
    finishCancelled({trackId:savedTrack.id,title:"Discarded Late Result"});await settle();
    expect(completions()).toHaveLength(1);
    expect([...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].some(button=>button.textContent?.startsWith("Cancel capture"))).toBe(true);
    finishWebviewIdentify!({trackId:savedTrack.id,title:"Next Result"});await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_identify_complete",expect.objectContaining({captureId:nextId,error:null,result:expect.objectContaining({title:"Next Result"})}));
    expect(document.querySelector(".banner.notice")?.textContent??"").not.toContain("Discarded Late Result");
  });

  it("rejects a stale automation identify event before PCM capture", async () => {
    previewMode = "direct";
    webviewSourceGeneration = 73;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await startWebviewIdentify();
    await new Promise(resolve => setTimeout(resolve, 1050));
    await settle();
    const playbackCall=[...mocks.call.mock.calls].reverse().find(([command])=>command==="automation_playback_state");
    const playback=playbackCall?.[1] && (playbackCall[1] as {playback?:Record<string,unknown>}).playback;
    const handler=eventHandlers.get("automation-identify-playing-audio");
    expect(handler).toBeDefined();
    const captureId="123e4567-e89b-12d3-a456-426614174001";
    handler!({event:"automation-identify-playing-audio",id:2,payload:{captureId,trackId:savedTrack.id,sessionId:String(playback?.sessionId??"old-session"),generation:webviewSourceGeneration,seekSerial:Number(playback?.seekSerial??0)+1}});
    await settle();
    expect(mocks.captureRadioPcm).not.toHaveBeenCalled();
    expect(mocks.call).toHaveBeenCalledWith("automation_identify_complete", expect.objectContaining({captureId,error:"Playback changed before identification began"}));
  });

  it("cancels an in-flight captured-audio recognition by nonce and ignores its late result", async () => {
    previewMode = "direct";
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(finishWebviewIdentify).toBeDefined();
    const identifyCall=mocks.call.mock.calls.find(([command])=>command==="identify_webview_playing_pcm");
    const captureId=(identifyCall?.[1] as {captureId?:string}|undefined)?.captureId;
    expect(captureId).toEqual(expect.any(String));
    [...document.querySelectorAll<HTMLButtonElement>(".playing-info button")].find((button)=>button.textContent?.startsWith("Cancel capture"))!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_webview_identification",{captureId});
    finishWebviewIdentify!({trackId:savedTrack.id,title:"Cancelled Song",artist:"Late Artist"});
    await settle();
    expect(document.querySelector(".banner.notice")?.textContent??"").not.toContain("Cancelled Song");
  });

  it("discards a WebView identification result after seeking", async () => {
    previewMode = "direct";
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(finishWebviewIdentify).toBeDefined();

    const seek = await automationAction("identify-stale-seek", "seek", { position: 32 });
    expect(seek.error).toBeNull();
    finishWebviewIdentify!({ trackId: savedTrack.id, title: "Stale Song", artist: "Old Artist" });
    await settle();

    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Stale Song");
  });

  it("discards a WebView identification result after a new playback session starts", async () => {
    previewMode = "direct";
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(finishWebviewIdentify).toBeDefined();

    const restarted = await automationAction("identify-new-session", "play", { trackIds: [savedTrack.id] });
    expect(restarted.error).toBeNull();
    finishWebviewIdentify!({ trackId: savedTrack.id, title: "Old Session Song", artist: "Old Artist" });
    await settle();

    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Old Session Song");
  });

  it("discards a WebView identification result after its source generation changes", async () => {
    previewMode = "direct";
    holdWebviewIdentify = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(finishWebviewIdentify).toBeDefined();

    webviewSourceGeneration = 74;
    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_playback_track", expect.objectContaining({ track: savedTrack }));
    finishWebviewIdentify!({ trackId: savedTrack.id, title: "Old Source Song", artist: "Old Artist" });
    await settle();

    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Old Source Song");
  });

  it("discards a WebView identification result after the active track changes", async () => {
    previewMode = "direct";
    holdWebviewIdentify = true;
    savedChartTracks = [savedTrack, secondTrack];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const identify = await startWebviewIdentify();
    identify.click(); await settle();
    expect(finishWebviewIdentify).toBeDefined();

    const changed = await automationAction("identify-new-track", "play", { trackIds: [secondTrack.id] });
    expect(changed.error).toBeNull();
    finishWebviewIdentify!({ trackId: savedTrack.id, title: "Wrong Track Result", artist: "Old Artist" });
    await settle();

    expect(document.querySelector(".banner.notice")?.textContent ?? "").not.toContain("Wrong Track Result");
  });

  async function mountInitialQueueFailureFixture(reason: string, options: {single?:boolean;hold?:boolean;allFail?:boolean;duplicate?:boolean;holdAfterSkip?:boolean;count?:number;failFirst?:number;repeat?:"all"|"one";acknowledgeHistory?:boolean} = {}) {
    previewMode="fallback";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const rows=options.allFail||options.failFirst?Array.from({length:options.count??5},(_,i)=>({...savedTrack,id:options.duplicate?savedTrack.id:`initial-failed-${i}`,title:options.duplicate?savedTrack.title:`Initial failed ${i}`})):[savedTrack,options.duplicate?{...savedTrack}:secondTrack];
    savedChartTracks=rows;
    const original=mocks.call.getMockImplementation()!;
    const resolved:string[]=[];let release:((error:Error)=>void)|undefined;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(options.acknowledgeHistory && ['automation_playback_state','automation_playback_complete'].includes(command)) return {...automationPlaybackAck,failureHistoryRevision:(args?.playback as {failureHistoryRevision?:string})?.failureHistoryRevision};
      if(command==='resolve_playback_track'){
        const track=args?.track as Track;resolved.push(track.id);
        if((options.hold&&resolved.length===1)||(options.holdAfterSkip&&resolved.length===2))return new Promise((_resolve,reject)=>{release=reject;});
        if(resolved.length===1||options.allFail||resolved.length<=(options.failFirst??0))throw new Error(reason);
      }
      return original(command,args);
    });
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    if(options.repeat)await automationAction('finite-scan-repeat','repeat',{mode:options.repeat});
    const requestId='initial-unavailable-opener';
    eventHandlers.get('automation-playback')!({event:'automation-playback',id:1,payload:{requestId,action:'play',trackIds:(options.single?rows.slice(0,1):rows).map(track=>track.id)}});await settle();
    return {rows,resolved,requestId,release:()=>release,completion:()=>mocks.call.mock.calls.find(([command,args])=>command==='automation_playback_complete'&&(args as {requestId?:string}).requestId===requestId)?.[1] as {error:string|null;playback:{currentIndex:number;trackId:string;paused:boolean;position:number;queue:Track[];queueEntryIds:string[];diagnostics:{recentTrackFailures:import("./lib/types").PlaybackTrackFailure[]}}}|undefined};
  }
  it("preserves rapid terminal failures with exact queue occurrence ownership in the completion snapshot",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{allFail:true,count:5});
    const snapshot=fixture.completion()!.playback;
    expect(snapshot.diagnostics.recentTrackFailures.map(item=>item.trackId)).toEqual(fixture.rows.map(item=>item.id));
    expect(snapshot.diagnostics.recentTrackFailures.map(item=>item.entryId)).toEqual(snapshot.queueEntryIds);
    expect(snapshot.diagnostics.recentTrackFailures.map(item=>item.outcome)).toEqual(['skipped','skipped','skipped','skipped','queue_exhausted']);
    expect(new Set(snapshot.diagnostics.recentTrackFailures.map(item=>item.generation)).size).toBe(5);
    expect(snapshot.diagnostics.recentTrackFailures.every(item=>item.operationId===fixture.requestId&&item.category==='no_playable_match')).toBe(true);
    const cleared=await automationAction('history-new-queue','queue',{trackIds:[fixture.rows[0]!.id],mode:'replace'});
    expect(cleared.playback.diagnostics).toEqual({recentTrackFailures:[]});
  });
  it("failure history distinguishes duplicate occurrences and later explicit retry attempts",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{allFail:true,count:2,duplicate:true});
    const first=fixture.completion()!.playback;
    expect(first.diagnostics.recentTrackFailures.map(item=>item.trackId)).toEqual([savedTrack.id,savedTrack.id]);
    expect(first.diagnostics.recentTrackFailures.map(item=>item.entryId)).toEqual(first.queueEntryIds);
    expect(new Set(first.queueEntryIds).size).toBe(2);
    document.querySelector<HTMLButtonElement>('[aria-label="Retry playback"]')!.click();await settle();
    const retried=await automationAction('history-explicit-retry-inspect','volume',{volume:0.8});
    const history=(retried.playback.diagnostics as typeof first.diagnostics).recentTrackFailures;
    expect(history).toHaveLength(3);
    expect(history.slice(-2).map(item=>item.entryId)).toEqual([first.queueEntryIds[1],first.queueEntryIds[1]]);
    expect(history[2]!.operationId).not.toBe(fixture.requestId);
    expect(new Set(history.map(item=>item.generation)).size).toBe(3);
  });
  it("failure history retains the most recent 64 actual terminal occurrences",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{allFail:true,count:67});
    const snapshot=fixture.completion()!.playback;
    expect(snapshot.diagnostics.recentTrackFailures).toHaveLength(64);
    expect(snapshot.diagnostics.recentTrackFailures.map(item=>item.entryId)).toEqual(snapshot.queueEntryIds.slice(-64));
    expect(snapshot.diagnostics.recentTrackFailures[63]!.outcome).toBe('queue_exhausted');
  });
  it("publishes constant position pulses after a 64-record failure history is acknowledged",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{failFirst:66,count:67,acknowledgeHistory:true});
    expect(fixture.completion()!.playback.diagnostics.recentTrackFailures).toHaveLength(64);
    mocks.call.mockClear();
    document.querySelector('audio')!.dispatchEvent(new Event('seeked'));await settle();
    const pulse=(lastCallOf('automation_playback_state')![1] as {playback:Record<string,unknown>}).playback;
    expect(pulse.failureHistoryUpdate).toMatchObject({kind:'unchanged'});
    expect(pulse.diagnostics).toEqual({});expect(pulse).not.toHaveProperty('queue');
    expect(JSON.stringify(pulse).length).toBeLessThan(5000);
    const oldRevision=pulse.failureHistoryRevision;
    const reset=await automationAction('history-source-owner-reset','queue',{trackIds:[fixture.rows[0]!.id],mode:'replace'});
    expect(reset.playback.failureHistoryUpdate).toEqual({kind:'full'});
    expect(reset.playback.failureHistoryRevision).not.toBe(oldRevision);
    expect(reset.playback.diagnostics).toEqual({recentTrackFailures:[]});
  });
  it("failure history survives position/index pulses and keeps independent retry ownership",async()=>{
    const fixture=await mountInitialQueueFailureFixture('Video has been deleted',{duplicate:true});
    const first=fixture.completion()!.playback.diagnostics.recentTrackFailures;
    expect(first).toHaveLength(1);
    const previous=await automationAction('history-retry-occurrence','previous');
    expect((previous.playback.diagnostics as {recentTrackFailures:typeof first}).recentTrackFailures).toEqual(first);
    expect(previous.playback.currentIndex).toBe(0);
  });
  it("failure history excludes cancelled and stale queue resolutions",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{hold:true});
    eventHandlers.get('automation-playback-cancelled')!({event:'automation-playback-cancelled',id:1,payload:{requestId:fixture.requestId}});
    fixture.release()!(new Error('No playable audio match was found'));await settle();await settle();
    expect(fixture.completion()!.playback.diagnostics.recentTrackFailures).toEqual([]);
    const replacement=await automationAction('history-after-cancel','queue',{trackIds:[secondTrack.id],mode:'replace'});
    expect(replacement.playback.diagnostics).toEqual({recentTrackFailures:[]});
  });
  it("initial multi-track play skips an exhausted opener to an exact supplied occurrence",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found for this recording');
    expect(fixture.resolved).toEqual([savedTrack.id,secondTrack.id]);
    const result=fixture.completion()!;expect(result.error).toBeNull();expect(result.playback.currentIndex).toBe(1);expect(result.playback.trackId).toBe(secondTrack.id);expect(result.playback.paused).toBe(false);expect(result.playback.position).toBe(0);
    expect(result.playback.queue.map(track=>track.id)).toEqual([savedTrack.id,secondTrack.id]);
    expect(document.querySelector('.banner.notice')?.textContent).toContain('No playable audio match');
    expect(mocks.call.mock.calls.some(([command])=>command==='get_song_radio')).toBe(false);
  });
  it("initial queue play accepts the exact later duplicate occurrence without deduplication",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{duplicate:true});
    expect(fixture.resolved).toEqual([savedTrack.id,savedTrack.id]);
    const result=fixture.completion()!;expect(result.error).toBeNull();expect(result.playback.currentIndex).toBe(1);expect(result.playback.paused).toBe(false);
    expect(result.playback.queue.map(track=>track.id)).toEqual([savedTrack.id,savedTrack.id]);
    const state=JSON.parse(localStorage.getItem('burrata.playback-queue.v1')!);expect(state.entryIds).toHaveLength(2);expect(new Set(state.entryIds).size).toBe(2);
  });
  it("cancellation ownership survives advancement to a held second occurrence",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{holdAfterSkip:true});
    expect(fixture.resolved).toEqual([savedTrack.id,secondTrack.id]);
    eventHandlers.get('automation-playback-cancelled')!({event:'automation-playback-cancelled',id:1,payload:{requestId:fixture.requestId}});
    fixture.release()!(new Error('No playable audio match was found'));await settle();await settle();
    expect(fixture.completion()!.error).toContain('cancelled or superseded');expect(fixture.completion()!.playback.currentIndex).toBe(1);expect(fixture.completion()!.playback.paused).toBe(true);expect(fixture.completion()!.playback.diagnostics.recentTrackFailures.map(item=>item.entryId)).toEqual([fixture.completion()!.playback.queueEntryIds[0]]);
  });
  it("keeps the complete normal no-match action for a selected single-track failure",async()=>{
    const reason = "No playable audio match was found for Spotify — Creep (Acoustic Version, 2018 Remastered Edition). No available source matched the requested recording and version. Open the catalog source or choose another source";
    expect(reason.length).toBeGreaterThan(180);
    const fixture=await mountInitialQueueFailureFixture(reason,{single:true});
    expect(fixture.resolved).toEqual([savedTrack.id]);expect(fixture.completion()!.error).not.toBeNull();expect(fixture.completion()!.playback.currentIndex).toBe(0);
    const completeCopy = "No playable audio match was found for the requested recording and version. Open the catalog source or choose another source.";
    expect(document.querySelector(".player-error")?.textContent).toContain(completeCopy);
    expect(fixture.completion()!.playback.trackId).toBe(savedTrack.id);
  });
  it.each(['HTTP 429 temporarily unavailable','Network connection unavailable','Playback request cancelled'])("initial queue global failure stays on the opener: %s",async(reason)=>{
    const fixture=await mountInitialQueueFailureFixture(reason);expect(fixture.resolved).toEqual([savedTrack.id]);expect(fixture.completion()!.error).not.toBeNull();expect(fixture.completion()!.playback.currentIndex).toBe(0);expect(fixture.completion()!.playback.diagnostics.recentTrackFailures).toEqual([]);
  });
  it("cancelling a held initial opener prevents a late queue skip or success",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{hold:true});
    eventHandlers.get('automation-playback-cancelled')!({event:'automation-playback-cancelled',id:1,payload:{requestId:fixture.requestId}});
    fixture.release()!(new Error('No playable audio match was found'));await settle();await settle();
    expect(fixture.resolved).toEqual([savedTrack.id]);expect(fixture.completion()?.error).not.toBeNull();
  });
  it("a newer queue cannot satisfy a held initial play or receive its late failure skip",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{hold:true});
    await automationAction('replace-during-initial-start','queue',{trackIds:[secondTrack.id],mode:'replace'});
    fixture.release()!(new Error('No playable audio match was found'));await settle();await settle();
    expect(fixture.resolved).toEqual([savedTrack.id]);expect(fixture.completion()!.error).not.toBeNull();expect(fixture.completion()!.playback.queue.map(track=>track.id)).toEqual([secondTrack.id]);expect(fixture.completion()!.playback.diagnostics.recentTrackFailures).toEqual([]);
  });
  it("a newer play request cannot satisfy an older held initial queue request",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{hold:true});
    const newer=await automationAction('newer-play-owner','play',{trackIds:[secondTrack.id]});expect(newer.error).toBeNull();
    fixture.release()!(new Error('No playable audio match was found'));await settle();await settle();
    expect(fixture.completion()!.error).toContain('cancelled or superseded');expect(fixture.completion()!.playback.trackId).toBe(secondTrack.id);
  });
  it("initial source failures reaching queue end do not start enabled radio",async()=>{
    localStorage.setItem('burrata.queue-end-radio.v1','true');
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{allFail:true,count:2});
    expect(fixture.resolved).toEqual(fixture.rows.map(track=>track.id));expect(fixture.completion()!.error).not.toBeNull();expect(fixture.completion()!.playback.currentIndex).toBe(1);expect(fixture.completion()!.playback.paused).toBe(true);expect(fixture.completion()!.playback.queue).toHaveLength(2);
    expect(mocks.call.mock.calls.some(([command])=>command==='get_song_radio')).toBe(false);
  });
  it("finite failure scan reaches a playable fifth occurrence after four unavailable openers",async()=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{failFirst:4,count:5});
    expect(fixture.resolved).toEqual(fixture.rows.map(track=>track.id));
    expect(fixture.completion()!.error).toBeNull();expect(fixture.completion()!.playback.currentIndex).toBe(4);expect(fixture.completion()!.playback.paused).toBe(false);
    expect(mocks.call.mock.calls.some(([command])=>command==='get_song_radio')).toBe(false);
  });
  it.each(["off","all","one"] as const)("initial exhausted finite queue visits every occurrence once and never starts radio under %s repeat",async(repeat)=>{
    const fixture=await mountInitialQueueFailureFixture('No playable audio match was found',{allFail:true,...(repeat==='off'?{}:{repeat})});
    expect(fixture.resolved).toEqual(fixture.rows.map(track=>track.id));expect(fixture.completion()!.error).not.toBeNull();expect(fixture.completion()!.playback.currentIndex).toBe(4);expect(fixture.completion()!.playback.paused).toBe(true);
    expect(mocks.call.mock.calls.some(([command])=>command==='get_song_radio')).toBe(false);
  });

  async function mountQueueFailureFixture(reason: string, mode: "normal"|"repeat-one"|"hold"|"refresh"|"radio"|"end" = "normal", middleOverrides: Partial<Track> = {}) {
    previewMode="fallback";
    const middle={...secondTrack,id:"album-unavailable-middle",title:"Unavailable middle album recording",...middleOverrides};
    if(mode==="radio") {
      Object.assign(middle,{provider:"internet-radio",providerId:"fixture-radio"});
      radioStationState=[{id:"fixture-radio",name:"Fixture Radio",provider:"custom",sourceUrl:"https://radio.example",streamUrl:"https://radio.example/live",isCustom:true}];
    }
    const last={...savedTrack,id:"album-success-last",title:"Successful last album recording"};
    savedChartTracks=mode==="end"?[savedTrack,middle]:[savedTrack,middle,last];
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    if(mode==="radio") play.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("Radio unavailable"));
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const original=mocks.call.getMockImplementation()!;
    let failHeld: ((error:Error)=>void)|undefined;
    const heldRefreshFailures: ((error:Error)=>void)[]=[];
    let refreshCalls=0;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="resolve_playback_track" && (args?.track as Track|undefined)?.id===middle.id) {
        if(mode==="repeat-one") await automationAction("failure-repeat-one","repeat",{mode:"one"});
        if(mode==="hold") return new Promise((_resolve,reject)=>{failHeld=reject;});
        if(mode==="refresh") { refreshCalls++; return new Promise((_resolve,reject)=>{heldRefreshFailures.push(reject);}); }
        throw new Error(reason);
      }
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();await settle();
    const audio=document.querySelector<HTMLAudioElement>("audio")!;
    audio.dispatchEvent(new Event("ended"));await settle();await settle();
    return {middle,last,failHeld:()=>failHeld,failRefresh:(index:number)=>heldRefreshFailures[index],refreshCalls:()=>refreshCalls,resolved:()=>mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track").map(([,args])=>(args.track as Track).id)};
  }
  it("clears a playback-skip notice when a manual play replaces its queue owner", async () => {
    await mountQueueFailureFixture("No playable audio match was found for the skipped recording");
    expect(document.querySelector(".banner.notice")?.textContent).toContain("Skipped Unavailable middle album recording");
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();
    await settle();
    expect(document.querySelector(".banner.notice")).toBeNull();
  });

  it("WebView queue audit advances an unavailable middle album occurrence to successful next playback",async()=>{
    const fixture=await mountQueueFailureFixture("No playable audio match was found for Test — Missing recording https://private.invalid/audio?token=secret-token authorization=hidden-value");
    const snapshot=(await automationAction("queue-failure-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id,fixture.last.id]);
    expect(snapshot.currentIndex).toBe(2);expect(snapshot.trackId).toBe(fixture.last.id);expect(snapshot.paused).toBe(false);
    expect((snapshot.queue as Track[]).map(track=>track.id)).toEqual([savedTrack.id,fixture.middle.id,fixture.last.id]);
    const history=(snapshot.diagnostics as {recentTrackFailures:import("./lib/types").PlaybackTrackFailure[]}).recentTrackFailures;
    expect(history).toHaveLength(1);expect(history[0]?.entryId).toBe((snapshot.queueEntryIds as string[])[1]);
    expect(history[0]?.category).toBe("no_playable_match");
    expect(JSON.stringify(history)).not.toMatch(/private\.invalid|secret-token|hidden-value/);
    const notice=document.querySelector(".banner.notice")?.textContent??"";
    expect(notice).toContain("Skipped Unavailable middle album recording");expect(notice).toContain("No playable audio match");
    expect(notice).toContain("Choose another source");expect(notice).not.toContain("private.invalid");expect(notice).not.toContain("secret-token");expect(notice).not.toContain("hidden-value");
  });
  it("ordinary notice after persistent failure expires and navigation clears persistent state", async () => {
    mountNative(makeNativeSnapshot());await settle();vi.useFakeTimers();
    try {
      const failure={trackId:savedTrack.id,title:savedTrack.title,artist:savedTrack.artist,reason:"No playable audio match"};
      const failed=makeNativeSnapshot({revision:30,diagnostics:{lastSourceFailure:failure,lastSkippedTrackFailure:null}});
      emitNative(failed);await settle();
      await vi.advanceTimersByTimeAsync(6000);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Could not play");
      const warning=eventHandlers.get("provider-warning");expect(warning).toBeTypeOf("function");
      warning!({event:"provider-warning",id:1,payload:"Ordinary update completed."});await tick();
      await vi.advanceTimersByTimeAsync(5499);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Ordinary update completed.");
      await vi.advanceTimersByTimeAsync(1);await tick();expect(document.querySelector(".banner.notice")).toBeNull();
      emitNative({...failed,revision:31,diagnostics:{lastSourceFailure:{...failure,title:"Fresh failure"},lastSkippedTrackFailure:null}});await settle();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Fresh failure");
      const navigation=eventHandlers.get("automation-ui-navigation");expect(navigation).toBeTypeOf("function");navigation!({event:"automation-ui-navigation",id:32,payload:{requestId:"notice-navigation-clear",target:{kind:"view",view:"favorites"}}});await settle();await settle();
      expect(document.querySelector(".banner.notice")).toBeNull();
    } finally {vi.useRealTimers();}
  });
  it("persistent playback notice survives the progress window and dismissal", async () => {
    vi.useFakeTimers();
    try {
      await mountQueueFailureFixture("No playable audio match was found", "normal", {
        id:"fixture:sasa-exact", title:"Sasa", artist:"Timmy Regisford", version:"Main Version", durationSeconds:281,
      });
      await vi.advanceTimersByTimeAsync(6500); await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Skipped Sasa:");
      await chooseTrackAction("Add to queue", savedTrack.title);await settle();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("added to queue");
      await vi.advanceTimersByTimeAsync(5499);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("added to queue");
      await vi.advanceTimersByTimeAsync(1);await tick();expect(document.querySelector(".banner.notice")).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it("keeps the Sasa skip explanation complete with one action after successful advancement", async () => {
    const reason = "No playable audio match was found for Timmy Regisford — Sasa (Main Version). No available source matched the requested recording and version. Open the catalog source or choose another source";
    expect(reason.length).toBeGreaterThan(180); // reproduces the actual sanitizer boundary
    const fixture = await mountQueueFailureFixture(reason, "normal", {
      id: "fixture:sasa-exact", title: "Sasa", artist: "Timmy Regisford", version: "Main Version", durationSeconds: 281,
    });
    const snapshot = (await automationAction("sasa-complete-skip-notice", "volume", { volume: 0.8 })).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id, fixture.middle.id, fixture.last.id]);
    expect(snapshot.currentIndex).toBe(2);
    expect(snapshot.trackId).toBe(fixture.last.id);
    expect(snapshot.paused).toBe(false);
    const notice = document.querySelector(".banner.notice")?.textContent ?? "";
    expect(notice).toContain("Skipped Sasa: No playable audio match was found for the requested recording and version (Main Version). Choose another source from the track actions.");
    expect(notice.match(/choose another source/gi)).toHaveLength(1);
    expect(notice).not.toContain("choose anot Choose");
  });
  it("shows the native missing-duration identity explanation without provider metadata", async () => {
    const explanation = "Saved recording duration is missing; no source provided enough verified identity evidence for automatic playback.";
    const title = "Unavailable duration track";
    const artist = "Fixture Artist";
    const reason = `No playable audio match was found for ${artist} — ${title}. ${explanation} Open the catalog source or choose another source`;
    expect(reason.length).toBeGreaterThan(180);
    await mountQueueFailureFixture(reason, "normal", {
      id: "fixture:missing-duration",
      title,
      artist,
      sourceUrl: "https://private.invalid/audio?token=fixture-secret",
    });

    const notice = document.querySelector(".banner.notice")?.textContent ?? "";
    expect(notice).toContain(`Skipped ${title}: ${explanation} Choose another source from the track actions.`);
    expect(notice).not.toContain("private.invalid");
    expect(notice).not.toContain("fixture-secret");
    expect(notice.match(/choose another source/gi)).toHaveLength(1);
  });
  it("does not treat a failure phrase embedded in artist metadata as the missing-duration reason", async () => {
    const explanation = "Saved recording duration is missing; no source provided enough verified identity evidence for automatic playback.";
    const artist = `Fixture ${explanation}`;
    const title = "Metadata phrase track";
    const reason = `No playable audio match was found for ${artist} — ${title}. No available source matched the requested recording and version. Open the catalog source or choose another source`;
    await mountQueueFailureFixture(reason, "normal", {
      id: "fixture:metadata-phrase",
      title,
      artist,
    });

    const notice = document.querySelector(".banner.notice")?.textContent ?? "";
    expect(notice).toContain(`Skipped ${title}: No playable audio match was found for the requested recording and version`);
    expect(notice).not.toContain("no source provided enough verified identity evidence");
  });
  it("sanitizes version metadata in the complete unavailable skip explanation", async () => {
    await mountQueueFailureFixture("No playable audio match was found for Timmy Regisford — Sasa (Main Version). https://private.invalid/?token=secret authorization=hidden", "normal", {
      title: "Sasa", version: "Main Version https://private.invalid/?token=secret authorization=hidden",
    });
    const notice = document.querySelector(".banner.notice")?.textContent ?? "";
    expect(notice).toContain("Main Version");
    expect(notice).toContain("Choose another source from the track actions.");
    expect(notice.match(/choose another source/gi)).toHaveLength(1);
    expect(notice).not.toContain("private.invalid");
    expect(notice).not.toContain("secret");
    expect(notice).not.toContain("hidden");
  });
  it("WebView queue audit reports actual early end without claiming the skip cap",async()=>{
    const fixture=await mountQueueFailureFixture("Video has been deleted","end");
    const snapshot=(await automationAction("queue-end-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id]);expect(snapshot.currentIndex).toBe(1);expect(snapshot.paused).toBe(true);
    expect(snapshot.playerMessage).toContain("reached the end of the queue (1 unavailable occurrences checked)");
    expect(snapshot.playerMessage).toContain("Video has been deleted");expect(snapshot.playerMessage).not.toContain("skipping 3");
  });
  it.each(["Network connection timed out","HTTP 403 forbidden","HTTP 404 unavailable","HTTP 429 too many requests"])("WebView queue audit stops retryable failures on the current occurrence: %s",async(reason)=>{
    const fixture=await mountQueueFailureFixture(reason);
    const snapshot=(await automationAction("queue-network-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id]);
    expect(snapshot.currentIndex).toBe(1);expect(snapshot.trackId).toBe(fixture.middle.id);expect(snapshot.paused).toBe(true);
    expect((snapshot.diagnostics as {recentTrackFailures:unknown[]}).recentTrackFailures).toEqual([]);
  });
  it.each([
    ["No audio results were returned by the available providers. Open the catalog source or choose another source.", "no_audio_results"],
    ["Audio results were found, but none could be verified as the requested recording and version. Open the catalog source or choose another source.", "recording_identity_unverified"],
    ["Audio results were found, but the saved recording has no duration and no source provided enough verified identity evidence. Open the catalog source or choose another source.", "recording_identity_unverified"],
  ] as const)("WebView queue audit preserves the full fixed resolver failure and advances: %s",async(reason,category)=>{
    const fixture=await mountQueueFailureFixture(reason);
    const snapshot=(await automationAction("fixed-resolver-failure-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id,fixture.last.id]);
    expect(snapshot.currentIndex).toBe(2);expect(snapshot.trackId).toBe(fixture.last.id);expect(snapshot.paused).toBe(false);
    const failures=(snapshot.diagnostics as {recentTrackFailures:import("./lib/types").PlaybackTrackFailure[]}).recentTrackFailures;
    expect(failures).toHaveLength(1);expect(failures[0]?.category).toBe(category);expect(failures[0]?.reason).toBe(reason);
    const notice=document.querySelector(".banner.notice")?.textContent??"";
    expect(notice.startsWith(`Skipped Unavailable middle album recording: ${reason}`)).toBe(true);
  });
  it("WebView queue audit bypasses repeat-one only for exhausted unavailable entries",async()=>{
    const fixture=await mountQueueFailureFixture("The resolved audio source identity does not match this recording","repeat-one");
    const snapshot=(await automationAction("queue-repeat-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id,fixture.last.id]);
    expect(snapshot.repeat).toBe("one");expect(snapshot.currentIndex).toBe(2);expect(snapshot.paused).toBe(false);
  });
  it("WebView queue audit excludes a failed radio station from recording auto-skip",async()=>{
    const fixture=await mountQueueFailureFixture("unused","radio");
    const snapshot=(await automationAction("queue-radio-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id]);expect(snapshot.currentIndex).toBe(1);expect(snapshot.paused).toBe(true);
    expect(snapshot.playerMessage).toContain("Radio could not play");
  });
  it("WebView queue audit bounds consecutive unavailable skips without wrapping into radio",async()=>{
    previewMode="fallback";savedChartTracks=[savedTrack,...Array.from({length:5},(_,i)=>({...secondTrack,id:`failed-${i}`,title:`Unavailable ${i}`}))];
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="resolve_playback_track" && (args?.track as Track|undefined)?.id!==savedTrack.id) throw new Error("Video has been deleted");
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();await settle();
    document.querySelector<HTMLAudioElement>("audio")!.dispatchEvent(new Event("ended"));
    for(let i=0;i<5;i++)await settle();
    const snapshot=(await automationAction("queue-cap-inspect","volume",{volume:0.8})).playback;
    expect(snapshot.currentIndex).toBe(5);expect(snapshot.paused).toBe(true);
    expect(mocks.call.mock.calls.filter(([command])=>command==="resolve_playback_track").map(([,args])=>(args.track as Track).id)).toEqual([savedTrack.id,"failed-0","failed-1","failed-2","failed-3","failed-4"]);
    expect(snapshot.playerMessage).toContain("reached the end of the queue");
    expect(mocks.call.mock.calls.some(([command])=>command==="get_song_radio")).toBe(false);
  });
  it("retries a queue occurrence once when library metadata changes during its pending resolution",async()=>{
    const fixture=await mountQueueFailureFixture("No playable audio match was found","refresh");
    expect(fixture.refreshCalls()).toBe(1);

    const firstRefresh={...fixture.middle,title:"Updated during pending resolution",artist:"Updated artist",localPath:"/library/updated-one.mp3",localAvailability:"available" as const};
    savedChartTracks=[savedTrack,firstRefresh,fixture.last];
    eventHandlers.get("autotag-status")!({event:"autotag-status",id:1,payload:{assetId:"asset-refresh",status:"applied"}});
    await settle();
    fixture.failRefresh(0)!(new Error("No playable audio match was found"));await settle();
    expect(fixture.refreshCalls()).toBe(2);
    const retried=(mocks.call.mock.calls.filter(([command,args])=>command==="resolve_playback_track"&&(args?.track as Track)?.id===fixture.middle.id).slice(-1)[0]?.[1] as {track:Track}).track;
    expect(retried).toMatchObject({title:firstRefresh.title,artist:firstRefresh.artist,localPath:firstRefresh.localPath});

    // A second row refresh during that retry does not start an unbounded chain,
    // and the retry's old-source failure cannot skip the now-changed occurrence.
    const secondRefresh={...firstRefresh,title:"Updated a second time",localPath:"/library/updated-two.mp3"};
    savedChartTracks=[savedTrack,secondRefresh,fixture.last];
    eventHandlers.get("autotag-status")!({event:"autotag-status",id:2,payload:{assetId:"asset-refresh",status:"applied"}});
    await settle();
    fixture.failRefresh(1)!(new Error("No playable audio match was found"));await settle();await settle();
    expect(fixture.refreshCalls()).toBe(2);
    const snapshot=(await automationAction("refresh-race-inspect","volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id,fixture.middle.id]);
    expect(snapshot.currentIndex).toBe(1);expect(snapshot.trackId).toBe(fixture.middle.id);expect(snapshot.paused).toBe(true);
    expect((snapshot.queue as {title:string;artist:string}[])[1]).toMatchObject({title:secondRefresh.title,artist:secondRefresh.artist});
    expect((snapshot.diagnostics as {recentTrackFailures:unknown[]}).recentTrackFailures).toEqual([]);
  });
  it.each(["pause","stop"])("WebView queue audit respects %s while an unavailable middle resolution is pending",async(action)=>{
    const fixture=await mountQueueFailureFixture("unused","hold");
    expect(fixture.failHeld()).toBeDefined();
    await automationAction(`queue-${action}`,action);
    fixture.failHeld()!(new Error("Video unavailable"));await settle();await settle();
    const snapshot=(await automationAction(`queue-${action}-inspect`,"volume",{volume:0.8})).playback;
    expect(fixture.resolved()).toEqual([savedTrack.id,fixture.middle.id]);
    expect(snapshot.currentIndex).toBe(1);expect(snapshot.paused).toBe(true);
  });

  it("prefetches queue radio only when enabled and continues only at a natural end", async () => {
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const playing=makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:seed.id,track:seed,candidate:youtubeCandidate(seed),queue:[seed],currentIndex:0,paused:false,duration:240});
    const recommendation={...secondTrack,providerId:"2",sourceUrl:"https://example.test/track/2"};
    songRadioTracks=[seed,recommendation,recommendation];
    mountNative(playing);
    await settle();
    expect(mocks.call.mock.calls.some(([command])=>command==="get_song_radio")).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_song_radio",expect.objectContaining({persist:false}));
    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args?.command as {action?:string})?.action==="play")).toBe(false);
    emitNative({...playing,revision:51,status:"ended",naturalQueueEnd:true,paused:true});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"play",index:1,tracks:[seed,recommendation]})});
  });

  it("does not start native queue radio after manual resume from the natural end", async () => {
    holdSongRadio = true;
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const playing=makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:seed.id,track:seed,candidate:youtubeCandidate(seed),queue:[seed],currentIndex:0,paused:false,duration:240});
    const recommendation={...secondTrack,id:"resumed-native-radio-result",providerId:"resumed-native-radio-provider",sourceUrl:"https://example.test/resumed-native-radio"};
    mountNative(playing);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    emitNative({...playing,revision:51,status:"ended",naturalQueueEnd:true,paused:true});
    await settle();
    expect(finishSongRadio).toBeDefined();

    document.querySelector<HTMLButtonElement>('.play-button')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"resume"}});
    mocks.call.mockClear();
    finishSongRadio!({tracks:[recommendation],total:1});
    await settle();

    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args?.command as {action?:string})?.action==="play")).toBe(false);
  });

  it("starts one deduplicated WebView queue-radio result after successful natural queue exhaustion", async () => {
    previewMode = "direct";
    holdSongRadio = true;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const seed = { ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: youtubeCandidate(savedTrack) } };
    const recommendation = { ...secondTrack, id: "webview-natural-radio-result", providerId: "webview-natural-radio-provider", sourceUrl: "https://example.test/webview-natural-radio" };
    savedChartTracks = [seed];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>(".track-title")!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    expect(finishSongRadio).toBeDefined();

    const audio = document.querySelector<HTMLAudioElement>("audio")!;
    Object.defineProperty(audio, "ended", { configurable: true, value: true });
    audio.dispatchEvent(new Event("ended", { bubbles: true }));
    await settle();
    mocks.call.mockClear();
    finishSongRadio!({ tracks: [seed, recommendation, recommendation], total: 3 });
    await settle(); await settle();

    expect(mocks.call.mock.calls.filter(([command, args]) => command === "resolve_preview" && (args?.track as Track | undefined)?.id === recommendation.id)).toHaveLength(1);
    const snapshot = (await automationAction("webview-natural-radio-inspect", "volume", { volume: 0.8 })).playback;
    expect(snapshot.trackId).toBe(recommendation.id);
    expect(snapshot.currentIndex).toBe(1);
    expect(snapshot.paused).toBe(false);
    expect((snapshot.queue as Track[]).map((track) => track.id)).toEqual([seed.id, recommendation.id]);
  });

  it("does not continue WebView queue radio after automation pauses an ended track", async () => {
    previewMode="direct";
    holdSongRadio=true;
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const recommendation={...secondTrack,id:"paused-webview-radio-result",providerId:"paused-webview-radio-provider",sourceUrl:"https://example.test/paused-webview-radio"};
    savedChartTracks=[seed];
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();
    document.querySelector<HTMLButtonElement>(`.track-title`)!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    const audio=document.querySelector<HTMLAudioElement>("audio")!;
    expect(finishSongRadio).toBeDefined();
    Object.defineProperty(audio,"ended",{configurable:true,value:true});
    audio.dispatchEvent(new Event("pause"));
    audio.dispatchEvent(new Event("ended",{bubbles:true}));
    await settle();

    const pause=await automationAction("pause-ended-radio","pause");
    expect(pause.error).toBeNull();
    mocks.call.mockClear();
    finishSongRadio!({tracks:[recommendation],total:1});
    await settle();

    expect(mocks.call.mock.calls.some(([command,args])=>command==="resolve_preview"&&(args?.track as Track | undefined)?.id===recommendation.id)).toBe(false);
  });

  it("does not accept a held native radio result after repeat is enabled then disabled", async () => {
    holdSongRadio = true;
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const playing=makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:seed.id,track:seed,candidate:youtubeCandidate(seed),queue:[seed],currentIndex:0,paused:false});
    const recommendation={...secondTrack,id:"stale-repeat-radio-result",providerId:"stale-repeat-radio-provider",sourceUrl:"https://example.test/stale-repeat-radio"};
    mountNative(playing);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    emitNative({...playing,revision:51,status:"ended",naturalQueueEnd:true,paused:true});
    await settle();
    expect(finishSongRadio).toBeDefined();

    expect((await automationAction("repeat-during-radio", "repeat", {mode:"all"})).error).toBeNull();
    expect((await automationAction("repeat-off-during-radio", "repeat", {mode:"off"})).error).toBeNull();
    mocks.call.mockClear();
    finishSongRadio!({tracks:[recommendation],total:1});
    await settle();

    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args?.command as {action?:string})?.action==="play")).toBe(false);
  });

  it("does not start a late native queue-radio result after an explicit stop", async () => {
    holdSongRadio = true;
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const playing=makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:seed.id,track:seed,candidate:youtubeCandidate(seed),queue:[seed],currentIndex:0,paused:false,duration:240});
    const recommendation={...secondTrack,id:"stale-native-radio-result",providerId:"stale-native-radio-provider",sourceUrl:"https://example.test/stale-native-radio"};
    mountNative(playing);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    expect(finishSongRadio).toBeDefined();

    emitNative({...playing,revision:51,status:"ended",naturalQueueEnd:true,paused:true});
    await settle();
    const resolveOldRequest=finishSongRadio!;
    const stop=await automationAction("stop-after-radio-end","stop");
    expect(stop.error).toBeNull();
    mocks.call.mockClear();
    resolveOldRequest({tracks:[recommendation],total:1});
    await settle();

    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args?.command as {action?:string})?.action==="play")).toBe(false);
  });

  it("discards pending queue radio when the ended WebView track is manually resumed", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    holdSongRadio = true;
    const seed = { ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: youtubeCandidate(savedTrack) } };
    const recommendation = { ...secondTrack, id: "stale-radio-result", sourceUrl: "https://example.test/stale-radio" };
    savedChartTracks = [seed];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>(".track-title")!.click();
    await settle();
    const audio = document.querySelector<HTMLAudioElement>("audio")!;
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    expect(finishSongRadio).toBeDefined();
    const resolveOldRequest = finishSongRadio!;

    Object.defineProperty(audio, "ended", { configurable: true, value: true });
    audio.dispatchEvent(new Event("pause"));
    audio.dispatchEvent(new Event("ended", { bubbles: true }));
    await settle();
    document.querySelector<HTMLButtonElement>(".play-button")!.click();
    await settle();
    resolveOldRequest({ tracks: [recommendation], total: 1 });
    await settle();

    expect(mocks.call.mock.calls.some(([command, args]) => command === "resolve_preview" && (args?.track as Track | undefined)?.id === recommendation.id)).toBe(false);
  });

  it("seeds queue radio from the canonical parent track when the final queue item is a chapter", async () => {
    const candidate = youtubeCandidate(savedTrack);
    const parent = { ...savedTrack, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    const chapter = {
      ...parent,
      id: "chapter:saved:one",
      provider: "chapter",
      providerId: "saved:one",
      title: "Chapter Song",
      playbackRange: { parentTrackId: parent.id, chapterId: "one", sourceUrl: candidate.url, startTime: 20, endTime: 60, parentTitle: parent.title },
      provenance: { sourceKind: "chapter", parentTrack: parent, audioCandidate: candidate },
    } as Track;
    songRadioTracks = [secondTrack];
    mountNative(makeNativeSnapshot({ status: "playing", trackId: chapter.id, track: chapter, candidate, queue: [chapter], currentIndex: 0, paused: false }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();
    await settle();
    const radioCall = mocks.call.mock.calls.find(([command]) => command === "get_song_radio");
    expect(radioCall).toBeDefined();
    const radioArgs = radioCall?.[1];
    expect((radioArgs?.track as Track | undefined)?.id).toBe(parent.id);
    expect((radioArgs?.candidate as Candidate | undefined)?.id).toBe(candidate.id);
    expect(radioArgs?.persist).toBe(false);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_candidates" && JSON.stringify(args?.track) === JSON.stringify(chapter))).toBe(false);
  });

  it("does not start queue radio after manually advancing past the end", async () => {
    const seed={...savedTrack,provenance:{sourceKind:"audioSearch",audioCandidate:youtubeCandidate(savedTrack)}};
    const playing=makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:seed.id,track:seed,queue:[seed],currentIndex:0,paused:false});
    songRadioTracks=[secondTrack];mountNative(playing);await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Autoplay radio after queue"]')!.click();await settle();
    mocks.call.mockClear();emitNative({...playing,revision:51,status:"ended",naturalQueueEnd:false,paused:true});await settle();
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
  });

  it("does not prepare neighboring remote audio when a WebView user loads and selects library rows", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll<HTMLElement>(".track-table > .track-row, .shared-track-list > li")].find(item => item.textContent?.includes(savedTrack.title));
    expect(row).toBeDefined();
    expect(mocks.call.mock.calls.some(([command]) => ["warm_track_previews", "warm_playback_candidate_search", "resolve_preview_audio", "find_candidates", "resolve_playback_track"].includes(String(command)))).toBe(false);
    row!.click(); await settle();
    expect(mocks.call.mock.calls.some(([command]) => command === "warm_track_previews" || command === "warm_playback_candidate_search")).toBe(false);
    const resolved = mocks.call.mock.calls.filter(([command]) => command === "resolve_playback_track");
    expect(resolved.every(([, args]) => (args?.track as Track | undefined)?.id === savedTrack.id)).toBe(true);
  });

  it("does not launch competing library preview extractors during native startup", async () => {
    mountNative(makeNativeSnapshot());
    await settle();
    expect(document.body.textContent).toContain(savedTrack.title);
    expect(mocks.call.mock.calls.some(([command])=>command==="warm_track_previews")).toBe(false);
  });

  it("shows saved track duration for a paused restored queue without enabling seek", async () => {
    const restoredTrack: Track = { ...savedTrack, durationSeconds: 166.953809523 };
    const queue = Array.from({length: 200}, (_, index) => index === 1
      ? restoredTrack
      : { ...secondTrack, id: `restored-queue-${index}` });
    mountNative(makeNativeSnapshot({status:"paused",track:restoredTrack,trackId:restoredTrack.id,queue,currentIndex:1,
      paused:true,position:89.003239,duration:0,canSeek:false}));
    await settle();

    const times = [...document.querySelectorAll<HTMLElement>(".player-scrubber .time")].map((item) => item.textContent);
    const seek = document.querySelector<HTMLInputElement>('[aria-label="Playback position"]')!;
    expect(times).toEqual(["1:29", "2:46"]);
    expect(seek.disabled).toBe(true);
    expect(seek.max).toBe("166.953809523");
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    const queueRows = [...document.querySelectorAll<HTMLElement>(".queue-list article")];
    expect(queueRows).toHaveLength(200);
    expect(queueRows[1]?.getAttribute("aria-current")).toBe("true");
    expect(queueRows[1]?.textContent).toContain(restoredTrack.title);
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
  });

  it("shows duration from a saved WebView queue and playhead without starting audio or changing storage", async () => {
    const restoredTrack: Track = { ...savedTrack, durationSeconds: 166.953809523 };
    const queue = Array.from({length: 200}, (_, index) => index === 1
      ? restoredTrack
      : { ...secondTrack, id: `webview-restored-${index}` });
    const queueJson = JSON.stringify({tracks:queue,currentIndex:1,repeat:"off"});
    const positionJson = JSON.stringify({trackId:restoredTrack.id,position:89.003239,savedAt:Date.now()});
    const local = new Map<string,string>([
      ["burrata.playback-queue.v1",queueJson],
      ["burrata.playback-position.v1",positionJson],
    ]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
    await settle();

    expect(document.querySelector("audio")).toBeNull();
    expect(document.querySelector<HTMLElement>(".player-status")?.title).toContain("Ready to resume from 1:29");
    expect([...document.querySelectorAll<HTMLElement>(".player-scrubber .time")].map((item) => item.textContent)).toEqual(["1:29", "2:46"]);
    expect(document.querySelector<HTMLInputElement>('[aria-label="Playback position"]')?.disabled).toBe(true);
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
    const migrated = JSON.parse(local.get("burrata.playback-queue.v1")!);
    expect(migrated.tracks).toEqual(queue);
    expect(migrated.entryIds).toHaveLength(queue.length);
    expect(new Set(migrated.entryIds).size).toBe(queue.length);
    expect(local.get("burrata.playback-position.v1")).toBe(positionJson);
    const restoredEntryIds = [...migrated.entryIds];
    unmountApp?.();
    unmountApp = undefined;
    document.body.innerHTML = "";
    const restarted = mount(App, { target: document.body });
    unmountApp = () => { void unmount(restarted); };
    await settle();
    expect(JSON.parse(local.get("burrata.playback-queue.v1")!).entryIds).toEqual(restoredEntryIds);

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    const queueRows = [...document.querySelectorAll<HTMLElement>(".queue-list article")];
    expect(queueRows).toHaveLength(200);
    expect(queueRows[1]?.getAttribute("aria-current")).toBe("true");
    expect(queueRows[1]?.textContent).toContain(restoredTrack.title);
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
  });

  it("preserves exact queue-entry IDs when the queue panel moves a duplicate occurrence", async () => {
    const queue = [savedTrack, savedTrack, secondTrack];
    const entryIds = ["entry-first", "entry-duplicate", "entry-next"];
    const local = new Map<string, string>([[
      "burrata.playback-queue.v1",
      JSON.stringify({ tracks: queue, entryIds, currentIndex: 0, repeat: "off" }),
    ]]);
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => local.get(key) ?? null,
      setItem: (key: string, value: string) => local.set(key, value),
      removeItem: (key: string) => local.delete(key),
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();

    expect(document.querySelector("audio")).toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    const grips = document.querySelectorAll<HTMLButtonElement>(".queue-list .queue-drag-select");
    grips[1]!.click();
    grips[1]!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await settle();

    const persisted = JSON.parse(local.get("burrata.playback-queue.v1")!) as { tracks: Track[]; entryIds: string[]; currentIndex: number };
    expect(persisted.entryIds).toEqual(["entry-first", "entry-next", "entry-duplicate"]);
    expect(persisted.tracks.map((item) => item.id)).toEqual([savedTrack.id, secondTrack.id, savedTrack.id]);
    expect(persisted.currentIndex).toBe(0);
    expect(mocks.call.mock.calls.some(([command]) => command === "playback_command")).toBe(false);
    expect(document.querySelector("audio")).toBeNull();
  });

  it("keeps unknown and live native durations unknown when paused", async () => {
    const unknownTrack = { ...savedTrack, durationSeconds: null };
    mountNative(makeNativeSnapshot({status:"paused",track:unknownTrack,trackId:unknownTrack.id,queue:[unknownTrack],currentIndex:0,
      paused:true,position:0,duration:0}));
    await settle();
    expect([...document.querySelectorAll<HTMLElement>(".player-scrubber .time")].map((item) => item.textContent)).toEqual(["0:00", "0:00"]);

    const liveTrack: Track = { ...savedTrack, id:"live-radio", provider:"internet-radio", durationSeconds:240,
      provenance:{sourceKind:"radio",stationId:"station"} };
    emitNative(makeNativeSnapshot({revision:2,status:"paused",track:liveTrack,trackId:liveTrack.id,queue:[liveTrack],currentIndex:0,
      paused:true,position:0,duration:0}));
    await settle();
    expect([...document.querySelectorAll<HTMLElement>(".player-scrubber .time")].map((item) => item.textContent)).toEqual(["0:00", "0:00"]);
  });

  it("restores directory, search, album, and artist routes while discarding a late album response after Back", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    directoryBrowserPage={rootId:"folder-root",path:"/music",relativePath:"",breadcrumbs:[{name:"Music",relativePath:""}],directories:[{name:"Jazz",relativePath:"Jazz"}],files:[],total:0,offset:0,limit:200};
    directoryBrowserNestedPage={...directoryBrowserPage,path:"/music/Jazz",relativePath:"Jazz",breadcrumbs:[{name:"Music",relativePath:""},{name:"Jazz",relativePath:"Jazz"}],directories:[]};
    const releaseTrack={...savedTrack,id:"mixed-history-album-track",title:"Mixed route song",durationSeconds:240};
    const lateTrack={...savedTrack,id:"mixed-history-late-track",title:"Stale late album row",durationSeconds:240};
    discoveryAlbum={release:{provider:"musicbrainz",id:"mixed-history-album"},title:"Mixed History Album",sourceUrl:"https://musicbrainz.org/release/mixed-history-album",tracks:[],credits:[{name:"Visible Artist",role:"artist",artist:{provider:"musicbrainz",id:"artist-1"},sourceUrl:"https://musicbrainz.org/artist/artist-1"}]};
    discoveryAlbumTracks=[releaseTrack];
    const original=mocks.call.getMockImplementation()!;
    let holdReleaseTracks=true;
    let finishLateRelease!:(tracks:Track[])=>void;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="get_release_tracks"&&holdReleaseTracks)return new Promise(resolve=>{finishLateRelease=resolve as (tracks:Track[])=>void;});
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('[role="treeitem"][aria-label="Music"] .folder-tree-chevron')!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    document.querySelector<HTMLButtonElement>('[role="treeitem"][aria-label="Jazz"]')!.click();await settle();
    const folderFilter=document.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')!;
    folderFilter.value="mixed-folder-state";folderFilter.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click();await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Albums")!.click();await settle();
    await searchDiscovery("Mixed route album query",["musicbrainz"]);

    document.querySelector<HTMLButtonElement>(".source-results .open-result")!.click();await settle();
    expect(finishLateRelease).toBeDefined();
    expect(document.querySelector(".release-view h1")?.textContent).toBe("Mixed History Album");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector(".search-panel")).not.toBeNull();
    finishLateRelease([lateTrack]);await settle();await settle();
    expect(document.querySelector(".release-view")).toBeNull();
    expect(document.querySelector(".source-results")?.textContent).not.toContain(lateTrack.title);

    holdReleaseTracks=false;
    document.querySelector<HTMLButtonElement>(".source-results .open-result")!.click();await settle();await settle();
    expect(document.querySelector(".release-view .track-title")?.textContent).toBe(releaseTrack.title);
    const releaseReadCount=()=>mocks.call.mock.calls.filter(([command])=>command==="get_release_tracks").length;
    const loadedReleaseReadCount=releaseReadCount();
    document.querySelector<HTMLButtonElement>(".release-view .artists button")!.click();await settle();await settle();
    expect(document.querySelector(".artist-page")).not.toBeNull();

    const back=async()=>{document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();};
    await back();
    expect(document.querySelector(".release-view .track-title")?.textContent).toBe(releaseTrack.title);
    expect(releaseReadCount()).toBe(loadedReleaseReadCount);
    await back();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Mixed route album query");
    await back();
    expect(document.querySelector('[aria-label="Folder path"]')?.textContent).toContain("Jazz");
    expect(document.querySelector<HTMLInputElement>('[aria-label="Filter directory tracks"]')?.value).toBe("mixed-folder-state");

    const forward=async()=>{document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();await settle();};
    await forward();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Mixed route album query");
    await forward();
    expect(document.querySelector(".release-view .track-title")?.textContent).toBe(releaseTrack.title);
    await forward();
    expect(document.querySelector(".artist-page")).not.toBeNull();
    expect(releaseReadCount()).toBe(loadedReleaseReadCount);
    providerNavigationSnapshots.clear();
  });

  it("makes an expired album history route explicit and leaves fresh search usable", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    discoveryAlbum={release:{provider:"musicbrainz",id:"expired-album"},title:"Expiring Album",sourceUrl:"https://musicbrainz.org/release/expired-album",tracks:[],credits:[{name:"Visible Artist",role:"artist",artist:{provider:"musicbrainz",id:"artist-1"},sourceUrl:"https://musicbrainz.org/artist/artist-1"}]};
    discoveryAlbumTracks=[{...savedTrack,id:"expired-album-track",title:"Expiring song"}];
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Albums")!.click();await settle();
    await searchDiscovery("Expiring Album",["musicbrainz"]);
    document.querySelector<HTMLButtonElement>(".open-result")!.click();await settle();
    document.querySelector<HTMLButtonElement>(".release-view .artists button")!.click();await settle();
    expect(document.querySelector(".artist-page")).not.toBeNull();
    providerNavigationSnapshots.clear();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector(".release-view")).toBeNull();
    expect(document.querySelector(".search-panel")?.textContent).toContain("Previous search snapshot expired. Search again to reload it.");
    const before=mocks.call.mock.calls.filter(([command])=>command==="search_source").length;
    await searchDiscovery("Fresh after expiry",["musicbrainz"]);
    expect(mocks.call.mock.calls.filter(([command])=>command==="search_source").length).toBeGreaterThan(before);
    expect(mocks.call).toHaveBeenCalledWith("search_source",expect.objectContaining({query:"Fresh after expiry"}));
    expect(document.querySelector(".source-results")?.textContent).toContain("Expiring Album");
    providerNavigationSnapshots.clear();
  });

  it("restores search album subroutes through album Back and artist Back Forward without refetching", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    discoveryAlbum={release:{provider:"musicbrainz",id:"history-album"},title:"History Album",sourceUrl:"https://musicbrainz.org/release/history-album",tracks:[],credits:[{name:"Visible Artist",role:"artist",artist:{provider:"musicbrainz",id:"artist-1"},sourceUrl:"https://musicbrainz.org/artist/artist-1"}]};
    discoveryAlbumTracks=[{...savedTrack,id:"history-album-track",title:"Album retained song",durationSeconds:240}];
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="search_source"&&(args?.sourceIds as string[])?.[0]==="musicbrainz")return {sourceId:"musicbrainz",complete:false,nextCursor:args?.cursor?"album-page-three":"album-page-two",results:[{id:args?.cursor?"other-album":"history-album",kind:"album",sourceId:"musicbrainz",title:args?.cursor?"Other Album":"History Album",url:discoveryAlbum!.sourceUrl,data:discoveryAlbum}]};
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Albums")!.click();await settle();
    await searchDiscovery("Album route search",["musicbrainz"]);
    document.querySelector<HTMLButtonElement>(".source-results .load-more")!.click();await settle();
    document.querySelector<HTMLElement>(".search-panel")!.scrollTop=420;
    const searchReads=()=>mocks.call.mock.calls.filter(([command,args])=>command==="search_source"&&args?.query==="Album route search").length;
    const releaseReads=()=>mocks.call.mock.calls.filter(([command])=>command==="get_release_tracks").length;
    const before=searchReads();
    document.querySelector<HTMLButtonElement>(".open-result")!.click();await settle();
    expect(document.querySelector(".release-view h1")?.textContent).toBe("History Album");
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Album route search");
    expect(document.querySelector(".source-results")?.textContent).toContain("Other Album");expect(document.querySelector<HTMLElement>(".search-panel")?.scrollTop).toBe(420);expect(searchReads()).toBe(before);
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();
    const filter=document.querySelector<HTMLInputElement>('.release-view [aria-label="Filter tracks"]')!;filter.value="retained";filter.dispatchEvent(new Event("input",{bubbles:true}));
    const sort=document.querySelector<HTMLSelectElement>('.release-view [aria-label="Sort tracks"]')!;sort.value="title";sort.dispatchEvent(new Event("change",{bubbles:true}));
    document.querySelector<HTMLElement>(".release-view")!.scrollTop=180;await settle();
    const loaded=releaseReads();
    document.querySelector<HTMLButtonElement>(".release-view .artists button")!.click();await settle();expect(document.querySelector(".artist-page")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();
    expect(document.querySelector<HTMLInputElement>('.release-view [aria-label="Filter tracks"]')?.value).toBe("retained");expect(document.querySelector<HTMLSelectElement>('.release-view [aria-label="Sort tracks"]')?.value).toBe("title");expect(document.querySelector<HTMLElement>(".release-view")?.scrollTop).toBe(180);expect(releaseReads()).toBe(loaded);
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();expect(document.querySelector(".artist-page")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();expect(document.querySelector(".release-view h1")?.textContent).toBe("History Album");expect(releaseReads()).toBe(loaded);expect(searchReads()).toBe(before);
    providerNavigationSnapshots.clear();
  });

  it("restores two-page search controls and scroll through artist Back and Forward without rerunning the query", async () => {
    const {providerNavigationSnapshots}=await import("./lib/provider-navigation-snapshots");providerNavigationSnapshots.clear();
    const first={...savedTrack,id:"search-history-first",title:"Alpha restored track",durationSeconds:240};
    const second={...savedTrack,id:"search-history-second",title:"Zulu restored track",durationSeconds:250};
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="search_source"&&(args?.sourceIds as string[])?.[0]==="musicbrainz")return {sourceId:"musicbrainz",complete:false,nextCursor:args?.cursor?"page-three":"page-two",results:[args?.cursor?{id:"second",kind:"track",sourceId:"musicbrainz",title:second.title,track:second}:{id:"first",kind:"track",sourceId:"musicbrainz",title:first.title,track:first}]};
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find(button=>button.textContent?.includes("Search"))!.click();await settle();
    await selectSearchSources(["musicbrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Tracks")!.click();await settle();
    await searchDiscovery("Navigation search",["musicbrainz"]);
    document.querySelector<HTMLButtonElement>(".source-results .load-more")!.click();await settle();
    const sort=document.querySelector<HTMLSelectElement>('[aria-label="Sort loaded tracks"]')!;sort.value="title";sort.dispatchEvent(new Event("change",{bubbles:true}));
    document.querySelector<HTMLButtonElement>('[aria-label="Toggle loaded track sort direction"]')!.click();
    for(const [label,value] of [["Minimum loaded track duration","200"],["Maximum loaded track duration","300"]]){const input=document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;input.value=value;input.dispatchEvent(new Event("input",{bubbles:true}));}
    await settle();
    const panel=document.querySelector<HTMLElement>(".search-panel")!;panel.scrollTop=480;panel.dispatchEvent(new Event("scroll"));
    const searchCount=()=>mocks.call.mock.calls.filter(([command,args])=>command==="search_source"&&args?.query==="Navigation search").length;
    expect(searchCount()).toBe(2);
    document.querySelector<HTMLButtonElement>(".compact-track .artist-link")!.click();await settle();
    expect(document.querySelector(".artist-page")).not.toBeNull();
    const assertRestored=()=>{
      expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe("Navigation search");
      expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("musicbrainz");
      expect([...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button=>button.textContent?.trim()==="Tracks")?.getAttribute("aria-pressed")).toBe("true");
      expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort loaded tracks"]')?.value).toBe("title");
      expect(document.querySelector('[aria-label="Toggle loaded track sort direction"]')?.textContent).toContain("Descending");
      expect(document.querySelector<HTMLInputElement>('[aria-label="Minimum loaded track duration"]')?.value).toBe("200");
      expect(document.querySelector<HTMLInputElement>('[aria-label="Maximum loaded track duration"]')?.value).toBe("300");
      expect(document.querySelector(".source-results")?.textContent).toContain(first.title);expect(document.querySelector(".source-results")?.textContent).toContain(second.title);
      expect(document.querySelector<HTMLElement>(".search-panel")?.scrollTop).toBe(480);expect(searchCount()).toBe(2);
    };
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();assertRestored();
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click();await settle();expect(document.querySelector(".artist-page")).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click();await settle();await settle();assertRestored();
    //Continuation stays bound to both retained pages, not a restarted first page.
    document.querySelector<HTMLButtonElement>(".source-results .load-more")!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_source",expect.objectContaining({query:"Navigation search",cursor:"page-three"}));
    providerNavigationSnapshots.clear();
  });

  it("opens the current radio station without restarting native playback", async () => {
    radioStationState=[{id:"main",name:"Main Mix",streamUrl:"https://radio.example/main",sourceUrl:"https://radio.example",provider:"radio_paradise",isCustom:false}];
    const track:Track={...savedTrack,id:"radio:main",provider:"radio",providerId:"main",title:"Overnight selection",artist:"Radio Paradise",sourceUrl:"https://radio.example",provenance:{stationId:"main",sourceKind:"radio",radioStreamUrl:"https://radio.example/main"}};
    mountNative(makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:track.id,track,queue:[track],currentIndex:0,paused:false,position:127,duration:0}));
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>(".radio-section .playlist-card")!.click();
    await settle();
    expect(document.querySelector(".radio-page")).not.toBeNull();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Main Mix");
    expect(document.querySelector(".player-source")).toBeNull();
    const footerInfo=document.querySelector("footer .station-info")!;
    footerInfo.querySelector("summary")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    expect(footerInfo.querySelector('a[href="https://radio.example/"]')).not.toBeNull();
    expect(footerInfo.querySelector('a[href="https://radio.example/main"]')).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();await settle();
    const panelInfo=document.querySelector(".player-panel-host .station-info")!;
    expect(panelInfo.querySelector("summary")?.getAttribute("aria-label")).toBe("Station information for Main Mix");
    panelInfo.querySelector("summary")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    expect(panelInfo.querySelector('a[href="https://radio.example/"]')).not.toBeNull();
    expect(panelInfo.querySelector('a[href="https://radio.example/main"]')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
    expect(mocks.call.mock.calls.some(([command])=>command==="resolve_preview"||command==="resolve_candidate_audio")).toBe(false);
  });

  it("starts a clicked track through playback_command without resolving or creating WebView audio", async () => {
    const mediaPlay=vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    mountNative(makeNativeSnapshot());
    await settle();
    mocks.call.mockClear();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(savedTrack.title))!
      .dispatchEvent(new MouseEvent("click",{bubbles:true}));
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"play",tracks:expect.arrayContaining([expect.objectContaining({id:savedTrack.id})]),index:0,candidate:undefined,position:0,operationId:expect.any(String),clientStartedAtMs:expect.any(Number)})});
    expect(mocks.call.mock.calls.some(([command])=>["resolve_preview","resolve_candidate_audio","start_candidate_preview"].includes(String(command)))).toBe(false);
    expect(document.querySelector("audio")).toBeNull();
    expect(document.body.textContent).not.toContain("This preview source is unavailable in this player.");
    expect([...document.querySelectorAll("button")].some(button=>button.textContent?.trim()==="Close player")).toBe(false);
    expect(mediaPlay).not.toHaveBeenCalled();
  });

  it("replays an explicitly clicked native track from zero despite a saved end position", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    const local=new Map<string,string>([["burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,position:190.31,savedAt:Date.now()})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    mountNative(makeNativeSnapshot({status:"ended",paused:true,track:savedTrack,trackId:savedTrack.id,queue:[savedTrack],currentIndex:0,position:190.31,duration:190.52,playerMessage:"Queue finished"}));
    await settle();mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"play",position:0})});
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('.play-button')!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"resume"}});
  });

  it("selects within an identical native queue without resending the full queue", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    mountNative(makeNativeSnapshot({status:"paused",track:savedTrack,trackId:savedTrack.id,queue:[savedTrack,secondTrack],currentIndex:0,paused:true}));
    await settle();
    mocks.call.mockClear();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(secondTrack.title))!
      .dispatchEvent(new MouseEvent("click",{bubbles:true}));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"select",index:1})});
    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args as {command:{action?:string}}).command.action==="play")).toBe(false);
  });

  it("sends a new native play command when the displayed source differs from its queue item", async () => {
    const changedTrack = { ...secondTrack, sourceUrl: "https://example.test/alternate-source" };
    savedChartTracks = [savedTrack, changedTrack];
    mountNative(makeNativeSnapshot({status:"paused",track:savedTrack,trackId:savedTrack.id,queue:[savedTrack,secondTrack],currentIndex:0,paused:true}));
    await settle();
    mocks.call.mockClear();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(changedTrack.title))!
      .dispatchEvent(new MouseEvent("click",{bubbles:true}));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"play",tracks:[savedTrack,changedTrack],index:1,candidate:undefined,position:0})});
  });

  it("sends chapter-relative seek positions and clears buffering when native output starts", async () => {
    const chapterTrack:Track={...savedTrack,id:"chapter-one",title:"Chapter One",playbackRange:{parentTrackId:"full-set",chapterId:"chapter-one",sourceUrl:"https://example.test/full-set",startTime:72,endTime:102,parentTitle:"Full Set"}};
    mountNative(makeNativeSnapshot({revision:4,generation:9,status:"buffering",trackId:chapterTrack.id,track:chapterTrack,queue:[chapterTrack],currentIndex:0,paused:false,position:3,duration:30,canSeek:true,playerMessage:"Buffering audio…"}));
    await settle();
    expect(document.querySelector<HTMLButtonElement>(".play-button")?.getAttribute("aria-busy")).toBe("true");
    mocks.call.mockClear();
    const seek=document.querySelector<HTMLInputElement>('[aria-label="Playback position"]')!;
    seek.value="7";seek.dispatchEvent(new Event("input",{bubbles:true}));
    expect(mocks.call).not.toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"seek",position:7})});
    seek.dispatchEvent(new Event("pointerup",{bubbles:true}));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"seek",position:7})});

    emitNative(makeNativeSnapshot({revision:5,generation:9,status:"playing",trackId:chapterTrack.id,track:chapterTrack,queue:[chapterTrack],currentIndex:0,paused:false,position:7,duration:30,canSeek:true,playerMessage:"Playing audio"}));
    await settle();
    expect(document.querySelector<HTMLButtonElement>(".play-button")?.getAttribute("aria-busy")).toBe("false");
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
  });

  it("uses absolute chapter time for a native ranged track and hides nested chapter rows", async () => {
    const rangedTrack: Track = { ...savedTrack, id: "chapter-window", provider: "chapter", title: "Chapter Window", playbackRange: {
      parentTrackId: "full-set", chapterId: "middle", sourceUrl: "https://youtube.com/watch?v=abcdefghi12",
      startTime: 100, endTime: 200, parentTitle: "Full Set",
    } };
    const candidate: Candidate = { ...youtubeCandidate(rangedTrack), chapters: [
      { id: "intro", title: "Intro", startTime: 100, endTime: 110, confidence: "review" },
      { id: "middle", title: "Middle", startTime: 110, endTime: 140, confidence: "review" },
    ] };
    mountNative(makeNativeSnapshot({ revision: 56, generation: 16, status: "playing", trackId: rangedTrack.id, track: rangedTrack, candidate, queue: [rangedTrack], currentIndex: 0, paused: false, position: 20, duration: 100 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();

    const activeChapter = document.querySelector<HTMLButtonElement>('.chapter-list .chapter-play[aria-current="time"]');
    expect(activeChapter?.textContent).toContain("Middle");
    expect(activeChapter?.textContent).not.toContain("Intro");
    document.querySelector<HTMLElement>('[aria-label="More actions for Chapter Window"]')!.click();
    await settle();
    const rowMenu = document.querySelector(".queue-list article.current .queue-more-menu");
    expect([...rowMenu!.querySelectorAll("button")].some((button) => button.textContent?.includes("Show chapters"))).toBe(false);
  });

  it("routes MCP queue requests to native playback and renders the queue after its snapshot event", async () => {
    savedChartTracks=[savedTrack,secondTrack];
    mountNative(makeNativeSnapshot({revision:10,generation:2,status:"paused",trackId:savedTrack.id,track:savedTrack,queue:[savedTrack],currentIndex:0,paused:true}));
    await settle();
    mocks.call.mockClear();
    const completed=await automationAction("native-queue-request","queue",{trackIds:[secondTrack.id],mode:"append"});
    expect(completed.error).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"queue",tracks:[secondTrack],mode:"append"}});
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(document.querySelectorAll(".playback-queue-panel .queue-list article")).toHaveLength(1);

    emitNative(makeNativeSnapshot({revision:11,generation:2,status:"paused",trackId:savedTrack.id,track:savedTrack,queue:[savedTrack,secondTrack],currentIndex:0,paused:true}));
    await settle();
    expect(document.querySelectorAll(".playback-queue-panel .queue-list article")).toHaveLength(2);
    expect(document.querySelector(".playback-queue-panel")?.textContent).toContain(secondTrack.title);
  });

  it("delegates manual Next to the native queue without clearing repeat-one", async () => {
    mountNative(makeNativeSnapshot({revision:20,generation:5,status:"playing",trackId:savedTrack.id,track:savedTrack,queue:[savedTrack,secondTrack],currentIndex:0,paused:false,repeat:"one"}));
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('[aria-label="Next recording"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"next"}});
    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args as {command:{action?:string}}).command.action==="repeat")).toBe(false);
  });

  it("keeps native favorite state canonical when a later snapshot has stale track metadata", async () => {
    favoriteTrackIds.add(savedTrack.id);
    const staleFavorite={...savedTrack,favorite:true};
    mountNative(makeNativeSnapshot({revision:25,generation:6,status:"paused",trackId:savedTrack.id,track:staleFavorite,queue:[staleFavorite],currentIndex:0,paused:true}));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Remove playing track from favorites"]')!.click();
    await settle();
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Add playing track to favorites"]')?.getAttribute("aria-pressed")).toBe("false");
    emitNative(makeNativeSnapshot({revision:26,generation:6,status:"paused",trackId:savedTrack.id,track:staleFavorite,queue:[staleFavorite],currentIndex:0,paused:true}));
    await settle();
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Add playing track to favorites"]')?.getAttribute("aria-pressed")).toBe("false");
  });

  it("preserves an active native song while replacing its queue with song radio", async () => {
    const candidate=youtubeCandidate(savedTrack);
    songRadioTracks=[secondTrack];
    mountNative(makeNativeSnapshot({revision:30,generation:7,status:"playing",trackId:savedTrack.id,track:savedTrack,candidate,queue:[savedTrack],currentIndex:0,paused:false}));
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.trim()==="Start song radio")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:{action:"queue",tracks:[savedTrack,secondTrack],mode:"replace",preserveCurrent:true}});
    expect(mocks.call.mock.calls.some(([command,args])=>command==="playback_command"&&(args as {command:{action?:string}}).command.action==="play")).toBe(false);
  });

  it("uses canonical radio metadata and retains older repeats in native radio history", async () => {
    const stationTrack:Track={...savedTrack,id:"station-radio",provider:"radio",providerId:"station-1",title:"Station Feed",artist:"Radio Station",provenance:{sourceKind:"radio",stationId:"station-1"}};
    const currentRadioTrack:Track={...savedTrack,id:"canonical-radio-song",title:"Current Song",artist:"Radio Artist",provider:"radio_song"};
    mountNative(makeNativeSnapshot({
      revision:40,generation:8,status:"playing",trackId:stationTrack.id,track:stationTrack,queue:[stationTrack],currentIndex:0,paused:false,
      radioNowPlaying:{id:currentRadioTrack.id,title:currentRadioTrack.title,artist:currentRadioTrack.artist,stationName:"Test Station",metadata:{stationId:"station-1",title:currentRadioTrack.title,artist:currentRadioTrack.artist,sourceUrl:"https://example.test/song",metadataConfidence:"high"}},
      radioHistory:[
        {id:"current-occurrence",track:currentRadioTrack,playedAt:"2026-10-14T10:00:00Z",stationName:"Test Station"},
        {id:"prior-occurrence",track:{...currentRadioTrack,title:"Earlier Repeat"},playedAt:"2026-10-13T10:00:00Z",stationName:"Test Station"},
      ],
    }));
    await settle();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Current Song");
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(document.querySelector(".playback-queue-panel .radio-current")?.textContent).toContain("Current Song");
    document.querySelector<HTMLButtonElement>(".playback-queue-panel .history-toggle")!.click();
    await settle();
    expect(document.querySelector(".playback-queue-panel .radio-history")?.textContent).toContain("Earlier Repeat");
  });
});

describe("authoritative startup folder status",()=>{
 it("uses loaded settings only for legacy status without a folder field",async()=>{
  configuredMusicDirectory='/legacy/configured';const original=mocks.call.getMockImplementation()!;
  let finish!:(value:{setupComplete:boolean})=>void;const delayed=new Promise<{setupComplete:boolean}>(resolve=>finish=resolve);
  mocks.call.mockImplementation((command,args)=>command==='get_release_setup'?delayed:original(command,args));
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();finish({setupComplete:false});await settle();expect(document.body.textContent).not.toContain('Welcome to Burrata');
 });

 it("does not show onboarding for a configured backend folder while settings are delayed",async()=>{
  const original=mocks.call.getMockImplementation()!;const settings=await original('get_settings') as Settings;
  let finish!:(value:Settings)=>void;const delayed=new Promise<Settings>(resolve=>finish=resolve);
  mocks.call.mockImplementation((command,args)=>command==='get_settings'?delayed:command==='get_release_setup'?Promise.resolve({setupComplete:false,musicDirectory:'/configured/music'}):original(command,args));
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
  expect(mocks.call).toHaveBeenCalledWith('get_release_setup');expect(document.body.textContent).not.toContain('Welcome to Burrata');
  finish({...settings,musicDirectory:'/configured/music'});await settle();expect(document.body.textContent).not.toContain('Welcome to Burrata');
 });
 it("still offers onboarding when authoritative status reports no folder",async()=>{
  configuredMusicDirectory="/stale/settings-folder";
  const original=mocks.call.getMockImplementation()!;
  mocks.call.mockImplementation((command,args)=>command==='get_release_setup'?Promise.resolve({setupComplete:false,musicDirectory:null,radioRecognition:false,mcpCommand:[],browser:{browsers:[]},toolPaths:[]}):original(command,args));
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();expect(document.body.textContent).toContain('Welcome to Burrata');
 });
});

describe("resume position persistence", () => {
  it("deduplicates routine timeupdates with a 60-second heartbeat and preserves forced and retry writes", async () => {
    previewMode = "direct";
    let nowValue = 1_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => nowValue);
    savedChartTracks = [savedTrack, secondTrack];
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const values = new Map<string, string>();
    let positionWrites = 0;
    let failNextPositionWrite = false;
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        if (key === "burrata.playback-position.v1") {
          positionWrites++;
          if (failNextPositionWrite) { failNextPositionWrite = false; throw new Error("storage unavailable"); }
        }
        values.set(key, value);
      },
      removeItem: (key: string) => values.delete(key),
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${savedTrack.title}"]`)!.click();
    await settle();

    let audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 12.5 });
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    const firstWriteCount = positionWrites;
    expect(firstWriteCount).toBeGreaterThan(0);
    audio.dispatchEvent(new Event("timeupdate")); audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(firstWriteCount);
    nowValue += 59_999;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(firstWriteCount);
    nowValue += 1;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(firstWriteCount + 1);

    Object.defineProperty(audio, "currentTime", { configurable: true, value: 12.75 });
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(firstWriteCount + 2);
    audio.dispatchEvent(new Event("pause")); await settle();
    expect(positionWrites).toBe(firstWriteCount + 3);

    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${secondTrack.title}"]`)!.click();
    await settle();
    audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 12.75 });
    const beforeTrackChangeWrite = positionWrites;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(beforeTrackChangeWrite + 1);
    expect(JSON.parse(values.get("burrata.playback-position.v1")!).trackId).toBe(secondTrack.id);

    Object.defineProperty(audio, "currentTime", { configurable: true, value: 18 });
    failNextPositionWrite = true;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    const failedWriteCount = positionWrites;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(failedWriteCount + 1);
    expect(JSON.parse(values.get("burrata.playback-position.v1")!).position).toBe(18);
    const beforeRollbackWrite = positionWrites;
    nowValue -= 1;
    audio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(positionWrites).toBe(beforeRollbackWrite + 1);
  });
});

describe("loaded search continuation playback snapshot", () => {
  it("queues visible sorted loaded rows and keeps that occurrence snapshot through paging and a later query", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const makeTrack = (id: string, title: string, durationSeconds: number): Track => {
      const base = { ...savedTrack, id, title, durationSeconds };
      return { ...base, provenance: { audioCandidate: youtubeCandidate(base) } };
    };
    const beta = makeTrack("snapshot-beta", "Beta", 90);
    const alpha = makeTrack("snapshot-alpha", "Alpha", 120);
    const short = makeTrack("snapshot-short", "Short", 30);
    const laterPage = makeTrack("snapshot-page-two", "Aardvark", 180);
    const replacement = makeTrack("snapshot-new-query", "New query result", 180);
    const asRow = (track: Track) => ({ id: track.id, kind: "video", sourceId: "youtube", title: track.title, subtitle: track.artist, track });
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.[0] === "youtube") {
        if (args?.query === "replacement query") return { sourceId: "youtube", complete: true, results: [asRow(replacement)] };
        if (args?.cursor === "page-two") return { sourceId: "youtube", complete: true, results: [asRow(laterPage)] };
        return { sourceId: "youtube", complete: false, nextCursor: "page-two", results: [asRow(beta), asRow(alpha), asRow(short)] };
      }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery("initial snapshot query", ["youtube"]);

    document.querySelector<HTMLElement>(".loaded-track-filters summary")!.click(); await settle();
    const minimum = document.querySelector<HTMLInputElement>('[aria-label="Minimum loaded track duration"]')!;
    minimum.value = "60"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort loaded tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const continuation = document.querySelector<HTMLInputElement>(".continue-setting input")!;
    continuation.click(); await settle();

    const visibleRows = [...document.querySelectorAll<HTMLElement>(".source-group .result-row")];
    expect(visibleRows.map(row => row.querySelector(".track-title")?.textContent?.trim())).toEqual(["Alpha", "Beta"]);
    expect(document.querySelector('[aria-label="Play Short"]')).toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Play Beta"]')!.click(); await settle(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    const queueTitles = () => [...document.querySelectorAll<HTMLElement>(".playback-queue-panel .queue-list article .track-copy strong")].map(node => node.textContent?.trim());
    expect(queueTitles()).toEqual(["Alpha", "Beta"]);
    expect(document.querySelector(".playback-queue-panel .queue-list article.current .track-copy strong")?.textContent?.trim()).toBe("Beta");

    document.querySelector<HTMLButtonElement>(".source-group .load-more")!.click(); await settle(); await settle();
    expect([...document.querySelectorAll<HTMLElement>(".source-group .result-row")].some(row => row.textContent?.includes("Aardvark"))).toBe(true);
    expect(queueTitles()).toEqual(["Alpha", "Beta"]);

    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true }));
    document.querySelector<HTMLButtonElement>('[aria-label="Toggle loaded track sort direction"]')!.click(); await settle();
    expect(queueTitles()).toEqual(["Alpha", "Beta"]);

    await searchDiscovery("replacement query", ["youtube"]);
    expect(document.querySelector('.source-group [aria-label="Play New query result"]')).not.toBeNull();
    expect(document.querySelector('.source-group [aria-label="Play Alpha"]')).toBeNull();
    expect(queueTitles()).toEqual(["Alpha", "Beta"]);
  });
});

describe("search-only upcoming WebView preparation", () => {
  it("warms one actual upcoming occurrence on playing, never timeupdate, and cancels it on seek", async () => {
    previewMode="direct";
    const local=new Map<string,string>([["burrata.playback-queue.v1",JSON.stringify({tracks:[savedTrack,secondTrack],currentIndex:0,repeat:"off"})]]);
    vi.stubGlobal("localStorage",{getItem:(key:string)=>local.get(key)??null,setItem:(key:string,value:string)=>local.set(key,value),removeItem:(key:string)=>local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command,args)=>command==="warm_playback_candidate_search"?new Promise(()=>{}):original(command,args));
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    expect(mocks.call.mock.calls.some(([command])=>command==="warm_track_previews"||command==="warm_playback_candidate_search")).toBe(false);
    await automationAction("candidate-prefetch-start","resume");await settle();
    const audio=document.querySelector("audio")!;
    expect(mocks.call.mock.calls.some(([command])=>command==="warm_playback_candidate_search")).toBe(false);
    audio.dispatchEvent(new Event("playing"));await settle();
    const warmups=mocks.call.mock.calls.filter(([command])=>command==="warm_playback_candidate_search");
    expect(warmups).toHaveLength(1);expect(warmups[0][1]).toMatchObject({track:{id:secondTrack.id}});
    for(let i=1;i<=20;i++){audio.currentTime=i;audio.dispatchEvent(new Event("timeupdate"));}await settle();
    audio.dispatchEvent(new Event("playing"));await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="warm_playback_candidate_search")).toHaveLength(1);
    await automationAction("candidate-prefetch-seek","seek",{position:30});await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_remote_read",{requestId:(warmups[0][1] as {requestId:string}).requestId});
    expect(mocks.call.mock.calls.filter(([command])=>command==="warm_playback_candidate_search")).toHaveLength(1);
    audio.dispatchEvent(new Event("playing")); await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="warm_playback_candidate_search")).toHaveLength(2);
    for (let i = 0; i < 20; i++) audio.dispatchEvent(new Event("playing")); await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="warm_playback_candidate_search")).toHaveLength(2);
  });

  it("reorders the playing queue and warms its new next occurrence without another playing event", async () => {
    previewMode = "direct";
    const thirdTrack: Track = {...savedTrack, id: "prefetch-third", title: "Third Recording"};
    const local = new Map<string, string>([["burrata.playback-queue.v1", JSON.stringify({tracks: [savedTrack, secondTrack, thirdTrack], currentIndex: 0, repeat: "off"})]]);
    vi.stubGlobal("localStorage", {getItem: (key: string) => local.get(key) ?? null, setItem: (key: string, value: string) => local.set(key, value), removeItem: (key: string) => local.delete(key)});
    vi.spyOn(HTMLMediaElement.prototype, "readyState", "get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockReturnValue(false);
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => command === "warm_playback_candidate_search" ? new Promise(() => {}) : original(command, args));
    const instance = mount(App, {target: document.body}); unmountApp = () => {void unmount(instance);}; await settle();
    await automationAction("prefetch-reorder-start", "resume"); await settle();
    const audio = document.querySelector("audio")!;
    audio.dispatchEvent(new Event("playing")); await settle();
    const first = mocks.call.mock.calls.find(([command]) => command === "warm_playback_candidate_search")!;
    expect(first[1]).toMatchObject({track: {id: secondTrack.id}});
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    document.querySelector<HTMLElement>('[aria-label="More actions for Third Recording"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Move up")!.click(); await settle();
    const warmups = mocks.call.mock.calls.filter(([command]) => command === "warm_playback_candidate_search");
    expect(warmups).toHaveLength(2);
    expect(warmups[1][1]).toMatchObject({track: {id: thirdTrack.id}});
    expect(mocks.call).toHaveBeenCalledWith("cancel_remote_read", {requestId: (first[1] as {requestId: string}).requestId});
    expect(document.querySelector("audio")).toBe(audio);
  });

  it("queued receipt accepts the installed exact album before source completion and keeps all 19 failures beyond 45 seconds", async () => {
    vi.useFakeTimers();
    const album = Array.from({ length: 19 }, (_, index) => ({
      ...savedTrack, id: index === 1 ? "queued-album-0" : `queued-album-${index}`, title: `Queued Recording ${index}`
    }));
    savedChartTracks = album;
    automationTrackLookup = album;
    previewMode = "fallback";
    const failures: Array<(reason: Error) => void> = [];
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "resolve_playback_track")
        return new Promise((_resolve, reject) => failures.push(reject));
      if (command === "automation_queued_playback_accept" || command === "automation_queued_playback_update")
        return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({
      event: "automation-queued-playback", id: 100, payload: {
        operationId: "queued-album-owner", requestId: "queued-album-rpc", trackIds: album.map(track => track.id)
      }
    });
    await settle();
    await settle();
    const acceptance = lastCallOf("automation_queued_playback_accept")![1] as {
      operationId: string;
      requestId: string;
      receipt: {
        trackIds: string[];
        queueEntryIds: string[];
        queueRevision: string;
        acceptedGeneration: number;
      };
    };
    expect(acceptance).toMatchObject({
      operationId: "queued-album-owner", requestId: "queued-album-rpc", receipt: { trackIds: album.map(track => track.id) }
    });
    expect(new Set(acceptance.receipt.queueEntryIds).size).toBe(19);
    expect(failures).toHaveLength(1);
    expect(mocks.call.mock.calls.findIndex(([command]) => command === "resolve_preview")).toBeLessThan(mocks.call.mock.calls.findIndex(([command]) => command === "automation_queued_playback_accept"));
    for (let index = 0; index < 19; index++) {
      await vi.advanceTimersByTimeAsync(4000);
      failures[index]!(new Error("No playable audio match was found"));
      await settle();
      await settle();
      if (index === 11) {
        expect(mocks.call.mock.calls.filter(([command]) => command === "automation_queued_playback_accept")).toHaveLength(1);
        expect(failures).toHaveLength(13);
      }
    }
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    expect(failures).toHaveLength(19);
    const history = (lastCallOf("automation_playback_state")![1] as {
      playback: {
        diagnostics: {
          recentTrackFailures: Array<{
            entryId: string;
            operationId: string;
            outcome: string;
            category: string;
          }>;
        };
      };
    }).playback.diagnostics.recentTrackFailures;
    expect(history).toHaveLength(19);
    expect(history.map(failure => failure.entryId)).toEqual(acceptance.receipt.queueEntryIds);
    expect(history.every(failure => failure.operationId === "queued-album-owner" && failure.category === "no_playable_match")).toBe(true);
    expect(history[18]!.outcome).toBe("queue_exhausted");
    expect(mocks.call).toHaveBeenCalledWith("automation_queued_playback_update", expect.objectContaining({
      operationId: "queued-album-owner", requestId: "queued-album-rpc", status: "exhausted"
    }));
    expect(mocks.call.mock.calls.some(([command, args]) => command === "automation_playback_complete" && (args as {
      requestId?: string;
    }).requestId === "queued-album-rpc")).toBe(false);
  });
  it("queued receipt cancellation from an older job cannot pause replacement playback or consume late source results", async () => {
    previewMode = "fallback";
    savedChartTracks = [savedTrack, secondTrack];
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => { });
    let rejectOld!: (reason: Error) => void;
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "resolve_playback_track" && (args?.track as Track)?.id === savedTrack.id)
        return new Promise((_resolve, reject) => { rejectOld = reject; });
      if (command === "automation_queued_playback_accept" || command === "automation_queued_playback_update")
        return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({
      event: "automation-queued-playback", id: 101, payload: {
        operationId: "old-queued-owner", requestId: "old-queued-rpc", trackIds: [savedTrack.id]
      }
    });
    await settle();
    await settle();
    expect(rejectOld).toBeDefined();
    await automationAction("replacement-play", "play", { trackIds: [secondTrack.id] });
    const pauses = pause.mock.calls.length, plays = play.mock.calls.length;
    eventHandlers.get("automation-queued-playback-cancelled")!({
      event: "automation-queued-playback-cancelled", id: 102, payload: { operationId: "old-queued-owner" }
    });
    rejectOld(new Error("No playable audio match was found"));
    await settle();
    await settle();
    expect(pause).toHaveBeenCalledTimes(pauses);
    expect(play).toHaveBeenCalledTimes(plays);
    expect((lastCallOf("automation_playback_complete")![1] as {playback:{trackId:string}}).playback.trackId).toBe(secondTrack.id);
    expect(mocks.call).toHaveBeenCalledWith("automation_queued_playback_update", expect.objectContaining({ operationId: "old-queued-owner", status: "superseded" }));
  });


  it("queued receipt keeps one owner through UI pause, transport seek/resume, transport pause, and UI resume",async()=>{
    previewMode="direct";let mediaPaused=true;
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockImplementation(()=>mediaPaused);
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(240);
    const play=vi.spyOn(HTMLMediaElement.prototype,"play").mockImplementation(()=>{mediaPaused=false;return Promise.resolve()});
    vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{mediaPaused=true});
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command,args)=>command==="automation_queued_playback_accept"||command==="automation_queued_playback_update"?Promise.resolve({accepted:true}):original(command,args));
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-queued-playback")!({event:"automation-queued-playback",id:103,payload:{operationId:"paused-root",requestId:"paused-root-rpc",trackIds:[savedTrack.id]}});await settle();await settle();
    document.querySelector<HTMLButtonElement>(".play-button")!.click();document.querySelector("audio")!.dispatchEvent(new Event("pause"));await settle();
    await automationAction("seek-transport","seek",{position:20});
    await automationAction("resume-transport","resume");
    await automationAction("pause-transport","pause");
    document.querySelector<HTMLButtonElement>(".play-button")!.click();await settle();await settle();
    const updates=mocks.call.mock.calls.filter(([command])=>command==="automation_queued_playback_update").map(([,args])=>args as {operationId:string;requestId:string;status:string});
    expect(updates.every(update=>update.operationId==="paused-root"&&update.requestId==="paused-root-rpc")).toBe(true);
    expect(updates.some(update=>update.status==="paused")).toBe(true);expect(updates[updates.length-1]!.status).toBe("playing");
    expect(updates.some(update=>["cancelled","superseded","failed"].includes(update.status))).toBe(false);
    expect(mocks.call.mock.calls.filter(([command])=>command==="automation_queued_playback_accept")).toHaveLength(1);expect(play).toHaveBeenCalledTimes(3);
    const receipt=lastCallOf("automation_queued_playback_accept")![1] as {receipt:{queueEntryIds:string[]}};
    expect((lastCallOf("automation_playback_complete")![1] as {playback:{queueEntryIds:string[]}}).playback.queueEntryIds).toEqual(receipt.receipt.queueEntryIds);
  });

  it("does not let a delayed pause event cancel the newer owner-bound resume", async () => {
    previewMode = "direct";
    let mediaPaused = true;
    vi.spyOn(HTMLMediaElement.prototype, "paused", "get").mockImplementation(() => mediaPaused);
    vi.spyOn(HTMLMediaElement.prototype, "duration", "get").mockReturnValue(240);
    let finishResume!: () => void;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => {
      if (play.mock.calls.length === 1) {
        mediaPaused = false;
        return Promise.resolve();
      }
      return new Promise<void>(resolve => { finishResume = () => { mediaPaused = false; resolve(); }; });
    });
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => { mediaPaused = true; });
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) =>
      command === "automation_queued_playback_accept" || command === "automation_queued_playback_update"
        ? Promise.resolve({ accepted: true }) : original(command, args));
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 107, payload: {
      operationId: "delayed-pause-owner", requestId: "delayed-pause-rpc", trackIds: [savedTrack.id],
    } });
    await settle(); await settle();
    await automationAction("pause-before-seek", "pause");
    await automationAction("seek-before-resume", "seek", { position: 20 });
    const resumeHandler = eventHandlers.get("automation-playback")!;
    resumeHandler({ event: "automation-playback", id: 108, payload: { requestId: "resume-after-seek", action: "resume" } });
    await settle();
    expect(finishResume).toBeDefined();
    // A pause event queued by pause-before-seek arrives while the newer
    // owner-bound play() promise is pending.
    document.querySelector("audio")!.dispatchEvent(new Event("pause"));
    await settle();
    finishResume();
    await settle(); await settle();
    const updates = mocks.call.mock.calls.filter(([command]) => command === "automation_queued_playback_update")
      .map(([, args]) => args as { operationId: string; status: string });
    expect(updates.every(update => update.operationId === "delayed-pause-owner")).toBe(true);
    expect(updates.some(update => update.status === "playing")).toBe(true);
    expect(updates.some(update => ["cancelled", "superseded", "failed"].includes(update.status))).toBe(false);
    expect(play).toHaveBeenCalledTimes(2);
  });

  it.each(["missing", "failed"])("queued receipt preserves established ownership and cancellation when new track loading is %s", async mode => {
    previewMode="direct";savedChartTracks=[savedTrack];
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const pause=vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command,args)=>{
      if(command==="get_tracks"&&(args?.ids as string[]).includes("invalid-new-track")){
        if(mode==="failed")return Promise.reject(new Error("Track lookup unavailable"));
        return Promise.resolve([]);
      }
      if(command==="automation_queued_playback_accept"||command==="automation_queued_playback_update")return Promise.resolve({accepted:true});
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-queued-playback")!({event:"automation-queued-playback",id:104,payload:{operationId:"valid-old-owner",requestId:"valid-old-rpc",trackIds:[savedTrack.id]}});await settle();await settle();
    eventHandlers.get("automation-queued-playback")!({event:"automation-queued-playback",id:105,payload:{operationId:"invalid-new-owner",requestId:"invalid-new-rpc",trackIds:["invalid-new-track"]}});await settle();await settle();
    expect(mocks.call.mock.calls.filter(([command])=>command==="automation_queued_playback_accept")).toHaveLength(1);
    const result=await automationAction("inspect-preserved-queue","volume",{volume:.5});expect(result.playback.queue).toEqual([expect.objectContaining({id:savedTrack.id})]);
    const oldUpdates=mocks.call.mock.calls.filter(([command,args])=>command==="automation_queued_playback_update"&&(args as {operationId:string}).operationId==="valid-old-owner");
    expect(oldUpdates.some(([,args])=>["superseded","cancelled"].includes((args as {status:string}).status))).toBe(false);
    const pauses=pause.mock.calls.length;
    eventHandlers.get("automation-queued-playback-cancelled")!({event:"automation-queued-playback-cancelled",id:106,payload:{operationId:"valid-old-owner"}});await settle();
    expect(pause.mock.calls.length).toBeGreaterThan(pauses);expect(mocks.call).toHaveBeenCalledWith("automation_queued_playback_update",expect.objectContaining({operationId:"valid-old-owner",status:"cancelled"}));
  });
  it("queued publication acknowledges an immediate full queue and failure history before any timer pulse", async () => {
    vi.useFakeTimers();
    previewMode = "fallback";
    delayPlaybackTrackId = savedTrack.id;
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "automation_queued_playback_accept") {
        const packet = args?.playback as Record<string, unknown>;
        expect(packet.queueUpdate).toEqual({ kind: "full" });
        expect(packet.failureHistoryUpdate).toEqual({ kind: "full" });
        expect(packet.queue).toEqual([expect.objectContaining({ id: savedTrack.id })]);
        expect(packet.currentIndex).toBe(0);
        expect(packet.trackId).toBe(savedTrack.id);
        return Promise.resolve({
          accepted: true,
          playbackAck: { protocolVersion: 1, accepted: true, needFull: false, failureHistoryRevision: packet.failureHistoryRevision },
        });
      }
      if (command === "automation_queued_playback_update") return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 201, payload: {
      operationId: "publication-owner", requestId: "publication-rpc", trackIds: [savedTrack.id],
    } });
    await settle(); await settle();
    const acceptance = lastCallOf("automation_queued_playback_accept")![1] as {
      receipt: { queueEntryIds: string[] }; playback: { entryId: string; queueEntryIds: string[] };
    };
    expect(acceptance.playback.queueEntryIds).toEqual(acceptance.receipt.queueEntryIds);
    expect(acceptance.playback.entryId).toBe(acceptance.receipt.queueEntryIds[0]);
    expect(finishPlaybackResolve).toBeDefined();
    await vi.advanceTimersByTimeAsync(1000); await settle();
    const pulse = (lastCallOf("automation_playback_state")![1] as { playback: Record<string, unknown> }).playback;
    expect(pulse.queueUpdate).toMatchObject({ kind: "unchanged" });
    expect(pulse.failureHistoryUpdate).toMatchObject({ kind: "unchanged" });
  });

  it("queued publication retries a stale full snapshot using a newer sequence and full history", async () => {
    previewMode = "fallback";
    delayPlaybackTrackId = savedTrack.id;
    const packets: Array<Record<string, unknown>> = [];
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "automation_queued_playback_accept") {
        const packet = args?.playback as Record<string, unknown>;
        packets.push(packet);
        if (packets.length === 1) return Promise.resolve({ accepted: false, retryable: true, playbackAck: { protocolVersion: 1, accepted: false, needFull: false } });
        return Promise.resolve({ accepted: true, playbackAck: { protocolVersion: 1, accepted: true, failureHistoryRevision: packet.failureHistoryRevision } });
      }
      if (command === "automation_queued_playback_update") return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 202, payload: {
      operationId: "retry-publication", requestId: "retry-publication-rpc", trackIds: [savedTrack.id],
    } });
    await settle(); await settle();
    expect(packets).toHaveLength(2);
    expect(Number(packets[1]!.stateSequence)).toBeGreaterThan(Number(packets[0]!.stateSequence));
    expect(packets.every(packet => (packet.queueUpdate as { kind: string }).kind === "full" &&
      (packet.failureHistoryUpdate as { kind: string }).kind === "full")).toBe(true);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "automation_queued_playback_update" &&
      (args as { status: string }).status === "cancelled")).toBe(false);
  });

  it("queued publication bounds rejected snapshot retries and cancels only its unaccepted owner", async () => {
    previewMode = "fallback";
    delayPlaybackTrackId = savedTrack.id;
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "automation_queued_playback_accept") return Promise.resolve({ accepted: false, retryable: true, playbackAck: { protocolVersion: 1, accepted: false, needFull: true } });
      if (command === "automation_queued_playback_update") return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 203, payload: {
      operationId: "rejected-publication", requestId: "rejected-publication-rpc", trackIds: [savedTrack.id],
    } });
    await settle(); await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "automation_queued_playback_accept")).toHaveLength(3);
    expect(mocks.call).toHaveBeenCalledWith("automation_queued_playback_update", expect.objectContaining({ operationId: "rejected-publication", status: "cancelled" }));
  });

  it("queued publication cancellation during an ACK wait cannot stop replacement playback", async () => {
    previewMode = "fallback";
    savedChartTracks = [savedTrack, secondTrack];
    delayPlaybackTrackId = savedTrack.id;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    let releaseAck!: (value: { accepted: boolean; retryable: boolean }) => void;
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "automation_queued_playback_accept") return new Promise(resolve => { releaseAck = resolve; });
      if (command === "automation_queued_playback_update") return Promise.resolve({ accepted: true });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 204, payload: {
      operationId: "pending-publication", requestId: "pending-publication-rpc", trackIds: [savedTrack.id],
    } });
    await settle(); await settle();
    expect(releaseAck).toBeDefined();
    eventHandlers.get("automation-queued-playback-cancelled")!({ event: "automation-queued-playback-cancelled", id: 205, payload: { operationId: "pending-publication" } });
    await automationAction("publication-replacement", "play", { trackIds: [secondTrack.id] });
    const pauseCount = pause.mock.calls.length;
    releaseAck({ accepted: false, retryable: true });
    await settle(); await settle();
    expect(pause).toHaveBeenCalledTimes(pauseCount);
    expect(mocks.call.mock.calls.filter(([command]) => command === "automation_queued_playback_accept")).toHaveLength(1);
    expect((lastCallOf("automation_playback_complete")![1] as { playback: { trackId: string } }).playback.trackId).toBe(secondTrack.id);
  });

  it("queued publication waits for the selected occurrence after old preview teardown and cancels that wait safely", async () => {
    savedChartTracks = [savedTrack, secondTrack];
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    let releaseStop!: () => void;
    const stopping = new Promise<void>(resolve => { releaseStop = resolve; });
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "resolve_preview" && (args?.track as Track)?.id === savedTrack.id) {
        return Promise.resolve({ kind: "audio", url: "/old-session.mp3", sessionId: "old-session", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false });
      }
      if (command === "stop_candidate_preview") return stopping;
      if (command === "automation_queued_playback_update") return Promise.resolve({ accepted: true });
      if (command === "automation_queued_playback_status") return Promise.resolve({ status: "cancelled" });
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await automationAction("old-session-start", "play", { trackIds: [savedTrack.id] });
    eventHandlers.get("automation-queued-playback")!({ event: "automation-queued-playback", id: 206, payload: {
      operationId: "anchor-wait", requestId: "anchor-wait-rpc", trackIds: [secondTrack.id],
    } });
    await settle(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("stop_candidate_preview", { sessionId: "old-session" });
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_queued_playback_accept")).toBe(false);
    eventHandlers.get("automation-queued-playback-cancelled")!({ event: "automation-queued-playback-cancelled", id: 207, payload: { operationId: "anchor-wait" } });
    releaseStop(); await settle(); await settle();
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_queued_playback_accept")).toBe(false);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "resolve_preview" && (args?.track as Track)?.id === secondTrack.id)).toBe(false);
  });

});

describe("remote occurrence save wiring", () => {
  it("wires selected repeated YouTube playlist occurrences into a local append in source order", async () => {
    const original = mocks.call.getMockImplementation()!;
    const playlistId = "PLfixture123";
    const playlistUrl = "https://www.youtube.com/playlist?list=PLfixture123";
    const first = { ...savedTrack, id: "remote-repeat", provider: "youtube", providerId: "same-recording", title: "Repeated", provenance: { remoteEntryId: "entry-first", position: 1 } };
    const middle = { ...secondTrack, id: "remote-middle", provider: "youtube", providerId: "middle-recording", title: "Middle", provenance: { remoteEntryId: "entry-middle", position: 2 } };
    const last = { ...first, sourceUrl: "https://www.youtube.com/watch?v=same-recording", provenance: { remoteEntryId: "entry-last", position: 3 } };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_sidebar_layout") return { sections: [], assignments: {}, showAccountPlaylists: {} };
      if (command === "list_remote_playlists" && args?.provider === "youtube") return { items: [{ id: playlistId, title: "Repeated remote playlist", url: playlistUrl }], nextCursor: null };
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "youtube", name: "YouTube", kinds: ["track", "playlist"], enabled: true },
      ];
      if (command === "search_source" && (args?.sourceIds as string[] | undefined)?.[0] === "youtube") return {
        sourceId: "youtube", complete: true,
        results: [{ id: playlistId, kind: "playlist", sourceId: "youtube", title: "Repeated remote playlist", url: playlistUrl }],
      };
      if (command === "browse_remote_playlist") return { provider: "youtube", playlistId, title: "Repeated remote playlist", tracks: [first, middle, last], total: 3, complete: true };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    sourceGroup("youtube").open = true; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search YouTube playlists"]')!.click(); await settle();
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Repeated remote playlist"; query.dispatchEvent(new Event("input", { bubbles: true })); await settle(); submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Open playlist: Repeated remote playlist"]')!.click(); await settle();
    const detail = document.querySelector(".remote-playlist-detail")!;
    detail.querySelector<HTMLButtonElement>(".start-selection")!.click(); await settle();
    const duplicateChecks = [...detail.querySelectorAll<HTMLInputElement>('[aria-label="Select Repeated for playlist"]')];
    expect(duplicateChecks).toHaveLength(2);
    duplicateChecks[1]!.click(); duplicateChecks[0]!.click(); await settle();
    detail.querySelector<HTMLButtonElement>(".add-selected")!.click(); await settle();
    const destination = document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    destination.value = "collection-empty"; destination.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button => button.textContent?.trim() === "Add to playlist")!.click();
    await settle();
    const append = mocks.call.mock.calls.find(([command]) => command === "append_playlist_entries");
    expect(append?.[1]).toMatchObject({ collectionId: "collection-empty", tracks: [first, last] });
    expect(((append?.[1] as { tracks: (typeof first)[] }).tracks).map(track => track.provenance?.remoteEntryId)).toEqual(["entry-first", "entry-last"]);
    expect(mocks.call.mock.calls.some(([command]) => ["playback_command", "resolve_preview", "resolve_playback_track", "play_tracks", "play_tracks_queued", "automation_queued_playback", "add_remote_playlist_tracks", "write_remote_playlist"].includes(String(command)))).toBe(false);
  });

  it("records a nested Spotify album before mutation so toolbar back and forward restore artist and release state", async () => {
    const original = mocks.call.getMockImplementation()!;
    const artistId = "1234567890123456789012";
    const album = { id: "history-album", title: "History Album", url: "https://open.spotify.com/album/history-album", artists: ["History Artist"], totalTracks: 1 };
    const row = { ...savedTrack, id: "history-album-track", title: "History Track", provider: "spotify", durationSeconds: 180 };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 1 }, spotifyAccountId: "history-fixture" };
      if (command === "browse_remote_artist") return { artist: { id: artistId, name: "History Artist", url: `https://open.spotify.com/artist/${artistId}`, genres: [] }, albums: [album], totalAlbums: 1 };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId, tracks: [], total: 0 };
      if (command === "browse_remote_album_tracks") return { album, tracks: [row], total: 1, complete: true };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 1, payload: { requestId: "history-artist", target: { kind: "spotify_artist", spotifyArtistId: artistId } } });
    await settle(); await settle();
    const releaseSort = document.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!;
    releaseSort.value = "title"; releaseSort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const artistPane = document.querySelector<HTMLElement>(".spotify-artist")!;
    artistPane.scrollTop = 275; artistPane.dispatchEvent(new Event("scroll"));
    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("History Album");
    expect([...document.querySelectorAll(".spotify-artist button")].some(button => /^(← Back|Back to releases)/.test(button.textContent ?? ""))).toBe(false);
    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("History Artist");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("title");
    expect(document.querySelector<HTMLElement>(".spotify-artist")?.scrollTop).toBe(275);
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle(); await settle();
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe("History Album");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.value).toBe("title");
    expect(document.querySelector(".spotify-artist")?.textContent).toContain("History Track");
    expect(mocks.call.mock.calls.filter(([command]) => command === "browse_remote_album_tracks")).toHaveLength(1);
  });

  it("wires selected duplicate album occurrences into a local append without collapsing positions", async () => {
    const { providerNavigationSnapshots } = await import("./lib/provider-navigation-snapshots");
    providerNavigationSnapshots.clear();
    const original = mocks.call.getMockImplementation()!;
    const artistId = "1234567890123456789012";
    const album = { id: "duplicate-album-app", title: "Duplicate Album", url: "https://open.spotify.com/album/duplicate-album-app", artists: ["Album Artist"], totalTracks: 3 };
    const occurrence = (title: string, position: number) => ({
      ...savedTrack, id: "album-same-recording", provider: "spotify", providerId: "same-recording",
      title, artist: "Album Artist", sourceUrl: "https://open.spotify.com/track/same-recording",
      provenance: { remoteEntryId: `album:${position}`, position, positionScope: "album" },
    });
    const first = occurrence("One", 1), middle = occurrence("Middle", 2), last = occurrence("Three", 3);
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 1 }, spotifyAccountId: "album-selection-fixture" };
      if (command === "browse_remote_artist") return { artist: { id: artistId, name: "Album Artist", url: `https://open.spotify.com/artist/${artistId}`, genres: [] }, albums: [album], totalAlbums: 1 };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId, tracks: [], total: 0 };
      if (command === "browse_remote_album_tracks") return { album, tracks: [first, middle, last], total: 3, complete: true };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 1, payload: { requestId: "open-duplicate-album", target: { kind: "spotify_artist", spotifyArtistId: artistId } } });
    await settle(); await settle();
    document.querySelector<HTMLButtonElement>(".spotify-artist .album")!.click(); await settle();
    const artist = document.querySelector(".spotify-artist")!;
    artist.querySelector<HTMLButtonElement>(".start-selection")!.click(); await settle();
    const duplicateChecks = [...artist.querySelectorAll<HTMLInputElement>('[aria-label^="Select "]')];
    expect(duplicateChecks).toHaveLength(3);
    duplicateChecks[2]!.click(); duplicateChecks[0]!.click(); await settle();
    artist.querySelector<HTMLButtonElement>(".add-selected")!.click(); await settle();
    const destination = document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    destination.value = "collection-empty"; destination.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button => button.textContent?.trim() === "Add to playlist")!.click();
    await settle();
    const append = mocks.call.mock.calls.find(([command]) => command === "append_playlist_entries");
    expect(append?.[1]).toMatchObject({ collectionId: "collection-empty", tracks: [first, last] });
    expect(((append?.[1] as { tracks: (typeof first)[] }).tracks).map(track => track.provenance?.position)).toEqual([1, 3]);
    expect(((append?.[1] as { tracks: (typeof first)[] }).tracks).map(track => track.provenance?.remoteEntryId)).toEqual(["album:1", "album:3"]);
    expect(mocks.call.mock.calls.some(([command]) => ["playback_command", "resolve_preview", "resolve_playback_track", "play_tracks", "play_tracks_queued", "automation_queued_playback", "add_remote_playlist_tracks", "write_remote_playlist"].includes(String(command)))).toBe(false);
  });

});
