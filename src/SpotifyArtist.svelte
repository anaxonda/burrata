<script lang="ts">
  import TrackDownloadFilter from "./TrackDownloadFilter.svelte";
  let downloadAvailability = $state<"all"|"downloaded"|"not-downloaded">("all");
  import TrackDurationFilter from "./TrackDurationFilter.svelte";
  import { trackSortAvailabilityForSources, trackSortOptionLabelForSources } from "./lib/track-sort-availability";
  import type { Track } from "./lib/types";
  import TrackList from "./TrackList.svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";
  import { providerAccountScope, providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";
  import { safeArtworkUrls } from "./lib/artwork-url";

  type Album = { id: string; title: string; url: string; artworkUrl?: string | null; releaseDate?: string | null; albumType?: string | null; totalTracks?: number | null; artists: string[] };
  type Artist = { id: string; name: string; url: string; artworkUrl?: string | null; genres: string[]; popularity?: number | null; followers?: number | null };
  type ArtistPage = { artist: Artist; albums: Album[]; nextCursor?: string | null; totalAlbums?: number | null; topTracksUnavailable?: string | null };
  type AlbumPage = { album: Album; tracks: Track[]; nextCursor?: string | null; total?: number | null; complete?: boolean };
  type ArtistTracksPage = { provider: string; accountId?: string | null; artistId: string; tracks: Track[]; nextCursor?: string | null; total?: number | null };
  type ReleaseSortField = Extract<TrackSortField, "sourceOrder" | "title" | "releaseDate">;
  type ArtistSnapshot = { page: ArtistPage | null; albums: Album[]; albumPage: AlbumPage | null; albumId: string | null; tracks: Track[]; albumCursor: string | null; trackCursor: string | null; seenAlbumCursors: string[]; seenTrackCursors: string[]; topTracks?: Track[]; topTracksCursor?: string | null; topTracksTotal?: number | null; seenTopTrackCursors?: string[]; topTracksLoaded?: boolean; topTrackSort?: TrackSortField; topTrackSortDirection?: SortDirection; topTrackQuery?: string; topTrackMinDuration?: string; topTrackMaxDuration?: string; loadedTopTrackCount?: number; sort: TrackSortField; sortDirection: SortDirection; releaseSort?: ReleaseSortField; releaseSortDirection?: SortDirection; minDuration: string; maxDuration: string; scroll: number; loadedAlbumCount: number; loadedTrackCount: number; needsRefetch: boolean };
  let { artistId, initialAlbumId = null, navigationSnapshotKey = null, call, onPlay = () => {}, onAlbumOpening = () => {}, onPlaylist = () => {}, onNext, onQueue, onFavorite, onDownload, onArtist, onTrackAlbum, onContext, onAddSelectedToPlaylist, onAddAlbumToPlaylist, onSaveToLibrary, onBrowseArtistProvider, playingTrack = null, bufferingTrack = null, playingTrackId = "", bufferingTrackId = "", paused = false, initialScroll = 0, onScroll = () => {}, searchArtistFallback = null }: {
    artistId: string;
    initialAlbumId?: string | null;
    navigationSnapshotKey?: string | null;
    call: (command: string, args: Record<string, unknown>) => Promise<unknown>;
    onPlay?: (track: Track, list: Track[], index: number) => void;
    onAlbumOpening?: () => void;
    onPlaylist?: (track: Track) => void;
    onNext?: (track: Track) => void;
    onQueue?: (track: Track) => void;
    onFavorite?: (track: Track) => void;
    onDownload?: (track: Track) => void;
    onArtist?: (track: Track) => void;
    onTrackAlbum?: (track: Track) => void | Promise<void>;
    onContext?: (track: Track, event: MouseEvent) => void;
    onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>; onAddAlbumToPlaylist?: (tracks: Track[], title: string) => void | Promise<void>; onSaveToLibrary?: (tracks: Track[]) => void | Promise<void>;
    onBrowseArtistProvider?: (name: string, provider: "spotify" | "musicbrainz") => void | Promise<void>;
    playingTrack?: Track | null; bufferingTrack?: Track | null; playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
    initialScroll?: number; onScroll?: (scrollTop: number) => void;
    searchArtistFallback?: { id: string; title: string; url: string | null; artworkUrl: string | null } | null;
  } = $props();
  const providerReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));
  const topTracksReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));
  let page = $state<ArtistPage | null>(null);
  let albums = $state<Album[]>([]);
  let albumPage = $state<AlbumPage | null>(null);
  let tracks = $state<Track[]>([]);
  let topTracks = $state<Track[]>([]);
  let topTracksCursor = $state<string | null>(null);
  let topTracksTotal = $state<number | null>(null);
  let topTracksLoaded = $state(false);
  let topTracksBusy = $state(false);
  let topTracksError = $state("");
  let topTracksNotice = $state("");
  let seenTopTrackCursors = new Set<string>();
  let topTracksRequest = 0;
  const MAX_RESTORED_TOP_TRACKS = 5_000;
  let topTrackSort = $state<TrackSortField>("sourceOrder");
  let topTrackSortDirection = $state<SortDirection>("asc");
  let topTrackQuery = $state("");
  let topTrackMinDuration = $state("");
  let topTrackMaxDuration = $state("");
  let busy = $state(false);
  let error = $state("");
  let failedArtistArtwork = $state<string[]>([]);
  let librarySaveStatus = $state<{ albumId: string; kind: "saving" | "success" | "error"; text: string } | null>(null);
  let librarySaveGeneration = 0;
  let albumCursor = $state<string | null>(null);
  let trackCursor = $state<string | null>(null);
  let seenAlbumCursors = new Set<string>();
  let seenTrackCursors = new Set<string>();
  let sort = $state<TrackSortField>("sourceOrder");
  let sortDirection = $state<SortDirection>("asc");
  let releaseSort = $state<ReleaseSortField>("sourceOrder");
  let releaseSortDirection = $state<SortDirection>("asc");
  let minDuration = $state("");
  let maxDuration = $state("");
  let scrollElement: HTMLElement | null = null;
  let scrollTop = 0;
  let accountScope: string | null = null;
  let initializedArtistId = "";
  let initializedSnapshotKey: string | null = null;
  let restoreGeneration = 0;
  const visibleTrackOccurrences = $derived.by(() => {
    const bySortObject = new Map<Track, { track: Track; sourceIndex: number }>();
    const sortable = tracks.map((track, sourceIndex) => { const copy = { ...track }; bySortObject.set(copy, { track, sourceIndex }); return copy; });
    const filtered = filterTracks(sortable, {
      availability: downloadAvailability,
      ...(parseDurationBound(minDuration) === null ? {} : { minDurationSeconds: parseDurationBound(minDuration)! }),
      ...(parseDurationBound(maxDuration) === null ? {} : { maxDurationSeconds: parseDurationBound(maxDuration)! }),
    });
    return sortTracks(filtered, sort, sortDirection).map(track => bySortObject.get(track)!);
  });
  const visibleTracks = $derived(visibleTrackOccurrences.map(row => row.track));
  const albumTracksComplete = $derived(Boolean(albumPage && albumPage.complete !== false && albumPage.total !== null && albumPage.total !== undefined && tracks.length === albumPage.total && !trackCursor));
  const sortAvailability = $derived(trackSortAvailabilityForSources(
    tracks, ["spotify"], Boolean(trackCursor || (albumPage?.total != null && tracks.length < albumPage.total)),
  ));
  const visibleTopTrackOccurrences = $derived.by(() => {
    const bySortObject = new Map<Track, { track: Track; sourceIndex: number }>();
    const sortable = topTracks.map((track, sourceIndex) => { const copy = { ...track }; bySortObject.set(copy, { track, sourceIndex }); return copy; });
    const filtered = filterTracks(sortable, {
      availability: downloadAvailability,
    query: topTrackQuery,
    ...(parseDurationBound(topTrackMinDuration) === null ? {} : { minDurationSeconds: parseDurationBound(topTrackMinDuration)! }),
    ...(parseDurationBound(topTrackMaxDuration) === null ? {} : { maxDurationSeconds: parseDurationBound(topTrackMaxDuration)! }),
    });
    return sortTracks(filtered, topTrackSort, topTrackSortDirection).map(track => bySortObject.get(track)!);
  });
  const visibleTopTracks = $derived(visibleTopTrackOccurrences.map(row => row.track));
  const topTrackSortAvailability = $derived(trackSortAvailabilityForSources(
    topTracks, ["spotify"], Boolean(topTracksCursor || (topTracksTotal != null && topTracks.length < topTracksTotal)),
  ));
  const visibleAlbums = $derived.by(() => {
    const byId = new Map(albums.map((album) => [album.id, album]));
    const sortRows: Track[] = albums.map((album) => ({
      id: album.id, provider: "spotify", providerId: `spotify:${album.id}`, sourceUrl: album.url,
      title: album.title, artist: album.artists.join(", "), album: album.title,
      releaseDate: album.releaseDate, favorite: false, provenance: null,
    }));
    return sortTracks(sortRows, releaseSort, releaseSortDirection)
      .map((track) => byId.get(track.id)!)
      .filter(Boolean);
  });
  const artistArtworkFallback = $derived(searchArtistFallback?.id === artistId ? searchArtistFallback.artworkUrl : null);
  const artistArtworkCandidates = $derived(safeArtworkUrls([
    page?.artist.artworkUrl,
    artistArtworkFallback,
    ...albums.slice(0, 6).map(album => album.artworkUrl),
  ]));
  const artistArtworkUrl = $derived(artistArtworkCandidates.find(url => !failedArtistArtwork.includes(url)) ?? null);
  function rejectArtistArtwork(url: string) {
    if (!failedArtistArtwork.includes(url)) failedArtistArtwork = [...failedArtistArtwork, url].slice(-8);
  }
  function formatReleaseDate(value?: string | null, fallback?: string | null) {
    if (!value) return fallback || "Release";
    const parts = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?/.exec(value.trim());
    if (!parts) return value;
    const year = Number(parts[1]);
    if (!parts[2]) return String(year);
    const month = Number(parts[2]);
    const day = parts[3] ? Number(parts[3]) : 1;
    const date = new Date(Date.UTC(year, month - 1, day));
    if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return value;
    return new Intl.DateTimeFormat("en", parts[3]
      ? { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" }
      : { month: "long", year: "numeric", timeZone: "UTC" }).format(date);
  }
  let generation = 0;
  async function loadArtist(id: string, cursor: string | null = null, restoreToken?: number) {
    if (restoreToken === undefined) restoreGeneration += 1;
    else if (restoreToken !== restoreGeneration || artistId !== id) return;
    if (!cursor) { seenAlbumCursors = new Set(); failedArtistArtwork = []; }
    providerReads.cancelAll(); const current = ++generation; busy = true; error = "";
    try {
      const result = await providerReads.read("browse_remote_artist", { artistId: id, cursor });
      if (current !== generation || (restoreToken !== undefined && (restoreToken !== restoreGeneration || artistId !== id))) return;
      page = result as ArtistPage;
      const knownAlbums = new Set((cursor ? albums : []).map(album => album.id));
      albums = [...(cursor ? albums : []), ...page.albums.filter(album => {
        if (knownAlbums.has(album.id)) return false;
        knownAlbums.add(album.id);
        return true;
      })];
      const nextCursor = page.nextCursor ?? null;
      if (nextCursor && (nextCursor === cursor || seenAlbumCursors.has(nextCursor))) {
        albumCursor = null;
        error = "Spotify repeated a releases page. Load more has been stopped to prevent duplicate results.";
      } else {
        if (nextCursor) seenAlbumCursors.add(nextCursor);
        albumCursor = nextCursor;
      }
    } catch (cause) { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) error = String(cause); }
    finally { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) busy = false; }
  }
  async function loadTopTracks(id: string, cursor: string | null = null, restoreToken?: number) {
    if (restoreToken !== undefined && (restoreToken !== restoreGeneration || artistId !== id)) return;
    if (cursor === null) { topTracks = []; topTracksTotal = null; topTracksCursor = null; seenTopTrackCursors = new Set(); topTracksError = ""; topTracksNotice = ""; }
    topTracksReads.cancelAll();
    const request = ++topTracksRequest;
    const isCurrent = () => request === topTracksRequest && artistId === id && (restoreToken === undefined || restoreToken === restoreGeneration);
    topTracksBusy = true; topTracksError = "";
    try {
      const result = await topTracksReads.read<ArtistTracksPage>("browse_remote_artist_tracks", { artistId: id, cursor });
      if (!isCurrent()) return;
      if (result.provider !== "spotify" || result.artistId !== id || !Array.isArray(result.tracks)) {
        throw new Error("Spotify returned artist tracks for a different artist or provider.");
      }
      topTracks = [...(cursor ? topTracks : []), ...result.tracks];
      topTracksTotal = Number.isSafeInteger(result.total) && (result.total ?? -1) >= 0 ? result.total! : null;
      topTracksLoaded = true;
      const nextCursor = result.nextCursor ?? null;
      if (nextCursor && (nextCursor === cursor || seenTopTrackCursors.has(nextCursor))) {
        topTracksCursor = null;
        topTracksNotice = "Spotify repeated a top-tracks page. Further loading stopped; the collected tracks are kept.";
      } else {
        if (nextCursor) seenTopTrackCursors.add(nextCursor);
        topTracksCursor = nextCursor;
        topTracksNotice = "";
      }
    } catch (cause) {
      if (isCurrent()) {
        topTracksLoaded = true;
        topTracksError = `Spotify top tracks could not be loaded: ${cause instanceof Error ? cause.message : String(cause)}`;
      }
    } finally { if (isCurrent()) topTracksBusy = false; }
  }
  async function openAlbum(album: Album, restoreToken?: number) {
    if (restoreToken === undefined) onAlbumOpening();
    invalidateLibrarySave();
    if (restoreToken === undefined) restoreGeneration += 1;
    else if (restoreToken !== restoreGeneration || artistId !== initializedArtistId) return;
    cancelTopTracks();
    seenTrackCursors = new Set();
    providerReads.cancelAll(); const current = ++generation; busy = true; error = ""; albumPage = null; tracks = []; trackCursor = null;
    try {
      const result = await providerReads.read("browse_remote_album_tracks", { albumId: album.id, cursor: null }) as AlbumPage;
      if (current !== generation || (restoreToken !== undefined && (restoreToken !== restoreGeneration || artistId !== initializedArtistId))) return;
      albumPage = result; tracks = appendAlbumTracks([], result.tracks, null);
      const nextCursor = result.nextCursor ?? null;
      if (nextCursor) { seenTrackCursors.add(nextCursor); trackCursor = nextCursor; }
    } catch (cause) { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) error = String(cause); }
    finally { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) busy = false; }
  }
  async function moreTracks(restoreToken?: number) {
    if (restoreToken === undefined) restoreGeneration += 1;
    else if (restoreToken !== restoreGeneration || artistId !== initializedArtistId) return;
    if (!albumPage || !trackCursor) return;
    providerReads.cancelAll(); const current = ++generation;
    const albumId = albumPage.album.id;
    const cursor = trackCursor;
    busy = true; error = "";
    try {
      const result = await providerReads.read("browse_remote_album_tracks", { albumId, cursor }) as AlbumPage;
      if (current !== generation || albumPage?.album.id !== albumId || (restoreToken !== undefined && (restoreToken !== restoreGeneration || artistId !== initializedArtistId))) return;
      tracks = appendAlbumTracks(tracks, result.tracks, cursor);
      const nextCursor = result.nextCursor ?? null;
      if (nextCursor && (nextCursor === cursor || seenTrackCursors.has(nextCursor))) {
        trackCursor = null;
        error = "Spotify repeated a tracks page. Load more has been stopped to prevent duplicate results.";
      } else {
        if (nextCursor) seenTrackCursors.add(nextCursor);
        trackCursor = nextCursor;
      }
    } catch (cause) { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) error = String(cause); }
    finally { if (current === generation && (restoreToken === undefined || restoreToken === restoreGeneration)) busy = false; }
  }
  function invalidateLibrarySave() { librarySaveGeneration += 1; librarySaveStatus = null; }
  async function saveLoadedAlbumTracks() {
    const currentAlbum = albumPage;
    if (!currentAlbum || !tracks.length || !onSaveToLibrary || librarySaveStatus?.kind === "saving") return;
    const request = ++librarySaveGeneration;
    const albumId = currentAlbum.album.id;
    const savedTracks = [...tracks];
    const complete = albumTracksComplete;
    librarySaveStatus = { albumId, kind: "saving", text: complete ? "Saving album…" : "Saving loaded tracks…" };
    try {
      await onSaveToLibrary(savedTracks);
      if (request !== librarySaveGeneration || albumPage?.album.id !== albumId) return;
      librarySaveStatus = { albumId, kind: "success", text: complete ? `Saved album (${savedTracks.length} tracks) to your library.` : `Saved ${savedTracks.length} loaded tracks to your library.` };
    } catch (cause) {
      if (request !== librarySaveGeneration || albumPage?.album.id !== albumId) return;
      librarySaveStatus = { albumId, kind: "error", text: `Could not save tracks to your library: ${cause instanceof Error ? cause.message : String(cause)}` };
    }
  }

  function appendAlbumTracks(existing: Track[], incoming: Track[], cursor: string | null) {
    const numericOffset = cursor !== null && /^\d+$/.test(cursor) ? Number(cursor) : null;
    const positioned = (track: Track, fallback: number | null): Track => {
      const provenance = track.provenance && typeof track.provenance === "object"
        ? track.provenance as Record<string, unknown>
        : {};
      const serverPosition = provenance.positionScope === "album" && typeof provenance.position === "number"
        && Number.isSafeInteger(provenance.position) && provenance.position > 0
        ? provenance.position - 1
        : null;
      const position = serverPosition ?? fallback;
      return position === null ? track : {
        ...track,
        provenance: { ...provenance, albumPosition: position },
      };
    };
    const seen = new Set(existing.flatMap(track => {
      const entryId = remoteEntryId(track);
      const position = albumPosition(track);
      return entryId ? [`entry:${entryId}`] : position === null ? [] : [`position:${position}`];
    }));
    const additions: Track[] = [];
    incoming.forEach((rawTrack, index) => {
      const fallbackPosition = numericOffset !== null && Number.isSafeInteger(numericOffset + index)
        ? numericOffset + index
        : null;
      const track = positioned(rawTrack, fallbackPosition);
      const entryId = remoteEntryId(track);
      // Older provider bridges did not include an occurrence key. Use a validated numeric
      // offset when available; with opaque cursors, append safely rather than collapsing tracks.
      const occurrenceKey = entryId ? `entry:${entryId}`
        : fallbackPosition === null ? null : `position:${fallbackPosition}`;
      if (occurrenceKey && seen.has(occurrenceKey)) return;
      if (occurrenceKey) seen.add(occurrenceKey);
      additions.push(track);
    });
    const collected = [...existing, ...additions];
    if (!collected.some(track => albumPosition(track) !== null)) return collected;
    return collected.map((track, index) => ({ track, index, position: albumPosition(track) }))
      .sort((a, b) => a.position === null ? (b.position === null ? a.index - b.index : 1)
        : b.position === null ? -1 : a.position - b.position || a.index - b.index)
      .map(({ track }) => track);
  }
  function remoteEntryId(track: Track): string | null {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown>
      : {};
    return typeof provenance.remoteEntryId === "string" && provenance.remoteEntryId.trim()
      ? provenance.remoteEntryId
      : null;
  }
  function browseCurrentArtistOnMusicBrainz() {
    const name = page?.artist.name;
    if (name && onBrowseArtistProvider) void onBrowseArtistProvider(name, "musicbrainz");
  }
  function albumPosition(track: Track): number | null {
    const provenance = track.provenance && typeof track.provenance === "object"
      ? track.provenance as Record<string, unknown>
      : {};
    const position = provenance.albumPosition;
    return typeof position === "number" && Number.isSafeInteger(position) && position >= 0 ? position : null;
  }
  $effect(() => {
    const id = artistId, snapshotKey = navigationSnapshotKey;
    untrack(() => { if (id && (id !== initializedArtistId || snapshotKey !== initializedSnapshotKey)) void restoreArtist(id, snapshotKey); });
  });
  onMount(() => () => onScroll(scrollElement?.scrollTop ?? scrollTop));
  onDestroy(() => {
    if (initializedArtistId && accountScope) saveArtistSnapshot(initializedArtistId);
    generation++; restoreGeneration++; providerReads.cancelAll(); cancelTopTracks();
  });
  onDestroy(() => { generation++; invalidateLibrarySave(); });

  function saveArtistSnapshot(id: string, key = id) {
    const snapshot: ArtistSnapshot = { page, albums, albumPage, albumId: albumPage?.album.id ?? null, tracks, albumCursor, trackCursor, seenAlbumCursors: [...seenAlbumCursors], seenTrackCursors: [...seenTrackCursors], topTracks, topTracksCursor, topTracksTotal, seenTopTrackCursors: [...seenTopTrackCursors], topTracksLoaded, topTrackSort, topTrackSortDirection, topTrackQuery, topTrackMinDuration, topTrackMaxDuration, loadedTopTrackCount: topTracks.length, sort, sortDirection, releaseSort, releaseSortDirection, minDuration, maxDuration, scroll: scrollElement?.scrollTop ?? scrollTop, loadedAlbumCount: albums.length, loadedTrackCount: tracks.length, needsRefetch: false };
    if (accountScope) providerNavigationSnapshots.put({ kind: "spotify-artist", key, accountScope, snapshot, rows: albums.length + tracks.length + topTracks.length, stripRows: value => ({ ...value, page: null, albums: [], albumPage: null, tracks: [], albumCursor: null, trackCursor: null, seenAlbumCursors: [], seenTrackCursors: [], topTracks: [], topTracksCursor: null, seenTopTrackCursors: [], topTracksLoaded: false, needsRefetch: true }) });
  }

  export function captureNavigationSnapshot(key: string): string {
    saveArtistSnapshot(initializedArtistId || artistId, key);
    return key;
  }

  export async function restoreNavigationSnapshot(key: string) { await restoreArtist(artistId, key); }

  async function restoreArtist(id: string, snapshotKey: string | null = null) {
    invalidateLibrarySave();
    if (initializedArtistId) saveArtistSnapshot(initializedArtistId);
    initializedArtistId = id; initializedSnapshotKey = snapshotKey;
    const request = ++restoreGeneration;
    const isCurrent = () => request === restoreGeneration && artistId === id && initializedArtistId === id && initializedSnapshotKey === snapshotKey;
    providerReads.cancelAll(); generation++; cancelTopTracks();
    const scope = await providerAccountScope(call, "spotify");
    if (!isCurrent()) return;
    accountScope = scope;
    const saved = providerNavigationSnapshots.get<ArtistSnapshot>("spotify-artist", snapshotKey ?? id, accountScope);
    if (initialAlbumId && id === initialAlbumId) {
      page = null; albums = []; topTracks = []; topTracksCursor = null; topTracksTotal = null; topTracksLoaded = false;
      if (saved && !saved.needsRefetch && saved.albumPage?.album.id === initialAlbumId) {
        sort = saved.sort; sortDirection = saved.sortDirection; minDuration = saved.minDuration; maxDuration = saved.maxDuration; scrollTop = saved.scroll;
        albumPage = saved.albumPage; tracks = saved.tracks; trackCursor = saved.trackCursor; seenTrackCursors = new Set(saved.seenTrackCursors);
      } else {
        albumPage = null; tracks = []; trackCursor = null; seenTrackCursors = new Set();
        await openAlbum({ id: initialAlbumId, title: "", url: `https://open.spotify.com/album/${initialAlbumId}`, artists: [] }, request);
      }
      if (!isCurrent()) return;
      await tick(); if (isCurrent() && scrollElement) scrollElement.scrollTop = scrollTop;
      return;
    }
    if (saved && isCurrent()) {
      sort = saved.sort; sortDirection = saved.sortDirection; releaseSort = saved.releaseSort ?? "sourceOrder"; releaseSortDirection = saved.releaseSortDirection ?? "asc"; minDuration = saved.minDuration; maxDuration = saved.maxDuration; scrollTop = saved.scroll;
      topTracks = saved.topTracks ?? []; topTracksCursor = saved.topTracksCursor ?? null; topTracksTotal = saved.topTracksTotal ?? null; seenTopTrackCursors = new Set(saved.seenTopTrackCursors ?? []); topTracksLoaded = saved.topTracksLoaded ?? false; topTrackSort = saved.topTrackSort ?? "sourceOrder"; topTrackSortDirection = saved.topTrackSortDirection ?? "asc"; topTrackQuery = saved.topTrackQuery ?? ""; topTrackMinDuration = saved.topTrackMinDuration ?? ""; topTrackMaxDuration = saved.topTrackMaxDuration ?? "";
      page = saved.page; albums = saved.albums; albumPage = saved.albumPage; tracks = saved.tracks; albumCursor = saved.albumCursor; trackCursor = saved.trackCursor;
      seenAlbumCursors = new Set(saved.seenAlbumCursors); seenTrackCursors = new Set(saved.seenTrackCursors);
      if (saved.needsRefetch || !page || (!albums.length && saved.loadedAlbumCount > 0)) {
        albums = []; albumPage = null; tracks = []; albumCursor = null;
        await loadArtist(id, null, request);
        if (!isCurrent()) return;
        topTracksLoaded = false; topTracks = []; topTracksCursor = null; seenTopTrackCursors = new Set();
      }
      if (!topTracksLoaded) await loadTopTracks(id, null, request);
      if (!isCurrent()) return;
      const savedTopTrackCount = Math.min(saved.loadedTopTrackCount ?? 0, MAX_RESTORED_TOP_TRACKS);
      const started = Date.now();
      while (isCurrent() && topTracks.length < savedTopTrackCount && topTracksCursor && Date.now() - started < 15_000) {
        const before = topTracks.length; await loadTopTracks(id, topTracksCursor, request); if (!isCurrent() || topTracks.length <= before) break;
      }
      while (isCurrent() && albums.length < saved.loadedAlbumCount && albumCursor && albums.length < 5000 && Date.now() - started < 15_000) {
        const before = albums.length; await loadArtist(id, albumCursor, request); if (!isCurrent() || albums.length <= before) break;
      }
      if (saved.albumId && (saved.albumPage || saved.loadedTrackCount > 0) && (!albumPage || tracks.length < saved.loadedTrackCount)) {
        const album = albums.find(item => item.id === saved.albumId) ?? (saved.albumPage?.album.id === saved.albumId ? saved.albumPage.album : undefined);
        if (album) {
          if (!isCurrent()) return;
          await openAlbum(album, request);
          if (!isCurrent()) return;
          while (isCurrent() && tracks.length < saved.loadedTrackCount && trackCursor && tracks.length < 5000 && Date.now() - started < 15_000) {
            const before = tracks.length; await moreTracks(request); if (!isCurrent() || tracks.length <= before) break;
          }
        }
      }
      if (!isCurrent()) return;
      await tick();
      if (!isCurrent()) return;
      requestAnimationFrame(() => { if (isCurrent() && scrollElement) scrollElement.scrollTop = scrollTop; });
      return;
    }
    page = null; albums = []; albumPage = null; tracks = []; albumCursor = null; trackCursor = null; seenAlbumCursors = new Set(); seenTrackCursors = new Set(); topTracks = []; topTracksCursor = null; topTracksTotal = null; seenTopTrackCursors = new Set(); topTracksLoaded = false; topTracksError = ""; topTracksNotice = "";
    if (scrollElement) scrollElement.scrollTop = initialScroll;
    await loadArtist(id, null, request);
    if (isCurrent() && page) await loadTopTracks(id, null, request);
  }
  function cancelTopTracks() { topTracksRequest += 1; topTracksReads.cancelAll(); topTracksBusy = false; }
  function cancelRestore() { restoreGeneration += 1; generation += 1; providerReads.cancelAll(); busy = false; }
