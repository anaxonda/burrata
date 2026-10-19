<script lang="ts">
  import { trackSortAvailabilityForSources, trackSortOptionLabelForSources } from "./lib/track-sort-availability";
  import { onDestroy, tick, untrack } from "svelte";
  import TrackList from "./TrackList.svelte";
  import TrackDurationFilter from "./TrackDurationFilter.svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";
  import type { Track } from "./lib/types";
  import { normalizeSpotifyUserProfileUrl } from "./lib/spotify-profile-url";
  import { providerAccountScope, providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";

  type Provider = "spotify" | "youtube";
  type Page = {
    provider: Provider; playlistId: string; title?: string | null; url?: string | null; artworkUrl?: string | null; imageUrl?: string | null;
    ownerName?: string | null; ownerUrl?: string | null; visibility?: string | null;
    tracks: Track[]; nextCursor?: string | null; total?: number | null; complete?: boolean;
    canEdit?: boolean; writeCapabilities?: { add: boolean; remove: boolean; reorder: boolean };
  };
  type ImportJob = { jobId?: string; state?: string; completed?: number; total?: number; results?: { complete?: boolean }[]; errors?: { message?: string }[]; error?: string | null };
  type DetailSnapshot = { page: Page | null; tracks: Track[]; cursor: string | null; seenCursors: string[]; filter: string; sort: TrackSortField; sortDirection: SortDirection; minDuration: string; maxDuration: string; scroll: number; loadedCount: number; needsRefetch: boolean };

  let {
    provider, playlistUrl, call, onBack = () => {}, onImported = () => {}, onBrowseOwner,
    onPlay, onQueue, onNext, onDownload, onFavorite, onPlaylist, onAddSelectedToPlaylist, onArtist, onInfo, onContext,
    onQueueTracks, playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false,
  }: {
    provider: Provider; playlistUrl: string;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onBack?: () => void; onImported?: () => void;
    onBrowseOwner?: (provider: Provider, ownerUrl: string) => void;
    onPlay?: (track: Track, tracks: Track[], index: number) => void | Promise<void>;
    onQueue?: (track: Track) => void | Promise<void>; onNext?: (track: Track) => void | Promise<void>;
    onDownload?: (track: Track) => void | Promise<void>; onFavorite?: (track: Track) => void | Promise<void>;
    onPlaylist?: (track: Track) => void | Promise<void>; onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>; onArtist?: (track: Track) => void | Promise<void>;
    onInfo?: (track: Track) => void | Promise<void>; onContext?: (track: Track, event: MouseEvent) => void;
    onQueueTracks?: (tracks: Track[], mode: "append" | "next") => void | Promise<void>;
    playingTrack?: Track | null; bufferingTrack?: Track | null;
    playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
  } = $props();
  const providerReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));

  let page = $state<Page | null>(null);
  let tracks = $state<Track[]>([]);
  let cursor = $state<string | null>(null);
  let loading = $state(false);
  let loadingMore = $state(false);
  let adding = $state(false);
  let importProgress = $state<ImportJob | null>(null);
  let activeJobId = $state<string | null>(null);
  let cancelRequested = $state(false);
  let cancelSent = false;
  let error = $state("");
  let notice = $state("");
  let filter = $state("");
  let sort = $state<TrackSortField>("sourceOrder");
  let sortDirection = $state<SortDirection>("asc");
  let minDuration = $state("");
  let maxDuration = $state("");
  let destroyed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let finishWait: (() => void) | null = null;
  let browseGeneration = 0;
  let seenCursors = new Set<string>();
  let accountScope: string | null = null;
  let scrollElement: HTMLElement | null = null;
  let scrollTop = 0;
  let restoreGeneration = 0;
  let initializedKey = "";
  const snapshotKey = () => `${provider}:${playlistUrl}`;

  const visibleOccurrences = $derived.by(() => {
    const lower = parseDurationBound(minDuration), upper = parseDurationBound(maxDuration);
    const occurrences = tracks.map((track, sourceIndex) => ({ track, sourceIndex, sortObject: { ...track } }));
    const bySortObject = new Map(occurrences.map(row => [row.sortObject, row]));
    const filtered = filterTracks(occurrences.map(row => row.sortObject), {
      query: filter,
      ...(lower === null ? {} : { minDurationSeconds: lower }),
      ...(upper === null ? {} : { maxDurationSeconds: upper }),
    });
    return sortTracks(filtered, sort, sortDirection).map(row => bySortObject.get(row)!);
  });
  const visibleTracks = $derived(visibleOccurrences.map(row => row.track));
  const hasActiveTrackFilter = $derived(Boolean(filter.trim() || parseDurationBound(minDuration) !== null || parseDurationBound(maxDuration) !== null));
  const hasMoreRemoteTracks = $derived(Boolean(cursor || page?.complete !== true || (page?.total != null && tracks.length < page.total)));
  const sortAvailability = $derived(trackSortAvailabilityForSources(tracks, [provider], hasMoreRemoteTracks));
  const actionScope = $derived(hasActiveTrackFilter
    ? (hasMoreRemoteTracks ? "matching loaded tracks" : "matching tracks")
    : (hasMoreRemoteTracks ? "loaded tracks" : "all tracks"));
  const loadedScopeLabel = $derived.by(() => {
    const loaded = tracks.length;
    const total = page?.total;
    const loadedNoun = loaded === 1 ? "track" : "tracks";
    if (hasActiveTrackFilter) return `${visibleTracks.length} matching · ${loaded} loaded${total != null ? ` of ${total}` : ""} ${total === 1 ? "track" : "tracks"}`;
    if (page?.complete === true) {
      if (total != null && loaded === total) return `${total} tracks`;
      if (total != null) return `${loaded} tracks loaded (provider reported ${total})`;
      return `${loaded} ${loadedNoun}`;
    }
    return total != null
      ? `${loaded} of ${total} ${total === 1 ? "track" : "tracks"} loaded`
      : `${loaded} ${loadedNoun} loaded`;
  });
  const emptyStateMessage = $derived.by(() => {
    if (tracks.length > 0 || error) return "";
    if (page?.complete === true && page.total === 0) return "This playlist is empty.";
    if (page?.complete !== true && page?.total != null && page.total > 0) {
      return `Couldn't load this playlist's tracks. The provider reported ${page.total} ${page.total === 1 ? "track" : "tracks"}, but returned none.`;
    }
    if (page?.complete === false) return "The playlist snapshot is incomplete; no tracks have loaded yet.";
    if (page?.complete === true) return "The provider marked this playlist complete but returned no tracks.";
    return "";
  });

  $effect(() => {
    const key = snapshotKey();
    untrack(() => {
      if (key !== initializedKey) {
        if (initializedKey) saveSnapshot(initializedKey);
        initializedKey = key;
        void restoreOrBrowse();
      }
    });
  });
  onDestroy(() => {
    saveSnapshot(snapshotKey()); destroyed = true; providerReads.cancelAll(); finishWait?.();
  });

  function saveSnapshot(key: string) {
    const snapshot: DetailSnapshot = { page, tracks, cursor, seenCursors: [...seenCursors], filter, sort, sortDirection, minDuration, maxDuration, scroll: scrollElement?.scrollTop ?? scrollTop, loadedCount: tracks.length, needsRefetch: false };
    if (accountScope) providerNavigationSnapshots.put({ kind: "playlist-detail", key, accountScope, snapshot, rows: tracks.length, stripRows: value => ({ ...value, page: null, tracks: [], cursor: null, seenCursors: [], needsRefetch: true }) });
  }

  async function restoreOrBrowse() {
    const generation = ++restoreGeneration;
    const requestedKey = snapshotKey();
    const isCurrent = () => !destroyed && generation === restoreGeneration && requestedKey === snapshotKey();
    browseGeneration++; providerReads.cancelAll();
    const scope = await providerAccountScope(call, provider);
    if (!isCurrent()) return;
    accountScope = scope;
    const saved = providerNavigationSnapshots.get<DetailSnapshot>("playlist-detail", snapshotKey(), accountScope);
    browseGeneration++;
    providerReads.cancelAll();
    if (saved && isCurrent()) {
      page = saved.page; tracks = saved.tracks; cursor = saved.cursor; seenCursors = new Set(saved.seenCursors);
      filter = saved.filter; sort = saved.sort; sortDirection = saved.sortDirection; minDuration = saved.minDuration; maxDuration = saved.maxDuration; scrollTop = saved.scroll;
      if (saved.needsRefetch || !saved.page || saved.tracks.length < saved.loadedCount) {
        page = saved.page ? { ...saved.page, complete: false } : null;
        notice = `Restoring ${saved.loadedCount} previously loaded tracks; provider pages are being fetched again.`;
        const started = Date.now();
        if (!page && isCurrent()) await browse(true, browseGeneration, provider, playlistUrl, generation);
        if (!isCurrent()) return;
        while (isCurrent() && tracks.length < saved.loadedCount && cursor && tracks.length < 5000 && Date.now() - started < 15_000) {
          const before = tracks.length; await browse(false, browseGeneration, provider, playlistUrl, generation);
          if (!isCurrent() || tracks.length <= before) break;
        }
        if (tracks.length < saved.loadedCount) notice = `Showing ${tracks.length} tracks; the previous loaded set could not be fully restored.`;
      }
      if (!isCurrent()) return;
      await tick();
      if (!isCurrent()) return;
      requestAnimationFrame(() => { if (isCurrent() && scrollElement) scrollElement.scrollTop = scrollTop; });
      return;
    }
    page = null; tracks = []; cursor = null; loading = false; loadingMore = false;
    error = ""; notice = ""; filter = ""; sort = "sourceOrder"; sortDirection = "asc"; minDuration = ""; maxDuration = ""; seenCursors = new Set();
    await browse(true, browseGeneration, provider, playlistUrl, generation);
  }

  function cancelRestore() { restoreGeneration += 1; browseGeneration += 1; providerReads.cancelAll(); loading = false; loadingMore = false; }
  function retryPlaylistBrowse() {
    // A cursorless incomplete snapshot cannot be continued. Start a fresh first-page
    // request, keeping the partial rows until a replacement response succeeds.
    cursor = null; seenCursors = new Set(); error = ""; notice = "";
    void browse(true);
  }
  async function browse(reset: boolean, generation?: number, requestProvider = provider, requestPlaylistUrl = playlistUrl, restoreToken?: number) {
    if (restoreToken === undefined) { cancelRestore(); generation = browseGeneration; }
    else if (restoreToken !== restoreGeneration) return;
    const requestGeneration = generation ?? browseGeneration;
    if ((loading || loadingMore) && requestGeneration === browseGeneration) return;
    const requestedCursor = reset ? null : cursor;
    if (!reset && requestedCursor) seenCursors.add(requestedCursor);
    loading = reset; loadingMore = !reset; error = ""; notice = "";
    try {
      const result = await providerReads.read<Page>("browse_remote_playlist", { provider: requestProvider, playlistId: requestPlaylistUrl, ...(requestedCursor ? { cursor: requestedCursor } : {}) });
      if (destroyed || requestGeneration !== browseGeneration || (restoreToken !== undefined && restoreToken !== restoreGeneration) || requestProvider !== provider || requestPlaylistUrl !== playlistUrl) return;
      const previousPage = page;
      page = reset || !previousPage ? result : {
        ...result,
        // Continuation responses may describe a recommendation/sidebar row instead
        // of the playlist whose cursor is being followed. Keep first-page identity
        // and known metadata while taking pagination state from the latest response.
        provider: previousPage.provider,
        playlistId: previousPage.playlistId,
        title: previousPage.title ?? result.title,
        url: previousPage.url ?? result.url,
        artworkUrl: previousPage.artworkUrl ?? result.artworkUrl,
        imageUrl: previousPage.imageUrl ?? result.imageUrl,
        ownerName: previousPage.ownerName ?? result.ownerName,
        ownerUrl: previousPage.ownerUrl ?? result.ownerUrl,
        visibility: previousPage.visibility ?? result.visibility,
        canEdit: previousPage.canEdit ?? result.canEdit,
        writeCapabilities: previousPage.writeCapabilities ?? result.writeCapabilities,
        total: previousPage.total ?? result.total,
      };
      tracks = reset ? result.tracks ?? [] : dedupe([...tracks, ...(result.tracks ?? [])]);
      const nextCursor = result.nextCursor ?? null;
      if (!reset && (nextCursor || (page?.total != null && tracks.length !== page.total))) {
        page = { ...page!, complete: false };
      }
      if (!reset && nextCursor && seenCursors.has(nextCursor)) {
        cursor = null;
        page = { ...page!, complete: false };
        error = "Stopped loading because the provider repeated a page cursor. The tracks loaded so far are shown; the playlist may be incomplete.";
      } else cursor = nextCursor;
    } catch (failure) { if (!destroyed && requestGeneration === browseGeneration && (restoreToken === undefined || restoreToken === restoreGeneration)) error = `Could not load playlist tracks: ${String(failure)}`; }
    finally { if (!destroyed && requestGeneration === browseGeneration && (restoreToken === undefined || restoreToken === restoreGeneration)) { loading = false; loadingMore = false; } }
  }

  function dedupe(rows: Track[]) {
    const keys = new Set<string>();
    return rows.filter(track => {
      const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
      const occurrenceId = typeof provenance.remoteEntryId === "string" && provenance.remoteEntryId ? provenance.remoteEntryId : null;
      // Track IDs identify catalog songs, not playlist occurrences. Without a stable
      // occurrence ID we cannot distinguish overlap from a deliberate repeat.
      if (!occurrenceId) return true;
      if (keys.has(occurrenceId)) return false;
      keys.add(occurrenceId);
      return true;
    });
  }

  async function addToLibrary() {
    if (adding) return;
    adding = true; cancelRequested = false; cancelSent = false; importProgress = null; error = ""; notice = "";
    try {
      let job = await call<ImportJob>("import_remote_playlists", { provider, playlistIds: [playlistUrl] });
      if (destroyed) return;
      activeJobId = job.jobId || null; importProgress = job;
      while (activeJobId && !destroyed && !terminal(job.state)) {
        if (cancelRequested && !cancelSent) { await call("cancel_remote_playlist_import", { jobId: activeJobId }); cancelSent = true; }
        await waitForPoll();
        if (destroyed) return;
        job = await call<ImportJob>("get_remote_playlist_import_job", { jobId: activeJobId });
        if (destroyed) return;
        importProgress = job;
      }
      if (job.state === "failed") throw new Error(job.error || job.errors?.[0]?.message || "Import failed.");
      const partial = job.state === "partial" || Boolean(job.errors?.length) || Boolean(job.results?.some(result => result.complete === false));
      if (partial && !job.results?.length && job.state !== "cancelled") throw new Error(job.error || job.errors?.[0]?.message || "No playlist was added. The provider returned an incomplete result.");
      if (job.results?.length) await onImported();
      if (job.state === "cancelled") notice = "Import canceled. Any playlists already added remain in your library.";
      else if (partial) notice = "Playlist added with missing items. Review its incomplete status in your library.";
      else { notice = "Playlist added to your library."; if (!job.results?.length) await onImported(); }
      activeJobId = null;
    } catch (failure) { if (!destroyed) error = `Could not add playlist: ${String(failure)}`; }
    finally { if (!destroyed) adding = false; }
  }

  function waitForPoll() {
    return new Promise<void>(resolve => {
      let done = false;
      const finish = () => { if (done) return; done = true; if (timer !== null) clearTimeout(timer); timer = null; finishWait = null; resolve(); };
      finishWait = finish; timer = setTimeout(finish, 800);
    });
  }

  async function cancelImport() {
    if (!adding || cancelRequested) return;
    cancelRequested = true;
    if (activeJobId) {
      try { await call("cancel_remote_playlist_import", { jobId: activeJobId }); cancelSent = true; }
      catch (failure) { error = `Could not cancel import: ${String(failure)}`; cancelRequested = false; }
    }
  }

  function terminal(state?: string) { return ["completed", "partial", "cancelled", "failed"].includes((state ?? "").toLowerCase()); }
  function progressCount() { return Math.min(importProgress?.total ?? 0, importProgress?.completed ?? 0); }
  function ownerUrl() {
    if (!page?.ownerUrl) return "";
    try {
      const url = new URL(page.ownerUrl);
      if (url.protocol !== "https:" || url.username || url.password || (url.port && url.port !== "443")) return "";
      if (provider === "spotify") {
        return normalizeSpotifyUserProfileUrl(page.ownerUrl) ?? "";
      }
      return ["www.youtube.com", "youtube.com", "music.youtube.com"].includes(url.hostname) ? url.toString() : "";
    } catch { return ""; }
  }
  function openOwner() {
    const url = ownerUrl();
    if (!url) return;
    if (provider === "spotify" && onBrowseOwner) {
      cancelRestore();
      onBrowseOwner("spotify", url);
    } else void call("open_external_url", { url });
  }
  function playAll() { if (visibleTracks.length) void onPlay?.(visibleTracks[0]!, visibleTracks, 0); }
  function trackMeta(track: Track, index: number) {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const availability = typeof provenance.availability === "string" ? provenance.availability : track.localAvailability;
    const unavailable = provenance.available === false || availability === "unavailable" || availability === "none" || availability === "missing";
    const rowPosition = provenance.positionScope === "page" || typeof provenance.position !== "number"
      ? (tracks.indexOf(track) >= 0 ? tracks.indexOf(track) + 1 : index + 1)
      : provenance.position;
    return `${unavailable ? `Unavailable from ${provider === "spotify" ? "Spotify" : "YouTube"} · ` : ""}Playlist position ${rowPosition}`;
  }
  function queueAll(mode: "append" | "next") {
    if (!visibleTracks.length) return;
    if (onQueueTracks) void onQueueTracks(visibleTracks, mode);
    else {
      const action = mode === "next" ? onNext : onQueue;
      for (const track of visibleTracks) void action?.(track);
    }
  }
