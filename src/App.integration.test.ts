// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount as svelteUnmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserImportResult, Candidate, Chart, Collection, DownloadJob, LocalAsset, NativePlaybackSnapshot, PlaylistMirrorOptions, PlaylistMirrorOptionsView, Preview, Settings, Track } from "./lib/types";
import type { ArtistReleaseRef } from "./lib/artist-types";
import type { TrackSortAvailability } from "./lib/track-sort-availability";
import { streamAudioVariantIdentity, streamSourceIdentitySha256 } from "./lib/stream-media-analysis";
import { prepareObservedWaveformBinding } from "./lib/observed-waveform-cache";
import { LIVE_MEASUREMENT_CACHE_NAMESPACE, liveMeasurementCacheKey, type LiveMeasurementCacheRecord } from "./lib/live-measurement-cache";

const mocks = vi.hoisted(() => ({ call: vi.fn(), listen: vi.fn(), captureRadioPcm: vi.fn(), isDesktop: true }));
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

const nativeRadioSongHash = "a".repeat(64);
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
function nativeHandoffSnapshot(args?: Record<string, unknown>): NativePlaybackSnapshot {
  const request = args as {handoffId?:string;tracks?:Track[];entryIds?:string[];index?:number;candidate?:Candidate;targetPosition?:number;paused?:boolean;volume?:number;repeat?:string;shuffle?:boolean} | undefined;
  const tracks = request?.tracks ?? [];
  const index = request?.index ?? -1;
  const track = tracks[index] ?? null;
  return makeNativeSnapshot({
    handoffId:request?.handoffId,
    engine:"native", revision:2, generation:2, status:request?.paused ? "paused" : "resolving",
    trackId:track?.id ?? null, entryId:request?.entryIds?.[index] ?? null, track, candidate:request?.candidate ?? null,
    queue:tracks, queueEntryIds:request?.entryIds ? [...request.entryIds] : [], queueRevision:1, currentIndex:index,
    paused:request?.paused ?? true, position:request?.targetPosition ?? 0,
    duration:track?.durationSeconds ?? 0, volume:request?.volume ?? .8, canSeek:true,
    repeat:request?.repeat ?? "off", shuffle:request?.shuffle ?? false,
    playerMessage:request?.paused ? "Paused" : "Finding audio…",
  });
}
const eventHandlers = new Map<string, (event: { event: string; id: number; payload: unknown }) => void>();
const pendingSvelteUnmounts: Promise<void>[] = [];
const nativeCurrentTimeDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "currentTime");
const mockAudioTimes = new WeakMap<HTMLMediaElement, number>();
const mockAudioPlaying = new WeakMap<HTMLMediaElement, boolean>();
const mockAudioPlayingListeners = new WeakSet<HTMLMediaElement>();
let pendingMockStartupClocks = 0;
function playAndAdvanceMediaClock(this: HTMLMediaElement): Promise<void> {
  const currentTimeDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "currentTime");
  if (vi.isMockFunction(currentTimeDescriptor?.set)) {
    // Preserve test-owned time storage and setter assertions while simulating
    // the short initial decode interval required by the startup gate.
    this.currentTime = Number(this.currentTime) + 0.3;
  } else if (!vi.isMockFunction(currentTimeDescriptor?.get)) {
    const readNativeTime = () => nativeCurrentTimeDescriptor?.get?.call(this) ?? 0;
    if (!Object.getOwnPropertyDescriptor(this, "currentTime")) mockAudioTimes.set(this, Number(readNativeTime()) || 0);
    Object.defineProperty(this, "currentTime", {
      configurable: true,
      get: () => mockAudioTimes.get(this) ?? 0,
      set: (value: number) => mockAudioTimes.set(this, Number(value)),
    });
    mockAudioTimes.set(this, (mockAudioTimes.get(this) ?? 0) + 0.3);
  }
  const pausedDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "paused");
  if (!vi.isMockFunction(pausedDescriptor?.get)) {
    mockAudioPlaying.set(this, true);
    if (!mockAudioPlayingListeners.has(this)) {
      this.addEventListener("pause", () => mockAudioPlaying.set(this, false), true);
      this.addEventListener("play", () => mockAudioPlaying.set(this, true), true);
      this.addEventListener("ended", () => mockAudioPlaying.set(this, false), true);
      mockAudioPlayingListeners.add(this);
    }
    Object.defineProperty(this, "paused", { configurable: true, get: () => !mockAudioPlaying.get(this) });
  }
  const readyStateDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "readyState");
  if (!vi.isMockFunction(readyStateDescriptor?.get)) Object.defineProperty(this, "readyState", { configurable: true, get: () => 4 });
  const seekingDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "seeking");
  if (!vi.isMockFunction(seekingDescriptor?.get)) Object.defineProperty(this, "seeking", { configurable: true, get: () => false });
  pendingMockStartupClocks++;
  setTimeout(() => { pendingMockStartupClocks = Math.max(0, pendingMockStartupClocks - 1); }, 25);
  return Promise.resolve();
}
function unmount(instance: Parameters<typeof svelteUnmount>[0]): Promise<void> {
  const pending = svelteUnmount(instance);
  pendingSvelteUnmounts.push(pending);
  return pending;
}
let failNextCollectionLoad = false;
let unmountApp: (() => void) | undefined;
let pendingBackupPlaybackRestore: Record<string, unknown> | null = null;
let backupPlaybackRestoreAckCalls: { nonce: string; receiptSessionId: string }[] = [];
let failNextBackupPlaybackAck = false;
const backupPlaybackTestSessionId = "20000000-0000-4000-8000-000000000001";
let holdStartupCalls = false;
let delayedBrowserImports: Promise<BrowserImportResult[]> | undefined;
let directBrowserImport: BrowserImportResult | null = null;
let directBrowserImportFailure = "";
let previewMode: "idle" | "fallback" | "race" | "direct" | "alternates" = "idle";
let directPreviewExcerpt = true;
let directPreviewDurationSeconds: number | undefined;
let playbackResolverFailure = "";
let playbackResolverCandidate: Candidate | null | undefined;
let playbackResolverCandidates: (Candidate | Error)[] = [];
let playbackResolverResultOverride: unknown | ((args?:Record<string,unknown>)=>unknown) = undefined;
let delayPlaybackTrackId: string | undefined;
let delayedPlaybackRequestGeneration: number | undefined;
let finishPlaybackResolve: ((value: unknown) => void) | undefined;
let afterPlaybackResolver: (() => void) | undefined;
let afterPreviewResolve: (() => void) | undefined;
let finishOldSearch: ((value: Candidate[]) => void) | undefined;
let finishAudioResolve: ((value: Preview) => void) | undefined;
let delayAudioCandidateId: string | undefined;
let candidateAudioFailure = "";
let finishCandidateInspect: ((value: Candidate) => void) | undefined;
let delayCandidateInspectId: string | undefined;
let delayedCandidateProviders = new Map<string, (value: Candidate[]) => void>();
let holdDownloadMatchPreparation = false;
let finishDownloadMatchPreparation: (() => void) | undefined;
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
let batchDowngradedCandidateIds = new Set<string>();
let batchCandidateOverrides = new Map<string, Candidate[] | Error>();
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
let configuredPlaylistMirroringEnabled: boolean | undefined = undefined;
let playlistMirrorOptionsById: Record<string, PlaylistMirrorOptions> = {};
let configuredPreferredDownloadSource = "youtube";
let configuredPreferredStreamingSource = "youtube";
let configuredPauseOnSourceOpen = false;
let configuredSourceOpenAtPosition = false;
let identifyPlayingTrackResult: { trackId: string; title?: string | null; artist?: string | null; note?: string } = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist" };
let holdNativeIdentify = false;
let finishNativeIdentify: ((value: typeof identifyPlayingTrackResult) => void) | undefined;
let playingTrackDetectionMock: { resourceKey:string; enabled: boolean; observations: { title:string; artist:string; detectedAtSeconds:number; capturedStartSeconds:number; capturedEndSeconds:number; observedAt:string }[] } = { resourceKey:"a".repeat(64), enabled:false, observations:[] };
let savedTrackDetectionMock: typeof playingTrackDetectionMock = { resourceKey:"b".repeat(64), enabled:true, observations:[] };
let holdPlayingTrackDetection = false;
let finishPlayingTrackDetection: ((value: typeof playingTrackDetectionMock) => void) | undefined;
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
type MockSelectedAnalysisJob = { jobId: string; state: string; total: number; completed: number; results: { assetId: string; recordingId: string; status: string; analysis: Record<string, unknown> | null; error?: string | null }[] };
let selectedAnalysisJobMock: MockSelectedAnalysisJob | null = null;
let selectedAnalysisInitialState = "completed";
let selectedAnalysisApplyCalls: { jobId: string; assetId: string; selection: Record<string, unknown> }[] = [];
type MockSelectedStreamJob = { jobId: string; state: string; total: number; completed: number; results: { trackId: string; recordingId: string; status: string; provider: string | null; sourceId: string | null; analysis: Record<string, unknown> | null; error?: string | null }[] };
let selectedStreamAnalysisJobMock: MockSelectedStreamJob | null = null;
let selectedStreamAnalysisApplyCalls: { jobId: string; trackId: string; selection: Record<string, unknown> }[] = [];
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
let explicitNativeFixture = false;
let nativeHandoffDelay = false;
let nativeHandoffEventBeforeResponse = false;
let finishNativeHandoff: (() => void) | undefined;
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
function handoffYoutubeTrack(): Track {
  return { ...savedTrack, id:"youtube-set", provider:"youtube", providerId:"abcdefghijk", sourceUrl:"https://www.youtube.com/watch?v=abcdefghijk", durationSeconds:5_400 };
}
function handoffYoutubeCandidate(track: Track): Candidate {
  return { ...youtubeCandidate(track), id:`youtube:${track.providerId}`, url:track.sourceUrl, durationSeconds:track.durationSeconds };
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
  for (let pass = 0; pass < 2; pass++) {
    for (let i = 0; i < 12; i++) await Promise.resolve();
    if (pendingMockStartupClocks > 0) {
      if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(25);
      else await new Promise((resolve) => setTimeout(resolve, 30));
    } else if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(0);
    else await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
  }
  // Lazy route imports expose their loading status in the DOM. Wait only when
  // a lazy view is actually mounting, so route assertions observe its content.
  const lazyViewDeadline = Date.now() + 3000;
  while (document.querySelector(".lazy-view-status") && Date.now() < lazyViewDeadline) {
    await Promise.resolve();
    if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(0);
    else await new Promise(resolve => setTimeout(resolve, 10));
    await tick();
  }
}
async function openDownloadActivityFromLibrary() {
  const downloads = [...document.querySelectorAll<HTMLButtonElement>(".built-in-playlists .playlist-card")]
    .find((button) => button.textContent?.includes("Downloads"));
  expect(downloads, "Downloads remains available as a library playlist").toBeDefined();
  downloads!.click();
  await settle();
  const activity = document.querySelector<HTMLButtonElement>('[aria-label="Open download activity"]');
  expect(activity, "The Downloads playlist exposes job activity").not.toBeNull();
  activity!.click();
  await settle();
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
  const urlChoice = [...document.querySelectorAll<HTMLButtonElement>(".import-hub button")]
    .find(button => button.textContent?.includes("Import a playlist or album link"));
  if (urlChoice) {
    urlChoice.click();
    await settle();
  }
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
  collections.find(item => item.id === "collection-full")!.trackCount = 131;
  document.body.innerHTML = "";
  pendingMockStartupClocks = 0;
  vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(playAndAdvanceMediaClock);
  const localStorageValues = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => localStorageValues.get(key) ?? null,
    setItem: (key: string, value: string) => localStorageValues.set(key, value),
    removeItem: (key: string) => localStorageValues.delete(key),
  });
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  Object.defineProperty(HTMLDialogElement.prototype, "show", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value: function(this: HTMLDialogElement) { this.removeAttribute("open"); this.dispatchEvent(new Event("close")); } });
  // A frozen test frame may execute immediately, but callbacks scheduled from
  // that callback belong to a future frame rather than recursively executing.
  let runningAnimationFrame = false;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    if (!runningAnimationFrame) {
      runningAnimationFrame = true;
      try { callback(0); } finally { runningAnimationFrame = false; }
    }
    return 1;
  });
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
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
  directPreviewDurationSeconds = undefined;
  playbackResolverFailure = "";
  playbackResolverCandidate = undefined;
  playbackResolverCandidates = [];
  playbackResolverResultOverride = undefined;
  delayPlaybackTrackId = undefined;
  delayedPlaybackRequestGeneration = undefined;
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
  holdDownloadMatchPreparation = false;
  finishDownloadMatchPreparation = undefined;
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
  batchDowngradedCandidateIds = new Set();
  batchCandidateOverrides = new Map();
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
  configuredPlaylistMirroringEnabled = undefined;
  playlistMirrorOptionsById = {};
  configuredPreferredDownloadSource = "youtube";
  configuredPreferredStreamingSource = "youtube";
  configuredPauseOnSourceOpen = false;
  configuredSourceOpenAtPosition = false;
  identifyPlayingTrackResult = { trackId: savedTrack.id, title: "Saved Recording", artist: "Visible Artist" };
  holdNativeIdentify = false;
  finishNativeIdentify = undefined;
  playingTrackDetectionMock = { resourceKey:"a".repeat(64), enabled:false, observations:[] };
  savedTrackDetectionMock = { resourceKey:"b".repeat(64), enabled:true, observations:[] };
  holdPlayingTrackDetection = false;
  finishPlayingTrackDetection = undefined;
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
  selectedAnalysisJobMock = null;
  selectedAnalysisInitialState = "completed";
  selectedAnalysisApplyCalls = [];
  selectedStreamAnalysisJobMock = null;
  selectedStreamAnalysisApplyCalls = [];
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
  nativePlaybackSnapshot = null; explicitNativeFixture = false;
  pendingBackupPlaybackRestore = null;
  backupPlaybackRestoreAckCalls = [];
  failNextBackupPlaybackAck = false;
  nativeHandoffDelay = false;
  nativeHandoffEventBeforeResponse = false;
  finishNativeHandoff = undefined;
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
      case "migrate_native_playback": {
        if (nativePlaybackSnapshot) return nativePlaybackSnapshot;
        if (!args?.legacy) return {migrationRequired:true};
        const legacy = args.legacy as {tracks:Track[];entryIds:string[];index:number;position:number;volume:number;repeat:string;shuffle:boolean;queueLoudnessContexts?:NativePlaybackSnapshot["queueLoudnessContexts"]};
        const track = legacy.tracks[legacy.index] ?? null;
        nativePlaybackSnapshot = makeNativeSnapshot({queue:legacy.tracks,queueEntryIds:legacy.entryIds,queueRevision:0,currentIndex:legacy.index,
          track,trackId:track?.id??null,entryId:legacy.entryIds[legacy.index]??null,status:track?"paused":"idle",position:legacy.position,volume:legacy.volume,repeat:legacy.repeat,shuffle:legacy.shuffle,queueLoudnessContexts:legacy.queueLoudnessContexts});
        return nativePlaybackSnapshot;
      }
      case "get_playback_snapshot": return nativePlaybackSnapshot ?? makeNativeSnapshot();
      case "get_backup_frontend_preferences": return null;
      case "ack_backup_frontend_preferences": return "acknowledged";
      case "get_backup_playback_restore": return pendingBackupPlaybackRestore
        ? { ...pendingBackupPlaybackRestore, currentSessionId: backupPlaybackTestSessionId }
        : null;
      case "ack_backup_playback_restore": {
        const nonce = String(args?.nonce ?? "");
        const receiptSessionId = String(args?.receiptSessionId ?? "");
        backupPlaybackRestoreAckCalls.push({ nonce, receiptSessionId });
        if (failNextBackupPlaybackAck) { failNextBackupPlaybackAck = false; throw new Error("stale nonce"); }
        if (pendingBackupPlaybackRestore?.nonce !== nonce) return "stale";
        if (receiptSessionId === backupPlaybackTestSessionId) return "deferred";
        pendingBackupPlaybackRestore = null;
        return "acknowledged";
      }
      case "get_restore_work_policy": return { reviewRequired: false, categories: [] };
      case "check_dependencies": if (deferDependencyCheck) return new Promise<void>((resolve) => { finishDependencyCheck = () => resolve(); }); return [];
      case "playback_command": {
        const command = args?.command as {action?:string;expectedHandoffId?:string} | undefined;
        if (deferNativePause && command?.action === "pause") return new Promise<void>((resolve) => { finishNativePause = resolve; });
        if (command?.action === "stop" && command.expectedHandoffId && nativePlaybackSnapshot?.handoffId === command.expectedHandoffId) {
          nativePlaybackSnapshot = {...nativePlaybackSnapshot,revision:nativePlaybackSnapshot.revision+1,handoffId:null,status:"idle",trackId:null,track:null,candidate:null,currentIndex:-1,paused:true,position:0,playerMessage:"Stopped"};
          eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:0,payload:nativePlaybackSnapshot});
          return nativePlaybackSnapshot;
        }
        if (explicitNativeFixture) return undefined;
        const action = args?.command as {action:string;tracks?:Track[];index?:number;candidate?:Candidate;position?:number;volume?:number;enabled?:boolean;mode?:string};
        if (!action) return undefined;
        let snapshot = nativePlaybackSnapshot ?? makeNativeSnapshot();
        if (action.action === "play") {
          const queue = action.tracks ?? [];
          const index = action.index ?? 0;
          const track = queue[index] ?? null;
          snapshot = {...snapshot,queue,queueEntryIds:queue.map((_,i)=>`native-${snapshot.revision+1}-${i}`),currentIndex:index,
            track,trackId:track?.id??null,candidate:action.candidate??null,status:"playing",paused:false,position:action.position??0,duration:track?.durationSeconds??240,canSeek:true};
          snapshot.entryId=snapshot.queueEntryIds?.[index];
          if (playbackResolverFailure && track) snapshot={...snapshot,status:"failed",paused:true,playerMessage:playbackResolverFailure,
            diagnostics:{lastSourceFailure:{trackId:track.id,title:track.title,artist:track.artist,reason:playbackResolverFailure}}};
        } else if (action.action === "queue") {
          const added = action.tracks ?? [];
          const queue = action.mode === "replace" ? added : action.mode === "next"
            ? [...snapshot.queue.slice(0,snapshot.currentIndex+1),...added,...snapshot.queue.slice(snapshot.currentIndex+1)] : [...snapshot.queue,...added];
          const index = action.mode === "replace" || snapshot.currentIndex < 0 ? (queue.length ? 0 : -1) : snapshot.currentIndex;
          const track = queue[index] ?? null;
          snapshot={...snapshot,queue,queueEntryIds:queue.map((_,i)=>`queued-${snapshot.revision+1}-${i}`),queueRevision:(snapshot.queueRevision??0)+1,currentIndex:index,track,trackId:track?.id??null};
          snapshot.entryId=snapshot.queueEntryIds?.[index];
        } else if (action.action === "reorder") {
          const move=args?.command as {from:number;to:number};
          const queue=[...snapshot.queue], ids=[...(snapshot.queueEntryIds??[])], current=snapshot.entryId;
          queue.splice(move.to,0,queue.splice(move.from,1)[0]!);ids.splice(move.to,0,ids.splice(move.from,1)[0]!);
          snapshot={...snapshot,queue,queueEntryIds:ids,currentIndex:ids.indexOf(current??""),queueRevision:(snapshot.queueRevision??0)+1};
        } else if(action.action === "pause") snapshot={...snapshot,paused:true,status:"paused"};
        else if(action.action === "resume") snapshot={...snapshot,paused:false,status:"playing"};
        else if(action.action === "volume") snapshot={...snapshot,volume:action.volume??snapshot.volume};
        else if(action.action === "seek") snapshot={...snapshot,position:action.position??0,seekSerial:(snapshot.seekSerial??0)+1};
        else if(action.action === "shuffle") snapshot={...snapshot,shuffle:action.enabled??false};
        else if(action.action === "repeat") snapshot={...snapshot,repeat:action.mode??"off"};
        else return undefined;
        snapshot={...snapshot,revision:snapshot.revision+1,generation:action.action==="play"?snapshot.generation+1:snapshot.generation};
        nativePlaybackSnapshot=snapshot;
        eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:snapshot.revision,payload:snapshot});
        return snapshot;
      }
      case "handoff_webview_playback": {
        const handoffArgs=args?.handoff as Record<string,unknown>|undefined;
        if (nativeHandoffDelay && nativeHandoffEventBeforeResponse) {
          const snapshot=nativeHandoffSnapshot(handoffArgs);nativePlaybackSnapshot=snapshot;
          eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:0,payload:snapshot});
          return new Promise(resolve => { finishNativeHandoff = () => resolve(snapshot); });
        }
        if (nativeHandoffDelay) return new Promise(resolve => { finishNativeHandoff = () => { const snapshot=nativeHandoffSnapshot(handoffArgs); nativePlaybackSnapshot=snapshot; eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:0,payload:snapshot}); resolve(snapshot); }; });
        const snapshot=nativeHandoffSnapshot(handoffArgs);nativePlaybackSnapshot=snapshot;return snapshot;
      }
      case "handoff_resolved_playback_source": {
        const handoff=args?.handoff as Record<string,unknown>|undefined;
        const tracks=(handoff?.tracks as Track[]|undefined) ?? [];
        const index=Number(handoff?.index ?? -1);
        const track=tracks[index] ?? null;
        const snapshot=makeNativeSnapshot({engine:"native",revision:3,generation:Number(handoff?.sourceGeneration ?? 0),
          status:handoff?.paused ? "paused" : "resolving",trackId:track?.id ?? null,track,
          queue:tracks,queueRevision:1,currentIndex:index,paused:Boolean(handoff?.paused),
          position:Number(handoff?.targetPosition ?? 0),duration:track?.durationSeconds ?? 0,canSeek:true,
          handoffId:String(handoff?.handoffId ?? "resolved-source-handoff")});
        nativePlaybackSnapshot=snapshot;
        eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:0,payload:snapshot});
        return snapshot;
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
        return { musicDirectory: configuredMusicDirectory, ...(configuredPlaylistMirroringEnabled === undefined ? {} : { playlistMirroringEnabled: configuredPlaylistMirroringEnabled }), preferredDownloadSource: configuredPreferredDownloadSource, preferredStreamingSource: configuredPreferredStreamingSource, overridePlaylistTrackUrls, streamingFallbackEnabled, streamingFallbackSources, streamingFallbackPolicyVersion: 1, bandcampFallbackEnabled: streamingFallbackSources.includes("bandcamp"), downloadFallbackEnabled, downloadFallbackSources, pauseOnSourceOpen: configuredPauseOnSourceOpen, sourceOpenAtPosition: configuredSourceOpenAtPosition, playbackDiagnosticsEnabled: false, autoPreview: false, previewDelayMs: 250, theme: "dark", ytDlpPath: "yt-dlp", ffmpegPath: "ffmpeg", ffprobePath: "ffprobe", jsRuntimePath: "deno" };
      case "get_release_setup": return { setupComplete: true, musicDirectory: configuredMusicDirectory, radioRecognition: true, mcpCommand: [], browser: { browsers: [] }, toolPaths: [] };
      case "get_playlist_mirror_options": {
        const collectionId = String(args?.collectionId ?? "");
        const options = playlistMirrorOptionsById[collectionId] ?? { included: true, includeMediaLinks: null, includeYtdlpSearch: null };
        const defaults = { includeMediaLinks: true, includeYtdlpSearch: false };
        return { collectionId, options, defaults, effective: { included: options.included, includeMediaLinks: options.includeMediaLinks ?? defaults.includeMediaLinks, includeYtdlpSearch: options.includeYtdlpSearch ?? defaults.includeYtdlpSearch }, globallyEnabled: configuredPlaylistMirroringEnabled !== false } satisfies PlaylistMirrorOptionsView;
      }
      case "set_playlist_mirror_options": {
        const collectionId = String(args?.collectionId ?? "");
        playlistMirrorOptionsById[collectionId] = args?.options as PlaylistMirrorOptions;
        const options = playlistMirrorOptionsById[collectionId];
        const defaults = { includeMediaLinks: true, includeYtdlpSearch: false };
        return { collectionId, options, defaults, effective: { included: options.included, includeMediaLinks: options.includeMediaLinks ?? defaults.includeMediaLinks, includeYtdlpSearch: options.includeYtdlpSearch ?? defaults.includeYtdlpSearch }, globallyEnabled: configuredPlaylistMirroringEnabled !== false } satisfies PlaylistMirrorOptionsView;
      }
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
      case "get_playing_track_detections": return playingTrackDetectionMock;
      case "get_saved_track_detections": return savedTrackDetectionMock;
      case "set_playing_track_detection_enabled": playingTrackDetectionMock={...playingTrackDetectionMock,enabled:Boolean(args?.enabled)}; return playingTrackDetectionMock;
      case "detect_playing_track_segment": return holdPlayingTrackDetection
        ? new Promise(resolve => { finishPlayingTrackDetection = value => { playingTrackDetectionMock=value; resolve(value); }; })
        : playingTrackDetectionMock;
      case "cancel_playing_track_identification": return true;
      case "cancel_webview_identification": return true;
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
      case "preview_collection_import":
        if (directBrowserImportFailure) throw new Error(directBrowserImportFailure);
        if (directBrowserImport) return {...directBrowserImport,previewId:"preview-import",uniqueRecordings:directBrowserImport.importedCount,changes:{added:directBrowserImport.importedCount,removed:0,moved:0,retainedFromEarlierCapture:0,partial:!directBrowserImport.complete},failedSources:[],sample:savedChartTracks.map(t=>({title:t.title,artist:t.artist}))};
        throw new Error("Direct source import unavailable");
      case "apply_collection_import": return directBrowserImport;
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
      case "start_selected_asset_analysis": {
        const assetIds = (args?.assetIds ?? []) as string[];
        const results: MockSelectedAnalysisJob["results"] = selectedAnalysisInitialState === "completed" ? assetIds.flatMap((assetId): MockSelectedAnalysisJob["results"] => {
          const asset = savedAssets.find(item => item.id === assetId);
          if (!asset) return [{ assetId, recordingId: "", status: "failed", analysis: null, error: "Registered local audio asset not found" }];
          return [{ assetId, recordingId: asset.recordingId, status: "completed", analysis: {
            schemaVersion: 1, analyzerVersion: "burrata-whole-track-analysis-v1-bpm-v3-keyfinder-2.2.8",
            resource: { kind: "registeredAsset", assetId, recordingId: asset.recordingId, fileHash: asset.hash },
            durationSeconds: 180, scope: "wholeTrack", coverage: 1, measuredAt: "2026-10-25T12:00:00Z",
            bpm: { bpm: 124, confidence: 0.9, relatedTempoAlternativeBpm: 62 },
            key: { key: "Am", confidence: 0.8, wholeTrackKey: "Am", windowKeys: ["Am", "Am", "Am", "Am", null], agreementCount: 4 },
          } }];
        }) : [];
        selectedAnalysisJobMock = {
          jobId: "selected-analysis-1", state: selectedAnalysisInitialState,
          total: assetIds.length, completed: results.length, results,
        };
        return selectedAnalysisJobMock;
      }
      case "get_selected_asset_analysis": return selectedAnalysisJobMock?.jobId === args?.jobId ? selectedAnalysisJobMock : null;
      case "cancel_selected_asset_analysis": {
        const current = selectedAnalysisJobMock;
        if (current && current.jobId === args?.jobId) selectedAnalysisJobMock = {
          jobId: current.jobId, state: "cancelled", total: current.total,
          completed: current.completed, results: current.results,
        };
        return undefined;
      }
      case "start_selected_stream_analysis": {
        const trackIds = (args?.trackIds ?? []) as string[];
        selectedStreamAnalysisJobMock = {
          jobId: "selected-stream-analysis-1", state: "completed", total: trackIds.length, completed: trackIds.length,
          results: trackIds.map(trackId => ({
            trackId, recordingId: trackId, status: "completed", provider: "youtube", sourceId: "stable-video-1",
            analysis: {
              schemaVersion: 1, analyzerVersion: "burrata-whole-track-analysis-v1-bpm-v3-keyfinder-2.2.8",
              resource: { kind: "stream", recordingId: trackId, provider: "youtube", sourceIdentitySha256: "a".repeat(64) },
              durationSeconds: 180, scope: "wholeTrack", coverage: 1, measuredAt: "2026-10-25T12:00:00Z",
              bpm: { bpm: 124, confidence: 0.9, relatedTempoAlternativeBpm: 62 },
              key: { key: "Am", confidence: 0.8, wholeTrackKey: "Am", windowKeys: ["Am", "Am", "Am", "Am", null], agreementCount: 4 },
            },
          })),
        };
        return selectedStreamAnalysisJobMock;
      }
      case "get_selected_stream_analysis": return selectedStreamAnalysisJobMock?.jobId === args?.jobId ? selectedStreamAnalysisJobMock : null;
      case "cancel_selected_stream_analysis": {
        const current = selectedStreamAnalysisJobMock;
        if (current && current.jobId === args?.jobId) selectedStreamAnalysisJobMock = {
          jobId: current.jobId, state: "cancelled", total: current.total,
          completed: current.completed, results: current.results,
        };
        return undefined;
      }
      case "apply_selected_stream_analysis": {
        const trackId = String(args?.trackId ?? "");
        const selection = (args?.selection ?? {}) as { fields?: string[]; policies?: Record<string, string> };
        const jobId = String(args?.jobId ?? "");
        selectedStreamAnalysisApplyCalls.push({ jobId, trackId, selection: selection as Record<string, unknown> });
        const result = selectedStreamAnalysisJobMock?.results.find(row => row.trackId === trackId)?.analysis as {
          bpm?: { bpm: number } | null; key?: { key: string } | null;
        } | null | undefined;
        const current = savedChartTracks.find(track => track.id === trackId) ?? savedTrack;
        const updated: Track = { ...current, provenance: { ...(current.provenance as Record<string, unknown> ?? {}) } };
        if (selection.fields?.includes("bpm") && result?.bpm && (selection.policies?.bpm === "replace" || updated.bpm == null)) updated.bpm = result.bpm.bpm;
        if (selection.fields?.includes("key") && result?.key && (selection.policies?.key === "replace" || updated.key == null)) updated.key = result.key.key;
        savedChartTracks = savedChartTracks.map(track => track.id === trackId ? updated : track);
        return updated;
      }
      case "apply_selected_asset_analysis": {
        const assetId = String(args?.assetId ?? "");
        const selection = (args?.selection ?? {}) as { fields?: string[]; policies?: Record<string, string> };
        const jobId = String(args?.jobId ?? "");
        selectedAnalysisApplyCalls.push({ jobId, assetId, selection: selection as Record<string, unknown> });
        const result = selectedAnalysisJobMock?.results.find(row => row.assetId === assetId)?.analysis as {
          bpm?: { bpm: number; confidence: number; relatedTempoAlternativeBpm?: number } | null;
          key?: { key: string; confidence: number; wholeTrackKey: string; windowKeys: Array<string | null>; agreementCount: number } | null;
        } | null | undefined;
        const asset = savedAssets.find(item => item.id === assetId);
        if (!asset || !result) throw new Error("Selected analysis result is unavailable");
        const updated: Track = { ...asset.track, provenance: { ...(asset.track.provenance as Record<string, unknown> ?? {}) } };
        if (selection.fields?.includes("bpm") && result.bpm && (selection.policies?.bpm === "replace" || updated.bpm == null)) updated.bpm = result.bpm.bpm;
        if (selection.fields?.includes("key") && result.key && (selection.policies?.key === "replace" || updated.key == null)) updated.key = result.key.key;
        savedAssets = savedAssets.map(item => item.id === assetId ? { ...item, track: updated } : item);
        return updated;
      }
      case "update_track": return args?.track;
      case "search_audio_tracks": return audioSearchResult;
      case "save_settings": {
        const value = args?.settings as Settings;
        configuredMusicDirectory = value.musicDirectory ?? configuredMusicDirectory;
        configuredPlaylistMirroringEnabled = value.playlistMirroringEnabled ?? configuredPlaylistMirroringEnabled;
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
      case "prepare_download_match_identity": {
        if (holdDownloadMatchPreparation) {
          return new Promise<void>((resolve) => { finishDownloadMatchPreparation = resolve; });
        }
        return undefined;
      }
      case "cancel_remote_read": return false;
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
        if (mockCollectionRows && ["collection-full","linked-working"].includes(String(args?.collectionId))) {
          const from=Number(args?.from),to=Number(args?.to),rows=[...mockCollectionRows];
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
      case "assess_download_candidate": {
        const candidate = args?.candidate as Candidate;
        return batchDowngradedCandidateIds.has(candidate.id) ? {action:"enrich",relationship:"unresolved"} : {action:"accept",relationship:"exact"};
      }
      case "inspect_candidate": {
        const candidate = args?.candidate as Candidate;
        if (candidate.id === delayCandidateInspectId) return new Promise((resolve) => { finishCandidateInspect = resolve; });
        return { ...candidate, ...(batchDowngradedCandidateIds.has(candidate.id) ? { confidence: "review" } : batchInspectionHigh ? { confidence: "high" } : {}), formats: [{ id: "audio-best", codec: "Opus", extension: "webm", bitrate: 192, lossless: false }] };
      }
      case "get_track_details": return { ...(args?.track as Track), assetId: preferredAssetChoice || (args?.track as Track | undefined)?.assetId };
      case "set_preferred_track_file": preferredAssetChoice = String(args?.assetId ?? ""); return undefined;
      case "resolve_preview": {
        const track = args?.track as Track;
        if (previewMode === "direct") { const result = { kind: "audio", url: "/cached-preview.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: directPreviewExcerpt, sourceGeneration: webviewSourceGeneration, ...(directPreviewDurationSeconds ? { durationSeconds: directPreviewDurationSeconds } : {}), audioMimeType: "audio/mp4", audioCodec: "mp4a.40.2", audioFormatId: "140", audioFormatHasVideo: false, audioExpectedContentLength: 123456 } satisfies Preview; afterPreviewResolve?.(); return result; }
        if (previewMode === "race" && track.id === savedTrack.id) throw new Error("Cached source unavailable");
        if (previewMode === "fallback") throw new Error("No cached preview");
        return { kind: "external", url: track.sourceUrl, canSeek: false, canVolume: false, canAutoplay: false, isExcerpt: false } satisfies Preview;
      }
      case "resolve_playback_track": {
        const track = args?.track as Track;
        if (track.id === delayPlaybackTrackId) { delayedPlaybackRequestGeneration=Number(args?.requestGeneration); return new Promise((resolve) => { finishPlaybackResolve = resolve; }); }
        if (playbackResolverFailure) throw new Error(playbackResolverFailure);
        if (playbackResolverResultOverride !== undefined) { afterPlaybackResolver?.(); return typeof playbackResolverResultOverride === "function" ? playbackResolverResultOverride(args) : playbackResolverResultOverride; }
        const hinted = (args?.selectedCandidate as Candidate | null | undefined)
          ?? (track.provenance as Record<string, unknown> | undefined)?.audioCandidate as Candidate | undefined;
        const queued = playbackResolverCandidates.shift();
        if (queued instanceof Error) throw queued;
        const candidate = queued ?? (playbackResolverCandidate === undefined ? hinted ?? withChapters(youtubeCandidate(track)) : playbackResolverCandidate);
        afterPlaybackResolver?.();
        return {
          candidate,
          preview: { kind: "audio", url: `/resolved-${candidate?.id ?? track.id}.mp3`, canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false, durationSeconds: track.durationSeconds, provider: candidate?.provider, title: candidate?.title, sourceGeneration: webviewSourceGeneration, ...(args?.normalizationRouteRequired ? { normalizationRoute: "same-origin-relay" as const } : {}), audioMimeType: "audio/mp4", audioCodec: "mp4a.40.2", audioFormatId: "140", audioFormatHasVideo: false, audioExpectedContentLength: 123456 } satisfies Preview,
        };
      }
      case "cancel_playback_resolution": return undefined;
      case "find_candidates":
      case "find_download_candidates": {
        const track = args?.track as Track;
        const requestedProvider = command === "find_download_candidates"
          ? String(args?.providerId ?? "")
          : (args?.providerIds as string[] | undefined)?.[0];
        const providerIds = command === "find_download_candidates"
          ? [requestedProvider]
          : (args?.providerIds as string[] | undefined) ?? [];
        if (requestedProvider && failedCandidateProviders.has(requestedProvider)) throw new Error(`${requestedProvider} unavailable`);
        const batchOverride = requestedProvider ? batchCandidateOverrides.get(`${track.id}:${requestedProvider}`) : undefined;
        if (batchOverride instanceof Error) throw batchOverride;
        if (batchOverride) return batchOverride;
        if (requestedProvider && delayedCandidateProviders.has(requestedProvider)) {
          return new Promise((resolve) => delayedCandidateProviders.set(requestedProvider, resolve as (value: Candidate[]) => void));
        }
        if (providerIds.includes("monochrome")) return monochromeCandidates;
        if (providerIds.includes("slskd")) {
          const candidate = soulseekCandidate(track);
          return [{ ...candidate, confidence: "review", score: 0.85 }];
        }
        if (providerIds.includes("youtube") && providerIds.includes("soundcloud")) {
          return [withChapters(youtubeCandidate(track)), { ...withChapters(youtubeCandidate(track)), id: `soundcloud:${track.id}`, provider: "soundcloud", url: "https://soundcloud.com/visible-artist/saved-recording" }];
        }
        if (providerIds.includes("soundcloud")) {
          return [{ ...withChapters(youtubeCandidate(track)), id: `soundcloud:${track.id}`, provider: "soundcloud",
            url: "https://soundcloud.com/visible-artist/saved-recording" }];
        }
        if (previewMode === "race" && track.id === savedTrack.id) {
          return new Promise((resolve) => { finishOldSearch = resolve; });
        }
        if (previewMode === "alternates" && providerIds.includes("youtube")) {
          return [withChapters(youtubeCandidate(track)), { ...withChapters(youtubeCandidate(track)), id: `youtube-alt:${track.id}`, url: `https://youtube.com/watch?v=${track.id.padEnd(10, "y")}z`, title: `${track.artist} - ${track.title} (Live)` }];
        }
        return [withChapters(youtubeCandidate(track))];
      }
      case "find_soulseek_album_candidates": {
        return (args?.tracks as Track[]).map((track) => ({
          trackId: track.id,
          candidate: { ...soulseekCandidate(track), confidence: "review", score: 0.95 },
          groupPeer: "peer-user",
          groupDirectory: "Music/Visible Artist/Album",
          groupCoverage: (args?.tracks as Track[]).length,
          expectedTracks: (args?.tracks as Track[]).length,
        }));
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
          isExcerpt: false, provider: candidate.provider, sourceGeneration: args?.normalizationRouteRequired ? Number(args?.requestGeneration) : webviewSourceGeneration,
          title: candidate.title, ...(args?.normalizationRouteRequired ? { normalizationRoute: "same-origin-relay" as const } : {}) } satisfies Preview & { normalizationRoute?: "same-origin-relay" };
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

afterEach(async () => {
  unmountApp?.();
  unmountApp = undefined;
  const pendingUnmounts = pendingSvelteUnmounts.splice(0);
  await Promise.allSettled(pendingUnmounts);
  await tick();
  sessionStorage.removeItem("burrata.webviewMediaEventDiagnostics");
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  if (nativeCurrentTimeDescriptor) Object.defineProperty(HTMLMediaElement.prototype, "currentTime", nativeCurrentTimeDescriptor);
  vi.clearAllMocks();
});

afterEach(() => { if (vi.isFakeTimers()) vi.useRealTimers(); });

describe("saved library navigation", () => {
  it("browses a saved catalog album using its saved artist identity", async () => {
    const originalCall=mocks.call.getMockImplementation()!;
    const releaseId='11111111-1111-4111-8111-111111111111',artistId='22222222-2222-4222-8222-222222222222';
    const entry={id:`album:ref:musicbrainz:${releaseId}`,kind:'album',name:'Saved catalog album',artist:'Catalog Artist',provider:'musicbrainz',providerId:releaseId,explicitlySaved:true,trackCount:1,totalTracks:1,albumCount:0};
    const track={...savedTrack,id:'catalog:mb-one',provider:'catalog',providerId:'mb-one',artist:'Catalog Artist',album:'Saved catalog album',provenance:{catalogProvider:'musicbrainz',releaseId,artistCredits:[{name:'Catalog Artist',artist:{provider:'musicbrainz',id:artistId},role:'artist'}]}};
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==='list_library_entities')return {items:[entry],total:1};
      if(command==='list_library_entity_tracks')return {tracks:[track],total:1};
      if(command==='get_release_tracks')return [track];
      if(command==='match_artist_playback_sources')return args?.sourceTracks;
      return originalCall(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.library-view-tabs button')].find(b=>b.textContent==='Albums')!.click();await settle();
    document.querySelector<HTMLButtonElement>('.entity-card')!.click();await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.library-browser button')].find(b=>b.textContent==='Browse catalog')!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith('get_release_tracks',expect.objectContaining({artist:{provider:'musicbrainz',id:artistId},release:expect.objectContaining({release:{provider:'musicbrainz',id:releaseId}})}));
    expect(mocks.call.mock.calls.some(([command])=>command==='search_artists')).toBe(false);
  });

  it("shows the shared saved album detail and analyzes its complete locally registered saved scope", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const entry = { id: "album:ref:spotify:library-detail", kind: "album", name: "Borrowed Light", artist: "Nora Vale", provider: "spotify", providerId: "library-detail", explicitlySaved: false, trackCount: 2, totalTracks: 2, albumCount: 0 };
    const first: Track = { ...savedTrack, id: "library-album-one", title: "First Saved Song", album: entry.name, artist: entry.artist };
    const second: Track = { ...savedTrack, id: "library-album-two", title: "Second Saved Song", album: entry.name, artist: entry.artist };
    const rows = [first, second];
    savedAssets = rows.map((track, index) => ({ id: `library-asset-${index + 1}`, recordingId: track.id, path: `/music/library-${index + 1}.flac`, hash: String.fromCharCode(97 + index).repeat(64), sourceCodec: "flac", finalCodec: "flac", processing: "ready", track }));
    const resource = { scope: "album" as const, members: savedAssets.map(asset => ({ assetId: asset.id, fileHash: asset.hash, range: { startSeconds: 0, endSeconds: null } })), interTrackGapsSeconds: [0], resourceKey: "c".repeat(64) };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_library_entities") return { items: [entry], total: 1 };
      if (command === "list_library_entity_tracks") return { tracks: rows, total: rows.length };
      if (command === "start_loudness_analysis") return "library-album-loudness";
      if (command === "get_loudness_analysis_status") return { jobId: "library-album-loudness", status: "completed", total: 2, completed: 2, error: null, resource };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".library-view-tabs button")].find(button => button.textContent === "Albums")!.click(); await settle();
    document.querySelector<HTMLButtonElement>(".entity-card")!.click(); await settle();

    expect(document.querySelector(".album-detail h1")?.textContent).toBe("Borrowed Light");
    expect(document.querySelector(".album-detail .artists")?.textContent).toBe("Nora Vale");
    expect(document.querySelector(".album-track-scope")?.textContent).toContain("2 of 2 catalog tracks saved");
    expect(document.body.textContent).toContain("Download album");
    expect(document.body.textContent).toContain("Play album");

    document.querySelector<HTMLElement>("[aria-label=\"More album actions for Borrowed Light\"]")!.click();
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Analyze album loudness")!.click(); await settle(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_track_files_for_recordings", { recordingIds: rows.map(track => track.id) });
    expect(mocks.call).toHaveBeenCalledWith("start_loudness_analysis", { assetIds: savedAssets.map(asset => asset.id), interTrackGapsSeconds: [0] });
    expect(document.body.textContent).toContain("Play or queue this album to use its measured album gain.");
  });

  it("opens saved artists and restores artist subsections through back and forward", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const artist = { id: 'artist:ref:spotify:library-artist', kind: 'artist', name: 'Library Artist', artist: '', provider: 'spotify', providerId: 'library-artist', explicitlySaved: true, trackCount: 1, albumCount: 1 };
    const album = { ...artist, id: 'album:ref:spotify:library-album', kind: 'album', name: 'Library Album', artist: 'Library Artist', providerId: 'library-album', totalTracks: 10 };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === 'list_library_entities') return { items: args?.kind === 'artist' ? [artist] : [album], total: 1 };
      if (command === 'list_library_entity_tracks') return { tracks: [savedTrack], total: 1 };
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.library-view-tabs button')].find(b => b.textContent === 'Artists')!.click(); await settle();
    document.querySelector<HTMLButtonElement>('.entity-card')!.click(); await settle();
    expect(document.querySelector('.entity-header h2')?.textContent).toBe('Library Artist');
    expect(document.querySelector('.entity-card')?.textContent).toContain('1 of 10 tracks saved');
    [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Library artist sections"] button')].find(b => b.textContent === 'All tracks')!.click(); await settle();
    expect(document.querySelector('.library-browser .shared-track-list')?.textContent).toContain('Saved Recording');
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector('.entity-card')?.textContent).toContain('Library Album');
    document.querySelector<HTMLButtonElement>('[aria-label="Go forward"]')!.click(); await settle();
    expect(document.querySelector('.library-browser .shared-track-list')?.textContent).toContain('Saved Recording');
    expect(mocks.call.mock.calls.some(([command]) => command === 'browse_remote_artist' || command === 'get_artist_profile')).toBe(false);
  });
});