</script>

<section class="spotify-artist" aria-label={initialAlbumId ? "Spotify album" : "Spotify artist"} bind:this={scrollElement} onscroll={(event) => { scrollTop = (event.currentTarget as HTMLElement).scrollTop; onScroll(scrollTop); }}>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if albumPage}
    <header class="hero">{#if albumPage.album.artworkUrl}<img src={albumPage.album.artworkUrl} alt="" />{/if}<div><p class="eyebrow">{albumPage.album.albumType ?? "Album"}</p><h1>{albumPage.album.title}</h1><p>{formatReleaseDate(albumPage.album.releaseDate)} · {albumPage.album.artists.join(", ")}</p></div></header>
    <div class="track-heading"><h2>Tracks <span>{visibleTracks.length}</span></h2><div class="track-controls"><select aria-label="Sort tracks" bind:value={sort} onchange={cancelRestore}><option value="sourceOrder">Track number</option><option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabelForSources(sortAvailability, "title", "Title", ["spotify"])}</option><option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabelForSources(sortAvailability, "artist", "Artist", ["spotify"])}</option><option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabelForSources(sortAvailability, "duration", "Duration", ["spotify"])}</option><option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabelForSources(sortAvailability, "album", "Album", ["spotify"])}</option><option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabelForSources(sortAvailability, "releaseDate", "Release date", ["spotify"])}</option><option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabelForSources(sortAvailability, "dateAdded", "Date added", ["spotify"])}</option><option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabelForSources(sortAvailability, "bpm", "BPM", ["spotify"])}</option><option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabelForSources(sortAvailability, "key", "Key", ["spotify"])}</option><option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabelForSources(sortAvailability, "plays", "Plays", ["spotify"])}</option><option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabelForSources(sortAvailability, "views", "Views", ["spotify"])}</option></select><select aria-label="Sort direction" bind:value={sortDirection} onchange={cancelRestore}><option value="asc">Ascending</option><option value="desc">Descending</option></select><TrackDurationFilter bind:minDuration={minDuration} bind:maxDuration={maxDuration} minLabel="Minimum track duration" maxLabel="Maximum track duration" onchange={cancelRestore} /><TrackDownloadFilter bind:value={downloadAvailability} />{#if onAddAlbumToPlaylist && tracks.length}<button type="button" aria-label="Add album to playlist" onclick={() => void onAddAlbumToPlaylist?.([...tracks], albumPage!.album.title)}>Add to playlist</button>{:else if onSaveToLibrary && tracks.length}<button class="save-album-to-library" type="button" aria-label={albumTracksComplete ? "Save album to library" : "Save loaded tracks to library"} disabled={librarySaveStatus?.kind === "saving"} onclick={() => void saveLoadedAlbumTracks()}>{librarySaveStatus?.kind === "saving" ? "Saving…" : albumTracksComplete ? "Save album" : "Save loaded tracks"}</button>{/if}</div></div>
    <p class="collection-scope">Sorting and duration filters apply to collected tracks ({tracks.length}{albumPage.total != null ? ` of ${albumPage.total}` : ""}).{#if trackCursor} Load more to include additional tracks.{/if}</p>
    {#if librarySaveStatus?.albumId === albumPage.album.id}<p class={librarySaveStatus.kind === "error" ? "error" : "notice"} role={librarySaveStatus.kind === "error" ? "alert" : "status"}>{librarySaveStatus.text}</p>{/if}
    <TrackList presentation="playlist" position={(_track, index) => (visibleTrackOccurrences[index]?.sourceIndex ?? index) + 1} tracks={visibleTracks} selectionContext={`album:${artistId}:${albumPage.album.id}`} selectionKeys={visibleTrackOccurrences.map(row => String(row.sourceIndex))} selectionTracks={tracks} selectionTrackKeys={tracks.map((_, index) => String(index))} listLabel={`${albumPage.album.title} tracks`} onPlay={(track, list, index) => onPlay(track, list, index)} onDownload={onDownload} onFavorite={onFavorite} onNext={onNext} onQueue={onQueue} onPlaylist={onPlaylist} onArtist={onArtist} onAlbum={onTrackAlbum} onContext={onContext} {onAddSelectedToPlaylist} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
    {#if trackCursor}<button disabled={busy} onclick={() => { cancelRestore(); void moreTracks(); }}>Load more tracks</button>{/if}
  {:else if page}
    <header class="hero">{#if artistArtworkUrl}<img src={artistArtworkUrl} alt="" onerror={() => rejectArtistArtwork(artistArtworkUrl!)} />{:else}<div class="placeholder">♫</div>{/if}<div><p class="eyebrow">Spotify artist</p><h1>{page.artist.name}</h1>{#if page.artist.genres.length}<p>{page.artist.genres.join(" · ")}</p>{/if}<a href={page.artist.url} target="_blank" rel="noreferrer">Open in Spotify</a>{#if onBrowseArtistProvider}<button class="browse-provider" type="button" aria-label={`Find ${page.artist.name} on MusicBrainz`} onclick={browseCurrentArtistOnMusicBrainz}>Find on MusicBrainz</button>{/if}</div></header>
    <section class="top-tracks" aria-labelledby="spotify-top-tracks-title">
      <h2 id="spotify-top-tracks-title">Top tracks {#if topTracksTotal != null}<span>{topTracksTotal}</span>{/if}</h2>
      {#if topTracks.length}
        <div class="track-controls" aria-label="Top tracks controls">
          <input aria-label="Filter top tracks" placeholder="Filter loaded top tracks" bind:value={topTrackQuery} />
          <select aria-label="Sort top tracks" bind:value={topTrackSort}><option value="sourceOrder">Provider order</option><option value="title" disabled={!topTrackSortAvailability.title}>{trackSortOptionLabelForSources(topTrackSortAvailability, "title", "Title", ["spotify"])}</option><option value="artist" disabled={!topTrackSortAvailability.artist}>{trackSortOptionLabelForSources(topTrackSortAvailability, "artist", "Artist", ["spotify"])}</option><option value="album" disabled={!topTrackSortAvailability.album}>{trackSortOptionLabelForSources(topTrackSortAvailability, "album", "Album", ["spotify"])}</option><option value="duration" disabled={!topTrackSortAvailability.duration}>{trackSortOptionLabelForSources(topTrackSortAvailability, "duration", "Duration", ["spotify"])}</option><option value="releaseDate" disabled={!topTrackSortAvailability.releaseDate}>{trackSortOptionLabelForSources(topTrackSortAvailability, "releaseDate", "Release date", ["spotify"])}</option><option value="dateAdded" disabled={!topTrackSortAvailability.dateAdded}>{trackSortOptionLabelForSources(topTrackSortAvailability, "dateAdded", "Date added", ["spotify"])}</option><option value="bpm" disabled={!topTrackSortAvailability.bpm}>{trackSortOptionLabelForSources(topTrackSortAvailability, "bpm", "BPM", ["spotify"])}</option><option value="key" disabled={!topTrackSortAvailability.key}>{trackSortOptionLabelForSources(topTrackSortAvailability, "key", "Key", ["spotify"])}</option><option value="plays" disabled={!topTrackSortAvailability.plays}>{trackSortOptionLabelForSources(topTrackSortAvailability, "plays", "Plays", ["spotify"])}</option><option value="views" disabled={!topTrackSortAvailability.views}>{trackSortOptionLabelForSources(topTrackSortAvailability, "views", "Views", ["spotify"])}</option></select>
          <select aria-label="Top tracks sort direction" bind:value={topTrackSortDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select>
          <TrackDurationFilter bind:minDuration={topTrackMinDuration} bind:maxDuration={topTrackMaxDuration} minLabel="Minimum top track duration" maxLabel="Maximum top track duration" /><TrackDownloadFilter bind:value={downloadAvailability} />
          <span>{visibleTopTracks.length} matching · {topTracks.length} loaded{topTracksTotal != null ? ` of ${topTracksTotal}` : ""}</span>
        </div>
        <TrackList presentation="playlist" tracks={visibleTopTracks} selectionContext={`artist-top:${artistId}`} selectionKeys={visibleTopTrackOccurrences.map(row => String(row.sourceIndex))} selectionTracks={topTracks} selectionTrackKeys={topTracks.map((_, index) => String(index))} listLabel={`${page.artist.name} top tracks`} onPlay={onPlay} onDownload={onDownload} onFavorite={onFavorite} onNext={onNext} onQueue={onQueue} onPlaylist={onPlaylist} onArtist={onArtist} onAlbum={onTrackAlbum} onContext={onContext} {onAddSelectedToPlaylist} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
      {:else if topTracksBusy}<p class="notice" role="status">Loading Spotify top tracks…</p>
      {:else if topTracksError}<p class="notice error" role="alert">{topTracksError}</p>
      {:else}<p class="notice" role="status">{page.topTracksUnavailable || "No top tracks are available for this artist."}</p>{/if}
      {#if topTracksError && topTracks.length}<p class="notice error" role="alert">{topTracksError}</p>{/if}
      {#if topTracksNotice}<p class="notice" role="status">{topTracksNotice}</p>{/if}
      {#if topTracksError}<button disabled={topTracksBusy} onclick={() => void loadTopTracks(artistId, topTracksCursor)}>Retry top tracks</button>{/if}
      {#if topTracksCursor}<button disabled={topTracksBusy} onclick={() => void loadTopTracks(artistId, topTracksCursor)}>Load more top tracks</button>{/if}
    </section>
    <h2>Releases {#if page.totalAlbums != null}<span>{page.totalAlbums}</span>{/if}</h2>
    <div class="release-controls" aria-label="Release sorting">
      <label>Sort releases<select aria-label="Sort releases" bind:value={releaseSort} onchange={cancelRestore}><option value="sourceOrder">Provider order</option><option value="releaseDate">Release date</option><option value="title">Title</option></select></label>
      <label>Direction<select aria-label="Release sort direction" bind:value={releaseSortDirection} onchange={cancelRestore}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
      <span>{albums.length} loaded releases</span>
    </div>
    <div class="albums">{#each visibleAlbums as album (album.id)}<button class="album" onclick={() => { cancelRestore(); void openAlbum(album); }}>{#if album.artworkUrl}<img src={album.artworkUrl} alt="" />{:else}<span class="placeholder">♫</span>{/if}<strong title={album.title}>{album.title}</strong><span>{formatReleaseDate(album.releaseDate, album.albumType)}</span></button>{/each}</div>
    {#if albumCursor}<button disabled={busy} onclick={() => { cancelRestore(); void loadArtist(artistId, albumCursor!); }}>Load more releases</button>{/if}
  {:else if busy}<p role="status">Loading Spotify artist…</p>{/if}
</section>

<style>
  .spotify-artist{height:100%;min-height:0;overflow:auto;padding:1rem;display:flex;flex-direction:column;gap:.8rem;color:var(--text,#eee);background:var(--base,#17191c);border-radius:12px}.spotify-artist button,.spotify-artist input,.spotify-artist select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .7rem}.spotify-artist button{cursor:pointer}.spotify-artist button:disabled{opacity:.5}.hero{display:flex;align-items:center;justify-content:flex-start;gap:1rem;min-width:0;padding:.5rem 0 .75rem;border-bottom:1px solid var(--border,#383d43)}.hero>div{flex:1;min-width:0}.hero img,.hero .placeholder{flex:none;width:clamp(96px,12vw,124px);height:clamp(96px,12vw,124px);object-fit:cover;border-radius:9px}.placeholder{display:grid;place-items:center;background:#252b31;font-size:3rem;color:var(--accent,#67c3a5)}.hero h1{margin:.25rem 0}.hero p{color:var(--muted,#a3a8ae)}.hero a{color:var(--accent,#67c3a5)}.eyebrow{font-size:.75rem;text-transform:uppercase;letter-spacing:.1em}.browse-provider{display:block;margin-top:.45rem;font-size:.85rem}.notice{padding:.7rem;border-radius:7px;background:var(--surface,#202328);color:var(--muted,#a3a8ae)}h2 span{font-size:.8rem;color:var(--muted,#a3a8ae);font-weight:400}.release-controls{display:flex;align-items:center;gap:.5rem;flex-wrap:wrap}.release-controls label{display:flex;align-items:center;gap:.35rem}.release-controls span{color:var(--muted,#a3a8ae);font-size:.85rem}.albums{display:grid;grid-template-columns:repeat(auto-fill,minmax(145px,1fr));gap:.8rem}.album{display:flex;flex-direction:column;align-items:flex-start;text-align:left;gap:.35rem;min-width:0;width:100%;overflow:hidden}.album img,.album .placeholder{width:100%;aspect-ratio:1;object-fit:cover}.album strong{display:block;width:100%;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.album span:last-child{font-size:.85rem;color:var(--muted,#a3a8ae)}.track-heading{display:flex;flex-wrap:wrap;justify-content:space-between;gap:.7rem;align-items:center}.track-controls{display:flex;gap:.4rem;flex-wrap:wrap}.track-controls select{color-scheme:dark}.track-controls input{width:5.5rem}.error{color:#ff9b9b}
  .spotify-artist .track-controls{align-items:center;gap:6px 8px;min-width:0}

  .spotify-artist .track-controls select,.spotify-artist .track-controls input,.spotify-artist .track-controls .save-album-to-library{box-sizing:border-box;flex:none;height:34px;min-height:34px;padding:.3rem .5rem;font-size:.8rem;line-height:normal}

  .spotify-artist .track-heading h2{margin:0;font-size:1.1rem}
  .spotify-artist .release-controls label{display:flex;flex-direction:row;align-items:center;gap:.35rem;margin:0;font-size:.8rem}.spotify-artist .release-controls select{height:34px;min-height:34px;padding:.3rem .5rem;font-size:.8rem}.spotify-artist>h2{margin:0;line-height:1.2}.spotify-artist>.notice{margin:0}
  .top-tracks h2{margin:0;line-height:1.2}.top-tracks h2 span{font-size:.8rem;color:var(--muted,#a3a8ae);font-weight:400}.top-tracks .track-controls{align-items:center;margin:.35rem 0}.top-tracks .track-controls input,.top-tracks .track-controls select{height:34px;min-height:34px;padding:.3rem .5rem;font-size:.8rem}.top-tracks .track-controls>span{font-size:.78rem;color:var(--muted,#a3a8ae)}

  @media (min-width:901px) {
    .top-tracks .track-controls { gap:6px 8px; }
    .top-tracks .track-controls>input { flex:1 1 220px; width:auto; min-width:180px; max-width:420px; }


    .top-tracks .track-controls select { height:auto; min-height:34px; line-height:normal; box-sizing:border-box; }
    .top-tracks .track-controls>span { white-space:nowrap; }
  }
  @media (min-width:901px) and (max-height:650px) {
    .spotify-artist { padding:8px 16px; gap:6px; }
    .spotify-artist>.hero { flex:none; gap:12px; padding:0 0 6px; }
    .spotify-artist>.hero img,.spotify-artist>.hero .placeholder { width:72px; height:72px; }
    .spotify-artist>.hero h1 { margin:2px 0; font-size:21px; line-height:1.15; }
    .spotify-artist>.hero p { margin:2px 0; line-height:1.2; }
    .spotify-artist>.hero a { font-size:13px; }
    .top-tracks .track-controls { margin:4px 0; }
  }
  .spotify-artist .browse-provider{box-sizing:border-box;min-height:34px;height:auto;padding:.3rem .6rem;font-size:.8rem;line-height:1.2}
  .spotify-artist .browse-provider{display:inline-flex;vertical-align:middle;margin-top:0;margin-left:.5rem}
  .spotify-artist .track-controls select,.spotify-artist .release-controls select{appearance:none;-webkit-appearance:none;box-sizing:border-box;display:block;height:34px;min-height:34px;max-height:34px;padding:0 30px 0 9px;font-size:12px;line-height:18px;text-align:left;background-image:linear-gradient(45deg,transparent 50%,var(--muted,#a3a8ae) 50%),linear-gradient(135deg,var(--muted,#a3a8ae) 50%,transparent 50%);background-position:calc(100% - 14px) 50%,calc(100% - 10px) 50%;background-size:4px 4px;background-repeat:no-repeat}
  .spotify-artist>.collection-scope{flex:none;margin:0;padding:0;border:0;border-radius:0;background:none;color:var(--muted,#a3a8ae);font-size:12px;line-height:1.4}
</style>
