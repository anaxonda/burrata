<script module lang="ts">
  import type { SortDirection, TrackSortField } from "./lib/track-sort";
  import { parseDurationBound } from "./lib/track-sort";
  export type DirectoryBrowserLocation = { rootId: string; relativePath: string };
  export type DirectoryBrowserSnapshot = { location: DirectoryBrowserLocation | null; query: string; sortField: "filename" | TrackSortField; sortDirection: SortDirection; durationMinimum: string; durationMaximum: string; loadedCount: number; scrollTop: number };
</script>

<script lang="ts">
  import TrackDownloadFilter from "./TrackDownloadFilter.svelte";
  let downloadAvailability = $state<"all"|"downloaded"|"not-downloaded">("all");
  import { onDestroy, tick, untrack } from "svelte";
  import TrackList from "./TrackList.svelte";
  import type { DirectoryAudioFile, DirectoryBrowsePage, DirectoryBrowserRoot, DirectoryScanJob, DirectorySortAvailabilityFields, DirectorySortAvailabilityJob } from "./lib/directory-browser";
  import type { Track } from "./lib/types";

  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  const PAGE_SIZE = 200;
  type Location = DirectoryBrowserLocation;
  type LocationChangeMeta = { reason: "user" | "registry"; previousSnapshot: DirectoryBrowserSnapshot };
  let { call, location = null, rootRegistryRevision = 0, onLocationChange = (_location: Location | null, _meta: LocationChangeMeta) => {}, onRootsChanged = () => {}, onPlayFile = (_file: DirectoryAudioFile, _track: Track) => {}, onDownload, onFavorite, onQueue, onNext, onPlaylist, onArtist, onTrackAlbum, onInfo, onContext, onAddSelectedToPlaylist, playingTrack = null, bufferingTrack = null, playingTrackId = "", bufferingTrackId = "", paused = false }: {
    location?: Location | null;
    rootRegistryRevision?: number;
    onLocationChange?: (location: Location | null, meta: LocationChangeMeta) => void;
    onRootsChanged?: () => void;
    call: Call;
    onPlayFile?: (file: DirectoryAudioFile, track: Track) => void;
    onDownload?: (track: Track) => void | Promise<void>; onFavorite?: (track: Track) => void | Promise<void>;
    onQueue?: (track: Track) => void | Promise<void>; onNext?: (track: Track) => void | Promise<void>;
    onPlaylist?: (track: Track) => void | Promise<void>; onArtist?: (track: Track) => void | Promise<void>; onTrackAlbum?: (track: Track) => void | Promise<void>;
    onInfo?: (track: Track) => void | Promise<void>; onContext?: (track: Track, event: MouseEvent) => void;
    onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>;
    playingTrack?: Track | null; bufferingTrack?: Track | null; playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
  } = $props();

  let roots = $state<DirectoryBrowserRoot[]>([]);
  let selectedRootId = $state("");
  let selectedRelativePath = $state("");
  let page = $state<DirectoryBrowsePage | null>(null);
  // These rows enumerate existing files, including files not indexed yet.
  const displayedFiles = $derived(downloadAvailability === "not-downloaded" ? [] : page?.files ?? []);

  let loadingRoots = $state(false);
  let loadingPage = $state(false);
  let loadingMore = $state(false);
  let rootBusy = $state(false);
  let error = $state("");
  let query = $state("");
  let sortField = $state<"filename" | TrackSortField>("filename");
  let sortDirection = $state<SortDirection>("asc");
  let durationMinimum = $state("");
  let durationMaximum = $state("");
  const durationProblem = $derived.by(() => {
    const minimum = durationMinimum.trim() ? parseDurationBound(durationMinimum) : null;
    const maximum = durationMaximum.trim() ? parseDurationBound(durationMaximum) : null;
    if ((durationMinimum.trim() && minimum === null) || (durationMaximum.trim() && maximum === null)) return "Enter duration in seconds or mm:ss.";
    if (minimum !== null && maximum !== null && minimum > maximum) return "Minimum duration must be less than or equal to maximum duration.";
    return "";
  });
  let queryTimer: ReturnType<typeof setTimeout> | null = null;
  let scan = $state<DirectoryScanJob | null>(null);
  let scanError = $state("");
  let sortAvailability = $state<DirectorySortAvailabilityFields | null>(null);
  let availabilityState = $state<"idle" | "pending" | "ready" | "failed">("idle");
  let availabilityError = $state("");
  let availabilityJobId = "";
  let availabilityRevision = 0;
  let availabilityTimer: ReturnType<typeof setTimeout> | null = null;
  let availabilityStartedAt = 0;
  let scanTimer: ReturnType<typeof setTimeout> | null = null;
  let browseRevision = 0;
  let snapshotRestoreRevision = 0;
  let scanRevision = 0;
  let rootsRequestRevision = 0;
  let locationRevision = 0;
  let destroyed = false;
  const selectedRoot = $derived(roots.find(root => root.id === selectedRootId) ?? null);
  const indexedOccurrences = $derived(displayedFiles.flatMap(file => file.track ? [{ file, track: file.track }] : []));
  const orderedRows = $derived.by(() => {
    let trackIndex = 0;
    return displayedFiles.map(file => file.track
      ? { kind: "track" as const, key: file.path, trackIndex: trackIndex++ }
      : { kind: "status" as const, key: file.path, title: file.name, detail: "Metadata unavailable · Scan this folder to index" });
  });

  function durationValue(raw: string): number | undefined { return raw.trim() ? parseDurationBound(raw) ?? undefined : undefined; }
  function cancelSnapshotRestore() {
    snapshotRestoreRevision++;
    browseRevision++;
    loadingPage = false;
    loadingMore = false;
  }
  function refreshResults() {
    if (queryTimer !== null) clearTimeout(queryTimer);
    queryTimer = null;
    cancelSnapshotRestore();
    if (!durationProblem) void browseDirectory(selectedRelativePath, false);
  }
  function boundedQueryText() {
    let output = "";
    let bytes = 0;
    for (const character of query.trim()) {
      const size = new TextEncoder().encode(character).length;
      if (bytes + size > 512) break;
      output += character;
      bytes += size;
    }
    return output;
  }
  function filterChanged() {
    cancelSnapshotRestore();
    invalidateSortAvailability();
    if (queryTimer !== null) clearTimeout(queryTimer);
    if (durationProblem) { queryTimer = null; return; }
    queryTimer = setTimeout(() => { queryTimer = null; refreshResults(); }, 250);
  }

  export function getSnapshot(): DirectoryBrowserSnapshot {
    const pendingFilter = queryTimer !== null || Boolean(durationProblem);
    return {
      location: selectedRootId ? { rootId: selectedRootId, relativePath: selectedRelativePath } : null,
      query, sortField, sortDirection, durationMinimum, durationMaximum,
      loadedCount: pendingFilter ? 0 : page?.files.length ?? 0,
      scrollTop: pendingFilter ? 0 : document.querySelector<HTMLElement>(".directory-browser .track-list-viewport")?.scrollTop ?? 0,
    };
  }

  export async function restoreSnapshot(snapshot: DirectoryBrowserSnapshot) {
    const restoreRevision = ++snapshotRestoreRevision;
    const isCurrent = () => !destroyed && restoreRevision === snapshotRestoreRevision;
    if (queryTimer !== null) clearTimeout(queryTimer);
    queryTimer = null;
    if (!isCurrent()) return;
    query = snapshot.query; sortField = snapshot.sortField; sortDirection = snapshot.sortDirection;
    durationMinimum = snapshot.durationMinimum; durationMaximum = snapshot.durationMaximum;
    if (!snapshot.location) return;
    const target = snapshot.location;
    if (target.rootId !== selectedRootId || target.relativePath !== selectedRelativePath) {
      selectedRootId = target.rootId; selectedRelativePath = target.relativePath; page = null;
    }
    await browseDirectory(target.relativePath, false, restoreRevision);
    if (!isCurrent()) return;
    const controlsMatch = () => query === snapshot.query && sortField === snapshot.sortField && sortDirection === snapshot.sortDirection && durationMinimum === snapshot.durationMinimum && durationMaximum === snapshot.durationMaximum;
    while (isCurrent() && controlsMatch() && selectedRootId === target.rootId && selectedRelativePath === target.relativePath && page && page.files.length < snapshot.loadedCount && page.files.length < page.total) {
      const before = page.files.length;
      await browseDirectory(target.relativePath, true, restoreRevision);
      if (!isCurrent()) return;
      if (!page || page.files.length <= before) break;
    }
    if (!isCurrent() || !controlsMatch() || selectedRootId !== target.rootId || selectedRelativePath !== target.relativePath) return;
    await tick();
    requestAnimationFrame(() => {
      if (!isCurrent()) return;
      const viewport = document.querySelector<HTMLElement>(".directory-browser .track-list-viewport");
      if (viewport) viewport.scrollTop = snapshot.scrollTop;
    });
  }

  function clearScanTimer() {
    if (scanTimer !== null) clearTimeout(scanTimer);
    scanTimer = null;
    if (queryTimer !== null) clearTimeout(queryTimer);
    queryTimer = null;
  }

  async function loadRoots(preferredId = selectedRootId, preferredPath = selectedRelativePath) {
    const revision = ++rootsRequestRevision;
    loadingRoots = true;
    error = "";
    try {
      const next = await call<DirectoryBrowserRoot[]>("list_directory_browser_roots");
      if (destroyed || revision !== rootsRequestRevision) return;
      roots = next;
      const nextId = next.some(root => root.id === preferredId) ? preferredId : next[0]?.id ?? "";
      if (nextId) await selectLocation({rootId:nextId,relativePath:nextId === preferredId ? preferredPath : ""}, nextId !== preferredId || location === null, "registry");
      else {
        if (scan?.state === "running") void call("cancel_directory_browser_scan", {jobId:scan.jobId}).catch(()=>{});
        const hadLocation = Boolean(selectedRootId);
        const previousSnapshot = getSnapshot();
        snapshotRestoreRevision++; browseRevision++; scanRevision++; clearScanTimer(); selectedRootId="";selectedRelativePath="";page=null;scan=null;
        if (hadLocation) onLocationChange(null, { reason: "registry", previousSnapshot });
      }
    } catch (cause) {
      if (!destroyed && revision === rootsRequestRevision) error = String(cause);
    } finally {
      if (!destroyed && revision === rootsRequestRevision) loadingRoots = false;
    }
  }

  async function chooseRoot() {
    if (rootBusy) return;
    rootBusy = true;
    error = "";
    try {
      const choice = await call<{ path: string } | null>("choose_directory_browser_root");
      if (destroyed || !choice?.path) return;
      const root = await call<DirectoryBrowserRoot>("add_directory_browser_root", { path: choice.path });
      if (destroyed) return;
      roots = [...roots.filter(item => item.id !== root.id), root];
      onRootsChanged();
      await selectLocation({rootId:root.id,relativePath:""}, true, "user");
    } catch (cause) {
      error = String(cause);
    } finally {
      rootBusy = false;
    }
  }

  function clearAvailabilityTimer() { if (availabilityTimer !== null) clearTimeout(availabilityTimer); availabilityTimer = null; }
  function invalidateSortAvailability() {
    availabilityRevision++;
    clearAvailabilityTimer();
    const previous = availabilityJobId; availabilityJobId = "";
    if (previous) void call("cancel_directory_browser_sort_availability", {jobId:previous}).catch(()=>{});
    sortAvailability = null; availabilityState = "idle"; availabilityError = "";
  }
  async function requestSortAvailability(rootId: string, relativePath: string) {
    invalidateSortAvailability();
    const revision = availabilityRevision;
    availabilityState = "pending"; availabilityStartedAt = Date.now();
    try {
      const started = await call<DirectorySortAvailabilityJob>("start_directory_browser_sort_availability", {
        rootId, relativePath, query: boundedQueryText(),
        ...(durationValue(durationMinimum) === undefined ? {} : {durationMin:durationValue(durationMinimum)}),
        ...(durationValue(durationMaximum) === undefined ? {} : {durationMax:durationValue(durationMaximum)}),
      });
      if (destroyed || revision !== availabilityRevision || rootId !== selectedRootId || relativePath !== selectedRelativePath) {
        void call("cancel_directory_browser_sort_availability", {jobId:started.jobId}).catch(()=>{}); return;
      }
      availabilityJobId = started.jobId;
      if (started.state === "completed" && started.fields) { sortAvailability=started.fields; availabilityState="ready"; availabilityJobId=""; return; }
      if (started.state === "failed" || started.state === "cancelled") { availabilityState="failed"; availabilityError=started.error || "Sort availability could not be checked. Options remain enabled."; availabilityJobId=""; return; }
      const poll = async () => {
        if (destroyed || revision !== availabilityRevision) return;
        if (Date.now() - availabilityStartedAt > 120_000) {
          availabilityState = "failed"; availabilityError = "Sort availability is taking too long. Options remain enabled.";
          const jobId = availabilityJobId; availabilityJobId = "";
          if (jobId) void call("cancel_directory_browser_sort_availability", {jobId}).catch(()=>{});
          return;
        }
        try {
          const status = await call<DirectorySortAvailabilityJob>("get_directory_browser_sort_availability", {jobId:availabilityJobId});
          if (destroyed || revision !== availabilityRevision) return;
          if (status.state === "completed" && status.fields) { sortAvailability=status.fields; availabilityState="ready"; availabilityJobId=""; return; }
          if (status.state === "failed" || status.state === "cancelled") { availabilityState="failed"; availabilityError=status.error || "Sort availability could not be checked. Options remain enabled."; availabilityJobId=""; return; }
          availabilityTimer=setTimeout(()=>void poll(),150);
        } catch (cause) {
          if (destroyed || revision !== availabilityRevision) return;
          availabilityState="failed"; availabilityError=`Sort availability could not be checked: ${String(cause)}`; availabilityJobId="";
        }
      };
      availabilityTimer=setTimeout(()=>void poll(),0);
    } catch (cause) {
      if (!destroyed && revision === availabilityRevision) { availabilityState="failed"; availabilityError=`Sort availability could not be checked: ${String(cause)}`; }
    }
  }
  function isSortFieldAvailable(field: string): boolean {
    if (!sortAvailability || availabilityState !== "ready") return true;
    if (field === "filename" || field === "sourceOrder") return true;
    const index = ["title","artist","album","duration","releaseDate","dateAdded","bpm","key","plays","views"].indexOf(field);
    return index < 0 || sortAvailability[index];
  }
  function sortFieldLabel(field: string, label: string) { return isSortFieldAvailable(field) ? label : `${label} (unavailable)`; }

  async function browseDirectory(relativePath: string, append = false, restoreRevision?: number) {
    if (restoreRevision === undefined) snapshotRestoreRevision++;
    else if (restoreRevision !== snapshotRestoreRevision) return;
    const rootId = selectedRootId;
    if (!rootId || (append && loadingMore)) return;
    if (!append) invalidateSortAvailability();
    if (durationProblem) return;
    const revision = ++browseRevision;
    if (append) loadingMore = true;
    else {
      loadingMore = false;
      loadingPage = true;
      error = "";
      page = null;
    }
    const offset = append ? (page?.files.length ?? 0) : 0;
    try {
      const next = await call<DirectoryBrowsePage>("browse_directory_browser", {
        rootId, relativePath, offset, limit: PAGE_SIZE,
        query: boundedQueryText(), sortField, sortDirection,
        ...(durationValue(durationMinimum) === undefined ? {} : { durationMin: durationValue(durationMinimum) }),
        ...(durationValue(durationMaximum) === undefined ? {} : { durationMax: durationValue(durationMaximum) }),
      });
      if (destroyed || revision !== browseRevision || rootId !== selectedRootId || relativePath !== selectedRelativePath) return;
      if (next.rootId !== rootId || next.relativePath !== relativePath || next.offset !== offset) {
        throw new Error("The directory response did not match the current location.");
      }
      page = append && page
        ? { ...next, files: [...page.files, ...next.files], sortAvailability: page.sortAvailability ?? next.sortAvailability }
        : next;
      if (!append) {
        if (next.sortAvailability) { sortAvailability=next.sortAvailability; availabilityState="ready"; }
        else void requestSortAvailability(rootId, relativePath);
      }
    } catch (cause) {
      if (!destroyed && revision === browseRevision) error = String(cause);
    } finally {
      if (!destroyed && revision === browseRevision) {
        loadingPage = false;
        loadingMore = false;
      }
    }
  }

  async function startScan() {
    if (!selectedRootId || scan?.state === "running") return;
    const revision = ++scanRevision;
    scanError = "";
    try {
      const job = await call<DirectoryScanJob>("start_directory_browser_scan", { rootId: selectedRootId });
      if (destroyed || revision !== scanRevision) return;
      scan = job;
      if (job.state === "running") void pollScan(job.jobId, revision);
      else await finishScan(job);
    } catch (cause) {
      if (!destroyed && revision === scanRevision) scanError = String(cause);
    }
  }

  async function pollScan(jobId: string, revision: number) {
    while (!destroyed && revision === scanRevision) {
      try {
        const next = await call<DirectoryScanJob>("get_directory_browser_scan", { jobId });
        if (destroyed || revision !== scanRevision || next.jobId !== jobId) return;
        scan = next;
        if (next.state !== "running") {
          await finishScan(next);
          return;
        }
      } catch (cause) {
        if (revision === scanRevision) scanError = String(cause);
        return;
      }
      await new Promise<void>(resolve => { scanTimer = setTimeout(resolve, 500); });
      scanTimer = null;
    }
  }

  async function finishScan(status: DirectoryScanJob) {
    clearScanTimer();
    if (status.state === "completed") {
      scanError = status.failed > 0
        ? `Scan completed with errors: ${status.indexed.toLocaleString()} indexed, ${status.failed.toLocaleString()} failed.`
        : "";
      await browseDirectory(selectedRelativePath, false);
    } else if (status.state === "failed") {
      scanError = status.error || `Scan failed: ${status.indexed.toLocaleString()} indexed, ${status.failed.toLocaleString()} failed.`;
    } else if (status.state === "cancelled") {
      scanError = "Directory scan canceled. Any files already indexed remain available.";
      await browseDirectory(selectedRelativePath, false);
    }
  }

  async function cancelScan() {
    const jobId = scan?.jobId;
    if (!jobId || scan?.state !== "running") return;
    try {
      // Cancellation is cooperative. Keep polling until the worker reports a terminal state.
      const status = await call<DirectoryScanJob>("cancel_directory_browser_scan", { jobId });
      if (status.jobId !== jobId || scan?.jobId !== jobId) return;
      scan = status;
      if (status.state !== "running") {
        scanRevision++;
        await finishScan(status);
      }
    } catch (cause) {
      scanError = String(cause);
    }
  }

  async function selectLocation(next: Location, emit = false, reason: "user" | "registry" = "user") {
    if (next.rootId === selectedRootId && next.relativePath === selectedRelativePath && (page || loadingPage)) return;
    const revision = ++locationRevision;
    const changingRoot = next.rootId !== selectedRootId;
    const previousJob = changingRoot && scan?.state === "running" ? scan.jobId : null;
    snapshotRestoreRevision++; browseRevision++; loadingPage = false; loadingMore = false;
    if (changingRoot) {
      scanRevision++; clearScanTimer(); scan=null;
      scanError=previousJob ? "Scan cancellation requested because you changed folders. Indexed files remain available." : "";
    }
    if (emit) onLocationChange({...next}, { reason, previousSnapshot: getSnapshot() });
    selectedRootId=next.rootId;selectedRelativePath=next.relativePath;page=null;
    if (previousJob) try { await call("cancel_directory_browser_scan", {jobId:previousJob}); } catch { /* the new folder remains usable */ }
    if (destroyed || revision !== locationRevision) return;
    await browseDirectory(next.relativePath, false);
  }

  function formatScanProgress(job: DirectoryScanJob) {
    const done = job.completed.toLocaleString();
    const total = job.total == null ? "" : ` of ${job.total.toLocaleString()}`;
    return `Scanning files: ${done}${total} checked · ${job.indexed.toLocaleString()} indexed · ${job.failed.toLocaleString()} failed`;
  }

  function playOccurrence(track: Track, _list: Track[], index: number) {
    const occurrence = indexedOccurrences[index];
    if (occurrence) onPlayFile(occurrence.file, occurrence.track);
  }
  function occurrenceKey(file: DirectoryAudioFile) { return file.path; }

  $effect(() => {
    void rootRegistryRevision;
    const target = location ? {rootId:location.rootId,relativePath:location.relativePath} : null;
    untrack(() => { void loadRoots(target?.rootId ?? selectedRootId, target?.relativePath ?? selectedRelativePath); });
  });
  onDestroy(() => {
    destroyed = true;
    browseRevision++;
    scanRevision++;
    clearScanTimer();
    const summaryJob = availabilityJobId; availabilityJobId = ""; availabilityRevision++; clearAvailabilityTimer();
    if (summaryJob) void call("cancel_directory_browser_sort_availability", {jobId:summaryJob}).catch(()=>{});
    if (scan?.state === "running") void call("cancel_directory_browser_scan", { jobId: scan.jobId }).catch(() => {});
  });