describe("selected playback restore startup gate", () => {
  it("retains an interrupted migration and retries its exact queue and playhead", async () => {
    const queue = {tracks:[savedTrack,savedTrack],entryIds:["first","second"],currentIndex:1,repeat:"all"};
    localStorage.setItem("burrata.playback-queue.v1",JSON.stringify(queue));
    localStorage.setItem("burrata.playback-position.v1",JSON.stringify({trackId:savedTrack.id,entryId:"second",position:37,savedAt:Date.now()}));
    const original=mocks.call.getMockImplementation()!;
    let failCommit=true;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="migrate_native_playback"&&args?.legacy&&failCommit)throw new Error("disk full");
      return original(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    expect(document.querySelector<HTMLElement>(".app-shell")?.inert).toBe(true);
    expect(JSON.parse(localStorage.getItem("burrata.playback-queue.v1")!).entryIds).toEqual(["first","second"]);
    expect(mocks.call.mock.calls.some(([command])=>command==="get_playback_snapshot")).toBe(false);
    window.dispatchEvent(new KeyboardEvent("keydown",{key:" ",bubbles:true}));await settle();
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
    expect(mocks.call.mock.calls.some(([command,args])=>command==="set_automation_frontend_ready"&&args?.ready===true)).toBe(false);
    failCommit=false;
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent==="Retry playback restore")!.click();await settle();
    expect(document.querySelector<HTMLElement>(".app-shell")?.inert).toBe(false);
    expect(nativePlaybackSnapshot).toMatchObject({queueEntryIds:["first","second"],currentIndex:1,position:37,status:"paused"});
    expect(mocks.call.mock.calls.some(([command,args])=>command==="set_automation_frontend_ready"&&args?.ready===true)).toBe(true);
    expect(document.querySelector("audio")).toBeNull();
  });

  function pendingRestore() {
    return {
      version: 1, nonce: "a1b2c3d4-e5f6-4711-8222-334455667788",
      queue: { tracks: [{ ...savedTrack, id: "local:restored", localPath: "/verified/restored/audio.wav" }], entryIds: ["occurrence-restored"], currentIndex: 0, repeat: "off" },
      position: 7.5, volume: 0, repeat: "off", shuffle: false, paused: true,
    };
  }
  it("shows startup status while hydration is pending and keeps the app inert", async () => {
    const original = mocks.call.getMockImplementation()!;
    let finishRestore: ((value: null) => void) | undefined;
    mocks.call.mockImplementation((command: string, args?: Record<string, unknown>) => command === "get_backup_playback_restore"
      ? new Promise(resolve => { finishRestore = resolve; }) : original(command, args));
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector('[role="status"].backup-hydration-gate')?.textContent).toContain("Starting Burrata…");
    expect(document.querySelector<HTMLElement>(".app-shell")?.inert).toBe(true);
    expect(document.querySelector(".app-shell")?.getAttribute("aria-busy")).toBe("true");
    expect(document.querySelector(".library-sidebar")).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_playback_state")).toBe(false);
    finishRestore?.(null); await settle();
    expect(document.querySelector(".backup-hydration-gate")).toBeNull();
    expect(document.querySelector<HTMLElement>(".app-shell")?.inert).toBe(false);
  });
  it.each([2, 3])("does not initialize or publish playback while localStorage write %i fails; retry completes hydration", async (failedWrite: number) => {
    pendingBackupPlaybackRestore = pendingRestore();
    let restoreWrites = 0; let alreadyFailed = false;
    const rawSet = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, "setItem").mockImplementation((key, value) => {
      if (["burrata.playback-queue.v1", "burrata.playback-position.v1", "burrata.playback-controls.v1", "burrata.engine-switch-controls.v1"].includes(key)) {
        restoreWrites++;
        if (!alreadyFailed && restoreWrites === failedWrite) { alreadyFailed = true; throw new DOMException("quota", "QuotaExceededError"); }
      }
      rawSet(key, value);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    const gate = document.querySelector<HTMLElement>(".backup-hydration-gate")!;
    expect(gate.textContent).toContain("Could not finish restoring saved playback. Retry to continue.");
    expect(gate.textContent).toContain("Free some app storage space, then retry.");
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_playback_state")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "set_automation_frontend_ready")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "get_restore_work_policy")).toBe(false);
    expect(backupPlaybackRestoreAckCalls).toEqual([]);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
    expect(mocks.call.mock.calls.some(([command]) => command === "playback_command")).toBe(false);
    const retry = document.querySelector<HTMLButtonElement>(".backup-hydration-gate button")!;
    retry.focus();
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    window.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(false);
    retry.click();
    await settle();
    expect(backupPlaybackRestoreAckCalls).toEqual([{ nonce: pendingRestore().nonce, receiptSessionId: backupPlaybackTestSessionId }]);
    expect(localStorage.getItem("burrata.playback-controls.v1")).toBe(JSON.stringify({ volume: 0, shuffle: false }));
    expect(mocks.call).toHaveBeenCalledWith("get_restore_work_policy");
    expect(mocks.call.mock.calls.some(([command]) => command === "get_playback_snapshot")).toBe(true);
  });

  it("keeps startup blocked when localStorage silently ignores a write, then loads the restored queue on retry", async () => {
    pendingBackupPlaybackRestore = pendingRestore();
    const rawSet = localStorage.setItem.bind(localStorage);
    rawSet("burrata.playback-queue.v1", JSON.stringify({
      tracks: [{ ...savedTrack, id: "local:old" }], entryIds: ["old-occurrence"], currentIndex: 0, repeat: "one",
    }));
    let ignoreQueueWrite = true;
    vi.spyOn(localStorage, "setItem").mockImplementation((key, value) => {
      if (ignoreQueueWrite && key === "burrata.playback-queue.v1") return;
      rawSet(key, value);
    });

    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    const gate = document.querySelector<HTMLElement>(".backup-hydration-gate")!;
    expect(gate.textContent).toContain("Could not finish restoring saved playback. Retry to continue.");
    expect(localStorage.getItem("burrata.playback-queue.v1")).toContain("old-occurrence");
    expect(localStorage.getItem("burrata.backup-playback-restore-receipt.v1")).toBeNull();
    expect(backupPlaybackRestoreAckCalls).toEqual([]);
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_playback_state")).toBe(false);

    ignoreQueueWrite = false;
    document.querySelector<HTMLButtonElement>(".backup-hydration-gate button")!.click();
    await settle();
    expect(backupPlaybackRestoreAckCalls).toEqual([{ nonce: pendingRestore().nonce, receiptSessionId: backupPlaybackTestSessionId }]);
    const restored = JSON.parse(localStorage.getItem("burrata.playback-queue.v1")!);
    expect(restored.tracks.map((track: Track) => track.id)).toEqual(["local:restored"]);
    expect(restored.entryIds).toEqual(["occurrence-restored"]);
    expect(restored.repeat).toBe("off");
    expect(mocks.call.mock.calls.some(([command]) => command === "get_playback_snapshot")).toBe(true);
  });

  it("starts from the restored queue and controls when existing storage records roll back after ACK", async () => {
    pendingBackupPlaybackRestore = {
      ...pendingRestore(), repeat: "all",
      queue: { ...pendingRestore().queue, repeat: "all" },
    };
    const oldQueue = JSON.stringify({
      tracks: [{ ...savedTrack, id: "local:old" }], entryIds: ["old-occurrence"], currentIndex: 0, repeat: "one",
    });
    const oldControls = JSON.stringify({ volume: 0.5, shuffle: true });
    const oldValues = new Map([
      ["burrata.playback-queue.v1", oldQueue],
      ["burrata.playback-position.v1", JSON.stringify({ trackId: "local:old", position: 2, savedAt: Date.now() })],
      ["burrata.playback-controls.v1", oldControls],
      ["burrata.engine-switch-controls.v1", oldControls],
    ]);
    for (const [key, value] of oldValues) localStorage.setItem(key, value);
    const rawSet = localStorage.setItem.bind(localStorage);
    vi.spyOn(localStorage, "setItem").mockImplementation((key, value) => {
      rawSet(key, value);
      if (oldValues.has(key)) queueMicrotask(() => rawSet(key, oldValues.get(key)!));
    });

    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    expect(backupPlaybackRestoreAckCalls).toEqual([{ nonce: pendingRestore().nonce, receiptSessionId: backupPlaybackTestSessionId }]);
    expect(localStorage.getItem("burrata.playback-queue.v1")).toBe(oldQueue);
    expect(localStorage.getItem("burrata.playback-controls.v1")).toBe(oldControls);
    expect(document.querySelector(".backup-hydration-gate")).toBeNull();
    const migration = mocks.call.mock.calls.find(([command,args]) => command === "migrate_native_playback" && args?.legacy);
    expect(migration?.[1]?.legacy).toMatchObject({repeat:"all",shuffle:false,volume:0,tracks:[expect.objectContaining({id:"local:restored"})]});
    expect(pendingBackupPlaybackRestore).not.toBeNull();
  });

  it("keeps startup blocked on a stale ack and starts only after retry succeeds", async () => {
    pendingBackupPlaybackRestore = pendingRestore(); failNextBackupPlaybackAck = true;
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    const gate = document.querySelector<HTMLElement>(".backup-hydration-gate")!;
    expect(gate.textContent).toContain("Could not finish restoring saved playback. Retry to continue.");
    expect(gate.textContent).not.toContain("Free some app storage space");
    expect(mocks.call.mock.calls.some(([command]) => command === "automation_playback_state")).toBe(false);
    expect(backupPlaybackRestoreAckCalls).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "get_restore_work_policy")).toBe(false);
    document.querySelector<HTMLButtonElement>(".backup-hydration-gate button")!.click();
    await settle();
    expect(backupPlaybackRestoreAckCalls).toHaveLength(2);
    expect(pendingBackupPlaybackRestore).not.toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("get_restore_work_policy");
  });
});

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
    expect(mocks.call).toHaveBeenCalledWith("playback_command", {command:expect.objectContaining({action:"play",tracks:[expect.objectContaining({localPath:"/music/Jazz/Cut.flac",assetId:"asset-jazz-cut"})]})});
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
    expect(mocks.call).toHaveBeenCalledWith("preview_collection_import", { url });
    expect(mocks.call.mock.calls.some(([command])=>command==="apply_collection_import")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button=>button.textContent?.trim()==="Apply import")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("apply_collection_import",{previewId:"preview-import"});
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
    expect(mocks.call).toHaveBeenCalledWith("preview_collection_import", { url });
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
    expect(mocks.call).toHaveBeenCalledWith("preview_collection_import", { url });
    expect(mocks.call.mock.calls.some(([command]) => command === "apply_collection_import")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>('section[aria-label="Import preview"] button')].find(button => button.textContent?.trim() === "Apply import")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("apply_collection_import", { previewId: "preview-import" });
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

  it("opens portable export for the virtual Favorites playlist and exposes copy text", async () => {
    sidebarLayoutState = { sections: [], assignments: {}, names: {} };
    playlistPreferences = { favoritesPlaylistId: "", shortcuts: Array<string | null>(9).fill(null), shortcutBindings: Array.from({ length: 9 }, (_, index) => `Alt+${index + 1}`) };
    localStorage.setItem("burrata.playlist.include-ytdlp-search.v1", "true");
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command, args) => command === "preview_portable_playlist_export"
      ? { entryCount: 1, localEntries: 0, identityPlaceholders: 1, searchLinks: 0, requiresBurrata: true, excludedPrivateFields: [] }
      : original(command, args));
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const favorite = [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("Liked Songs"))!;
    favorite.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 310, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Liked Songs"]')!;
    expect([...menu.querySelectorAll<HTMLButtonElement>("button")].map(menuActionText)).toContain("Export playlist");
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find(button => menuActionText(button) === "Export playlist")!.click();
    await settle();
    expect(document.querySelector<HTMLDialogElement>('dialog[aria-label="Portable playlist"]')).toBeTruthy();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(true);
    document.querySelector<HTMLDetailsElement>('dialog[aria-label="Portable playlist"] .advanced')!.open = true;
    expect(mocks.call).toHaveBeenCalledWith("preview_portable_playlist_export", expect.objectContaining({ collectionId: "favorites", options: expect.objectContaining({ includeYtdlpSearch: true }) }));
    expect(document.querySelector<HTMLDialogElement>('dialog[aria-label="Portable playlist"]')?.textContent).toContain("Copy playlist file contents");
  });

  it.each(["", "favorite-collection-42"])("makes an editable copy of Favorites with backing collection %s in its current order", async (favoritesPlaylistId) => {
    sidebarLayoutState = { sections: [], assignments: {}, names: {} };
    playlistPreferences = { favoritesPlaylistId, shortcuts: Array<string | null>(9).fill(null), shortcutBindings: Array.from({ length: 9 }, (_, index) => `Alt+${index + 1}`) };
    if (favoritesPlaylistId) mutableFavoritesCollection = { id: favoritesPlaylistId, name: "Liked Songs", trackCount: 1 };
    favoriteTrackIds.add(savedTrack.id);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const favorite = [...document.querySelectorAll<HTMLButtonElement>(".playlist-card")].find(button => button.textContent?.includes("Liked Songs"))!;
    favorite.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 310, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Liked Songs"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find(button => menuActionText(button) === "Make editable copy")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "favorites" });
    expect(mocks.call).toHaveBeenCalledWith("create_collection", { name: "Liked Songs (Copy)" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-new", tracks: [{ ...savedTrack, favorite: true }] });
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

  it("dismisses playlist import choices with Close, Escape, or the backdrop", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard("Empty Collection").click();
    await settle();
    for (const method of ["close", "escape", "backdrop"]) {
      [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Import tracks")!.click();
      await settle();
      const dialog = document.querySelector<HTMLDialogElement>("dialog.import-dialog")!;
      expect(dialog.querySelector(".import-hub")).not.toBeNull();
      if (method === "close") dialog.querySelector<HTMLButtonElement>('[aria-label="Close import"]')!.click();
      else if (method === "escape") dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      else dialog.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: -1, clientY: -1 }));
      await settle();
      expect(document.querySelector("dialog.import-dialog")).toBeNull();
      expect(document.querySelector("h1")?.textContent).toBe("Empty Collection");
    }
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
    [...document.querySelectorAll<HTMLButtonElement>(".import-hub button")].find(button => button.textContent?.includes("Import a playlist or album link"))!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Chart, playlist or album URL"]')!;
    input.value = url; input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button => button.textContent?.trim() === "Import URL")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("preview_collection_import", { url });
    expect(mocks.call.mock.calls.some(([command])=>command==="apply_collection_import")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button=>button.textContent?.trim()==="Apply import")!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("apply_collection_import",{previewId:"preview-import"});
    expect(mocks.call).toHaveBeenCalledWith("load_browser_import", { id: "snapshot-into-playlist" });
    expect(mocks.call).toHaveBeenCalledWith("append_playlist_entries", { collectionId: "collection-empty", tracks: savedChartTracks });
    expect(document.querySelector("h1")?.textContent).toBe("Empty Collection");
    expect(document.querySelector(".browser-capture")).toBeNull();
  });

  it("applies a URL preview to a chosen playlist with reviewed folder, section, and duplicate settings", async () => {
    const url = "https://www.shazam.com/charts/top-200/world";
    directBrowserImport = { ...latestImport, id: "snapshot-url-destination", provider: "shazam", kind: "chart", title: "Shazam tracks", sourceUrl: url, collectionId: "collection-imported-source" };
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle(); await openLibraryImport();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Chart, playlist or album URL"]')!;
    input.value = url; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button => button.textContent?.trim() === "Import URL")!.click(); await settle();
    expect(document.querySelector('[aria-label="Import preview"]')).not.toBeNull();
    const fields = [...document.querySelectorAll<HTMLSelectElement>(".url-import-destination select")];
    fields[0].value = "collection-empty"; fields[0].dispatchEvent(new Event("change", { bubbles: true }));
    fields[1].value = "skip"; fields[1].dispatchEvent(new Event("change", { bubbles: true }));
    const textInputs = [...document.querySelectorAll<HTMLInputElement>(".url-import-destination input")];
    textInputs[1].value = "Imported/Charts"; textInputs[1].dispatchEvent(new Event("input", { bubbles: true }));
    textInputs[2].value = "Top 200"; textInputs[2].dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(document.querySelector(".url-import-destination")?.textContent).toContain("Empty Collection");
    expect(document.querySelector(".url-import-destination")?.textContent).toContain("skip duplicates");
    expect(mocks.call.mock.calls.some(([command]) => command === "apply_collection_import")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>(".import-panel button")].find(button => button.textContent?.trim() === "Apply import")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("apply_collection_import", { previewId: "preview-import", destinationCollectionId: "collection-empty", duplicatePolicy: "skip", folderPath: "Imported/Charts", sectionName: "Top 200" });
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

  it("renames a playlist and removes checked tracks from that playlist only", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Playlist context menu"]')!.click();
    await settle();
    const playlistMenu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Saved Collection"]')!;
    [...playlistMenu.querySelectorAll<HTMLButtonElement>("button")].find((item) => menuActionText(item) === "Rename")!.click();
    await settle();
    await saveSidebarRename("Weekend Set");
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

  it("plays and queues every saved playlist occurrence in stored order, independent of the visible filters", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const pinned = {
      ...collectionTrack,
      id: "duplicate-occurrence",
      title: "Pinned source",
      provider: "youtube" as const,
      providerId: "pinned-video-id",
      sourceUrl: "https://youtube.com/watch?v=pinned-video-id",
      provenance: { sourceKind: "playlist", playlistPosition: 1, playbackMode: "source", userSelectedSource: true, selectedCandidate: { provider: "youtube", id: "pinned-video-id", url: "https://youtube.com/watch?v=pinned-video-id", title: "Pinned source" } },
    };
    const repeated = { ...pinned, title: "Repeated source", provenance: { ...pinned.provenance, playlistPosition: 2 } };
    const middle = Array.from({ length: 202 }, (_, index) => ({
      ...secondTrack,
      id: `playlist-track-${index}`,
      title: `Saved track ${index + 1}`,
      provenance: { sourceKind: "playlist", playlistPosition: index + 3 },
    }));
    const final = { ...secondTrack, id: "playlist-final", title: "Last saved track", provenance: { sourceKind: "playlist", playlistPosition: 205 } };
    const savedRows = [pinned, repeated, ...middle, final];
    collections.find(item => item.id === "collection-full")!.trackCount = savedRows.length;
    mockCollectionRows = savedRows;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click();
    await settle();

    const sort = document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title";
    sort.dispatchEvent(new Event("change", { bubbles: true }));
    const filter = document.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "no visible match";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    mocks.call.mockClear();

    const play = document.querySelector<HTMLButtonElement>('[aria-label="Play all 205 saved tracks in playlist order, ignoring filters"]')!;
    expect(play.disabled).toBe(false);
    play.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "collections", collectionId: "collection-full" });
    expect(mocks.call).toHaveBeenCalledWith("playback_command", expect.objectContaining({
      command: expect.objectContaining({ action: "play", tracks: savedRows, index: 0 }),
    }));
    const playCall = mocks.call.mock.calls.find(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "play");
    const playedTracks = (playCall?.[1] as { command?: { tracks?: Track[] } } | undefined)?.command?.tracks ?? [];
    expect(playedTracks).toHaveLength(205);
    expect(playedTracks[0]).toEqual(pinned);
    expect(playedTracks[1]).toEqual(repeated);
    expect(playedTracks[204]).toEqual(final);

    const queue = document.querySelector<HTMLButtonElement>('[aria-label="Queue all 205 saved tracks in playlist order, ignoring filters"]')!;
    expect(queue.disabled).toBe(false);
    queue.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", expect.objectContaining({
      command: expect.objectContaining({ action: "queue", tracks: savedRows, mode: "append" }),
    }));
  });

  it("cancels a pending full-playlist action after navigating away", async () => {
    delayedBrowserImports = Promise.resolve([]);
    let finishRead: ((rows: Track[]) => void) | undefined;
    const base = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "list_tracks" && args?.view === "collections") return new Promise<Track[]>(resolve => { finishRead = resolve; });
      return base(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click();
    await settle();
    const play = document.querySelector<HTMLButtonElement>('[aria-label="Play all 131 saved tracks in playlist order, ignoring filters"]')!;
    play.click();
    await settle();
    expect(play.disabled).toBe(true);
    const callsBeforeNavigation = mocks.call.mock.calls.filter(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "play").length;
    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 1, payload: { requestId: "playlist-play-away", target: { kind: "view", view: "library" } } });
    await settle();
    await settle();
    finishRead!([collectionTrack]);
    await settle();
    expect(mocks.call.mock.calls.filter(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "play")).toHaveLength(callsBeforeNavigation);
  });

  it("does not publish a failed playlist read after navigating away", async () => {
    delayedBrowserImports = Promise.resolve([]);
    let rejectRead: ((reason?: unknown) => void) | undefined;
    const base = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "list_tracks" && args?.view === "collections") return new Promise<Track[]>((_resolve, reject) => { rejectRead = reject; });
      return base(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play all 131 saved tracks in playlist order, ignoring filters"]')!.click();
    await settle();
    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 1, payload: { requestId: "playlist-play-failure-away", target: { kind: "view", view: "library" } } });
    await settle();
    await settle();
    rejectRead!(new Error("stale playlist read failure"));
    await settle();
    expect(document.querySelector(".banner.error")?.textContent ?? "").not.toContain("stale playlist read failure");
  });

  it("disables full-playlist playback actions for an empty saved playlist", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard("Empty Collection").click();
    await settle();
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Play all 0 saved tracks in playlist order, ignoring filters"]')?.disabled).toBe(true);
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Queue all 0 saved tracks in playlist order, ignoring filters"]')?.disabled).toBe(true);
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

  it("persists dragged playlist occurrences and inverts occurrence selection without provider writes",async()=>{
    delayedBrowserImports=Promise.resolve([{...latestImport,provider:"spotify",title:"Linked Spotify",sourceUrl:"https://open.spotify.com/playlist/11223344556677889900aa"}]);
    remoteBindingState=[{bindingId:"local-order",provider:"spotify",collectionId:"linked-working",sourceCollectionId:"collection-full",url:"https://open.spotify.com/playlist/11223344556677889900aa",canEdit:true}];
    mockCollectionRows=[{...collectionTrack,id:"duplicate",title:"Repeated",provenance:{playlistPosition:1}},{...collectionTrack,id:"duplicate",title:"Repeated",provenance:{playlistPosition:2}},{...collectionTrack,id:"last",title:"Last",provenance:{playlistPosition:3}}];
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();await refreshSidebarBrowserImports();collectionCard("Linked Spotify").click();await settle();await settle();
    expect(document.querySelector('.playlist-reorder-handle')).toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Select playlist tracks"]')!.click();await settle();
    const handle=document.querySelector<HTMLButtonElement>('[aria-label^="Reorder Repeated at position 2;"]')!;
    expect(handle).not.toBeNull();
    const transfer={setData:vi.fn(),effectAllowed:"",dropEffect:""};
    const start=new Event("dragstart",{bubbles:true,cancelable:true});Object.defineProperty(start,"dataTransfer",{value:transfer});handle.dispatchEvent(start);await settle();
    const target=document.querySelector<HTMLElement>('.desktop-track-list li[data-track-index="2"]')!;
    vi.spyOn(target,"getBoundingClientRect").mockReturnValue({top:0,height:100,bottom:100,left:0,right:100,width:100,x:0,y:0,toJSON(){}});
    const drop=new Event("drop",{bubbles:true,cancelable:true});Object.defineProperty(drop,"dataTransfer",{value:transfer});Object.defineProperty(drop,"clientY",{value:90});target.dispatchEvent(drop);await settle();
    expect(mocks.call).toHaveBeenCalledWith("move_playlist_entry",{collectionId:"linked-working",from:1,to:2});
    const first=document.querySelector<HTMLInputElement>('[aria-label^="Select occurrence 1:"]')!;first.click();await settle();
    document.querySelector<HTMLButtonElement>(".desktop-track-list .invert-selection")!.click();await settle();
    const selected=[...document.querySelectorAll<HTMLInputElement>('.desktop-track-list .select-track')].map(input=>input.checked);
    expect(selected).toEqual([false,true,true]);
    expect(mocks.call.mock.calls.some(([command])=>["apply_remote_playlist_sync_preview","create_verification_playlist","connect_remote_playlist_provider"].includes(command))).toBe(false);
  });

  it("hides playlist reorder controls under title sorting and filtering", async () => {
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
    expect(document.querySelector('.playlist-reorder-handle')).toBeNull();
    expect([...document.querySelectorAll<HTMLButtonElement>("button")].some(button => /^(Move up|Move down)$/.test(button.textContent?.trim() ?? ""))).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "move_playlist_entry")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "list_tracks")).toBe(false);
    expect(document.querySelector(".shared-track-list")?.textContent).toContain("Aardvark Recording");
  });

  it("replaces only the explicitly selected existing analysis field", async () => {
    const localTrack: Track = { ...savedTrack, assetId: "asset-1", localAvailability: "available", bpm: 110, key: "C", provenance: { manualFields: { bpm: true, key: true } } };
    savedAssets = [{ id: "asset-1", recordingId: localTrack.id, path: "/music/saved.mp3", hash: "hash-1", sourceCodec: "mp3", finalCodec: "mp3", processing: "ready", track: localTrack }];
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    const savedRow = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"));
    savedRow?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("nav button")].find(item => item.textContent?.includes("Library"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("button")].find(item => item.textContent?.includes("Analyze selected BPM / key"))!.click();
    await settle();
    const applyKey = [...document.querySelectorAll<HTMLLabelElement>(".analysis-apply-options label")].find(label => label.textContent?.includes("Key"))!.querySelector("input")!;
    applyKey.click();
    const replaceBpm = [...document.querySelectorAll<HTMLLabelElement>(".analysis-replace-options label")].find(label => label.textContent?.includes("Replace existing BPM"))!.querySelector("input")!;
    replaceBpm.click();
    await settle();
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Existing BPM will be replaced by your explicit choice");
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Key will stay unchanged.");
    document.querySelector<HTMLButtonElement>(".analysis-review button.primary")!.click();
    await settle();
    expect(selectedAnalysisApplyCalls).toHaveLength(1);
    expect(selectedAnalysisApplyCalls[0]?.selection).toEqual({ fields: ["bpm"], policies: { bpm: "replace", key: "fillMissing" } });
    expect(savedAssets[0]?.track.bpm).toBe(124);
    expect(savedAssets[0]?.track.key).toBe("C");
  });

  it("cancels a pending selected-analysis job without applying partial results", async () => {
    vi.useFakeTimers();
    selectedAnalysisInitialState = "running";
    const localTrack: Track = { ...savedTrack, assetId: "asset-1", localAvailability: "available" };
    savedAssets = [{ id: "asset-1", recordingId: localTrack.id, path: "/music/saved.mp3", hash: "hash-1", sourceCodec: "mp3", finalCodec: "mp3", processing: "ready", track: localTrack }];
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    const savedRow = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"));
    savedRow?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("nav button")].find(item => item.textContent?.includes("Library"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    [...document.querySelectorAll("button")].find(item => item.textContent?.includes("Analyze selected BPM / key"))!.click();
    await settle();
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Analyzing selected audio");
    const cancel = [...document.querySelectorAll<HTMLButtonElement>(".analysis-review button")].find(button => button.textContent === "Cancel analysis")!;
    cancel.click();
    await settle();
    await vi.advanceTimersByTimeAsync(651);
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_selected_asset_analysis", { jobId: "selected-analysis-1" });
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Analysis cancelled");
    expect(selectedAnalysisApplyCalls).toHaveLength(0);
  });

  it.each([true, false])("closes selected analysis on Escape and restores its visible opener (action focused: %s)", async (actionFocused) => {
    vi.useFakeTimers();
    selectedAnalysisInitialState = "running";
    const localTrack: Track = { ...savedTrack, assetId: "asset-1", localAvailability: "available" };
    savedAssets = [{ id: "asset-1", recordingId: localTrack.id, path: "/music/saved.mp3", hash: "hash-1", sourceCodec: "mp3", finalCodec: "mp3", processing: "ready", track: localTrack }];
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    [...document.querySelectorAll("nav button")].find(item => item.textContent?.includes("Library"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const trigger = [...document.querySelectorAll<HTMLButtonElement>("button")].find(item => item.textContent?.includes("Analyze selected BPM / key"))!;
    const menu = trigger.closest("details")!;
    menu.open = true;
    const opener = menu.querySelector<HTMLElement>(":scope > summary")!;
    const externalFocus = document.createElement("button");
    document.body.append(externalFocus);
    (actionFocused ? trigger : externalFocus).focus();
    trigger.click(); await settle();
    const dialog = document.querySelector<HTMLDialogElement>(".analysis-review")!;
    expect(dialog.closest(".app-shell")).not.toBeNull();
    const modalSpace = new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true });
    dialog.dispatchEvent(modalSpace);
    expect(modalSpace.defaultPrevented).toBe(false);
    const cancel = new Event("cancel", { cancelable: true }); dialog.dispatchEvent(cancel); await settle();
    await vi.advanceTimersByTimeAsync(651); await settle();
    expect(cancel.defaultPrevented).toBe(true);
    expect(document.querySelector(".analysis-review")).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("cancel_selected_asset_analysis", { jobId: "selected-analysis-1" });
    expect(selectedAnalysisApplyCalls).toHaveLength(0);
    expect(document.activeElement).toBe(opener);
    const menuSpace = new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true });
    opener.dispatchEvent(menuSpace);
    expect(menuSpace.defaultPrevented).toBe(false);
  });

  it("keeps analysis controls and status out of the player footer while exposing the setting in Playback settings", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const footer = document.querySelector("footer.player")!;
    expect(footer.querySelector(".stream-analysis-action")).toBeNull();
    expect(footer.querySelector(".player-analysis-status")).toBeNull();
    expect(footer.querySelector('[aria-label="Analyze full local waveform"]')).toBeNull();
    expect(footer.querySelector('[aria-label="Waveform seek bar"]')).not.toBeNull();
    expect(footer.textContent).not.toMatch(/BPM|Whole-track analysis|Full stream analysis/);
    expect(footer.querySelector('input[aria-label="Analyze full streamed tracks in background"]')).toBeNull();

    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 1, payload: {
      requestId: "analysis-settings-location", target: { kind: "view", view: "settings", settingsCategory: "playback" },
    } });
    await settle(); await settle();
    expect(document.querySelector('input[aria-label="Analyze full streamed tracks in background"]')).not.toBeNull();
  });

  it("resolves selected saved streaming sources on demand and previews source identity before applying", async () => {
    const remoteTrack: Track = {
      ...savedTrack, bpm: 110, key: "C",
      provenance: { bpmSource: "catalog", keySource: "import", manualFields: { key: true } },
    };
    savedChartTracks = [remoteTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")]
      .find(item => item.textContent?.includes("Saved Recording"))!;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const libraryOptions = document.querySelector<HTMLDetailsElement>(".header-library-tools")!;
    expect(document.querySelector('main > header .header-actions > [aria-label="Analyze selected streaming sources"]')).toBeNull();
    expect(document.querySelector('main > header .header-actions > [aria-label="Analyze selected loudness"]')).toBeNull();
    expect([...document.querySelectorAll<HTMLButtonElement>("main > header .header-actions > button")]
      .some((button) => button.textContent?.includes("New playlist"))).toBe(true);
    libraryOptions.querySelector("summary")!.click();
    expect(libraryOptions.querySelector('[aria-label="Analyze selected loudness"]')?.getAttribute("title"))
      .toContain("analysis does not rewrite audio");
    const analyze = libraryOptions.querySelector<HTMLButtonElement>('[aria-label="Analyze selected streaming sources"]')!;
    expect(analyze.title).toContain("Resolve verified streaming sources");
    analyze.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("start_selected_stream_analysis", {
      trackIds: [remoteTrack.id], fields: ["bpm", "key"],
    });
    expect(mocks.call).not.toHaveBeenCalledWith("enqueue_download", expect.anything());
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Verified source: youtube · stable-video-1");
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Audio isn’t saved, and existing BPM and key values are kept unless you choose to replace them.");
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Existing BPM will be kept");
    expect(document.querySelector(".analysis-review")?.textContent).toContain("Existing key will be kept · marked manual");
    document.querySelector<HTMLButtonElement>(".analysis-review button.primary")!.click();
    await settle();
    expect(selectedStreamAnalysisApplyCalls).toEqual([{
      jobId: "selected-stream-analysis-1", trackId: remoteTrack.id,
      selection: { fields: ["bpm", "key"], policies: { bpm: "fillMissing", key: "fillMissing" } },
    }]);
    expect(savedChartTracks[0]?.bpm).toBe(110);
    expect(savedChartTracks[0]?.key).toBe("C");
    expect(document.querySelector(".analysis-review")).toBeNull();
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
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click();
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
    expect(options.map((option) => option.value)).toEqual(["all", "custom", "musicbrainz", "discogs", "youtube", "soundcloud", "soulseek"]);
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

  it("adds a loaded Spotify album through a named playlist picker despite filtering, without playback or remote writes", async () => {
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
    document.querySelector<HTMLButtonElement>('[aria-label="Add album to playlist"]')!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="New playlist for album"]')?.value).toBe("Save Album");
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')?.value).toBe("");
    expect(mocks.call.mock.calls.some(([command]) => command === "save_library_tracks")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "append_playlist_entries")).toBe(false);
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button=>button.textContent?.trim()==="Create & add")!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection",{name:"Save Album"});
    const append = mocks.call.mock.calls.find(([command])=>command === "append_playlist_entries");
    expect((append?.[1] as {tracks:Track[]}).tracks.map(track=>track.id)).toEqual([first.id,second.id]);
    document.querySelector<HTMLButtonElement>('[aria-label="Add album to playlist"]')!.click(); await settle();
    const existing = document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')!;
    existing.value="collection-full";existing.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button=>button.textContent?.trim()==="Add to playlist")!.click();await settle();
    const existingAppend=mocks.call.mock.calls.find(([command,args])=>command==="append_playlist_entries" && args?.collectionId==="collection-full");
    expect((existingAppend?.[1] as {tracks:Track[]}).tracks.map(track=>track.id)).toEqual([first.id,second.id]);
    document.querySelector<HTMLButtonElement>('[aria-label="Add album to playlist"]')!.click();await settle();
    const customName=document.querySelector<HTMLInputElement>('[aria-label="New playlist for album"]')!;
    customName.value="My album collection";customName.dispatchEvent(new Event("input",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".playlist-picker-dialog button")].find(button=>button.textContent?.trim()==="Create & add")!.click();await settle();
    expect(mocks.call).toHaveBeenCalledWith("create_collection",{name:"My album collection"});
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

  it("opens a Spotify album search result and returns without importing or playing", async () => {
    const albumId = "dddddddddddddddddddddd";
    const album = { id: albumId, title: "Search Album Edition", url: `https://open.spotify.com/album/${albumId}`, artists: ["Exact Artist"] };
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 14 }, spotifyAccountId: "fixture-account" };
      if (command === "list_search_sources") return [{ id: "library", name: "Library", kinds: ["track"], enabled: true }, { id: "spotify", name: "Spotify", kinds: ["album"], enabled: true }];
      if (command === "search_source") return { sourceId: "spotify", complete: true, results: [{ id: albumId, kind: "album", sourceId: "spotify", title: album.title, url: album.url }] };
      if (command === "browse_remote_album_tracks") return { provider: "spotify", accountId: "fixture-account", album, tracks: [], total: 0, complete: true };
      return original(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Search"]')!.click(); await settle();
    await searchDiscovery(album.title, ["spotify"]);
    const result = [...document.querySelectorAll<HTMLElement>(".result-row")].find(row => row.textContent?.includes(album.title))!;
    const open = [...result.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Open album"))!;
    expect(open).toBeTruthy(); open.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("browse_remote_album_tracks", expect.objectContaining({ albumId, cursor: null }));
    expect(document.querySelector(".spotify-artist h1")?.textContent).toBe(album.title);
    expect(mocks.call.mock.calls.some(([command]) => ["import_remote_playlists", "resolve_playback_track", "playback_command"].includes(command))).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Go back"]')!.click(); await settle();
    expect(document.querySelector(".search-panel")?.textContent).toContain(album.title);
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
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_saved_track_detections")).toHaveLength(0);
    document.querySelector<HTMLButtonElement>('.spotify-artist [aria-label="Play Spotify Song 000"]')!.click();
    await settle();
    expect(matchingCalls().map(([, args]) => (args as { sourceTracks: Track[] }).sourceTracks.length)).toEqual([200, 200, 101]);
    expect(mocks.call).toHaveBeenCalledWith("playback_command", {command:expect.objectContaining({action:"play",tracks:expect.arrayContaining([expect.objectContaining({
      id: remoteTracks[0]!.id, provider: "spotify", providerId: "spotify-track-shared", localPath: `/music/${remoteTracks[0]!.id}.flac`,
    })])})});
    await new Promise(resolve => setTimeout(resolve, 1100));
    await settle();
    const playbackStates = mocks.call.mock.calls.filter(([command,args]) => command === "playback_command" && (args?.command as {action?:string})?.action === "play");
    const lastPlayback = playbackStates[playbackStates.length - 1]?.[1] as { command?: { tracks?: Track[] } } | undefined;
    expect(lastPlayback?.command?.tracks?.map(track => track.id)).toEqual(remoteTracks.map(track => track.id));

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
  }, 20_000);

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

    await openDownloadActivityFromLibrary();
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

  it("exposes explicit transport toggle states, repeat-one badge and stateful tooltips", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const shuffle=document.querySelector<HTMLButtonElement>('.player [aria-label="Shuffle"]')!;
    const autoplay=document.querySelector<HTMLButtonElement>('.player [aria-label="Autoplay radio after queue"]')!;
    for (const button of [shuffle,autoplay]) {
      expect(button.getAttribute("aria-pressed")).toBe("false");expect(button.title).toContain("off");
      button.click();await settle();
      expect(button.getAttribute("aria-pressed")).toBe("true");expect(button.title).toContain("on");expect(button.classList.contains("transport-mode")).toBe(true);
    }
    const repeat=document.querySelector<HTMLButtonElement>('.player [aria-label="Repeat: off"]')!;
    expect(repeat.title).toContain("Repeat: off");repeat.click();await settle();
    expect(repeat.getAttribute("aria-pressed")).toBe("true");expect(repeat.title).toContain("Repeat: queue");
    repeat.click();await settle();expect(repeat.querySelector(".repeat-one-badge")?.textContent).toBe("1");expect(repeat.title).toContain("Repeat: one");
    repeat.click();await settle();expect(repeat.getAttribute("aria-pressed")).toBe("false");expect(repeat.querySelector(".repeat-one-badge")).toBeNull();
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
    expect(document.body.textContent).toContain("Balanced allows metadata and credit variations while preserving the requested recording.");
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
    expect(document.body.textContent).toContain("Choose which alternatives to allow.");
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
      expect(control.closest("details")?.open).toBe(true);
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

  it("keeps the Native queue authoritative when browser checkpoint mirroring fails", async () => {
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
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(JSON.parse(stored.get("burrata.playback-queue.v1")!).tracks).toHaveLength(1);

    await automationAction("queue-save-failure-repeat", "queue", {
      trackIds: [thirdTrack.id], mode: "append",
    });
    expect(document.querySelectorAll('[role="alert"]')).toHaveLength(0);

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

  it("refreshes open track details on native first audio and fences a stale owner", async () => {
    const summaryCounts = new Map([[savedTrack.id, 0], [secondTrack.id, 0]]);
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "get_track_library_summary") {
        const trackId = String(args?.trackId), playCount = summaryCounts.get(trackId) ?? 0;
        return Promise.resolve({ trackId, inLibrary: false, downloaded: false, playCount,
          playedAt: [], playedAtHasMore: false, playlists: [] });
      }
      return originalCall(command, args);
    });
    const initial = makeNativeSnapshot({
      revision: 1, generation: 41, status: "playing", paused: false, track: savedTrack,
      trackId: savedTrack.id, entryId: "native-entry-a", queue: [savedTrack], currentIndex: 0,
      diagnostics: { firstAudioMs: null, operationId: "native-operation-a" },
    });
    nativePlaybackSnapshot = initial;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click();
    await settle(); await settle();
    const readsBeforeStart = mocks.call.mock.calls.filter(([command]) => command === "get_track_library_summary").length;

    // A first-audio owner event is followed by a bounded, delayed reread.
    summaryCounts.set(savedTrack.id, 1);
    const firstAudio = { ...initial, revision: 2, diagnostics: { firstAudioMs: 100, operationId: "native-operation-a" } };
    eventHandlers.get("native-playback-state")?.({ event: "native-playback-state", id: 2, payload: firstAudio });
    // Supersede the owner before its delayed query can land.
    const next = makeNativeSnapshot({
      revision: 3, generation: 42, status: "playing", paused: false, track: secondTrack,
      trackId: secondTrack.id, entryId: "native-entry-b", queue: [secondTrack], currentIndex: 0,
      diagnostics: { firstAudioMs: null, operationId: "native-operation-b" },
    });
    summaryCounts.set(secondTrack.id, 2);
    eventHandlers.get("native-playback-state")?.({ event: "native-playback-state", id: 3, payload: next });
    await new Promise(resolve => setTimeout(resolve, 240));
    await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Another Recording");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Plays2");
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_track_library_summary").length)
      .toBeGreaterThan(readsBeforeStart);
    expect(mocks.call.mock.calls.filter(([command, args]) =>
      command === "get_track_library_summary" && args?.trackId === savedTrack.id,
    )).toHaveLength(1);

    const readsBeforeCurrentOwnerAudio = mocks.call.mock.calls.filter(([command]) => command === "get_track_library_summary").length;
    summaryCounts.set(secondTrack.id, 3);
    eventHandlers.get("native-playback-state")?.({
      event: "native-playback-state", id: 4,
      payload: { ...next, revision: 4, diagnostics: { firstAudioMs: 200, operationId: "native-operation-b" } },
    });
    await new Promise(resolve => setTimeout(resolve, 240));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_track_library_summary").length)
      .toBeGreaterThan(readsBeforeCurrentOwnerAudio);
    expect(document.querySelector(".playing-info")?.textContent).toContain("Plays3");
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
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click();
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

  it("cancels in-flight batch identity preparation before searching providers", async () => {
    configuredMusicDirectory = "/saved/music";
    holdDownloadMatchPreparation = true;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    rightClickPlaylist("Empty Collection");
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Download playlist")!.click();
    await settle();

    const preparation = mocks.call.mock.calls.find(([command]) => command === "prepare_download_match_identity");
    expect(preparation).toBeDefined();
    const requestId = (preparation![1] as { requestId: string }).requestId;
    const stop = [...document.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent?.includes("Stop batch matching"));
    expect(stop).toBeDefined();
    stop!.click();
    expect(mocks.call).toHaveBeenCalledWith("cancel_remote_read", { requestId });
    finishDownloadMatchPreparation?.();
    await settle();
    expect(mocks.call.mock.calls.some(([command]) => command === "find_download_candidates")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
  });

  it("downloads a playlist through the configured fallback when its preferred source has no usable match", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    downloadFallbackEnabled = true;
    downloadFallbackSources = ["youtube"];
    batchInspectionHigh = true;
    batchCandidateOverrides.set(`${collectionTrack.id}:soundcloud`, []);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    rightClickPlaylist("Empty Collection");
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Download playlist")!.click();
    await settle();

    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_download_candidates" &&
      (args as { track: Track; providerId: string }).track.id === collectionTrack.id &&
      (args as { providerId: string }).providerId === "soundcloud")).toBe(true);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_download_candidates" &&
      (args as { track: Track; providerId: string }).track.id === collectionTrack.id &&
      (args as { providerId: string }).providerId === "youtube")).toBe(true);
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate; track: Track } }).request.candidate.provider).toBe("youtube");
    expect((queued?.[1] as { request: { candidate: Candidate; track: Track } }).request.track.id).toBe(collectionTrack.id);
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("reuses a linked playlist source, preserves requested metadata, and continues after a partial failure", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    batchInspectionHigh = true;
    const linkedCandidate: Candidate = { ...youtubeCandidate(collectionTrack), provider: "soundcloud",
      id: "playlist-linked-source", url: "https://soundcloud.com/visible-artist/saved-recording" };
    const linkedTrack: Track = { ...collectionTrack, version: "Club Mix", album: "Requested Album", bpm: 124,
      provenance: { sourceKind: "audioSearch", audioCandidate: linkedCandidate } };
    const unavailable = { ...secondTrack, id: "playlist-unavailable", title: "Unavailable Recording" };
    mockCollectionRows = [linkedTrack, unavailable];
    for (const provider of ["youtube", "soundcloud", "monochrome"]) {
      batchCandidateOverrides.set(`${unavailable.id}:${provider}`, []);
    }
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    rightClickPlaylist("Empty Collection");
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Download playlist")!.click();
    await settle();

    expect(mocks.call.mock.calls.some(([command, args]) => command === "find_candidates" &&
      (args as { track: Track }).track.id === linkedTrack.id)).toBe(false);
    const queued = mocks.call.mock.calls.filter(([command]) => command === "enqueue_download");
    expect(queued).toHaveLength(1);
    const request = (queued[0]![1] as { request: { candidate: Candidate; track: Track } }).request;
    expect(request.candidate.provider).toBe("soundcloud");
    expect(request.track).toMatchObject({ id: linkedTrack.id, version: "Club Mix", album: "Requested Album", bpm: 124 });
    expect(document.body.textContent).toContain("Unavailable Recording");
    expect(document.body.textContent).toContain("no suitable source found");
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("does not batch-download a candidate the shared recording policy rejects and does not open a chooser", async () => {
    configuredMusicDirectory = "/saved/music";
    batchInspectionHigh = true;
    const wrongVersion = { ...youtubeCandidate(collectionTrack), id: "wrong-version", title: "Visible Artist - Existing Collection Member (Live)" };
    batchCandidateOverrides.set(`${collectionTrack.id}:youtube`, [wrongVersion]);
    batchDowngradedCandidateIds.add(wrongVersion.id);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    rightClickPlaylist("Empty Collection");
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Download playlist")!.click();
    await settle();

    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
    expect(document.body.textContent).toContain("Existing Collection Member");
    expect(document.body.textContent).toContain("was not accepted by recording match policy");
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
    // Catalog tracks follow global matching rules; upload-specific controls are only for linked uploads.
    expect(info.querySelector('select[aria-label="Playback source preference"]')).toBeNull();
    expect([...info.querySelectorAll("select")].some(select => [...select.options].some(option => option.textContent?.includes("Favorites")))).toBe(false);
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

  it("opens an exact catalog release and routes its revision-guarded download through the loaded album action", async () => {
    const artist={provider:"musicbrainz",id:"123e4567-e89b-12d3-a456-426614174000"} as const;
    const releaseId="223e4567-e89b-12d3-a456-426614174001";
    const release:ArtistReleaseRef={release:{provider:"musicbrainz",id:releaseId},title:"Exact Release",sourceUrl:`https://musicbrainz.org/release/${releaseId}`,tracks:[],credits:[{name:"Exact Artist",role:"artist",artist,sourceUrl:`https://musicbrainz.org/artist/${artist.id}`}]};
    discoveryAlbumTracks=[{...savedTrack,id:"exact-release-track",sourceUrl:release.sourceUrl,provenance:{catalogProvider:"musicbrainz",releaseId}}];
    configuredMusicDirectory="/music";
    configuredPreferredDownloadSource="youtube";
    let deferSettings=false,finishBatchSettings:((settings:Settings)=>void)|undefined;
    const originalCall=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="get_artist_releases")return {artist,items:[release],nextCursor:null,warnings:[]};
      if(command==="get_settings"&&deferSettings)return await new Promise<Settings>(resolve=>{finishBatchSettings=resolve;});
      return originalCall(command,args);
    });
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:101,payload:{requestId:"exact-release-nav",target:{kind:"catalog_release",catalogArtist:artist,catalogRelease:release.release}}});
    await settle();await settle();
    expect(document.querySelector(".release-view h1")?.textContent).toBe("Exact Release");
    const navigationCompletion=mocks.call.mock.calls.find(([command,args])=>command==="automation_ui_navigation_complete"&&args?.requestId==="exact-release-nav")?.[1];
    expect(navigationCompletion).toMatchObject({destination:releaseId,status:"complete",trackCount:1});
    expect(typeof navigationCompletion?.releaseRevision).toBe("string");
    const operationId="123e4567-e89b-42d3-a456-426614174000";
    const settingsCallsBeforeBatch=mocks.call.mock.calls.filter(([command])=>command==="get_settings").length;
    deferSettings=true;
    eventHandlers.get("automation-release-download")!({event:"automation-release-download",id:102,payload:{operationId,catalogArtist:artist,catalogRelease:release.release,releaseRevision:navigationCompletion!.releaseRevision,sourceProviderId:"slskd"}});
    await settle();
    expect(finishBatchSettings).toBeTypeOf("function");
    const concurrentOperationId="123e4567-e89b-42d3-a456-426614174099";
    eventHandlers.get("automation-release-download")!({event:"automation-release-download",id:103,payload:{operationId:concurrentOperationId,catalogArtist:artist,catalogRelease:release.release,releaseRevision:navigationCompletion!.releaseRevision}});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_layout_complete",expect.objectContaining({requestId:`release-download:${concurrentOperationId}`,layout:expect.objectContaining({status:"busy"})}));
    expect(mocks.call.mock.calls.filter(([command])=>command==="get_settings")).toHaveLength(settingsCallsBeforeBatch+1);
    deferSettings=false;
    finishBatchSettings!(await originalCall("get_settings") as Settings);
    await settle();await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_layout_complete",expect.objectContaining({requestId:`release-download:${operationId}`,layout:expect.objectContaining({requestedCount:1,status:"completed"})}));
    expect(mocks.call).toHaveBeenCalledWith("find_candidates",expect.objectContaining({track:expect.objectContaining({id:"exact-release-track"}),providerIds:["slskd"]}));
    expect(mocks.call.mock.calls.some(([command])=>command==="save_settings")).toBe(false);
    expect(configuredPreferredDownloadSource).toBe("youtube");
    expect(mocks.call.mock.calls.some(([command])=>command==="get_release_tracks")).toBe(true);
  });

  it("opens the exact album plus credited artist as an enabled Soulseek album search", async () => {
    slskdConnectionEnabled = true;
    const artist = { provider: "musicbrainz", id: "650bf385-6f6d-4992-a3b9-779d144920a4" } as const;
    const releaseId = "4d654402-6a53-46c5-bf68-26c19e4d0fa9";
    const albumTitle = "The Parisian Series: Django Reinhardt (Restored Edition '25)";
    const release: ArtistReleaseRef = {
      release: { provider: "musicbrainz", id: releaseId }, title: albumTitle,
      sourceUrl: `https://musicbrainz.org/release/${releaseId}`, tracks: [], credits: [
        { name: "Django Reinhardt", artist, role: "artist credit", sourceUrl: `https://musicbrainz.org/artist/${artist.id}` },
        { name: "Quintette du Hot Club de France", role: "artist credit", sourceUrl: `https://musicbrainz.org/release/${releaseId}` },
      ],
    };
    const sourceTracks = Array.from({ length: 12 }, (_, index) => ({
      ...savedTrack, id: `catalog:${releaseId}:${index + 1}`, artist: index % 2 ? "Django Reinhardt & Stéphane Grappelli" : "Django Reinhardt & Quintette du Hot Club de France",
      title: `Parisian Track ${index + 1}`, album: albumTitle, trackNumber: index + 1, sourceUrl: release.sourceUrl,
    }));
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_artist_releases") return { artist, items: [release], nextCursor: null, warnings: [] };
      if (command === "get_release_tracks") return sourceTracks;
      if (command === "list_search_sources") return [
        { id: "library", name: "Library", kinds: ["track"], enabled: true },
        { id: "musicbrainz", name: "MusicBrainz", kinds: ["album", "artist", "track"], enabled: true },
        { id: "soulseek", name: "Soulseek", kinds: ["album", "track"], enabled: true },
      ];
      return original(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({ event: "automation-ui-navigation", id: 105, payload: {
      requestId: "album-soulseek-search", target: { kind: "catalog_release", catalogArtist: artist, catalogRelease: release.release },
    } });
    await settle(); await settle();
    expect(document.querySelector(".release-view h1")?.textContent).toBe(albumTitle);
    document.querySelector<HTMLDetailsElement>(".album-actions-menu")!.querySelector("summary")!.click();
    const soulseekAction = document.querySelector<HTMLButtonElement>(`[aria-label="Search album ${albumTitle} on Soulseek"]`)!;
    expect(soulseekAction.disabled).toBe(false);
    soulseekAction.click();
    await settle(); await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search music"]')?.value).toBe(`${albumTitle} Django Reinhardt`);
    expect(document.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("soulseek");
    expect(document.querySelector<HTMLButtonElement>('.filters button.active')?.textContent).toBe("Albums");
    expect(mocks.call.mock.calls.some(([command]) => ["enqueue_download", "save_settings", "add_remote_playlist", "create_remote_playlist"].includes(String(command)))).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_release_tracks")).toHaveLength(1);
  });

  it("captures and downloads an exact saved local playlist revision through the playlist batch path", async () => {
    mockCollectionRows=[{...collectionTrack,title:"Stable playlist entry"}];
    configuredMusicDirectory="/music";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    const navigate=eventHandlers.get("automation-ui-navigation")!;
    const target={kind:"local_playlist",collectionId:"collection-empty"};
    navigate({event:"automation-ui-navigation",id:201,payload:{requestId:"local-playlist-nav-stale",target}});await settle();await settle();
    const first=mocks.call.mock.calls.find(([command,args])=>command==="automation_ui_navigation_complete"&&args?.requestId==="local-playlist-nav-stale")?.[1];
    expect(first).toMatchObject({destination:"collection-empty",status:"complete",trackCount:1});
    expect(typeof first?.collectionRevision).toBe("string");
    mockCollectionRows=[{...collectionTrack,title:"Changed after capture"}];
    const settingsBefore=mocks.call.mock.calls.filter(([command])=>command==="get_settings").length;
    eventHandlers.get("automation-local-playlist-download")!({event:"automation-local-playlist-download",id:202,payload:{operationId:"123e4567-e89b-42d3-a456-426614174000",collectionId:"collection-empty",collectionRevision:first!.collectionRevision}});
    await settle();await settle();
    expect(mocks.call).toHaveBeenCalledWith("automation_layout_complete",expect.objectContaining({requestId:"local-playlist-download:123e4567-e89b-42d3-a456-426614174000",layout:expect.objectContaining({status:"failed"})}));
    expect(mocks.call.mock.calls.filter(([command])=>command==="get_settings")).toHaveLength(settingsBefore);

    navigate({event:"automation-ui-navigation",id:203,payload:{requestId:"local-playlist-nav-current",target}});await settle();await settle();
    const current=mocks.call.mock.calls.find(([command,args])=>command==="automation_ui_navigation_complete"&&args?.requestId==="local-playlist-nav-current")?.[1];
    expect(typeof current?.collectionRevision).toBe("string");
    const operationId="123e4567-e89b-42d3-a456-426614174001";
    eventHandlers.get("automation-local-playlist-download")!({event:"automation-local-playlist-download",id:204,payload:{operationId,collectionId:"collection-empty",collectionRevision:current!.collectionRevision}});
    await settle();await settle();
    expect(mocks.call).toHaveBeenCalledWith("list_tracks",{view:"collections",collectionId:"collection-empty"});
    expect(mocks.call).toHaveBeenCalledWith("find_download_candidates",expect.objectContaining({track:expect.objectContaining({id:"collection-recording"})}));
    expect(mocks.call).toHaveBeenCalledWith("automation_layout_complete",expect.objectContaining({requestId:`local-playlist-download:${operationId}`,layout:expect.objectContaining({requestedCount:1})}));
    expect(mocks.call.mock.calls.some(([command])=>command==="save_settings")).toBe(false);
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
    streamingFallbackSources = ["youtube", "soundcloud", "bandcamp"];
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const source=document.querySelector('[aria-label="Streaming fallback 1"]') as HTMLSelectElement;
    source.value="soundcloud";source.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackSources).toEqual(["soundcloud","youtube","bandcamp"]);
    const off=document.querySelector('[aria-label="Try another source if playback fails"]') as HTMLInputElement;
    off.checked=false;off.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackEnabled).toBe(false);expect(document.querySelector('[aria-label="Streaming fallback 1"]')).toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Downloads")!.click();await settle();
    const downloads=document.querySelector('[aria-label="Try another source if download fails"]') as HTMLInputElement;
    downloads.checked=true;downloads.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(downloadFallbackEnabled).toBe(true);expect(streamingFallbackEnabled).toBe(false);
    expect((mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]![1] as {settings:Settings}).settings).toMatchObject({streamingFallbackSources:["soundcloud","youtube","bandcamp"],streamingFallbackPolicyVersion:1,bandcampFallbackEnabled:true});
  });

  it("can disable saved-radio preparation without changing streaming fallback", async () => {
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click();await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const preparation=document.querySelector<HTMLInputElement>('[aria-label="Prepare saved radio songs"]')!;
    expect(preparation.checked).toBe(true);
    preparation.checked=false;preparation.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    const save=mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]!;
    expect((save[1] as {settings:Settings}).settings).toMatchObject({backgroundSourcePreparation:false,streamingFallbackEnabled:true});
  });

  it("defaults the compact playback bar off and persists the general setting", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')?.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")]
      .find((item) => item.textContent?.trim() === "General")!.click();
    await settle();
    const compact = document.querySelector<HTMLInputElement>('[aria-label="Compact playback bar"]')!;
    expect(compact.checked).toBe(false);
    expect(compact.closest("label")?.textContent).toContain("Leave more room for your library");
    compact.checked = true;
    compact.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const saved = mocks.call.mock.calls.filter(([command]) => command === "save_settings").slice(-1)[0]!;
    expect((saved[1] as { settings: Settings }).settings.compactPlaybackBar).toBe(true);
    expect(document.querySelector("footer.player")?.classList.contains("compact")).toBe(true);
  });

  it("defaults M3U mirroring on for older settings, saves the global toggle, and links Library options back to it", async () => {
    configuredPlaylistMirroringEnabled = undefined;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')?.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")]
      .find((item) => item.textContent?.trim() === "Library")!.click();
    await settle();

    const toggle = document.querySelector<HTMLInputElement>('[aria-label="Keep local M3U playlist copies"]')!;
    expect(toggle.checked).toBe(true);
    expect(toggle.closest("label")?.textContent).toContain("playlists, Liked Songs, and listening history as M3U8 files");
    expect(toggle.closest("label")?.textContent).toContain("Manual playlist import and export remain available");
    toggle.checked = false;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const saved = mocks.call.mock.calls.filter(([command]) => command === "save_settings").slice(-1)[0]!;
    expect((saved[1] as { settings: Settings }).settings.playlistMirroringEnabled).toBe(false);
    expect(configuredPlaylistMirroringEnabled).toBe(false);

    document.querySelector<HTMLButtonElement>('[aria-label="Library"]')!.click();
    await settle();
    document.querySelector<HTMLElement>('.library-tools summary[aria-label="Library options"]')!.click();
    const mirroringLink = [...document.querySelectorAll<HTMLButtonElement>(".library-tools button")]
      .find((item) => item.textContent?.includes("M3U mirroring is off"))!;
    expect(mirroringLink).toBeDefined();
    mirroringLink.click();
    await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Keep local M3U playlist copies"]')?.checked).toBe(false);
  });

  it("sets per-playlist M3U inclusion and link overrides, with a metadata-only warning and defaults reset", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')?.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")]
      .find((item) => item.textContent?.trim() === "Library")!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("button")]
      .find((item) => item.textContent?.trim() === "Choose playlists to sync…")!.click();
    await settle();
    const dialog = document.querySelector<HTMLDialogElement>('dialog[aria-label="Local M3U copy settings"]')!;
    expect(dialog).not.toBeNull();
    const playlist = dialog.querySelector<HTMLSelectElement>('[aria-label="Playlist for local M3U copy"]')!;
    playlist.value = "collection-full";
    playlist.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_playlist_mirror_options", { collectionId: "collection-full" });

    const mediaLinks = dialog.querySelector<HTMLInputElement>('[aria-label="Include media links in this playlist copy"]')!;
    expect(mediaLinks.checked).toBe(true);
    mediaLinks.checked = false;
    mediaLinks.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(dialog.textContent).toContain("This copy will contain playlist metadata only");
    const reset = [...dialog.querySelectorAll<HTMLButtonElement>("button")]
      .find((item) => item.textContent?.trim() === "Use current defaults");
    expect(reset).toBeDefined();
    reset!.click();
    await settle();
    expect(dialog.querySelector<HTMLInputElement>('[aria-label="Include media links in this playlist copy"]')?.checked).toBe(true);
    expect(dialog.textContent).not.toContain("This copy will contain playlist metadata only");
    [...dialog.querySelectorAll<HTMLButtonElement>("button")]
      .find((item) => item.textContent?.trim() === "Save playlist copy")!.click();
    await settle();
    const saved = mocks.call.mock.calls.filter(([command]) => command === "set_playlist_mirror_options").slice(-1)[0]!;
    expect(saved[1]).toMatchObject({ collectionId: "collection-full", options: { included: true, includeMediaLinks: null, includeYtdlpSearch: null } });
    dialog.querySelector<HTMLButtonElement>('[aria-label="Close local M3U settings"]')!.click();
    document.querySelector<HTMLButtonElement>(".library-sidebar button[aria-label='Library']")!.click();
    await settle();
    expect(document.querySelector("main h1")?.textContent).toBe("Library");
    const renderedPlaylist = importedPlaylistCard("Latest Saved Import") ?? collectionCard("Saved Collection");
    expect(renderedPlaylist).toBeDefined();
    renderedPlaylist.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 340, clientY: 200 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>('dialog.playlist-menu[aria-label="Actions for playlist Saved Collection"]')!;
    expect([...menu.querySelectorAll<HTMLButtonElement>("button")].some((button) => menuActionText(button) === "Local M3U copy…")).toBe(true);
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => menuActionText(button) === "Local M3U copy…")!.click();
    await settle();
    expect(document.querySelector<HTMLDialogElement>('dialog[aria-label="Local M3U copy settings"]')).not.toBeNull();
  });

  it("saves playback preferences without saving an unfinished music-folder edit", async () => {
    configuredMusicDirectory="/saved/music";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Library")!.click();await settle();
    const folder=document.querySelector('input[placeholder="/home/you/Music/DJ"]') as HTMLInputElement;
    folder.value="/unsaved/music";folder.dispatchEvent(new Event("input",{bubbles:true}));
    [...document.querySelectorAll<HTMLButtonElement>('.settings-nav button')].find(item=>item.textContent?.trim()==="Playback")!.click();await settle();
    const delay=document.querySelector('input[type="number"][max="5000"]') as HTMLInputElement;
    delay.value="500";delay.dispatchEvent(new Event("input",{bubbles:true}));delay.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    const save=mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]!;
    expect((save[1] as {settings:Settings}).settings).toMatchObject({musicDirectory:"/saved/music",previewDelayMs:500});
    expect(folder.value).toBe("/unsaved/music");
  });

  it("serializes rapid individual preference saves without losing changes or a folder draft", async () => {
    configuredMusicDirectory = "/saved/music";
    const original = mocks.call.getMockImplementation()!;
    let persisted: Settings | undefined;
    let releaseFirst: (() => void) | undefined;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_settings" && persisted) return { ...persisted };
      if (command === "save_settings") {
        const next = args!.settings as Settings;
        if (!releaseFirst) await new Promise<void>(resolve => { releaseFirst = resolve; });
        persisted = { ...next };
        return undefined;
      }
      return original(command, args);
    });
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find(button => button.textContent?.trim() === "Library")!.click(); await settle();
    const folder = document.querySelector<HTMLInputElement>('input[placeholder="/home/you/Music/DJ"]')!;
    folder.value = "/unsaved/music"; folder.dispatchEvent(new Event("input", { bubbles: true }));
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find(button => button.textContent?.trim() === "General")!.click(); await settle();
    const theme = [...document.querySelectorAll<HTMLLabelElement>("label")].find(label => label.textContent?.trim().startsWith("Theme"))!.querySelector("select")!;
    theme.value = "light"; theme.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const compact = document.querySelector<HTMLInputElement>('[aria-label="Compact playback bar"]')!;
    compact.checked = true; compact.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "save_settings")).toHaveLength(1);
    releaseFirst?.(); await settle(); await settle();
    expect(persisted).toMatchObject({ theme: "light", compactPlaybackBar: true, musicDirectory: "/saved/music" });
    expect(folder.value).toBe("/unsaved/music");
  });

  it("opens the AcoustID connection from download settings", async () => {
    const previousScrollIntoView = HTMLElement.prototype.scrollIntoView;
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
    try {
      const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
      document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click(); await settle();
      [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find(button => button.textContent?.trim() === "Downloads")!.click(); await settle();
      [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Set up connection")!.click(); await settle();
      expect(document.querySelector('.settings-nav [aria-current="page"]')?.textContent).toBe("Connections");
      const connection = document.querySelector<HTMLDetailsElement>("#acoustid-connection")!;
      expect(connection.open).toBe(true);
      expect(document.activeElement).toBe(connection.querySelector("summary"));
    } finally {
      if (previousScrollIntoView) Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: previousScrollIntoView });
      else delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
    }
  });

  it("saves preferred source immediately without persisting unrelated typed settings", async () => {
    configuredMusicDirectory = "/saved/music";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!
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

  it("keeps loudness analysis active when native cancellation rejects and allows a retry", async () => {
    const localTrack: Track = { ...savedTrack, assetId: "asset-cancel", localPath: "/music/cancel.flac", localAvailability: "available" };
    savedChartTracks = [localTrack];
    savedAssets = [{ id: "asset-cancel", recordingId: localTrack.id, path: localTrack.localPath!, hash: "cancel-hash",
      sourceCodec: "flac", finalCodec: "flac", processing: "ready", track: localTrack }];
    const originalCall = mocks.call.getMockImplementation()!;
    let rejectNextCancel = true;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_loudness_settings") return { mode: "track", targetLufs: -18, ceilingDbtp: -1,
        maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
      if (command === "start_loudness_analysis") return "loudness-cancel-job";
      if (command === "get_loudness_analysis_status") return { jobId: "loudness-cancel-job", status: "running", total: 1, completed: 0, error: null };
      if (command === "cancel_loudness_analysis") {
        if (rejectNextCancel) { rejectNextCancel = false; throw new Error("worker is busy"); }
        return undefined;
      }
      return originalCall(command, args);
    });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll(".shared-track-list > li")].find(row => row.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const tools = document.querySelector<HTMLDetailsElement>(".header-library-tools")!;
    tools.querySelector("summary")!.click();
    tools.querySelector<HTMLButtonElement>('[aria-label="Analyze selected loudness"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("start_loudness_analysis", { assetIds: ["asset-cancel"] });
    expect(document.body.textContent).toContain("Analyzing loudness…");

    document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find(button => button.textContent?.trim() === "Playback")!.click();
    await settle();
    const analysisSection = document.querySelector<HTMLElement>('[aria-label="Selected loudness analysis"]')!;
    const cancelButton = () => [...analysisSection.querySelectorAll<HTMLButtonElement>("button")]
      .find(button => button.textContent?.trim() === "Cancel loudness analysis");
    cancelButton()!.click();
    await settle();
    expect(analysisSection.textContent).toContain("Analyzing 0 of 1 selected tracks");
    expect(analysisSection.textContent).toContain("still running because cancellation failed");
    expect(cancelButton()).toBeDefined();
    const pollsAfterFailure = mocks.call.mock.calls.filter(([command]) => command === "get_loudness_analysis_status").length;
    await new Promise(resolve => setTimeout(resolve, 700));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "get_loudness_analysis_status").length).toBeGreaterThan(pollsAfterFailure);

    cancelButton()!.click();
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "cancel_loudness_analysis")).toHaveLength(2);
    expect(analysisSection.textContent).toContain("Analysis paused");
    expect(cancelButton()).toBeUndefined();
  });

  it("passes album loudness identity only for direct album playback occurrences", async () => {
    previewMode = "direct";
    const artist = { provider: "musicbrainz", id: "123e4567-e89b-12d3-a456-426614174000" } as const;
    const release: ArtistReleaseRef = {
      release: { provider: "musicbrainz", id: "223e4567-e89b-42d3-a456-426614174001" },
      title: "Context Bound Album", year: 2024, primaryType: "Album", trackCount: 2,
      sourceUrl: "https://musicbrainz.org/release/album-loudness-context", tracks: [],
      credits: [{ name: "Visible Artist", role: "artist", artist, sourceUrl: `https://musicbrainz.org/artist/${artist.id}` }],
    };
    const albumTracks = [
      { ...savedTrack, id: "album-context-one", title: "Album Context One", album: release.title, localAvailability: "available", localPath: "/music/album-one.flac" },
      { ...savedTrack, id: "album-context-two", title: "Album Context Two", album: release.title, localAvailability: "available", localPath: "/music/album-two.flac" },
    ];
    discoveryAlbumTracks = albumTracks;
    savedAssets = albumTracks.map((track, index) => ({
      id: `album-asset-${index + 1}`, recordingId: track.id, path: track.localPath!, hash: String.fromCharCode(97 + index).repeat(64),
      sourceCodec: "flac", finalCodec: "flac", processing: "ready", track,
    }));
    const albumResource = {
      scope: "album" as const,
      members: savedAssets.map(asset => ({ assetId: asset.id, fileHash: asset.hash, range: { startSeconds: 0, endSeconds: null } })),
      interTrackGapsSeconds: [0], resourceKey: "f".repeat(64),
    };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_artist_releases") return { artist, items: [release], nextCursor: null, warnings: [] };
      if (command === "get_release_tracks") return albumTracks;
      if (command === "get_loudness_settings") return { mode: "album", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
      if (command === "set_loudness_album_context") return undefined;
      if (command === "start_loudness_analysis") return "album-loudness-job";
      if (command === "get_loudness_analysis_status") return { jobId: "album-loudness-job", status: "completed", total: 2, completed: 2, error: null, resource: albumResource };
      if (command === "get_loudness_decision_for_asset") return { mode: "album", status: "applied", appliedGainDb: -2 };
      if (command === "resolve_preview") {
        const track = args?.track as Track;
        const base = await originalCall(command, args) as Record<string, unknown>;
        const asset = savedAssets.find(item => item.recordingId === track.id);
        return asset ? { ...base, assetId: asset.id, normalizationRoute: "same-origin-relay" } : base;
      }
      return originalCall(command, args);
    });

    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-ui-navigation")!({
      event: "automation-ui-navigation", id: 501,
      payload: { requestId: "album-loudness-context", target: { kind: "catalog_release", catalogArtist: artist, catalogRelease: release.release } },
    });
    await settle(); await settle();
    document.querySelector<HTMLButtonElement>(".album-loudness-analysis button")!.click();
    await settle(); await settle();
    expect(document.body.textContent).toContain("Play or queue this release to use its measured album gain.");
    document.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click();
    await settle(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", {command:expect.objectContaining({action:"play",loudnessAlbumResource:albumResource})});
    document.querySelector<HTMLButtonElement>('[aria-label="Play Album Context Two"]')!.click();
    await settle(); await settle();
    const starts = mocks.call.mock.calls.filter(([command]) => command === "playback_command");
    expect(starts[starts.length-1]?.[1]?.command).toMatchObject({loudnessAlbumResource:null});
  });

  it("downloads every album track through the automatic fallback path", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    downloadFallbackEnabled = true;
    downloadFallbackSources = ["youtube"];
    batchInspectionHigh = true;
    discoveryAlbum = {
      release: { provider: "musicbrainz", id: "download-album-1" }, title: "Download Album", year: 2022,
      primaryType: "Album", trackCount: 2, sourceUrl: "https://musicbrainz.org/release/download-album-1", tracks: [],
      credits: [{ name: "Visible Artist", role: "artist", artist: { provider: "musicbrainz", id: "artist-1" }, sourceUrl: "https://musicbrainz.org/artist/artist-1" }],
    };
    discoveryAlbumTracks = [
      { ...savedTrack, id: "download-album-track-1", title: "Album Song One", album: "Download Album", trackNumber: 1 },
      { ...secondTrack, id: "download-album-track-2", title: "Album Song Two", album: "Download Album", trackNumber: 2 },
    ];
    for (const track of discoveryAlbumTracks) batchCandidateOverrides.set(`${track.id}:soundcloud`, []);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find((item) => item.textContent?.includes("Search"))!.click();
    await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Albums")!.click();
    await settle();
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Download Album"; query.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    expect(document.querySelector(".release-view h1")?.textContent).toBe("Download Album");
    [...document.querySelectorAll<HTMLButtonElement>(".hero-actions button")].find(button => button.textContent?.includes("Download album"))!.click();
    await settle();

    const queued = mocks.call.mock.calls.filter(([command]) => command === "enqueue_download");
    expect(queued).toHaveLength(2);
    expect(queued.map(([, args]) => (args as { request: { track: Track; candidate: Candidate } }).request.track.id))
      .toEqual(discoveryAlbumTracks.map(track => track.id));
    expect(queued.every(([, args]) => (args as { request: { candidate: Candidate } }).request.candidate.provider === "youtube")).toBe(true);
    expect(queued.map(([, args]) => (args as { request: { track: Track } }).request.track.album))
      .toEqual(["Download Album", "Download Album"]);
    expect(document.querySelector("dialog.download-dialog")).toBeNull();
  });

  it("uses one lazy Soulseek album search and queues its matched peer files per track", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "slskd";
    downloadFallbackEnabled = false;
    discoveryAlbum = {
      release: { provider: "musicbrainz", id: "soulseek-download-album-1" }, title: "Album", year: 2022,
      primaryType: "Album", trackCount: 2, sourceUrl: "https://musicbrainz.org/release/soulseek-download-album-1", tracks: [],
      credits: [{ name: "Visible Artist", role: "artist", artist: { provider: "musicbrainz", id: "artist-1" }, sourceUrl: "https://musicbrainz.org/artist/artist-1" }],
    };
    discoveryAlbumTracks = [
      { ...savedTrack, id: "soulseek-album-track-1", title: "Album Song One", album: "Album", trackNumber: 1 },
      { ...secondTrack, id: "soulseek-album-track-2", title: "Album Song Two", album: "Album", trackNumber: 2 },
    ];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>("nav button")].find((item) => item.textContent?.includes("Search"))!.click();
    await settle();
    await selectSearchSources(["MusicBrainz"]);
    [...document.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent === "Albums")!.click();
    await settle();
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    query.value = "Album"; query.dispatchEvent(new Event("input", { bubbles: true }));
    submitSearch(); await settle();
    document.querySelector<HTMLButtonElement>(".open-result")!.click(); await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".hero-actions button")].find(button => button.textContent?.includes("Download album"))!.click();
    await settle();

    const searches = mocks.call.mock.calls.filter(([command]) => command === "find_soulseek_album_candidates");
    expect(searches).toHaveLength(1);
    expect((searches[0]![1] as { tracks: Track[] }).tracks.map((track) => track.id))
      .toEqual(discoveryAlbumTracks.map((track) => track.id));
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
    const queued = mocks.call.mock.calls.filter(([command]) => command === "enqueue_download");
    expect(queued).toHaveLength(2);
    expect(queued.map(([, args]) => (args as { request: { candidate: Candidate; track: Track } }).request.candidate.provider))
      .toEqual(["slskd", "slskd"]);
    expect(queued.map(([, args]) => (args as { request: { track: Track } }).request.track.id))
      .toEqual(discoveryAlbumTracks.map((track) => track.id));
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
    expect(mocks.call).toHaveBeenCalledWith("playback_command", {command:expect.objectContaining({action:"play",tracks:expect.arrayContaining([expect.objectContaining({
      id: savedTrack.id, provider: savedTrack.provider, providerId: savedTrack.providerId, localPath: "/music/saved.flac", assetId: "asset-saved",
    })])})});
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

  it("announces when a background approximate album excerpt becomes ready without autoplay", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const startsBefore = mocks.call.mock.calls.filter(([command]) => ["resolve_playback_track", "play_tracks", "playback_command"].includes(String(command))).length;

    eventHandlers.get("playback-source-ready")?.({ event: "playback-source-ready", id: 1, payload: {
      trackId: savedTrack.id, title: "Saved Recording", provider: "approximateAlbumExcerpt",
    } });
    await settle();

    expect(document.querySelector(".banner.notice[role='status']")?.textContent).toContain("An approximate album excerpt is ready for Saved Recording. You can play it again.");
    expect(mocks.call.mock.calls.filter(([command]) => ["resolve_playback_track", "play_tracks", "playback_command"].includes(String(command)))).toHaveLength(startsBefore);
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
    expect(document.querySelector(".player-status")?.textContent).toContain("unavailable");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("uploader disabled this video’s audio stream");
    expect(mocks.call.mock.calls.filter(([command,args]) => command === "playback_command" && (args?.command as {action?:string})?.action === "play")).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
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

  it("auditions the selected download source through Native without downloading or editing the recording", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await selectDownloadSource("soundcloud");
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('[aria-label="Preview selected source"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({
      action: "play", candidateOnly: true, tracks: [expect.objectContaining({ id: savedTrack.id })],
      candidate: expect.objectContaining({ provider: "soundcloud", url: "https://soundcloud.com/visible-artist/saved-recording" }),
    }) });
    expect(mocks.call.mock.calls.some(([command]) => ["enqueue_download", "save_track", "update_track", "start_candidate_preview"].includes(String(command)))).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Pause selected source preview"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "pause" } });
    expect(document.querySelector("dialog.download-dialog")).not.toBeNull();
  });

  it.each(["resolving", "buffering"] as const)("pauses a selected source preview while Native is %s without restarting playback", async (status) => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll(".shared-track-list > li")].find(item => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await selectDownloadSource("soundcloud");
    document.querySelector<HTMLButtonElement>('[aria-label="Preview selected source"]')!.click();
    await settle();

    // Native reports paused=true until output is ready, including seek buffering.
    nativePlaybackSnapshot = {...nativePlaybackSnapshot!, revision:nativePlaybackSnapshot!.revision+1,
      status, paused:true, position:77, playerMessage:"Buffering audio…"};
    eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:nativePlaybackSnapshot.revision,payload:nativePlaybackSnapshot});
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>('[aria-label="Pause selected source preview"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "pause" } });
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command" && (args as {command?:{action?:string}})?.command?.action === "play")).toBe(false);
    expect(nativePlaybackSnapshot?.position).toBe(77);

    document.querySelector<HTMLButtonElement>('[aria-label="Preview selected source"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: { action: "resume" } });
    expect(nativePlaybackSnapshot?.position).toBe(77);
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

  async function liveRecoveryFixture() {
    vi.useFakeTimers();
    let clock = 1_000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    radioStationState = [{ id:"main",name:"Main Mix",streamUrl:"https://radio.example/live",sourceUrl:"https://radio.example",provider:"radio_paradise",isCustom:false }];
    vi.spyOn(HTMLMediaElement.prototype,"duration","get").mockReturnValue(Infinity);
    vi.spyOn(HTMLMediaElement.prototype,"readyState","get").mockReturnValue(2);
    vi.spyOn(HTMLMediaElement.prototype,"paused","get").mockReturnValue(false);
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => Promise.resolve());
    const pause = vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")].find(button=>button.textContent?.includes("Main Mix"))!.click();await settle();
    const advance = async (milliseconds:number) => {clock+=milliseconds;await vi.advanceTimersByTimeAsync(milliseconds);await settle();};
    return {play,pause,advance,setClock:(value:number)=>{clock=value;}};
  }

  async function loadedResumeFixture() {
    savedChartTracks=[savedTrack,secondTrack];
    previewMode="fallback";automationPlaybackAck={protocolVersion:1,accepted:true,needFull:false};
    const play=vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(playAndAdvanceMediaClock);
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
    nativePlaybackSnapshot=snapshot; explicitNativeFixture = true;
    const instance=mount(App,{target:document.body});
    unmountApp=()=>{void unmount(instance);};
  }
  function emitNative(snapshot:NativePlaybackSnapshot) {
    eventHandlers.get("native-playback-state")?.({event:"native-playback-state",id:snapshot.revision,payload:snapshot});
  }
  it("supports one-time Native stream analysis while automatic analysis stays off", async () => {
    const original=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async(command,args)=>{
      if(command==="start_stream_media_analysis")return {jobId:"native-manual",state:"queued"};
      if(command==="get_selected_stream_analysis"&&args?.jobId==="native-manual")return {jobId:"native-manual",state:"completed",completed:1,total:1,results:[{trackId:savedTrack.id,status:"failed",error:"Fixture analysis finished"}]};
      return original(command,args);
    });
    mountNative(makeNativeSnapshot({status:"playing",track:savedTrack,trackId:savedTrack.id,candidate:youtubeCandidate(savedTrack),queue:[savedTrack],currentIndex:0,paused:false,duration:240}));await settle();
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{requestId:"manual-native-analysis",target:{kind:"view",view:"settings",settingsCategory:"playback"}}});await settle();await settle();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Analyze full streamed tracks in background"]')?.checked).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Analyze full stream for BPM, key, and waveform"]')!.click();await settle();await settle();
    expect(mocks.call).toHaveBeenCalledWith("start_stream_media_analysis",expect.objectContaining({automatic:false,provider:"youtube"}));
    expect(mocks.call.mock.calls.some(([command])=>command==="cancel_selected_stream_analysis")).toBe(false);
    expect(document.querySelector<HTMLInputElement>('[aria-label="Analyze full streamed tracks in background"]')?.checked).toBe(false);
    expect(document.querySelector("audio")).toBeNull();
  });

  function expectNoPlaybackStart() {
    const starts = new Set(["play", "select", "resume", "next", "seek"]);
    expect(mocks.call.mock.calls.some(([command, args]) => command === "playback_command"
      && starts.has(String((args as { command?: { action?: string } } | undefined)?.command?.action)))).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "play_tracks" || command === "resolve_preview")).toBe(false);
  }

  it("dispatches reviewed portable tracks to native without preflight or stale candidate", async () => {
    const stale = { ...youtubeCandidate(savedTrack), id: "youtube:stale" };
    const track: Track = { ...savedTrack, provenance: { sourceKind: "audioSearch", userSelectedSource: true,
      audioCandidate: stale, portablePlaybackSource: { provider: "youtube", sourceId: "reviewed", matchingLevel: "exact", sourceHash: "a".repeat(64) } } };
    savedChartTracks = [track, secondTrack];
    mountNative(makeNativeSnapshot()); await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${track.title}"]`)!.click(); await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("resolve_portable_playback_candidate", expect.anything());
    const playCall = mocks.call.mock.calls.find(([command, args]) => command === "playback_command" && (args as { command?: { action?: string } })?.command?.action === "play");
    expect(playCall?.[1]).toMatchObject({ command: { tracks: [track, secondTrack], candidate: undefined } });
  });

  it("does not select a native queue occurrence with an outdated portable binding", async () => {
    const binding = { provider: "youtube", sourceId: "reviewed", matchingLevel: "exact", sourceHash: "a".repeat(64) };
    const track: Track = { ...savedTrack, provenance: { portablePlaybackSource: binding } };
    const stale: Track = { ...track, provenance: { portablePlaybackSource: { ...binding, sourceId: "previous" } } };
    savedChartTracks = [track, secondTrack];
    mountNative(makeNativeSnapshot({ queue: [stale, secondTrack], queueEntryIds: ["first", "second"], currentIndex: 0 }));
    await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${track.title}"]`)!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({ action: "play", tracks: [track, secondTrack], candidate: undefined }) });
    expect(mocks.call).not.toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({ action: "select" }) });
  });

  it.each(["soundcloud", "youtube"] as const)("keeps a downloaded %s Library row implicit when only the registered file made it authoritative", async provider => {
    const providerId = provider === "soundcloud" ? "219610160" : "abcdefghijk";
    const url = provider === "soundcloud" ? "https://soundcloud.com/fixture/remembered" : "https://www.youtube.com/watch?v=abcdefghijk";
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), id: `${provider}:${providerId}`, provider, url, title: "Remembered Source" };
    const downloaded: Track = { ...savedTrack, id: `${provider}:${providerId}`, provider, providerId, sourceUrl: url,
      title: "Remembered Source", artist: "", localPath: "/registered/corrupt.m4a", assetId: "registered-source-asset", localAvailability: "available",
      provenance: { sourceKind: "audioSearch", identity: "unverified source search result", manualFields: {}, audioCandidate: candidate } };
    const original = JSON.stringify(downloaded);
    savedChartTracks = [downloaded];
    mountNative(makeNativeSnapshot()); await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${downloaded.title}"]`)!.click(); await settle();
    const playCall = mocks.call.mock.calls.find(([command, args]) => command === "playback_command"
      && (args as { command?: { action?: string } } | undefined)?.command?.action === "play");
    expect(playCall?.[1]).toMatchObject({ command: { candidate: undefined, tracks: [downloaded] } });
    expect(JSON.stringify(downloaded)).toBe(original);
    expect(mocks.call.mock.calls.some(([command]) => ["update_track", "save_track", "delete_asset"].includes(String(command)))).toBe(false);
  });

  it.each(["userSelectedSource", "explicitSourceLock", "sourceMode"] as const)("preserves explicit %s candidate intent on a downloaded Library row", async lock => {
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), id: "soundcloud:219610160", provider: "soundcloud", url: "https://soundcloud.com/fixture/remembered" };
    const provenance = {sourceKind:"audioSearch",audioCandidate:candidate,...(lock === "sourceMode" ? {playbackMode:"source"} : {[lock]:true})};
    const downloaded: Track = {...savedTrack,localPath:"/registered/corrupt.m4a",assetId:"registered-asset",provenance};
    savedChartTracks=[downloaded];mountNative(makeNativeSnapshot());await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${downloaded.title}"]`)!.click();await settle();
    const playCall=mocks.call.mock.calls.find(([command,args])=>command==="playback_command"&&(args as {command?:{action?:string}})?.command?.action==="play");
    expect(playCall?.[1]).toMatchObject({command:{candidate,tracks:[downloaded]}});
  });

  it("does not pin a remembered Monochrome candidate when starting a catalog track", async () => {
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), id: "monochrome:remembered", provider: "monochrome",
      url: "https://tracks.monochrome.st/track/remembered" };
    const catalogTrack: Track = { ...savedTrack, provenance: { catalogProvider: "beatport", audioCandidate: candidate } };
    savedChartTracks = [catalogTrack];
    mountNative(makeNativeSnapshot());
    await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${catalogTrack.title}"]`)!.click();
    await settle();
    const playCall = mocks.call.mock.calls.find(([command, args]) => command === "playback_command"
      && (args as { command?: { action?: string } } | undefined)?.command?.action === "play");
    expect(playCall?.[1]).toMatchObject({ command: { candidate: undefined } });
  });

  it.each([false,true])("honors playlist URL override=%s before dispatching native playback", async override => {
    overridePlaylistTrackUrls=override;
    const candidate=youtubeCandidate(savedTrack);
    const imported:Track={...savedTrack,provider:"import",sourceUrl:candidate.url,provenance:{sourceKind:"audioSearch",audioCandidate:candidate}};
    savedChartTracks=[imported]; mountNative(makeNativeSnapshot()); await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${imported.title}"]`)!.click(); await settle();
    const playCall=mocks.call.mock.calls.find(([command,args])=>command === "playback_command" && (args as {command?:{action?:string}})?.command?.action === "play");
    expect(playCall?.[1]).toMatchObject({command:{candidate:undefined, tracks:[imported]}});
  });

  it.each(["stream", "localAsset"])("shows active Native %s quality for a recording with a saved local file", async sourceKind => {
    const track: Track = { ...savedTrack, localPath: "/music/saved.wav", assetId: "quality-local-asset" };
    savedAssets = [{ id: track.assetId!, recordingId: track.id, path: track.localPath!, hash: "quality-hash",
      sourceCodec: "pcm_s16le", finalCodec: "pcm_s16le", finalBitrate: 1536, sampleRate: 48000, channels: 2,
      processing: "ready", track }];
    mountNative(makeNativeSnapshot({ status: "playing", paused: false, trackId: track.id, track,
      candidate: sourceKind === "stream" ? youtubeCandidate(track) : null, queue: [track], currentIndex: 0,
      quality: sourceKind === "stream"
        ? { sourceKind, codec: "mp4a.40.2", bitrateKbps: 129, sampleRate: 44100, channels: 2 }
        : { sourceKind, assetId: track.assetId! } }));
    await settle();
    const assertQuality = (element: Element | null) => {
      expect(element?.textContent).toContain(sourceKind === "stream" ? "MP4A.40.2" : "PCM_S16LE");
      expect(element?.textContent).toContain(sourceKind === "stream" ? "129 kbps" : "1536 kbps");
      expect(element?.textContent).not.toContain(sourceKind === "stream" ? "PCM_S16LE" : "MP4A.40.2");
    };
    assertQuality(document.querySelector("footer .playback-quality"));
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    assertQuality(document.querySelector(".playing-info .playback-quality"));
  });

  it("labels a saved local alternative using retained native quality evidence", async () => {
    const local={...savedTrack,title:"Song (Live)",artist:"Artist",localPath:"/music/live.flac"};
    const matching={assessment:{action:"accept",relationship:"alternative",alternativeKind:"performance"},resolvedRecording:{title:"Song (Live)"},requestedRecording:{title:"Song",artist:"Artist"}};
    mountNative(makeNativeSnapshot({status:"playing",paused:false,trackId:local.id,track:local,queue:[local],currentIndex:0,quality:{matching} as unknown as NativePlaybackSnapshot["quality"]}));
    await settle();
    expect(document.body.textContent).toContain("Other performance");
    expect(document.body.textContent).toContain("Requested: Song · Artist");
  });

  it("preserves an explicit audioSearch Monochrome source when starting a native track", async () => {
    const candidate: Candidate = { ...youtubeCandidate(savedTrack), id: "monochrome:explicit", provider: "monochrome",
      url: "https://tracks.monochrome.st/track/explicit" };
    const explicitTrack: Track = { ...savedTrack, provenance: { sourceKind: "audioSearch", userSelectedSource: true, audioCandidate: candidate } };
    savedChartTracks = [explicitTrack];
    mountNative(makeNativeSnapshot());
    await settle();
    document.querySelector<HTMLButtonElement>(`[aria-label="Play ${explicitTrack.title}"]`)!.click();
    await settle();
    const playCall = mocks.call.mock.calls.find(([command, args]) => command === "playback_command"
      && (args as { command?: { action?: string } } | undefined)?.command?.action === "play");
    expect(playCall?.[1]).toMatchObject({ command: { candidate } });
  });

  it("shows a camelCase cached stream estimate and rejects stale track or generation results", async () => {
    const track: Track = { ...savedTrack, id: "bandcamp-recording-2039312545", durationSeconds: 162.891 };
    const candidate: Candidate = {
      ...youtubeCandidate(track), id: "bandcamp:2039312545", provider: "bandcamp",
      title: "Amynedd Dingle Dell", uploader: "Amynedd", durationSeconds: 162.891,
    };
    const cached = {
      schemaVersion: 1,
      analyzerVersion: "burrata-whole-track-analysis-v1-bpm-v4-keyfinder-2.2.8",
      resource: {
        kind: "stream" as const,
        recordingId: track.id,
        provider: "bandcamp",
        sourceIdentitySha256: "e".repeat(64),
      },
      durationSeconds: 162.891,
      scope: "wholeTrack" as const,
      coverage: 1.0000026,
      measuredAt: "2026-10-26T00:18:06.348396951Z",
      bpm: { bpm: 95.1, confidence: 0.48, relatedTempoAlternativeBpm: null },
      key: null,
    };
    const originalCall = mocks.call.getMockImplementation()!;
    let resolveStaleGeneration: ((value: unknown) => void) | undefined;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_cached_stream_analysis") {
        if (args?.trackId === track.id && args?.generation === 9) return cached;
        if (args?.trackId === track.id && args?.generation === 10) {
          return new Promise(resolve => { resolveStaleGeneration = resolve; });
        }
        // Simulate a delayed old-record reply for another track. The frontend
        // must keep its owner fence even if an IPC response is stale/misbound.
        return cached;
      }
      return originalCall(command, args);
    });

    const first = makeNativeSnapshot({
      revision: 1, generation: 9, status: "playing", paused: false,
      track, trackId: track.id, candidate, queue: [track], currentIndex: 0,
      duration: 162.891, volume: 0,
    });
    mountNative(first);
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_cached_stream_analysis", { trackId: track.id, generation: 9 });
    expect(document.querySelector("footer.player")?.textContent).not.toContain("Whole-track analysis");
    expect(document.querySelector("footer.player")?.textContent).not.toContain("95.1 BPM");

    emitNative({ ...first, revision: 2, generation: 10 });
    await settle();
    expect(resolveStaleGeneration).toBeDefined();
    const otherTrack: Track = { ...track, id: "different-stream-recording" };
    emitNative({
      ...first, revision: 3, generation: 11, track: otherTrack, trackId: otherTrack.id,
      candidate: { ...candidate, id: "bandcamp:another-recording" }, queue: [otherTrack],
    });
    await settle();
    resolveStaleGeneration!(cached);
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_cached_stream_analysis", { trackId: otherTrack.id, generation: 11 });
    expect(document.querySelector(".now-playing")?.textContent).not.toContain("95.1 BPM");
    expectNoPlaybackStart();
  });

  it("uses generation and resource scoped stream-analysis events without recurring cache polls", async () => {
    const track: Track = { ...savedTrack, id: "event-stream-track", durationSeconds: 162.891 };
    const candidate: Candidate = { ...youtubeCandidate(track), id: "youtube:event-stream-source", provider: "youtube" };
    const digest = await streamSourceIdentitySha256(candidate.provider, candidate.id);
    const analysis = {
      schemaVersion: 1,
      analyzerVersion: "burrata-whole-track-analysis-v1-bpm-v4-keyfinder-2.2.8",
      resource: { kind: "stream" as const, recordingId: track.id, provider: candidate.provider, sourceIdentitySha256: digest },
      durationSeconds: 162.891, scope: "wholeTrack" as const, coverage: 1,
      measuredAt: "2026-11-02T00:00:00Z", bpm: { bpm: 95.1, confidence: 0.48, relatedTempoAlternativeBpm: null }, key: null,
    };
    const initial = makeNativeSnapshot({
      revision: 1, generation: 33, status: "playing", paused: false,
      track, trackId: track.id, candidate, queue: [track], currentIndex: 0,
      duration: track.durationSeconds ?? 162.891, volume: 0,
    });
    const originalCall = mocks.call.getMockImplementation()!;
    let resolveInitialCache: ((value: unknown) => void) | undefined;
    let cacheReadCount = 0;
    let digestCalls = 0;
    let pendingDigests = 0;
    const nativeDigest = crypto.subtle.digest.bind(crypto.subtle);
    vi.spyOn(crypto.subtle, "digest").mockImplementation(async (algorithm, data) => {
      digestCalls++;
      pendingDigests++;
      try { return await nativeDigest(algorithm, data); }
      finally { pendingDigests--; }
    });
    const waitForAnalysisDigests = async (previousCalls: number) => {
      await vi.waitFor(() => {
        expect(digestCalls).toBeGreaterThan(previousCalls);
        expect(pendingDigests).toBe(0);
      });
      await settle();
    };
    mocks.call.mockImplementation((command, args) => {
      if (command === "get_cached_stream_analysis") {
        cacheReadCount++;
        expect(args).toEqual({ trackId: track.id, generation: 33 });
        return new Promise(resolve => { resolveInitialCache = resolve; });
      }
      return originalCall(command, args);
    });
    mountNative(initial);
    await settle();
    expect(resolveInitialCache).toBeDefined();
    eventHandlers.get("stream-analysis-updated")?.({ event: "stream-analysis-updated", id: 1, payload: {
      generation: 32, trackId: track.id, resourceIdentity: "unrelated-resource",
      sourceIdentitySha256: digest, audioVariantSha256: null, provider: candidate.provider,
      sourceId: candidate.id, state: "started",
    } });
    await settle();
    const beforeCacheDigest = digestCalls;
    resolveInitialCache!(analysis);
    await waitForAnalysisDigests(beforeCacheDigest);
    eventHandlers.get("automation-ui-navigation")?.({ event: "automation-ui-navigation", id: 2, payload: {
      requestId: "stream-analysis-event-settings", target: { kind: "view", view: "settings", settingsCategory: "playback" },
    } });
    await settle();

    expect(document.body.textContent).toContain("Whole-track analysis · 95.1 BPM");
    expect(cacheReadCount).toBe(1);

    let beforeEventDigest = digestCalls;
    eventHandlers.get("stream-analysis-updated")?.({ event: "stream-analysis-updated", id: 2, payload: {
      generation: 33, trackId: track.id, resourceIdentity: "different-audio-variant",
      sourceIdentitySha256: digest, audioVariantSha256: "a".repeat(64), provider: candidate.provider,
      sourceId: candidate.id, state: "started",
    } });
    await waitForAnalysisDigests(beforeEventDigest);
    expect(document.body.textContent).toContain("Whole-track analysis · 95.1 BPM");

    beforeEventDigest = digestCalls;
    eventHandlers.get("stream-analysis-updated")?.({ event: "stream-analysis-updated", id: 3, payload: {
      generation: 33, trackId: track.id, resourceIdentity: "resource-cache-key",
      sourceIdentitySha256: digest, audioVariantSha256: null, provider: candidate.provider,
      sourceId: candidate.id, state: "started",
    } });
    await waitForAnalysisDigests(beforeEventDigest);
    expect(document.body.textContent).not.toContain("Whole-track analysis · 95.1 BPM");

    beforeEventDigest = digestCalls;
    eventHandlers.get("stream-analysis-updated")?.({ event: "stream-analysis-updated", id: 4, payload: {
      generation: 33, trackId: track.id, resourceIdentity: "resource-cache-key",
      sourceIdentitySha256: digest, audioVariantSha256: null, provider: candidate.provider,
      sourceId: candidate.id, state: "completed", analysis,
    } });
    await waitForAnalysisDigests(beforeEventDigest);
    expect(document.body.textContent).toContain("Whole-track analysis · 95.1 BPM");
    expect(cacheReadCount).toBe(1);

    eventHandlers.get("stream-analysis-updated")?.({ event: "stream-analysis-updated", id: 5, payload: {
      generation: 32, trackId: track.id, resourceIdentity: "stale-resource",
      sourceIdentitySha256: digest, audioVariantSha256: null, provider: candidate.provider,
      sourceId: candidate.id, state: "started",
    } });
    await settle();
    expect(document.body.textContent).toContain("Whole-track analysis · 95.1 BPM");
    expect(cacheReadCount).toBe(1);
    expectNoPlaybackStart();
  });

  it("keeps a deferred stream-cache lookup alive across same-owner position snapshots", async () => {
    const track: Track = { ...savedTrack, id: "stream-cache-position-track", durationSeconds: 162.891 };
    const candidate: Candidate = {
      ...youtubeCandidate(track), id: "bandcamp:2039312545", provider: "bandcamp",
      title: "Amynedd Dingle Dell", uploader: "Amynedd", durationSeconds: 162.891,
    };
    const cached = {
      schemaVersion: 1,
      analyzerVersion: "burrata-whole-track-analysis-v1-bpm-v4-keyfinder-2.2.8",
      resource: {
        kind: "stream" as const, recordingId: track.id, provider: "bandcamp",
        sourceIdentitySha256: "e".repeat(64),
      },
      durationSeconds: 162.891, scope: "wholeTrack" as const, coverage: 1,
      measuredAt: "2026-10-26T00:18:06.348396951Z",
      bpm: { bpm: 95.1, confidence: 0.48, relatedTempoAlternativeBpm: null }, key: null,
    };
    const originalCall = mocks.call.getMockImplementation()!;
    let resolveCache: ((value: unknown) => void) | undefined;
    let cacheCalls = 0;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_cached_stream_analysis") {
        cacheCalls++;
        expect(args).toEqual({ trackId: track.id, generation: 22 });
        return new Promise(resolve => { resolveCache = resolve; });
      }
      return originalCall(command, args);
    });

    const initial = makeNativeSnapshot({
      revision: 1, generation: 22, status: "playing", paused: false,
      track, trackId: track.id, candidate, queue: [track], currentIndex: 0,
      duration: 162.891, volume: 0, position: 0,
    });
    mountNative(initial);
    await settle();
    const settledOwnerCallCount = cacheCalls;
    expect(settledOwnerCallCount).toBeGreaterThan(0);
    expect(resolveCache).toBeDefined();

    for (const [index, position] of [1, 2, 3, 4].entries()) {
      emitNative({ ...initial, revision: index + 2, position });
      await settle();
    }
    expect(cacheCalls).toBe(settledOwnerCallCount);
    resolveCache!(cached);
    await settle();
    expect(document.querySelector("footer.player")?.textContent).not.toContain("Whole-track analysis");
    expect(document.querySelector("footer.player")?.textContent).not.toContain("95.1 BPM");
    expectNoPlaybackStart();
  });

  it("automatically loads the complete local waveform and displays a cached key without applying metadata", async () => {
    const hash = "c".repeat(64);
    const track: Track = {...savedTrack,id:"automatic-waveform-track",provider:"local",providerId:"automatic-waveform-track",
      sourceUrl:"local:automatic-waveform-track",localPath:"/music/whole.mp3",assetId:"automatic-waveform-asset",durationSeconds:180,bpm:124,key:undefined};
    const asset: LocalAsset = {id:track.assetId!,recordingId:track.id,path:track.localPath!,hash,sourceCodec:"mp3",finalCodec:"mp3",durationSeconds:180,processing:"ready",track};
    savedAssets=[asset]; selectedAnalysisInitialState="completed";
    const bins=Array.from({length:1200},()=>({min:-.02,max:.025}));
    const originalCall=mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command,args)=>{
      if(command==="get_asset_by_id")return asset;
      if(command==="get_local_waveform_analysis")return {assetId:asset.id,fileHash:hash,analyzerVersion:"ffmpeg-mono-peaks-8khz-v1",durationSeconds:180,bins,complete:true,scope:"wholeTrack"};
      if(command==="get_playback_waveform")return null;
      return originalCall(command,args);
    });
    localStorage.setItem("burrata.waveform-enabled.v1","true");
    mountNative(makeNativeSnapshot({revision:1,generation:30,status:"playing",paused:false,trackId:track.id,track,queue:[track],currentIndex:0,duration:180,volume:0,
      quality:{sourceKind:"localAsset",assetId:asset.id}}));
    for(let i=0;i<20&&!document.querySelector('.waveform-status')?.textContent?.includes("Full audio waveform");i++)await settle();
    expect(document.querySelectorAll('.waveform rect')).toHaveLength(1200);
    expect(document.querySelector('.waveform-status')?.textContent).toContain("Full audio waveform");
    expect(mocks.call).toHaveBeenCalledWith("get_local_waveform_analysis",expect.objectContaining({assetId:asset.id,expectedHash:hash}));
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click();
    await settle();await settle();
    const keyLabel=[...document.querySelectorAll("dt")].find(node=>node.textContent==="Key");
    expect(keyLabel?.nextElementSibling?.textContent).toContain("Am");
    expect(keyLabel?.nextElementSibling?.textContent).toContain("estimated from audio");
    expect(mocks.call).toHaveBeenCalledWith("start_selected_asset_analysis",{assetIds:[asset.id],fields:["key"]});
    expect(mocks.call.mock.calls.some(([command])=>command==="apply_selected_asset_analysis")).toBe(false);
    expect(savedAssets[0]?.track.key).toBeUndefined();
  });

  it("rebinds a verified full local waveform after seek generations and clears it for another asset", async () => {
    const track: Track = {
      ...savedTrack, id: "waveform-local-track", provider: "local", providerId: "waveform-local-track",
      sourceUrl: "local:waveform-local-track", localPath: "/music/waveform.wav", localAvailability: "available",
      assetId: "waveform-asset-a", durationSeconds: 20,
    };
    const hashA = "a".repeat(64), hashB = "b".repeat(64);
    const assetA: LocalAsset = { id: "waveform-asset-a", recordingId: track.id, path: track.localPath!, hash: hashA, sourceCodec: "wav", finalCodec: "wav", durationSeconds: 20, processing: "ready", track };
    const assetB: LocalAsset = { ...assetA, id: "waveform-asset-b", path: "/music/waveform-copy.wav", hash: hashB };
    savedAssets = [assetA, assetB];
    const bins = Array.from({ length: 1200 }, () => ({ min: -0.25, max: 0.35 }));
    const cachedRecord = (asset: LocalAsset) => ({
      assetId: asset.id, fileHash: asset.hash, analyzerVersion: "ffmpeg-mono-peaks-8khz-v1",
      durationSeconds: 20, bins, complete: true, scope: "wholeTrack" as const,
    });
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_asset_by_id") return savedAssets.find(asset => asset.id === args?.assetId) ?? null;
      if (command === "get_local_waveform_analysis") {
        const asset = savedAssets.find(item => item.id === args?.assetId);
        return asset && args?.expectedHash === asset.hash ? cachedRecord(asset) : null;
      }
      if (command === "get_playback_waveform") return null;
      return originalCall(command, args);
    });
    localStorage.setItem("burrata.waveform-enabled.v1", "true");
    const snapshot = makeNativeSnapshot({
      revision: 1, generation: 10, status: "paused", paused: true, trackId: track.id, track,
      queue: [track], currentIndex: 0, duration: 20, canSeek: true, volume: 0,
      quality: { sourceKind: "localAsset", assetId: assetA.id },
    });
    mountNative(snapshot);
    await settle();
    const toggle = document.querySelector<HTMLButtonElement>('[aria-label="Waveform seek bar"]')!;
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    eventHandlers.get("automation-ui-navigation")!({event:"automation-ui-navigation",id:1,payload:{
      requestId:"local-waveform-settings",target:{kind:"view",view:"settings",settingsCategory:"playback"},
    }});
    await settle(); await settle();
    const fullWaveButton = document.querySelector<HTMLButtonElement>('[aria-label="Analyze full local waveform"]')!;
    fullWaveButton.click();
    for (let attempt = 0; attempt < 20 && !document.querySelector(".waveform-status")?.textContent?.includes("Full audio waveform"); attempt += 1) await settle();
    expect(document.querySelector(".waveform-status")?.textContent).toContain("Full audio waveform");
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(1200);

    const range = document.querySelector<HTMLInputElement>('[aria-label="Playback position, waveform"]')!;
    range.value = "8";
    range.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 180));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", expect.objectContaining({ command: expect.objectContaining({ action: "seek", position: 8 }) }));
    emitNative({ ...snapshot, revision: 2, generation: 11, seekSerial: 1, position: 8 });
    for (let attempt = 0; attempt < 20 && (mocks.call.mock.calls.filter(([command, args]) => command === "get_local_waveform_analysis" && (args as Record<string, unknown>)?.assetId === assetA.id).length < 2 || !document.querySelector(".waveform-status")?.textContent?.includes("Full audio waveform")); attempt += 1) await settle();
    expect(document.querySelector(".waveform-status")?.textContent).toContain("Full audio waveform");
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(1200);
    expect(mocks.call.mock.calls.filter(([command, args]) => command === "get_local_waveform_analysis" && (args as Record<string, unknown>)?.assetId === assetA.id).length).toBeGreaterThanOrEqual(2);

    savedAssets = savedAssets.map(asset => asset.id === assetA.id ? { ...asset, hash: "c".repeat(64) } : asset);
    emitNative({ ...snapshot, revision: 3, generation: 12, track, quality: { sourceKind: "localAsset", assetId: assetA.id } });
    await settle();
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(0);
    expect(document.querySelector(".waveform-status")?.textContent).toContain("Standard seek bar shown");
    expect(document.querySelector('[aria-label="Playback position"]')).not.toBeNull();
    expect(mocks.call.mock.calls.filter(([command, args]) => command === "get_local_waveform_analysis" && (args as Record<string, unknown>)?.assetId === assetA.id)).toHaveLength(2);

    emitNative({ ...snapshot, revision: 4, generation: 13, track, quality: { sourceKind: "localAsset", assetId: assetB.id } });
    await settle();
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(0);
    expect(document.querySelector(".waveform-status")?.textContent).toContain("Standard seek bar shown");
    expect(document.querySelector('[aria-label="Playback position"]')).not.toBeNull();
    expect(mocks.call.mock.calls.some(([command, args]) => command === "get_local_waveform_analysis" && (args as Record<string, unknown>)?.assetId === assetB.id)).toBe(false);
  });

  it("retries open lyrics when the decoder supplies an imported track's missing duration", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command,args) => {
      if(command === "get_current_track_lyrics") {
        const visible = args?.visibleTrack as {durationSeconds?:number} | undefined;
        return visible?.durationSeconds === 241.92
          ? {status:"unsynced",provider:"lrclib",plainLyrics:"Synthetic current lyric",lines:[{id:"line-1",timeSeconds:null,text:"Synthetic current lyric"}]}
          : {status:"noCurrentTrack",provider:"lrclib"};
      }
      return originalCall(command,args);
    });
    const track = {...savedTrack,durationSeconds:null};
    const snapshot = makeNativeSnapshot({status:"playing",paused:false,track,trackId:track.id,queue:[track],currentIndex:0,duration:0});
    mountNative(snapshot); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Open fullscreen lyrics"]')!.click(); await settle();
    expect(document.body.textContent).toContain("No current full-track metadata");
    emitNative({...snapshot,revision:2,duration:241.92}); await settle();
    expect(document.body.textContent).toContain("Synthetic current lyric");
    expect(document.body.textContent).not.toContain("No current full-track metadata");
    expect(track.durationSeconds).toBeNull();
    expectNoPlaybackStart();
  });

  it("shows a qualified live key when the same PCM window has no live BPM estimate", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const track = { ...savedTrack, key: "C" };
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_settings") return { ...await originalCall(command, args), liveBpmEnabled: true };
      if (command === "get_live_playback_estimates") return {
        bpm: null,
        key: { generation: 36, seekSerial: 0, trackId: track.id, key: "Am", profileFit: .34, profileMargin: .08, windowSeconds: 10 },
      };
      return originalCall(command, args);
    });
    mountNative(makeNativeSnapshot({ generation: 36, seekSerial: 0, status: "playing", paused: false, track, trackId: track.id, queue: [track], currentIndex: 0 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click(); await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Live keyAm · recent window");
    expect(document.querySelector(".playing-info")?.textContent).toContain("KeyC");
    expect(document.querySelector(".playing-info")?.textContent).not.toContain("84.6 BPM");
    eventHandlers.get("automation-inspect-playback-layout")!({ event: "automation-inspect-playback-layout", id: 73, payload: { requestId: "live-key-status" } });
    await settle();
    expect(lastCallOf("automation_layout_complete")?.[1]).toMatchObject({
      layout: { liveBpmDiagnostics: { engine: "native", generation: 36, seekSerial: 0, status: "estimating" } },
    });
    expectNoPlaybackStart();
  });

  it("persists last-valid live BPM and key by source and shows them after playback pauses", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const cache = new Map<string, unknown>();
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_settings") return { ...await originalCall(command, args), liveBpmEnabled: true };
      if (command === "get_live_playback_estimates") return {
        bpm: { generation: 56, seekSerial: 0, trackId: savedTrack.id, bpm: 97.4, confidence: .61, windowSeconds: 12 },
        key: { generation: 56, seekSerial: 0, trackId: savedTrack.id, key: "Am", profileFit: .42, profileMargin: .08, windowSeconds: 12 },
      };
      if (command === "read_analysis_cache_record") return cache.get(String(args?.key)) ?? null;
      if (command === "write_analysis_cache_record") { cache.set(String(args?.key), structuredClone(args?.payload)); return undefined; }
      return originalCall(command, args);
    });
    const track = { ...savedTrack, durationSeconds: 240 };
    const snapshot = makeNativeSnapshot({ generation: 56, seekSerial: 0, status: "playing", paused: false, track, trackId: track.id,
      candidate: youtubeCandidate(track), queue: [track], currentIndex: 0, position: 38 });
    mountNative(snapshot); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click(); await settle();
    for (let attempt = 0; attempt < 20 && !document.querySelector(".playing-info")?.textContent?.includes("Last measured"); attempt += 1) await settle();
    const written = [...cache.values()][0] as { durationSeconds: number; bpm: { value: { bpm: number }; measuredAt: string; positionSeconds: number }; key: { value: { key: string }; measuredAt: string; positionSeconds: number } } | undefined;
    expect(written?.durationSeconds).toBe(240);
    expect(written?.bpm.value.bpm).toBe(97.4);
    expect(written?.key.value.key).toBe("Am");
    expect(written?.bpm.positionSeconds).toBe(38);
    expect(written?.key.positionSeconds).toBe(38);
    expect(document.querySelector(".playing-info")?.textContent).toContain("Live BPM97.4 BPM");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Live keyAm · recent window");

    emitNative({ ...snapshot, revision: 2, status: "paused", paused: true }); await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last BPM97.4 BPM");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last keyAm");
    expect(document.querySelector(".playing-info .last-measurement")?.textContent).toContain("around 0:38");
    expectNoPlaybackStart();
  });

  it("keeps loaded last-measured values stable across native position pulses and clears them on source change", async () => {
    const originalCall = mocks.call.getMockImplementation()!;
    const cache = new Map<string, unknown>();
    const track = { ...savedTrack, durationSeconds: 240 };
    const candidate = youtubeCandidate(track);
    const sourceResource = { kind: "stream" as const, provider: "youtube" as const, sourceId: candidate.id, durationSeconds: 240, recordingId: track.id };
    const cacheKey = await liveMeasurementCacheKey(sourceResource);
    expect(cacheKey).toMatch(/^[a-f0-9]{64}$/);
    const saved: LiveMeasurementCacheRecord = {
      schemaVersion: 1,
      durationSeconds: 240,
      bpm: { value: { bpm: 101.2, confidence: .63, windowSeconds: 12 }, measuredAt: "2026-10-28T10:00:00.000Z", positionSeconds: 42 },
      key: { value: { key: "Dm", profileFit: .44, profileMargin: .09, windowSeconds: 12 }, measuredAt: "2026-10-28T10:00:05.000Z", positionSeconds: 47 },
    };
    cache.set(cacheKey!, saved);
    let cacheReads = 0;
    mocks.call.mockImplementation(async (command, args) => {
      if (command === "get_settings") return { ...await originalCall(command, args), liveBpmEnabled: true };
      if (command === "get_live_playback_estimates") return { bpm: null, key: null };
      if (command === "read_analysis_cache_record") { cacheReads++; expect(args?.namespace).toBe(LIVE_MEASUREMENT_CACHE_NAMESPACE); return cache.get(String(args?.key)) ?? null; }
      if (command === "write_analysis_cache_record") { cache.set(String(args?.key), structuredClone(args?.payload)); return undefined; }
      return originalCall(command, args);
    });
    const snapshot = makeNativeSnapshot({ generation: 57, seekSerial: 0, status: "playing", paused: false, track, trackId: track.id,
      candidate, queue: [track], currentIndex: 0, position: 48 });
    mountNative(snapshot); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click(); await settle();
    for (let attempt = 0; attempt < 10 && !document.querySelector(".playing-info")?.textContent?.includes("Last BPM101.2 BPM"); attempt += 1) await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last BPM101.2 BPM");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last keyDm");
    expect(cacheReads).toBe(1);

    emitNative({ ...snapshot, revision: 2, position: 49 }); await settle();
    emitNative({ ...snapshot, revision: 3, position: 51 }); await settle();
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last BPM101.2 BPM");
    expect(document.querySelector(".playing-info")?.textContent).toContain("Last keyDm");
    expect(cacheReads).toBe(1);

    emitNative({ ...snapshot, revision: 4, candidate: { ...candidate, id: "youtube:AnotherSourceID" }, position: 52 }); await settle();
    for (let attempt = 0; attempt < 10 && cacheReads < 2; attempt += 1) await settle();
    expect(cacheReads).toBe(2);
    expect(document.querySelector(".playing-info")?.textContent).not.toContain("Last BPM101.2 BPM");
    expect(document.querySelector(".playing-info")?.textContent).not.toContain("Last keyDm");
    expectNoPlaybackStart();
  });

  it("keeps the approximate album excerpt caption for a native asset-backed playback", async () => {
    const excerpt = {
      schemaVersion: 1, policy: "flexible", evaluatorVersion: "1", relationship: "probable",
      label: "Approximate album excerpt", source: { provider: "youtube", id: "youtube:album-upload" },
      startSeconds: 83, endSeconds: 126, boundaryUncertaintySeconds: 2,
    };
    const track: Track = { ...savedTrack, localPath: "/music/approximate.flac", provenance: { approximateExcerpt: excerpt } };
    mountNative(makeNativeSnapshot({ revision: 1, status: "playing", paused: false, trackId: track.id, track,
      candidate: youtubeCandidate(track), queue: [track], currentIndex: 0,
      quality: { sourceKind: "approximateAlbumExcerpt", label: "Approximate album excerpt", approximateExcerpt: excerpt } }));
    await settle();

    expect(document.querySelector(".player-source")?.textContent).toBe("Approximate album excerpt · YouTube");
    expect(document.querySelector(".player-source")?.textContent).not.toContain("Local file");
    expectNoPlaybackStart();
  });

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
    expect(status()?.querySelector(".playback-playing-icon")).not.toBeNull();
    expect(document.querySelector("footer.player .play-button-loading-ring")).toBeNull();
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
    expect(playControl()?.querySelector("svg.buffering-spinner")).not.toBeNull();
    expect(status()?.getAttribute("aria-label")).toBe(`Buffering ${savedTrack.title}`);
    expect(playControl()?.classList.contains("buffering")).toBe(true);
    expect(playControl()?.getAttribute("aria-busy")).toBe("true");
    expect(document.querySelector("footer.player .play-button-loading-ring")).not.toBeNull();
    const unrelatedSameTitle = [...document.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")].find(row => row.getAttribute("aria-current") !== "true");
    expect(unrelatedSameTitle?.querySelector<HTMLButtonElement>(".position-play")?.getAttribute("aria-busy")).toBe("false");

    emitNative({ ...buffering, revision: 73, status: "playing", paused: false, playerMessage: "Playing" }); await settle();
    expect(document.querySelector("footer.player .play-button-loading-ring")).toBeNull();
    expect(currentRows()[0]?.querySelector(".playback-playing-icon")).not.toBeNull();

    const unknownRange = { parentTrackId: "same-id-parent", chapterId: "part", sourceUrl: "https://example.test/mix", startTime: 30, endTime: 60, parentTitle: "Unknown range" };
    const unknownPlaybackIdentity = { ...savedTrack, playbackRange: unknownRange };
    emitNative({ ...initial, revision: 74, status: "playing", paused: false, track: unknownPlaybackIdentity, trackId: unknownPlaybackIdentity.id, queue: [unknownPlaybackIdentity] }); await settle();
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

  it("keeps the open track summary stable across same-track startup snapshots", async () => {
    const track = { ...savedTrack };
    let summaryReads = 0;
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((command, args) => {
      if (command === "get_track_library_summary") {
        summaryReads++;
        if (summaryReads === 1) return Promise.resolve({ trackId: track.id, inLibrary: true, downloaded: false,
          playCount: 4, playedAt: [], playedAtHasMore: false, playlists: [] });
        // Keep later reads pending so a redundant request visibly replaces the
        // already rendered summary with its loading state.
        return new Promise(() => {});
      }
      return original(command, args);
    });
    const initial = makeNativeSnapshot({ revision: 1, generation: 51, status: "resolving", paused: false,
      track, trackId: track.id, queue: [track], currentIndex: 0, candidate: youtubeCandidate(track),
      quality: { sourceKind: "http", codec: "opus", bitrateKbps: 160 } });
    mountNative(initial);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show track details"]')!.click();
    await settle();
    expect(document.querySelector('[aria-label="Library and listening activity"]')).not.toBeNull();
    expect(summaryReads).toBe(1);
    const infoPanel = document.querySelector(".playing-info");
    expect(infoPanel).not.toBeNull();

    for (const [revision, status, sourceKind, bitrateKbps] of [
      [2, "buffering", "http", 128],
      [3, "playing", "http", 160],
      [4, "playing", "localAsset", 0],
    ] as const) {
      emitNative({ ...initial, revision, status, paused: false,
        track: { ...track }, candidate: youtubeCandidate({ ...track }),
        quality: { sourceKind, codec: sourceKind === "localAsset" ? "flac" : "opus", bitrateKbps } });
      await settle();
      expect(document.querySelector(".playing-info")).toBe(infoPanel);
      expect(document.querySelector('[aria-label="Library and listening activity"]')).not.toBeNull();
      expect(document.querySelector('[role="status"]')?.textContent).not.toContain("Loading listening and library details");
    }
    expect(summaryReads).toBe(1);
  });

  it("uses the migrated Native owner over a stale browser queue, including an empty Native session", async () => {
    const track = { ...savedTrack };
    const tracks = [track, track];
    const entryIds = ["legacy-first", "legacy-second"];
    localStorage.setItem("burrata.playback-queue.v1", JSON.stringify({ tracks, entryIds, currentIndex: 1, repeat: "off" }));
    const initialLegacyQueue = localStorage.getItem("burrata.playback-queue.v1");
    nativePlaybackSnapshot = makeNativeSnapshot({ engine: "native", revision: 1, queue: [], queueEntryIds: [], currentIndex: -1 });
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(JSON.parse(localStorage.getItem("burrata.playback-queue.v1")!).tracks).toEqual([]);

    const owned = makeNativeSnapshot({ engine: "native", revision: 2, generation: 8, status: "paused", trackId: track.id,
      entryId: entryIds[1], track, queue: tracks, queueEntryIds: entryIds, currentIndex: 1, paused: true, position: 12.5 });
    nativePlaybackSnapshot = owned;
    emitNative(owned);
    await settle();
    expect(JSON.parse(localStorage.getItem("burrata.playback-queue.v1")!).currentIndex).toBe(1);

    const cleared = makeNativeSnapshot({ engine: "native", revision: 3, generation: 9, queue: [], queueEntryIds: [], currentIndex: -1 });
    nativePlaybackSnapshot = cleared;
    emitNative(cleared);
    await settle();
    expect(JSON.parse(localStorage.getItem("burrata.playback-queue.v1")!)).toMatchObject({ tracks: [], entryIds: [], currentIndex: -1 });
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
    expect(document.querySelector(".banner.notice")).toBeNull();
    expect(document.body.textContent).not.toContain("private.example");
    expect(document.body.textContent).not.toContain("secret-value");

    emitNative({ ...failed, revision: 45, status: "failed", paused: false });
    await settle();
    expect(document.querySelector(".player-error")?.textContent).toContain("No eligible audio source");
    expect(document.querySelector(".player-status")?.textContent).toBe("Playback unavailable");
    expect(document.querySelector('[aria-label="Playback error details"]')).not.toBeNull();
    expect(document.querySelector<HTMLElement>(".player-status")?.title).not.toContain("private.example");
    expect(document.querySelector<HTMLElement>(".player-status")?.title).not.toContain("secret-value");

    emitNative({ ...failed, revision: 46, status: "buffering", paused: false });
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toBe("Buffering…");
    expect(document.querySelector(".player-error")).toBeNull();
    expect(document.querySelector(".banner.notice")).toBeNull();

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
      emitNative({...next,revision:2,status:"failed",diagnostics:{lastSourceFailure:another,lastSkippedTrackFailure:null}});await settle();
      await vi.advanceTimersByTimeAsync(6500);await tick();
      expect(document.querySelector(".banner.notice")).toBeNull();
      expect(document.querySelector(".player-error")?.textContent).toContain("Another failed recording");
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
      queue: duplicateQueue, queueRevision: 51, queueEntryIds: ["saved-first-occurrence", "second-track-occurrence", "saved-active-occurrence"],
      currentIndex: 2, paused: false, playerMessage: "Unavailable",
      diagnostics: { lastSourceFailure: { trackId: savedTrack.id, title: savedTrack.title, artist: savedTrack.artist, reason: "No eligible audio source" } },
    }));
    await settle();

    document.querySelector<HTMLButtonElement>('[aria-label="Retry playback"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({
      action: "select", index: 2, expectedQueueRevision: 51, expectedEntryId: "saved-active-occurrence",
    }) });
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
    document.querySelector<HTMLAnchorElement>(`.playing-info dl a[href="${candidate.url}"]`)?.click();
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
    document.querySelector<HTMLAnchorElement>(`.playing-info dl a[href="${candidate.url}"]`)?.click();
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
    document.querySelector<HTMLAnchorElement>(`.playing-info dl a[href="${candidate.url}"]`)!.click();
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

  it("loads inactive saved detections into track information and routes playlist and title actions", async () => {
    const parent: Track = { ...secondTrack, id: "inactive-long-set", provider: "youtube", providerId: "longset12345",
      title: "Inactive Long Set", durationSeconds: 1200, sourceUrl: "https://youtube.com/watch?v=longset12345" };
    const observation = { title: "Detected Song", artist: "Recognized Artist", detectedAtSeconds: 42,
      capturedStartSeconds: 36, capturedEndSeconds: 51, observedAt: "2026-10-28T00:00:00Z",
      identifiers: { isrc: "USABC2600001", shazamId: "shazam-42" } };
    savedTrackDetectionMock = { resourceKey: "c".repeat(64), enabled: true, observations: [observation] };
    mountNative(makeNativeSnapshot({ revision: 88, generation: 22, status: "playing", trackId: savedTrack.id,
      track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack, parent], currentIndex: 0, paused: false }));
    await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("get_saved_track_detections", { track: parent });

    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_saved_track_detections", { track: parent });
    document.querySelector<HTMLElement>('[aria-label="More actions for Inactive Long Set"]')!.click();
    await settle();
    [...document.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.includes("Track information"))!.click();
    await settle();
    const info = document.querySelector<HTMLElement>(".playing-info")!;
    expect(info.textContent).toContain("Detected Song");
    expect(info.querySelector('.detected-tracks input[type="checkbox"]')).toBeNull();

    info.querySelector<HTMLButtonElement>('.detected-tracks .add-one')!.click();
    await settle();
    expect(document.querySelector(".playlist-picker-dialog")?.textContent).toContain("Recognized Artist — Detected Song");
    document.querySelector<HTMLButtonElement>('[aria-label="Close playlist picker"]')!.click();
    await settle();

    document.querySelector<HTMLButtonElement>('.playing-info .detected-tracks .track-link')!.click();
    await settle();
    const playRequest = mocks.call.mock.calls.find(([command, args]) => command === "playback_command"
      && (args as { command?: { action?: string; tracks?: Track[] } })?.command?.action === "play"
      && (args as { command: { tracks?: Track[] } }).command.tracks?.[0]?.title === "Detected Song");
    expect(playRequest?.[1]).toMatchObject({ command: { tracks: [{ provider: "detected", title: "Detected Song", artist: "Recognized Artist", isrc: "USABC2600001" }] } });
  });

  it("routes an inactive saved detection timestamp to its parent queue occurrence", async () => {
    const parent: Track = { ...secondTrack, id: "inactive-set-timestamp", provider: "youtube", providerId: "timestamp123",
      title: "Timestamp Set", durationSeconds: 1200, sourceUrl: "https://youtube.com/watch?v=timestamp123" };
    savedTrackDetectionMock = { resourceKey: "d".repeat(64), enabled: true, observations: [{
      title: "Timestamp Song", artist: "Artist", detectedAtSeconds: 78, capturedStartSeconds: 72,
      capturedEndSeconds: 90, observedAt: "2026-10-28T00:00:00Z",
    }] };
    mountNative(makeNativeSnapshot({ revision: 89, generation: 23, status: "playing", trackId: savedTrack.id,
      track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack, parent], queueRevision: 93,
      queueEntryIds: ["currently-playing-occurrence", "inactive-parent-occurrence"], currentIndex: 0, paused: false }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    const timestamp = document.querySelector<HTMLButtonElement>('[aria-label="Seek to ~1:18 in Timestamp Set"]');
    expect(timestamp).not.toBeNull();
    timestamp!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command", { command: expect.objectContaining({
      action: "select", index: 1, expectedQueueRevision: 93, expectedEntryId: "inactive-parent-occurrence",
    }) });
  });

  it("requests lyrics for the active detected song and switches with playback position", async () => {
    const setTrack: Track = { ...savedTrack, id: "lyrics-long-set", provider: "youtube", providerId: "lyrics-set-01",
      title: "Lyrics Long Set", durationSeconds: 900, sourceUrl: "https://youtube.com/watch?v=lyrics-set-01" };
    playingTrackDetectionMock = { resourceKey: "e".repeat(64), enabled: true, observations: [
      { title: "First Detected Song", artist: "First Artist", detectedAtSeconds: 18, capturedStartSeconds: 11, capturedEndSeconds: 30, observedAt: "2026-10-28T00:00:00Z" },
      { title: "Second Detected Song", artist: "Second Artist", detectedAtSeconds: 62, capturedStartSeconds: 56, capturedEndSeconds: 75, observedAt: "2026-10-28T00:01:00Z" },
    ] };
    const requests: { visibleTrack?: Record<string, unknown> }[] = [];
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_current_track_lyrics") {
        requests.push(structuredClone(args) as { visibleTrack?: Record<string, unknown> });
        return { status: "noCurrentTrack", provider: "lrclib" };
      }
      return originalCall(command, args);
    });
    mountNative(makeNativeSnapshot({ revision: 91, generation: 24, seekSerial: 4, status: "playing", trackId: setTrack.id,
      track: setTrack, candidate: youtubeCandidate(setTrack), queue: [setTrack], currentIndex: 0, paused: false, position: 35, duration: 900 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Open fullscreen lyrics"]')!.click();
    await settle();

    const firstRequest = requests.find(request => request.visibleTrack?.trackName === "First Detected Song");
    expect(firstRequest?.visibleTrack).toMatchObject({
      trackId: setTrack.id, trackName: "First Detected Song", artistName: "First Artist", durationSeconds: 0,
      detection: { trackId: setTrack.id, generation: 24, seekSerial: 4, resourceKey: "e".repeat(64), detectedAtSeconds: 18 },
    });
    expect(document.querySelector('[role="dialog"][aria-label="Fullscreen lyrics"]')?.textContent).toContain("No current full-track metadata is available for lyrics.");

    emitNative({ ...makeNativeSnapshot({ revision: 92, generation: 24, seekSerial: 4, status: "playing", trackId: setTrack.id,
      track: setTrack, candidate: youtubeCandidate(setTrack), queue: [setTrack], currentIndex: 0, paused: false, position: 70, duration: 900 }) });
    for (let attempt = 0; attempt < 12 && !requests.some(request => request.visibleTrack?.trackName === "Second Detected Song"); attempt++) await settle();
    const secondRequest = requests.find(request => request.visibleTrack?.trackName === "Second Detected Song");
    expect(secondRequest?.visibleTrack).toMatchObject({
      trackId: setTrack.id, trackName: "Second Detected Song", artistName: "Second Artist",
      detection: { trackId: setTrack.id, generation: 24, seekSerial: 4, resourceKey: "e".repeat(64), detectedAtSeconds: 62 },
    });
  });

  it("keeps mix detection off by default and cancels its owned native capture when toggled off", async () => {
    holdPlayingTrackDetection=true;
    const candidate=youtubeCandidate(savedTrack);
    mountNative(makeNativeSnapshot({revision:61,generation:14,seekSerial:3,status:"playing",trackId:savedTrack.id,track:savedTrack,candidate,queue:[savedTrack],currentIndex:0,paused:false,position:90,duration:240}));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const toggle=document.querySelector<HTMLInputElement>('.playing-info .detected-tracks input[type="checkbox"]')!;
    expect(toggle.checked).toBe(false);
    expect(mocks.call.mock.calls.some(([command])=>command==="detect_playing_track_segment")).toBe(false);

    toggle.checked=true;toggle.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    const detectionCall=mocks.call.mock.calls.find(([command])=>command==="detect_playing_track_segment");
    expect(detectionCall?.[1]).toEqual({trackId:savedTrack.id,generation:14,seekSerial:3,captureId:expect.stringMatching(/^[0-9a-f-]{36}$/i)});
    const captureId=(detectionCall?.[1] as {captureId:string}).captureId;
    expect(mocks.call.mock.calls.some(([command])=>command==="identify_playing_track")).toBe(false);
    expect(toggle.disabled).toBe(false);

    toggle.checked=false;toggle.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_playing_track_identification",{captureId});
    expect(mocks.call.mock.calls).toContainEqual(["set_playing_track_detection_enabled",{trackId:savedTrack.id,generation:14,seekSerial:3,enabled:false}]);
    finishPlayingTrackDetection?.({resourceKey:"a".repeat(64),enabled:false,observations:[]});await settle();
    expect(mocks.call.mock.calls.some(([command])=>["update_track","save_audio_match","apply_autotag_review"].includes(String(command)))).toBe(false);
  });

  it.each([false, true])("continues detection across queue tracks only with queue continuation enabled: %s", async (continueQueue) => {
    const next = {...savedTrack, id:"queue-detection-next", title:"Next recording"};
    const last = {...savedTrack, id:"queue-detection-last", title:"Last recording"};
    const snapshot = makeNativeSnapshot({revision:61,generation:14,seekSerial:3,status:"paused",trackId:savedTrack.id,track:savedTrack,candidate:youtubeCandidate(savedTrack),queue:[savedTrack,next,last],currentIndex:0,paused:true,duration:240});
    mountNative(snapshot); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click(); await settle();
    const main = () => document.querySelector<HTMLInputElement>('.playing-info .detection-toggle input')!;
    const continuation = () => document.querySelector<HTMLInputElement>('.playing-info .detection-queue-toggle input')!;
    expect(continuation().checked).toBe(false);
    expect(continuation().disabled).toBe(true);
    main().checked=true; main().dispatchEvent(new Event("change", {bubbles:true})); await settle();
    if (continueQueue) {
      continuation().checked=true; continuation().dispatchEvent(new Event("change", {bubbles:true})); await settle();
    }
    playingTrackDetectionMock={...playingTrackDetectionMock,enabled:false};
    emitNative({...snapshot,revision:62,generation:15,trackId:next.id,track:next,candidate:youtubeCandidate(next),currentIndex:1}); await settle();
    expect(main().checked).toBe(continueQueue);
    expect(mocks.call.mock.calls.some(([command,args]) => command==="set_playing_track_detection_enabled" && args?.trackId===next.id && args?.enabled===true)).toBe(continueQueue);
    if (continueQueue) {
      main().checked=false; main().dispatchEvent(new Event("change", {bubbles:true})); await settle();
      expect(continuation().checked).toBe(false);
    }
    playingTrackDetectionMock={...playingTrackDetectionMock,enabled:false};
    emitNative({...snapshot,revision:63,generation:16,trackId:last.id,track:last,candidate:youtubeCandidate(last),currentIndex:2}); await settle();
    expect(main().checked).toBe(false);
    expect(mocks.call.mock.calls.some(([command,args]) => command==="set_playing_track_detection_enabled" && args?.trackId===last.id)).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command==="detect_playing_track_segment")).toBe(false);
  });

  it("keeps detection enabled across a six-minute seek while its owner read races playback", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    playingTrackDetectionMock={...playingTrackDetectionMock,enabled:true};
    const original=mocks.call.getMockImplementation()!;
    let seekReads=0;
    mocks.call.mockImplementation(async (command:string,args?:Record<string,unknown>) => {
      if(command==="get_playing_track_detections" && args?.seekSerial===4 && ++seekReads===1)
        throw new Error("Playback position changed before detection options were read");
      return original(command,args);
    });
    const snapshot=makeNativeSnapshot({revision:61,generation:14,seekSerial:3,status:"paused",trackId:savedTrack.id,
      track:savedTrack,candidate:youtubeCandidate(savedTrack),queue:[savedTrack],currentIndex:0,paused:true,position:90,duration:900});
    mountNative(snapshot);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const toggle=()=>document.querySelector<HTMLInputElement>('.playing-info .detection-toggle input')!;
    expect(toggle().checked).toBe(true);
    emitNative({...snapshot,revision:62,seekSerial:4,position:450});
    await settle();
    expect(seekReads).toBe(1);
    expect(toggle().checked).toBe(true);
    expect(toggle().disabled).toBe(false);
    expect(document.querySelector('.detected-tracks')?.textContent).toContain("Detection will continue when playback is ready");
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    expect(seekReads).toBe(2);
    expect(toggle().checked).toBe(true);
    expect(document.querySelector('.detected-tracks')?.textContent).not.toContain("stable identity");
    expect(mocks.call.mock.calls.some(([command])=>command==="set_playing_track_detection_enabled")).toBe(false);
  });

  it("cancels detection at the old seek position and resumes without accepting its stale result", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    playingTrackDetectionMock={...playingTrackDetectionMock,enabled:true};
    holdPlayingTrackDetection=true;
    const snapshot=makeNativeSnapshot({revision:61,generation:14,seekSerial:3,status:"playing",trackId:savedTrack.id,
      track:savedTrack,candidate:youtubeCandidate(savedTrack),queue:[savedTrack],currentIndex:0,paused:false,position:90,duration:900});
    mountNative(snapshot);
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    const first=mocks.call.mock.calls.find(([command])=>command==="detect_playing_track_segment")![1] as {captureId:string};
    emitNative({...snapshot,revision:62,seekSerial:4,position:450});
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_playing_track_identification",{captureId:first.captureId});
    finishPlayingTrackDetection?.({resourceKey:"a".repeat(64),enabled:false,observations:[]});
    await settle();
    expect(document.querySelector<HTMLInputElement>('.playing-info .detection-toggle input')?.checked).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("detect_playing_track_segment",expect.objectContaining({seekSerial:4}));
    finishPlayingTrackDetection?.(playingTrackDetectionMock);
    await settle();
  });

  it("offers optional detection for a finite recording identified from radio", async () => {
    const finite = { ...savedTrack, provider: "radio", provenance: { sourceKind: "radioSong" } };
    mountNative(makeNativeSnapshot({ revision: 61, generation: 14, seekSerial: 3, status: "playing", trackId: finite.id,
      track: finite, candidate: youtubeCandidate(finite), queue: [finite], currentIndex: 0, paused: false, position: 90, duration: 240 }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("get_playing_track_detections", { trackId: finite.id, generation: 14, seekSerial: 3 });
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const toggle = document.querySelector<HTMLInputElement>('.playing-info .detected-tracks input[type="checkbox"]');
    expect(toggle).not.toBeNull();
    expect(toggle?.checked).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "detect_playing_track_segment")).toBe(false);
  });

  it("spaces automatic detection from capture start and never overlaps pending captures", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    holdPlayingTrackDetection = true;
    mountNative(makeNativeSnapshot({ revision: 61, generation: 14, seekSerial: 3, status: "playing", trackId: savedTrack.id,
      track: savedTrack, candidate: youtubeCandidate(savedTrack), queue: [savedTrack], currentIndex: 0, paused: false, position: 90, duration: 240 }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();
    await settle();
    const toggle = document.querySelector<HTMLInputElement>('.playing-info .detected-tracks input[type="checkbox"]')!;
    toggle.checked = true; toggle.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const calls = () => mocks.call.mock.calls.filter(([command]) => command === "detect_playing_track_segment").length;
    expect(calls()).toBe(1);
    await vi.advanceTimersByTimeAsync(12_000);
    finishPlayingTrackDetection?.({ resourceKey: "a".repeat(64), enabled: true, observations: [] }); await settle();
    await vi.advanceTimersByTimeAsync(22_000);
    expect(calls()).toBe(1);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(calls()).toBe(2);
    await vi.advanceTimersByTimeAsync(35_000);
    expect(calls()).toBe(2);
    finishPlayingTrackDetection?.({ resourceKey: "a".repeat(64), enabled: true, observations: [] }); await settle();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(calls()).toBe(3);
    finishPlayingTrackDetection?.({ resourceKey: "a".repeat(64), enabled: true, observations: [] }); await settle();
  });

  it("restores cached partial waveform bins for paused native playback", async () => {
    const track = { ...savedTrack, provider: "youtube", durationSeconds: 240, localPath: "/missing/former-download.wav" };
    const candidate = youtubeCandidate(track);
    const variant = { formatId: "251", codec: "opus", mimeType: "audio/webm", bitrateKbps: 128, expectedContentLength: 12345, containsVideo: false };
    const resourceKey = playingTrackDetectionMock.resourceKey;
    const preparedFixture = await prepareObservedWaveformBinding({ resourceKey, trackId: track.id, generation: 14, durationSeconds: 240,
      binCount: 1200, scope: "wholeTrack", range: null, identity: { kind: "stream", provider: "youtube", sourceId: candidate.id, audioVariant: variant } });
    const cacheKey = preparedFixture!.persistenceKey!;
    const now = Date.now();
    const bins: ([number, number] | null)[] = Array.from({ length: 1200 }, () => null);
    bins[4] = [-0.4, 0.5];
    const analysisCache = new Map<string, unknown>([[cacheKey, {
      key: cacheKey, updatedAt: now, expiresAt: now + 7 * 24 * 60 * 60 * 1000,
      durationSeconds: 240, binCount: 1200, scope: "wholeTrack", range: null, bins,
    }]]);
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "read_analysis_cache_record") return analysisCache.get(String(args?.key)) ?? null;
      if (command === "write_analysis_cache_record") { analysisCache.set(String(args?.key), structuredClone(args?.payload)); return undefined; }
      return originalCall(command, args);
    });
    localStorage.setItem("burrata.waveform-enabled.v1", "true");
    mountNative(makeNativeSnapshot({ revision: 62, generation: 14, seekSerial: 3, status: "paused", trackId: track.id, track, candidate,
      queue: [track], currentIndex: 0, paused: true, duration: 240, quality: { ...variant, formatHasVideo: false, sourceKind: "stream" } }));
    await settle();
    for (let attempt = 0; attempt < 40 && !document.querySelector(".waveform rect"); attempt++) {
      await new Promise(resolve => setTimeout(resolve, 5));
      await settle();
    }
    expect(mocks.call).toHaveBeenCalledWith("get_playback_waveform", expect.objectContaining({ generation: 14, binCount: 1200 }));
    expect(mocks.call).toHaveBeenCalledWith("read_analysis_cache_record", { namespace: "observed-waveform-v1", key: cacheKey });
    expect(mocks.call).toHaveBeenCalledWith("get_playing_track_detections", { trackId: track.id, generation: 14, seekSerial: 3 });
    const renderedBins = [...document.querySelectorAll<SVGRectElement>(".waveform rect")];
    expect(renderedBins).toHaveLength(1);
    expect(Number(renderedBins[0]?.getAttribute("x"))).toBeGreaterThan(4);
    expect(Number(renderedBins[0]?.getAttribute("x"))).toBeLessThan(5);
    eventHandlers.get("automation-inspect-playback-layout")!({ event: "automation-inspect-playback-layout", id: 72, payload: { requestId: "cache-binding-state" } });
    await settle();
    const inspection = lastCallOf("automation_layout_complete")?.[1] as { layout?: { waveformDiagnostics?: { observedCache?: unknown } } } | undefined;
    expect(inspection?.layout?.waveformDiagnostics?.observedCache).toMatchObject({ resourceAvailable: true, resourceStatus: "ready",
      bindingPrepared: true, persistable: true, trackMatches: true, generationMatches: true, persistentEntries: 1 });
    window.dispatchEvent(new Event("pagehide"));
    expect(analysisCache.get(cacheKey)).toMatchObject({ key: cacheKey, bins: expect.arrayContaining([[-0.4, 0.5]]) });
  });

  it("merges newly observed native bins into restored cache coverage", async () => {
    const track = { ...savedTrack, provider: "youtube", durationSeconds: 240 };
    const candidate = youtubeCandidate(track);
    const variant = { formatId: "251", codec: "opus", mimeType: "audio/webm", bitrateKbps: 128, expectedContentLength: 12345, containsVideo: false };
    const resourceKey = playingTrackDetectionMock.resourceKey;
    const preparedFixture = await prepareObservedWaveformBinding({ resourceKey, trackId: track.id, generation: 14, durationSeconds: 240,
      binCount: 1200, scope: "wholeTrack", range: null, identity: { kind: "stream", provider: "youtube", sourceId: candidate.id, audioVariant: variant } });
    const cacheKey = preparedFixture!.persistenceKey!;
    const now = Date.now();
    const bins: ([number, number] | null)[] = Array.from({ length: 1200 }, () => null);
    bins[4] = [-0.4, 0.5];
    const analysisCache = new Map<string, unknown>([[cacheKey, {
      key: cacheKey, updatedAt: now, expiresAt: now + 7 * 24 * 60 * 60 * 1000,
      durationSeconds: 240, binCount: 1200, scope: "wholeTrack", range: null, bins,
    }]]);
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "read_analysis_cache_record") return analysisCache.get(String(args?.key)) ?? null;
      if (command === "write_analysis_cache_record") { analysisCache.set(String(args?.key), structuredClone(args?.payload)); return undefined; }
      return originalCall(command, args);
    });
    const observedBins: ({ min: number; max: number } | null)[] = Array.from({ length: 1200 }, () => null);
    observedBins[8] = { min: -0.2, max: 0.3 };
    const original = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => command === "get_playback_waveform"
      ? { generation: 14, durationSeconds: 240, bins: observedBins, complete: false }
      : original(command, args));
    localStorage.setItem("burrata.waveform-enabled.v1", "true");
    mountNative(makeNativeSnapshot({ revision: 62, generation: 14, seekSerial: 3, status: "paused", trackId: track.id, track, candidate,
      queue: [track], currentIndex: 0, paused: true, duration: 240, quality: { ...variant, formatHasVideo: false } }));
    for (let attempt = 0; attempt < 300 && document.querySelectorAll(".waveform rect").length < 2; attempt++) {
      await new Promise(resolve => setTimeout(resolve, 5));
      await settle();
    }
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(2);
    expect(mocks.call).toHaveBeenCalledWith("read_analysis_cache_record", { namespace: "observed-waveform-v1", key: cacheKey });
    window.dispatchEvent(new Event("pagehide"));
    const stored = JSON.parse(localStorage.getItem("burrata.observed-waveform-cache.v1")!) as { records: { key: string; bins: ([number, number] | null)[] }[] };
    expect(stored.records).toHaveLength(1);
    expect(stored.records[0]?.key).toBe(cacheKey);
    expect(stored.records[0]?.bins[4]).toEqual([-0.4, 0.5]);
    expect(stored.records[0]?.bins[8]).toEqual([-0.2, 0.3]);
    expect(analysisCache.get(cacheKey)).toMatchObject({ key: cacheKey, bins: expect.arrayContaining([[-0.4, 0.5], [-0.2, 0.3]]) });
  });

  it("restores a streamed playback range using absolute parent bounds", async () => {
    const track = { ...savedTrack, provider: "youtube", durationSeconds: 10,
      playbackRange: { parentTrackId: savedTrack.id, chapterId: "clip-100", sourceUrl: savedTrack.sourceUrl,
        startTime: 100, endTime: 110, parentDurationSeconds: 240, parentTitle: savedTrack.title } };
    const candidate = youtubeCandidate(track);
    const variant = { formatId: "251", codec: "opus", mimeType: "audio/webm", bitrateKbps: 128, expectedContentLength: 12345, containsVideo: false };
    const resourceKey = playingTrackDetectionMock.resourceKey;
    const identity = `stream\nyoutube\n${candidate.id}\n${streamAudioVariantIdentity(variant)}`;
    const persistentText = `burrata-observed-waveform-cache-v1\nresource=${resourceKey}\ntrack=${track.id}\nduration=10\nbins=1200\nscope=playbackRange\nrange=100\n110\nidentity=${identity}`;
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(persistentText));
    const cacheKey = [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
    const now = Date.now();
    const bins: ([number, number] | null)[] = Array.from({ length: 1200 }, () => null);
    bins[4] = [-0.4, 0.5];
    localStorage.setItem("burrata.observed-waveform-cache.v1", JSON.stringify({ schemaVersion: 1, records: [{
      key: cacheKey, updatedAt: now, expiresAt: now + 7 * 24 * 60 * 60 * 1000,
      durationSeconds: 10, binCount: 1200, scope: "playbackRange", range: [100, 110], bins,
    }] }));
    localStorage.setItem("burrata.waveform-enabled.v1", "true");
    mountNative(makeNativeSnapshot({ revision: 62, generation: 14, seekSerial: 3, status: "paused", trackId: track.id, track, candidate,
      queue: [track], currentIndex: 0, paused: true, duration: 10, quality: { ...variant, formatHasVideo: false } }));
    for (let attempt = 0; attempt < 40 && !document.querySelector(".waveform rect"); attempt++) {
      await new Promise(resolve => setTimeout(resolve, 5));
      await settle();
    }
    expect(mocks.call).toHaveBeenCalledWith("get_playback_waveform", expect.objectContaining({ generation: 14, startSeconds: 100, durationSeconds: 10 }));
    expect(document.querySelectorAll(".waveform rect")).toHaveLength(1);
  });

  it("keeps the newest observed binding when an older stable waveform hash resolves late", async () => {
    const track = { ...savedTrack, provider: "youtube", durationSeconds: 240 };
    const candidate = youtubeCandidate(track);
    const firstVariant = { formatId: "251", codec: "opus", mimeType: "audio/webm", bitrateKbps: 128, expectedContentLength: 12345, containsVideo: false };
    const nextVariant = { ...firstVariant, formatId: "140", expectedContentLength: 23456 };
    const resourceKey = playingTrackDetectionMock.resourceKey;
    const observedBins: ({ min: number; max: number } | null)[] = Array.from({ length: 1200 }, () => null);
    observedBins[11] = { min: -0.3, max: 0.4 };
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => command === "get_playback_waveform"
      ? { generation: 14, durationSeconds: 240, bins: observedBins, complete: false }
      : originalCall(command, args));
    const nativeDigest = crypto.subtle.digest.bind(crypto.subtle);
    let finishOlderDigest: ((value: ArrayBuffer) => void) | undefined;
    vi.spyOn(crypto.subtle, "digest").mockImplementation((algorithm, data) => {
      const text = new TextDecoder().decode(data as BufferSource);
      if (text.startsWith("burrata-observed-waveform-cache-v2\n") && text.includes(`identity=stream-recording-v1\nyoutube\n${candidate.id}`) && !finishOlderDigest) {
        return new Promise<ArrayBuffer>(resolve => { finishOlderDigest = resolve; });
      }
      return nativeDigest(algorithm, data);
    });
    localStorage.setItem("burrata.waveform-enabled.v1", "true");
    mountNative(makeNativeSnapshot({ revision: 62, generation: 14, seekSerial: 3, status: "paused", trackId: track.id, track, candidate,
      queue: [track], currentIndex: 0, paused: true, duration: 240, quality: { ...firstVariant, formatHasVideo: false } }));
    await settle();
    expect(finishOlderDigest).toBeDefined();
    emitNative(makeNativeSnapshot({ revision: 63, generation: 14, seekSerial: 3, status: "paused", trackId: track.id, track, candidate,
      queue: [track], currentIndex: 0, paused: true, duration: 240, quality: { ...nextVariant, formatHasVideo: false } }));
    await settle();
    const olderText = `burrata-observed-waveform-cache-v2\nbins=1200\nscope=wholeTrack\nrange=whole-track\nidentity=stream-recording-v1\nyoutube\n${candidate.id}`;
    finishOlderDigest?.(await nativeDigest("SHA-256", new TextEncoder().encode(olderText)));
    await settle();
    window.dispatchEvent(new Event("pagehide"));
    const nextText = `burrata-observed-waveform-cache-v2\nbins=1200\nscope=wholeTrack\nrange=whole-track\nidentity=stream-recording-v1\nyoutube\n${candidate.id}`;
    const nextHash = await nativeDigest("SHA-256", new TextEncoder().encode(nextText));
    const nextKey = [...new Uint8Array(nextHash)].map(byte => byte.toString(16).padStart(2, "0")).join("");
    const stored = JSON.parse(localStorage.getItem("burrata.observed-waveform-cache.v1")!) as { records: { key: string; bins: ([number, number] | null)[] }[] };
    expect(stored.records).toHaveLength(1);
    expect(stored.records[0]?.key).toBe(nextKey);
    expect(stored.records[0]?.bins[11]).toEqual([-0.3, 0.4]);
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
    expect(status.textContent).toContain("Paused");
    expect(status.textContent).not.toContain("Unavailable");
    expect([...document.querySelectorAll(".player-scrubber .time")].map(node=>node.textContent)).toContain("1:01");
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
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(playAndAdvanceMediaClock);vi.spyOn(HTMLMediaElement.prototype,"pause").mockImplementation(()=>{});
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    if(options.repeat)await automationAction('finite-scan-repeat','repeat',{mode:options.repeat});
    const requestId='initial-unavailable-opener';
    eventHandlers.get('automation-playback')!({event:'automation-playback',id:1,payload:{requestId,action:'play',trackIds:(options.single?rows.slice(0,1):rows).map(track=>track.id)}});await settle();
    return {rows,resolved,requestId,release:()=>release,completion:()=>mocks.call.mock.calls.find(([command,args])=>command==='automation_playback_complete'&&(args as {requestId?:string}).requestId===requestId)?.[1] as {error:string|null;playback:{currentIndex:number;trackId:string;paused:boolean;position:number;queue:Track[];queueEntryIds:string[];diagnostics:{recentTrackFailures:import("./lib/types").PlaybackTrackFailure[]}}}|undefined};
  }

  async function mountQueueFailureFixture(reason: string, mode: "normal"|"repeat-one"|"hold"|"refresh"|"radio"|"end" = "normal", middleOverrides: Partial<Track> = {}) {
    previewMode="fallback";
    const middle={...secondTrack,id:"album-unavailable-middle",title:"Unavailable middle album recording",...middleOverrides};
    if(mode==="radio") {
      Object.assign(middle,{provider:"internet-radio",providerId:"fixture-radio"});
      radioStationState=[{id:"fixture-radio",name:"Fixture Radio",provider:"custom",sourceUrl:"https://radio.example",streamUrl:"https://radio.example/live",isCustom:true}];
    }
    const last={...savedTrack,id:"album-success-last",title:"Successful last album recording"};
    savedChartTracks=mode==="end"?[savedTrack,middle]:[savedTrack,middle,last];
    const play=vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(playAndAdvanceMediaClock);
    if(mode==="radio") play.mockImplementationOnce(playAndAdvanceMediaClock).mockRejectedValueOnce(new Error("Radio unavailable"));
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

  it("ordinary notices expire independently of the persistent player error", async () => {
    mountNative(makeNativeSnapshot());await settle();vi.useFakeTimers();
    try {
      const failure={trackId:savedTrack.id,title:savedTrack.title,artist:savedTrack.artist,reason:"No playable audio match"};
      const failed=makeNativeSnapshot({revision:30,status:"failed",diagnostics:{lastSourceFailure:failure,lastSkippedTrackFailure:null}});
      emitNative(failed);await settle();
      await vi.advanceTimersByTimeAsync(6000);await tick();
      expect(document.querySelector(".banner.notice")).toBeNull();
      expect(document.querySelector(".player-error")?.textContent).toContain("Could not play");
      const warning=eventHandlers.get("provider-warning");expect(warning).toBeTypeOf("function");
      warning!({event:"provider-warning",id:1,payload:"Ordinary update completed."});await tick();
      await vi.advanceTimersByTimeAsync(5499);await tick();
      expect(document.querySelector(".banner.notice")?.textContent).toContain("Ordinary update completed.");
      await vi.advanceTimersByTimeAsync(1);await tick();expect(document.querySelector(".banner.notice")).toBeNull();
      emitNative({...failed,revision:31,diagnostics:{lastSourceFailure:{...failure,title:"Fresh failure"},lastSkippedTrackFailure:null}});await settle();
      expect(document.querySelector(".banner.notice")).toBeNull();
      expect(document.querySelector(".player-error")?.textContent).toContain("Fresh failure");
      const navigation=eventHandlers.get("automation-ui-navigation");expect(navigation).toBeTypeOf("function");navigation!({event:"automation-ui-navigation",id:32,payload:{requestId:"notice-navigation-clear",target:{kind:"view",view:"favorites"}}});await settle();await settle();
      expect(document.querySelector(".banner.notice")).toBeNull();
    } finally {vi.useRealTimers();}
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
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({action:"reorder"})});
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
    expect([...document.querySelectorAll<HTMLElement>(".player-scrubber .time")].map((item) => item.textContent)).toEqual([]);
    expect(document.querySelector(".radio-live-meta .time")?.textContent).toBe("0:00");
    expect(document.querySelector(".radio-live-label")?.textContent).toBe("Live radio");
    expect(document.querySelector("#burrata-playback-controls")?.classList.contains("radio-live")).toBe(true);
    expect(document.querySelector('[aria-label="Playback position"]')).toBeNull();
    expect(document.querySelector('[aria-label="Playback position, waveform"]')).toBeNull();
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

  it("opens the current radio station without replacing Native metadata or restarting playback", async () => {
    radioStationState=[{id:"main",name:"Main Mix",streamUrl:"https://radio.example/main",sourceUrl:"https://radio.example",provider:"radio_paradise",isCustom:false}];
    const track:Track={...savedTrack,id:"radio:main",provider:"radio",providerId:"main",title:"Overnight selection",artist:"Radio Paradise",sourceUrl:"https://radio.example",provenance:{stationId:"main",sourceKind:"radio",radioStreamUrl:"https://radio.example/main"}};
    mountNative(makeNativeSnapshot({revision:50,generation:9,status:"playing",trackId:track.id,track,queue:[track],currentIndex:0,paused:false,position:127,duration:0}));
    await settle();
    mocks.call.mockClear();
    document.querySelector<HTMLButtonElement>(".radio-section .playlist-card")!.click();
    await settle();
    expect(document.querySelector(".radio-page")).not.toBeNull();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Overnight selection");
    expect(document.querySelector(".player-source")).toBeNull();
    const footerInfo=document.querySelector("footer .station-info")!;
    footerInfo.querySelector("summary")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    expect(footerInfo.querySelector('a[href="https://radio.example/"]')).not.toBeNull();
    expect(footerInfo.querySelector('a[href="https://radio.example/main"]')).not.toBeNull();
    document.querySelector<HTMLButtonElement>('[aria-label="Show playing track information"]')!.click();await settle();
    await vi.waitFor(() => expect(document.querySelector('[aria-label="Playing track information"]')).not.toBeNull());
    const panelInfo=document.querySelector('[aria-label="Playing track information"]')!;
    const sourceRow=Array.from(panelInfo.querySelectorAll("dl > div")).find(row=>row.querySelector("dt")?.textContent==="Source")!;
    const stationLink=sourceRow.querySelector<HTMLAnchorElement>("a")!;
    expect(stationLink.textContent).toBe("Main Mix ↗");
    expect(stationLink.getAttribute("href")).toBe("https://radio.example/");
    expect(panelInfo.querySelector(".station-info")).toBeNull();
    stationLink.dispatchEvent(new MouseEvent("click",{bubbles:true,cancelable:true}));await settle();
    expect(mocks.call).toHaveBeenCalledWith("open_external_url",{url:"https://radio.example/"});
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Overnight selection");
    expect(mocks.call.mock.calls.some(([command])=>command==="playback_command")).toBe(false);
    expect(mocks.call.mock.calls.some(([command])=>command==="resolve_preview"||command==="resolve_candidate_audio")).toBe(false);
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
    mountNative(makeNativeSnapshot({status:"paused",track:savedTrack,trackId:savedTrack.id,queue:[savedTrack,secondTrack],
      queueRevision:27,queueEntryIds:["saved-track-occurrence","second-track-occurrence"],currentIndex:0,paused:true}));
    await settle();
    mocks.call.mockClear();
    [...document.querySelectorAll(".shared-track-list > li")].find(row=>row.textContent?.includes(secondTrack.title))!
      .dispatchEvent(new MouseEvent("click",{bubbles:true}));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("playback_command",{command:expect.objectContaining({
      action:"select",index:1,expectedQueueRevision:27,expectedEntryId:"second-track-occurrence",
    })});
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
    const currentRadioTrack:Track={...savedTrack,id:`radio_song:${nativeRadioSongHash}`,providerId:nativeRadioSongHash,title:"Current Song",artist:"Radio Artist",provider:"radio_song"};
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
    expect(document.querySelector(".radio-status-copy")?.textContent).toBe("Test Station");
    expect(mocks.call.mock.calls.some(([command])=>command==="poll_radio_station")).toBe(false);
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
    await settle();
    expect(document.querySelector(".playback-queue-panel .radio-current")?.textContent).toContain("Current Song");
    document.querySelector<HTMLButtonElement>(".playback-queue-panel .history-toggle")!.click();
    await settle();
    expect(document.querySelector(".playback-queue-panel .radio-history")?.textContent).toContain("Earlier Repeat");
  });

  it("favorites the canonical current native radio song and keeps it favorited through a stale snapshot", async () => {
    const stationTrack: Track = {
      ...savedTrack, id: "station-radio", provider: "radio", providerId: "station-1",
      title: "Station Feed", artist: "Radio Station", provenance: { sourceKind: "radio", stationId: "station-1" },
    };
    const currentRadioTrack: Track = {
      ...savedTrack, id: `radio_song:${nativeRadioSongHash}`, provider: "radio_song", providerId: nativeRadioSongHash,
      title: "Identified Song", artist: "Identified Artist", sourceUrl: "", isrc: "USABC1234567",
      provenance: {sourceKind: "radioSong", radioStationId: "station-1", recognitionIdentifiers: {isrc: "US-ABC-12-34567"}},
    };
    savedChartTracks = [savedTrack, currentRadioTrack];
    radioStationState = [{ id: "station-1", name: "Test Station", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
    const snapshot = makeNativeSnapshot({
      revision: 50, generation: 20, status: "playing", trackId: stationTrack.id, track: stationTrack,
      queue: [stationTrack], currentIndex: 0, paused: false,
      radioNowPlaying: {
        id: currentRadioTrack.id, title: currentRadioTrack.title, artist: currentRadioTrack.artist,
        stationName: "Test Station", track: currentRadioTrack,
        metadata: { stationId: "station-1", title: currentRadioTrack.title, artist: currentRadioTrack.artist, sourceUrl: "https://example.test/song", metadataConfidence: "high", identifiers: {isrc: "US-ABC-12-34567"} },
      },
    });
    mountNative(snapshot);
    await settle();

    expect(document.querySelector(".radio-status-copy")?.textContent).toBe("Test Station");
    expect(document.querySelector('.radio-section [aria-current="true"]')?.textContent).toContain("Test Station");
    const addFavorite = document.querySelector<HTMLButtonElement>('[aria-label="Add playing track to favorites"]')!;
    expect(addFavorite.disabled).toBe(false);
    addFavorite.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("set_favorite", {
      track: expect.objectContaining({ id: currentRadioTrack.id, provider: "radio_song", providerId: nativeRadioSongHash, sourceUrl: "", isrc: "USABC1234567", provenance: expect.objectContaining({sourceKind: "radioSong", recognitionIdentifiers: {isrc: "US-ABC-12-34567"}}) }),
      value: true,
    });
    expect(favoriteTrackIds.has(currentRadioTrack.id)).toBe(true);

    emitNative({
      ...snapshot,
      revision: 51,
      track: { ...stationTrack, favorite: false, title: "Stale station title" },
      queue: [{ ...stationTrack, favorite: false, title: "Stale station title" }],
    });
    await settle();
    const removeFavorite = document.querySelector<HTMLButtonElement>('[aria-label="Remove playing track from favorites"]')!;
    expect(removeFavorite.disabled).toBe(false);
    expect(removeFavorite.getAttribute("aria-pressed")).toBe("true");
  });

  it.each(["radio_song:", "radio_song:radio_song:canonical-song"])(
    "disables favorite for malformed current native radio identity %s",
    async (id) => {
      const stationTrack: Track = {
        ...savedTrack, id: "station-radio", provider: "radio", providerId: "station-1",
        title: "Station Feed", artist: "Radio Station", provenance: { sourceKind: "radio", stationId: "station-1" },
      };
      radioStationState = [{ id: "station-1", name: "Test Station", streamUrl: "https://radio.example/live", sourceUrl: "https://radio.example", provider: "radio_paradise", isCustom: false }];
      mountNative(makeNativeSnapshot({
        revision: 60, generation: 21, status: "playing", trackId: stationTrack.id, track: stationTrack,
        queue: [stationTrack], currentIndex: 0, paused: false,
        radioNowPlaying: {
          id, title: "Identified Song", artist: "Identified Artist", stationName: "Test Station",
          metadata: { stationId: "station-1", title: "Identified Song", artist: "Identified Artist", sourceUrl: "https://example.test/song", metadataConfidence: "high" },
        },
      }));
      await settle();

      const favoriteButton = document.querySelector<HTMLButtonElement>('[aria-label="Add playing track to favorites"]')!;
      expect(favoriteButton.disabled).toBe(true);
      favoriteButton.click();
      await settle();
      expect(mocks.call.mock.calls.some(([command]) => command === "set_favorite")).toBe(false);
    },
  );
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

it("does not expose an audio engine selector in playback settings", async () => {
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
  document.querySelector<HTMLButtonElement>('.library-sidebar [aria-label="Settings"]')!.click();await settle();
  [...document.querySelectorAll<HTMLButtonElement>(".settings-nav button")].find(button=>button.textContent?.trim()==="Playback")!.click();await settle();
  expect(document.querySelector('select[aria-label="Audio playback engine"]')).toBeNull();
  expect(document.body.textContent).not.toContain("Apply and restart");
});

it("opens saved Soulseek searches from Downloads and reviews a saved query in Soulseek Search",async()=>{
  slskdConnectionEnabled=true;
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
  expect(document.querySelector("dialog.wishlist[open]")).toBeNull();
  const navigation=eventHandlers.get("automation-ui-navigation")!;
  navigation({event:"automation-ui-navigation",id:1,payload:{requestId:"wishlist-downloads",target:{kind:"view",view:"downloads"}}});await settle();await settle();
  document.querySelector<HTMLDetailsElement>("details.download-tools")!.open=true;
  [...document.querySelectorAll<HTMLButtonElement>("details.download-tools button")].find(button=>button.textContent?.includes("Saved Soulseek searches"))!.click();await settle();
  const panel=document.querySelector<HTMLDialogElement>("dialog.wishlist[open]")!;expect(panel).not.toBeNull();
  const query=panel.querySelector<HTMLInputElement>('[aria-label="Soulseek wishlist query"]')!;query.value="Synthetic wishlist track";query.dispatchEvent(new Event("input",{bubbles:true}));await settle();
  [...panel.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.includes("Save search"))!.click();await settle();
  expect(JSON.parse(localStorage.getItem("burrata.soulseek.wishlist.v1")!).entries).toHaveLength(1);
  [...panel.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent==="Search now")!.click();await settle();
  expect(document.querySelector("dialog.wishlist[open]")).toBeNull();
  expect(document.querySelector<HTMLSelectElement>('select[aria-label="Search in"]')?.value).toBe("soulseek");
  expect(document.querySelector<HTMLInputElement>('input[aria-label="Search music"]')?.value).toBe("Synthetic wishlist track");
});

it.each(["next in queue", "playback bar"])("keeps fullscreen open when opening the queue from %s", async (entry) => {
  nativePlaybackSnapshot = makeNativeSnapshot({ status: "paused", track: savedTrack, trackId: savedTrack.id, queue: [savedTrack], currentIndex: 0 });
  const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); }; await settle();
  document.querySelector<HTMLButtonElement>('[aria-label="Full-screen Now Playing"]')!.click(); await settle();
  const fullscreen = document.querySelector(".now-playing-view")!;
  expect(fullscreen).not.toBeNull();
  if (entry === "next in queue") {
    [...fullscreen.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Open queue")!.click();
  } else document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click();
  await settle();
  expect(document.querySelector(".now-playing-view")).toBe(fullscreen);
  expect(document.querySelector(".player-panel-host.fullscreen-queue .playback-queue-panel")).not.toBeNull();
  expect(document.querySelector('[aria-label="Full-screen Now Playing"]')?.getAttribute("aria-pressed")).toBe("true");
  document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
  expect(document.querySelector(".player-panel-host")).toBeNull();
  expect(document.querySelector(".now-playing-view")).toBe(fullscreen);
  document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
  document.querySelector<HTMLButtonElement>('[aria-label="Close player panel"]')!.click(); await settle();
  expect(document.querySelector(".player-panel-host")).toBeNull();
  expect(document.querySelector(".now-playing-view")).toBe(fullscreen);
  document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })); await settle();
  expect(document.querySelector(".player-panel-host")).toBeNull();
  expect(document.querySelector(".now-playing-view")).toBe(fullscreen);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })); await settle();
  expect(document.querySelector(".now-playing-view")).toBeNull();
});

