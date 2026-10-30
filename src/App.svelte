<script lang="ts">
  import { dismissDetails } from "./lib/dismiss-details";
  import { HEALTHY_PLAYBACK_RESET_MS, emptyRecurringStallRecoveryState, recordPlaybackStall, type RecurringStallRecoveryState } from "./lib/recurring-stall-recovery";
  import { hasExplicitSourceLock } from "./lib/source-lock";
  import PlaybackDiagnosticsSettings from "./PlaybackDiagnosticsSettings.svelte";
  import LibraryCleanup from "./LibraryCleanup.svelte";
  import BackupRestore, { type BackupInspection, type BackupMediaEstimate, type BackupMediaRootPreview, type BackupProgress, type BackupSchedule, type SettingsImportPreview, type SettingsImportSelection } from "./BackupRestore.svelte";
  import RestoreWorkReview from "./RestoreWorkReview.svelte";
  import ImportHub, { type ImportChoice } from "./ImportHub.svelte";
  import ImportSources from "./ImportSources.svelte";
  import {
    applyBackupFrontendPreferences,
    type BackupFrontendPreferenceAck,
    type PendingBackupFrontendPreferences,
  } from "./lib/backup-frontend-preferences";
  import { respondToBackupCheckpoint, type BackupCheckpointRequest } from "./lib/backup-checkpoint";
  import { startRestoreAndWaitForCompletion } from "./lib/restore-job";
  let showLibraryCleanup = $state(false);
  import { WebviewNextCandidatePrefetch } from "./lib/webview-next-candidate-prefetch";
  import {dispatchQueueUi,queueUiMoves,type QueueUiAction,type QueueUiResult} from './lib/queue-ui-automation';
  import { createWebviewMediaEventRecorder } from "./lib/webview-media-events";
  import { WebviewBufferingRecovery } from "./lib/webview-buffering-recovery";
  import { WebviewStallWatchdog, type WebviewStallOwner } from "./webview-stall-watchdog";
  import { WebviewPostSeekProgress } from "./webview-post-seek-progress";
  import { WebviewSeekCompletionWatchdog, hasWebviewSeekLanded, type WebviewSeekCompletionOwner } from "./webview-seek-completion-watchdog";
import { hasWebviewStartupResumeProgress, shouldRefreshWebviewResumeSource, startupResumeBlocksHistory } from "./webview-startup-resume";
  import { sanitizeWebviewSourceFormatDiagnostics } from "./lib/webview-source-format-diagnostics";
  import { appendTrackFailure, classifyPlaybackFailure, FIXED_RESOLVER_FAILURES, shouldAdvanceAfterExhaustedFailure, trackFailureCategory } from "./lib/playback-failure";
  import { allTrackSortFieldsAvailable, trackSortAvailability, trackSortOptionLabel, type TrackSortAvailability } from "./lib/track-sort-availability";
  import { onMount, onDestroy, tick, setContext, untrack } from "svelte";
  import { createQueuedPlaybackOutbox, dispatchQueuedPlayback, type QueuedPlayRequest, type QueuedPlayOutcome } from "./lib/queued-playback";
  import { createAutomationPlaybackPublisher, queueProjectionRevision, type PlaybackStateAck } from "./lib/automation-playback-publisher";
  import { startupStageTimings, waitForStartupClock, captureWebviewStartupFailure, type WebviewStartupFailure, type WebviewStartupPhase } from "./lib/webview-startup-diagnostics";
  import { attachAudioStream, type AudioStreamHandle } from "./lib/audio-stream";
  import { createWebviewAudioCapabilitiesProbe } from "./lib/webview-audio-capabilities";
  import { captureWebviewRadioWavBase64 } from "./lib/webview-radio-pcm";
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
import { canRequestWebviewLiveBpm, parseWebviewLiveBpmResponse, supportsWebviewLiveBpmRoute, WebviewLiveBpmWindow, WEBVIEW_LIVE_BPM_SAMPLE_RATE, type WebviewLiveBpmDiagnostics } from "./lib/webview-live-bpm";
  import type { WaveformSnapshot } from "./lib/waveform-data";
  import * as waveformData from "./lib/waveform-data";
  import { isFreshWebviewWaveformWindowAfterSeek, type WebviewWaveformSeekDrain } from "./lib/webview-waveform-seek";
  import { claimAutomaticStreamMediaAnalysis, fullStreamAnalysisOutcome, isCompleteStreamMediaWaveform, streamMediaResourceKey, streamMediaTrackContext, verifiedStreamWaveform, type StreamMediaResource, type StreamAudioVariant } from "./lib/stream-media-analysis";
  import { isStreamMediaAnalysisPaused } from "./lib/stream-analysis-policy";
import { ObservedWaveformCache, prepareObservedWaveformBinding, type PreparedObservedWaveformBinding } from "./lib/observed-waveform-cache";
  import { QueueRadioController, queueRadioSeedKey } from "./lib/queue-radio";
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
  import RadioView, {type RadioStation,type RadioTrackMetadata} from "./RadioView.svelte";
  import PlaybackQuality from "./PlaybackQuality.svelte";
  import LoudnessSettings from "./LoudnessSettings.svelte";
  import WaveformSeek from "./WaveformSeek.svelte";
  import StreamMediaAnalysisSettings from "./StreamMediaAnalysisSettings.svelte";
  import NowPlayingView from "./NowPlayingView.svelte";
  import { createTrackArtistInformationLoader } from "./lib/track-information";
  import { createRecordingInformationLoader, type RecordingReleaseInfo } from "./lib/recording-information";
  import DownloadSourceChooser from "./DownloadSourceChooser.svelte";
  import TrackFiles from "./TrackFiles.svelte";
  import BrowserConnections from "./BrowserConnections.svelte";
  import RemotePlaylists from "./RemotePlaylists.svelte";
  import RemotePlaylistDetail from "./RemotePlaylistDetail.svelte";
  import DirectoryBrowser, { type DirectoryBrowserSnapshot } from "./DirectoryBrowser.svelte";
  import { normalizeSpotifyUserProfileUrl } from "./lib/spotify-profile-url";
  import PlaylistSync from "./PlaylistSync.svelte";
  import PortablePlaylist from "./PortablePlaylist.svelte";
  import PortableHistory from "./PortableHistory.svelte";
  import AccountPlaylistSettings from "./AccountPlaylistSettings.svelte";
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
  import LocalAlbumLoudnessReview from "./LocalAlbumLoudnessReview.svelte";
  import {providerNavigationSnapshots} from "./lib/provider-navigation-snapshots";
  import { sortTracks, filterTracks, parseDurationBound, playableDuration, type TrackSortField, type SortDirection } from "./lib/track-sort";
  import SpotifyArtist from "./SpotifyArtist.svelte";
  import SearchPanel, { type SearchKind } from "./SearchPanel.svelte";
  import SoulseekWishlist from "./SoulseekWishlist.svelte";
  import SoulseekDownloadPolicy from "./SoulseekDownloadPolicy.svelte";
  import DownloadWatches from "./DownloadWatches.svelte";
  let activeDownloadWatchCount = $state(0);
  let showSavedSoulseekSearches = $state(false);
  import InfoPopover from "./InfoPopover.svelte";
  import LyricsPanel from "./LyricsPanel.svelte";
  import { queueForEngineMigration } from "./lib/playback-engine-migration";
  import { createNativePositionBookmark, nativeBookmarkMatchesOccurrence, shouldPersistNativePositionBookmark, validNativePositionBookmark, type NativePositionBookmark, type NativePositionOwner } from "./lib/native-position-bookmark";
  import PlaybackEngineSettings from "./PlaybackEngineSettings.svelte";
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
  import ArtistPage from "./ArtistPage.svelte";
  import AlbumSearch from "./AlbumSearch.svelte";
  import SeekSlider from "./SeekSlider.svelte";
  import FallbackSettings from "./FallbackSettings.svelte";
  import { streamingProviderOrder, downloadProviderOrder, type StreamingProvider } from "./lib/source-fallback";
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
  import { playbackAction } from "./lib/playback-action";
  import { isExplicitSoulseekFolderSource, isExplicitUnresolvedAudioSearchSource, resolveBatchSource, type BatchMatchAssessment } from "./lib/batch";
  import { invertSelection } from "./lib/selection";
  import { LatestRequest } from "./lib/latest-request";
  import { choosePreviewCandidate, isLinkedAudioSearchCandidate, plausiblePreviewCandidate } from "./lib/preview-source";
  import { sourcePosition, playlistPosition } from "./lib/source-position";
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
import { NativePlaybackBridge, nativeSnapshotFailure, type NativePositionEvent, type WebviewPlaybackHandoff, type ResolvedSourceHandoff } from "./lib/native-playback";
import { hasWebAudioGainGraph, inspectWebAudioGainGraph, retireWebAudioGainGraph, setWebAudioUserVolume, startWebAudioStereoDiagnosticTap, type LoudnessAnalysisJobStatus, type LoudnessPlaybackStatus, type LoudnessResourceIdentity } from "./lib/loudness-normalization";
import { encodeStereoDiagnosticSamples, stereoDiagnosticCompletionArguments } from "./lib/webview-stereo-diagnostic";
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
  let spotifyArtistComponent = $state<{captureNavigationSnapshot:(key:string)=>string}|undefined>();
  let spotifyNavigationSnapshotKey = $state<string|null>(null);
  let searchComponent = $state<{
    captureNavigationSnapshot:(key:string)=>string;
    restoreNavigationSnapshot:(key:string)=>Promise<void>;
    inspectAutomationSearchUi:(offset?:number,limit?:number,expiresAtMs?:number)=>unknown;
    setAutomationContinueSearch:(revision:string,enabled:boolean,expiresAtMs:number)=>Promise<unknown>;
    playAutomationSearchResult:(revision:string,occurrenceToken:string,expiresAtMs:number)=>unknown;
  }|undefined>();
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
  let webviewWaveformSeekDrain: WebviewWaveformSeekDrain | null = null;
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
    return nativeActive() && playingTrack?.playbackRange
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
  let completedWebviewOccurrence = "";
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
  function updatePlaybackPosition(absolute:number, completeRangeBoundary=true) {
    const resumeGate = webviewStartupResumeAudioGate;
    if (resumeGate && resumeGate.node === audio && resumeGate.source === preview && resumeGate.trackId === playingTrack?.id && !resumeGate.verified) {
      if (!hasWebviewStartupResumeProgress({
        target: resumeGate.target, startPosition: resumeGate.target, currentTime: absolute,
        duration: resumeGate.node.duration, elapsedMs: performance.now() - resumeGate.startedAtMs,
        readyState: resumeGate.node.readyState, seeking: resumeGate.node.seeking,
        paused: resumeGate.node.paused, playbackRate: resumeGate.node.playbackRate,
      })) {
        // Keep the saved resume clock stable while WebKit is still reporting the
        // beginning of a stream after an optimistic currentTime assignment.
        position = resumeGate.target;
        persistPlaybackPosition(true);
        return;
      }
      resumeGate.verified = true;
      releaseWebviewStartupResumeGate(resumeGate.node, resumeGate.source, true, false);
      void recordSuccessfulPlayback();
    } else if (resumeGate && resumeGate.node === audio && resumeGate.source !== preview) {
      releaseWebviewStartupResumeGate(resumeGate.node, resumeGate.source, false, false);
    }
    position=absolute;
    const range=playingTrack?.playbackRange;
    if (completeRangeBoundary && range && !paused && absolute>=range.endTime) completeWebviewQueueOccurrence(true);
    persistPlaybackPosition(true);
  }
  function completeWebviewQueueOccurrence(chapterBoundary = false, node?: HTMLAudioElement) {
    if (nativeActive() || !preview || !playingTrack || !playbackSession || resolvingPlayback || (node && node !== audio)) return;
    const occurrence = JSON.stringify([playbackSession, preview.sourceGeneration ?? null, preview.sessionId ?? null, seekSerial, playbackRunGeneration]);
    if (completedWebviewOccurrence === occurrence) return;
    completedWebviewOccurrence = occurrence;
    bufferingAudio = false;
    if (chapterBoundary) {
      audio?.pause();
      if (preview.sessionId) void call("candidate_preview_control",{action:"pause",value:null}).catch(()=>{});
    }
    void advanceQueue(1,true);
  }
  function stableWebviewSourceDuration(track = playingTrack, source = preview, node = audio): number | null {
    if (track?.playbackRange) {
      const parentDuration = track.playbackRange.parentDurationSeconds ?? track.durationSeconds;
      return Number.isFinite(parentDuration) && Number(parentDuration) > 0 ? Number(parentDuration) : null;
    }
    // A resolved WebView media element reports the actual finite resource
    // duration (including fractional seconds). Provider metadata is often
    // rounded to whole seconds and is too coarse for verified full-track
    // analysis and waveform coverage checks.
    const sourceGeneration = Number.isSafeInteger(source?.sourceGeneration) ? Number(source?.sourceGeneration) : null;
    if (!source?.isExcerpt && !source?.approximateExcerpt && source && ["audio", "direct", "local"].includes(source.kind) && node &&
        webviewLoadedMetadataOwner?.url === source.url && webviewLoadedMetadataOwner.sourceGeneration === sourceGeneration &&
        Number.isFinite(node.duration) && node.duration > 0 && node.duration <= (source.kind === "local" ? 86_400 : 900)) return node.duration;
    const resolvedDuration = source?.durationSeconds;
    if (Number.isFinite(resolvedDuration) && Number(resolvedDuration) > 0) return Number(resolvedDuration);
    if (source?.isExcerpt) return node && Number.isFinite(node.duration) && node.duration > 0 ? node.duration : null;
    const trackDuration = track ? playableDuration(track) : null;
    return trackDuration ?? null;
  }
  function stableWebviewSeekLimit(node = audio): number | null {
    const rangeEnd = playingTrack?.playbackRange?.endTime;
    if (Number.isFinite(rangeEnd) && Number(rangeEnd) > 0) return Number(rangeEnd);
    return stableWebviewSourceDuration(playingTrack, preview, node);
  }
  // All supported WebView seeks originate here, not in delayed DOM completion
  // events. This also invalidates an older pending resume before moving media.
  function seekWebviewAudio(node: HTMLAudioElement, target: number, watchCompletion = true): boolean {
    if (!Number.isFinite(target) || target < 0) {
      previewStatus = "Could not seek to an invalid audio position.";
      return false;
    }
    const stableDuration = stableWebviewSeekLimit(node);
    if (stableDuration !== null && Number.isFinite(stableDuration) && stableDuration > 0) {
      // Keep seeks inside a known source boundary. A small margin avoids a
      // browser treating an exact-duration seek as an ended stream.
      const chapterEnd = playingTrack?.playbackRange?.endTime;
      target = Math.min(target, chapterEnd === stableDuration ? stableDuration : Math.max(0, stableDuration - 0.05));
    }
    webviewBufferingRecovery.reset();
    nextCandidatePrefetch.cancel();
    cancelPendingWebviewResume();
    webviewSeekCompletionWatchdog.cancel();
    webviewPostSeekProgress.cancel(); pendingPostSeekProgress = null;
    activeWebviewSeek = null;
    seekSerial++;
    webviewWaveformSeekDrain = { node, sourceGeneration: preview?.sourceGeneration ?? null, seekSerial, targetPositionSeconds: target };
    node.currentTime = target;
    if (watchCompletion) beginWebviewSeekCompletionWatch(node, target);
    return true;
  }
  async function seekPlayer(relative:number) {
    await pendingWebviewHandoff?.settled;
    nextCandidatePrefetch.cancel();
    const value=nativeActive() ? relative : playingTrack ? chapterAbsolutePosition(playingTrack,relative) : relative;
    let mediaSeek = false;
    if (nativeActive()) await nativeCommand({action:"seek",position:value,...playbackOperationTrace()});
    else if (preview?.sessionId) await call("candidate_preview_control",{action:"seek",value});
    else if(audio) { if(!seekWebviewAudio(audio,value))return; mediaSeek = true; }
    position=mediaSeek && audio ? audio.currentTime : value;if(!mediaSeek)seekSerial++;if(!nativeActive()){if(audio&&!audio.paused)prepareUpcomingWebviewCandidate(audio);void publishAutomationPlayback();}
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
    if(captureId && desktop()) void call(nativeIdentifyCaptureId === captureId ? "cancel_playing_track_identification" : "cancel_webview_identification",{captureId}).catch(()=>{});
  }
  function currentPlayingTrackDetectionOwner():PlayingTrackDetectionOwner|null {
    if (!playingTrack || playingRadioStation || currentRadioSong || isStationPlaybackTrack(playingTrack)
      || (playingTrack.provenance as Record<string,unknown>|undefined)?.sourceKind==="radio") return null;
    const generation=nativeActive()?nativeSnapshot?.generation:preview?.sourceGeneration;
    if (!Number.isSafeInteger(generation) || generation == null || !Number.isSafeInteger(seekSerial)) return null;
    return {trackId:playingTrack.id,generation,seekSerial:nativeActive()?(nativeSnapshot?.seekSerial??0):seekSerial};
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
    if(nativeActive())nativeIdentifyCaptureId=captureId;
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
        const result=await call<PlayingTrackDetectionView>("get_playing_track_detections",{...owner});
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
  const previousRadioQueue = $derived(radioQueueHistory.filter((item,index)=>nativeActive() ? !currentRadioSong || index!==0 || item.track.id!==currentRadioSong.id : !currentRadioSong || item.id!==radioOccurrence?.key).map(item=>({...item,track:{...item.track,favorite:favoriteTrackIds.includes(item.track.id)}})));
  onMount(()=>{if(desktop())for(const provider of ["spotify","youtube"] as const)void call<{writeSyncEnabled?:boolean}>("remote_playlist_connection_status",{provider}).then(status=>remotePlaylistWriteSyncEnabled={...remotePlaylistWriteSyncEnabled,[provider]:status?.writeSyncEnabled===true}).catch(()=>{});});
  onMount(()=>{if(desktop())void call<{available:boolean}>("get_mpv_status").then(status=>mpvAvailable=status?.available===true).catch(()=>{});});
  type WebviewPcmRequest = {captureId?:string;trackId:string;stationId?:string;sessionId:string;generation:number;seekSerial:number;chapterId:string|null;isCurrent:()=>boolean;signal:AbortSignal;onProgress?:(seconds:number)=>void};
  function currentWebviewChapterId():string|null {
    return playingTrack?.playbackRange?.chapterId ?? activeChapter?.id ?? null;
  }
  async function identifyCurrentWebviewAudio(request:WebviewPcmRequest):Promise<any|null> {
    const sourceUrl=audio?.currentSrc || audio?.src || "";
    const linuxOutputCapture=desktop() && navigator.userAgent.includes("Linux");
    if(!audio || (!sourceUrl && !linuxOutputCapture))throw new Error("Current WebView audio is not available for capture.");
    const captureId=request.captureId ?? crypto.randomUUID();
    radioIdentifyCaptureId=captureId;
    try {
      await publishAutomationPlayback();
      if(request.signal.aborted || !request.isCurrent())return null;
      const identity={captureId,trackId:request.trackId,stationId:request.stationId,sessionId:request.sessionId,generation:request.generation,seekSerial:request.seekSerial,chapterId:request.chapterId};
      async function recognize(wavBase64?:string,captureSeconds?:number):Promise<any|null> {
        if(request.signal.aborted || !request.isCurrent())return null;
        let abortListener:(()=>void)|undefined;
        const cancelled=new Promise<never>((_,reject)=>{
          abortListener=()=>{const error=new Error("Audio identification cancelled because playback changed.");error.name="AbortError";reject(error);};
          if(request.signal.aborted)abortListener();else request.signal.addEventListener("abort",abortListener,{once:true});
        });
        try {
          const command=linuxOutputCapture ? call("identify_webview_playing_output",{...identity,webAudioGraphExpected:hasWebAudioGainGraph(audio!),captureSeconds}) : call("identify_webview_playing_pcm",{...identity,wavBase64});
          const result=await Promise.race([command,cancelled]);
          return request.signal.aborted || !request.isCurrent() ? null : result;
        } finally {if(abortListener)request.signal.removeEventListener("abort",abortListener);}
      }
      let result:any=null;
      const matched=(value:any)=>Boolean(value?.nowPlaying || value?.title);
      if(linuxOutputCapture) {
        let lastStarted=0;
        for(const seconds of [3,6,12]) {
          const wait=Math.max(0,lastStarted+10000-Date.now());
          if(wait)await new Promise<void>((resolve,reject)=>{
            const abort=()=>{clearTimeout(timer);request.signal.removeEventListener("abort",abort);const failure=new Error("Audio identification cancelled");failure.name="AbortError";reject(failure);};
            const timer=setTimeout(()=>{request.signal.removeEventListener("abort",abort);resolve();},wait);
            if(request.signal.aborted)abort();else request.signal.addEventListener("abort",abort,{once:true});
          });
          if(request.signal.aborted || !request.isCurrent())return null;
          lastStarted=Date.now();
          request.onProgress?.(seconds);
          result=await recognize(undefined,seconds);
          if(!result || matched(result))break;
        }
      } else {
        await captureWebviewRadioWavBase64(audio!,request.isCurrent,request.signal,request.onProgress,{
          onCancel:()=>{void call("cancel_webview_identification",{captureId}).catch(()=>{});},
          onSample:async(wavBase64)=>{
            result=await recognize(wavBase64);
            return !result || matched(result);
          },
        });
      }
      return request.signal.aborted || !request.isCurrent() ? null : result;
    } catch(failure) {
      // Capture deadlines and worklet errors must also cancel a provider request
      // already in flight, before a fresh Identify can be admitted by the UI.
      await call("cancel_webview_identification",{captureId}).catch(()=>{});
      throw failure;
    } finally { if(radioIdentifyCaptureId===captureId)radioIdentifyCaptureId=null; }
  }
  let lastRadioAudioAttempt:{stationId:string;generation:number|undefined;at:number}|null=null;
  async function identifyPlayingRadio(automatic=false) {
    const station=playingRadioStation;if(!station || !playingTrack || radioRecognizing)return;
    const trackId=playingTrack.id, session=playbackSession, serial=seekSerial, chapterId=currentWebviewChapterId();
    const nativeIdentification=nativeActive(), sourceGeneration=preview?.sourceGeneration, nativeGeneration=nativeSnapshot?.generation;
    const sourceUrl=audio?.currentSrc || audio?.src || "";
    const current=()=>playingTrack?.id===trackId && playingRadioStation?.id===station.id && playbackSession===session &&
      seekSerial===serial && currentWebviewChapterId()===chapterId && preview?.sourceGeneration===sourceGeneration && nativeActive()===nativeIdentification &&
      (!nativeIdentification || nativeSnapshot?.generation===nativeGeneration) && (nativeIdentification || Boolean(audio && sourceUrl && (audio.currentSrc || audio.src)===sourceUrl));
    radioRecognizing=true;radioIdentifyProgress=0;
    lastRadioAudioAttempt={stationId:station.id,generation:sourceGeneration,at:Date.now()};
    const controller=new AbortController();radioIdentifyAbort=controller;
    const captureId=nativeIdentification?crypto.randomUUID():null;
    if(captureId){nativeIdentifyCaptureId=captureId;radioIdentifyCaptureId=captureId;}
    try {
      let result:{nowPlaying:RadioTrackMetadata|null;note?:string};
      if(nativeIdentification) result=await call("identify_radio_station",{stationId:station.id,captureId});
      else {
        if(sourceGeneration===undefined || !sourceUrl)throw new Error("Current WebView radio audio is not available for capture.");
        result=await identifyCurrentWebviewAudio({trackId,stationId:station.id,sessionId:session,generation:sourceGeneration,seekSerial:serial,chapterId,isCurrent:current,signal:controller.signal,onProgress:(seconds)=>radioIdentifyProgress=seconds});
        if(!result)return;
      }
      if(controller.signal.aborted || !current())return;radioNowPlaying(station,result.nowPlaying);if(!automatic)setNotice(result.nowPlaying?`Identified: ${result.nowPlaying.artist} — ${result.nowPlaying.title}`:result.note||"No song identified. Try again during music.");
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
    const activeTrackId=currentTrack?.id, session=playbackSession, native=nativeActive();
    const nativeGeneration=nativeSnapshot?.generation;
    const positionAtOpen = playerPosition;
    const positionTrack=parentChapter ? currentTrack! : track;
    if (active && settings.pauseOnSourceOpen && !paused) {
      if (nativeActive()) await nativeCommand({action:"pause"});
      else if (preview?.sessionId) await run(() => call("candidate_preview_control", {action:"pause", value:null}));
      else await toggleCurrentPlayback();
      if (playbackSession!==session || playingTrack?.id!==activeTrackId || nativeActive()!==native ||
        native && nativeSnapshot?.generation!==nativeGeneration) return;
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
    const nativeIdentification = nativeActive();
    const session = playbackSession, serial = seekSerial, chapterId=currentWebviewChapterId();
    const sourceGeneration = preview?.sourceGeneration, nativeGeneration = nativeSnapshot?.generation;
    const sourceUrl=audio?.currentSrc || audio?.src || "";
    const current = () => playingTrack?.id === id && playbackSession === session && seekSerial === serial && currentWebviewChapterId()===chapterId &&
      preview?.sourceGeneration === sourceGeneration && nativeActive() === nativeIdentification && (!nativeIdentification || nativeSnapshot?.generation === nativeGeneration) && (nativeIdentification || Boolean(audio && sourceUrl && (audio.currentSrc || audio.src)===sourceUrl));
    radioRecognizing = true;
    radioIdentifyProgress=0;
    const controller=new AbortController();radioIdentifyAbort=controller;
    const captureId = nativeIdentification ? crypto.randomUUID() : null;
    if (captureId) { nativeIdentifyCaptureId=captureId; radioIdentifyCaptureId=captureId; }
    try {
      if (!nativeIdentification && sourceGeneration === undefined) throw new Error("Restart this recording to prepare its audio for identification.");
      let result:PlayingTrackIdentification;
      if(nativeIdentification) result=await call("identify_playing_track",{captureId});
      else {
        if(!sourceUrl)throw new Error("Current WebView audio is not available for capture.");
        result=await identifyCurrentWebviewAudio({trackId:id,sessionId:session,generation:sourceGeneration!,seekSerial:serial,chapterId,isCurrent:current,signal:controller.signal,onProgress:(seconds)=>radioIdentifyProgress=seconds});
        if(!result)return null;
      }
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
    if (!playingTrack || playingTrack.id!==trackId || !nativeActive() || nativeSnapshot?.status!=="playing" || paused || playingRadioStation || playingTrack.playbackRange) return null;
    const track=playingTrack, generation=nativeSnapshot.generation, serial=seekSerial, entryId=playbackQueue.entryIds[queueIndex] ?? null;
    if (!entryId) return null;
    const result=await identifyPlayingTrack();
    const anchor=result?.timingAnchor;
    if (!result || !anchor || playingTrack?.id!==track.id || !nativeActive() || nativeSnapshot?.status!=="playing" || nativeSnapshot?.generation!==generation || seekSerial!==serial || paused || playingRadioStation || playingTrack.playbackRange || (playbackQueue.entryIds[queueIndex] ?? null)!==entryId) return null;
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
      else if(preview?.sessionId)await call("candidate_preview_control",{action:"pause",value:null});
      else audio?.pause();
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
        nativeActive()&&(nativePlaybackStatus!=="ended"||!nativeNaturalQueueEnd)||
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
  let activeFailedQueueRun: FailedQueueRun | null = null;
  let resumeTrackId = "", resumePosition = 0, pendingResumePosition = 0;
  let requiredRecoveryResume: { request: number; position: number; verifiedAtMs: number | null } | null = null;
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
  type Route = { searchReleaseSnapshotKey?:string|null; searchNavigationSnapshotKey?:string|null; nativeArtistSnapshotKey?:string|null; spotifyNavigationSnapshotKey?:string|null; directorySnapshot?: DirectoryBrowserSnapshot; directoryLocation?: {rootId:string;relativePath:string}|null; view: View; collectionId: string; provider: string; genreUrl: string; query: string; filter: string; localFilter: "all" | "local" | "not-local"; sort: string; descending: boolean; librarySource: string; searchCategory: string; searchMode: typeof searchMode; importId?: string; artistName: string; artistRef: ArtistRef | null; artistTrack: Track | null; artistSection: typeof artistSection; artistScroll: number; loadedTrackCount?:number; durationMinimum?:string; durationMaximum?:string; spotifyArtistId?:string; spotifyAlbumId?:string; spotifyArtistSearchFallback?: SpotifyArtistSearchFallback | null; remoteOwner?:string; remotePlaylistUrl?:string; chartId?: string; historyQuery?: string; historyProvider?: string; historyStationId?: string; historyFrom?: string; historyTo?: string; historyOrder?: string; historySortField?: TrackSortField; historySortDirection?: SortDirection; historyMinDuration?: string; historyMaxDuration?: string; historyDownloadAvailability?: "all" | "downloaded" | "not-downloaded"; historyLoadedCount?: number; historyScroll?: number; historyFocusedSessionId?: string | null; focusedPlaylistRowKey?: string | null; selected: Track | null; scroll: number };
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
    audio = $state<HTMLAudioElement>(),
    paused = $state(true),
    position = $state(0),
    duration = $state(0),
    volume = $state(0.8),
    table = $state<HTMLDivElement>();
  const activeDetectedArtwork = $derived(playingTrackDetectionView?.enabled && samePlayingTrackDetectionOwner(playingTrackDetectionOwner, currentPlayingTrackDetectionOwner()) ? activeDetectedTrack(playingTrackDetectionView.observations, currentDetectionPositionSeconds())?.artworkUrl ?? null : null);
  type WebviewLoudnessSettings = {
    mode: "off" | "track" | "album";
    targetLufs: number;
    ceilingDbtp: number;
    maximumBoostDb: number;
    albumTrackFallback: boolean;
    analyzeWhenIdle: boolean;
  };
  type WebviewNormalizationRoute = "same-origin-relay" | "unsupported";
  type WebviewLoudnessDecision = {
    mode: "off" | "track" | "album";
    status: "off" | "applied" | "album_fallback_track" | "unavailable";
    appliedGainDb: number;
    measurementSource?: string | null;
    reason?: string | null;
    sourceResourceKey?: string | null;
  };
  type WebviewResolvedPreview = Preview & {
    assetId?: string | null;
    normalizationRoute?: WebviewNormalizationRoute;
  };
  let webviewLoudnessSettings = $state<WebviewLoudnessSettings>({
    mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false,
  });
  let webviewLoudnessSettingsLoaded = false;
  let webviewLoudnessDecision = $state<WebviewLoudnessDecision>({ mode: "off", status: "off", appliedGainDb: 0 });
  let activeWebviewLoudnessAlbumResource = $state<LoudnessResourceIdentity | null>(null);
  let webviewNormalizationRouteStatus = $state<string | null>(null);
  let webviewGainAdapter: import("./lib/loudness-normalization").WebAudioGainAdapter | null = null;
  let webviewLoadedMetadataOwner: { url: string; sourceGeneration: number | null } | null = null;
  type WebviewStartupResumeAudioGate = {node:HTMLAudioElement;source:Preview;trackId:string;target:number;startedAtMs:number;requestedVolume:number;verified:boolean};
  let webviewStartupResumeAudioGate: WebviewStartupResumeAudioGate | null = null;
  function applyWebviewOutputVolume(node: HTMLAudioElement | null | undefined, value: number): void {
    if (!node || !Number.isFinite(value) || value < 0 || value > 1) return;
    if (setWebAudioUserVolume(node, value)) {
      if (node.volume !== 1) node.volume = 1;
      return;
    }
    node.volume = value;
  }
  function releaseWebviewStartupResumeGate(node:HTMLAudioElement,source:Preview,restoreVolume:boolean,preserveTarget:boolean): void {
    const gate=webviewStartupResumeAudioGate;
    if(!gate||gate.node!==node||gate.source!==source)return;
    webviewStartupResumeAudioGate=null;
    if(restoreVolume)applyWebviewOutputVolume(node,gate.requestedVolume);
    if(preserveTarget&&playingTrack?.id===gate.trackId&&preview===source){position=gate.target;persistPlaybackPosition(true);}
  }
  function setWebviewOutputVolume(node: HTMLAudioElement | null | undefined, value: number): void {
    if (!node || !Number.isFinite(value) || value < 0 || value > 1) return;
    const gate=webviewStartupResumeAudioGate;
    if(gate?.node===node&&gate.source===preview){gate.requestedVolume=value;applyWebviewOutputVolume(node,0);return;}
    if(gate?.node===node)releaseWebviewStartupResumeGate(node,gate.source,false,false);
    applyWebviewOutputVolume(node,value);
  }
  function retireAudioGraphOnDestroy(node: HTMLAudioElement) {
    return { destroy: () => retireWebAudioGainGraph(node) };
  }
  type WebviewObservedStreamCapture = {
    sessionId: string; generation: number; trackId: string; node: HTMLAudioElement;
    tap: import("./lib/loudness-normalization").WebAudioPcmTap; durationSeconds: number;
    expectedPositionSeconds: number; nextSequence: number; started: boolean;
    finishing?: boolean;
  };
  let webviewAnalysisArmTrackId = $state<string | null>(null);
  let webviewAnalysisGeneration = 0;
  let webviewObservedCapture = $state<WebviewObservedStreamCapture | null>(null);
  let webviewObservedAnalysisStatus = $state<string | null>(null);
  let webviewObservedAnalysis = $state<WholeTrackAnalysisDto | null>(null);
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
    const chapter=nativeActive()?nativeCanonicalChapter:activeChapter;
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
  type PendingWebviewHandoff = { handoffId: string; cancelled: boolean; isCurrent: () => boolean; settled: Promise<void> };
  let pendingWebviewHandoff: PendingWebviewHandoff | null = null;
  type StartupNativeHandoffReceipt = {
    handoffId: string;
    request: number;
    startupGeneration: number;
    node: HTMLAudioElement;
    source: Preview;
    sourceGeneration: number;
    sessionId: string;
    trackId: string;
    entryId: string;
    queueEntryIds: string[];
    index: number;
    targetPosition: number;
    candidateId: string;
    committed: boolean;
    consumed: boolean;
  };
  let startupNativeHandoffReceipt: StartupNativeHandoffReceipt | null = null;
  let playbackEngineReady = !desktop();
  let playbackControlsOwned = $state(false);
  const nativeActive = () => nativeSnapshot?.engine === "native";
  const liveBpmTrackId = $derived(playingTrack?.id ?? null);
  const liveBpmGeneration = $derived(nativeSnapshot?.engine === "native" ? nativeSnapshot.generation : (preview as WebviewResolvedPreview | null)?.sourceGeneration ?? null);
  const liveBpmNativeSeekSerial = $derived(nativeSnapshot?.seekSerial ?? 0);
  const liveBpmCanPoll = $derived(settings.liveBpmEnabled === true && nativeSnapshot?.engine === "native" && nativeSnapshot.status === "playing" && !paused && Boolean(liveBpmTrackId));
  function currentLiveMeasurementResource(): LiveMeasurementResource | null {
    const track = playingTrack;
    if (!track || track.playbackRange) return null;
    const activeAssetId = nativeActive() && nativeSnapshot?.quality?.sourceKind === "localAsset"
      ? nativeSnapshot.quality.assetId
      : !nativeActive() && (preview as WebviewResolvedPreview | null)?.kind === "local"
        ? (preview as WebviewResolvedPreview).assetId
        : null;
    if (activeAssetId) {
      const asset = scopedAssetsById.get(activeAssetId) ?? trackAsset(track);
      if (asset?.id === activeAssetId && /^[a-f0-9]{64}$/i.test(asset.hash)) {
        return { kind: "localAsset", assetId: asset.id, fileHash: asset.hash, durationSeconds: asset.durationSeconds ?? undefined, recordingId: asset.recordingId };
      }
      return null;
    }
    const stream = currentObservedStreamIdentity();
    const durationSeconds = nativeActive()
      ? nativeSnapshot?.duration || nativeSnapshot?.candidate?.durationSeconds || track.durationSeconds || 0
      : stableWebviewSourceDuration(track, preview, audio) || (preview as WebviewResolvedPreview | null)?.durationSeconds || playingCandidate?.durationSeconds || track.durationSeconds || 0;
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
  const waveformGeneration = $derived(nativeSnapshot?.engine === "native" ? nativeSnapshot.generation : (preview as WebviewResolvedPreview | null)?.sourceGeneration ?? null);
  const cachedStreamAnalysisTrackId = $derived(playingTrack?.id ?? null);
  const cachedStreamAnalysisGeneration = $derived(nativeSnapshot?.engine === "native" ? nativeSnapshot.generation : null);
  const cachedStreamAnalysisEligible = $derived.by(() => {
    const track = playingTrack;
    const provider = track?.provider;
    const radioTrack = provider === "radioparadise" || provider === "internet-radio" ||
      provider === "radio" && (track?.provenance as { sourceKind?: string } | undefined)?.sourceKind === "radio";
    return nativeSnapshot?.engine === "native" &&
      ["buffering", "playing", "paused", "ended"].includes(nativeSnapshot.status) &&
      !radioTrack && !track?.playbackRange;
  });
  const waveformLocalAssetId = $derived.by(() => nativeSnapshot?.engine === "native"
    ? nativeSnapshot.quality?.sourceKind === "localAsset" ? nativeSnapshot.quality.assetId ?? null : null
    : (preview as WebviewResolvedPreview | null)?.kind === "local" && !preview?.isExcerpt
      ? (preview as WebviewResolvedPreview).assetId ?? null : null);
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
  let liveBpmObservation = $state<WebviewLiveBpmDiagnostics | null>(null);
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
        if (!active || latestTrackId !== trackId || latestGeneration !== generation || seekSerial !== serial || (nativeSnapshot?.seekSerial ?? 0) !== nativeSerial || paused || !nativeActive() || nativeSnapshot?.status !== "playing") return;
        const bpm = estimates?.bpm ?? null;
        const key = estimates?.key ?? null;
        const validBpm = isCurrentLiveBpmEstimate(bpm, latestOwner) ? bpm : null;
        const validKey = isCurrentLiveKeyEstimate(key, latestOwner) ? key : null;
        liveBpmEstimate = validBpm;
        liveKeyEstimate = validKey;
        if (validBpm || validKey) {
          void persistLiveMeasurementUpdate(measurementResource, { bpm: validBpm, key: validKey }, nativeSnapshot?.position ?? null);
        }
        if (!validBpm && !validKey) liveBpmPollStatus = Date.now() - startedAt < 15000 ? "warming" : "insufficient";
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
  $effect(() => {
    const trackId = playingTrack?.id ?? null;
    const measurementToken = liveMeasurementResourceToken;
    const measurementResource = untrack(() => liveMeasurementResource);
    const source = preview as WebviewResolvedPreview | null;
    const generation = source?.sourceGeneration ?? null;
    const serial = seekSerial;
    const node = audio;
    const enabled = settings.liveBpmEnabled === true;
    const eligible = enabled && !nativeActive() && !paused && Boolean(trackId) && generation !== null &&
      supportsWebviewLiveBpmRoute(source?.kind, source?.normalizationRoute) && Boolean(node);
    if (!eligible || !trackId || generation === null || !source || !node) {
      if (!nativeActive()) {
        liveBpmEstimate = null;
        liveKeyEstimate = null;
        liveBpmObservation = null;
        if (enabled && !paused && trackId && supportsWebviewLiveBpmRoute(source?.kind, "same-origin-relay")) {
          liveBpmPollStatus = "unsupported";
          liveBpmPollReason = "The active Web Audio playback route is not ready to provide PCM samples.";
        } else {
          liveBpmPollStatus = "warming";
          liveBpmPollReason = null;
        }
      }
      return;
    }
    let active = true;
    let inFlight = false;
    let tap: import("./lib/loudness-normalization").WebAudioPcmTap | null = null;
    let timer = 0;
    const startedAt = Date.now();
    const owner = { trackId, generation, seekSerial: serial };
    const pcm = new WebviewLiveBpmWindow(owner);
    const publishObservation = () => { if (isCurrent()) liveBpmObservation = pcm.diagnosticSnapshot; };
    const isCurrent = () => active && audio === node && preview?.url === source.url && preview?.sourceGeneration === generation &&
      playingTrack?.id === trackId && seekSerial === serial && !paused && !nativeActive() && node === audio;
    const poll = async () => {
      const playing = isCurrent() && !node.paused;
      if (inFlight || !canRequestWebviewLiveBpm(enabled, playing, owner, playing ? owner : null, pcm.seconds)) return;
      const samples = pcm.snapshot(playing, owner);
      if (!samples) return;
      inFlight = true;
      pcm.recordAttempt();
      liveBpmPollStatus = "estimating";
      publishObservation();
      const requestStartedAt = performance.now();
      try {
        const rawResponse = await call<unknown>("estimate_webview_live_bpm", {
          trackId, generation, seekSerial: serial, sampleRate: WEBVIEW_LIVE_BPM_SAMPLE_RATE,
          samplesBase64: (await import("./lib/loudness-normalization")).encodeWebAudioPcm16Base64(samples),
        });
        const response = parseWebviewLiveBpmResponse<LiveBpmEstimate>(rawResponse);
        const owner = { trackId: playingTrack?.id ?? "", generation: preview?.sourceGeneration ?? -1, seekSerial };
        if (!isCurrent()) return;
        pcm.recordOutcome(response.status, performance.now() - requestStartedAt);
        publishObservation();
        if (response.status === "owner_mismatch_before" || response.status === "owner_changed_after") {
          liveBpmEstimate = null;
          liveKeyEstimate = null;
          liveBpmPollStatus = "waiting";
          liveBpmPollReason = null;
          return;
        }
        if (response.status === "no_estimate" || response.status === "disabled" || response.status === "native_active") {
          liveBpmEstimate = null;
          liveKeyEstimate = null;
          liveBpmPollStatus = Date.now() - startedAt < 15_000 ? "warming" : "insufficient";
          liveBpmPollReason = null;
          return;
        }
        const estimate = response.estimate ?? null;
        const keyEstimate = response.keyEstimate ?? null;
        const validBpm = isCurrentLiveBpmEstimate(estimate, owner) ? estimate : null;
        const validKey = isCurrentLiveKeyEstimate(keyEstimate, owner) ? keyEstimate : null;
        liveBpmEstimate = validBpm;
        liveKeyEstimate = validKey;
        if (validBpm || validKey) {
          void persistLiveMeasurementUpdate(measurementResource, { bpm: validBpm, key: validKey }, node.currentTime);
        }
        liveBpmPollStatus = validBpm ? "warming" : Date.now() - startedAt < 15_000 ? "warming" : "insufficient";
        liveBpmPollReason = null;
      } catch {
        if (isCurrent()) {
          liveBpmEstimate = null;
          liveKeyEstimate = null;
          liveBpmPollStatus = "unavailable";
          liveBpmPollReason = "The Web Audio live playback analysis request could not be completed.";
        }
      } finally { inFlight = false; }
    };
    liveBpmEstimate = null;
    liveKeyEstimate = null;
    liveBpmPollStatus = "warming";
    liveBpmPollReason = null;
    liveBpmObservation = pcm.diagnosticSnapshot;
    void (async () => {
      try {
        if (!isCurrent() || !await ensureWebviewWaveformRoute() || !isCurrent()) {
          if (isCurrent()) {
            liveBpmPollStatus = "unsupported";
            liveBpmPollReason = "The active Web Audio playback route could not provide PCM samples.";
          }
          return;
        }
        const module = await import("./lib/loudness-normalization");
        if (!isCurrent()) return;
        const attached = await module.attachWebAudioPcmTap(node, event => {
          if (!isCurrent()) { pcm.reset(); return; }
          if (event.kind === "gap") {
            pcm.reset("capture_gap");
            publishObservation();
            liveBpmEstimate = null;
            liveKeyEstimate = null;
            liveBpmPollStatus = "warming";
            liveBpmPollReason = event.reason.slice(0, 160);
            return;
          }
          if (!pcm.append(event.batch.samples, owner)) {
            liveBpmEstimate = null;
            liveKeyEstimate = null;
            liveBpmPollStatus = "unavailable";
            liveBpmPollReason = "The PCM observation did not match the active playback window.";
          }
          publishObservation();
        });
        if (!attached.tap) {
          if (isCurrent()) {
            liveBpmPollStatus = "unsupported";
            liveBpmPollReason = (attached.reason ?? "The Web Audio PCM observer is unavailable.").slice(0, 160);
          }
          return;
        }
        tap = attached.tap;
        pcm.markTapAttached();
        publishObservation();
        timer = window.setInterval(() => void poll(), 6000);
        void poll();
      } catch {
        if (isCurrent()) {
          liveBpmPollStatus = "unavailable";
          liveBpmPollReason = "The Web Audio PCM observer could not start.";
        }
      }
    })();
    return () => {
      active = false;
      window.clearInterval(timer);
      tap?.stop();
      pcm.bind(null);
      pcm.reset();
      if (liveBpmObservation) liveBpmObservation = null;
      if (!nativeActive()) liveBpmEstimate = null;
      if (!nativeActive()) liveKeyEstimate = null;
      if (!nativeActive()) liveBpmPollReason = null;
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
    const handoff = pendingWebviewHandoff;
    if (handoff && snapshot.handoffId === handoff.handoffId && handoff.cancelled) {
      handoff.cancelled = true;
      void nativeBridge.command({action:"stop",expectedHandoffId:handoff.handoffId}).catch(()=>{});
      return;
    }
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
    const radio = snapshot.radioNowPlaying as {id?:string;title?:string;artist?:string;stationName?:string;metadata?:RadioTrackMetadata|null}|null|undefined;
    const radioMetadata = radio?.metadata;
    playingTrack = snapshotTrack && radioMetadata ? {
      ...snapshotTrack,
      title:radioMetadata.title || radio?.title || snapshotTrack.title, artist:radioMetadata.artist || radio?.artist || snapshotTrack.artist,
      album:radioMetadata.album ?? snapshotTrack.album, artworkUrl:radioMetadata.artworkUrl ?? snapshotTrack.artworkUrl,
      sourceUrl:radioMetadata.sourceUrl || snapshotTrack.sourceUrl,
      provenance:{...(snapshotTrack.provenance as Record<string,unknown>),radioStationName:radio?.stationName,radioMetadata,radioStationId:radioMetadata.stationId},
    } : snapshotTrack;
    currentRadioSong = snapshotTrack && radio?.id && radioMetadata && playingTrack ? { ...playingTrack,id:radio.id,provider:"radio_song",providerId:radio.id,favorite:favoriteTrackIds.includes(radio.id) } : null;
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
    confirmStartupNativeHandoffSnapshot(snapshot);

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
    // An initial empty native snapshot can be the handoff point for the
    // legacy-WebView-to-native migration. Leave that browser queue intact
    // until migration is decided; after ownership exists, empty is a real
    // native queue change and must clear stale browser queue state.
    if (isEmpty && !lastNativeQueueCheckpoint) {
      let migrationAlreadyDecided = false;
      try { migrationAlreadyDecided = localStorage.getItem("burrata.native-playback-migrated.v1") === "1"; } catch { /* Preserve the legacy queue if marker storage is unavailable. */ }
      if (!migrationAlreadyDecided) return;
    }
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
  let engineSwitchBusy = $state(false);
  async function applyPlaybackEngineAndRestart(engine: "webview" | "native") {
    if (engineSwitchBusy) return;
    engineSwitchBusy = true;
    try {
      const isNative = nativeActive();
      if (isNative) await nativeBridge.command({action:"pause"});
      else { audio?.pause(); paused = true; }
      const snapshot = isNative ? await call<NativePlaybackSnapshot & {queueTotal:number;queueEntryIds:string[]}>("get_playback_snapshot") : null;
      const queue = snapshot ? queueForEngineMigration(snapshot) : playbackQueue.snapshot();
      const track = queue.tracks[queue.currentIndex];
      const relativePosition = snapshot ? snapshot.position : track ? chapterRelativePosition(track, position) : 0;
      const absolutePosition = snapshot && track ? chapterAbsolutePosition(track, relativePosition) : position;
      const persisted = new PlaybackQueue();
      persisted.restore(queue);
      if (!savePlaybackQueue(persisted)) throw new Error("Could not save the current queue");
      const entryId = queue.entryIds?.[queue.currentIndex];
      localStorage.setItem(PLAYBACK_POSITION_KEY, JSON.stringify({trackId:track?.id ?? null,...(entryId ? {entryId} : {}),position:absolutePosition,savedAt:Date.now()}));
      localStorage.setItem("burrata.engine-switch-controls.v1", JSON.stringify({volume:snapshot?.volume ?? volume,shuffle:snapshot?.shuffle ?? shuffleEnabled}));
      await call("restart_with_playback_engine", {engine, checkpoint:{
        tracks:queue.tracks, entryIds:queue.entryIds, index:queue.currentIndex,
        ...(queue.queueLoudnessContexts ? { queueLoudnessContexts: queue.queueLoudnessContexts } : {}),
        currentEntryId:queue.entryIds?.[queue.currentIndex] ?? null,
        queueRevision:snapshot?.queueRevision ?? 0, position:relativePosition,
        volume:snapshot?.volume ?? volume, repeat:queue.repeat, shuffle:snapshot?.shuffle ?? shuffleEnabled,
      }});
    } finally { engineSwitchBusy = false; }
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
  const nextCandidatePrefetch = new WebviewNextCandidatePrefetch((command,args)=>call(command,args));
  $effect(() => { if (nativeActive()) nextCandidatePrefetch.cancel(); });
  const previewRequests = new LatestRequest();
  let nativeResolutionGeneration = $state<number | null>(null);
  let audioStartupGeneration = 0;
  let artistPlaybackMatchGeneration = 0;
  let artistPlaybackMatchRevision = 0;
  const preparedNoLocalArtistMatches = new Map<string, number>();
  function nextPreviewRequest() {
    nextCandidatePrefetch.cancel();
    cancelPendingWebviewResume();
    cancelWebviewStartupPlay?.();
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
  function queuedEntryAnchored() {
    return Boolean(playingTrack && playbackQueue.current && playingTrack.id === playbackQueue.current.id);
  }
  function resolveQueuedAcceptance(owner: string | null) {
    if (!owner || !queuedEntryAnchored()) return;
    const waiter = queuedAcceptanceWaiters.get(owner);
    if (!waiter || activeAutomationPlayback !== owner || queuedPlaybackOwners.get(owner) !== waiter.requestId) return;
    queuedAcceptanceWaiters.delete(owner);
    waiter.resolve(automationPlaybackSnapshot(true));
  }
  function snapshotForQueuedAcceptance(request: QueuedPlayRequest): Promise<QueuedAcceptanceEnvelope> {
    if (activeAutomationPlayback !== request.operationId || queuedPlaybackOwners.get(request.operationId) !== request.requestId) {
      return Promise.reject(new Error("Playback acceptance was cancelled or superseded"));
    }
    if (queuedEntryAnchored()) return Promise.resolve(automationPlaybackSnapshot(true));
    return new Promise((resolve, reject) => {
      queuedAcceptanceWaiters.set(request.operationId, { requestId: request.requestId, resolve, reject });
    });
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
    audio?.pause();
    paused = true;
    const sessionId = preview?.sessionId;
    // Cancelling an older owner must preserve a separately validated preparation.
    setAutomationPlaybackOwner(null, true);
    if (sessionId) void call("stop_candidate_preview", { sessionId }).catch(() => {});
  }

  async function handleQueuedPlayback(request: QueuedPlayRequest) {
    await cancelPendingWebviewHandoff();
    if (nativeActive()) return; // Native queued jobs are owned by the backend.
    if (!queuedPlaybackOutbox.reserve(request)) {
      void call("automation_queued_playback_update", {
        operationId: request.operationId,
        requestId: request.requestId,
        status: "failed",
        detail: { reason: "Playback operation status backlog is full" },
      }).catch(() => {});
      return;
    }
    retireQueuedPreparation("superseded");
    queuedPlaybackPrepareIntent = request;
    let installed = false;
    let startup: Promise<unknown> | null = null;
    const current = () => installed
      ? activeAutomationPlayback === request.operationId &&
        queuedPlaybackOwners.get(request.operationId) === request.requestId
      : queuedPlaybackPrepareIntent === request;
    try {
      await dispatchQueuedPlayback<Track>(request, {
        load: ids => call<Track[]>("get_tracks", { ids }),
        current,
        prepare: requestedTracks => {
          // Hydration and multiplicity validation finish before replacing the
          // established owner, queue, or its cancellation address.
          playbackQueue.setTracks(requestedTracks, 0);
          refreshQueue();
          const suppliedEntryIds = [...playbackQueue.entryIds];
          const revision = automationQueueProjection.revision;
          queuedPlaybackOwners.set(request.operationId, request.requestId);
          setAutomationPlaybackOwner(request.operationId);
          installed = true;
          queuedPlaybackPrepareIntent = null;
          const start = startPreview(
            requestedTracks[0]!, true, false, undefined, undefined, 0, request.operationId,
          );
          startup = start;
          const acceptedGeneration = playbackRunGeneration;
          const captureOutcome = (failure?: unknown): QueuedPlayOutcome => {
            const sameQueue = suppliedEntryIds.length === playbackQueue.entryIds.length &&
              suppliedEntryIds.every((id, index) => playbackQueue.entryIds[index] === id) &&
              sameNativePlaybackQueue(playbackQueue.tracks, requestedTracks);
            const occurrence = requestedTracks[queueIndex];
            const played = current() && sameQueue && occurrence && playingTrack &&
              sameNativePlaybackTrack(playingTrack, occurrence) && preview && !paused;
            const exhausted = recentTrackFailures.some(failure =>
              failure.operationId === request.operationId && failure.outcome === "queue_exhausted",
            );
            let status: QueuedPlayOutcome["status"];
            if (!current() || !sameQueue) status = "superseded";
            else if (queuedPausedOwner === request.operationId) status = "paused";
            else if (played) status = "playing";
            else status = exhausted ? "exhausted" : "failed";
            return {
              status,
              detail: {
                currentGeneration: playbackRunGeneration,
                entryId: playbackQueue.entryIds[queueIndex] ?? null,
                reason: played ? null : failure === undefined ? previewStatus : String(failure),
              },
            };
          };
          const completion = start.then(async () => {
            if (current() && audioRecovery) await audioRecovery;
            return captureOutcome();
          }).catch(captureOutcome);
          return {
            receipt: {
              engine: "webview",
              trackIds: request.trackIds,
              queueRevision: revision,
              queueEntryIds: suppliedEntryIds,
              acceptedGeneration,
            },
            completion,
          };
        },
        accept: async receipt => {
          for (let attempt = 0; attempt < 3; attempt++) {
            if (!current()) return { accepted: false };
            const pendingSnapshot = snapshotForQueuedAcceptance(request);
            const completedSnapshot = startup!.then(() => {
              if (!current() || !queuedEntryAnchored()) throw new Error("Playback exited before publishing its queue occurrence");
              return automationPlaybackSnapshot(true);
            });
            const envelope = await Promise.race([pendingSnapshot, completedSnapshot]);
            if (!current() || envelope.queueRevision !== receipt.queueRevision) return { accepted: false };
            const result = await call<{
              accepted: boolean;
              playbackAck?: PlaybackStateAck;
              retryable?: boolean;
            }>("automation_queued_playback_accept", {
              operationId: request.operationId,
              requestId: request.requestId,
              receipt,
              playback: envelope.playback,
            });
            if (result.playbackAck) automationPlaybackPublisher.acknowledge(envelope, result.playbackAck);
            if (!current()) return { accepted: false };
            if (result.accepted) return result;
            if (!result.retryable) return result;
          }
          return { accepted: false };
        },
        lookup: () => call("automation_queued_playback_status", { operationId: request.operationId }),
        stopOwned: () => cancelQueuedPlayback(request),
        update: outcome => queuedPlaybackOutbox.publish(request, {
          ...outcome,
          detail: { ...outcome.detail, currentGeneration: playbackRunGeneration },
        }),
      });
    } finally {
      const waiter = queuedAcceptanceWaiters.get(request.operationId);
      if (waiter?.requestId === request.requestId) queuedAcceptanceWaiters.delete(request.operationId);
      if (!installed && queuedPlaybackPrepareIntent === request) queuedPlaybackPrepareIntent = null;
    }
  }

  let lastMediaFailure: string | null = null;
  let lastStartupFailure = $state<WebviewStartupFailure | null>(null);
  let lastWebviewDiagnosticFailure: WebviewStartupFailure | null = null;
  let webviewDiagnosticFailureHistory: Array<{attempt:number;failure:WebviewStartupFailure}> = [];
  let webviewStartupOutcomes: Array<{attempt:number;operationId:string;outcome:"playing"|"failed"|"cancelled";elapsedMs:number}> = [];
  let currentWebviewDiagnosticAttempt = 0;
  let lastWebviewDiagnosticContext: Record<string, unknown> | null = null;
  let webviewDiagnosticsCleared = false;
  function recordWebviewStartupFailure(failure: WebviewStartupFailure) {
    lastStartupFailure = failure;
    if (webviewMediaEventRecorder.enabled) {
      lastWebviewDiagnosticFailure = failure;
      webviewDiagnosticFailureHistory.push({attempt:currentWebviewDiagnosticAttempt || 1,failure});
      if (webviewDiagnosticFailureHistory.length > 8) webviewDiagnosticFailureHistory.splice(0,webviewDiagnosticFailureHistory.length-8);
      webviewDiagnosticsCleared = false;
    }
  }
  type ResolvedWebviewSource = {provider:string|null;id:string|null;sourceTransport?:string;headerPolicy:NonNullable<Preview["sourceHeaderPolicy"]>|null};
  let lastResolvedSource: ResolvedWebviewSource | null = null;
  let lastMediaFailureSource: ResolvedWebviewSource | null = null;
  function rememberResolvedSource(candidate: Candidate | null, result: Preview) {
    const raw = result.sourceHeaderPolicy;
    const names = ["authorization","cookie","referer","origin","userAgent","other"] as const;
    const valid = raw && Number.isInteger(raw.headerCount) && raw.headerCount >= 0 && raw.headerCount <= 10000 && names.every(name => typeof raw[name] === "boolean");
    const headerPolicy = valid ? {headerCount:raw.headerCount, authorization:raw.authorization,cookie:raw.cookie,referer:raw.referer,origin:raw.origin,userAgent:raw.userAgent,other:raw.other} : null;
    const provider = candidate?.provider && /^[a-z]{1,32}$/.test(candidate.provider) ? candidate.provider : null;
    const id = candidate?.id && /^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id) ? candidate.id : null;
    lastResolvedSource = {provider,id,headerPolicy,...(["direct","loopback-relay","loopback-transcode"].includes(result.sourceTransport ?? "") ? {sourceTransport:result.sourceTransport} : {})};
  }
  let successfulRecoveryAttachment: { request: number; startedAt: number; position: number; seekSerial: number; healthy?: boolean; healthStartedAt?: number; healthPosition?: number } | null = null;
  let automaticPreviewRecovery: {
    request: number;
    track: Track;
    tried: Set<string>;
    attempts: number;
    sourceAttempts?: number;
    deadline?: number;
    sourceStartupDeadline?: number;
    liveRecoveryDeadline?: number;
    exactSourceOnly?: boolean;
    transportRefreshUsed?: boolean;
    recurringStalls?: RecurringStallRecoveryState;
    stallFallbacks?: number;
    transportRecoveryWindowUsed?: boolean;
    failureReason?: string;
    stallCause?: string;
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
  let durationMinimum = $state(""), durationMaximum = $state("");
  const activeDurationFilterCount = $derived([durationMinimum, durationMaximum].filter(value => parseDurationBound(value) !== null).length);
  let localPageTotal = $state(0), localPageLoading = $state(false);
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
  function displayPosition(track: Track) { return view === "collections" ? playlistPosition(track) ?? sourcePosition(track) : sourcePosition(track); }
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
    return { searchReleaseSnapshotKey:view === "search" && searchRelease ? captureSearchRelease() : null, searchNavigationSnapshotKey:view === "search" && !searchRelease ? searchComponent?.captureNavigationSnapshot(`route-search:${++navigationSnapshotSequence}`) : null, nativeArtistSnapshotKey:view === "artist" && !spotifyArtistId && !spotifyAlbumId ? nativeArtistComponent?.captureNavigationSnapshot(`route-catalog:${++navigationSnapshotSequence}`) : null, spotifyNavigationSnapshotKey:view === "artist" && (spotifyArtistId || spotifyAlbumId) ? spotifyArtistComponent?.captureNavigationSnapshot(`route-artist:${++navigationSnapshotSequence}`) : null, directorySnapshot:view === "directory-browser" ? directoryBrowser?.getSnapshot() : undefined, directoryLocation:directoryLocation ? {...directoryLocation}:null, view, collectionId, provider, genreUrl, query, filter, localFilter, sort, descending, librarySource, searchCategory, searchMode, importId: view === "charts" ? activeBrowserImport?.id : undefined, artistName: activeArtistName, artistRef: activeArtistRef, artistTrack: activeArtistTrack, artistSection, artistScroll, loadedTrackCount:tracks.length,durationMinimum,durationMaximum,spotifyArtistId, spotifyAlbumId, spotifyArtistSearchFallback, remoteOwner, remotePlaylistUrl, chartId: view === "charts" ? chart?.id : undefined, historyQuery, historyProvider, historyStationId, historyFrom, historyTo, historyOrder, historySortField, historySortDirection, historyMinDuration, historyMaxDuration, historyDownloadAvailability, historyLoadedCount: view === "history" ? historyItems.length : undefined, historyScroll: view === "history" ? historyScroll : undefined, historyFocusedSessionId, focusedPlaylistRowKey, selected, scroll: table?.scrollTop ?? 0 };
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
      directoryLocation = route.directoryLocation ? {...route.directoryLocation} : null;
      if (route.view === "directory-browser") directoryBrowserVisited = true;
      collectionId = route.collectionId; provider = route.provider; genreUrl = route.genreUrl; query = route.query;
      spotifyArtistId=route.spotifyArtistId??""; spotifyAlbumId=route.spotifyAlbumId??""; spotifyArtistSearchFallback=route.spotifyArtistSearchFallback??null; remoteOwner=route.remoteOwner??""; remotePlaylistUrl=route.remotePlaylistUrl??""; historyQuery=route.historyQuery??""; historyProvider=route.historyProvider??""; historyStationId=route.historyStationId??""; historyFrom=route.historyFrom??""; historyTo=route.historyTo??""; historyOrder=route.historyOrder??"newest"; historySortField=route.historySortField??"sourceOrder"; historySortDirection=route.historySortDirection??"desc"; historyMinDuration=route.historyMinDuration??""; historyMaxDuration=route.historyMaxDuration??""; historyDownloadAvailability=route.historyDownloadAvailability??"all"; historyScroll=route.historyScroll??0; historyFocusedSessionId=route.historyFocusedSessionId??null; focusedPlaylistRowKey=route.focusedPlaylistRowKey??null; const historyRestoreSignature = route.view === "history" ? JSON.stringify(historySearchOptions()) : null; activeArtistName = route.artistName; activeArtistRef = route.artistRef; activeArtistTrack = route.artistTrack; artistSection = route.artistSection; artistScroll = route.artistScroll; librarySource = route.librarySource; searchCategory = route.searchCategory; searchMode = route.searchMode; albumQuery = route.query;
      await navigate(route.view, false);
      let restoredControlsMatch = () => true;
      const restoreIsCurrent = () => restoredControlsMatch() && restorationEpoch === routeRestorationEpoch && navigationRequest === expectedNavigationRequest && view === route.view && collectionId === route.collectionId && provider === route.provider;
      if (!restoreIsCurrent()) return false;
      if (route.view === "artist" && !route.spotifyArtistId && !route.spotifyAlbumId && route.nativeArtistSnapshotKey) {
        await tick();if (!restoreIsCurrent()) return false;
        await nativeArtistComponent?.restoreNavigationSnapshot(route.nativeArtistSnapshotKey);
        if (!restoreIsCurrent()) return false;
      }
      if(route.view==="search"&&restoredRelease){
        await tick();if(!restoreIsCurrent())return false;
        if(restoredRelease.controls)await searchReleaseComponent?.restoreNavigationState(restoredRelease.controls);
        if(!restoreIsCurrent())return false;
      }
      if (route.view === "search" && !restoredRelease && searchNavigationSnapshotKey) {
        const effectiveSearchKey=searchNavigationSnapshotKey;
        await tick();if (!restoreIsCurrent()) return false;
        await searchComponent?.restoreNavigationSnapshot(effectiveSearchKey);
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
  const sourceCollections = $derived(visibleCollections.filter((item) => {
    const source = latestImports.find((entry) => entry.collectionId === item.id)?.provider;
    return librarySource === "all" || (librarySource === "local" ? localPlaylistIds.includes(item.id) : librarySource === "personal" ? !source && !localPlaylistIds.includes(item.id) : source === librarySource);
  }).map((item) => ({ ...item, trackCount: latestImports.find((entry) => entry.collectionId === item.id)?.capturedCount ?? item.trackCount })));
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
    rememberRoute(); librarySource = source;
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
        if (nativeActive()) queueTracks=queueTracks.map(track=>byId.get(track.id) ?? track);
        else { playbackQueue.updateTracks(playbackQueue.tracks.map(track=>byId.get(track.id) ?? track)); refreshQueue(); }
      }
    } catch (failure) { mirrorStatus = String(failure); }
  }
  function refreshQueue() {
    nextCandidatePrefetch.cancel();
    queueIndex = playbackQueue.currentIndex;
    queueTracks = [...playbackQueue.tracks];
    repeatMode = playbackQueue.repeat;
    playbackQueueGeneration++;
    resetTrackFailureHistoryForQueue(automationQueueProjection.revision, automationQueueProjection.sourceKey);
    playbackQueueSaveError = savePlaybackQueue(playbackQueue)
      ? ""
      : "Your playback queue couldn't be saved. You can continue listening, but changes may be lost after restarting Burrata.";
    if (!paused && !resolvingPlayback && !bufferingAudio) prepareUpcomingWebviewCandidate(audio ?? null);
  }
  async function advanceQueue(direction: number, ended = false, failedSkips = 0, requestId: string | null = null) {
    if (nativeActive()) {
      if (!ended) clearPlaybackSkipNotice();
      await nativeCommand({action:direction > 0 ? "next" : "previous"});
      return;
    }
    if (!ended && direction > 0 && repeatMode === "one") { playbackQueue.setRepeat("off"); }
    const next = direction < 0 ? playbackQueue.previous() : playbackQueue.next();
    playbackQueue.setRepeat(repeatMode); refreshQueue();
    if (next) { selected = next; await startPreview(next, true, ended, undefined, undefined, failedSkips, requestId); } else {if(ended&&playingTrack&&await continueQueueRadio(playingTrack,true))return;paused = true;}
  }
  let lastPersistedPlaybackPosition: {trackId:string;position:number;savedAt:number} | null = null;
  function persistPlaybackPosition(routine = false) {
    if (engineSwitchBusy) return;
    if (!playingTrack || typeof localStorage === "undefined") return;
    resumeTrackId = playingTrack.id;
    resumePosition = Math.max(0, Number.isFinite(position) ? position : 0);
    const savedAt = Date.now();
    const unchanged = lastPersistedPlaybackPosition?.trackId === resumeTrackId && lastPersistedPlaybackPosition.position === resumePosition;
    if (routine && lastPersistedPlaybackPosition && unchanged && savedAt >= lastPersistedPlaybackPosition.savedAt && savedAt - lastPersistedPlaybackPosition.savedAt < 60_000) return;
    try {
      const entryId = playbackQueue.current?.id === resumeTrackId ? playbackQueue.entryIds[queueIndex] : undefined;
      localStorage.setItem(PLAYBACK_POSITION_KEY, JSON.stringify({ trackId: resumeTrackId, ...(entryId ? { entryId } : {}), position: resumePosition, savedAt }));
      lastPersistedPlaybackPosition = {trackId:resumeTrackId,position:resumePosition,savedAt};
    } catch {
      lastPersistedPlaybackPosition = null;
      /* Storage can be unavailable or full; leave subsequent writes retryable. */
    }
  }
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
  async function seekPendingResume(request: number, stillCurrent: () => boolean = () => true) {
    const target = pendingResumePosition;
    const player = audio;
    if (!target || !player) return;
    if (player.readyState < 1) {
      await new Promise<void>((resolve) => {
        let settled = false;
        let timer: ReturnType<typeof setTimeout>;
        let cancelCheck: ReturnType<typeof setInterval>;
        const finish = () => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          clearInterval(cancelCheck);
          player.removeEventListener("loadedmetadata", finish);
          player.removeEventListener("error", finish);
          resolve();
        };
        player.addEventListener("loadedmetadata", finish, { once: true });
        player.addEventListener("error", finish, { once: true });
        timer = setTimeout(finish, 2000);
        cancelCheck = setInterval(() => { if (!stillCurrent()) finish(); }, 25);
      });
    }
    if (!stillCurrent() || !previewRequests.isCurrent(request) || player !== audio) return;
    // Recovery validation retains the finite media bound for legacy sources
    // that provide neither resolver nor catalog duration. Ordinary seeks do
    // not treat this potentially changing estimate as a stable bound.
    const limit = stableWebviewSeekLimit(player) ?? (Number.isFinite(player.duration) ? player.duration : 0);
    if (target > 0 && (!limit || target < limit - 1)) {
      try { seekWebviewAudio(player,target,false); position = target; } catch { /* Some remote streams cannot seek yet. */ }
    }
    const requiredResume = requiredRecoveryResume;
    if (requiredResume?.request === request) {
      if (!Number.isFinite(player.currentTime) || Math.abs(player.currentTime - requiredResume.position) > 1 || (limit > 0 && target >= limit - 1)) {
        throw new Error("The recovered audio source could not resume at its previous playback position.");
      }
      requiredRecoveryResume = { ...requiredResume, verifiedAtMs: performance.now() };
    }
  }
  async function recordSuccessfulPlayback() {
    const track = playingTrack, sessionId = playbackSession, candidate = playingCandidate;
    if (!track || !sessionId || recordedSession === sessionId) return;
    const startupResumeGate = webviewStartupResumeAudioGate;
    if (startupResumeBlocksHistory(startupResumeGate, audio, preview, track.id)) return;
    if(playingRadioStation){
      const metadata=(track.provenance as {radioMetadata?:RadioTrackMetadata})?.radioMetadata;
      if(metadata)void recordCurrentRadioSong(playingRadioStation,metadata);
      return;
    }
    recordedSession = sessionId;
    if (candidate) void rememberAudioSource(track, candidate);
    const chapter = candidate?.chapters?.find((item) => position >= item.startTime && position < item.endTime);
    try {
      await call("record_playback", { sessionId, provider: candidate?.provider ?? preview?.provider ?? track.provider, track, chapter: chapter ?? null });
      await refreshTrackLibrarySummaryFor(track.id);
    } catch (failure) {
      mirrorStatus = `History: ${String(failure)}`;
      if (recordedSession === sessionId) recordedSession = "";
    }
  }
  async function rememberAudioSource(track: Track, candidate: Candidate) {
    if (portablePlaybackSource(track)) return;
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
  const playerPosition = $derived(nativeActive() ? position : playingTrack ? chapterRelativePosition(playingTrack, position) : position);
  function isStationPlaybackTrack(track: Track | null | undefined): boolean {
    return Boolean(track && (track.provider === "radioparadise" || track.provider === "internet-radio" ||
      track.provider === "radio" && (track.provenance as { sourceKind?: string } | undefined)?.sourceKind === "radio"));
  }
  const liveRadioPlayback = $derived(isStationPlaybackTrack(playingTrack));
  const playerDuration = $derived.by(() => {
    const provenance = playingTrack?.provenance as { sourceKind?: string } | null | undefined;
    const liveRadio = playingTrack?.provider === "radioparadise" || playingTrack?.provider === "internet-radio"
      || playingTrack?.provider === "radio" && provenance?.sourceKind === "radio";
    if (nativeActive()) {
      if (duration > 0 || !paused) return duration;
      return liveRadio || !playingTrack ? duration : playableDuration(playingTrack) ?? duration;
    }
    if (playingTrack?.playbackRange) return playableDuration(playingTrack) ?? duration;
    if (liveRadio || !playingTrack) return duration;
    return stableWebviewSourceDuration(playingTrack, preview, audio) ?? duration;
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
        const nativeChapter = nativeActive() ? nativeSnapshot?.chapter : null;
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
      waveformLocalAssetId === assetId && waveformGeneration === generation && nativeActive() &&
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
    if (!nativeActive() || !waveformEnabled) return;
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
        if (!active || !result || localWaveformAnalysisJobId || localWaveformCacheRebindOwnerKey === owner || currentFullLocalWaveform || playingTrack?.id !== trackId || nativeSnapshot?.generation !== generation || !nativeActive()) return;
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
    const trackId = waveformTrackId;
    const generation = waveformGeneration;
    const durationSeconds = playerDuration;
    const observedSeekSerial = seekSerial;
    const shouldObserveWebview = !nativeActive() && !paused && !playingRadioStation &&
      (preview as WebviewResolvedPreview | null)?.normalizationRoute === "same-origin-relay" && Boolean(audio) && Boolean(trackId) &&
      generation !== null && Number.isFinite(durationSeconds) && durationSeconds > 0 && durationSeconds <= 86_400;
    if (!shouldObserveWebview) return;
    let active = true;
    let inFlight = false;
    const poll = async () => {
      if (!active || inFlight || localWaveformAnalysisJobId || playbackWaveform?.source === "local-analysis" && playbackWaveform.complete || currentStreamWaveformMatchesPlayback()) return;
      inFlight = true;
      try {
        if (!await ensureWebviewWaveformRoute()) {
          // Cache verification can finish while route setup is awaiting a
          // decision or AudioContext operation. Never let that stale poll
          // replace the newly installed complete waveform with an empty one.
          if (!active || currentStreamWaveformMatchesPlayback()) return;
          if (active) {
            playbackWaveformReason = webviewNormalizationRouteStatus && !webviewNormalizationRouteStatus.toLowerCase().includes("route active")
              ? webviewNormalizationRouteStatus : "This WebView cannot expose waveform samples for the current audio route.";
            const current = playbackWaveform;
            if (!current || current.generation !== generation || current.trackId !== trackId || current.reason !== playbackWaveformReason) {
              const waveform = waveformData;
              playbackWaveform = waveform.emptyWaveform(generation!, trackId!, durationSeconds, 1200, playbackWaveformReason);
            }
          }
          return;
        }
        const node = audio;
        if (!active || !node || paused || playingTrack?.id !== trackId || waveformGeneration !== generation || seekSerial !== observedSeekSerial) return;
        const module = await import("./lib/loudness-normalization");
        const windowPeak = module.readWebAudioPeakWindow(node);
        if (!windowPeak || !active || paused || localWaveformAnalysisJobId || playbackWaveform?.source === "local-analysis" && playbackWaveform.complete || currentStreamWaveformMatchesPlayback() || playingTrack?.id !== trackId || waveformGeneration !== generation || seekSerial !== observedSeekSerial) return;
        const pendingDrain = webviewWaveformSeekDrain;
        if (!isFreshWebviewWaveformWindowAfterSeek(node, generation, observedSeekSerial, windowPeak, pendingDrain)) return;
        if (pendingDrain?.node === node && pendingDrain.sourceGeneration === generation && pendingDrain.seekSerial === observedSeekSerial) webviewWaveformSeekDrain = null;
        const waveform = waveformData;
        const current = playbackWaveform;
        const base = current?.source === "observed-playback" && current.generation === generation && current.trackId === trackId &&
          Math.abs(current.durationSeconds - durationSeconds) <= 0.25
          ? current
          : waveform.emptyWaveform(generation, trackId!, durationSeconds, 1200);
        const rangeStartSeconds = playingTrack?.playbackRange?.startTime ?? 0;
        playbackWaveform = waveform.observeWaveformSegment(base, {
          generation,
          trackId: trackId!,
          startSeconds: windowPeak.startPositionSeconds - rangeStartSeconds,
          endSeconds: windowPeak.endPositionSeconds - rangeStartSeconds,
          min: windowPeak.min,
          max: windowPeak.max,
        });
        observedWaveformSnapshotOwner = currentObservedWaveformOwnerKey();
        playbackWaveformReason = "WebView audio is sampled during playback; blank areas remain unobserved.";
      } catch {
        if (!active || currentStreamWaveformMatchesPlayback()) return;
        if (active) {
          playbackWaveformReason = "WebView waveform observation is unavailable for this audio route.";
          const current = playbackWaveform;
          if (!current || current.generation !== generation || current.trackId !== trackId || current.reason !== playbackWaveformReason) {
            const waveform = waveformData;
            playbackWaveform = waveform.emptyWaveform(generation!, trackId!, durationSeconds, 1200, playbackWaveformReason);
          }
        }
      } finally { inFlight = false; }
    };
    // This poll reads playbackWaveform before awaiting and later publishes a
    // new snapshot; tracking those reads would create an immediate feedback
    // loop while waveform sampling is active.
    untrack(() => { void poll(); });
    const timer = window.setInterval(() => void poll(), 80);
    return () => { active = false; window.clearInterval(timer); };
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
    const ownerKey = `${trackId ?? ""}:${generation ?? ""}`;
    if (cachedStreamAnalysisOwnerKey !== ownerKey) {
      cachedStreamAnalysisOwnerKey = ownerKey;
      cachedStreamAnalysis = null;
    }
    if (!trackId || generation === null || !cachedStreamAnalysisEligible) return;
    let active = true;
    let inFlight = false;
    const poll = async () => {
      if (!active || inFlight) return;
      inFlight = true;
      try {
        const result = await call<WholeTrackAnalysisDto | null>("get_cached_stream_analysis", { trackId, generation });
        if (!active || !result || playingTrack?.id !== trackId || nativeSnapshot?.generation !== generation || !nativeActive()) return;
        if (result.scope !== "wholeTrack" || result.resource?.kind !== "stream" || result.resource.recordingId !== trackId ||
            !Number.isFinite(result.durationSeconds) || !(8 <= result.durationSeconds && result.durationSeconds <= 900) ||
            (!result.bpm && !result.key)) return;
        cachedStreamAnalysis = result;
      } catch {
        // An optional cache read must not interrupt playback or track changes.
      } finally { inFlight = false; }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 2200);
    return () => { active = false; window.clearInterval(timer); };
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
  const nativeCanonicalChapter = $derived(nativeActive() && nativeSnapshot?.chapter?.endSeconds !== undefined
    ? chapterForRange(playingCandidate?.chapters ?? [], {startTime:nativeSnapshot.chapter.startSeconds,endTime:nativeSnapshot.chapter.endSeconds})
    : null);
  const activeChapter = $derived(nativeActive() && nativeSnapshot?.chapter
    ? nativeCanonicalChapter ?? {title:nativeSnapshot.chapter.title,startTime:nativeSnapshot.chapter.startSeconds,endTime:nativeSnapshot.chapter.endSeconds ?? Number.POSITIVE_INFINITY,id:"native",confidence:"native"}
    : playingCandidate?.chapters?.find((item) => position >= item.startTime && position < item.endTime));
  const playerAudioSource = $derived.by(() => {
    const approximateNativeSource = nativeActive() && nativeSnapshot?.quality?.sourceKind === "approximateAlbumExcerpt";
    const excerpt = nativeActive() ? nativeSnapshot?.quality?.approximateExcerpt : preview?.approximateExcerpt;
    const approximateLocalAsset = !nativeActive() && preview?.kind === "local" && excerpt?.label === "Approximate album excerpt";
    if (approximateNativeSource || approximateLocalAsset) {
      const sourceId = excerpt?.source?.id;
      const candidateProvider = nativeActive() ? nativeSnapshot?.candidate?.provider : playingCandidate?.provider;
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
  const activeRecordingMatch = $derived(nativeActive() ? nativeSnapshot?.candidate?.sourceMetadata?.matching as Preview["matching"] : preview?.matching);
  const playingTitle = $derived(playingTrack?.playbackRange ? playingTrack.title : activeChapter?.title ?? playingTrack?.title ?? preview?.title ?? "Nothing playing");
  const playbackActionTrack = $derived.by(()=>{const chapter=nativeActive()?nativeCanonicalChapter:activeChapter;const track=currentRadioSong ?? (playingTrack && !playingTrack.playbackRange && playingCandidate && chapter ? virtualChapterTrack(playingTrack,playingCandidate,chapter) : null) ?? playingTrack;return track?{...track,favorite:favoriteTrackIds.includes(track.id)||track.favorite}:null;});
  const expandedPlayingTrack = $derived.by(() => {
    const track = playbackActionTrack;
    const match = activeRecordingMatch;
    if (!track || match?.assessment.relationship !== "alternative") return track;
    const source = nativeActive() ? nativeSnapshot?.candidate : playingCandidate;
    return {...track, title: match.resolvedRecording.title, artist: match.resolvedRecording.artist ?? "Alternative performer",
      provider: source?.provider ?? "unknown", providerId: source?.id ?? "", sourceUrl: source?.url ?? "",
      version: match.resolvedRecording.version, album: null, albumArtist: null, label: null, remixers: null,
      isrc:null,catalogNumber:null,releaseDate:null,recordingDate:null,genre:null,normalizedGenre:null,
      trackNumber:null,trackTotal:null,discNumber:null,discTotal:null,durationSeconds:source?.durationSeconds ?? null,
      artworkUrl: source?.artworkUrl ?? null, provenance: {}};
  });
  function seekChapter(start: number) { void seekPlayer(playingTrack ? chapterRelativePosition(playingTrack,start) : start); }
  async function executeSearch() {
    if (searchCategory === "artists") await searchArtistDiscography();
    else if (searchCategory === "albums") albumQuery = query;
    else if (searchCategory === "catalog") await search();
    else await findAudio();
  }
  async function searchArtistDiscography() {
    if (!query.trim()) return;
    spotifyArtistId = ""; spotifyAlbumId = ""; spotifyArtistSearchFallback = null;
    activeArtistName = query.trim(); activeArtistRef = null; activeArtistTrack = null;
    artistSection = "releases"; artistScroll = 0;
    await navigate("artist");
  }
  const contextKey = () =>
    `${view}:${view === "collections" ? collectionId : ""}`;
  async function navigate(next: View, recordRoute = true) {
    if (recordRoute && (next !== view || !["library", "favorites", "history", "downloads", "settings"].includes(next))) rememberRoute();
    if (next === "search" && !restoringRoute) {searchNavigationSnapshotKey = null;searchRelease=null;searchReleaseRequest++;}
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

  function prepareUpcomingWebviewCandidate(node: HTMLAudioElement | null) {
    if (!node || node !== audio || !playbackEngineReady || nativeActive() || !preview || !playingTrack || node.paused) return;
    const remainingSeconds = !playingTrack.playbackRange && Number.isFinite(node.duration) && node.duration > 0 &&
      Number.isFinite(node.currentTime) ? Math.max(0, node.duration - node.currentTime) : null;
    nextCandidatePrefetch.started({ webview: true, playing: true, generation: playbackQueueGeneration,
      session: playbackSession, currentTrackId: playingTrack.id, queue: playbackQueue.snapshot(), pinnedTrackIds: Object.keys(queueCandidates), remainingSeconds });
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

  let audioStreamHandle: AudioStreamHandle | null = null;
  let lastHlsFailure = "";
  async function ensureWebviewLoudnessSettings(): Promise<WebviewLoudnessSettings> {
    if (!webviewLoudnessSettingsLoaded) {
      try {
        const loaded = await call<WebviewLoudnessSettings | null>("get_loudness_settings");
        if (loaded && ["off", "track", "album"].includes(loaded.mode) &&
            Number.isFinite(loaded.targetLufs) && Number.isFinite(loaded.ceilingDbtp) &&
            Number.isFinite(loaded.maximumBoostDb) && typeof loaded.albumTrackFallback === "boolean") {
          webviewLoudnessSettings = { ...loaded, analyzeWhenIdle: loaded.analyzeWhenIdle === true };
        } else {
          webviewLoudnessSettings = { mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
        }
      } catch {
        webviewLoudnessSettings = { mode: "off", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
      }
      webviewLoudnessSettingsLoaded = true;
    }
    return webviewLoudnessSettings;
  }
  onMount(() => { void ensureWebviewLoudnessSettings(); });
  async function isWebviewNormalizationRouteRequired(collectWaveform = true): Promise<boolean> {
    const config = await ensureWebviewLoudnessSettings();
    const module = await import("./lib/loudness-normalization");
    // Finite-track waveform observation starts with playback, independently of
    // whether its seek bar is visible. Radio keeps its existing route policy.
    return collectWaveform || config.mode !== "off" || waveformEnabled || Boolean(webviewAnalysisArmTrackId) || module.hasWebAudioGainGraph(audio);
  }
  function currentWebviewStreamCaptureIdentity(): { provider: string; sourceId: string; durationSeconds: number; audioVariant: StreamAudioVariant | null } | null {
    const track = playingTrack, candidate = playingCandidate, source = preview as WebviewResolvedPreview | null;
    const durationSeconds = stableWebviewSourceDuration(track, source, audio) ?? candidate?.durationSeconds ?? track?.durationSeconds ?? 0;
    if (!track || !candidate || nativeActive() || playingRadioStation || track.localPath || source?.kind === "local" ||
        track.playbackRange || source?.isExcerpt || source?.approximateExcerpt || source?.normalizationRoute !== "same-origin-relay" ||
        !["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider) ||
        !/^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id) || !Number.isFinite(durationSeconds) || durationSeconds < 8 || durationSeconds > 900) return null;
    return { provider: candidate.provider, sourceId: candidate.id, durationSeconds, audioVariant: currentStreamAudioVariant() };
  }
  function currentStreamAudioVariant(): StreamAudioVariant | null {
    if (nativeActive()) {
      const quality = nativeSnapshot?.quality;
      return quality ? { formatId: quality.formatId, codec: quality.codec, mimeType: quality.mimeType,
        bitrateKbps: quality.bitrateKbps, expectedContentLength: quality.expectedContentLength,
        containsVideo: quality.formatHasVideo } : null;
    }
    return preview ? { formatId: preview.audioFormatId, codec: preview.audioCodec, mimeType: preview.audioMimeType,
      bitrateKbps: preview.audioBitrateKbps, expectedContentLength: preview.audioExpectedContentLength,
      containsVideo: preview.audioFormatHasVideo } : null;
  }
  type ActiveStreamMediaResource = StreamMediaResource & { key: string };
  function currentStreamMediaResource(): ActiveStreamMediaResource | null {
    const track = playingTrack;
    const candidate = nativeActive() ? nativeSnapshot?.candidate : playingCandidate;
    const durationSeconds = nativeActive()
      ? nativeSnapshot?.duration || candidate?.durationSeconds || track?.durationSeconds || 0
      : stableWebviewSourceDuration(track, preview, audio) || preview?.durationSeconds || candidate?.durationSeconds || track?.durationSeconds || 0;
    if (!track || !candidate || playingRadioStation || track.localPath || track.playbackRange ||
        nativeActive() && nativeSnapshot?.quality?.sourceKind === "localAsset" ||
        !nativeActive() && (preview?.kind === "local" || preview?.isExcerpt || preview?.approximateExcerpt) ||
        !["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider) ||
        !/^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id) || !Number.isFinite(durationSeconds) || durationSeconds < 8 || durationSeconds > 900) return null;
    const resource = { trackId: track.id, provider: candidate.provider, sourceId: candidate.id, durationSeconds, audioVariant: currentStreamAudioVariant(),
      sessionGeneration: nativeActive() ? nativeSnapshot?.generation : preview?.sourceGeneration };
    return { ...resource, key: streamMediaResourceKey(resource) };
  }
  function currentObservedStreamIdentity(): { provider: string; sourceId: string; audioVariant: StreamAudioVariant | null } | null {
    const track = playingTrack;
    const candidate = nativeActive() ? nativeSnapshot?.candidate : playingCandidate;
    // A catalog track can retain a local path while an explicitly selected
    // remote source is playing. Bind observations to that active source.
    if (!track || !candidate || playingRadioStation ||
        nativeActive() && nativeSnapshot?.quality?.sourceKind === "localAsset" ||
        !nativeActive() && preview?.kind === "local" ||
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
    return nativeActive()
      ? nativeSnapshot?.status === "playing" && nativeSnapshot.paused === false
      : Boolean(audio && !audio.paused && preview && (preview.kind === "audio" || preview.kind === "direct"));
  }
  function streamMediaAnalysisUnavailableReason(): string {
    if (playingRadioStation) return "Full-track analysis is unavailable for live radio.";
    if (!playingTrack || playingTrack.localPath || nativeActive() && nativeSnapshot?.quality?.sourceKind === "localAsset" ||
        !nativeActive() && preview?.kind === "local") return "";
    if (playingTrack.playbackRange || preview?.isExcerpt || preview?.approximateExcerpt) return "Full-track analysis is unavailable for an excerpt or playback range.";
    const candidate = nativeActive() ? nativeSnapshot?.candidate : playingCandidate;
    if (!candidate) return "A stable verified stream identity is unavailable for this playback.";
    if (!["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider)) return "This provider does not support full streamed-track analysis.";
    const duration = nativeActive() ? nativeSnapshot?.duration ?? 0 : stableWebviewSourceDuration(playingTrack, preview, audio) ?? 0;
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
    if (wasStarting || wasRunning) streamMediaAnalysisStatus = statusMessage;
  }
  async function startStreamMediaAnalysis(resource: ActiveStreamMediaResource, automatic = false): Promise<void> {
    if (streamMediaAnalysisStartingKey === resource.key) return;
    if (streamMediaAnalysisJob && !["completed", "failed", "cancelled"].includes(streamMediaAnalysisJob.state)) {
      if (streamMediaAnalysisJob.resourceKey === resource.key) return;
      await cancelStreamMediaAnalysis();
    }
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
      const activeWebviewGeneration = !nativeActive() ? preview?.sourceGeneration : undefined;
      const started = await call<SelectedStreamJobDto>("start_stream_media_analysis", {
        track: streamMediaTrackContext(trackContext, resource.provider, resource.sourceId),
        provider: resource.provider, sourceId: resource.sourceId, durationSeconds: resource.durationSeconds, audioVariant: resource.audioVariant ?? null, automatic,
        webviewGeneration: typeof activeWebviewGeneration === "number" && Number.isSafeInteger(activeWebviewGeneration) && activeWebviewGeneration > 0
          ? activeWebviewGeneration : null,
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
    if (streamMediaAnalysisStartingKey && (!enabled || streamMediaAnalysisStartingKey !== resource?.key)) {
      void cancelStreamMediaAnalysis(enabled ? "Analysis stopped because the playing source changed." : "Background analysis is off; verified waveform cache remains available.");
      return;
    }
    if (job && (!enabled || job.resourceKey !== resource?.key)) {
      void cancelStreamMediaAnalysis(enabled ? "Analysis stopped because the playing source changed." : "Background analysis is off; verified waveform cache remains available.");
      return;
    }
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
  function cancelWebviewObservedCapture(reason: string): void {
    const capture = webviewObservedCapture;
    webviewObservedCapture = null;
    if (capture) {
      capture.tap.stop();
      void call("cancel_webview_stream_analysis", { sessionId: capture.sessionId }).catch(() => {});
    }
    if (reason) webviewObservedAnalysisStatus = reason;
  }
  async function analyzeCurrentWebviewStream(): Promise<void> {
    const track = playingTrack, identity = currentWebviewStreamCaptureIdentity();
    if (!track || !identity) {
      webviewObservedAnalysisStatus = "Whole-track WebView analysis requires a supported, non-excerpt stream with a stable duration.";
      return;
    }
    cancelWebviewObservedCapture("");
    webviewObservedAnalysis = null;
    webviewAnalysisArmTrackId = track.id;
    webviewObservedAnalysisStatus = "Restarting this stream from the beginning for an explicit whole-track capture…";
    try {
      await startPreview(track, true, false, undefined, undefined, 0, null, false, undefined, activeWebviewLoudnessAlbumResource);
      if (webviewAnalysisArmTrackId === track.id && !webviewObservedCapture) {
        webviewAnalysisArmTrackId = null;
        webviewObservedAnalysisStatus = "Whole-track capture could not start for this WebView source.";
      }
    } catch (failure) {
      if (webviewAnalysisArmTrackId === track.id) {
        webviewAnalysisArmTrackId = null;
        webviewObservedAnalysisStatus = `Whole-track capture could not start: ${failure instanceof Error ? failure.message : String(failure)}`;
      }
    }
  }
  async function beginWebviewObservedCapture(
    node: HTMLAudioElement,
    source: WebviewResolvedPreview,
    request: number,
    module: typeof import("./lib/loudness-normalization"),
  ): Promise<void> {
    const track = playingTrack, candidate = playingCandidate, identity = currentWebviewStreamCaptureIdentity();
    if (webviewAnalysisArmTrackId !== track?.id) return;
    if (!track || !candidate || !identity || source.normalizationRoute !== "same-origin-relay" ||
        source.isExcerpt || source.approximateExcerpt || node.currentTime > 0.05 || node.playbackRate !== 1) {
      webviewAnalysisArmTrackId = null;
      webviewObservedAnalysisStatus = "This WebView route cannot prove continuous whole-track coverage.";
      return;
    }
    const generation = ++webviewAnalysisGeneration;
    const sessionId = crypto.randomUUID();
    let capture: WebviewObservedStreamCapture | null = null;
    const attached = await module.attachWebAudioPcmTap(node, async event => {
      if (event.kind === "gap") {
        if (capture && webviewObservedCapture === capture) cancelWebviewObservedCapture(`Whole-track capture stopped: ${event.reason}`);
        return;
      }
      if (!capture || webviewObservedCapture !== capture ||
          (!capture.finishing && (!previewRequests.isCurrent(request) || playingTrack?.id !== capture.trackId || node !== audio || node.paused)) ||
          node.playbackRate !== 1) {
        throw new Error("Playback changed, paused, or left normal speed during whole-track capture.");
      }
      const batch = event.batch;
      if (batch.sequence !== capture.nextSequence) throw new Error("A WebAudio PCM batch was dropped.");
      const nextPosition = capture.expectedPositionSeconds + batch.samples.length / batch.sampleRate;
      if (!capture.finishing && Math.abs(node.currentTime - nextPosition) > 0.15) throw new Error("WebView source time diverged from captured PCM.");
      await call("push_webview_stream_analysis", {
        sessionId, generation, endPositionSeconds: nextPosition, sampleRate: batch.sampleRate,
        samplesBase64: module.encodeWebAudioPcm16Base64(batch.samples),
      });
      capture.expectedPositionSeconds = nextPosition;
      capture.nextSequence++;
    });
    if (!attached.tap) {
      webviewAnalysisArmTrackId = null;
      webviewObservedAnalysisStatus = attached.reason ?? "Bounded WebAudio PCM capture is unavailable.";
      return;
    }
    capture = {
      sessionId, generation, trackId: track.id, node, tap: attached.tap,
      durationSeconds: identity.durationSeconds, expectedPositionSeconds: 0, nextSequence: 0, started: false,
    };
    try {
      await call("begin_webview_stream_analysis", {
        sessionId, generation, recordingId: track.id, provider: identity.provider, sourceId: identity.sourceId,
        durationSeconds: identity.durationSeconds, audioVariant: identity.audioVariant, initialPositionSeconds: node.currentTime,
      });
      if (!previewRequests.isCurrent(request) || node !== audio || playingTrack?.id !== track.id) {
        attached.tap.stop();
        await call("cancel_webview_stream_analysis", { sessionId }).catch(() => {});
        return;
      }
      webviewObservedCapture = capture;
      webviewAnalysisArmTrackId = null;
      webviewObservedAnalysisStatus = "Capturing this full WebView stream; pausing or seeking will discard the capture.";
    } catch (failure) {
      attached.tap.stop();
      webviewAnalysisArmTrackId = null;
      webviewObservedAnalysisStatus = `Whole-track capture is unavailable: ${failure instanceof Error ? failure.message : String(failure)}`;
    }
  }
  async function finishWebviewObservedCapture(node: HTMLAudioElement): Promise<void> {
    const capture = webviewObservedCapture;
    if (!capture || capture.node !== node || capture.finishing) return;
    capture.finishing = true;
    const naturalEndPosition = node.currentTime;
    try {
      await capture.tap.finish();
      if (Math.abs(naturalEndPosition - capture.durationSeconds) > Math.max(0.05, capture.durationSeconds * 0.001) ||
          Math.abs(capture.expectedPositionSeconds - capture.durationSeconds) > Math.max(0.05, capture.durationSeconds * 0.001)) {
        throw new Error("The WebView stream ended without verified full-track PCM coverage.");
      }
      const result = await call<WholeTrackAnalysisDto>("finish_webview_stream_analysis", {
        sessionId: capture.sessionId, generation: capture.generation,
        naturalEndPositionSeconds: naturalEndPosition,
      });
      if (result.scope !== "wholeTrack" || result.resource?.recordingId !== capture.trackId || result.coverage < 0.999) {
        throw new Error("The backend rejected incomplete whole-track coverage.");
      }
      webviewObservedAnalysis = result;
      webviewObservedAnalysisStatus = "Complete WebView BPM/key analysis is cached. Track metadata was not changed.";
    } catch (failure) {
      void call("cancel_webview_stream_analysis", { sessionId: capture.sessionId }).catch(() => {});
      webviewObservedAnalysisStatus = `WebView analysis was not cached: ${failure instanceof Error ? failure.message : String(failure)}`;
    } finally { capture.tap.stop(); if (webviewObservedCapture === capture) webviewObservedCapture = null; }
  }
  function unavailableWebviewLoudness(reason: string): WebviewLoudnessDecision {
    return { mode: webviewLoudnessSettings.mode, status: "unavailable", appliedGainDb: 0, reason };
  }
  async function resolveWebviewLoudnessDecision(
    source: WebviewResolvedPreview,
    albumResource: LoudnessResourceIdentity | null = activeWebviewLoudnessAlbumResource,
  ): Promise<WebviewLoudnessDecision> {
    const config = await ensureWebviewLoudnessSettings();
    if (config.mode === "off") return { mode: "off", status: "off", appliedGainDb: 0 };
    if (source.normalizationRoute !== "same-origin-relay") {
      return unavailableWebviewLoudness("This source has no verified same-origin audio route. Normalization is unavailable until the next supported playback.");
    }
    if (!source.assetId) {
      return unavailableWebviewLoudness("No trusted loudness measurement is available for this stream.");
    }
    const track = playingTrack;
    const range = track?.playbackRange;
    try {
      const decision = await call<WebviewLoudnessDecision | null>("get_loudness_decision_for_asset", {
        assetId: source.assetId,
        range: { startSeconds: range?.startTime ?? 0, endSeconds: range?.endTime ?? null },
        albumResource: config.mode === "album" ? albumResource : null,
      });
      if (decision && ["off", "track", "album"].includes(decision.mode) &&
          ["off", "applied", "album_fallback_track", "unavailable"].includes(decision.status) &&
          Number.isFinite(decision.appliedGainDb)) return decision;
      return unavailableWebviewLoudness("The backend did not return a valid loudness decision.");
    } catch (failure) {
      return unavailableWebviewLoudness(failure instanceof Error ? failure.message : "The loudness decision is unavailable.");
    }
  }
  async function refreshCurrentWebviewNormalization(savedSettings?: WebviewLoudnessSettings): Promise<void> {
    if (savedSettings) {
      webviewLoudnessSettings = savedSettings;
      webviewLoudnessSettingsLoaded = true;
    } else {
      webviewLoudnessSettingsLoaded = false;
      await ensureWebviewLoudnessSettings();
    }
    const node = audio;
    const source = preview as WebviewResolvedPreview | null;
    if (!node || !source || nativeActive()) return;
    const decision = await resolveWebviewLoudnessDecision(source);
    webviewLoudnessDecision = decision;
    const module = await import("./lib/loudness-normalization");
    const graphExists = module.hasWebAudioGainGraph(node);
    const route = source.normalizationRoute ?? "unsupported";
    if (graphExists && route !== "same-origin-relay") {
      webviewNormalizationRouteStatus = "Normalization is unavailable for this audio route.";
      return;
    }
    if (webviewLoudnessSettings.mode === "off" && !graphExists) {
      webviewNormalizationRouteStatus = null;
      return;
    }
    const adapter = webviewGainAdapter ??= new module.WebAudioGainAdapter();
    const result = graphExists
      ? { status: "supported" as const }
      : await adapter.attach(node, route, decision.status === "unavailable" ? 0 : decision.appliedGainDb, volume);
    if (result.status === "supported") {
      if (graphExists) adapter.setGainDb(decision.status === "unavailable" ? 0 : decision.appliedGainDb, 150);
      webviewNormalizationRouteStatus = "Same-origin Web Audio route active.";
    } else {
      webviewNormalizationRouteStatus = result.reason ?? "Web Audio normalization is unavailable.";
      if (webviewLoudnessSettings.mode === "off") webviewLoudnessDecision = { mode: "off", status: "off", appliedGainDb: 0 };
    }
  }
  async function ensureWebviewWaveformRoute(): Promise<boolean> {
    const node = audio;
    const source = preview as WebviewResolvedPreview | null;
    if (!node || !source || nativeActive() || source.normalizationRoute !== "same-origin-relay") return false;
    const trackId = playingTrack?.id;
    const isCurrent = () => audio === node && preview?.url === source.url && playingTrack?.id === trackId && !nativeActive();
    const module = await import("./lib/loudness-normalization");
    if (!isCurrent()) return false;
    if (module.hasWebAudioGainGraph(node)) return module.hasWebAudioWaveformTap(node);
    if ((node.currentSrc || node.getAttribute("src")) && node.crossOrigin !== "anonymous") return false;
    const decision = await resolveWebviewLoudnessDecision(source);
    if (!isCurrent()) return false;
    const adapter = webviewGainAdapter ??= new module.WebAudioGainAdapter();
    const attached = await adapter.attach(node, "same-origin-relay", decision.status === "unavailable" ? 0 : decision.appliedGainDb, volume);
    if (!isCurrent()) return false;
    webviewLoudnessDecision = decision;
    if (attached.status === "supported") {
      webviewNormalizationRouteStatus = "Same-origin Web Audio route active.";
      return module.hasWebAudioWaveformTap(node);
    }
    webviewNormalizationRouteStatus = attached.reason ?? "Web Audio waveform observation is unavailable.";
    return false;
  }
  async function prepareAudioStream(request:number, stillCurrent: () => boolean = () => true) {
    const node=audio, value=preview;
    if(!node || !value || !previewRequests.isCurrent(request) || !stillCurrent())throw new Error("Audio setup cancelled");
    audioStreamHandle?.destroy();audioStreamHandle=null;lastHlsFailure="";
    const isCurrent=()=>stillCurrent()&&previewRequests.isCurrent(request)&&audio===node&&preview?.url===value.url;
    const config = await ensureWebviewLoudnessSettings();
    if (!isCurrent()) throw new Error("Audio setup cancelled");
    const resolved = value as WebviewResolvedPreview;
    const normalization = await resolveWebviewLoudnessDecision(resolved);
    if (!isCurrent()) throw new Error("Audio setup cancelled");
    webviewLoudnessDecision = normalization;
    const module = await import("./lib/loudness-normalization");
    if (!isCurrent()) throw new Error("Audio setup cancelled");
    const graphExists = module.hasWebAudioGainGraph(node);
    const route = resolved.normalizationRoute ?? "unsupported";
    const captureArmed = webviewAnalysisArmTrackId === playingTrack?.id;
    const shouldNormalize = config.mode !== "off" || graphExists || waveformEnabled || captureArmed ||
      route === "same-origin-relay" && !playingRadioStation;
    if (shouldNormalize && route !== "same-origin-relay") {
      webviewNormalizationRouteStatus = "Normalization is unavailable for this audio route. It can be applied after the next supported playback.";
      if (graphExists) throw new Error("The existing Web Audio route cannot play an unsupported audio source.");
    } else if (shouldNormalize) {
      // At a source transition, retire the old URL before changing CORS mode or
      // creating the permanent MediaElementSource graph.
      if (!graphExists && (node.currentSrc || node.getAttribute("src"))) {
        node.pause();
        node.removeAttribute("src");
        node.load();
      }
      node.crossOrigin = "anonymous";
      const adapter = webviewGainAdapter ??= new module.WebAudioGainAdapter();
      const initialGainDb = config.mode === "off" || normalization.status === "unavailable" ? 0 : normalization.appliedGainDb;
      const attached = await adapter.attach(node, route, initialGainDb, volume);
      if (!isCurrent()) throw new Error("Audio setup cancelled");
      if (attached.status !== "supported") {
        webviewNormalizationRouteStatus = attached.reason ?? "Web Audio normalization is unavailable.";
        if (module.hasWebAudioGainGraph(node)) throw new Error(attached.reason ?? "The existing Web Audio output route could not resume.");
      } else {
        webviewNormalizationRouteStatus = "Same-origin Web Audio route active.";
      }
    } else {
      webviewNormalizationRouteStatus = null;
    }
    if (captureArmed) {
      if (route === "same-origin-relay" && module.hasWebAudioGainGraph(node)) {
        await beginWebviewObservedCapture(node, resolved, request, module);
        if (!isCurrent()) throw new Error("WebView analysis capture setup was cancelled.");
      } else {
        webviewAnalysisArmTrackId = null;
        webviewObservedAnalysisStatus = "This source has no verified same-origin WebAudio route for whole-track capture.";
      }
    }
    // Backend-confirmed routes opt into CORS. The legacy route check remains
    // for existing playable relays whose DTO predates normalization metadata.
    let relaySource = route === "same-origin-relay";
    if (resolved.normalizationRoute === undefined) {
      try { const url = new URL(value.url); relaySource = url.protocol === "http:" && url.hostname === "127.0.0.1" && (url.pathname.startsWith("/webview-audio/") || url.pathname.startsWith("/webview-radio-mp3/")) && Boolean(url.searchParams.get("token")); } catch {}
    }
    if (relaySource) node.crossOrigin = "anonymous"; else node.removeAttribute("crossorigin");
    const handle=await attachAudioStream(node,playable(value.url),value.audioMimeType,{isCurrent,onError:(failure)=>{if(isCurrent())lastHlsFailure=safePreviewError(failure);}});
    if(!isCurrent()){handle.destroy();throw new Error("Audio setup cancelled");}
    audioStreamHandle=handle;
  }
  const webviewAudioCapabilities = createWebviewAudioCapabilitiesProbe();
  function resolvePlaybackWithMediaWarmup(args: Record<string, unknown>): Promise<PlaybackResolution> {
    // First-use canPlayType() can synchronously scan the platform media registry
    // for several seconds. Dispatch source lookup first so its network work runs
    // during initialization, rather than spending the startup budget serially.
    const pending = call<PlaybackResolution>("resolve_playback_track", args);
    webviewAudioCapabilities();
    return pending;
  }
  function boundedPreviewResolution<T>(request: number, operation: Promise<T>, deadlineOverride?: number): Promise<T | undefined> {
    const deadline = deadlineOverride ?? (automaticPreviewRecovery?.request === request ? automaticPreviewRecovery.deadline : undefined);
    return withinPreviewStartupBudget(() => previewRequests.resolve(request, operation), deadline, () => previewRequests.isCurrent(request));
  }
  function playbackStartupTimeoutSeconds(): number {
    return [20, 30, 45].includes(Number(settings.playbackStartupTimeoutSeconds)) ? Number(settings.playbackStartupTimeoutSeconds) : 20;
  }
  function playbackStartupTimeoutMs(): number { return playbackStartupTimeoutSeconds() * 1000; }
  const finalSourceResolutionReserveMs = 3_000;
  const candidateStartupWindowMs = 5_000;
  function recoveryProviderLabel(provider: string): string { return ({youtube:"YouTube",monochrome:"Monochrome",soundcloud:"SoundCloud",bandcamp:"Bandcamp"} as Record<string,string>)[provider] ?? provider; }
  function sourceIsAuthoritative(track: Track): boolean {
    return hasExplicitSourceLock(track, {selectedCandidate:queueCandidates[track.id],audioSearchResult:audioSearchTrackIds.has(track.id)});
  }
  function firstSourceStartupDeadline(recovery: NonNullable<typeof automaticPreviewRecovery>): number {
    const overallDeadline = recovery.deadline ?? performance.now() + playbackStartupTimeoutMs();
    return overallDeadline;
  }
  function sourceAttemptDeadline(recovery: NonNullable<typeof automaticPreviewRecovery>, attempt: number): number {
    const overall = recovery.deadline ?? performance.now() + playbackStartupTimeoutMs();
    if (recovery.exactSourceOnly || attempt > 1) return overall;
    // Resolver latency can consume the preferred reserve. Give a newly
    // resolved candidate a real chance to start, while preserving time for
    // one alternate whenever the original deadline permits it.
    const now = performance.now();
    return Math.min(overall, Math.max(now + 2_000, Math.min(now + candidateStartupWindowMs, overall - finalSourceResolutionReserveMs)));
  }
  // Keep late play settlements and their queued media events from reviving a cancelled source.
  const blockedStartupNodes = new WeakSet<HTMLAudioElement>();
  const startupPlayOwners = new WeakMap<HTMLAudioElement, object>();
  let cancelWebviewStartupPlay: (() => void) | null = null;
  function ownedStartupPlay(node: HTMLAudioElement): Promise<void> {
    const owner = {};
    startupPlayOwners.set(node, owner);
    blockedStartupNodes.delete(node);
    const cancel = () => {
      if (startupPlayOwners.get(node) !== owner) return;
      blockedStartupNodes.add(node);
      node.pause();
    };
    cancelWebviewStartupPlay = cancel;
    const promise = node.play();
    void promise.then(() => {
      if (startupPlayOwners.get(node) === owner && blockedStartupNodes.has(node)) node.pause();
    }, () => {});
    return promise;
  }
  async function withinPreviewStartupBudget<T>(operation: () => Promise<T>, deadline: number | undefined, stillCurrent: () => boolean): Promise<T> {
    if (!stillCurrent()) throw new Error("Audio startup was cancelled.");
    deadline = Number.isFinite(deadline) ? deadline! : performance.now() + playbackStartupTimeoutMs();
    const remaining = Math.floor(deadline! - performance.now());
    if (remaining <= 0) throw new Error(`Audio startup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline.`);
    const pending = operation();
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let cancellationCheck: ReturnType<typeof setInterval> | undefined;
    try {
      const value = await Promise.race([
        pending,
        new Promise<T>((_, reject) => {
          timeout = setTimeout(() => reject(new Error(`Audio startup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline.`)), remaining);
          cancellationCheck = setInterval(() => {
            if (!stillCurrent()) reject(new Error("Audio startup was cancelled."));
          }, 25);
        }),
      ]);
      if (!stillCurrent()) throw new Error("Audio startup was cancelled.");
      if (performance.now() >= deadline!) throw new Error(`Audio startup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline.`);
      return value;
    } finally {
      if (timeout !== undefined) clearTimeout(timeout);
      if (cancellationCheck !== undefined) clearInterval(cancellationCheck);
    }
  }
  $effect(()=>{if(!preview || !["audio","local","direct"].includes(preview.kind)){audioStreamHandle?.destroy();audioStreamHandle=null;}});
  onMount(()=>()=>{previewRequests.next();audioStartupGeneration++;cancelWebviewStartupPlay?.();cancelPendingWebviewResume();if(webviewStartupResumeAudioGate)releaseWebviewStartupResumeGate(webviewStartupResumeAudioGate.node,webviewStartupResumeAudioGate.source,false,false);if(nativeHistorySummaryTimer!==null)clearTimeout(nativeHistorySummaryTimer);nativeHistorySummaryTimer=null;pendingNativeHistorySummaryOwner="";webviewStallWatchdog.reset();webviewSeekCompletionWatchdog.cancel();webviewPostSeekProgress.cancel();pendingPostSeekProgress=null;activeWebviewSeek=null;cancelRadioIdentification();audioStreamHandle?.destroy();audioStreamHandle=null;});
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
  function skippedPreviewFailureNotice(track: Track, reason: string): string {
    const safeReason = safePreviewError(reason);
    const label = (value: unknown, fallback: string, maximum: number) => {
      const text = safeNativeFailurePart(value, fallback).replace(/\s+/g, " ");
      if (text.length <= maximum) return text;
      const prefix = text.slice(0, maximum - 1), boundary = prefix.lastIndexOf(" ");
      return `${prefix.slice(0, boundary > 0 ? boundary : prefix.length)}…`;
    };
    const title = label(track.title, "Unavailable recording", 120);
    const version = label(track.version, "", 80);
    const category = trackFailureCategory(reason);
    if (category === "no_audio_results" || category === "recording_identity_unverified") {
      return `Skipped ${title}${version ? ` (${version})` : ""}: ${safeReason}`;
    }
    const missingDurationExplanation = "Saved recording duration is missing; no source provided enough verified identity evidence for automatic playback.";
    if (track.durationSeconds == null && safeReason === `No playable audio match was found. ${missingDurationExplanation}`) {
      return `Skipped ${title}${version ? ` (${version})` : ""}: ${missingDurationExplanation} Choose another source from the track actions.`;
    }
    if (!/^No playable audio match was found(?: for\b|[.:]|$)/i.test(safeReason)) {
      return `Skipped ${title}: ${safeReason} Choose another source from the track actions.`;
    }
    // This known resolver category already contains its own action. The shared
    // 180-character sanitizer may cut that action mid-word; compose one complete
    // bounded explanation instead of appending another instruction to its tail.
    return `Skipped ${title}: No playable audio match was found for the requested recording and version${version ? ` (${version})` : ""}. Choose another source from the track actions.`;
  }
  function mediaAudioFailureReason(): string {
    if(lastHlsFailure)return lastHlsFailure;
    const code = audio?.error?.code;
    if (code === 2) return "The source could not be loaded because of a network error.";
    if (code === 3) return "The source audio could not be decoded.";
    if (code === 4) return "The source did not provide a supported audio format.";
    return audio?.error?.message || "The selected source did not provide playable audio.";
  }

  async function resolveCandidateAudio(t: Track, candidate: Candidate, request: number) {
    if (!previewRequests.isCurrent(request)) return false;
    if (candidate.provider === "monochrome") {
      const inspected = await boundedPreviewResolution(request, call<Candidate>("inspect_candidate", {track: t, candidate}));
      if (!inspected) return false;
      candidate = inspected;
    }
    const deadline = automaticPreviewRecovery?.request === request
      ? automaticPreviewRecovery.sourceStartupDeadline ?? automaticPreviewRecovery.deadline : undefined;
    const budgetMs = Math.floor((deadline ?? performance.now() + playbackStartupTimeoutMs()) - performance.now());
    if (budgetMs <= 0) throw new Error(`Audio startup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline.`);
    const normalizationRouteRequired = await isWebviewNormalizationRouteRequired();
    const result = await boundedPreviewResolution(request, call<WebviewResolvedPreview>("resolve_candidate_audio", {
      candidate, requestGeneration: request, trackId: t.id, budgetMs, normalizationRouteRequired,
      trackContext: streamMediaTrackContext(t, candidate.provider, candidate.id),
    }));
    return result ? startResolvedPreviewAudio(t, candidate, result, request) : false;
  }

  async function upgradeCurrentWebviewWaveformRoute(): Promise<void> {
    const track = playingTrack;
    const candidate = playingCandidate;
    const source = preview as WebviewResolvedPreview | null;
    const node = audio;
    const durationSeconds = currentStreamMediaResource()?.durationSeconds ?? 0;
    if (!waveformEnabled || !desktop() || nativeActive() || !track || !candidate || !source || !node ||
        source.normalizationRoute === "same-origin-relay" || source.isExcerpt || source.approximateExcerpt ||
        source.sessionId || !Number.isSafeInteger(source.sourceGeneration) ||
        track.playbackRange || !["youtube", "soundcloud", "bandcamp", "monochrome"].includes(candidate.provider) ||
        !/^[A-Za-z0-9:_-]{1,512}$/.test(candidate.id) || !Number.isFinite(durationSeconds) || durationSeconds < 8 || durationSeconds > 900 ||
        resolvingPlayback || playbackLoading || bufferingAudio || pendingWebviewResume) return;
    const requestOwner = ++waveformRouteUpgradeRequest;
    waveformRouteUpgradeStatus = "Preparing a waveform-capable route for this stream…";
    const wasPaused = paused;
    const initialSeekSerial = seekSerial;
    // The last startup play owner remains installed after settling; retire that
    // stale cancel closure so the next generation does not pause healthy media.
    cancelWebviewStartupPlay = null;
    const previousRecovery = automaticPreviewRecovery;
    const previousAttachment = successfulRecoveryAttachment;
    const request = nextPreviewRequest();
    const sourceOwner = { ...source, sourceGeneration: request };
    // Advance the frontend generation while retaining the current direct URL as
    // a fallback until the exact-candidate relay is ready. This prevents late
    // events from the pre-upgrade generation owning the active source.
    preview = sourceOwner;
    const isCurrent = () => requestOwner === waveformRouteUpgradeRequest && waveformEnabled && !nativeActive() &&
      audio === node && playingTrack?.id === track.id && playingCandidate?.provider === candidate.provider && playingCandidate.id === candidate.id &&
      preview?.url === sourceOwner.url &&
      preview?.sourceGeneration === request && seekSerial === initialSeekSerial && paused === wasPaused;
    const deadline = performance.now() + playbackStartupTimeoutMs();
    // The old route is still playing during this optional lookup. Transfer its
    // recovery ownership without resetting its retry history or deadline.
    automaticPreviewRecovery = previousRecovery ? { ...previousRecovery, request } : undefined;
    successfulRecoveryAttachment = previousAttachment && previousAttachment.request === previousRecovery?.request
      ? { ...previousAttachment, request } : null;
    armWebviewStallWatchdog(node);
    try {
      let routeCandidate = candidate;
      if (candidate.provider === "monochrome") {
        const inspected = await boundedPreviewResolution(request, call<Candidate>("inspect_candidate", { track, candidate }), deadline);
        if (!inspected || !isCurrent() || inspected.provider !== candidate.provider || inspected.id !== candidate.id) return;
        routeCandidate = inspected;
      }
      const remaining = Math.floor(deadline - performance.now());
      if (remaining <= 0) throw new Error("Audio startup deadline expired.");
      const resolved = await boundedPreviewResolution(request, call<WebviewResolvedPreview>("resolve_candidate_audio", {
        candidate: routeCandidate, requestGeneration: request, trackId: track.id, budgetMs: remaining, normalizationRouteRequired: true,
        trackContext: streamMediaTrackContext(track, routeCandidate.provider, routeCandidate.id),
      }), deadline);
      if (!resolved || !isCurrent()) return;
      if (!resolved.url || !["audio", "direct"].includes(resolved.kind) || resolved.normalizationRoute !== "same-origin-relay") {
        waveformRouteUpgradeStatus = "This exact stream cannot provide WebView waveform samples on a same-origin route.";
        return;
      }
      const playhead = Number.isFinite(node.currentTime) && node.currentTime >= 0 ? node.currentTime : 0;
      // Keep the current source playing through resolution; freeze only at the
      // verified same-candidate route swap so the replacement resumes here.
      node.pause();
      paused = true;
      position = playhead;
      pendingResumePosition = playhead;
      if (!requestOwnerIsCurrentAfterFreeze()) return;
      automaticPreviewRecovery = {
        request, track, tried: new Set(), attempts: 0, sourceAttempts: 0, deadline,
        exactSourceOnly: true,
      };
      const started = await startResolvedPreviewAudio(track, routeCandidate, resolved, request, wasPaused);
      if (started && preview?.url === resolved.url && playingTrack?.id === track.id && requestOwner === waveformRouteUpgradeRequest) {
        waveformRouteUpgradeStatus = "Waveform-capable route active for this stream.";
      } else if (!started && requestOwner === waveformRouteUpgradeRequest) {
        automaticPreviewRecovery = undefined;
        waveformRouteUpgradeStatus = "The waveform-capable route could not start; retry playback to restore the stream.";
      }
    } catch {
      if (isCurrent()) waveformRouteUpgradeStatus = "Could not prepare a waveform-capable route for this stream.";
    }
    function requestOwnerIsCurrentAfterFreeze(): boolean {
      return requestOwner === waveformRouteUpgradeRequest && waveformEnabled && !nativeActive() &&
        audio === node && playingTrack?.id === track?.id && playingCandidate?.provider === candidate?.provider && playingCandidate?.id === candidate?.id &&
        preview?.url === sourceOwner.url &&
        preview?.sourceGeneration === request && seekSerial === initialSeekSerial && paused;
    }
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
    void upgradeCurrentWebviewWaveformRoute();
  }

  async function startResolvedPreviewAudio(t: Track, candidate: Candidate | null, result: Preview, request: number, startPaused = false) {
    if (!["audio", "local", "direct"].includes(result.kind) || !result.url || !previewRequests.isCurrent(request)) return false;
    startupNativeHandoffReceipt = null;
    rememberResolvedSource(candidate, result);
    webviewLoadedMetadataOwner = null;
    preview = result;
    playingTrack = t;
    playingCandidate = candidate;
    paused = true;
    position = pendingResumePosition;
    duration = 0;
    previewStatus = "Loading verified audio…";
    await tick();
    if (!previewRequests.isCurrent(request) || playingTrack?.id !== t.id || !audio) return false;
    const node = audio;
    const source = preview;
    const startupGeneration = ++audioStartupGeneration;
    const recovery = automaticPreviewRecovery;
    const recoveryDeadline = recovery?.request === request && recovery.track.id === t.id ? recovery.deadline : undefined;
    const deadline = recoveryDeadline ?? performance.now() + playbackStartupTimeoutMs();
    const sourceDeadline = automaticPreviewRecovery?.request === request && automaticPreviewRecovery.track.id === t.id
      ? automaticPreviewRecovery.sourceStartupDeadline ?? deadline : deadline;
    let attemptActive = true;
    const startupStartedAt = performance.now();
    const resumeRequested = pendingResumePosition > 0;
    let startupPhase: WebviewStartupPhase = 'attach';
          const startupStages = startupStageTimings();
          startupStages.mark('attach');
    const stillCurrent = () => attemptActive && previewRequests.isCurrent(request) && playingTrack?.id === t.id && audio === node && preview === source && audioStartupGeneration === startupGeneration;
    const startupHandoffIsCurrent = () => {
      const receipt = startupNativeHandoffReceipt;
      if (!receipt || !receipt.committed || receipt.request !== request || receipt.startupGeneration !== startupGeneration ||
          receipt.sourceGeneration !== source.sourceGeneration || receipt.trackId !== t.id || !receipt.sessionId ||
          receipt.node !== node || receipt.source !== source || !previewRequests.isCurrent(request) ||
          (audio !== node && audio !== null) || !blockedStartupNodes.has(node) || audioStartupGeneration !== startupGeneration ||
          playingTrack?.id !== t.id || playbackSession !== receipt.sessionId || !nativeActive() ||
          nativeSnapshot?.handoffId !== receipt.handoffId || nativeSnapshot.trackId !== receipt.trackId ||
          nativeSnapshot.entryId !== receipt.entryId || nativeSnapshot.candidate?.id !== receipt.candidateId) return false;
      const queue = playbackQueue.snapshot();
      const entryIds = queue.entryIds ?? [];
      return queue.currentIndex === receipt.index && queue.currentIndex === queueIndex &&
        queue.tracks[queue.currentIndex]?.id === receipt.trackId && entryIds[receipt.index] === receipt.entryId &&
        entryIds.length === receipt.queueEntryIds.length && entryIds.every((id, index) => id === receipt.queueEntryIds[index]) &&
        (!nativeSnapshot.queueEntryIds || (nativeSnapshot.queueEntryIds.length === receipt.queueEntryIds.length &&
          nativeSnapshot.queueEntryIds.every((id, index) => id === receipt.queueEntryIds[index])));
    };
    const startupStillCurrent = () => stillCurrent() || startupHandoffIsCurrent();
    let progressCheck: ReturnType<typeof setInterval> | undefined;
    setWebviewOutputVolume(node, volume);
    if (resumeRequested && !startPaused && pendingResumePosition > 0) {
      webviewStartupResumeAudioGate = {node,source,trackId:t.id,target:pendingResumePosition,startedAtMs:performance.now(),requestedVolume:volume,verified:false};
      applyWebviewOutputVolume(node,0);
    }
    try {
      await withinPreviewStartupBudget(() => prepareAudioStream(request, stillCurrent), sourceDeadline, stillCurrent);
      startupPhase = 'resume_seek'; startupStages.mark('resume_seek');
      await withinPreviewStartupBudget(() => seekPendingResume(request, stillCurrent), sourceDeadline, stillCurrent);
      if (!stillCurrent()) return false;
      if (startPaused) {
        pendingResumePosition = 0;
        position = node.currentTime;
        paused = true;
        previewStatus = "Paused";
        if (candidate) void prepareCandidateDetails(t, candidate, request);
        return true;
      }
      if (resumeRequested && pendingResumePosition > 0) {
        beginWebviewSeekCompletionWatch(node, pendingResumePosition, true, request);
      }
      startupPhase = 'play'; startupStages.mark('play');
      if (requiredRecoveryResume?.request === request && requiredRecoveryResume.verifiedAtMs === null) {
        requiredRecoveryResume = { ...requiredRecoveryResume, verifiedAtMs: performance.now() };
      }
      let playPromise: Promise<void>;
      await withinPreviewStartupBudget(() => {
        const startPosition = node.currentTime;
        playPromise = ownedStartupPlay(node);
        void playPromise.then(() => {
          if (stillCurrent()) { startupPhase = 'clock_progress'; startupStages.mark('clock_progress'); }
        }, () => {});
        if (resumeRequested) void playPromise.then(() => {
          const seek = activeWebviewSeek;
          if (seek?.owner.node === node && seek.source === source && seek.startupResume && isCurrentWebviewSeek(seek)) armPendingWebviewSeekAfterResume(node);
        }, () => {});
        void playPromise.then(() => {
          if (!attemptActive && startupGeneration === audioStartupGeneration) {
            node.pause();
            if (preview?.url === result.url) {
              audioStreamHandle?.destroy();
              audioStreamHandle = null;
            }
          }
        }, () => {});

        // A fulfilled play promise only proves that playback was requested. A
        // resumed attachment must leave seeking and advance this source's clock.
        const progressed = new Promise<void>((resolve, reject) => {
          void playPromise.catch(reject);
          progressCheck = setInterval(() => {
            if (stillCurrent() && requiredRecoveryResume?.request === request && node.currentTime < requiredRecoveryResume.position - 1) {
              reject(new Error("The recovered audio source could not resume at its previous playback position."));
              return;
            }
            if (startupHandoffIsCurrent()) { resolve(); return; }
            if (stillCurrent() && resumeRequested && hasWebviewStartupResumeProgress({
              target: pendingResumePosition,
              startPosition,
              currentTime: node.currentTime,
              duration: node.duration,
              elapsedMs: performance.now() - startupStartedAt,
              readyState: node.readyState,
              seeking: node.seeking,
              paused: node.paused,
              playbackRate: node.playbackRate,
            })) resolve();
            else if (stillCurrent() && !resumeRequested && !node.paused && !node.seeking && node.readyState >= 2 &&
                Number.isFinite(node.currentTime) && node.currentTime - startPosition >= 0.25) resolve();
          }, 25);
        });
        return progressed;
      }, sourceDeadline, resumeRequested ? startupStillCurrent : stillCurrent);
      if (progressCheck !== undefined) { clearInterval(progressCheck); progressCheck = undefined; }
      const startupHandoffAccepted = startupHandoffIsCurrent();
      if (!stillCurrent() && !startupHandoffAccepted) return false;
      if (startupHandoffAccepted) {
        attemptActive = false;
        releaseWebviewStartupResumeGate(node, source, false, false);
        position = nativeSnapshot?.position ?? pendingResumePosition;
        pendingResumePosition = 0;
        const receipt = startupNativeHandoffReceipt as StartupNativeHandoffReceipt | null;
        if (receipt) receipt.consumed = true;
        if (pendingWebviewHandoff?.handoffId !== receipt?.handoffId) startupNativeHandoffReceipt = null;
        return true;
      }
      if (activeWebviewSeek?.owner.node === node && activeWebviewSeek.source === source && activeWebviewSeek.startupResume) {
        webviewSeekCompletionWatchdog.complete(activeWebviewSeek.owner);
        activeWebviewSeek = null;
      }
      releaseWebviewStartupResumeGate(node, source, true, false);
      const requiredResume = requiredRecoveryResume;
      if (requiredResume?.request === request) {
        const elapsedSeconds = requiredResume.verifiedAtMs === null ? 0 : Math.max(0, (performance.now() - requiredResume.verifiedAtMs) / 1000);
        if (!Number.isFinite(node.currentTime) || node.currentTime < requiredResume.position - 1 ||
            node.currentTime > requiredResume.position + elapsedSeconds * 1.5 + 1) {
          throw new Error("The recovered audio source could not resume at its previous playback position.");
        }
      }
      successfulRecoveryAttachment = { request, startedAt: performance.now(), position: node.currentTime, seekSerial };
      pendingResumePosition = 0;
      if (requiredRecoveryResume?.request === request) requiredRecoveryResume = null;
      paused = false;
      previewStatus = result.isExcerpt ? "Playing preview" : `Playing ${candidate?.provider ?? "verified"} audio`;
      armWebviewStallWatchdog(node);
      if (candidate) void prepareCandidateDetails(t, candidate, request);
      if (resumeRequested) void recordSuccessfulPlayback();
      return true;
    } catch (failure) {
      if (progressCheck !== undefined) clearInterval(progressCheck);
      const ownsSource = stillCurrent();
      releaseWebviewStartupResumeGate(node, source, false, ownsSource && resumeRequested);
      if (ownsSource) {
        if (activeWebviewSeek?.owner.node === node && activeWebviewSeek.source === source && activeWebviewSeek.startupResume) {
          webviewSeekCompletionWatchdog.cancel();
          activeWebviewSeek = null;
        }
        recordWebviewStartupFailure(captureWebviewStartupFailure(node, startupPhase, result.audioMimeType, audioStreamHandle?.kind, resumeRequested, performance.now() - startupStartedAt, pendingResumePosition, result.sourceTransport, startupStages.snapshot()));
      }
      attemptActive = false;
      if (ownsSource) {
        blockedStartupNodes.add(node);
        node.pause();
        audioStreamHandle?.destroy();
        audioStreamHandle = null;
      }
      if (ownsSource) {
        if (automaticPreviewRecovery?.request === request) automaticPreviewRecovery.failureReason = automaticPreviewRecovery.stallCause
          ? `${automaticPreviewRecovery.stallCause} A refreshed verified source could not start: ${safePreviewError(failure)}`
          : `Playback could not start: ${safePreviewError(failure)}`;
        preview = null;
        paused = true;
      }
      if (!ownsSource && startupHandoffIsCurrent()) {
        attemptActive = false;
        releaseWebviewStartupResumeGate(node, source, false, false);
        position = nativeSnapshot?.position ?? pendingResumePosition;
        pendingResumePosition = 0;
        const receipt = startupNativeHandoffReceipt as StartupNativeHandoffReceipt | null;
        if (receipt) receipt.consumed = true;
        if (pendingWebviewHandoff?.handoffId !== receipt?.handoffId) startupNativeHandoffReceipt = null;
        return true;
      }
      return false;
    }
  }

  type NativeSourceResolution = {transport: "native"; sourceHandle: string; sourceGeneration: number};
  type PlaybackResolution = NativeSourceResolution | {transport?: "webview"; preview: Preview; candidate: Candidate | null; quality?: {sourceKind?: string; approximateExcerpt?: import("./lib/types").ApproximateExcerptProvenance}};

  async function startNativeResolvedSource(t: Track, result: NativeSourceResolution, request: number): Promise<boolean> {
    if (!previewRequests.isCurrent(request) || result.sourceGeneration !== request || nativeActive() || pendingWebviewHandoff) return false;
    const queue = playbackQueue.snapshot(), entryIds = queue.entryIds ?? [];
    const entryId = entryIds[queue.currentIndex], sessionId = playbackSession;
    if (!sessionId || !entryId || queue.currentIndex !== queueIndex || queue.tracks[queueIndex]?.id !== t.id ||
        entryIds.length !== queue.tracks.length || playingTrack?.id !== t.id) return false;
    const handoff: ResolvedSourceHandoff = {
      handoffId:crypto.randomUUID(), sourceHandle:result.sourceHandle, sourceGeneration:request,
      seekSerial, targetPosition:pendingResumePosition, tracks:[...queue.tracks], entryIds:[...entryIds],
      index:queueIndex, sessionId, entryId, paused:false, volume, repeat:repeatMode, shuffle:shuffleEnabled,
      recorded:recordedSession === sessionId,
    };
    const pausedAtStart = paused;
    const isCurrent = () => {
      const current = playbackQueue.snapshot();
      return previewRequests.isCurrent(request) && !nativeActive() && playingTrack?.id === t.id &&
        playbackSession === sessionId && seekSerial === handoff.seekSerial && paused === pausedAtStart &&
        queueIndex === handoff.index && current.currentIndex === handoff.index &&
        current.entryIds?.length === handoff.entryIds.length && current.entryIds.every((id,index)=>id===handoff.entryIds[index]) &&
        current.tracks.length === handoff.tracks.length && current.tracks.every((track,index)=>track.id===handoff.tracks[index].id) &&
        volume === handoff.volume && repeatMode === handoff.repeat && shuffleEnabled === handoff.shuffle;
    };
    let settle!: () => void;
    const pending: PendingWebviewHandoff = {handoffId:handoff.handoffId,cancelled:false,isCurrent,settled:new Promise<void>(resolve=>{settle=resolve;})};
    pendingWebviewHandoff = pending;
    nativeResolutionGeneration = request;
    bufferingAudio = true;
    try {
      // Publish the exact resolving occurrence before the Rust owner check.
      await publishAutomationPlayback();
      if (pending.cancelled || !isCurrent()) return false;
      const snapshot = await nativeBridge.handoffResolvedSource(handoff);
      const committed = nativeActive() && nativeSnapshot?.handoffId === handoff.handoffId;
      if (pending.cancelled || (!committed && !isCurrent())) {
        await nativeBridge.command({action:"stop",expectedHandoffId:handoff.handoffId}).catch(()=>{});
        return false;
      }
      if (snapshot.engine !== "native" || snapshot.handoffId !== handoff.handoffId) throw new Error("The native player did not accept this recording.");
      if (nativeBridge.acceptSnapshot(snapshot) && nativeBridge.snapshot) applyNativeSnapshot(nativeBridge.snapshot);
      else if (nativeBridge.snapshot?.handoffId === handoff.handoffId) applyNativeSnapshot(nativeBridge.snapshot);
      else throw new Error("The native player did not confirm playback ownership.");
      webviewSeekCompletionWatchdog.cancel(); activeWebviewSeek = null; webviewStallWatchdog.reset();
      audioStreamHandle?.destroy(); audioStreamHandle = null;
      if (audio) { audio.pause(); audio.removeAttribute("src"); audio.load(); }
      pendingResumePosition = 0;
      return true;
    } finally {
      if (pendingWebviewHandoff === pending) pendingWebviewHandoff = null;
      nativeResolutionGeneration = null;
      settle();
    }
  }

  async function findAndStartPreviewSource(t: Track, request: number, recovering = false, attemptLimit?: 1) {
    const previous = automaticPreviewRecovery?.track.id === t.id ? automaticPreviewRecovery : undefined;
    const recovery: NonNullable<typeof automaticPreviewRecovery> = automaticPreviewRecovery = {
      request, track: t, tried: recovering ? previous?.tried ?? new Set<string>() : new Set<string>(),
      attempts: recovering ? previous?.attempts ?? 0 : 0,
      sourceAttempts: recovering ? previous?.sourceAttempts ?? 0 : 0,
      deadline: previous?.deadline ?? performance.now() + playbackStartupTimeoutMs(),
      exactSourceOnly: sourceIsAuthoritative(t),
      failureReason: previous?.failureReason,
      stallCause: recovering ? previous?.stallCause : undefined,
      recurringStalls: recovering ? previous?.recurringStalls : undefined,
      stallFallbacks: recovering ? previous?.stallFallbacks : undefined,
    };
    const overallDeadline = recovery.deadline ?? performance.now() + playbackStartupTimeoutMs();
    previewStatus = "Finding a verified playback source…";
    const maxSources = recovery.exactSourceOnly || attemptLimit ? 1 : 2;
    const sourceLimit = attemptLimit ? 1 : Math.min(maxSources, Math.max(0, maxSources - (recovery.sourceAttempts ?? 0)));
    for (let index = 0; index < sourceLimit; index++) {
      if (!previewRequests.isCurrent(request)) return;
      const sourceAttempt = (recovery.sourceAttempts ?? 0) + 1;
      const resolveDeadline = sourceAttempt === 1 || recovering || attemptLimit
        ? overallDeadline
        : Math.max(performance.now(), overallDeadline - finalSourceResolutionReserveMs);
      const budgetMs = Math.floor(resolveDeadline - performance.now());
      if (budgetMs <= 0) break;
      try {
        const result = await boundedPreviewResolution(request, resolvePlaybackWithMediaWarmup({
          track: t, requestGeneration: request, selectedCandidate: recovery.exactSourceOnly ? queueCandidates[t.id] ?? null : null, budgetMs,
          normalizationRouteRequired: await isWebviewNormalizationRouteRequired(),
          failedCandidates: recovery.exactSourceOnly ? [] : [...recovery.tried].slice(-2).map(key => {
            const separator = key.indexOf(":");
            return { provider: key.slice(0, separator), id: key.slice(separator + 1) };
          }),
          ...(attemptLimit ? { attemptLimit } : {}),
        }), resolveDeadline);
        if (!result || !previewRequests.isCurrent(request)) break;
        if (result.transport === "native") {
          if (await startNativeResolvedSource(t, result, request)) return;
          if (!previewRequests.isCurrent(request)) return;
          throw new Error("Playback ownership changed before native audio could start.");
        }
        const key = result.candidate ? candidateKey(result.candidate) : null;
        if (key && recovery.tried.has(key) && !recovery.exactSourceOnly) {
          recovery.failureReason = recovery.failureReason || "The resolver returned a playback source that was already tried.";
          break;
        }
        if (key) recovery.tried.add(key);
        recovery.sourceAttempts = sourceAttempt;
        recovery.attempts++;
        recovery.sourceStartupDeadline = recovering || attemptLimit
          ? overallDeadline : sourceAttemptDeadline(recovery, sourceAttempt);
        if (result.candidate) candidates = [result.candidate];
        const resolvedPreview = result.preview.kind === "local" && !result.preview.approximateExcerpt
          && result.quality?.sourceKind === "approximateAlbumExcerpt" && result.quality.approximateExcerpt
          ? { ...result.preview, approximateExcerpt: { label: result.quality.approximateExcerpt.label, source: result.quality.approximateExcerpt.source } }
          : result.preview;
        if (await startResolvedPreviewAudio(t, result.candidate, resolvedPreview, request)) return;
        if (!previewRequests.isCurrent(request)) return;
        if (!recovery.failureReason) recovery.failureReason = recovery.stallCause
          ? `${recovery.stallCause} The verified source did not start.`
          : "The verified source did not provide playable audio.";
        if (sourceAttempt >= maxSources || performance.now() >= overallDeadline) break;
      } catch (failure) {
        if (!previewRequests.isCurrent(request)) return;
        recovery.failureReason = recovery.stallCause
          ? `${recovery.stallCause} Another verified source could not be resolved: ${safePreviewError(failure)}`
          : safePreviewError(failure);
        break;
      }
    }
    if ((recovery.sourceAttempts ?? 0) === 0 && performance.now() >= overallDeadline) {
      recovery.failureReason = recovery.failureReason || `Audio lookup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline; try again or choose a source.`;
    }
    previewStatus = recovery.failureReason || "The verified sources could not play. Choose another source.";
    if (previewRequests.isCurrent(request)) { preview = null; paused = true; }
  }

  async function refreshStalledCandidate(t: Track, request: number, candidate: Candidate, cause: string, allowExpiredWindow = false, seekRecovery = false): Promise<boolean> {
    const recovery = automaticPreviewRecovery;
    if (!recovery || recovery.request !== request || recovery.track.id !== t.id || !previewRequests.isCurrent(request)) return false;
    recovery.stallCause = cause;
    recovery.failureReason = cause;
    if (recovery.attempts >= 2) {
      recovery.failureReason = `${cause} The playback recovery attempt limit was reached.`;
      previewStatus = recovery.failureReason;
      return false;
    }
    let budgetMs = Math.floor((recovery.deadline ?? performance.now() + playbackStartupTimeoutMs()) - performance.now());
    if (budgetMs <= 0) {
      // A media transport can fail after the original startup deadline has
      // elapsed. Grant one short, explicit window for the already-verified
      // candidate URL refresh; do not reopen the broad search deadline.
      if (!allowExpiredWindow || recovery.transportRecoveryWindowUsed) {
        recovery.failureReason = `${cause} The playback recovery deadline expired.`;
        previewStatus = recovery.failureReason;
        return false;
      }
      recovery.transportRecoveryWindowUsed = true;
      recovery.deadline = performance.now() + 8_000;
      recovery.sourceStartupDeadline = recovery.deadline;
      budgetMs = 8_000;
    }
    recovery.sourceStartupDeadline = recovery.deadline ?? performance.now() + budgetMs;
    recovery.attempts++;
    try {
      const result = await boundedPreviewResolution(request, resolvePlaybackWithMediaWarmup({
        track: t, requestGeneration: request, selectedCandidate: recovery.exactSourceOnly ? queueCandidates[t.id] ?? candidate : null,
        refreshCandidateFresh: true, refreshCandidate: candidate, seekRecovery, budgetMs, failedCandidates: [],
        normalizationRouteRequired: await isWebviewNormalizationRouteRequired(),
      }));
      if (!result || !previewRequests.isCurrent(request)) return false;
      if (result.transport === "native") return await startNativeResolvedSource(t, result, request);
      if (!result.candidate || candidateKey(result.candidate) !== candidateKey(candidate)) {
        recovery.failureReason = `${cause} The refresh did not return the same playback candidate.`;
        previewStatus = recovery.failureReason;
        return false;
      }
      candidates = [result.candidate];
      const resolvedPreview = result.preview.kind === "local" && !result.preview.approximateExcerpt
        && result.quality?.sourceKind === "approximateAlbumExcerpt" && result.quality.approximateExcerpt
        ? { ...result.preview, approximateExcerpt: { label: result.quality.approximateExcerpt.label, source: result.quality.approximateExcerpt.source } }
        : result.preview;
      if (await startResolvedPreviewAudio(t, result.candidate, resolvedPreview, request)) return true;
      if (!recovery.failureReason.startsWith(cause)) recovery.failureReason = `${cause} The refreshed source could not start.`;
    } catch (failure) {
      if (!previewRequests.isCurrent(request)) return false;
      recovery.failureReason = `${cause} The same-source refresh failed: ${safePreviewError(failure)}`;
    }
    previewStatus = recovery.failureReason;
    return false;
  }

  const mediaEventDiagnosticsEnabled = (() => {
    try { return typeof window !== "undefined" && window.sessionStorage.getItem("burrata.webviewMediaEventDiagnostics") === "1"; }
    catch { return false; }
  })();
  const webviewMediaEventRecorder = createWebviewMediaEventRecorder({enabled:mediaEventDiagnosticsEnabled});
  const webviewBufferingRecovery = new WebviewBufferingRecovery();
  const webviewStallWatchdog = new WebviewStallWatchdog();
  const webviewSeekCompletionWatchdog = new WebviewSeekCompletionWatchdog();
  const webviewPostSeekProgress = new WebviewPostSeekProgress();
  let pendingPostSeekProgress: NonNullable<typeof activeWebviewSeek> | null = null;
  let activeWebviewSeek: { owner: WebviewSeekCompletionOwner; source: Preview; target: number; sessionId: string | null; trackId: string; deadline: number | null; startedAtMs: number; handoffTimeoutMs: number; startupResume: boolean; startupRequest: number | null } | null = null;
  function webviewSeekOwner(node: HTMLAudioElement): WebviewSeekCompletionOwner | null {
    const source=preview, track=playingTrack, occurrenceId=automationQueueProjection.entryIds[queueIndex] ?? null;
    if(node!==audio || !source || !track || nativeActive() || !occurrenceId || !["audio","local","direct"].includes(source.kind))return null;
    return {node,sourceGeneration:source.sourceGeneration??null,seekSerial,queueOccurrenceId:occurrenceId};
  }
  function isCurrentWebviewSeek(seek: Omit<NonNullable<typeof activeWebviewSeek>, "handoffTimeoutMs">): boolean {
    const owner=webviewSeekOwner(seek.owner.node);
    return Boolean(owner && owner.sourceGeneration===seek.owner.sourceGeneration && owner.seekSerial===seek.owner.seekSerial &&
      owner.queueOccurrenceId===seek.owner.queueOccurrenceId && preview===seek.source && playbackSession===seek.sessionId && playingTrack?.id===seek.trackId);
  }
  function youtubeVideoId(value: string | null | undefined): string | null {
    if (!value) return null;
    try {
      const url = new URL(value);
      if (url.username || url.password) return null;
      if (["youtube.com", "www.youtube.com", "m.youtube.com"].includes(url.hostname)) return url.pathname === "/watch" ? url.searchParams.get("v") : null;
      if (url.hostname === "youtu.be") return url.pathname.slice(1).split("/")[0] || null;
    } catch { /* Candidate metadata can be malformed; it is never trusted for handoff. */ }
    return null;
  }
  function canHandoffWebviewSeek(node: HTMLAudioElement, seek: Omit<NonNullable<typeof activeWebviewSeek>, "handoffTimeoutMs">): WebviewPlaybackHandoff | null {
    const track = playingTrack, source = preview, candidate = playingCandidate;
    const queue = playbackQueue.snapshot();
    const entryIds = queue.entryIds ?? [];
    const entryId = seek.owner.queueOccurrenceId;
    if (!track || !source || !candidate || node !== audio || source !== seek.source || !isCurrentWebviewSeek(seek) ||
        nativeActive() || source.kind !== "audio" || source.isExcerpt || source.sessionId || source.sourceGeneration === undefined ||
        track.provider !== "youtube" || track.playbackRange || track.localPath || track.assetId || playingRadioStation || currentRadioSong ||
        candidate.provider !== "youtube" || candidate.source || candidate.id !== `youtube:${track.providerId}` ||
        youtubeVideoId(candidate.url) !== track.providerId || youtubeVideoId(track.sourceUrl) !== track.providerId ||
        !Number.isFinite(track.durationSeconds) || track.durationSeconds! < 5 || track.durationSeconds! > 86_400 ||
        !Number.isFinite(seek.target) || seek.target < 0 || seek.target >= track.durationSeconds! - 0.05 ||
        !Number.isSafeInteger(source.sourceGeneration) || source.sourceGeneration! < 0 || !playbackSession ||
        !Number.isSafeInteger(seek.owner.seekSerial) || seek.owner.seekSerial < (seek.startupResume ? 0 : 1) || queue.currentIndex !== queueIndex ||
        queue.currentIndex < 0 || queue.currentIndex >= queue.tracks.length || entryIds[queue.currentIndex] !== entryId ||
        queue.tracks[queue.currentIndex]?.id !== track.id || !entryId || !queue.tracks.length || queue.tracks.length !== entryIds.length) return null;
    return {
      handoffId: crypto.randomUUID(),
      sourceGeneration: source.sourceGeneration,
      seekSerial: seek.owner.seekSerial,
      targetPosition: seek.target,
      tracks: [...queue.tracks],
      entryIds: [...entryIds],
      index: queue.currentIndex,
      sessionId: playbackSession,
      entryId,
      // A media error can make the element report paused even while the user
      // still intends playback to continue.
      paused: seek.startupResume ? false : paused,
      volume,
      repeat: repeatMode,
      shuffle: shuffleEnabled,
      candidate,
      recorded: recordedSession === playbackSession,
    };
  }
  function handoffOwnerStillCurrent(node: HTMLAudioElement, seek: NonNullable<typeof activeWebviewSeek>, request: WebviewPlaybackHandoff, candidateKeyAtStart: string, candidateUrlAtStart: string): boolean {
    const queue = playbackQueue.snapshot();
    const entryIds = queue.entryIds ?? [];
    return node === audio && playingTrack?.id === request.tracks[request.index]?.id &&
      playingTrack?.providerId === request.candidate.id.slice("youtube:".length) && playbackSession === request.sessionId &&
      seekSerial === request.seekSerial && queueIndex === request.index &&
      queue.currentIndex === request.index && entryIds[request.index] === request.entryId &&
      entryIds.length === request.entryIds.length && entryIds.every((id, index) => id === request.entryIds[index]) &&
      queue.tracks.length === request.tracks.length && queue.tracks.every((track, index) => track.id === request.tracks[index]?.id) &&
      Boolean(playingCandidate) && candidateKey(playingCandidate!) === candidateKeyAtStart && playingCandidate?.url === candidateUrlAtStart &&
      repeatMode === request.repeat && shuffleEnabled === request.shuffle && volume === request.volume;
  }
  function prepareStartupNativeHandoffReceipt(node: HTMLAudioElement, seek: NonNullable<typeof activeWebviewSeek>, request: WebviewPlaybackHandoff): boolean {
    if (!seek.startupResume || seek.startupRequest === null) return true;
    if (!seek.sessionId || !seek.owner.queueOccurrenceId || audio !== node || preview !== seek.source ||
        playingTrack?.id !== seek.trackId || playbackSession !== request.sessionId || !previewRequests.isCurrent(seek.startupRequest)) return false;
    const trackId = request.tracks[request.index]?.id;
    if (!trackId || request.entryIds[request.index] !== request.entryId || trackId !== seek.trackId) return false;
    startupNativeHandoffReceipt = {
      handoffId: request.handoffId,
      request: seek.startupRequest,
      startupGeneration: audioStartupGeneration,
      node,
      source: seek.source,
      sourceGeneration: request.sourceGeneration,
      sessionId: request.sessionId,
      trackId,
      entryId: request.entryId,
      queueEntryIds: [...request.entryIds],
      index: request.index,
      targetPosition: request.targetPosition,
      candidateId: request.candidate.id,
      committed: false,
      consumed: false,
    };
    return true;
  }
  function startupHandoffSnapshotMatches(receipt: StartupNativeHandoffReceipt, snapshot: NativePlaybackSnapshot): boolean {
    const queue = playbackQueue.snapshot();
    const entryIds = queue.entryIds ?? [];
    return snapshot.engine === "native" && snapshot.handoffId === receipt.handoffId &&
      snapshot.trackId === receipt.trackId && snapshot.entryId === receipt.entryId &&
      snapshot.candidate?.id === receipt.candidateId && Number.isFinite(snapshot.position) &&
      Math.abs(snapshot.position - receipt.targetPosition) <= 1 && snapshot.currentIndex === receipt.index &&
      queue.currentIndex === receipt.index && queueIndex === receipt.index &&
      queue.tracks[receipt.index]?.id === receipt.trackId && entryIds[receipt.index] === receipt.entryId &&
      entryIds.length === receipt.queueEntryIds.length && entryIds.every((id, index) => id === receipt.queueEntryIds[index]) &&
      (snapshot.queueEntryIds === undefined || (snapshot.queueEntryIds.length === receipt.queueEntryIds.length &&
        snapshot.queueEntryIds.every((id, index) => id === receipt.queueEntryIds[index])));
  }
  function confirmStartupNativeHandoffSnapshot(snapshot: NativePlaybackSnapshot): void {
    const receipt = startupNativeHandoffReceipt, pending = pendingWebviewHandoff;
    if (!receipt || receipt.committed || !pending || pending.cancelled || pending.handoffId !== receipt.handoffId ||
        nativeSnapshot?.engine !== "native" || nativeSnapshot.handoffId !== receipt.handoffId ||
        snapshot.handoffId !== nativeSnapshot.handoffId || !previewRequests.isCurrent(receipt.request) ||
        audioStartupGeneration !== receipt.startupGeneration || playbackSession !== receipt.sessionId ||
        !startupHandoffSnapshotMatches(receipt, snapshot)) return;
    receipt.committed = true;
    blockedStartupNodes.add(receipt.node);
    releaseWebviewStartupResumeGate(receipt.node, receipt.source, false, false);
    webviewSeekCompletionWatchdog.cancel();
    if (activeWebviewSeek?.owner.node === receipt.node && activeWebviewSeek.startupRequest === receipt.request) activeWebviewSeek = null;
    webviewStallWatchdog.reset();
    audioStreamHandle?.destroy(); audioStreamHandle = null;
    receipt.node.pause(); receipt.node.removeAttribute("src"); receipt.node.load();
  }
  function recordStartupNativeHandoffReceipt(seek: NonNullable<typeof activeWebviewSeek>, request: WebviewPlaybackHandoff): boolean {
    if (!seek.startupResume || seek.startupRequest === null) return true;
    const receipt = startupNativeHandoffReceipt, snapshot = nativeSnapshot;
    if (!receipt || receipt.handoffId !== request.handoffId || receipt.request !== seek.startupRequest ||
        !snapshot || receipt.sessionId !== request.sessionId || receipt.entryId !== request.entryId ||
        receipt.candidateId !== request.candidate.id || !startupHandoffSnapshotMatches(receipt, snapshot)) return false;
    if (!receipt.committed) confirmStartupNativeHandoffSnapshot(snapshot);
    return receipt.committed;
  }
  async function attemptNativeSeekHandoff(node: HTMLAudioElement, seek: NonNullable<typeof activeWebviewSeek>): Promise<"not-applicable" | "handled" | "retry"> {
    if (pendingWebviewHandoff) return "handled";
    const request = canHandoffWebviewSeek(node, seek);
    if (!request) return "not-applicable";
    const candidateKeyAtStart = candidateKey(request.candidate), candidateUrlAtStart = request.candidate.url;
    const isCurrent = () => handoffOwnerStillCurrent(node, seek, request, candidateKeyAtStart, candidateUrlAtStart);
    const pending: PendingWebviewHandoff = { handoffId: request.handoffId, cancelled: false, isCurrent, settled: Promise.resolve() };
    let settle!: () => void;
    pending.settled = new Promise<void>(resolve => { settle = resolve; });
    pendingWebviewHandoff = pending;
    if (!prepareStartupNativeHandoffReceipt(node, seek, request)) {
      if (pendingWebviewHandoff === pending) pendingWebviewHandoff = null;
      settle();
      return "not-applicable";
    }
    node.pause();
    paused = true;
    position = request.targetPosition;
    bufferingAudio = true;
    try {
      if (!isCurrent()) return "handled";
      const snapshot = await nativeBridge.handoffWebview(request);
      const alreadyCommitted = nativeActive() && nativeSnapshot?.handoffId === request.handoffId;
      if (pending.cancelled || (!alreadyCommitted && !isCurrent())) {
        await nativeBridge.command({ action:"stop", expectedHandoffId:request.handoffId }).catch(()=>{});
        return "handled";
      }
      if (snapshot.engine !== "native" || snapshot.handoffId !== request.handoffId) throw new Error("The native player rejected ownership of this seek.");
      if (nativeBridge.acceptSnapshot(snapshot) && nativeBridge.snapshot) applyNativeSnapshot(nativeBridge.snapshot);
      else if (nativeBridge.snapshot?.handoffId === request.handoffId) applyNativeSnapshot(nativeBridge.snapshot);
      else throw new Error("The native player did not confirm the seek handoff.");
      if (!recordStartupNativeHandoffReceipt(seek, request)) throw new Error("The native player changed playback ownership before startup resume completed.");
      if (seek.startupResume) {
        releaseWebviewStartupResumeGate(node, seek.source, false, false);
      }
      webviewSeekCompletionWatchdog.cancel();
      activeWebviewSeek = null;
      webviewStallWatchdog.reset();
      audioStreamHandle?.destroy(); audioStreamHandle = null;
      blockedStartupNodes.add(node);
      node.pause(); node.removeAttribute("src"); node.load();
      return "handled";
    } catch (failure) {
      if (nativeActive() && nativeSnapshot?.handoffId === request.handoffId && !pending.cancelled) {
        const startupReceiptValid = recordStartupNativeHandoffReceipt(seek, request);
        if (!startupReceiptValid) await nativeBridge.command({ action:"stop", expectedHandoffId:request.handoffId }).catch(()=>{});
        if (seek.startupResume) {
          releaseWebviewStartupResumeGate(node, seek.source, false, false);
        }
        webviewSeekCompletionWatchdog.cancel();
        activeWebviewSeek = null;
        webviewStallWatchdog.reset();
        audioStreamHandle?.destroy(); audioStreamHandle = null;
        blockedStartupNodes.add(node);
        node.pause(); node.removeAttribute("src"); node.load();
        return "handled";
      }
      if (pending.cancelled || !isCurrent()) {
        await nativeBridge.command({ action:"stop", expectedHandoffId:request.handoffId }).catch(()=>{});
        return "handled";
      }
      error = String(failure);
      if (isCurrent()) paused = request.paused;
      return "retry";
    } finally {
      if (pendingWebviewHandoff === pending) pendingWebviewHandoff = null;
      if (startupNativeHandoffReceipt?.handoffId === pending.handoffId &&
          (!startupNativeHandoffReceipt.committed || startupNativeHandoffReceipt.consumed)) startupNativeHandoffReceipt = null;
      settle();
    }
  }
  async function cancelPendingWebviewHandoff(): Promise<void> {
    const pending = pendingWebviewHandoff;
    if (!pending) return;
    pending.cancelled = true;
    await pending.settled;
  }
  function deferUntilWebviewHandoffSettles(action: () => void, cancel = true): boolean {
    const pending = pendingWebviewHandoff;
    if (!pending) return false;
    if (cancel) pending.cancelled = true;
    void pending.settled.then(action);
    return true;
  }
  function beginWebviewSeekCompletionWatch(node: HTMLAudioElement, target: number, startupResume = false, startupRequest: number | null = null) {
    const owner=webviewSeekOwner(node), track=playingTrack;
    if(!owner || !track)return;
    const source=preview!;
    const startedAtMs=performance.now();
    const pendingSeek={owner,source,target,sessionId:playbackSession,trackId:track.id,startedAtMs,startupResume,startupRequest,
      deadline:paused||node.paused?null:startedAtMs+playbackStartupTimeoutMs()};
    const seek={...pendingSeek,handoffTimeoutMs:canHandoffWebviewSeek(node,pendingSeek)?5_000:playbackStartupTimeoutMs()};
    activeWebviewSeek=seek;
    const onExpired: (requestedTarget:number)=>void = requestedTarget=>{
      if(activeWebviewSeek!==seek || !isCurrentWebviewSeek(seek))return;
      if(seek.handoffTimeoutMs<playbackStartupTimeoutMs() && seek.deadline!==null && performance.now()<seek.deadline && !canHandoffWebviewSeek(node,seek)){
        webviewSeekCompletionWatchdog.begin({owner,target:seek.target,paused:paused||node.paused,isCurrent:()=>isCurrentWebviewSeek(seek),timeoutMs:Math.max(1,seek.deadline-performance.now()),onExpired});
        return;
      }
      const reason=`Audio did not complete the requested seek within ${playbackStartupTimeoutSeconds()} seconds; refreshing the same source at the requested position.`;
      void attemptNativeSeekHandoff(node, seek).then(result=>{
        if(result!=="handled" && activeWebviewSeek===seek && isCurrentWebviewSeek(seek)) recoverOwnedAudioFailure(node,reason,requestedTarget,playingCandidate??undefined);
      });
    };
    webviewSeekCompletionWatchdog.begin({owner,target,paused:paused||node.paused,isCurrent:()=>isCurrentWebviewSeek(seek),timeoutMs:seek.handoffTimeoutMs,onExpired});
  }
  function armPendingWebviewSeekAfterResume(node: HTMLAudioElement) {
    const seek=activeWebviewSeek;
    if(seek && seek.owner.node===node && isCurrentWebviewSeek(seek)){
      if(seek.deadline===null){seek.startedAtMs=performance.now();seek.deadline=seek.startedAtMs+playbackStartupTimeoutMs();}
      webviewSeekCompletionWatchdog.resume(seek.owner,seek.handoffTimeoutMs);
    }
  }
  function refreshSteadyMonitorAfterCompletedSeek(node: HTMLAudioElement) {
    if(pendingWebviewResume)return;
    const attached=successfulRecoveryAttachment, recovery=automaticPreviewRecovery;
    if(!attached || !recovery || attached.request!==recovery.request)return;
    updateSuccessfulRecoverySeek(node);
    armWebviewStallWatchdog(node);
  }
  function completeWebviewSeekIfLanded(node: HTMLAudioElement, browserAcknowledged = false) {
    const seek=activeWebviewSeek;
    if(!seek || seek.owner.node!==node || !isCurrentWebviewSeek(seek) || node.seeking)return false;
    if(seek.startupResume && Number.isFinite(node.duration) && node.duration>0 && seek.target>node.duration+2)return false;
    if(seek.startupResume&&!hasWebviewStartupResumeProgress({target:seek.target,startPosition:seek.target,currentTime:node.currentTime,
      duration:node.duration,elapsedMs:performance.now()-seek.startedAtMs,readyState:node.readyState,seeking:node.seeking,
      paused:node.paused,playbackRate:node.playbackRate}))return false;
    if(node.error || node.readyState<2 || !hasWebviewSeekLanded({target:seek.target,currentTime:node.currentTime,
      duration:node.duration,elapsedMs:performance.now()-seek.startedAtMs,playing:!paused&&!node.paused,
      playbackRate:node.playbackRate,browserAcknowledged}))return false;
    webviewSeekCompletionWatchdog.complete(seek.owner);
    activeWebviewSeek=null;
    if (!seek.startupResume) { pendingPostSeekProgress = seek; armPostSeekProgress(node); }
    return true;
  }
  function armPostSeekProgress(node: HTMLAudioElement) {
    const seek = pendingPostSeekProgress;
    if (!seek || seek.owner.node !== node || node.paused || paused || !isCurrentWebviewSeek(seek)) return;
    pendingPostSeekProgress = null;
    const candidate = playingCandidate;
    webviewPostSeekProgress.begin({ owner: seek.owner, isCurrent: () => isCurrentWebviewSeek(seek) && !paused,
      onExpired: () => recoverOwnedAudioFailure(node, "Audio stopped after the requested seek; recovering at that position.", seek.target, candidate ?? undefined) });
  }
  $effect(() => {
    audio; preview; preview?.url; preview?.sourceGeneration; seekSerial; playingTrack?.id; queueIndex;
    automationQueueProjection.entryIds[queueIndex];
    webviewBufferingRecovery.reset();
    webviewStallWatchdog.reset();
    const seek=activeWebviewSeek;
    if(seek && !isCurrentWebviewSeek(seek)){webviewSeekCompletionWatchdog.cancel();activeWebviewSeek=null;}
    if(pendingPostSeekProgress && !isCurrentWebviewSeek(pendingPostSeekProgress)) pendingPostSeekProgress=null;
    if(audio)untrack(()=>armWebviewStallWatchdog(audio!));
    return () => webviewStallWatchdog.reset();
  });
  function recordWebviewMediaEvent(type:string, element:EventTarget | null) {
    if(!(element instanceof HTMLAudioElement)||element!==audio||!preview||nativeActive())return;
    if (webviewMediaEventRecorder.enabled) webviewDiagnosticsCleared = false;
    webviewMediaEventRecorder.record({sessionId:playbackSession??null,sourceGeneration:preview.sourceGeneration??null,entryId:automationQueueProjection.entryIds[queueIndex]??null,seekSerial},type,element);
  }
  function updateSuccessfulRecoverySeek(node: HTMLAudioElement) {
    // Repeated playing events after buffering retain the same source's steady progress.
    // Only an actual seek starts a new baseline; source startup creates its own.
    const attached = successfulRecoveryAttachment, recovery = automaticPreviewRecovery;
    if (node === audio && preview && playingTrack && !nativeActive() && !paused && attached && recovery &&
        attached.request === recovery.request && recovery.track.id === playingTrack.id && attached.seekSerial !== seekSerial && Number.isFinite(node.currentTime)) {
      successfulRecoveryAttachment = { request: attached.request, startedAt: performance.now(), position: node.currentTime, seekSerial };
    }
  }
  function resetLiveSuccessfulPlayback(node: HTMLAudioElement) {
    const attached = successfulRecoveryAttachment;
    if (node !== audio || !attached || !Number.isFinite(node.currentTime)) return;
    if (!playingRadioStation) {
      // Preserve proof of a completed healthy interval before waiting/pause
      // starts a new uninterrupted-progress baseline.
      const healthy = attached.healthy || (!paused && performance.now()-(attached.healthStartedAt ?? attached.startedAt) >= HEALTHY_PLAYBACK_RESET_MS
        && node.currentTime-(attached.healthPosition ?? attached.position) >= HEALTHY_PLAYBACK_RESET_MS/1000);
      if (healthy) activeFailedQueueRun = null;
      successfulRecoveryAttachment = {...attached,healthStartedAt:performance.now(),healthPosition:node.currentTime,healthy};
      return;
    }
    const healthy = attached.healthy || (!paused && performance.now()-attached.startedAt >= 5_000 && node.currentTime-attached.position >= 5);
    successfulRecoveryAttachment = healthy ? {...attached,healthy:true} : {...attached,startedAt:performance.now(),position:node.currentTime};
  }
  function updateWebviewTime(event: Event) {
    const node = event.currentTarget, source = preview, serial = seekSerial;
    if (!(node instanceof HTMLAudioElement) || node !== audio || !source || nativeActive()) return;
    const mediaTime = node.currentTime;
    updatePlaybackPosition(mediaTime);
    if (completeWebviewSeekIfLanded(node)) refreshSteadyMonitorAfterCompletedSeek(node);
    if (audio !== node || preview !== source || seekSerial !== serial || nativeActive()) return;
    const stallOwner = webviewStallOwner(node, source, serial);
    if (stallOwner) webviewStallWatchdog.observeProgress(stallOwner, mediaTime);
    if (node.readyState < 2) { webviewBufferingRecovery.reset(); return; }
    const recovered = webviewBufferingRecovery.observe({
      owner: {node, sourceGeneration:source.sourceGeneration??null, seekSerial:serial}, currentNode:audio,
      mediaTime, wallTimeMs:performance.now(), paused:paused || node.paused,
      seeking:node.seeking, ended:node.ended, hasError:Boolean(node.error),
    });
    if (recovered) bufferingAudio = false;
  }
  let resolvingPlayback = $state(false), bufferingAudio = $state(false);
  let playbackLoadSequence = 0;
  const playbackLoading = $derived(resolvingPlayback || bufferingAudio);
  const playerErrorMessage = $derived.by(() => {
    if (playbackLoading) return "";
    if (nativeActive()) {
      if (!nativeSnapshot || !nativeSnapshotFailure(nativeSnapshot)) return "";
      const diagnostic = nativeFailureMessage(nativeSnapshot?.diagnostics?.lastSourceFailure);
      if (diagnostic) return diagnostic;
      return safeNativeFailurePart(nativeSnapshot.playerMessage, "Playback failed.");
    }
    const message = previewStatus.trim();
    if (!playingTrack || !message || /^(?:Paused\b|Ready\b|Playing\b|Loading\b|Finding\b|Buffering\b|Searching\b|Audio preview failed\. Trying another match)/i.test(message)) return "";
    if (/^Ready to resume\b/i.test(message)) return "";
    if (preview && !/^(?:The selected .+ source could not play:|Radio could not play:|Could not start\b|Audio lookup exceeded\b|The verified source (?:could not|did not)\b|Playback could not start:)/i.test(message)) return "";
    return message;
  });
  const playerStatusMessage = $derived.by(() => {
    if (nativeActive()) {
      switch (nativeSnapshot?.status) {
        case "failed": return "Playback unavailable";
        case "buffering": return "Buffering…";
        case "resolving": return "Finding audio…";
        case "paused": return "Paused";
        case "playing": return nativeSnapshot.playerMessage || "Playing";
        default: return nativeSnapshot?.playerMessage || (paused ? "Paused" : "Ready");
      }
    }
    if (playbackLoading) return resolvingPlayback ? "Finding audio…" : "Buffering…";
    if (!playingTrack) return previewStatus || "Press Space to play";
    if (!preview) return previewStatus === "Paused" ? "Paused" : previewStatus === "Ready" || /^Ready to resume\b/i.test(previewStatus) ? "Ready" : previewStatus ? "Unavailable" : "Ready";
    if (paused) return "Paused";
    if (previewStatus.startsWith("Searching")) return "Finding audio";
    if (previewStatus.startsWith("Loading")) return "Loading";
    return preview.isExcerpt ? "Playing preview" : "Playing";
  });
  async function startPreview(t: Track, fromQueue = false, automaticContinuation = false, displayedTracks?: Track[], displayedIndex?: number, failedSkips = 0, automationRequestId: string | null = null, resumeSavedPosition = false, failedQueueRun?: FailedQueueRun, loudnessAlbumResource: LoudnessResourceIdentity | null = null, advanceResumeFailure = false) {
    waveformRouteUpgradeRequest++;
    waveformRouteUpgradeStatus = "";
    let selectedAlbumResource = loudnessAlbumResource;
    if (!selectedAlbumResource && (fromQueue || automaticContinuation || resumeSavedPosition)) {
      const entryId = nativeActive()
        ? nativeSnapshot?.queueEntryIds?.[nativeSnapshot.currentIndex] ?? nativeSnapshot?.entryId
        : playbackQueue.entryIds[queueIndex];
      selectedAlbumResource = playbackQueue.loudnessContextFor(entryId)
        ?? (entryId ? nativeSnapshot?.queueLoudnessContexts?.[entryId] ?? null : null);
    }
    if (selectedAlbumResource && (!t.assetId || !selectedAlbumResource.members.some(member => member.assetId === t.assetId))) {
      selectedAlbumResource = null;
    }
    if (!failedQueueRun) activeFailedQueueRun = null;
    if (!automaticContinuation && !resumeSavedPosition && !failedQueueRun) clearPlaybackSkipNotice();
    if(resumeSavedPosition){automationRequestId=queuedOwnerForResume(automationRequestId);queuedPausedOwner=null;}
    nextCandidatePrefetch.cancel();
    const trace=playbackOperationTrace();
    const sequence = ++playbackLoadSequence;
    resolvingPlayback = true; bufferingAudio = false; lastStartupFailure = null;
    if (settings.playbackDiagnosticsEnabled) {
      currentWebviewDiagnosticAttempt = webviewMediaEventRecorder.beginAttempt();
      lastWebviewDiagnosticContext = null;
      lastWebviewDiagnosticFailure = null;
      webviewDiagnosticsCleared = false;
    }
    const diagnosticAttempt = currentWebviewDiagnosticAttempt;
    const diagnosticStarted = performance.now();
    let diagnosticOutcome: "playing"|"failed"|"cancelled" = "failed";
    try {
      await startPreviewInner(t, fromQueue, automaticContinuation, displayedTracks, displayedIndex, failedSkips, automationRequestId,trace,resumeSavedPosition,failedQueueRun,selectedAlbumResource,advanceResumeFailure);
      if(resumeSavedPosition && automationRequestId && activeAutomationPlayback===automationRequestId && queuedPausedOwner!==automationRequestId)reportQueuedPlaybackPhase(preview&&!paused?"playing":"failed");
      const started = sequence === playbackLoadSequence && (nativeActive() || Boolean(preview && !paused));
      diagnosticOutcome = sequence !== playbackLoadSequence ? "cancelled" : started ? "playing" : "failed";
      return started;
    } finally {
      if (settings.playbackDiagnosticsEnabled && !webviewDiagnosticsCleared && diagnosticAttempt > 0) {
        webviewStartupOutcomes.push({attempt:diagnosticAttempt,operationId:trace.operationId,
          outcome:sequence !== playbackLoadSequence ? "cancelled" : diagnosticOutcome,
          elapsedMs:Math.min(60000, Math.max(0, Math.round(performance.now()-diagnosticStarted)))});
        webviewStartupOutcomes = webviewStartupOutcomes.slice(-32);
      }
      if (sequence === playbackLoadSequence) {
        resolvingPlayback = false;
        // A failed/cancelled source has no audio node left to emit a pause event.
        if (!nativeActive() && !preview) { bufferingAudio = false; paused = true; }
      }
    }
  }
  function sameNativePlaybackTrack(left: Track, right: Track) {
    const a = left.playbackRange, b = right.playbackRange;
    const sameRange = !a && !b || Boolean(a && b && a.parentTrackId === b.parentTrackId && a.chapterId === b.chapterId && a.sourceUrl === b.sourceUrl && a.startTime === b.startTime && a.endTime === b.endTime);
    return left.id === right.id && left.provider === right.provider && left.sourceUrl === right.sourceUrl &&
      left.artist === right.artist && left.title === right.title && (left.version ?? null) === (right.version ?? null) &&
      (left.durationSeconds ?? null) === (right.durationSeconds ?? null) && (left.localPath ?? null) === (right.localPath ?? null) &&
      (left.assetId ?? null) === (right.assetId ?? null) && sameRange &&
      JSON.stringify(portablePlaybackSource(left)) === JSON.stringify(portablePlaybackSource(right));
  }
  function sameNativePlaybackQueue(left: readonly Track[], right: readonly Track[]) {
    return left.length === right.length && left.every((track, index) => sameNativePlaybackTrack(track, right[index]!));
  }
  async function startPreviewInner(t: Track, fromQueue = false, automaticContinuation = false, displayedTracks?: Track[], displayedIndex?: number, failedSkips = 0, automationRequestId: string | null = null,trace?:PlaybackOperationTrace,resumeSavedPosition = false, failedQueueRun?: FailedQueueRun,loudnessAlbumResource: LoudnessResourceIdentity | null = null,advanceResumeFailure = false) {
    if (webviewObservedCapture && !webviewObservedCapture.finishing) cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because playback changed.");
    if (webviewAnalysisArmTrackId && webviewAnalysisArmTrackId !== t.id) {
      webviewAnalysisArmTrackId = null;
      webviewObservedAnalysisStatus = "Whole-track WebView analysis was cancelled because playback changed.";
    }
    await cancelPendingWebviewHandoff();
    activeWebviewLoudnessAlbumResource = loudnessAlbumResource;
    try {
      await call("set_loudness_album_context", { resource: loudnessAlbumResource });
    } catch (failure) {
      if (loudnessAlbumResource) throw failure;
    }
    playbackRunGeneration++;
    cancelRadioIdentification();
    if (nativeActive()) {
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
      if (canSelectFromNativeQueue) await nativeCommand({action:"select",index,loudnessAlbumResource,...trace});
      else await nativeCommand({action:"play",tracks:list.length ? list : [t],index:index >= 0 ? index : 0,candidate,position:0,loudnessAlbumResource,...trace});
      return;
    }
    if (radioBusy && radioBusy !== t.id) { radioRequest++; radioBusy = null; }
    setAutomationPlaybackOwner(automationRequestId);
    systemPausedTrack = false;
    audioRecovery = null;
    if (!fromQueue) {
      const list = displayedTracks ?? (view === "history" ? historyItems.map((item) => item.track) : sorted.some((item) => item.id === t.id) ? [...sorted] : [t]);
      const index = displayedIndex ?? list.findIndex((item) => item === t);
      playbackQueue.setTracks(list, index >= 0 ? index : Math.max(0, list.findIndex((item) => item.id === t.id)));
      if (loudnessAlbumResource) playbackQueue.setLoudnessContext(playbackQueue.entryIds, loudnessAlbumResource);
      playbackQueue.setRepeat(repeatMode); if (shuffleEnabled) playbackQueue.shuffle(); refreshQueue();
    }
    // Failed entry advancement is queue intent, not a natural-completion signal.
    const advanceFailedQueueEntry = queueTracks.length > 1 && (!resumeSavedPosition || advanceResumeFailure);
    const failedQueueEntryIds = [...playbackQueue.entryIds];
    const failedQueueSourceKeys = playbackQueue.tracks.map(queueRadioSeedKey);
    const failedQueueSourceKey = JSON.stringify(failedQueueSourceKeys);
    const failedQueueIndex = queueIndex;
    const failureRun = failedQueueRun ?? {entryIds: failedQueueEntryIds, sourceKey: failedQueueSourceKey, visited: new Set<string>(), sourceRefreshRetries: new Set<string>()};
    const occurrenceId = failedQueueEntryIds[failedQueueIndex];
    if (advanceFailedQueueEntry) {
      if (!occurrenceId || failureRun.sourceKey !== failedQueueSourceKey ||
          failureRun.entryIds.length !== failedQueueEntryIds.length ||
          !failureRun.entryIds.every((id,index) => id === failedQueueEntryIds[index]) || failureRun.visited.has(occurrenceId)) return;
      failureRun.visited.add(occurrenceId);
    }
    // Explicit selections start a new occurrence. Only Resume restores a saved playhead.
    pendingResumePosition = resumeSavedPosition && !automaticContinuation && resumeTrackId === t.id ? resumePosition : t.playbackRange?.startTime ?? 0;
    playbackSession = crypto.randomUUID(); recordedSession = ""; playingCandidate = null; currentRadioSong=null; radioOccurrence=null;
    clearTimeout(autoTimer);
    const request = nextPreviewRequest();
    async function advanceFailedQueue() {
    const failedQueueOccurrenceIsCurrent = advanceFailedQueueEntry &&
      failedQueueEntryIds.length === playbackQueue.entryIds.length &&
      failedQueueEntryIds.every((id,index) => playbackQueue.entryIds[index] === id) &&
      failedQueueIndex === queueIndex && previewRequests.isCurrent(request) && !preview;
    const currentQueueTrack = playbackQueue.tracks[failedQueueIndex];
    const currentOccurrenceSourceKey = currentQueueTrack ? queueRadioSeedKey(currentQueueTrack) : "";
    const currentOccurrenceSourceChanged = Boolean(occurrenceId) &&
      currentOccurrenceSourceKey !== failedQueueSourceKeys[failedQueueIndex];
    if (failedQueueOccurrenceIsCurrent && occurrenceId && currentOccurrenceSourceChanged &&
        !failureRun.sourceRefreshRetries.has(occurrenceId)) {
      // A library/autotag refresh can enrich this same occurrence while its old
      // resolver request is pending. Retry it once with the current row payload;
      // the old source's failure cannot decide whether the refreshed source works.
      failureRun.sourceRefreshRetries.add(occurrenceId);
      failureRun.visited.delete(occurrenceId);
      failureRun.sourceKey = JSON.stringify(playbackQueue.tracks.map(queueRadioSeedKey));
      selected = currentQueueTrack!;
      await startPreview(currentQueueTrack!, true, false, undefined, undefined, failedSkips, automationRequestId, false, failureRun);
      return;
    }
    if (failedQueueOccurrenceIsCurrent && !currentOccurrenceSourceChanged && shouldAdvanceAfterExhaustedFailure(previewStatus)) {
      pendingResumePosition = 0;
      const failedTitle = safeNativeFailurePart(t.title, "Unavailable recording");
      const failedReason = safePreviewError(previewStatus);
      // Attempt each occurrence at most once; repeat-all may wrap to an unvisited entry.
      activeFailedQueueRun = failureRun;
      const next = playbackQueue.nextAfterFailedStart(failureRun.visited);
      recentTrackFailures = appendTrackFailure(recentTrackFailures, t, {
        queueRevision: automationQueueProjection.revision, generation: request, operationId: automationRequestId ?? playbackSession,
        entryId: occurrenceId, outcome: next ? "skipped" : "queue_exhausted",
      }, previewStatus);
      failureHistoryRevision++;
      setNotice(skippedPreviewFailureNotice(t, failedReason), "persistent", "playbackSkip");
      if (next) {
        selected = next;
        refreshQueue();
        failureRun.sourceKey = JSON.stringify(playbackQueue.tracks.map(queueRadioSeedKey));
        // Source exhaustion is not a natural end and must retain cancellation ownership.
        await startPreview(next, true, false, undefined, undefined, failedSkips + 1, automationRequestId, false, failureRun);
      } else {
        paused = true;
        previewStatus = `Could not start ${failedTitle}; reached the end of the queue (${failureRun.visited.size} unavailable occurrences checked). ${failedReason}`;
      }
    }
    }
    successfulRecoveryAttachment = null;
    automaticPreviewRecovery = { request, track: t, tried: new Set(), attempts: 0, sourceAttempts: 0,
      exactSourceOnly: sourceIsAuthoritative(t), deadline: performance.now() + playbackStartupTimeoutMs() };
    audio?.pause();
    if (preview?.sessionId) await call("stop_candidate_preview", { sessionId: preview.sessionId }).catch((e) => (error = String(e)));
    if (!previewRequests.isCurrent(request)) return;
    paused = true;
    preview = null;
    if (portablePlaybackSource(t) && !t.localPath) {
      const source = portablePlaybackSource(t)!;
      const bindingSequence = playbackLoadSequence;
      let bound: Candidate | null;
      try {
        bound = await call<Candidate | null>("resolve_portable_playback_candidate", { track: t });
      } catch (failure) {
        if (bindingSequence === playbackLoadSequence) { previewStatus = `Reviewed provider source could not be resolved: ${String(failure)}`; await advanceFailedQueue(); }
        return;
      }
      if (bindingSequence !== playbackLoadSequence) return;
      const expectedId = `${source.provider}:${source.sourceId}`;
      if (!bound || bound.provider !== source.provider || bound.id !== expectedId) {
        const remainingBindings = { ...queueCandidates };
        delete remainingBindings[t.id];
        queueCandidates = remainingBindings;
        previewStatus = "The reviewed provider recording is unavailable. Review the source selection before playing this entry.";
        await advanceFailedQueue();
        return;
      }
      queueCandidates = { ...queueCandidates, [t.id]: bound };
    }
    if(t.playbackRange && desktop()) {
      try { t=await call<Track>("register_chapter_track",{track:t}); }
      catch(failure) {
        if (!previewRequests.isCurrent(request)) return;
        previewStatus=String(failure);
        await advanceFailedQueue();
        return;
      }
      if (!previewRequests.isCurrent(request)) return;
    }
    playingTrack = t;
    position = pendingResumePosition;
    duration = 0;
    resolveQueuedAcceptance(automationRequestId);
    previewStatus = "Loading preview…";
    const linked = importedAudioCandidate(t);
    if (linked && (t.playbackRange || isLinkedAudioSearchCandidate(t, linked))) playingCandidate = linked;
    if (t.artworkUrl || assetIdForTrack(t)) void cacheCover(t);
    if(isStationPlaybackTrack(t)) {
      const station=radioStations.find(item=>item.id===t.providerId);if(!station){previewStatus="Unknown radio station";await advanceFailedQueue();return;}
      playingTrack={...t,title:station.name,artist:station.provider==="radio_paradise"?"Radio Paradise":"Internet radio",sourceUrl:station.homepageUrl??station.sourceUrl};
      automaticPreviewRecovery!.attempts = 1;
      await startRadioWebviewAudio(t, request);
      await advanceFailedQueue();
      return;
    }
    if (portablePlaybackSource(t)) {
      await findAndStartPreviewSource(t, request);
      await advanceFailedQueue();
      return;
    }
    await run(async () => {
      let result: Preview | undefined;
      try {
        const recovery = automaticPreviewRecovery;
        const deadline = recovery?.request === request && recovery.track.id === t.id
          ? recovery.sourceStartupDeadline ?? firstSourceStartupDeadline(recovery) : undefined;
        result = await withinPreviewStartupBudget(
          () => previewRequests.resolve(
            request,
            call<WebviewResolvedPreview>("resolve_preview", { track: t, requestGeneration: request }),
          ),
          deadline,
          () => previewRequests.isCurrent(request),
        );
      } catch {
        if (previewRequests.isCurrent(request)) await findAndStartPreviewSource(t, request);
        return;
      }
      if (!result || !previewRequests.isCurrent(request)) return;
      if (["audio", "local", "direct"].includes(result.kind)) {
        preview = result;
        const recovery = automaticPreviewRecovery;
        if (recovery?.request === request && recovery.track.id === t.id) {
          recovery.sourceStartupDeadline = sourceAttemptDeadline(recovery, 1);
          recovery.sourceAttempts = (recovery.sourceAttempts ?? 0) + 1;
          if (playingCandidate && !recovery.exactSourceOnly) recovery.tried.add(candidateKey(playingCandidate));
        }
        await tick();
        if (!previewRequests.isCurrent(request)) return;
        if (audio) {
          const node = audio;
          const startupGeneration = ++audioStartupGeneration;
          const recovery = automaticPreviewRecovery;
          const deadline = recovery?.request === request && recovery.track.id === t.id
            ? recovery.sourceStartupDeadline ?? recovery.deadline : undefined;
          let attemptActive = true;
          const startupStartedAt = performance.now();
          const resumeRequested = pendingResumePosition > 0;
          let startupPhase: WebviewStartupPhase = 'attach';
          const startupStages = startupStageTimings();
          startupStages.mark('attach');
          const stillCurrent = () => attemptActive && previewRequests.isCurrent(request) && playingTrack?.id === t.id && audio === node && preview?.url === result.url;
          setWebviewOutputVolume(node, volume);
          try {
            await withinPreviewStartupBudget(() => prepareAudioStream(request, stillCurrent), deadline, stillCurrent);
            startupPhase = 'resume_seek'; startupStages.mark('resume_seek');
            await withinPreviewStartupBudget(() => seekPendingResume(request, stillCurrent), deadline, stillCurrent);
            if (!stillCurrent()) throw new Error("Audio startup was cancelled.");
            startupPhase = 'play'; startupStages.mark('play');
            const initialClockPosition = node.currentTime;
            let playPromise: Promise<void>;
            await withinPreviewStartupBudget(() => {
              playPromise = ownedStartupPlay(node);
              void playPromise.then(() => {
                if (!attemptActive && startupGeneration === audioStartupGeneration) {
                  node.pause();
                  if (preview?.url === result.url) {
                    audioStreamHandle?.destroy();
                    audioStreamHandle = null;
                  }
                }
              }, () => {});
              return playPromise;
            }, deadline, stillCurrent);
            startupPhase = 'clock_progress'; startupStages.mark('clock_progress');
            await waitForStartupClock(node, initialClockPosition, stillCurrent, deadline ?? performance.now() + playbackStartupTimeoutMs());
            if (stillCurrent()) {
              successfulRecoveryAttachment = { request, startedAt: performance.now(), position: node.currentTime, seekSerial };
              paused = false;
              pendingResumePosition = 0;
              previewStatus = result.isExcerpt ? "Playing preview" : "Playing";
              armWebviewStallWatchdog(node);
              return;
            }
          } catch (failure) {
            if (previewRequests.isCurrent(request) && playingTrack?.id === t.id && audio === node && preview?.url === result.url) {
              recordWebviewStartupFailure(captureWebviewStartupFailure(node, startupPhase, result.audioMimeType, audioStreamHandle?.kind, resumeRequested, performance.now() - startupStartedAt, pendingResumePosition, result.sourceTransport, startupStages.snapshot()));
            }
            attemptActive = false;
            if (!previewRequests.isCurrent(request)) return;
            if (playingTrack?.id === t.id && preview?.url === result.url) {
              blockedStartupNodes.add(node);
              node.pause();
              audioStreamHandle?.destroy();
              audioStreamHandle = null;
              preview = null;
              paused = true;
            }
            if (/(?:playback deadline|startup deadline)/.test(safePreviewError(failure))) {
              if (recovery?.request === request) recovery.failureReason = `Audio startup exceeded the ${playbackStartupTimeoutSeconds()}-second playback deadline.`;
            } else if (recovery?.request === request) recovery.failureReason = safePreviewError(failure);
          }
        }
      }
      if (previewRequests.isCurrent(request)) await findAndStartPreviewSource(t, request);
    });
    await advanceFailedQueue();
  }
  function liveStationForTrack(track: Track) {
    // A song identified on a station is a recording, not that station's live
    // transport. Its origin attribution must never become a fallback source.
    if (!isStationPlaybackTrack(track)) return null;
    const provenance = track.provenance as { stationId?: string; radioStationId?: string };
    return radioStations.find(station => station.id === (provenance?.radioStationId ?? provenance?.stationId ?? track.providerId));
  }
  async function startRadioWebviewAudio(track: Track, request: number): Promise<boolean> {
    const station = liveStationForTrack(track);
    if (!station) { previewStatus = "Unknown radio station"; return false; }
    const deadline = automaticPreviewRecovery?.deadline ?? performance.now() + playbackStartupTimeoutMs();
    const current = () => previewRequests.isCurrent(request) && playingTrack?.id === track.id;
    let node: HTMLAudioElement | undefined;
    let source: Preview | null = null;
    try {
      let streamUrl = station.streamUrl;
      let transport: { audioMimeType?: string; audioCodec?: string; sourceTransport?: string; normalizationRoute?: WebviewNormalizationRoute } = {};
      if (desktop()) {
        const normalizationRouteRequired = await isWebviewNormalizationRouteRequired(false);
        const resolved = await withinPreviewStartupBudget(() => call<string | ({url:string} & typeof transport)>("resolve_radio_station_playback_url", {
          stationId:station.id,requestGeneration:request,webviewCapabilities:webviewAudioCapabilities(),detailed:true,
          normalizationRouteRequired,
        }), deadline, current);
        if (typeof resolved === "string" && resolved) streamUrl = resolved;
        else if (resolved && typeof resolved === "object" && resolved.url) { streamUrl = resolved.url; transport = resolved; }
        else throw new Error("Station playback transport is unavailable.");
      }
      if (!current()) return false;
      source = {kind:"audio",url:streamUrl,sourceGeneration:request,canSeek:false,canVolume:true,canAutoplay:true,isExcerpt:false,
        provider:track.provider,title:station.name,audioCodec:station.codec??undefined,audioBitrateKbps:station.bitrateKbps??undefined,...transport};
      preview = source; source = preview; playingCandidate = null; position = 0; pendingResumePosition = 0; requiredRecoveryResume = null;
      await tick();
      if (!current() || !audio) return false;
      node = audio;
      const owns = () => current() && audio === node && preview === source;
      setWebviewOutputVolume(node, volume);
      await withinPreviewStartupBudget(() => prepareAudioStream(request, owns), deadline, owns);
      await withinPreviewStartupBudget(() => ownedStartupPlay(node!), deadline, owns);
      if (!owns()) return false;
      successfulRecoveryAttachment = {request,startedAt:performance.now(),position:node.currentTime,seekSerial};
      paused = false; previewStatus = `Playing ${station.name}`;
      armWebviewStallWatchdog(node);
      return true;
    } catch (failure) {
      if (current() && node && audio === node && preview === source) {
        blockedStartupNodes.add(node); node.pause(); audioStreamHandle?.destroy(); audioStreamHandle = null;
      }
      if (!current()) return false;
      preview = null; paused = true; bufferingAudio = false;
      previewStatus = `Radio could not play: ${safePreviewError(failure)}`;
      return false;
    }
  }
  function recoverAudioFailure(event: Event) {
    if (!(event.currentTarget instanceof HTMLAudioElement)) return;
    const node = event.currentTarget;
    const seek = activeWebviewSeek;
    if (pendingWebviewHandoff) return;
    if (seek && isCurrentWebviewSeek(seek)) {
      void attemptNativeSeekHandoff(node, seek).then(result => {
        if (result !== "handled" && !pendingWebviewHandoff && activeWebviewSeek === seek && isCurrentWebviewSeek(seek)) {
          if (result === "not-applicable") {
            recoverOwnedAudioFailure(node);
            return;
          }
          recoverOwnedAudioFailure(node, undefined, seek.target, playingCandidate ?? undefined);
        }
      });
      return;
    }
    recoverOwnedAudioFailure(node);
  }
  function webviewStallOwner(node: HTMLAudioElement, source: Preview, serial: number): WebviewStallOwner | null {
    const track = playingTrack;
    const occurrenceId = automationQueueProjection.entryIds[queueIndex] ?? null;
    if (node !== audio || preview !== source || !track || nativeActive() || paused ||
        !["audio", "local", "direct"].includes(source.kind) || !occurrenceId) return null;
    return { node, sourceGeneration: source.sourceGeneration ?? null, seekSerial: serial, queueOccurrenceId: occurrenceId };
  }
  function armWebviewStallWatchdog(node: HTMLAudioElement) {
    const source = preview, track = playingTrack, serial = seekSerial, ownerCandidate = playingCandidate;
    if (!(node instanceof HTMLAudioElement) || node !== audio || !source || !track || nativeActive()) return;
    const live = isStationPlaybackTrack(track);
    const owner = webviewStallOwner(node, source, serial);
    const attached = successfulRecoveryAttachment;
    const recovery = automaticPreviewRecovery;
    if (!owner || !attached || !recovery || attached.request !== recovery.request || attached.seekSerial !== serial ||
        source.kind === "local" || source.isExcerpt) return;
    const trackScope = JSON.stringify([track.id, track.provider, track.playbackRange ?? null, track.durationSeconds ?? null]);
    webviewStallWatchdog.arm({
      owner, currentNode: audio,
      isCurrent: () => audio === node && preview === source && playingTrack?.id === track.id &&
        JSON.stringify([playingTrack?.id, playingTrack?.provider, playingTrack?.playbackRange ?? null, playingTrack?.durationSeconds ?? null]) === trackScope && seekSerial === serial &&
        (automationQueueProjection.entryIds[queueIndex] ?? null) === owner.queueOccurrenceId && !nativeActive() && !paused &&
        successfulRecoveryAttachment?.request === recovery.request && successfulRecoveryAttachment.seekSerial === serial,
      mediaTime: node.currentTime, duration: node.duration, paused: paused || node.paused,
      seeking: node.seeking, ended: node.ended, hasError: Boolean(node.error), live, timeoutMs: live ? 5_000 : 20_000,
      onExpired: () => {
        recordWebviewMediaEvent("watchdog-expired", node);
        recoverOwnedAudioFailure(node, live ? "Radio made no playable progress for 5 seconds; reconnecting to the same station." : "Audio remained stalled for 20 seconds after playback started. Check the connection or choose another source.", undefined, ownerCandidate ?? undefined);
      },
    });
  }
  function recoverOwnedAudioFailure(node: HTMLAudioElement, fixedReason?: string, requestedRecoveryPosition?: number, stalledCandidate?: Candidate, allowPausedRecovery = false) {
    const ownedSeek = activeWebviewSeek && isCurrentWebviewSeek(activeWebviewSeek) ? activeWebviewSeek : null;
    if (node !== audio || !playingTrack || !preview ||
        !["audio", "local", "direct"].includes(preview.kind) || blockedStartupNodes.has(node) || (paused && successfulRecoveryAttachment !== null && !ownedSeek && !allowPausedRecovery)) return;
    webviewBufferingRecovery.reset();
    webviewStallWatchdog.reset();
    bufferingAudio = false;
    webviewSeekCompletionWatchdog.cancel(); activeWebviewSeek=null;
    const track = playingTrack, source = preview, oldRequest = automaticPreviewRecovery;
    const recoveryEntryIds = [...playbackQueue.entryIds];
    const recoveryIndex = queueIndex;
    const recoveryEntryId = recoveryEntryIds[recoveryIndex];
    if(ownedSeek?.deadline !== null && ownedSeek?.deadline !== undefined && oldRequest)oldRequest.deadline=ownedSeek.deadline;
    const live = Boolean(isStationPlaybackTrack(track) && liveStationForTrack(track));
    const failedCandidate = playingCandidate;
    const transportFailure = node.error?.code === 2 || classifyPlaybackFailure(mediaAudioFailureReason()).retryable ||
      /(?:manifest|level|audioTrack|frag|key)Load(?:Error|TimeOut)/i.test(lastHlsFailure);
    const currentTime = requestedRecoveryPosition ?? ownedSeek?.target ?? node.currentTime;
    const range = track.playbackRange;
    const sourceDuration = stableWebviewSourceDuration(track, source, node) ??
      (Number.isFinite(node.duration) && node.duration > 0 ? node.duration : null);
    const upperBound = range?.endTime ?? sourceDuration;
    const lowerBound = range?.startTime ?? 0;
    const resumeLimit = Number(upperBound);
    const scopedPosition = Number.isFinite(currentTime) && Number.isFinite(resumeLimit) && resumeLimit > 0 &&
      currentTime >= lowerBound && currentTime < resumeLimit;
    const positionPreservingScope = source.kind !== "local" && !source.isExcerpt && !playingRadioStation && !currentRadioSong &&
      !isStationPlaybackTrack(track) &&
      Number.isFinite(sourceDuration) && Number(sourceDuration) > 0;
    const canResumePosition = positionPreservingScope && scopedPosition;
    const seekRefresh = requestedRecoveryPosition !== undefined;
    async function advanceExhaustedRecovery(request: number, load: number) {
      // Only the still-owned occurrence can advance after recovery is exhausted.
      if (recoveryEntryIds.length < 2 || !previewRequests.isCurrent(request) || load !== playbackLoadSequence ||
          preview || !recoveryEntryId || recoveryIndex !== queueIndex ||
          recoveryEntryIds.length !== playbackQueue.entryIds.length ||
          !recoveryEntryIds.every((id, index) => id === playbackQueue.entryIds[index]) ||
          !shouldAdvanceAfterExhaustedFailure(previewStatus)) return;
      const sourceKey = JSON.stringify(playbackQueue.tracks.map(queueRadioSeedKey));
      const previous = activeFailedQueueRun;
      const failureRun: FailedQueueRun = previous && previous.sourceKey === sourceKey &&
        previous.entryIds.length === recoveryEntryIds.length && previous.entryIds.every((id, index) => id === recoveryEntryIds[index])
        ? previous : { entryIds: recoveryEntryIds, sourceKey, visited: new Set(), sourceRefreshRetries: new Set() };
      failureRun.visited.add(recoveryEntryId);
      activeFailedQueueRun = failureRun;
      const reason = previewStatus;
      const next = playbackQueue.nextAfterFailedStart(failureRun.visited);
      recentTrackFailures = appendTrackFailure(recentTrackFailures, track, {
        queueRevision: automationQueueProjection.revision, generation: request, operationId: playbackSession,
        entryId: recoveryEntryId, outcome: next ? "skipped" : "queue_exhausted",
      }, reason);
      failureHistoryRevision++;
      setNotice(skippedPreviewFailureNotice(track, safePreviewError(reason)), "persistent", "playbackSkip");
      if (next) {
        pendingResumePosition = 0;
        selected = next; refreshQueue();
        await startPreview(next, true, false, undefined, undefined, 1, null, false, failureRun);
      } else {
        previewStatus = `Could not continue ${safeNativeFailurePart(track.title, "Unavailable recording")}; reached the end of the queue. ${safePreviewError(reason)}`;
      }
    }
    function advanceEarlyRecoveryFailure() {
      const request = nextPreviewRequest();
      const load = ++playbackLoadSequence;
      void advanceExhaustedRecovery(request, load);
    }
    if(requestedRecoveryPosition !== undefined && !canResumePosition){
      const reason="The requested seek did not complete, and this source cannot be automatically resumed at that position. Choose Play to retry from the requested position.";
      node.pause(); preview=null; paused=true; bufferingAudio=false; requiredRecoveryResume=null;
      position=requestedRecoveryPosition; pendingResumePosition=requestedRecoveryPosition;
      resumeTrackId=track.id; resumePosition=requestedRecoveryPosition; previewStatus=reason; lastMediaFailure=reason;
      lastMediaFailureSource=lastResolvedSource; successfulRecoveryAttachment=null; persistPlaybackPosition();
      // Retry this exact queue occurrence once from its saved position. A terminal
      // startup failure then advances through the same bounded failure pass.
      if (source.kind === "local" && fixedReason?.startsWith("Audio stopped after the requested seek") && recoveryEntryIds.length > 1)
        void startPreview(track, true, false, undefined, undefined, 0, null, true, undefined, null, true);
      else advanceEarlyRecoveryFailure();
      return;
    }
    if (seekRefresh && (!stalledCandidate || !oldRequest)) {
      const reason = "The seek did not complete, and this source cannot be refreshed safely; choose Play to retry from the requested position.";
      node.pause(); preview=null; paused=true; bufferingAudio=false; requiredRecoveryResume=null;
      position=requestedRecoveryPosition; pendingResumePosition=requestedRecoveryPosition;
      resumeTrackId=track.id; resumePosition=requestedRecoveryPosition; previewStatus=reason; lastMediaFailure=reason;
      lastMediaFailureSource=lastResolvedSource; successfulRecoveryAttachment=null; persistPlaybackPosition();
      // Retry this exact queue occurrence once from its saved position. A terminal
      // startup failure then advances through the same bounded failure pass.
      if (source.kind === "local" && fixedReason?.startsWith("Audio stopped after the requested seek") && recoveryEntryIds.length > 1)
        void startPreview(track, true, false, undefined, undefined, 0, null, true, undefined, null, true);
      else advanceEarlyRecoveryFailure();
      return;
    }
    const invalidScopedPosition = positionPreservingScope && !scopedPosition;
    if (canResumePosition) {
      pendingResumePosition = currentTime;
      position = currentTime;
    } else {
      pendingResumePosition = 0;
      requiredRecoveryResume = null;
    }
    lastMediaFailureSource = lastResolvedSource;
    const attached = successfulRecoveryAttachment;
    const establishedPlayback = !paused && oldRequest && attached?.request === oldRequest.request
      && attached.seekSerial === seekSerial && performance.now() - attached.startedAt >= 5_000
      && Number.isFinite(node.currentTime) && node.currentTime - attached.position >= 5;
    const steadyPlayback = establishedPlayback && (attached.healthy || (live ? node.currentTime-attached.position >= 5
      : performance.now()-(attached.healthStartedAt ?? attached.startedAt) >= HEALTHY_PLAYBACK_RESET_MS && node.currentTime-(attached.healthPosition ?? attached.position) >= HEALTHY_PLAYBACK_RESET_MS/1000));
    if (steadyPlayback && !ownedSeek) {
      activeFailedQueueRun = null;
      oldRequest.deadline = performance.now() + playbackStartupTimeoutMs();
      oldRequest.attempts = 0;
      oldRequest.sourceAttempts = 0;
      oldRequest.sourceStartupDeadline = undefined;
      oldRequest.tried = new Set<string>();
      oldRequest.liveRecoveryDeadline = undefined;
      oldRequest.transportRefreshUsed = false;
      oldRequest.transportRecoveryWindowUsed = false;
      oldRequest.recurringStalls = emptyRecurringStallRecoveryState();
      oldRequest.stallFallbacks = 0;
    }
    // A proven attachment gets a new bounded recovery budget, without erasing
    // its recent failures merely because it played briefly.
    if (establishedPlayback && !ownedSeek && oldRequest) {
      oldRequest.deadline = performance.now()+playbackStartupTimeoutMs();
      oldRequest.attempts = 0;
      oldRequest.sourceAttempts = 0;
      oldRequest.sourceStartupDeadline = undefined;
      oldRequest.transportRecoveryWindowUsed = false;
    }
    let replaceRecurringSource = false;
    if (oldRequest && failedCandidate && !live && !seekRefresh && (stalledCandidate || transportFailure)) {
      const recorded = recordPlaybackStall(oldRequest.recurringStalls ?? emptyRecurringStallRecoveryState(),candidateKey(failedCandidate),performance.now());
      oldRequest.recurringStalls = recorded.state;
      replaceRecurringSource = recorded.replaceCandidate;
    }
    successfulRecoveryAttachment = null;
    const request = nextPreviewRequest();
    requiredRecoveryResume = canResumePosition ? { request, position: currentTime, verifiedAtMs: null } : null;
    const failureReason = fixedReason ?? mediaAudioFailureReason();
    node.pause();
    preview = null;
    paused = true;
    if (invalidScopedPosition) {
      const resumeFailure = "Audio could not resume at its previous playback position; choose a source that supports seeking.";
      if (oldRequest) oldRequest.failureReason = resumeFailure;
      previewStatus = resumeFailure;
      lastMediaFailure = resumeFailure;
      void advanceExhaustedRecovery(request, ++playbackLoadSequence);
      return;
    }
    previewStatus = fixedReason ? fixedReason : "Audio preview failed. Trying another match…";
    lastMediaFailure = failureReason;
    const load = ++playbackLoadSequence; resolvingPlayback = true;
    audioRecovery = run(async () => {
      if (!oldRequest || oldRequest.track.id !== track.id) {
        automaticPreviewRecovery = { request, track, tried: new Set(), attempts: 0 };
        await findAndStartPreviewSource(track, request);
        return;
      }
      oldRequest.request = request;
      oldRequest.failureReason = failureReason;
      if (live) {
        oldRequest.liveRecoveryDeadline ??= performance.now() + playbackStartupTimeoutMs();
        oldRequest.deadline = oldRequest.liveRecoveryDeadline;
        while (previewRequests.isCurrent(request) && oldRequest.attempts < 2 && performance.now() < oldRequest.deadline) {
          oldRequest.attempts++;
          previewStatus = "Reconnecting to the same station…";
          if (await startRadioWebviewAudio(track, request)) return;
        }
        if (previewRequests.isCurrent(request)) { paused = true; previewStatus = "Radio reconnect failed; choose Play to retry this station."; }
        return;
      }
      if (replaceRecurringSource && failedCandidate && !oldRequest.exactSourceOnly && !seekRefresh) {
        if ((oldRequest.stallFallbacks ?? 0) >= 2) {
          previewStatus = "Several verified sources repeatedly stalled; choose Play to retry.";
          lastMediaFailure = previewStatus;
          return;
        }
        oldRequest.stallFallbacks = (oldRequest.stallFallbacks ?? 0)+1;
        oldRequest.tried.add(candidateKey(failedCandidate));
        oldRequest.deadline = performance.now()+playbackStartupTimeoutMs();
        oldRequest.attempts = 0; oldRequest.sourceAttempts = 0;
        oldRequest.transportRefreshUsed = false;
        await findAndStartPreviewSource(track,request,true,1);
        if (previewRequests.isCurrent(request) && (preview || nativeActive()) && playingCandidate && candidateKey(playingCandidate)!==candidateKey(failedCandidate)) {
          setNotice(`Repeated buffering on ${recoveryProviderLabel(failedCandidate.provider)}; switched to ${recoveryProviderLabel(playingCandidate.provider)}.`);
        }
        return;
      }
      // A network/transport failure may mean an expired URL, including a pinned source.
      // Keep decode/unsupported-format failures terminal for pins.
      const refreshCandidate = stalledCandidate ?? (transportFailure ? failedCandidate ?? undefined : undefined);
      if (refreshCandidate && !oldRequest.transportRefreshUsed) {
        oldRequest.transportRefreshUsed = true;
        oldRequest.stallCause = failureReason;
        if (await refreshStalledCandidate(track, request, refreshCandidate, failureReason, Boolean(stalledCandidate) || transportFailure || seekRefresh, seekRefresh)) return;
        if (!previewRequests.isCurrent(request)) return;
        if (seekRefresh) {
          oldRequest.failureReason = `${failureReason} The same-source refresh failed; choose Play to retry from the requested position.`;
          previewStatus = oldRequest.failureReason;
          lastMediaFailure = oldRequest.failureReason;
          return;
        }
        if (!oldRequest.exactSourceOnly && oldRequest.attempts < 2) {
          await findAndStartPreviewSource(track, request, true, 1);
          return;
        }
        previewStatus = oldRequest.failureReason ?? failureReason;
        lastMediaFailure = failureReason;
        return;
      }
      if (refreshCandidate && oldRequest.transportRefreshUsed) {
        previewStatus = oldRequest.failureReason ?? failureReason;
        lastMediaFailure = failureReason;
        return;
      }
      if (oldRequest.exactSourceOnly) {
        oldRequest.failureReason = failureReason;
        previewStatus = `The selected ${importedAudioCandidate(track)?.provider ?? "audio"} source could not play: ${oldRequest.failureReason}`;
        return;
      }
      await findAndStartPreviewSource(track, request, true);
    }).then(() => advanceExhaustedRecovery(request, load)).finally(() => { if(load===playbackLoadSequence) { resolvingPlayback=false; if (!preview) bufferingAudio=false; } });
  }
  let webviewResumeSequence = 0;
  let pendingWebviewResume: {token:number;cancelledGeneration:number|null;node:HTMLAudioElement;source:Preview;
    startPosition:number;playRequestedAtMs:number|null;started:Promise<void>;resolveStarted:()=>void} | null = null;
  function cancelPendingWebviewResume() {
    const attempt = pendingWebviewResume;
    if (!attempt) return;
    attempt.cancelledGeneration = ++webviewResumeSequence;
    pendingWebviewResume = null;
    if (audio === attempt.node && preview === attempt.source) {
      bufferingAudio = false; paused = true; attempt.node.pause(); previewStatus = "Paused";
    }
  }
  function signalWebviewResumeProgress(node: HTMLAudioElement) {
    const attempt = pendingWebviewResume;
    if (!attempt || attempt.node !== node || node !== audio || attempt.source !== preview ||
        attempt.playRequestedAtMs === null || node.paused || node.seeking || node.ended || node.error || node.readyState < 2 ||
        !Number.isFinite(node.currentTime) || node.currentTime - attempt.startPosition < 0.25) return;
    // Some WebViews leave play() pending after media has actually resumed. Only
    // accept a resume after this exact node/source has advanced media time while
    // unpaused; a bare play/playing event or readyState alone is not enough.
    attempt.resolveStarted();
  }
  async function resumeCurrentWebviewAudio(automationRequestId?: string): Promise<"played"|"cancelled"|"failed"> {
    const node = audio, source = preview, trackId = playingTrack?.id;
    if (!node || !source || !trackId) return "cancelled";
    cancelPendingWebviewResume();
    const session = playbackSession, serial = seekSerial, startupGeneration = audioStartupGeneration;
    const entryId = automationQueueProjection.entryIds[queueIndex] ?? null, index = queueIndex;
    let resolveStarted!: () => void;
    const started = new Promise<void>(resolve => { resolveStarted = resolve; });
    const attempt = {token:++webviewResumeSequence,cancelledGeneration:null as number|null,node,source,
      startPosition:node.ended ? 0 : node.currentTime,playRequestedAtMs:null as number|null,started,resolveStarted};
    pendingWebviewResume = attempt;
    const startedAt = performance.now(), deadline = startedAt + playbackStartupTimeoutMs();
    const sameSource = () => audio === node && preview === source && playingTrack?.id === trackId && playbackSession === session && source.sourceGeneration === preview?.sourceGeneration;
    const stillCurrent = () => pendingWebviewResume === attempt && sameSource() && seekSerial === serial && audioStartupGeneration === startupGeneration && queueIndex === index && (automationQueueProjection.entryIds[index] ?? null) === entryId && (!automationRequestId || activeAutomationPlayback === automationRequestId);
    let progressCheck: ReturnType<typeof setInterval> | undefined;
    bufferingAudio = true; previewStatus = "Resuming audio…";
    try {
      await withinPreviewStartupBudget(() => {
        attempt.playRequestedAtMs = performance.now();
        blockedStartupNodes.delete(node);
        const promise = node.play();
        // A cancelled play can settle late; never pause a newer resume/source.
        void promise.then(() => {
          if (attempt.cancelledGeneration !== null && webviewResumeSequence === attempt.cancelledGeneration && !pendingWebviewResume && sameSource()) node.pause();
        }, () => {});
        // As at first startup, a settled play() is not proof that the decoder
        // has resumed. Require progress from this source under the same budget.
        progressCheck = setInterval(() => {
          if (stillCurrent()) signalWebviewResumeProgress(node);
        }, 25);
        return new Promise<void>((resolve, reject) => {
          void promise.then(() => {
            if (stillCurrent()) signalWebviewResumeProgress(node);
          }, reject);
          void attempt.started.then(resolve);
        });
      }, deadline, stillCurrent);
      if (!stillCurrent()) return "cancelled";
      paused = false; bufferingAudio = false;
      armPendingWebviewSeekAfterResume(node);
      if(!activeWebviewSeek){updateSuccessfulRecoverySeek(node);armWebviewStallWatchdog(node);}
      previewStatus = source.isExcerpt ? "Playing preview" : `Playing ${playingCandidate?.provider ?? source.provider ?? "verified"} audio`;
      reportQueuedPlaybackPhase("playing");
      return "played";
    } catch (failure) {
      if (!stillCurrent()) {
        if (pendingWebviewResume === attempt) cancelPendingWebviewResume();
        return "cancelled";
      }
      pendingWebviewResume = null;
      attempt.cancelledGeneration = ++webviewResumeSequence;
      node.pause(); paused = true; bufferingAudio = false;
      recordWebviewStartupFailure(captureWebviewStartupFailure(node,"play",source.audioMimeType,audioStreamHandle?.kind,true,performance.now()-startedAt,pendingResumePosition,source.sourceTransport));
      const reason = mediaAudioFailureReason();
      if (shouldRefreshWebviewResumeSource(node.error?.code, reason)) {
        recoverOwnedAudioFailure(node, reason, undefined, playingCandidate ?? undefined, true);
      } else {
        previewStatus = `Playback could not resume: ${safePreviewError(failure)}`;
        reportQueuedPlaybackPhase("failed");
      }
      return "failed";
    } finally {
      if (progressCheck !== undefined) clearInterval(progressCheck);
      if (pendingWebviewResume === attempt) { pendingWebviewResume = null; bufferingAudio = false; }
    }
  }
  function playCurrentWebViewAudio() {
    if (!audio) return;
    if (audio.ended) {
      playbackRunGeneration++;
      queueRadio.cancel();
      preparedRadioKey = "";
    }
    void resumeCurrentWebviewAudio();
  }
  function togglePlayback() {
    if (nativeActive()) { toggleCurrentPlayback(); return; }
    if(playbackLoading){toggleCurrentPlayback();return;}
    const action = playbackAction(
      selected?.id,
      playingTrack?.id,
      Boolean(preview && (audio || preview.sessionId)),
    );
    if (action === "start-selected" && selected) {
      // Space on the restored current track is Resume; selecting another row is explicit playback.
      const resumeCurrent = !preview && Boolean(playingTrack && sameTrackIdentity(selected, playingTrack) && playbackQueue.current && sameTrackIdentity(selected, playbackQueue.current));
      if (resumeCurrent) void startPreview(selected, true, false, undefined, undefined, 0, null, true);
      else void startPreview(selected);
      return;
    }
    if(action==="toggle-current" && !paused)reportQueuedPlaybackPhase("paused");
    if (action === "toggle-current" && preview?.sessionId) {
      void run(() => call("candidate_preview_control", { action: paused ? "play" : "pause", value: null }));
    } else if (action === "toggle-current" && audio && preview) {
      if (audio.paused) playCurrentWebViewAudio();
      else audio.pause();
    } else if (selected) void startPreview(selected);
    else if (playbackQueue.current) void startPreview(playbackQueue.current, true, false, undefined, undefined, 0, null, true);
  }
  function cancelNaturalQueueRadioContinuation() {
    playbackRunGeneration++;
    queueRadio.cancel();
    preparedRadioKey = "";
  }
  function toggleCurrentPlayback() {
    if (deferUntilWebviewHandoffSettles(() => toggleCurrentPlayback(), false)) return;
    if(!paused)reportQueuedPlaybackPhase("paused");
    if (nativeActive()) {
      if (paused && nativePlaybackStatus === "ended" && nativeNaturalQueueEnd) cancelNaturalQueueRadioContinuation();
      void nativeCommand({action:paused ? "resume" : "pause"});
      return;
    }
    if (pendingWebviewResume) { cancelPendingWebviewResume(); return; }
    if (resolvingPlayback) { void closePreview(true); previewStatus = "Paused"; return; }
    if (preview?.sessionId) void run(() => call("candidate_preview_control", { action: paused ? "play" : "pause", value: null }));
    else if (audio && preview) { if (audio.paused) playCurrentWebViewAudio(); else audio.pause(); }
    else if (playingTrack) void startPreview(playingTrack, true, false, undefined, undefined, 0, null, true);
    else if (playbackQueue.current) void startPreview(playbackQueue.current, true, false, undefined, undefined, 0, null, true);
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
    if(!request || typeof request.captureId!=="string" || !/^[0-9a-f-]{36}$/i.test(request.captureId) ||
      request.trackId!==playingTrack?.id || request.sessionId!==playbackSession || request.generation!==preview?.sourceGeneration ||
      request.seekSerial!==seekSerial || requestChapterId!==currentWebviewChapterId() || nativeActive() || !audio) {
      await call("automation_identify_complete",{captureId:request?.captureId??"",result:{},error:"Playback changed before identification began"}).catch(()=>{});
      return;
    }
    if (radioRecognizing) {
      await call("automation_identify_complete",{captureId:request.captureId,result:{},error:"Audio identification is already in progress"}).catch(()=>{});
      return;
    }
    const id=request.trackId,session=request.sessionId,sourceGeneration=request.generation,serial=request.seekSerial,chapterId=requestChapterId;
    const sourceUrl=audio.currentSrc||audio.src||"";
    const current=()=>playingTrack?.id===id && playbackSession===session && preview?.sourceGeneration===sourceGeneration &&
      seekSerial===serial && currentWebviewChapterId()===chapterId && !nativeActive() && Boolean(audio && sourceUrl && (audio.currentSrc||audio.src)===sourceUrl);
    const controller=new AbortController();radioIdentifyAbort=controller;radioIdentifyCaptureId=request.captureId;radioRecognizing=true;radioIdentifyProgress=0;
    try {
      const result=await identifyCurrentWebviewAudio({captureId:request.captureId,trackId:id,stationId:playingRadioStation?.id,sessionId:session,generation:sourceGeneration,seekSerial:serial,chapterId,isCurrent:current,signal:controller.signal,onProgress:(seconds)=>radioIdentifyProgress=seconds});
      const recognized=(result as any)?.nowPlaying;
      if(recognized && current() && playingRadioStation){
        lastRadioAudioAttempt={stationId:playingRadioStation.id,generation:sourceGeneration,at:Date.now()};
        radioNowPlaying(playingRadioStation,recognized);
      }
      const completion={trackId:id,title:recognized?.title??(result as any)?.title??null,artist:recognized?.artist??(result as any)?.artist??null,artworkUrl:recognized?.artworkUrl??(result as any)?.artworkUrl??null,note:(result as any)?.note??null};
      await call("automation_identify_complete",{captureId:request.captureId,result:completion,error:result?null:"Playback changed during identification"}).catch(()=>{});
    } catch(failure) {
      const completionError = controller.signal.aborted ? "Audio identification cancelled" : String(failure).slice(0,500);
      await call("automation_identify_complete",{captureId:request.captureId,result:{},error:completionError}).catch(()=>{});
    } finally { if(radioIdentifyAbort===controller){radioIdentifyAbort=null;radioIdentifyCaptureId=null;radioRecognizing=false;} }
  }
  function cancelAutomationIdentify(request:{captureId:string}) {
    if(request?.captureId && radioIdentifyCaptureId===request.captureId)cancelRadioIdentification(request.captureId);
  }
  function automationPlaybackSnapshot(forceFull = false) {
    const projection=automationQueueProjection;
    resetTrackFailureHistoryForQueue(projection.revision, projection.sourceKey);
    const base={ trackId: playingTrack?.id ?? null, entryId: projection.entryIds[queueIndex] ?? null, sessionId: playbackSession, chapterId: currentWebviewChapterId(), track: playingTrack ? { id: playingTrack.id, artist: playingTrack.artist, title: playingTitle, album: playingTrack.album, artworkUrl: playingTrack.artworkUrl, artworkFilePath: artworkFilePaths.get(playingTrack.id) ?? null, localPath: playingTrack.localPath, durationSeconds: playingTrack.durationSeconds, sourceUrl: playingTrack.sourceUrl } : null,
      paused, position:playerPosition, absolutePosition:position, sourceGeneration:nativeResolutionGeneration ?? preview?.sourceGeneration ?? null, duration:playerDuration, volume, seekSerial, canSeek: Boolean(preview?.canSeek), canVolume: Boolean(preview?.canVolume), shuffle: shuffleEnabled, repeat: repeatMode, previewKind: preview?.kind ?? null, playerMessage: previewStatus, readyState: audio?.readyState ?? null, mediaError: audio?.error?.message ?? null, lastMediaFailure, lastStartupFailure, lastResolvedSource, lastMediaFailureSource, radioNowPlaying:currentRadioSong?{id:currentRadioSong.id,title:currentRadioSong.title,artist:currentRadioSong.artist,stationName:playingRadioStation?.name}:null,radioHistory:radioQueueHistory.map(item=>({id:item.id,trackId:item.track.id,title:item.track.title,artist:item.track.artist,playedAt:item.playedAt,stationName:item.stationName})), status: playbackLoading ? "buffering" : preview ? (paused ? "paused" : "playing") : systemPausedTrack ? "paused" : "idle", currentIndex: queueIndex };
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
    if (pendingWebviewHandoff && !["seek", "pause", "resume"].includes(request.action)) await cancelPendingWebviewHandoff();
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
          if(nativeActive()) {
            await startPreview(requestedTracks[0]!, false, false, requestedTracks, 0, 0, request.requestId);
            if (audioRecovery) await audioRecovery;
            if (playingTrack?.id !== requestedTracks[0]!.id || !preview || paused) throw new Error(previewStatus || "No playable audio found");
          } else {
            playbackQueue.setTracks(requestedTracks, 0); refreshQueue();
            const suppliedEntryIds = playbackQueue.snapshot().entryIds ?? [];
            await startPreview(requestedTracks[0]!, true, false, undefined, undefined, 0, request.requestId);
            if (audioRecovery) await audioRecovery;
            const currentQueue = playbackQueue.snapshot();
            const exactSuppliedQueue = suppliedEntryIds.length === requestedTracks.length && currentQueue.entryIds?.length === suppliedEntryIds.length
              && suppliedEntryIds.every((id,index) => currentQueue.entryIds?.[index] === id)
              && sameNativePlaybackQueue(currentQueue.tracks, requestedTracks);
            const suppliedOccurrence = requestedTracks[currentQueue.currentIndex];
            if (activeAutomationPlayback !== request.requestId) throw new Error("Playback request was cancelled or superseded");
            const sameActiveOccurrence = suppliedOccurrence && playingTrack &&
              (suppliedOccurrence.provider === "radio" && playingTrack.provider === "radio"
                ? suppliedOccurrence.id === playingTrack.id && suppliedOccurrence.providerId === playingTrack.providerId
                : sameNativePlaybackTrack(playingTrack, suppliedOccurrence));
            if (!exactSuppliedQueue || !suppliedOccurrence || !sameActiveOccurrence || !preview || paused) {
              throw new Error(previewStatus || "No playable audio found");
            }
          }
        } else if (nativeActive()) {
          if (request.mode === "replace") clearPlaybackSkipNotice();
          await nativeCommand({action:"queue",tracks:requestedTracks,mode:request.mode === "replace" ? "replace" : request.mode === "next" ? "next" : "append"});
        } else {
          if (request.mode === "replace") clearPlaybackSkipNotice();
          if (request.mode === "replace" || !queueTracks.length) playbackQueue.setTracks(requestedTracks, 0);
          else if (request.mode === "next") playbackQueue.appendNextTracks(requestedTracks);
          else playbackQueue.appendTracks(requestedTracks);
          refreshQueue();
        }
      } else if (request.action === "pause" || request.action === "resume") {
        await pendingWebviewHandoff?.settled;
        const action = request.action === "pause" ? "pause" : "play";
        const atNaturalQueueEnd = nativeActive()
          ? nativePlaybackStatus === "ended" && nativeNaturalQueueEnd
          : Boolean(audio?.ended);
        if (atNaturalQueueEnd) cancelNaturalQueueRadioContinuation();
        if (nativeActive()) await nativeCommand({action:action === "pause" ? "pause" : "resume"});
        else if (preview?.sessionId) await call("candidate_preview_control", { action, value: null });
        else if (audio && preview) {
          if (action === "pause") { cancelPendingWebviewResume(); audio.pause(); }
          else {
            const resumed = await resumeCurrentWebviewAudio(resumeOwner??undefined);
            if (resumed === "cancelled") throw new Error("Playback resume was cancelled or superseded");
            if (resumed !== "played") throw new Error(previewStatus || "Playback could not resume");
          }
        }
        else if (action === "play" && (playingTrack || playbackQueue.current)) {
          const started = await startPreview((playingTrack || playbackQueue.current)!, true, false, undefined, undefined, 0, resumeOwner, true);
          if (!started) throw new Error(previewStatus || "Playback could not start.");
        }
        else if (action === "play") throw new Error("Nothing is queued to play");
        if (action === "pause") {
          if(queuedOwnerForResume())reportQueuedPlaybackPhase("paused");else setAutomationPlaybackOwner(null);
          // Pausing attached media must retain its relay registration. Advancing
          // the backend generation here retires the URL that paused seek/resume
          // still needs. Pending resolution or a stale attachment still cancels.
          const attachedSourceIsCurrent = !nativeActive() && audio && preview && !resolvingPlayback &&
            typeof preview.sourceGeneration === "number" && previewRequests.isCurrent(preview.sourceGeneration);
          if (attachedSourceIsCurrent) nextCandidatePrefetch.cancel();
          else nextPreviewRequest();
        }
        else reportQueuedPlaybackPhase("playing");
        paused = action === "pause";
      } else if (request.action === "seek") {
        await pendingWebviewHandoff?.settled;
        const seconds = nativeActive() ? Number(request.position) : playingTrack ? chapterAbsolutePosition(playingTrack, Number(request.position)) : Number(request.position);
        if (!Number.isFinite(seconds) || seconds < 0 || (nativeActive() ? !nativeSnapshot?.canSeek : !preview?.canSeek)) throw new Error("This audio cannot seek");
        let mediaSeek = false;
        if (nativeActive()) await nativeCommand({action:"seek",position:seconds});
        else if (preview?.sessionId) await call("candidate_preview_control", { action: "seek", value: seconds });
        else if (audio) {
          if(!seekWebviewAudio(audio,seconds))throw new Error(previewStatus||"Could not seek to an invalid audio position.");
          mediaSeek = true;
        }
        else throw new Error("Player is not ready");
        position = mediaSeek && audio ? audio.currentTime : seconds; if(!mediaSeek)seekSerial++;
      } else if (request.action === "volume") {
        const value = Number(request.volume);
        if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error("Volume must be from 0 to 1");
        volume = value;
        if (nativeActive()) await nativeCommand({action:"volume",volume:value});
        else { if (audio) setWebviewOutputVolume(audio, value); if (preview?.sessionId) await call("candidate_preview_control", { action: "volume", value }); }
      } else if (request.action === "stop") {
        await closePreview(); position = 0; if(!nativeActive())persistPlaybackPosition();
      } else if (request.action === "shuffle") {
        const enabled = request.mode === "true";
        if (enabled !== shuffleEnabled) { shuffleEnabled = enabled; if (nativeActive()) await nativeCommand({action:"shuffle",enabled}); else { if (enabled) playbackQueue.shuffle(); refreshQueue(); } }
      } else if (request.action === "repeat") {
        if (!["off", "all", "one"].includes(request.mode ?? "")) throw new Error("Invalid repeat mode");
        repeatMode = request.mode as "off" | "all" | "one"; if(nativeActive())await nativeCommand({action:"repeat",mode:repeatMode});else{playbackQueue.setRepeat(repeatMode);refreshQueue();}
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
        if(nativeActive()) await startPreview(track,false,false,[track],0,0,request.requestId);
        else { playbackQueue.setTracks([track], 0); refreshQueue(); await startPreview(track, true, false, undefined, undefined, 0, request.requestId); }
      } else if (request.action === "next" || request.action === "previous") {
        clearPlaybackSkipNotice();
        if (nativeActive()) {
          const direction = request.action === "next" ? 1 : -1;
          await nativeCommand({action:direction > 0 ? "next" : "previous"});
        } else if (paused) {
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
    const envelope=automationPlaybackSnapshot(true);
    await call<PlaybackStateAck|undefined>("automation_playback_complete", { requestId: request.requestId, playback: envelope.playback, error: failure }).then(ack=>automationPlaybackPublisher.acknowledge(envelope,ack)).catch(() => {});
  }
  async function saveAutomationPermissions() {
    await run(async () => { const result = await call<{permissions: {control: boolean; manage: boolean}}>("set_automation_permissions", { ...automationPermissions }); automationPermissions = result.permissions; });
  }
  function openPlayerPanel(panel: "info" | "queue" | "downloads" | null, track?: Track) {
    showFullNowPlaying = false;
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
  $effect(() => {
    const request = ++trackLibrarySummaryRequest;
    const trackId = showPlayingInfo ? (infoTrack ?? playingTrack)?.id ?? null : null;
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
    if (desktop() && !backupHydrationReady) return;
    if(engineSwitchBusy){e.preventDefault();return;}
    const target = e.target instanceof Element ? e.target : null;
    if (target?.closest("dialog[open]")) return;
    if (e.key === "Escape" && showFullNowPlaying) { e.preventDefault(); showFullNowPlaying = false; return; }
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
    const rows = displayed ?? [track];
    const rowIndex = Number.isInteger(index) && index >= 0 && index < rows.length ? index : Math.max(0, rows.indexOf(track));
    let playableTrack = rows[rowIndex] ?? track;
    if (!(playableTrack.localAvailability === "available" && playableTrack.localPath)) {
      try { playableTrack = (await matchArtistPlaybackSources([playableTrack]))[0] ?? playableTrack; }
      catch (failure) { if (request === artistPlaybackMatchGeneration) error = String(failure); playableTrack = rows[rowIndex] ?? track; }
    }
    if (request !== artistPlaybackMatchGeneration) return;
    const playableRows = playableTrack === rows[rowIndex] ? rows : rows.map((item, itemIndex) => itemIndex === rowIndex ? playableTrack : item);
    selected = playableTrack;
    if(nativeActive()) { await startPreview(playableTrack,false,false,playableRows,rowIndex,0,null,false,undefined,albumResource); return; }
    playbackQueue.setTracks(playableRows, rowIndex);
    if (albumResource) playbackQueue.setLoudnessContext(playbackQueue.entryIds, albumResource);
    playbackQueue.setRepeat(repeatMode);
    refreshQueue();
    await startPreview(playableTrack, true, false, undefined, undefined, 0, null, false, undefined, albumResource);
  }
  function queueArtistTracks(items: Track[], mode: "append" | "next", albumResource: LoudnessResourceIdentity | null = null) {
    if (!items.length) return;
    if (deferUntilWebviewHandoffSettles(() => queueArtistTracks(items, mode, albumResource))) return;
    if (nativeActive()) { void nativeCommand({action:"queue",tracks:items,mode,loudnessAlbumResource:albumResource}); return; }
    let added: readonly import("./lib/playback-queue").PlaybackQueueEntry[];
    if (!queueTracks.length) {
      playbackQueue.setTracks(items, 0);
      added = playbackQueue.entries;
    }
    else if (mode === "next") added = playbackQueue.appendNextTracks(items);
    else added = playbackQueue.appendTracks(items);
    if (albumResource) playbackQueue.setLoudnessContext(added.map(entry => entry.entryId), albumResource);
    refreshQueue();
    setNotice(`${items.length === 1 ? items[0].title : `${items.length} tracks`} added ${mode === "next" ? "next" : "to queue"}.`);
  }
  function sortUpcoming(field:TrackSortField,direction:SortDirection) {
    if (deferUntilWebviewHandoffSettles(() => sortUpcoming(field,direction))) return;
    if(nativeActive())void nativeCommand({action:"sort_upcoming",field,direction});
    else {playbackQueue.sortUpcoming(field,direction);refreshQueue();}
  }
  async function moveQueueItem(from:number,to:number,expectedQueueRevision?:number) { await cancelPendingWebviewHandoff(); if(nativeActive()){try{return await nativeBridge.command({action:"reorder",from,to,...(expectedQueueRevision===undefined?{}:{expectedQueueRevision})});}catch(failure){error=String(failure);throw failure;}}else{playbackQueue.reorder(from,to);refreshQueue();} }
  async function removeQueueItems(indices:number[]) {
    await cancelPendingWebviewHandoff();
    const unique = [...new Set(indices)].filter(index => Number.isInteger(index) && index >= 0 && index < queueTracks.length && index !== queueIndex).sort((a,b)=>b-a);
    if (!unique.length) return;
    if (nativeActive()) await nativeCommand({action:"remove_many",indices:unique,expectedQueueRevision:nativeSnapshot?.queueRevision});
    else { for (const index of unique) playbackQueue.removeAt(index); refreshQueue(); }
  }
  function removeQueueItem(index:number) { if(deferUntilWebviewHandoffSettles(()=>removeQueueItem(index)))return;if(nativeActive())void nativeCommand({action:"remove",index});else{playbackQueue.removeAt(index);refreshQueue();} }
  function toggleShuffle() { if(deferUntilWebviewHandoffSettles(()=>toggleShuffle()))return;shuffleEnabled=!shuffleEnabled;if(nativeActive())void nativeCommand({action:"shuffle",enabled:shuffleEnabled});else{if(shuffleEnabled)playbackQueue.shuffle();refreshQueue();} }
  function cycleRepeat() { if(deferUntilWebviewHandoffSettles(()=>cycleRepeat()))return;repeatMode=repeatMode==="off"?"all":repeatMode==="all"?"one":"off";if(nativeActive())void nativeCommand({action:"repeat",mode:repeatMode});else{playbackQueue.setRepeat(repeatMode);refreshQueue();} }
  function playQueueIndex(index:number) { if(deferUntilWebviewHandoffSettles(()=>playQueueIndex(index)))return;clearPlaybackSkipNotice(); if(nativeActive())void nativeCommand({action:"select",index});else{playbackQueue.restore({...playbackQueue.snapshot(),currentIndex:index});refreshQueue();const track=queueTracks[index];if(track)void startPreview(track,true);} }
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
  function analysisStatusForTrack(track: Track) {
    const assetId = assetIdForTrack(track);
    return assetId ? analysisStatuses.find((status) => status.assetId === assetId) : undefined;
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
    if (webviewLoudnessSettings.analyzeWhenIdle && isLoudnessPlaybackActive()) {
      loudnessRetryRequired = false;
      pendingLoudnessAssetIds = [...new Set([...pendingLoudnessAssetIds, ...assetIds])];
      loudnessAnalysisMessage = "Waiting for playback to pause. Selected files are untouched until analysis runs.";
      setNotice(loudnessAnalysisMessage);
      return;
    }
    loudnessRetryRequired = false;
    pendingLoudnessAssetIds = [];
    await launchLoudnessAnalysis(assetIds, webviewLoudnessSettings.analyzeWhenIdle);
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
    void launchLoudnessAnalysis(pendingLoudnessAssetIds, webviewLoudnessSettings.analyzeWhenIdle);
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
    const enabled = webviewLoudnessSettings.analyzeWhenIdle;
    const idle = !isLoudnessPlaybackActive();
    if (!pending.length || !enabled || !idle || loudnessAnalysisBusy || loudnessLaunchScheduled || loudnessRetryRequired) return;
    loudnessLaunchScheduled = true;
    queueMicrotask(() => {
      loudnessLaunchScheduled = false;
      if (pendingLoudnessAssetIds.length && webviewLoudnessSettings.analyzeWhenIdle && !isLoudnessPlaybackActive() && !loudnessAnalysisBusy) {
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
        if(nativeActive()) { await nativeCommand({action:"queue",tracks:queue,mode:"replace",preserveCurrent:Boolean(keepPlaying)}); if(!keepPlaying)await startPreview(seedTrack,false,false,queue,0); }
        else { playbackQueue.setTracks(queue, 0); refreshQueue(); if (!keepPlaying) await startPreview(seedTrack, true); }
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
    if (nativeActive()) {
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

    cancelWebviewStartupPlay?.();
    cancelPendingWebviewResume();
    const node = audio;
    if (!node) {
      if (paused) return;
      throw new Error("WebView playback has no controllable audio element to pause");
    }
    const source = preview;
    node.pause();
    let previousTime = Number.NaN;
    let stablePolls = 0;
    while (Date.now() < deadline) {
      if (audio !== node || preview !== source) {
        throw new Error("WebView playback changed before it reached a stable paused checkpoint");
      }
      if (node.paused && !activeWebviewSeek && !node.seeking && Number.isFinite(node.currentTime) && Math.abs(node.currentTime - previousTime) < 0.005) {
        stablePolls += 1;
        if (stablePolls >= 2) {
          position = node.currentTime;
          paused = true;
          bufferingAudio = false;
          return;
        }
      } else {
        stablePolls = 0;
      }
      previousTime = node.currentTime;
      await wait(60);
    }
    throw new Error("WebView playback did not reach a stable paused checkpoint");
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
        position: nativeActive() ? position : playerPosition,
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
  const playingRadioStation=$derived((playingTrack?.provider==="radioparadise" || playingTrack?.provider==="internet-radio" || (playingTrack?.provider==="radio" && (playingTrack?.provenance as {sourceKind?:string})?.sourceKind==="radio")) ? radioStations.find(station=>station.id===(playingTrack?.provenance as {stationId?:string})?.stationId)??null:null);
  async function recordCurrentRadioSong(station:RadioStation,item:RadioTrackMetadata) {
    if(paused || !audio || audio.paused || playingTrack?.id!==`radio:${station.id}` || !playbackSession)return;
    const session=playbackSession;
    const identity=`${station.id}\n${item.artist.trim().toLocaleLowerCase()}\n${item.title.trim().toLocaleLowerCase()}`;
    if(!radioOccurrence || radioOccurrence.session!==session || radioOccurrence.identity!==identity)radioOccurrence={session,identity,key:crypto.randomUUID(),at:new Date().toISOString()};
    const occurrence=radioOccurrence;
    try {
      const track=await call<Track>("get_radio_song_track",{stationId:station.id,metadata:item});
      if(playbackSession===session && radioOccurrence?.key===occurrence.key){
        currentRadioSong=track;
        const entry={id:occurrence.key,track,playedAt:occurrence.at,stationName:station.name};
        const existing=radioQueueHistory.findIndex(item=>item.id===entry.id);
        radioQueueHistory=existing>=0?radioQueueHistory.map(item=>item.id===entry.id?entry:item):[...radioQueueHistory,entry].slice(-100);
      }
      if(includeRadioHistory)await call("record_radio_history",{listeningSessionId:session,occurrenceKey:occurrence.key,listenedAt:occurrence.at,stationId:station.id,metadata:item});
    } catch(failure) { mirrorStatus=`Radio history: ${String(failure)}`; }
  }
  function radioNowPlaying(station:RadioStation,item:RadioTrackMetadata|null){
    if(playingTrack?.id!==`radio:${station.id}`)return;
    playingTrack={...playingTrack,artist:item?.artist??(station.provider==="radio_paradise"?"Radio Paradise":"Internet radio"),title:item?.title??station.name,album:item?.album??undefined,artworkUrl:item?.artworkUrl??(item?undefined:station.faviconUrl??undefined),provenance:{...(playingTrack.provenance as Record<string,unknown>),radioMetadata:item}};
    if(item){
      if(paused || !audio || audio.paused){
        void call<Track>("get_radio_song_track",{stationId:station.id,metadata:item}).then(track=>{const current=playingTrack?.provenance as {radioMetadata?:RadioTrackMetadata}|undefined;if(playingTrack?.id===`radio:${station.id}`&&current?.radioMetadata?.artist===item.artist&&current.radioMetadata.title===item.title)currentRadioSong=track;}).catch(()=>{});
      }else void recordCurrentRadioSong(station,item);
    }else currentRadioSong=null;
    if(playingTrack.artworkUrl)void cacheCover(playingTrack);void publishAutomationPlayback();
  }
  function playRadioStation(station:RadioStation){
    selectedRadioStationId=station.id;showAddRadio=false;
    const track:Track={id:`radio:${station.id}`,provider:"radio",providerId:station.id,title:station.name,artist:station.provider === "radio_paradise" ? "Radio Paradise" : "Internet radio",artworkUrl:station.faviconUrl??undefined,sourceUrl:station.homepageUrl??station.sourceUrl,favorite:false,provenance:{stationId:station.id,radioStreamUrl:station.streamUrl,sourceKind:"radio"}};
    void startPreview(track,false,false,[track],0);
  }
  $effect(()=>{
    const station=playingRadioStation, generation=preview?.sourceGeneration, session=playbackSession, nativeGeneration=nativePlaybackGeneration, trackId=station ? `radio:${station.id}` : "";
    if(!station||(view==="radio"&&selectedRadioStationId===station.id)||paused)return;
    let stopped=false;let timer:ReturnType<typeof setTimeout>;
    const current=()=>!stopped && playingRadioStation?.id===station.id && playingTrack?.id===trackId && playbackSession===session && preview?.sourceGeneration===generation && nativePlaybackGeneration===nativeGeneration;
    async function poll(){
      try {
        const result=await call<{nowPlaying:RadioTrackMetadata|null}>("poll_radio_station",{stationId:station!.id,recognize:false});
        if(current())radioNowPlaying(station!,result.nowPlaying);
      }catch(failure){
        if(current() && !String(failure).includes("Playback changed during station metadata lookup"))radioNowPlaying(station!,null);
      }finally{if(!stopped)timer=setTimeout(()=>void poll(),30000);}
    }
    void poll();return()=>{stopped=true;clearTimeout(timer);};
  });
  $effect(()=>{
    const station=playingRadioStation, generation=preview?.sourceGeneration, session=playbackSession;
    if(!station || nativeActive() || paused || generation===undefined)return;
    const stationId=station.id;
    let stopped=false, timer:ReturnType<typeof setTimeout>|undefined;
    let pair="", unchangedSince=Date.now();
    const current=()=>!stopped && !nativeActive() && !paused && playingRadioStation?.id===stationId && playbackSession===session && preview?.sourceGeneration===generation;
    async function check(){
      try {
        if(!current() || radioRecognizing || !audio || audio.paused || audio.readyState<3)return;
        const metadata=(playingTrack?.provenance as {radioMetadata?:RadioTrackMetadata|null})?.radioMetadata;
        const nextPair=metadata?`${metadata.artist}\n${metadata.title}`:"";
        if(nextPair!==pair){pair=nextPair;unchangedSince=Date.now();}
        const last=lastRadioAudioAttempt?.stationId===stationId && lastRadioAudioAttempt.generation===generation ? lastRadioAudioAttempt.at : 0;
        const due=!last || !metadata || /SongRec|audio-fingerprint/i.test(metadata.metadataConfidence) || Date.now()-unchangedSince>=90000;
        if(!due || Date.now()-last<45000)return;
        const enabled=await call<boolean>("get_radio_recognition");
        if(enabled && current() && !radioRecognizing)await identifyPlayingRadio(true);
      } catch { /* Automatic recognition failures leave playback uninterrupted. */ }
      finally {if(current())timer=setTimeout(()=>void check(),5000);}
    }
    timer=setTimeout(()=>void check(),3000);
    return()=>{stopped=true;if(timer)clearTimeout(timer);};
  });
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
  async function renameCollection() {
    if (!collectionId || !renamedCollection.trim()) return;
    await run(async () => {
      const updated = await call<Collection>("rename_collection", {
        collectionId, name: renamedCollection.trim(),
      });
      collections = collections.map((collection) => collection.id === updated.id ? updated : collection);
      renamedCollection = "";
      setNotice("Playlist renamed.");
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
    if (playlistReordering || localPageLoading || view !== "collections" || !collectionId) return;
    if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from === to || to >= playlistMoveLimit) return;
    const targetId = collectionId, targetContext = activeContext, movedTrack = selected;
    playlistReordering = true;
    try { await run(async () => {
      const editedId = await call<string>("move_playlist_entry", { collectionId: targetId, from, to });
      collections = await call<Collection[]>("list_collections");
      if (view !== "collections" || collectionId !== targetId || activeContext !== targetContext) return;
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
    }); } finally { playlistReordering = false; }
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
  async function importTracks() {
    selectionRequest++;
    clearTimeout(autoTimer);
    await run(async () => {
      tracks = await call<Track[]>("import_tracks", {
        text: importText,
        kind: importKind,
      });
      selected = null;
      closeImportDialog();
      importText = "";
      setNotice("Imported tracks are ready for review.");
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
      selected = null;
      closeImportDialog();
      importText = "";
      setNotice("Playlist metadata imported for review.");
    });
    input.value = "";
  }
  async function previewCandidate(c: Candidate) {
    if (!selected) return;
    setAutomationPlaybackOwner(null);
    clearTimeout(autoTimer);
    const request = nextPreviewRequest(), track = selected;
    const load = ++playbackLoadSequence; resolvingPlayback = true; bufferingAudio = false;
    playbackSession = crypto.randomUUID(); recordedSession = "";currentRadioSong=null;radioOccurrence=null;
    audio?.pause(); paused = true; position = 0; duration = 0;
    preview = null;
    pendingResumePosition = track.playbackRange?.startTime ?? 0;
    playingTrack = track;
    automaticPreviewRecovery = { request, track, tried: new Set([candidateKey(c)]), attempts: 1, exactSourceOnly: true, deadline: performance.now() + playbackStartupTimeoutMs() };
    try { await run(async () => {
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
    }); } finally { if (load === playbackLoadSequence) resolvingPlayback = false; }
  }
  async function closePreview(preserveQueuedOwner=false) {
    await cancelPendingWebviewHandoff();
    if(preserveQueuedOwner && queuedOwnerForResume())reportQueuedPlaybackPhase("paused");else setAutomationPlaybackOwner(null);
    nextPreviewRequest();
    if (nativeActive()) { playbackLoadSequence++; playbackRunGeneration++; queueRadio.cancel(); resolvingPlayback=false; bufferingAudio=false; await nativeCommand({action:"stop"}); return; }
    playbackLoadSequence++; resolvingPlayback = false; bufferingAudio = false;
    if(!preserveQueuedOwner)setAutomationPlaybackOwner(null);
    systemPausedTrack = false;
    audio?.pause();
    const candidate = preview?.sessionId;
    preview = null; paused = true;
    if (candidate) await run(() => call("stop_candidate_preview", { sessionId: candidate }));
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
  async function importBrowserJson() {
    await run(async () => {
      const parsed = JSON.parse(importText);
      const result = await call<BrowserImportResult>("import_browser_snapshot", { snapshot: parsed.snapshot ?? parsed });
      await finishBrowserImport(result); importText = "";
    });
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
      const context = buildWebviewDiagnosticsContext();
      if (context) lastWebviewDiagnosticContext = context;
    }
    await call("set_playback_diagnostics_enabled", {enabled});
    settings = {...settings, playbackDiagnosticsEnabled:enabled};
    webviewMediaEventRecorder.setEnabled(enabled || mediaEventDiagnosticsEnabled);
  }
  function buildWebviewDiagnosticsContext(): Record<string, unknown> | null {
    if (webviewDiagnosticsCleared || nativeActive()) return null;
    const streamKind = audioStreamHandle?.kind === "direct" ? "direct" : audioStreamHandle?.kind?.endsWith("hls") ? "hls" : "unavailable";
    const bounded = (value: number | null | undefined) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 86400 ? value : null;
    const ranges = (value: TimeRanges | undefined) => {
      const rows: number[][] = [];
      if (!value) return rows;
      try {
        for (let index=0; index<Math.min(value.length,8); index++) {
          const start=bounded(value.start(index)), end=bounded(value.end(index));
          if(start!==null&&end!==null&&end>=start)rows.push([start,end]);
        }
      } catch { /* Detached media can invalidate TimeRanges during export. */ }
      return rows;
    };
    const events = webviewMediaEventRecorder.snapshotHistory();
    const firstAt = events[0]?.atMs ?? 0;
    return {
      status: preview ? (playbackLoading ? "buffering" : paused ? "paused" : "playing") : lastWebviewDiagnosticFailure ? "failed" : "idle",
      currentTime:audio?bounded(audio.currentTime):null,
      duration:audio?bounded(audio.duration):null,
      requestedSeekPosition:bounded(activeWebviewSeek?.target ?? pendingWebviewResume?.startPosition ?? (pendingResumePosition>0?pendingResumePosition:null)),
      bufferedRanges:ranges(audio?.buffered),
      seekableRanges:ranges(audio?.seekable),
      startupFailure: lastWebviewDiagnosticFailure,
      startupOutcomes: webviewStartupOutcomes,
      startupFailures: webviewDiagnosticFailureHistory.map(({attempt,failure})=>({attempt,...failure})),
      sourceFormat: sanitizeWebviewSourceFormatDiagnostics({codec:preview?.audioCodec,mimeType:preview?.audioMimeType,formatId:preview?.audioFormatId,hasVideo:preview?.audioFormatHasVideo,expectedContentLength:preview?.audioExpectedContentLength},streamKind),
      mediaEvents: events.map(event => ({...event, atMs:event.atMs === null ? null : event.atMs - firstAt})),
    };
  }
  function webviewDiagnosticsForExport() {
    if (webviewDiagnosticsCleared) return null;
    if (settings.playbackDiagnosticsEnabled) {
      const context = buildWebviewDiagnosticsContext();
      if (context) lastWebviewDiagnosticContext = context;
    }
    return lastWebviewDiagnosticContext;
  }
  function clearWebviewDiagnostics() {
    webviewMediaEventRecorder.reset();
    lastWebviewDiagnosticContext = null;
    lastWebviewDiagnosticFailure = null;
    webviewDiagnosticFailureHistory = [];
    webviewStartupOutcomes = [];
    currentWebviewDiagnosticAttempt = 0;
    webviewDiagnosticsCleared = true;
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
  async function saveSettings() {
    await run(async () => {
      await call("save_settings", { settings });
      persistedMusicDirectory = settings.musicDirectory;
      setNotice("Settings saved.");
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
      setNotice(`Preferred download source saved: ${label}.`);
    });
  }
  function initializeAppLifecycle(): () => void {
    if(desktop())void call<{setupComplete:boolean;musicDirectory?:string|null}>("get_release_setup").then(status=>{const musicDirectory=status?.musicDirectory===undefined?settings.musicDirectory:status.musicDirectory;if(status&&!status.setupComplete&&!musicDirectory)showReleaseSetup=true;}).catch(()=>{});
    void ensureWebviewLoudnessSettings();

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
    const automationTimer = setInterval(() => { void queuedPlaybackOutbox.flush(); if (!nativeActive()) void publishAutomationPlayback(); }, 1000);
    const syncTimer = setInterval(() => {
      if (desktop()) void runPeriodicPlaylistSyncFallback(
        () => call<boolean>("playlist_watcher_running"),
        syncMirrors,
      );
    }, 60000);
    if (desktop()) { void runPeriodicPlaylistSyncFallback(() => call<boolean>("playlist_watcher_running"), syncMirrors); if (view !== "library") void navigate("library", false); }
    if (desktop()) void nativeBridge.initialize((snapshot) => {
      if (disposed) return;
      applyNativeSnapshot(snapshot);
      if (snapshot.engine !== "native" || snapshot.queue.length || snapshot.track || localStorage.getItem("burrata.native-playback-migrated.v1")) return;
      const legacy = loadPlaybackQueue()?.snapshot();
      if (!legacy?.tracks.length) { localStorage.setItem("burrata.native-playback-migrated.v1", "1"); return; }
      let savedPosition = 0;
      try {
        const saved = JSON.parse(localStorage.getItem(PLAYBACK_POSITION_KEY) ?? "null") as NativePositionBookmark | null;
        const currentTrack = legacy.tracks[legacy.currentIndex];
        const currentEntryId = legacy.entryIds?.[legacy.currentIndex];
        if (validNativePositionBookmark(saved) && Date.now() - saved.savedAt <= 30 * 24 * 60 * 60 * 1000 &&
            nativeBookmarkMatchesOccurrence(saved, currentTrack?.id, currentEntryId)) savedPosition = saved.position;
      } catch { /* Ignore corrupt prior position. */ }
      localStorage.setItem("burrata.native-playback-migrated.v1", "1");
      const current = legacy.tracks[legacy.currentIndex];
      void nativeCommand({ action: "restore", tracks: legacy.tracks, index: legacy.currentIndex, position: current?.playbackRange ? chapterRelativePosition(current,savedPosition) : savedPosition, volume, repeat: legacy.repeat, shuffle: shuffleEnabled, ...(legacy.queueLoudnessContexts ? { queueLoudnessContexts: legacy.queueLoudnessContexts } : {}) });
    }, event => { if(!disposed)applyNativePosition(event); }).then(()=>{playbackEngineReady=true;playbackControlsOwned=true;if(disposed)nativeBridge.dispose();}).catch(() => { playbackEngineReady=true;playbackControlsOwned=true; /* Older desktop builds keep using webview playback. */ });
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
        const waveformRoute = (preview as WebviewResolvedPreview | null)?.normalizationRoute ?? null;
        const waveformReasonCode = !waveformEnabled ? "disabled"
          : !preview && !nativeActive() ? "no-playback-source"
          : !nativeActive() && waveformRoute !== "same-origin-relay" ? "unsupported-audio-route"
          : !waveform ? "no-waveform-snapshot"
          : !waveformGenerationMatches ? "generation-mismatch"
          : !waveformTrackMatches ? "track-mismatch"
          : !waveformDurationMatches ? "duration-mismatch"
          : displayedWaveform !== waveform ? "resource-binding-mismatch"
          : populatedWaveformBins === 0 ? "no-sampled-bins"
          : "waveform-ready";
        const mediaGraph = inspectWebAudioGainGraph(audio);
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
          routeActive: webviewNormalizationRouteStatus === "Same-origin Web Audio route active.",
          analyserAttached: mediaGraph.attached,
          mediaGraphUserVolume: mediaGraph.userVolumeLinear,
          mediaVolume: audio?.volume ?? null,
          mediaMuted: audio?.muted ?? null,
        };
        const artworkUrl = playingTrack?.artworkUrl ?? "";
        void inspectArtworkImages(document, artworkUrl, Boolean(failedArtwork[artworkUrl])).then(artworkDiagnostics =>
          call("automation_layout_complete", {requestId:request.requestId,layout:{...layout,artworkDiagnostics,waveformDiagnostics,liveBpmDiagnostics:liveBpmObservation}})).catch(()=>{});
      });
      const mediaInspectionListenerReady = register<{requestId:string;trackId:string|null;sessionId:string|null;sourceGeneration:number|null;seekSerial:number;currentIndex:number;queueTotal:number;entryId:string|null;queueTrackId:string|null}>("automation-inspect-webview-media", request => {
        if(disposed||!request||typeof request.requestId!=="string")return;
        const identity = () => ({trackId:playingTrack?.id??null,sessionId:playbackSession??null,sourceGeneration:preview?.sourceGeneration??null,seekSerial,currentIndex:queueIndex,entryId:automationQueueProjection.entryIds[queueIndex]??null,queueTrackId:queueTracks[queueIndex]?.id??null});
        const before=identity(), element=audio, sourceBefore=element?(element.currentSrc||element.src):"";
        const finite=(value:number)=>Number.isFinite(value)&&value>=0?value:null;
        const ranges=(value:TimeRanges|undefined)=>{const rows:number[][]=[];if(!value)return rows;try{for(let index=0;index<Math.min(value.length,8);index++){const start=value.start(index),end=value.end(index);if(Number.isFinite(start)&&Number.isFinite(end)&&start>=0&&end>=start&&end<=1e9)rows.push([start,end]);}}catch{}return rows;};
        const graph = inspectWebAudioGainGraph(element);
        const seek = activeWebviewSeek;
        const seekCompletion = seek && element && seek.owner.node === element ? {
          target: finite(seek.target), elapsedMs: finite(Math.max(0, performance.now()-seek.startedAtMs)),
          playbackRate: finite(element.playbackRate), mediaPaused: element.paused, appPaused: paused,
          ownerCurrent: isCurrentWebviewSeek(seek), hasError: Boolean(element.error),
          withinWindow: hasWebviewSeekLanded({target:seek.target,currentTime:element.currentTime,duration:element.duration,
            elapsedMs:performance.now()-seek.startedAtMs,playing:!paused&&!element.paused,playbackRate:element.playbackRate}),
        } : null;
        const sample:any={engine:"webview",applicable:true,...before,queueTotal:queueTracks.length,previewKind:preview?.kind??null,seekCompletion,media:{
          present:Boolean(element),currentTime:element?finite(element.currentTime):null,duration:element?finite(element.duration):null,
          paused:element?element.paused:null,seeking:element?element.seeking:null,buffered:ranges(element?.buffered),seekable:ranges(element?.seekable),readyState:element?element.readyState:null,networkState:element?element.networkState:null,
          volume:element?finite(element.volume):null,muted:element?.muted??null,
          webAudioGainGraph:graph,
          errorCode:element?.error?.code??null,sourcePresent:Boolean(element&&(element.currentSrc||element.src)),sourceStable:Boolean(element&&(element.currentSrc||element.src)===sourceBefore)
        }};
        if (settings.playbackDiagnosticsEnabled || mediaEventDiagnosticsEnabled) {
          sample.startupDiagnostics = webviewDiagnosticsForExport();
          const eventDiagnostics=webviewMediaEventRecorder.snapshot(before);
          if(eventDiagnostics) {
            const streamKind=audioStreamHandle?.kind==="direct"?"direct":audioStreamHandle?.kind?.endsWith("hls")?"hls":"unavailable";
            sample.diagnostics={...eventDiagnostics,sourceFormat:sanitizeWebviewSourceFormatDiagnostics({codec:preview?.audioCodec,mimeType:preview?.audioMimeType,formatId:preview?.audioFormatId,hasVideo:preview?.audioFormatHasVideo,expectedContentLength:preview?.audioExpectedContentLength},streamKind)};
          }
        }
        const after=identity();
        const same=before.trackId===after.trackId&&before.sessionId===after.sessionId&&before.sourceGeneration===after.sourceGeneration&&before.seekSerial===after.seekSerial&&before.currentIndex===after.currentIndex&&before.entryId===after.entryId&&before.queueTrackId===after.queueTrackId;
        const matchesRequest=before.trackId===request.trackId&&before.sessionId===request.sessionId&&before.sourceGeneration===request.sourceGeneration&&before.seekSerial===request.seekSerial&&before.currentIndex===request.currentIndex&&before.entryId===request.entryId&&before.queueTrackId===request.queueTrackId&&queueTracks.length===request.queueTotal;
        if(!same||!matchesRequest||element!==audio){sample.trackId=after.trackId;sample.sessionId=after.sessionId;sample.sourceGeneration=after.sourceGeneration;sample.seekSerial=after.seekSerial;sample.currentIndex=after.currentIndex;sample.entryId=after.entryId;sample.queueTrackId=after.queueTrackId;}
        void call("automation_layout_complete",{requestId:request.requestId,layout:sample}).catch(()=>{});
      });
      const stereoDiagnosticListenerReady = register<{requestId:string;trackId:string;sessionId:string;sourceGeneration:number;seekSerial:number;currentIndex:number;queueTotal:number;entryId:string;queueTrackId:string;durationSeconds:number}>("automation-capture-webview-stereo-diagnostic", request => {
        if(disposed||!request||typeof request.requestId!=="string")return;
        const identity=()=>({trackId:playingTrack?.id??null,sessionId:playbackSession??null,sourceGeneration:preview?.sourceGeneration??null,seekSerial,currentIndex:queueIndex,entryId:automationQueueProjection.entryIds[queueIndex]??null,queueTrackId:queueTracks[queueIndex]?.id??null});
        const before=identity();
        const provenance=playingTrack?.provenance&&typeof playingTrack.provenance==="object"?playingTrack.provenance as Record<string,unknown>:{};
        const graph=inspectWebAudioGainGraph(audio);
        const fixture=playingTrack?.provider==="local"&&provenance.fixture==="burrata-local-qualification-v1";
        const matches=before.trackId===request.trackId&&before.sessionId===request.sessionId&&before.sourceGeneration===request.sourceGeneration&&before.seekSerial===request.seekSerial&&before.currentIndex===request.currentIndex&&before.entryId===request.entryId&&before.queueTrackId===request.queueTrackId&&queueTracks.length===request.queueTotal;
        const audioAtStart=audio;
        const finish=async(capture:Record<string,unknown>)=>{
          const after=identity();
          const same=before.trackId===after.trackId&&before.sessionId===after.sessionId&&before.sourceGeneration===after.sourceGeneration&&before.seekSerial===after.seekSerial&&before.currentIndex===after.currentIndex&&before.entryId===after.entryId&&before.queueTrackId===after.queueTrackId&&audioAtStart===audio;
          const safeCapture=same?capture:{status:"cancelled",sampleRate:Number(capture.sampleRate)||0,channels:2,frames:0,sourceGeneration:request.sourceGeneration};
          const completionIdentity={requestId:request.requestId,trackId:before.trackId??request.trackId,sessionId:before.sessionId??request.sessionId,sourceGeneration:before.sourceGeneration??request.sourceGeneration,seekSerial:before.seekSerial,currentIndex:before.currentIndex>=0?before.currentIndex:request.currentIndex,queueTotal:queueTracks.length||request.queueTotal,entryId:before.entryId??request.entryId,queueTrackId:before.queueTrackId??request.queueTrackId};
          await call("automation_stereo_diagnostic_complete",stereoDiagnosticCompletionArguments(completionIdentity,safeCapture)).catch(()=>{});
        };
        if(!fixture||!audioAtStart||!matches||paused||audioAtStart.paused||!graph.attached||graph.userVolumeLinear==null||graph.userVolumeLinear<=0||request.durationSeconds!==2){void finish({status:"unavailable",sampleRate:0,channels:2,frames:0,sourceGeneration:request.sourceGeneration});return;}
        const mediaTimeAtStart=Number.isFinite(audioAtStart.currentTime)?audioAtStart.currentTime:0;
        void (async()=>{
          let session:Awaited<ReturnType<typeof startWebAudioStereoDiagnosticTap>>=null;
          try{
            session=await startWebAudioStereoDiagnosticTap(audioAtStart,2);
            if(!session){await finish({status:"unavailable",sampleRate:0,channels:2,frames:0,sourceGeneration:request.sourceGeneration});return;}
            const result=await session.capture;
            const currentGraph=inspectWebAudioGainGraph(audioAtStart);
            const mediaTimeAtEnd=Number.isFinite(audioAtStart.currentTime)?audioAtStart.currentTime:mediaTimeAtStart;
            const payload:Record<string,unknown>={status:result.status,sampleRate:result.sampleRate,channels:2,frames:result.frames,sourceGeneration:request.sourceGeneration};
            if(result.status==="complete"&&result.samples&&result.firstFrame!=null&&result.firstContextTime!=null){Object.assign(payload,{firstFrame:result.firstFrame,firstContextTime:result.firstContextTime,mediaTimeAtStart,mediaTimeAtEnd,gainLinear:currentGraph.gainLinear,userVolumeLinear:currentGraph.userVolumeLinear,samplesFloat32LeBase64:encodeStereoDiagnosticSamples(result.samples)});}
            await finish(payload);
          }catch{
            session?.stop();
            await finish({status:"unavailable",sampleRate:0,channels:2,frames:0,sourceGeneration:request.sourceGeneration});
          }
        })();
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
        const current=()=>{const snapshot=nativeSnapshot as (NativePlaybackSnapshot&{entryId?:string})|null;const valid=!disposed&&!controller.signal.aborted&&showQueue&&nativeActive()&&snapshot?.status==="paused"&&snapshot.generation===request.context.generation&&snapshot.entryId===request.context.entryId&&snapshot.trackId===request.context.trackId&&(snapshot.queueRevision??-1)>=request.context.queueRevision&&(snapshot.queueRevision??Infinity)<=moves.maximumObservedRevision&&queueTracks.length===request.context.rowCount;if(!valid)controller.abort();return valid;};
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
      void Promise.all([layoutListenerReady, mediaInspectionListenerReady, stereoDiagnosticListenerReady, queueUiListenerReady, queueUiCancelListenerReady, searchListenerReady, playerPanelListenerReady, navigationListenerReady, identifyListenerReady, identifyCancelListenerReady]).then(ready => { if (!disposed && ready.every(Boolean)) void call("set_automation_frontend_ready", {ready:true}).catch(()=>{}); });
      register<{requestId: string}>("automation-playback-cancelled", (request) => { if (activeAutomationPlayback === request.requestId) { nextPreviewRequest(); if(nativeActive())void nativeCommand({action:"stop"});else audio?.pause(); paused = true; setAutomationPlaybackOwner(null); if (preview?.sessionId) void call("stop_candidate_preview", {sessionId: preview.sessionId}); } });
      register<{stations:RadioStation[]}>("radio-stations-changed",({stations})=>{
        radioStations=stations;
        if(!stations.some(station=>station.id===selectedRadioStationId))selectedRadioStationId=stations[0]?.id??"";
      });
      register<unknown>("automation-library-changed", () => { void refreshRemotePlaylistBindings(); void call<typeof browserImports>("list_browser_imports").then(value => browserImports = value).catch(() => {}); void refreshFavorites().catch(()=>{}); void runPeriodicPlaylistSyncFallback(() => call<boolean>("playlist_watcher_running"), syncMirrors); requestLocalPrefixRefresh(undefined, true); });
      void call<{permissions: {control: boolean; manage: boolean}}>("get_automation_status").then((status) => { if (status?.permissions) automationPermissions = status.permissions; }).catch(() => {});
      if (!nativeActive()) void publishAutomationPlayback();
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
        if(nativeActive())queueTracks=queueTracks.map(track=>byId.get(track.id) ?? track);else{playbackQueue.updateTracks(queueTracks.map(track=>byId.get(track.id) ?? track));refreshQueue();}
        requestLocalPrefixRefresh(undefined,true);
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
      void call<{ settings: { slskd: { enabled: boolean }; lyricsTranslation?: { targetLanguage?: string; enabled?: boolean; provider?: "openaiCompatible" | "googleFree" | "auto" } }; lyricsTranslationCredential?: { present?: boolean } }>("get_connections")
        .then((status) => {slskdEnabled = status.settings.slskd.enabled;lyricsTranslationLanguage=status.settings.lyricsTranslation?.targetLanguage ?? "en";lyricsTranslationEnabled=status.settings.lyricsTranslation?.enabled===true;lyricsTranslationCredentialPresent=status.settings.lyricsTranslation?.provider === "googleFree" || status.settings.lyricsTranslation?.provider === "auto" || status.lyricsTranslationCredential?.present===true;sourceCapabilities={soulseek:slskdEnabled,discogs:Boolean((status.settings as {discogsEnabled?:boolean}).discogsEnabled)};})
        .catch(() => {});
      void call<Settings>("get_settings").then((value) => {
        persistedMusicDirectory = value.musicDirectory;
        webviewMediaEventRecorder.setEnabled(value.playbackDiagnosticsEnabled === true || mediaEventDiagnosticsEnabled);
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
      nextCandidatePrefetch.cancel();
      disposed = true;
      queueUiController?.abort();
      artworkDisplayActive = false; artworkDisplayCache.clear(); artworkDisplayUrls = {};
      if (desktop()) void call("set_automation_frontend_ready", {ready:false}).catch(()=>{});
      nativeBridge.dispose();
      stopBatchMatching();
      unlisten?.();
      unlistenWarning?.();
      extraListeners.forEach((fn) => fn()); clearInterval(browserTimer); clearInterval(syncTimer); clearInterval(automationTimer);
      if (preview?.sessionId) void call("stop_candidate_preview", { sessionId: preview.sessionId }).catch(() => {});
      clearTimeout(autoTimer);
      nextPreviewRequest();
      audio?.pause();
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
<div class:light={settings.theme === "light" || settings.theme === "system" && systemLight} class:has-player-panel={showPlayingInfo || showQueue} class="app-shell" inert={engineSwitchBusy || desktop() && !backupHydrationReady} aria-busy={desktop() && !backupHydrationReady} style={`--player-height: ${playerHeight}px; --library-sidebar-width:${screenWidth<=900?64:Math.min(sidebarWidth,Math.max(160,screenWidth-((showPlayingInfo||showQueue)?280:0)-320))}px; --info-panel-width:${Math.min(infoPanelWidth,Math.max(280,screenWidth-(screenWidth<=900?64:sidebarWidth)-280))}px`}>
  <LibrarySidebar inert={showFullNowPlaying} {call} showAccountPlaylists={sidebarLayout.showAccountPlaylists} onRemotePlaylist={(source,url) => void openRemotePlaylist(source,url)} compact={screenWidth<=900} favoritesPlaylistId={playlistPreferences.favoritesPlaylistId} playlistNames={sidebarLayout.names??{}} onRadioContext={(station,event)=>radioActions={station,anchor:{x:event.clientX,y:event.clientY}}} {radioStations} activeRadioStationId={selectedRadioStationId} onRadioStation={(station)=>{showAddRadio=false;selectedRadioStationId=station.id;void navigate("radio");if(playingRadioStation?.id!==station.id)playRadioStation(station);}} onAddRadio={()=>{showAddRadio=true;void navigate("radio");}} width={sidebarWidth} onWidthChange={(width)=>saveLayoutWidth("sidebar",width)} {view} collections={namedCollections} browserImports={sidebarImports} {localPlaylistIds} activeSource="all" activeCollectionId={view === "collections" || view === "charts" ? collectionId : ""}
    onDirectoryBrowser={() => { directoryBrowserVisited = true; if (view !== "directory-browser") void navigate("directory-browser"); }}
    selectedFolder={view === "directory-browser" ? directoryLocation : null} rootsRevision={directoryRootsRevision} onSelectFolder={browseFolder} onRootsChanged={() => directoryRootsRevision++}
    onNavigate={(next) => { if (next === "library") { if (view !== "library" || librarySource !== "all") void chooseLibrarySource("all"); } else if (view !== next) void navigate(next as View); }}
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
      <DirectoryBrowser onTrackAlbum={(track) => void openTrackAlbum(track)} bind:this={directoryBrowser} {call} location={directoryLocation} rootRegistryRevision={directoryRootsRevision} onLocationChange={(location, context) => { if (context?.reason === "user") { rememberRoute({directorySnapshot:context.previousSnapshot,directoryLocation:context.previousSnapshot.location}); } directoryLocation = location; }} onRootsChanged={() => directoryRootsRevision++} onPlayFile={(_file, track) => void startPreview(track, false, false, [track], 0)}
        {playingTrack} bufferingTrack={playbackLoading ? playingTrack : null} {paused}
        onDownload={openDownloadChooser} onFavorite={favorite}
        onQueue={(track) => queueArtistTracks([track], "append")} onNext={(track) => queueArtistTracks([track], "next")}
        onPlaylist={addArtistTrack} onArtist={discoverArtist} onInfo={(track) => openPlayerPanel("info", track)}
        onContext={(track, event) => { actionTrack = track; actionAnchor = { x: event.clientX, y: event.clientY }; }}
        onAddSelectedToPlaylist={addTracksToPlaylist} />
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
        <section class="import-panel"><ImportHub onChoose={(choice) => { importChoice = choice; if (choice === "playlist-file") importKind = "m3u"; else if (choice === "paste-playlist") importKind = "tracklist"; }} /></section>
      {:else if importChoice === "restore-backup"}
        <section class="import-panel"><button type="button" onclick={returnToImportChoices}>← Import choices</button>{#if desktop()}{#key droppedBackupPreview?.previewId ?? "backup-picker"}<BackupRestore inspectBackup={inspectBackupForUi} createBackup={createBackupForUi} restoreBackup={restoreBackupForUi} chooseMediaRoot={chooseBackupMediaRoot} estimateBackup={estimateBackupForUi} cancelJob={cancelBackupJob} loadActiveBackupJob={loadActiveBackupJob} subscribeBackupProgress={subscribeBackupProgress} loadSchedule={loadBackupSchedule} saveSchedule={saveBackupSchedule} chooseScheduleDestination={chooseBackupScheduleDestination} exportSettings={exportBackupSettings} inspectSettings={inspectBackupSettings} applySettings={applyBackupSettings} previewRestore={previewBackupRestore} allowReplace={true} initialPreview={droppedBackupPreview} />{/key}{:else}<p>Backups require the Burrata desktop app.</p>{/if}</section>
      {:else if importChoice !== "import-url"}
        <section class="import-panel"><button type="button" onclick={returnToImportChoices}>← Import choices</button><ImportSources choice={importChoice} {call} {collections} currentCollectionId={importTargetCollectionId} desktop={desktop()} onApply={applySourceImport} onPortableImported={async (created) => { collections = await call<Collection[]>("list_collections"); await openPlaylist(created.id); importChoice = null; }} onBackupSelected={() => { if (droppedBackupPreview) return; importChoice = "restore-backup"; setNotice("Select the backup in Restore Backup to review its contents."); }} onBackupPreview={openDroppedBackupPreview} onClose={returnToImportChoices} /></section>
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
    {#if autotagReviewId && currentTagJob && currentTagAsset}<section class="autotag-page"><button onclick={()=>{autotagReviewId=null;autotagReviewAsset=null;}}>← Back</button><AutotagReview job={currentTagJob} asset={currentTagAsset} onReview={(assetId,providerId)=>call<AutotagJob>("review_autotag_candidate",{assetId,providerId})} onApply={async(assetId,fieldKeys)=>{await call("apply_autotag_review",{assetId,fieldKeys});autotagReviewAsset=await call<LocalAsset|null>("get_asset_by_id",{assetId});await loadAutotagState();}} onApplyRecognition={async(assetId,fieldKeys)=>{await call("apply_recognition_review",{assetId,fieldKeys});autotagReviewAsset=await call<LocalAsset|null>("get_asset_by_id",{assetId});await loadAutotagState();}} onRepairSource={desktop()?recoverDownloadSourceTags:undefined} onRetry={async(assetId)=>{await call("retry_autotag",{assetId});await loadAutotagState();}} /></section>
    {:else if view === "artist"}
      <section class="artist-page-host">
        {#if spotifyArtistId || spotifyAlbumId}<SpotifyArtist onTrackAlbum={(track) => void openTrackAlbum(track)} bind:this={spotifyArtistComponent} navigationSnapshotKey={spotifyNavigationSnapshotKey} artistId={spotifyArtistId || spotifyAlbumId} initialAlbumId={spotifyAlbumId || null} searchArtistFallback={spotifyArtistSearchFallback} call={spotifyArtistCall} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {paused} onPlay={playArtistTrack} onPlaylist={addArtistTrack} onAddSelectedToPlaylist={addTracksToPlaylist} onAddAlbumToPlaylist={addAlbumToPlaylist} onBrowseArtistProvider={browseArtistProvider} onFavorite={(track)=>void favorite(track)} onDownload={(track)=>void openDownloadChooser(track)} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onArtist={(track)=>void discoverArtist(track)} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};void matchArtistPlaybackSources([track]).then(([matched])=>{if(actionTrack===track&&matched)actionTrack=matched;}).catch(failure=>{error=String(failure);});}} onAlbumOpening={()=>rememberRoute()} initialScroll={artistScroll} onScroll={(value)=>artistScroll=value}/>{:else}
        <ArtistPage onTrackAlbum={(track) => void openTrackAlbum(track)} showSearchResultsButton={false} bind:this={nativeArtistComponent} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {paused}
          activeTrack={activeArtistTrack}
          artistName={activeArtistName}
          initialArtistRef={activeArtistRef}
          initialSection={artistSection} initialScroll={artistScroll} onScroll={(position) => (artistScroll = position)} onSectionChange={(section) => { if (artistSection !== section) { rememberRoute(); if (!restoringRoute) artistScroll = 0; } artistSection = section; }}
          onArtistSelected={(artist, name) => { if (activeArtistRef?.id === artist?.id && activeArtistRef?.provider === artist?.provider) return; rememberRoute(); if (!restoringRoute) artistScroll = 0; activeArtistRef = artist; if (name) activeArtistName = name; }}
          localTracks={[]}
          onLoadLocalAssetsPage={getArtistLocalAssetsPage}
          {searchArtists}
          {getArtistProfile}
          {getArtistReleases}
          {getReleaseTracks}
          {getArtistTracks}
          {getArtistCollaborations}
          {resolveRecordingMetadata}
          onApplyMetadata={applyArtistMetadata}
          onPlayTrack={playArtistTrack}
          onAddAlbumToPlaylist={addAlbumToPlaylist}
          onBrowseArtistProvider={browseArtistProvider}
          onDownloadAlbum={(items)=>void queueSelected(items)}
          onQueueTracks={queueArtistTracks}
          onTrackContext={(track, event) => { actionAnchor = { x: event.clientX, y: event.clientY }; actionTrack = track; void matchArtistPlaybackSources([track]).then(([matched]) => { if (actionTrack === track && matched) actionTrack = matched; }).catch(failure => { error = String(failure); }); }}
          onDownloadTrack={(track) => openDownloadChooser(track)}
          onFavoriteTrack={(track)=>void favorite(track)} onAddTrackToPlaylist={addArtistTrack} onAddSelectedToPlaylist={addTracksToPlaylist}
          onArtistNavigate={discoverArtistRef}
        />{/if}
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
          <LoudnessSettings settings={webviewLoudnessSettings} onSettingsSaved={refreshCurrentWebviewNormalization} playbackStatus={nativeActive() ? (nativeSnapshot as (NativePlaybackSnapshot & {loudnessNormalization?: LoudnessPlaybackStatus}) | null)?.loudnessNormalization ?? null : webviewLoudnessDecision} webviewRouteStatus={webviewNormalizationRouteStatus} engine={nativeActive() ? "native" : "webview"} analysisJob={loudnessAnalysisJob} pendingAnalysisCount={pendingLoudnessAssetIds.length} analysisMessage={loudnessAnalysisMessage} onCancelAnalysis={cancelLoudnessAnalysis} onRetryAnalysis={retryQueuedLoudnessAnalysis} />
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
          <small>{settings.matchingLevel === "flexible" ? "Other versions, covers or remixes may be used." : "Keep the requested recording."}</small>
        </section>
        <div class="settings-source-grid">
          {#if settingsCategory === "playback"}<section class="settings-source-card" aria-labelledby="streaming-settings-title">
            <h3 id="streaming-settings-title">Audio sources</h3><p>Local files play first. Choose where to look for other songs.</p>
            <label>Primary source<select aria-label="Preferred streaming source" bind:value={settings.preferredStreamingSource} onchange={()=>void saveSettingsFields(["preferredStreamingSource"],"Streaming source saved.")}>
            <option value="youtube">YouTube</option><option value="soundcloud">SoundCloud</option><option value="monochrome">Monochrome</option>
          </select><small>Monochrome requires a strong recording match.</small></label>
            <FallbackSettings label="Streaming" enabled={settings.streamingFallbackEnabled !== false} providers={settings.streamingFallbackSources ?? ["youtube", "soundcloud", "bandcamp"]} primary={settings.preferredStreamingSource ?? "youtube"} sources={[{id:"monochrome",name:"Monochrome"},{id:"youtube",name:"YouTube"},{id:"soundcloud",name:"SoundCloud"},{id:"bandcamp",name:"Bandcamp"}]} onchange={(enabled,sources)=>void saveFallbackSettings("streaming",enabled,sources)} />
            <label class="settings-option"><input type="checkbox" aria-label="Prepare saved radio songs" checked={settings.backgroundSourcePreparation !== false} onchange={(event) => { settings.backgroundSourcePreparation = event.currentTarget.checked; void saveSettingsFields(["backgroundSourcePreparation"], "Background preparation saved."); }} /><span><strong>Prepare saved radio songs</strong><small>Find and cache playable sources for saved radio songs in the background.</small></span></label>
            <label class="settings-option"><input type="checkbox" aria-label="Override playlist track URLs" bind:checked={settings.overridePlaylistTrackUrls} onchange={()=>void saveSettingsFields(["overridePlaylistTrackUrls"],"Playlist source preference saved.")} /><span><strong>Find alternative sources for playlist links</strong><small>Off keeps the audio source linked in each playlist.</small></span></label>
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
        <RestoreWorkReview onReviewed={updateStreamMediaAnalysisRestorePolicy} />
        <section class="settings-basic-card" aria-labelledby="library-settings-title">
          <h2 id="library-settings-title">Music folder</h2><p>Completed downloads are saved here after validation and tagging.</p>
          <label>Download destination<input bind:value={settings.musicDirectory} placeholder="/home/you/Music/DJ" /></label>
          <button disabled={busy>0} onclick={()=>void chooseMusicDirectory()}>Choose music folder…</button>
          {#if settings.musicDirectory !== persistedMusicDirectory}<button onclick={()=>void saveSettingsFields(["musicDirectory"],"Music folder saved.")}>Save music folder</button>{/if}
        </section>
        <LocalPlaylistFiles mode="settings" {settings} onChange={(value) => (settings = value)} onSave={() => void saveSettingsFields(["playlistDirectories"],"Playlist folders saved.")} />
        {#if desktop()}<BackupRestore inspectBackup={inspectBackupForUi} createBackup={createBackupForUi} restoreBackup={restoreBackupForUi} chooseMediaRoot={chooseBackupMediaRoot} estimateBackup={estimateBackupForUi} cancelJob={cancelBackupJob} loadActiveBackupJob={loadActiveBackupJob} subscribeBackupProgress={subscribeBackupProgress} loadSchedule={loadBackupSchedule} saveSchedule={saveBackupSchedule} chooseScheduleDestination={chooseBackupScheduleDestination} exportSettings={exportBackupSettings} inspectSettings={inspectBackupSettings} applySettings={applyBackupSettings} previewRestore={previewBackupRestore} allowReplace={true} />{/if}
        {/if}
        {#if settingsCategory === "playback"}<label class="settings-option"><input type="checkbox" bind:checked={includeRadioHistory} onchange={()=>void run(()=>call("set_radio_history_setting",{enabled:includeRadioHistory}))}/><span><strong>Include radio songs in playback history</strong><small>Save identified songs you hear with their station and listening time.</small></span></label>{/if}
        {#if settingsCategory === "playback"}
            {@const mediaResource = currentStreamMediaResource()}
            {@const mediaJob = streamMediaAnalysisJob?.resourceKey === mediaResource?.key ? streamMediaAnalysisJob : null}
        <details class="settings-advanced"><summary><strong>Audio analysis</strong><span>BPM, musical key and waveforms</span></summary><div class="settings-details-body">
          <div class="settings-option-help"><label class="settings-option"><input type="checkbox" aria-label="Enable live BPM and key" checked={settings.liveBpmEnabled === true} onchange={(event) => { settings.liveBpmEnabled = event.currentTarget.checked; void saveSettingsFields(["liveBpmEnabled"], "Live playback estimate preference saved."); }} /><span><strong>Show live BPM and key</strong><small>Estimate tempo and key while listening.</small></span></label><InfoPopover label="About live BPM and key">Uses recent audio from Native or supported Web Audio playback. Estimates may change as the song plays and do not update saved metadata. Enabling analysis uses additional processing power.</InfoPopover></div>
            <StreamMediaAnalysisSettings
              enabled={streamMediaAnalysisEnabled}
              status={streamMediaAnalysisStatus}
              onEnabledChange={(enabled)=>streamMediaAnalysisEnabled=enabled}
              mediaEligible={Boolean(streamMediaAnalysisEnabled && desktop() && mediaResource)}
              mediaState={streamMediaAnalysisStartingKey === mediaResource?.key ? "starting" : mediaJob ? mediaJob.state === "queued" || mediaJob.state === "running" || mediaJob.state === "cancelling" ? "running" : "idle" : "idle"}
              mediaStatus={streamMediaAnalysisStartingKey === mediaResource?.key || mediaJob ? streamMediaAnalysisStatus : ""}
              onMediaStart={() => { const resource = currentStreamMediaResource(); if (resource) { attemptedStreamMediaAnalysis.add(resource.key); void startStreamMediaAnalysis(resource); } }}
              onMediaCancel={() => void cancelStreamMediaAnalysis()}
              observedEligible={Boolean(!streamMediaAnalysisEnabled && playingTrack && !nativeActive() && !playingRadioStation && currentWebviewStreamCaptureIdentity())}
              observedState={webviewAnalysisArmTrackId === playingTrack?.id ? "preparing" : playingTrack && webviewObservedCapture?.trackId === playingTrack.id ? (webviewObservedCapture.finishing ? "finalizing" : "capturing") : "idle"}
              observedStatus={webviewObservedAnalysisStatus ?? ""}
              observedResult={webviewObservedAnalysis ? `Whole-track analysis${webviewObservedAnalysis.bpm ? ` · ${webviewObservedAnalysis.bpm.bpm.toFixed(1)} BPM` : ""}${webviewObservedAnalysis.key ? ` · ${webviewObservedAnalysis.key.key}` : ""}` : ""}
              cachedResult={cachedStreamAnalysis && cachedStreamAnalysisOwnerKey === `${playingTrack?.id ?? ""}:${nativeSnapshot?.generation ?? ""}` ? `Whole-track analysis${cachedStreamAnalysis.bpm ? ` · ${cachedStreamAnalysis.bpm.bpm.toFixed(1)} BPM${cachedStreamAnalysis.bpm.relatedTempoAlternativeBpm ? ` (tempo alternative ${cachedStreamAnalysis.bpm.relatedTempoAlternativeBpm.toFixed(1)})` : ""}` : ""}${cachedStreamAnalysis.key ? ` · ${cachedStreamAnalysis.key.key}` : ""}` : ""}
              onObservedStart={() => void analyzeCurrentWebviewStream()}
              onObservedCancel={() => cancelWebviewObservedCapture("Whole-track WebView analysis was cancelled.")}
              localEligible={Boolean(waveformEnabled && waveformLocalAssetId)}
              localBusy={Boolean(localWaveformAnalysisJobId)}
              localStatus={localWaveformAnalysisStatus}
              onLocalStart={() => void analyzeCurrentLocalWaveform()}
              onLocalCancel={() => void cancelLocalWaveformAnalysis()}
            />

        </div></details>
        <details class="settings-advanced"><summary><strong>Advanced playback</strong><span>Audio engine and timing</span></summary><div class="settings-details-body">
          <PlaybackEngineSettings onApplyRestart={applyPlaybackEngineAndRestart} />
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
          <PlaybackDiagnosticsSettings {call} enabled={settings.playbackDiagnosticsEnabled === true} onEnabledChange={savePlaybackDiagnosticsEnabled} getWebviewDiagnostics={webviewDiagnosticsForExport} onClearWebviewDiagnostics={clearWebviewDiagnostics} />
          <ToolSetup {settings} {dependencies} onChange={(value)=>(settings=value)} onCheck={()=>void run(async()=>(dependencies=await call<Dependency[]>("check_dependencies")))} onSave={()=>void saveSettingsFields(["ytDlpPath","ffmpegPath","ffprobePath","jsRuntimePath"],"Tool paths saved.")} checking={busy>0} />
        </div>
        {/if}{#if settingsCategory === "integrations"}<details class="settings-advanced" open><summary><strong>Assistant permissions</strong><span>Control access to your player and library</span></summary>
          <section class="automation-settings"><p>Allow a connected assistant to use the player and library.</p><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.control} onchange={()=>void saveAutomationPermissions()} /><span>Allow playback and playlist changes</span></label><label class="settings-option"><input type="checkbox" bind:checked={automationPermissions.manage} onchange={()=>void saveAutomationPermissions()} /><span>Allow downloads and audio analysis</span></label></section>
        </details>{/if}
      </section>
    {:else if view === "search"}
      {#if searchRelease}<ReleaseView bind:this={searchReleaseComponent} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {paused} release={searchRelease} tracks={searchReleaseTracks} loading={searchReleaseBusy} error={searchReleaseError} onAddAlbumToPlaylist={addAlbumToPlaylist} onAnalyzeLoudness={prepareReleaseLoudnessAnalysis} onPlayAlbum={playReleaseAlbum} onQueueAlbum={(items,resource)=>queueArtistTracks(items,"append",resource)} onPlay={playArtistTrack} onQueue={queueArtistTracks} onDownload={(track)=>void openDownloadChooser(track)} onDownloadAlbum={(items,onReceipt)=>queueSelected(items,onReceipt as ((receipt:AlbumBatchReceipt)=>void|Promise<void>)|undefined)} onFavorite={(track)=>void favorite(track)} onArtist={(track)=>void discoverArtist(track)} onPlaylist={(track)=>void addArtistTrack(track)} onAddSelectedToPlaylist={addTracksToPlaylist} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} />{:else}
      <SearchPanel startupReadStorage={backupStartupReadStorage} searchArtists={(value)=>searchArtists(value,1).then(page=>page.results)} loadArtistProfile={loadArtistProfileArtwork} onTrackAlbum={(track) => void openTrackAlbum(track)} bind:this={searchComponent} navigationSnapshotKey={searchNavigationSnapshotKey} {automationSearch} {automationSearchCancelled} onAutomationSearchComplete={completeAutomationSearch} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} playingTrackId={playingTrack?.id??""} bufferingTrackId={playbackLoading?playingTrack?.id??"":""} {paused} onRemoteResult={(row)=>void openRemoteSearchResult(row)} onAddRemotePlaylist={addRemoteSearchPlaylist} onPlaylistTracks={addTracksToPlaylist} onQueueTracks={queueArtistTracks} onFavorite={(track)=>void favorite(track)} {availableSources} {query} searchPreset={catalogSearchRequest ? {sourceId:catalogSearchSource,kind:catalogSearchKind,requestId:catalogSearchRequest} : undefined} onQueryChange={(value)=>query=value} {sourceCapabilities} onArtist={(artist)=>void discoverArtistRef(artist)} onArtistTrack={(track)=>void discoverArtist(track)} onAlbum={(release)=>void openSearchAlbum(release)} onPlay={(track,list,index)=>void startPreview(track,false,false,list??[track],index??0)} onDownload={(track)=>void openDownloadChooser(track)} onDownloadTracks={(items)=>void queueSelected(items,undefined,"slskd")} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onContext={(track,event)=>{actionTrack=track;actionAnchor=event ? {x:event.clientX,y:event.clientY}:null;}} onPlayChapter={playChapter} onQueueChapter={queueChapter} onDownloadChapter={(parent,candidate,chapter)=>void downloadChapter(parent,candidate,chapter)} onInfoChapter={(parent,candidate,chapter)=>{const virtual=virtualChapterTrack(parent,candidate,chapter);if(virtual)searchMoreInfo(virtual,candidate,chapter.title);}} />{/if}
    {:else if view === "radio"}
      <RadioView onAddSelectedToPlaylist={addTracksToPlaylist} onTrackAlbum={(track) => void openTrackAlbum(track)} currentStationMetadata={(playingTrack?.provenance as {radioMetadata?:RadioTrackMetadata}|undefined)?.radioMetadata ?? null} onIdentifyPlayingStation={()=>identifyPlayingRadio()} identifyingPlayingStation={radioRecognizing} playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} onInfoTrack={(track)=>openPlayerPanel("info",track)} onOpenHistory={(stationId)=>{historyQuery="";historyProvider="radio";historyStationId=stationId;historyFrom="";historyTo="";historyOrder="newest";void navigate("history");}} {favoriteTrackIds} onPlayTrack={(track,list,index)=>void startPreview(track,false,false,list,index)} onDownloadTrack={(track)=>void openDownloadChooser(track)} onFavoriteTrack={(track)=>void favorite(track)} onQueueTracks={queueArtistTracks} onAddTrackToPlaylist={addArtistTrack} savedStations={radioStations} stationNames={Object.fromEntries(radioStations.map(station=>[station.id,station.name]))} onAddClosed={()=>showAddRadio=false} selectedStationId={selectedRadioStationId} showAdd={showAddRadio} onStationsChanged={(stations)=>{const added=stations.find(station=>!radioStations.some(existing=>existing.id===station.id));radioStations=stations;if(added)selectedRadioStationId=added.id;else if(!stations.some(station=>station.id===selectedRadioStationId))selectedRadioStationId=stations[0]?.id??"";}} onPlayStation={playRadioStation} onArtist={(track)=>void discoverArtist(track)} onTrackContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};void matchArtistPlaybackSources([track]).then(([matched])=>{if(actionTrack===track&&matched)actionTrack=matched;}).catch(failure=>{error=String(failure);});}} playingStationId={!paused ? playingRadioStation?.id??"" : ""} onNowPlaying={radioNowPlaying} />
    {:else if view === "source-browser"}
      {#if provider==="spotify" || provider==="youtube"}{#key `${provider}:${remoteOwner}:${remotePlaylistUrl}`}
        {#if remotePlaylistUrl}<RemotePlaylistDetail playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} {provider} playlistUrl={remotePlaylistUrl} {call} onBrowseOwner={openRemoteOwnerProfile} onAddSelectedToPlaylist={addTracksToPlaylist} onBack={() => { if(backRoutes.length) void travelRoute("back"); else remotePlaylistUrl = ""; }} onImported={remotePlaylistsImported}
          playingTrackId={playingTrack?.id ?? ""} bufferingTrackId={playbackLoading ? playingTrack?.id ?? "" : ""} {paused}
          onPlay={async (track,list,index) => { await startPreview(track,false,false,list,index); }} onQueue={(track) => queueArtistTracks([track],"append")} onNext={(track) => queueArtistTracks([track],"next")} onQueueTracks={queueArtistTracks}
          onDownload={openDownloadChooser} onFavorite={favorite} onPlaylist={addArtistTrack} onArtist={discoverArtist} onAlbum={(track) => void openTrackAlbum(track)} onInfo={(track) => openPlayerPanel("info",track)} onContext={(track,event) => {actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} />
        {:else if remoteOwner}<RemotePlaylists {call} initialProvider={provider} initialOwner={remoteOwner} profileOnly onOpenPlaylist={(source,url) => void openRemotePlaylist(source,url)} onBrowseOwner={openRemoteOwnerProfile} onImported={remotePlaylistsImported} />
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
        {#if displayedHistoryItems.length}<TrackList playbackFailed={nativeActive() ? nativePlaybackStatus === "failed" : Boolean(playerErrorMessage)} onAlbum={(track) => void openTrackAlbum(track)} virtualizationThreshold={50} orderedRows={displayedHistoryItems.map((item,index)=>({kind:"track" as const,key:item.sessionId,trackIndex:index}))} focusedRowKey={historyFocusedSessionId} tracks={displayedHistoryItems.map(item=>item.track)} listLabel="Playback history tracks" presentation="playlist" playingTrack={playingTrack} bufferingTrack={playbackLoading?playingTrack:null} playingTrackId={playingTrack?.id??""} bufferingTrackId={playbackLoading?playingTrack?.id??"":""} {paused} position={(_,index)=>index+1} meta={historySource} timestamp={(_,index)=>({label:new Date(displayedHistoryItems[index].playedAt).toLocaleString(undefined,{dateStyle:"medium",timeStyle:"long"}),full:new Date(displayedHistoryItems[index].playedAt).toString()})} onPlay={async (track,list,index)=>{ await startPreview(track,false,false,list,index); }} onDownload={(track)=>openDownloadChooser(track)} onFavorite={favorite} onArtist={(track)=>discoverArtist(track)} onNext={(track)=>queueArtistTracks([track],"next")} onQueue={(track)=>queueArtistTracks([track],"append")} onPlaylist={addArtistTrack} onInfo={(track)=>openPlayerPanel("info",track)} onContext={(track,event)=>{actionTrack=track;actionAnchor={x:event.clientX,y:event.clientY};}} initialScroll={historyScroll} onScroll={(value)=>historyScroll=value}/>
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

        {:else if view === "collections"}{#if collectionId===DOWNLOADS_PLAYLIST_ID}<button aria-label="Open download activity" onclick={()=>{openPlayerPanel("downloads");void refreshJobs();}}>Download activity</button>{/if}<button onclick={()=>{renamePlaylistName=currentCollection?.name??"";renamingPlaylist=true;}}>Rename</button><button onclick={openPlaylistImport}>Import tracks</button>
          {#if renamingPlaylist}<input aria-label="Playlist name" bind:value={renamePlaylistName} onkeydown={(event)=>{if(event.key==="Enter")void renameCurrentPlaylist();if(event.key==="Escape")renamingPlaylist=false;}} /><button onclick={()=>void renameCurrentPlaylist()}>Save name</button>{/if}
          <button disabled={!collectionId || batchRunning} onclick={() => void downloadPlaylist(collectionId, true)}>↓ Download playlist</button>{#if currentPlaylistCreator}<button disabled={Boolean(busy)} onclick={()=>void refreshLinkedPlaylist(collectionId)}>Refresh</button>{#if (currentPlaylistCreator.provider === "spotify" || currentPlaylistCreator.provider === "youtube") && currentPlaylistCreator.canEdit}<PlaylistSync {call} bindingId={currentPlaylistCreator.bindingId} enabled={remotePlaylistWriteSyncEnabled[currentPlaylistCreator.provider as "spotify" | "youtube"]} onApplied={()=>refreshLinkedPlaylist(collectionId)}/>{/if}{/if}<button class="playlist-context-trigger" aria-label="Playlist context menu" title="Playlist actions" onclick={openCurrentPlaylistMenu}>⋯</button>{/if}
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
            {#if view === "collections" && checked.length && selected}<button disabled={selectedPlaylistIndex() <= 0} onclick={() => void moveSelectedPlaylistEntry(-1)}>Move up</button><button disabled={selectedPlaylistIndex() < 0 || selectedPlaylistIndex() >= playlistMoveLimit - 1} onclick={() => void moveSelectedPlaylistEntry(1)}>Move down</button>{/if}
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
            <TrackList playbackFailed={nativeActive() ? nativePlaybackStatus === "failed" : Boolean(playerErrorMessage)} reorderable={view === "collections" && sort === "rank" && !descending && !filter.trim() && localFilter === "all" && !durationMinimum && !durationMaximum && !playlistReordering && !localPageLoading} onReorder={movePlaylistEntry} onInvertSelection={() => {if(view === "collections") selectedLocalOccurrences = invertSelection(localOccurrenceRows.map(row=>row.key),selectedLocalOccurrences); else {checked=invertSelection(sorted.map(track=>track.id),checked);mergeConfirmed=false;}}} onAlbum={(track) => void openTrackAlbum(track)} tracks={virtualized ? visibleTracks : sorted} focusedRowKey={view === "collections" ? focusedPlaylistRowKey : null} listLabel="Recordings" presentation="playlist" playingTrack={playingTrack} bufferingTrack={playbackLoading ? playingTrack : null} {paused} selectionMode={view === "collections" ? localPlaylistSelectionMode : batchSelectionMode} selectedTrackId={selected?.id} selectedTrackIds={view === "collections" ? selectedLocalOccurrences : checked} selectionKeys={view === "collections" ? localOccurrenceRows.slice(virtualized ? startRow : 0, virtualized ? endRow : sorted.length).map(row => row.key) : undefined} selectionLabel={(track, index) => view === "collections" ? `Select occurrence ${(virtualized ? startRow : 0) + index + 1}: ${track.title} for playlist` : `Select ${track.title}`} trackTitle={(track) => [track.title, bpmAnnotation(track), analysisAnnotation(track, "key")].filter(Boolean).join("\n")} position={(_, index) => (virtualized ? startRow : 0) + index + 1} meta={(track) => [track.bpm != null ? `${analysisAnnotation(track, "bpm") ? "≈" : ""}${String(track.bpm)} BPM` : "", track.key ? `${analysisAnnotation(track, "key") ? "≈" : ""}${track.key}` : "", !track.album && track.label ? track.label : "", track.localAvailability === "available" || (!track.localAvailability && track.localPath) ? "Downloaded" : "", track.disposition === "shortlist" ? "Shortlisted" : "", rowDownloadFeedback[track.id] ?? ""].filter(Boolean).join(" · ")} onPlay={(track, _window, index) => select(track, "click", (virtualized ? startRow : 0) + index)} onRowSelect={(track, event) => selectTrackRow(track, event)} onFavorite={favorite} onDownload={(track) => void openDownloadChooser(track, undefined, undefined, currentSavedPlaylistContext())} onArtist={(track) => void discoverArtist(track)} onContext={(track, event) => { actionTrack = track; const rect = event.type === "contextmenu" ? null : (event.currentTarget as HTMLElement).getBoundingClientRect(); actionAnchor = rect ? { x: rect.right, y: rect.bottom } : { x: event.clientX, y: event.clientY }; }} onSelect={(track, event, index) => { const checkedNow = (event.currentTarget as HTMLInputElement).checked; if (view === "collections") toggleLocalOccurrence((virtualized ? startRow : 0) + index, checkedNow); else { checked = checkedNow ? [...new Set([...checked, track.id])] : checked.filter(id => id !== track.id); mergeConfirmed = false; } }} virtualizationThreshold={100000} />
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
  {#if showPlayingInfo || showQueue}<section class="player-panel-host" aria-label="Player panel">{#if showQueue}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (Keyboard and pointer splitter.) -->
  <div class="queue-panel-resize" role="separator" tabindex="0" aria-label="Resize queue panel" aria-orientation="vertical" aria-valuemin="280" aria-valuemax="420" aria-valuenow={infoPanelWidth} onpointerdown={resizeQueuePanel} onkeydown={(event)=>{if(event.key==="ArrowLeft"||event.key==="ArrowRight"){event.preventDefault();saveLayoutWidth("info",infoPanelWidth+(event.key==="ArrowLeft"?10:-10));}}}></div>{/if}<nav class="player-panel-tabs"><button class:active={showPlayingInfo} onclick={()=>openPlayerPanel("info")}>Now playing</button><button class:active={showQueue} onclick={()=>openPlayerPanel("queue")}>Queue</button><button aria-label="Close player panel" onclick={()=>openPlayerPanel(null)}>×</button></nav>
  {#if showPlayingInfo && (infoTrack || playingTrack)}{@const shownTrack = infoTrack || playingTrack!}{@const shownActionTrack = shownTrack.id===playingTrack?.id ? playbackActionTrack??shownTrack : shownTrack}<PlayingInfo loadRecording={recordingInformation.load} onRecordingRelease={(release,artist) => void browseRecordingRelease(release,artist)} loadArtist={loadNowPlayingArtist} onArtistRef={(artist) => {openPlayerPanel(null);void discoverArtistRef(artist);}} onAlbum={trackAlbumTarget(shownActionTrack) ? () => {openPlayerPanel(null);void openTrackAlbum(shownActionTrack);} : undefined} stationName={shownTrack.id===playingTrack?.id ? playingRadioStation?.name ?? "" : ""} liveBpm={shownTrack.id===playingTrack?.id && !paused ? liveBpmEstimate : null} liveKey={shownTrack.id===playingTrack?.id && !paused ? liveKeyEstimate : null} lastLiveBpm={shownTrack.id===playingTrack?.id ? lastLiveBpm : null} lastLiveKey={shownTrack.id===playingTrack?.id ? lastLiveKey : null} liveBpmStatus={shownTrack.id===playingTrack?.id ? settings.liveBpmEnabled !== true ? "disabled" : paused ? "paused" : liveBpmPollStatus : undefined} liveBpmStatusReason={shownTrack.id===playingTrack?.id ? liveBpmPollReason : null} artworkDisplayUrl={artworkDisplayUrls[shownTrack.artworkUrl ?? ""]} onArtworkError={(track,failedUrl)=>void recoverDisplayArtwork(track,failedUrl)} onFavorite={()=>void favorite(shownTrack.id===playingTrack?.id?(playbackActionTrack??shownTrack):shownTrack)} favorite={(shownTrack.id===playingTrack?.id?(playbackActionTrack??shownTrack):shownTrack).favorite} docked={true} activeChapter={shownTrack.id===playingTrack?.id ? activeChapter?.title : undefined} panelWidth={infoPanelWidth} onPanelWidthChange={(width)=>saveLayoutWidth("info",width)} onPlay={()=>{if(shownTrack.id===playingTrack?.id)void toggleCurrentPlayback();else void startPreview(shownTrack);}} onOpenMpv={mpvAvailable?()=>void openTrackInMpv(shownTrack):undefined} onIdentifyRadio={shownTrack.id===playingTrack?.id&&(playingRadioStation||nativeActive()||preview?.sourceGeneration!==undefined)?()=>void (radioRecognizing?cancelRadioIdentification():identifyPlayingTrack()):undefined} onCancelIdentifyRadio={!nativeActive() || !playingRadioStation ? ()=>cancelRadioIdentification():undefined} identifyProgress={radioIdentifyProgress} identifyingRadio={radioRecognizing} loudnessStatus={shownTrack.id===playingTrack?.id ? nativeActive() ? (nativeSnapshot as (NativePlaybackSnapshot & {loudnessNormalization?: LoudnessPlaybackStatus}) | null)?.loudnessNormalization ?? null : webviewLoudnessDecision : null} loudnessMode={webviewLoudnessSettings.mode} loudnessAnalysisJob={loudnessAnalysisJob} pendingLoudnessAnalysisCount={pendingLoudnessAssetIds.length} loudnessAnalysisMessage={loudnessAnalysisMessage} onAnalyzeLoudness={trackAsset(shownActionTrack) || shownActionTrack.localPath ? () => void analyzeLoudnessTracks([shownActionTrack]) : undefined} onCancelLoudnessAnalysis={cancelLoudnessAnalysis} onRetryLoudnessAnalysis={retryQueuedLoudnessAnalysis} analysisEstimate={shownTrack.id===playingTrack?.id ? currentPlaybackAnalysisEstimate : null} analysisStatus={shownTrack.id===playingTrack?.id ? currentLocalAnalysisStatus || streamMediaAnalysisStatus : ""} trackDetectionView={shownTrack.id===playingTrack?.id && currentPlayingTrackDetectionOwner()!==null ? playingTrackDetectionView : savedDetectionViewsByTrack[shownTrack.id] ?? null} trackDetectionStatus={shownTrack.id===playingTrack?.id ? playingTrackDetectionStatus : ""} trackDetectionDisabledReason={shownTrack.id===playingTrack?.id ? playingTrackDetectionDisabledReason : ""} trackDetectionAvailable={desktop() && shownTrack.id===playingTrack?.id && currentPlayingTrackDetectionOwner()!==null} onTrackDetectionToggle={togglePlayingTrackDetection} trackDetectionMarkersEnabled={detectionMarkersEnabled} onTrackDetectionMarkersToggle={(enabled)=>detectionMarkersEnabled=enabled} onTrackDetectionSeek={(seconds)=>{if(shownTrack.id===playingTrack?.id)seekChapter(seconds);else void (async()=>{await startPreview(shownTrack);if(playingTrack?.id===shownTrack.id)seekChapter(seconds);})();}} onTrackDetectionOpen={(track)=>void startPreview(track)} onTrackDetectionAddToPlaylist={(track)=>void addArtistTrack(track,false)} onTrackDetectionsAddToPlaylist={(tracks)=>void addTracksToPlaylist(tracks)} track={shownTrack} trackLibrarySummary={trackLibrarySummaryTrackId === shownTrack.id ? trackLibrarySummary : null} trackLibrarySummaryLoading={trackLibrarySummaryLoading && trackLibrarySummaryTrackId === shownTrack.id} trackLibrarySummaryRequested={trackLibrarySummaryTrackId === shownTrack.id} asset={trackAsset(shownTrack)} availableFiles={filesForTrack(shownTrack)} preferredAssetId={shownTrack.assetId??null} onPreferredFile={(assetId)=>void preferTrackFile(shownTrack,assetId)} streamQuality={shownTrack.id===playingTrack?.id && (preview?.kind==="audio" || nativeActive() && !playingTrack?.localPath) ? {codec:nativeSnapshot?.quality?.codec ?? preview?.audioCodec,bitrateKbps:nativeSnapshot?.quality?.bitrateKbps ?? preview?.audioBitrateKbps,sampleRate:nativeSnapshot?.quality?.sampleRate,channels:nativeSnapshot?.quality?.channels,bitrateOrigin:nativeSnapshot?.quality?.bitrateOrigin,live:Boolean(playingRadioStation)} : undefined} onUpgrade={()=>void openDownloadChooser(shownActionTrack)} onRadio={() => { showPlayingInfo = false; void startSongRadio(shownActionTrack); }} radioBusy={radioBusy === shownTrack.id} candidate={shownTrack.id === playingTrack?.id ? playingCandidate : null} onClose={() => { showPlayingInfo = false; infoTrack = null; }} onArtist={() => { showPlayingInfo = false; void discoverArtist(shownActionTrack); }} onDownload={() => { showPlayingInfo = false; void openDownloadChooser(shownActionTrack); }} onPlaylist={() => { showPlayingInfo = false; void addArtistTrack(shownActionTrack); }} onHistoryOccurrence={(sessionId)=>void openHistoryOccurrence(sessionId)} onPlaylistMembership={(playlistId,trackId)=>void openPlaylistMembership(playlistId,trackId)} onMoreInfo={() => searchMoreInfo(shownTrack,shownTrack.id===playingTrack?.id?playingCandidate:null,shownTrack.id===playingTrack?.id?activeChapter?.title:undefined)} />{/if}
  {#if showQueue}<PlaybackQueuePanel onVisibleTrack={loadSavedTrackDetections} {detectionsByTrack} onSeekDetection={(parent,observation,index)=>{void (async()=>{if(parent.id!==playingTrack?.id)await startPreview(parent,true,false,queueTracks,index);if(playingTrack?.id===parent.id)seekChapter(observation.detectedAtSeconds);})();}} onInfoDetection={(track)=>void startPreview(track)} onAddDetectionToPlaylist={(track)=>void addArtistTrack(track,false)} onAddDetectionsToPlaylist={(tracks)=>void addTracksToPlaylist(tracks)} queueRevision={nativeActive()?nativeSnapshot?.queueRevision:undefined} onInteractionMove={queueUiMove??undefined} interactionSignal={queueUiController?.signal} paused={paused} buffering={playbackLoading} activePlaybackTrack={playbackActionTrack} {chapterRangesByTrack} radioNowPlaying={currentRadioSong} radioStationName={playingRadioStation?.name??""} radioHistory={previousRadioQueue} {radioRecognizing} radioIdentifyProgress={radioIdentifyProgress} onIdentifyRadio={playingRadioStation?()=>void (radioRecognizing?cancelRadioIdentification():identifyPlayingRadio()):undefined} onCancelIdentifyRadio={!nativeActive()?cancelRadioIdentification:undefined} onIdentifyCurrent={playingTrack&&!playingRadioStation&&(nativeActive()||preview?.sourceGeneration!==undefined)?()=>void (radioRecognizing?cancelRadioIdentification():identifyPlayingTrack()):undefined} onCancelIdentifyCurrent={()=>cancelRadioIdentification()} identifyingCurrentAudio={radioRecognizing} onPlayRadioTrack={(track)=>void startPreview(track)} onDownloadRadioTrack={(track)=>void openDownloadChooser(track)} onFavoriteRadioTrack={(track)=>void favorite(track)} onArtistRadioTrack={(track)=>void discoverArtist(track)} onInfoRadioTrack={(track)=>openPlayerPanel("info",track)} onContextRadioTrack={(track)=>{actionTrack=track;actionAnchor=null;}} onInfoTrack={(track)=>openPlayerPanel("info",track)} onAddToPlaylist={(track)=>void addArtistTrack(track,false)} {chaptersByTrack} onPlayChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)playChapter(parent,candidate,chapter);}} onQueueChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)queueChapter(parent,candidate,chapter,"append");}} onAddChapterToPlaylist={(parent,chapter)=>void addChapterToPlaylist(parent,chapter)} onDownloadChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate)void downloadChapter(parent,candidate,chapter);}} onInfoChapter={(parent,chapter)=>{const candidate=candidateForQueue(parent);if(candidate){const virtual=virtualChapterTrack(parent,candidate,chapter);if(virtual)searchMoreInfo(virtual,candidate,chapter.title);}}} onQueueAllChapters={(parent,chapters)=>queueAllChapters(parent,chapters)} onInspectChapters={(parent)=>void inspectQueueChapters(parent)} tracks={queueTracks} currentIndex={queueIndex} chapters={playingCandidate?.chapters ?? []} {position} chapterPosition={nativeActive() && playingTrack?.playbackRange ? chapterAbsolutePosition(playingTrack,position) : position} {repeatMode} {shuffleEnabled} onClose={() => openPlayerPanel(null)} onPlay={playQueueIndex} onMove={moveQueueItem} onSortUpcoming={sortUpcoming} onRemove={removeQueueItem} onRemoveMany={removeQueueItems} onSeek={seekChapter} onToggleShuffle={toggleShuffle} onCycleRepeat={cycleRepeat} />{/if}
  </section>{/if}
  {#if spotifyTrackArtistChoices.length}<div class="modal-backdrop"><dialog use:activateDownloadDialog={() => spotifyTrackArtistChoices = []} tabindex="-1" class="download-dialog playlist-picker-dialog spotify-track-artist-chooser" aria-label="Choose Spotify artist"><h2>Choose Spotify artist</h2><p>This track credits multiple Spotify artists.</p>{#each spotifyTrackArtistChoices as artist (artist.id)}<button type="button" aria-label={`Open Spotify artist ${artist.name}`} onclick={() => void openSpotifyTrackArtist(artist)}>{artist.name}</button>{/each}<button type="button" onclick={() => spotifyTrackArtistChoices = []}>Cancel</button></dialog></div>{/if}
  {#if showReleaseSetup}<dialog open class="download-dialog release-onboarding" aria-label="Welcome to Burrata"><ReleaseSetup onboarding onSettingsChanged={()=>void call<Settings>("get_settings").then(value=>settings={...value,matchingLevel:value.matchingLevel??"balanced",playbackStartupTimeoutSeconds:value.playbackStartupTimeoutSeconds??20})} onComplete={()=>showReleaseSetup=false}/></dialog>{/if}
  {#if renameTarget}<dialog class="download-dialog playlist-picker-dialog" use:activateRenameDialog aria-label={`Rename ${renameTarget.kind}`} tabindex="-1"><form onsubmit={(event)=>{event.preventDefault();void saveSidebarRename();}}><h2>Rename {renameTarget.kind==="radio"?"station":"playlist"}</h2><label>Name<input aria-label="New name" maxlength="160" bind:value={sidebarRenameValue}/></label><footer><button type="button" onclick={()=>renameTarget=null}>Cancel</button><button class="primary" disabled={!sidebarRenameValue.trim()}>Save</button></footer></form></dialog>{/if}
  {#if radioActions}{@const menu=radioActions}<PlaylistMenu onDelete={()=>void removeSidebarStation(menu.station)} kind="radio station" name={menu.station.name} anchor={menu.anchor} {playerHeight} onRename={()=>startSidebarRename("radio",menu.station.id,menu.station.name)} onOpen={()=>{selectedRadioStationId=menu.station.id;void navigate("radio");}} onOpenSource={()=>void openSourceToRefresh(menu.station.homepageUrl??menu.station.sourceUrl)} onClose={()=>radioActions=null}/>{/if}
  {#if playlistActions}{@const menu = playlistActions}<PlaylistMenu onRefresh={remotePlaylistBindings.some(binding=>binding.collectionId===menu.collection.id||binding.sourceCollectionId===menu.collection.id)?()=>void refreshLinkedPlaylist(menu.collection.id):undefined} onExport={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"?()=>{portablePlaylistInitialMode="export";if(menu.collection.id==="favorites")portablePlaylistTarget={id:"favorites",name:menu.collection.name,trackCount:favoriteTrackIds.length};else portablePlaylistTarget=menu.collection;}:menu.collection.id==="history"?()=>showPortableHistory=true:undefined} onMoveToFolder={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"?()=>{portablePlaylistInitialMode="folder";if(menu.collection.id==="favorites")portablePlaylistTarget={id:"favorites",name:menu.collection.name,trackCount:favoriteTrackIds.length};else portablePlaylistTarget=menu.collection;}:undefined} onCopy={menu.collection.id==="favorites"||menu.collection.id===playlistPreferences.favoritesPlaylistId?()=>void copyFavoritesPlaylist():activeBrowserImport?.collectionId===menu.collection.id?()=>void run(copyCurrentImport):undefined} onRename={()=>startSidebarRename("playlist",menu.collection.id,menu.collection.name)} onDownload={collections.some(item=>item.id===menu.collection.id)?()=>void downloadPlaylist(menu.collection.id):undefined} onOpenSource={(sourceUrlForCollection(menu.collection.id) || (menu.collection.id===chart?.id?currentChartSourceUrl:null)) ? () => void openSourceToRefresh((sourceUrlForCollection(menu.collection.id) || currentChartSourceUrl)!) : undefined} name={menu.collection.name} anchor={menu.anchor} {playerHeight} onClose={() => (playlistActions = null)} onOpen={() => void openPlaylist(menu.collection.id)} onMirrorSettings={collections.some(item=>item.id===menu.collection.id)||menu.collection.id==="favorites"||menu.collection.id==="history"?() => playlistMirrorSettingsTarget = { collectionId: menu.collection.id, name: menu.collection.name }:undefined} onDelete={menu.collection.id!==DOWNLOADS_PLAYLIST_ID && collections.some(item=>item.id===menu.collection.id)?()=>void deletePlaylist(menu.collection):undefined} />{/if}
  {#if actionTrack}{@const menuTrack = actionTrack}{@const albumTarget = trackAlbumTarget(menuTrack)}<TrackMenu track={menuTrack} onRadio={() => void startSongRadio(menuTrack)} radioBusy={radioBusy === menuTrack.id} anchor={actionAnchor} {playerHeight} onRemove={view === "collections" ? () => void removePlaylistTrack(menuTrack) : undefined} onClose={() => (actionTrack = null)} onNext={() => queueArtistTracks([menuTrack], "next")} onQueue={() => queueArtistTracks([menuTrack], "append")} onPlaylist={() => void addArtistTrack(menuTrack)} onGoToAlbum={albumTarget ? () => void openTrackAlbum(menuTrack) : undefined} onGoToAlbumUnavailable={!albumTarget} onFavorite={() => void favorite(menuTrack)} onDownload={() => void openDownloadChooser(menuTrack, undefined, undefined, currentSavedPlaylistContext())} onUpgrade={()=>void openDownloadChooser(menuTrack)} onMoreInfo={() => searchMoreInfo(menuTrack,importedAudioCandidate(menuTrack))} onArtist={() => void discoverArtist(menuTrack)} onOpenMpv={mpvAvailable?()=>void openTrackInMpv(menuTrack):undefined} onDetails={() => openPlayerPanel("info", menuTrack)} onSelect={() => { if (view === "collections") localPlaylistSelectionMode = true; else batchSelectionMode = true; if (!checked.includes(menuTrack.id)) checked = [...checked, menuTrack.id]; }} />{/if}
  {#if infoSearchTarget}<TrackInfoSearch track={infoSearchTarget.track} candidate={infoSearchTarget.candidate} chapterTitle={infoSearchTarget.chapterTitle} onClose={() => (infoSearchTarget=null)} onCatalogSearch={openCatalogInfoSearch} onWebSearch={openWebInfoSearch} onOpenSource={openInfoSource} />{/if}
  {#if showFullNowPlaying && playingTrack}
    <NowPlayingView onSeek={seekChapter} track={expandedPlayingTrack ?? playingTrack} lyricsTrack={currentLyricsTrack ?? playingTrack} absolutePosition={position} loadLyrics={()=>call("get_current_track_lyrics",{visibleTrack:currentVisibleLyricsTrack()})} findLyrics={()=>call("search_current_track_lyric_candidates",{visibleTrack:currentVisibleLyricsTrack()})} chooseLyrics={(scopeId,candidateId)=>call("select_current_track_lyric_candidate",{visibleTrack:currentVisibleLyricsTrack(),scopeId,candidateId})} saveLyrics={async(sourceHash)=>{const result=await call("save_current_track_lyrics_to_file",{visibleTrack:currentVisibleLyricsTrack(),sourceHash});await refreshCurrentAssetFiles();return result;}} exportLyrics={(sourceHash,text)=>call("export_current_track_lyrics_text",{visibleTrack:currentVisibleLyricsTrack(),sourceHash,text})} translationLanguage={lyricsTranslationLanguage} changeTranslationLanguage={changeLyricsTranslationLanguage} translationEnabled={lyricsTranslationEnabled} translationCredentialPresent={lyricsTranslationCredentialPresent} translateLyrics={translateCurrentTrackLyrics} syncLyrics={playingTrack && currentLyricsTrack?.id===playingTrack.id && nativeActive() && nativeSnapshot?.status==="playing" && !paused && !playingRadioStation && !playingTrack.playbackRange && !nativeSnapshot?.chapter ? () => syncLyricsWithAudio(playingTrack!.id) : undefined} syncScope={JSON.stringify([playingTrack?.id??null,nativeSnapshot?.generation??null,seekSerial,playbackQueue.entryIds[queueIndex]??null])} title={activeRecordingMatch?.assessment.relationship === "alternative" ? activeRecordingMatch.resolvedRecording.title : playingTitle}
      artworkUrl={artworkDisplayUrls[(expandedPlayingTrack ?? playingTrack).artworkUrl ?? ""]} sourceArtworkUrl={originalArtworkSource((expandedPlayingTrack ?? playingTrack).artworkUrl)} detectedArtworkUrl={activeDetectedArtwork} artworkMode={fullscreenArtworkMode} onArtworkModeChange={(mode)=>fullscreenArtworkMode=mode} bottom={playerHeight}
      nextTrack={queueTracks[queueIndex + 1] ?? (repeatMode === "all" ? queueTracks[0] : null) ?? null} context={playingRadioStation?.name ?? "Now playing"}
      loadRecording={recordingInformation.load} onRecordingRelease={(release,artist) => void browseRecordingRelease(release,artist)} loadArtist={loadNowPlayingArtist} onArtistRef={(artist) => {showFullNowPlaying=false;void discoverArtistRef(artist);}} onClose={() => showFullNowPlaying = false}
      onArtist={() => { showFullNowPlaying = false; const track = expandedPlayingTrack ?? playingTrack!; void discoverArtist(activeRecordingMatch?.assessment.relationship === "alternative" ? {...track,provider:"unknown",providerId:""} : track); }}
      onAlbum={activeRecordingMatch?.assessment.relationship !== "alternative" && trackAlbumTarget(playbackActionTrack ?? playingTrack) ? () => { showFullNowPlaying = false; void openTrackAlbum(playbackActionTrack ?? playingTrack!); } : undefined}
      onQueue={() => { showFullNowPlaying = false; openPlayerPanel("queue"); }} onFavorite={() => void favorite(playbackActionTrack ?? playingTrack!)} />
  {/if}
  {#if showFullscreenLyrics && playingTrack}
    <LyricsPanel bottomInset={playerHeight} onSeek={seekChapter} track={currentLyricsTrack ?? playingTrack} absolutePosition={position} fullscreenRequested={true} onFullscreenClose={()=>{showFullscreenLyrics=false;void tick().then(()=>lyricsButton?.focus());}}
      loadLyrics={()=>call("get_current_track_lyrics",{visibleTrack:currentVisibleLyricsTrack()})} findLyrics={()=>call("search_current_track_lyric_candidates",{visibleTrack:currentVisibleLyricsTrack()})} chooseLyrics={(scopeId,candidateId)=>call("select_current_track_lyric_candidate",{visibleTrack:currentVisibleLyricsTrack(),scopeId,candidateId})} saveLyrics={async(sourceHash)=>{const result=await call("save_current_track_lyrics_to_file",{visibleTrack:currentVisibleLyricsTrack(),sourceHash});await refreshCurrentAssetFiles();return result;}} exportLyrics={(sourceHash,text)=>call("export_current_track_lyrics_text",{visibleTrack:currentVisibleLyricsTrack(),sourceHash,text})} translationLanguage={lyricsTranslationLanguage} changeTranslationLanguage={changeLyricsTranslationLanguage}
      translationEnabled={lyricsTranslationEnabled} translationCredentialPresent={lyricsTranslationCredentialPresent} translateLyrics={translateCurrentTrackLyrics} />
  {/if}
  <footer id="burrata-playback-controls" aria-label="Playback controls" class="player" class:compact={settings.compactPlaybackBar === true} bind:offsetHeight={playerHeight}>
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
        {#if activeRecordingMatch?.assessment.relationship === "alternative"}<small class="player-source">Alternative recording · requested {playingTrack?.title}</small>{/if}
        <small class="player-status" role="status" title={previewStatus}>
          {playerStatusMessage}
        </small>
        {#if playingTrack}<PlaybackQuality compact track={playingTrack} asset={preview?.kind==="local" || nativeActive() && Boolean(playingTrack?.localPath) ? trackAsset(playingTrack) : null} streamQuality={preview?.kind==="audio" || nativeActive() && !playingTrack?.localPath ? {codec:nativeSnapshot?.quality?.codec ?? preview?.audioCodec,bitrateKbps:nativeSnapshot?.quality?.bitrateKbps ?? preview?.audioBitrateKbps,sampleRate:nativeSnapshot?.quality?.sampleRate,channels:nativeSnapshot?.quality?.channels,bitrateOrigin:nativeSnapshot?.quality?.bitrateOrigin,live:Boolean(playingRadioStation)} : undefined} />{/if}
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

      <div class="player-scrubber">
        <span class="time">{time(playerPosition)}</span>
        {#if liveRadioPlayback}<span class="radio-live-label">Live radio</span>
        {:else if waveformEnabled}
          <WaveformSeek position={playerPosition} duration={playerDuration} playing={!paused && !bufferingAudio} playbackIdentity={`${playingTrack?.id}:${nativeSnapshot?.generation}:${seekSerial}:${nativeSnapshot?.seekSerial}`}
            disabled={nativeActive() ? !nativeSnapshot?.canSeek : !preview?.canSeek || (!audio && !preview?.sessionId)}
            generation={waveformGeneration} trackId={waveformTrackId} waveform={playbackWaveformForDisplay}
            detections={playbackDetectionMarkers} detectionSourceOffset={playbackDetectionMarkerSourceOffset}
            onSeek={(value) => void seekPlayer(value)} />
        {:else}
          <SeekSlider position={playerPosition} duration={playerDuration} playing={!paused && !bufferingAudio} playbackIdentity={`${playingTrack?.id}:${nativeSnapshot?.generation}:${seekSerial}:${nativeSnapshot?.seekSerial}`}
            disabled={nativeActive() ? !nativeSnapshot?.canSeek : !preview?.canSeek || (!audio && !preview?.sessionId)}
            detections={playbackDetectionMarkers} detectionSourceOffset={playbackDetectionMarkerSourceOffset}
            onSeek={(value) => void seekPlayer(value)} />
        {/if}
        {#if !liveRadioPlayback}<span class="time">{time(playerDuration)}</span>{/if}
      </div>
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
      <button class="player-heart" aria-label={playbackActionTrack?.favorite?"Remove playing track from favorites":"Add playing track to favorites"} aria-pressed={playbackActionTrack?.favorite??false} disabled={!playbackActionTrack || Boolean(playingRadioStation&&!currentRadioSong)} title={playbackActionTrack?.favorite ? "Remove playing track from favorites" : "Add playing track to favorites"} onclick={()=>{const track=playbackActionTrack;if(track)void favorite(track);}}>{playbackActionTrack?.favorite?"♥":"♡"}</button>
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
          disabled={nativeActive() ? !nativeSnapshot?.canVolume : !preview?.canVolume || (!audio && !preview?.sessionId)}
          aria-label="Volume"
          oninput={(e) => {
            const value = Number(e.currentTarget.value);
            if (nativeActive()) void nativeCommand({action:"volume",volume:value});
            else if (preview?.sessionId) void run(() => call("candidate_preview_control", { action: "volume", value }));
            else if (audio) setWebviewOutputVolume(audio, value);
          }}
        />
      </label>
      <button bind:this={lyricsButton} title="Fullscreen lyrics" aria-label="Open fullscreen lyrics" disabled={!playingTrack} onclick={()=>showFullscreenLyrics=true}><svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10v2a7 7 0 0 0 14 0v-2M12 19v3M8 22h8"/></svg></button>
      <button title="Track details" aria-label="Show track details" onclick={() => openPlayerPanel("info")} disabled={!playingTrack}>ⓘ</button>
    </div>
    {#if preview && ["audio", "local", "direct"].includes(preview.kind)}<audio
        bind:this={audio}
        use:retireAudioGraphOnDestroy
        ontimeupdate={(event) => { signalWebviewResumeProgress(event.currentTarget); updateWebviewTime(event); }}
        onseeked={(event) => { if(event.currentTarget!==audio)return; webviewBufferingRecovery.reset(); webviewStallWatchdog.reset(); recordWebviewMediaEvent("seeked",event.currentTarget); updatePlaybackPosition(audio?.currentTime ?? 0,false); const landed=completeWebviewSeekIfLanded(event.currentTarget,true); if(landed || (!activeWebviewSeek && !pendingWebviewResume))refreshSteadyMonitorAfterCompletedSeek(event.currentTarget); void publishAutomationPlayback(); }}
        onloadedmetadata={(event) => {
          const activeSource = preview;
          if(event.currentTarget===audio && activeSource && event.currentTarget.getAttribute("src")===activeSource.url) {
            webviewLoadedMetadataOwner = { url: activeSource.url, sourceGeneration: Number.isSafeInteger(activeSource.sourceGeneration) ? Number(activeSource.sourceGeneration) : null };
            duration = event.currentTarget.duration;
          }
        }}
        onplay={(event) => { if(event.currentTarget!==audio)return; if(blockedStartupNodes.has(event.currentTarget)){event.currentTarget.pause();return;} if(webviewObservedCapture?.node===event.currentTarget)webviewObservedCapture.started=true; webviewBufferingRecovery.reset(); recordWebviewMediaEvent("play",event.currentTarget); paused = false; }}
        onpause={(event) => { if(event.currentTarget!==audio || (audio && !audio.paused))return; resetLiveSuccessfulPlayback(event.currentTarget); webviewBufferingRecovery.reset(); webviewStallWatchdog.reset(); webviewSeekCompletionWatchdog.pause(); webviewPostSeekProgress.cancel(); if(activeWebviewSeek)activeWebviewSeek.deadline=null; nextCandidatePrefetch.cancel();
          // HTMLMediaElement events can be delivered after the state transition
          // that queued them. A pause caused by the previous transport pause may
          // arrive while the subsequent play() promise is pending; explicit pause
          // commands cancel that attempt before calling pause(), so this event
          // must not cancel a newer, owner-checked resume.
          if (!pendingWebviewResume || pendingWebviewResume.node !== event.currentTarget || pendingWebviewResume.source !== preview) cancelPendingWebviewResume();
          paused = true; bufferingAudio = false; if(webviewObservedCapture?.node===event.currentTarget && webviewObservedCapture.started && !event.currentTarget.ended)cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because playback paused."); if(radioRecognizing&&!nativeActive())cancelRadioIdentification(); persistPlaybackPosition(); }}
        onwaiting={(event) => { if(event.currentTarget!==audio)return; if(webviewObservedCapture?.node===event.currentTarget && webviewObservedCapture.started)cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because the stream stalled."); resetLiveSuccessfulPlayback(event.currentTarget); webviewBufferingRecovery.reset(); recordWebviewMediaEvent("waiting",event.currentTarget); bufferingAudio = Boolean(audio && !audio.paused); armWebviewStallWatchdog(event.currentTarget); }}
        onstalled={(event) => { if(event.currentTarget!==audio)return; resetLiveSuccessfulPlayback(event.currentTarget); webviewBufferingRecovery.reset(); recordWebviewMediaEvent("stalled",event.currentTarget); bufferingAudio = Boolean(audio && !audio.paused && audio.readyState < 3); armWebviewStallWatchdog(event.currentTarget); }}
        oncanplay={(event) => recordWebviewMediaEvent("canplay",event.currentTarget)}
        oncanplaythrough={(event) => recordWebviewMediaEvent("canplaythrough",event.currentTarget)}
        onseeking={(event) => { if(event.currentTarget!==audio)return; if(!webviewWaveformSeekDrain || webviewWaveformSeekDrain.node!==event.currentTarget || webviewWaveformSeekDrain.sourceGeneration!==(preview?.sourceGeneration??null) || webviewWaveformSeekDrain.seekSerial!==seekSerial)webviewWaveformSeekDrain={node:event.currentTarget,sourceGeneration:preview?.sourceGeneration??null,seekSerial,targetPositionSeconds:event.currentTarget.currentTime}; if(webviewObservedCapture?.node===event.currentTarget && webviewObservedCapture.started)cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because the source was seeked."); webviewBufferingRecovery.reset(); webviewStallWatchdog.reset(); recordWebviewMediaEvent("seeking",event.currentTarget); }}
        onratechange={(event) => { if(event.currentTarget!==audio)return; if(webviewObservedCapture?.node===event.currentTarget && webviewObservedCapture.started && event.currentTarget.playbackRate!==1)cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because playback speed changed."); }}
        onplaying={(event) => { if(event.currentTarget!==audio)return; if(blockedStartupNodes.has(event.currentTarget)){event.currentTarget.pause();return;} signalWebviewResumeProgress(event.currentTarget); webviewBufferingRecovery.reset(); recordWebviewMediaEvent("playing",event.currentTarget); paused = false; armPendingWebviewSeekAfterResume(event.currentTarget); armPostSeekProgress(event.currentTarget); if(!pendingWebviewResume && !activeWebviewSeek)refreshSteadyMonitorAfterCompletedSeek(event.currentTarget); const source=preview; if(source){const owner=webviewStallOwner(event.currentTarget,source,seekSerial);if(owner)webviewStallWatchdog.observeProgress(owner,event.currentTarget.currentTime);} bufferingAudio = Boolean(pendingWebviewResume && pendingWebviewResume.node === event.currentTarget && pendingWebviewResume.source === preview); void recordSuccessfulPlayback(); prepareUpcomingWebviewCandidate(event.currentTarget); }}
        onended={(event) => { if(event.currentTarget!==audio)return; void finishWebviewObservedCapture(event.currentTarget); webviewSeekCompletionWatchdog.cancel(); webviewPostSeekProgress.cancel(); pendingPostSeekProgress=null; activeWebviewSeek=null; webviewBufferingRecovery.reset(); webviewStallWatchdog.reset(); recordWebviewMediaEvent("ended",event.currentTarget); if(playingRadioStation)recoverOwnedAudioFailure(event.currentTarget,"Radio stream ended; reconnecting to the same station.");else completeWebviewQueueOccurrence(false, event.currentTarget); }}
        onerror={(event) => { if(event.currentTarget!==audio)return; if(webviewObservedCapture?.node===event.currentTarget)cancelWebviewObservedCapture("Whole-track WebView analysis was discarded because the stream failed."); webviewBufferingRecovery.reset(); recordWebviewMediaEvent("error",event.currentTarget); recoverAudioFailure(event); }}
      >
      </audio>{:else if preview?.kind === "external"}<div class="embed-player">
        <p class="external-notice">
          In-app preview unavailable for this recording.
        </p>
        <button
          onclick={() => {
            const track=playingTrack, url=preview?.url;
            if(track && url)void run(() => openTrackSource(track,url));
          }}
          >Open provider page</button
        >
        <button
          onclick={() => {
            nextPreviewRequest();
            preview = null;
            paused = true;
          }}>Close</button
        >
      </div>{:else if preview?.sessionId}<div class="embed-player">
        <p class="external-notice">Playing in the provider preview window. Use its Play button if playback is blocked.</p>
        <button onclick={() => void closePreview()}>Close player</button>
      </div>{:else if preview && preview.kind !== "native"}<div class="embed-player">
        <p class="external-notice">This preview source is unavailable in this player.</p>
        <button onclick={() => void closePreview()}>Close player</button>
      </div>{/if}
  </footer>
</div>

{#if localAlbumLoudnessReview}
  <LocalAlbumLoudnessReview
    light={settings.theme === "light" || settings.theme === "system" && systemLight}
    tracks={localAlbumLoudnessReview.tracks}
    album={localAlbumLoudnessReview.album}
    albumArtist={localAlbumLoudnessReview.albumArtist}
    canReorder={localAlbumLoudnessReview.canReorder}
    focusReturn={localAlbumLoudnessFocusReturn}
    onAnalyze={(orderedTracks) => prepareReleaseLoudnessAnalysis(orderedTracks, 0, true)}
    onPlay={(orderedTracks, resource) => playReleaseAlbum(orderedTracks, resource)}
    onQueue={(orderedTracks, resource) => queueArtistTracks(orderedTracks, "append", resource)}
    onClose={() => { localAlbumLoudnessReview = null; localAlbumLoudnessFocusReturn = null; }}
  />
{/if}


<LibraryCleanup open={showLibraryCleanup} {call} onClose={() => showLibraryCleanup=false} onRemoved={() => requestLocalPrefixRefresh(undefined,true)} />