</script>

<section class="directory-browser" aria-label="Music folders">
  {#if selectedRoot}<div class="directory-toolbar">
    <nav class="directory-breadcrumbs" aria-label="Folder path">
      <span class="directory-path-label">Folder</span>
      {#each page?.breadcrumbs ?? [{ name: selectedRoot.name, relativePath: "" }] as crumb, index (crumb.relativePath)}
        {#if index === (page?.breadcrumbs.length ?? 1) - 1}<span aria-current="location">{crumb.name}</span>
        {:else}<button type="button" onclick={() => void selectLocation({rootId:selectedRootId,relativePath:crumb.relativePath},true)}>{crumb.name}</button><span aria-hidden="true">/</span>{/if}
      {/each}
    </nav>
    <button type="button" disabled={scan?.state === "running" || loadingPage} onclick={() => void startScan()}>{scan?.state === "running" ? "Scanning…" : "Scan folder"}</button>
  </div>{/if}

  {#if error}<p class="directory-error" role="alert">{error}</p>{/if}
  {#if scan?.state === "running"}<div class="directory-scan-status" role="status"><span>{formatScanProgress(scan)}</span><button type="button" onclick={() => void cancelScan()}>Cancel scan</button></div>{/if}
  {#if scanError}<p class="directory-scan-note" role={scan?.state === "failed" ? "alert" : "status"}>{scanError}</p>{/if}

  {#if selectedRoot}
    <div class="directory-view-tools" aria-label="Directory track view options">
      <TrackDownloadFilter bind:value={downloadAvailability} />
      <input aria-label="Filter directory tracks" placeholder="Filter tracks by title, artist, album, or filename" bind:value={query} oninput={filterChanged} />
      <label>Sort by<select aria-label="Sort directory tracks" bind:value={sortField} onchange={refreshResults}>
        <option value="filename">Filename</option><option value="sourceOrder">Original order</option>
        <option value="title" disabled={!isSortFieldAvailable("title")}>{sortFieldLabel("title","Title")}</option><option value="artist" disabled={!isSortFieldAvailable("artist")}>{sortFieldLabel("artist","Artist")}</option><option value="album" disabled={!isSortFieldAvailable("album")}>{sortFieldLabel("album","Album")}</option>
        <option value="duration" disabled={!isSortFieldAvailable("duration")}>{sortFieldLabel("duration","Duration")}</option><option value="releaseDate" disabled={!isSortFieldAvailable("releaseDate")}>{sortFieldLabel("releaseDate","Release date")}</option><option value="dateAdded" disabled={!isSortFieldAvailable("dateAdded")}>{sortFieldLabel("dateAdded","Date added")}</option>
        <option value="bpm" disabled={!isSortFieldAvailable("bpm")}>{sortFieldLabel("bpm","BPM")}</option><option value="key" disabled={!isSortFieldAvailable("key")}>{sortFieldLabel("key","Key")}</option><option value="plays" disabled={!isSortFieldAvailable("plays")}>{sortFieldLabel("plays","Plays")}</option><option value="views" disabled={!isSortFieldAvailable("views")}>{sortFieldLabel("views","Views")}</option>
      </select></label>
      <button type="button" aria-label="Toggle directory sort direction" onclick={() => { sortDirection = sortDirection === "asc" ? "desc" : "asc"; refreshResults(); }}>{sortDirection === "asc" ? "Ascending ↑" : "Descending ↓"}</button>
      <details class="directory-duration-filters"><summary>Duration</summary><div>
        <label>Minimum<input aria-label="Minimum directory track duration" placeholder="seconds or mm:ss" bind:value={durationMinimum} oninput={filterChanged} /></label>
        <label>Maximum<input aria-label="Maximum directory track duration" placeholder="seconds or mm:ss" bind:value={durationMaximum} oninput={filterChanged} /></label>
      </div></details>
      {#if page}<span class="directory-file-count" aria-live="polite">{page.files.length}{page.total > page.files.length ? ` of ${page.total}` : ""}{page.total === 1 ? " file" : " files"}</span>{/if}
    </div>
    {#if availabilityState === "pending"}<p class="directory-sort-availability" role="status">Checking sort fields across this folder…</p>{:else if availabilityError}<p class="directory-sort-availability" role="status">{availabilityError}</p>{/if}
    {#if durationProblem}<p class="directory-error" role="alert">{durationProblem}</p>{/if}
    {#if loadingPage}<p class="directory-loading" role="status">Loading folder…</p>
    {:else if page}
      {#if orderedRows.length}<div class="directory-list-scroll" class:virtualized={orderedRows.length > 40} style:flex="1 1 0" style:min-height="0" style:overflow-y={orderedRows.length > 40 ? "hidden" : "auto"}><TrackList presentation="playlist" virtualizationThreshold={40} tracks={indexedOccurrences.map(row => row.track)} {orderedRows} selectionContext={`directory:${selectedRootId}:${selectedRelativePath}`} selectionKeys={indexedOccurrences.map(row => occurrenceKey(row.file))} selectionTracks={indexedOccurrences.map(row => row.track)} selectionTrackKeys={indexedOccurrences.map(row => occurrenceKey(row.file))} listLabel="Audio files" onPlay={playOccurrence} {onDownload} {onFavorite} {onQueue} {onNext} {onPlaylist} {onArtist} onAlbum={onTrackAlbum} {onInfo} {onContext} onAddSelectedToPlaylist={indexedOccurrences.length ? onAddSelectedToPlaylist : undefined} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} meta={(_track, index) => indexedOccurrences[index]?.file.name ?? ""} /></div>{/if}
      {#if !displayedFiles.length}<p class="directory-empty">No audio files match these filters.</p>{/if}
      {#if page.files.length < page.total}<div class="directory-load-more"><button type="button" disabled={loadingMore} onclick={() => void browseDirectory(page!.relativePath, true)}>{loadingMore ? "Loading…" : `Load more files (${page.total - page.files.length} remaining)`}</button></div>{/if}
    {:else if !error}<p class="directory-empty">Choose a folder to browse.</p>{/if}
  {:else if !loadingRoots}<div class="directory-empty-state"><p>No folders registered yet.</p><button type="button" disabled={rootBusy} onclick={() => void chooseRoot()}>Add a folder to browse</button></div>{/if}
</section>

<style>
  :global(.app-shell > main > div[aria-hidden="false"]:has(> .directory-browser)) { flex:1 1 0; min-height:0; display:flex; overflow:hidden; }
  .directory-browser { min-width:0; min-height:0; flex:1 1 auto; display:flex; flex-direction:column; gap:6px; padding:8px 10px; }
  .directory-list-scroll { display:flex; flex-direction:column; min-width:0; overscroll-behavior:contain; }
  .directory-list-scroll :global(.track-list-viewport) { flex:1 1 0; min-height:0; max-height:none; overflow:visible; }
  .directory-list-scroll.virtualized :global(.track-list-viewport.virtualized) { overflow:auto; }
  .directory-toolbar { display:flex; align-items:center; gap:6px; flex-wrap:wrap; }
  .directory-toolbar button,.directory-load-more button,.directory-empty-state button { min-height:34px; }
  .directory-breadcrumbs { flex:1;min-width:0;display:flex; align-items:center; gap:7px; flex-wrap:wrap; min-height:34px; font-size:.9rem; }
  .directory-path-label { color:var(--muted);font-size:12px; }
  .directory-breadcrumbs button { border:0; background:transparent; padding:2px; color:var(--accent); cursor:pointer; }
  .directory-breadcrumbs [aria-current] { font-weight:650; }
  .directory-view-tools { display:flex; align-items:center; gap:6px; flex-wrap:wrap; min-width:0; }
  .directory-view-tools > input { flex:1 1 220px; min-width:160px; max-width:340px; width:auto; }
  .directory-view-tools > input,.directory-view-tools select,.directory-view-tools > button,.directory-duration-filters > summary,.directory-duration-filters input { box-sizing:border-box; height:34px; min-height:34px; padding:6px 8px; font-size:12px; }
  .directory-view-tools > label { display:flex; flex-direction:row; align-items:center; gap:5px; margin:0; color:var(--muted); font-size:.8rem; white-space:nowrap; }
  .directory-view-tools > label select { min-width:132px; padding:0 8px; line-height:normal; overflow:visible; }
  .directory-view-tools > button { white-space:nowrap; }
  .directory-file-count { flex:none; margin-left:auto; color:var(--muted); font-size:.8rem; white-space:nowrap; }
  .directory-duration-filters { position:relative; flex:none; color:var(--muted); font-size:.8rem; }
  .directory-duration-filters > summary { display:flex; align-items:center; cursor:pointer; padding:6px 8px; border:1px solid var(--line); border-radius:6px; background:var(--surface); color:var(--text); list-style:none; white-space:nowrap; }
  .directory-duration-filters > summary::-webkit-details-marker { display:none; }
  .directory-duration-filters > summary::after { content:"▾"; margin-left:8px; color:var(--muted); }
  .directory-duration-filters[open] > div { position:absolute; z-index:30; top:calc(100% + 6px); right:0; display:flex; gap:8px; flex-wrap:wrap; width:max-content; max-width:calc(100vw - 32px); padding:8px; border:1px solid var(--line); border-radius:8px; background:var(--surface); box-shadow:0 10px 28px #0008; }
  .directory-duration-filters > div label { display:flex; flex-direction:column; align-items:stretch; gap:4px; margin:0; font-size:.75rem; color:var(--muted); }
  .directory-duration-filters input { width:132px; }
  .directory-browser :global(.playlist-selection-actions) { box-sizing:border-box; min-height:34px; padding:0 0 6px; margin:0; border-bottom:0; }
  .directory-browser :global(.playlist-selection-actions button) { box-sizing:border-box; height:34px; min-height:34px; padding:6px 8px; font-size:12px; }
  .directory-load-more { display:flex; justify-content:center; padding:8px; }
  .directory-empty,.directory-empty-state,.directory-loading,.directory-error,.directory-scan-note { margin:4px 0; color:var(--muted); }
  .directory-empty-state { display:flex; align-items:center; justify-content:center; gap:12px; padding:28px 12px; }
  .directory-error { color:var(--danger); }.directory-scan-note { color:var(--muted); }
  .directory-scan-status { display:flex; justify-content:space-between; align-items:center; gap:10px; padding:8px 10px; background:var(--surface-raised); border-radius:6px; font-size:.85rem; }
  @media(max-width:600px) { .directory-browser { padding:8px; }.directory-toolbar { align-items:center; }.directory-view-tools > input { flex:1 1 100%; min-width:0; max-width:none; }.directory-file-count { margin-left:auto; } }
</style>