it("keeps the selected public artwork source available for fullscreen upgrades after caching",async()=>{
  const remote="https://yt3.googleusercontent.com/SyntheticAsset_123=w120-h120-l90-rj";
  const cached="asset://localhost/cached-qualification-art.jpg";
  const probes:string[]=[];
  class ImageProbe {
    naturalWidth=1200;naturalHeight=1200;
    onload:(()=>void)|null=null;onerror:(()=>void)|null=null;
    set src(value:string){probes.push(value);queueMicrotask(()=>this.onload?.());}
  }
  vi.stubGlobal("Image",ImageProbe);
  const track={...savedTrack,provider:"youtube",artworkUrl:remote};
  nativePlaybackSnapshot=makeNativeSnapshot({status:"paused",track,trackId:track.id,queue:[track],currentIndex:0});
  const original=mocks.call.getMockImplementation()!;
  mocks.call.mockImplementation(async(command:string,args?:Record<string,unknown>)=>command==="cache_artwork"?cached:original(command,args));
  const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
  document.querySelector<HTMLButtonElement>('[aria-label="Full-screen Now Playing"]')!.click();await settle();
  const image=document.querySelector<HTMLImageElement>(".now-playing-view .hero-art img")!;
  expect(image.src).toBe(cached);
  Object.defineProperty(image,"naturalWidth",{configurable:true,value:120});image.dispatchEvent(new Event("load"));await settle();
  expect(probes).toContain(remote.replace("w120-h120","w600-h600"));
  expect(document.querySelector<HTMLImageElement>(".now-playing-view .hero-art img")?.src).toContain("=w600-h600-l90-rj");
});
