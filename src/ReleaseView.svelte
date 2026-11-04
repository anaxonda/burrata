<script lang="ts">
  import { territoryLabel } from "./lib/territory-label";
  let downloadAvailability = $state<"all"|"downloaded"|"not-downloaded">("all");
  import {tick} from "svelte";
  import type { ComponentProps } from "svelte";
  import AlbumDetailView from "./AlbumDetailView.svelte";
  import type { AlbumDetailMetadata } from "./lib/album-detail-view";
  import { trackSortAvailability, trackSortOptionLabel } from "./lib/track-sort-availability";
  import type { ArtistReleaseRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";
  import type { LoudnessResourceIdentity } from "./lib/loudness-normalization";
  import TrackList from "./TrackList.svelte";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";

  let {
    release, tracks = [], loading = false, error = "", onPlay = () => {}, onQueue = () => {}, onQueueAlbum,
    onDownload = () => {}, onDownloadAlbum, onSearchAlbumSoulseek, soulseekSearchAvailable = false, onAnalyzeLoudness, onPlayAlbum, onAddAlbumToPlaylist, onSaveToLibrary, onSaveCompleteAlbum, onArtist, onContext, onPlaylist, onAddSelectedToPlaylist, onFavorite,
    playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false,
  }: {
    release: ArtistReleaseRef & { artworkUrl?: string | null };
    tracks?: Track[]; loading?: boolean; error?: string;
    onPlay?: (track: Track, tracks: Track[], index: number) => void;
    onQueue?: (tracks: Track[], mode: "next" | "append") => void; onQueueAlbum?: (tracks: Track[], resource: LoudnessResourceIdentity | null) => void; onDownload?: (track: Track) => void; onDownloadAlbum?: (tracks: Track[], onReceipt?: (receipt: unknown) => void | Promise<void>) => unknown | Promise<unknown>; onSearchAlbumSoulseek?: () => unknown | Promise<unknown>; soulseekSearchAvailable?: boolean; onAnalyzeLoudness?: (tracks: Track[], gapSeconds: number) => Promise<{ jobId: string; playbackTracks: Track[] }>; onPlayAlbum?: (tracks: Track[], resource: LoudnessResourceIdentity | null) => void | Promise<void>; onAddAlbumToPlaylist?: (tracks: Track[], title: string) => void | Promise<void>; onSaveToLibrary?: (tracks: Track[]) => Promise<void>;
    onSaveCompleteAlbum?: (tracks: Track[], release: ArtistReleaseRef) => Promise<void>;
    onArtist?: (track: Track) => void; onContext?: (track: Track, event: MouseEvent) => void; onPlaylist?: (track: Track) => void; onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>; onFavorite?: (track: Track) => void;
    playingTrack?: Track | null; bufferingTrack?: Track | null;
    playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
  } = $props();

  let releaseElement: HTMLElement | null = null;
  export function captureNavigationState() { return {filter,sort,sortDirection,minDuration,maxDuration,scroll:releaseElement?.scrollTop??0}; }
  export async function restoreNavigationState(state:ReturnType<typeof captureNavigationState>) {
    ({filter,sort,sortDirection,minDuration,maxDuration}=state);
    await tick();
    if(releaseElement)releaseElement.scrollTop=state.scroll;
  }
  let filter = $state("");
  let sort = $state<TrackSortField>("sourceOrder");
  let sortDirection = $state<SortDirection>("asc");
  let minDuration = $state("");
  let maxDuration = $state("");
  let savingToLibrary = $state(false);
  let saveLibraryError = $state("");
  let saveLibraryNotice = $state("");
  let activeSaveReleaseIdentity = "";
  let saveReleaseGeneration = 0;
  const saveReleaseIdentity = $derived(`${release.release.provider}:${release.release.id}:${release.sourceUrl}`);
  function syncSaveReleaseIdentity(identity: string) {
    if (identity !== activeSaveReleaseIdentity) {
      activeSaveReleaseIdentity = identity;
      saveReleaseGeneration++;
      savingToLibrary = false; saveLibraryError = ""; saveLibraryNotice = "";
    }
  }
  $effect(() => syncSaveReleaseIdentity(saveReleaseIdentity));
  function trackNumber(track: Track, index: number): number {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const raw = provenance.position ?? provenance.trackNumber ?? provenance.track_number;
    if (typeof raw === "number") return raw;
    if (typeof raw === "string") {
      const found = raw.match(/(?:^|\D)(\d+)\s*$/);
      if (found) return Number(found[1]);
    }
    return index + 1;
  }
  const albumOccurrences = $derived(tracks.map((track, sourceIndex) => ({ track, sourceIndex })).sort((a, b) => trackNumber(a.track, a.sourceIndex) - trackNumber(b.track, b.sourceIndex) || a.sourceIndex - b.sourceIndex));
  const albumTracks = $derived(albumOccurrences.map(row => row.track));
  function loadedAlbumSnapshot() {
    const identity = `${release.release.provider}:${release.release.id}`;
    const source = `${identity}\n${release.sourceUrl}\n${albumOccurrences.map(row => `${row.sourceIndex}:${row.track.id}`).join("\n")}`;
    let hash = 2166136261;
    for (let index = 0; index < source.length; index++) hash = Math.imul(hash ^ source.charCodeAt(index), 16777619);
    return { identity, revision: `${albumOccurrences.length}:${(hash >>> 0).toString(16).padStart(8, "0")}`, trackCount: albumOccurrences.length, loaded: !loading && albumOccurrences.length > 0 };
  }
  export function captureLoadedAlbumSnapshot() { return loadedAlbumSnapshot(); }
  export function downloadLoadedAlbum(expectedIdentity: string, expectedRevision: string, onReceipt?: (receipt: unknown) => void | Promise<void>) {
    const snapshot = loadedAlbumSnapshot();
    if (!snapshot.loaded || snapshot.identity !== expectedIdentity || snapshot.revision !== expectedRevision || !onDownloadAlbum) {
      throw new Error("The loaded catalog release changed; navigate to it again before downloading.");
    }
    if (!onDownloadAlbum || loading || !albumTracks.length) throw new Error("The loaded catalog release is not ready to download.");
    return onDownloadAlbum(albumTracks,onReceipt);
  }
  const saveAlbumButtonLabel = $derived(
    release.trackCount != null && albumTracks.length > 0 && albumTracks.length === release.trackCount
      ? "Save album to Library"
      : `Save loaded tracks (${albumTracks.length}${release.trackCount != null && albumTracks.length < release.trackCount ? ` of ${release.trackCount}` : ""}) to Library`,
  );
  const sortAvailability = $derived(trackSortAvailability(albumTracks));
  const visibleOccurrences = $derived.by(() => {
    const lower = parseDurationBound(minDuration), upper = parseDurationBound(maxDuration);
    // Give each occurrence a unique sort object, so a repeated reference still
    // carries its original source index through descending and other sorts.
    const bySortObject = new Map<Track, (typeof albumOccurrences)[number]>();
    const sortable = albumOccurrences.map(row => {
      const sortObject = { ...row.track };
      bySortObject.set(sortObject, row);
      return sortObject;
    });
    const filtered = filterTracks(sortable, {
      availability: downloadAvailability,
      query: filter,
      ...(lower === null ? {} : { minDurationSeconds: lower }),
      ...(upper === null ? {} : { maxDurationSeconds: upper }),
    });
    return sortTracks(filtered, sort, sortDirection).map(row => bySortObject.get(row)!);
  });
  const visibleTracks = $derived(visibleOccurrences.map(row => row.track));
  const artists = $derived([...new Set(release.credits.map(credit => credit.name).filter(Boolean))]);
  const cover = $derived(release.artworkUrl || tracks.find(track => track.artworkUrl)?.artworkUrl || null);
  const detailMetadata = $derived<AlbumDetailMetadata>({
    title: release.title,
    artist: artists.join(" · "),
    artistNames: artists,
    year: release.year ?? release.releaseDate?.slice(0, 4) ?? null,
    artworkUrl: cover,
    sourceLabel: release.primaryType ?? release.format ?? "Release",
    sourceUrl: release.sourceUrl,
    scopeLabel: release.label || release.country ? [release.label, release.country ? territoryLabel(release.country) : ""].filter(Boolean).join(" · ") : null,
    trackCountLabel: release.trackCount != null && albumTracks.length < release.trackCount ? `${albumTracks.length} loaded of ${release.trackCount} tracks` : `${albumTracks.length} tracks loaded`,
  });
  const sortOptions = $derived([
    { value: "sourceOrder", label: "Track number" },
    ...(["title", "artist", "album", "duration", "releaseDate", "dateAdded", "bpm", "key", "plays", "views"] as TrackSortField[]).map(field => ({ value: field, label: trackSortOptionLabel(sortAvailability, field, field === "releaseDate" ? "Release date" : field === "dateAdded" ? "Date added" : field === "bpm" ? "BPM" : field === "key" ? "Key" : field[0].toUpperCase() + field.slice(1)), disabled: !sortAvailability[field] })),
  ]);
  const availabilityOptions = [
    { value: "all", label: "All tracks" }, { value: "downloaded", label: "Downloaded" }, { value: "not-downloaded", label: "Not downloaded" },
  ];
  const trackListProps: Omit<ComponentProps<typeof TrackList>, "presentation"> = $derived({
    tracks: visibleTracks,
    selectionContext: `release:${release.sourceUrl}`,
    selectionKeys: visibleOccurrences.map(row => String(row.sourceIndex)),
    selectionTracks: albumTracks,
    selectionTrackKeys: albumOccurrences.map(row => String(row.sourceIndex)),
    listLabel: "Release tracks",
    position: (track, index) => trackNumber(track, visibleOccurrences[index]?.sourceIndex ?? index),
    onPlay: (track, list, index) => onPlay(track, list, index),
    onDownload,
    onFavorite,
    onNext: track => onQueue([track], "next"),
    onQueue: track => onQueue([track], "append"),
    onQueueSelected: onQueue ? items => onQueue(items, "append") : undefined,
    onPlaylist,
    onAddSelectedToPlaylist,
    onArtist,
    onContext,
    playingTrack,
    bufferingTrack,
    playingTrackId,
    bufferingTrackId,
    paused,
  });
  const analysisIdentity = $derived.by(() => {
    const snapshot = loadedAlbumSnapshot();
    return `${snapshot.identity}:${snapshot.revision}`;
  });
  function playAlbumWithResource(selectedTracks: Track[], resource: LoudnessResourceIdentity | null) {
    if (!selectedTracks.length) return;
    if (onPlayAlbum) return onPlayAlbum(selectedTracks, resource);
    return onPlay(selectedTracks[0], selectedTracks, 0);
  }
  function queueAlbumWithResource(selectedTracks: Track[], resource: LoudnessResourceIdentity | null) {
    if (!selectedTracks.length) return;
    if (onQueueAlbum) return onQueueAlbum(selectedTracks, resource);
    return onQueue(selectedTracks, "append");
  }
  async function saveAlbumToLibrary(complete = false) {
    if ((!onSaveToLibrary && !onSaveCompleteAlbum) || !albumTracks.length || savingToLibrary) return;
    const identity = saveReleaseIdentity;
    syncSaveReleaseIdentity(identity);
    const generation = ++saveReleaseGeneration;
    const isCurrentSave = () => generation === saveReleaseGeneration && identity === saveReleaseIdentity;
    savingToLibrary = true; saveLibraryError = ""; saveLibraryNotice = "";
    try {
      if (complete && onSaveCompleteAlbum) await onSaveCompleteAlbum(albumTracks, release);
      else await onSaveToLibrary?.(albumTracks);
      if (isCurrentSave()) saveLibraryNotice = complete ? "Album saved to Library." : "Album tracks saved to Library.";
    } catch (reason) {
      if (isCurrentSave()) saveLibraryError = reason instanceof Error ? reason.message : String(reason || "Could not save tracks to Library.");
    } finally {
      if (isCurrentSave()) savingToLibrary = false;
    }
  }
</script>

<section bind:this={releaseElement} class="release-view" aria-label={`${release.title} release`}>
  {#snippet notices()}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if saveLibraryError}<p class="error" role="alert">{saveLibraryError}</p>{/if}
    {#if saveLibraryNotice}<p class="muted" role="status">{saveLibraryNotice}</p>{/if}
    {#if loading && !tracks.length}<p class="muted" role="status">Loading release tracks…</p>{/if}
  {/snippet}
  <AlbumDetailView
    metadata={detailMetadata} albumTracks={albumTracks} {analysisIdentity} {loading} ownsScroll={false} onArtistClick={() => tracks[0] && onArtist?.(tracks[0])}
    onPlayAlbum={playAlbumWithResource} onQueueAlbum={queueAlbumWithResource}
    onDownloadAlbum={onDownloadAlbum ? (items => onDownloadAlbum?.(items)) : undefined}
    onSearchAlbumSoulseek={onSearchAlbumSoulseek} {soulseekSearchAvailable}
    onAddAlbumToPlaylist={onAddAlbumToPlaylist}
    onSaveAlbum={onSaveCompleteAlbum ? (_items => saveAlbumToLibrary(true)) : undefined} saveAlbumLabel="Save album"
    onSaveLoadedTracks={onSaveToLibrary ? (_items => saveAlbumToLibrary()) : undefined}
    saveLoadedTracksLabel={onSaveCompleteAlbum ? "Save loaded tracks" : saveAlbumButtonLabel} saving={savingToLibrary}
    onAnalyzeLoudness={onAnalyzeLoudness} analysisTargetLabel="this release" analysisScopeComplete={release.trackCount == null || albumTracks.length === release.trackCount} analysisUnavailableReason={`Album analysis requires all ${release.trackCount ?? "catalog"} release tracks. ${albumTracks.length} are currently loaded.`} {trackListProps}
    filterValue={filter} onFilterChange={value => filter = value}
    sortValue={sort} {sortOptions} onSortChange={value => sort = value as TrackSortField}
    {sortDirection} onDirectionChange={value => sortDirection = value}
    availabilityValue={downloadAvailability} availabilityOptions={availabilityOptions} onAvailabilityChange={value => downloadAvailability = value as typeof downloadAvailability}
    {minDuration} {maxDuration} onDurationChange={(minimum, maximum) => { minDuration = minimum; maxDuration = maximum; }}
    visibleTrackCount={visibleTracks.length} actionStatus=""
    loadingMessage="Loading release tracks…" emptyMessage={tracks.length ? "No tracks match this filter." : "No track details available for this release."}
    {notices}
  />
</section>
<style>
  .release-view{display:flex;flex-direction:column;min-width:0;min-height:0;height:100%;overflow:auto;border-radius:12px}.release-view .error{color:#ff9b9b}.release-view .muted{color:var(--muted,#a3a8ae)}
</style>
