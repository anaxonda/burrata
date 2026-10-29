<script lang="ts">
  import { getContext } from "svelte";
  import { SOURCE_OPENER, type SourceOpener } from "./lib/source-opening";
  import { liveBpmLabel } from "./lib/live-bpm";
  import { metadataCount } from "./lib/provider-counts";
  import { call } from "./lib/api";
  import PlaybackQuality from "./PlaybackQuality.svelte";
  import ArtistInformation from "./ArtistInformation.svelte";
  import RecordingInformation from "./RecordingInformation.svelte";
  import type {RecordingInformationResult, RecordingReleaseInfo} from "./lib/recording-information";
  import type {ArtistProfile, ArtistRef} from "./lib/artist-types";
  import TrackFiles from "./TrackFiles.svelte";
  import PlayingTrackDetections from "./PlayingTrackDetections.svelte";
  import type { PlayingTrackDetectionView } from "./lib/playing-track-detection";
  import { detectedObservationTrack } from "./lib/detected-track";
import { trackSource } from "./lib/source-link";
  import StationInfo from "./StationInfo.svelte";
  import type { Track, Candidate, LocalAsset, LiveBpmEstimate, LiveKeyEstimate } from "./lib/types";
  import type { TrackLibrarySummary } from "./lib/track-library-summary";
  import { loudnessMeasurementSourceLabel, type LoudnessAnalysisJobStatus, type LoudnessPlaybackStatus } from "./lib/loudness-normalization";

  function estimateNote(track: Track, field: "bpm" | "key"): string {
    const provenance = (track.provenance ?? {}) as Record<string, unknown>;
    if ((provenance.manualFields as Record<string, unknown> | undefined)?.[field] === true) return "";
    const analysis = provenance[field === "bpm" ? "bpmAnalysis" : "keyAnalysis"] as Record<string, unknown> | undefined;
    if (!analysis || typeof analysis !== "object") return "";
    const value = field === "bpm" ? analysis.bpm ?? analysis.estimatedBpm : analysis.key ?? analysis.estimatedKey ?? analysis.wholeTrackKey;
    const matches = field === "bpm" ? typeof value === "number" && track.bpm != null && Math.abs(value - track.bpm) <= 0.1
      : typeof value === "string" && track.key != null && value.trim().toLowerCase() === track.key.trim().toLowerCase();
    if (!matches || /failed|error|unavailable|no[_ -](?:confident[_ -])?estimate/i.test(String(analysis.status ?? ""))) return "";
    return ` · estimated${field === "bpm" && typeof analysis.relatedTempoAlternativeBpm === "number" ? ` · alternative ${analysis.relatedTempoAlternativeBpm} BPM` : ""}`;
  }

  let {
    track,
    candidate,
    artworkDisplayUrl,
    onArtworkError,
    onClose,
    onArtist,
    onArtistRef,
    loadArtist,
    loadRecording,
    onRecordingRelease,
    onAlbum,
    onDownload,
    onUpgrade = onDownload,
    asset = null,
    streamQuality = null,
    availableFiles = [],
    preferredAssetId = asset?.id ?? null,
    onPreferredFile,
    onPlaylist,
    onRadio,
    radioBusy = false,
    docked = false,
    activeChapter = "",
    onPlay,
    onMoreInfo,
    onOpenMpv, onIdentifyRadio, onCancelIdentifyRadio, identifyingRadio = false, identifyProgress = 0,
    onFavorite,
    favorite = track.favorite,
    panelWidth = 340,
    onPanelWidthChange,
    stationName = "",
    liveBpm = null,
    liveKey = null,
    liveBpmStatus,
    liveBpmStatusReason = null,
    lastLiveBpm = null, lastLiveKey = null,
    analysisEstimate = null,
    analysisStatus = "",
    loudnessStatus = null, loudnessMode = null, loudnessAnalysisJob = null, pendingLoudnessAnalysisCount = 0, loudnessAnalysisMessage = "",
    trackLibrarySummary = null, trackLibrarySummaryLoading = false, trackLibrarySummaryRequested = false,
    onHistoryOccurrence, onPlaylistMembership,
    trackDetectionView = null, trackDetectionStatus = "", trackDetectionDisabledReason = "",
    trackDetectionAvailable = false, onTrackDetectionToggle, onTrackDetectionSeek, onTrackDetectionOpen, onTrackDetectionAddToPlaylist, onTrackDetectionsAddToPlaylist,
    trackDetectionMarkersEnabled = false, onTrackDetectionMarkersToggle,
    onAnalyzeLoudness, onCancelLoudnessAnalysis, onRetryLoudnessAnalysis,
  }: {
    track: Track;
    candidate?: Candidate | null;
    artworkDisplayUrl?: string;
    onArtworkError?: (track: Track, failedUrl: string) => void;
    onClose: () => void;
    onArtist: () => void;
    onArtistRef?: (artist: ArtistRef) => void;
    loadArtist?: (track: Track) => Promise<ArtistProfile | null>;
    loadRecording?: (track: Track) => Promise<RecordingInformationResult>;
    onRecordingRelease?: (release:RecordingReleaseInfo, artist:ArtistRef|null) => void;
    onAlbum?: () => void;
    onDownload: () => void;
    onUpgrade?: () => void;
    asset?: LocalAsset | null;
    streamQuality?: { codec?: string; bitrateKbps?: number; bitrateOrigin?: string; sampleRate?: number; channels?: number; bitDepth?: number; live?: boolean } | null;
    availableFiles?: LocalAsset[];
    preferredAssetId?: string | null;
    onPreferredFile?: (assetId: string) => void;
    onPlaylist: () => void;
    onTrackDetectionOpen?: (track: Track) => void;
    onTrackDetectionAddToPlaylist?: (track: Track) => void;
    onTrackDetectionsAddToPlaylist?: (tracks: Track[]) => void;
    onRadio?: () => void;
    radioBusy?: boolean;
    docked?: boolean;
    activeChapter?: string;
    onPlay?: () => void;
    onMoreInfo?: () => void;
    onOpenMpv?: () => void; onIdentifyRadio?: () => void; onCancelIdentifyRadio?: () => void; identifyingRadio?: boolean; identifyProgress?: number;
    onFavorite?: () => void;
    favorite?: boolean;
    panelWidth?: number;
    onPanelWidthChange?: (width: number) => void;
    stationName?: string;
    liveBpm?: LiveBpmEstimate | null;
    liveKey?: LiveKeyEstimate | null;
    liveBpmStatus?: "disabled" | "unsupported" | "paused" | "warming" | "estimating" | "insufficient" | "unavailable" | "waiting";
    liveBpmStatusReason?: string | null;
    lastLiveBpm?: { value: number; measuredAt: string; positionSeconds: number } | null;
    lastLiveKey?: { value: string; measuredAt: string; positionSeconds: number } | null;
    analysisEstimate?: { bpm?: { bpm: number } | null; key?: { key: string } | null } | null;
    analysisStatus?: string;
    loudnessStatus?: LoudnessPlaybackStatus | null;
    loudnessMode?: "off" | "track" | "album" | null;
    loudnessAnalysisJob?: LoudnessAnalysisJobStatus | null;
    pendingLoudnessAnalysisCount?: number;
    loudnessAnalysisMessage?: string;
    trackLibrarySummary?: TrackLibrarySummary | null;
    trackLibrarySummaryLoading?: boolean;
    trackLibrarySummaryRequested?: boolean;
    onHistoryOccurrence?: (sessionId: string) => void;
    onPlaylistMembership?: (playlistId: string, trackId: string) => void;
    trackDetectionView?: PlayingTrackDetectionView | null;
    trackDetectionStatus?: string;
    trackDetectionDisabledReason?: string;
    trackDetectionAvailable?: boolean;
    onTrackDetectionToggle?: (enabled: boolean) => void;
    trackDetectionMarkersEnabled?: boolean;
    onTrackDetectionMarkersToggle?: (enabled: boolean) => void;
    onTrackDetectionSeek?: (seconds: number) => void;
    onAnalyzeLoudness?: () => void;
    onCancelLoudnessAnalysis?: () => void;
    onRetryLoudnessAnalysis?: () => void;
  } = $props();

  function loudnessStatusText(): string {
    if (!loudnessStatus) return "Playback status is unavailable for this recording.";
    if (loudnessStatus.mode === "off" || loudnessStatus.status === "off") return "Off · playback levels are unchanged.";
    if (loudnessStatus.status === "unavailable") return loudnessStatus.reason || "No trusted loudness measurement is available.";
    const source = loudnessMeasurementSourceLabel(loudnessStatus.measurementSource);
    const gain = Number.isFinite(loudnessStatus.appliedGainDb) ? `${loudnessStatus.appliedGainDb > 0 ? "+" : ""}${loudnessStatus.appliedGainDb.toFixed(1)} dB` : "0.0 dB";
    return `${loudnessStatus.status === "album_fallback_track" ? "Track fallback" : "Applied"} · ${gain} · ${source}${loudnessStatus.reason ? ` · ${loudnessStatus.reason}` : ""}`;
  }

  function analysisStatusText(): string {
    const status = analysisStatus.trim().toLowerCase();
    if (!status) return "";
    if (/failed|error|unavailable|unsupported|could not/.test(status)) return "Audio analysis is unavailable for this source.";
    if (/\b(?:queued|running|analyzing|analysing|starting|checking)\b/.test(status)) return "Analyzing this audio for BPM and key…";
    if (/complete|finished|cached|done/.test(status)) return "No estimate was detected for this audio.";
    if (/paused|disabled|background analysis is off/.test(status)) return "Not analyzed.";
    return "Audio analysis status: " + analysisStatus.trim().slice(0, 120);
  }

  const sourceOpener = getContext<SourceOpener | undefined>(SOURCE_OPENER);
  const sourcePage = $derived(!candidate?.url ? trackSource(track) : null);
  let sourceOpenError = $state("");
  async function openAudioSource(event: MouseEvent, url: string) {
    event.preventDefault();
    sourceOpenError = "";
    try {
      await (sourceOpener ? sourceOpener(track, url) : call("open_external_url", { url }));
    } catch (error) {
      sourceOpenError = `Could not open the audio source: ${String(error)}`;
    }
  }
  const visibleTrackDetections = $derived.by(() => {
    const observations = trackDetectionView?.observations ?? [];
    const range = track.playbackRange;
    return range ? observations.filter(item => item.detectedAtSeconds >= range.startTime && item.detectedAtSeconds < range.endTime) : observations;
  });

  const catalogProvider = $derived.by(() => {
    if (track.provider !== "catalog") return track.provider;
    const provenance = (track.provenance ?? {}) as Record<string, unknown>;
    return typeof provenance.catalogProvider === "string" ? provenance.catalogProvider : "";
  });
  function providerLabel(provider: string): string {
    return ({ musicbrainz: "MusicBrainz", discogs: "Discogs", spotify: "Spotify", youtube: "YouTube", soundcloud: "SoundCloud", monochrome: "Monochrome", soulseek: "Soulseek", slskd: "Soulseek" } as Record<string, string>)[provider] || provider;
  }
  let resizing = $state(false);
  function resizePanel(event: PointerEvent) {
    if (!docked || event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = panelWidth;
    resizing = true;
    const move = (next: PointerEvent) => onPanelWidthChange?.(Math.max(280, Math.min(420, startWidth + startX - next.clientX)));
    const end = () => { resizing = false; window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", end); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end, { once: true });
  }
  let imageFailed = $state(false);
  let imageUrl = $state("");
  const originalCoverUrl = $derived(track.artworkUrl || candidate?.artworkUrl || "");
  const coverUrl = $derived(artworkDisplayUrl || originalCoverUrl);
  function safeHttpUrl(value: unknown): string {
    if (typeof value !== "string") return "";
    try {
      const url = new URL(value);
      return (url.protocol === "http:" || url.protocol === "https:") && !url.username && !url.password ? url.href : "";
    } catch { return ""; }
  }
  const radioLinks = $derived.by(() => {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const streamUrl = safeHttpUrl(provenance.radioStreamUrl);
    const isRadio = provenance.sourceKind === "radio" || Boolean(streamUrl);
    return { isRadio, streamUrl, stationUrl: isRadio ? safeHttpUrl(track.sourceUrl) : "" };
  });
  const displayedStationName = $derived.by(() => {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    return stationName.trim() || (typeof provenance.radioStationName === "string" ? provenance.radioStationName : "") || track.title;
  });
  const views = $derived(metadataCount(track,candidate,["views", "viewCount", "view_count"]));
  const plays = $derived(metadataCount(track,candidate,["plays", "playCount", "play_count", "playcount"]));
  const formatCount = (value: number) => new Intl.NumberFormat().format(value);
  const currentLiveBpm = $derived(liveBpm?.trackId === track.id ? liveBpm : null);
  const currentLiveKey = $derived(liveKey?.trackId === track.id ? liveKey : null);
  const currentLiveStatus = $derived(
    liveBpmStatus && (!liveBpm || liveBpm.trackId === track.id) && (!liveKey || liveKey.trackId === track.id)
      ? liveBpmStatus
      : undefined,
  );
  function liveKeyStatusText(): string {
    if (!currentLiveStatus || currentLiveStatus === "disabled") return "";
    if (currentLiveBpm && !currentLiveKey) return "No confident key estimate yet";
    return ({
      unsupported: "Live key is unavailable for this audio route",
      paused: "Paused",
      warming: "Listening for audio…",
      estimating: "Estimating key…",
      insufficient: "No confident key estimate yet",
      unavailable: "Live key estimate temporarily unavailable",
      waiting: "Waiting for enough audio…",
    } as const)[currentLiveStatus] ?? "";
  }
  function lastMeasurementText(measuredAt: string, positionSeconds: number): string {
    const date = new Date(measuredAt);
    const when = Number.isFinite(date.getTime())
      ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date)
      : "previously";
    const seconds = Math.max(0, Math.floor(Number.isFinite(positionSeconds) ? positionSeconds : 0));
    const position = seconds >= 3600
      ? `${Math.floor(seconds / 3600)}:${String(Math.floor(seconds % 3600 / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`
      : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
    return `Last measured ${when} · around ${position}`;
  }
  const acceptedLastLiveBpm = $derived(lastLiveBpm && Number.isFinite(lastLiveBpm.value) && lastLiveBpm.value > 0 ? lastLiveBpm : null);
  const acceptedLastLiveKey = $derived(lastLiveKey && typeof lastLiveKey.value === "string" && lastLiveKey.value.trim() ? lastLiveKey : null);
  const librarySummary = $derived(trackLibrarySummary?.trackId === track.id ? trackLibrarySummary : null);
  const historyOccurrences = $derived.by(() => {
    if (!librarySummary) return [];
    if (librarySummary.playedOccurrences?.length) return librarySummary.playedOccurrences;
    return librarySummary.playedAt.map((playedAt, index) => ({ sessionId: "", playedAt, key: `legacy-${index}` }));
  });
  const playedAtLabel = (value: string) => {
    const date = new Date(value);
    return Number.isFinite(date.getTime())
      ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date)
      : value;
  };
  $effect(() => {
    if (imageUrl !== coverUrl) {
      imageUrl = coverUrl;
      imageFailed = false;
    }
  });

  function focusPanel(node: HTMLElement) {
    if (docked) return { destroy() {} };
    const previous = document.activeElement as HTMLElement | null;
    node.querySelector<HTMLButtonElement>(".close")?.focus();
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    node.addEventListener("keydown", onKeydown);
    return {
      destroy() {
        node.removeEventListener("keydown", onKeydown);
        if (previous?.isConnected) previous.focus();
      },
    };
  }