</script>

<section class="remote-playlist-detail" aria-labelledby="remote-playlist-detail-title" bind:this={scrollElement} onscroll={(event) => { scrollTop = (event.currentTarget as HTMLElement).scrollTop; }}>
  <header class="remote-detail-header">
    <button class="remote-detail-back" onclick={() => { cancelRestore(); onBack(); }}>← Back to playlists</button>
    <h2 id="remote-playlist-detail-title">{page?.title || "Remote playlist"}</h2>
    {#if ownerUrl()}<button class="remote-detail-owner" aria-label={provider === "spotify" && onBrowseOwner ? `Browse ${page?.ownerName || "creator"} playlists` : `Open ${page?.ownerName || "creator"} profile in browser`} title={provider === "spotify" && onBrowseOwner ? `Browse ${page?.ownerName || "creator"} playlists` : `Open ${page?.ownerName || "creator"} profile in browser`} onclick={openOwner}>{page?.ownerName || "Open creator profile"}</button>{:else if page?.ownerName}<span class="remote-detail-owner-name">{page.ownerName}</span>{/if}
    <p>{loadedScopeLabel}{page?.visibility ? ` · ${page.visibility}` : ""}{page?.complete === false ? " · Snapshot incomplete" : page && page.complete !== true ? " · Completeness unknown" : ""}</p>
    <div class="remote-detail-actions">
      {#if tracks.length}
        <button onclick={playAll} disabled={!visibleTracks.length}>Play {actionScope}</button>
        <button onclick={() => queueAll("append")} disabled={!visibleTracks.length}>Queue {actionScope}</button>
        <button onclick={() => queueAll("next")} disabled={!visibleTracks.length}>Play {actionScope} next</button>
      {/if}
      <button class="primary" onclick={() => void addToLibrary()} disabled={adding}>Add to library</button>
    </div>
  </header>
  <div class="remote-detail-tools">
    <input aria-label="Filter playlist tracks" bind:value={filter} oninput={cancelRestore} placeholder="Filter tracks by title, artist, or album" />
    <label>Sort
      <select aria-label="Sort playlist tracks" bind:value={sort} onchange={cancelRestore}><option value="sourceOrder">Playlist order</option><option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabelForSources(sortAvailability, "title", "Title", [provider])}</option><option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabelForSources(sortAvailability, "artist", "Artist", [provider])}</option><option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabelForSources(sortAvailability, "album", "Album", [provider])}</option><option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabelForSources(sortAvailability, "duration", "Duration", [provider])}</option><option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabelForSources(sortAvailability, "releaseDate", "Release date", [provider])}</option><option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabelForSources(sortAvailability, "dateAdded", "Date added", [provider])}</option><option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabelForSources(sortAvailability, "bpm", "BPM", [provider])}</option><option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabelForSources(sortAvailability, "key", "Key", [provider])}</option><option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabelForSources(sortAvailability, "plays", "Plays", [provider])}</option><option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabelForSources(sortAvailability, "views", "Views", [provider])}</option></select>
    </label>
    <label>Direction<select aria-label="Sort direction" bind:value={sortDirection} onchange={cancelRestore}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
    <TrackDurationFilter bind:minDuration={minDuration} bind:maxDuration={maxDuration} minLabel="Minimum track duration" maxLabel="Maximum track duration" onchange={cancelRestore} />
  </div>
  {#if loading}<p class="remote-detail-status" role="status">Loading playlist…</p>{/if}
  {#if error}<p class="remote-detail-error" role="alert">{error}</p>{/if}
  {#if notice}<p class="remote-detail-notice" role="status">{notice}</p>{/if}
  {#if adding && importProgress}<div class="remote-detail-progress" role="status"><span>Adding playlist · {progressCount()} of {importProgress.total ?? 1}</span><progress max={importProgress.total || 1} value={progressCount()}></progress><button onclick={() => void cancelImport()} disabled={cancelRequested}>{cancelRequested ? "Canceling…" : "Cancel import"}</button></div>{/if}
  {#if tracks.length}<TrackList tracks={visibleTracks} selectionContext={`playlist:${provider}:${playlistUrl}`} selectionKeys={visibleOccurrences.map(row => String(row.sourceIndex))} selectionTracks={tracks} selectionTrackKeys={tracks.map((_, index) => String(index))} listLabel={`${page?.title || "Remote playlist"} tracks`} meta={trackMeta} onPlay={onPlay} onQueue={onQueue} onNext={onNext} onDownload={onDownload} onFavorite={onFavorite} onPlaylist={onPlaylist} {onAddSelectedToPlaylist} onArtist={onArtist} onInfo={onInfo} onContext={onContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />{:else if !loading && !error && page}<p class="remote-detail-status">{emptyStateMessage}</p>{/if}
  {#if cursor}<button class="remote-detail-more" disabled={loadingMore} onclick={() => void browse(false)}>{loadingMore ? "Loading more…" : "Load more tracks"}</button>{/if}
  {#if page?.complete !== true && !cursor}<button class="remote-detail-retry primary" disabled={loading || loadingMore} onclick={retryPlaylistBrowse}>{loading ? "Retrying playlist…" : "Retry loading playlist"}</button>{/if}
</section>

<style>
  .remote-playlist-detail { box-sizing:border-box; flex:1 1 0; min-width:0; min-height:0; height:auto; overflow:auto; display:flex; flex-direction:column; align-items:stretch; gap:8px; color:var(--text); }
  .remote-playlist-detail h2 { flex:1 1 220px; min-width:0; margin:0; font-size:18px; line-height:1.3; overflow-wrap:anywhere; }
  .remote-detail-header { box-sizing:border-box; flex:none; min-width:0; min-height:0; height:auto; display:flex; flex-direction:row; flex-wrap:wrap; justify-content:flex-start; align-items:center; align-content:flex-start; gap:6px 12px; padding:8px 12px; }
  .remote-detail-header p { flex:0 0 auto; margin:0; color:var(--muted); font-size:11px; }
  .remote-detail-back,.remote-detail-owner { justify-self:start; padding:0; border:0; background:transparent; color:var(--accent,#8e9df2); font-size:11px; line-height:1.4; }
  .remote-detail-back { flex:0 0 100%; text-align:left; }
  .remote-detail-owner { flex:0 1 auto; }
  .remote-detail-owner-name { color:var(--muted); font-size:11px; }
  .remote-detail-actions,.remote-detail-tools { display:flex; flex:none; flex-wrap:wrap; align-items:center; align-content:flex-start; gap:6px; min-width:0; min-height:0; height:auto; }
  .remote-detail-actions button,.remote-detail-more,.remote-detail-progress button { min-height:32px; height:auto; padding:6px 9px; font-size:11px; white-space:nowrap; }
  .remote-detail-tools { box-sizing:border-box; padding:4px 12px; }
  .remote-detail-tools > input { flex:1 1 240px; width:auto; min-width:160px; }
  .remote-detail-tools label { flex:0 0 auto; display:flex; flex-direction:row; align-items:center; gap:5px; margin:0; color:var(--muted); font-size:10px; white-space:nowrap; }
  .remote-detail-tools input,.remote-detail-tools select { box-sizing:border-box; align-self:center; height:34px; min-height:34px; max-height:34px; padding:6px 8px; font-size:12px; }
  .remote-detail-tools select { flex:none; min-width:130px; max-width:180px; height:auto; min-height:34px; max-height:none; padding:4px 8px; line-height:normal; }
  .remote-detail-tools label:nth-of-type(2) select { min-width:112px; }
  .remote-detail-status,.remote-detail-error,.remote-detail-notice { flex:none; margin:0; padding:0 12px; font-size:11px; line-height:1.4; overflow-wrap:anywhere; }
  .remote-detail-status { color:var(--muted); }
  .remote-detail-error { color:var(--danger,#d66); }
  .remote-detail-notice { color:var(--success,#5bab8a); }
  .remote-detail-progress { flex:none; display:grid; grid-template-columns:minmax(0,1fr) auto; gap:6px; align-items:center; padding:0 12px; color:var(--muted); font-size:11px; }
  .remote-detail-progress progress { grid-column:1 / -1; width:100%; }
  .remote-detail-more { align-self:center; }
  .remote-detail-retry { align-self:flex-start; width:auto; min-width:0; min-height:34px; height:auto; margin:0 12px; padding:5px 10px; font-size:12px; line-height:1.25; }
  .remote-playlist-detail :global(.shared-track-list) { flex:none; }
</style>
