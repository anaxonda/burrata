<script lang="ts">
  import { dismissDetails } from "./lib/dismiss-details";

  import { hasExplicitSourceLock } from "./lib/source-lock";
  const loadLibraryBrowser = () => import("./LibraryBrowser.svelte").then(module => module.default);
  import { initialLibraryBrowserState, releaseLibraryEntity, saveCompleteLibraryAlbum, completeLibraryAlbumTrackCount, type LibraryBrowserState, type LibraryEntity } from "./lib/library";
  let libraryBrowser = $state<LibraryBrowserState>(initialLibraryBrowserState());
  function navigateLibraryBrowser(next: LibraryBrowserState) { rememberRoute(); libraryBrowser = next; }
  async function saveTracksToLibrary(items: Track[]) { await call("save_library_tracks", { tracks: items }); requestLocalPrefixRefresh(undefined, true); }
  async function saveReleaseToLibrary(items: Track[], release: ArtistReleaseRef) {
    await saveCompleteLibraryAlbum(call, releaseLibraryEntity(release, items[0]?.albumArtist ?? items[0]?.artist ?? ""), items, completeLibraryAlbumTrackCount(items, release.trackCount));
    requestLocalPrefixRefresh(undefined, true);
  }
  async function browseLibraryEntity(entry: LibraryEntity) {
    if (entry.provider === "local") {
      const canonical = entry.id.match(/^(?:album|artist):ref:(spotify|musicbrainz|discogs|monochrome):(.+)$/);
      if (canonical) entry = { ...entry, provider: canonical[1], providerId: canonical[2] };
    }
    if (entry.provider === "spotify" && entry.providerId) {
      if (entry.kind === "album") await openSpotifyAlbum(entry.providerId, entry.name);
      else await openSpotifyTrackArtist({ provider: "spotify", id: entry.providerId, name: entry.name });
    } else if (entry.kind === "artist" && entry.providerId && ["musicbrainz", "discogs", "monochrome"].includes(entry.provider ?? "")) {
      await discoverArtistRef({ provider: entry.provider as ArtistRef["provider"], id: entry.providerId });
    } else if (entry.kind === "album" && entry.providerId && ["musicbrainz", "discogs", "monochrome"].includes(entry.provider ?? "")) {
      const owner = navigationRequest;
      const page = await call<{ tracks: Track[] }>("list_library_entity_tracks", { id: entry.id, limit: 1, offset: 0 });
      if (owner !== navigationRequest || libraryBrowser.entity?.id !== entry.id) return;
      const target = page.tracks[0] ? trackAlbumTarget(page.tracks[0]) : null;
      if (target?.kind === "catalog") await openSearchAlbum({ ...target.release, trackCount: entry.totalTracks }, target.artist);
      else {
        const sourceUrl = entry.sourceUrl || (entry.provider === "musicbrainz" ? `https://musicbrainz.org/release/${entry.providerId}` : entry.provider === "discogs" ? `https://www.discogs.com/release/${entry.providerId}` : `https://tracks.monochrome.st/releases/${entry.providerId}`);
        await call("open_external_url", { url: sourceUrl });
      }
    } else await browseArtistProvider(entry.kind === "artist" ? entry.name : entry.artist, "musicbrainz");
  }
  import LibraryCleanup from "./LibraryCleanup.svelte";
  import type { BackupInspection, BackupMediaEstimate, BackupMediaRootPreview, BackupProgress, BackupSchedule, SettingsImportPreview, SettingsImportSelection } from "./BackupRestore.svelte";
  import type { ImportChoice } from "./ImportHub.svelte";
  import type { DirectoryBrowserLocation, DirectoryBrowserSnapshot } from "./DirectoryBrowser.svelte";
  import type { SearchKind, SearchRow } from "./SearchPanel.svelte";
  import LazyView from "./LazyView.svelte";
  import {
    applyBackupFrontendPreferences,
    type BackupFrontendPreferenceAck,
    type PendingBackupFrontendPreferences,
  } from "./lib/backup-frontend-preferences";
  import { respondToBackupCheckpoint, type BackupCheckpointRequest } from "./lib/backup-checkpoint";
  import { startRestoreAndWaitForCompletion } from "./lib/restore-job";
  let showLibraryCleanup = $state(false);

  import {dispatchQueueUi,queueUiMoves,type QueueUiAction,type QueueUiResult} from './lib/queue-ui-automation';

  import { FIXED_RESOLVER_FAILURES } from "./lib/playback-failure";
  import { allTrackSortFieldsAvailable, trackSortAvailability, trackSortOptionLabel, type TrackSortAvailability } from "./lib/track-sort-availability";
  import { onMount, onDestroy, tick, setContext, untrack } from "svelte";
  import { createQueuedPlaybackOutbox, type QueuedPlayRequest } from "./lib/queued-playback";
  import { createAutomationPlaybackPublisher, queueProjectionRevision, type PlaybackStateAck } from "./lib/automation-playback-publisher";

  import { measurePlaybackLayout } from "./lib/playback-layout";
  import { inspectArtworkImages } from "./lib/artwork-diagnostics";
  import { ArtworkDisplayCache } from "./lib/artwork-display";
  import { metadataCount } from "./lib/provider-counts";
  import { savedLocalPlaylistContext } from "./lib/download-playlist-context";
  import type { LyricsTranslationResult, LyricsSourceLine } from "./lib/lyrics-translation";
  import { translateLyricsInChunks, type TranslationProgress } from "./lib/lyrics-translation-progress";
  import { visibleLyricsTrack as makeVisibleLyricsTrack, lyricsTrackWithPlaybackDuration } from "./lib/lyrics-request";
  import { playbackOperationTrace, type PlaybackOperationTrace } from "./lib/native-playback";
import { isCurrentLiveBpmEstimate, isCurrentLiveKeyEstimate } from "./lib/live-bpm";
import { readLiveMeasurementCache, saveLiveMeasurements, type LiveMeasurementCacheRecord, type LiveMeasurementCacheApi, type LiveMeasurementResource } from "./lib/live-measurement-cache";
import { PLAYING_TRACK_DETECTION_INTERVAL_MS, activeDetectedTrack, playingTrackDetectionOwnerKey, samePlayingTrackDetectionOwner, type PlayingTrackDetectionOwner, type PlayingTrackDetectionView } from "./lib/playing-track-detection";

  import type { WaveformSnapshot } from "./lib/waveform-data";
  import * as waveformData from "./lib/waveform-data";

  import { claimAutomaticStreamMediaAnalysis, fullStreamAnalysisOutcome, isCompleteStreamMediaWaveform, streamAudioVariantSha256, streamMediaResourceKey, streamMediaTrackContext, streamSourceIdentitySha256, verifiedStreamWaveform, type StreamMediaResource, type StreamAudioVariant } from "./lib/stream-media-analysis";
  import { isStreamMediaAnalysisPaused } from "./lib/stream-analysis-policy";
import { ObservedWaveformCache, prepareObservedWaveformBinding, type PreparedObservedWaveformBinding } from "./lib/observed-waveform-cache";
  import { QueueRadioController, queueRadioSeedKey } from "./lib/queue-radio";
  import { radioSongTrackFromSnapshot } from "./lib/radio-song-identity";
  import { sameTrackIdentity } from "./lib/track-identity";
  import { PendingSortAvailabilityRefresh, sortAvailabilityIsPending } from "./lib/pending-sort-availability";
  import { applyLocalTrackPage, requestLocalTrackPage, type LocalTrackPage } from "./lib/local-track-page";
  import { trackFileGroupsByTrackId } from "./lib/track-file-groups";
  import { historyOccurrenceDateRange } from "./lib/history-occurrence-range";
  import { SOURCE_OPENER, sourcePositionUrl, type SourceOpener } from "./lib/source-opening";
  import SourceLink from "./SourceLink.svelte";
  import SourcePresets from "./SourcePresets.svelte";
  import {playlistTitle} from "./lib/playlist-title";
  import { runPeriodicPlaylistSyncFallback } from "./lib/playlist-sync-fallback";
  import type { RadioStation, RadioTrackMetadata } from "./RadioView.svelte";
  import PlaybackQuality from "./PlaybackQuality.svelte";
  import LoudnessSettings from "./LoudnessSettings.svelte";
  import FlexibleMatchingSettings from "./FlexibleMatchingSettings.svelte";
  import CrossfadeSettings from "./CrossfadeSettings.svelte";
  import AlternativeSourceNotice from "./AlternativeSourceNotice.svelte";
  import WaveformSeek from "./WaveformSeek.svelte";
  import StreamMediaAnalysisSettings from "./StreamMediaAnalysisSettings.svelte";
  import NowPlayingView from "./NowPlayingView.svelte";
  import { createTrackArtistInformationLoader } from "./lib/track-information";
  import { createRecordingInformationLoader, type RecordingReleaseInfo } from "./lib/recording-information";
  import DownloadSourceChooser from "./DownloadSourceChooser.svelte";

  import BrowserConnections from "./BrowserConnections.svelte";

  import { normalizeSpotifyUserProfileUrl } from "./lib/spotify-profile-url";
  import PlaylistSync from "./PlaylistSync.svelte";
  import PortablePlaylist from "./PortablePlaylist.svelte";
  import PortableHistory from "./PortableHistory.svelte";
  import AccountPlaylistSettings from "./AccountPlaylistSettings.svelte";
  const loadBackupRestore = () => import("./BackupRestore.svelte").then(module => module.default);
  const loadImportHub = () => import("./ImportHub.svelte").then(module => module.default);
  const loadImportSources = () => import("./ImportSources.svelte").then(module => module.default);
  const loadRestoreWorkReview = () => import("./RestoreWorkReview.svelte").then(module => module.default);
  const loadPlaybackDiagnosticsSettings = () => import("./PlaybackDiagnosticsSettings.svelte").then(module => module.default);
  const loadDirectoryBrowser = () => import("./DirectoryBrowser.svelte").then(module => module.default);
  const loadSearchPanel = () => import("./SearchPanel.svelte").then(module => module.default);
  const loadArtistPage = () => import("./ArtistPage.svelte").then(module => module.default);
  const loadRadioView = () => import("./RadioView.svelte").then(module => module.default);
  const loadRemotePlaylists = () => import("./RemotePlaylists.svelte").then(module => module.default);
  const loadRemotePlaylistDetail = () => import("./RemotePlaylistDetail.svelte").then(module => module.default);
  import type {PlaylistSection} from "./lib/playlist-sections";
  const DOWNLOADS_PLAYLIST_ID="00000000-0000-4000-8000-000000000001";
  let sidebarLayout=$state<{sections:PlaylistSection[];assignments:Record<string,string>;names?:Record<string,string>;showAccountPlaylists?:{spotify?:boolean;youtube?:boolean}}>({sections:[],assignments:{}});
  let radioStations=$state<RadioStation[]>([]);
  let selectedRadioStationId=$state("main");
  let showAddRadio=$state(false);
  let remoteOwner=$state("");let remotePlaylistUrl=$state("");
  let renamePlaylistName=$state(""),renamingPlaylist=$state(false),importTargetCollectionId=$state<string|null>(null);
  let selectionAnchorId=$state<string|null>(null);

  import TrackMenu from "./TrackMenu.svelte";
  import PlaylistMenu from "./PlaylistMenu.svelte";
  import PlaylistMirrorSettings from "./PlaylistMirrorSettings.svelte";
  let radioActions=$state<{station:RadioStation;anchor:{x:number;y:number}}|null>(null);
  let showReleaseSetup=$state(false);
  let renameTarget=$state<{kind:"playlist"|"radio";id:string;name:string}|null>(null);
  let sidebarRenameValue=$state("");
  let playlistActions = $state<{ collection: Collection; anchor: { x: number; y: number } } | null>(null);
  let playlistMirrorSettingsTarget = $state<{ collectionId: string | null; name?: string } | null>(null);
  let portablePlaylistTarget = $state<Collection | null>(null);
  let portablePlaylistInitialMode = $state<"export" | "folder">("export");
  let showPortableHistory = $state(false);
  let seekSerial = $state(0);
  let downloadSubmitting = $state(false);
  let downloadFiles = $state<LocalAsset[]>([]);
  let downloadAssetsByJob = $state<Record<string, LocalAsset>>({});
  let autotagReviewAsset = $state<LocalAsset | null>(null);
  let detailAssets = $state<LocalAsset[]>([]);
  let pageFilesByTrack = $state<Record<string, LocalAsset[]>>({});
  let detailFilesByTrack = $state<Record<string, LocalAsset[]>>({});
  let scopedAssetCommandsSupported = true;
  function isUnknownAssetCommand(failure: unknown) { return /(?:unknown|not found|does not exist).{0,80}(?:command|list_track_files_for_recordings|list_assets_for_paths|get_analysis_status_for_assets|count_assets|list_assets_by_ids)|(?:command|list_track_files_for_recordings|list_assets_for_paths|get_analysis_status_for_assets|count_assets|list_assets_by_ids).{0,80}(?:unknown|not found|does not exist)/i.test(String(failure)); }
  let downloadDisposition = $state<{mode:"keep"}|{mode:"replace";targetAssetId:string;expectedHash:string}>({mode:"keep"});
  let downloadPlaylistContext = $state<PlaylistContext | null>(null);
  function trackAsset(track: Track | null): LocalAsset | null {
    if (!track) return null;
    const activeAssetId=track.id===playingTrack?.id ? nativeSnapshot?.quality?.assetId : null;
    return scopedAssetsById.get(activeAssetId || track.assetId || "")
      ?? (track.localPath ? scopedAssets.find(asset => asset.path === track.localPath) : undefined)
      ?? filesForTrack(track)[0] ?? null;
  }
  function filesForTrack(track: Track) { return pageFilesByTrack[track.id] ?? detailFilesByTrack[track.id] ?? scopedAssetsByRecording.get(track.id) ?? []; }
  async function preferTrackFile(track:Track,assetId:string) {
    await run(async()=>{
      await call("set_preferred_track_file",{trackId:track.id,assetId});
      invalidateLegacyArtistAssets();
      const files=await call<LocalAsset[]>("list_track_files",{trackId:track.id});
      const canonicalId=files[0]?.recordingId ?? pageFilesByTrack[track.id]?.[0]?.recordingId ?? detailFilesByTrack[track.id]?.[0]?.recordingId ?? track.id;
      const refreshGroups=(groups:Record<string,LocalAsset[]>)=>{
        const next={...groups};
        for(const [id,items] of Object.entries(next)) if(id===track.id||items.some(item=>item.recordingId===canonicalId||sameTrack(item.track,track))) next[id]=files;
        next[track.id]=files;
        return next;
      };
      pageFilesByTrack=refreshGroups(pageFilesByTrack);
      detailFilesByTrack=refreshGroups(detailFilesByTrack);
      assets=[...assets.filter(item=>item.recordingId!==canonicalId),...files];
      const updated=await call<Track>("get_track_details",{track});tracks=tracks.map(item=>item.id===track.id?{...item,...updated}:item);
      if(downloadTrack?.id===track.id){downloadTrack={...downloadTrack,...updated};downloadFiles=files;}
    });
  }
  async function remotePlaylistsImported(){ await refreshRemotePlaylistBindings(); collections=await call<Collection[]>("list_collections");browserImports=await call<BrowserImportResult[]>("list_browser_imports");await syncMirrors(); }

  let systemPausedTrack = $state(false);
  let actionTrack = $state<Track | null>(null);
  let actionAnchor = $state<{ x: number; y: number } | null>(null);
  let deletedPlaylist = $state<Collection | null>(null);
  let deletedCollections = $state<Collection[] | null>(null);
  let showTrackDetails = $state(false);
  let showCreatePlaylist = $state(false);
  import ReleaseView from "./ReleaseView.svelte";
  const loadLocalAlbumLoudnessReview = () => import("./LocalAlbumLoudnessReview.svelte").then(module => module.default);
  import {providerNavigationSnapshots} from "./lib/provider-navigation-snapshots";
  import { sortTracks, filterTracks, parseDurationBound, playableDuration, type TrackSortField, type SortDirection } from "./lib/track-sort";
  import SpotifyArtist from "./SpotifyArtist.svelte";
  import SoulseekWishlist from "./SoulseekWishlist.svelte";
  import SoulseekDownloadPolicy from "./SoulseekDownloadPolicy.svelte";
  import DownloadWatches from "./DownloadWatches.svelte";
  let activeDownloadWatchCount = $state(0);
  let showSavedSoulseekSearches = $state(false);
  import InfoPopover from "./InfoPopover.svelte";
  import LyricsPanel from "./LyricsPanel.svelte";
  import { migrateNativePlayback } from "./lib/playback-engine-migration";
  import { createNativePositionBookmark, nativeBookmarkMatchesOccurrence, shouldPersistNativePositionBookmark, validNativePositionBookmark, type NativePositionBookmark, type NativePositionOwner } from "./lib/native-position-bookmark";
  import TrackDownloadFilter from "./TrackDownloadFilter.svelte";
  import { isTrackDownloaded } from "./lib/track-sort";
  let historyDownloadAvailability = $state<"all"|"downloaded"|"not-downloaded">("all");
  import TrackDurationFilter from "./TrackDurationFilter.svelte";
  import type { AutomationSearchRequest, AutomationSearchReport } from "./lib/search-ui-automation";
  import { searchUiControlExpired } from "./lib/search-ui-control-revision";
  import AutotagSetup from "./AutotagSetup.svelte";
  import AutotagReview, { type AutotagJob } from "./AutotagReview.svelte";
  import { virtualChapterTrack, chapterPlaybackRange, chapterForRange, chapterAbsolutePosition, chapterRelativePosition } from "./lib/chapters";
  import ReleaseSetup from "./ReleaseSetup.svelte";
  import SettingsNavigation from "./SettingsNavigation.svelte";
  import PlaylistSettings from "./PlaylistSettings.svelte";
  import HistoryMirrorSettings from "./HistoryMirrorSettings.svelte";
  import RestoredPlaylistSyncSettings from "./RestoredPlaylistSyncSettings.svelte";
  import TrackList from "./TrackList.svelte";
  import ToolSetup from "./ToolSetup.svelte";
  import ConnectionsSetup from "./ConnectionsSetup.svelte";
  import LocalPlaylistFiles from "./LocalPlaylistFiles.svelte";
  import MetadataReview from "./MetadataReview.svelte";

  import SeekSlider from "./SeekSlider.svelte";
  import FallbackSettings from "./FallbackSettings.svelte";
  import { downloadProviderOrder } from "./lib/source-fallback";
  import PlayingInfo from "./PlayingInfo.svelte";
  import StationInfo from "./StationInfo.svelte";
  import TrackInfoSearch from "./TrackInfoSearch.svelte";
  import PlaybackQueuePanel from "./PlaybackQueuePanel.svelte";
  import DownloadActivity from "./DownloadActivity.svelte";
  import VirtualResults from "./VirtualResults.svelte";
  import { downloadStateLabel, downloadSourceLabel, downloadPeer, downloadTitle, downloadTransferDetail } from "./lib/download-display";
  import LibrarySidebar from "./LibrarySidebar.svelte";
  import { PlaybackQueue, savePlaybackQueue, loadPlaybackQueue } from "./lib/playback-queue";
  import { applyPendingBackupPlaybackRestore, type BackupPlaybackRestoreAck, type PendingBackupPlaybackRestore } from "./lib/backup-playback-hydration";
  import { consumeStartupPlaybackControls, persistPlaybackControlsWhenOwned } from "./lib/playback-controls";
  import { lyricsTiming } from "./lib/lyrics-timing.svelte";
  import { startAfterBackupHydration } from "./lib/backup-startup-gate";
  import { artistPlaybackTracks } from "./lib/artist-playback";
  import type { ArtistRef, ArtistSearchPage, ArtistProfile, ArtistReleasePage, ArtistReleaseRef, ArtistTrackPage, ArtistCollaborationPage, ArtistLocalAssetsPage, ArtistLocalAssetsPageRequest, AlbumSearchPage, MetadataReview as MetadataReviewData, MetadataFieldKey } from "./lib/artist-types";

  import { isExplicitSoulseekFolderSource, isExplicitUnresolvedAudioSearchSource, resolveBatchSource, type BatchMatchAssessment } from "./lib/batch";
  import { invertSelection } from "./lib/selection";
  import { LatestRequest } from "./lib/latest-request";
  import { choosePreviewCandidate, isLinkedAudioSearchCandidate } from "./lib/preview-source";
  import { playlistPosition } from "./lib/source-position";
  import { rowWindow } from "./lib/table";

  // A WebKit write may pass immediate readback and later roll back an existing
  // record. These values are used only for this startup's initial reads; all
  // receipts, persistence, and subsequent reads still use real localStorage.
  const backupStartupStorageOverrides = new Map<string, string>();
  const backupStartupReadStorage = {
    getItem(key: string) {
      if (!backupStartupStorageOverrides.has(key)) return localStorage.getItem(key);
      const value = backupStartupStorageOverrides.get(key)!;
      backupStartupStorageOverrides.delete(key);
      return value;
    },
    removeItem(key: string) { localStorage.removeItem(key); },
  };
  const backupStartupPersistence = {
    getItem: (key: string) => backupStartupReadStorage.getItem(key),
    setItem: (key: string, value: string) => localStorage.setItem(key, value),
    removeItem: (key: string) => localStorage.removeItem(key),
  };
  import { call, desktop, playable, listen } from "./lib/api";
import { NativePlaybackBridge, nativeSnapshotFailure, type NativePositionEvent } from "./lib/native-playback";
import { type LoudnessAnalysisJobStatus, type LoudnessPlaybackStatus, type LoudnessResourceIdentity } from "./lib/loudness-normalization";

  import type {
    Track, Chapter,
    Genre,
    Chart,
    Preview,
    Candidate,
    Collection,
    DownloadJob,
    Settings, PlaylistContext,
    Dependency,
    LocalAsset,
    ChartMembership,
    BrowserStatus, BrowserImportResult, CollectionImportPreview, CandidatePreviewStatus, NativePlaybackSnapshot, NativePlaybackFailure, PlaybackTrackFailure, LiveBpmEstimate, LiveKeyEstimate, LivePlaybackEstimates,
  } from "./lib/types";
  import type { TrackLibrarySummary } from "./lib/track-library-summary";
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
    | "history"
    | "directory-browser";
  let view = $state<View>("library"),
    provider = $state("traxsource"),
    genres = $state<Genre[]>([]),
    genreUrl = $state(""),
    chart = $state<Chart | null>(null),
    snapshots = $state<Chart[]>([]);
  let nativeArtistComponent = $state<{captureNavigationSnapshot:(key:string)=>string;restoreNavigationSnapshot:(key:string)=>Promise<void>}|undefined>();
  type NativeArtistPageInstance = NonNullable<typeof nativeArtistComponent>;
  const pendingNativeArtistResolvers = new Set<(instance: NativeArtistPageInstance | null) => void>();
  function captureNativeArtistInstance(instance: unknown) {
    nativeArtistComponent = instance as NativeArtistPageInstance;
    for (const resolve of pendingNativeArtistResolvers) resolve(nativeArtistComponent);
    pendingNativeArtistResolvers.clear();
  }
  function clearNativeArtistInstance() {
    nativeArtistComponent = undefined;
    for (const resolve of pendingNativeArtistResolvers) resolve(null);
    pendingNativeArtistResolvers.clear();
  }
  function waitForNativeArtistInstance(): Promise<NativeArtistPageInstance | null> {
    if (nativeArtistComponent) return Promise.resolve(nativeArtistComponent);
    return new Promise(resolve => pendingNativeArtistResolvers.add(resolve));
  }

  let spotifyArtistComponent = $state<{captureNavigationSnapshot:(key:string)=>string}|undefined>();
  let spotifyNavigationSnapshotKey = $state<string|null>(null);
  let searchComponent = $state<{
    captureNavigationSnapshot:(key:string)=>string;
    restoreNavigationSnapshot:(key:string)=>Promise<void>;
    inspectAutomationSearchUi:(offset?:number,limit?:number,expiresAtMs?:number)=>unknown;
    setAutomationContinueSearch:(revision:string,enabled:boolean,expiresAtMs:number)=>Promise<unknown>;
    playAutomationSearchResult:(revision:string,occurrenceToken:string,expiresAtMs:number)=>unknown;
  }|undefined>();
  type SearchPanelInstance = NonNullable<typeof searchComponent>;
  const pendingSearchPanelResolvers = new Set<(instance: SearchPanelInstance | null) => void>();
  function captureSearchPanelInstance(instance: unknown) {
    searchComponent = instance as SearchPanelInstance;
    for (const resolve of pendingSearchPanelResolvers) resolve(searchComponent);
    pendingSearchPanelResolvers.clear();
  }
  function clearSearchPanelInstance() {
    searchComponent = undefined;
    for (const resolve of pendingSearchPanelResolvers) resolve(null);
    pendingSearchPanelResolvers.clear();
  }
  function waitForSearchPanelInstance(): Promise<SearchPanelInstance | null> {
    if (searchComponent) return Promise.resolve(searchComponent);
    return new Promise(resolve => pendingSearchPanelResolvers.add(resolve));
  }

  let searchNavigationSnapshotKey = $state<string|null>(null);
  let navigationSnapshotSequence = 0;
  let directoryBrowserVisited = $state(false);
  let directoryBrowser = $state<{getSnapshot:()=>DirectoryBrowserSnapshot;restoreSnapshot:(snapshot:DirectoryBrowserSnapshot)=>Promise<void>}|undefined>();
  let directoryLocation = $state<{ rootId: string; relativePath: string } | null>(null);
  let directoryRootsRevision = $state(0);
  function browseFolder(location: { rootId: string; relativePath: string } | null) {
    const changed = directoryLocation?.rootId !== location?.rootId || directoryLocation?.relativePath !== location?.relativePath;
    if (view !== "directory-browser" || changed) rememberRoute();
    directoryLocation = location;
    directoryBrowserVisited = true;
    if (view !== "directory-browser") void navigate("directory-browser", false);
  }
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
  type NoticeExpiry = "transient" | "persistent";
  type NoticeOwner = "playbackSkip" | null;
  let noticeState = $state<{message: string; expiry: NoticeExpiry; owner: NoticeOwner}>({message: "", expiry: "transient", owner: null});
  const notice = $derived(noticeState.message);
  function setNotice(message: string, expiry: NoticeExpiry = "transient", owner: NoticeOwner = null) {
    noticeState = {message, expiry, owner};
  }
  function clearPlaybackSkipNotice() {
    if (noticeState.owner === "playbackSkip") setNotice("");
  }
  let busy = $state(0),
    error = $state(""),
    collections = $state<Collection[]>([]),
    collectionId = $state(""),
    newCollection = $state(""),
    renamedCollection = $state(""),
    jobs = $state<DownloadJob[]>([]),
    assets = $state<LocalAsset[]>([]),
    assetTotal = $state(0),
    dependencies = $state<Dependency[]>([]);
  let batchSelectionMode = $state(false);
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
    type WholeTrackAnalysisDto = {
    schemaVersion: number; analyzerVersion: string;
    resource:
      | { kind: "registeredAsset"; recordingId: string; assetId: string; fileHash: string }
      | { kind: "stream"; recordingId: string; provider: string; sourceIdentitySha256: string; audioVariantSha256?: string | null };
    durationSeconds: number; scope: "wholeTrack"; coverage: number; measuredAt: string;
    bpm?: { bpm: number; confidence: number; relatedTempoAlternativeBpm?: number | null } | null;
    key?: { key: string; confidence: number; wholeTrackKey: string; windowKeys: Array<string | null>; agreementCount: number } | null;
    waveform?: { analyzerVersion: string; binCount: number; durationSeconds: number; bins: Array<{ min: number; max: number } | null>; scope: "wholeTrack"; complete: boolean; measuredAt: string } | null;
  };
  type SelectedAnalysisResult = { assetId: string; recordingId: string; status: string; analysis: WholeTrackAnalysisDto | null; error?: string | null; kind?: "asset" | "stream"; sourceProvider?: string | null; sourceId?: string | null };
  type StreamAnalysisUpdatedEvent = { generation: number; trackId: string; resourceIdentity: string; sourceIdentitySha256: string; audioVariantSha256?: string | null; provider: string; sourceId: string; state: "started" | "completed" | "cancelled"; analysis?: WholeTrackAnalysisDto | null };
  type SelectedAnalysisJob = { jobId: string; state: string; total: number; completed: number; results: SelectedAnalysisResult[] };
  let selectedAnalysisJob = $state<SelectedAnalysisJob | null>(null);
  let selectedAnalysisKind = $state<"asset" | "stream">("asset");
  let selectedAnalysisOpen = $state(false);
  let selectedAnalysisBusy = $state(false);
  let selectedAnalysisRequest = 0;
  let selectedAnalysisTracks = $state<Record<string, Track>>({});
  let selectedAnalysisApplied = $state<string[]>([]);
  let applyAnalysisBpm = $state(true);
  let applyAnalysisKey = $state(true);
  let replaceAnalysisBpm = $state(false);
  let replaceAnalysisKey = $state(false);
  let loudnessAnalysisJob = $state<LoudnessAnalysisJobStatus | null>(null);
  let loudnessAnalysisBusy = $state(false);
  let loudnessAnalysisMessage = $state("");
  let localAlbumLoudnessReview = $state<{ album: string; albumArtist: string; tracks: Track[]; canReorder: boolean } | null>(null);
  let localAlbumLoudnessFocusReturn = $state<HTMLElement | null>(null);
  let pendingLoudnessAssetIds = $state<string[]>([]);
  let loudnessRetryRequired = $state(false);
  let activeLoudnessAssetIds: string[] = [];
  let activeLoudnessJobRequest = 0;
  let activeLoudnessJobId = "";
  let activeLoudnessJobIsQueued = false;
  let loudnessLaunchScheduled = false;
  let batchCancelled = false;
  let activeBatchReadId: string | null = null;
  const scopedAssets = $derived.by(() => {
    const byId = new Map<string, LocalAsset>();
    for (const asset of [...assets, ...detailAssets]) byId.set(asset.id, asset);
    return [...byId.values()];
  });
  const scopedAssetsById = $derived.by(() => new Map(scopedAssets.map(asset => [asset.id, asset])));
  const scopedAssetsByRecording = $derived.by(() => {
    const byRecording = new Map<string, LocalAsset[]>();
    for (const asset of scopedAssets) for (const id of new Set([asset.recordingId, asset.track.id])) {
      const files = byRecording.get(id) ?? [];
      files.push(asset);
      byRecording.set(id, files);
    }
    return byRecording;
  });
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
  let importChoice = $state<ImportChoice | null>(null);
  let droppedBackupPreview = $state<BackupInspection | null>(null);
  let nativeBackupDropRequest = 0;
  type NativeImportFileDrop = { token: string; kind: "backup" | "playlist" | "unsupported"; coordinateSpace: "logical" | "physical"; position: { x: number; y: number } };
  type ConsumedImportDrop = { kind: "playlist"; name: string; text: string } | { kind: "backup"; preview: BackupInspection };
  type RemotePlaylistBinding = { bindingId: string; provider: string; canEdit: boolean; collectionId: string; sourceCollectionId?: string | null; url?: string; ownerId?: string | null; ownerName?: string | null; ownerUrl?: string | null };
  let remotePlaylistBindings = $state<RemotePlaylistBinding[]>([]);
  let remotePlaylistWriteSyncEnabled = $state<Record<"spotify" | "youtube", boolean>>({ spotify: false, youtube: false });
  async function refreshRemotePlaylistBindings() {
    try { const value = await call<RemotePlaylistBinding[]>("list_remote_playlist_bindings"); if (Array.isArray(value)) remotePlaylistBindings = value; } catch { /* Imported playlists remain available while connection metadata is unavailable. */ }
  }
  let browserUrl = $state(""), browserMessage = $state(""), browserRequest = $state(""),
    browserStatus = $state<BrowserStatus | null>(null), browserImports = $state<BrowserImportResult[]>([]),
    activeBrowserImport = $state<BrowserImportResult | null>(null),
    collectionImportPreview = $state<CollectionImportPreview | null>(null),
    urlImportDestinationId = $state(""), urlImportNewPlaylistName = $state(""), urlImportFolderPath = $state(""), urlImportSectionName = $state(""), urlImportDuplicatePolicy = $state<"keep" | "skip">("keep"),
    activeArtistTrack = $state<Track | null>(null),
    activeArtistName = $state(""),
    activeArtistRef = $state<ArtistRef | null>(null),
    playlistTrack = $state<Track | null>(null),
    playlistTracks = $state<Track[]>([]);
  let playlistDestinationQuery = $state("");
  let playlistAlbumName = $state<string | null>(null);
  let playlistDestinationId = $state("");
  let playlistWriteBusy = $state(false);
  let playlistPickerGeneration = $state(0);
  let waveformEnabled = $state(false);
  let waveformPreferenceReady = $state(false);
  let detectionMarkersEnabled = $state(false);
  let detectionMarkersPreferenceReady = $state(false);
  let streamMediaAnalysisEnabled = $state(false);
  let streamMediaAnalysisPreferenceReady = $state(false);
  let streamMediaAnalysisRestorePolicyReady = $state(false);
  let streamMediaAnalysisRestorePaused = $state(true);
  let streamMediaAnalysisRestoreReviewRequired = $state(false);
  let streamMediaAnalysisRestoreCategoryPaused = $state(false);
  let streamMediaAnalysisStatus = $state("");
  let streamMediaAnalysisResult = $state<WholeTrackAnalysisDto | null>(null);
  let streamMediaAnalysisResultKey = $state("");
  let streamMediaAnalysisJob = $state<{ jobId: string; resourceKey: string; state: string } | null>(null);
  let streamMediaAnalysisStartingKey = $state("");
  let manualStreamMediaAnalysisKey = $state("");
  let streamMediaAnalysisRequest = 0;
  let streamWaveformCacheLookupOwner = "";
  const observedWaveformCache = new ObservedWaveformCache();
  const observedWaveformDbWrites = new Map<string, unknown>();
  const observedWaveformDbVersions = new Map<string, object>();
  const observedWaveformDbRecordingIds = new Map<string, string>();
  let observedWaveformDbWriteTimer: ReturnType<typeof setTimeout> | null = null;
  let observedWaveformDbPersistenceClosed = false;
  function scheduleObservedWaveformDbWrite(): void {
    if (observedWaveformDbPersistenceClosed || observedWaveformDbWriteTimer !== null || observedWaveformDbWrites.size === 0) return;
    observedWaveformDbWriteTimer = window.setTimeout(() => {
      observedWaveformDbWriteTimer = null;
      void flushObservedWaveformDbWrites();
    }, 15_000);
  }
  async function flushObservedWaveformDbWrites(): Promise<void> {
    if (observedWaveformDbWriteTimer !== null) {
      window.clearTimeout(observedWaveformDbWriteTimer);
      observedWaveformDbWriteTimer = null;
    }
    const pending = [...observedWaveformDbWrites.entries()];
    const pendingRecordingIds = new Map(observedWaveformDbRecordingIds);
    observedWaveformDbWrites.clear();
    observedWaveformDbRecordingIds.clear();
    for (const [key, payload] of pending) {
      const recordingId = pendingRecordingIds.get(key) ?? null;
      try {
        await call("write_analysis_cache_record", { namespace: "observed-waveform-v1", key, payload, recordingId });
      } catch {
        if (!observedWaveformDbWrites.has(key)) {
          observedWaveformDbWrites.set(key, payload);
          if (typeof recordingId === "string") observedWaveformDbRecordingIds.set(key, recordingId);
        }
      }
    }
    scheduleObservedWaveformDbWrite();
  }
  function queueObservedWaveformDbWrite(binding: PreparedObservedWaveformBinding): void {
    if (!binding.persistenceKey) return;
    const version = observedWaveformCache.persistedVersion(binding);
    if (version === null || observedWaveformDbVersions.get(binding.persistenceKey) === version) return;
    const payload = observedWaveformCache.exportPersistedRecord(binding);
    if (!payload) return;
    observedWaveformDbVersions.delete(binding.persistenceKey);
    observedWaveformDbVersions.set(binding.persistenceKey, version);
    if (observedWaveformDbVersions.size > 64) observedWaveformDbVersions.delete(observedWaveformDbVersions.keys().next().value!);
    observedWaveformDbWrites.set(binding.persistenceKey, payload);
    observedWaveformDbRecordingIds.set(binding.persistenceKey, binding.trackId);
    scheduleObservedWaveformDbWrite();
  }
  let observedWaveformBinding = $state<PreparedObservedWaveformBinding | null>(null);
  let observedWaveformBindingOwner = "";
  let observedWaveformBindingSequence = 0;
  let observedWaveformSnapshotOwner = "";
  const attemptedStreamMediaAnalysis = new Set<string>();
  function updateStreamMediaAnalysisRestorePolicy(policy: { reviewRequired: boolean; categories: Array<{ kind: string; paused: boolean }> }): void {
    const category = policy.categories.find((item) => item.kind === "background_stream_media_analysis");
    const wasExplicitlyPaused = streamMediaAnalysisRestoreReviewRequired || streamMediaAnalysisRestoreCategoryPaused;
    streamMediaAnalysisRestoreReviewRequired = policy.reviewRequired;
    streamMediaAnalysisRestoreCategoryPaused = category?.paused === true;
    // Restore review can concern other kinds of automatic work. Honor the
    // stream-analysis category when present; only fail closed to the legacy
    // profile-wide flag if the category is absent from an older policy DTO.
    streamMediaAnalysisRestorePaused = isStreamMediaAnalysisPaused(policy);
    streamMediaAnalysisRestorePolicyReady = true;
    if (wasExplicitlyPaused && !streamMediaAnalysisRestorePaused) {
      const resource = currentStreamMediaResource();
      if (resource) attemptedStreamMediaAnalysis.delete(resource.key);
      streamMediaAnalysisStatus = "Background stream analysis resumed after restore review.";
    } else if (streamMediaAnalysisRestorePaused && streamMediaAnalysisEnabled) {
      streamMediaAnalysisStatus = !category && policy.reviewRequired
        ? "Background stream analysis is paused until restore review. You can start a one-time analysis manually."
        : "Background stream analysis is paused for this restored profile. You can start a one-time analysis manually.";
    }
  }
  async function refreshStreamMediaAnalysisRestorePolicy(): Promise<void> {
    try {
      const policy = await call<{ reviewRequired: boolean; categories: Array<{ kind: string; paused: boolean }> }>("get_restore_work_policy");
      updateStreamMediaAnalysisRestorePolicy(policy);
    } catch {
      streamMediaAnalysisRestorePolicyReady = true;
      streamMediaAnalysisRestorePaused = true;
      streamMediaAnalysisStatus = "Background analysis is paused until restore work policy can be checked.";
    }
  }
  let playbackWaveform = $state<WaveformSnapshot | null>(null);
  let playbackWaveformReason = $state("");

  let waveformRouteUpgradeRequest = 0;
  let waveformRouteUpgradeStatus = $state("");
  let verifiedLocalWaveformBinding = $state<{ trackId: string; assetId: string; fileHash: string; fullTrack: boolean } | null>(null);
  let currentLocalAnalysisResult = $state<WholeTrackAnalysisDto | null>(null);
  let currentLocalAnalysisStatus = $state("");
  let currentLocalAnalysisOwner = "";
  let currentLocalAnalysisRequest = 0;
  let currentLocalAnalysisJobId: string | null = null;
  const currentPlaybackAnalysisEstimate = $derived.by(() => {
    const local = currentLocalAnalysisResult;
    if (local?.resource.kind === "registeredAsset" && local.resource.assetId === waveformLocalAssetId &&
        local.resource.recordingId === playingTrack?.id) return local;
    const stream = currentStreamMediaResource();
    return stream && streamMediaAnalysisResultKey === stream.key ? streamMediaAnalysisResult : null;
  });
  let localWaveformCacheRebindRequest = 0;
  let localWaveformCacheRebindOwnerKey = "";
  let localWaveformAnalysisStatus = $state("");
  let localWaveformAnalysisJobId = $state<string | null>(null);
  let localWaveformAnalysisRequest = 0;
  let localWaveformAnalysisOwnerKey = $state("");
  let cachedStreamAnalysis = $state<WholeTrackAnalysisDto | null>(null);
  let cachedStreamAnalysisOwnerKey = $state("");
  let cachedStreamAnalysisLookupOwnerKey = "";
  let cachedStreamAnalysisEventsReady = $state(false);
  let cachedStreamAnalysisEventRevision = 0;
  let pendingCreatedPlaylist = $state<{ collection: Collection; generation: number } | null>(null);
  let settings = $state<Settings>({
    musicDirectory: "",
    autoPreview: false,
    liveBpmEnabled: false,
    previewDelayMs: 250,
    playbackStartupTimeoutSeconds: 20,
    matchingLevel: "balanced",
    theme: "dark",
    preferredDownloadSource: "youtube",
    downloadLayout: "artistAlbum",
    preferredStreamingSource: "youtube",
    overridePlaylistTrackUrls: false,
    streamingFallbackEnabled: true,
    streamingFallbackDelayMs: 1500,
    streamingFallbackSources: ["youtube", "soundcloud", "bandcamp"],
    bandcampFallbackEnabled: true,
    backgroundSourcePreparation: true,
    downloadFallbackEnabled: false,
    downloadFallbackSources: ["monochrome", "slskd", "youtube", "soundcloud"],
    downloadFallbackAllowLossy: false,
    soulseekWaitForSource: false,
    soulseekLosslessUpgrades: false,
    playlistMirroringEnabled: true,
    compactPlaybackBar: false,
    ytDlpPath: "yt-dlp",
    ffmpegPath: "ffmpeg",
    ffprobePath: "ffprobe",
    jsRuntimePath: "node",
  });
  let lyricsTranslationLanguage = $state("en");
  async function changeLyricsTranslationLanguage(language:string) {const status=await call<{settings:Record<string,unknown>&{lyricsTranslation:Record<string,unknown>}}>("get_connections");await call("save_connections",{update:{settings:{...status.settings,lyricsTranslation:{...status.settings.lyricsTranslation,targetLanguage:language}}}});lyricsTranslationLanguage=language;}
  let lyricsTranslationEnabled = $state(false);
  let lyricsTranslationCredentialPresent = $state(false);
  function currentDetectionPositionSeconds() {
    return (playingTrack?.playbackRange)
      ? chapterAbsolutePosition(playingTrack,position)
      : position;
  }
  function currentVisibleLyricsTrack() {
    const detected=(currentLyricsTrack?.provenance as {activeLyricsDetection?:{detectedAtSeconds:number}}|undefined)?.activeLyricsDetection;
    const owner=currentPlayingTrackDetectionOwner(), view=playingTrackDetectionView;
    const observation=detected && owner && view?.enabled && samePlayingTrackDetectionOwner(owner,playingTrackDetectionOwner)
      ? activeDetectedTrack(view.observations,currentDetectionPositionSeconds()) : null;
    const detection=observation && detected && observation.detectedAtSeconds===detected.detectedAtSeconds && view?.resourceKey
      ? {...owner!,resourceKey:view.resourceKey,detectedAtSeconds:observation.detectedAtSeconds} : undefined;
    return makeVisibleLyricsTrack(currentLyricsTrack,detection);
  }
  async function translateCurrentTrackLyrics(sourceHash: string, lines: LyricsSourceLine[], progress?: TranslationProgress, signal?:AbortSignal, options?:{useFallback?:boolean}): Promise<LyricsTranslationResult> {
    const visibleTrack=currentVisibleLyricsTrack();
    return translateLyricsInChunks(sourceHash,lines,(lineStart,lineEnd)=>call<LyricsTranslationResult>("translate_current_track_lyrics",{sourceHash,visibleTrack,lines,lineStart,lineEnd,useFallback:options?.useFallback}),progress,signal);
  }
  let systemLight = $state(false);
  onMount(()=>{if(typeof window.matchMedia!=="function")return;const query=window.matchMedia("(prefers-color-scheme: light)");const update=()=>systemLight=query.matches;update();query.addEventListener("change",update);return()=>query.removeEventListener("change",update);});
  function handleDesktopExternalAnchor(event: MouseEvent) {
    if (!desktop() || event.defaultPrevented) return;
    if (event.type === "click" && event.button !== 0) return;
    if (event.type === "auxclick" && event.button !== 1) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const anchor = target.closest<HTMLAnchorElement>("a[href]");
    if (!anchor || anchor.target.toLowerCase() !== "_blank" || anchor.hasAttribute("download") || anchor.onclick) return;
    let url: URL;
    try { url = new URL(anchor.href); } catch { event.preventDefault(); return; }
    if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password) {
      event.preventDefault();
      return;
    }
    event.preventDefault();
    void run(() => call("open_external_url", { url: url.href }));
  }
  onMount(() => {
    document.addEventListener("click", handleDesktopExternalAnchor);
    document.addEventListener("auxclick", handleDesktopExternalAnchor);
    return () => {
      document.removeEventListener("click", handleDesktopExternalAnchor);
      document.removeEventListener("auxclick", handleDesktopExternalAnchor);
    };
  });
  type SettingsCategory = "general"|"playback"|"library"|"downloads"|"connections"|"integrations"|"advanced";
  let settingsCategory = $state<SettingsCategory>("general");
  type PlaylistPreferences = { favoritesPlaylistId: string; shortcuts: (string|null)[]; shortcutBindings?: string[] };
  let playlistPreferences = $state<PlaylistPreferences>({ favoritesPlaylistId:"", shortcuts:Array(9).fill(null) });
  let includeYtdlpSearchDefault = $state(false);
  let ytdlpSearchPreferenceReady = $state(false);
  let favoriteTrackIds = $state<string[]>([]);
  let shortcutSlot = $state<number|null>(null);
  let persistedMusicDirectory = $state("");
  let automationPermissions = $state({ control: true, manage: false });
  let playerHeight = $state(96);
  let sidebarWidth = $state(200), infoPanelWidth = $state(340), screenWidth = $state(1440);
  let sourceCapabilities = $state<Record<string, boolean>>({ soulseek: false, discogs: false });
  let availableSources = $state<{id:string;name:string;kinds?:string[];enabled?:boolean;detail?:string}[]>([]);
  let queueCandidates = $state<Record<string, Candidate>>({});
  let autotagJobs = $state<AutotagJob[]>([]), autotagReviewId = $state<string | null>(null);
  let autotagSettings = $state({enabled:true,shazamFallback:true,fpcalcAvailable:false,songrecAvailable:false,acoustidConfigured:false});

  function saveLayoutWidth(kind: "sidebar"|"info", width:number) {
    if (kind === "sidebar") sidebarWidth=Math.max(160,Math.min(320,width)); else infoPanelWidth=Math.max(280,Math.min(420,width));
    try {localStorage.setItem(`burrata.layout.${kind}.v1`,String(kind === "sidebar" ? sidebarWidth : infoPanelWidth));}catch{}
  }
  function resizeQueuePanel(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    const startX=event.clientX, startWidth=infoPanelWidth;
    const move=(next: PointerEvent)=>saveLayoutWidth("info",startWidth+startX-next.clientX);
    const end=()=>{window.removeEventListener("pointermove",move);window.removeEventListener("pointerup",end);window.removeEventListener("pointercancel",end);};
    window.addEventListener("pointermove",move);window.addEventListener("pointerup",end,{once:true});window.addEventListener("pointercancel",end,{once:true});
  }

  // All supported WebView seeks originate here, not in delayed DOM completion
  // events. This also invalidates an older pending resume before moving media.

  async function seekPlayer(relative:number) {

    const value=(relative);
    let mediaSeek = false;
    await nativeCommand({action:"seek",position:value,...playbackOperationTrace()});
    position=value;if(!mediaSeek)seekSerial++;
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
    rememberRoute();
    infoSearchTarget=null;query=value;catalogSearchSource="musicbrainz";catalogSearchKind="all";catalogSearchRequest++;
    await navigate("search", false);
  }
  function openWebInfoSearch(value:string) {
    void call("open_external_url",{url:`https://www.google.com/search?q=${encodeURIComponent(value)}`});
  }
  function openInfoSource(url:string) {
    if(!/^https?:\/\//i.test(url))return;
    const track=infoSearchTarget?.track;
    if(track)void openTrackSource(track,url);
    else void call("open_external_url",{url});
  }
  async function queueChapter(parent:Track,candidate:Candidate,chapter:Chapter,mode:"next"|"append") {
    rememberQueueCandidate(parent,candidate);const track=virtualChapterTrack(parent,candidate,chapter);
    if(track) { try {const registered=desktop()?await call<Track>("register_chapter_track",{track}):track;queueArtistTracks([registered],mode);}catch(failure){error=String(failure);} }
  }
  async function downloadChapter(parent:Track,candidate:Candidate,chapter:Chapter) {
    rememberQueueCandidate(parent,candidate);
    await openDownloadChooser(parent,candidate,chapter.id);
  }
  async function addChapterToPlaylist(parent:Track,chapter:Chapter) {
    const generation=++playlistPickerGeneration, request=navigationRequest;
    const isCurrent=()=>generation===playlistPickerGeneration&&request===navigationRequest;
    const candidate=queueCandidates[parent.id] ?? importedAudioCandidate(parent) ?? (playingTrack?.id===parent.id ? playingCandidate : null);
    if(!candidate){if(isCurrent())error="This chapter's audio source is unavailable. Refresh the chapter list and try again.";return;}
    const virtual=virtualChapterTrack(parent,candidate,chapter);
    if(!virtual){if(isCurrent())error="This chapter has an invalid source range and cannot be added to a playlist.";return;}
    try {
      const registered=desktop()?await call<Track>("register_chapter_track",{track:virtual}):virtual;
      if(!isCurrent())return;
      await addArtistTrack(registered,false);
    } catch(failure) { if(isCurrent())error=safePreviewError(failure); }
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
  async function recoverDownloadSourceTags(assetId: string) {
    const result = await call<{ changed: boolean; fields: string[]; identityStatus: string }>("repair_download_source_tags", { assetId });
    autotagReviewAsset = await call<LocalAsset | null>("get_asset_by_id", { assetId });
    await loadAutotagState();
    return result;
  }
  async function openSearchAlbum(release:ArtistReleaseRef, artistHint?:ArtistRef|null) {
    rememberRoute();
    if (view !== "search") await navigate("search", false);
    clearSearchPanelInstance();
    const request=++searchReleaseRequest;
    searchRelease=release;searchReleaseTracks=[];searchReleaseError="";searchReleaseBusy=true;
    try {
      let artist=release.credits.find(credit=>credit.artist)?.artist ?? artistHint;
      if(!artist) {
        const found=await searchArtists(release.credits.map(c=>c.name).join(" "),1);
        artist=found.results.find(a=>a.name.toLowerCase()===release.credits[0]?.name.toLowerCase())?.artist;
      }
      if(!artist)throw new Error("This release has no unambiguous artist identity. Open its source to inspect it.");
      const rows=await getReleaseTracks(release,artist);
      if(request===searchReleaseRequest&&view==="search"&&searchRelease?.sourceUrl===release.sourceUrl&&searchRelease.release.id===release.release.id)searchReleaseTracks=rows;
    }catch(failure){if(request===searchReleaseRequest)searchReleaseError=String(failure);}finally{if(request===searchReleaseRequest)searchReleaseBusy=false;}
  }
  const soulseekAlbumSearchAvailable = $derived.by(() => {
    const source = availableSources.find(item => item.id === "soulseek");
    return Boolean(source && source.enabled !== false && (!source.kinds?.length || source.kinds.includes("album")));
  });
  async function openSoulseekAlbumSearch(release: ArtistReleaseRef) {
    const credits = release.credits.filter(credit => credit.name.trim());
    const artist = credits.find(credit => /^(?:artist|primary artist|main artist|performer)$/i.test(credit.role.trim())) ?? credits.find(credit => credit.artist) ?? credits[0];
    if (!artist?.name.trim()) throw new Error("This release has no credited artist name to search.");
    await openSoulseekAlbumSearchByTitle(release.title, artist.name);
  }
  async function openSoulseekAlbumSearchByTitle(title: string, artistName: string) {
    if (!soulseekAlbumSearchAvailable) throw new Error("Soulseek is not enabled. Configure it in Settings → Connections and retry.");
    rememberRoute();
    await navigate("search", false);
    searchNavigationSnapshotKey = null;
    query = `${title.trim()} ${artistName.trim()}`.trim();
    catalogSearchSource = "soulseek";
    catalogSearchKind = "albums";
    catalogSearchRequest++;
    searchRelease = null;
    searchReleaseTracks = [];
    searchReleaseRequest++;
  }
  type TrackAlbumTarget = { kind: "spotify"; albumId: string } | { kind: "catalog"; release: ArtistReleaseRef; artist: ArtistRef } | { kind: "search"; album: string; artist: string };
  let trackAlbumLookupRequest = 0;
  function albumHint(value: unknown): string {
    if (typeof value !== "string") return "";
    const hint = value.normalize("NFKC").trim().replace(/\s+/g, " ");
    return /^(?:unknown|untitled|n\/?a|none|not available)$/i.test(hint) ? "" : hint;
  }
  function trackAlbumTarget(track: Track): TrackAlbumTarget | null {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    if (track.provider === "spotify") {
      const context = provenance.recordingContext && typeof provenance.recordingContext === "object" ? provenance.recordingContext as Record<string, unknown> : null;
      const contextual = context?.albumId, direct = provenance.albumId;
      if (typeof contextual === "string" && typeof direct === "string" && contextual !== direct) return null;
      const albumId = typeof contextual === "string" ? contextual : typeof direct === "string" ? direct : "";
      if (!albumId) {
        const album = albumHint(track.album), artist = albumHint(track.albumArtist) || albumHint(track.artist);
        return album && artist ? { kind: "search", album, artist } : null;
      }
      if (!/^[A-Za-z0-9]{22}$/.test(albumId)) return null;
      if (context?.albumUri !== undefined && context.albumUri !== `spotify:album:${albumId}`) return null;
      return { kind: "spotify", albumId };
    }
    if (track.provider !== "catalog") {
      const album = albumHint(track.album), artist = albumHint(track.albumArtist) || albumHint(track.artist);
      return album && artist ? { kind: "search", album, artist } : null;
    }
    if (provenance.catalogProvider === "monochrome") {
      const releaseId = provenance.releaseId;
      if (typeof releaseId !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(releaseId)) return null;
      const rawCredits = Array.isArray(provenance.artistCredits) ? provenance.artistCredits : [];
      const validCredits = rawCredits.flatMap(value => {
        if (!value || typeof value !== "object") return [];
        const credit = value as Record<string, unknown>, rawArtist = credit.artist && typeof credit.artist === "object" ? credit.artist as Record<string, unknown> : null;
        if (rawArtist?.provider !== "monochrome" || typeof rawArtist.id !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(rawArtist.id) || typeof credit.name !== "string" || !credit.name.trim()) return [];
        return [{ name: credit.name, artist: { provider: "monochrome" as const, id: rawArtist.id }, role: typeof credit.role === "string" ? credit.role : "artist credit", creditedAs: null, joinPhrase: null, sourceUrl: `https://tracks.monochrome.st/releases/${releaseId}` }];
      });
      const artist = validCredits.find(credit => normalizeArtistLookup(credit.name) === normalizeArtistLookup(track.artist))?.artist ?? validCredits[0]?.artist;
      if (!artist) return null;
      const sourceUrl = `https://tracks.monochrome.st/releases/${releaseId}`;
      return { kind: "catalog", artist, release: { release: { provider: "monochrome", id: releaseId }, title: track.album?.trim() || track.title, releaseDate: track.releaseDate, artworkUrls: track.artworkUrl ? [track.artworkUrl] : [], tracks: [], sourceUrl, credits: validCredits } };
    }
    if (provenance.catalogProvider !== "musicbrainz") return null;
    const releaseId = provenance.releaseId;
    if (typeof releaseId !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(releaseId)) return null;
    const credits = Array.isArray(provenance.artistCredits) ? provenance.artistCredits : [];
    const validCredits = credits.flatMap(value => {
      if (!value || typeof value !== "object") return [];
      const credit = value as Record<string, unknown>;
      const rawArtist = credit.artist && typeof credit.artist === "object" ? credit.artist as Record<string, unknown> : null;
      const id = rawArtist?.id, name = credit.name;
      if (rawArtist?.provider !== "musicbrainz" || typeof id !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(id) || typeof name !== "string" || !name.trim()) return [];
      const artist: ArtistRef = { provider: "musicbrainz", id };
      return [{ name, artist, role: typeof credit.role === "string" ? credit.role : "artist credit", creditedAs: typeof credit.creditedAs === "string" ? credit.creditedAs : null, joinPhrase: typeof credit.joinPhrase === "string" ? credit.joinPhrase : null, sourceUrl: `https://musicbrainz.org/release/${releaseId}` }];
    });
    const artist = validCredits.find(credit => credit.role.toLowerCase().includes("artist"))?.artist;
    if (!artist) return null;
    const sourceUrl = `https://musicbrainz.org/release/${releaseId}`;
    return { kind: "catalog", artist, release: { release: { provider: "musicbrainz", id: releaseId }, title: track.album?.trim() || track.title, tracks: [], sourceUrl, credits: validCredits } };
  }
  async function openTrackAlbum(track: Track) {
    const target = trackAlbumTarget(track);
    if (!target) return;
    if (target.kind === "catalog") { await openSearchAlbum(target.release, target.artist); return; }
    if (target.kind === "search") {
      const request = ++trackAlbumLookupRequest, ownerNavigation = navigationRequest;
      try {
        const result = await call<AlbumSearchPage>("search_albums", { query: target.album, page: 1, pageSize: 20 });
        if (request !== trackAlbumLookupRequest || ownerNavigation !== navigationRequest) return;
        const titleKey = normalizeArtistLookup(target.album), artistKey = normalizeArtistLookup(target.artist);
        const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
        const catalog = provenance.catalog && typeof provenance.catalog === "object" ? provenance.catalog as Record<string, unknown> : {};
        const knownArtistIds = (Array.isArray(catalog.artists) ? catalog.artists : []).flatMap(value => {
          if (!value || typeof value !== "object") return [];
          const item = value as Record<string, unknown>;
          const provider = item.provider, id = item.id, name = item.name;
          return (provider === "musicbrainz" || provider === "discogs") && typeof id === "string" && id &&
            typeof name === "string" && normalizeArtistLookup(name) === artistKey ? [{ provider, id }] : [];
        });
        const exact = result.items.filter(release => normalizeArtistLookup(release.title) === titleKey &&
          release.credits.some(credit => normalizeArtistLookup(credit.name) === artistKey &&
            (!knownArtistIds.length || knownArtistIds.some(identity => credit.artist?.provider === identity.provider && credit.artist.id === identity.id))));
        if (exact.length === 1) { await openSearchAlbum(exact[0]!); return; }
        // No exact match or multiple editions: let the existing album search
        // show the catalog choices instead of guessing at a release.
        query = target.album;
        catalogSearchSource = "musicbrainz";
        catalogSearchKind = "albums";
        catalogSearchRequest++;
        await navigate("search");
      } catch (failure) {
        if (request === trackAlbumLookupRequest && ownerNavigation === navigationRequest) setNotice(safePreviewError(failure));
      }
      return;
    }
    await openSpotifyAlbum(target.albumId, track.album?.trim() || "");
  }
  async function openSpotifyAlbum(albumId: string, title: string) {
    rememberRoute();
    spotifyNavigationSnapshotKey = null;
    spotifyArtistId = ""; spotifyAlbumId = albumId; spotifyArtistSearchFallback = null;
    activeArtistName = title; activeArtistRef = null; activeArtistTrack = null; artistSection = "overview"; artistScroll = 0;
    await navigate("artist", false);
  }
  let importDateFilter = $state("");
  let showFullNowPlaying = $state(false);
  let showFullscreenLyrics = $state(false);
  let lyricsButton = $state<HTMLButtonElement | null>(null);
  const trackArtistInformation = createTrackArtistInformationLoader({
    searchArtists: name => searchArtists(name, 1),
    getArtistProfile: artist => call<ArtistProfile>("get_artist_profile", {artist}),
    lookupSpotifyArtist: artistId => call<ArtistRef | null>("cached_spotify_artist_ref", {artistId}),
    lookupRecordingArtist: async track => {
      const result = await recordingInformation.load(track);
      return result.recording?.recordingArtist?.artist ?? null;
    },
  });
  async function loadNowPlayingArtist(track: Track): Promise<ArtistProfile | null> {
    if (track.provider === "radio" && !(track.provenance as {radioMetadata?:unknown}|null)?.radioMetadata) return null;
    return (await trackArtistInformation.load(track))?.profile ?? null;
  }
  const recordingInformation = createRecordingInformationLoader({searchSource: request => call("search_source", {...request, cursor:null})});
  async function browseRecordingRelease(release: RecordingReleaseInfo, artist: ArtistRef | null) {
    openPlayerPanel(null);
    await openSearchAlbum({release:{provider:"musicbrainz",id:release.id},title:release.title,
      releaseDate:release.date,country:release.country,sourceUrl:release.sourceUrl,credits:release.credits ?? [],tracks:[]},artist);
  }
  let showPlayingInfo = $state(false);
  let infoTrack = $state<Track | null>(null);
  let trackLibrarySummary = $state<TrackLibrarySummary | null>(null);
  let trackLibrarySummaryTrackId = $state<string | null>(null);
  let trackLibrarySummaryLoading = $state(false);
  let trackLibrarySummaryRequest = 0;
  let lastNativeHistorySummaryOwner = "";
  let pendingNativeHistorySummaryOwner = "";
  let nativeHistorySummaryTimer: ReturnType<typeof setTimeout> | null = null;
  let infoSearchTarget = $state<{ track: Track; candidate: Candidate | null; chapterTitle?: string } | null>(null);
  let catalogSearchRequest = $state(0);
  let catalogSearchSource = $state("musicbrainz");
  let catalogSearchKind = $state<SearchKind>("all");
  let automationSearch = $state<AutomationSearchRequest | null>(null);
  let automationSearchCancelled = $state<string | null>(null);
  function completeAutomationSearch(report: AutomationSearchReport) {
    if (automationSearch?.requestId !== report.requestId) return;
    automationSearch = null;
    automationSearchCancelled = null;
    void call("automation_search_ui_complete", report).catch(() => {});
  }
  let showDownloads = $state(false);
  let pausedMirrors = $state<string[]>([]), localPlaylistIds = $state<string[]>([]);
  let albumQuery = $state("");
  let searchReleaseRequest=0;
  type SearchReleaseControls={filter:string;sort:TrackSortField;sortDirection:SortDirection;minDuration:string;maxDuration:string;scroll:number};
  let searchReleaseComponent=$state<{captureNavigationState:()=>SearchReleaseControls;restoreNavigationState:(state:SearchReleaseControls)=>Promise<void>;captureLoadedAlbumSnapshot:()=>{identity:string;revision:string;trackCount:number;loaded:boolean};downloadLoadedAlbum:(identity:string,revision:string,onReceipt?:(receipt:unknown)=>void|Promise<void>)=>unknown|Promise<unknown>}|undefined>();
  type SearchReleaseSnapshot={release:ArtistReleaseRef;tracks:Track[];error:string;controls:SearchReleaseControls|null;needsReload:boolean};
  function captureSearchRelease():string|null {
    if(!searchRelease)return null;
    const key=`route-release:${++navigationSnapshotSequence}`;
    const rows=searchReleaseTracks.length+searchRelease.tracks.length;
    const stripRows=(saved:SearchReleaseSnapshot):SearchReleaseSnapshot=>({...saved,release:{...saved.release,tracks:[]},tracks:[],needsReload:true});
    let snapshot:SearchReleaseSnapshot={release:searchRelease,tracks:searchReleaseTracks,error:searchReleaseBusy?"Album loading was interrupted. Return to search and reopen it to load tracks.":searchReleaseError,controls:searchReleaseComponent?.captureNavigationState()??null,needsReload:false};
    if(rows>10000)snapshot=stripRows(snapshot);
    providerNavigationSnapshots.put({kind:"search-release",key,accountScope:"public-catalog",snapshot,rows:snapshot.needsReload?0:rows,stripRows});
    return key;
  }
  let searchRelease = $state<ArtistReleaseRef | null>(null), searchReleaseTracks = $state<Track[]>([]), searchReleaseBusy = $state(false), searchReleaseError = $state("");
  let librarySource = $state("all"), searchCategory = $state("tracks"), showQueue = $state(false);
  let historyQuery = $state(""), historyProvider = $state(""), historyStationId = $state(""), historyFrom = $state(""), historyTo = $state(""), historyOrder = $state("newest"), historyBusy = $state(false), historyHasMore = $state(false), historyTotal = $state(0);
  let historyAppliedOptions = $state("");
  let historyRequest = 0;
  let includeRadioHistory = $state(true), currentRadioSong = $state<Track|null>(null);
  let radioOccurrence: { session:string; key:string; identity:string; at:string } | null = null;
  let radioQueueHistory = $state<{id:string;track:Track;playedAt:string;stationName:string}[]>([]);
  let radioRecognizing = $state(false), radioIdentifyProgress = $state(0), mpvAvailable = $state(false);
  let radioIdentifyAbort: AbortController | null = null;
  let radioIdentifyCaptureId: string | null = null;
  let playingTrackDetectionView = $state<PlayingTrackDetectionView | null>(null);
  let detectedArtworkMode = $state<"set" | "detected" | "both">("detected");
  let fullscreenArtworkMode = $state<"set" | "detected" | "both">("set");
  let detectionsByTrack = $state<Record<string, import("./lib/playing-track-detection").DetectedTrackObservation[]>>({});
  let savedDetectionViewsByTrack = $state<Record<string, PlayingTrackDetectionView>>({});
  const savedDetectionCache = new Map<string, { signature: string; bytes: number }>();
  const savedDetectionPending = new Map<string, { track: Track; signature: string }>();
  const savedDetectionInFlight = new Map<string, string>();
  let savedDetectionCacheBytes = 0;
  let savedDetectionDisposed = false;
  onDestroy(() => { savedDetectionDisposed = true; savedDetectionPending.clear(); });
  function detectionTrackSignature(track: Track): string {
    const provenance = track.provenance as Record<string, unknown> | undefined;
    return JSON.stringify([track.id, track.provider, track.providerId, track.assetId,
      track.localPath, track.durationSeconds, track.playbackRange, provenance?.localAudioFactsHash, provenance?.audioCandidate]);
  }
  function rememberDetectionView(trackId: string, signature: string, view: PlayingTrackDetectionView): void {
    const bytes = JSON.stringify(view).length * 2 + signature.length * 2;
    savedDetectionCacheBytes -= savedDetectionCache.get(trackId)?.bytes ?? 0;
    savedDetectionCache.delete(trackId); savedDetectionCache.set(trackId, { signature, bytes });
    savedDetectionCacheBytes += bytes;
    savedDetectionViewsByTrack[trackId] = view;
    detectionsByTrack[trackId] = view.observations;
    for (const [id, entry] of savedDetectionCache) {
      if (savedDetectionCache.size <= 64 && savedDetectionCacheBytes <= 4 * 1024 * 1024) break;
      if (id === playingTrack?.id || id === infoTrack?.id) continue;
      savedDetectionCache.delete(id); savedDetectionCacheBytes -= entry.bytes;
      delete savedDetectionViewsByTrack[id]; delete detectionsByTrack[id];
    }
  }
  function drainSavedDetectionRequests(): void {
    if (savedDetectionDisposed) return;
    while (savedDetectionInFlight.size < 4) {
      const next = [...savedDetectionPending.entries()].find(([id]) => !savedDetectionInFlight.has(id));
      if (!next) return;
      const [id, request] = next; savedDetectionPending.delete(id);
      savedDetectionInFlight.set(id, request.signature);
      void call<PlayingTrackDetectionView>("get_saved_track_detections", { track: request.track }).then(view => {
        if (savedDetectionDisposed || !Array.isArray(view?.observations) || id === playingTrack?.id ||
            savedDetectionPending.has(id) && savedDetectionPending.get(id)?.signature !== request.signature) return;
        rememberDetectionView(id, request.signature, view);
      }).catch(() => {}).finally(() => { savedDetectionInFlight.delete(id); drainSavedDetectionRequests(); });
    }
  }
  function loadSavedTrackDetections(track: Track): void {
    if (!desktop() || track.id === playingTrack?.id) return;
    const signature = detectionTrackSignature(track);
    if (savedDetectionCache.get(track.id)?.signature === signature || savedDetectionInFlight.get(track.id) === signature) return;
    if (savedDetectionPending.size >= 64) savedDetectionPending.delete(savedDetectionPending.keys().next().value!);
    savedDetectionPending.set(track.id, { track, signature });
    drainSavedDetectionRequests();
  }
  $effect(() => {
    const trackId = playingTrackDetectionOwner?.trackId;
    const view = playingTrackDetectionView;
    if (trackId && view) untrack(() => rememberDetectionView(trackId,
      playingTrack ? detectionTrackSignature(playingTrack) : view.resourceKey ?? trackId, view));
  });
  $effect(() => {
    const track = showPlayingInfo ? infoTrack : null;
    const queueVisible = showQueue;
    untrack(() => {
      if (!queueVisible) savedDetectionPending.clear();
      if (track) loadSavedTrackDetections(track);
    });
  });
  let playingTrackDetectionOwner = $state<PlayingTrackDetectionOwner | null>(null);
  let playingTrackDetectionBusy = $state(false);
  let keepTrackDetectionForQueue = $state(false);
  let playingTrackDetectionStatus = $state("");
  let playingTrackDetectionDisabledReason = $state("");
  let playingTrackDetectionCaptureId: string | null = null;
  let lastPlayingTrackDetectionAt = 0;
  let lastPlayingTrackDetectionReadAt = 0;
  let playingTrackDetectionNeedsRead = false;
  let nativeIdentifyCaptureId: string | null = null;
  function cancelRadioIdentification(expectedCaptureId?:string) {
    const captureId=radioIdentifyCaptureId;
    if(expectedCaptureId && expectedCaptureId!==captureId)return;
    radioIdentifyAbort?.abort();
    if(captureId && desktop()) void call("cancel_playing_track_identification",{captureId}).catch(()=>{});
  }
  function currentPlayingTrackDetectionOwner():PlayingTrackDetectionOwner|null {
    if (!playingTrack || playingRadioStation || currentRadioSong || isStationPlaybackTrack(playingTrack)
      || (playingTrack.provenance as Record<string,unknown>|undefined)?.sourceKind==="radio") return null;
    const generation=(nativeSnapshot?.generation);
    if (!Number.isSafeInteger(generation) || generation == null || !Number.isSafeInteger(seekSerial)) return null;
    return {trackId:playingTrack.id,generation,seekSerial:((nativeSnapshot?.seekSerial??0))};
  }
  function trackDetectionOwnerIsCurrent(owner:PlayingTrackDetectionOwner):boolean {
    return samePlayingTrackDetectionOwner(owner,currentPlayingTrackDetectionOwner()) && !playingRadioStation;
  }
  function trackDetectionCanCapture():boolean {
    return !paused&&!playbackLoading&&!bufferingAudio&&!resolvingPlayback;
  }
  function cancelPlayingTrackDetection() {
    const captureId=playingTrackDetectionCaptureId;
    if (!captureId) return;
    if (radioIdentifyCaptureId===captureId) cancelRadioIdentification(captureId);
    playingTrackDetectionCaptureId=null;
  }
  async function runPlayingTrackDetection(owner:PlayingTrackDetectionOwner) {
    if (playingTrackDetectionBusy || radioRecognizing || !trackDetectionOwnerIsCurrent(owner) || !trackDetectionCanCapture() || !playingTrackDetectionView?.enabled || playingTrackDetectionView.retentionLimitReached) return;
    const captureId=crypto.randomUUID();
    const controller=new AbortController();
    playingTrackDetectionBusy=true;
    playingTrackDetectionCaptureId=captureId;
    radioIdentifyCaptureId=captureId;
    radioIdentifyAbort=controller;
    nativeIdentifyCaptureId=captureId;
    radioRecognizing=true;
    radioIdentifyProgress=0;
    lastPlayingTrackDetectionAt=Date.now();
    playingTrackDetectionStatus="Listening for tracks in this recording…";
    try {
      const result=await call<PlayingTrackDetectionView>("detect_playing_track_segment",{trackId:owner.trackId,generation:owner.generation,seekSerial:owner.seekSerial,captureId});
      if(!controller.signal.aborted&&trackDetectionOwnerIsCurrent(owner)){
        playingTrackDetectionView=result;
        playingTrackDetectionStatus=result.observations.length?"":"No tracks detected in that sample yet.";
      }
    } catch(failure) {
      if(!controller.signal.aborted&&trackDetectionOwnerIsCurrent(owner)) {
        const message=String(failure);
        playingTrackDetectionStatus=/not enough audio|not enough.*identify/i.test(message)
          ? "There was not enough audio at this position. Detection will try again later."
          : /resume/i.test(message)
            ? "Detection will continue when playback resumes."
            : "Could not check this audio. Detection will try again later.";
      }
    } finally {
      if(playingTrackDetectionCaptureId===captureId)playingTrackDetectionCaptureId=null;
      if(radioIdentifyCaptureId===captureId)radioIdentifyCaptureId=null;
      if(nativeIdentifyCaptureId===captureId)nativeIdentifyCaptureId=null;
      if(radioIdentifyAbort===controller)radioIdentifyAbort=null;
      if(playingTrackDetectionBusy&&playingTrackDetectionCaptureId==null)playingTrackDetectionBusy=false;
      if(radioRecognizing&&radioIdentifyCaptureId==null)radioRecognizing=false;
      // Keep the start timestamp: waiting another full interval after the
      // capture/service delay would exceed the deduplication cadence.
    }
  }
  async function refreshPlayingTrackDetection() {
    const owner=currentPlayingTrackDetectionOwner();
    const key=playingTrackDetectionOwnerKey(owner);
    const oldKey=playingTrackDetectionOwnerKey(playingTrackDetectionOwner);
    if(key!==oldKey) {
      const sameSource = owner && playingTrackDetectionOwner && owner.trackId===playingTrackDetectionOwner.trackId
        && owner.generation===playingTrackDetectionOwner.generation;
      cancelPlayingTrackDetection();
      playingTrackDetectionOwner=owner;
      // Seeking invalidates captures, not the recording's persisted opt-in.
      if(!sameSource)playingTrackDetectionView=null;
      playingTrackDetectionStatus=sameSource&&playingTrackDetectionView?.enabled ? "Detection will continue when playback is ready." : "";
      playingTrackDetectionDisabledReason="";
      lastPlayingTrackDetectionAt=0;
      lastPlayingTrackDetectionReadAt=0;
      playingTrackDetectionNeedsRead=Boolean(owner);
    }
    if(owner&&desktop()&&playingTrackDetectionNeedsRead) {
      if(Date.now()-lastPlayingTrackDetectionReadAt<1000)return;
      lastPlayingTrackDetectionReadAt=Date.now();
      try {
        let result=await call<PlayingTrackDetectionView>("get_playing_track_detections",{...owner});
        if (trackDetectionOwnerIsCurrent(owner) && keepTrackDetectionForQueue && !result.enabled) {
          result=await call<PlayingTrackDetectionView>("set_playing_track_detection_enabled",{...owner,enabled:true});
        }
        if(trackDetectionOwnerIsCurrent(owner)){
          playingTrackDetectionView=result;
          playingTrackDetectionNeedsRead=false;
          playingTrackDetectionDisabledReason="";
          playingTrackDetectionStatus="";
        }
      } catch {
        if(trackDetectionOwnerIsCurrent(owner)) {
          if(playingTrackDetectionView)playingTrackDetectionStatus="Detection will continue when playback is ready.";
          else playingTrackDetectionDisabledReason="Track detection needs a prepared audio source with a stable identity.";
        }
      }
      return;
    }
    if(!owner||!desktop()||!playingTrackDetectionView?.enabled||playingTrackDetectionView.retentionLimitReached||!trackDetectionCanCapture()||radioRecognizing||playingTrackDetectionBusy) return;
    if(Date.now()-lastPlayingTrackDetectionAt>=PLAYING_TRACK_DETECTION_INTERVAL_MS) await runPlayingTrackDetection(owner);
  }
  async function togglePlayingTrackDetection(enabled:boolean) {
    if (!enabled) keepTrackDetectionForQueue=false;
    const owner=currentPlayingTrackDetectionOwner();
    if(!owner||!desktop())return;
    if(!enabled)cancelPlayingTrackDetection();
    try {
      const result=await call<PlayingTrackDetectionView>("set_playing_track_detection_enabled",{...owner,enabled});
      if(!trackDetectionOwnerIsCurrent(owner))return;
      playingTrackDetectionView=result;
      playingTrackDetectionNeedsRead=false;
      playingTrackDetectionDisabledReason="";
      playingTrackDetectionStatus=enabled?(!trackDetectionCanCapture()?"Detection will start when playback is ready.":"Listening for tracks in this recording…"):"Detection is off. Saved detections are kept.";
      if(enabled&&trackDetectionCanCapture()&&!radioRecognizing)await runPlayingTrackDetection(owner);
    } catch {
      if(trackDetectionOwnerIsCurrent(owner))playingTrackDetectionDisabledReason="Track detection needs a prepared audio source with a stable identity.";
    }
  }
  onMount(()=>{
    void refreshPlayingTrackDetection();
    const timer=window.setInterval(()=>{void refreshPlayingTrackDetection();},1000);
    return ()=>{window.clearInterval(timer);cancelPlayingTrackDetection();};
  });
  $effect(()=>{
    const currentKey=playingTrackDetectionOwnerKey(currentPlayingTrackDetectionOwner());
    const boundKey=playingTrackDetectionOwnerKey(playingTrackDetectionOwner);
    if(currentKey!==boundKey)untrack(()=>{void refreshPlayingTrackDetection();});
  });
  const previousRadioQueue = $derived(radioQueueHistory.filter((item,index)=>(!currentRadioSong || index!==0 || item.track.id!==currentRadioSong.id)).map(item=>({...item,track:{...item.track,favorite:favoriteTrackIds.includes(item.track.id)}})));
  onMount(()=>{if(desktop())for(const provider of ["spotify","youtube"] as const)void call<{writeSyncEnabled?:boolean}>("remote_playlist_connection_status",{provider}).then(status=>remotePlaylistWriteSyncEnabled={...remotePlaylistWriteSyncEnabled,[provider]:status?.writeSyncEnabled===true}).catch(()=>{});});
  onMount(()=>{if(desktop())void call<{available:boolean}>("get_mpv_status").then(status=>mpvAvailable=status?.available===true).catch(()=>{});});

  let lastRadioAudioAttempt:{stationId:string;generation:number|undefined;at:number}|null=null;
  async function identifyPlayingRadio(automatic=false) {
    const station=playingRadioStation;if(!station || !playingTrack || radioRecognizing)return;
    const trackId=playingTrack.id, session=playbackSession, serial=seekSerial, chapterId=false;
    const nativeIdentification=true, sourceGeneration=preview?.sourceGeneration, nativeGeneration=nativeSnapshot?.generation;
    const sourceUrl=playingTrack?.sourceUrl ?? "";
    const current=()=>playingTrack?.id===trackId && playingRadioStation?.id===station.id && playbackSession===session &&
      seekSerial===serial && false===chapterId && preview?.sourceGeneration===sourceGeneration && true===nativeIdentification &&
      ((nativeSnapshot?.generation===nativeGeneration)) && (true);
    radioRecognizing=true;radioIdentifyProgress=0;
    lastRadioAudioAttempt={stationId:station.id,generation:sourceGeneration,at:Date.now()};
    const controller=new AbortController();radioIdentifyAbort=controller;
    const captureId=(crypto.randomUUID());
    if(captureId){nativeIdentifyCaptureId=captureId;radioIdentifyCaptureId=captureId;}
    try {
      let result:{nowPlaying:RadioTrackMetadata|null;note?:string};
      result=await call("identify_radio_station",{stationId:station.id,captureId});
      if(controller.signal.aborted || !current())return;if(!automatic)setNotice(result.nowPlaying?`Identified: ${result.nowPlaying.artist} — ${result.nowPlaying.title}`:result.note||"No song identified. Try again during music.");
    }catch(failure){if(!automatic && current() && !controller.signal.aborted && !/cancelled|canceled|AbortError/i.test(String(failure)))error=String(failure);}
    finally{
      if(captureId && nativeIdentifyCaptureId===captureId)nativeIdentifyCaptureId=null;
      if(captureId && radioIdentifyCaptureId===captureId)radioIdentifyCaptureId=null;
      if(radioIdentifyAbort===controller){radioIdentifyAbort=null;radioRecognizing=false;}
    }
  }
  async function openTrackSource(track:Track,url:string) {
    const sourceKey=(raw:string)=>{try{const parsed=new URL(raw);return ["http:","https:"].includes(parsed.protocol)?parsed.href:null;}catch{return null;}};
    const currentTrack=playingTrack, range=currentTrack?.playbackRange;
    const parentChapter=Boolean(currentTrack && range && range.parentTrackId===track.id &&
      sourceKey(url)!==null && sourceKey(url)===sourceKey(range.sourceUrl));
    const active=Boolean(currentTrack && (sameTrackIdentity(track,currentTrack)||parentChapter));
    const activeTrackId=currentTrack?.id, session=playbackSession, native=true;
    const nativeGeneration=nativeSnapshot?.generation;
    const positionAtOpen = playerPosition;
    const positionTrack=parentChapter ? currentTrack! : track;
    if (active && settings.pauseOnSourceOpen && !paused) {
      await nativeCommand({action:"pause"});
      if (playbackSession!==session || playingTrack?.id!==activeTrackId || true!==native ||
        (nativeSnapshot?.generation!==nativeGeneration)) return;
    }
    await call("open_external_url", {url: active && settings.sourceOpenAtPosition ? sourcePositionUrl(url, positionTrack, positionAtOpen) : url});
  }
  setContext<SourceOpener>(SOURCE_OPENER, openTrackSource);
  function focusPlaylistSearch(node: HTMLInputElement) { void tick().then(()=>{ if (!playlistAlbumName) node.focus(); }); }
  type RecognitionTimingAnchor = { shazamMatchOffsetSeconds:number; capturedAudioStartPositionSeconds:number; capturedAudioDurationSeconds:number; sourceGeneration:number; seekSerial:number; trackId:string; entryId:string };
  type PlayingTrackIdentification = { trackId:string; title?:string|null; artist?:string|null; note?:string; timingAnchor?:RecognitionTimingAnchor|null };
  async function identifyPlayingTrack(): Promise<PlayingTrackIdentification|null> {
    if (playingRadioStation) { await identifyPlayingRadio(); return null; }
    if (!playingTrack || radioRecognizing) return null;
    const id = playingTrack.id;
    const nativeIdentification = true;
    const session = playbackSession, serial = seekSerial, chapterId=false;
    const sourceGeneration = preview?.sourceGeneration, nativeGeneration = nativeSnapshot?.generation;
    const sourceUrl=playingTrack?.sourceUrl ?? "";
    const current = () => playingTrack?.id === id && playbackSession === session && seekSerial === serial && false===chapterId &&
      preview?.sourceGeneration === sourceGeneration && true === nativeIdentification && ((nativeSnapshot?.generation === nativeGeneration)) && (true);
    radioRecognizing = true;
    radioIdentifyProgress=0;
    const controller=new AbortController();radioIdentifyAbort=controller;
    const captureId = (crypto.randomUUID());
    if (captureId) { nativeIdentifyCaptureId=captureId; radioIdentifyCaptureId=captureId; }
    try {

      let result:PlayingTrackIdentification;
      result=await call("identify_playing_track",{captureId});
      if (controller.signal.aborted || !current()) return null;
      setNotice(result.title ? `Identified: ${result.artist ?? "Unknown artist"} — ${result.title}` : result.note || "No song identified. Try again during music.");
      return result;
    } catch (failure) { if (current() && !controller.signal.aborted && !/cancelled|canceled|AbortError/i.test(String(failure))) error = safePreviewError(failure); return null; }
    finally {
      if(captureId && nativeIdentifyCaptureId===captureId)nativeIdentifyCaptureId=null;
      if(captureId && radioIdentifyCaptureId===captureId)radioIdentifyCaptureId=null;
      if(radioIdentifyAbort===controller){radioIdentifyAbort=null;radioRecognizing=false;}
    }
  }
  async function syncLyricsWithAudio(trackId:string):Promise<{offsetSeconds:number}|null> {
    if (!playingTrack || playingTrack.id!==trackId || !true || nativeSnapshot?.status!=="playing" || paused || playingRadioStation || playingTrack.playbackRange) return null;
    const track=playingTrack, generation=nativeSnapshot.generation, serial=seekSerial, entryId=playbackQueue.entryIds[queueIndex] ?? null;
    if (!entryId) return null;
    const result=await identifyPlayingTrack();
    const anchor=result?.timingAnchor;
    if (!result || !anchor || playingTrack?.id!==track.id || !true || nativeSnapshot?.status!=="playing" || nativeSnapshot?.generation!==generation || seekSerial!==serial || paused || playingRadioStation || playingTrack.playbackRange || (playbackQueue.entryIds[queueIndex] ?? null)!==entryId) return null;
    if (result.trackId!==trackId || anchor.trackId!==trackId || anchor.entryId!==entryId || anchor.sourceGeneration!==generation || anchor.seekSerial!==serial ||
        !Number.isFinite(anchor.shazamMatchOffsetSeconds) || !Number.isFinite(anchor.capturedAudioStartPositionSeconds) ||
        !Number.isFinite(anchor.capturedAudioDurationSeconds) || anchor.capturedAudioDurationSeconds<=0) return null;
    const offsetSeconds=anchor.shazamMatchOffsetSeconds-anchor.capturedAudioStartPositionSeconds;
    return Number.isFinite(offsetSeconds) && Math.abs(offsetSeconds)<=30 ? {offsetSeconds} : null;
  }
  async function openTrackInMpv(track:Track) {
    if (!mpvAvailable) return;
    await run(async()=>{
      const provenance=track.provenance as Record<string,unknown>|null;
      let source=track.localPath && track.localAvailability!=="missing" ? track.localPath : typeof provenance?.radioStreamUrl==="string" ? provenance.radioStreamUrl : candidateForQueue(track)?.url || (playingTrack?.id===track.id?playingCandidate?.url:undefined);
      if(!source && ["youtube","soundcloud"].includes(track.provider))source=track.sourceUrl;
      if(!source){const found=await call<Candidate[]>("find_candidates",{track,providerIds:["youtube","soundcloud"]});source=found[0]?.url;}
      if(!source)throw new Error("No playable source found for mpv.");
      const localSource=Boolean(track.localPath && source===track.localPath);
      const current=track.id===playingTrack?.id && Boolean(preview?.canSeek);
      await call("open_in_mpv",{source,startSeconds:current?(localSource?playerPosition:position):localSource?null:track.playbackRange?.startTime??null,endSeconds:localSource?null:track.playbackRange?.endTime??null,video:true});
      if(resolvingPlayback)await closePreview();
      else await nativeBridge.command({action:"pause"});

      paused=true;bufferingAudio=false;
      setNotice("Opened in mpv");
    });
  }
  let historySortField = $state<TrackSortField>("sourceOrder"), historySortDirection = $state<SortDirection>("desc"), historyMinDuration = $state(""), historyMaxDuration = $state("");
  let historyItems = $state<{ id: string; track: Track; provider: string; sessionId: string; playedAt: string; chapter?: unknown }[]>([]);
  let historyFocusedSessionId = $state<string | null>(null);
  let focusedPlaylistRowKey = $state<string | null>(null);
  const displayedHistoryItems = $derived(historyItems.filter(item=>historyDownloadAvailability === "all" || (historyDownloadAvailability === "downloaded" ? isTrackDownloaded(item.track) : !isTrackDownloaded(item.track))));
  let historyScroll = $state(0);
  let queueIndex = $state(-1);
  let queueTracks = $state<Track[]>([]), repeatMode = $state<"off" | "all" | "one">("off"), shuffleEnabled = $state(false);
  let playbackQueueSaveError = $state("");
  const automationQueueProjection = $derived.by(() => {
    const queue=queueTracks.map(track=>({id:track.id,artist:track.artist,title:track.title}));
    const entryIds=[...playbackQueue.entryIds];
    return {queue,entryIds,revision:queueProjectionRevision(queue,entryIds),sourceKey:JSON.stringify(queueTracks.map(queueRadioSeedKey))};
  });
  const automationPlaybackPublisher = createAutomationPlaybackPublisher(
    globalThis.crypto?.randomUUID?.() ?? "page-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2),
  );
  let autoQueueRadio = $state(false), queueRadioPreferenceReady = $state(false);
  let nativePlaybackStatus = $state("idle"), nativePlaybackGeneration = $state(0), nativeNaturalQueueEnd = $state(false), nativeEngineEnabled = $state(false);
  let preparedRadioKey = "", consumingRadio = false;
  let playbackRunGeneration = 0, playbackQueueGeneration = 0;
  function canonicalQueueRadioSeed(track:Track):Track {
    if(!track.playbackRange)return track;
    const provenance=track.provenance&&typeof track.provenance==="object"?track.provenance as Record<string,unknown>:{};
    const parent=provenance.parentTrack;
    return parent&&typeof parent==="object"&&!Array.isArray(parent)&&
      (parent as Track).id===track.playbackRange.parentTrackId ? parent as Track : track;
  }
  const queueRadio = new QueueRadioController(async (seed, signal) => {
    const canonicalSeed=canonicalQueueRadioSeed(seed);
    let candidate=seed.playbackRange?importedAudioCandidate(seed):linkedSearchCandidate(seed);
    if(candidate?.provider!=="youtube") {
      const found = await call<Candidate[]>("find_candidates",{track:canonicalSeed,providerIds:["youtube"]});
      if(signal.aborted)return [];
      candidate=choosePreviewCandidate(canonicalSeed,found,"youtube");
    }
    if(!candidate||signal.aborted)return [];
    const result=await call<{tracks:Track[]}>("get_song_radio",{track:canonicalSeed,candidate,limit:25,persist:false});
    return signal.aborted?[]:result.tracks;
  });
  function hydrateStoredFrontendFlags() {
    try { autoQueueRadio=backupStartupReadStorage.getItem("burrata.queue-end-radio.v1")==="true"; } catch {}
    try { waveformEnabled=backupStartupReadStorage.getItem("burrata.waveform-enabled.v1")==="true"; } catch {}
    try { detectionMarkersEnabled=backupStartupReadStorage.getItem("burrata.detection-markers-enabled.v1")==="true"; } catch {}
    try { streamMediaAnalysisEnabled=backupStartupReadStorage.getItem("burrata.stream-media-analysis-enabled.v1")==="true"; } catch {}
    try { includeYtdlpSearchDefault=backupStartupReadStorage.getItem("burrata.playlist.include-ytdlp-search.v1")==="true"; } catch {}
    ytdlpSearchPreferenceReady=true;
    waveformPreferenceReady=true;
    detectionMarkersPreferenceReady=true;
    streamMediaAnalysisPreferenceReady=true;
    queueRadioPreferenceReady=true;
  }
  onMount(() => () => queueRadio.cancel());
  function loadStreamMediaAnalysisRestorePolicy() {
    if (!desktop()) {
      streamMediaAnalysisRestorePolicyReady = true;
      streamMediaAnalysisRestorePaused = false;
      return;
    }
    void refreshStreamMediaAnalysisRestorePolicy();
  }
  $effect(()=>{if(queueRadioPreferenceReady){try{localStorage.setItem("burrata.queue-end-radio.v1",String(autoQueueRadio));}catch{}}});
  $effect(()=>{if(waveformPreferenceReady){try{localStorage.setItem("burrata.waveform-enabled.v1",String(waveformEnabled));}catch{}}});
  $effect(()=>{if(detectionMarkersPreferenceReady){try{localStorage.setItem("burrata.detection-markers-enabled.v1",String(detectionMarkersEnabled));}catch{}}});
  $effect(()=>{if(streamMediaAnalysisPreferenceReady){try{localStorage.setItem("burrata.stream-media-analysis-enabled.v1",String(streamMediaAnalysisEnabled));}catch{}}});
  $effect(()=>{if(ytdlpSearchPreferenceReady){try{localStorage.setItem("burrata.playlist.include-ytdlp-search.v1",String(includeYtdlpSearchDefault));}catch{}}});
  const radioQueueKey = $derived(JSON.stringify(queueTracks.map(queueRadioSeedKey)));
  $effect(()=>{
    const enabled=autoQueueRadio, seed=playingTrack, queue=queueTracks, index=queueIndex, repeat=repeatMode;
    const status=nativeEngineEnabled?nativePlaybackStatus:preview?(paused?"paused":"playing"):"idle";
    const generation=nativeEngineEnabled?nativePlaybackGeneration:0;
    const key=`${generation}:${radioQueueKey}:${seed?queueRadioSeedKey(seed):""}`;
    if(!enabled||!seed||repeat!=="off"||index!==queue.length-1||["idle","failed"].includes(status)){
      untrack(()=>{preparedRadioKey="";queueRadio.cancel();});return;
    }
    if(status==="playing"&&key!==preparedRadioKey)untrack(()=>{
      preparedRadioKey=key;queueRadio.prepare(seed,queue,historyItems.slice(0,200).map(item=>item.track));
    });
    if(status==="ended"&&nativeNaturalQueueEnd)untrack(()=>void continueQueueRadio(seed,true));
  });
  async function continueQueueRadio(seed:Track,natural:boolean):Promise<boolean> {
    if(!autoQueueRadio||consumingRadio||repeatMode!=="off")return false;
    consumingRadio=true;
    const generation=nativePlaybackGeneration;
    const runGeneration=playbackRunGeneration;
    const queueGeneration=playbackQueueGeneration;
    const queueKey=radioQueueKey;
    try{
      const more=await queueRadio.take(seed,queueTracks,historyItems.slice(0,200).map(item=>item.track),natural,repeatMode);
      if(!more.length||!autoQueueRadio||repeatMode!=="off"||playingTrack?.id!==seed.id||
        ((nativePlaybackStatus!=="ended"||!nativeNaturalQueueEnd))||
        generation!==nativePlaybackGeneration||runGeneration!==playbackRunGeneration||queueGeneration!==playbackQueueGeneration||queueKey!==radioQueueKey)return false;
      const index=queueTracks.length,list=[...queueTracks,...more];
      await startPreview(more[0],false,false,list,index);
      return true;
    }finally{consumingRadio=false;}
  }
  const playbackQueue = new PlaybackQueue();
  let playbackSession = "", recordedSession = "";
  const PLAYBACK_POSITION_KEY = "burrata.playback-position.v1";
  let lastNativeBookmarkOwner: NativePositionOwner | null = null;
  let lastNativeQueueCheckpoint: {
    revision: number | null;
    index: number;
    entryId: string | null;
    repeat: string;
    trackIds: string[];
    entryIds: string[];
    contextEntryIds: string[];
  } | null = null;
  let recentTrackFailures = $state<PlaybackTrackFailure[]>([]);
  let failureHistoryQueueRevision = "", failureHistorySourceKey = "", failureHistoryRevision = 0;
  function resetTrackFailureHistoryForQueue(revision: string, sourceKey: string) {
    // New occurrences and source edits reset history; position/index changes retain it.
    if (failureHistoryQueueRevision !== revision || failureHistorySourceKey !== sourceKey) {
      failureHistoryQueueRevision = revision;
      failureHistorySourceKey = sourceKey;
      recentTrackFailures = [];
      failureHistoryRevision++;
    }
  }
  type FailedQueueRun = {entryIds: string[]; sourceKey: string; visited: Set<string>; sourceRefreshRetries: Set<string>};

  let resumeTrackId = "", resumePosition = 0, pendingResumePosition = 0;

  let playingCandidate = $state<Candidate | null>(null), splitChapters = $state(false), selectedChapterIds = $state<string[]>([]);
  let focusedDownloadChapterId = $state<string | null>(null);
  let mirrorStatus = $state("");
  let spotifyArtistId = $state("");
  let spotifyAlbumId = $state("");
  type SpotifyTrackArtistRef = { provider: "spotify"; id: string; name: string };
  let spotifyTrackArtistChoices = $state<SpotifyTrackArtistRef[]>([]);
  type SpotifyArtistSearchFallback = { id: string; title: string; url: string | null; artworkUrl: string | null };
  let spotifyArtistSearchFallback = $state<SpotifyArtistSearchFallback | null>(null);
  let artistScroll = $state(0);
  let artistSection = $state<"overview" | "releases" | "tracks" | "collaborations" | "local">("overview");
  type Route = { libraryBrowser?: LibraryBrowserState; searchReleaseSnapshotKey?:string|null; searchNavigationSnapshotKey?:string|null; nativeArtistSnapshotKey?:string|null; spotifyNavigationSnapshotKey?:string|null; directorySnapshot?: DirectoryBrowserSnapshot; directoryLocation?: {rootId:string;relativePath:string}|null; view: View; collectionId: string; provider: string; genreUrl: string; query: string; filter: string; localFilter: "all" | "local" | "not-local"; sort: string; descending: boolean; librarySource: string; searchCategory: string; searchMode: typeof searchMode; importId?: string; artistName: string; artistRef: ArtistRef | null; artistTrack: Track | null; artistSection: typeof artistSection; artistScroll: number; loadedTrackCount?:number; durationMinimum?:string; durationMaximum?:string; spotifyArtistId?:string; spotifyAlbumId?:string; spotifyArtistSearchFallback?: SpotifyArtistSearchFallback | null; remoteOwner?:string; remotePlaylistUrl?:string; chartId?: string; historyQuery?: string; historyProvider?: string; historyStationId?: string; historyFrom?: string; historyTo?: string; historyOrder?: string; historySortField?: TrackSortField; historySortDirection?: SortDirection; historyMinDuration?: string; historyMaxDuration?: string; historyDownloadAvailability?: "all" | "downloaded" | "not-downloaded"; historyLoadedCount?: number; historyScroll?: number; historyFocusedSessionId?: string | null; focusedPlaylistRowKey?: string | null; selected: Track | null; scroll: number };
  let backRoutes = $state<Route[]>([]), forwardRoutes = $state<Route[]>([]);
  let restoringRoute = $state(false);
  let routeRestorationEpoch = 0;
  const artworkCache = new Map<string, { path: string; url: string }>();
  const artworkFilePaths = new Map<string, string>();
  let radioBusy = $state<string | null>(null);
  let radioRequest = 0;
  let failedArtwork = $state<Record<string, boolean>>({});
  const artworkDisplayCache = new ArtworkDisplayCache(()=>[
    playingTrack?.artworkUrl,showPlayingInfo?(infoTrack??playingTrack)?.artworkUrl:undefined,
  ].filter((url):url is string=>typeof url==="string"));
  let artworkDisplayActive = true;
  let artworkDisplayUrls = $state<Record<string,string>>({});
  let playingTrack = $state<Track | null>(null),
    preview = $state<Preview | null>(null),
    previewStatus = $state(""),
    paused = $state(true),
    position = $state(0),
    duration = $state(0),
    volume = $state(0.8),
    table = $state<HTMLDivElement>();
  const activeDetectedArtwork = $derived(playingTrackDetectionView?.enabled && samePlayingTrackDetectionOwner(playingTrackDetectionOwner, currentPlayingTrackDetectionOwner()) ? activeDetectedTrack(playingTrackDetectionView.observations, currentDetectionPositionSeconds())?.artworkUrl ?? null : null);
  type LoudnessSettingsValue = {
    mode: "off" | "track" | "album";
    targetLufs: number;
    ceilingDbtp: number;
    maximumBoostDb: number;
    albumTrackFallback: boolean;
    analyzeWhenIdle: boolean;
  };

  let loudnessSettings = $state<LoudnessSettingsValue>({
    mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false,
  });
  let loudnessSettingsLoaded = false;

  let queueUiController = $state<AbortController|null>(null);
  let queueUiRequestId:string|null=null;
  let queueUiMove=$state<((from:number,to:number)=>Promise<void>)|null>(null);
  let nativeSnapshot = $state<NativePlaybackSnapshot | null>(null);
  const currentLyricsTrack = $derived.by(() => {
    const owner=currentPlayingTrackDetectionOwner(), view=playingTrackDetectionView;
    const observation=owner && view?.enabled && samePlayingTrackDetectionOwner(owner,playingTrackDetectionOwner)
      ? activeDetectedTrack(view.observations,currentDetectionPositionSeconds()) : null;
    const detectedLyrics=playingTrack && observation
      ? {...playingTrack,id:`${playingTrack.id}:detected:${observation.detectedAtSeconds}`,title:observation.title,artist:observation.artist,album:null,durationSeconds:0,localPath:null,assetId:null,playbackRange:undefined,provenance:{...((playingTrack.provenance as Record<string,unknown>|undefined)??{}),activeLyricsDetection:{detectedAtSeconds:observation.detectedAtSeconds}}}
      : null;
    if(detectedLyrics)return detectedLyrics;
    const chapter=(nativeCanonicalChapter);
    const detected=playingTrack && !playingRadioStation && !playingTrack.playbackRange && playingCandidate && chapter
      ? virtualChapterTrack(playingTrack,playingCandidate,chapter)
      : null;
    return lyricsTrackWithPlaybackDuration(currentRadioSong ?? detected ?? playingTrack, nativeSnapshot);
  });
  let liveBpmEstimate = $state<LiveBpmEstimate | null>(null);
  let liveKeyEstimate = $state<LiveKeyEstimate | null>(null);
  let persistedLiveMeasurements = $state<LiveMeasurementCacheRecord | null>(null);
  const lastLiveBpm = $derived(persistedLiveMeasurements?.bpm && persistedLiveMeasurements.bpm.positionSeconds !== null
    ? { value: persistedLiveMeasurements.bpm.value.bpm, measuredAt: persistedLiveMeasurements.bpm.measuredAt, positionSeconds: persistedLiveMeasurements.bpm.positionSeconds }
    : null);
  const lastLiveKey = $derived(persistedLiveMeasurements?.key && persistedLiveMeasurements.key.positionSeconds !== null
    ? { value: persistedLiveMeasurements.key.value.key, measuredAt: persistedLiveMeasurements.key.measuredAt, positionSeconds: persistedLiveMeasurements.key.positionSeconds }
    : null);
  let activeLiveMeasurementResourceToken: string | null = null;
  let lastNativeQueue: Track[] | null = null;
  const nativeBridge = new NativePlaybackBridge(call, listen);

  let playbackControlsOwned = $state(false);
  let playbackStartupError = $state<string | null>(null);
  let retryPlaybackStartup = $state<(() => void) | null>(null);

  const liveBpmTrackId = $derived(playingTrack?.id ?? null);
  const liveBpmGeneration = $derived(nativeSnapshot?.generation ?? null);
  const liveBpmNativeSeekSerial = $derived(nativeSnapshot?.seekSerial ?? 0);
  const liveBpmCanPoll = $derived(settings.liveBpmEnabled === true && nativeSnapshot?.engine === "native" && nativeSnapshot.status === "playing" && !paused && Boolean(liveBpmTrackId));
  function currentLiveMeasurementResource(): LiveMeasurementResource | null {
    const track = playingTrack;
    if (!track || track.playbackRange) return null;
    const activeAssetId = (nativeSnapshot?.quality?.sourceKind === "localAsset")
      ? nativeSnapshot.quality.assetId
      : (null);
    if (activeAssetId) {
      const asset = scopedAssetsById.get(activeAssetId) ?? trackAsset(track);
      if (asset?.id === activeAssetId && /^[a-f0-9]{64}$/i.test(asset.hash)) {
        return { kind: "localAsset", assetId: asset.id, fileHash: asset.hash, durationSeconds: asset.durationSeconds ?? undefined, recordingId: asset.recordingId };
      }
      return null;
    }
    const stream = currentObservedStreamIdentity();
    const durationSeconds = (nativeSnapshot?.duration || nativeSnapshot?.candidate?.durationSeconds || track.durationSeconds || 0);
    return stream && Number.isFinite(durationSeconds) && durationSeconds > 0
      ? { kind: "stream", provider: stream.provider as "youtube" | "soundcloud" | "bandcamp" | "monochrome", sourceId: stream.sourceId, durationSeconds, recordingId: track.id }
      : null;
  }
  const liveMeasurementResource = $derived.by(() => currentLiveMeasurementResource());
  const liveMeasurementResourceToken = $derived.by(() => liveMeasurementResource ? JSON.stringify(liveMeasurementResource) : null);
  const liveMeasurementCacheApi: LiveMeasurementCacheApi = {
    read: (namespace, key) => call<unknown>("read_analysis_cache_record", { namespace, key }),
    write: (namespace, key, payload, recordingId) => call("write_analysis_cache_record", { namespace, key, payload, recordingId: recordingId ?? null }),
  };
  $effect(() => {
    // Snapshot objects change with native position pulses; only a different
    // stable audio identity should clear the retained display or reload it.
    const token = liveMeasurementResourceToken;
    const resource = untrack(() => liveMeasurementResource);
    activeLiveMeasurementResourceToken = token;
    persistedLiveMeasurements = null;
    if (!resource || !token) return;
    let active = true;
    void readLiveMeasurementCache(resource, liveMeasurementCacheApi).then(record => {
      if (active && activeLiveMeasurementResourceToken === token) persistedLiveMeasurements = record;
    });
    return () => { active = false; if (activeLiveMeasurementResourceToken === token) activeLiveMeasurementResourceToken = null; };
  });
  async function persistLiveMeasurementUpdate(
    resource: LiveMeasurementResource | null,
    estimates: { bpm?: LiveBpmEstimate | null; key?: LiveKeyEstimate | null },
    positionSeconds: number | null,
  ): Promise<void> {
    if (!resource || (!estimates.bpm && !estimates.key)) return;
    const resourceToken = JSON.stringify(resource);
    const now = Date.now();
    // Save only metrics accepted in this window. Throttling the pair together
    // can lose a first key arriving shortly after BPM, or re-date an older key.
    const saved = await saveLiveMeasurements(resource, estimates, new Date(now).toISOString(), positionSeconds, liveMeasurementCacheApi);
    if (!saved || activeLiveMeasurementResourceToken !== resourceToken) return;
    const record = await readLiveMeasurementCache(resource, liveMeasurementCacheApi);
    if (activeLiveMeasurementResourceToken === resourceToken) persistedLiveMeasurements = record;
  }
  const waveformTrackId = $derived(playingTrack?.id ?? null);
  const waveformGeneration = $derived(nativeSnapshot?.generation ?? null);
  const cachedStreamAnalysisTrackId = $derived(playingTrack?.id ?? null);
  const cachedStreamAnalysisGeneration = $derived(nativeSnapshot?.engine === "native" ? nativeSnapshot.generation : null);
  const cachedStreamAnalysisResource = $derived(currentStreamMediaResource());
  const cachedStreamAnalysisCurrentOwnerKey = $derived(`${cachedStreamAnalysisTrackId ?? ""}:${cachedStreamAnalysisGeneration ?? ""}:${cachedStreamAnalysisResource?.key ?? ""}`);
  const cachedStreamAnalysisEligible = $derived.by(() => {
    const track = playingTrack;
    const provider = track?.provider;
    const radioTrack = provider === "radioparadise" || provider === "internet-radio" ||
      provider === "radio" && (track?.provenance as { sourceKind?: string } | undefined)?.sourceKind === "radio";
    return nativeSnapshot?.engine === "native" &&
      ["buffering", "playing", "paused", "ended"].includes(nativeSnapshot.status) &&
      !radioTrack && !track?.playbackRange;
  });
  async function readCachedStreamAnalysis(trackId: string, generation: number, resource: ActiveStreamMediaResource, expectedRevision?: number) {
    try {
      const result = await call<WholeTrackAnalysisDto | null>("get_cached_stream_analysis", { trackId, generation });
      const active = currentStreamMediaResource();
      if (expectedRevision !== undefined && cachedStreamAnalysisEventRevision !== expectedRevision) return;
      if (!result || active?.key !== resource.key || playingTrack?.id !== trackId || nativeSnapshot?.generation !== generation) return;
      const digest = await streamSourceIdentitySha256(resource.provider, resource.sourceId);
      const activeVariantDigest = await streamAudioVariantSha256(resource.audioVariant);
      if (expectedRevision !== undefined && cachedStreamAnalysisEventRevision !== expectedRevision) return;
      if (currentStreamMediaResource()?.key !== resource.key || playingTrack?.id !== trackId ||
          nativeSnapshot?.generation !== generation || result.scope !== "wholeTrack" || result.resource?.kind !== "stream" ||
          result.resource.recordingId !== trackId || result.resource.provider !== resource.provider ||
          result.resource.sourceIdentitySha256 !== digest || (result.resource.audioVariantSha256 ?? null) !== activeVariantDigest || !Number.isFinite(result.durationSeconds) ||
          !(8 <= result.durationSeconds && result.durationSeconds <= 900) || (!result.bpm && !result.key)) return;
      cachedStreamAnalysis = result;
      cachedStreamAnalysisOwnerKey = `${trackId}:${generation}:${resource.key}`;
    } catch {
      // An optional cache read must not interrupt playback or track changes.
    }
  }
  async function applyStreamAnalysisUpdated(event: StreamAnalysisUpdatedEvent) {
    const resource = currentStreamMediaResource();
    if (!resource || event.trackId !== playingTrack?.id || event.generation !== nativeSnapshot?.generation ||
        event.provider !== resource.provider || event.sourceId !== resource.sourceId || !event.resourceIdentity) return;
    const digest = await streamSourceIdentitySha256(resource.provider, resource.sourceId);
    const activeVariantDigest = await streamAudioVariantSha256(resource.audioVariant);
    if (currentStreamMediaResource()?.key !== resource.key || playingTrack?.id !== event.trackId ||
        nativeSnapshot?.generation !== event.generation || event.sourceIdentitySha256 !== digest ||
        (event.audioVariantSha256 ?? null) !== activeVariantDigest) return;
    // Only an event proven to belong to the active resource may fence a cache
    // read. Unrelated stale events must not strand the current initial lookup.
    const revision = ++cachedStreamAnalysisEventRevision;
    const ownerKey = `${event.trackId}:${event.generation}:${resource.key}`;
    if (event.state === "started") {
      cachedStreamAnalysis = null;
      cachedStreamAnalysisOwnerKey = ownerKey;
      return;
    }
    if (event.analysis) {
      const analysis = event.analysis;
      if (analysis.scope === "wholeTrack" && analysis.resource?.kind === "stream" &&
          analysis.resource.recordingId === event.trackId && analysis.resource.provider === event.provider &&
          analysis.resource.sourceIdentitySha256 === event.sourceIdentitySha256 &&
          (analysis.resource.audioVariantSha256 ?? null) === activeVariantDigest && (analysis.bpm || analysis.key)) {
        cachedStreamAnalysis = analysis;
        cachedStreamAnalysisOwnerKey = ownerKey;
        return;
      }
    }
    await readCachedStreamAnalysis(event.trackId, event.generation, resource, revision);
  }
  const waveformLocalAssetId = $derived(nativeSnapshot?.quality?.sourceKind === "localAsset" ? nativeSnapshot.quality.assetId ?? null : null);
  const playbackWaveformForDisplay = $derived.by(() => {
    if (playbackWaveform?.source === "local-analysis") {
      const binding = verifiedLocalWaveformBinding;
      if (!binding || binding.trackId !== waveformTrackId || binding.assetId !== waveformLocalAssetId) return null;
    }
    if (playbackWaveform?.source === "stream-analysis") {
      const resource = currentStreamMediaResource();
      const binding = playbackWaveform.resource;
      if (!resource || !binding || resource.trackId !== waveformTrackId || resource.trackId !== playbackWaveform.trackId ||
          resource.provider !== binding.provider || resource.sourceId !== binding.sourceId || binding.bindingKey !== resource.key) return null;
    }
    return playbackWaveform;
  });
  function currentStreamWaveformMatchesPlayback(): boolean {
    const waveform = playbackWaveform;
    const resource = currentStreamMediaResource();
    return Boolean(waveform?.source === "stream-analysis" && waveform.complete && waveform.resource && resource &&
      waveform.trackId === resource.trackId && waveform.resource.provider === resource.provider && waveform.resource.sourceId === resource.sourceId &&
      waveform.resource.bindingKey === resource.key);
  }
  const completeStreamWaveformVisible = $derived.by(() => waveformEnabled && playbackWaveformForDisplay?.source === "stream-analysis" &&
    playbackWaveformForDisplay.complete && currentStreamWaveformMatchesPlayback());
  const playbackDetectionMarkers = $derived.by(() => {
    const owner = currentPlayingTrackDetectionOwner();
    const view = playingTrackDetectionView;
    if (!detectionMarkersEnabled || !owner || !samePlayingTrackDetectionOwner(owner, playingTrackDetectionOwner) || !view) return [];
    return view.observations;
  });
  // Detection records use the parent source clock for both playback engines,
  // while an excerpt scrubber starts at zero.
  const playbackDetectionMarkerSourceOffset = $derived(playingTrack?.playbackRange?.startTime ?? 0);
  let liveBpmPollStatus = $state<"warming" | "estimating" | "insufficient" | "unavailable" | "unsupported" | "waiting">("warming");
  let liveBpmPollReason = $state<string | null>(null);

  $effect(() => {
    const trackId = liveBpmTrackId;
    const generation = liveBpmGeneration;
    const measurementToken = liveMeasurementResourceToken;
    const measurementResource = untrack(() => liveMeasurementResource);
    const serial = seekSerial;
    const nativeSerial = liveBpmNativeSeekSerial;
    const shouldPoll = liveBpmCanPoll;
    if (!shouldPoll || generation === null || trackId === null) {
      liveBpmEstimate = null;
      liveKeyEstimate = null;
      liveBpmPollStatus = "warming";
      liveBpmPollReason = null;
      return;
    }
    let active = true;
    let inFlight = false;
    const startedAt = Date.now();
    liveBpmPollStatus = "warming";
    liveBpmPollReason = null;
    const poll = async () => {
      if (!active || inFlight) return;
      inFlight = true;
      try {
        const estimates = await call<LivePlaybackEstimates | null>("get_live_playback_estimates");
        const latestTrackId = playingTrack?.id ?? null;
        const latestGeneration = nativeSnapshot?.generation ?? null;
        const latestOwner = { generation: latestGeneration ?? -1, seekSerial: nativeSnapshot?.seekSerial ?? 0, trackId: latestTrackId ?? "" };
        if (!active || latestTrackId !== trackId || latestGeneration !== generation || seekSerial !== serial || (nativeSnapshot?.seekSerial ?? 0) !== nativeSerial || paused || !true || nativeSnapshot?.status !== "playing") return;
        const bpm = estimates?.bpm ?? null;
        const key = estimates?.key ?? null;
        const validBpm = isCurrentLiveBpmEstimate(bpm, latestOwner) ? bpm : null;
        const validKey = isCurrentLiveKeyEstimate(key, latestOwner) ? key : null;
        liveBpmEstimate = validBpm;
        liveKeyEstimate = validKey;
        if (validBpm || validKey) {
          void persistLiveMeasurementUpdate(measurementResource, { bpm: validBpm, key: validKey }, nativeSnapshot?.position ?? null);
        }
        liveBpmPollStatus = validBpm || validKey ? "estimating"
          : Date.now() - startedAt < 15000 ? "warming" : "insufficient";
      } catch {
        if (active) { liveBpmEstimate = null; liveKeyEstimate = null; liveBpmPollStatus = "unavailable"; }
      } finally { inFlight = false; }
    };
    void poll();
    // Twelve-second analysis windows need only half-window refreshes.
    const timer = window.setInterval(() => void poll(), 6000);
    return () => {
      active = false;
      window.clearInterval(timer);
      liveBpmEstimate = null;
      liveKeyEstimate = null;
    };
  });
  let lastSourceFailureKey="";
  let lastSkippedTrackFailureKey="";
  function safeNativeFailurePart(value: unknown, fallback: string): string {
    if (typeof value !== "string" || !value.trim()) return fallback;
    return value.replace(/[\u0000-\u001f\u007f]/g, " ")
      .replace(/\b(?:https?|ftp):\/\/[^\s<>"']+/gi, "[source]")
      .replace(/\b(bearer|token|access[_-]?token|refresh[_-]?token|password|authorization|api[_-]?key)\b\s*[:=]?\s*[^\s,;]+/gi, "$1=[redacted]")
      .replace(/(?:[A-Za-z]:\\|\/(?:home|Users|tmp|var|private|Volumes)\/)[^\s<>"']+/g, "[path]")
      .trim().slice(0, 180) || fallback;
  }
  function nativeFailureMessage(failure: NativePlaybackFailure | null | undefined): string {
    if (!failure) return "";
    const artist = safeNativeFailurePart(failure.artist, "Unknown artist");
    const title = safeNativeFailurePart(failure.title, "Unknown recording");
    const reason = safeNativeFailurePart(failure.reason, "No playable audio source was found. Check local files or choose another source.");
    return `Could not play ${artist} — ${title}: ${reason}`;
  }
  function applyNativeSnapshot(snapshot: NativePlaybackSnapshot) {
    const previousNativeSnapshot = nativeSnapshot;
    nativeSnapshot = snapshot;
    const failure=snapshot.diagnostics?.lastSourceFailure;
    const skippedFailure=snapshot.diagnostics?.lastSkippedTrackFailure;
    const failureMessage=nativeFailureMessage(failure);
    if(failure){lastSourceFailureKey=failureMessage;if(snapshot.status === "failed")clearPlaybackSkipNotice();}
    else lastSourceFailureKey="";
    if(skippedFailure){
      const skippedMessage=nativeFailureMessage(skippedFailure);
      const isNew=skippedMessage!==lastSkippedTrackFailureKey;
      lastSkippedTrackFailureKey=skippedMessage;
      if(!failure && isNew)setNotice(skippedMessage, "persistent", "playbackSkip");
    }else lastSkippedTrackFailureKey="";
    nativeEngineEnabled=snapshot.engine==="native";nativePlaybackStatus=snapshot.status;nativePlaybackGeneration=snapshot.generation;nativeNaturalQueueEnd=snapshot.naturalQueueEnd===true;
    if (snapshot.engine !== "native") return;
    const snapshotTrack = snapshot.track ? {...snapshot.track,favorite:favoriteTrackIds.includes(snapshot.track.id)} : null;
    const oldTrack = playingTrack?.id;
    if(oldTrack !== snapshotTrack?.id) { currentRadioSong=null; radioOccurrence=null; }
    const radio = snapshot.radioNowPlaying as {id?:string;title?:string;artist?:string;stationName?:string;track?:Track|null;metadata?:RadioTrackMetadata|null}|null|undefined;
    const radioMetadata = radio?.metadata;
    playingTrack = snapshotTrack && radioMetadata ? {
      ...snapshotTrack,
      title:radioMetadata.title || radio?.title || snapshotTrack.title, artist:radioMetadata.artist || radio?.artist || snapshotTrack.artist,
      album:radioMetadata.album ?? snapshotTrack.album, artworkUrl:radioMetadata.artworkUrl ?? snapshotTrack.artworkUrl,
      sourceUrl:radioMetadata.sourceUrl || snapshotTrack.sourceUrl,
      provenance:{...(snapshotTrack.provenance as Record<string,unknown>),radioStationName:radio?.stationName,radioMetadata,radioStationId:radioMetadata.stationId},
    } : snapshotTrack;
    const radioSong = typeof radio?.id === "string" ? radioSongTrackFromSnapshot({...radio, id: radio.id}) : null;
    currentRadioSong = snapshotTrack && isStationPlaybackTrack(snapshotTrack) && radioSong
      ? { ...radioSong, favorite: favoriteTrackIds.includes(radioSong.id) }
      : null;
    radioQueueHistory = (snapshot.radioHistory ?? []).flatMap((item) => {
      if (!item || typeof item !== "object") return [];
      const entry = item as {id?:unknown;track?:unknown;playedAt?:unknown;stationName?:unknown};
      if (typeof entry.id !== "string" || !entry.track || typeof entry.track !== "object") return [];
      return [{id:entry.id,track:entry.track as Track,playedAt:typeof entry.playedAt === "string" ? entry.playedAt : "",stationName:typeof entry.stationName === "string" ? entry.stationName : "Radio"}];
    });
    playingCandidate = snapshot.candidate;
    if (lastNativeQueue !== snapshot.queue) {
      lastNativeQueue = snapshot.queue;
      const favorites = new Set(favoriteTrackIds);
      queueTracks = snapshot.queue.map(track=>({...track,favorite:favorites.has(track.id)}));
    }
    if (Array.isArray(snapshot.queueEntryIds) && snapshot.queueEntryIds.length === snapshot.queue.length) {
      playbackQueue.restore({
        tracks: snapshot.queue, entryIds: snapshot.queueEntryIds, currentIndex: snapshot.currentIndex,
        repeat: snapshot.repeat === "all" || snapshot.repeat === "one" ? snapshot.repeat : "off",
        ...(snapshot.queueLoudnessContexts ? { queueLoudnessContexts: snapshot.queueLoudnessContexts } : {}),
      });
    }
    queueIndex = snapshot.currentIndex;
    persistNativeQueueCheckpoint(snapshot);
    paused = snapshot.paused; position = snapshot.position; duration = snapshot.duration;
    volume = snapshot.volume; repeatMode = (snapshot.repeat === "all" || snapshot.repeat === "one" ? snapshot.repeat : "off");
    shuffleEnabled = snapshot.shuffle;
    bufferingAudio = snapshot.status === "buffering";
    resolvingPlayback = snapshot.status === "resolving";
    previewStatus = failureMessage || snapshot.playerMessage || (snapshot.status === "paused" ? "Paused" : snapshot.status === "playing" ? "Playing" : "");
    preview = snapshot.track ? { kind: "native", url: snapshot.track.sourceUrl, canSeek: snapshot.canSeek, canVolume: snapshot.canVolume, canAutoplay: true, isExcerpt: Boolean(snapshot.track.playbackRange), provider: snapshot.candidate?.provider, audioCodec: snapshot.quality?.codec, audioBitrateKbps: snapshot.quality?.bitrateKbps } : null;
    persistNativePositionBookmark(snapshot, previousNativeSnapshot);
    if (oldTrack !== snapshot.track?.id && snapshot.track?.artworkUrl) void cacheCover(snapshot.track);
    if (snapshotTrack) selected = snapshotTrack;

    // Native history is recorded by Rust at the first real output audio frame.
    // Refresh once for this generation/queue occurrence, after the write has
    // had a chance to finish; the delayed callback rechecks the same owner.
    const firstAudioMs = snapshot.diagnostics?.firstAudioMs;
    const trackId = snapshot.trackId;
    const owner = snapshot.status === "playing" && !snapshot.paused && firstAudioMs != null && typeof trackId === "string"
      ? JSON.stringify([snapshot.generation, snapshot.entryId, trackId, snapshot.diagnostics?.operationId])
      : "";
    if (pendingNativeHistorySummaryOwner && owner !== pendingNativeHistorySummaryOwner) {
      if (nativeHistorySummaryTimer !== null) clearTimeout(nativeHistorySummaryTimer);
      nativeHistorySummaryTimer = null;
      pendingNativeHistorySummaryOwner = "";
    }
    if (owner && typeof trackId === "string" && owner !== lastNativeHistorySummaryOwner && owner !== pendingNativeHistorySummaryOwner) {
      const generation = snapshot.generation, entryId = snapshot.entryId, ownedTrackId = trackId;
      pendingNativeHistorySummaryOwner = owner;
      nativeHistorySummaryTimer = setTimeout(() => {
        nativeHistorySummaryTimer = null;
        if (pendingNativeHistorySummaryOwner !== owner) return;
        pendingNativeHistorySummaryOwner = "";
        lastNativeHistorySummaryOwner = owner;
        void refreshTrackLibrarySummaryFor(ownedTrackId, () => {
          const current = nativeSnapshot;
          return current?.engine === "native" &&
            current.generation === generation && current.entryId === entryId && current.trackId === ownedTrackId &&
            current.diagnostics?.firstAudioMs != null;
        });
      }, 200);
    }
  }
  function applyNativePosition(event: NativePositionEvent) {
    if (nativeSnapshot && (event.chapter !== undefined && JSON.stringify(event.chapter) !== JSON.stringify(nativeSnapshot.chapter) ||
        event.quality !== undefined && JSON.stringify(event.quality) !== JSON.stringify(nativeSnapshot.quality))) nativeSnapshot = {
      ...nativeSnapshot,
      ...(event.revision !== undefined ? {revision:event.revision} : {}),
      ...(event.chapter !== undefined ? {chapter:event.chapter} : {}),
      ...(event.quality !== undefined ? {quality:event.quality} : {}),
    };
    position = event.position;
    if (event.duration !== undefined) duration = event.duration;
    if (event.buffering !== undefined) bufferingAudio = event.buffering;
    if (nativeSnapshot?.engine === "native" && nativeSnapshot.generation === event.generation) {
      persistNativePositionBookmark({ ...nativeSnapshot, position: event.position, ...(event.duration !== undefined ? { duration: event.duration } : {}) });
    }
  }
  function persistNativeQueueCheckpoint(snapshot: NativePlaybackSnapshot) {
    if (snapshot.engine !== "native" ||
        !Array.isArray(snapshot.queueEntryIds) || snapshot.queueEntryIds.length !== snapshot.queue.length ||
        !Number.isInteger(snapshot.currentIndex) || (snapshot.queue.length > 0
          ? snapshot.currentIndex < 0 || snapshot.currentIndex >= snapshot.queue.length
          : snapshot.currentIndex !== -1 && snapshot.currentIndex !== 0)) return;
    const isEmpty = snapshot.queue.length === 0;
    const currentEntryId = isEmpty ? null : snapshot.entryId ?? snapshot.queueEntryIds[snapshot.currentIndex];
    if (!isEmpty && (!currentEntryId || snapshot.queueEntryIds[snapshot.currentIndex] !== currentEntryId)) return;
    const revision = Number.isSafeInteger(snapshot.queueRevision) && Number(snapshot.queueRevision) >= 0
      ? Number(snapshot.queueRevision) : null;
    const repeat = snapshot.repeat === "all" || snapshot.repeat === "one" ? snapshot.repeat : "off";
    const previous = lastNativeQueueCheckpoint;
    const sameRevisionOwner = previous !== null && revision !== null && previous.revision === revision &&
      previous.index === snapshot.currentIndex && previous.entryId === currentEntryId && previous.repeat === repeat;
    if (sameRevisionOwner) return;
    const contextEntryIds = Object.keys(snapshot.queueLoudnessContexts ?? {}).sort();
    const sameLegacyOwner = previous !== null && revision === null && previous.revision === null &&
      previous.index === snapshot.currentIndex && previous.entryId === currentEntryId && previous.repeat === repeat &&
      previous.trackIds.length === snapshot.queue.length && previous.trackIds.every((id, index) => id === snapshot.queue[index]?.id) &&
      previous.entryIds.length === snapshot.queueEntryIds.length && previous.entryIds.every((id, index) => id === snapshot.queueEntryIds?.[index]) &&
      previous.contextEntryIds.length === contextEntryIds.length && previous.contextEntryIds.every((id, index) => id === contextEntryIds[index]);
    if (sameLegacyOwner) return;
    if (!savePlaybackQueue(playbackQueue)) return;
    lastNativeQueueCheckpoint = {
      revision, index: snapshot.currentIndex, entryId: currentEntryId, repeat,
      trackIds: snapshot.queue.map(track => track.id), entryIds: [...snapshot.queueEntryIds], contextEntryIds,
    };
  }
  function persistNativePositionBookmark(snapshot: NativePlaybackSnapshot, previous: NativePlaybackSnapshot | null = nativeSnapshot) {
    if (snapshot.engine !== "native" || !snapshot.track || playingRadioStation || snapshot.radioNowPlaying ||
        typeof snapshot.entryId !== "string" || !snapshot.entryId) return;
    const track = snapshot.track;
    // Native chapter positions are relative to their chapter; the WebView
    // startup bookmark is absolute within the source recording.
    const absolutePosition = chapterAbsolutePosition(track, snapshot.position);
    const now = Date.now();
    const nativeSeekSerial = Number.isSafeInteger(snapshot.seekSerial) && Number(snapshot.seekSerial) >= 0
      ? Number(snapshot.seekSerial) : 0;
    const checkpoint = {
      trackId: track.id, entryId: snapshot.entryId, generation: snapshot.generation,
      seekSerial: nativeSeekSerial, position: absolutePosition, now,
      paused: snapshot.paused, radio: false,
    };
    const force = (snapshot.paused && previous?.paused !== true) ||
      (previous?.seekSerial !== undefined && nativeSeekSerial !== previous.seekSerial) ||
      (previous?.entryId !== undefined && snapshot.entryId !== previous.entryId);
    if (!shouldPersistNativePositionBookmark(checkpoint, lastNativeBookmarkOwner, force)) return;
    try {
      localStorage.setItem(PLAYBACK_POSITION_KEY, JSON.stringify(createNativePositionBookmark(checkpoint)));
      lastNativeBookmarkOwner = {
        trackId: track.id, entryId: snapshot.entryId, generation: snapshot.generation,
        seekSerial: nativeSeekSerial, savedAt: now, paused: snapshot.paused,
      };
      lastPersistedPlaybackPosition = { trackId: track.id, position: absolutePosition, savedAt: now };
    } catch { /* Storage quota must not interrupt native playback. */ }
  }
  async function nativeCommand(command: Parameters<NativePlaybackBridge["command"]>[0]) {
    try {
      await nativeBridge.command(command);
    } catch (failure) { error = String(failure); }
  }
  let scrollTop = $state(0),
    viewportHeight = $state(600);
  const rowHeight = 60,
    overscan = 8;

  const previewRequests = new LatestRequest();
  let nativeResolutionGeneration = $state<number | null>(null);
  let audioStartupGeneration = 0;
  let artistPlaybackMatchGeneration = 0;
  let artistPlaybackMatchRevision = 0;
  const preparedNoLocalArtistMatches = new Map<string, number>();
  function nextPreviewRequest() {

    artistPlaybackMatchGeneration++;
    const requestGeneration = previewRequests.next();
    nativeResolutionGeneration = null;
    audioStartupGeneration++;
    if (desktop()) void call("cancel_playback_resolution", {requestGeneration}).catch(() => {});
    return requestGeneration;
  }
  let audioRecovery: Promise<void> | null = null;
  let activeAutomationPlayback: string | null = null;
  const queuedPlaybackOwners = new Map<string, string>();
  const queuedPlaybackOutbox = createQueuedPlaybackOutbox((request, outcome) =>
    call<{ accepted: boolean }>("automation_queued_playback_update", {
      operationId: request.operationId,
      requestId: request.requestId,
      status: outcome.status,
      detail: outcome.detail,
    }),
  );
  type QueuedAcceptanceEnvelope = ReturnType<typeof automationPlaybackSnapshot>;
  const queuedAcceptanceWaiters = new Map<string, {
    requestId: string;
    resolve: (envelope: QueuedAcceptanceEnvelope) => void;
    reject: (error: Error) => void;
  }>();
  function rejectQueuedAcceptance(owner: string) {
    const waiter = queuedAcceptanceWaiters.get(owner);
    if (!waiter) return;
    queuedAcceptanceWaiters.delete(owner);
    waiter.reject(new Error("Playback acceptance was cancelled or superseded"));
  }

  let queuedPlaybackPrepareIntent: QueuedPlayRequest | null = null;
  let queuedPausedOwner: string | null = null;

  function retireQueuedPreparation(status: "cancelled" | "superseded") {
    const request = queuedPlaybackPrepareIntent;
    if (!request) return;
    queuedPlaybackPrepareIntent = null;
    void queuedPlaybackOutbox.publish(request, {
      status,
      detail: { currentGeneration: playbackRunGeneration },
    }).catch(() => {});
  }

  function queuedOwnerForResume(fallback: string | null = null): string | null {
    return activeAutomationPlayback && queuedPlaybackOwners.has(activeAutomationPlayback)
      ? activeAutomationPlayback : fallback;
  }

  function reportQueuedPlaybackPhase(status: "paused" | "playing" | "failed") {
    const owner = queuedOwnerForResume();
    if (!owner) return;
    const transport = queuedPlaybackOwners.get(owner)!;
    queuedPausedOwner = status === "paused" ? owner : null;
    void queuedPlaybackOutbox.publish({
      operationId: owner,
      requestId: transport,
      trackIds: [],
    }, {
      status,
      detail: {
        currentGeneration: playbackRunGeneration,
        entryId: playbackQueue.entryIds[queueIndex] ?? null,
        reason: status === "failed" ? previewStatus : null,
      },
    }).catch(() => {});
  }

  function setAutomationPlaybackOwner(owner: string | null, preservePreparation = false) {
    const previous = activeAutomationPlayback;
    if (!preservePreparation && queuedPlaybackPrepareIntent &&
        owner !== queuedPlaybackPrepareIntent.operationId && (previous !== owner || owner === null)) {
      retireQueuedPreparation(owner ? "superseded" : "cancelled");
    }
    activeAutomationPlayback = owner;
    if (previous !== owner && queuedPausedOwner === previous) queuedPausedOwner = null;
    if (previous && previous !== owner) {
      rejectQueuedAcceptance(previous);
      const transport = queuedPlaybackOwners.get(previous);
      if (transport) {
        queuedPlaybackOwners.delete(previous);
        void queuedPlaybackOutbox.publish({
          operationId: previous,
          requestId: transport,
          trackIds: [],
        }, {
          status: owner ? "superseded" : "cancelled",
          detail: { currentGeneration: playbackRunGeneration },
        }).catch(() => {});
      }
    }
  }

  function cancelQueuedPlayback(request: { operationId: string; requestId?: string }) {
    const preparing = queuedPlaybackPrepareIntent;
    if (preparing?.operationId === request.operationId &&
        (!request.requestId || preparing.requestId === request.requestId)) {
      retireQueuedPreparation("cancelled");
    }
    const transport = queuedPlaybackOwners.get(request.operationId);
    if (!transport || request.requestId && request.requestId !== transport ||
        activeAutomationPlayback !== request.operationId) return;
    nextPreviewRequest();
    paused = true;
    const sessionId = preview?.sessionId;
    // Cancelling an older owner must preserve a separately validated preparation.
    setAutomationPlaybackOwner(null, true);
  }

  async function handleQueuedPlayback(request: QueuedPlayRequest) {

    return; // Native queued jobs are owned by the backend.

  }

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
  let durationMinimum = $state(""), durationMaximum = $state("");
  const activeDurationFilterCount = $derived([durationMinimum, durationMaximum].filter(value => parseDurationBound(value) !== null).length);
  let localPageTotal = $state(0), localPageLoading = $state(false);
  let playlistPlaybackAction = $state<"play" | "queue" | null>(null);
  let playlistPlaybackRequest = 0;
  let localSortPending = $state(false);
  let localSortAvailability = $state<TrackSortAvailability | null>(null);
  let localSortAvailabilityPending = $state(false);
  const pendingSortAvailabilityRefresh = new PendingSortAvailabilityRefresh<TrackSortAvailability>();
  let localPageRequest = 0, localPageQueryRevision = 0;
  let localPrefixRefreshRevision = 0, localPrefixRefreshHandled = 0, localPrefixRefreshRunning = false;
  let localPrefixRefreshAll = false;
  const localPrefixUpdatedTracks = new Map<string, Track>();
  async function getTracksInBatches(ids: string[], isCurrent: () => boolean = () => true) {
    const uniqueIds = [...new Set(ids)], byId = new Map<string, Track>();
    const chunks = Array.from({length: Math.ceil(uniqueIds.length / 200)}, (_, index) =>
      uniqueIds.slice(index * 200, (index + 1) * 200));
    for (let index = 0; index < chunks.length; index += 4) {
      if (!isCurrent()) return byId;
      const pages = await Promise.all(chunks.slice(index, index + 4).map(ids => call<Track[]>("get_tracks", {ids})));
      for (const track of pages.flat()) byId.set(track.id, track);
      if (!isCurrent()) return byId;
    }
    return byId;
  }
  function carryLocalPrefixRefresh(updated: Track[], refreshAll: boolean) {
    for (const track of updated) if (!localPrefixUpdatedTracks.has(track.id)) localPrefixUpdatedTracks.set(track.id, track);
    localPrefixRefreshAll ||= refreshAll;
  }
  function requestLocalPrefixRefresh(updated?: Track, refreshCachedRows = false) {
    if (updated) localPrefixUpdatedTracks.set(updated.id, updated);
    localPrefixRefreshAll ||= refreshCachedRows;
    localPrefixRefreshRevision++;
    if (!localPrefixRefreshRunning) void drainLocalPrefixRefreshes();
  }
  async function drainLocalPrefixRefreshes() {
    localPrefixRefreshRunning = true;
    let missingRetrySignature = "", missingRetryCount = 0;
    try {
      while (localPrefixRefreshHandled < localPrefixRefreshRevision) {
        const revision = localPrefixRefreshRevision;
        localPrefixRefreshHandled = revision;
        const options = localPageOptions();
        if (!desktop() || !["library", "favorites", "collections"].includes(options.view)) continue;
        const signature = JSON.stringify(options), context = activeContext, pageRequest = localPageRequest;
        const count = tracks.length;
        const updated = [...localPrefixUpdatedTracks.values()];
        localPrefixUpdatedTracks.clear();
        const refreshAll = localPrefixRefreshAll;
        localPrefixRefreshAll = false;
        if (count === 0) { localPageRequest++; await loadLocalPage(false, options); continue; }
        if (localPageReads.size) {
          carryLocalPrefixRefresh(updated, refreshAll);
          await Promise.allSettled([...localPageReads.values()].map(read => read.promise));
          localPrefixRefreshRevision++;
          continue;
        }
        localPageLoading = true;
        const isCurrent = () => revision === localPrefixRefreshRevision && pageRequest === localPageRequest
          && JSON.stringify(localPageOptions()) === signature && activeContext === context;
        try {
          const prefix = await call<{rows:{listingId:string;playlistPosition:number|null}[];total:number}>("list_track_prefix", {...options,limit:count});
          if (revision !== localPrefixRefreshRevision) { carryLocalPrefixRefresh(updated, refreshAll); continue; }
          if (pageRequest !== localPageRequest) {
            carryLocalPrefixRefresh(updated, refreshAll);
            if (localPageReads.size) await Promise.allSettled([...localPageReads.values()].map(read => read.promise));
            localPrefixRefreshRevision++;
            continue;
          }
          if (JSON.stringify(localPageOptions()) !== signature || activeContext !== context || !Array.isArray(prefix?.rows)) continue;
          const cache = new Map(tracks.map(track => [track.id, track]));
          const refreshedById = new Map<string, Track>();
          const dirtyIds = new Set(updated.map(track => track.id));
          for (const row of prefix.rows) {
            const cached = cache.get(row.listingId);
            const changed = updated.find(track => track.id === row.listingId) ?? updated.find(track => cached && sameTrack(cached, track));
            if (changed) refreshedById.set(row.listingId, {...cached, ...changed, id: row.listingId});
          }
          const idsToFetch = prefix.rows.map(row => row.listingId).filter(id =>
            refreshAll || dirtyIds.has(id) || refreshedById.has(id) || !cache.has(id));
          const fetched = await getTracksInBatches(idsToFetch, isCurrent);
          if (revision !== localPrefixRefreshRevision) { carryLocalPrefixRefresh(updated, refreshAll); continue; }
          if (pageRequest !== localPageRequest) {
            carryLocalPrefixRefresh(updated, refreshAll);
            if (localPageReads.size) await Promise.allSettled([...localPageReads.values()].map(read => read.promise));
            localPrefixRefreshRevision++;
            continue;
          }
          if (JSON.stringify(localPageOptions()) !== signature || activeContext !== context) continue;
          for (const [id, track] of fetched) {
            const changed = updated.find(item => item.id === id) ?? updated.find(item => sameTrack(track, item));
            refreshedById.set(id, changed ? {...track, ...changed, id} : track);
          }
          const missing = idsToFetch.filter(id => !fetched.has(id));
          if (missing.length) {
            if (missingRetrySignature !== signature) { missingRetrySignature = signature; missingRetryCount = 0; }
            if (missingRetryCount === 0) {
              missingRetryCount++;
              carryLocalPrefixRefresh(updated, refreshAll);
              localPrefixRefreshRevision++;
              continue;
            }
            setNotice("The saved list changed during refresh; retry the view to restore its exact order.");
            missingRetryCount = 0;
            continue;
          }
          tracks = prefix.rows.map(row => {
            const track = refreshedById.get(row.listingId) ?? cache.get(row.listingId)!;
            const provenance: Record<string, unknown> = track.provenance && typeof track.provenance === "object" ? {...track.provenance as Record<string, unknown>} : {};
            if (row.playlistPosition == null) delete provenance.playlistPosition;
            else provenance.playlistPosition = row.playlistPosition;
            return {...track, provenance};
          });
          localPageTotal = prefix.total;
          missingRetryCount = 0;
          if (selected) {
            const selectedPosition = view === "collections" ? playlistPosition(selected) : null;
            selected = tracks.find(track => sameTrack(track, selected!) && (selectedPosition == null || playlistPosition(track) === selectedPosition))
              ?? tracks.find(track => sameTrack(track, selected!)) ?? selected;
          }
        } catch (failure) {
          if (revision !== localPrefixRefreshRevision) carryLocalPrefixRefresh(updated, refreshAll);
          else if (pageRequest !== localPageRequest) {
            carryLocalPrefixRefresh(updated, refreshAll);
            if (localPageReads.size) await Promise.allSettled([...localPageReads.values()].map(read => read.promise));
            localPrefixRefreshRevision++;
          } else error = String(failure);
        } finally {
          if (revision === localPrefixRefreshRevision && pageRequest === localPageRequest) localPageLoading = false;
        }
      }
    } finally {
      localPrefixRefreshRunning = false;
      if (localPrefixRefreshHandled < localPrefixRefreshRevision) void drainLocalPrefixRefreshes();
    }
  }
  function localPageOptions() { return {view,collectionId:view==="collections"?collectionId:null,query:filter,sortField:sort==="rank"?"sourceOrder":sort==="durationSeconds"?"duration":sort,direction:descending?"desc":"asc",minDurationSeconds:parseDurationBound(durationMinimum),maxDurationSeconds:parseDurationBound(durationMaximum),availability:localFilter,provider:view==="library"?librarySource:"all"}; }
  let legacyArtistAssetsCache: LocalAsset[] | null = null;
  let legacyArtistAssetsPending: { revision: number; promise: Promise<LocalAsset[]> } | null = null;
  let legacyArtistAssetsRevision = 0;
  function invalidateLegacyArtistAssets() {
    legacyArtistAssetsRevision++;
    artistPlaybackMatchRevision++;
    preparedNoLocalArtistMatches.clear();
    legacyArtistAssetsCache = null;
  }
  async function legacyArtistAssets(): Promise<LocalAsset[]> {
    while (true) {
      if (legacyArtistAssetsCache) return legacyArtistAssetsCache;
      const revision = legacyArtistAssetsRevision;
      if (!legacyArtistAssetsPending || legacyArtistAssetsPending.revision !== revision) {
        const pending = { revision, promise: Promise.resolve([] as LocalAsset[]) };
        pending.promise = call<LocalAsset[]>("list_assets").then(items => {
          if (revision === legacyArtistAssetsRevision) legacyArtistAssetsCache = items;
          return items;
        }).finally(() => { if (legacyArtistAssetsPending === pending) legacyArtistAssetsPending = null; });
        legacyArtistAssetsPending = pending;
      }
      const pending = legacyArtistAssetsPending;
      const items = await pending.promise;
      if (pending.revision === revision && revision === legacyArtistAssetsRevision) return items;
    }
  }
  function isUnknownArtistCommand(failure: unknown, command: "list_artist_local_assets_page" | "match_artist_playback_sources") {
    return new RegExp(`(?:unknown|not found|does not exist).{0,80}(?:command|${command})|(?:command|${command}).{0,80}(?:unknown|not found|does not exist)`, "i").test(String(failure));
  }
  const normalizeArtistLookup = (value: string) => value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase();
  async function getLegacyArtistLocalAssetsPage(request: ArtistLocalAssetsPageRequest): Promise<ArtistLocalAssetsPage> {
    const names = new Set(request.names.map(normalizeArtistLookup).filter(Boolean));
    const matching = (await legacyArtistAssets()).filter(asset => names.has(normalizeArtistLookup(asset.track.artist)));
    const filtered = filterTracks(matching.map(asset => asset.track), {
      query: request.query,
      ...(request.minDurationSeconds === undefined ? {} : { minDurationSeconds: request.minDurationSeconds }),
      ...(request.maxDurationSeconds === undefined ? {} : { maxDurationSeconds: request.maxDurationSeconds }),
    }).filter(track => request.availability === "all"
      || (request.availability === "on-device" ? track.localAvailability === "available" || Boolean(track.localPath) : track.localAvailability !== "available" && !track.localPath));
    const items = sortTracks(filtered, request.sortField, request.direction);
    return {
      items: items.slice(request.offset, request.offset + request.limit),
      total: items.length,
      limit: Math.min(200, Math.max(1, request.limit)),
      offset: request.offset,
      sortAvailability: trackSortAvailability(filtered),
    };
  }
  async function getArtistLocalAssetsPage(request: ArtistLocalAssetsPageRequest): Promise<ArtistLocalAssetsPage> {
    try {
      const page = await call<{ items: LocalAsset[]; total: number; limit: number; offset: number; sortAvailability?: ArtistLocalAssetsPage["sortAvailability"] }>("list_artist_local_assets_page", {
        names: request.names.slice(0, 200), limit: Math.min(200, Math.max(1, request.limit)), offset: Math.max(0, request.offset),
        query: request.query, sortField: request.sortField, direction: request.direction,
        minDurationSeconds: request.minDurationSeconds, maxDurationSeconds: request.maxDurationSeconds, availability: request.availability,
      });
      return { ...page, items: page.items.map(asset => asset.track) };
    } catch (failure) {
      if (!isUnknownArtistCommand(failure, "list_artist_local_assets_page")) throw failure;
      return getLegacyArtistLocalAssetsPage(request);
    }
  }
  function artistPlaybackMatchKey(track: Track): string {
    return [track.provider, track.providerId ?? "", normalizeArtistLookup(track.artist), normalizeArtistLookup(track.title), normalizeArtistLookup(track.version ?? ""),
      Number.isFinite(track.durationSeconds) ? String(track.durationSeconds) : ""].join("\u001f");
  }
  function rememberNoLocalArtistMatch(key: string, revision: number) {
    if (revision !== artistPlaybackMatchRevision) return;
    preparedNoLocalArtistMatches.delete(key);
    preparedNoLocalArtistMatches.set(key, revision);
    while (preparedNoLocalArtistMatches.size > 4096) {
      const oldest = preparedNoLocalArtistMatches.keys().next().value;
      if (oldest === undefined) break;
      preparedNoLocalArtistMatches.delete(oldest);
    }
  }
  async function matchArtistPlaybackSources(sourceTracks: Track[], artistNames: string[] = []): Promise<Track[]> {
    if (!sourceTracks.length) return sourceTracks;
    const result: Track[] = [];
    for (let offset = 0; offset < sourceTracks.length; offset += 200) {
      const batch = sourceTracks.slice(offset, offset + 200);
      const resolved = new Map<number, Track>();
      const pending: { track: Track; index: number; key: string }[] = [];
      batch.forEach((track, index) => {
        if (track.localAvailability === "available" && track.localPath) resolved.set(index, track);
        else {
          const key = artistPlaybackMatchKey(track);
          if (preparedNoLocalArtistMatches.get(key) === artistPlaybackMatchRevision) resolved.set(index, track);
          else pending.push({ track, index, key });
        }
      });
      if (pending.length) {
        const batchRevision = artistPlaybackMatchRevision;
        const pendingTracks = pending.map(item => item.track);
        try {
          const matches = await call<Array<LocalAsset | null>>("match_artist_playback_sources", {
            artistNames: artistNames.slice(0, 200), sourceTracks: pendingTracks,
          });
          if (!Array.isArray(matches) || matches.length !== pending.length) throw new Error("Artist playback matcher returned an unaligned result.");
          pending.forEach((item, index) => {
            const asset = matches[index];
            if (asset) resolved.set(item.index, { ...item.track, localPath: asset.path, assetId: asset.id, localAvailability: "available", provenance: { ...(item.track.provenance && typeof item.track.provenance === "object" ? item.track.provenance as Record<string, unknown> : {}), localPlaybackMatch: "unique exact artist, title and version" } });
            else {
              rememberNoLocalArtistMatch(item.key, batchRevision);
              resolved.set(item.index, item.track);
            }
          });
        } catch (failure) {
          if (!isUnknownArtistCommand(failure, "match_artist_playback_sources")) throw failure;
          const fallback = artistPlaybackTracks(pendingTracks, await legacyArtistAssets());
          pending.forEach((item, index) => {
            const track = fallback[index] ?? item.track;
            if (!(track.localAvailability === "available" && track.localPath)) rememberNoLocalArtistMatch(item.key, batchRevision);
            resolved.set(item.index, track);
          });
        }
      }
      result.push(...batch.map((_track, index) => resolved.get(index)!));
    }
    return result;
  }
  async function spotifyArtistCall(command: string, args: Record<string, unknown>) {
    const result = await call<unknown>(command, args);
    if (!["browse_remote_album_tracks", "browse_remote_artist_tracks"].includes(command) || !result || typeof result !== "object" || Array.isArray(result)) return result;
    const page = result as { tracks?: unknown } & Record<string, unknown>;
    if (!Array.isArray(page.tracks)) return result;
    return { ...page, tracks: await matchArtistPlaybackSources(page.tracks as Track[]) };
  }
  function mergeAssetFiles(existing: LocalAsset[], incoming: LocalAsset[]) {
    const byId = new Map(existing.map(asset => [asset.id, asset]));
    for (const asset of incoming) byId.set(asset.id, asset);
    return [...byId.values()];
  }
  async function analysisStatusesForAssets(ids: string[]) {
    if (!ids.length) return [] as typeof analysisStatuses;
    const legacy = async () => (await call<typeof analysisStatuses>("get_analysis_status")).filter(status => ids.includes(status.assetId));
    if (!scopedAssetCommandsSupported) return legacy();
    const statuses: typeof analysisStatuses = [];
    try {
      for (let index = 0; index < ids.length; index += 200) {
        statuses.push(...await call<typeof analysisStatuses>("get_analysis_status_for_assets", { assetIds: ids.slice(index, index + 200) }));
      }
      return statuses;
    } catch (failure) {
      if (!isUnknownAssetCommand(failure)) throw failure;
      scopedAssetCommandsSupported = false;
      return legacy();
    }
  }
  async function loadAssetFilesForTracks(pageTracks: Track[], append: boolean, isCurrent: () => boolean) {
    const recordingIds = [...new Set(pageTracks.map(track => track.id))].slice(0, 200);
    if (!recordingIds.length) { if (!append && isCurrent()) { assets = []; pageFilesByTrack = {}; analysisStatuses = []; } return; }
    let groups: { trackId: string; files: LocalAsset[] }[];
    if (scopedAssetCommandsSupported) {
      try { groups = await call("list_track_files_for_recordings", { recordingIds }); }
      catch (failure) {
        if (!isUnknownAssetCommand(failure)) throw failure;
        scopedAssetCommandsSupported = false;
        const legacy = await call<LocalAsset[]>("list_assets");
        groups = pageTracks.map(track => ({ trackId: track.id, files: legacy.filter(asset => asset.recordingId === track.id || sameTrack(asset.track, track)) }));
      }
    } else {
      const legacy = await call<LocalAsset[]>("list_assets");
      groups = pageTracks.map(track => ({ trackId: track.id, files: legacy.filter(asset => asset.recordingId === track.id || sameTrack(asset.track, track)) }));
    }
    if (!isCurrent()) return;
    const files = groups.flatMap(group => group.files);
    pageFilesByTrack = append ? { ...pageFilesByTrack, ...trackFileGroupsByTrackId(groups) } : trackFileGroupsByTrackId(groups);
    const ids = [...new Set(files.map(asset => asset.id))];
    const statuses = await analysisStatusesForAssets(ids);
    if (!isCurrent()) return;
    assets = append ? mergeAssetFiles(assets, files) : files;
    analysisStatuses = append
      ? [...analysisStatuses.filter(status => !ids.includes(status.assetId)), ...statuses]
      : statuses;
  }
  let detailAssetRequest = 0;
  let detailAssetRevision = $state(0);
  let loadedDetailAssetSignature = "";
  let activeDetailAssetSignature = "";
  const detailAssetTrackSignature = $derived(`${detailAssetRevision}:${[...new Set([selected?.id, playingTrack?.id, infoTrack?.id].filter((id): id is string => Boolean(id)))].join("\u001f")}`);
  async function refreshDetailAssetFiles(detailTracks: Track[], request: number) {
    const recordingIds = [...new Set(detailTracks.flatMap(track => [track.id, track.playbackRange?.parentTrackId].filter((id): id is string => Boolean(id))))].slice(0, 200);
    const assetIds = [...new Set(detailTracks.map(track => track.assetId).filter((id): id is string => Boolean(id)))].slice(0, 200);
    let groups: { trackId: string; files: LocalAsset[] }[] = [];
    if (recordingIds.length && scopedAssetCommandsSupported) {
      try { groups = await call("list_track_files_for_recordings", { recordingIds }); }
      catch (failure) {
        if (!isUnknownAssetCommand(failure)) throw failure;
        scopedAssetCommandsSupported = false;
      }
    }
    if (recordingIds.length && !scopedAssetCommandsSupported) {
      const legacy = await call<LocalAsset[]>("list_assets");
      groups = detailTracks.flatMap(track => {
        const canonical = track.playbackRange?.parentTrackId ?? track.id;
        const files = legacy.filter(asset => asset.recordingId === canonical || sameTrack(asset.track, track));
        return files.length ? [{ trackId: track.id, files }] : [];
      });
    }
    if (request !== detailAssetRequest) return false;
    let explicitAssets: LocalAsset[] = [];
    if (assetIds.length && scopedAssetCommandsSupported) {
      try { explicitAssets = await call("list_assets_by_ids", { assetIds }); }
      catch (failure) {
        if (!isUnknownAssetCommand(failure)) throw failure;
        scopedAssetCommandsSupported = false;
      }
    }
    if (assetIds.length && !scopedAssetCommandsSupported && explicitAssets.length === 0) {
      const legacy = await call<LocalAsset[]>("list_assets");
      explicitAssets = legacy.filter(asset => assetIds.includes(asset.id));
    }
    if (request !== detailAssetRequest) return false;
    const files = [...new Map([...groups.flatMap(group => group.files), ...explicitAssets].map(asset => [asset.id, asset])).values()];
    const grouped = trackFileGroupsByTrackId(groups);
    for (const track of detailTracks) {
      if (track.playbackRange) {
        const inherited = grouped[track.playbackRange.parentTrackId] ?? [];
        const direct = explicitAssets.filter(asset => asset.id === track.assetId);
        grouped[track.id] = [...new Map([...inherited, ...direct].map(asset => [asset.id, asset])).values()];
      }
    }
    detailFilesByTrack = grouped;
    const ids = [...new Set(files.map(asset => asset.id))];
    const statuses = await analysisStatusesForAssets(ids);
    if (request !== detailAssetRequest) return false;
    const previousIds = new Set(detailAssets.map(asset => asset.id));
    detailAssets = files;
    analysisStatuses = [...analysisStatuses.filter(status => !previousIds.has(status.assetId) && !ids.includes(status.assetId)), ...statuses];
    return true;
  }
  async function refreshCurrentAssetFiles() {
    const extras = [selected, playingTrack].filter((track): track is Track => Boolean(track));
    const tracksToRefresh = [...new Map([...extras, ...tracks.slice(0, 200)].map(track => [track.id, track])).values()].slice(0, 200);
    const request = localPageRequest;
    try { await loadAssetFilesForTracks(tracksToRefresh, true, () => request === localPageRequest); }
    catch (failure) { error = String(failure); }
  }
  const localPageReads=new Map<string,{request:number;promise:Promise<void>}>();
  async function loadLocalPage(append = false, options = localPageOptions()) {
    const key=JSON.stringify([options,append?tracks.length:0]);
    const existing=localPageReads.get(key);
    if(existing?.request===localPageRequest)return existing.promise;
    const promise=loadLocalPageOnce(append,options);
    localPageReads.set(key,{request:localPageRequest,promise});
    try{await promise;}finally{if(localPageReads.get(key)?.promise===promise)localPageReads.delete(key);}
  }
  async function refreshLocalSortAvailability(options: ReturnType<typeof localPageOptions>, queryRevision: number, context: string) {
    const signature = JSON.stringify(options);
    const isCurrent = () => queryRevision === localPageQueryRevision && activeContext === context
      && JSON.stringify(localPageOptions()) === signature && desktop()
      && ["library", "favorites", "collections"].includes(options.view);
    try {
      await pendingSortAvailabilityRefresh.refresh(
        `${queryRevision}:${context}:${signature}`,
        () => localSortAvailabilityPending,
        isCurrent,
        () => call<{sortAvailability?:TrackSortAvailability|null;sortAvailabilityPending?:boolean;sortPending?:boolean}>("list_tracks_page", {...options,limit:1,offset:0}),
        page => {
          if (page.sortPending === true) {
            localSortPending = true;
            return;
          }
          if (page.sortAvailabilityPending === false) {
            localSortAvailabilityPending = false;
            localSortAvailability = page.sortAvailability ?? null;
          } else if (page.sortAvailabilityPending === true) {
            localSortAvailability = null;
          }
        },
      );
    } catch (failure) {
      if (isCurrent()) error = String(failure);
    }
  }
  function handleSortAvailabilityUpdated() {
    pendingSortAvailabilityRefresh.noteUpdated();
    if (desktop() && localSortPending) {
      void loadLocalPage(false, localPageOptions());
      return;
    }
    if (desktop() && localSortAvailabilityPending) {
      void refreshLocalSortAvailability(localPageOptions(), localPageQueryRevision, activeContext);
    }
  }
  async function loadLocalPageOnce(append = false, options = localPageOptions()) {
    if (!["library","favorites","collections"].includes(options.view)) return;
    const request = ++localPageRequest;
    if (!append) { localPageQueryRevision++; selectedLocalOccurrences = []; localPlaylistSelectionMode = false; }
    const queryRevision = localPageQueryRevision;
    const offset = append ? tracks.length : 0;
    const availabilityEventRevision = pendingSortAvailabilityRefresh.revision;
    const requestContext = activeContext;
    localPageLoading = true;
    if (!append) { localSortAvailability = null; localSortAvailabilityPending = false; }
    try {
      const result = await requestLocalTrackPage<Track>(
        args => call<LocalTrackPage<Track>>("list_tracks_page", args),
        options,
        offset,
        () => request === localPageRequest,
      );
      if (result.kind === "stale" || result.kind === "invalid") return;
      if (result.kind === "sortPending") {
        localSortPending = true;
        if (!append) {
          localSortAvailabilityPending = result.page.sortAvailabilityPending === true;
          localSortAvailability = null;
        }
        if (pendingSortAvailabilityRefresh.revision !== availabilityEventRevision) {
          setTimeout(() => {
            if (localSortPending && requestContext === activeContext && request === localPageRequest) {
              void loadLocalPage(false, options);
            }
          }, 0);
        }
        return;
      }
      const page = result.page;
      const applied = applyLocalTrackPage(tracks, localPageTotal, page, append);
      localSortPending = false;
      tracks = applied.tracks;
      localPageTotal = applied.total;
      if (!append) {
        localSortAvailabilityPending = sortAvailabilityIsPending(page);
        localSortAvailability = localSortAvailabilityPending ? null : page.sortAvailability ?? null;
        if (localSortAvailabilityPending && pendingSortAvailabilityRefresh.revision !== availabilityEventRevision) {
          void refreshLocalSortAvailability(options, queryRevision, requestContext);
        }
      }
      void loadAssetFilesForTracks(page.tracks, append, () => request === localPageRequest && requestContext === activeContext)
        .catch(failure => { if (request === localPageRequest && requestContext === activeContext) error = String(failure); });
    } catch (failure) { if(request===localPageRequest)error=String(failure); }
    finally { if(request===localPageRequest)localPageLoading=false; }
  }
  let assetCountRequest = 0;
  $effect(() => {
    const shouldCount = view === "library" && desktop();
    const request = ++assetCountRequest;
    if (!shouldCount) return;
    untrack(() => {
      void (scopedAssetCommandsSupported ? call<number>("count_assets").catch(async failure => {
        if (!isUnknownAssetCommand(failure)) throw failure;
        scopedAssetCommandsSupported = false;
        return (await call<LocalAsset[]>("list_assets")).length;
      }) : call<LocalAsset[]>("list_assets").then(items => items.length)).then(count => {
        if (request === assetCountRequest && view === "library") assetTotal = count;
      }).catch(failure => { if (request === assetCountRequest) error = String(failure); });
    });
    return () => { if (request === assetCountRequest) assetCountRequest++; };
  });
  $effect(()=>{
    const options = localPageOptions();
    if (desktop() && ["library","favorites","collections"].includes(options.view)) untrack(()=>void loadLocalPage(false,options));
    else untrack(()=>{localPageRequest++;});
  });
  $effect(() => {
    const signature = detailAssetTrackSignature;
    if (signature === loadedDetailAssetSignature || signature === activeDetailAssetSignature) return;
    activeDetailAssetSignature = signature;
    const ids = signature.split(":").slice(1).join(":").split("\u001f").filter(Boolean).slice(0, 200);
    const detailTracks = ids.map(id => [selected, playingTrack, infoTrack].find(track => track?.id === id)).filter((track): track is Track => Boolean(track));
    const request = ++detailAssetRequest;
    untrack(() => { void refreshDetailAssetFiles(detailTracks, request).then(completed => {
      if (completed && request === detailAssetRequest) loadedDetailAssetSignature = signature;
      if (activeDetailAssetSignature === signature) activeDetailAssetSignature = "";
    }).catch(failure => { if (request === detailAssetRequest) error = String(failure); if (activeDetailAssetSignature === signature) activeDetailAssetSignature = ""; }); });
    return () => {
      if (request === detailAssetRequest) detailAssetRequest++;
      if (activeDetailAssetSignature === signature) activeDetailAssetSignature = "";
    };
  });

  const sorted = $derived.by(() => {
    if (["library","favorites","collections"].includes(view)) return tracks;
    const q = filter.toLowerCase();
    const list = filterTracks(tracks, {minDurationSeconds:parseDurationBound(durationMinimum)??undefined,maxDurationSeconds:parseDurationBound(durationMaximum)??undefined}).filter((t) => {
      const availability = (t as Track & { localAvailability?: string }).localAvailability;
      const isLocal = availability ? availability === "available" : Boolean(t.localPath);
      const matchesLocal = localFilter === "all" || (localFilter === "local" ? isLocal : !isLocal);
      const matchesSource = view !== "library" || librarySource === "all" || (librarySource === "local" ? isLocal : librarySource === "personal" ? true : t.provider === librarySource);
      return matchesLocal && matchesSource && `${t.artist} ${t.title} ${t.album ?? ""} ${t.version ?? ""} ${t.label ?? ""} ${t.genre ?? ""}`
        .toLowerCase().includes(q);
    });
    if (sort === "rank") {
      if (!descending) return list;
      return [...list].reverse();
    }
    return sortTracks(list, (sort === "durationSeconds" ? "duration" : sort) as TrackSortField, descending ? "desc" : "asc");
  });
  const virtualized = $derived(sorted.length > 1000);
  const windowRows = $derived(
    rowWindow(sorted.length, scrollTop, viewportHeight, rowHeight, overscan),
  );
  const startRow = $derived(windowRows.start);
  const endRow = $derived(windowRows.end);
  const visibleTracks = $derived(sorted.slice(startRow, endRow));
  let selectedLocalOccurrences = $state<string[]>([]);
  let localPlaylistSelectionMode = $state(false);
  const localOccurrenceRows = $derived.by(() => {
    const counts = new Map<string, number>();
    return sorted.map(track => {
      const ordinal = counts.get(track.id) ?? 0;
      counts.set(track.id, ordinal + 1);
      return { key: JSON.stringify([track.id, ordinal]), track };
    });
  });
  const selectedLocalOccurrenceSet = $derived(new Set(selectedLocalOccurrences));
  const selectedLocalOccurrenceTracks = $derived(localOccurrenceRows.filter(row => selectedLocalOccurrenceSet.has(row.key)).map(row => row.track));
  function toggleLocalOccurrence(index: number, checked: boolean) {
    const key = localOccurrenceRows[index]?.key;
    if (!key) return;
    const next = new Set(selectedLocalOccurrences);
    if (checked) next.add(key); else next.delete(key);
    selectedLocalOccurrences = [...next];
  }
  $effect(() => {
    const scope = JSON.stringify(localPageOptions());
    untrack(() => { selectedLocalOccurrences = []; localPlaylistSelectionMode = false; batchSelectionMode = false; });
  });

  const currentCollection = $derived(
    collections.find((c) => c.id === collectionId),
  );
  function savedPlaylistContext(id: string): PlaylistContext | undefined {
    // Imported and synced playlists count once they are present as saved local collections.
    // Remote source IDs that were never saved have no matching local collection.
    return savedLocalPlaylistContext(collections, id);
  }
  function currentSavedPlaylistContext(): PlaylistContext | undefined {
    return view === "collections" ? savedPlaylistContext(collectionId) : undefined;
  }
  const playlistMoveLimit = $derived(Math.max(localPageTotal,currentCollection?.trackCount ?? 0,
    tracks.reduce((maximum,track)=>Math.max(maximum,playlistPosition(track) ?? 0),0)));
  function sourceUrlForCollection(id: string): string | null {
    const imported = latestImports.find((item) => item.collectionId === id);
    const collection = collections.find((item) => item.id === id) as (Collection & { sourceUrl?: string | null; sourceProvider?: string | null; provenance?: unknown }) | undefined;
    const provenance = collection?.provenance && typeof collection.provenance === "object" ? collection.provenance as Record<string, unknown> : {};
    const source = imported?.sourceUrl || remotePlaylistBindings.find(binding => binding.collectionId === id || binding.sourceCollectionId === id)?.url || collection?.sourceUrl || (typeof provenance.sourceUrl === "string" ? provenance.sourceUrl : "");
    try { const parsed = new URL(source); return ["https:", "http:"].includes(parsed.protocol) ? parsed.href : null; } catch { return null; }
  }

  const currentPlaylistCreator = $derived((view === "collections" || (view === "charts" && activeBrowserImport))
    ? remotePlaylistBindings.find(binding => binding.collectionId === collectionId || binding.sourceCollectionId === collectionId) : undefined);
  const currentChartSourceUrl = $derived(view === "charts" ? (activeBrowserImport?.sourceUrl || chart?.sourceUrl || null) : null);
  async function openSourceToRefresh(url: string) {
    if (!url) return;
    await run(async () => {
      await call("open_external_url", { url });
      setNotice("Opened the source page. Refresh it, then use the browser extension to capture a new import.");
    });
  }
  $effect(() => {
    const current = noticeState;
    if (!current.message || current.expiry === "persistent") return;
    const timer = setTimeout(() => { if (noticeState === current) setNotice(""); }, 5500);
    return () => clearTimeout(timer);
  });
  $effect(() => {
    const current = deletedPlaylist;
    if (!current) return;
    const timer = setTimeout(() => {
      if (deletedPlaylist === current) deletedPlaylist = null;
    }, 10000);
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
    return { libraryBrowser: { ...libraryBrowser }, searchReleaseSnapshotKey:view === "search" && searchRelease ? captureSearchRelease() : null, searchNavigationSnapshotKey:view === "search" && !searchRelease ? searchComponent?.captureNavigationSnapshot(`route-search:${++navigationSnapshotSequence}`) : null, nativeArtistSnapshotKey:view === "artist" && !spotifyArtistId && !spotifyAlbumId ? nativeArtistComponent?.captureNavigationSnapshot(`route-catalog:${++navigationSnapshotSequence}`) : null, spotifyNavigationSnapshotKey:view === "artist" && (spotifyArtistId || spotifyAlbumId) ? spotifyArtistComponent?.captureNavigationSnapshot(`route-artist:${++navigationSnapshotSequence}`) : null, directorySnapshot:view === "directory-browser" ? directoryBrowser?.getSnapshot() : undefined, directoryLocation:directoryLocation ? {...directoryLocation}:null, view, collectionId, provider, genreUrl, query, filter, localFilter, sort, descending, librarySource, searchCategory, searchMode, importId: view === "charts" ? activeBrowserImport?.id : undefined, artistName: activeArtistName, artistRef: activeArtistRef, artistTrack: activeArtistTrack, artistSection, artistScroll, loadedTrackCount:tracks.length,durationMinimum,durationMaximum,spotifyArtistId, spotifyAlbumId, spotifyArtistSearchFallback, remoteOwner, remotePlaylistUrl, chartId: view === "charts" ? chart?.id : undefined, historyQuery, historyProvider, historyStationId, historyFrom, historyTo, historyOrder, historySortField, historySortDirection, historyMinDuration, historyMaxDuration, historyDownloadAvailability, historyLoadedCount: view === "history" ? historyItems.length : undefined, historyScroll: view === "history" ? historyScroll : undefined, historyFocusedSessionId, focusedPlaylistRowKey, selected, scroll: table?.scrollTop ?? 0 };
  }
  function rememberRoute(overrides: Partial<Route> = {}) {
    routeRestorationEpoch++; restoringRoute = false;
    backRoutes = [...backRoutes.slice(-99), {...captureRoute(),...overrides}]; forwardRoutes = [];
  }
  async function travelRoute(direction: "back" | "forward"): Promise<boolean> {
    if (restoringRoute) return false;
    const stack = direction === "back" ? backRoutes : forwardRoutes;
    const route = stack[stack.length - 1]; if (!route) return false;
    const current = captureRoute();
    const restorationEpoch = ++routeRestorationEpoch;
    let expectedNavigationRequest = navigationRequest + 1;
    if (direction === "back") { backRoutes = backRoutes.slice(0, -1); forwardRoutes = [...forwardRoutes, current]; }
    else { forwardRoutes = forwardRoutes.slice(0, -1); backRoutes = [...backRoutes, current]; }
    restoringRoute = true;
    try {
      spotifyNavigationSnapshotKey = route.spotifyNavigationSnapshotKey ?? null;
      searchNavigationSnapshotKey = route.searchNavigationSnapshotKey ?? null;
      searchReleaseRequest++;
      const restoredRelease=route.view==="search"&&route.searchReleaseSnapshotKey?providerNavigationSnapshots.get<SearchReleaseSnapshot>("search-release",route.searchReleaseSnapshotKey,"public-catalog"):null;
      searchRelease=restoredRelease?.release??null;searchReleaseTracks=restoredRelease?.tracks??[];searchReleaseBusy=false;
      if(route.searchReleaseSnapshotKey&&!restoredRelease)searchNavigationSnapshotKey=`expired:${route.searchReleaseSnapshotKey}`;
      searchReleaseError=restoredRelease?.needsReload?"Previous album tracks were evicted from memory. Return to search and reopen the album to reload them.":restoredRelease?.error??"";
      libraryBrowser = route.libraryBrowser ? { ...route.libraryBrowser } : initialLibraryBrowserState();
      directoryLocation = route.directoryLocation ? {...route.directoryLocation} : null;
      if (route.view === "directory-browser") directoryBrowserVisited = true;
      collectionId = route.collectionId; provider = route.provider; genreUrl = route.genreUrl; query = route.query;
      spotifyArtistId=route.spotifyArtistId??""; spotifyAlbumId=route.spotifyAlbumId??""; spotifyArtistSearchFallback=route.spotifyArtistSearchFallback??null; remoteOwner=route.remoteOwner??""; remotePlaylistUrl=route.remotePlaylistUrl??""; historyQuery=route.historyQuery??""; historyProvider=route.historyProvider??""; historyStationId=route.historyStationId??""; historyFrom=route.historyFrom??""; historyTo=route.historyTo??""; historyOrder=route.historyOrder??"newest"; historySortField=route.historySortField??"sourceOrder"; historySortDirection=route.historySortDirection??"desc"; historyMinDuration=route.historyMinDuration??""; historyMaxDuration=route.historyMaxDuration??""; historyDownloadAvailability=route.historyDownloadAvailability??"all"; historyScroll=route.historyScroll??0; historyFocusedSessionId=route.historyFocusedSessionId??null; focusedPlaylistRowKey=route.focusedPlaylistRowKey??null; const historyRestoreSignature = route.view === "history" ? JSON.stringify(historySearchOptions()) : null; activeArtistName = route.artistName; activeArtistRef = route.artistRef; activeArtistTrack = route.artistTrack; artistSection = route.artistSection; artistScroll = route.artistScroll; librarySource = route.librarySource; searchCategory = route.searchCategory; searchMode = route.searchMode; albumQuery = route.query;
      await navigate(route.view, false);
      let restoredControlsMatch = () => true;
      const restoreIsCurrent = () => restoredControlsMatch() && restorationEpoch === routeRestorationEpoch && navigationRequest === expectedNavigationRequest && view === route.view && collectionId === route.collectionId && provider === route.provider;
      if (!restoreIsCurrent()) return false;
      if (route.view === "artist" && !route.spotifyArtistId && !route.spotifyAlbumId && route.nativeArtistSnapshotKey) {
        const readyArtistPage=await waitForNativeArtistInstance();
        if (!restoreIsCurrent() || !readyArtistPage) return false;
        await readyArtistPage.restoreNavigationSnapshot(route.nativeArtistSnapshotKey);
        if (!restoreIsCurrent()) return false;
      }
      if(route.view==="search"&&restoredRelease){
        await tick();if(!restoreIsCurrent())return false;
        if(restoredRelease.controls)await searchReleaseComponent?.restoreNavigationState(restoredRelease.controls);
        if(!restoreIsCurrent())return false;
      }
    if (route.view === "search" && !restoredRelease && searchNavigationSnapshotKey) {
        const effectiveSearchKey=searchNavigationSnapshotKey;
        const readySearchPanel=await waitForSearchPanelInstance();
        if (!restoreIsCurrent() || !readySearchPanel) return false;
        await readySearchPanel.restoreNavigationSnapshot(effectiveSearchKey);
        if (!restoreIsCurrent()) return false;
      }
      if (route.view === "directory-browser" && route.directorySnapshot) {
        await tick();if (!restoreIsCurrent()) return false;
        await directoryBrowser?.restoreSnapshot(route.directorySnapshot);
        if (!restoreIsCurrent()) return false;
      }
      const historyRestoreIsCurrent = () => route.view === "history" && view === "history" && navigationRequest === expectedNavigationRequest && historyRestoreSignature === JSON.stringify(historySearchOptions());
      if (route.view === "history" && !historyRestoreIsCurrent()) return false;
      if (route.importId) { const saved = browserImports.find((item) => item.id === route.importId); if (saved) { expectedNavigationRequest++; await openBrowserImport(saved, false, false); } }
      else if (route.view === "charts" && route.chartId) await loadSnapshot(route.chartId, expectedNavigationRequest);
      if (!restoreIsCurrent()) return false;
      if (route.view === "charts") await loadGenres(false);
      if (!restoreIsCurrent()) return false;
      if (route.view === "history") {
        const targetCount = Math.max(0, route.historyLoadedCount ?? historyItems.length);
        const signature = historyRestoreSignature!;
        while (historyRestoreIsCurrent() && historyAppliedOptions === signature && historyItems.length < Math.min(targetCount, historyTotal) && historyHasMore) {
          const before = historyItems.length;
          await loadHistory(true);
          if (!historyRestoreIsCurrent()) return false;
          if (historyAppliedOptions !== signature || historyItems.length <= before) break;
        }
        if (!historyRestoreIsCurrent()) return false;
        historyScroll = route.historyScroll ?? 0;
        await tick();
      }
      if (!restoreIsCurrent()) return false;
      durationMinimum=route.durationMinimum??"";durationMaximum=route.durationMaximum??"";filter = route.filter; localFilter = route.localFilter; sort = route.sort; descending = route.descending; selected = route.selected;
      if(["library","favorites","collections"].includes(route.view)) {
        const restoredOptions = JSON.stringify(localPageOptions());
        restoredControlsMatch = () => JSON.stringify(localPageOptions()) === restoredOptions;
        await tick();
        if (!restoreIsCurrent()) return false;
        await loadLocalPage();
        if (!restoreIsCurrent()) return false;
        const target=route.loadedTrackCount??200;
        while(restoreIsCurrent()&&tracks.length<target&&tracks.length<localPageTotal){const before=tracks.length;await loadLocalPage(true);if(!restoreIsCurrent()||tracks.length<=before)break;}
      }
      requestAnimationFrame(() => { if (restoreIsCurrent() && table) table.scrollTop = route.scroll; });
      return true;
    } finally { if (restorationEpoch === routeRestorationEpoch) restoringRoute = false; }
  }
  const latestImports = $derived.by(() => {
    const seen = new Set<string>();
    return browserImports.filter((item) => !seen.has(item.sourceUrl) && Boolean(seen.add(item.sourceUrl)));
  });
  const sidebarImports = $derived(latestImports.map(item => {
    const binding = remotePlaylistBindings.find(binding => binding.sourceCollectionId === item.collectionId || binding.url === item.sourceUrl);
    return binding ? {...item, collectionId: binding.collectionId, ownerName: binding.ownerName, ownerId: binding.ownerId} : item;
  }));
  const visibleCollections = $derived(collections.filter(item => !remotePlaylistBindings.some(binding => binding.sourceCollectionId === item.id || browserImports.some(imported => imported.collectionId === item.id && imported.sourceUrl === binding.url))));

  async function openHistoryOccurrence(sessionId: string) {
    const shownTrackId = (infoTrack || playingTrack)?.id;
    const summary = shownTrackId && trackLibrarySummaryTrackId === shownTrackId ? trackLibrarySummary : null;
    const occurrence = summary?.playedOccurrences?.find(item => item.sessionId === sessionId);
    const range = occurrence ? historyOccurrenceDateRange(occurrence.playedAt) : null;
    if (!range) { setNotice("That playback occurrence is no longer available in recent history."); return; }
    rememberRoute();
    showPlayingInfo = false;
    historyQuery = ""; historyProvider = ""; historyStationId = "";
    historyFrom = range.from; historyTo = range.to; historyOrder = "newest"; historySortField = "sourceOrder";
    historyDownloadAvailability = "all"; historyMinDuration = ""; historyMaxDuration = "";
    historyFocusedSessionId = sessionId; focusedPlaylistRowKey = null;
    await navigate("history", false);
    const targetNavigationRequest = navigationRequest;
    let pages = 0;
    while (view === "history" && navigationRequest === targetNavigationRequest && historyFocusedSessionId === sessionId && !historyItems.some(item => item.sessionId === sessionId) && historyHasMore && pages < 50) {
      const before = historyItems.length;
      await loadHistory(true);
      if (historyItems.length <= before) break;
      pages++;
    }
    if (view !== "history" || navigationRequest !== targetNavigationRequest || historyFocusedSessionId !== sessionId) return;
    if (!historyItems.some(item => item.sessionId === sessionId)) {
      historyFocusedSessionId = null;
      setNotice("That playback occurrence is no longer available in recent history.");
    }
  }
  async function openPlaylistMembership(playlistId: string, trackId: string) {
    showPlayingInfo = false;
    await openPlaylist(playlistId);
    if (view !== "collections" || collectionId !== playlistId) return;
    const membershipNavigationRequest = navigationRequest;
    const stillViewingMembership = () => view === "collections" && collectionId === playlistId && navigationRequest === membershipNavigationRequest;
    const existing = sorted.findIndex(track => track.id === trackId);
    if (existing >= 0) {
      const track = sorted[existing]!;
      selected = track;
      focusedPlaylistRowKey = `${track.id}:${existing}`;
      await tick();
      scrollToTrack(track.id);
      return;
    }
    const matches = await call<Track[]>("get_tracks", { ids: [trackId] });
    if (!stillViewingMembership()) return;
    const track = matches[0];
    if (!track) { setNotice("That playlist membership is no longer available."); return; }
    filter = track.title;
    await loadLocalPage();
    if (!stillViewingMembership()) return;
    const index = sorted.findIndex(item => item.id === trackId);
    if (index >= 0) {
      selected = sorted[index]!;
      focusedPlaylistRowKey = `${trackId}:${index}`;
      await tick();
      scrollToTrack(trackId);
    } else setNotice("The playlist is open, but that track is no longer in its current contents.");
  }
  async function openPlaylist(id: string) {
    if (id === "favorites" || id === "history") { await navigate(id); return; }
    const binding = remotePlaylistBindings.find(binding => binding.collectionId === id || binding.sourceCollectionId === id);
    if (binding) id = binding.collectionId;
    const imported = !binding && latestImports.find((item) => item.collectionId === id);
    if (imported) { await openBrowserImport(imported); return; }
    rememberRoute(); collectionId = id;
    await navigate("collections", false);
  }
  async function importPreset(url:string) {
    browserUrl=url;importChoice="import-url";closeImportDialog();
    await requestBrowserImport();
    if(view==="source-browser")setNotice(browserMessage);
  }
  async function browseProviderCharts(source: string) {
    if (source === "spotify" || source === "youtube") {
      void call<typeof availableSources>("list_search_sources").then(sources => availableSources = sources).catch(() => {});
      await searchProviderPlaylists(source);
      return;
    }
    remoteOwner="";remotePlaylistUrl="";
    if(["shazam","lastfm","bandcamp","spotify","youtube"].includes(source)) {
      await navigate("source-browser");provider=source;activeBrowserImport=null;chart=null;tracks=[];collectionId="";return;
    }
    await navigate("charts"); provider = source; activeBrowserImport = null; chart = null; tracks = []; genreUrl = ""; genres = []; await loadGenres();
  }
  async function searchProviderPlaylists(source: "spotify" | "youtube") {
    catalogSearchSource = source;
    catalogSearchKind = "playlists";
    catalogSearchRequest++;
    await navigate("search");
  }
  async function openRemotePlaylist(source: "spotify" | "youtube", url: string) {
    // The sidebar can open a provider playlist from any view. Record the user's
    // actual current route once before changing providers; routing through the
    // provider list first would add an empty intermediate Back destination.
    rememberRoute();
    remotePlaylistUrl = url;
    if (view !== "source-browser" || provider !== source) {
      remoteOwner = "";
      provider = source;
      activeBrowserImport = null;
      chart = null;
      collectionId = "";
      await navigate("source-browser", false);
      tracks = [];
    }
  }
  async function openRemoteOwnerProfile(source: "spotify" | "youtube", ownerUrl: string) {
    if (source !== "spotify") {
      await run(() => call("open_external_url", { url: ownerUrl }));
      return;
    }
    const profileUrl = normalizeSpotifyUserProfileUrl(ownerUrl);
    if (!profileUrl) { error = "This Spotify creator link is not a valid public user profile URL."; return; }
    rememberRoute();
    spotifyArtistId = ""; spotifyAlbumId = ""; spotifyArtistSearchFallback = null;
    activeArtistName = ""; activeArtistRef = null; activeArtistTrack = null; artistSection = "overview"; artistScroll = 0;
    collectionId = ""; activeBrowserImport = null; chart = null; tracks = [];
    provider = "spotify"; remoteOwner = profileUrl; remotePlaylistUrl = "";
    await navigate("source-browser", false);
  }
  async function openCurrentPlaylistCreator() {
    const creator = currentPlaylistCreator;
    if (!creator?.ownerUrl) return;
    if (creator.provider === "spotify") { await openRemoteOwnerProfile("spotify", creator.ownerUrl); return; }
    await run(() => call("open_external_url", { url: creator.ownerUrl }));
  }
  async function openRemoteSearchResult(row: {id?:string;sourceId:string;kind:string;url?:string|null;title:string;artworkUrl?:string|null;data?:unknown}) {
    if(row.kind==="artist"&&row.sourceId==="spotify") {
      const id=row.id?.replace(/^spotify:(?:artist:)?/,"") || row.url?.match(/\/artist\/([a-zA-Z0-9]+)/)?.[1];
      if(!id || !/^[A-Za-z0-9]{10,64}$/.test(id)){error="This Spotify result has no usable artist ID.";return;}
      const rawData=row.data&&typeof row.data==="object"&&!Array.isArray(row.data)?row.data as Record<string,unknown>:null;
      const rawItemId=typeof rawData?.id==="string"?rawData.id.replace(/^spotify:(?:artist:)?/,""):null;
      const nestedArtworkIdMatches=rawItemId===id;
      const validArtworkUrl=(value:unknown):string|null=>{
        if(typeof value!=="string")return null;
        try{const parsed=new URL(value);return parsed.protocol==="https:"&&!parsed.username&&!parsed.password?value:null;}catch{return null;}
      };
      const artworkUrl=validArtworkUrl(nestedArtworkIdMatches?rawData?.artworkUrl:null)??validArtworkUrl(row.artworkUrl);
      rememberRoute();spotifyNavigationSnapshotKey=null;spotifyArtistId=id;spotifyAlbumId="";spotifyArtistSearchFallback={id,title:row.title,url:row.url??null,artworkUrl};activeArtistName=row.title;artistScroll=0;await navigate("artist",false);return;
    }
    if(row.kind==="artist") { await discoverArtist({id:`${row.sourceId}:artist-search`,provider:row.sourceId,providerId:"artist-search",sourceUrl:row.url??"",artist:row.title,title:"",favorite:false,provenance:{}});return; }
    if (row.kind === "album" && row.sourceId === "spotify") {
      const rowId = row.id?.replace(/^spotify:(?:album:)?/, "");
      let urlId: string | undefined;
      if (row.url) {
        try {
          const url = new URL(row.url);
          if (url.protocol === "https:" && url.hostname === "open.spotify.com" && !url.username && !url.password)
            urlId = url.pathname.match(/^\/(?:intl-[a-z-]+\/)?album\/([A-Za-z0-9]{22})\/?$/)?.[1];
        } catch { /* The result ID can still identify this album. */ }
      }
      const id = rowId && /^[A-Za-z0-9]{22}$/.test(rowId) ? rowId : urlId;
      if (!id || (urlId && urlId !== id)) { error = "This Spotify result has no usable album ID."; return; }
      await openSpotifyAlbum(id, row.title);
      return;
    }
    if(row.kind==="album") { if(row.url)await importPreset(row.url);return; }
    if ((row.sourceId === "spotify" || row.sourceId === "youtube") && row.kind === "playlist") {
      rememberRoute(); provider = row.sourceId; remoteOwner = ""; remotePlaylistUrl = row.url ?? ""; activeBrowserImport = null; chart = null; tracks = []; collectionId = "";
      await navigate("source-browser", false); return;
    }
    if ((row.sourceId === "spotify" && row.kind === "profile") || (row.sourceId === "youtube" && row.kind === "channel")) {
      rememberRoute(); provider = row.sourceId; remoteOwner = row.url ?? row.title; remotePlaylistUrl = ""; activeBrowserImport = null; chart = null; tracks = []; collectionId = "";
      await navigate("source-browser", false); return;
    }
    await browseProviderCharts(row.sourceId);
  }
  async function chooseLibrarySource(source: string) {
    rememberRoute(); librarySource = source; libraryBrowser = initialLibraryBrowserState();
    await navigate("library", false);
  }
  async function syncMirrors() {
    if (!desktop() || settings.playlistMirroringEnabled === false) return;
    try {
      const result = await call<{ synchronized: number; repairedTrackMetadata?: number; warnings: string[]; mirrorsDirectory?: string; pausedMirrors?: string[]; sources?: {collectionId?: string | null}[] }>("sync_playlists");
      if (untrack(() => settings.playlistMirroringEnabled === false)) return;
      mirrorStatus = result.warnings?.join(" · ") ?? "";
      pausedMirrors = result.pausedMirrors ?? []; localPlaylistIds = result.sources?.map((source) => source.collectionId).filter((id): id is string => Boolean(id)) ?? [];
      collections = await call<Collection[]>("list_collections");
      if ((result.repairedTrackMetadata ?? 0) > 0) requestLocalPrefixRefresh(undefined, true);
      if ((result.repairedTrackMetadata ?? 0) > 0) {
        const ids=[...queueTracks.map(track=>track.id),...(playingTrack?[playingTrack.id]:[]),...(selected?[selected.id]:[])];
        const byId=await getTracksInBatches(ids);
        if (playingTrack) playingTrack=byId.get(playingTrack.id) ?? playingTrack;
        if (selected) selected=byId.get(selected.id) ?? selected;
        queueTracks=queueTracks.map(track=>byId.get(track.id) ?? track);
      }
    } catch (failure) { mirrorStatus = String(failure); }
  }
  function refreshQueue() {

    queueIndex = playbackQueue.currentIndex;
    queueTracks = [...playbackQueue.tracks];
    repeatMode = playbackQueue.repeat;
    playbackQueueGeneration++;
    resetTrackFailureHistoryForQueue(automationQueueProjection.revision, automationQueueProjection.sourceKey);
    playbackQueueSaveError = savePlaybackQueue(playbackQueue)
      ? ""
      : "Your playback queue couldn't be saved. You can continue listening, but changes may be lost after restarting Burrata.";
  }
  async function advanceQueue(direction: number, ended = false, failedSkips = 0, requestId: string | null = null) {

      if (!ended) clearPlaybackSkipNotice();
      await nativeCommand({action:direction > 0 ? "next" : "previous"});
      return;

  }
  let lastPersistedPlaybackPosition: {trackId:string;position:number;savedAt:number} | null = null;

  function restorePlaybackPosition() {
    if (typeof localStorage === "undefined") return;
    try {
      const value = JSON.parse(backupStartupReadStorage.getItem(PLAYBACK_POSITION_KEY) ?? "null") as NativePositionBookmark | null;
      if (!validNativePositionBookmark(value) || value.position <= 0 ||
          Date.now() - value.savedAt > 30 * 24 * 60 * 60 * 1000 ||
          !nativeBookmarkMatchesOccurrence(value, playbackQueue.current?.id, playbackQueue.entryIds[queueIndex])) return;
      resumeTrackId = value.trackId;
      resumePosition = value.position;
      position = resumePosition;
      previewStatus = `Ready to resume from ${time(resumePosition)}`;
    } catch { /* Ignore corrupt or inaccessible saved state. */ }
  }

  function updateArtwork(track: Track, artworkUrl: string) {
    if (artworkUrl.startsWith("https://")) artworkFilePaths.delete(track.id);
    if (playingTrack?.id === track.id) { playingTrack = { ...playingTrack, artworkUrl }; void publishAutomationPlayback(); }
    if (selected?.id === track.id) selected = { ...selected, artworkUrl };
  }
  function originalArtworkSource(url: string | null | undefined): string | null | undefined {
    if (!url || url.startsWith("https://")) return url;
    for (const [original, cached] of artworkCache) if (cached.url === url) return original;
    return url;
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
  async function recoverDisplayArtwork(track: Track, failed: string) {
    const matches = (item: Track | null | undefined) => item?.id === track.id && item.artworkUrl === failed;
    const isCurrent = () => artworkDisplayActive && (matches(playingTrack) || (showPlayingInfo && matches(infoTrack ?? playingTrack)));
    if (!isCurrent()) return;
    if (failed.startsWith("asset:")) {
      failedArtwork = { ...failedArtwork, [failed]: true };
      const displayUrl = await artworkDisplayCache.recover(failed, path => call<string>("read_cached_artwork_display", {path}), isCurrent);
      if (!isCurrent()) return;
      if (displayUrl) artworkDisplayUrls = artworkDisplayCache.snapshot();
      return;
    }
    recoverArtwork(track, failed);
  }
  const currentArtworkRecoveryIdentity=$derived(JSON.stringify([
    playingTrack?.id,playingTrack?.artworkUrl,
    showPlayingInfo?(infoTrack??playingTrack)?.id:null,
    showPlayingInfo?(infoTrack??playingTrack)?.artworkUrl:null,
  ]));
  $effect(()=>{
    currentArtworkRecoveryIdentity;
    // Blacklisting removes the failed image, so revisiting an evicted URL cannot
    // rely on another image error. Retry on identity changes only, never on our
    // own blacklist/display publication or on position-only native snapshots.
    untrack(()=>{
      for(const track of [playingTrack,showPlayingInfo?(infoTrack??playingTrack):null]){
        const url=track?.artworkUrl;
        if(track&&url?.startsWith("asset:")&&failedArtwork[url]&&!artworkDisplayUrls[url])void recoverDisplayArtwork(track,url);
      }
    });
  });
  function recoverArtwork(track: Track, failed = track.artworkUrl) {
    if (!failed || track.artworkUrl !== failed) return;
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
  const playerPosition = $derived((position));
  function isStationPlaybackTrack(track: Track | null | undefined): boolean {
    return Boolean(track && (track.provider === "radioparadise" || track.provider === "internet-radio" ||
      track.provider === "radio" && (track.provenance as { sourceKind?: string } | undefined)?.sourceKind === "radio"));
  }
  const liveRadioPlayback = $derived(isStationPlaybackTrack(playingTrack));
  const playerDuration = $derived.by(() => {
    // Media element properties are not reactive; durationchange updates this signal.
    duration;
    const provenance = playingTrack?.provenance as { sourceKind?: string } | null | undefined;
    const liveRadio = playingTrack?.provider === "radioparadise" || playingTrack?.provider === "internet-radio"
      || playingTrack?.provider === "radio" && provenance?.sourceKind === "radio";

      if (duration > 0 || !paused) return duration;
      return liveRadio || !playingTrack ? duration : playableDuration(playingTrack) ?? duration;

  });
  async function cancelLocalWaveformAnalysis(message = "Waveform analysis cancelled.") {
    localWaveformAnalysisRequest++;
    const jobId = localWaveformAnalysisJobId;
    localWaveformAnalysisJobId = null;
    localWaveformAnalysisOwnerKey = "";
    if (jobId) await call("cancel_local_waveform_analysis", { jobId }).catch(() => {});
    localWaveformAnalysisStatus = message;
  }
  async function analyzeCurrentLocalWaveform() {
    const assetId = waveformLocalAssetId;
    const trackId = waveformTrackId;
    const generation = waveformGeneration;
    if (!assetId || !trackId || generation === null || !waveformEnabled) return;
    if (localWaveformAnalysisJobId) await cancelLocalWaveformAnalysis("Replacing previous waveform request…");
    const request = ++localWaveformAnalysisRequest;
    const isCurrent = () => request === localWaveformAnalysisRequest && waveformEnabled && waveformLocalAssetId === assetId && waveformTrackId === trackId && waveformGeneration === generation;
    localWaveformAnalysisStatus = "Checking registered audio file…";
    try {
      const asset = await call<LocalAsset | null>("get_asset_by_id", { assetId });
      if (!isCurrent()) return;
      if (!asset || asset.id !== assetId || !/^[a-f0-9]{64}$/i.test(asset.hash)) {
        localWaveformAnalysisStatus = "A complete waveform is unavailable for this file.";
        return;
      }
      void analyzeCurrentLocalPlaybackDetails(asset);
      const requestData = { assetId, expectedHash: asset.hash, binCount: 1200 };
      const install = async (record: {
        assetId: string; fileHash: string; analyzerVersion: string; durationSeconds: number;
        bins: Array<{ min: number; max: number } | null>; complete: boolean; scope?: "wholeTrack" | "playbackRange";
      }) => {
        if (!isCurrent() || record.assetId !== assetId || record.fileHash !== asset.hash || record.analyzerVersion !== "ffmpeg-mono-peaks-8khz-v1" ||
            record.bins.length !== 1200 || !Number.isFinite(record.durationSeconds) || record.durationSeconds <= 0) return false;
        const waveform = waveformData;
        const playbackRange = playingTrack?.playbackRange;
        const nativeChapter = (nativeSnapshot?.chapter);
        const startSeconds = nativeChapter?.startSeconds ?? playbackRange?.startTime ?? 0;
        const endSeconds = nativeChapter?.endSeconds ?? playbackRange?.endTime ?? record.durationSeconds;
        const isPlaybackRange = startSeconds > 0.001 || endSeconds < record.durationSeconds - 0.001;
        const bins = isPlaybackRange
          ? waveform.cropWaveformBins({ bins: record.bins, durationSeconds: record.durationSeconds, startSeconds, endSeconds, outputBinCount: 1200 })
          : record.bins;
        const waveformDurationSeconds = isPlaybackRange ? endSeconds - startSeconds : record.durationSeconds;
        playbackWaveform = waveform.localWaveformSnapshot({
          generation, trackId, durationSeconds: waveformDurationSeconds, bins,
          complete: !isPlaybackRange && record.complete,
          scope: isPlaybackRange ? "playbackRange" : "wholeTrack",
        });
        verifiedLocalWaveformBinding = {
          trackId, assetId, fileHash: asset.hash,
          fullTrack: record.complete && record.scope === "wholeTrack" && record.bins.every(bin => bin !== null),
        };
        playbackWaveformReason = "";
        localWaveformAnalysisStatus = "Full local waveform ready.";
        return true;
      };
      const cached = await call<Parameters<typeof install>[0] | null>("get_local_waveform_analysis", requestData);
      if (!isCurrent()) return;
      if (cached && await install(cached)) return;
      const jobId = await call<string>("start_local_waveform_analysis", requestData);
      if (!isCurrent()) { await call("cancel_local_waveform_analysis", { jobId }).catch(() => {}); return; }
      localWaveformAnalysisJobId = jobId;
      localWaveformAnalysisOwnerKey = `${trackId}\u0000${assetId}\u0000${generation}`;
      localWaveformAnalysisStatus = "Analyzing the registered audio file…";
      while (isCurrent()) {
        await new Promise(resolve => window.setTimeout(resolve, 500));
        if (!isCurrent()) break;
        const status = await call<{
          jobId: string; status: string; assetId: string; fileHash: string; binCount: number;
          durationSeconds?: number | null; processedSeconds: number; error?: string | null;
        }>("get_local_waveform_analysis_status", { jobId });
        if (status.jobId !== jobId || status.assetId !== assetId || status.fileHash !== asset.hash || status.binCount !== 1200) {
          localWaveformAnalysisStatus = "Waveform analysis result did not match the current audio file.";
          break;
        }
        if (status.status === "completed") {
          const record = await call<Parameters<typeof install>[0] | null>("get_local_waveform_analysis", requestData);
          if (!record || !await install(record)) localWaveformAnalysisStatus = "Waveform analysis finished, but a verified cached waveform is unavailable.";
          break;
        }
        if (status.status === "failed" || status.status === "cancelled") {
          localWaveformAnalysisStatus = status.error ?? (status.status === "cancelled" ? "Waveform analysis cancelled." : "Waveform analysis failed.");
          break;
        }
        const total = status.durationSeconds ?? asset.durationSeconds ?? 0;
        const progress = total > 0 ? Math.min(99, Math.floor(status.processedSeconds / total * 100)) : 0;
        localWaveformAnalysisStatus = `Analyzing local audio… ${progress}%`;
      }
      if (!isCurrent()) await call("cancel_local_waveform_analysis", { jobId }).catch(() => {});
      if (localWaveformAnalysisJobId === jobId) {
        localWaveformAnalysisJobId = null;
        localWaveformAnalysisOwnerKey = "";
      }
    } catch (failure) {
      if (isCurrent()) localWaveformAnalysisStatus = failure instanceof Error ? failure.message : "Waveform analysis is unavailable.";
      localWaveformAnalysisJobId = null;
      localWaveformAnalysisOwnerKey = "";
    }
  }
  async function analyzeCurrentLocalPlaybackDetails(asset: LocalAsset) {
    const track = playingTrack;
    if (!track || track.id !== waveformTrackId || asset.id !== waveformLocalAssetId || asset.recordingId !== track.id) return;
    const owner = `${track.id}\u0000${asset.id}\u0000${asset.hash}`;
    if (currentLocalAnalysisOwner === owner) return;
    const fields = [...(!track.bpm ? ["bpm"] : []), ...(!track.key?.trim() ? ["key"] : [])];
    currentLocalAnalysisOwner = owner;
    currentLocalAnalysisResult = null;
    currentLocalAnalysisStatus = "";
    if (!fields.length) return;
    const request = ++currentLocalAnalysisRequest;
    const isCurrent = () => request === currentLocalAnalysisRequest && currentLocalAnalysisOwner === owner &&
      playingTrack?.id === track.id && waveformLocalAssetId === asset.id;
    currentLocalAnalysisStatus = fields.includes("key") ? "Analyzing musical key from the full file…" : "Analyzing BPM from the full file…";
    try {
      const started = await call<SelectedAnalysisJob>("start_selected_asset_analysis", { assetIds: [asset.id], fields });
      if (!isCurrent()) { if (started?.jobId) void call("cancel_selected_asset_analysis", {jobId:started.jobId}).catch(()=>{}); return; }
      if (!started?.jobId) throw new Error("Full-file analysis is unavailable.");
      currentLocalAnalysisJobId = started.jobId;
      let job = started;
      while (isCurrent() && !["completed", "failed", "cancelled"].includes(job.state)) {
        await new Promise(resolve => window.setTimeout(resolve, 500));
        if (!isCurrent()) return;
        job = await call<SelectedAnalysisJob>("get_selected_asset_analysis", {jobId:started.jobId});
        if (!job || job.jobId !== started.jobId) throw new Error("Full-file analysis is unavailable.");
      }
      if (!isCurrent()) return;
      const row = job.results.find(item => item.assetId === asset.id && item.recordingId === track.id);
      const result = row?.analysis;
      if (row?.status !== "completed" || !result || result.scope !== "wholeTrack" || result.coverage < 0.99 ||
          result.resource.kind !== "registeredAsset" || result.resource.assetId !== asset.id ||
          result.resource.recordingId !== track.id || result.resource.fileHash !== asset.hash) {
        currentLocalAnalysisStatus = row?.error || "No confident full-file estimate is available.";
        return;
      }
      currentLocalAnalysisResult = result;
      currentLocalAnalysisStatus = result.key ? "Full-file analysis ready." : "No confident key estimate is available.";
    } catch (failure) {
      if (isCurrent()) currentLocalAnalysisStatus = failure instanceof Error ? failure.message : "Full-file analysis is unavailable.";
    } finally {
      if (isCurrent()) currentLocalAnalysisJobId = null;
    }
  }
  $effect(() => {
    const trackId = waveformTrackId, assetId = waveformLocalAssetId;
    const result = currentLocalAnalysisResult;
    if (currentLocalAnalysisOwner && (!trackId || !assetId || currentLocalAnalysisOwner.split("\u0000")[0] !== trackId ||
        currentLocalAnalysisOwner.split("\u0000")[1] !== assetId)) {
      currentLocalAnalysisRequest++;
      const jobId = currentLocalAnalysisJobId;
      currentLocalAnalysisJobId = null; currentLocalAnalysisOwner = "";
      currentLocalAnalysisResult = null; currentLocalAnalysisStatus = "";
      if (jobId) void call("cancel_selected_asset_analysis", {jobId}).catch(()=>{});
    }
    if (!desktop() || paused || !showPlayingInfo || infoTrack && infoTrack.id !== trackId || !assetId || !trackId || result) return;
    const expectedAsset = assetId, expectedTrack = trackId;
    let active = true;
    untrack(() => { void call<LocalAsset | null>("get_asset_by_id", {assetId}).then(asset => {
      if (active && asset && waveformLocalAssetId === expectedAsset && playingTrack?.id === expectedTrack) void analyzeCurrentLocalPlaybackDetails(asset);
    }).catch(()=>{}); });
    return () => { active = false; };
  });
  let automaticLocalWaveformOwner = "";
  $effect(() => {
    const enabled = waveformEnabled, assetId = waveformLocalAssetId, trackId = waveformTrackId, generation = waveformGeneration;
    if (!enabled || !assetId || !trackId || generation === null) { automaticLocalWaveformOwner = ""; return; }
    if (!desktop() || paused) return;
    const owner = `${trackId}\u0000${assetId}\u0000${generation}`;
    if (automaticLocalWaveformOwner === owner) return;
    automaticLocalWaveformOwner = owner;
    untrack(() => { void analyzeCurrentLocalWaveform(); });
  });
  $effect(() => {
    const owner = `${waveformTrackId ?? ""}\u0000${waveformLocalAssetId ?? ""}\u0000${waveformGeneration ?? ""}`;
    const jobId = localWaveformAnalysisJobId;
    const jobOwner = localWaveformAnalysisOwnerKey;
    if (jobId && (!waveformEnabled || !waveformTrackId || !waveformLocalAssetId || waveformGeneration === null || owner !== jobOwner)) {
      void cancelLocalWaveformAnalysis();
    }
  });
  let previousLocalWaveformOwner = "";
  $effect(() => {
    const trackId = waveformTrackId;
    const assetId = waveformLocalAssetId;
    const generation = waveformGeneration;
    const owner = `${trackId ?? ""}\u0000${assetId ?? ""}\u0000${generation ?? ""}`;
    const previousOwner = previousLocalWaveformOwner;
    previousLocalWaveformOwner = owner;
    const binding = verifiedLocalWaveformBinding;
    if (binding && (binding.trackId !== trackId || binding.assetId !== assetId)) {
      verifiedLocalWaveformBinding = null;
      if (playbackWaveform?.source === "local-analysis") playbackWaveform = null;
      playbackWaveformReason = "";
      localWaveformAnalysisStatus = "";
      return;
    }
    if (!binding || !binding.fullTrack || !waveformEnabled || !trackId || !assetId || generation === null || localWaveformAnalysisJobId) return;
    const alreadyBound = playbackWaveform?.source === "local-analysis" && playbackWaveform.trackId === trackId && playbackWaveform.generation === generation;
    if (alreadyBound || !previousOwner) return;
    const previousResource = previousOwner.split("\u0000").slice(0, 2).join("\u0000");
    const currentResource = `${trackId}\u0000${assetId}`;
    if (previousResource !== currentResource) return;
    // A chapter or user-selected range has its own seek axis. Keep its existing
    // range-local policy; only rebind a verified whole-track cache to a
    // whole-track playback owner here.
    if (playingTrack?.playbackRange || nativeSnapshot?.chapter) return;

    const request = ++localWaveformCacheRebindRequest;
    localWaveformCacheRebindOwnerKey = owner;
    const isCurrent = () => request === localWaveformCacheRebindRequest && waveformEnabled && waveformTrackId === trackId &&
      waveformLocalAssetId === assetId && waveformGeneration === generation && true &&
      verifiedLocalWaveformBinding?.trackId === trackId && verifiedLocalWaveformBinding.assetId === assetId &&
      verifiedLocalWaveformBinding.fileHash === binding.fileHash && verifiedLocalWaveformBinding.fullTrack;
    const clearUnverifiedBinding = () => {
      if (!isCurrent()) return;
      verifiedLocalWaveformBinding = null;
      if (playbackWaveform?.source === "local-analysis") playbackWaveform = null;
      playbackWaveformReason = "Waveform cache could not be verified for this audio file.";
    };
    void (async () => {
      try {
        const asset = await call<LocalAsset | null>("get_asset_by_id", { assetId });
        if (!isCurrent()) return;
        if (!asset || asset.id !== assetId || asset.hash !== binding.fileHash || !/^[a-f0-9]{64}$/i.test(asset.hash)) {
          clearUnverifiedBinding();
          return;
        }
        const record = await call<{
          assetId: string; fileHash: string; analyzerVersion: string; durationSeconds: number;
          bins: Array<{ min: number; max: number } | null>; complete: boolean; scope?: "wholeTrack" | "playbackRange";
        } | null>("get_local_waveform_analysis", { assetId, expectedHash: asset.hash, binCount: 1200 });
        if (!isCurrent()) return;
        if (!record || record.assetId !== assetId || record.fileHash !== asset.hash || record.analyzerVersion !== "ffmpeg-mono-peaks-8khz-v1" ||
            record.scope !== "wholeTrack" || record.complete !== true || record.bins.length !== 1200 || record.bins.some(bin =>
              !bin || !Number.isFinite(bin.min) || !Number.isFinite(bin.max) || bin.min < -1 || bin.max > 1 || bin.min > bin.max) ||
            !Number.isFinite(record.durationSeconds) || record.durationSeconds <= 0) {
          clearUnverifiedBinding();
          return;
        }
        const waveform = waveformData;
        if (!isCurrent()) return;
        playbackWaveform = waveform.localWaveformSnapshot({
          generation, trackId, durationSeconds: record.durationSeconds,
          bins: record.bins, complete: true, scope: "wholeTrack",
        });
        playbackWaveformReason = "";
        localWaveformAnalysisStatus = "Full local waveform ready.";
      } catch {
        clearUnverifiedBinding();
      } finally {
        if (localWaveformCacheRebindOwnerKey === owner) localWaveformCacheRebindOwnerKey = "";
      }
    })();
    if (!localWaveformAnalysisJobId) localWaveformAnalysisStatus = "";
  });
  onMount(() => () => {
    localWaveformAnalysisRequest++;
    currentLocalAnalysisRequest++;
    const detailsJobId = currentLocalAnalysisJobId;
    if (detailsJobId) void call("cancel_selected_asset_analysis", {jobId:detailsJobId}).catch(()=>{});
    const jobId = localWaveformAnalysisJobId;
    if (jobId) void call("cancel_local_waveform_analysis", { jobId }).catch(() => {});
  });
  $effect(() => {
    const enabled = waveformEnabled;
    const resource = currentStreamMediaResource();
    const generation = waveformGeneration;
    const playerDurationSeconds = playerDuration;
    if (!enabled || !resource || generation === null || !Number.isFinite(playerDurationSeconds) || playerDurationSeconds <= 0) {
      streamWaveformCacheLookupOwner = "";
      return;
    }
    const owner = `${resource.key}:${generation}`;
    if (streamWaveformCacheLookupOwner === owner) return;
    streamWaveformCacheLookupOwner = owner;
    let active = true;
    void (async () => {
      try {
        const trackContext = playingTrack;
        if (!trackContext || trackContext.id !== resource.trackId) return;
        const analysis = await call<WholeTrackAnalysisDto | null>("get_cached_stream_media_analysis", {
          track: streamMediaTrackContext(trackContext, resource.provider, resource.sourceId),
          provider: resource.provider, sourceId: resource.sourceId,
          durationSeconds: resource.durationSeconds, audioVariant: resource.audioVariant ?? null,
        });
        if (!active || currentStreamMediaResource()?.key !== resource.key || waveformGeneration !== generation) return;
        if (!analysis) {
          playbackWaveformReason = "No complete cached waveform is available for this stream; observed playback remains partial.";
          return;
        }
        const stillOwnsLookup = () => active && waveformEnabled && waveformGeneration === generation &&
          currentStreamMediaResource()?.key === resource.key;
        if (await installStreamMediaWaveform(analysis, resource, generation, stillOwnsLookup)) {
          if (!stillOwnsLookup()) return;
          streamMediaAnalysisResult = analysis;
          streamMediaAnalysisResultKey = resource.key;
          streamMediaAnalysisStatus = "Complete waveform cache available for this exact stream resource.";
          playbackWaveformReason = "Full waveform is cached for this exact verified stream source.";
        } else if (stillOwnsLookup()) {
          playbackWaveformReason = "A complete waveform is unavailable for this exact stream resource; observed playback remains partial.";
        }
      } catch {
        if (active) playbackWaveformReason = "No complete waveform is cached for this stream; observed playback remains partial.";
      }
    })();
    return () => {
      active = false;
      // A metadata refresh can invalidate a pending exact-resource lookup.
      // Its replacement effect must be allowed to retry the same resource.
      if (streamWaveformCacheLookupOwner === owner) streamWaveformCacheLookupOwner = "";
    };
  });
  $effect(() => {
    const trackId = waveformTrackId;
    const generation = waveformGeneration;
    const durationSeconds = playerDuration;
    const startSeconds = nativeSnapshot?.chapter?.startSeconds ?? playingTrack?.playbackRange?.startTime ?? 0;
    // This effect owns native observer snapshots only. WebView cache/observer
    // snapshots have a separate lifecycle and must survive metadata refreshes
    // that rerun this effect for the same playback item.
    const observerPaused = paused;
    // Cache binding is asynchronous. Take a fresh final sample once it is ready,
    // including while paused, so restored coverage cannot replace new bins.
    const observerBinding = observedWaveformBinding;
    void observerBinding;
    if ((!waveformEnabled)) return;
    if (!trackId || generation === null || !Number.isFinite(durationSeconds) || durationSeconds <= 0 || durationSeconds > 86_400) {
      playbackWaveform = null;
      playbackWaveformReason = "";
      return;
    }
    let active = true;
    let inFlight = false;
    const poll = async () => {
      const owner = `${trackId ?? ""}\u0000${waveformLocalAssetId ?? ""}\u0000${generation ?? ""}`;
      const completeAnalysisWaveformIsCurrent = playbackWaveform?.source === "local-analysis" && playbackWaveform.complete &&
        playbackWaveform.trackId === trackId && playbackWaveform.generation === generation || currentStreamWaveformMatchesPlayback() &&
        playbackWaveform?.trackId === trackId && playbackWaveform.generation === generation;
      if (!active || inFlight || localWaveformAnalysisJobId || localWaveformCacheRebindOwnerKey === owner || completeAnalysisWaveformIsCurrent) return;
      inFlight = true;
      try {
        const result = await call<{
          generation: number; durationSeconds: number; bins: Array<{ min: number; max: number } | null>;
          observedFraction: number; continuousFromZero: boolean; observedEndSeconds: number | null;
          complete: boolean; reason?: string | null;
        } | null>("get_playback_waveform", { generation, startSeconds, durationSeconds, binCount: 1200 });
        const currentFullLocalWaveform = playbackWaveform?.source === "local-analysis" && playbackWaveform.complete &&
          playbackWaveform.trackId === trackId && playbackWaveform.generation === generation || currentStreamWaveformMatchesPlayback() &&
          playbackWaveform?.trackId === trackId && playbackWaveform.generation === generation;
        if (!active || !result || localWaveformAnalysisJobId || localWaveformCacheRebindOwnerKey === owner || currentFullLocalWaveform || playingTrack?.id !== trackId || nativeSnapshot?.generation !== generation || !true) return;
        if (!Number.isSafeInteger(result.generation) || result.generation !== generation ||
            !Number.isFinite(result.durationSeconds) || result.durationSeconds <= 0 ||
            !Array.isArray(result.bins) || result.bins.length > 4096) {
          playbackWaveform = null;
          playbackWaveformReason = "Waveform data is unavailable for this playback position.";
          return;
        }
        const bins = result.bins.map((bin) => bin && Number.isFinite(bin.min) && Number.isFinite(bin.max) && bin.min >= -1 && bin.max <= 1 && bin.min <= bin.max
          ? { min: bin.min, max: bin.max } : null);
        observedWaveformSnapshotOwner = currentObservedWaveformOwnerKey();
        playbackWaveform = {
          generation: result.generation,
          trackId,
          durationSeconds: result.durationSeconds,
          bins,
          source: "observed-playback",
          complete: result.complete === true && bins.every((bin) => bin !== null),
        };
        playbackWaveformReason = typeof result.reason === "string" ? result.reason.slice(0, 180) : "";
      } catch {
        // The player remains usable when the optional waveform command is unavailable.
      } finally { inFlight = false; }
    };
    // The poll reads the waveform it is about to update. Keep those reads
    // out of the effect's dependency set so each sample does not immediately
    // restart the effect and spin another poll in a microtask loop.
    untrack(() => { void poll(); });
    const timer = observerPaused ? null : window.setInterval(() => void poll(), 1200);
    return () => { active = false; if (timer !== null) window.clearInterval(timer); };
  });

  $effect(() => {
    const detection = playingTrackDetectionView;
    const owner = currentPlayingTrackDetectionOwner();
    const durationSeconds = playerDuration;
    const range = playingTrack?.playbackRange ?? null;
    const stream = currentStreamMediaResource();
    const streamIdentity = stream ?? currentObservedStreamIdentity();
    const assetId = waveformLocalAssetId;
    const resourceKey = detection?.resourceKey;
    const sourceVariantKey = JSON.stringify(streamIdentity?.audioVariant ?? currentStreamAudioVariant());
    const ownerKey = owner && resourceKey ? `${playingTrackDetectionOwnerKey(owner)}\u0000${resourceKey}\u0000${stream?.key ?? ""}\u0000${assetId ?? ""}\u0000${sourceVariantKey}\u0000${durationSeconds}\u0000${range?.startTime ?? ""}\u0000${range?.endTime ?? ""}` : "";
    if (!owner) {
      void flushObservedWaveformDbWrites();
      observedWaveformBinding = null;
      observedWaveformBindingOwner = "";
      observedWaveformBindingSequence++;
      observedWaveformSnapshotOwner = "";
      if (playbackWaveform?.source === "observed-playback") playbackWaveform = null;
      return;
    }
    if (!resourceKey || !Number.isFinite(durationSeconds) || durationSeconds <= 0) {
      observedWaveformBinding = null;
      observedWaveformBindingOwner = "";
      observedWaveformBindingSequence++;
      return;
    }
    if (observedWaveformBindingOwner === ownerKey) return;
    void flushObservedWaveformDbWrites();
    observedWaveformBindingOwner = ownerKey;
    const sequence = ++observedWaveformBindingSequence;
    observedWaveformBinding = null;
    if (playbackWaveform?.source === "observed-playback" && observedWaveformSnapshotOwner !== ownerKey) {
      playbackWaveform = null;
      observedWaveformSnapshotOwner = "";
    }
    const isRange = Boolean(range);
    const identity = streamIdentity
      ? { kind: "stream" as const, provider: streamIdentity.provider, sourceId: streamIdentity.sourceId, audioVariant: streamIdentity.audioVariant ?? null }
      : assetId && /^[a-f0-9]{64}$/i.test(resourceKey)
        ? { kind: "localResource" as const, resourceKeySha256: resourceKey }
        : { kind: "session" as const };
    void prepareObservedWaveformBinding({
      resourceKey, trackId: owner.trackId, generation: owner.generation, durationSeconds,
      binCount: 1200, scope: isRange ? "playbackRange" : "wholeTrack",
      range: isRange ? { startSeconds: range!.startTime, endSeconds: range!.endTime } : null,
      identity,
    }).then(async binding => {
      if (sequence !== observedWaveformBindingSequence || observedWaveformBindingOwner !== ownerKey || !trackDetectionOwnerIsCurrent(owner) ||
          playingTrackDetectionView?.resourceKey !== resourceKey) return;
      observedWaveformBinding = binding;
      if (!binding) return;
      if (binding.persistenceKey) {
        try {
          let persisted = await call<unknown | null>("read_analysis_cache_record", {
            namespace: "observed-waveform-v1", key: binding.persistenceKey,
          });
          let restoredPersisted = persisted
            ? observedWaveformCache.restorePersistedRecord(binding, persisted)
            : null;
          for (const legacyKey of binding.legacyPersistenceKeys ?? []) {
            if (restoredPersisted) break;
            persisted = await call<unknown | null>("read_analysis_cache_record", {
              namespace: "observed-waveform-v1", key: legacyKey,
            });
            if (persisted) restoredPersisted = observedWaveformCache.restorePersistedRecord(binding, persisted);
          }
        } catch { /* Keep the in-memory/localStorage cache available offline. */ }
        if (sequence !== observedWaveformBindingSequence || observedWaveformBindingOwner !== ownerKey ||
            !trackDetectionOwnerIsCurrent(owner) || playingTrackDetectionView?.resourceKey !== resourceKey) return;
      }
      const current = playbackWaveform;
      if (current?.source === "local-analysis" && current.complete || current?.source === "stream-analysis" && current.complete) return;
      const restored = observedWaveformCache.restore(binding);
      if (current?.source === "observed-playback" && (!observedWaveformSnapshotOwner || observedWaveformSnapshotOwner === ownerKey) && current.generation === owner.generation && current.trackId === owner.trackId &&
          current.durationSeconds === durationSeconds && current.bins.some(Boolean)) {
        observedWaveformSnapshotOwner = ownerKey;
        const merged = observedWaveformCache.record(binding, current, { flushNow: paused });
        if (merged && sequence === observedWaveformBindingSequence && trackDetectionOwnerIsCurrent(owner)) playbackWaveform = merged;
        queueObservedWaveformDbWrite(binding);
        return;
      }
      if (restored && sequence === observedWaveformBindingSequence && observedWaveformBindingOwner === ownerKey && trackDetectionOwnerIsCurrent(owner)) {
        observedWaveformSnapshotOwner = ownerKey;
        playbackWaveform = restored;
        playbackWaveformReason = "Partial waveform restored for this exact audio source.";
      }
      queueObservedWaveformDbWrite(binding);
    }).catch(() => {});
  });
  $effect(() => {
    const binding = observedWaveformBinding;
    const snapshot = playbackWaveform;
    if (!binding || !snapshot || snapshot.source !== "observed-playback" || snapshot.complete ||
        snapshot.trackId !== binding.trackId || snapshot.generation !== binding.generation ||
        snapshot.durationSeconds !== binding.durationSeconds || snapshot.bins.length !== binding.binCount ||
        observedWaveformBindingOwner === "" || observedWaveformSnapshotOwner !== observedWaveformBindingOwner) return;
    const recorded = observedWaveformCache.record(binding, snapshot, { flushNow: paused });
    queueObservedWaveformDbWrite(binding);
    if (recorded && recorded.bins.some((bin, index) => {
      const current = snapshot.bins[index];
      return bin === null ? current !== null : !current || bin.min !== current.min || bin.max !== current.max;
    })) playbackWaveform = { ...recorded, reason: snapshot.reason };
    if (paused) {
      observedWaveformCache.flush();
      void flushObservedWaveformDbWrites();
    }
  });
  $effect(() => {
    if (paused) {
      observedWaveformCache.flush();
      void flushObservedWaveformDbWrites();
    }
  });
  onMount(() => {
    const flush = () => { observedWaveformCache.flush(); void flushObservedWaveformDbWrites(); };
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      observedWaveformDbPersistenceClosed = true;
      observedWaveformCache.flush();
      void flushObservedWaveformDbWrites();
    };
  });
  $effect(() => {
    const trackId = cachedStreamAnalysisTrackId;
    const generation = cachedStreamAnalysisGeneration;
    const resource = cachedStreamAnalysisResource;
    const ownerKey = `${trackId ?? ""}:${generation ?? ""}:${resource?.key ?? ""}`;
    if (cachedStreamAnalysisOwnerKey !== ownerKey) {
      cachedStreamAnalysisOwnerKey = ownerKey;
      cachedStreamAnalysis = null;
    }
    if (!trackId || generation === null || !resource || !cachedStreamAnalysisEligible || !cachedStreamAnalysisEventsReady ||
        cachedStreamAnalysisLookupOwnerKey === ownerKey) return;
    cachedStreamAnalysisLookupOwnerKey = ownerKey;
    const revision = untrack(() => cachedStreamAnalysisEventRevision);
    void readCachedStreamAnalysis(trackId, generation, resource, revision);
  });
  const chaptersByTrack = $derived(Object.fromEntries(queueTracks.map(track=>[track.id,track.playbackRange ? [] : candidateForQueue(track)?.chapters ?? (track.id===playingTrack?.id ? playingCandidate?.chapters ?? [] : [])])));
  const chapterRangesByTrack = $derived(Object.fromEntries(queueTracks.map(track=>{
    const candidate=candidateForQueue(track);
    const sourceChapters=chaptersByTrack[track.id]??[];
    return [track.id,Object.fromEntries(candidate?sourceChapters.flatMap(chapter=>{const range=chapterPlaybackRange(track,candidate,chapter);return range?[[chapter.id,range]]:[];}):[])];
  })));
  const currentTagJob = $derived(autotagJobs.find(job=>job.assetId===autotagReviewId));
  const currentTagAsset = $derived(autotagReviewAsset?.id===autotagReviewId ? autotagReviewAsset : assets.find(asset=>asset.id===autotagReviewId));
  const downloadTagStatuses = $derived.by(()=>Object.fromEntries(jobs.flatMap(job=>{
    const asset=downloadAssetsByJob[job.id] ?? assets.find(item=>item.path===job.outputPath);if(!asset)return[];
    const tagging=autotagJobs.find(item=>item.assetId===asset.id);if(!tagging)return[];
    const status=tagging.status;
    const review=["review","approved","no_match","failed","retry","recognition_applied"].includes(status);
    const label=status==="recognition_applied"?"Recognized tags applied": ["applied","completed"].includes(status)?"Tagged":status==="review"||status==="approved"?"Needs review":status==="no_match"?"No catalog match":status==="failed"?"Tagging failed":status==="retry"?"Retrying tag lookup":status==="disabled"?"Automatic tagging off":"Tagging…";
    return [[job.id,{label,assetId:asset.id,review}]];
  })));
  const nativeCanonicalChapter = $derived((nativeSnapshot?.chapter?.endSeconds !== undefined)
    ? chapterForRange(playingCandidate?.chapters ?? [], {startTime:nativeSnapshot.chapter.startSeconds,endTime:nativeSnapshot.chapter.endSeconds})
    : null);
  const activeChapter = $derived((nativeSnapshot?.chapter)
    ? nativeCanonicalChapter ?? {title:nativeSnapshot.chapter.title,startTime:nativeSnapshot.chapter.startSeconds,endTime:nativeSnapshot.chapter.endSeconds ?? Number.POSITIVE_INFINITY,id:"native",confidence:"native"}
    : playingCandidate?.chapters?.find((item) => position >= item.startTime && position < item.endTime));
  const playerAudioSource = $derived.by(() => {
    const approximateNativeSource = (nativeSnapshot?.quality?.sourceKind === "approximateAlbumExcerpt");
    const excerpt = (nativeSnapshot?.quality?.approximateExcerpt);
    const approximateLocalAsset = false;
    if (approximateNativeSource || approximateLocalAsset) {
      const sourceId = excerpt?.source?.id;
      const candidateProvider = (nativeSnapshot?.candidate?.provider);
      const provider = excerpt?.source?.provider || candidateProvider || preview?.provider ||
        (typeof sourceId === "string" ? sourceId.split(":", 1)[0] : undefined);
      const names: Record<string, string> = { youtube: "YouTube", soundcloud: "SoundCloud", monochrome: "Monochrome" };
      const label = excerpt?.label || nativeSnapshot?.quality?.label || "Approximate album excerpt";
      return provider ? `${label} · ${names[provider] ?? provider}` : label;
    }
    if (!preview || !["audio", "local", "direct"].includes(preview.kind)) return "";
    if (preview.kind === "local") return "Local file";
    const providerId = preview.provider || (preview.isExcerpt ? playingTrack?.provider : playingCandidate?.provider);
    const names: Record<string, string> = { youtube: "YouTube", soundcloud: "SoundCloud", monochrome: "Monochrome", beatport: "Beatport", traxsource: "Traxsource", apple: "Apple Music", itunes: "Apple Music" };
    let source = providerId ? names[providerId] : undefined;
    if (!source) { try { source = new URL(preview.url).hostname; } catch { /* No source identity available. */ } }
    return source ? `${preview.isExcerpt ? "Preview from" : "Streaming from"} ${source}` : "Streaming audio";
  });
  const activeRecordingMatch = $derived(((nativeSnapshot?.candidate?.sourceMetadata?.matching as Preview["matching"] | undefined) ?? nativeSnapshot?.quality?.matching));
  const playingTitle = $derived(playingTrack?.playbackRange ? playingTrack.title : activeChapter?.title ?? playingTrack?.title ?? preview?.title ?? "Nothing playing");
  const playbackActionTrack = $derived.by(()=>{const chapter=(nativeCanonicalChapter);const track=currentRadioSong ?? (playingTrack && !playingTrack.playbackRange && playingCandidate && chapter ? virtualChapterTrack(playingTrack,playingCandidate,chapter) : null) ?? playingTrack;return track?{...track,favorite:favoriteTrackIds.includes(track.id)||track.favorite}:null;});
  const expandedPlayingTrack = $derived.by(() => {
    const track = playbackActionTrack;
    const match = activeRecordingMatch;
    if (!track || match?.assessment.relationship !== "alternative") return track;
    const source = (nativeSnapshot?.candidate);
    return {...track, title: match.resolvedRecording.title, artist: match.resolvedRecording.artist ?? "Alternative performer",
      provider: source?.provider ?? "unknown", providerId: source?.id ?? "", sourceUrl: source?.url ?? "",
      version: match.resolvedRecording.version, album: null, albumArtist: null, label: null, remixers: null,
      isrc:null,catalogNumber:null,releaseDate:null,recordingDate:null,genre:null,normalizedGenre:null,
      trackNumber:null,trackTotal:null,discNumber:null,discTotal:null,durationSeconds:source?.durationSeconds ?? null,
      artworkUrl: source?.artworkUrl ?? null, provenance: {}};
  });
  function seekChapter(start: number) { void seekPlayer(playingTrack ? chapterRelativePosition(playingTrack,start) : start); }

  const contextKey = () =>
    `${view}:${view === "collections" ? collectionId : ""}`;
  async function navigate(next: View, recordRoute = true) {
    if (recordRoute && (next !== view || !["library", "favorites", "history", "downloads", "settings"].includes(next))) rememberRoute();
    if (next !== "search") clearSearchPanelInstance();
    if (next !== "artist") clearNativeArtistInstance();
    if (next === "search" && !restoringRoute) {
      if (searchRelease) clearSearchPanelInstance();
      searchNavigationSnapshotKey = null;searchRelease=null;searchReleaseRequest++;
    }
    if(next!=="search")searchReleaseRequest++;
    setNotice("");
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
    if (next === "search" && desktop()) {
      try {
        const sources = await call<typeof availableSources>("list_search_sources");
        if (request === navigationRequest && view === "search") availableSources = sources;
      } catch { /* Retain the last known capabilities when discovery is unavailable. */ }
    }
    if (next !== "history") historyRequest++;
    if (["favorites", "collections", "library"].includes(next))
      await run(async () => {
        await loadLocalPage();
      });
    if (next === "history") await loadHistory();
    if (next === "downloads") await refreshJobs();
    if (next === "settings")
      await run(
        async () =>
          (dependencies = await call<Dependency[]>("check_dependencies")),
      );
    requestAnimationFrame(() => {
      if (request === navigationRequest && view === next && table) table.scrollTop = cached?.scroll ?? 0;
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
      selected = null;
      candidates = [];
      setNotice(result.warning ?? (result.stale ? "Showing cached chart." : ""));
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
  function select(t: Track, source: "click" | "navigation" = "click", displayedIndex?: number) {
    if (radioBusy && radioBusy !== t.id) { radioRequest++; radioBusy = null; }
    selected = t;
    candidates = [];
    const request = ++selectionRequest;
    void loadMemberships(t, request);
    clearTimeout(autoTimer);
    const displayed = [...sorted];
    const index = displayedIndex ?? displayed.findIndex(item => item === t);
    const list = displayed.some(item => item.id === t.id) ? displayed : [t];
    const selectedIndex = index >= 0 ? index : Math.max(0, list.findIndex(item => item.id === t.id));
    if (source === "click") {
      void startPreview(t, false, false, list, selectedIndex);
    } else if (settings.autoPreview) {
      autoTimer = setTimeout(
        () => void startPreview(t, false, false, list, selectedIndex),
        settings.previewDelayMs,
      );
    }
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
  type PortablePlaybackSource = { provider: "youtube" | "soundcloud" | "monochrome"; sourceId: string; matchingLevel: "exact" | "balanced" | "flexible"; sourceHash: string };
  function portablePlaybackSource(track: Track): PortablePlaybackSource | null {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown> : {};
    const value = provenance.portablePlaybackSource;
    if (!value || typeof value !== "object") return null;
    const source = value as Record<string, unknown>;
    if (!(["youtube", "soundcloud", "monochrome"].includes(String(source.provider)) &&
        typeof source.sourceId === "string" && /^[A-Za-z0-9:_-]{1,512}$/.test(source.sourceId) &&
        ["exact", "balanced", "flexible"].includes(String(source.matchingLevel)) &&
        typeof source.sourceHash === "string" && /^[a-f0-9]{64}$/i.test(source.sourceHash))) return null;
    return {
      provider: source.provider as PortablePlaybackSource["provider"], sourceId: source.sourceId,
      matchingLevel: source.matchingLevel as PortablePlaybackSource["matchingLevel"], sourceHash: source.sourceHash,
    };
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

  async function ensureLoudnessSettings(): Promise<LoudnessSettingsValue> {
    if (!loudnessSettingsLoaded) {
      try {
        const loaded = await call<LoudnessSettingsValue | null>("get_loudness_settings");
        if (loaded && ["off", "track", "album"].includes(loaded.mode) &&
            Number.isFinite(loaded.targetLufs) && Number.isFinite(loaded.ceilingDbtp) &&
            Number.isFinite(loaded.maximumBoostDb) && typeof loaded.albumTrackFallback === "boolean") {
          loudnessSettings = { ...loaded, analyzeWhenIdle: loaded.analyzeWhenIdle === true };
        } else {
          loudnessSettings = { mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
        }
      } catch {
        loudnessSettings = { mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
      }
      loudnessSettingsLoaded = true;
    }
    return loudnessSettings;
  }
  onMount(() => { void ensureLoudnessSettings(); });

  function currentStreamAudioVariant(): StreamAudioVariant | null {

      const quality = nativeSnapshot?.quality;
      return quality ? { formatId: quality.formatId, codec: quality.codec, mimeType: quality.mimeType,
        bitrateKbps: quality.bitrateKbps, expectedContentLength: quality.expectedContentLength,
        containsVideo: quality.formatHasVideo } : null;

  }
  type ActiveStreamMediaResource = StreamMediaResource & { key: string };
  function currentStreamMediaResource(): ActiveStreamMediaResource | null {
    const track = playingTrack;
    const candidate = (nativeSnapshot?.candidate);
    const durationSeconds = (nativeSnapshot?.duration || candidate?.durationSeconds || track?.durationSeconds || 0);
    if (!track || !candidate || playingRadioStation || track.localPath || track.playbackRange ||
        (nativeSnapshot?.quality?.sourceKind === "localAsset") ||
        false ||
        !["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider) ||
        !/^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id) || !Number.isFinite(durationSeconds) || durationSeconds < 8 || durationSeconds > 900) return null;
    const resource = { trackId: track.id, provider: candidate.provider, sourceId: candidate.id, durationSeconds, audioVariant: currentStreamAudioVariant(),
      sessionGeneration: (nativeSnapshot?.generation) };
    return { ...resource, key: streamMediaResourceKey(resource) };
  }
  function currentObservedStreamIdentity(): { provider: string; sourceId: string; audioVariant: StreamAudioVariant | null } | null {
    const track = playingTrack;
    const candidate = (nativeSnapshot?.candidate);
    // A catalog track can retain a local path while an explicitly selected
    // remote source is playing. Bind observations to that active source.
    if (!track || !candidate || playingRadioStation ||
        (nativeSnapshot?.quality?.sourceKind === "localAsset") ||
        false ||
        !["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider) ||
        !/^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id)) return null;
    return { provider: candidate.provider, sourceId: candidate.id, audioVariant: currentStreamAudioVariant() };
  }
  function currentObservedWaveformOwnerKey(): string {
    const owner = currentPlayingTrackDetectionOwner();
    const resourceKey = playingTrackDetectionView?.resourceKey;
    if (!owner || !resourceKey) return "";
    const range = playingTrack?.playbackRange ?? null;
    const stream = currentStreamMediaResource();
    const streamIdentity = stream ?? currentObservedStreamIdentity();
    return `${playingTrackDetectionOwnerKey(owner)}\u0000${resourceKey}\u0000${stream?.key ?? ""}\u0000${waveformLocalAssetId ?? ""}\u0000${JSON.stringify(streamIdentity?.audioVariant ?? currentStreamAudioVariant())}\u0000${playerDuration}\u0000${range?.startTime ?? ""}\u0000${range?.endTime ?? ""}`;
  }
  function streamMediaAnalysisPlaying(): boolean {
    return (nativeSnapshot?.status === "playing" && nativeSnapshot.paused === false);
  }
  function streamMediaAnalysisUnavailableReason(): string {
    if (playingRadioStation) return "Full-track analysis is unavailable for live radio.";
    if (!playingTrack || playingTrack.localPath || (nativeSnapshot?.quality?.sourceKind === "localAsset") ||
        false) return "";
    if (playingTrack.playbackRange || preview?.isExcerpt || preview?.approximateExcerpt) return "Full-track analysis is unavailable for an excerpt or playback range.";
    const candidate = (nativeSnapshot?.candidate);
    if (!candidate) return "A stable verified stream identity is unavailable for this playback.";
    if (!["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider)) return "This provider does not support full streamed-track analysis.";
    const duration = (nativeSnapshot?.duration ?? 0);
    if (!Number.isFinite(duration) || duration < 8 || duration > 900) return "Full analysis requires a verified stream duration between 8 seconds and 15 minutes.";
    return "This playback route cannot provide an eligible full stream resource.";
  }
  async function installStreamMediaWaveform(
    analysis: WholeTrackAnalysisDto,
    resource: StreamMediaResource,
    generation: number,
    stillCurrent: () => boolean = () => waveformEnabled && waveformGeneration === generation && currentStreamMediaResource()?.key === streamMediaResourceKey(resource),
  ): Promise<boolean> {
    if (!stillCurrent()) return false;
    const snapshot = await verifiedStreamWaveform(analysis, resource, generation);
    if (!snapshot || !stillCurrent()) return false;
    playbackWaveform = snapshot;
    playbackWaveformReason = "Full waveform ready for this recording.";
    return true;
  }
  async function cancelStreamMediaAnalysis(statusMessage = "Stream analysis cancelled."): Promise<void> {
    const job = streamMediaAnalysisJob;
    const wasStarting = Boolean(streamMediaAnalysisStartingKey);
    const wasRunning = Boolean(job && !["completed", "failed", "cancelled"].includes(job.state));
    streamMediaAnalysisRequest++;
    if (wasRunning && job) {
      await call("cancel_selected_stream_analysis", { jobId: job.jobId }).catch(() => {});
    }
    streamMediaAnalysisJob = null;
    streamMediaAnalysisStartingKey = "";
    manualStreamMediaAnalysisKey = "";
    if (wasStarting || wasRunning) streamMediaAnalysisStatus = statusMessage;
  }
  async function startStreamMediaAnalysis(resource: ActiveStreamMediaResource, automatic = false): Promise<void> {
    if (streamMediaAnalysisStartingKey === resource.key) return;
    if (streamMediaAnalysisJob && !["completed", "failed", "cancelled"].includes(streamMediaAnalysisJob.state)) {
      if (streamMediaAnalysisJob.resourceKey === resource.key) return;
      await cancelStreamMediaAnalysis();
    }
    manualStreamMediaAnalysisKey = automatic ? "" : resource.key;
    const request = ++streamMediaAnalysisRequest;
    streamMediaAnalysisStartingKey = resource.key;
    streamMediaAnalysisResult = null;
    streamMediaAnalysisResultKey = "";
    streamMediaAnalysisStatus = automatic ? "Starting background stream analysis…" : "Starting full stream analysis…";
    try {
      const trackContext = playingTrack;
      if (!trackContext || trackContext.id !== resource.trackId) {
        if (request === streamMediaAnalysisRequest) streamMediaAnalysisStartingKey = "";
        return;
      }

      const started = await call<SelectedStreamJobDto>("start_stream_media_analysis", {
        track: streamMediaTrackContext(trackContext, resource.provider, resource.sourceId),
        provider: resource.provider, sourceId: resource.sourceId, durationSeconds: resource.durationSeconds, audioVariant: resource.audioVariant ?? null, automatic,

      });
      if (request !== streamMediaAnalysisRequest) { await call("cancel_selected_stream_analysis", { jobId: started.jobId }).catch(() => {}); return; }
      streamMediaAnalysisStartingKey = "";
      streamMediaAnalysisJob = { jobId: started.jobId, resourceKey: resource.key, state: started.state };
      while (request === streamMediaAnalysisRequest) {
        const current = await call<SelectedStreamJobDto>("get_selected_stream_analysis", { jobId: started.jobId });
        if (request !== streamMediaAnalysisRequest) return;
        const result = current.results.find(row => row.trackId === resource.trackId);
        streamMediaAnalysisJob = { jobId: current.jobId, resourceKey: resource.key, state: fullStreamAnalysisOutcome(current.state, result) };
        streamMediaAnalysisStatus = current.state === "completed"
          ? result?.analysis && isCompleteStreamMediaWaveform(result.analysis)
            ? "Full streamed-track BPM, key, and waveform analysis is saved."
            : result?.error ?? "Full waveform analysis is unavailable for this stream."
          : current.state === "cancelled" || current.state === "failed"
            ? result?.error ?? `Stream analysis ${current.state}.`
            : `Analyzing verified stream · ${current.completed} of ${current.total}`;
        if (current.state === "completed" || current.state === "cancelled" || current.state === "failed") {
          if (current.state === "completed" && result?.status === "completed" && result.analysis) {
            streamMediaAnalysisResult = result.analysis;
            streamMediaAnalysisResultKey = resource.key;
            const currentResource = currentStreamMediaResource();
            if (currentResource?.key === resource.key && waveformEnabled && waveformGeneration !== null) {
              if (await installStreamMediaWaveform(result.analysis, resource, waveformGeneration)) streamWaveformCacheLookupOwner = `${resource.key}:${waveformGeneration}`;
              else streamMediaAnalysisStatus = "BPM/key analysis is cached, but a complete matching stream waveform is unavailable.";
            }
          }
          break;
        }
        await new Promise(resolve => window.setTimeout(resolve, 650));
      }
    } catch (failure) {
      if (request === streamMediaAnalysisRequest) {
        streamMediaAnalysisStartingKey = "";
        streamMediaAnalysisJob = streamMediaAnalysisJob ? { ...streamMediaAnalysisJob, state: "failed" } : null;
        streamMediaAnalysisStatus = `Stream analysis unavailable: ${failure instanceof Error ? failure.message : String(failure)}`;
      }
    }
  }
  $effect(() => {
    const enabled = desktop() && streamMediaAnalysisEnabled && streamMediaAnalysisPreferenceReady;
    const restorePolicyReady = streamMediaAnalysisRestorePolicyReady;
    const restorePaused = streamMediaAnalysisRestorePaused;
    const reviewRequired = streamMediaAnalysisRestoreReviewRequired;
    const resource = currentStreamMediaResource();
    const playing = streamMediaAnalysisPlaying();
    const job = streamMediaAnalysisJob;
    const manual = Boolean(resource && manualStreamMediaAnalysisKey === resource.key);
    if (streamMediaAnalysisStartingKey && (!enabled && !manual || streamMediaAnalysisStartingKey !== resource?.key)) {
      void cancelStreamMediaAnalysis(enabled ? "Analysis stopped because the playing source changed." : "Background analysis is off; verified waveform cache remains available.");
      return;
    }
    if (job && (!enabled && !manual || job.resourceKey !== resource?.key)) {
      void cancelStreamMediaAnalysis(enabled ? "Analysis stopped because the playing source changed." : "Background analysis is off; verified waveform cache remains available.");
      return;
    }
    if (manual) return;
    if (!resource && waveformLocalAssetId) {
      streamMediaAnalysisStatus = "";
      return;
    }
    if (enabled && playing && !resource) {
      streamMediaAnalysisStatus = streamMediaAnalysisUnavailableReason();
      return;
    }
    if (enabled && playing && !restorePolicyReady) {
      streamMediaAnalysisStatus = "Checking restore work policy before background stream analysis.";
      return;
    }
    if (enabled && playing && restorePaused) {
      streamMediaAnalysisStatus = reviewRequired
        ? "Background stream analysis is paused until restore review. You can start a one-time analysis manually."
        : "Background stream analysis is paused for this restored profile. You can start a one-time analysis manually.";
      return;
    }
    if (!claimAutomaticStreamMediaAnalysis(enabled, playing, resource?.key ?? null, attemptedStreamMediaAnalysis) || !resource) return;
    void startStreamMediaAnalysis(resource, true);
  });

  async function refreshNormalizationSettings(savedSettings?: LoudnessSettingsValue): Promise<void> {
    if (savedSettings) {
      loudnessSettings = savedSettings;
      loudnessSettingsLoaded = true;
    } else {
      loudnessSettingsLoaded = false;
      await ensureLoudnessSettings();
    }
    return;

  }

  function playbackStartupTimeoutSeconds(): number {
    return [20, 30, 45].includes(Number(settings.playbackStartupTimeoutSeconds)) ? Number(settings.playbackStartupTimeoutSeconds) : 20;
  }

  function sourceIsAuthoritative(track: Track): boolean {
    return hasExplicitSourceLock(track, {selectedCandidate:queueCandidates[track.id],audioSearchResult:audioSearchTrackIds.has(track.id),overridePlaylistTrackUrls:settings.overridePlaylistTrackUrls === true,localPathPins:false});
  }

  onMount(() => () => {
    previewRequests.next();
    if (nativeHistorySummaryTimer !== null) clearTimeout(nativeHistorySummaryTimer);
    nativeHistorySummaryTimer = null;
    pendingNativeHistorySummaryOwner = "";
    cancelRadioIdentification();
  });
  function safePreviewError(reason: unknown): string {
    const message = reason instanceof Error ? reason.message : String(reason ?? "");
    // These resolver outcomes are fixed safe contracts. Keep them whole even
    // when they are composed into a longer skipped-track notice.
    const fixedResolverFailure = Object.values(FIXED_RESOLVER_FAILURES).find(value => value === message);
    if (fixedResolverFailure) return fixedResolverFailure;
    const missingDurationExplanation = "Saved recording duration is missing; no source provided enough verified identity evidence for automatic playback.";
    const nativeMissingDurationSuffix = `. ${missingDurationExplanation} Open the catalog source or choose another source`;
    const normalNoMatchSuffix = ". No available source matched the requested recording and version. Open the catalog source or choose another source";
    if (message.startsWith("No playable audio match was found for ") && message.endsWith(normalNoMatchSuffix)) {
      return "No playable audio match was found for the requested recording and version. Open the catalog source or choose another source.";
    }
    // Preserve the fixed missing-duration explanation before the normal length
    // cap can clip it. Do not surface interpolated metadata or raw error text.
    if (message.startsWith("No playable audio match was found for ") && message.endsWith(nativeMissingDurationSuffix)) {
      return `No playable audio match was found. ${missingDurationExplanation}`;
    }
    return safeNativeFailurePart(message, "No playable audio stream was returned.");
  }

  function toggleWaveformSeek(): void {
    if (waveformEnabled) {
      waveformEnabled = false;
      waveformRouteUpgradeRequest++;
      waveformRouteUpgradeStatus = "";
      return;
    }
    waveformEnabled = true;
    waveformRouteUpgradeStatus = "";
    void Promise.resolve(null);
  }

  let resolvingPlayback = $state(false), bufferingAudio = $state(false);
  let playbackLoadSequence = 0;
  const playbackLoading = $derived(resolvingPlayback || bufferingAudio);
  const playerErrorMessage = $derived.by(() => {
    if (playbackLoading) return "";

      if (!nativeSnapshot || !nativeSnapshotFailure(nativeSnapshot)) return "";
      const diagnostic = nativeFailureMessage(nativeSnapshot?.diagnostics?.lastSourceFailure);
      if (diagnostic) return diagnostic;
      return safeNativeFailurePart(nativeSnapshot.playerMessage, "Playback failed.");

  });
  const playerStatusMessage = $derived.by(() => {

      switch (nativeSnapshot?.status) {
        case "failed": return "Playback unavailable";
        case "buffering": return "Buffering…";
        case "resolving": return "Finding audio…";
        case "paused": return "Paused";
        case "playing": return nativeSnapshot.playerMessage || "Playing";
        default: return nativeSnapshot?.playerMessage || (paused ? "Paused" : "Ready");
      }

  });
  async function startPreview(t: Track, fromQueue = false, automaticContinuation = false, displayedTracks?: Track[], displayedIndex?: number, failedSkips = 0, automationRequestId: string | null = null, resumeSavedPosition = false, failedQueueRun?: FailedQueueRun, loudnessAlbumResource: LoudnessResourceIdentity | null = null, advanceResumeFailure = false) {
    if (!automaticContinuation) clearPlaybackSkipNotice();
    const entryId = nativeSnapshot?.entryId;
    const resource = loudnessAlbumResource ?? ((fromQueue || resumeSavedPosition) && entryId
      ? nativeSnapshot?.queueLoudnessContexts?.[entryId] ?? null : null);
    const sequence = ++playbackLoadSequence;
    resolvingPlayback = true;
    try {
      await startPreviewInner(t, fromQueue, automaticContinuation, displayedTracks, displayedIndex, failedSkips,
        automationRequestId, playbackOperationTrace(), resumeSavedPosition, failedQueueRun, resource, advanceResumeFailure, sequence);
      return sequence === playbackLoadSequence;
    } finally { if (sequence === playbackLoadSequence) resolvingPlayback = false; }
  }
  function sameNativePlaybackTrack(left: Track, right: Track) {
    const a = left.playbackRange, b = right.playbackRange;
    const sameRange = !a && !b || Boolean(a && b && a.parentTrackId === b.parentTrackId && a.chapterId === b.chapterId && a.sourceUrl === b.sourceUrl && a.startTime === b.startTime && a.endTime === b.endTime);
    return left.id === right.id && left.provider === right.provider && left.providerId === right.providerId &&
      left.sourceUrl === right.sourceUrl && left.isrc === right.isrc &&
      left.artist === right.artist && left.title === right.title && (left.version ?? null) === (right.version ?? null) &&
      (left.durationSeconds ?? null) === (right.durationSeconds ?? null) && (left.localPath ?? null) === (right.localPath ?? null) &&
      (left.assetId ?? null) === (right.assetId ?? null) && sameRange &&
      JSON.stringify(portablePlaybackSource(left)) === JSON.stringify(portablePlaybackSource(right)) &&
      JSON.stringify(left.provenance) === JSON.stringify(right.provenance);
  }
  function sameNativePlaybackQueue(left: readonly Track[], right: readonly Track[]) {
    return left.length === right.length && left.every((track, index) => sameNativePlaybackTrack(track, right[index]!));
  }
  function nativeQueueSelectIdentity(index: number, displayed: readonly Track[]) {
    const snapshot = nativeSnapshot;
    const entryIds = snapshot?.queueEntryIds;
    if (!snapshot || !Number.isSafeInteger(snapshot.queueRevision) || snapshot.queueRevision! < 0 || !Array.isArray(entryIds) ||
        entryIds.length !== displayed.length || index < 0 || index >= displayed.length ||
        !sameNativePlaybackQueue(snapshot.queue ?? [], displayed)) return null;
    const expectedEntryId = entryIds[index];
    if (typeof expectedEntryId !== "string" || !expectedEntryId || entryIds.filter(id => id === expectedEntryId).length !== 1) return null;
    return {expectedQueueRevision:snapshot.queueRevision!, expectedEntryId};
  }
  async function startPreviewInner(t: Track, fromQueue = false, automaticContinuation = false, displayedTracks?: Track[], displayedIndex?: number, failedSkips = 0, automationRequestId: string | null = null,trace?:PlaybackOperationTrace,resumeSavedPosition = false, failedQueueRun?: FailedQueueRun,loudnessAlbumResource: LoudnessResourceIdentity | null = null,advanceResumeFailure = false, intentSequence = playbackLoadSequence) {
    if (intentSequence !== playbackLoadSequence) return;
    playbackRunGeneration++;
    cancelRadioIdentification();

      clearTimeout(autoTimer); setAutomationPlaybackOwner(automationRequestId); systemPausedTrack=false;
      const list = displayedTracks ?? (fromQueue ? queueTracks : view === "history" ? historyItems.map(item => item.track) : sorted.some(item => item.id === t.id) ? [...sorted] : [t]);
      const index = displayedIndex ?? (fromQueue ? queueTracks.findIndex(item => item.id === t.id) : list.findIndex(item => item.id === t.id));
      // Native resolves the reviewed portable identity itself under its queue
      // ownership, so a failed lookup can advance the occurrence safely.
      const candidate = !portablePlaybackSource(t) && sourceIsAuthoritative(t) ? candidateForQueue(t) ?? importedAudioCandidate(t) ?? undefined : undefined;
      const canSelectFromNativeQueue = index >= 0 && sameNativePlaybackQueue(nativeSnapshot?.queue ?? [], list) &&
        (!candidate || !queueCandidates[t.id] && candidate.url === t.sourceUrl);
      // Explicit track selection starts at the beginning. The native Resume command
      // owns restoration of paused playback, including its saved playhead.
      if (canSelectFromNativeQueue) {
        const occurrence = nativeQueueSelectIdentity(index, list);
        if (!occurrence) { error = "The playback queue identity is unavailable. Refresh the queue and try again."; return; }
        await nativeCommand({action:"select",index,...occurrence,loudnessAlbumResource,...trace});
      } else await nativeCommand({action:"play",tracks:list.length ? list : [t],index:index >= 0 ? index : 0,candidate,position:0,loudnessAlbumResource,...trace});
      return;

  }

  function togglePlayback() {
     toggleCurrentPlayback(); return;

  }
  function cancelNaturalQueueRadioContinuation() {
    playbackRunGeneration++;
    queueRadio.cancel();
    preparedRadioKey = "";
  }
  function toggleCurrentPlayback() {

    if(!paused)reportQueuedPlaybackPhase("paused");

      if (paused && nativePlaybackStatus === "ended" && nativeNaturalQueueEnd) cancelNaturalQueueRadioContinuation();
      void nativeCommand({action:paused ? "resume" : "pause"});
      return;

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
      table.querySelector(".shared-track-list li.selected")?.scrollIntoView({ block: "nearest" });
      return;
    }
    const index = sorted.findIndex((t) => t.id === id),
      top = index * rowHeight;
    if (top < table.scrollTop) table.scrollTop = top;
    else if (top + rowHeight > table.scrollTop + table.clientHeight - 38)
      table.scrollTop = top + rowHeight - table.clientHeight + 38;
    scrollTop = table.scrollTop;
  }
  async function refreshCollections() { collections = await call<Collection[]>("list_collections"); }
  async function refreshFavorites() {
    favoriteTrackIds = await call<string[]>("list_favorite_track_ids");
    const ids = new Set(favoriteTrackIds);
    const apply = (track:Track) => ({...track, favorite:ids.has(track.id)});
    tracks=tracks.map(apply); historyItems=historyItems.map(entry=>({...entry,track:apply(entry.track)}));
    queueTracks=queueTracks.map(apply); if(playingTrack)playingTrack=apply(playingTrack); if(currentRadioSong)currentRadioSong=apply(currentRadioSong); if(infoTrack)infoTrack=apply(infoTrack);
    await refreshCollections();
  }
  function historySearchOptions() {
    return {availability:historyDownloadAvailability,query:historyQuery,provider:historyProvider||null,stationId:historyStationId||null,from:historyFrom?new Date(`${historyFrom}T00:00:00`).toISOString():null,to:historyTo?new Date(`${historyTo}T23:59:59.999`).toISOString():null,sort:historyOrder,sortField:historySortField,direction:historySortField==="sourceOrder"?(historyOrder==="oldest"?"asc":"desc"):historySortDirection,minDurationSeconds:parseDurationBound(historyMinDuration),maxDurationSeconds:parseDurationBound(historyMaxDuration)};
  }
  const historyOptionsPending = $derived(historyAppliedOptions !== JSON.stringify(historySearchOptions()));
  async function loadHistory(append=false) {
    const request=++historyRequest; historyBusy=true;
    if (!append) historyScroll=0;
    try {
      const searchOptions = historySearchOptions();
      const searchSignature = JSON.stringify(searchOptions);
      const shouldAppend = append && historyAppliedOptions === searchSignature;
      const page=await call<{items:typeof historyItems;total:number;hasMore:boolean}>("search_playback_history",{options:{...searchOptions,limit:200,offset:shouldAppend?historyItems.length:0}});
      if(request!==historyRequest)return;
      historyItems=shouldAppend?[...historyItems,...page.items]:page.items;historyAppliedOptions=searchSignature;historyTotal=page.total;historyHasMore=page.hasMore;
    } catch(failure) { if(request===historyRequest)error=String(failure); }
    finally {if(request===historyRequest)historyBusy=false;}
  }
  function historySource(track:Track, index:number) {
    const provenance=(track.provenance??{}) as Record<string,unknown>;
    return typeof provenance.radioStationName === "string" ? `Via ${provenance.radioStationName}` : displayedHistoryItems[index]?.provider;
  }
  async function appendPlaylistItems(args: { collectionId: string; tracks: Track[] }) {
    const target = args.collectionId;
    const unique = target === "favorites" || target === "history" || target === DOWNLOADS_PLAYLIST_ID || target === playlistPreferences.favoritesPlaylistId;
    await call(unique ? "add_to_collection" : "append_playlist_entries", args);
  }
  async function invokePlaylistShortcut(slot:number) {
    const track=playbackActionTrack;
    if(!track){setNotice("Play a track before using a playlist shortcut.");return;}
    playlistPreferences=await call<PlaylistPreferences>("get_playlist_preferences");
    const target=playlistPreferences.shortcuts[slot];
    if(!target){shortcutSlot=slot;await addArtistTrack(track);return;}
    await run(async()=>{await appendPlaylistItems({collectionId:target,tracks:[track]});await refreshFavorites();setNotice(`Added ${track.title} to ${collections.find(item=>item.id===target)?.name??"playlist"}.`);});
  }
  function playlistShortcut(event:KeyboardEvent):number {
    if(event.repeat || (event.target as HTMLElement)?.closest("input,textarea,select,[contenteditable],dialog") || downloadTrack || playlistTrack || renameTarget)return -1;
    const parts=[event.ctrlKey?"Ctrl":"",event.altKey?"Alt":"",event.shiftKey?"Shift":"",event.metaKey?"Meta":"",/^Digit[0-9]$/.test(event.code)?event.code.slice(5):/^Key[A-Z]$/.test(event.code)?event.code.slice(3):event.key.length===1?event.key.toUpperCase():event.key].filter(Boolean).join("+");
    return (playlistPreferences.shortcutBindings??Array.from({length:9},(_,i)=>`Alt+${i+1}`)).indexOf(parts);
  }
  async function favorite(t: Track) {
    await run(async () => {
      await call("set_favorite", { track: t, value: !t.favorite });
      t.favorite = !t.favorite;
      await refreshFavorites();
    });
  }
  async function disposition(t: Track, value: string) {
    await run(async () => {
      await call("set_disposition", { track: t, value });
      t.disposition = value;
      tracks = [...tracks];
    });
  }
  async function handleAutomationIdentify(request:{captureId:string;trackId:string;sessionId:string;generation:number;seekSerial:number;chapterId?:string|null}) {
    const requestChapterId=request?.chapterId??null;

      await call("automation_identify_complete",{captureId:request?.captureId??"",result:{},error:"Playback changed before identification began"}).catch(()=>{});
      return;

  }
  function cancelAutomationIdentify(request:{captureId:string}) {
    if(request?.captureId && radioIdentifyCaptureId===request.captureId)cancelRadioIdentification(request.captureId);
  }
  function automationPlaybackSnapshot(forceFull = false) {
    const projection=automationQueueProjection;
    resetTrackFailureHistoryForQueue(projection.revision, projection.sourceKey);
    const base={ trackId: playingTrack?.id ?? null, entryId: projection.entryIds[queueIndex] ?? null, sessionId: playbackSession, chapterId: false, track: playingTrack ? { id: playingTrack.id, artist: playingTrack.artist, title: playingTitle, album: playingTrack.album, artworkUrl: playingTrack.artworkUrl, artworkFilePath: artworkFilePaths.get(playingTrack.id) ?? null, localPath: playingTrack.localPath, durationSeconds: playingTrack.durationSeconds, sourceUrl: playingTrack.sourceUrl } : null,
      transition:nativeSnapshot?.transition ?? null, paused, position:playerPosition, absolutePosition:position, sourceGeneration:nativeResolutionGeneration ?? preview?.sourceGeneration ?? null, duration:playerDuration, volume, seekSerial, canSeek: Boolean(preview?.canSeek), canVolume: Boolean(preview?.canVolume), shuffle: shuffleEnabled, repeat: repeatMode, previewKind: preview?.kind ?? null, playerMessage: previewStatus, radioNowPlaying:currentRadioSong?{id:currentRadioSong.id,title:currentRadioSong.title,artist:currentRadioSong.artist,stationName:playingRadioStation?.name}:null,radioHistory:radioQueueHistory.map(item=>({id:item.id,trackId:item.track.id,title:item.track.title,artist:item.track.artist,playedAt:item.playedAt,stationName:item.stationName})), status: playbackLoading ? "buffering" : preview ? (paused ? "paused" : "playing") : systemPausedTrack ? "paused" : "idle", currentIndex: queueIndex };
    return automationPlaybackPublisher.next(base,projection.revision,projection.queue,forceFull,projection.entryIds,{revision:String(failureHistoryRevision),failures:recentTrackFailures});
  }
  async function publishAutomationPlayback() {
    if (!desktop()) return;
    try {
      let envelope=automationPlaybackSnapshot();
      let ack=await call<PlaybackStateAck|undefined>("automation_playback_state",{playback:envelope.playback});
      const outcome=automationPlaybackPublisher.acknowledge(envelope,ack);
      if(outcome==="retry_full"){
        envelope=automationPlaybackSnapshot(true);
        ack=await call("automation_playback_state",{playback:envelope.playback});
        automationPlaybackPublisher.acknowledge(envelope,ack);
      }
    } catch {}
  }
  async function handleAutomationPlayback(request: { requestId: string; action: string; trackIds?: string[]; mode?: string | null; position?: number | null; volume?: number | null }) {

    let failure: string | null = null;
    const resumeOwner=queuedOwnerForResume(request.requestId);
    if (["play", "next", "previous", "openUri"].includes(request.action) || request.action==="resume" && resumeOwner===request.requestId)setAutomationPlaybackOwner(request.requestId);
    try {
      if (request.action === "play" || request.action === "queue") {
        const trackIds = request.trackIds ?? [];
        if (!trackIds.length || trackIds.some((id) => typeof id !== "string" || !id.trim())) throw new Error("Unknown or empty track IDs");
        const loaded = await call<Track[]>("get_tracks", { ids: trackIds });
        if (!Array.isArray(loaded) || loaded.length !== trackIds.length) throw new Error("Unknown or empty track IDs");
        const tracksById = new Map<string, Track[]>();
        for (const track of loaded) {
          if (!track || typeof track.id !== "string") throw new Error("Unknown or empty track IDs");
          const matches = tracksById.get(track.id) ?? [];
          matches.push(track);
          tracksById.set(track.id, matches);
        }
        const list = trackIds.map((id) => tracksById.get(id)?.shift());
        if (list.some((track) => !track) || [...tracksById.values()].some((matches) => matches.length)) throw new Error("Unknown or empty track IDs");
        const requestedTracks = list as Track[];
        if (request.action === "play") {
          if (activeAutomationPlayback !== request.requestId) throw new Error("Playback request was cancelled or superseded");

            await startPreview(requestedTracks[0]!, false, false, requestedTracks, 0, 0, request.requestId);
            if (audioRecovery) await audioRecovery;
            if (playingTrack?.id !== requestedTracks[0]!.id || !preview || paused) throw new Error(previewStatus || "No playable audio found");

        } else {
          if (request.mode === "replace") clearPlaybackSkipNotice();
          await nativeCommand({action:"queue",tracks:requestedTracks,mode:request.mode === "replace" ? "replace" : request.mode === "next" ? "next" : "append"});
        }
      } else if (request.action === "pause" || request.action === "resume") {
            const action = request.action === "pause" ? "pause" : "play";
        const atNaturalQueueEnd = (nativePlaybackStatus === "ended" && nativeNaturalQueueEnd);
        if (atNaturalQueueEnd) cancelNaturalQueueRadioContinuation();
        await nativeCommand({action:action === "pause" ? "pause" : "resume"});
        if (action === "pause") {
          if(queuedOwnerForResume())reportQueuedPlaybackPhase("paused");else setAutomationPlaybackOwner(null);
          // Pausing attached media must retain its relay registration. Advancing
          // the backend generation here retires the URL that paused seek/resume
          // still needs. Pending resolution or a stale attachment still cancels.
          const attachedSourceIsCurrent = false;
          nextPreviewRequest();
        }
        else reportQueuedPlaybackPhase("playing");
        paused = action === "pause";
      } else if (request.action === "seek") {
            const seconds = (Number(request.position));
        if (!Number.isFinite(seconds) || seconds < 0 || ((!nativeSnapshot?.canSeek))) throw new Error("This audio cannot seek");
        let mediaSeek = false;
        await nativeCommand({action:"seek",position:seconds});
        position = seconds; if(!mediaSeek)seekSerial++;
      } else if (request.action === "volume") {
        const value = Number(request.volume);
        if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error("Volume must be from 0 to 1");
        volume = value;
        await nativeCommand({action:"volume",volume:value});
      } else if (request.action === "stop") {
        await closePreview(); position = 0;
      } else if (request.action === "shuffle") {
        const enabled = request.mode === "true";
        if (enabled !== shuffleEnabled) { shuffleEnabled = enabled; await nativeCommand({action:"shuffle",enabled}); }
      } else if (request.action === "repeat") {
        if (!["off", "all", "one"].includes(request.mode ?? "")) throw new Error("Invalid repeat mode");
        repeatMode = request.mode as "off" | "all" | "one"; await nativeCommand({action:"repeat",mode:repeatMode});
      } else if (request.action === "openUri") {
        const uri = new URL(request.mode ?? "");
        const library = await call<Track[]>("list_tracks", { view: "library", collectionId: null });
        let track = library.find(item => item.sourceUrl === uri.href || (uri.protocol === "file:" && item.localPath === decodeURIComponent(uri.pathname)));
        if (!track) {
          const youtube = ["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"].includes(uri.hostname);
          const soundcloud = ["soundcloud.com", "www.soundcloud.com"].includes(uri.hostname);
          if (!["http:", "https:"].includes(uri.protocol) || (!youtube && !soundcloud) || uri.username || uri.password) throw new Error("OpenUri supports saved library items and YouTube/SoundCloud audio links");
          // Resolve the explicit page through the linked-source path, as native
          // OpenUri does. Text search can return no hits (or an unrelated hit)
          // for a valid page URL and must not choose the recording for this link.
          track = {
            id: `uri:${crypto.randomUUID()}`, provider: youtube ? "youtube" : "soundcloud",
            providerId: "", sourceUrl: uri.href, title: uri.href, artist: "",
            favorite: false, provenance: { sourceKind: "audioSearch", identity: "explicit source link; recording identity unverified" },
          };
        }
        if (activeAutomationPlayback !== request.requestId) throw new Error("Playback request was cancelled or superseded");
        await startPreview(track,false,false,[track],0,0,request.requestId);
      } else if (request.action === "next" || request.action === "previous") {
        clearPlaybackSkipNotice();

          const direction = request.action === "next" ? 1 : -1;
          await nativeCommand({action:direction > 0 ? "next" : "previous"});

      }
      else throw new Error("Unknown playback action");
    } catch (error) { failure = String(error); }
    const envelope=automationPlaybackSnapshot(true);
    await call<PlaybackStateAck|undefined>("automation_playback_complete", { requestId: request.requestId, playback: envelope.playback, error: failure }).then(ack=>automationPlaybackPublisher.acknowledge(envelope,ack)).catch(() => {});
  }
  async function saveAutomationPermissions() {
    await run(async () => { const result = await call<{permissions: {control: boolean; manage: boolean}}>("set_automation_permissions", { ...automationPermissions }); automationPermissions = result.permissions; });
  }
  function openPlayerPanel(panel: "info" | "queue" | "downloads" | null, track?: Track) {
    if (panel === "info" || panel === "downloads") showFullNowPlaying = false;
    infoTrack = panel === "info" ? track ?? null : null;
    showPlayingInfo = panel === "info"; showQueue = panel === "queue"; showDownloads = panel === "downloads";
  }
  async function refreshTrackLibrarySummaryFor(trackId: string, ownerStillCurrent: () => boolean = () => true) {
    const visibleTrackId = () => showPlayingInfo ? (infoTrack ?? playingTrack)?.id ?? null : null;
    if (!desktop() || visibleTrackId() !== trackId || !ownerStillCurrent()) return;
    const request = ++trackLibrarySummaryRequest;
    trackLibrarySummaryTrackId = trackId;
    trackLibrarySummaryLoading = true;
    try {
      const summary = await call<TrackLibrarySummary>("get_track_library_summary", { trackId });
      if (request === trackLibrarySummaryRequest && visibleTrackId() === trackId && ownerStillCurrent() && summary.trackId === trackId) {
        trackLibrarySummary = summary;
      }
    } catch {
      if (request === trackLibrarySummaryRequest && visibleTrackId() === trackId && ownerStillCurrent()) trackLibrarySummary = null;
    } finally {
      if (request === trackLibrarySummaryRequest) trackLibrarySummaryLoading = false;
    }
  }
  const visibleSummaryTrackId = $derived(showPlayingInfo ? (infoTrack ?? playingTrack)?.id ?? null : null);
  $effect(() => {
    const request = ++trackLibrarySummaryRequest;
    const trackId = visibleSummaryTrackId;
    if (!desktop() || !trackId) {
      trackLibrarySummary = null;
      trackLibrarySummaryTrackId = null;
      trackLibrarySummaryLoading = false;
      return;
    }
    let cancelled = false;
    trackLibrarySummary = null;
    trackLibrarySummaryTrackId = trackId;
    trackLibrarySummaryLoading = true;
    void call<TrackLibrarySummary>("get_track_library_summary", { trackId })
      .then(summary => {
        if (!cancelled && request === trackLibrarySummaryRequest && summary.trackId === trackId) trackLibrarySummary = summary;
      })
      .catch(() => {
        if (!cancelled && request === trackLibrarySummaryRequest) trackLibrarySummary = null;
      })
      .finally(() => {
        if (!cancelled && request === trackLibrarySummaryRequest) trackLibrarySummaryLoading = false;
      });
    return () => { cancelled = true; };
  });
  function keyboard(e: KeyboardEvent) {
    if (desktop() && (!backupHydrationReady || !playbackControlsOwned)) return;
    const target = e.target instanceof Element ? e.target : null;
    if (target?.closest("dialog[open]")) return;
    if (e.key === "Escape" && showFullNowPlaying) { e.preventDefault(); if (showQueue) openPlayerPanel(null); else showFullNowPlaying = false; return; }
    if (showFullNowPlaying && e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { e.preventDefault(); showFullNowPlaying = false; return; }
    const slot=playlistShortcut(e); if(slot>=0){e.preventDefault();void invokePlaylistShortcut(slot).catch(failure=>error=String(failure));return;}
    if (e.key === "Escape" && view === "history" && target?.closest(".history-advanced-filters")) {
      e.preventDefault();
      const details = target.closest(".history-advanced-filters") as HTMLDetailsElement;
      details.open = false;
      details.querySelector("summary")?.focus();
      return;
    }
    if (e.key === "Escape" && deletedCollections) { deletedCollections = null; return; }
    if (e.key === "Escape" && (showDownloads || showQueue || showPlayingInfo)) { openPlayerPanel(null); return; }
    if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { e.preventDefault(); void travelRoute(e.key === "ArrowLeft" ? "back" : "forward"); return; }
    if (downloadTrack || playlistTrack) return;
    if (
      target?.closest(
        "input,textarea,select,button,summary,[contenteditable]",
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
  function dismissHistoryFiltersOutside(event: PointerEvent) {
    const target = event.target instanceof Element ? event.target : null;
    if (view !== "history" || target?.closest(".history-advanced-filters")) return;
    const details = document.querySelector<HTMLDetailsElement>(".history-advanced-filters[open]");
    if (details) details.open = false;
  }
  function activateRenameDialog(node:HTMLDialogElement){
    const close=(event:Event)=>{event.preventDefault();renameTarget=null;};node.addEventListener("cancel",close);
    if(typeof node.showModal==="function")node.showModal();else node.setAttribute("open","");
    return {destroy(){node.removeEventListener("cancel",close);if(node.open&&typeof node.close==="function")node.close();}};
  }
  function activateDownloadDialog(node: HTMLDialogElement, closeChooser = () => {
    downloadRequest++;
    downloadTrack = null;
    downloadPlaylistContext = null;
    downloadCandidates = [];
    downloadCandidate = null;
  }) {
    const restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
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

  async function discoverArtist(track: Track) {
    if (!track.artist.trim()) return;
    const provenance=track.provenance as {catalog?:{provider?:string;providerId?:string;artists?:Array<{provider:string;id:string;name?:string}>;artistCount?:number}} | undefined;
    const catalog = provenance?.catalog;
    const credited=catalog?.artists?.filter(artist=>artist.name?.toLowerCase()===track.artist.trim().toLowerCase());
    if(track.provider !== "spotify" && credited?.length===1 && ["musicbrainz","discogs","monochrome"].includes(credited[0].provider))return discoverArtistRef({provider:credited[0].provider as ArtistRef["provider"],id:credited[0].id});
    const spotifyCredits = catalog?.artists?.filter((artist): artist is SpotifyTrackArtistRef => artist.provider === "spotify" && /^[A-Za-z0-9]{22}$/.test(artist.id) && Boolean(artist.name?.trim())) ?? [];
    const completeSpotifyCreditSet = track.provider === "spotify" && catalog !== undefined && catalog.provider === "spotify" && catalog.providerId === track.providerId && catalog.artistCount === catalog.artists?.length && spotifyCredits.length === catalog.artistCount
      && spotifyCredits.map(artist => artist.name.trim()).join(", ").toLocaleLowerCase() === track.artist.trim().toLocaleLowerCase();
    const uniqueSpotifyCredits = completeSpotifyCreditSet ? [...new Map(spotifyCredits.map(artist => [artist.id, artist])).values()] : [];
    if (uniqueSpotifyCredits.length === 1) { await openSpotifyTrackArtist(uniqueSpotifyCredits[0]!); return; }
    if (uniqueSpotifyCredits.length > 1) { spotifyTrackArtistChoices = uniqueSpotifyCredits; return; }
    if (track.provider === "spotify") { await browseArtistProvider(track.artist, "spotify"); return; }
    rememberRoute();
    spotifyArtistId=""; spotifyAlbumId="";
    spotifyArtistSearchFallback=null;
    artistSection = "overview"; artistScroll = 0;
    activeArtistTrack = track;
    activeArtistName = track.artist;
    activeArtistRef = null;
    await navigate("artist", false);
  }
  async function browseArtistProvider(name: string, target: "spotify" | "musicbrainz") {
    const artistName = name.trim();
    if (!artistName) return;
    const source = availableSources.find(item => item.id === target);
    if (availableSources.length && (!source || source.enabled === false)) {
      error = source?.detail || `${target === "spotify" ? "Spotify" : "MusicBrainz"} artist search is unavailable. Check the connection in Settings.`;
      return;
    }
    rememberRoute();
    query = artistName;
    searchRelease = null;
    automationSearch = null;
    catalogSearchSource = target;
    catalogSearchKind = "artists";
    catalogSearchRequest++;
    await navigate("search", false);
  }
  async function openSpotifyTrackArtist(artist: SpotifyTrackArtistRef) {
    if (!/^[A-Za-z0-9]{22}$/.test(artist.id)) return;
    spotifyTrackArtistChoices = [];
    rememberRoute();
    spotifyNavigationSnapshotKey = null;
    spotifyArtistId = artist.id; spotifyAlbumId = "";
    spotifyArtistSearchFallback = { id: artist.id, title: artist.name, url: `https://open.spotify.com/artist/${artist.id}`, artworkUrl: null };
    activeArtistName = artist.name; activeArtistRef = null; activeArtistTrack = null; artistSection = "overview"; artistScroll = 0;
    await navigate("artist", false);
  }
  async function discoverArtistRef(artist: ArtistRef, initialSection: "overview" | "releases" | "tracks" = "overview") {
    rememberRoute();
    spotifyArtistId=""; spotifyAlbumId="";
    spotifyArtistSearchFallback=null;
    activeArtistTrack = null;
    activeArtistName = "";
    artistSection = initialSection; artistScroll = 0;
    activeArtistRef = artist;
    await navigate("artist", false);
  }
  async function prepareReleaseLoudnessAnalysis(tracks: Track[], gapSeconds: number, localOnly = false): Promise<{ jobId: string; playbackTracks: Track[] }> {
    if (gapSeconds !== 0) throw new Error("Album analysis currently matches gapless queue playback only.");
    let prepared: Track[];
    if (localOnly) {
      const groups = await call<{ trackId: string; files: LocalAsset[] }[]>("list_track_files_for_recordings", { recordingIds: tracks.map(track => track.id) });
      prepared = tracks.map(track => {
        const files = groups.find(group => group.trackId === track.id)?.files ?? [];
        const exact = track.assetId ? files.find(file => file.id === track.assetId) : null;
        const asset = exact ?? (files.length === 1 ? files[0] : null);
        if (!asset || !asset.path) {
          throw new Error(files.length > 1
            ? `Choose one registered local file for “${track.title}” before analyzing this album.`
            : `“${track.title}” no longer has a registered local audio file.`);
        }
        return { ...track, assetId: asset.id, localPath: asset.path, localAvailability: "available" as const };
      });
    } else {
      prepared = await matchArtistPlaybackSources(tracks);
    }
    if (prepared.length !== tracks.length || prepared.some(track => !track.localPath)) {
      throw new Error("Album loudness analysis requires a registered local audio file for every release track.");
    }
    const paths = prepared.map(track => track.localPath!);
    const assets = await call<LocalAsset[]>("list_assets_for_paths", { paths });
    const assetByPath = new Map(assets.map(asset => [asset.path, asset]));
    const playbackTracks = prepared.map(track => {
      const asset = assetByPath.get(track.localPath!);
      if (!asset || (track.assetId && track.assetId !== asset.id)) {
        throw new Error("A release track no longer matches its registered local audio file.");
      }
      return { ...track, assetId: asset.id, localAvailability: "available" as const };
    });
    const assetIds = playbackTracks.map(track => track.assetId!);
    if (new Set(assetIds).size !== assetIds.length) {
      throw new Error("This release repeats a local audio asset, so it cannot be measured as a single ordered programme.");
    }
    const jobId = await call<string>("start_loudness_analysis", {
      assetIds,
      interTrackGapsSeconds: Array(Math.max(0, assetIds.length - 1)).fill(gapSeconds),
    });
    return { jobId, playbackTracks };
  }

  function localAlbumOrder(tracks: Track[]): { tracks: Track[]; canReorder: boolean } {
    const position = (track: Track): { disc: number; track: number } | null => {
      const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
      const raw = provenance.position ?? provenance.trackNumber ?? provenance.track_number;
      let trackNumber = Number.isFinite(track.trackNumber) && Number(track.trackNumber) > 0 ? Number(track.trackNumber) : null;
      let discNumber = Number.isFinite(track.discNumber) && Number(track.discNumber) > 0 ? Number(track.discNumber) : 1;
      if (trackNumber == null && typeof raw === "number" && Number.isFinite(raw) && raw > 0) trackNumber = raw;
      if (trackNumber == null && typeof raw === "string") {
        const match = raw.trim().match(/^(?:(\d+)[.-])?(\d+)$/);
        if (match) { if (match[1]) discNumber = Number(match[1]); trackNumber = Number(match[2]); }
      }
      return trackNumber == null ? null : { disc: discNumber, track: trackNumber };
    };
    const positions = tracks.map(position);
    const complete = positions.every((value): value is { disc: number; track: number } => value !== null);
    const unique = complete && new Set(positions.map(value => `${value.disc}:${value.track}`)).size === tracks.length;
    if (!complete || !unique) return { tracks: [...tracks], canReorder: true };
    return {
      tracks: tracks.map((track, index) => ({ track, index, position: positions[index]! }))
        .sort((left, right) => left.position.disc - right.position.disc || left.position.track - right.position.track || left.index - right.index)
        .map(row => row.track),
      canReorder: false,
    };
  }

  async function openSelectedLocalAlbumLoudness(): Promise<void> {
    localAlbumLoudnessFocusReturn = document.querySelector<HTMLElement>(".header-library-tools summary");
    closeLibraryActionsPopover();
    const selectedTracks = currentLoudnessSelection();
    if (selectedTracks.length < 2 || selectedTracks.length > 200) {
      setNotice("Select 2–200 downloaded tracks from one album to analyze a local album programme.");
      return;
    }
    if (new Set(selectedTracks.map(track => track.id)).size !== selectedTracks.length) {
      setNotice("A local album programme cannot repeat the same selected recording.");
      return;
    }
    const album = selectedTracks[0]?.album?.trim() ?? "";
    const albumArtist = selectedTracks[0]?.albumArtist?.trim() ?? "";
    const same = (value: string | null | undefined, expected: string) => Boolean(value?.trim()) && value!.trim().toLocaleLowerCase() === expected.toLocaleLowerCase();
    if (!album || !albumArtist || selectedTracks.some(track => !same(track.album, album) || !same(track.albumArtist, albumArtist))) {
      setNotice("Select tracks with one matching album and album artist to analyze a local album programme.");
      return;
    }
    try {
      const groups = await call<{ trackId: string; files: LocalAsset[] }[]>("list_track_files_for_recordings", { recordingIds: selectedTracks.map(track => track.id) });
      const localTracks = selectedTracks.map(track => {
        const files = groups.find(group => group.trackId === track.id)?.files ?? [];
        const exact = track.assetId ? files.find(file => file.id === track.assetId) : null;
        const asset = exact ?? (files.length === 1 ? files[0] : null);
        if (!asset || !asset.path) throw new Error(files.length > 1
          ? `Choose one registered local file for “${track.title}” before analyzing this album.`
          : `“${track.title}” is not a downloaded local track.`);
        return { ...track, assetId: asset.id, localPath: asset.path, localAvailability: "available" as const };
      });
      if (new Set(localTracks.map(track => track.assetId)).size !== localTracks.length) {
        throw new Error("Each selected album track must use a unique registered local audio asset.");
      }
      const order = localAlbumOrder(localTracks);
      localAlbumLoudnessReview = { album, albumArtist, ...order };
    } catch (failure) {
      setNotice(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function playReleaseAlbum(tracks: Track[], resource: LoudnessResourceIdentity | null) {
    if (!tracks.length) return;
    await playArtistTrack(tracks[0]!, tracks, 0, resource);
  }

  async function playArtistTrack(track: Track, displayed: Track[] = [track], index = 0, albumResource: LoudnessResourceIdentity | null = null) {
    const request = ++artistPlaybackMatchGeneration;
    const intentSequence = playbackLoadSequence;
    const rows = displayed ?? [track];
    const rowIndex = Number.isInteger(index) && index >= 0 && index < rows.length ? index : Math.max(0, rows.indexOf(track));
    let playableTrack = rows[rowIndex] ?? track;
    if (!(playableTrack.localAvailability === "available" && playableTrack.localPath)) {
      try { playableTrack = (await matchArtistPlaybackSources([playableTrack]))[0] ?? playableTrack; }
      catch (failure) { if (request === artistPlaybackMatchGeneration && intentSequence === playbackLoadSequence) error = String(failure); playableTrack = rows[rowIndex] ?? track; }
    }
    if (request !== artistPlaybackMatchGeneration || intentSequence !== playbackLoadSequence) return;
    const playableRows = playableTrack === rows[rowIndex] ? rows : rows.map((item, itemIndex) => itemIndex === rowIndex ? playableTrack : item);
    selected = playableTrack;
     await startPreview(playableTrack,false,false,playableRows,rowIndex,0,null,false,undefined,albumResource); return;

  }
  function queueArtistTracks(items: Track[], mode: "append" | "next", albumResource: LoudnessResourceIdentity | null = null) {
    if (!items.length) return;

     void nativeCommand({action:"queue",tracks:items,mode,loudnessAlbumResource:albumResource}); return;

  }
  function sortUpcoming(field:TrackSortField,direction:SortDirection) {

    void nativeCommand({action:"sort_upcoming",field,direction});
  }
  async function moveQueueItem(from:number,to:number,expectedQueueRevision?:number) {  try{return await nativeBridge.command({action:"reorder",from,to,...(expectedQueueRevision===undefined?{}:{expectedQueueRevision})});}catch(failure){error=String(failure);throw failure;} }
  async function removeQueueItems(indices:number[]) {

    const unique = [...new Set(indices)].filter(index => Number.isInteger(index) && index >= 0 && index < queueTracks.length && index !== queueIndex).sort((a,b)=>b-a);
    if (!unique.length) return;
    await nativeCommand({action:"remove_many",indices:unique,expectedQueueRevision:nativeSnapshot?.queueRevision});
  }
  function removeQueueItem(index:number) { void nativeCommand({action:"remove",index}); }
  function toggleShuffle() { shuffleEnabled=!shuffleEnabled;void nativeCommand({action:"shuffle",enabled:shuffleEnabled}); }
  function cycleRepeat() { repeatMode=repeatMode==="off"?"all":repeatMode==="all"?"one":"off";void nativeCommand({action:"repeat",mode:repeatMode}); }
  function playQueueIndex(index:number) {
    clearPlaybackSkipNotice();
    const occurrence = nativeQueueSelectIdentity(index, queueTracks);
    if (!occurrence) { error = "The playback queue identity is unavailable. Refresh the queue and try again."; return; }
    void nativeCommand({action:"select",index,...occurrence});
  }
  function closePlaylistPicker() {
    playlistAlbumName = null;
    playlistPickerGeneration++; playlistTrack = null; playlistTracks = []; pendingCreatedPlaylist = null; shortcutSlot = null;
  }
  async function addArtistTrack(track: Track, closePlayerPanel = true) {
    playlistAlbumName = null;
    if (closePlayerPanel) openPlayerPanel(null);
    const generation = ++playlistPickerGeneration;
    pendingCreatedPlaylist = null;
    playlistDestinationQuery = "";
    playlistTrack = track;
    playlistTracks = [track];
    if (!collections.length) {
      const loaded = await call<Collection[]>("list_collections");
      if (generation !== playlistPickerGeneration) return;
      collections = loaded;
    }
    if (!playlistDestinationId && collections.length) playlistDestinationId = collections.some(item => item.id === collectionId) ? collectionId : collections[0].id;
  }
  async function addTracksToPlaylist(items: Track[]) {
    playlistAlbumName = null;
    if (!items.length) return;
    openPlayerPanel(null);
    playlistPickerGeneration += 1;
    pendingCreatedPlaylist = null;
    const generation = playlistPickerGeneration;
    playlistDestinationQuery = "";
    playlistTrack = items[0]!;
    playlistTracks = [...items];
    if (!collections.length) {
      const loaded = await call<Collection[]>("list_collections");
      if (generation !== playlistPickerGeneration) return;
      collections = loaded;
    }
    if (!playlistDestinationId && collections.length) playlistDestinationId = collections.some(item => item.id === collectionId) ? collectionId : collections[0]!.id;
  }
  async function addAlbumToPlaylist(items: Track[], title: string) {
    if (!items.length) return;
    // Open a reviewable picker; no tracks or playlist are written until its action is chosen.
    const opening = addTracksToPlaylist(items);
    const generation = playlistPickerGeneration;
    playlistAlbumName = title.trim() || items[0].album?.trim() || "New playlist";
    newCollection = playlistAlbumName;
    playlistDestinationId = "";
    await opening;
    if (generation !== playlistPickerGeneration) return;
    playlistDestinationId = "";
    await tick();
    document.querySelector<HTMLInputElement>('[aria-label="New playlist for album"]')?.focus();
  }
  async function addRemoteSearchPlaylist(row: { sourceId: string; kind: string; url?: string | null }, options: { signal: AbortSignal; onProgress: (message: string) => void }) {
    const provider = row.sourceId === "spotify" || row.sourceId === "youtube" ? row.sourceId : null;
    if (!provider || row.kind !== "playlist" || !row.url) throw new Error("This result is not a supported streaming playlist.");
    type ImportJob = { jobId?: string; state?: string; completed?: number; total?: number; results?: { complete?: boolean }[]; error?: string | null; errors?: { message?: string }[] };
    if (options.signal.aborted) return { cancelled: true };
    let job = await call<ImportJob>("import_remote_playlists", { provider, playlistIds: [row.url] });
    let cancelSent = false;
    const terminal = (state?: string) => ["completed", "partial", "cancelled", "failed"].includes((state ?? "").toLowerCase());
    while (job.jobId && !terminal(job.state)) {
      if (options.signal.aborted && !cancelSent) { await call("cancel_remote_playlist_import", { jobId: job.jobId }); cancelSent = true; }
      options.onProgress(`Adding playlist · ${Math.min(job.completed ?? 0, job.total ?? 1)} of ${job.total ?? 1}`);
      await new Promise<void>(resolve => setTimeout(resolve, 800));
      job = await call<ImportJob>("get_remote_playlist_import_job", { jobId: job.jobId });
    }
    if (job.state === "failed") throw new Error(job.error || job.errors?.[0]?.message || "Import failed.");
    const partial = job.state === "partial" || Boolean(job.errors?.length) || Boolean(job.results?.some(result => result.complete === false));
    if (partial && !job.results?.length && job.state !== "cancelled") throw new Error(job.error || job.errors?.[0]?.message || "No playlist was added. The provider returned an incomplete result.");
    await remotePlaylistsImported();
    if (job.state === "cancelled") return { cancelled: true };
    return { partial };
  }
  async function addArtistTrackToPlaylist() {
    const track = playlistTrack;
    if (!track || playlistWriteBusy || !playlistDestinations.some(destination => destination.id === playlistDestinationId)) return;
    const items = playlistTracks.length ? playlistTracks : [track];
    const destinationId = playlistDestinationId, generation = playlistPickerGeneration;
    const legacyUniqueCollection = destinationId === "favorites" || destinationId === "history" || destinationId === DOWNLOADS_PLAYLIST_ID || destinationId === playlistPreferences.favoritesPlaylistId;
    playlistWriteBusy = true;
    let saved = false;
    try { const result = await run(async () => {
      await call(legacyUniqueCollection ? "add_to_collection" : "append_playlist_entries", { collectionId: destinationId, tracks: items });
      collections = await call<Collection[]>("list_collections");
      if(generation === playlistPickerGeneration && shortcutSlot!==null){playlistPreferences=await call<PlaylistPreferences>("set_playlist_shortcut",{slot:shortcutSlot,collectionId:destinationId});shortcutSlot=null;}
      await refreshFavorites();
      setNotice(legacyUniqueCollection ? "Added to playlist. This collection keeps one copy of each recording." : `${items.length === 1 ? "Track" : `${items.length} tracks`} added to playlist.`);
      if (generation === playlistPickerGeneration) pendingCreatedPlaylist = null;
      return true;
    }); saved = result === true; }
    finally { playlistWriteBusy = false; }
    if (saved && generation === playlistPickerGeneration) { playlistTrack = null; playlistTracks = []; }
  }
  async function createPlaylistForArtistTrack() {
    const track = playlistTrack;
    if (!track || playlistWriteBusy || !newCollection.trim()) return;
    const items = playlistTracks.length ? playlistTracks : [track];
    const generation = playlistPickerGeneration;
    playlistWriteBusy = true;
    let saved = false;
    try { const result = await run(async () => {
      const retrying = pendingCreatedPlaylist?.generation === generation && pendingCreatedPlaylist.collection.name === newCollection.trim() ? pendingCreatedPlaylist.collection : null;
      const created = retrying ?? await call<Collection>("create_collection", { name: newCollection.trim() });
      pendingCreatedPlaylist = { collection: created, generation };
      if (!retrying) collections = await call<Collection[]>("list_collections");
      await call("append_playlist_entries", { collectionId: created.id, tracks: items });
      collections = await call<Collection[]>("list_collections");
      if (generation === playlistPickerGeneration) { playlistDestinationId = created.id; newCollection = ""; }
      if(generation === playlistPickerGeneration && shortcutSlot!==null){playlistPreferences=await call<PlaylistPreferences>("set_playlist_shortcut",{slot:shortcutSlot,collectionId:created.id});shortcutSlot=null;}
      await refreshFavorites();
      setNotice(`Created ${created.name} and added ${items.length === 1 ? track.title : `${items.length} tracks`}.`);
      if (generation === playlistPickerGeneration) pendingCreatedPlaylist = null;
      return true;
    }); saved = result === true; }
    finally { playlistWriteBusy = false; }
    if (saved && generation === playlistPickerGeneration) { playlistTrack = null; playlistTracks = []; }
  }
  async function searchArtists(query: string, page: number) {
    return call<ArtistSearchPage>("search_artists", { query, page, pageSize: 20 });
  }
  async function getArtistProfile(artist: ArtistRef) {
    const profile = await call<ArtistProfile>("get_artist_profile", { artist });
    const names = [profile.artist.name, ...profile.aliases].filter(Boolean);
    return { ...profile, topTracks: await matchArtistPlaybackSources(profile.topTracks, names) };
  }
  async function loadArtistProfileArtwork(artist: ArtistRef) {
    return call<{imageUrl?:string|null}>("get_artist_artwork", { artist });
  }
  async function getArtistReleases(artist: ArtistRef, cursor?: string | null) {
    return call<ArtistReleasePage>("get_artist_releases", { artist, cursor });
  }
  async function getReleaseTracks(release: ArtistReleaseRef, artist: ArtistRef) {
    const tracks = await call<Track[]>("get_release_tracks", { release, artist });
    return matchArtistPlaybackSources(tracks);
  }
  async function getArtistTracks(artist: ArtistRef, cursor?: string | null) {
    const page = await call<ArtistTrackPage>("get_artist_tracks", { artist, cursor });
    return { ...page, items: await matchArtistPlaybackSources(page.items) };
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
    invalidateLegacyArtistAssets();
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
    setNotice(assetIdForTrack(track) || track.localAvailability === "available"
      ? "Metadata saved; local tags are updating."
      : "Accepted metadata fields were saved.");
  }
  function sameTrack(a: Track, b: Track): boolean {
    return a.id === b.id || (a.provider === b.provider && a.providerId === b.providerId) || (Boolean(a.sourceUrl && b.sourceUrl) && a.sourceUrl === b.sourceUrl);
  }
  function assetIdForTrack(track: Track): string | undefined {
    const explicit = (track as Track & { assetId?: string | null }).assetId;
    return explicit ?? trackAsset(track)?.id;
  }

  function analysisAnnotation(track: Track, field: "bpm" | "key"): string {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    if ((provenance.manualFields as Record<string,unknown> | undefined)?.[field] === true) return "";
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
    return `Estimated from audio; not verified catalog metadata${record.method ? ` · ${record.method}` : ""}${record.confidence != null ? ` · ${field === "bpm" ? "rhythmic correlation" : "excerpt agreement"} ${record.confidence}` : ""}${field === "bpm" && typeof record.relatedTempoAlternativeBpm === "number" ? ` · alternative tempo ${record.relatedTempoAlternativeBpm} BPM` : ""}${record.uncertainty ? ` · ${record.uncertainty}` : ""}.`;
  }
  function analysisFieldIsManual(track: Track | undefined, field: "bpm" | "key"): boolean {
    const provenance = track?.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    return (provenance.manualFields as Record<string, unknown> | undefined)?.[field] === true;
  }
  function analysisFieldExists(track: Track | undefined, field: "bpm" | "key"): boolean {
    return field === "bpm" ? track?.bpm != null : track?.key != null;
  }
  function closeLibraryActionsPopover(): void {
    const details = document.querySelector<HTMLDetailsElement>(".header-library-tools[open]");
    if (!details) return;
    // The menu action becomes hidden when details closes. Give its visible
    // opener focus before the dialog captures the element to return to.
    details.querySelector<HTMLElement>(":scope > summary")?.focus({ preventScroll: true });
    details.removeAttribute("open");
  }
  async function analyzeLibraryAssets() {
    if (selectedAnalysisBusy) return;
    const selectedIds = checked.length ? [...new Set(checked)] : selected ? [selected.id] : playingTrack ? [playingTrack.id] : [];
    if (!selectedIds.length) { setNotice("Select one or more local tracks to analyze."); return; }
    if (selectedIds.length > 200) { setNotice("Analyze at most 200 selected tracks at a time."); return; }
    closeLibraryActionsPopover();
    const candidateTracks = [...new Map<string, Track>([...tracks, ...sorted, ...queueTracks, selected, playingTrack].filter((track): track is Track => Boolean(track)).map(track => [track.id, track] as const)).values()];
    const selectedTracks = selectedIds.map(id => candidateTracks.find(track => track.id === id) ?? scopedAssetsById.get(id)?.track).filter((track): track is Track => Boolean(track));
    const existingAssets = new Map<string, LocalAsset>();
    for (const track of selectedTracks) {
      const asset = trackAsset(track) ?? filesForTrack(track)[0];
      if (asset) existingAssets.set(track.id, asset);
    }
    const missingTracks = selectedTracks.filter(track => !existingAssets.has(track.id));
    if (missingTracks.length) {
      let groups: { trackId: string; files: LocalAsset[] }[];
      try { groups = await call<{ trackId: string; files: LocalAsset[] }[]>("list_track_files_for_recordings", { recordingIds: missingTracks.map(track => track.id) }); }
      catch (failure) { error = String(failure); return; }
      for (const group of groups) {
        const track = missingTracks.find(item => item.id === group.trackId);
        const asset = track?.assetId ? group.files.find(file => file.id === track.assetId) : null;
        if (track && (asset || group.files[0])) existingAssets.set(track.id, asset ?? group.files[0]!);
      }
    }
    const selectedAssets = [...new Map<string, LocalAsset>([...existingAssets.values()].map(asset => [asset.id, asset] as const)).values()];
    if (!selectedAssets.length) { setNotice("No selected tracks have a registered local audio file. Download a track or select a downloaded track to analyze it."); return; }
    if (selectedAssets.length > 200) { setNotice("Select at most 200 downloaded tracks at a time."); return; }
    const tracksByAsset = Object.fromEntries(selectedAssets.map(asset => [asset.id, asset.track]));
    selectedAnalysisTracks = tracksByAsset;
    selectedAnalysisApplied = [];
    selectedAnalysisJob = null;
    selectedAnalysisKind = "asset";
    selectedAnalysisOpen = true;
    selectedAnalysisBusy = true;
    analysisBusy = true;
    const request = ++selectedAnalysisRequest;
    try {
      const started = await call<SelectedAnalysisJob>("start_selected_asset_analysis", {
        assetIds: selectedAssets.map(asset => asset.id), fields: ["bpm", "key"],
      });
      selectedAnalysisJob = started;
      while (request === selectedAnalysisRequest) {
        const current = await call<SelectedAnalysisJob>("get_selected_asset_analysis", { jobId: started.jobId });
        if (request !== selectedAnalysisRequest) return;
        selectedAnalysisJob = current;
        if (["completed", "cancelled", "failed"].includes(current.state)) break;
        await new Promise(resolve => window.setTimeout(resolve, 650));
      }
      if (request === selectedAnalysisRequest && selectedAnalysisJob?.state === "completed") {
        const count = selectedAnalysisJob.results.filter(row => row.analysis).length;
        setNotice(`Analysis ready to review for ${count} of ${selectedAnalysisJob.total} selected track${selectedAnalysisJob.total === 1 ? "" : "s"}.`);
      }
    } catch (failure) { error = String(failure); }
    finally { if (request === selectedAnalysisRequest) { selectedAnalysisBusy = false; analysisBusy = false; } }
  }
  function isLoudnessPlaybackActive(): boolean {
    return Boolean((playingTrack || playingRadioStation || currentRadioSong) && !paused);
  }
  function currentLoudnessSelection(): Track[] {
    const occurrenceTracks = view === "collections" ? selectedLocalOccurrenceTracks : [];
    if (occurrenceTracks.length) return occurrenceTracks;
    const ids = checked.length ? [...new Set(checked)] : selected ? [selected.id] : playingTrack ? [playingTrack.id] : [];
    const candidates = [...new Map<string, Track>([...tracks, ...sorted, ...queueTracks, selected, playingTrack]
      .filter((track): track is Track => Boolean(track)).map(track => [track.id, track] as const)).values()];
    return ids.map(id => candidates.find(track => track.id === id) ?? scopedAssetsById.get(id)?.track).filter((track): track is Track => Boolean(track));
  }
  async function resolveLoudnessAssets(inputTracks: Track[]): Promise<LocalAsset[]> {
    const byTrack = new Map<string, LocalAsset>();
    for (const track of inputTracks) {
      const asset = trackAsset(track) ?? filesForTrack(track)[0];
      if (asset) byTrack.set(track.id, asset);
    }
    const missing = inputTracks.filter(track => !byTrack.has(track.id));
    if (missing.length) {
      const groups = await call<{ trackId: string; files: LocalAsset[] }[]>("list_track_files_for_recordings", { recordingIds: missing.map(track => track.id) });
      for (const group of groups) {
        const track = missing.find(item => item.id === group.trackId);
        const preferred = track?.assetId ? group.files.find(file => file.id === track.assetId) : null;
        if (track && (preferred || group.files[0])) byTrack.set(track.id, preferred ?? group.files[0]!);
      }
    }
    return [...new Map([...byTrack.values()].map(asset => [asset.id, asset] as const)).values()];
  }
  async function analyzeLoudnessTracks(inputTracks: Track[]) {
    if (loudnessAnalysisBusy) return;
    const uniqueTracks = [...new Map(inputTracks.map(track => [track.id, track] as const)).values()];
    if (!uniqueTracks.length) { setNotice("Select a downloaded track to analyze its loudness."); return; }
    let assets: LocalAsset[];
    try { assets = await resolveLoudnessAssets(uniqueTracks); }
    catch (failure) { error = String(failure); return; }
    if (!assets.length) { setNotice("No selected track has a registered local audio file. Loudness analysis does not change audio files or metadata."); return; }
    if (assets.length > 50) { setNotice("Analyze at most 50 selected tracks at a time."); return; }
    const assetIds = [...new Set(assets.map(asset => asset.id))];
    if (loudnessSettings.analyzeWhenIdle && isLoudnessPlaybackActive()) {
      loudnessRetryRequired = false;
      pendingLoudnessAssetIds = [...new Set([...pendingLoudnessAssetIds, ...assetIds])];
      loudnessAnalysisMessage = "Waiting for playback to pause. Selected files are untouched until analysis runs.";
      setNotice(loudnessAnalysisMessage);
      return;
    }
    loudnessRetryRequired = false;
    pendingLoudnessAssetIds = [];
    await launchLoudnessAnalysis(assetIds, loudnessSettings.analyzeWhenIdle);
  }
  async function launchLoudnessAnalysis(assetIds: string[], queued: boolean) {
    if (!assetIds.length || loudnessAnalysisBusy) return;
    const request = ++activeLoudnessJobRequest;
    activeLoudnessAssetIds = [...new Set(assetIds)];
    activeLoudnessJobIsQueued = queued;
    activeLoudnessJobId = "";
    loudnessAnalysisMessage = queued ? "Selected analysis will pause if playback resumes." : "";
    loudnessAnalysisBusy = true;
    try {
      const jobId = await call<string>("start_loudness_analysis", { assetIds: activeLoudnessAssetIds });
      if (request !== activeLoudnessJobRequest) { await call("cancel_loudness_analysis", { jobId }).catch(() => {}); return; }
      activeLoudnessJobId = jobId;
      const initial = await call<LoudnessAnalysisJobStatus>("get_loudness_analysis_status", { jobId });
      loudnessAnalysisJob = initial;
      if (queued && isLoudnessPlaybackActive()) {
        await pauseQueuedLoudnessAnalysis();
        return;
      }
      while (request === activeLoudnessJobRequest) {
        const current = await call<LoudnessAnalysisJobStatus>("get_loudness_analysis_status", { jobId });
        if (request !== activeLoudnessJobRequest) return;
        loudnessAnalysisJob = current;
        if (["completed", "cancelled", "failed"].includes(current.status)) {
          if (current.status === "completed") {
            pendingLoudnessAssetIds = [];
            loudnessAnalysisMessage = "Selected loudness measurements are ready. Audio files and recording metadata were not changed.";
          } else if (current.status === "failed") {
            pendingLoudnessAssetIds = activeLoudnessJobIsQueued ? activeLoudnessAssetIds : [];
            loudnessRetryRequired = activeLoudnessJobIsQueued;
            loudnessAnalysisMessage = current.error ?? "Loudness analysis failed.";
          }
          break;
        }
        await new Promise(resolve => window.setTimeout(resolve, 650));
      }
    } catch (failure) {
      if (request === activeLoudnessJobRequest) {
        pendingLoudnessAssetIds = queued ? activeLoudnessAssetIds : [];
        loudnessRetryRequired = queued;
        loudnessAnalysisMessage = String(failure);
        error = String(failure);
      }
    } finally {
      if (request === activeLoudnessJobRequest) {
        loudnessAnalysisBusy = false;
        activeLoudnessJobId = "";
        activeLoudnessAssetIds = [];
        activeLoudnessJobIsQueued = false;
      }
    }
  }
  async function pauseQueuedLoudnessAnalysis() {
    if (!activeLoudnessJobIsQueued || !activeLoudnessJobId) return;
    const ids = activeLoudnessAssetIds;
    const jobId = activeLoudnessJobId;
    const request = activeLoudnessJobRequest;
    try {
      await call("cancel_loudness_analysis", { jobId });
    } catch (failure) {
      if (request !== activeLoudnessJobRequest || activeLoudnessJobId !== jobId) return;
      const detail = failure instanceof Error ? failure.message : String(failure);
      loudnessAnalysisMessage = `Playback resumed, but analysis could not be canceled and is still running. Retry cancel or wait for it to finish. ${detail}`;
      return;
    }
    if (request !== activeLoudnessJobRequest || activeLoudnessJobId !== jobId) return;
    activeLoudnessJobRequest++;
    pendingLoudnessAssetIds = [...new Set([...ids, ...pendingLoudnessAssetIds])];
    loudnessRetryRequired = false;
    loudnessAnalysisJob = loudnessAnalysisJob ? { ...loudnessAnalysisJob, status: "cancelled" } : null;
    loudnessAnalysisMessage = "Playback resumed. Analysis was canceled and the selected tracks are queued to retry when playback is idle.";
    loudnessAnalysisBusy = false;
    activeLoudnessJobId = "";
    activeLoudnessAssetIds = [];
    activeLoudnessJobIsQueued = false;
  }
  async function cancelLoudnessAnalysis() {
    const jobId = activeLoudnessJobId;
    const request = activeLoudnessJobRequest;
    if (jobId) {
      try {
        await call("cancel_loudness_analysis", { jobId });
      } catch (failure) {
        if (request !== activeLoudnessJobRequest || activeLoudnessJobId !== jobId) return;
        const detail = failure instanceof Error ? failure.message : String(failure);
        loudnessAnalysisMessage = `Loudness analysis is still running because cancellation failed. Retry cancel or wait for it to finish. ${detail}`;
        return;
      }
      if (request !== activeLoudnessJobRequest || activeLoudnessJobId !== jobId) return;
    } else if (request !== activeLoudnessJobRequest) {
      return;
    }
    activeLoudnessJobRequest++;
    pendingLoudnessAssetIds = [];
    loudnessRetryRequired = false;
    loudnessAnalysisBusy = false;
    activeLoudnessJobId = "";
    activeLoudnessAssetIds = [];
    activeLoudnessJobIsQueued = false;
    loudnessAnalysisJob = jobId ? { jobId, status: "cancelled", total: loudnessAnalysisJob?.total ?? 0, completed: loudnessAnalysisJob?.completed ?? 0, error: null } : null;
    loudnessAnalysisMessage = "Selected loudness analysis canceled.";
  }
  function retryQueuedLoudnessAnalysis() {
    if (!pendingLoudnessAssetIds.length) return;
    if (isLoudnessPlaybackActive()) { loudnessAnalysisMessage = "Pause playback before retrying selected analysis."; return; }
    loudnessRetryRequired = false;
    void launchLoudnessAnalysis(pendingLoudnessAssetIds, loudnessSettings.analyzeWhenIdle);
  }
  let previousLoudnessPlaybackActive = false;
  $effect(() => {
    const active = isLoudnessPlaybackActive();
    if (active === previousLoudnessPlaybackActive) return;
    previousLoudnessPlaybackActive = active;
    if (active) void pauseQueuedLoudnessAnalysis();
  });
  $effect(() => {
    const pending = pendingLoudnessAssetIds;
    const enabled = loudnessSettings.analyzeWhenIdle;
    const idle = !isLoudnessPlaybackActive();
    if (!pending.length || !enabled || !idle || loudnessAnalysisBusy || loudnessLaunchScheduled || loudnessRetryRequired) return;
    loudnessLaunchScheduled = true;
    queueMicrotask(() => {
      loudnessLaunchScheduled = false;
      if (pendingLoudnessAssetIds.length && loudnessSettings.analyzeWhenIdle && !isLoudnessPlaybackActive() && !loudnessAnalysisBusy) {
        void launchLoudnessAnalysis(pendingLoudnessAssetIds, true);
      }
    });
  });
  async function analyzeSelectedStreamTracks() {
    if (selectedAnalysisBusy) return;
    const occurrenceTracks = view === "collections" ? selectedLocalOccurrenceTracks : [];
    const ids = occurrenceTracks.length
      ? [...new Set(occurrenceTracks.map(track => track.id))]
      : checked.length ? [...new Set(checked)] : selected ? [selected.id] : playingTrack ? [playingTrack.id] : [];
    if (!ids.length) { setNotice("Select one or more saved recordings to analyze their verified streaming sources."); return; }
    if (ids.length > 25) { setNotice("Analyze at most 25 streaming sources at a time."); return; }
    closeLibraryActionsPopover();
    const available = [...new Map<string, Track>([...tracks, ...sorted, ...queueTracks, selected, playingTrack]
      .filter((track): track is Track => Boolean(track)).map(track => [track.id, track] as const)).values()];
    const tracksById = Object.fromEntries(ids.flatMap(id => {
      const track = available.find(item => item.id === id);
      return track ? [[id, track] as const] : [];
    }));
    selectedAnalysisTracks = tracksById;
    selectedAnalysisApplied = [];
    selectedAnalysisJob = null;
    selectedAnalysisKind = "stream";
    selectedAnalysisOpen = true;
    selectedAnalysisBusy = true;
    analysisBusy = true;
    const request = ++selectedAnalysisRequest;
    const normalize = (job: {
      jobId: string; state: string; total: number; completed: number;
      results: { trackId: string; recordingId: string; status: string; provider?: string | null; sourceId?: string | null; analysis: WholeTrackAnalysisDto | null; error?: string | null }[];
    }): SelectedAnalysisJob => ({
      jobId: job.jobId, state: job.state, total: job.total, completed: job.completed,
      results: job.results.map(row => ({
        assetId: row.trackId, recordingId: row.recordingId, status: row.status,
        analysis: row.analysis, error: row.error, kind: "stream",
        sourceProvider: row.provider, sourceId: row.sourceId,
      })),
    });
    try {
      const started = normalize(await call<SelectedStreamJobDto>("start_selected_stream_analysis", {
        trackIds: ids, fields: ["bpm", "key"],
      }));
      selectedAnalysisJob = started;
      while (request === selectedAnalysisRequest) {
        const current = normalize(await call<SelectedStreamJobDto>("get_selected_stream_analysis", { jobId: started.jobId }));
        if (request !== selectedAnalysisRequest) return;
        selectedAnalysisJob = current;
        if (["completed", "cancelled", "failed"].includes(current.state)) break;
        await new Promise(resolve => window.setTimeout(resolve, 650));
      }
      if (request === selectedAnalysisRequest && selectedAnalysisJob?.state === "completed") {
        const count = selectedAnalysisJob.results.filter(row => row.analysis).length;
        setNotice(`Streaming analysis ready to review for ${count} of ${selectedAnalysisJob.total} selected recording${selectedAnalysisJob.total === 1 ? "" : "s"}.`);
      }
    } catch (failure) { error = String(failure); }
    finally { if (request === selectedAnalysisRequest) { selectedAnalysisBusy = false; analysisBusy = false; } }
  }
  type SelectedStreamJobDto = {
    jobId: string; state: string; total: number; completed: number;
    results: { trackId: string; recordingId: string; status: string; provider?: string | null; sourceId?: string | null; analysis: WholeTrackAnalysisDto | null; error?: string | null }[];
  };
  async function cancelSelectedAnalysis() {
    const jobId = selectedAnalysisJob?.jobId;
    if (!jobId) return;
    try { await call(selectedAnalysisKind === "stream" ? "cancel_selected_stream_analysis" : "cancel_selected_asset_analysis", { jobId }); }
    catch (failure) { error = String(failure); }
  }
  function closeSelectedAnalysis() {
    if (selectedAnalysisBusy && selectedAnalysisJob?.jobId) void cancelSelectedAnalysis();
    selectedAnalysisRequest++;
    selectedAnalysisOpen = false;
    selectedAnalysisBusy = false;
    analysisBusy = false;
  }
  async function applySelectedAnalysis() {
    const job = selectedAnalysisJob;
    if (!job || selectedAnalysisBusy) return;
    const fields = [applyAnalysisBpm ? "bpm" : null, applyAnalysisKey ? "key" : null].filter((field): field is "bpm" | "key" => field !== null);
    if (!fields.length) { setNotice("Choose BPM, key, or both before applying analysis."); return; }
    const policies = {
      bpm: replaceAnalysisBpm ? "replace" : "fillMissing",
      key: replaceAnalysisKey ? "replace" : "fillMissing",
    };
    selectedAnalysisBusy = true;
    analysisBusy = true;
    try {
      for (const row of job.results) {
        if (row.status !== "completed" || !row.analysis || selectedAnalysisApplied.includes(row.assetId)) continue;
        const track = await call<Track>(row.kind === "stream" ? "apply_selected_stream_analysis" : "apply_selected_asset_analysis", {
          jobId: job.jobId,
          ...(row.kind === "stream" ? { trackId: row.assetId } : { assetId: row.assetId }),
          selection: { fields, policies },
        });
        mergeLibraryTrack(track);
        selectedAnalysisTracks = { ...selectedAnalysisTracks, [row.assetId]: track };
        selectedAnalysisApplied = [...selectedAnalysisApplied, row.assetId];
      }
      setNotice(selectedAnalysisKind === "stream"
        ? `Applied reviewed analysis to ${selectedAnalysisApplied.length} selected streamed recording${selectedAnalysisApplied.length === 1 ? "" : "s"}.`
        : `Applied reviewed analysis to ${selectedAnalysisApplied.length} selected local asset${selectedAnalysisApplied.length === 1 ? "" : "s"}.`);
      closeSelectedAnalysis();
      await refreshCurrentAssetFiles();
    } catch (failure) { error = String(failure); }
    finally { selectedAnalysisBusy = false; analysisBusy = false; }
  }
  function mergeLibraryTrack(updated: Track) {
    invalidateLegacyArtistAssets();
    const merge = (track: Track) => sameTrack(track, updated) ? { ...track, ...updated } : track;
    tracks = tracks.map(merge);
    if (selected && sameTrack(selected, updated)) selected = merge(selected);
    if (playingTrack && sameTrack(playingTrack, updated)) playingTrack = merge(playingTrack);
    if (chart) chart = { ...chart, tracks: chart.tracks.map(merge) };
    for (const [key, context] of contexts) {
      contexts.set(key, { ...context, tracks: context.tracks.map(merge), selected: context.selected && sameTrack(context.selected, updated) ? merge(context.selected) : context.selected });
    }
    const updateAssets = (items: LocalAsset[]) => items.map((asset) => asset.recordingId === updated.id || sameTrack(asset.track, updated) ? { ...asset, track: merge(asset.track) } : asset);
    assets = updateAssets(assets);
  }
  async function handleLibraryUpdated(updated: Track) {
    mergeLibraryTrack(updated);
    if (!["library", "favorites", "collections"].includes(view)) return;
    requestLocalPrefixRefresh(updated);
  }
  async function applyArtistMetadata(track: Track, review: MetadataReviewData, acceptedFieldKeys: MetadataFieldKey[]) {
    const accepted = new Set(acceptedFieldKeys);
    const changes = Object.fromEntries(review.fields.filter((field) => accepted.has(field.key)).map((field) => [field.key, field.value]));
    if (!Object.keys(changes).length) return;
    const updated = await call<Track>("update_track", { track: { ...track, ...changes } });
    invalidateLegacyArtistAssets();
    tracks = tracks.map((item) => sameTrack(item, track) ? { ...item, ...updated } : item);
    if (selected && sameTrack(selected, track)) selected = { ...selected, ...updated };
    if (activeArtistTrack && sameTrack(activeArtistTrack, track)) activeArtistTrack = { ...activeArtistTrack, ...updated };
    setNotice(assetIdForTrack(track) || track.localAvailability === "available"
      ? "Metadata saved; local tags are updating."
      : "Accepted metadata fields were saved.");
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
        selected = null;
        candidates = [];
      }
    });
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
      setNotice("Verified recordings merged. Provider chart rankings are retained.");
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
  async function openDownloadChooser(track: Track, initialCandidate?: Candidate, initialChapterId?: string, playlistContext?: PlaylistContext, preferredProvider?: "youtube" | "soundcloud" | "monochrome" | "slskd") {
    if(track.playbackRange) {
      const provenance=track.provenance as {parentTrack?:Track};const candidate=importedAudioCandidate(track);
      const chapter=candidate?.chapters?.find(ch=>ch.id===track.playbackRange!.chapterId);
      if(provenance.parentTrack && candidate && chapter)return downloadChapter(provenance.parentTrack,candidate,chapter);
    }
    const request = ++downloadRequest;
    const typedDirectory = settings.musicDirectory;
    let cached = initialCandidate ?? (preferredProvider || portablePlaybackSource(track) ? undefined : linkedSearchCandidate(track));
    openPlayerPanel(null);
    downloadTrack = track;
    downloadPlaylistContext = playlistContext ? { ...playlistContext } : null;
    downloadDisposition = {mode:"keep"};
    downloadFiles = filesForTrack(track);
    void call<LocalAsset[]>("list_track_files",{trackId:track.id}).then(files=>{if(request===downloadRequest)downloadFiles=files;}).catch(()=>{});
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
        const preferred = preferredProvider ?? searchSource ?? preferredSource(freshSettings);
        const providers = slskdEnabled ? ["monochrome", "youtube", "soundcloud", "slskd"] : ["monochrome", "youtube", "soundcloud"];
        if (!cached && portablePlaybackSource(track)) {
          try {
            const bound = await call<Candidate | null>("resolve_portable_playback_candidate", { track });
            if (request !== downloadRequest || downloadTrack?.id !== track.id) return;
            const source = portablePlaybackSource(track)!;
            if (bound?.provider === source.provider && bound.id === `${source.provider}:${source.sourceId}`) {
              cached = bound;
              downloadCandidates = [bound, ...downloadCandidates];
              downloadCandidate = bound;
            }
          } catch { /* Provider search still offers alternatives when the saved identity is unavailable. */ }
        }
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
            if (!downloadCandidate && !portablePlaybackSource(track) && provider === preferred) {
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
        const keepPlaying = (playingTrack?.id === track.id || currentRadioSong?.id === track.id) && preview && !paused;
        const seedTrack = keepPlaying ? track : { ...track, provenance: { ...(track.provenance && typeof track.provenance === "object" ? track.provenance : {}), sourceKind: "youtubeRadio", audioCandidate: seed } };
        const queue = [seedTrack, ...result.tracks];
         await nativeCommand({action:"queue",tracks:queue,mode:"replace",preserveCurrent:Boolean(keepPlaying)}); if(!keepPlaying)await startPreview(seedTrack,false,false,queue,0);
        if (request !== radioRequest) return;
        openPlayerPanel("queue");
        setNotice(`Song radio · ${result.total} YouTube recommendations.`);
      } finally { if (request === radioRequest) radioBusy = null; }
    });
  }
  async function saveFallbackSettings(kind: "streaming" | "download", enabled: boolean, sources: string[]) {
    const changes = kind === "streaming" ? { streamingFallbackEnabled:enabled, streamingFallbackSources:sources, streamingFallbackPolicyVersion:1, bandcampFallbackEnabled:sources.includes("bandcamp") } : { downloadFallbackEnabled:enabled, downloadFallbackSources:sources };
    settings = { ...settings, ...changes };
    await run(async () => { const fresh = await call<Settings>("get_settings"); await call("save_settings", { settings:{ ...fresh, ...changes } }); });
  }
  function preferredSource(settingsValue: Settings): "youtube" | "soundcloud" | "slskd" | "monochrome" {
    const source = settingsValue.preferredDownloadSource;
    return source === "soundcloud" || source === "slskd" || source === "monochrome" ? source : "youtube";
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
      if (request === downloadRequest && downloadTrack?.id === track.id && downloadCandidate && candidateKey(downloadCandidate)===candidateKey(candidate)) {
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
    if (downloadDisposition.mode === "replace" && splitChapters) { error = "Replace applies to one recording file. Keep existing files when downloading chapters."; return; }
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
          ...(downloadPlaylistContext ? { playlistContext: downloadPlaylistContext } : {}),
          disposition: downloadDisposition,
          track: {
            ...track,
            provenance: {...(track.provenance as Record<string, unknown>), userSelectedSource: true},
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
      setNotice("Added to download queue.");
      downloadRequest++;
      downloadTrack = null;
      downloadPlaylistContext = null;
      downloadCandidate = null;
      downloadCandidates = [];
      focusedDownloadChapterId = null;
    });
    downloadSubmitting = false;
  }
  type AlbumBatchReceipt={status:"accepted"|"running"|"completed"|"cancelled"|"busy"|"empty"|"failed";requestedCount:number;completedCount:number;queuedJobs:Array<{index:number;trackId:string;jobId:string}>;skipped:Array<{index:number;trackId:string;reason:string}>;failed:Array<{index:number;trackId:string;reason:string}>};
  function stopBatchMatching() {
    batchCancelled = true;
    const requestId = activeBatchReadId;
    if (requestId) void call("cancel_remote_read", { requestId }).catch(() => {});
  }

  async function queueSelected(items?: Track[], onReceipt?:(receipt:AlbumBatchReceipt)=>void|Promise<void>, sourceProviderOverride?: "monochrome" | "slskd" | "youtube" | "soundcloud", playlistContext?: PlaylistContext):Promise<AlbumBatchReceipt> {
    const batch = items ?? tracks.filter((t) => checked.includes(t.id));
    const receipt:AlbumBatchReceipt={status:batchRunning?"busy":batch.length?"accepted":"empty",requestedCount:batch.length,completedCount:0,queuedJobs:[],skipped:[],failed:[]};
    if (batchRunning || !batch.length) return receipt;
    // Reserve the single batch slot before settings refreshes, so concurrent
    // automation requests cannot both pass the busy check and enqueue twice.
    batchRunning=true;
    batchCancelled=false;
    const typedDirectory = settings.musicDirectory.trim();
    let latestSettings: Settings;
    try {
      latestSettings = await call<Settings>("get_settings");
    } catch (failure) {
      error = `Could not refresh the saved music folder: ${String(failure)}`;
      receipt.status="failed";receipt.failed.push({index:-1,trackId:"",reason:error});batchRunning=false;await onReceipt?.(receipt);return receipt;
    }
    const targetDirectory = typedDirectory || latestSettings.musicDirectory.trim();
    if (!targetDirectory) {
      settings = latestSettings;
      persistedMusicDirectory = latestSettings.musicDirectory;
      error = "Configure a music directory in Settings before batch acquisition.";
      receipt.status="failed";receipt.failed.push({index:-1,trackId:"",reason:error});batchRunning=false;await onReceipt?.(receipt);return receipt;
    }
    if (typedDirectory && typedDirectory !== latestSettings.musicDirectory) {
      latestSettings.musicDirectory = typedDirectory;
      try {
        await call("save_settings", { settings: latestSettings });
      } catch (failure) {
        error = `Could not save the music folder: ${String(failure)}`;
        receipt.status="failed";receipt.failed.push({index:-1,trackId:"",reason:error});batchRunning=false;await onReceipt?.(receipt);return receipt;
      }
    } else {
      latestSettings.musicDirectory = targetDirectory;
    }
    settings = latestSettings;
    persistedMusicDirectory = targetDirectory;
    const source = sourceProviderOverride ?? preferredSource(latestSettings);
    const profile = outputProfile;
    let queued = 0, skipped = 0, failed = 0;
    const failures: string[] = [];
    const albumKey = (value: string | null | undefined) => (value ?? "")
      .normalize("NFKC").toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim();
    const requestedAlbum = albumKey(batch[0]?.album);
    const canSearchSoulseekAlbum = batch.length > 1 && Boolean(requestedAlbum) &&
      batch.every((track) => albumKey(track.album) === requestedAlbum);
    let soulseekAlbumSearchAttempted = false;
    const soulseekAlbumCandidates = new Map<string, Candidate>();
    try {
      receipt.status="running";await onReceipt?.(receipt);
      for (let i = 0; i < batch.length && !batchCancelled; i++) {
        const track = batch[i];
        batchStatus = `Matching ${i + 1}/${batch.length}: ${track.title}`;
        try {
          const current = queueCandidates[track.id] ?? (playingTrack?.id === track.id ? playingCandidate ?? undefined : undefined);
          const linked = current ??
            linkedSearchCandidate(track) ?? importedAudioCandidate(track);
          const providers = [...new Set([
            ...(linked ? [linked.provider] : []),
            ...downloadProviderOrder(source, latestSettings.downloadFallbackEnabled === true,
              latestSettings.downloadFallbackSources ?? ["monochrome", "slskd", "youtube", "soundcloud"]),
          ])];
          if (providers.some((provider) => provider !== "slskd")) {
            batchStatus = `Finding audio ${i + 1}/${batch.length}: ${track.title}`;
            const requestId = crypto.randomUUID();
            activeBatchReadId = requestId;
            try {
              await call("prepare_download_match_identity", { track, requestId });
            } catch {
              if (batchCancelled) break;
              // Identity enrichment is best-effort. Canonical provider search
              // and the shared assessor still run when metadata is unavailable.
            } finally {
              if (activeBatchReadId === requestId) activeBatchReadId = null;
            }
            if (batchCancelled) break;
          }
          let explicitFolderSourceId: string | undefined;
          const resolution = await resolveBatchSource(
            providers,
            linked,
            async (provider) => {
              if (provider === "slskd") {
                if (canSearchSoulseekAlbum) {
                  if (!soulseekAlbumSearchAttempted) {
                    soulseekAlbumSearchAttempted = true;
                    const requestId = crypto.randomUUID();
                    activeBatchReadId = requestId;
                    try {
                      const assignments = await call<Array<{trackId:string;candidate:Candidate}>>(
                        "find_soulseek_album_candidates", { tracks: batch, requestId },
                      );
                      for (const assignment of assignments) {
                        if (assignment?.trackId && assignment.candidate?.provider === "slskd") {
                          soulseekAlbumCandidates.set(assignment.trackId, assignment.candidate);
                        }
                      }
                    } catch (failure) {
                      if (batchCancelled) throw failure;
                      // The album query is an optimization. Preserve the ordinary
                      // per-track Soulseek fallback if it cannot return a group.
                    } finally {
                      if (activeBatchReadId === requestId) activeBatchReadId = null;
                    }
                  }
                  const albumCandidate = soulseekAlbumCandidates.get(track.id);
                  if (albumCandidate) return [albumCandidate];
                }
                return call<Candidate[]>("find_candidates", { track, providerIds: [provider] });
              }
              const requestId = crypto.randomUUID();
              activeBatchReadId = requestId;
              try {
                return await call<Candidate[]>("find_download_candidates", {
                  track,
                  providerId: provider,
                  requestId,
                });
              } finally {
                if (activeBatchReadId === requestId) activeBatchReadId = null;
              }
            },
            (provider, candidates) => provider === "slskd"
              ? candidates.filter(item => item.provider === "slskd" && item.source?.kind === "soulseek" &&
                  item.source.username?.trim() && item.source.filename?.trim() && item.permission !== "restricted")
                  .sort((a,b) => b.score-a.score)
              : candidates.filter(item => item.provider === provider && item.permission !== "restricted")
                  .sort((a,b) => b.score-a.score),
            (candidate) => call<Candidate>("inspect_candidate", { track, candidate }),
            (candidate) => call<BatchMatchAssessment>("assess_download_candidate", { track, candidate }),
            () => batchCancelled,
            (candidate) => candidate.provider === "slskd" && candidate.source?.kind === "soulseek" &&
              Boolean(candidate.source.username?.trim() && candidate.source.filename?.trim()) &&
              candidate.permission !== "restricted",
            (candidate) => call<BatchMatchAssessment>("assess_download_candidate", { track, candidate }),
            (candidate, assessment) => {
              if (isExplicitSoulseekFolderSource(track, candidate, assessment)) {
                explicitFolderSourceId = candidate.id;
                return true;
              }
              return isExplicitUnresolvedAudioSearchSource(track, candidate, assessment);
            },
            latestSettings.soulseekWaitForSource === true,
          );
          const chosen = resolution.candidate;
          if (batchCancelled) break;
          if (!chosen && resolution.waitingForSoulseek) {
            await call("queue_download_watch", {request:{track,outputProfile:profile,playlistContext:playlistContext ?? null,requiresLossless:false}});
            queued++;receipt.completedCount=i+1;await onReceipt?.(receipt);
            continue;
          }
          if (!chosen) {
            skipped++;
            const reason = resolution.failures.slice(-3).join("; ") || "no suitable source found";
            receipt.skipped.push({index:i,trackId:track.id,reason});receipt.completedCount=i+1;await onReceipt?.(receipt);
            failures.push(`${track.artist} — ${track.title}: ${reason}`);
            continue;
          }
          const job = await call<DownloadJob>("enqueue_download", {
            request: {
              track: explicitFolderSourceId === chosen.id
                ? {...track, provenance:{...(track.provenance && typeof track.provenance === "object" ? track.provenance : {}),userSelectedSource:true}}
                : track,
              ...(playlistContext ? { playlistContext } : {}),
              candidate: chosen,
              formatId: null,
              outputProfile: profile,
            },
          });
          jobs = [job, ...jobs.filter((j) => j.id !== job.id)];
          queued++;
          receipt.queuedJobs.push({index:i,trackId:track.id,jobId:job.id});receipt.completedCount=i+1;await onReceipt?.(receipt);
        } catch (e) {
          failed++;
          receipt.failed.push({index:i,trackId:track.id,reason:String(e)});receipt.completedCount=i+1;await onReceipt?.(receipt);
          failures.push(`${track.artist} — ${track.title}: ${String(e)}`);
        }
      }
      setNotice(`Batch ${batchCancelled ? "stopped" : "complete"}: ${queued} queued; ${skipped} need review; ${failed} failed.`);
      if (failures.length) error = [...failures.slice(0, 5), ...(failures.length > 5 ? [`${failures.length - 5} more track details in the download status`] : [])].join(" · ");
      receipt.status=batchCancelled?"cancelled":"completed";await onReceipt?.(receipt);
    } finally {
      batchRunning = false;
      batchStatus = "";
    }
    return receipt;
  }
  function localPlaylistSnapshot(items: Track[]) {
    const source=JSON.stringify(items.map((track,index)=>[index,track.id,track.provider,track.providerId,track.sourceUrl,track.artist,track.title,track.version,track.album,track.albumArtist,track.label,track.genre,track.durationSeconds,track.releaseDate,track.isrc,track.catalogNumber,track.trackNumber,track.discNumber,track.assetId,track.localPath]));
    let hash=2166136261;
    for(let index=0;index<source.length;index++)hash=Math.imul(hash^source.charCodeAt(index),16777619);
    return {trackCount:items.length,revision:`${items.length}:${(hash>>>0).toString(16).padStart(8,"0")}`};
  }
  async function downloadPlaylist(id: string, useDisplayed = false, expectedRevision?:string, onReceipt?:(receipt:AlbumBatchReceipt)=>void|Promise<void>) {
    // A local playlist download includes the entire stored order, beyond its visible page.
    // Imported snapshots retain their explicitly selected import version.
    const isDisplayedImport = Boolean(useDisplayed && activeBrowserImport?.collectionId === id);
    const items = isDisplayedImport ? [...tracks] : await call<Track[]>("list_tracks", { view: "collections", collectionId: id });
    if(expectedRevision && localPlaylistSnapshot(items).revision!==expectedRevision)throw new Error("Playlist changed; reopen the exact local playlist before downloading");
    return await queueSelected(items,onReceipt,undefined,isDisplayedImport ? undefined : savedPlaylistContext(id));
  }
  async function playCurrentPlaylist(mode: "play" | "queue") {
    const id = collectionId;
    if (view !== "collections" || !id || playlistPlaybackAction) return;
    const request = ++playlistPlaybackRequest;
    const context = activeContext;
    const navigation = navigationRequest;
    const isCurrent = () => request === playlistPlaybackRequest && view === "collections"
      && collectionId === id && activeContext === context && navigationRequest === navigation;
    playlistPlaybackAction = mode;
    error = "";
    try {
      // Read the complete persisted occurrence list. The visible page can be
      // filtered, sorted, or limited to the first 200 rows.
      const items = await call<Track[]>("list_tracks", { view: "collections", collectionId: id });
      if (!isCurrent()) return;
      if (!Array.isArray(items)) throw new Error("The saved playlist could not be read.");
      if (!items.length) {
        setNotice("This playlist has no saved tracks to play.");
        return;
      }
      if (mode === "play") await startPreview(items[0]!, false, false, items, 0);
      else await nativeCommand({ action: "queue", tracks: items, mode: "append" });
    } catch (failure) {
      if (isCurrent()) error = String(failure);
    } finally {
      if (request === playlistPlaybackRequest) playlistPlaybackAction = null;
    }
  }
  let downloadSortField = $state<TrackSortField>("sourceOrder"),downloadSortDirection = $state<SortDirection>("asc"),downloadMinDuration=$state(""),downloadMaxDuration=$state("");
  const sortedDownloadJobs=$derived.by(()=>{
    const byId=new Map(jobs.map(job=>[job.id,job]));
    const rows=filterTracks(jobs.filter(job=>!(job.track.provenance as Record<string,unknown>)?.deferredSoulseekWatchId).map(job=>({...job.track,id:job.id})),{minDurationSeconds:parseDurationBound(downloadMinDuration)??undefined,maxDurationSeconds:parseDurationBound(downloadMaxDuration)??undefined});
    return sortTracks(rows,downloadSortField,downloadSortDirection).map(track=>byId.get(track.id)!);
  });
  // A paged first page cannot establish that a field is globally absent from the
  // active history query. Keep all choices available until every matching play
  // has been loaded; then derive availability from the complete loaded result.
  const historySortAvailability = $derived(historyHasMore
    ? allTrackSortFieldsAvailable()
    : trackSortAvailability(historyItems.map(item => item.track)));
  const downloadSortAvailability = $derived(trackSortAvailability(sortedDownloadJobs.map(job => job.track)));
  const mainSortAvailability = $derived(["library", "favorites", "collections"].includes(view)
    ? localSortAvailabilityPending ? allTrackSortFieldsAvailable() : localSortAvailability ?? (localPageLoading || localPageTotal > tracks.length ? allTrackSortFieldsAvailable() : trackSortAvailability(tracks))
    : trackSortAvailability(tracks));
  async function refreshJobs() {
    await run(async () => {
      jobs = await call<DownloadJob[]>("list_jobs");
      const paths = [...new Set(jobs.map(job => job.outputPath).filter((path): path is string => Boolean(path)))];
      const files: LocalAsset[] = [];
      if (scopedAssetCommandsSupported) {
        try { for (let index = 0; index < paths.length; index += 200) files.push(...await call<LocalAsset[]>("list_assets_for_paths", { paths: paths.slice(index, index + 200) })); }
        catch (failure) {
          if (!isUnknownAssetCommand(failure)) throw failure;
          scopedAssetCommandsSupported = false;
          files.push(...(await call<LocalAsset[]>("list_assets")).filter(asset => paths.includes(asset.path)));
        }
      } else files.push(...(await call<LocalAsset[]>("list_assets")).filter(asset => paths.includes(asset.path)));
      const byPath = new Map<string, LocalAsset>();
      for (const asset of files) if (!byPath.has(asset.path)) byPath.set(asset.path, asset);
      downloadAssetsByJob = Object.fromEntries(jobs.flatMap(job => { const asset = job.outputPath ? byPath.get(job.outputPath) : undefined; return asset ? [[job.id, asset]] : []; }));
    });
  }
  async function openAutotagReview(assetId: string) {
    autotagReviewId = assetId;
    autotagReviewAsset = await call<LocalAsset | null>("get_asset_by_id", { assetId });
    showDownloads = false;
  }
  const namedCollections=$derived(visibleCollections.map(item=>{
    const binding=remotePlaylistBindings.find(binding=>binding.collectionId===item.id || binding.sourceCollectionId===item.id);
    return {...item,...(binding ? {ownerName:binding.ownerName,ownerId:binding.ownerId} : {}),name:sidebarLayout.names?.[item.id]??(binding?item.name.replace(/ \(remote copy\)$/," ").trim():item.name)};
  }));
  const playlistDestinations = $derived(namedCollections.filter(item=>item.name.toLocaleLowerCase().includes(playlistDestinationQuery.toLocaleLowerCase())));
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
      closeImportDialog();importTargetCollectionId=null;await openPlaylist(created.id);renamePlaylistName=name;renamingPlaylist=true;});
  }
  async function renameCurrentPlaylist(){if(!currentCollection||!renamePlaylistName.trim())return;await run(async()=>{await call("rename_collection",{collectionId:currentCollection!.id,name:renamePlaylistName.trim()});if(sidebarLayout.names?.[currentCollection!.id])sidebarLayout=await call<typeof sidebarLayout>("save_sidebar_layout",{layout:{...sidebarLayout,names:{...sidebarLayout.names,[currentCollection!.id]:renamePlaylistName.trim()}}});collections=await call<Collection[]>("list_collections");renamingPlaylist=false;});}
  async function removeSidebarStation(station:RadioStation){
    await run(async()=>{
      radioStations=await call<RadioStation[]>("remove_radio_station",{stationId:station.id});
      if(selectedRadioStationId===station.id)selectedRadioStationId=radioStations[0]?.id??"";
      setNotice(`Removed station “${station.name}”.`);
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
  function captureBackupFrontendPreferences(): Record<string, unknown> {
    const preferences: Record<string, unknown> = {};
    if (typeof localStorage === "undefined") return preferences;
    const jsonKeys = [
      "burrata.discovery.search-scope.v2",
      "burrata.discovery.sources.v1",
      "burrata.discovery.search-history.v1",
      "burrata.soulseek.wishlist.v1",
      "burrata.lyrics-timing.v1",
    ];
    for (const key of jsonKeys) {
      try {
        const raw = localStorage.getItem(key);
        if (raw != null) preferences[key] = JSON.parse(raw);
      } catch { /* The native allowlist ignores malformed local values. */ }
    }
    for (const key of ["burrata.discovery.continue-search.v1", "burrata.queue-end-radio.v1", "burrata.waveform-enabled.v1", "burrata.stream-media-analysis-enabled.v1", "burrata.playlist.include-ytdlp-search.v1"]) {
      const raw = localStorage.getItem(key);
      if (raw === "true" || raw === "false") preferences[key] = raw === "true";
    }
    for (const kind of ["sidebar", "info"] as const) {
      const raw = localStorage.getItem(`burrata.layout.${kind}.v1`);
      const width = raw == null ? NaN : Number(raw);
      if (Number.isFinite(width)) preferences[`burrata.layout.${kind}.v1`] = width;
    }
    return preferences;
  }

  async function applyPendingBackupFrontendPreferences(): Promise<boolean> {
    if (!desktop()) return true;
    try {
      await hydrateBackupFrontendPreferences();
    } catch (failure) {
      // Keep the native pending envelope so the next startup can retry.
      backupHydrationError = String(failure);
      backupHydrationFailureKind = "preferences";
      return false;
    }
    try {
      await applyPendingBackupPlaybackRestore(
        () => call<PendingBackupPlaybackRestore | null>("get_backup_playback_restore"),
        localStorage,
        (nonce, receiptSessionId) => call<BackupPlaybackRestoreAck>("ack_backup_playback_restore", { nonce, receiptSessionId }),
        values => values.forEach((value, key) => backupStartupStorageOverrides.set(key, value)),
      );
      return true;
    } catch (failure) {
      backupHydrationError = String(failure);
      backupHydrationFailureKind = "playback";
      return false;
    }
  }
  async function hydrateBackupFrontendPreferences() {
    return applyBackupFrontendPreferences(
      () => call<PendingBackupFrontendPreferences | null>("get_backup_frontend_preferences"),
      localStorage,
      (nonce, receiptSessionId) => call<BackupFrontendPreferenceAck>("ack_backup_frontend_preferences", { nonce, receiptSessionId }),
      values => values.forEach((value, key) => backupStartupStorageOverrides.set(key, value)),
    );
  }
  let backupHydrationError = $state<string | null>(null);
  let backupHydrationFailureKind = $state<"preferences" | "playback" | null>(null);
  let backupHydrationReady = $state(false);
  let retryBackupStartup = $state<(() => void) | null>(null);
  $effect(() => {
    if (!desktop() || !backupHydrationReady || !playbackControlsOwned) return;
    persistPlaybackControlsWhenOwned(
      localStorage,
      { volume, shuffle: shuffleEnabled },
      { desktop: desktop(), hydrated: backupHydrationReady, engineOwned: playbackControlsOwned },
    );
  });
  const backupFrontendHydration = applyPendingBackupFrontendPreferences();

  async function pausePlaybackForRestoreCheckpoint(request: BackupCheckpointRequest) {
    const deadline = Date.now() + Math.max(250, Math.min(request.deadlineMs, 7_500));
    const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

      await nativeBridge.command({ action: "pause" });
      let previous: { position: number; revision: number | undefined; entryIds: string[] } | null = null;
      while (Date.now() < deadline) {
        const snapshot = await call<NativePlaybackSnapshot & { queueTotal: number; queueEntryIds: string[] }>("get_playback_snapshot");
        const stable = snapshot.engine === "native" && snapshot.paused === true
          && ["paused", "idle", "ended", "failed"].includes(snapshot.status)
          && Number.isFinite(snapshot.position)
          && Array.isArray(snapshot.queueEntryIds) && snapshot.queueEntryIds.length === snapshot.queue.length
          && previous?.position === snapshot.position
          && previous?.revision === snapshot.queueRevision
          && JSON.stringify(previous?.entryIds) === JSON.stringify(snapshot.queueEntryIds);
        if (stable) {
          applyNativeSnapshot(snapshot);
          return;
        }
        previous = { position: snapshot.position, revision: snapshot.queueRevision, entryIds: [...(snapshot.queueEntryIds ?? [])] };
        await wait(60);
      }
      throw new Error("Native playback did not reach a stable paused checkpoint");

  }

  function captureBackupPlayback(): Record<string, unknown> {
    const queue = playbackQueue.snapshot();
    const tracks = [...queue.tracks];
    const entryIds = [...(queue.entryIds ?? [])];
    return {
      queue: {
        tracks, entryIds, currentIndex: queue.currentIndex, repeat: queue.repeat,
        ...(queue.queueLoudnessContexts ? { queueLoudnessContexts: queue.queueLoudnessContexts } : {}),
      },
      session: {
        index: queue.currentIndex,
        position: (position),
        repeat: queue.repeat,
        shuffle: shuffleEnabled,
        volume,
        queueRevision: playbackQueueGeneration,
      },
    };
  }

  async function waitForBackupJob(jobId: string, report: (progress: BackupProgress) => void) {
    const unlisten = await listen<BackupProgress>("backup-progress", event => {
      if (event.payload.jobId === jobId) report(event.payload);
    });
    try {
      while (true) {
        const status = await call<BackupProgress>("get_backup_job", { jobId });
        report(status);
        if (status.state === "complete") return { jobId };
        if (status.state === "failed" || status.state === "canceled") {
          throw new Error(status.message || `Backup job ${status.state}`);
        }
        await new Promise(resolve => setTimeout(resolve, 500));
      }
    } finally { unlisten(); }
  }

  async function inspectBackupForUi() {
    if (!desktop()) return null;
    return await call<BackupInspection | null>("inspect_backup");
  }
  async function previewBackupRestore(previewId: string, options: { mode: "merge" | "replace"; componentIds: string[]; pathMappings: { rootKey: string; destinationToken: string }[] }) {
    return call<NonNullable<BackupInspection["summary"]>>("preview_backup_restore", { previewId, options });
  }
  async function createBackupForUi(options: { includeMedia: boolean }, report: (progress: BackupProgress) => void) {
    const started = await call<{ jobId: string }>("create_backup", { options: { includeMedia: options.includeMedia } });
    return waitForBackupJob(started.jobId, report);
  }
  async function restoreBackupForUi(previewId: string, options: { mode: "merge" | "replace"; componentIds: string[]; pathMappings: { rootKey: string; destinationToken: string }[] }, report: (progress: BackupProgress) => void) {
    return startRestoreAndWaitForCompletion(
      () => call<{ jobId: string }>("restore_backup", { previewId, options }),
      waitForBackupJob,
      report,
    );
  }
  async function chooseBackupMediaRoot(previewId: string, rootKey: string) {
    return call<BackupMediaRootPreview | null>("choose_backup_restore_root", { previewId, rootKey });
  }
  async function estimateBackupForUi(includeMedia: boolean) { return call<BackupMediaEstimate>("estimate_backup", { includeMedia }); }
  async function cancelBackupJob(jobId: string) { await call("cancel_backup_job", { jobId }); }
  async function loadBackupSchedule() { return call<BackupSchedule>("get_backup_schedule"); }
  async function loadActiveBackupJob() { return call<BackupProgress | null>("get_active_backup_job"); }
  async function subscribeBackupProgress(handler: (progress: BackupProgress) => void) {
    return listen<BackupProgress>("backup-progress", event => handler(event.payload));
  }
  async function saveBackupSchedule(schedule: BackupSchedule) { return call<BackupSchedule>("set_backup_schedule", { schedule }); }
  async function chooseBackupScheduleDestination() { return call<string | null>("choose_backup_schedule_destination"); }
  async function exportBackupSettings() {
    return call<boolean>("export_backup_settings");
  }
  async function inspectBackupSettings() { return call<SettingsImportPreview | null>("inspect_backup_settings"); }
  async function applyBackupSettings(previewId: string, selection: SettingsImportSelection, preview: SettingsImportPreview) {
    const importsBackgroundWork = selection.preferenceKeys.includes("autotag.settings") ||
      selection.frontendPreferenceKeys.includes("burrata.stream-media-analysis-enabled.v1") ||
      selection.application && preview.applicationFields.includes("backgroundSourcePreparation");
    if (importsBackgroundWork) {
      streamMediaAnalysisRestorePolicyReady = false;
      streamMediaAnalysisRestorePaused = true;
    }
    const result = await call<{ frontendPreferences?: Record<string, unknown> }>("apply_backup_settings", { previewId, selection });
    let frontendPreferencesPending = false;
    if (result.frontendPreferences && Object.keys(result.frontendPreferences).length) {
      try {
        frontendPreferencesPending = (await hydrateBackupFrontendPreferences()) === "deferred";
      } catch {
        throw new Error("Native settings were applied, but imported interface preferences could not be saved. They remain queued for retry when app storage is available.");
      }
    }
    const importedStreamAnalysis = result.frontendPreferences?.["burrata.stream-media-analysis-enabled.v1"];
    if (typeof importedStreamAnalysis === "boolean") {
      streamMediaAnalysisEnabled = importedStreamAnalysis;
      streamMediaAnalysisPreferenceReady = true;
    }
    if (selection.application || selection.preferenceKeys.length > 0 || selection.frontendPreferenceKeys.length > 0) {
      await refreshStreamMediaAnalysisRestorePolicy();
    }
    const refreshed = await call<Settings>("get_settings");
    settings = { ...refreshed, matchingLevel: refreshed.matchingLevel ?? "balanced", playbackStartupTimeoutSeconds: refreshed.playbackStartupTimeoutSeconds ?? 20 };
    return frontendPreferencesPending ? { frontendPreferencesPending: true } : undefined;
  }

  function preventFileDropNavigation(event: DragEvent) {
    const types = Array.from(event.dataTransfer?.types ?? []);
    const alreadyHandled = event.defaultPrevented;
    if (types.includes("Files") || types.includes("text/uri-list")) event.preventDefault();
    if (event.type !== "drop" || alreadyHandled) return;
    const files = Array.from(event.dataTransfer?.files ?? []);
    if (files.length === 1 && files[0].name.toLowerCase().endsWith(".burrata-backup")) {
      droppedBackupPreview = null;
      importChoice = "restore-backup";
      showImport = true;
      setNotice("Select the dropped backup in Restore Backup to review its contents.");
    }
  }

  function openPlaylistImport(){droppedBackupPreview=null;importTargetCollectionId=view==="collections"?collectionId:null;importChoice=null;showImport=true;void refreshBrowser().catch(()=>{});}
  function openDroppedBackupPreview(preview: BackupInspection) {
    droppedBackupPreview = preview;
    importChoice = "restore-backup";
  }
  function closeImportDialog() {
    nativeBackupDropRequest++;
    droppedBackupPreview = null;
    showImport = false;
  }
  function returnToImportChoices() {
    nativeBackupDropRequest++;
    droppedBackupPreview = null;
    importChoice = null;
  }
  onMount(() => {
    if (!desktop() || !window.__TAURI__?.event?.listen) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<NativeImportFileDrop>("import-file-drop", ({ payload }) => {
      if (!payload || payload.kind !== "backup" || typeof payload.token !== "string" || !payload.token) return;
      const request = ++nativeBackupDropRequest;
      void call<ConsumedImportDrop>("consume_import_drop", { token: payload.token }).then((result) => {
        if (disposed || request !== nativeBackupDropRequest) return;
        if (result.kind !== "backup") throw new Error("The dropped item did not contain a supported backup.");
        openDroppedBackupPreview(result.preview);
        showImport = true;
      }).catch((failure) => {
        if (!disposed && request === nativeBackupDropRequest) setNotice(`Could not inspect dropped backup: ${String(failure)}`);
      });
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((failure) => {
      if (!disposed) setNotice(`Could not listen for dropped backups: ${String(failure)}`);
    });
    return () => {
      disposed = true;
      nativeBackupDropRequest++;
      unlisten?.();
    };
  });

  async function finishBrowserImport(imported:BrowserImportResult){
    await refreshBrowser();
    if(importTargetCollectionId){const target=importTargetCollectionId;const importedChart=await call<Chart>("load_browser_import",{id:imported.id});await appendPlaylistItems({collectionId:target,tracks:importedChart.tracks});collections=await call<Collection[]>("list_collections");closeImportDialog();importTargetCollectionId=null;await openPlaylist(target);}
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
  const playingRadioStationId = $derived.by(() => {
    if (!liveRadioPlayback || !playingTrack) return "";
    const provenance = playingTrack.provenance as {stationId?:string;radioStationId?:string} | undefined;
    return provenance?.radioStationId || provenance?.stationId || playingTrack.providerId || "";
  });
  const playingRadioStation = $derived(radioStations.find(station => station.id === playingRadioStationId) ?? null);
  const playingRadioStationName = $derived.by(() => {
    if (!liveRadioPlayback) return "";
    const provenance = playingTrack?.provenance as {radioStationName?:string} | undefined;
    return playingRadioStation?.name || provenance?.radioStationName || nativeSnapshot?.radioNowPlaying?.stationName || "Radio";
  });
  function playRadioStation(station:RadioStation){
    selectedRadioStationId=station.id;showAddRadio=false;
    const track:Track={id:`radio:${station.id}`,provider:"radio",providerId:station.id,title:station.name,artist:station.provider === "radio_paradise" ? "Radio Paradise" : "Internet radio",artworkUrl:station.faviconUrl??undefined,sourceUrl:station.homepageUrl??station.sourceUrl,favorite:false,provenance:{stationId:station.id,radioStreamUrl:station.streamUrl,sourceKind:"radio"}};
    void startPreview(track,false,false,[track],0);
  }
  async function createCollection() {
    if (!newCollection.trim()) return;
    const startingView = view, startingContext = activeContext;
    await run(async () => {
      const c = await call<Collection>("create_collection", {
        name: newCollection.trim(),
      });
      collections = [...collections, c];
      newCollection = "";
      if (view === startingView && activeContext === startingContext) collectionId = c.id;
      if (startingView === "collections" && view === startingView && activeContext === startingContext) {
        activeContext = contextKey();
        tracks = [];
        localPageTotal = 0;
        selected = null;
        checked = [];
        contexts.set(activeContext, { tracks, selected: null, filter, sort, descending, scroll: 0 });
      }
    });
  }
  async function createAndAddPlaylist() {
    const toAdd = checked.length ? tracks.filter((track) => checked.includes(track.id)) : selected ? [selected] : [];
    if (!newCollection.trim() || !toAdd.length) return;
    const startingView = view, startingContext = activeContext;
    await run(async () => {
      const created = await call<Collection>("create_collection", { name: newCollection.trim() });
      await appendPlaylistItems( { collectionId: created.id, tracks: toAdd });
      collections = await call<Collection[]>("list_collections");
      newCollection = "";
      const stillCurrent = view === startingView && activeContext === startingContext;
      if (stillCurrent) {
        collectionId = created.id;
        checked = [];
        setNotice(`Created ${created.name} and added ${toAdd.length} recording${toAdd.length === 1 ? "" : "s"}.`);
      }
      if (startingView === "collections" && stillCurrent) {
        activeContext = contextKey();
        tracks = [];
        localPageTotal = 0;
        selected = null;
        contexts.set(activeContext, { tracks, selected, filter, sort, descending, scroll: 0 });
      }
    });
  }

  function selectedPlaylistIndex() {
    if (!selected) return -1;
    const position = playlistPosition(selected);
    return position == null ? -1 : position - 1;
  }
  let playlistReordering = $state(false);
  async function moveSelectedPlaylistEntry(direction: number) {
    const from = selectedPlaylistIndex();
    await movePlaylistEntry(from, from + direction);
  }
  async function movePlaylistEntry(from: number, to: number) {
    if (playlistReordering || localPageLoading || view !== "collections" || !collectionId) throw new Error("Playlist order could not be changed in the current view.");
    if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from === to || to >= playlistMoveLimit) throw new Error("Playlist move is outside the saved playlist order.");
    const targetId = collectionId, targetContext = activeContext, movedTrack = selected;
    playlistReordering = true;
    try {
      const editedId = await call<string>("move_playlist_entry", { collectionId: targetId, from, to });
      collections = await call<Collection[]>("list_collections");
      if (view !== "collections" || collectionId !== targetId || activeContext !== targetContext) throw new Error("Playlist changed before the new order could be shown.");
      if (editedId && editedId !== targetId) {
        collectionId = editedId; activeContext = contextKey(); tracks = []; localPageTotal = 0;
        setNotice("Created a local playlist copy with the new order.");
      }
      selectedLocalOccurrences = [];
      if (movedTrack && selected === movedTrack) {
        const selectedPosition = playlistPosition(movedTrack);
        if (selectedPosition != null) {
          const old = selectedPosition - 1;
          const next = old === from ? to : from < old && to >= old ? old - 1 : from > old && to <= old ? old + 1 : old;
          selected = {...movedTrack,provenance:{...(movedTrack.provenance as Record<string,unknown> ?? {}),playlistPosition:next+1}};
        }
      }
      requestLocalPrefixRefresh(undefined, true);
    } catch (failure) {
      error = String(failure);
      throw failure;
    } finally { playlistReordering = false; }
  }
  async function removePlaylistTrack(track: Track) {
    if (view !== "collections" || !collectionId) return;
    const targetId = collectionId, targetContext = activeContext;
    await run(async () => {
      const editedId = await call<string | null>("remove_from_collection", { collectionId: targetId, trackIds: [track.id] }) || targetId;
      collections = await call<Collection[]>("list_collections");
      const stillCurrent = view === "collections" && collectionId === targetId && activeContext === targetContext;
      if (stillCurrent && editedId !== targetId) {
        collectionId = editedId;
        activeContext = contextKey();
        tracks = [];
        localPageTotal = 0;
      } else if (stillCurrent) requestLocalPrefixRefresh(undefined, true);
      if (stillCurrent) {
        checked = checked.filter(id => id !== track.id);
        if (selected?.id === track.id) selected = null;
      }
      setNotice("Removed from playlist.");
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
      setNotice("Playlist restored.");
    });
  }
  async function removeSelectedFromPlaylist() {
    if (view !== "collections" || !collectionId || !checked.length) return;
    const targetId = collectionId, targetContext = activeContext, targetTrackIds = [...checked];
    await run(async () => {
      const editedId = await call<string | null>("remove_from_collection", { collectionId: targetId, trackIds: targetTrackIds }) || targetId;
      collections = await call<Collection[]>("list_collections");
      if (view === "collections" && collectionId === targetId && activeContext === targetContext) {
        if (editedId !== targetId) {
          collectionId = editedId;
          activeContext = contextKey();
          tracks = [];
          localPageTotal = 0;
        } else requestLocalPrefixRefresh(undefined, true);
        checked = [];
        selected = null;
      }
      setNotice("Removed selected recordings from this playlist.");
    });
  }
  async function addSelected() {
    if ((!selected && !checked.length) || !collectionId) return;
    await run(async () => {
      await appendPlaylistItems( {
        collectionId,
        tracks: checked.length
          ? tracks.filter((t) => checked.includes(t.id))
          : [selected!],
      });
      collections = await call<Collection[]>("list_collections");
      setNotice("Track added to playlist.");
    });
  }

  async function applySourceImport(incoming: Track[], options: { destinationId: string; newPlaylistName: string; folderPath: string; sectionName: string; duplicatePolicy: "keep" | "skip-repeated"; localAssetsImported?: boolean }) {
    const targetId = importTargetCollectionId ?? options.destinationId;
    let target = targetId;
    if (!target) {
      const name = options.newPlaylistName.trim();
      if (!name) throw new Error("Choose a destination playlist or enter a new playlist name.");
      const created = await call<Collection>("create_collection", { name });
      target = created.id;
    }
    const isUniqueTarget = target === "favorites" || target === "history" || target === DOWNLOADS_PLAYLIST_ID || target === playlistPreferences.favoritesPlaylistId;
    if (isUniqueTarget && (options.folderPath.trim() || options.sectionName.trim())) throw new Error("Folders and sections can only be applied to regular playlists.");
    const sourceTracks = options.duplicatePolicy === "skip-repeated"
      ? incoming.filter((track, index) => incoming.findIndex(candidate => candidate.id === track.id) === index)
      : incoming;
    const imported = sourceTracks.map(track => track.provider === "local" && track.localPath ? track : null);
    const prepared: Track[] = [];
    for (let index = 0; index < sourceTracks.length; index++) {
      const track = sourceTracks[index];
      if (imported[index] && !options.localAssetsImported) prepared.push(await call<Track>("import_local_file", { path: track.localPath! }));
      else prepared.push(track);
    }
    const previousCount = collections.find(item => item.id === target)?.trackCount ?? 0;
    await call(isUniqueTarget || options.duplicatePolicy === "skip-repeated" ? "add_to_collection" : "append_playlist_entries", { collectionId: target, tracks: prepared });
    collections = await call<Collection[]>("list_collections");
    const placedCount = options.duplicatePolicy === "skip-repeated"
      ? Math.max(0, (collections.find(item => item.id === target)?.trackCount ?? previousCount) - previousCount)
      : prepared.length;
    if (options.folderPath.trim() || options.sectionName.trim()) {
      await call("place_imported_playlist_items", { collectionId: target, folderPath: options.folderPath, sectionName: options.sectionName, occurrenceCount: placedCount });
    }
    await refreshFavorites();
    setNotice(`Applied ${prepared.length} tracks to ${collections.find(item => item.id === target)?.name ?? "playlist"}.`);
    closeImportDialog(); importChoice = null; importTargetCollectionId = null;
  }

  function isCandidatePreviewCurrent(candidate: Candidate, track: Track | null): boolean {
    return Boolean(track && nativeSnapshot?.trackId === track.id &&
      nativeSnapshot.candidate?.provider === candidate.provider && nativeSnapshot.candidate.id === candidate.id &&
      ["resolving", "buffering", "playing", "paused"].includes(nativeSnapshot.status));
  }
  async function previewCandidate(candidate: Candidate, track: Track | null = selected) {
    if (!track) return;
    clearTimeout(autoTimer);
    setAutomationPlaybackOwner(null);
    if (isCandidatePreviewCurrent(candidate, track)) {
      await nativeCommand({action: nativeSnapshot?.status === "paused" ? "resume" : "pause"});
      return;
    }
    await run(() => nativeBridge.command({action:"play",tracks:[track],index:0,candidate,candidateOnly:true,...playbackOperationTrace()}));
  }
  async function closePreview(preserveQueuedOwner=false) {

    if(preserveQueuedOwner && queuedOwnerForResume())reportQueuedPlaybackPhase("paused");else setAutomationPlaybackOwner(null);
    nextPreviewRequest();
     playbackLoadSequence++; playbackRunGeneration++; queueRadio.cancel(); resolvingPlayback=false; bufferingAudio=false; await nativeCommand({action:"stop"}); return;

  }
  let browserRefreshInFlight: Promise<void> | null = null;
  function refreshBrowser(): Promise<void> {
    if (browserRefreshInFlight) return browserRefreshInFlight;
    browserRefreshInFlight = (async () => {
      try {
        const [status, imports] = await Promise.all([
          call<BrowserStatus>("get_browser_status"), call<BrowserImportResult[]>("list_browser_imports"),
        ]);
        browserStatus = status; browserImports = imports;
      } finally { browserRefreshInFlight = null; }
    })();
    return browserRefreshInFlight;
  }
  async function openBrowserImport(result: BrowserImportResult, revealImportDetails = false, recordRoute = true) {
    if (recordRoute) rememberRoute();
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
    setNotice(revealImportDetails ? `${result.title}: ${result.capturedCount} tracks imported${result.complete ? "." : " (partial capture)."}` : "");
  }
  async function copyCurrentImport() {
    if (!activeBrowserImport) return;
    const date = importDateFilter === "all" ? "all dates" : new Date(activeBrowserImport.fetchedAt).toLocaleDateString();
    const copy = await call<Collection>("create_collection", { name: `${activeBrowserImport.title} (${date})`.slice(0, 160) });
    await appendPlaylistItems( { collectionId: copy.id, tracks });
    collections = await call<Collection[]>("list_collections"); await openPlaylist(copy.id);
  }
  async function copyFavoritesPlaylist() {
    await run(async () => {
      const tracks = await call<Track[]>("list_tracks", { view: "favorites" });
      const name = namedCollections.find(item => item.id === playlistPreferences.favoritesPlaylistId)?.name ?? sidebarLayout.names?.favorites ?? "Liked Songs";
      const copy = await call<Collection>("create_collection", { name: `${name} (Copy)`.slice(0, 160) });
      await appendPlaylistItems({ collectionId: copy.id, tracks });
      collections = await call<Collection[]>("list_collections");
      await openPlaylist(copy.id);
      setNotice(`Made an editable copy of ${name}.`);
    });
  }
  async function refreshLinkedPlaylist(id: string) {
    const binding = remotePlaylistBindings.find(binding => binding.collectionId === id || binding.sourceCollectionId === id);
    if (!binding) return;
    await run(async () => {
      await call("refresh_remote_playlist_binding", { bindingId: binding.bindingId });
      await refreshRemotePlaylistBindings();
      await refreshCollections();
      if (collectionId === binding.collectionId || collectionId === binding.sourceCollectionId) await openPlaylist(binding.collectionId);
      setNotice("Playlist refreshed. Local edits are preserved.");
    });
  }
  function openCurrentPlaylistMenu(event: MouseEvent) {
    const collection = view === "collections" ? currentCollection : (activeBrowserImport ? { id: activeBrowserImport.collectionId, name: playlistTitle(activeBrowserImport.title, activeBrowserImport.provider), trackCount: tracks.length } : chart ? { id: chart.id, name: chart.title, trackCount: tracks.length } : null);
    if (!collection) return;
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    playlistActions = { collection, anchor: { x: rect.right - 260, y: rect.bottom + 4 } };
  }
  function playlistMirrorCollections(): Collection[] {
    const byId = new Map(collections.map((collection) => [collection.id, collection]));
    byId.set("favorites", { id: "favorites", name: "Liked Songs", trackCount: favoriteTrackIds.length });
    byId.set("history", { id: "history", name: "Listening history", trackCount: 0 });
    return [...byId.values()];
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
    setNotice(""); filter = "";
  }
  async function requestBrowserImport() {
    await run(async () => {
      const url=browserUrl.trim();browserRequest="direct";browserMessage="Reading the public source…";
      if (!importTargetCollectionId) { urlImportDestinationId="";urlImportNewPlaylistName="";urlImportFolderPath="";urlImportSectionName="";urlImportDuplicatePolicy="keep"; }
      try {
        collectionImportPreview=await call<CollectionImportPreview>("preview_collection_import",{url});
        showImport=true;
        browserRequest="";browserMessage="Import preview ready. Review the changes below.";
      }catch(failure){
        browserRequest="";
        const reason=String(failure);
        browserRequest=await call<string>("request_browser_import",{url});
        browserMessage=`Direct import unavailable: ${reason}. Sent to the connected browser; use the companion to capture the latest tracklist.`;
        await refreshBrowser();
      }
    });
  }
  async function applyCollectionImportPreview() {
    if (!collectionImportPreview) return;
    const previewId=collectionImportPreview.previewId;
    await run(async()=>{
      const contextualTarget = importTargetCollectionId;
      let destinationId = contextualTarget ?? urlImportDestinationId;
      const wantsPlacement = Boolean(urlImportFolderPath.trim() || urlImportSectionName.trim());
      if (!contextualTarget && !destinationId && urlImportNewPlaylistName.trim()) {
        destinationId = (await call<Collection>("create_collection", { name: urlImportNewPlaylistName.trim() })).id;
      }
      if (!contextualTarget && wantsPlacement && !destinationId) throw new Error("Choose a destination playlist or name a new playlist before applying folder or section placement.");
      const args: Record<string, unknown> = { previewId };
      if (!contextualTarget && destinationId) {
        args.destinationCollectionId = destinationId;
        args.duplicatePolicy = urlImportDuplicatePolicy;
        args.folderPath = urlImportFolderPath;
        args.sectionName = urlImportSectionName;
      }
      const result=await call<BrowserImportResult>("apply_collection_import",args);
      collectionImportPreview=null;
      await finishBrowserImport(result);
      if (!contextualTarget && destinationId) await openPlaylist(destinationId);
    });
  }
  async function retryFailedImport() {
    if (!activeBrowserImport) return;
    await run(async()=>{collectionImportPreview=await call<CollectionImportPreview>("retry_import_failures",{importId:activeBrowserImport!.id});});
  }

  async function chooseMusicDirectory() {
    await run(async () => {
      const directory = await call<string | null>("choose_music_directory");
      if (!directory) return;
      settings = { ...settings, musicDirectory: directory };
      persistedMusicDirectory = directory;
      setNotice("Music folder saved.");
    });
  }
  async function savePlaybackDiagnosticsEnabled(enabled: boolean) {
    if (!enabled && settings.playbackDiagnosticsEnabled) {
      const context = false;

    }
    await call("set_playback_diagnostics_enabled", {enabled});
    settings = {...settings, playbackDiagnosticsEnabled:enabled};

  }

  let settingsSaveQueue: Promise<unknown> = Promise.resolve();
  async function saveSettingsFields(keys: (keyof Settings)[], message: string) {
    const changes = Object.fromEntries(keys.map(key=>[key,settings[key]]));
    // Capture each edit now, but read persisted settings only after earlier saves.
    // Rapid edits must not overwrite one another with a stale full-settings copy.
    const save = settingsSaveQueue.then(() => run(async()=>{
      const fresh = await call<Settings>("get_settings");
      await call("save_settings",{settings:{...fresh,...changes}});
      if(keys.includes("musicDirectory")) persistedMusicDirectory=changes.musicDirectory as string;
      if(keys.includes("playlistDirectories")) void syncMirrors();
      if(keys.includes("playlistMirroringEnabled") && changes.playlistMirroringEnabled !== false) void syncMirrors();
      setNotice(message);
    }));
    settingsSaveQueue = save.catch(() => {});
    await save;
  }
  async function configureAcoustId() {
    settingsCategory = "connections";
    await tick();
    const connection = document.getElementById("acoustid-connection") as HTMLDetailsElement | null;
    if (connection) {
      connection.open = true;
      connection.scrollIntoView({block:"center"});
      connection.querySelector<HTMLElement>("summary")?.focus({preventScroll:true});
    }
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
      setNotice(`Preferred download source saved: ${label}.`);
    });
  }
  function initializeAppLifecycle(): () => void {
    if(desktop())void call<{setupComplete:boolean;musicDirectory?:string|null}>("get_release_setup").then(status=>{const musicDirectory=status?.musicDirectory===undefined?settings.musicDirectory:status.musicDirectory;if(status&&!status.setupComplete&&!musicDirectory)showReleaseSetup=true;}).catch(()=>{});
    void ensureLoudnessSettings();

    void call<typeof sidebarLayout>("get_sidebar_layout").then(layout=>{if(layout?.sections&&layout?.assignments)sidebarLayout=layout;}).catch(()=>{});
    void call<PlaylistPreferences>("get_playlist_preferences").then(value=>{playlistPreferences=value;return refreshFavorites();}).catch(()=>{});
    void call<boolean>("get_radio_history_setting").then(value=>includeRadioHistory=value).catch(()=>{});
    void call<RadioStation[]>("get_radio_stations").then(stations=>{radioStations=stations??[];}).catch(()=>{});
    const restored = loadPlaybackQueue(backupStartupReadStorage);
    if (restored) { playbackQueue.restore(restored.snapshot()); repeatMode = playbackQueue.repeat; refreshQueue(); playingTrack = playbackQueue.current; if (playingTrack) void cacheCover(playingTrack); }
    restorePlaybackPosition();
    try {
      const controls = consumeStartupPlaybackControls(backupStartupReadStorage);
      if (controls) { volume = controls.volume; shuffleEnabled = controls.shuffle; }
    } catch { /* Keep the last in-memory/default controls if storage is unavailable. */ }
    try {const left=Number(backupStartupReadStorage.getItem("burrata.layout.sidebar.v1"));if(left>=160&&left<=320)sidebarWidth=left;const right=Number(backupStartupReadStorage.getItem("burrata.layout.info.v1"));if(right>=280&&right<=420)infoPanelWidth=right;}catch{}
    void loadAutotagState().catch(()=>{});
    if(desktop())void call<typeof availableSources>("list_search_sources").then(sources=>availableSources=sources).catch(()=>{});
    const automationTimer = setInterval(() => { void queuedPlaybackOutbox.flush();  }, 1000);
    const syncTimer = setInterval(() => {
      if (desktop()) void runPeriodicPlaylistSyncFallback(
        () => call<boolean>("playlist_watcher_running"),
        syncMirrors,
      );
    }, 60000);
    if (desktop()) { void runPeriodicPlaylistSyncFallback(() => call<boolean>("playlist_watcher_running"), syncMirrors); if (view !== "library") void navigate("library", false); }
    let frontendListenersReady = false;
    const publishFrontendReady = () => {
      if (!disposed && frontendListenersReady && playbackControlsOwned) void call("set_automation_frontend_ready", {ready:true}).catch(()=>{});
    };
    const restorePlayback = async () => {
      playbackStartupError = null;
      try {
        await migrateNativePlayback(call, backupStartupReadStorage, {volume,shuffle:shuffleEnabled}, restored ? {queue:restored.snapshot(),position} : undefined);
        if (disposed) return;
        await nativeBridge.initialize(snapshot => {
          if (!disposed) applyNativeSnapshot(snapshot);
        }, event => { if (!disposed) applyNativePosition(event); });
        if (disposed) { nativeBridge.dispose(); return; }
        playbackControlsOwned = true;
        publishFrontendReady();
      } catch (failure) {
        if (!disposed) playbackStartupError = String(failure);
      }
    };
    const retryPlayback = () => { void restorePlayback(); };
    if (desktop()) { retryPlaybackStartup = retryPlayback; retryPlayback(); }
    let unlisten: (() => void) | undefined;
    let unlistenWarning: (() => void) | undefined;
    let disposed = false;
    const extraListeners: (() => void)[] = [];
    const register = <T,>(event: string, handler: (payload: T) => void): Promise<boolean> =>
      listen<T>(event, (e) => handler(e.payload)).then((fn) => {
        if (disposed) { fn(); return false; }
        extraListeners.push(fn);
        return true;
      }).catch((e) => { error = String(e); return false; });
    void register<StreamAnalysisUpdatedEvent>("stream-analysis-updated", event => { void applyStreamAnalysisUpdated(event); })
      .then(ready => { if (!disposed) cachedStreamAnalysisEventsReady = ready; });
    register<BackupCheckpointRequest>("backup-checkpoint-request", request => {
      if (!backupHydrationReady) return;
      void respondToBackupCheckpoint(
          request,
          captureBackupFrontendPreferences,
          captureBackupPlayback,
          response => call("submit_backup_checkpoint", { response }),
          () => pausePlaybackForRestoreCheckpoint(request),
        ).catch(failure => {
          error = `Could not submit the backup state checkpoint: ${String(failure)}`;
        });
    });
    void listen<unknown>("sort-availability-updated", () => handleSortAvailabilityUpdated()).then(fn => {
      if (disposed) fn();
      else {
        extraListeners.push(fn);
        if (localSortAvailabilityPending || localSortPending) handleSortAvailabilityUpdated();
      }
    }).catch(() => { /* Older desktop builds do not emit sort-availability-updated. */ });
    const browserTimer = setInterval(() => {
      if (desktop() && showImport) void refreshBrowser().catch((e) => (browserMessage = String(e)));
    }, 5000);
    if (desktop()) {
      const startupNavigationRequest = navigationRequest;
      void refreshRemotePlaylistBindings();
      register<{ liveBpmEnabled: boolean; downloadLayout: Settings["downloadLayout"] }>("acquisition-preferences-changed", preferences => {
        const next: Settings = { ...settings };
        if (typeof preferences?.liveBpmEnabled === "boolean") next.liveBpmEnabled = preferences.liveBpmEnabled;
        if (preferences?.downloadLayout === "flat" || preferences?.downloadLayout === "artistAlbum" || preferences?.downloadLayout === "playlist") {
          next.downloadLayout = preferences.downloadLayout;
        }
        settings = next;
      });
      register<{trackId:string;title:string;provider:string}>("playback-source-ready", result => {
        if (result.provider === "bandcamp") setNotice(`A verified Bandcamp source is ready for ${result.title}. You can play it again.`);
        else if (result.provider === "approximateAlbumExcerpt") setNotice(`An approximate album excerpt is ready for ${result.title}. You can play it again.`);
      });
      register<QueuedPlayRequest>("automation-queued-playback",request=>{void handleQueuedPlayback(request);});
      register<{operationId:string;requestId?:string}>("automation-queued-playback-cancelled",cancelQueuedPlayback);
      register<Parameters<typeof handleAutomationPlayback>[0]>("automation-playback", (request) => { void handleAutomationPlayback(request); });
      let preparingSearchRequest: string | null = null;
      const searchListenerReady = register<AutomationSearchRequest>("automation-search-ui", request => {
        preparingSearchRequest = request.requestId;
        automationSearchCancelled = null;
        automationSearch = null;
        catalogSearchRequest = 0;
        searchRelease = null;
        const expectedNavigation = navigationRequest + 1;
        void navigate("search").then(() => {
          if (disposed || preparingSearchRequest !== request.requestId || navigationRequest !== expectedNavigation || view !== "search") return;
          preparingSearchRequest = null;
          query = request.query;
          automationSearch = request;
        });
      });
      const cancelledSearchControlIds = new Set<string>();
      register<{requestId:string}>("automation-search-controls-cancelled", request => {
        cancelledSearchControlIds.add(request.requestId);
        if (cancelledSearchControlIds.size > 100) cancelledSearchControlIds.delete(cancelledSearchControlIds.values().next().value!);
      });
      register<{requestId:string;expiresAtMs:number;action:"inspect"|"set_continue"|"play";revision?:string;offset?:number;limit?:number;enabled?:boolean;occurrenceToken?:string}>("automation-search-controls", async request => {
        let result: unknown;
        try {
          if (cancelledSearchControlIds.has(request.requestId) || searchUiControlExpired(request.expiresAtMs,Date.now())) result = {status:"expired"};
          else if (disposed || view !== "search" || !searchComponent) result = {status:"unavailable"};
          else if (request.action === "inspect") result = searchComponent.inspectAutomationSearchUi(request.offset,request.limit,request.expiresAtMs);
          else if (request.action === "set_continue" && typeof request.revision === "string" && typeof request.enabled === "boolean") result = await searchComponent.setAutomationContinueSearch(request.revision,request.enabled,request.expiresAtMs);
          else if (request.action === "play" && typeof request.revision === "string" && typeof request.occurrenceToken === "string") result = searchComponent.playAutomationSearchResult(request.revision,request.occurrenceToken,request.expiresAtMs);
          else result = {status:"invalid_request"};
        } catch { result = {status:"failed"}; }
        void call("automation_search_controls_complete",{requestId:request.requestId,result}).catch(()=>{});
      });
      register<{requestId:string}>("automation-search-ui-cancelled", request => {
        if (preparingSearchRequest === request.requestId) preparingSearchRequest = null;
        if (automationSearch?.requestId === request.requestId) automationSearchCancelled = request.requestId;
      });
      const identifyListenerReady = register<{captureId:string;trackId:string;sessionId:string;generation:number;seekSerial:number;chapterId?:string|null}>("automation-identify-playing-audio", request => { void handleAutomationIdentify(request); });
      const identifyCancelListenerReady = register<{captureId:string}>("automation-identify-cancelled", request => cancelAutomationIdentify(request));
      const layoutListenerReady = register<{requestId:string}>("automation-inspect-playback-layout", request => {
        const layout = measurePlaybackLayout(document, {width:window.innerWidth,height:window.innerHeight});
        const waveform = playbackWaveform;
        const displayedWaveform = playbackWaveformForDisplay;
        const waveformDurationMatches = Boolean(waveform && Number.isFinite(playerDuration) && playerDuration > 0 &&
          Math.abs(waveform.durationSeconds - playerDuration) <= 0.25);
        const waveformGenerationMatches = Boolean(waveform && waveform.generation === waveformGeneration);
        const waveformTrackMatches = Boolean(waveform && waveform.trackId === waveformTrackId);
        const populatedWaveformBins = waveform?.bins.filter(Boolean).length ?? 0;
        const waveformRoute = "native";
        const waveformReasonCode = !waveformEnabled ? "disabled"
          : ((!waveform ? "no-waveform-snapshot"
          : !waveformGenerationMatches ? "generation-mismatch"
          : !waveformTrackMatches ? "track-mismatch"
          : !waveformDurationMatches ? "duration-mismatch"
          : displayedWaveform !== waveform ? "resource-binding-mismatch"
          : populatedWaveformBins === 0 ? "no-sampled-bins"
          : "waveform-ready"));

        const mediaAnalysisResource = currentStreamMediaResource();
        const activeMediaJob = streamMediaAnalysisJob?.resourceKey === mediaAnalysisResource?.key ? streamMediaAnalysisJob : null;
        const waveformDiagnostics = {
          enabled: waveformEnabled,
          backgroundAnalysisEnabled: streamMediaAnalysisEnabled,
          fullStreamAnalysisEligible: Boolean(desktop() && mediaAnalysisResource),
          fullStreamAnalysisState: streamMediaAnalysisStartingKey === mediaAnalysisResource?.key ? "starting"
            : activeMediaJob ? activeMediaJob.state
            : streamMediaAnalysisResultKey === mediaAnalysisResource?.key && streamMediaAnalysisResult ? "completed" : "idle",
          reasonCode: waveformReasonCode,
          snapshotPresent: Boolean(waveform),
          observedCache: {
            resourceAvailable: Boolean(playingTrackDetectionView?.resourceKey),
            resourceStatus: playingTrackDetectionView?.resourceKey ? "ready" : playingTrackDetectionDisabledReason ? "unavailable" : "waiting",
            bindingPrepared: Boolean(observedWaveformBinding),
            persistable: Boolean(observedWaveformBinding?.persistenceKey),
            trackMatches: Boolean(observedWaveformBinding && observedWaveformBinding.trackId === waveformTrackId),
            generationMatches: Boolean(observedWaveformBinding && observedWaveformBinding.generation === waveformGeneration),
            memoryEntries: observedWaveformCache.memoryEntryCount,
            persistentEntries: observedWaveformCache.persistedEntryCount,
          },
          source: waveform?.source ?? null,
          complete: waveform?.complete ?? false,
          generationMatches: waveformGenerationMatches,
          trackMatches: waveformTrackMatches,
          durationMatches: waveformDurationMatches,
          displayBindingMatches: Boolean(waveform && displayedWaveform === waveform),
          binCount: waveform?.bins.length ?? 0,
          populatedBins: populatedWaveformBins,
          coverageFraction: waveform?.bins.length ? populatedWaveformBins / waveform.bins.length : 0,
          route: waveformRoute,
          routeActive: Boolean(nativeSnapshot),
          analyserAttached: settings.liveBpmEnabled === true,
          mediaGraphUserVolume: null,
          mediaVolume: volume,
          mediaMuted: volume === 0,
        };
        const artworkUrl = playingTrack?.artworkUrl ?? "";
        void inspectArtworkImages(document, artworkUrl, Boolean(failedArtwork[artworkUrl])).then(artworkDiagnostics =>
          call("automation_layout_complete", {requestId:request.requestId,layout:{...layout,artworkDiagnostics,waveformDiagnostics,liveBpmDiagnostics:{engine:"native",generation:nativeSnapshot?.generation,seekSerial:nativeSnapshot?.seekSerial,status:liveBpmPollStatus}}})).catch(()=>{});
      });

      let activeUiNavigationRequest: string | null = null;
      const navigationListenerReady = register<{requestId:string;target:{kind:"view";view:"library"|"favorites"|"history"|"downloads"|"settings"|"radio";settingsCategory?:SettingsCategory;settingsControl?:string}|{kind:"catalog_artist";catalogArtist:ArtistRef;catalogArtistSection?:"overview"|"releases"|"tracks"}|{kind:"catalog_release";catalogArtist:ArtistRef;catalogRelease:ArtistRef}|{kind:"spotify_artist";spotifyArtistId:string}|{kind:"spotify_profile";spotifyProfileId:string}|{kind:"spotify_playlist";spotifyPlaylistId:string}|{kind:"local_playlist";collectionId:string}|{kind:"history";historyDirection:"back"|"forward"}}>("automation-ui-navigation", request => {
        activeUiNavigationRequest = request.requestId;
        const target = request.target;
        const destination = target.kind === "catalog_artist" ? (target.catalogArtist?.id ?? "") : target.kind === "catalog_release" ? (target.catalogRelease?.id ?? "") : target.kind === "view" ? target.view : target.kind === "spotify_artist" ? target.spotifyArtistId : target.kind === "spotify_profile" ? target.spotifyProfileId : target.kind === "spotify_playlist" ? target.spotifyPlaylistId : target.kind === "local_playlist" ? target.collectionId : target.historyDirection;
        const historyStack = target.kind === "history" ? (target.historyDirection === "back" ? backRoutes : forwardRoutes) : [];
        const historyRoute = historyStack[historyStack.length - 1];
        const targetView = target.kind === "view" ? target.view : target.kind === "local_playlist" ? "collections" : (target.kind === "spotify_artist" || target.kind === "catalog_artist") ? "artist" : target.kind === "catalog_release" ? "search" : target.kind === "history" ? historyRoute?.view : "source-browser";
        const hasSettingsControl = "settingsControl" in target;
        const before = navigationRequest;
        let expectedNavigation = before + 1;
        let loadedLocalPlaylistSnapshot:ReturnType<typeof localPlaylistSnapshot>|undefined;
        void (async () => {
          try {
            if (hasSettingsControl && (target.kind !== "view" || target.view !== "settings" || target.settingsCategory !== "playback" || target.settingsControl !== "playbackStartupTimeout")) throw new Error("invalid settings control");
            if (target.kind === "view") {
              if (target.settingsCategory !== undefined) {
                if (target.view !== "settings" || !["general","playback","library","downloads","connections","integrations","advanced"].includes(target.settingsCategory)) throw new Error("invalid settings category");
                settingsCategory = target.settingsCategory;
              }
              if (target.view === "radio" && playingRadioStation) selectedRadioStationId = playingRadioStation.id;
              await navigate(target.view);
            } else if (target.kind === "catalog_artist") {
              const artist = target.catalogArtist, section = target.catalogArtistSection ?? "overview";
              const validArtist = artist && Object.keys(artist).length === 2 && typeof artist.id === "string" && (
                artist.provider === "musicbrainz" ? /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(artist.id) :
                artist.provider === "discogs" ? /^[1-9][0-9]{0,19}$/.test(artist.id) && BigInt(artist.id) <= 18446744073709551615n :
                artist.provider === "monochrome" && /^[A-Za-z0-9_-]{1,80}$/.test(artist.id));
              if (!validArtist || !["overview","releases","tracks"].includes(section)) throw new Error("invalid catalog artist target");
              await discoverArtistRef(artist, section);
            } else if (target.kind === "catalog_release") {
              const {catalogArtist: artist,catalogRelease: releaseRef}=target;
              const valid=(identity:ArtistRef)=>identity && Object.keys(identity).length===2 && typeof identity.id==="string" && (identity.provider==="musicbrainz" ? /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(identity.id) : identity.provider==="discogs" ? /^[1-9][0-9]{0,19}$/.test(identity.id) : identity.provider==="monochrome" && /^[A-Za-z0-9_-]{1,80}$/.test(identity.id));
              if(!valid(artist)||!valid(releaseRef)||artist.provider!==releaseRef.provider)throw new Error("invalid or mismatched catalog release identity");
              let cursor:string|null|undefined=null, exact:ArtistReleaseRef|undefined;
              for(let pageIndex=0;pageIndex<100;pageIndex++){
                if(disposed||activeUiNavigationRequest!==request.requestId) return;
                const page=await getArtistReleases(artist,cursor);
                if(disposed||activeUiNavigationRequest!==request.requestId) return;
                exact=page.items.find(item=>item.release.provider===releaseRef.provider&&item.release.id===releaseRef.id);
                if(exact)break;
                if(!page.nextCursor)break;
                cursor=page.nextCursor;
              }
              if(!exact)throw new Error("Catalog release was not found in this artist’s releases");
              expectedNavigation=before+(view!=="search"?1:0);
              await openSearchAlbum(exact,artist);
              if(disposed||activeUiNavigationRequest!==request.requestId) return;
              if(searchRelease?.release.provider!==releaseRef.provider||searchRelease.release.id!==releaseRef.id||searchReleaseBusy||!searchReleaseTracks.length)throw new Error("Catalog release did not finish loading");
            } else if (target.kind === "spotify_artist") {
              if (!/^[A-Za-z0-9]{22}$/.test(target.spotifyArtistId)) throw new Error("invalid target");
              spotifyNavigationSnapshotKey=null;spotifyArtistId = target.spotifyArtistId; spotifyAlbumId = ""; spotifyArtistSearchFallback = null; activeArtistName = ""; artistScroll = 0;
              await navigate("artist");
            } else if (target.kind === "spotify_profile") {
              const profileUrl = normalizeSpotifyUserProfileUrl(`https://open.spotify.com/user/${encodeURIComponent(target.spotifyProfileId)}`);
              if (!profileUrl) throw new Error("invalid target");
              await openRemoteOwnerProfile("spotify", profileUrl);
            } else if (target.kind === "spotify_playlist") {
              if (!/^[A-Za-z0-9]{22}$/.test(target.spotifyPlaylistId)) throw new Error("invalid target");
              expectedNavigation = before + (view !== "source-browser" || provider !== "spotify" ? 1 : 0);
              await openRemotePlaylist("spotify", `https://open.spotify.com/playlist/${target.spotifyPlaylistId}`);
            } else if (target.kind === "local_playlist") {
              if(typeof target.collectionId!=="string"||!target.collectionId||target.collectionId.length>300||["favorites","history"].includes(target.collectionId))throw new Error("invalid local playlist target");
              if(!collections.some(item=>item.id===target.collectionId)||latestImports.some(item=>item.collectionId===target.collectionId)||remotePlaylistBindings.some(binding=>binding.collectionId===target.collectionId||binding.sourceCollectionId===target.collectionId))throw new Error("target is not a saved local playlist");
              await openPlaylist(target.collectionId);
              if(view!=="collections"||collectionId!==target.collectionId||activeBrowserImport)throw new Error("saved local playlist did not open");
              const items=await call<Track[]>("list_tracks",{view:"collections",collectionId:target.collectionId});
              loadedLocalPlaylistSnapshot=localPlaylistSnapshot(items);
            } else {
              if (!historyRoute || !["back","forward"].includes(target.historyDirection)) throw new Error("History destination unavailable");
              const beforeRestoration = routeRestorationEpoch;
              if (!await travelRoute(target.historyDirection)) throw new Error("History restoration did not finish");
              expectedNavigation = navigationRequest;
              if (routeRestorationEpoch !== beforeRestoration + 1 || collectionId !== historyRoute.collectionId || provider !== historyRoute.provider || (view === "source-browser" && remotePlaylistUrl !== (historyRoute.remotePlaylistUrl ?? ""))) throw new Error("History restoration was superseded");
            }
            await tick();
            if (disposed || activeUiNavigationRequest !== request.requestId || navigationRequest !== expectedNavigation || view !== targetView) return;
            if (target.kind === "view" && target.settingsCategory !== undefined && settingsCategory !== target.settingsCategory) return;
            if (target.kind === "catalog_artist" && (activeArtistRef?.provider !== target.catalogArtist.provider || activeArtistRef?.id !== target.catalogArtist.id || artistSection !== (target.catalogArtistSection ?? "overview") || spotifyArtistId || spotifyAlbumId)) return;
            if (target.kind === "catalog_release" && (searchRelease?.release.provider!==target.catalogRelease.provider||searchRelease.release.id!==target.catalogRelease.id||searchReleaseBusy||!searchReleaseTracks.length))return;
            if (target.kind === "local_playlist" && (view!=="collections"||collectionId!==target.collectionId||activeBrowserImport||!loadedLocalPlaylistSnapshot)) return;
            if (hasSettingsControl) {
              const control = document.getElementById("playback-startup-timeout") as HTMLSelectElement | null;
              if (!control || !control.isConnected || control.tagName !== "SELECT") throw new Error("Playback startup timeout control is unavailable");
              for (let parent = control.parentElement; parent; parent = parent.parentElement) {
                if (parent instanceof HTMLDetailsElement) parent.open = true;
              }
              await tick();
              control.scrollIntoView({block:"center",inline:"nearest"});
              control.focus({preventScroll:true});
              await tick();
              const rect = control.getBoundingClientRect();
              const bounds = [rect.x,rect.y,rect.top,rect.right,rect.bottom,rect.left,rect.width,rect.height,window.innerWidth,window.innerHeight];
              const visible = rect.width > 0 && rect.height > 0 && rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
              if (disposed || activeUiNavigationRequest !== request.requestId || !control.isConnected || document.activeElement !== control || !bounds.every(Number.isFinite) || !visible) throw new Error("Playback startup timeout control did not become visible and focused");
            }
            if (target.kind === "spotify_playlist" && (provider !== "spotify" || remotePlaylistUrl !== `https://open.spotify.com/playlist/${target.spotifyPlaylistId}`)) return;
            activeUiNavigationRequest = null;
            // Route completion is not proof that remote content has loaded.
            const albumSnapshot=target.kind==="catalog_release"?searchReleaseComponent?.captureLoadedAlbumSnapshot():undefined;
            if(target.kind==="catalog_release"&&(!albumSnapshot?.loaded||albumSnapshot.trackCount!==searchReleaseTracks.length))throw new Error("Loaded catalog release changed before completion");
            void call("automation_ui_navigation_complete", {requestId:request.requestId,destination,status:"complete",...(albumSnapshot?{releaseRevision:albumSnapshot.revision,trackCount:albumSnapshot.trackCount}:{}),...(loadedLocalPlaylistSnapshot?{collectionRevision:loadedLocalPlaylistSnapshot.revision,trackCount:loadedLocalPlaylistSnapshot.trackCount}:{})}).catch(()=>{});
          } catch {
            if (disposed || activeUiNavigationRequest !== request.requestId) return;
            activeUiNavigationRequest = null;
            void call("automation_ui_navigation_complete", {requestId:request.requestId,destination,status:"failed"}).catch(()=>{});
          }
        })();
      });
      register<{operationId:string;catalogArtist:ArtistRef;catalogRelease:ArtistRef;releaseRevision:string;sourceProviderId?:"monochrome"|"slskd"|"youtube"|"soundcloud"}>("automation-release-download", request=>{
        const identity=`${request.catalogRelease.provider}:${request.catalogRelease.id}`;
        void (async()=>{
          let receipt:AlbumBatchReceipt;
          try {
            const snapshot=searchReleaseComponent?.captureLoadedAlbumSnapshot();
            if(view!=="search"||searchReleaseBusy||searchRelease?.release.provider!==request.catalogRelease.provider||searchRelease.release.id!==request.catalogRelease.id||!snapshot?.loaded||snapshot.revision!==request.releaseRevision||snapshot.identity!==identity)throw new Error("Loaded release changed; reopen its exact catalog release before downloading");
            const selected=searchReleaseTracks;
            if(!selected.length)throw new Error("Loaded catalog release has no tracks to download");
            const sourceProviderOverride=["monochrome","slskd","youtube","soundcloud"].includes(request.sourceProviderId??"") ? request.sourceProviderId : undefined;
            receipt=await queueSelected(selected,progress=>call("automation_layout_complete",{requestId:`release-download:${request.operationId}`,layout:progress}).then(()=>{}),sourceProviderOverride) as AlbumBatchReceipt;
            if(!receipt||typeof receipt.status!=="string")throw new Error("Album download action did not return a batch receipt");
          } catch(failure) {
            receipt={status:"failed",requestedCount:searchReleaseTracks.length,completedCount:0,queuedJobs:[],skipped:[],failed:[{index:-1,trackId:"",reason:String(failure)}]};
          }
          await call("automation_layout_complete",{requestId:`release-download:${request.operationId}`,layout:receipt}).catch(()=>{});
        })();
      });
      register<{operationId:string;collectionId:string;collectionRevision:string}>("automation-local-playlist-download",request=>{
        void (async()=>{
          let receipt:AlbumBatchReceipt;
          try {
            if(!request.operationId||!request.collectionId||view!=="collections"||collectionId!==request.collectionId||activeBrowserImport)throw new Error("Loaded local playlist changed; reopen the exact saved playlist before downloading");
            const collection=collections.find(item=>item.id===request.collectionId);
            if(!collection||latestImports.some(item=>item.collectionId===request.collectionId)||remotePlaylistBindings.some(binding=>binding.collectionId===request.collectionId||binding.sourceCollectionId===request.collectionId))throw new Error("Playlist is not a saved local playlist");
            receipt=await downloadPlaylist(request.collectionId,false,request.collectionRevision,progress=>call("automation_layout_complete",{requestId:`local-playlist-download:${request.operationId}`,layout:progress}).then(()=>{})) as AlbumBatchReceipt;
            if(!receipt||typeof receipt.status!=="string")throw new Error("Playlist download action did not return a batch receipt");
          } catch(failure) {
            receipt={status:"failed",requestedCount:0,completedCount:0,queuedJobs:[],skipped:[],failed:[{index:-1,trackId:"",reason:String(failure)}]};
          }
          await call("automation_layout_complete",{requestId:`local-playlist-download:${request.operationId}`,layout:receipt}).catch(()=>{});
        })();
      });
      register<{requestId:string}>("automation-ui-navigation-cancelled", request => { if(activeUiNavigationRequest===request.requestId)activeUiNavigationRequest=null; });
      const queueUiListenerReady=register<{requestId:string;action:QueueUiAction;context:{generation:number;queueRevision:number;entryId:string;trackId:string;rowCount:number}}>("automation-queue-ui",request=>{
        if(disposed||!/^queue-ui:[0-9a-f-]{36}$/.test(request?.requestId??""))return;
        if(queueUiController){void call("automation_layout_complete",{requestId:request.requestId,layout:{protocolVersion:1,action:request.action.action,dispatched:false,syntheticEvents:true,trustedGesture:false,movePending:true,selectedIndices:[],focusedIndex:null,rowCount:Math.min(200,queueTracks.length),errorCode:"busy",acknowledgedMoves:[],mutationState:"unchanged"}}).catch(()=>{});return;}
        const controller=new AbortController();queueUiController=controller;queueUiRequestId=request.requestId;
        const moves=queueUiMoves(request.context.queueRevision,async(from,to,expectedQueueRevision)=>{
          if(!current())throw new Error("stale");
          const ack=await nativeBridge.command({action:"reorder",from,to,expectedQueueRevision});
          if(!ack)throw new Error("missing acknowledgement");return ack;
        },controller.signal);
        queueUiMove=moves.move;
        const current=()=>{const snapshot=nativeSnapshot as (NativePlaybackSnapshot&{entryId?:string})|null;const valid=!disposed&&!controller.signal.aborted&&showQueue&&true&&snapshot?.status==="paused"&&snapshot.generation===request.context.generation&&snapshot.entryId===request.context.entryId&&snapshot.trackId===request.context.trackId&&(snapshot.queueRevision??-1)>=request.context.queueRevision&&(snapshot.queueRevision??Infinity)<=moves.maximumObservedRevision&&queueTracks.length===request.context.rowCount;if(!valid)controller.abort();return valid;};
        void(async()=>{
          let result:QueueUiResult;
          try{
            const initiallyCurrent=current()&&nativeSnapshot?.queueRevision===request.context.queueRevision;
            await tick();
            result=await dispatchQueueUi(document,request.action,controller.signal,()=>initiallyCurrent&&current());
            result.acknowledgedMoves=[...moves.acknowledgedMoves];
            result.mutationState=result.errorCode&&moves.mutationState!=="unchanged"?"partial_or_unknown":moves.mutationState;
          }finally{if(queueUiController===controller){queueUiController=null;queueUiRequestId=null;queueUiMove=null;}}
          if(!disposed)await call("automation_layout_complete",{requestId:request.requestId,layout:result}).catch(()=>{});
        })().catch(()=>{});
      });
      const queueUiCancelListenerReady=register<{requestId:string}>("automation-queue-ui-cancelled",request=>{if(queueUiRequestId===request.requestId)queueUiController?.abort();});
      const playerPanelListenerReady = register<{requestId:string;panel:"queue"|"info"|"now-playing"|"now-playing-details"|"lyrics"|"closed"}>("automation-player-panel", request => {
        showFullscreenLyrics = request.panel === "lyrics" && Boolean(playingTrack);
        if (request.panel === "now-playing" || request.panel === "now-playing-details") { openPlayerPanel(null); showFullNowPlaying = Boolean(playingTrack); }
        else if (request.panel !== "lyrics") openPlayerPanel(request.panel === "closed" ? null : request.panel);
        void tick().then(async () => {
          if (request.panel === "lyrics") await tick();
          const expanded = request.panel === "now-playing" || request.panel === "now-playing-details";
          const view = document.querySelector(".now-playing-view");
          const cards = view?.querySelector(".cards");
          if (request.panel === "now-playing-details") cards?.scrollIntoView({block:"start"});
          const mounted = request.panel === "lyrics" ? Boolean(document.querySelector('[role="dialog"][aria-label="Fullscreen lyrics"]')) : !expanded || Boolean(view && (request.panel !== "now-playing-details" || cards));
          return call("automation_player_panel_complete", {requestId:request.requestId,panel:request.panel,status:mounted ? "complete" : "failed"});
        }).catch(()=>{});
      });
      void Promise.all([layoutListenerReady, queueUiListenerReady, queueUiCancelListenerReady, searchListenerReady, playerPanelListenerReady, navigationListenerReady, identifyListenerReady, identifyCancelListenerReady]).then(ready => { frontendListenersReady = ready.every(Boolean); publishFrontendReady(); });
      register<{stations:RadioStation[]}>("radio-stations-changed",({stations})=>{
        radioStations=stations;
        if(!stations.some(station=>station.id===selectedRadioStationId))selectedRadioStationId=stations[0]?.id??"";
      });
      register<unknown>("automation-library-changed", () => { void refreshRemotePlaylistBindings(); void call<typeof browserImports>("list_browser_imports").then(value => browserImports = value).catch(() => {}); void refreshFavorites().catch(()=>{}); void runPeriodicPlaylistSyncFallback(() => call<boolean>("playlist_watcher_running"), syncMirrors); requestLocalPrefixRefresh(undefined, true); });
      void call<{permissions: {control: boolean; manage: boolean}}>("get_automation_status").then((status) => { if (status?.permissions) automationPermissions = status.permissions; }).catch(() => {});

      register<{ assetId: string; inputHash?: string; version?: number; status: string; bpm?: number | null; key?: string | null; error?: string; payload?: Record<string, unknown> | null }>("analysis-progress", (status) => {
        analysisStatuses = [...analysisStatuses.filter((item) => item.assetId !== status.assetId), status];
      });
      register<Track>("library-updated", (updated) => { detailAssetRevision++; void handleLibraryUpdated(updated); });
      register<string>("asset-facts-updated",()=>{invalidateLegacyArtistAssets();detailAssetRevision++;void refreshCurrentAssetFiles();});
      register<{ok: boolean; status?: {warnings: string[]; pausedMirrors: string[]; sources: {collectionId?: string | null}[]}; error?: string; watcherWarnings?: string[]}>("playlist-sync", (event) => {
        if (settings.playlistMirroringEnabled === false) return;
        if (event.status) { mirrorStatus = [...event.status.warnings, ...(event.watcherWarnings ?? [])].join(" · "); pausedMirrors = event.status.pausedMirrors; localPlaylistIds = event.status.sources.map((source) => source.collectionId).filter((id): id is string => Boolean(id)); }
        else if (event.error) mirrorStatus = event.error;
        void call<Collection[]>("list_collections").then((result) => { collections = result; }).catch(() => {});
        if (view === "collections" || view === "library" || view === "favorites") requestLocalPrefixRefresh(undefined, true);
      });
      register("autotag-status", ()=>void (async()=>{
        await loadAutotagState();await refreshCurrentAssetFiles();
        if (autotagReviewId) autotagReviewAsset = await call<LocalAsset | null>("get_asset_by_id", { assetId: autotagReviewId });
        const ids=[...queueTracks.map(track=>track.id),...(playingTrack?[playingTrack.id]:[])];
        const byId=await getTracksInBatches(ids);
        if(playingTrack)playingTrack=byId.get(playingTrack.id) ?? playingTrack;
        queueTracks=queueTracks.map(track=>byId.get(track.id) ?? track);
        requestLocalPrefixRefresh(undefined,true);
      })().catch(()=>{}));
      register<CandidatePreviewStatus>("candidate-preview-status", (status) => {
        if (preview?.sessionId !== status.sessionId) return;
        paused = status.paused;  duration = status.duration;
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
      void call<{ settings: { slskd: { enabled: boolean }; lyricsTranslation?: { targetLanguage?: string; enabled?: boolean; provider?: "openaiCompatible" | "googleFree" | "auto" } }; lyricsTranslationCredential?: { present?: boolean } }>("get_connections")
        .then((status) => {slskdEnabled = status.settings.slskd.enabled;lyricsTranslationLanguage=status.settings.lyricsTranslation?.targetLanguage ?? "en";lyricsTranslationEnabled=status.settings.lyricsTranslation?.enabled===true;lyricsTranslationCredentialPresent=status.settings.lyricsTranslation?.provider === "googleFree" || status.settings.lyricsTranslation?.provider === "auto" || status.lyricsTranslationCredential?.present===true;sourceCapabilities={soulseek:slskdEnabled,discogs:Boolean((status.settings as {discogsEnabled?:boolean}).discogsEnabled)};})
        .catch(() => {});
      void call<Settings>("get_settings").then((value) => {
        persistedMusicDirectory = value.musicDirectory;

        // Keep a directory the user has typed while startup settings are still resolving.
        if (!settings.musicDirectory || settings.musicDirectory === persistedMusicDirectory) settings = {...value, matchingLevel:value.matchingLevel ?? "balanced", playbackStartupTimeoutSeconds:value.playbackStartupTimeoutSeconds ?? 20};
      }).catch((e) => (error = String(e)));
      void call<string[]>("get_diagnostics").then((diagnostics) => {
        if (diagnostics.length) setNotice(diagnostics.join(" · "));
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
        (e) => (setNotice(String(e.payload))),
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
      queueUiController?.abort();
      artworkDisplayActive = false; artworkDisplayCache.clear(); artworkDisplayUrls = {};
      if (desktop()) void call("set_automation_frontend_ready", {ready:false}).catch(()=>{});
      nativeBridge.dispose();
      if (retryPlaybackStartup === retryPlayback) retryPlaybackStartup = null;
      stopBatchMatching();
      unlisten?.();
      unlistenWarning?.();
      extraListeners.forEach((fn) => fn()); clearInterval(browserTimer); clearInterval(syncTimer); clearInterval(automationTimer);
      clearTimeout(autoTimer);
      nextPreviewRequest();
      };
  }
  onMount(() => {
    let canceled = false;
    let cleanup: (() => void) | undefined;
    let attempt = 0;
    const start = (hydration: Promise<boolean>) => {
      const currentAttempt = ++attempt;
      void startAfterBackupHydration(
        async () => (await hydration) && !canceled && currentAttempt === attempt,
        () => {
          if (canceled) return;
          lyricsTiming.hydrateFromStorage(backupStartupReadStorage);
          hydrateStoredFrontendFlags();
          loadStreamMediaAnalysisRestorePolicy();
          backupHydrationReady = true;
          retryBackupStartup = null;
          cleanup = initializeAppLifecycle();
        },
      ).then(ready => {
        if (!ready && !canceled && currentAttempt === attempt) backupHydrationReady = false;
      });
    };
    retryBackupStartup = () => {
      backupHydrationError = null;
      backupHydrationFailureKind = null;
      start(applyPendingBackupFrontendPreferences());
    };
    start(backupFrontendHydration);
    return () => { canceled = true; attempt++; retryBackupStartup = null; cleanup?.(); };
  });


  const artistPageProps = $derived({
    onTrackAlbum: (track: Track) => void openTrackAlbum(track), showSearchResultsButton: false,
    playingTrack, bufferingTrack: playbackLoading ? playingTrack : null, paused,
    activeTrack: activeArtistTrack, artistName: activeArtistName, initialArtistRef: activeArtistRef,
    initialSection: artistSection, initialScroll: artistScroll,
    onScroll: (position: number) => artistScroll = position,
    onSectionChange: (section: string) => { if (artistSection !== section) { rememberRoute(); if (!restoringRoute) artistScroll = 0; } artistSection = section as typeof artistSection; },
    onArtistSelected: (artist: ArtistRef | null, name: string) => { if (activeArtistRef?.id === artist?.id && activeArtistRef?.provider === artist?.provider) return; rememberRoute(); if (!restoringRoute) artistScroll = 0; activeArtistRef = artist; if (name) activeArtistName = name; },
    localTracks: [], onLoadLocalAssetsPage: getArtistLocalAssetsPage,
    searchArtists, getArtistProfile, getArtistReleases, getReleaseTracks, getArtistTracks,
    getArtistCollaborations, resolveRecordingMetadata, onApplyMetadata: applyArtistMetadata,
    onPlayTrack: playArtistTrack, onAddAlbumToPlaylist: addAlbumToPlaylist, onSaveAlbumToLibrary: saveReleaseToLibrary, onSaveLoadedTracks: saveTracksToLibrary,
    onBrowseArtistProvider: browseArtistProvider, onDownloadAlbum: (items: Track[]) => queueSelected(items),
    onSearchAlbumSoulseek: (release: ArtistReleaseRef) => openSoulseekAlbumSearch(release), soulseekSearchAvailable: soulseekAlbumSearchAvailable,
    onQueueTracks: queueArtistTracks,
    onTrackContext: (track: Track, event: MouseEvent) => { actionAnchor = { x: event.clientX, y: event.clientY }; actionTrack = track; void matchArtistPlaybackSources([track]).then(([matched]) => { if (actionTrack === track && matched) actionTrack = matched; }).catch(failure => { error = String(failure); }); },
    onDownloadTrack: (track: Track) => openDownloadChooser(track), onFavoriteTrack: (track: Track) => void favorite(track),
    onAddTrackToPlaylist: addArtistTrack, onAddSelectedToPlaylist: addTracksToPlaylist, onArtistNavigate: discoverArtistRef,
  });

  const searchPanelProps = $derived({
    startupReadStorage: backupStartupReadStorage,
    searchArtists: (value: string) => searchArtists(value, 1).then(page => page.results),
    loadArtistProfile: loadArtistProfileArtwork,
    onTrackAlbum: (track: Track) => void openTrackAlbum(track),
    navigationSnapshotKey: searchNavigationSnapshotKey,
    automationSearch, automationSearchCancelled,
    onAutomationSearchComplete: completeAutomationSearch,
    playingTrack, bufferingTrack: playbackLoading ? playingTrack : null,
    playingTrackId: playingTrack?.id ?? "", bufferingTrackId: playbackLoading ? playingTrack?.id ?? "" : "",
    paused, onRemoteResult: (row: SearchRow) => void openRemoteSearchResult(row),
    onAddRemotePlaylist: addRemoteSearchPlaylist,
    onPlaylistTracks: addTracksToPlaylist, onQueueTracks: queueArtistTracks,
    onFavorite: (track: Track) => void favorite(track), availableSources, query,
    searchPreset: catalogSearchRequest ? { sourceId: catalogSearchSource, kind: catalogSearchKind, requestId: catalogSearchRequest } : undefined,
    onQueryChange: (value: string) => query = value, sourceCapabilities,
    onArtist: (artist: ArtistRef) => void discoverArtistRef(artist),
    onArtistTrack: (track: Track) => void discoverArtist(track),
    onAlbum: (release: ArtistReleaseRef) => void openSearchAlbum(release),
    onPlay: (track: Track, list?: Track[], index?: number) => void startPreview(track, false, false, list ?? [track], index ?? 0),
    onDownload: (track: Track) => void openDownloadChooser(track),
    onDownloadTracks: (items: Track[]) => void queueSelected(items, undefined, "slskd"),
    onNext: (track: Track) => queueArtistTracks([track], "next"),
    onQueue: (track: Track) => queueArtistTracks([track], "append"),
    onContext: (track: Track, event?: MouseEvent) => { actionTrack = track; actionAnchor = event ? { x: event.clientX, y: event.clientY } : null; },
    onPlayChapter: playChapter, onQueueChapter: queueChapter,
    onDownloadChapter: (parent: Track, candidate: Candidate, chapter: Chapter) => void downloadChapter(parent, candidate, chapter),
    onInfoChapter: (parent: Track, candidate: Candidate, chapter: Chapter) => { const virtual = virtualChapterTrack(parent, candidate, chapter); if (virtual) searchMoreInfo(virtual, candidate, chapter.title); },
  });
  const radioViewProps = $derived({
    onAddSelectedToPlaylist: addTracksToPlaylist, onTrackAlbum: (track: Track) => void openTrackAlbum(track),
    currentStationMetadata: (playingTrack?.provenance as { radioMetadata?: RadioTrackMetadata } | undefined)?.radioMetadata ?? null,
    onIdentifyPlayingStation: () => identifyPlayingRadio(), identifyingPlayingStation: radioRecognizing,
    playingTrack, bufferingTrack: playbackLoading ? playingTrack : null,
    onInfoTrack: (track: Track) => openPlayerPanel("info", track),
    onOpenHistory: (stationId: string) => { historyQuery = ""; historyProvider = "radio"; historyStationId = stationId; historyFrom = ""; historyTo = ""; historyOrder = "newest"; void navigate("history"); },
    favoriteTrackIds, onPlayTrack: (track: Track, list: Track[], index: number) => void startPreview(track, false, false, list, index),
    onDownloadTrack: (track: Track) => void openDownloadChooser(track), onFavoriteTrack: (track: Track) => void favorite(track),
    onQueueTracks: queueArtistTracks, onAddTrackToPlaylist: addArtistTrack,
    savedStations: radioStations, stationNames: Object.fromEntries(radioStations.map(station => [station.id, station.name])),
    onAddClosed: () => showAddRadio = false, selectedStationId: selectedRadioStationId, showAdd: showAddRadio,
    onStationsChanged: (stations: RadioStation[]) => { const added = stations.find(station => !radioStations.some(existing => existing.id === station.id)); radioStations = stations; if (added) selectedRadioStationId = added.id; else if (!stations.some(station => station.id === selectedRadioStationId)) selectedRadioStationId = stations[0]?.id ?? ""; },
    onPlayStation: playRadioStation, onArtist: (track: Track) => void discoverArtist(track),
    onTrackContext: (track: Track, event: MouseEvent) => { actionTrack = track; actionAnchor = { x: event.clientX, y: event.clientY }; void matchArtistPlaybackSources([track]).then(([matched]) => { if (actionTrack === track && matched) actionTrack = matched; }).catch(failure => { error = String(failure); }); },
    playingStationId: !paused ? playingRadioStation?.id ?? "" : "",
  });
  const remotePlaylistDetailProps = $derived({
    playingTrack, bufferingTrack: playbackLoading ? playingTrack : null, provider, playlistUrl: remotePlaylistUrl, call,
    onBrowseOwner: openRemoteOwnerProfile, onAddSelectedToPlaylist: addTracksToPlaylist,
    onBack: () => { if (backRoutes.length) void travelRoute("back"); else remotePlaylistUrl = ""; },
    onImported: remotePlaylistsImported,
    playingTrackId: playingTrack?.id ?? "", bufferingTrackId: playbackLoading ? playingTrack?.id ?? "" : "", paused,
    onPlay: async (track: Track, list: Track[], index: number) => { await startPreview(track, false, false, list, index); },
    onQueue: (track: Track) => queueArtistTracks([track], "append"), onNext: (track: Track) => queueArtistTracks([track], "next"), onQueueTracks: queueArtistTracks,
    onDownload: openDownloadChooser, onFavorite: favorite, onPlaylist: addArtistTrack, onArtist: discoverArtist,
    onAlbum: (track: Track) => void openTrackAlbum(track), onInfo: (track: Track) => void openPlayerPanel("info", track),
    onContext: (track: Track, event: MouseEvent) => { actionTrack = track; actionAnchor = { x: event.clientX, y: event.clientY }; },
  });
  const remotePlaylistsProps = $derived({
    call, initialProvider: provider, initialOwner: remoteOwner, profileOnly: true,
    onOpenPlaylist: (source: "spotify" | "youtube", url: string) => void openRemotePlaylist(source, url),
    onBrowseOwner: openRemoteOwnerProfile, onImported: remotePlaylistsImported,
  });
</script>

<svelte:window onkeydown={keyboard} onpointerdown={dismissHistoryFiltersOutside} ondragover={preventFileDropNavigation} ondrop={preventFileDropNavigation} bind:innerWidth={screenWidth} />
{#if desktop() && !backupHydrationReady}
  <div class="backup-hydration-gate" class:restore-error={Boolean(backupHydrationError)} role={backupHydrationError ? "alert" : "status"} aria-live="polite">
    {#if backupHydrationError}
      {#if backupHydrationFailureKind === "preferences"}
        <p>Imported interface preferences could not be saved. They remain queued; check available app storage and retry.</p>
      {:else}<p>Could not finish restoring saved playback. Retry to continue.</p>{/if}
      {#if /quota/i.test(backupHydrationError ?? "")}<small>Free some app storage space, then retry.</small>{/if}
      <button type="button" onclick={() => retryBackupStartup?.()}>Retry</button>
    {:else}<p>Starting Burrata…</p>{/if}
  </div>
{/if}
{#if desktop() && backupHydrationReady && !playbackControlsOwned}
  <div class="backup-hydration-gate" class:restore-error={Boolean(playbackStartupError)} role={playbackStartupError ? "alert" : "status"} aria-live="polite">
    {#if playbackStartupError}
      <p>Could not finish restoring saved playback. Your saved session is retained.</p>
      <button type="button" onclick={() => retryPlaybackStartup?.()}>Retry playback restore</button>
    {:else}<p>Restoring saved playback…</p>{/if}
  </div>
{/if}
<div class:light={settings.theme === "light" || settings.theme === "system" && systemLight} class:has-player-panel={showPlayingInfo || showQueue} class="app-shell" inert={desktop() && (!backupHydrationReady || !playbackControlsOwned)} aria-busy={desktop() && (!backupHydrationReady || !playbackControlsOwned)} style={`--player-height: ${playerHeight}px; --library-sidebar-width:${screenWidth<=900?64:Math.min(sidebarWidth,Math.max(160,screenWidth-((showPlayingInfo||showQueue)?280:0)-320))}px; --info-panel-width:${Math.min(infoPanelWidth,Math.max(280,screenWidth-(screenWidth<=900?64:sidebarWidth)-280))}px`}>
  <LibrarySidebar inert={showFullNowPlaying} {call} showAccountPlaylists={sidebarLayout.showAccountPlaylists} onRemotePlaylist={(source,url) => void openRemotePlaylist(source,url)} compact={screenWidth<=900} favoritesPlaylistId={playlistPreferences.favoritesPlaylistId} playlistNames={sidebarLayout.names??{}} onRadioContext={(station,event)=>radioActions={station,anchor:{x:event.clientX,y:event.clientY}}} {radioStations} activeRadioStationId={selectedRadioStationId} {playingRadioStationId} onRadioStation={(station)=>{showAddRadio=false;selectedRadioStationId=station.id;void navigate("radio");if(playingRadioStation?.id!==station.id)playRadioStation(station);}} onAddRadio={()=>{showAddRadio=true;void navigate("radio");}} width={sidebarWidth} onWidthChange={(width)=>saveLayoutWidth("sidebar",width)} {view} collections={namedCollections} browserImports={sidebarImports} {localPlaylistIds} activeSource="all" activeCollectionId={view === "collections" || view === "charts" ? collectionId : ""}
    onDirectoryBrowser={() => { directoryBrowserVisited = true; if (view !== "directory-browser") void navigate("directory-browser"); }}
    selectedFolder={view === "directory-browser" ? directoryLocation : null} rootsRevision={directoryRootsRevision} onSelectFolder={browseFolder} onRootsChanged={() => directoryRootsRevision++}
    onNavigate={(next) => { if (next === "library") { if (view === "library" && (libraryBrowser.tab !== "tracks" || libraryBrowser.entity)) navigateLibraryBrowser(initialLibraryBrowserState()); if (view !== "library" || librarySource !== "all") void chooseLibrarySource("all"); } else if (view !== next) void navigate(next as View); }}
    onPlaylist={(id) => void openPlaylist(id)} onPlaylistContext={(id, event) => { const collection = namedCollections.find(item => item.id === id)??(["favorites","history"].includes(id)?{id,name:sidebarLayout.names?.[id]??(id==="favorites"?"Liked Songs":"History"),trackCount:0}:null); if (collection) playlistActions = { collection, anchor: { x: event.clientX, y: event.clientY } }; }} onSource={(source) => source === "spotify" || source === "youtube" ? void searchProviderPlaylists(source) : void browseProviderCharts(source)}
    sections={sidebarLayout.sections} assignments={sidebarLayout.assignments} onCreateSection={createSection} onRenameSection={renameSection} onDeleteSection={deleteSection} onMovePlaylist={movePlaylistToSection} onCreatePlaylist={(sectionId)=>void createNewPlaylist(sectionId)}
    onImport={() => void createNewPlaylist()} onSettings={() => void navigate("settings")} onDownloads={() => { openPlayerPanel("downloads"); void refreshJobs(); }}
    activeJobs={jobs.filter((job) => !["completed", "failed", "cancelled", "waiting_for_source", "interrupted"].includes(job.state)).length} />
  <main inert={showFullNowPlaying}>
    <header>
      <div class="navigation-controls"><button aria-label="Go back" disabled={restoringRoute || !backRoutes.length} onclick={() => void travelRoute("back")}>←</button><button aria-label="Go forward" disabled={restoringRoute || !forwardRoutes.length} onclick={() => void travelRoute("forward")}>→</button></div>
      <div>
        <div class="eyebrow">DISCOVER · AUDITION · COLLECT</div>
        <h1>
          {view === "directory-browser" ? "Folders" : view === "source-browser" ? (provider === "spotify" ? "Spotify" : provider === "youtube" ? "YouTube" : provider === "lastfm" ? "Last.fm" : provider === "shazam" ? "Shazam" : "Bandcamp") : view === "charts"
            ? (activeBrowserImport ? (sidebarLayout.names?.[activeBrowserImport.collectionId]??playlistTitle(activeBrowserImport.title,activeBrowserImport.provider)) : "Top 100")
            : view === "collections"
              ? (sidebarLayout.names?.[collectionId]??namedCollections.find(item=>item.id===collectionId)?.name ?? "Playlists")
              : view === "favorites" ? (sidebarLayout.names?.favorites??"Liked Songs") : view === "history" ? (sidebarLayout.names?.history??"Listening history") : view[0].toUpperCase() + view.slice(1)}
        </h1>
        {#if currentPlaylistCreator?.ownerName || currentPlaylistCreator?.ownerId}
          <p class="playlist-creator">By {#if currentPlaylistCreator.ownerUrl}<button class="creator-profile-link" title="Browse Spotify creator playlists" onclick={() => void openCurrentPlaylistCreator()}>{currentPlaylistCreator.ownerName || currentPlaylistCreator.ownerId} ↗</button>{:else}{currentPlaylistCreator.ownerName || currentPlaylistCreator.ownerId}{/if}</p>
        {/if}
      </div>
      <div class="header-actions">
        {#if view === "library"}
          <details class="library-tools header-library-tools"><summary aria-label="Library options" title="Library options">Library options</summary><div><button onclick={openPlaylistImport}>Import charts or playlists</button>
            {#if settings.playlistMirroringEnabled === false}
              <button onclick={() => { settingsCategory = "library"; void navigate("settings"); }}>M3U mirroring is off · Settings</button>
            {:else}
              <button onclick={() => void syncMirrors()}>Sync M3U files</button>
            {/if}
            <button onclick={() => void run(async () => { deletedCollections = await call<Collection[]>("list_deleted_collections"); })}>Deleted playlists</button>
            {#if assetTotal > 0}<button disabled={analysisBusy || (!checked.length && !selected && !playingTrack)} title="Analyze selected local recordings, review the results, then choose which fields to apply" onclick={() => void analyzeLibraryAssets()}>{analysisBusy ? "Analyzing…" : "Analyze selected BPM / key"}</button>{/if}
            {#if assetTotal > 1}<button disabled={loudnessAnalysisBusy || (!checked.length && !selected && !playingTrack)} title="Review selected downloaded tracks from one album, then analyze their exact gapless programme order" onclick={() => void openSelectedLocalAlbumLoudness()}>Analyze selected album loudness</button>{/if}
            {#if (checked.length > 0 || selected || playingTrack)}
              <button aria-label="Analyze selected streaming sources" title="Resolve verified streaming sources for selected saved recordings, analyze full tracks, then review BPM and key" disabled={analysisBusy} onclick={() => void analyzeSelectedStreamTracks()}>
                {analysisBusy && selectedAnalysisKind === "stream" ? "Analyzing streams…" : "Analyze selected streams"}
              </button>
              <button aria-label="Analyze selected loudness" title="Analyze only selected downloaded tracks; analysis does not rewrite audio or recording metadata" disabled={loudnessAnalysisBusy} onclick={() => void analyzeLoudnessTracks(currentLoudnessSelection())}>
                {loudnessAnalysisBusy ? "Analyzing loudness…" : "Analyze selected loudness"}
              </button>
            {/if}
          </div></details>
        {/if}
        {#if busy}<span class="busy">Working…</span>{/if}{#if view !== "directory-browser" && view !== "artist" && view !== "settings" && !searchRelease}<button
          onclick={() => void createNewPlaylist()}>＋ New playlist</button
        >{/if}
      </div>
    </header>
    {#if backupHydrationReady}<SoulseekWishlist startupStorage={backupStartupPersistence} visible={showSavedSoulseekSearches} onClose={()=>showSavedSoulseekSearches=false} onReview={async(value)=>{showSavedSoulseekSearches=false;await navigate("search");query=value;catalogSearchSource="soulseek";catalogSearchKind="tracks";catalogSearchRequest++;}} />{/if}
    {#if !desktop()}<div class="banner">
        Desktop connection unavailable. Run the Tauri app to browse real charts
        and access local files.
      </div>{/if}
    {#if directoryBrowserVisited}<div hidden={view !== "directory-browser"} aria-hidden={view !== "directory-browser"}>
      <LazyView loader={loadDirectoryBrowser} label="directory browser" onInstance={(instance) => directoryBrowser = instance as typeof directoryBrowser} componentProps={{ onTrackAlbum: (track: Track) => void openTrackAlbum(track), call, location: directoryLocation, rootRegistryRevision: directoryRootsRevision, onLocationChange: (location: DirectoryBrowserLocation | null, context: { reason?: string; previousSnapshot: DirectoryBrowserSnapshot } | null) => { if (context?.reason === "user") rememberRoute({directorySnapshot:context.previousSnapshot,directoryLocation:context.previousSnapshot.location}); directoryLocation = location; }, onRootsChanged: () => directoryRootsRevision++, onPlayFile: (_file: unknown, track: Track) => void startPreview(track, false, false, [track], 0), playingTrack, bufferingTrack: playbackLoading ? playingTrack : null, paused, onDownload: openDownloadChooser, onFavorite: favorite, onQueue: (track: Track) => queueArtistTracks([track], "append"), onNext: (track: Track) => queueArtistTracks([track], "next"), onPlaylist: addArtistTrack, onArtist: discoverArtist, onInfo: (track: Track) => openPlayerPanel("info", track), onContext: (track: Track, event: MouseEvent) => { actionTrack = track; actionAnchor = { x: event.clientX, y: event.clientY }; }, onAddSelectedToPlaylist: addTracksToPlaylist }} />
    </div>{/if}
    {#if error || playbackQueueSaveError}<div class="banner error" role="alert" aria-live="assertive">
        {[error, playbackQueueSaveError].filter(Boolean).join(" ")}<button aria-label="Dismiss error" onclick={() => { error = ""; playbackQueueSaveError = ""; }}
          >×</button
        >
      </div>{/if}
    {#if notice}<div class="banner notice" role="status">
        {notice}<button
          aria-label="Dismiss notice"
          onclick={() => setNotice("")}>×</button
        >
      </div>{/if}
    {#if deletedPlaylist}<div class="banner notice" role="status"><span>Deleted playlist: {deletedPlaylist.name}. Audio files are kept.</span><button onclick={() => void undoDeletePlaylist()}>Undo deletion</button><button aria-label="Dismiss deleted playlist notice" onclick={() => (deletedPlaylist = null)}>×</button></div>{/if}
    {#if deletedCollections}<div class="modal-backdrop"><dialog open aria-label="Deleted playlists" class="playlist-trash"><h2>Deleted playlists</h2><p>Restore a playlist with its tracks. Audio files are kept when playlists are deleted.</p>{#each deletedCollections as item (item.id)}<div class="trash-row"><span>{item.name}<small>{item.trackCount} tracks</small></span><button aria-label={`Restore ${item.name}`} onclick={() => void undoDeletePlaylist(item)}>Restore</button></div>{:else}<p>No deleted playlists.</p>{/each}<button onclick={() => (deletedCollections = null)}>Close</button></dialog></div>{/if}
    {#if showImport}
      <dialog class="download-dialog import-dialog" use:activateDownloadDialog={closeImportDialog} aria-label="Import" tabindex="-1" onclick={(event) => {
        if (event.target !== event.currentTarget) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) closeImportDialog();
      }}>
        <button type="button" class="import-dialog-close" aria-label="Close import" title="Close import" onclick={closeImportDialog}>×</button>
      {#if importChoice === null}
        <section class="import-panel"><LazyView loader={loadImportHub} label="import choices" componentProps={{ onChoose: (choice: ImportChoice) => { importChoice = choice; if (choice === "playlist-file") importKind = "m3u"; else if (choice === "paste-playlist") importKind = "tracklist"; } }} /></section>
      {:else if importChoice === "restore-backup"}
        <section class="import-panel"><button type="button" onclick={returnToImportChoices}>← Import choices</button>{#if desktop()}{#key droppedBackupPreview?.previewId ?? "backup-picker"}<LazyView loader={loadBackupRestore} label="backup restore" componentProps={{ inspectBackup: inspectBackupForUi, createBackup: createBackupForUi, restoreBackup: restoreBackupForUi, chooseMediaRoot: chooseBackupMediaRoot, estimateBackup: estimateBackupForUi, cancelJob: cancelBackupJob, loadActiveBackupJob, subscribeBackupProgress, loadSchedule: loadBackupSchedule, saveSchedule: saveBackupSchedule, chooseScheduleDestination: chooseBackupScheduleDestination, exportSettings: exportBackupSettings, inspectSettings: inspectBackupSettings, applySettings: applyBackupSettings, previewRestore: previewBackupRestore, allowReplace: true, initialPreview: droppedBackupPreview }} />{/key}{:else}<p>Backups require the Burrata desktop app.</p>{/if}</section>
      {:else if importChoice !== "import-url"}
        <section class="import-panel"><button type="button" onclick={returnToImportChoices}>← Import choices</button><LazyView loader={loadImportSources} label="import sources" componentProps={{ choice: importChoice, call, collections, currentCollectionId: importTargetCollectionId, desktop: desktop(), onApply: applySourceImport, onPortableImported: async (created: Collection) => { collections = await call<Collection[]>("list_collections"); await openPlaylist(created.id); importChoice = null; }, onBackupSelected: () => { if (droppedBackupPreview) return; importChoice = "restore-backup"; setNotice("Select the backup in Restore Backup to review its contents."); }, onBackupPreview: openDroppedBackupPreview, onClose: returnToImportChoices }} /></section>
      {:else}
      <section class="import-panel">
        <button type="button" onclick={() => importChoice = null}>← Import choices</button>
        <h3>{importTargetCollectionId ? "Import tracks into this playlist" : "Import from a URL"}</h3>
        <p class="browser-help">Public tracklists are imported directly when available. Otherwise Burrata sends the source to your connected companion browser; use the companion to import the latest version. Your browser handles sign-in or verification.</p>
        <div class="toolbar">
          <input aria-label="Chart, playlist or album URL" bind:value={browserUrl} placeholder="Chart, Spotify playlist, Bandcamp album, Shazam or Last.fm URL" />
          <button disabled={!browserUrl.trim() || Boolean(browserRequest)} onclick={() => void requestBrowserImport()}>Import URL</button>
          <button onclick={() => void run(refreshBrowser)}>Refresh connection</button>
        </div>
        <p role="status">{browserStatus?.connected ? "Browser extension connected" : "Companion not seen yet · open its popup to connect"}{browserStatus?.pendingCount ? ` · ${browserStatus.pendingCount} pending` : ""}{browserMessage ? ` · ${browserMessage}` : ""}</p>
        {#if collectionImportPreview}<section class="browser-capture" aria-label="Import preview">
          <h4>{collectionImportPreview.title}</h4>
          <p>{collectionImportPreview.capturedCount} entries · {collectionImportPreview.uniqueRecordings} unique recordings · {collectionImportPreview.complete ? "complete" : "partial capture"}</p>
          <p>Changes from the previous capture: {collectionImportPreview.changes.added} added · {collectionImportPreview.changes.removed} removed · {collectionImportPreview.changes.moved} moved{collectionImportPreview.changes.retainedFromEarlierCapture ? ` · ${collectionImportPreview.changes.retainedFromEarlierCapture} earlier recordings retained` : ""}</p>
          {#each collectionImportPreview.warnings as warning}<p>{warning}</p>{/each}
          {#if !importTargetCollectionId}<fieldset class="url-import-destination"><legend>Destination and duplicate handling</legend>
            <label>Existing playlist<select bind:value={urlImportDestinationId}><option value="">Keep as an imported source</option>{#each collections.filter(item => !["favorites", "history", "downloads"].includes(item.id)) as collection}<option value={collection.id}>{collection.name}</option>{/each}</select></label>
            <label>Or create a playlist<input bind:value={urlImportNewPlaylistName} placeholder={collectionImportPreview.title} /></label>
            <label>Folder path<input bind:value={urlImportFolderPath} placeholder="Optional, e.g. Live sets/2026" /></label>
            <label>Section name<input bind:value={urlImportSectionName} placeholder="Optional section for these tracks" /></label>
            <label>Duplicates<select bind:value={urlImportDuplicatePolicy}><option value="keep">Keep each occurrence</option><option value="skip">Skip destination duplicates</option></select></label>
            <p>Reviewing for {urlImportDestinationId ? collections.find(item => item.id === urlImportDestinationId)?.name ?? "selected playlist" : urlImportNewPlaylistName.trim() || "an imported source"} · {urlImportDuplicatePolicy === "keep" ? "keep occurrences" : "skip duplicates"}{urlImportFolderPath.trim() ? ` · folder ${urlImportFolderPath}` : ""}{urlImportSectionName.trim() ? ` · section ${urlImportSectionName}` : ""}</p>
          </fieldset>{:else}<p>Destination: {collections.find(item => item.id === importTargetCollectionId)?.name ?? "current playlist"} · repeated tracks are appended.</p>{/if}
          <details><summary>Track preview</summary>{#each collectionImportPreview.sample as item}<p>{item.artist} — {item.title}{item.version ? ` (${item.version})` : ""}</p>{/each}</details>
          <button onclick={()=>void applyCollectionImportPreview()}>Apply import</button><button onclick={()=>collectionImportPreview=null}>Cancel</button>
        </section>{/if}
        {#if browserRequest}<button onclick={() => { browserRequest = ""; browserMessage = "The browser request may still finish. You can submit another URL."; }}>Allow another request</button>{/if}
        {#if browserImports.length}<details open><summary>Saved browser imports</summary><div class="browser-imports">
          {#each browserImports as imported}<button onclick={() => void run(() => openBrowserImport(imported))}>
            {imported.provider} · {imported.title} · {imported.importedCount}{imported.expectedTotal != null ? ` / ${imported.expectedTotal}` : ""} recordings · {imported.complete ? "complete" : "partial"}
          </button>{/each}
        </div></details>{/if}
        {#if activeBrowserImport && !importTargetCollectionId}<div class="browser-capture">
          <strong>{activeBrowserImport.title}</strong> · {activeBrowserImport.kind} · {activeBrowserImport.capturedCount} entries captured, {activeBrowserImport.importedCount} unique recordings{activeBrowserImport.expectedTotal != null ? ` of ${activeBrowserImport.expectedTotal} expected entries` : " · total unknown"} · {activeBrowserImport.complete ? "complete" : "partial capture"}
          {#if activeBrowserImport.changes}<p>Changes from the previous capture: {activeBrowserImport.changes.added} added · {activeBrowserImport.changes.removed} removed · {activeBrowserImport.changes.moved} moved</p>{/if}
          {#if activeBrowserImport.failedSources?.length}<button onclick={()=>void retryFailedImport()}>Retry {activeBrowserImport.failedSources.length} failed album pages</button>{/if}
          {#each activeBrowserImport.warnings as warning}<p>{warning}</p>{/each}
        </div>{/if}

        <button type="button" onclick={closeImportDialog}>Close</button>
      </section>
      {/if}
      </dialog>
    {/if}
    {#if view === "library"}<nav class="library-view-tabs" aria-label="Library views">{#each [["tracks", "Tracks"], ["artists", "Artists"], ["albums", "Albums"]] as [tab, label]}<button aria-pressed={libraryBrowser.tab === tab} onclick={() => navigateLibraryBrowser({ ...initialLibraryBrowserState(), tab: tab as LibraryBrowserState["tab"] })}>{label}</button>{/each}</nav>{/if}
    {#if autotagReviewId && currentTagJob && currentTagAsset}<section class="autotag-page"><button onclick={()=>{autotagReviewId=null;autotagReviewAsset=null;}}>← Back</button><AutotagReview job={currentTagJob} asset={currentTagAsset} onReview={(assetId,providerId)=>call<AutotagJob>("review_autotag_candidate",{assetId,providerId})} onApply={async(assetId,fieldKeys)=>{await call("apply_autotag_review",{assetId,fieldKeys});autotagReviewAsset=await call<LocalAsset|null>("get_asset_by_id",{assetId});await loadAutotagState();}} onApplyRecognition={async(assetId,fieldKeys)=>{await call("apply_recognition_review",{assetId,fieldKeys});autotagReviewAsset=await call<LocalAsset|null>("get_asset_by_id",{assetId});await loadAutotagState();}} onRepairSource={desktop()?recoverDownloadSourceTags:undefined} onRetry={async(assetId)=>{await call("retry_autotag",{assetId});await loadAutotagState();}} /></section>
    {:else if view === "artist"}
      <section class="artist-page-host">
        {#if spotifyArtistId || spotifyAlbumId}<SpotifyArtist onSaveCompleteAlbum={async (entry, items, expected) => { await saveCompleteLibraryAlbum(call, entry, items, expected); requestLocalPrefixRefresh(undefined, true); }} onSaveToLibrary={saveTracksToLibrary} onTrackAlbum={(track) => void openTrackAlbum(track)} bind:this={spotifyArtistComponent} navigationSnapshotKey={spotifyNavigationSnapshotKey} artistId={spotifyArtistId || spotifyAlbumId} initialAlbumId={spotifyAlbumId || null} searchArtistFallback={spotifyArtistSearchFallback} call={spotifyArtistCall} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {paused} onPlay={playArtistTrack} onPlaylist={addArtistTrack} onAddSelectedToPlaylist={addTracksToPlaylist} onAddAlbumToPlaylist={addAlbumToPlaylist} onBrowseArtistProvider={browseArtistProvider} onFavorite={(track)=>void favorite(track)} onDownload={(track)=>void openDownloadChooser(track)} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onArtist={(track)=>void discoverArtist(track)} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};void matchArtistPlaybackSources([track]).then(([matched])=>{if(actionTrack===track&&matched)actionTrack=matched;}).catch(failure=>{error=String(failure);});}} onAlbumOpening={()=>rememberRoute()} initialScroll={artistScroll} onScroll={(value)=>artistScroll=value}/>{:else}
        <LazyView loader={loadArtistPage} label="artist" componentProps={artistPageProps} onInstance={captureNativeArtistInstance} />{/if}
      </section>
    {:else if view === "settings"}
      <section class="settings" aria-label="Settings">
        <SettingsNavigation active={settingsCategory} onchange={(category)=>settingsCategory=category}/>
        {#if settingsCategory === "general"}
        <section class="settings-basic-card" aria-labelledby="appearance-settings-title">
          <h2 id="appearance-settings-title">Appearance</h2>
          <label>Theme<select bind:value={settings.theme} onchange={()=>void saveSettingsFields(["theme"],"Appearance saved.")}><option value="dark">Dark</option><option value="light">Light</option><option value="system">System</option></select></label>
          <label class="settings-option"><input type="checkbox" aria-label="Compact playback bar" checked={settings.compactPlaybackBar === true} onchange={(event) => { settings.compactPlaybackBar = event.currentTarget.checked; void saveSettingsFields(["compactPlaybackBar"], "Playback bar preference saved."); }} /><span><strong>Compact playback bar</strong><small>Leave more room for your library.</small></span></label>
        </section>
        {:else if settingsCategory === "playback"}
        <section class="settings-basic-card" aria-labelledby="playback-settings-title">
          <h2 id="playback-settings-title">Playback</h2>
          <LoudnessSettings settings={loudnessSettings} onSettingsSaved={refreshNormalizationSettings} playbackStatus={(nativeSnapshot as (NativePlaybackSnapshot & {loudnessNormalization?: LoudnessPlaybackStatus}) | null)?.loudnessNormalization ?? null} analysisJob={loudnessAnalysisJob} pendingAnalysisCount={pendingLoudnessAssetIds.length} analysisMessage={loudnessAnalysisMessage} onCancelAnalysis={cancelLoudnessAnalysis} onRetryAnalysis={retryQueuedLoudnessAnalysis} />
          <CrossfadeSettings enabled={settings.crossfadeEnabled === true} seconds={settings.crossfadeSeconds ?? 5} onchange={(enabled,seconds)=>{settings.crossfadeEnabled=enabled;settings.crossfadeSeconds=seconds;void saveSettingsFields(["crossfadeEnabled","crossfadeSeconds"],"Crossfade preference saved.");}} />
          <label class="settings-option"><input type="checkbox" bind:checked={settings.autoPreview} onchange={()=>void saveSettingsFields(["autoPreview"],"Playback preference saved.")} /><span><strong>Play songs selected with arrow keys</strong><small>Start the selected song after a short delay.</small></span></label>
          <details class="settings-inline-details"><summary>Opening a song’s source</summary>
            <label class="settings-option"><input type="checkbox" bind:checked={settings.pauseOnSourceOpen} onchange={()=>void saveSettingsFields(["pauseOnSourceOpen"],"Playback preference saved.")} /><span>Pause when opening the playing track’s source</span></label>
            <label class="settings-option"><input type="checkbox" bind:checked={settings.sourceOpenAtPosition} onchange={()=>void saveSettingsFields(["sourceOpenAtPosition"],"Playback preference saved.")} /><span>Open source at current position (YouTube and SoundCloud)</span></label>
          </details>
        </section>
        {/if}
        {#if ["connections","integrations","advanced"].includes(settingsCategory)}<ReleaseSetup category={settingsCategory} onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings={...value,matchingLevel:value.matchingLevel??"balanced",playbackStartupTimeoutSeconds:value.playbackStartupTimeoutSeconds??20})}/>{/if}
        {#if settingsCategory === "playback" || settingsCategory === "downloads"}
        <section class="settings-basic-card recording-matching-card" aria-label="Recording matching"><div class="settings-label-help"><label for="recording-matching">Recording matching</label><InfoPopover label="About recording matching">This app-wide rule applies to both playback and downloads. Exact requires complete recording agreement. Balanced allows metadata and credit variations while preserving the requested recording. Flexible can substitute another live version, cover or remix, labeled when used.</InfoPopover></div>
          <select id="recording-matching" aria-label="Recording match level" value={settings.matchingLevel ?? "balanced"} onchange={(event) => { settings.matchingLevel = event.currentTarget.value as "exact" | "balanced" | "flexible"; void saveSettingsFields(["matchingLevel"], "Recording matching saved."); }}>
            <option value="exact">Exact</option><option value="balanced">Balanced</option><option value="flexible">Flexible</option>
          </select>
          <small>{settings.matchingLevel === "flexible" ? "Choose which alternatives to allow." : "Keep the requested recording."}</small>
          {#if settings.matchingLevel === "flexible"}<FlexibleMatchingSettings value={settings.flexibleMatching} noticePending={settings.flexibleMatchingNoticePending === true} onchange={(value)=>{settings.flexibleMatching=value;void saveSettingsFields(["flexibleMatching"],"Flexible matching saved.");}} onNoticeDismiss={()=>{settings.flexibleMatchingNoticePending=false;void saveSettingsFields(["flexibleMatchingNoticePending"],"Matching notice dismissed.");}} />{/if}
        </section>
        <div class="settings-source-grid">
          {#if settingsCategory === "playback"}<section class="settings-source-card" aria-labelledby="streaming-settings-title">
            <h3 id="streaming-settings-title">Audio sources</h3><p>Local files play first. Choose where to look for other songs.</p>
            <label>Primary source<select aria-label="Preferred streaming source" bind:value={settings.preferredStreamingSource} onchange={()=>void saveSettingsFields(["preferredStreamingSource"],"Streaming source saved.")}>
            <option value="youtube">YouTube</option><option value="soundcloud">SoundCloud</option><option value="monochrome">Monochrome</option>
          </select><small>Monochrome requires a strong recording match.</small></label>
            <FallbackSettings label="Streaming" enabled={settings.streamingFallbackEnabled !== false} providers={settings.streamingFallbackSources ?? ["youtube", "soundcloud", "bandcamp"]} primary={settings.preferredStreamingSource ?? "youtube"} sources={[{id:"monochrome",name:"Monochrome"},{id:"youtube",name:"YouTube"},{id:"soundcloud",name:"SoundCloud"},{id:"bandcamp",name:"Bandcamp"}]} onchange={(enabled,sources)=>void saveFallbackSettings("streaming",enabled,sources)} />
            <label class="settings-option"><input type="checkbox" aria-label="Prepare saved radio songs" checked={settings.backgroundSourcePreparation !== false} onchange={(event) => { settings.backgroundSourcePreparation = event.currentTarget.checked; void saveSettingsFields(["backgroundSourcePreparation"], "Background preparation saved."); }} /><span><strong>Prepare saved radio songs</strong><small>Find and cache playable sources for saved radio songs in the background.</small></span></label>
            <label class="settings-option"><input type="checkbox" aria-label="Override playlist track URLs" bind:checked={settings.overridePlaylistTrackUrls} onchange={()=>void saveSettingsFields(["overridePlaylistTrackUrls"],"Playlist source preference saved.")} /><span><strong>Search for matches before trying playlist links</strong><small>Off tries the linked audio first, then verified alternatives if it fails. Explicit source choices stay locked.</small></span></label>
          </section>
          {/if}{#if settingsCategory === "downloads"}<section class="settings-source-card" aria-labelledby="download-settings-title">
            <h3 id="download-settings-title">Downloads</h3><p>Save a recording to your music folder.</p>
            <label class="settings-option"><input type="checkbox" aria-label="Embed lyrics in downloads" checked={settings.embedLyricsOnDownload === true} onchange={(event)=>{settings.embedLyricsOnDownload=event.currentTarget.checked;void saveSettingsFields(["embedLyricsOnDownload"],"Lyric embedding preference saved.");}} /><span><strong>Embed lyrics in downloads</strong><small>Save a remembered lyric choice or verified matching lyrics into the audio file for offline use. Downloads without suitable lyrics are saved normally.</small></span></label>
            <label>Folder layout<select aria-label="Download folder layout" value={settings.downloadLayout ?? "artistAlbum"} onchange={(event) => { settings.downloadLayout = event.currentTarget.value as Settings["downloadLayout"]; void saveSettingsFields(["downloadLayout"], "Download folder layout saved."); }}>
              <option value="flat">Flat</option>
              <option value="artistAlbum">Artist and album</option>
              <option value="playlist">Playlist first</option>
            </select><small>{settings.downloadLayout === "flat" ? "Music/Artist - Title.ext" : settings.downloadLayout === "playlist" ? "Music/Playlists/Name/Artist - Title.ext" : "Music/Artist/Album/Artist - Title.ext"}</small><small>Playlist folders apply to downloads started from a saved local playlist. Other downloads use Artist and album when Playlist first is selected. Existing files are not moved.</small></label>
            <label>Primary source<select aria-label="Preferred download source" bind:value={settings.preferredDownloadSource} onchange={() => void savePreferredDownloadSource()}>
            <option value="youtube">YouTube</option>
            <option value="soundcloud">SoundCloud</option>
            <option value="slskd">Soulseek</option>
            <option value="monochrome">Monochrome</option>
          </select><small>Default for Charts and Library. A source you explicitly select takes priority.</small></label>
            <FallbackSettings label="Download" enabled={settings.downloadFallbackEnabled === true} providers={settings.downloadFallbackSources ?? ["monochrome","slskd","youtube","soundcloud"]} primary={settings.preferredDownloadSource ?? "youtube"} sources={[{id:"monochrome",name:"Monochrome"},{id:"slskd",name:"Soulseek"},{id:"youtube",name:"YouTube"},{id:"soundcloud",name:"SoundCloud"}]} onchange={(enabled,sources)=>void saveFallbackSettings("download",enabled,sources)} />
            <p class="settings-source-note">Soulseek is currently available for downloads only.</p>
            <label class="settings-option"><input type="checkbox" aria-label="Allow lossy download fallback" bind:checked={settings.downloadFallbackAllowLossy} onchange={()=>void saveSettingsFields(["downloadFallbackAllowLossy"],"Download fallback preference saved.")} /><span><strong>Allow a lossy fallback</strong><small>Otherwise, a lossless download only falls back to another lossless source. Chapter downloads keep their selected source.</small></span></label>
            <SoulseekDownloadPolicy unavailable={settings.soulseekWaitForSource ? "wait" : "continue"} upgrade={settings.soulseekLosslessUpgrades === true} inSourceOrder={settings.preferredDownloadSource === "slskd" || settings.downloadFallbackEnabled === true && (settings.downloadFallbackSources ?? []).includes("slskd")} onchange={(unavailable,upgrade)=>{settings.soulseekWaitForSource=unavailable==="wait";settings.soulseekLosslessUpgrades=upgrade;void saveSettingsFields(["soulseekWaitForSource","soulseekLosslessUpgrades"],"Soulseek download policy saved.");}} />
          </section>
          {/if}
        </div>{/if}
        {#if settingsCategory === "library"}
        <PlaylistSettings preferences={playlistPreferences} collections={visibleCollections.filter(item=>!latestImports.some(imported=>imported.collectionId===item.id))} {call} includeYtdlpSearchDefault={includeYtdlpSearchDefault} onYtdlpSearchDefaultChange={(enabled)=>includeYtdlpSearchDefault=enabled} onchange={(value)=>{playlistPreferences=value;void refreshFavorites();}} oncollections={()=>refreshCollections()}/>
        <section class="settings-basic-card" aria-label="Local playlist copies"><h2>Local playlist copies</h2>
        <label class="settings-option"><input type="checkbox" aria-label="Keep local M3U playlist copies" disabled={Boolean(busy)} checked={settings.playlistMirroringEnabled !== false} onchange={(event) => { settings.playlistMirroringEnabled = event.currentTarget.checked; void saveSettingsFields(["playlistMirroringEnabled"], "Playlist mirroring preference saved."); }} /><span><strong>Keep local M3U playlist copies</strong><small>Automatically read and update playlists, Liked Songs, and listening history as M3U8 files in your Music/Playlists folder. Off leaves existing files in place. Manual playlist import and export remain available.</small></span></label>
        {#if settings.playlistMirroringEnabled !== false}<button onclick={() => playlistMirrorSettingsTarget = { collectionId: null }}>Choose playlists to sync…</button>{/if}
        </section>
        <HistoryMirrorSettings {call} desktop={desktop()} />
        <RestoredPlaylistSyncSettings {call} desktop={desktop()} />
        <LazyView loader={loadRestoreWorkReview} label="restore review" componentProps={{ onReviewed: updateStreamMediaAnalysisRestorePolicy }} />
        <section class="settings-basic-card" aria-labelledby="library-settings-title">
          <h2 id="library-settings-title">Music folder</h2><p>Completed downloads are saved here after validation and tagging.</p>
          <label>Download destination<input bind:value={settings.musicDirectory} placeholder="/home/you/Music/DJ" /></label>
          <button disabled={busy>0} onclick={()=>void chooseMusicDirectory()}>Choose music folder…</button>
          {#if settings.musicDirectory !== persistedMusicDirectory}<button onclick={()=>void saveSettingsFields(["musicDirectory"],"Music folder saved.")}>Save music folder</button>{/if}
        </section>
        <LocalPlaylistFiles mode="settings" {settings} onChange={(value) => (settings = value)} onSave={() => void saveSettingsFields(["playlistDirectories"],"Playlist folders saved.")} />
        {#if desktop()}<LazyView loader={loadBackupRestore} label="backup settings" componentProps={{ inspectBackup: inspectBackupForUi, createBackup: createBackupForUi, restoreBackup: restoreBackupForUi, chooseMediaRoot: chooseBackupMediaRoot, estimateBackup: estimateBackupForUi, cancelJob: cancelBackupJob, loadActiveBackupJob, subscribeBackupProgress, loadSchedule: loadBackupSchedule, saveSchedule: saveBackupSchedule, chooseScheduleDestination: chooseBackupScheduleDestination, exportSettings: exportBackupSettings, inspectSettings: inspectBackupSettings, applySettings: applyBackupSettings, previewRestore: previewBackupRestore, allowReplace: true }} />{/if}
        {/if}
        {#if settingsCategory === "playback"}<label class="settings-option"><input type="checkbox" bind:checked={includeRadioHistory} onchange={()=>void run(()=>call("set_radio_history_setting",{enabled:includeRadioHistory}))}/><span><strong>Include radio songs in playback history</strong><small>Save identified songs you hear with their station and listening time.</small></span></label>{/if}
        {#if settingsCategory === "playback"}
            {@const mediaResource = currentStreamMediaResource()}
            {@const mediaJob = streamMediaAnalysisJob?.resourceKey === mediaResource?.key ? streamMediaAnalysisJob : null}
        <details class="settings-advanced"><summary><strong>Audio analysis</strong><span>BPM, musical key and waveforms</span></summary><div class="settings-details-body">
          <div class="settings-option-help"><label class="settings-option"><input type="checkbox" aria-label="Enable live BPM and key" checked={settings.liveBpmEnabled === true} onchange={(event) => { settings.liveBpmEnabled = event.currentTarget.checked; void saveSettingsFields(["liveBpmEnabled"], "Live playback estimate preference saved."); }} /><span><strong>Show live BPM and key</strong><small>Estimate tempo and key while listening.</small></span></label><InfoPopover label="About live BPM and key">Uses recent audio while listening. Estimates may change as the song plays and do not update saved metadata. Enabling analysis uses additional processing power.</InfoPopover></div>
            <StreamMediaAnalysisSettings
              enabled={streamMediaAnalysisEnabled}
              status={streamMediaAnalysisStatus}
              onEnabledChange={(enabled)=>streamMediaAnalysisEnabled=enabled}
              mediaEligible={Boolean(desktop() && mediaResource)}
              mediaState={streamMediaAnalysisStartingKey === mediaResource?.key ? "starting" : mediaJob ? mediaJob.state === "queued" || mediaJob.state === "running" || mediaJob.state === "cancelling" ? "running" : "idle" : "idle"}
              mediaStatus={streamMediaAnalysisStartingKey === mediaResource?.key || mediaJob ? streamMediaAnalysisStatus : ""}
              onMediaStart={() => { const resource = currentStreamMediaResource(); if (resource) { attemptedStreamMediaAnalysis.add(resource.key); void startStreamMediaAnalysis(resource); } }}
              onMediaCancel={() => void cancelStreamMediaAnalysis()}

              cachedResult={cachedStreamAnalysis && cachedStreamAnalysisOwnerKey === cachedStreamAnalysisCurrentOwnerKey ? `Whole-track analysis${cachedStreamAnalysis.bpm ? ` · ${cachedStreamAnalysis.bpm.bpm.toFixed(1)} BPM${cachedStreamAnalysis.bpm.relatedTempoAlternativeBpm ? ` (tempo alternative ${cachedStreamAnalysis.bpm.relatedTempoAlternativeBpm.toFixed(1)})` : ""}` : ""}${cachedStreamAnalysis.key ? ` · ${cachedStreamAnalysis.key.key}` : ""}` : ""}

              localEligible={Boolean(waveformEnabled && waveformLocalAssetId)}
              localBusy={Boolean(localWaveformAnalysisJobId)}
              localStatus={localWaveformAnalysisStatus}
              onLocalStart={() => void analyzeCurrentLocalWaveform()}
              onLocalCancel={() => void cancelLocalWaveformAnalysis()}
            />

        </div></details>
        <details class="settings-advanced"><summary><strong>Advanced playback</strong><span>Playback timing</span></summary><div class="settings-details-body">
          <div class="settings-grid">
            <label>Playback startup timeout<select id="playback-startup-timeout" bind:value={settings.playbackStartupTimeoutSeconds} onchange={()=>void saveSettingsFields(["playbackStartupTimeoutSeconds"],"Startup timeout saved.")}><option value={20}>20 seconds</option><option value={30}>30 seconds</option><option value={45}>45 seconds</option></select></label>
            <label>Preview delay (ms)<input type="number" min="0" max="5000" bind:value={settings.previewDelayMs} onchange={()=>void saveSettingsFields(["previewDelayMs"],"Preview delay saved.")} /></label>
          </div>
          <div class="settings-label-help"><label for="streaming-fallback-delay">Wait for preferred source (ms)</label><InfoPopover label="About source priority timing">Source lookups can run in parallel. When an alternative is ready first, Burrata gives your preferred source this much extra time before using the alternative. The playback startup timeout still applies.</InfoPopover></div>
          <input id="streaming-fallback-delay" type="number" min="0" max="10000" step="100" aria-label="Streaming fallback delay" bind:value={settings.streamingFallbackDelayMs} onchange={()=>void saveSettingsFields(["streamingFallbackDelayMs"],"Fallback delay saved.")} />
        </div></details>
        <ReleaseSetup category="playback" onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings={...value,matchingLevel:value.matchingLevel??"balanced",playbackStartupTimeoutSeconds:value.playbackStartupTimeoutSeconds??20})}/>
        {/if}
        {#if settingsCategory === "downloads"}<AutotagSetup {...autotagSettings} onConfigureAcoustId={()=>void configureAcoustId()} acoustIdConfigured={autotagSettings.acoustidConfigured} onChanged={(change)=>autotagSettings={...autotagSettings,...change}} onSave={()=>void run(async()=>{autotagSettings=await call("save_autotag_settings",{enabled:autotagSettings.enabled,shazamFallback:autotagSettings.shazamFallback});})} />
        {/if}{#if settingsCategory === "connections"}<AccountPlaylistSettings value={sidebarLayout.showAccountPlaylists} onChange={(value) => void persistSidebar({...sidebarLayout,showAccountPlaylists:value})} /><BrowserConnections {call} onWriteSyncChange={(provider,enabled)=>remotePlaylistWriteSyncEnabled={...remotePlaylistWriteSyncEnabled,[provider]:enabled}} onChanged={() => { void call<typeof availableSources>("list_search_sources").then(sources => availableSources = sources).catch(() => {}); }} onBrowse={(source)=>void searchProviderPlaylists(source)} /><ConnectionsSetup onChanged={(status) => { slskdEnabled = status.settings.slskd.enabled; lyricsTranslationLanguage=status.settings.lyricsTranslation?.targetLanguage ?? "en";lyricsTranslationEnabled=status.settings.lyricsTranslation?.enabled===true; lyricsTranslationCredentialPresent=status.settings.lyricsTranslation?.provider === "googleFree" || status.settings.lyricsTranslation?.provider === "auto" || status.lyricsTranslationCredential?.present===true; sourceCapabilities={soulseek:slskdEnabled,discogs:status.settings.discogsEnabled}; void call<typeof availableSources>("list_search_sources").then(sources=>availableSources=sources).catch(()=>{}); void loadAutotagState().catch(()=>{}); }} />
        {/if}{#if settingsCategory === "advanced"}<div class="settings-details-body">
          <LazyView loader={loadPlaybackDiagnosticsSettings} label="playback diagnostics settings" componentProps={{ call, enabled: settings.playbackDiagnosticsEnabled === true, onEnabledChange: savePlaybackDiagnosticsEnabled }} />
          <ToolSetup {settings} {dependencies} onChange={(value)=>(settings=value)} onCheck={()=>void run(async()=>(dependencies=await call<Dependency[]>("check_dependencies")))} onSave={()=>void saveSettingsFields(["ytDlpPath","ffmpegPath","ffprobePath","jsRuntimePath"],"Tool paths saved.")} checking={busy>0} />
        </div>
        {/if}{#if settingsCategory === "integrations"}<details class="settings-advanced" open><summary><strong>Assistant permissions</strong><span>Control access to your player and library</span></summary>
          <section class="automation-settings"><p>Allow a connected assistant to use the player and library.</p><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.control} onchange={()=>void saveAutomationPermissions()} /><span>Allow playback and playlist changes</span></label><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.manage} onchange={()=>void saveAutomationPermissions()} /><span>Allow downloads and audio analysis</span></label></section>
        </details>{/if}
      </section>
    {:else if view === "search"}
      {#if searchRelease}<ReleaseView onSaveCompleteAlbum={saveReleaseToLibrary} onSaveToLibrary={saveTracksToLibrary} bind:this={searchReleaseComponent} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {paused} release={searchRelease} tracks={searchReleaseTracks} loading={searchReleaseBusy} error={searchReleaseError} onAddAlbumToPlaylist={addAlbumToPlaylist} onAnalyzeLoudness={prepareReleaseLoudnessAnalysis} onPlayAlbum={playReleaseAlbum} onQueueAlbum={(items,resource)=>queueArtistTracks(items,"append",resource)} onPlay={playArtistTrack} onQueue={queueArtistTracks} onDownload={(track)=>void openDownloadChooser(track)} onDownloadAlbum={(items,onReceipt)=>queueSelected(items,onReceipt as ((receipt:AlbumBatchReceipt)=>void|Promise<void>)|undefined)} onSearchAlbumSoulseek={() => openSoulseekAlbumSearch(searchRelease!)} soulseekSearchAvailable={soulseekAlbumSearchAvailable} onFavorite={(track)=>void favorite(track)} onArtist={(track)=>void discoverArtist(track)} onPlaylist={(track)=>void addArtistTrack(track)} onAddSelectedToPlaylist={addTracksToPlaylist} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} />{:else}
<LazyView loader={loadSearchPanel} label="search" componentProps={searchPanelProps} onInstance={captureSearchPanelInstance} />{/if}
    {:else if view === "radio"}
      <LazyView loader={loadRadioView} label="radio" componentProps={radioViewProps} />
    {:else if view === "source-browser"}
      {#if provider==="spotify" || provider==="youtube"}{#key `${provider}:${remoteOwner}:${remotePlaylistUrl}`}
        {#if remotePlaylistUrl}<LazyView loader={loadRemotePlaylistDetail} label="remote playlist" componentProps={remotePlaylistDetailProps} />
        {:else if remoteOwner}<LazyView loader={loadRemotePlaylists} label="remote playlists" componentProps={remotePlaylistsProps} />
        {:else}<section class="provider-search-fallback"><h2>Search {provider === "spotify" ? "Spotify" : "YouTube"} playlists</h2><p>Use main Search to find public playlists. Saved account playlists stay in the sidebar.</p><button class="primary" onclick={() => void searchProviderPlaylists(provider as "spotify" | "youtube")}>Search playlists</button></section>{/if}
      {/key}{:else}<SourcePresets {provider} onImport={importPreset} onOpen={(url)=>void run(()=>call("open_external_url",{url}))} />{/if}
    {:else if view === "directory-browser"}
      <div aria-hidden="true"></div>
    {:else if view === "history"}
      <section class="history-page" aria-label="Listening history">
        <div class="history-export-action"><button onclick={()=>showPortableHistory=true}>Export history</button></div>
        <form class="history-filters" onsubmit={(event)=>{event.preventDefault();void loadHistory();}}>
          <label class="history-search">Search history<input aria-label="Search playback history" bind:value={historyQuery} placeholder="Track, artist, album or station"/></label>
          <button class="history-search-submit" disabled={historyBusy}>Search</button>
          <details class="history-advanced-filters">
            <summary aria-label="Show history filters">Filters{#if historyProvider || historyStationId || historyFrom || historyTo || historyOrder !== "newest" || historySortField !== "sourceOrder" || historyMinDuration || historyMaxDuration}<span> · active</span>{/if}</summary>
            <div class="history-advanced-fields">
          <label>Source<select aria-label="History source" bind:value={historyProvider} onchange={()=>{if(historyProvider!=="radio")historyStationId="";void loadHistory();}}><option value="">All sources</option><option value="radio">Radio</option><option value="non-radio">Other playback</option><option value="youtube">YouTube</option><option value="soundcloud">SoundCloud</option><option value="local">Local files</option></select></label>
          {#if historyProvider === "radio"}<label>Station<select aria-label="History station" bind:value={historyStationId} onchange={()=>void loadHistory()}><option value="">All stations</option>{#each radioStations as station}<option value={station.id}>{station.name}</option>{/each}</select></label>{/if}
          <label>From<input type="date" aria-label="History from date" bind:value={historyFrom}/></label><label>To<input type="date" aria-label="History to date" bind:value={historyTo}/></label>
          <TrackDownloadFilter bind:value={historyDownloadAvailability} onchange={()=>void loadHistory()} />
          <label>Order<select aria-label="History sort order" bind:value={historyOrder} onchange={()=>void loadHistory()}><option value="newest">Newest first</option><option value="oldest">Oldest first</option></select></label>
          <label>Sort tracks<select aria-label="Sort playback history tracks" bind:value={historySortField} onchange={()=>void loadHistory()}><option value="sourceOrder">Play order</option><option value="title" disabled={!historySortAvailability.title}>{trackSortOptionLabel(historySortAvailability, "title", "Title")}</option><option value="artist" disabled={!historySortAvailability.artist}>{trackSortOptionLabel(historySortAvailability, "artist", "Artist")}</option><option value="album" disabled={!historySortAvailability.album}>{trackSortOptionLabel(historySortAvailability, "album", "Album")}</option><option value="duration" disabled={!historySortAvailability.duration}>{trackSortOptionLabel(historySortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!historySortAvailability.releaseDate}>{trackSortOptionLabel(historySortAvailability, "releaseDate", "Release date")}</option><option value="dateAdded" disabled={!historySortAvailability.dateAdded}>{trackSortOptionLabel(historySortAvailability, "dateAdded", "Date added")}</option><option value="bpm" disabled={!historySortAvailability.bpm}>{trackSortOptionLabel(historySortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!historySortAvailability.key}>{trackSortOptionLabel(historySortAvailability, "key", "Key")}</option><option value="plays" disabled={!historySortAvailability.plays}>{trackSortOptionLabel(historySortAvailability, "plays", "Plays")}</option><option value="views" disabled={!historySortAvailability.views}>{trackSortOptionLabel(historySortAvailability, "views", "Views")}</option></select></label>
          {#if historySortField!=="sourceOrder"}<label>Direction<select aria-label="Playback history sort direction" bind:value={historySortDirection} onchange={()=>void loadHistory()}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>{/if}
          <label>Minimum duration<input aria-label="History minimum duration" bind:value={historyMinDuration} placeholder="seconds or mm:ss"/></label><label>Maximum duration<input aria-label="History maximum duration" bind:value={historyMaxDuration} placeholder="seconds or mm:ss"/></label>
              <button type="submit" class="history-apply-filters" disabled={historyBusy}>Apply filters</button>
            </div>
          </details>
        </form>
        <div class="detail-actions"><small>{historyTotal} plays</small><button disabled={!historyItems.length} onclick={() => void run(async () => { await call("clear_playback_history"); await loadHistory(); })}>Clear history</button></div>
        {#if displayedHistoryItems.length}<TrackList playbackFailed={nativePlaybackStatus === "failed"} onAlbum={(track) => void openTrackAlbum(track)} virtualizationThreshold={50} orderedRows={displayedHistoryItems.map((item,index)=>({kind:"track" as const,key:item.sessionId,trackIndex:index}))} focusedRowKey={historyFocusedSessionId} tracks={displayedHistoryItems.map(item=>item.track)} listLabel="Playback history tracks" presentation="playlist" playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} playingTrackId={playingTrack?.id??""} bufferingTrackId={playbackLoading?playingTrack?.id??"":""} {paused} position={(_,index)=>index+1} meta={historySource} timestamp={(_,index)=>({label:new Date(displayedHistoryItems[index].playedAt).toLocaleString(undefined,{dateStyle:"medium",timeStyle:"long"}),full:new Date(displayedHistoryItems[index].playedAt).toString()})} onPlay={async (track,list,index)=>{ await startPreview(track,false,false,list,index); }} onDownload={(track)=>openDownloadChooser(track)} onFavorite={favorite} onArtist={(track)=>discoverArtist(track)} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onPlaylist={addArtistTrack} onInfo={(track)=>openPlayerPanel("info",track)} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} initialScroll={historyScroll} onScroll={(value)=>historyScroll=value}/>
        {:else}<div class="empty"><h2>{historyBusy?"Loading history…":historyQuery||historyProvider||historyFrom||historyTo?"No matching plays":"Nothing played yet"}</h2><p>Songs appear here when audio starts.</p></div>{/if}
        {#if historyHasMore}<button disabled={historyBusy} onclick={()=>void loadHistory(!historyOptionsPending)}>{historyOptionsPending ? "Apply filters and reload" : "Show earlier plays"}</button>{/if}
      </section>
    {:else if view === "downloads"}
      <div class="toolbar">
        <span>{sortedDownloadJobs.length} jobs</span>
        <details class="download-tools"><summary aria-label="More download options">•••</summary><button onclick={(event)=>{const menu=event.currentTarget.closest("details");if(menu)menu.open=false;showSavedSoulseekSearches=true;}}>Saved Soulseek searches…</button></details>
        <label>Sort downloads<select aria-label="Sort download jobs" bind:value={downloadSortField}><option value="sourceOrder">Original order</option><option value="title" disabled={!downloadSortAvailability.title}>{trackSortOptionLabel(downloadSortAvailability, "title", "Title")}</option><option value="artist" disabled={!downloadSortAvailability.artist}>{trackSortOptionLabel(downloadSortAvailability, "artist", "Artist")}</option><option value="album" disabled={!downloadSortAvailability.album}>{trackSortOptionLabel(downloadSortAvailability, "album", "Album")}</option><option value="duration" disabled={!downloadSortAvailability.duration}>{trackSortOptionLabel(downloadSortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!downloadSortAvailability.releaseDate}>{trackSortOptionLabel(downloadSortAvailability, "releaseDate", "Release date")}</option><option value="dateAdded" disabled={!downloadSortAvailability.dateAdded}>{trackSortOptionLabel(downloadSortAvailability, "dateAdded", "Date added")}</option><option value="bpm" disabled={!downloadSortAvailability.bpm}>{trackSortOptionLabel(downloadSortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!downloadSortAvailability.key}>{trackSortOptionLabel(downloadSortAvailability, "key", "Key")}</option><option value="plays" disabled={!downloadSortAvailability.plays}>{trackSortOptionLabel(downloadSortAvailability, "plays", "Plays")}</option><option value="views" disabled={!downloadSortAvailability.views}>{trackSortOptionLabel(downloadSortAvailability, "views", "Views")}</option></select></label><label>Direction<select aria-label="Download sort direction" bind:value={downloadSortDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label><TrackDurationFilter bind:minDuration={downloadMinDuration} bind:maxDuration={downloadMaxDuration} minLabel="Download minimum duration" maxLabel="Download maximum duration" />
        <button onclick={() => void refreshJobs()}>Refresh queue</button>
      </div>
      <DownloadWatches onCount={(count)=>activeDownloadWatchCount=count} onReview={(track)=>openDownloadChooser(track,undefined,undefined,undefined,"slskd")} />
      <section class="queue">
        <VirtualResults items={sortedDownloadJobs} itemKey={(job) => job.id} estimateHeight={() => 112} scrollKey={`downloads:${downloadSortField}:${downloadSortDirection}:${downloadMinDuration}:${downloadMaxDuration}`} regionKey="download-jobs" scrollRootSelector=".queue" ariaLabel="Download job rows">
          {#snippet children(job)}<article class="job">
            <div>
              <strong>{downloadTitle(job)}</strong>
              {#if (job.track.provenance as Record<string,unknown>)?.soulseekDownloadWatchKind === "upgrade"}<small>Lossless upgrade · your saved copy stays available until ready</small>{/if}
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
          </article>{/snippet}
        </VirtualResults>
        {#if !sortedDownloadJobs.length && !activeDownloadWatchCount}<div class="empty">
          <span>↓</span>
          <h2>Your queue is clear</h2>
          <p>Choose a recording, compare its sources, then start a download.</p>
        </div>{/if}
      </section>
    {:else if view === "library" && (libraryBrowser.tab !== "tracks" || libraryBrowser.entity)}
      <LazyView loader={loadLibraryBrowser} label="library" componentProps={{ browserState: libraryBrowser, call, onNavigate: navigateLibraryBrowser, onStateChange: (next: LibraryBrowserState) => libraryBrowser = next, onBrowse: (entry: LibraryEntity) => void run(() => browseLibraryEntity(entry)), onPlay: playArtistTrack, onQueue: queueArtistTracks, onDownload: (track: Track) => void openDownloadChooser(track), onFavorite: (track: Track) => void favorite(track), onPlaylist: (track: Track) => void addArtistTrack(track), onArtist: (track: Track) => void discoverArtist(track), onAlbum: (track: Track) => void openTrackAlbum(track), onContext: (track: Track, event: MouseEvent) => { actionTrack = track; actionAnchor = { x: event.clientX, y: event.clientY }; }, onAddSelectedToPlaylist: (items: Track[]) => void addTracksToPlaylist(items), onDownloadAlbum: (items: Track[]) => queueSelected(items), onSearchAlbumSoulseek: (entry: LibraryEntity) => openSoulseekAlbumSearchByTitle(entry.name, entry.artist), soulseekSearchAvailable: soulseekAlbumSearchAvailable, onAddAlbumToPlaylist: addAlbumToPlaylist, onAnalyzeLoudness: (items: Track[], gaps: number) => prepareReleaseLoudnessAnalysis(items, gaps, true), playingTrack, bufferingTrack: playbackLoading ? playingTrack : null, bufferingTrackId: playbackLoading ? playingTrack?.id ?? "" : "", playbackFailed: nativePlaybackStatus === "failed", paused }} />
    {:else}
      <div class="toolbar" class:chart-toolbar={view === "charts"} class:library-toolbar={view === "library"} class:empty-library-toolbar={view === "library" && !showCreatePlaylist}>
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
          <button onclick={() => showLibraryCleanup = true}>Review unsorted entries</button>

          {#if showCreatePlaylist}<input bind:value={newCollection} placeholder="New playlist name" aria-label="New playlist name" /><button disabled={!newCollection.trim()} onclick={() => void createCollection().then(() => (showCreatePlaylist = false))}>Create</button>{/if}

        {:else if view === "collections"}
          {#if collectionId===DOWNLOADS_PLAYLIST_ID}<button aria-label="Open download activity" onclick={()=>{openPlayerPanel("downloads");void refreshJobs();}}>Download activity</button>{/if}
          {@const savedCount = currentCollection?.trackCount ?? 0}
          <button class="primary" aria-label={`Play all ${savedCount} saved tracks in playlist order, ignoring filters`} title="Plays every saved playlist track in saved order, including tracks hidden by filters" disabled={!collectionId || savedCount === 0 || playlistPlaybackAction !== null} aria-busy={playlistPlaybackAction === "play"} onclick={() => void playCurrentPlaylist("play")}>{playlistPlaybackAction === "play" ? "Starting…" : "▶ Play full playlist"}</button>
          <button aria-label={`Queue all ${savedCount} saved tracks in playlist order, ignoring filters`} title="Adds every saved playlist track in saved order, including tracks hidden by filters" disabled={!collectionId || savedCount === 0 || playlistPlaybackAction !== null} aria-busy={playlistPlaybackAction === "queue"} onclick={() => void playCurrentPlaylist("queue")}>{playlistPlaybackAction === "queue" ? "Queuing…" : "＋ Queue full playlist"}</button>
          <span class="playlist-action-scope">All {savedCount} saved tracks · filters ignored</span>
          <button onclick={openPlaylistImport}>Import tracks</button>
          {#if renamingPlaylist}<input aria-label="Playlist name" bind:value={renamePlaylistName} onkeydown={(event)=>{if(event.key==="Enter")void renameCurrentPlaylist();if(event.key==="Escape")renamingPlaylist=false;}} /><button onclick={()=>void renameCurrentPlaylist()}>Save name</button>{/if}
          {#if currentPlaylistCreator}<button disabled={Boolean(busy)} onclick={()=>void refreshLinkedPlaylist(collectionId)}>Refresh</button>{#if (currentPlaylistCreator.provider === "spotify" || currentPlaylistCreator.provider === "youtube") && currentPlaylistCreator.canEdit}<PlaylistSync {call} bindingId={currentPlaylistCreator.bindingId} enabled={remotePlaylistWriteSyncEnabled[currentPlaylistCreator.provider as "spotify" | "youtube"]} onApplied={()=>refreshLinkedPlaylist(collectionId)}/>{/if}{/if}<button class="playlist-context-trigger" aria-label="Playlist context menu" title="More playlist actions" onclick={openCurrentPlaylistMenu}>⋯</button>{/if}
      </div>
      {#if view === "charts"}<div class="chart-context">
          <span
            >{activeBrowserImport ? "" : chart?.title ?? "Genre charts"}{#if chart?.stale}<span class="tag"
                >Cached</span
              >{/if}</span
          >
          <div class="chart-source-actions">
            {#if activeBrowserImport}<button disabled={batchRunning} onclick={() => void downloadPlaylist(activeBrowserImport!.collectionId, true)}>↓ Download playlist</button><button class="playlist-context-trigger" aria-label="Playlist context menu" title="Playlist actions" onclick={openCurrentPlaylistMenu}>⋯</button>{:else if chart}<button class="playlist-context-trigger" aria-label="Playlist context menu" title="Playlist actions" onclick={openCurrentPlaylistMenu}>⋯</button>{/if}

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
        {#if mirrorStatus || pausedMirrors.length}<section class="library-overview" aria-label="Playlist file status">
          {#if mirrorStatus}<p class="sync-warning" role="status">{mirrorStatus}</p>{/if}
          {#each pausedMirrors as id}<button onclick={() => void run(async () => { await call("resume_playlist_mirror", { collectionId: id }); await syncMirrors(); })}>Restore M3U · {id === "favorites" ? "Liked Songs" : collections.find((item) => item.id === id)?.name ?? "playlist"}</button>{/each}
        </section>{/if}
        {#if librarySource === "local"}<div class="local-playlist-region"><LocalPlaylistFiles {settings} onSettings={() => void navigate("settings")} onImported={(created) => { void run(async () => { collections = await call<Collection[]>("list_collections"); await openPlaylist(created.id); }); }} /></div>{/if}
      {/if}
      <div class="workspace" class:track-info-open={showTrackDetails} class:other-panel={showPlayingInfo || showQueue || showDownloads}>
        <section class="track-area">
          <div class="table-tools" class:compact-tools={["library", "favorites", "collections"].includes(view)}>
            {#if view === "collections"}
              {#if localPlaylistSelectionMode}
                <button disabled={!selectedLocalOccurrenceTracks.length} onclick={() => void addTracksToPlaylist(selectedLocalOccurrenceTracks)}>Add {selectedLocalOccurrenceTracks.length || "selected"} to playlist</button>
                <button onclick={() => { selectedLocalOccurrences = []; localPlaylistSelectionMode = false; }}>Cancel selection</button>
              {:else}
                <button aria-label="Select playlist tracks" onclick={() => localPlaylistSelectionMode = true}>Select</button>
              {/if}
            {:else if batchSelectionMode}
              <button onclick={() => { checked = []; batchSelectionMode = false; mergeConfirmed = false; }}>Cancel selection</button>
            {:else}
              <button aria-label="Select tracks for batch actions" onclick={() => batchSelectionMode = true}>Select</button>
            {/if}
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
              <option value="rank">Original order</option><option value="title" disabled={!mainSortAvailability.title}>{trackSortOptionLabel(mainSortAvailability, "title", "Title")}</option><option value="artist" disabled={!mainSortAvailability.artist}>{trackSortOptionLabel(mainSortAvailability, "artist", "Artist")}</option><option value="album" disabled={!mainSortAvailability.album}>{trackSortOptionLabel(mainSortAvailability, "album", "Album")}</option><option value="dateAdded" disabled={!mainSortAvailability.dateAdded}>{trackSortOptionLabel(mainSortAvailability, "dateAdded", "Date added")}</option><option value="plays" disabled={!mainSortAvailability.plays}>{trackSortOptionLabel(mainSortAvailability, "plays", "Plays")}</option><option value="views" disabled={!mainSortAvailability.views}>{trackSortOptionLabel(mainSortAvailability, "views", "Views")}</option><option value="bpm" disabled={!mainSortAvailability.bpm}>{trackSortOptionLabel(mainSortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!mainSortAvailability.key}>{trackSortOptionLabel(mainSortAvailability, "key", "Key")}</option><option value="durationSeconds" disabled={!mainSortAvailability.duration}>{trackSortOptionLabel(mainSortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!mainSortAvailability.releaseDate}>{trackSortOptionLabel(mainSortAvailability, "releaseDate", "Release date")}</option>
            </select>
            {#if ["library", "favorites", "collections"].includes(view)}
              <button class="sort-direction" aria-label="Toggle sort direction" title={descending ? "Descending" : "Ascending"} onclick={()=>descending=!descending}><span aria-hidden="true">{descending ? "↓" : "↑"}</span><span>{descending?"Desc":"Asc"}</span></button>
              <details class="duration-filter-tools">
                <summary aria-label={activeDurationFilterCount ? `Duration filters, ${activeDurationFilterCount} active` : "Duration filters"}>Filters{#if activeDurationFilterCount}<span class="active-filter-count" aria-hidden="true">{activeDurationFilterCount}</span>{/if}</summary>
                <div class="duration-filter-fields">
                  <label>Minimum duration<input aria-label="Minimum duration" placeholder="seconds or mm:ss" bind:value={durationMinimum}/></label>
                  <label>Maximum duration<input aria-label="Maximum duration" placeholder="seconds or mm:ss" bind:value={durationMaximum}/></label>
                  {#if activeDurationFilterCount}<button type="button" class="clear-duration-filters" onclick={() => { durationMinimum = ""; durationMaximum = ""; }}>Clear duration filters</button>{/if}
                </div>
              </details>
              <span class="track-count" title={`${sorted.length}${localPageTotal>tracks.length?` of ${localPageTotal}`:""} tracks`} aria-label={`${sorted.length}${localPageTotal>tracks.length?` of ${localPageTotal}`:""} tracks`}>{sorted.length}{localPageTotal>tracks.length?`/${localPageTotal}`:""}<span class="sr-only"> tracks</span></span>
            {:else}
              <TrackDurationFilter bind:minDuration={durationMinimum} bind:maxDuration={durationMaximum} minLabel="Minimum duration" maxLabel="Maximum duration" />
              <button aria-label="Toggle sort direction" onclick={()=>descending=!descending}>{descending?"Descending":"Ascending"}</button>
              <span>{sorted.length}{localPageTotal>tracks.length?` of ${localPageTotal}`:""} tracks</span>
            {/if}
            {#if localSortPending}<span aria-live="polite">{tracks.length ? "Preparing results… Showing the previous view." : "Preparing results…"}</span>{/if}
            {#if checked.length}<input aria-label="New playlist for selection" bind:value={newCollection} placeholder="New playlist name" />
              <button disabled={!newCollection.trim()} onclick={() => void createAndAddPlaylist()}>Create & add</button>{/if}
            {#if checked.length}<button onclick={()=>queueArtistTracks(tracks.filter(track=>checked.includes(track.id)),"append")}>Add {checked.length} to queue</button><button aria-label="Select all filtered tracks" onclick={() => { checked = sorted.map(t => t.id); mergeConfirmed = false; }}>Select all</button><button
                onclick={() => void addSelected()}
                disabled={!collectionId}
                >Add {checked.length} to playlist</button
              ><button
                disabled={batchRunning}
                onclick={() => void queueSelected(undefined, undefined, undefined, currentSavedPlaylistContext())}
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
              ><button onclick={stopBatchMatching}
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
            {#if virtualized && startRow > 0}<div class="track-list-virtual-spacer" aria-hidden="true" style={`height:${startRow * rowHeight}px`}></div>{/if}
            <TrackList playbackFailed={nativePlaybackStatus === "failed"} reorderable={view === "collections" && sort === "rank" && !descending && !filter.trim() && localFilter === "all" && !durationMinimum && !durationMaximum && !playlistReordering && !localPageLoading} reorderCount={view === "collections" ? playlistMoveLimit : undefined} onReorder={movePlaylistEntry} onInvertSelection={() => {if(view === "collections") selectedLocalOccurrences = invertSelection(localOccurrenceRows.map(row=>row.key),selectedLocalOccurrences); else {checked=invertSelection(sorted.map(track=>track.id),checked);mergeConfirmed=false;}}} onAlbum={(track) => void openTrackAlbum(track)} tracks={virtualized ? visibleTracks : sorted} rowKeys={(virtualized ? visibleTracks : sorted).map((track,index) => `${track.id}:${(virtualized ? startRow : 0) + index}`)} focusedRowKey={view === "collections" ? focusedPlaylistRowKey : null} listLabel="Recordings" presentation="playlist" playingTrack={playingTrack} bufferingTrack={playbackLoading ? playingTrack : null} {paused} selectionMode={view === "collections" ? localPlaylistSelectionMode : batchSelectionMode} selectedTrackId={selected?.id} selectedTrackIds={view === "collections" ? selectedLocalOccurrences : checked} selectionKeys={view === "collections" ? localOccurrenceRows.slice(virtualized ? startRow : 0, virtualized ? endRow : sorted.length).map(row => row.key) : undefined} selectionLabel={(track, index) => view === "collections" ? `Select occurrence ${(virtualized ? startRow : 0) + index + 1}: ${track.title} for playlist` : `Select ${track.title}`} trackTitle={(track) => [track.title, bpmAnnotation(track), analysisAnnotation(track, "key")].filter(Boolean).join("\n")} position={(_, index) => (virtualized ? startRow : 0) + index + 1} meta={(track) => [track.bpm != null ? `${analysisAnnotation(track, "bpm") ? "≈" : ""}${String(track.bpm)} BPM` : "", track.key ? `${analysisAnnotation(track, "key") ? "≈" : ""}${track.key}` : "", !track.album && track.label ? track.label : "", track.disposition === "shortlist" ? "Shortlisted" : "", rowDownloadFeedback[track.id] ?? ""].filter(Boolean).join(" · ")} onPlay={(track, _window, index) => select(track, "click", (virtualized ? startRow : 0) + index)} onRowSelect={(track, event) => selectTrackRow(track, event)} onFavorite={favorite} onDownload={(track) => void openDownloadChooser(track, undefined, undefined, currentSavedPlaylistContext())} onArtist={(track) => void discoverArtist(track)} onContext={(track, event) => { actionTrack = track; const rect = event.type === "contextmenu" ? null : (event.currentTarget as HTMLElement).getBoundingClientRect(); actionAnchor = rect ? { x: rect.right, y: rect.bottom } : { x: event.clientX, y: event.clientY }; }} onSelect={(track, event, index) => { const checkedNow = (event.currentTarget as HTMLInputElement).checked; if (view === "collections") toggleLocalOccurrence((virtualized ? startRow : 0) + index, checkedNow); else { checked = checkedNow ? [...new Set([...checked, track.id])] : checked.filter(id => id !== track.id); mergeConfirmed = false; } }} virtualizationThreshold={100000} />
            {#if virtualized && endRow < sorted.length}<div class="track-list-virtual-spacer" aria-hidden="true" style={`height:${(sorted.length - endRow) * rowHeight}px`}></div>{/if}
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
                            ? "Use the heart on a recording to save it to your Favorites playlist."
                      : "Save recordings or import local files to build your library."}
                </p>
              </div>{/if}
            {#if ["library","favorites","collections"].includes(view)&&tracks.length<localPageTotal}<div class="load-more-row"><button disabled={localPageLoading||localSortPending} onclick={()=>void loadLocalPage(true)}>{localSortPending?"Preparing sort…":localPageLoading?"Loading…":"Load more tracks"}</button></div>{/if}
          </div>
          {#if view === "library" && selected && showTrackDetails}{@const asset = filesForTrack(selected)[0]}{#if asset}<div class="toolbar">
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
                      setNotice(`MP3 exported: ${result.path}`);
                    })}>Export MP3</button
                >
              </div>{/if}
          {/if}
        </section>
        {#if showTrackDetails && selected && !showPlayingInfo && !showQueue && !showDownloads}
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
              {#if metadataCount(selected,null,["views","viewCount","view_count"])!==null}<dt>Views</dt><dd>{metadataCount(selected,null,["views","viewCount","view_count"])?.toLocaleString()}</dd>{/if}
              {#if metadataCount(selected,null,["plays","playCount","play_count"])!==null}<dt>Plays</dt><dd>{metadataCount(selected,null,["plays","playCount","play_count"])?.toLocaleString()}</dd>{/if}
              {#each [["Label", selected.label], ["Release", selected.album], ["Genre", selected.genre], ["BPM", selected.bpm], ["Key", selected.key], ["Duration", time(playableDuration(selected))], ["Released", selected.releaseDate], ["ISRC", selected.isrc], ["Catalog", selected.catalogNumber]] as [key, value]}<dt
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
                    setNotice("Metadata saved.");
                  })}>Save correction</button
              >
            </details>{:else}<div class="details-empty">
              <span>◉</span>
              <h3>Listen closer</h3>
              <p>
                Select a recording to listen and compare sources.
              </p>
            </div>{/if}
        </aside>{/if}
      </div>
    {/if}
  {#if portablePlaylistTarget}<PortablePlaylist collection={portablePlaylistTarget} {call} initialMode={portablePlaylistInitialMode} includeYtdlpSearchDefault={includeYtdlpSearchDefault} onClose={()=>portablePlaylistTarget=null} />{/if}
  {#if playlistMirrorSettingsTarget}<PlaylistMirrorSettings collections={playlistMirrorCollections()} initialCollectionId={playlistMirrorSettingsTarget.collectionId} {call} onClose={()=>playlistMirrorSettingsTarget=null} onSaved={()=>{if(settings.playlistMirroringEnabled!==false)void syncMirrors();}} />{/if}
{#if selectedAnalysisOpen}
    <dialog class="download-dialog analysis-review" use:activateDownloadDialog={closeSelectedAnalysis} aria-labelledby="selected-analysis-title" aria-modal="true" tabindex="-1" onclick={(event) => {
      if (event.target !== event.currentTarget) return;
      const bounds = event.currentTarget.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) closeSelectedAnalysis();
    }}>
      <header>
        <button type="button" class="analysis-close" aria-label={selectedAnalysisBusy ? "Close and cancel analysis" : "Close analysis review"} onclick={closeSelectedAnalysis}>×</button>
        <h2 id="selected-analysis-title">Review BPM and key analysis</h2>
        {#if selectedAnalysisKind === "stream"}
          <p>Analysis reads the full streamed tracks. Audio isn’t saved, and existing BPM and key values are kept unless you choose to replace them.</p>
        {:else}
          <p>Analysis reads registered local audio and does not change audio files. Choose which measurements to apply.</p>
        {/if}
      </header>
      {#if selectedAnalysisJob}
        <p role="status">{selectedAnalysisJob.state === "completed" ? "Analysis complete" : selectedAnalysisJob.state === "cancelled" ? "Analysis cancelled" : selectedAnalysisJob.state === "cancelling" ? "Cancelling analysis…" : "Analyzing selected audio…"} · {selectedAnalysisJob.completed} of {selectedAnalysisJob.total}</p>
        {#if selectedAnalysisBusy}<progress max={selectedAnalysisJob.total || 1} value={selectedAnalysisJob.completed} aria-label="Selected track analysis progress"></progress>{/if}
        <div class="analysis-review-controls">
          <fieldset class="analysis-apply-options">
            <legend>Apply fields</legend>
            <label><input type="checkbox" bind:checked={applyAnalysisBpm} /><span>BPM</span></label>
            <label><input type="checkbox" bind:checked={applyAnalysisKey} /><span>Key</span></label>
          </fieldset>
          <fieldset class="analysis-replace-options">
            <legend>Replace existing values</legend>
            <label><input type="checkbox" bind:checked={replaceAnalysisBpm} disabled={!applyAnalysisBpm} /><span>Replace existing BPM, including manual values</span></label>
            <label><input type="checkbox" bind:checked={replaceAnalysisKey} disabled={!applyAnalysisKey} /><span>Replace existing key, including manual values</span></label>
            <p>Without replacement, existing imported and manually entered values are kept.</p>
          </fieldset>
        </div>
        <div class="analysis-review-list">
          {#each selectedAnalysisJob.results as row (row.assetId)}
            {@const reviewTrack = selectedAnalysisTracks[row.assetId]}
            <section class="analysis-review-row">
              <strong>{reviewTrack?.title ?? row.recordingId}</strong>
              <small>{reviewTrack?.artist ?? "Local audio"} · {row.status === "completed" ? `Whole track · ${Math.round((row.analysis?.coverage ?? 0) * 100)}% coverage` : row.error ?? row.status}</small>
              {#if row.kind === "stream" && row.sourceProvider && row.sourceId}<small class="analysis-source-identity">Verified source: {row.sourceProvider} · {row.sourceId}</small>{/if}
              {#if row.analysis}
                <div><span>BPM: {reviewTrack?.bpm ?? "—"} → {row.analysis.bpm?.bpm ?? "No estimate"}{#if row.analysis.bpm?.relatedTempoAlternativeBpm}<small> · alternative {row.analysis.bpm.relatedTempoAlternativeBpm}</small>{/if}</span>
                  {#if !applyAnalysisBpm}<small>BPM will stay unchanged.</small>{:else if analysisFieldExists(reviewTrack, "bpm") && !replaceAnalysisBpm}<small>Existing BPM will be kept{analysisFieldIsManual(reviewTrack, "bpm") ? " · marked manual" : ""}.</small>{:else if applyAnalysisBpm && replaceAnalysisBpm && analysisFieldExists(reviewTrack, "bpm")}<small>Existing BPM will be replaced by your explicit choice.</small>{/if}
                </div>
                <div><span>Key: {reviewTrack?.key ?? "—"} → {row.analysis.key?.key ?? "No confident estimate"}</span>
                  {#if !applyAnalysisKey}<small>Key will stay unchanged.</small>{:else if analysisFieldExists(reviewTrack, "key") && !replaceAnalysisKey}<small>Existing key will be kept{analysisFieldIsManual(reviewTrack, "key") ? " · marked manual" : ""}.</small>{:else if applyAnalysisKey && replaceAnalysisKey && analysisFieldExists(reviewTrack, "key")}<small>Existing key will be replaced by your explicit choice.</small>{/if}
                </div>
                <small>Detected with {row.analysis.analyzerVersion} · {Math.round(row.analysis.durationSeconds)} seconds · estimates are not catalog credits</small>
              {:else if row.error}<p role="alert">{row.error}</p>{/if}
            </section>
          {/each}
        </div>
      {:else}
        <p>Starting analysis…</p>
      {/if}
      <footer>
        {#if selectedAnalysisBusy}<button type="button" onclick={() => void cancelSelectedAnalysis()}>Cancel analysis</button>{/if}
        <button type="button" onclick={closeSelectedAnalysis}>{selectedAnalysisBusy ? "Close and cancel" : "Close"}</button>
        {#if selectedAnalysisJob?.state === "completed"}
          <button type="button" class="primary" disabled={selectedAnalysisBusy || (!applyAnalysisBpm && !applyAnalysisKey) || !selectedAnalysisJob.results.some(row => row.status === "completed" && row.analysis && !selectedAnalysisApplied.includes(row.assetId))} onclick={() => void applySelectedAnalysis()}>
            {selectedAnalysisBusy ? "Applying…" : "Apply reviewed values"}
          </button>
        {/if}
      </footer>
    </dialog>
{/if}

  {#if showPortableHistory}<PortableHistory {call} includeYtdlpSearchDefault={includeYtdlpSearchDefault} onClose={()=>showPortableHistory=false} />{/if}
  </main>
  {#if playlistTrack}<div class="download-backdrop">
    <dialog class="download-dialog playlist-picker-dialog" use:activateDownloadDialog={closePlaylistPicker} aria-labelledby="playlist-picker-title" tabindex="-1">
      <header><div><div class="eyebrow">PLAYLIST</div><h2 id="playlist-picker-title">{shortcutSlot!==null?`Assign playlist shortcut ${shortcutSlot+1}`:playlistAlbumName?`Add ${playlistAlbumName} to playlist`:playlistTracks.length>1?`Add ${playlistTracks.length} recordings`:"Add recording"}</h2><p>{#if playlistAlbumName}{playlistTracks.length} loaded {playlistTracks.length === 1 ? "track" : "tracks"}{:else}{playlistTrack.artist} — {playlistTrack.title}{#if playlistTracks.length>1}{` and ${playlistTracks.length-1} more`}{/if}{/if}</p></div>
        <button aria-label="Close playlist picker" onclick={closePlaylistPicker}>✕</button></header>
      {#if playlistAlbumName}<label>New playlist<input disabled={playlistWriteBusy} aria-label="New playlist for album" bind:value={newCollection} placeholder="Playlist name" onkeydown={(event) => { if (event.key === "Enter" && newCollection.trim() && !playlistWriteBusy) {event.preventDefault(); void createPlaylistForArtistTrack();} }} /></label><button class="primary" disabled={playlistWriteBusy || !newCollection.trim()} onclick={() => void createPlaylistForArtistTrack()}>Create &amp; add</button><p>Or choose an existing playlist below.</p>{/if}
      <label>Find playlist<input aria-label="Search playlist destinations" placeholder="Search playlists" bind:value={playlistDestinationQuery} use:focusPlaylistSearch onkeydown={(event)=>{if(event.key==="ArrowDown"){event.preventDefault();document.querySelector<HTMLSelectElement>('[aria-label="Choose playlist for track"]')?.focus();}}} /></label>
      {#if !playlistDestinations.length}<p role="status">No matching playlists. Create a new playlist below.</p>{/if}
      <label>Existing playlist
        <select aria-label="Choose playlist for track" bind:value={playlistDestinationId} onkeydown={(event)=>{if(event.key==="Enter"&&playlistDestinationId){event.preventDefault();void addArtistTrackToPlaylist();}}}>
          <option value="">Choose playlist</option>
          {#each playlistDestinations as collection}<option value={collection.id}>{collection.name} ({collection.trackCount})</option>{/each}
        </select>
      </label>
      <button class="primary" disabled={playlistWriteBusy || !playlistDestinations.some(destination => destination.id === playlistDestinationId)} onclick={() => void addArtistTrackToPlaylist()}>Add to playlist</button>
      {#if playlistWriteBusy}<p role="status">Adding to playlist…</p>{/if}
      {#if !playlistAlbumName}<label>Or create a new playlist<input disabled={playlistWriteBusy} aria-label="New playlist for track" bind:value={newCollection} placeholder="Playlist name" /></label>{/if}
      <footer><button onclick={closePlaylistPicker}>Cancel</button>{#if !playlistAlbumName}<button class="primary" disabled={playlistWriteBusy || !newCollection.trim()} onclick={() => void createPlaylistForArtistTrack()}>Create & add</button>{/if}</footer>
    </dialog>
  </div>{/if}
  {#if downloadTrack}<div class="download-backdrop">
      <dialog class="download-dialog download-sheet" use:activateDownloadDialog aria-labelledby="download-title" tabindex="-1">
        <header>
          <div><div class="eyebrow">DOWNLOAD REVIEW</div><h2 id="download-title">Download recording</h2>
            <p>{downloadTrack.artist} — {downloadTrack.title}</p></div>
          <button aria-label="Close download chooser" onclick={() => { downloadRequest++; downloadTrack = null; downloadPlaylistContext = null; downloadCandidates = []; downloadCandidate = null; }}>✕</button>
        </header>
        {#if downloadLoading}<p role="status">{downloadCandidate ? "Checking quality and additional sources…" : "Finding a source…"}</p>{/if}
        <DownloadSourceChooser candidates={downloadCandidates} selected={downloadCandidate} loading={downloadLoading} preferredProvider={preferredSource(settings)} providerOrder={downloadProviderOrder(preferredSource(settings),true,settings.downloadFallbackSources??["monochrome","slskd","youtube","soundcloud"])} existingFiles={downloadFiles} onSelect={(candidate)=>void inspectDownloadCandidate(candidate)} onPreferred={(assetId)=>downloadTrack&&void preferTrackFile(downloadTrack,assetId)} onReplace={(assetId)=>{const file=downloadFiles.find(item=>item.id===assetId);if(file)downloadDisposition={mode:"replace",targetAssetId:file.id,expectedHash:file.hash};}} />
        {#if downloadDisposition.mode==="replace"}<div class="download-replacement" role="status">Replace selected file after validation. A recoverable backup will be kept. <button onclick={()=>downloadDisposition={mode:"keep"}}>Keep existing files instead</button></div>{/if}
        {#if !downloadLoading && !downloadCandidates.length}<p>No source found. Try searching for this recording.</p>{/if}
        {#if downloadCandidate}
          {#if ["youtube", "soundcloud"].includes(downloadCandidate.provider)}
            <button type="button" aria-label={isCandidatePreviewCurrent(downloadCandidate, downloadTrack) && nativeSnapshot?.status !== "paused" ? "Pause selected source preview" : "Preview selected source"}
              onclick={() => downloadCandidate && void previewCandidate(downloadCandidate, downloadTrack)}>
              {isCandidatePreviewCurrent(downloadCandidate, downloadTrack) && nativeSnapshot?.status !== "paused" ? "Pause preview" : "Preview source"}
            </button>
          {/if}
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
          <button onclick={() => { downloadRequest++; downloadTrack = null; downloadPlaylistContext = null; downloadCandidates = []; downloadCandidate = null; }}>Cancel</button>
          <button class="primary" disabled={!downloadCandidate || downloadSubmitting || downloadCandidate.permission === "restricted" || (splitChapters && !focusedDownloadChapterId && !selectedChapterIds.length)} onclick={() => void queueDownloadChoice()}>Download</button>
        </footer>
      </dialog>
    </div>{/if}
  {#if showDownloads}<DownloadActivity onViewAll={() => { openPlayerPanel(null); void navigate("downloads"); }} onFindSource={(job) => void openDownloadChooser(job.track)} tagStatuses={downloadTagStatuses} onReviewTags={(id)=>void openAutotagReview(id)} jobs={jobs.filter(job=>!(job.track.provenance as Record<string,unknown>)?.deferredSoulseekWatchId)} onClose={() => (showDownloads = false)} onRefresh={() => void refreshJobs()} onCancel={(jobId) => void run(async () => { await call("cancel_download", {jobId}); await refreshJobs(); })} onRetry={(jobId) => void run(async () => { await call("retry_download", {jobId}); await refreshJobs(); })} onShowFile={(path) => void run(() => call("open_in_file_manager", {path}))} />{/if}
  {#if showPlayingInfo || showQueue}<section class="player-panel-host" class:fullscreen-queue={showFullNowPlaying && showQueue} aria-label="Player panel">{#if showQueue}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (Keyboard and pointer splitter.) -->
  <div class="queue-panel-resize" role="separator" tabindex="0" aria-label="Resize queue panel" aria-orientation="vertical" aria-valuemin="280" aria-valuemax="420" aria-valuenow={infoPanelWidth} onpointerdown={resizeQueuePanel} onkeydown={(event)=>{if(event.key==="ArrowLeft"||event.key==="ArrowRight"){event.preventDefault();saveLayoutWidth("info",infoPanelWidth+(event.key==="ArrowLeft"?10:-10));}}}></div>{/if}<nav class="player-panel-tabs"><button class:active={showPlayingInfo} onclick={()=>openPlayerPanel("info")}>Now playing</button><button class:active={showQueue} onclick={()=>openPlayerPanel("queue")}>Queue</button><button aria-label="Close player panel" onclick={()=>openPlayerPanel(null)}>×</button></nav>
  {#if showPlayingInfo && (infoTrack || playingTrack)}{@const shownTrack = infoTrack || playingTrack!}{@const shownActionTrack = shownTrack.id===playingTrack?.id ? playbackActionTrack??shownTrack : shownTrack}<PlayingInfo loadRecording={recordingInformation.load} onRecordingRelease={(release,artist) => void browseRecordingRelease(release,artist)} loadArtist={loadNowPlayingArtist} onArtistRef={(artist) => {openPlayerPanel(null);void discoverArtistRef(artist);}} onAlbum={trackAlbumTarget(shownActionTrack) ? () => {openPlayerPanel(null);void openTrackAlbum(shownActionTrack);} : undefined} stationName={shownTrack.id===playingTrack?.id ? playingRadioStation?.name ?? "" : ""} stationUrl={shownTrack.id===playingTrack?.id ? playingRadioStation?.homepageUrl ?? playingRadioStation?.sourceUrl : undefined} liveBpm={shownTrack.id===playingTrack?.id && !paused ? liveBpmEstimate : null} liveKey={shownTrack.id===playingTrack?.id && !paused ? liveKeyEstimate : null} lastLiveBpm={shownTrack.id===playingTrack?.id ? lastLiveBpm : null} lastLiveKey={shownTrack.id===playingTrack?.id ? lastLiveKey : null} liveBpmStatus={shownTrack.id===playingTrack?.id ? settings.liveBpmEnabled !== true ? "disabled" : paused ? "paused" : liveBpmPollStatus : undefined} liveBpmStatusReason={shownTrack.id===playingTrack?.id ? liveBpmPollReason : null} artworkDisplayUrl={artworkDisplayUrls[shownTrack.artworkUrl ?? ""]} onArtworkError={(track,failedUrl)=>void recoverDisplayArtwork(track,failedUrl)} onFavorite={()=>void favorite(shownTrack.id===playingTrack?.id?(playbackActionTrack??shownTrack):shownTrack)} favorite={(shownTrack.id===playingTrack?.id?(playbackActionTrack??shownTrack):shownTrack).favorite} docked={true} activeChapter={shownTrack.id===playingTrack?.id ? activeChapter?.title : undefined} panelWidth={infoPanelWidth} onPanelWidthChange={(width)=>saveLayoutWidth("info",width)} onPlay={()=>{if(shownTrack.id===playingTrack?.id)void toggleCurrentPlayback();else void startPreview(shownTrack);}} onOpenMpv={mpvAvailable?()=>void openTrackInMpv(shownTrack):undefined} onIdentifyRadio={shownTrack.id===playingTrack?.id&&(playingRadioStation||true||preview?.sourceGeneration!==undefined)?()=>void (radioRecognizing?cancelRadioIdentification():identifyPlayingTrack()):undefined} onCancelIdentifyRadio={!playingRadioStation ? () => cancelRadioIdentification() : undefined} identifyProgress={radioIdentifyProgress} identifyingRadio={radioRecognizing} loudnessStatus={shownTrack.id === playingTrack?.id ? (nativeSnapshot as (NativePlaybackSnapshot & {
    loudnessNormalization?: LoudnessPlaybackStatus;
}) | null)?.loudnessNormalization ?? null : null} loudnessMode={loudnessSettings.mode} loudnessAnalysisJob={loudnessAnalysisJob} pendingLoudnessAnalysisCount={pendingLoudnessAssetIds.length} loudnessAnalysisMessage={loudnessAnalysisMessage} onAnalyzeLoudness={trackAsset(shownActionTrack) || shownActionTrack.localPath ? () => void analyzeLoudnessTracks([shownActionTrack]) : undefined} onCancelLoudnessAnalysis={cancelLoudnessAnalysis} onRetryLoudnessAnalysis={retryQueuedLoudnessAnalysis} analysisEstimate={shownTrack.id===playingTrack?.id ? currentPlaybackAnalysisEstimate : null} analysisStatus={shownTrack.id===playingTrack?.id ? currentLocalAnalysisStatus || streamMediaAnalysisStatus : ""} trackDetectionView={shownTrack.id===playingTrack?.id && currentPlayingTrackDetectionOwner()!==null ? playingTrackDetectionView : savedDetectionViewsByTrack[shownTrack.id] ?? null} trackDetectionStatus={shownTrack.id===playingTrack?.id ? playingTrackDetectionStatus : ""} trackDetectionDisabledReason={shownTrack.id===playingTrack?.id ? playingTrackDetectionDisabledReason : ""} trackDetectionAvailable={desktop() && shownTrack.id===playingTrack?.id && currentPlayingTrackDetectionOwner()!==null} onTrackDetectionToggle={togglePlayingTrackDetection} trackDetectionKeepEnabledForQueue={keepTrackDetectionForQueue} onTrackDetectionKeepEnabledForQueueToggle={(enabled)=>keepTrackDetectionForQueue=enabled} trackDetectionMarkersEnabled={detectionMarkersEnabled} onTrackDetectionMarkersToggle={(enabled)=>detectionMarkersEnabled=enabled} onTrackDetectionSeek={(seconds)=>{if(shownTrack.id===playingTrack?.id)seekChapter(seconds);else void (async()=>{await startPreview(shownTrack);if(playingTrack?.id===shownTrack.id)seekChapter(seconds);})();}} onTrackDetectionOpen={(track)=>void startPreview(track)} onTrackDetectionAddToPlaylist={(track)=>void addArtistTrack(track,false)} onTrackDetectionsAddToPlaylist={(tracks)=>void addTracksToPlaylist(tracks)} track={shownTrack} trackLibrarySummary={trackLibrarySummaryTrackId === shownTrack.id ? trackLibrarySummary : null} trackLibrarySummaryLoading={trackLibrarySummaryLoading && trackLibrarySummaryTrackId === shownTrack.id} trackLibrarySummaryRequested={trackLibrarySummaryTrackId === shownTrack.id} asset={trackAsset(shownTrack)} availableFiles={filesForTrack(shownTrack)} preferredAssetId={shownTrack.assetId??null} onPreferredFile={(assetId)=>void preferTrackFile(shownTrack,assetId)} streamQuality={shownTrack.id === playingTrack?.id && ((preview?.kind === "native" ? nativeSnapshot?.quality?.sourceKind !== "localAsset" : preview?.kind === "audio" || !playingTrack?.localPath)) ? { codec: nativeSnapshot?.quality?.codec ?? preview?.audioCodec, bitrateKbps: nativeSnapshot?.quality?.bitrateKbps ?? preview?.audioBitrateKbps, sampleRate: nativeSnapshot?.quality?.sampleRate, channels: nativeSnapshot?.quality?.channels, bitrateOrigin: nativeSnapshot?.quality?.bitrateOrigin, live: Boolean(playingRadioStation) } : undefined} onUpgrade={()=>void openDownloadChooser(shownActionTrack)} onRadio={() => { showPlayingInfo = false; void startSongRadio(shownActionTrack); }} radioBusy={radioBusy === shownTrack.id} candidate={shownTrack.id === playingTrack?.id ? playingCandidate : null} onClose={() => { showPlayingInfo = false; infoTrack = null; }} onArtist={() => { showPlayingInfo = false; void discoverArtist(shownActionTrack); }} onDownload={() => { showPlayingInfo = false; void openDownloadChooser(shownActionTrack); }} onPlaylist={() => { showPlayingInfo = false; void addArtistTrack(shownActionTrack); }} onHistoryOccurrence={(sessionId)=>void openHistoryOccurrence(sessionId)} onPlaylistMembership={(playlistId,trackId)=>void openPlaylistMembership(playlistId,trackId)} onMoreInfo={() => searchMoreInfo(shownTrack,shownTrack.id===playingTrack?.id?playingCandidate:null,shownTrack.id===playingTrack?.id?activeChapter?.title:undefined)} />{/if}
  {#if showQueue}<PlaybackQueuePanel {favoriteTrackIds} onVisibleTrack={loadSavedTrackDetections} {detectionsByTrack} onSeekDetection={(parent,observation,index)=>{void (async()=>{if(parent.id!==playingTrack?.id)await startPreview(parent,true,false,queueTracks,index);if(playingTrack?.id===parent.id)seekChapter(observation.detectedAtSeconds);})();}} onInfoDetection={(track)=>void startPreview(track)} onAddDetectionToPlaylist={(track)=>void addArtistTrack(track,false)} onAddDetectionsToPlaylist={(tracks)=>void addTracksToPlaylist(tracks)} queueRevision={nativeSnapshot?.queueRevision} onInteractionMove={queueUiMove??undefined} interactionSignal={queueUiController?.signal} paused={paused} buffering={playbackLoading} activePlaybackTrack={playbackActionTrack} {chapterRangesByTrack} radioNowPlaying={currentRadioSong} radioStationName={playingRadioStation?.name??""} radioHistory={previousRadioQueue} {radioRecognizing} radioIdentifyProgress={radioIdentifyProgress} onIdentifyRadio={playingRadioStation?()=>void (radioRecognizing?cancelRadioIdentification():identifyPlayingRadio()):undefined} onCancelIdentifyRadio={undefined} onIdentifyCurrent={playingTrack && !playingRadioStation ? () => void (radioRecognizing ? cancelRadioIdentification() : identifyPlayingTrack()) : undefined} onCancelIdentifyCurrent={()=>cancelRadioIdentification()} identifyingCurrentAudio={radioRecognizing} onPlayRadioTrack={(track)=>void startPreview(track)} onDownloadRadioTrack={(track)=>void openDownloadChooser(track)} onFavoriteRadioTrack={(track)=>void favorite(track)} onArtistRadioTrack={(track)=>void discoverArtist(track)} onInfoRadioTrack={(track)=>openPlayerPanel("info",track)} onContextRadioTrack={(track)=>{actionTrack=track;actionAnchor=null;}} onInfoTrack={(track)=>openPlayerPanel("info",track)} onAddToPlaylist={(track)=>void addArtistTrack(track,false)} {chaptersByTrack} onPlayChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)playChapter(parent,candidate,chapter);}} onQueueChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)queueChapter(parent,candidate,chapter,"append");}} onAddChapterToPlaylist={(parent,chapter)=>void addChapterToPlaylist(parent,chapter)} onDownloadChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)void downloadChapter(parent,candidate,chapter);}} onInfoChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate){const virtual=virtualChapterTrack(parent,candidate,chapter);if(virtual)searchMoreInfo(virtual,candidate,chapter.title);}}} onQueueAllChapters={(parent,chapters)=>queueAllChapters(parent,chapters)} onInspectChapters={(parent)=>void inspectQueueChapters(parent)} tracks={queueTracks} currentIndex={queueIndex} chapters={playingCandidate?.chapters ?? []} {position} chapterPosition={playingTrack?.playbackRange ? chapterAbsolutePosition(playingTrack, position) : position} {repeatMode} {shuffleEnabled} onClose={() => openPlayerPanel(null)} onPlay={playQueueIndex} onMove={moveQueueItem} onSortUpcoming={sortUpcoming} onRemove={removeQueueItem} onRemoveMany={removeQueueItems} onSeek={seekChapter} onToggleShuffle={toggleShuffle} onCycleRepeat={cycleRepeat} />{/if}
  </section>{/if}
  {#if spotifyTrackArtistChoices.length}<div class="modal-backdrop"><dialog use:activateDownloadDialog={() => spotifyTrackArtistChoices = []} tabindex="-1" class="download-dialog playlist-picker-dialog spotify-track-artist-chooser" aria-label="Choose Spotify artist"><h2>Choose Spotify artist</h2><p>This track credits multiple Spotify artists.</p>{#each spotifyTrackArtistChoices as artist (artist.id)}<button type="button" aria-label={`Open Spotify artist ${artist.name}`} onclick={() => void openSpotifyTrackArtist(artist)}>{artist.name}</button>{/each}<button type="button" onclick={() => spotifyTrackArtistChoices = []}>Cancel</button></dialog></div>{/if}
  {#if showReleaseSetup}<dialog open class="download-dialog release-onboarding" aria-label="Welcome to Burrata"><ReleaseSetup onboarding onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings={...value,matchingLevel:value.matchingLevel??"balanced",playbackStartupTimeoutSeconds:value.playbackStartupTimeoutSeconds??20})} onComplete={()=>showReleaseSetup=false}/></dialog>{/if}
  {#if renameTarget}<dialog class="download-dialog playlist-picker-dialog" use:activateRenameDialog aria-label={`Rename ${renameTarget.kind}`} tabindex="-1"><form onsubmit={(event)=>{event.preventDefault();void saveSidebarRename();}}><h2>Rename {renameTarget.kind==="radio"?"station":"playlist"}</h2><label>Name<input aria-label="New name" maxlength="160" bind:value={sidebarRenameValue}/></label><footer><button type="button" onclick={()=>renameTarget=null}>Cancel</button><button class="primary" disabled={!sidebarRenameValue.trim()}>Save</button></footer></form></dialog>{/if}
  {#if radioActions}{@const menu=radioActions}<PlaylistMenu onDelete={()=>void removeSidebarStation(menu.station)} kind="radio station" name={menu.station.name} anchor={menu.anchor} {playerHeight} onRename={()=>startSidebarRename("radio",menu.station.id,menu.station.name)} onOpen={()=>{selectedRadioStationId=menu.station.id;void navigate("radio");}} onOpenSource={()=>void openSourceToRefresh(menu.station.homepageUrl??menu.station.sourceUrl)} onClose={()=>radioActions=null}/>{/if}
  {#if playlistActions}{@const menu = playlistActions}<PlaylistMenu onRefresh={remotePlaylistBindings.some(binding=>binding.collectionId===menu.collection.id||binding.sourceCollectionId===menu.collection.id)?()=>void refreshLinkedPlaylist(menu.collection.id):undefined} onExport={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"?()=>{portablePlaylistInitialMode="export";if(menu.collection.id==="favorites")portablePlaylistTarget={id:"favorites",name:menu.collection.name,trackCount:favoriteTrackIds.length};else portablePlaylistTarget=menu.collection;}:menu.collection.id==="history"?()=>showPortableHistory=true:undefined} onMoveToFolder={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"?()=>{portablePlaylistInitialMode="folder";if(menu.collection.id==="favorites")portablePlaylistTarget={id:"favorites",name:menu.collection.name,trackCount:favoriteTrackIds.length};else portablePlaylistTarget=menu.collection;}:undefined} onCopy={menu.collection.id==="favorites"||menu.collection.id===playlistPreferences.favoritesPlaylistId?()=>void copyFavoritesPlaylist():activeBrowserImport?.collectionId===menu.collection.id?()=>void run(copyCurrentImport):undefined} onRename={()=>startSidebarRename("playlist",menu.collection.id,menu.collection.name)} onDownload={collections.some(item=>item.id===menu.collection.id)?()=>void downloadPlaylist(menu.collection.id):undefined} onOpenSource={(sourceUrlForCollection(menu.collection.id) || (menu.collection.id===chart?.id?currentChartSourceUrl:null)) ? () => void openSourceToRefresh((sourceUrlForCollection(menu.collection.id) || currentChartSourceUrl)!) : undefined} name={menu.collection.name} anchor={menu.anchor} {playerHeight} onClose={() => (playlistActions = null)} onOpen={() => void openPlaylist(menu.collection.id)} onMirrorSettings={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"||menu.collection.id==="history"?() => playlistMirrorSettingsTarget = { collectionId: menu.collection.id, name: menu.collection.name }:undefined} onDelete={menu.collection.id!==DOWNLOADS_PLAYLIST_ID && collections.some(item=>item.id===menu.collection.id)?()=>void deletePlaylist(menu.collection):undefined} />{/if}
  {#if actionTrack}{@const menuTrack = actionTrack}{@const albumTarget = trackAlbumTarget(menuTrack)}<TrackMenu track={menuTrack} onRadio={() => void startSongRadio(menuTrack)} radioBusy={radioBusy === menuTrack.id} anchor={actionAnchor} {playerHeight} onRemove={view === "collections" ? () => void removePlaylistTrack(menuTrack) : undefined} onClose={() => (actionTrack = null)} onNext={() => queueArtistTracks([menuTrack], "next")} onQueue={() => queueArtistTracks([menuTrack], "append")} onPlaylist={() => void addArtistTrack(menuTrack)} onGoToAlbum={albumTarget ? () => void openTrackAlbum(menuTrack) : undefined} onGoToAlbumUnavailable={!albumTarget} onFavorite={() => void favorite(menuTrack)} onDownload={() => void openDownloadChooser(menuTrack, undefined, undefined, currentSavedPlaylistContext())} onUpgrade={()=>void openDownloadChooser(menuTrack)} onMoreInfo={() => searchMoreInfo(menuTrack,importedAudioCandidate(menuTrack))} onArtist={() => void discoverArtist(menuTrack)} onOpenMpv={mpvAvailable?()=>void openTrackInMpv(menuTrack):undefined} onDetails={() => openPlayerPanel("info", menuTrack)} onSelect={() => { if (view === "collections") localPlaylistSelectionMode = true; else batchSelectionMode = true; if (!checked.includes(menuTrack.id)) checked = [...checked, menuTrack.id]; }} />{/if}
  {#if infoSearchTarget}<TrackInfoSearch track={infoSearchTarget.track} candidate={infoSearchTarget.candidate} chapterTitle={infoSearchTarget.chapterTitle} onClose={() => (infoSearchTarget=null)} onCatalogSearch={openCatalogInfoSearch} onWebSearch={openWebInfoSearch} onOpenSource={openInfoSource} />{/if}
  {#if showFullNowPlaying && playingTrack}
    <NowPlayingView onSeek={seekChapter} track={expandedPlayingTrack ?? playingTrack} lyricsTrack={currentLyricsTrack ?? playingTrack} absolutePosition={position} loadLyrics={()=>call("get_current_track_lyrics",{visibleTrack:currentVisibleLyricsTrack()})} findLyrics={()=>call("search_current_track_lyric_candidates",{visibleTrack:currentVisibleLyricsTrack()})} chooseLyrics={(scopeId,candidateId)=>call("select_current_track_lyric_candidate",{visibleTrack:currentVisibleLyricsTrack(),scopeId,candidateId})} saveLyrics={async(sourceHash)=>{const result=await call("save_current_track_lyrics_to_file",{visibleTrack:currentVisibleLyricsTrack(),sourceHash});await refreshCurrentAssetFiles();return result;}} exportLyrics={(sourceHash,text)=>call("export_current_track_lyrics_text",{visibleTrack:currentVisibleLyricsTrack(),sourceHash,text})} translationLanguage={lyricsTranslationLanguage} changeTranslationLanguage={changeLyricsTranslationLanguage} translationEnabled={lyricsTranslationEnabled} translationCredentialPresent={lyricsTranslationCredentialPresent} translateLyrics={translateCurrentTrackLyrics} syncLyrics={playingTrack && currentLyricsTrack?.id === playingTrack.id && nativeSnapshot?.status === "playing" && !paused && !playingRadioStation && !playingTrack.playbackRange && !nativeSnapshot?.chapter ? () => syncLyricsWithAudio(playingTrack!.id) : undefined} syncScope={JSON.stringify([playingTrack?.id??null,nativeSnapshot?.generation??null,seekSerial,playbackQueue.entryIds[queueIndex]??null])} title={activeRecordingMatch?.assessment.relationship === "alternative" ? activeRecordingMatch.resolvedRecording.title : playingTitle}
      artworkUrl={artworkDisplayUrls[(expandedPlayingTrack ?? playingTrack).artworkUrl ?? ""]} sourceArtworkUrl={originalArtworkSource((expandedPlayingTrack ?? playingTrack).artworkUrl)} detectedArtworkUrl={activeDetectedArtwork} artworkMode={fullscreenArtworkMode} onArtworkModeChange={(mode)=>fullscreenArtworkMode=mode} bottom={playerHeight}
      queueOpen={showQueue} nextTrack={queueTracks[queueIndex + 1] ?? (repeatMode === "all" ? queueTracks[0] : null) ?? null} context={playingRadioStationName || "Now playing"}
      loadRecording={recordingInformation.load} onRecordingRelease={(release,artist) => void browseRecordingRelease(release,artist)} loadArtist={loadNowPlayingArtist} onArtistRef={(artist) => {showFullNowPlaying=false;void discoverArtistRef(artist);}} onClose={() => showFullNowPlaying = false}
      onArtist={() => { showFullNowPlaying = false; const track = expandedPlayingTrack ?? playingTrack!; void discoverArtist(activeRecordingMatch?.assessment.relationship === "alternative" ? {...track,provider:"unknown",providerId:""} : track); }}
      onAlbum={activeRecordingMatch?.assessment.relationship !== "alternative" && trackAlbumTarget(playbackActionTrack ?? playingTrack) ? () => { showFullNowPlaying = false; void openTrackAlbum(playbackActionTrack ?? playingTrack!); } : undefined}
      onQueue={() => openPlayerPanel("queue")} onFavorite={() => void favorite(playbackActionTrack ?? playingTrack!)} />
  {/if}
  {#if showFullscreenLyrics && playingTrack}
    <LyricsPanel bottomInset={playerHeight} onSeek={seekChapter} track={currentLyricsTrack ?? playingTrack} absolutePosition={position} fullscreenRequested={true} onFullscreenClose={()=>{showFullscreenLyrics=false;void tick().then(()=>lyricsButton?.focus());}}
      loadLyrics={()=>call("get_current_track_lyrics",{visibleTrack:currentVisibleLyricsTrack()})} findLyrics={()=>call("search_current_track_lyric_candidates",{visibleTrack:currentVisibleLyricsTrack()})} chooseLyrics={(scopeId,candidateId)=>call("select_current_track_lyric_candidate",{visibleTrack:currentVisibleLyricsTrack(),scopeId,candidateId})} saveLyrics={async(sourceHash)=>{const result=await call("save_current_track_lyrics_to_file",{visibleTrack:currentVisibleLyricsTrack(),sourceHash});await refreshCurrentAssetFiles();return result;}} exportLyrics={(sourceHash,text)=>call("export_current_track_lyrics_text",{visibleTrack:currentVisibleLyricsTrack(),sourceHash,text})} translationLanguage={lyricsTranslationLanguage} changeTranslationLanguage={changeLyricsTranslationLanguage}
      translationEnabled={lyricsTranslationEnabled} translationCredentialPresent={lyricsTranslationCredentialPresent} translateLyrics={translateCurrentTrackLyrics} />
  {/if}
  <footer id="burrata-playback-controls" aria-label="Playback controls" class="player" class:compact={settings.compactPlaybackBar === true} class:radio-live={liveRadioPlayback} bind:offsetHeight={playerHeight}>
    <div class="now-playing">
      <button class="now-playing-open click-info" aria-label="Show playing track information" disabled={!playingTrack} onclick={() => { infoTrack = null; openPlayerPanel("info"); }}>
      <div class="mini-art">
        {#if activeDetectedArtwork && detectedArtworkMode === "detected"}<img src={activeDetectedArtwork} alt="Detected track artwork" />{:else if playingTrack?.artworkUrl && (artworkDisplayUrls[playingTrack.artworkUrl] || !failedArtwork[playingTrack.artworkUrl])}{@const artworkSource = playingTrack.artworkUrl}{#key artworkSource}<img
          src={artworkDisplayUrls[artworkSource] ?? artworkSource}
          onerror={(event) => { if (event.currentTarget instanceof HTMLImageElement && event.currentTarget.isConnected && playingTrack && !event.currentTarget.src.startsWith("data:")) void recoverDisplayArtwork(playingTrack, artworkSource); }}
          alt=""
        />{/key}{:else}♫{/if}
      </div>
      <div class="player-track-copy">
        <strong class="player-track-title">{activeRecordingMatch?.assessment.relationship === "alternative" ? activeRecordingMatch.resolvedRecording.title : playingTitle}</strong>
        <small class="player-track-artist">{activeRecordingMatch?.assessment.relationship === "alternative" ? activeRecordingMatch.resolvedRecording.artist ?? "Alternative performer" : playingTrack?.playbackRange?.parentTitle ?? (activeChapter ? playingTrack?.title : playingTrack?.artist) ?? (playingTrack ? "Artist unknown" : "Select a recording")}</small>
        <AlternativeSourceNotice matching={activeRecordingMatch} requestedTitle={activeRecordingMatch?.requestedRecording?.title ?? playingTrack?.title} requestedArtist={activeRecordingMatch?.requestedRecording?.artist ?? playingTrack?.artist} />
        <small class="player-status" role={liveRadioPlayback ? undefined : "status"} title={previewStatus}>
          {#if liveRadioPlayback}
            <span class="radio-live-meta"><span class="time">{time(playerPosition)}</span><span class="radio-live-label">Live radio</span></span>
            <span class="radio-status-copy" title={playingRadioStationName} aria-label={`Station: ${playingRadioStationName}. ${playerStatusMessage}`}>{playingRadioStationName}</span>
          {:else}{playerStatusMessage}{/if}
        </small>
        {#if playingTrack}<PlaybackQuality compact track={playingTrack} asset={nativeSnapshot?.quality?.sourceKind === "localAsset" || preview?.kind === "local" || Boolean(playingTrack?.localPath) ? trackAsset(playingTrack) : null} streamQuality={(preview?.kind === "native" ? nativeSnapshot?.quality?.sourceKind !== "localAsset" : preview?.kind === "audio" || !playingTrack?.localPath) ? { codec: nativeSnapshot?.quality?.codec ?? preview?.audioCodec, bitrateKbps: nativeSnapshot?.quality?.bitrateKbps ?? preview?.audioBitrateKbps, sampleRate: nativeSnapshot?.quality?.sampleRate, channels: nativeSnapshot?.quality?.channels, bitrateOrigin: nativeSnapshot?.quality?.bitrateOrigin, live: Boolean(playingRadioStation) } : undefined} />{/if}
        {#if playerAudioSource && !playingRadioStation}<small class="player-source" title={preview?.kind === "local" ? playingTrack?.localPath ?? playerAudioSource : playerAudioSource}>{playerAudioSource}</small>{/if}
      </div>
      </button>
      {#if playingRadioStation && playingTrack}<StationInfo compact track={playingTrack} stationName={playingRadioStation.name} stationUrl={playingRadioStation.homepageUrl ?? playingRadioStation.sourceUrl} streamUrl={playingRadioStation.streamUrl} />{/if}
    </div>

    {#if playerErrorMessage}
      <div class="player-error" role="alert" aria-live="assertive">
        <p>{playingTrack ? `Could not play ${playingTrack.title}.` : "Playback unavailable."}</p>
        <div class="player-error-actions">
          <InfoPopover placement="top" label="Playback error details">{playerErrorMessage}</InfoPopover>
          <button type="button" aria-label="Retry playback" onclick={() => { if (playingTrack) void startPreview(playingTrack, true, false, queueTracks, queueIndex); }}>Retry</button>
          <button type="button" aria-label="Open playing track information" onclick={() => { infoTrack = null; openPlayerPanel("info"); }}>Track info</button>
        </div>
      </div>
    {/if}

    <div class="player-center">
      <div class="player-transport">
        <button class="transport-mode" class:active={shuffleEnabled} title={shuffleEnabled ? "Shuffle: on. Turn off shuffle." : "Shuffle: off. Turn on shuffle."} aria-label="Shuffle" aria-pressed={shuffleEnabled} onclick={toggleShuffle}>⤨</button>
        <button class="transport-mode" aria-label="Autoplay radio after queue" title={autoQueueRadio ? "Autoplay radio: on. Turn off related tracks after the queue." : "Autoplay radio: off. Turn on related tracks after the queue."} aria-pressed={autoQueueRadio} class:active={autoQueueRadio} onclick={()=>autoQueueRadio=!autoQueueRadio}>∞ Radio</button>
        <button title="Previous recording" aria-label="Previous recording" onclick={() => { void advanceQueue(-1); }}>⏮</button>
        <button class="play-button" class:buffering={playbackLoading} aria-busy={playbackLoading} aria-label={playbackLoading ? "Pause loading playback" : paused ? "Play" : "Pause"} title={playbackLoading ? "Loading audio · click to pause" : paused ? "Play" : "Pause"} onclick={toggleCurrentPlayback}>
          <span class="play-button-icon" aria-hidden="true">{playbackLoading ? "Ⅱ" : paused ? "▶" : "Ⅱ"}</span>
          {#if playbackLoading}<span class="play-button-loading-ring" aria-hidden="true"></span>{/if}
        </button>
        <button title="Next recording" aria-label="Next recording" onclick={() => { void advanceQueue(1); }}>⏭</button>
        <button class="transport-mode" class:active={repeatMode !== "off"} title={repeatMode === "off" ? "Repeat: off. Repeat the queue." : repeatMode === "all" ? "Repeat: queue. Switch to repeat one." : "Repeat: one. Turn off repeat."} aria-label={`Repeat: ${repeatMode}`} aria-pressed={repeatMode !== "off"} onclick={cycleRepeat}><span aria-hidden="true">↻</span>{#if repeatMode === "one"}<span class="repeat-one-badge" aria-hidden="true">1</span>{/if}</button>
      </div>

      {#if !liveRadioPlayback}<div class="player-scrubber">
        <span class="time">{time(playerPosition)}</span>
        {#if waveformEnabled}
          <WaveformSeek position={playerPosition} duration={playerDuration} playing={!paused && !bufferingAudio} playbackIdentity={`${playingTrack?.id}:${nativeSnapshot?.generation}:${seekSerial}:${nativeSnapshot?.seekSerial}`}
            disabled={!nativeSnapshot?.canSeek}
            generation={waveformGeneration} trackId={waveformTrackId} waveform={playbackWaveformForDisplay}
            detections={playbackDetectionMarkers} detectionSourceOffset={playbackDetectionMarkerSourceOffset}
            onSeek={(value) => void seekPlayer(value)} />
        {:else}
          <SeekSlider position={playerPosition} duration={playerDuration} playing={!paused && !bufferingAudio} playbackIdentity={`${playingTrack?.id}:${nativeSnapshot?.generation}:${seekSerial}:${nativeSnapshot?.seekSerial}`}
            disabled={!nativeSnapshot?.canSeek}
            detections={playbackDetectionMarkers} detectionSourceOffset={playbackDetectionMarkerSourceOffset}
            onSeek={(value) => void seekPlayer(value)} />
        {/if}
        <span class="time">{time(playerDuration)}</span>
      </div>{/if}
    </div>

    <div class="player-right">
      {#if activeDetectedArtwork}<details class="player-artwork-menu" use:dismissDetails>
        <summary aria-label="Playbar artwork options" title="Artwork display">▧</summary>
        <div class="player-artwork-options" role="group" aria-label="Playbar artwork">
          <button type="button" aria-pressed={detectedArtworkMode === "set"} onclick={(event)=>{detectedArtworkMode="set";event.currentTarget.closest("details")?.removeAttribute("open");}}>Set artwork</button>
          <button type="button" aria-pressed={detectedArtworkMode === "detected"} onclick={(event)=>{detectedArtworkMode="detected";event.currentTarget.closest("details")?.removeAttribute("open");}}>Detected track artwork</button>
        </div>
      </details>{/if}
      {#if !liveRadioPlayback}<button aria-label={completeStreamWaveformVisible ? "Waveform seek bar, full verified stream waveform" : "Waveform seek bar"} title={`${waveformEnabled ? "Disable" : "Enable"} waveform seek bar${waveformRouteUpgradeStatus ? ` · ${waveformRouteUpgradeStatus}` : playbackWaveformReason ? ` · ${playbackWaveformReason}` : ""}`} aria-pressed={waveformEnabled} disabled={!playingTrack} onclick={toggleWaveformSeek}>〰</button>{/if}
      <button aria-label="Full-screen Now Playing" title="Full-screen Now Playing" aria-pressed={showFullNowPlaying} disabled={!playingTrack} onclick={() => { const open = !showFullNowPlaying; openPlayerPanel(null); showFullNowPlaying = open; }}>⤢</button>
      <button class="player-heart" aria-label={playbackActionTrack?.favorite?"Remove playing track from favorites":"Add playing track to favorites"} aria-pressed={playbackActionTrack?.favorite??false} disabled={!playbackActionTrack || Boolean(liveRadioPlayback&&!currentRadioSong)} title={playbackActionTrack?.favorite ? "Remove playing track from favorites" : "Add playing track to favorites"} onclick={()=>{const track=playbackActionTrack;if(track)void favorite(track);}}>{playbackActionTrack?.favorite?"♥":"♡"}</button>
      <button class="queue-toggle" aria-label="Show playback queue and chapters" title="Queue and chapters" aria-pressed={showQueue} onclick={() => openPlayerPanel(showQueue ? null : "queue")}><svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 5h16M3 10h16M3 15h8"/><path d="m16 14 5 4-5 4z"/></svg><span>Queue</span></button>
      <label class="player-volume-control" title="Volume">
        <span aria-hidden="true">◖))</span>
        <input
          class="volume"
          type="range"
          min="0"
          max="1"
          step="0.01"
          bind:value={volume}
          disabled={!nativeSnapshot?.canVolume}
          aria-label="Volume"
          oninput={(e) => {
            const value = Number(e.currentTarget.value);
            if (true) void nativeCommand({action:"volume",volume:value});

          }}
        />
      </label>
      <button bind:this={lyricsButton} title="Fullscreen lyrics" aria-label="Open fullscreen lyrics" disabled={!playingTrack} onclick={()=>showFullscreenLyrics=true}><svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10v2a7 7 0 0 0 14 0v-2M12 19v3M8 22h8"/></svg></button>
      <button title="Track details" aria-label="Show track details" onclick={() => openPlayerPanel("info")} disabled={!playingTrack}>ⓘ</button>
    </div>
  </footer>
</div>

{#if localAlbumLoudnessReview}
  <LazyView loader={loadLocalAlbumLoudnessReview} label="album loudness review" componentProps={{
    light: settings.theme === "light" || settings.theme === "system" && systemLight,
    tracks: localAlbumLoudnessReview.tracks,
    album: localAlbumLoudnessReview.album,
    albumArtist: localAlbumLoudnessReview.albumArtist,
    canReorder: localAlbumLoudnessReview.canReorder,
    focusReturn: localAlbumLoudnessFocusReturn,
    onAnalyze: (orderedTracks: Track[]) => prepareReleaseLoudnessAnalysis(orderedTracks, 0, true),
    onPlay: (orderedTracks: Track[], resource: LoudnessResourceIdentity) => playReleaseAlbum(orderedTracks, resource),
    onQueue: (orderedTracks: Track[], resource: LoudnessResourceIdentity) => queueArtistTracks(orderedTracks, "append", resource),
    onClose: () => { localAlbumLoudnessReview = null; localAlbumLoudnessFocusReturn = null; },
  }} />
{/if}

<LibraryCleanup open={showLibraryCleanup} {call} onClose={() => showLibraryCleanup=false} onRemoved={() => requestLocalPrefixRefresh(undefined,true)} />