</script>

<div class="playing-info-backdrop" class:docked class:resizing style={`--info-panel-width:${Math.max(280, Math.min(420, panelWidth))}px`} role="presentation" onclick={(event) => { if (!docked && event.target === event.currentTarget) onClose(); }}>
  <aside
    use:focusPanel
    class="playing-info"
    aria-label="Playing track information"
    tabindex="-1"
  >
    {#if docked}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (Focusable window splitter provides keyboard resizing.) -->
    <div tabindex="0" class="panel-resize" role="separator" aria-label="Resize information panel" aria-orientation="vertical" aria-valuemin="280" aria-valuemax="420" aria-valuenow={Math.max(280, Math.min(420, panelWidth))} onpointerdown={resizePanel} onkeydown={(event) => { if (event.key === "ArrowLeft") { event.preventDefault(); onPanelWidthChange?.(Math.min(420, panelWidth + 10)); } else if (event.key === "ArrowRight") { event.preventDefault(); onPanelWidthChange?.(Math.max(280, panelWidth - 10)); } }}></div>{/if}
    <header>
      <div><span class="eyebrow">NOW PLAYING</span><strong>Track details</strong></div>
      <button class="close" aria-label="Close playing track information" onclick={onClose}>✕</button>
    </header>
    <div class="track-hero">
      {#if coverUrl && !imageFailed}
        {#key coverUrl}<img src={coverUrl} alt={"Artwork for " + track.title} onerror={(event) => {
          if (!event.currentTarget.isConnected) return;
          imageFailed = true;
          if (!artworkDisplayUrl && originalCoverUrl.startsWith("asset:")) onArtworkError?.(track, originalCoverUrl);
        }} />{/key}
      {:else}
        <div class="cover-placeholder" aria-label="No artwork available">♫</div>
      {/if}
      <div class="hero-copy">
      {#if candidate?.provider || track.provider}<p class="eyebrow">{candidate?.provider ?? track.provider} audio</p>{/if}
        <h2>{activeChapter || track.title}{!activeChapter && track.version ? " (" + track.version + ")" : ""}</h2>
      {#if track.artist}<button class="artist-link" onclick={onArtist}>{track.artist}</button>{:else}<p class="unknown-artist">Artist unknown</p>{/if}
        {#if track.album}{#if onAlbum}<button class="album album-link" onclick={onAlbum}>{track.album}</button>{:else}<p class="album">{track.album}</p>{/if}{/if}
      {#if activeChapter}<p class="chapter"><span>From</span><strong>{track.playbackRange?.parentTitle || track.title}</strong></p>{/if}
      </div>
    </div>

    {#if librarySummary}
      <section class="track-library-summary" aria-label="Library and listening activity">
        <dl>
          {#if librarySummary.inLibrary && !librarySummary.playlists.length}<div><dt>Library</dt><dd>In library</dd></div>{/if}
          {#if librarySummary.downloaded}<div><dt>Downloaded</dt><dd>Available on this device</dd></div>{/if}
        </dl>
        <div class="plays-disclosure-row">
          <details class="plays-disclosure">
            <summary><span>Plays</span><strong>{formatCount(librarySummary.playCount)}</strong><span class="plays-chevron" aria-hidden="true">⌄</span></summary>
            <div class="played-history-body">
              <p class="track-history-hint">Successful starts recorded in playback history; restored history may include other devices.</p>
              {#if historyOccurrences.length}
                {#if librarySummary.playedAtHasMore}
                  <p class="played-dates-note">Showing the {formatCount(historyOccurrences.length)} most recent of {formatCount(librarySummary.playCount)} recorded plays, newest first.</p>
                {:else}
                  <p class="played-dates-note">Dates are shown newest first.</p>
                {/if}
                <ol class="played-dates">
                  {#each historyOccurrences as occurrence, index (occurrence.sessionId || `${track.id}:${occurrence.playedAt}:${index}`)}
                    <li>
                      {#if occurrence.sessionId && onHistoryOccurrence}
                        <button type="button" class="history-occurrence" onclick={() => onHistoryOccurrence?.(occurrence.sessionId)}><time datetime={occurrence.playedAt}>{playedAtLabel(occurrence.playedAt)}</time></button>
                      {:else}
                        <time datetime={occurrence.playedAt}>{playedAtLabel(occurrence.playedAt)}</time>
                      {/if}
                    </li>
                  {/each}
                </ol>
              {:else}
                <p class="played-dates-note">No play dates are available.</p>
              {/if}
            </div>
          </details>
        </div>
        {#if librarySummary.playlists.length}
          <div class="playlist-membership">
            <strong>In playlists</strong>
          <ul>{#each librarySummary.playlists as playlist (playlist.id)}<li><button type="button" class="playlist-membership-link" onclick={() => onPlaylistMembership?.(playlist.id, track.id)}>{playlist.name}</button></li>{/each}</ul>
          </div>
        {/if}
      </section>
    {:else if trackLibrarySummaryLoading}
      <p class="track-library-loading" role="status">Loading listening and library details…</p>
    {:else if !trackLibrarySummaryRequested && (track.localAvailability === "available" || (!track.localAvailability && track.localPath))}
      <p class="downloaded-badge" aria-label="Downloaded and available on this device"><span aria-hidden="true">✓</span> Downloaded</p>
    {/if}

    {#if trackDetectionAvailable || visibleTrackDetections.length}
      <PlayingTrackDetections
        enabled={trackDetectionView?.enabled ?? false}
        observations={visibleTrackDetections}
        status={trackDetectionStatus}
        disabledReason={trackDetectionDisabledReason}
        retentionLimitReached={trackDetectionView?.retentionLimitReached ?? false}
        markersEnabled={trackDetectionMarkersEnabled}
        interactive={trackDetectionAvailable}
        onToggle={(enabled) => onTrackDetectionToggle?.(enabled)}
        onMarkersToggle={onTrackDetectionMarkersToggle}
        onSeek={(seconds) => onTrackDetectionSeek?.(seconds)}
        onOpenTrack={(observation) => onTrackDetectionOpen?.(detectedObservationTrack(observation))}
        onAddToPlaylist={(observation) => onTrackDetectionAddToPlaylist?.(detectedObservationTrack(observation))}
        onAddAllToPlaylist={(observations) => onTrackDetectionsAddToPlaylist?.(observations.map(detectedObservationTrack))}
      />
    {/if}

    <PlaybackQuality {track} {asset} {streamQuality} />
    {#if availableFiles.length}<TrackFiles assets={availableFiles} {preferredAssetId} onPreferred={onPreferredFile} compact />{/if}
    {#if loudnessMode !== "off" && loudnessStatus?.mode !== "off" && loudnessStatus?.status !== "off"}<section class="loudness-details" aria-label="Loudness normalization playback details">
      <strong>Loudness normalization</strong>
      <p>{loudnessStatusText()}</p>
      {#if onAnalyzeLoudness}<button type="button" onclick={onAnalyzeLoudness}>Analyze this local track</button>{/if}
      {#if loudnessAnalysisJob}<p>{loudnessAnalysisJob.status === "running" || loudnessAnalysisJob.status === "queued" ? "Analyzing " + loudnessAnalysisJob.completed + " of " + loudnessAnalysisJob.total + " selected tracks" : loudnessAnalysisJob.status === "failed" ? "Analysis failed: " + (loudnessAnalysisJob.error ?? "unknown error") : "Analysis " + loudnessAnalysisJob.status}</p>{#if loudnessAnalysisJob.status === "running" || loudnessAnalysisJob.status === "queued"}<progress max={loudnessAnalysisJob.total || 1} value={loudnessAnalysisJob.completed}></progress>{/if}{/if}
      {#if pendingLoudnessAnalysisCount}<p>{pendingLoudnessAnalysisCount} selected track(s) waiting for idle playback.</p>{/if}
      {#if loudnessAnalysisMessage}<p>{loudnessAnalysisMessage}</p>{/if}
      {#if onCancelLoudnessAnalysis && (loudnessAnalysisJob?.status === "running" || loudnessAnalysisJob?.status === "queued" || pendingLoudnessAnalysisCount)}<button type="button" onclick={onCancelLoudnessAnalysis}>Cancel loudness analysis</button>{/if}
      {#if onRetryLoudnessAnalysis && pendingLoudnessAnalysisCount && (!loudnessAnalysisJob || loudnessAnalysisJob.status === "failed" || loudnessAnalysisJob.status === "cancelled")}<button type="button" onclick={onRetryLoudnessAnalysis}>Retry selected analysis</button>{/if}
    </section>{/if}

    {#if radioLinks.isRadio && (radioLinks.stationUrl || radioLinks.streamUrl)}
      <StationInfo {track} stationName={displayedStationName} stationUrl={radioLinks.stationUrl} streamUrl={radioLinks.streamUrl} />
    {/if}

    <dl>
      {#if views !== null}<div><dt>Views</dt><dd>{formatCount(views)}</dd></div>{/if}
      {#if plays !== null}<div><dt>Source plays</dt><dd>{formatCount(plays)}</dd></div>{/if}
      {#if catalogProvider && catalogProvider !== "catalog"}<div><dt>Metadata source</dt><dd>{providerLabel(catalogProvider)}</dd></div>{/if}
      {#if sourcePage}<div><dt>Source</dt><dd><a href={sourcePage.url} target="_blank" rel="noopener noreferrer" onclick={event => void openAudioSource(event, sourcePage!.url)}>Open {sourcePage.label} ↗</a></dd></div>{/if}
      {#if candidate?.provider}<div><dt>Audio provider</dt><dd>{providerLabel(candidate.provider)}</dd></div>{/if}
      {#if candidate?.title}<div><dt>Audio source</dt><dd title={candidate.title}>{#if candidate.url && /^https:\/\//i.test(candidate.url)}<a href={candidate.url} target="_blank" rel="noopener noreferrer" onclick={event => void openAudioSource(event, candidate!.url!)}>{candidate.title} ↗</a>{:else}{candidate.title}{/if}</dd></div>{/if}
      {#if candidate?.uploader}<div><dt>Uploader</dt><dd title={candidate.uploader}>{candidate.uploader}</dd></div>{/if}
      {#if track.bpm}<div><dt>Tempo</dt><dd>{track.bpm} BPM{estimateNote(track, "bpm")}</dd></div>{:else if analysisEstimate?.bpm && Number.isFinite(analysisEstimate.bpm.bpm) && analysisEstimate.bpm.bpm > 0}<div><dt>Tempo</dt><dd>{analysisEstimate.bpm.bpm} BPM · estimated from audio</dd></div>{:else if analysisStatus}<div><dt>Tempo</dt><dd>{analysisStatusText()}</dd></div>{/if}
      {#if currentLiveBpm || acceptedLastLiveBpm}<div class="live-bpm" title={currentLiveBpm ? `Approximate tempo estimated from ${currentLiveBpm.windowSeconds.toFixed(1)} seconds of current playback` : undefined}><dt>{currentLiveBpm ? "Live BPM" : "Last BPM"}</dt><dd>{currentLiveBpm ? liveBpmLabel(currentLiveBpm) : `${acceptedLastLiveBpm!.value} BPM`}{#if acceptedLastLiveBpm}<small class="last-measurement">{lastMeasurementText(acceptedLastLiveBpm.measuredAt, acceptedLastLiveBpm.positionSeconds)}</small>{/if}</dd></div>{:else if currentLiveStatus}<div class="live-bpm" title={liveBpmStatusReason ?? undefined}><dt>Live BPM</dt><dd>{liveBpmStatusReason ? liveBpmStatusReason.slice(0, 160) : {disabled:"Disabled · enable in Settings → Playback",unsupported:"This source has no supported Web Audio PCM route",paused:"Paused",warming:"Listening for audio…",estimating:"Estimating BPM…",insufficient:"No live BPM reading yet",waiting:"Waiting for the current playback buffer…",unavailable:"Estimate temporarily unavailable"}[currentLiveStatus]}</dd></div>{/if}
      {#if currentLiveKey || acceptedLastLiveKey}<div class="live-bpm" title={currentLiveKey ? `Provisional key profile from ${currentLiveKey.windowSeconds.toFixed(1)} seconds of current playback. This does not change saved track metadata.` : undefined}><dt>{currentLiveKey ? "Live key" : "Last key"}</dt><dd>{currentLiveKey ? `${currentLiveKey.key} · recent window` : acceptedLastLiveKey!.value}{#if acceptedLastLiveKey}<small class="last-measurement">{lastMeasurementText(acceptedLastLiveKey.measuredAt, acceptedLastLiveKey.positionSeconds)}</small>{/if}</dd></div>{:else if liveKeyStatusText()}<div class="live-bpm live-key-status"><dt>Live key</dt><dd>{liveKeyStatusText()}</dd></div>{/if}
      {#if track.key}<div><dt>Key</dt><dd>{track.key}{estimateNote(track, "key")}</dd></div>{:else if analysisEstimate?.key?.key}<div><dt>Key</dt><dd>{analysisEstimate.key.key} · estimated from audio</dd></div>{:else if !currentLiveKey && !acceptedLastLiveKey}<div><dt>Key</dt><dd>{analysisStatus ? analysisStatusText() : "Not analyzed"}</dd></div>{/if}
      {#if track.label}<div><dt>Label</dt><dd>{track.label}</dd></div>{/if}
      {#if track.releaseDate}<div><dt>Released</dt><dd>{track.releaseDate}</dd></div>{/if}
      {#if track.durationSeconds}<div><dt>Duration</dt><dd>{Math.floor(track.durationSeconds / 60)}:{String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}</dd></div>{/if}
      {#if candidate?.provider === "monochrome" && candidate.formats[0]}
        <div><dt>Audio source</dt><dd>Monochrome · {candidate.formats[0].extension.toUpperCase()}{candidate.formats[0].lossless ? " (lossless)" : ""}</dd></div>
      {/if}
    </dl>

    {#if sourceOpenError}<p role="status">{sourceOpenError}</p>{/if}

    {#if loadArtist && track.artist && (!radioLinks.isRadio || track.provenance && (track.provenance as Record<string, unknown>).radioMetadata)}<ArtistInformation {track} {loadArtist} {onArtist} {onArtistRef} compact />{/if}
    {#if loadRecording && (!radioLinks.isRadio || track.provenance && (track.provenance as Record<string, unknown>).radioMetadata)}<RecordingInformation {track} {loadRecording} onRelease={onRecordingRelease} onArtist={onArtistRef} compact />{/if}
    <div class="actions">
      {#if onFavorite}<button aria-label={favorite ? "Remove from favorites" : "Add to favorites"} aria-pressed={favorite} onclick={onFavorite}>{favorite ? "♥ Favorited" : "♡ Add to favorites"}</button>{/if}
      {#if onPlay}<button class="primary" onclick={onPlay}>Play</button>{/if}
      {#if onOpenMpv}<button onclick={onOpenMpv}>Open in mpv</button>{/if}
      {#if onIdentifyRadio}<button disabled={identifyingRadio && !onCancelIdentifyRadio} onclick={identifyingRadio ? onCancelIdentifyRadio ?? onIdentifyRadio : onIdentifyRadio}>{identifyingRadio?onCancelIdentifyRadio?`Cancel capture · ${Math.floor(identifyProgress)} / 8 s`:"Identifying…":"Identify song"}</button>{/if}
      {#if onMoreInfo}<button onclick={onMoreInfo}>Search for more info</button>{/if}
      {#if track.localPath}<button onclick={() => void call("open_in_file_manager", { path: track.localPath })}>Show local file</button>{/if}
      <button onclick={onPlaylist}>Add to playlist</button>
      {#if onRadio}<button disabled={radioBusy} onclick={onRadio}>{radioBusy ? "Finding radio…" : "Start song radio"}</button>{/if}
      <button class="download" onclick={onDownload}>Download</button>
    </div>
  </aside>
</div>

<style>
  .playing-info-backdrop { position: fixed; z-index: 41; inset: 0; display: flex; justify-content: flex-end; padding: 12px 12px calc(var(--player-height, 92px) + 12px + env(safe-area-inset-bottom)); pointer-events: none; }
  .playing-info-backdrop.docked { position:relative; z-index:auto; inset:auto; display:flex; justify-content:flex-end; min-width:0; min-height:0; height:100%; padding:0; pointer-events:auto; }
  .playing-info { display: flex; flex-direction: column; position: relative; width: min(510px, calc(100vw - 24px)); min-width:0; min-height: 0; max-height: 100%; margin: 0; padding: 18px; overflow: auto; overscroll-behavior: contain; border: 1px solid var(--line); border-radius: 16px; background: var(--surface); color: var(--text); box-shadow: 0 18px 58px #0008; pointer-events: auto; }
  .docked .playing-info { width:var(--info-panel-width,340px); max-width:none; border-radius:0; border-block:0; box-shadow:none; }
  .panel-resize { position:absolute; z-index:2; left:0; top:0; width:8px; height:100%; padding:0; border:0; background:transparent; cursor:col-resize; touch-action:none; }
  .panel-resize:hover,.panel-resize:focus-visible { background:color-mix(in srgb,var(--accent) 35%,transparent); outline:0; }
  .resizing { user-select:none; }
  header { display: flex; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 12px; padding-bottom: 14px; }
  header > div { display: grid; gap: 4px; }
  header strong { font-size: 13px; font-weight: 580; }
  .eyebrow { margin: 0; color: var(--muted); font-size: 9px; font-weight: 650; letter-spacing: .12em; }
  .close { display: grid; place-items: center; width: 40px; height: 40px; flex: 0 0 40px; padding: 0; border-radius: 9px; background: transparent; color: var(--muted); touch-action: manipulation; }
  .track-hero { display: grid; grid-template-columns: 132px minmax(0, 1fr); align-items: center; gap: 16px; }
  img, .cover-placeholder { display: grid; width: 132px; aspect-ratio: 1; object-fit: cover; border-radius: 11px; box-shadow: 0 8px 24px #0004; }
  .cover-placeholder { place-items: center; background: var(--raised); color: var(--muted); font-size: 54px; }
  .hero-copy { min-width: 0; }
  .hero-copy .eyebrow { margin-bottom: 6px; }
  h2 { margin: 0 0 6px; overflow-wrap: anywhere; font-size: clamp(19px, 2vw, 26px); line-height: 1.15; letter-spacing: -.025em; }
  .artist-link { display: block; width: 100%; min-width: 0; max-width: 100%; padding: 2px 0; overflow: visible; overflow-wrap: anywhere; white-space: normal; text-overflow: clip; background: transparent; color: var(--accent); text-align: left; font-size: 15px; }
  .unknown-artist { margin:2px 0; color:var(--muted); font-size:13px; }
  .chapter { display:grid; gap:3px; margin:9px 0 0; font-size:11px; }.chapter span { color:var(--muted); }.chapter strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-weight:550; }
  .album-link{background:transparent;border:0;padding:0;text-align:left;text-decoration:underline;cursor:pointer}
  .album { margin: 5px 0 0; overflow-wrap: anywhere; color: var(--muted); font-size: 12px; }
  .downloaded-badge { display:flex;align-items:center;gap:6px;align-self:flex-start;margin:14px 0 0;padding:5px 9px;border:1px solid color-mix(in srgb,var(--accent) 55%,var(--line));border-radius:999px;color:var(--accent);font-size:11px;font-weight:650; }
  .downloaded-badge span { display:grid;place-items:center;width:16px;height:16px;border-radius:50%;background:color-mix(in srgb,var(--accent) 20%,transparent);font-size:11px; }
  .track-library-summary{display:grid;gap:8px;margin:12px 0 0;padding:10px 12px;border:1px solid var(--line);border-radius:10px;background:var(--raised)}
  .track-library-summary > dl{display:grid;grid-template-columns:minmax(0,1fr);gap:0;margin:0}
  .track-library-summary > dl>div{align-items:baseline;padding:5px 0}
  .track-library-summary dt,.track-library-summary dd{font-size:11px}
  .plays-disclosure-row{padding:0 0 5px;border-bottom:1px solid var(--line)}
  .plays-disclosure{width:100%}
  .plays-disclosure summary{display:grid;grid-template-columns:minmax(0,1fr) auto auto;align-items:center;gap:9px;min-height:32px;cursor:pointer;list-style:none;color:var(--text);font-size:11px}
  .plays-disclosure summary::-webkit-details-marker{display:none}
  .plays-disclosure summary strong{font-size:11px;font-weight:650}
  .plays-chevron{display:inline-block;color:var(--muted);font-size:15px;line-height:1;transition:transform .15s ease}
  .plays-disclosure[open] .plays-chevron{transform:rotate(180deg)}
  .played-history-body{display:grid;gap:5px;margin:2px 0 6px}
  .track-history-hint{margin:0;color:var(--muted);font-size:10px;line-height:1.4}
  .played-dates-note{margin:3px 0 0;color:var(--muted);font-size:10px}
  .played-dates,.playlist-membership ul{display:grid;gap:5px;margin:4px 0 0;padding-left:20px;color:var(--text);font-size:11px}
  .played-dates{max-height:150px;overflow:auto}
  .played-dates time{color:var(--muted)}
  .playlist-membership strong{font-size:11px}
  .playlist-membership ul{padding-left:16px;overflow-wrap:anywhere}
  .history-occurrence,.playlist-membership-link{min-height:28px;padding:2px 4px;border:0;background:transparent;color:var(--accent);text-align:left;text-decoration:underline;cursor:pointer}
  .history-occurrence:focus-visible,.playlist-membership-link:focus-visible{outline:2px solid var(--accent);outline-offset:2px;border-radius:3px}
  .track-library-loading{margin:12px 0 0;color:var(--muted);font-size:11px}
  .loudness-details{display:grid;gap:5px;margin-top:12px;padding:9px;border:1px solid var(--line);border-radius:8px}.loudness-details strong{font-size:12px}.loudness-details p{margin:0;color:var(--muted);font-size:11px;line-height:1.4}.loudness-details progress{width:100%}.loudness-details button{justify-self:start}
  dl { display: grid; grid-template-columns: 1fr 1fr; gap: 0 20px; margin: 18px 0 0; }
  dl > div { display: flex; justify-content: space-between; gap: 10px; min-width: 0; padding: 8px 0; border-bottom: 1px solid var(--line); }
  dt { flex: 0 0 auto; color: var(--muted); font-size: 11px; }
  dd { min-width: 0; margin: 0; overflow: hidden; text-align: right; text-overflow: ellipsis; white-space: nowrap; font-size: 11px; }
  .last-measurement { display:block; color:var(--muted); font-size:9px; font-weight:400; white-space:normal; }
  .actions { display: flex; flex-wrap: wrap; align-self: stretch; width: 100%; min-width: 0; max-width: 100%; gap: 8px; margin-top: auto; padding-top: 17px; }
  .actions button { min-width: 0; max-width: 100%; min-height: 40px; padding: 8px 12px; overflow-wrap: anywhere; white-space: normal; touch-action:manipulation; }
  .playing-info button:focus-visible { outline:2px solid var(--accent);outline-offset:2px; }
  @media (max-width: 1199px) {
    .playing-info-backdrop { padding: 8px 8px calc(var(--player-height, 100px) + 8px + env(safe-area-inset-bottom)); }
    .playing-info { width: 100%; padding: 14px; border-radius: 15px; }
    .docked .playing-info { width:100%; }
    .track-hero { grid-template-columns: 96px minmax(0, 1fr); gap: 12px; }
    img, .cover-placeholder { width: 96px; }
    .cover-placeholder { font-size: 40px; }
    h2 { font-size: 19px; }
    .artist-link { font-size: 13px; }
    dl { grid-template-columns: 1fr; margin-top: 14px; }
  }
  @media (min-width:1200px) { .docked .playing-info { width:var(--info-panel-width,340px); padding:16px; }.docked .track-hero { grid-template-columns:minmax(72px,104px) minmax(0,1fr); gap:12px; }.docked img,.docked .cover-placeholder { width:100%; }.docked dl { grid-template-columns:1fr; }.docked .actions { margin-top:12px; } }
</style>
