// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserImportResult, Candidate, Chart, Collection, DownloadJob, LocalAsset, Preview, Settings, Track } from "./lib/types";
import type { ArtistReleaseRef } from "./lib/artist-types";

const mocks = vi.hoisted(() => ({ call: vi.fn(), listen: vi.fn() }));
vi.mock("./lib/api", () => ({
  call: mocks.call,
  listen: mocks.listen,
  desktop: () => true,
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
const collectionTrack: Track = {
  ...savedTrack, id: "collection-recording", title: "Existing Collection Member",
};
const secondTrack: Track = {
  ...savedTrack, id: "recording-2", artist: "Another Artist", title: "Another Recording",
};
const eventHandlers = new Map<string, (event: { event: string; id: number; payload: unknown }) => void>();
let failNextCollectionLoad = false;
let unmountApp: (() => void) | undefined;
let holdStartupCalls = false;
let delayedBrowserImports: Promise<BrowserImportResult[]> | undefined;
let directBrowserImport: BrowserImportResult | null = null;
let directBrowserImportFailure = "";
let previewMode: "idle" | "fallback" | "race" | "direct" | "alternates" = "idle";
let finishOldSearch: ((value: Candidate[]) => void) | undefined;
let finishAudioResolve: ((value: Preview) => void) | undefined;
let delayAudioCandidateId: string | undefined;
let candidateAudioFailure = "";
let finishCandidateInspect: ((value: Candidate) => void) | undefined;
let delayCandidateInspectId: string | undefined;
let delayedCandidateProviders = new Map<string, (value: Candidate[]) => void>();
let failedCandidateProviders = new Set<string>();
let songRadioTracks: Track[] = [];
let songRadioFailure = "";
let batchInspectionHigh = false;
let artistProfileTopTracks: Track[] = [];
let savedChartTracks: Track[] = [savedTrack];
let audioSearchResult: Track[] = [];
let partialSoulseekSearch = false;
let discoveryAlbum: ArtistReleaseRef | null = null;
let discoveryAlbumTracks: Track[] = [];
let configuredMusicDirectory = "";
let configuredPreferredDownloadSource = "youtube";
let configuredPreferredStreamingSource = "youtube";
let overridePlaylistTrackUrls = false;
let monochromeCandidates: Candidate[] = [];
let streamingFallbackEnabled = true;
let streamingFallbackSources = ["youtube", "soundcloud"];
let downloadFallbackEnabled = false;
let downloadFallbackSources = ["monochrome", "slskd", "youtube", "soundcloud"];
let slskdConnectionEnabled = false;
let discogsConnectionEnabled = false;
let playlistRemoved = false;
let deletedCollectionIds = new Set<string>();
let deletedCollectionRecords: Collection[] = [];
let savedAssets: LocalAsset[] = [];
let analysisStatusResult: { assetId: string; inputHash?: string; version?: number; status: string; payload?: Record<string, unknown> | null }[] = [];
let savedSnapshots: Chart[] = [];
let historyEntries: { id: string; track: Track; provider: string; sessionId: string; playedAt: string; chapter?: unknown }[] = [];
let chapterResults: Candidate["chapters"] = null;
let downloadJobs: DownloadJob[] = [];
let sidebarLayoutState: { sections: { id: string; name: string }[]; assignments: Record<string, string>; names?: Record<string, string> } = { sections: [], assignments: {} };
let sidebarLayoutSaves: typeof sidebarLayoutState[] = [];
type MockRadioStation = { id: string; name: string; streamUrl: string; sourceUrl: string; provider?: string; description?: string; homepageUrl?: string; faviconUrl?: string; isCustom?: boolean };
let radioStationState: MockRadioStation[] = [];

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
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
}
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
      "musicbrainz": "musicbrainz", "discogs": "discogs", "youtube": "youtube", "soundcloud": "soundcloud", "soulseek": "soulseek",
    } as Record<string, string>)[requested] ?? "all";
    if (picker.value !== value) {
      picker.value = value;
      picker.dispatchEvent(new Event("change", { bubbles: true }));
    }
  }
  await settle();
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
  const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes(trackTitle));
  const button = row?.querySelector<HTMLButtonElement>(`[aria-label="Actions for ${trackTitle}"]`);
  if (!button) throw new Error(`Could not find actions for ${trackTitle}`);
  button.click();
  await settle();
  const menu = document.querySelector<HTMLDialogElement>("dialog.track-menu");
  if (!menu) throw new Error(`Track actions menu did not open for ${trackTitle}`);
  return menu;
}
async function chooseTrackAction(action: string, trackTitle = "Saved Recording") {
  const menu = await trackActions(trackTitle);
  const button = [...menu.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === action);
  if (!button) throw new Error(`Could not find '${action}' for ${trackTitle}`);
  button.click();
  await settle();
}
async function confirmReviewedDownload() {
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

beforeEach(() => {
  document.body.innerHTML = "";
  try { localStorage.removeItem("burrata.discovery.sources.v1"); localStorage.removeItem("burrata.discovery.search-scope.v2"); } catch {}
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
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
  mocks.listen.mockResolvedValue(() => {});
  eventHandlers.clear();
  failNextCollectionLoad = false;
  holdStartupCalls = false;
  delayedBrowserImports = undefined;
  directBrowserImport = null;
  directBrowserImportFailure = "";
  previewMode = "idle";
  finishOldSearch = undefined;
  finishAudioResolve = undefined;
  delayAudioCandidateId = undefined;
  candidateAudioFailure = "";
  finishCandidateInspect = undefined;
  delayCandidateInspectId = undefined;
  delayedCandidateProviders = new Map();
  failedCandidateProviders = new Set();
  songRadioTracks = [];
  songRadioFailure = "";
  batchInspectionHigh = false;
  artistProfileTopTracks = [];
  savedChartTracks = [savedTrack];
  audioSearchResult = [];
  partialSoulseekSearch = false;
  discoveryAlbum = null;
  discoveryAlbumTracks = [];
  configuredMusicDirectory = "";
  configuredPreferredDownloadSource = "youtube";
  configuredPreferredStreamingSource = "youtube";
  overridePlaylistTrackUrls = false;
  monochromeCandidates = [];
  streamingFallbackEnabled = true;
  streamingFallbackSources = ["youtube", "soundcloud"];
  downloadFallbackEnabled = false;
  downloadFallbackSources = ["monochrome", "slskd", "youtube", "soundcloud"];
  slskdConnectionEnabled = false;
  discogsConnectionEnabled = false;
  playlistRemoved = false;
  deletedCollectionIds = new Set();
  deletedCollectionRecords = [];
  savedAssets = [];
  analysisStatusResult = [];
  savedSnapshots = [];
  historyEntries = [];
  chapterResults = null;
  downloadJobs = [];
  sidebarLayoutState = { sections: [], assignments: {} };
  sidebarLayoutSaves = [];
  radioStationState = [];
  mocks.listen.mockImplementation(async (event: string, handler: (event: { event: string; id: number; payload: unknown }) => void) => {
    eventHandlers.set(event, handler);
    return () => {};
  });
  mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_connections": return { settings: { slskd: { enabled: slskdConnectionEnabled }, discogsEnabled: discogsConnectionEnabled }, slskdCredential: { present: slskdConnectionEnabled, sessionOnly: false }, discogsCredential: { present: discogsConnectionEnabled, sessionOnly: false }, lastfmCredential: { present: false, sessionOnly: false } };
      case "get_automation_status": return { permissions: { control: true, manage: false }, playback: { status: "idle", queue: [] } };
      case "automation_playback_state": return undefined;
      case "automation_playback_complete": return { acknowledged: true };
      case "search_artists": return { query: args?.query, page: 0, pageSize: 20, hasMore: false, results: [{ artist: { provider: "musicbrainz", id: "artist-1" }, name: "Visible Artist", sourceUrl: "https://musicbrainz.org/artist/artist-1" }], warnings: [] };
      case "get_artist_profile": {
        const artist = args?.artist as { provider: string; id: string };
        return { artist: { artist, name: "Visible Artist", sourceUrl: "https://musicbrainz.org/artist/artist-1" }, genres: [], tags: [], sourceTags: [], aliases: [], links: [], topTracks: artistProfileTopTracks, releases: { artist, items: [], warnings: [] }, discogsAvailable: false, lastfmAvailable: false, sourceCoverage: [], warnings: [] };
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
        return { musicDirectory: configuredMusicDirectory, preferredDownloadSource: configuredPreferredDownloadSource, preferredStreamingSource: configuredPreferredStreamingSource, overridePlaylistTrackUrls, streamingFallbackEnabled, streamingFallbackSources, downloadFallbackEnabled, downloadFallbackSources, autoPreview: false, previewDelayMs: 250, theme: "dark", ytDlpPath: "yt-dlp", ffmpegPath: "ffmpeg", ffprobePath: "ffprobe", jsRuntimePath: "deno" };
      case "get_sidebar_layout": return sidebarLayoutState;
      case "save_sidebar_layout": {
        sidebarLayoutState = args?.layout as typeof sidebarLayoutState;
        sidebarLayoutSaves.push(sidebarLayoutState);
        return sidebarLayoutState;
      }
      case "get_radio_stations": return radioStationState;
      case "get_station_details": return { stationId: args?.stationId, history: [], historyAvailable: false };
      case "poll_radio_station": return { stationId: args?.stationId, nowPlaying: null, history: [], historyAvailable: false, note: "" };
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
        return { tracks: songRadioTracks, total: songRadioTracks.length };
      case "get_diagnostics":
        if (holdStartupCalls) return new Promise(() => {});
        return [];
      case "list_collections": return collections.filter((collection) => !deletedCollectionIds.has(collection.id));
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
      case "list_playback_history": return historyEntries;
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
      case "list_tracks":
        if (args?.view === "collections" && failNextCollectionLoad) {
          failNextCollectionLoad = false;
          throw new Error("Offline");
        }
        return args?.view === "collections" ? (playlistRemoved ? [] : [collectionTrack]) : savedChartTracks;
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
      case "rename_collection": return { id: args?.collectionId, name: args?.name, trackCount: 131 };
      case "remove_from_collection": if (args?.collectionId === "collection-full") playlistRemoved = true; return undefined;
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
      case "inspect_candidate": {
        const candidate = args?.candidate as Candidate;
        if (candidate.id === delayCandidateInspectId) return new Promise((resolve) => { finishCandidateInspect = resolve; });
        return { ...candidate, ...(batchInspectionHigh ? { confidence: "high" } : {}), formats: [{ id: "audio-best", codec: "Opus", extension: "webm", bitrate: 192, lossless: false }] };
      }
      case "get_track_details": throw new Error("Traxsource 403");
      case "resolve_preview": {
        const track = args?.track as Track;
        if (previewMode === "direct") return { kind: "audio", url: "/cached-preview.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: true } satisfies Preview;
        if (previewMode === "race" && track.id === savedTrack.id) throw new Error("Cached source unavailable");
        if (previewMode === "fallback") throw new Error("No cached preview");
        return { kind: "external", url: track.sourceUrl, canSeek: false, canVolume: false, canAutoplay: false, isExcerpt: false } satisfies Preview;
      }
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
          isExcerpt: false, provider: candidate.provider,
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
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("saved recording navigation", () => {
  it("searches for more information from track actions without requiring a catalog match", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Search for more info"))!.click();
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Search for more info"))!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('dialog.info-search button[type="submit"]')!.click();
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("search_source", expect.objectContaining({
      query: "Saved Recording Visible Artist", sourceIds: ["musicbrainz"], kinds: ["track", "artist", "album", "video", "set"],
    }));
    expect(mocks.call.mock.calls.some(([command]) => ["set_favorite", "update_track", "import_browser_snapshot"].includes(String(command)))).toBe(false);
  });

  it("opens a public web search and source link from the more information panel", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const menu = await trackActions();
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Search for more info"))!.click();
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
    expect(document.querySelector("tbody")?.textContent).toContain("Saved Recording");
    expect(mocks.call).not.toHaveBeenCalledWith("load_browser_import", { id: "snapshot-new" });
    expect(document.querySelector('[role="alert"]')).toBeNull();
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
    expect(document.querySelector(".chart-source-actions")?.textContent).toContain("Open source");
    (document.querySelector(".chart-open-source") as HTMLButtonElement).click();
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
    const openSource = [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Open source to refresh"))!;
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
    const openSource = document.querySelector<HTMLButtonElement>(".chart-open-source")!;
    expect(openSource.textContent).toContain("Open source to refresh");
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

  it("opens the most populated collection by default and shows its recordings", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    collectionCard().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.body.textContent).toContain("Existing Collection Member");
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", {
      view: "collections", collectionId: "collection-full",
    });
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Rename")!.click();
    await settle();
    await saveSidebarRename("Sunset Charts");
    expect(mocks.call).toHaveBeenCalledWith("rename_collection", { collectionId: "collection-full", name: "Sunset Charts" });
    expect(sidebarLayoutState.names?.["collection-full"]).toBe("Sunset Charts");
    expect(sidebarLayoutSaves[sidebarLayoutSaves.length - 1]?.names?.["collection-full"]).toBe("Sunset Charts");
    expect(document.querySelector("main > header h1")?.textContent).toBe("Sunset Charts");
    expect([...document.querySelectorAll(".playlist-card")].some((button) => button.textContent?.includes("Sunset Charts"))).toBe(true);
  });

  it("renames the built-in favorites entry through its sidebar context menu", async () => {
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Rename")!.click();
    await settle();
    await saveSidebarRename("Starred Tracks");
    expect(sidebarLayoutState.names?.favorites).toBe("Starred Tracks");
    expect(document.querySelector("main > header h1")?.textContent).toBe("Starred Tracks");
    expect([...document.querySelectorAll(".playlist-card")].some((button) => button.textContent?.includes("Starred Tracks"))).toBe(true);
    expect(mocks.call.mock.calls.filter(([command]) => command === "rename_collection")).toHaveLength(0);
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.trim()==="Remove station")!.click();
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Rename")!.click();
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
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "collections", collectionId: "collection-new" });
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
    expect(mocks.call).toHaveBeenCalledWith("add_to_collection", { collectionId: "collection-empty", tracks: savedChartTracks });
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
    const rows = [...document.querySelectorAll<HTMLTableRowElement>("tbody tr:not(.spacer)")];
    expect(rows).toHaveLength(3);
    rows[0].dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const previews = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    rows[2].dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    await settle();
    expect([...document.querySelectorAll("tbody tr:not(.spacer).selected")]).toHaveLength(3);
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
    expect([...document.querySelectorAll("tbody tr:not(.spacer) .row-number")].map((node) => node.textContent?.trim())).toEqual(["1", "2", "3"]);
  });

  it("opens the row context menu without playing or changing the selected recording", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll("tbody tr:not(.spacer)")];
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
    expect(document.querySelector('[aria-label="Select Existing Collection Member"]')).not.toBeNull();
    [...document.querySelectorAll("button")].find((item) => item.textContent?.includes("Remove 1 from playlist"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("remove_from_collection", {
      collectionId: "collection-full", trackIds: [collectionTrack.id],
    });
    expect(document.querySelector("tbody")?.textContent).not.toContain("Existing Collection Member");
  });

  it("removes one recording directly from its active playlist only", async () => {
    delayedBrowserImports = Promise.resolve([]);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click(); await settle();
    await chooseTrackAction("Remove from playlist", "Existing Collection Member");
    expect(mocks.call).toHaveBeenCalledWith("remove_from_collection", { collectionId: "collection-full", trackIds: [collectionTrack.id] });
    expect(mocks.call.mock.calls.filter(([command]) => command === "remove_from_collection")).toHaveLength(1);
    expect(document.querySelector("tbody")?.textContent).not.toContain("Existing Collection Member");
    expect(collectionCard("Empty Collection")).not.toBeNull();
  });

  it("deletes and restores a playlist by stable ID without changing queued or playing audio", async () => {
    delayedBrowserImports = Promise.resolve([]);
    previewMode = "direct";
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    collectionCard().click(); await settle();
    [...document.querySelectorAll("tbody tr")].find((row) => row.textContent?.includes("Existing Collection Member"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Add to queue", "Existing Collection Member");
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    const queueCount = document.querySelectorAll(".queue-list article").length;
    const previewCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;
    expect(queueCount).toBeGreaterThan(0);
    expect(previewCount).toBe(1);
    expect(play).toHaveBeenCalledTimes(1);
    [...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === "Delete playlist")!.click();
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
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "collections", collectionId: "collection-full" });
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
    [...document.querySelectorAll("tbody tr")].find((row) => row.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await chooseTrackAction("Add to queue", "Another Recording");
    document.querySelector<HTMLButtonElement>('[aria-label="Show playback queue and chapters"]')!.click(); await settle();
    const queueBeforeDelete = [...document.querySelectorAll(".queue-list article .track-copy strong")].map((node) => node.textContent?.trim());
    expect(queueBeforeDelete.length).toBeGreaterThan(0);
    const previewCount = mocks.call.mock.calls.filter(([command]) => command === "resolve_preview").length;

    rightClickPlaylist("Empty Collection"); await settle();
    expect(document.querySelector('dialog.playlist-menu[aria-label="Actions for playlist Empty Collection"]')).not.toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>("dialog.playlist-menu button")].find((button) => button.textContent?.trim() === "Delete playlist")!.click();
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
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
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
    expect(mocks.call).toHaveBeenCalledWith("add_to_collection", {
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
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
    expect(row.textContent).toContain("124≈");
    expect(row.querySelector(".track-title")?.getAttribute("title")).toContain("Estimated from audio");
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".details dd small")?.textContent).toContain("Estimated");
    expect(document.querySelector(".details dd small")?.getAttribute("title")).toContain("not verified catalog metadata");
    await chooseTrackAction("Download options…");
    expect(document.querySelector(".download-source-primary")?.textContent).toContain("source · needs review");
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
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
    const updatedRow = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
    expect(updatedRow.textContent).toContain("124≈");
    expect(updatedRow.textContent).toContain("Am");
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
    expect(document.querySelector("tbody")?.textContent).not.toContain("Event Import");
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
    expect(document.querySelector("tbody")?.textContent).toContain("Saved Recording");
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
    expect(mocks.call).toHaveBeenCalledWith("list_tracks", { view: "all", collectionId: null });
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
    document.querySelector<HTMLButtonElement>('[aria-label="Expand chapters for Full Set"]')!.click(); await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Play chapter Song A"]')!.click(); await settle();
    expect(mocks.call).toHaveBeenCalledWith("register_chapter_track", expect.objectContaining({ track: expect.objectContaining({ playbackRange: expect.objectContaining({ startTime: 20, endTime: 60 }) }) }));
    const seek = await automationAction("chapter-relative-seek", "seek", { position: 900 });
    expect(seek.error).toBeNull();
    expect(seek.playback).toMatchObject({ position: 40, duration: 40 });
    const chapterAudio = document.querySelector("audio")!;
    chapterAudio.dispatchEvent(new Event("play"));
    Object.defineProperty(chapterAudio, "currentTime", { configurable: true, value: 60 });
    chapterAudio.dispatchEvent(new Event("timeupdate")); await settle();
    expect(document.querySelector(".player-track-title")?.textContent).toBe("Song A");
    expect(document.querySelector(".player-status")?.textContent).toContain("Paused");
    expect(play).toHaveBeenCalled();
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
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    eventHandlers.get("automation-playback")?.({ event: "automation-playback", id: 11,
      payload: { requestId: "cancel-pending-play", action: "play", trackIds: [savedTrack.id] } });
    await settle();
    expect(finishOldSearch).toBeDefined();
    expect(mocks.call).toHaveBeenCalledWith("find_candidates", expect.objectContaining({ track: savedTrack }));

    eventHandlers.get("automation-playback-cancelled")?.({ event: "automation-playback-cancelled", id: 12,
      payload: { requestId: "cancel-pending-play" } });
    finishOldSearch!([youtubeCandidate(savedTrack)]);
    await settle();

    expect(mocks.call.mock.calls.some(([command]) => command === "resolve_candidate_audio" || command === "start_candidate_preview")).toBe(false);
    expect(play).not.toHaveBeenCalled();
    expect(mocks.call).toHaveBeenCalledWith("automation_playback_complete", expect.objectContaining({
      requestId: "cancel-pending-play", error: expect.any(String),
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
    expect(document.querySelectorAll(".history-row")).toHaveLength(1);
    expect(document.querySelector(".history-row")?.textContent).toContain("Saved Recording");
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

  it("tries YouTube thumbnails before showing the artwork placeholder", async () => {
    savedChartTracks = [{ ...savedTrack, artworkUrl: "https://img.example.test/cover.jpg" }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const image = document.querySelector(".details img.artwork") as HTMLImageElement;
    expect(image).not.toBeNull();
    image.dispatchEvent(new Event("error"));
    await settle();
    expect(document.querySelector<HTMLImageElement>(".details img.artwork")?.src).toContain("hqdefault.jpg");
    document.querySelector(".details img.artwork")!.dispatchEvent(new Event("error"));
    await settle();
    expect(document.querySelector<HTMLImageElement>(".details img.artwork")?.src).toContain("mqdefault.jpg");
    document.querySelector(".details img.artwork")!.dispatchEvent(new Event("error"));
    await settle();
    expect(document.querySelector(".details .artwork-empty")?.textContent).toBe("♫");
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
    expect(document.querySelectorAll("tbody tr:not(.spacer)")).toHaveLength(3);
    localSelect.value = "local";
    localSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect([...document.querySelectorAll("tbody tr")].map((row) => row.textContent).join(" ")).toContain("Local Recording");
    expect([...document.querySelectorAll("tbody tr")].map((row) => row.textContent).join(" ")).not.toContain("Not Downloaded");
    localSelect.value = "not-local";
    localSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(document.querySelectorAll("tbody tr:not(.spacer)")).toHaveLength(2);
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
    expect(mocks.call.mock.calls.some(([command]) => command === "add_to_collection")).toBe(true);
    expect(document.querySelector("h1")?.textContent).toBe("Library");
  });

  it("opens the playlist picker from one Track information action instead of showing inline playlist controls", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    await chooseTrackAction("Track information");
    const details = document.querySelector(".details")!;
    expect(details.querySelectorAll("button").length).toBeGreaterThanOrEqual(3);
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
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Download playlist")!.click();
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Find recordings by Visible Artist"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    const topTrack = [...document.querySelectorAll<HTMLElement>(".track-list li")].find((item) => item.textContent?.includes("Saved Recording"))!;
    topTrack.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 180 }));
    await settle();
    const menu = document.querySelector<HTMLDialogElement>("dialog.track-menu")!;
    [...menu.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Track information")!.click();
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
    expect(document.querySelector(".download-source-primary")?.textContent?.toLocaleLowerCase()).toContain("slskd");
    await confirmReviewedDownload();
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.source?.kind).toBe("soulseek");
    expect(mocks.call.mock.calls.some(([command]) => command === "inspect_candidate")).toBe(false);
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
    expect(document.querySelector(".download-source-primary")?.textContent?.toLocaleLowerCase()).toContain("slskd");
    await confirmReviewedDownload();
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
    expect(document.querySelector(".download-source-primary")?.textContent?.toLocaleLowerCase()).toContain("slskd");
    await confirmReviewedDownload();
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect((queued?.[1] as { request: { candidate: Candidate } }).request.candidate.source?.kind).toBe("soulseek");
  });

  it("saves independent fallback switches and source order without unrelated edits", async () => {
    configuredPreferredStreamingSource="monochrome";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".library-footer button")].find(item=>item.textContent?.includes("Settings"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    const source=document.querySelector('[aria-label="Streaming fallback 1"]') as HTMLSelectElement;
    source.value="soundcloud";source.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackSources).toEqual(["soundcloud","youtube"]);
    const off=document.querySelector('[aria-label="Enable streaming fallback"]') as HTMLInputElement;
    off.checked=false;off.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(streamingFallbackEnabled).toBe(false);expect(source.disabled).toBe(true);
    const downloads=document.querySelector('[aria-label="Enable download fallback"]') as HTMLInputElement;
    downloads.checked=true;downloads.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    expect(downloadFallbackEnabled).toBe(true);expect(streamingFallbackEnabled).toBe(false);
    expect((mocks.call.mock.calls.filter(([command])=>command==="save_settings").slice(-1)[0]![1] as {settings:Settings}).settings.streamingFallbackSources).toEqual(["soundcloud","youtube"]);
  });

  it("saves playback preferences without saving an unfinished music-folder edit", async () => {
    configuredMusicDirectory="/saved/music";
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll(".library-footer button")].find(item=>item.textContent?.includes("Settings"))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    const folder=document.querySelector('input[placeholder="/home/you/Music/DJ"]') as HTMLInputElement;
    folder.value="/unsaved/music";folder.dispatchEvent(new Event("input",{bubbles:true}));
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
    [...document.querySelectorAll(".library-footer button")].find((item) => item.textContent?.includes("Settings"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const folder = document.querySelector('input[placeholder="/home/you/Music/DJ"]') as HTMLInputElement;
    folder.value = "/unsaved/music";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
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
    document.querySelector("tbody tr")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
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

  it("restores the selected artist profile through back and forward without adding a route loop", async () => {
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"));
    row!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).not.toHaveBeenCalledWith("get_track_details", expect.anything());
    expect(mocks.call.mock.calls.filter(([command]) => command === "find_candidates").map(([, args]) => (args as { providerIds: string[] }).providerIds)).toEqual([["youtube"]]);
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio")).toHaveLength(1);
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
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Salaam"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_candidate_audio", { candidate });
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
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("resolve_candidate_audio", { candidate });
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it.each([false,true])("stream fallback enabled=%s respects its configured source list", async enabled => {
    previewMode = "fallback";
    configuredPreferredStreamingSource = "monochrome";
    streamingFallbackEnabled = enabled;
    streamingFallbackSources = ["soundcloud"];
    failedCandidateProviders.add("monochrome");
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll("tbody tr")].find(row=>row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    const requests=mocks.call.mock.calls.filter(([command])=>command==="find_candidates").map(([,args])=>(args?.providerIds as string[])[0]);
    expect(requests).toEqual(enabled ? ["monochrome","soundcloud"] : ["monochrome"]);
    if(enabled) expect(document.querySelector(".player-source")?.textContent).toContain("SoundCloud");
    else expect(mocks.call.mock.calls.some(([command])=>command==="resolve_candidate_audio")).toBe(false);
  });

  it.each([false,true])("midstream recovery enabled=%s excludes unconfigured sources", async enabled => {
    previewMode="fallback";configuredPreferredStreamingSource="monochrome";streamingFallbackEnabled=enabled;streamingFallbackSources=["soundcloud"];
    monochromeCandidates=[{...youtubeCandidate(savedTrack),id:"monochrome:174208180224667648",provider:"monochrome",url:"https://tracks.monochrome.st/track/174208180224667648",confidence:"high",score:0.97}];
    vi.spyOn(HTMLMediaElement.prototype,"play").mockResolvedValue(undefined);
    const instance=mount(App,{target:document.body});unmountApp=()=>{void unmount(instance);};await settle();
    [...document.querySelectorAll("tbody tr")].find(row=>row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click",{bubbles:true}));await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));await settle();
    const resolutions=mocks.call.mock.calls.filter(([command])=>command==="resolve_candidate_audio").map(([,args])=>(args?.candidate as Candidate).provider);
    expect(resolutions).toEqual(enabled?["monochrome","soundcloud"]:["monochrome"]);
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
    const instance = mount(App, { target: document.body }); unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find(row => row.textContent?.includes(savedTrack.title))!.dispatchEvent(new MouseEvent("click", { bubbles:true }));
    await settle();
    const resolutions = mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio");
    expect(resolutions).toHaveLength(1);
    expect((resolutions[0]![1] as {candidate:Candidate}).candidate.provider).toBe(provider);
    expect(document.querySelector(".player-source")?.textContent).toContain(provider === "monochrome" ? "Monochrome" : "YouTube");
    expect(mocks.call.mock.calls.some(([command,args]) => command === "find_candidates" && (args?.providerIds as string[])?.includes("monochrome"))).toBe(override);
  });

  it("shows the exact source resolver failure without silently substituting another recording", async () => {
    previewMode = "fallback";
    candidateAudioFailure = "The uploader disabled this video’s audio stream.";
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Salaam (Francis Mercier Remix) - #afrohouse", confidence: "low", score: 0.1,
    };
    savedChartTracks = [{ ...savedTrack, title: "Salaam", provenance: { sourceKind: "audioSearch", audioCandidate: candidate } }];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Salaam"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector(".player-status")?.textContent).toContain("Unavailable");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("uploader disabled this video’s audio stream");
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio")).toHaveLength(1);
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
  });

  it("does not let a late explicit-source resolution replace a newer preview", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const candidate: Candidate = {
      ...youtubeCandidate(savedTrack), id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
      title: "Exact searched source", confidence: "low", score: 0.1,
    };
    const explicitTrack = { ...savedTrack, title: "Explicit source", provenance: { sourceKind: "audioSearch", audioCandidate: candidate } };
    savedChartTracks = [explicitTrack, secondTrack];
    delayAudioCandidateId = candidate.id;
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll("tbody tr")];
    rows.find((item) => item.textContent?.includes("Explicit source"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    rows.find((item) => item.textContent?.includes("Another Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    finishAudioResolve?.({ kind: "audio", url: "/late-explicit.mp3", canSeek: true, canVolume: true, canAutoplay: true, isExcerpt: false });
    await settle();
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
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
    expect(mocks.call.mock.calls.some(([command]) => command === "resolve_candidate_audio")).toBe(true);

    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(document.querySelector('[role="dialog"]')).toBeNull(); // Native dialog semantics
    expect(document.querySelector("dialog.download-dialog")?.textContent).toContain("Download recording");
    expect(document.querySelector(".download-source-primary")?.textContent).toContain("source · needs review");
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(false);
    const folder = document.querySelector('[aria-label="Download folder"]') as HTMLInputElement;
    folder.value = "/tmp/burrata-music";
    folder.dispatchEvent(new Event("input", { bubbles: true }));
    const tagTitle = document.querySelector('[aria-label="Download tag title"]') as HTMLInputElement;
    tagTitle.value = "Saved Recording (Club Mix)";
    tagTitle.dispatchEvent(new Event("input", { bubbles: true }));
    expect(document.querySelector(".download-review")).toBeNull();
    document.querySelector<HTMLButtonElement>(".download-dialog footer .primary")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("save_settings", expect.objectContaining({ settings: expect.objectContaining({ musicDirectory: "/tmp/burrata-music" }) }));
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { track: Track } }).request.track.title).toBe("Saved Recording (Club Mix)");
  });

  it("queues the best plausible match from the saved preferred source on row Download", async () => {
    configuredMusicDirectory = "/saved/music";
    configuredPreferredDownloadSource = "soundcloud";
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!;
    row.querySelector('[aria-label="Download Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(mocks.call).toHaveBeenCalledWith("find_candidates", { track: savedTrack, providerIds: ["soundcloud"] });
    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { candidate: Candidate } }).request.candidate.provider).toBe("soundcloud");
    expect((queued![1] as { request: { formatId: string | null } }).request.formatId).toBeNull();
    expect(document.querySelector("tbody")?.textContent).toContain("Queued · SoundCloud · best available quality");
    expect(document.body.textContent).toContain("The source match is unverified");
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
    expect(dialog.querySelector(".download-source-primary")?.textContent).toContain(candidate.title);
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
    const row = [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Peer Saved Recording"))!;
    row.querySelector('[aria-label="Download Peer Saved Recording"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    const queued = mocks.call.mock.calls.find(([command]) => command === "enqueue_download");
    expect(queued).toBeDefined();
    expect((queued![1] as { request: { candidate: Candidate } }).request.candidate).toEqual(peerCandidate);
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "inspect_candidate")).toBe(false);
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

    expect(document.querySelector(".download-source-primary")?.textContent?.toLocaleLowerCase()).toContain("soundcloud");
    await confirmReviewedDownload();
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
    expect(document.querySelector(".download-source-primary")).not.toBeNull();
    expect(document.querySelector(".download-review")).toBeNull();
    document.querySelector<HTMLButtonElement>(".download-dialog footer .primary")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(mocks.call.mock.calls.some(([command]) => command === "save_settings")).toBe(false);
    expect(mocks.call.mock.calls.some(([command]) => command === "enqueue_download")).toBe(true);
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

  it("does not open a stale A result after the user selects B", async () => {
    previewMode = "race";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    savedChartTracks = [savedTrack, secondTrack];
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    const rows = [...document.querySelectorAll("tbody tr")];
    rows.find((item) => item.textContent?.includes("Saved Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    rows.find((item) => item.textContent?.includes("Another Recording"))!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    finishOldSearch?.([youtubeCandidate(savedTrack)]);
    await settle();
    const starts = mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio");
    expect(starts).toHaveLength(1);
    expect((starts[0][1] as { candidate: Candidate }).candidate.id).toBe("youtube:recording-2");
    expect(document.querySelector(".now-playing")?.textContent).toContain("Another Recording");
  });

  it("plays a cached direct preview before searching provider candidates", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
    expect(mocks.call.mock.calls.some(([command]) => command === "find_candidates")).toBe(false);
    expect(document.querySelector(".now-playing")?.textContent).toContain("Playing preview");
  });

  it("tries another plausible audio result when the first source fails", async () => {
    previewMode = "alternates";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    const started = mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio");
    expect(started).toHaveLength(2);
    expect((started[1][1] as { candidate: Candidate }).candidate.id).toBe(`youtube-alt:${savedTrack.id}`);
    expect(mocks.call.mock.calls.filter(([command]) => command === "start_candidate_preview")).toHaveLength(0);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
  });

  it("discovers an alternative when cached audio fails after playback starts", async () => {
    previewMode = "direct";
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    const instance = mount(App, { target: document.body });
    unmountApp = () => { void unmount(instance); };
    await settle();
    [...document.querySelectorAll("tbody tr")].find((item) => item.textContent?.includes("Saved Recording"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    document.querySelector("audio")!.dispatchEvent(new Event("error"));
    await settle();
    expect(mocks.call.mock.calls.filter(([command]) => command === "resolve_candidate_audio")).toHaveLength(1);
    expect(document.querySelector(".player-status")?.textContent).toContain("Playing");
    expect(document.querySelector(".player-status")?.getAttribute("title")).toContain("youtube audio");
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });

});
