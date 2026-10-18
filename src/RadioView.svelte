<script lang="ts">
  import { trackSortAvailability, trackSortOptionLabel } from "./lib/track-sort-availability";
  import { onMount } from "svelte";
  import { call } from "./lib/api";
  import type { Track } from "./lib/types";
  import TrackList from "./TrackList.svelte";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";

  export type RadioStation = {
    id: string; name: string; streamUrl: string; sourceUrl: string;
    provider?: string | null; country?: string | null; tags?: string | null; homepageUrl?: string | null;
    faviconUrl?: string | null; favicon?: string | null; bitrate?: number | null; bitrateKbps?: number | null; codec?: string | null; language?: string | null; isCustom?: boolean;
    description?: string | null;
  };
  type RadioStationDetails = Partial<RadioStation> & { stationId: string; nowPlaying?: RadioTrackMetadata | null; history?: RadioTrackMetadata[]; historyAvailable?: boolean; note?: string };
  export type RadioStationSearchResult = RadioStation;
  export type RadioTrackMetadata = {
    stationId: string; artist: string; title: string; album?: string | null; year?: string | null;
    artworkUrl?: string | null; sourceUrl: string; metadataConfidence: string; observedAt?: string | null;
  };
  type RadioMetadata = {
    stationId: string; nowPlaying: RadioTrackMetadata | null; secondsRemaining?: number | null;
    history: RadioTrackMetadata[]; historyAvailable: boolean; note: string;
  };

  let { onPlayStation, onArtist, playingStationId = "", onNowPlaying, selectedStationId: requestedStationId = "", showAdd = false, onStationsChanged, onAddClosed, stationNames={}, savedStations,
    onPlayTrack, onDownloadTrack, onFavoriteTrack, onQueueTracks, onAddTrackToPlaylist, onTrackContext, onInfoTrack, onOpenHistory, playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false, favoriteTrackIds = [],
  }:
    { onPlayStation: (station: RadioStation) => void; onArtist: (track: Track) => void; playingStationId?: string; onNowPlaying?: (station: RadioStation, track: RadioTrackMetadata | null) => void; selectedStationId?: string; showAdd?: boolean; onStationsChanged?: (stations: RadioStation[]) => void; onAddClosed?: () => void;stationNames?:Record<string,string>; savedStations?:RadioStation[];
      onPlayTrack?: (track: Track, tracks: Track[], index: number) => void | Promise<void>; onDownloadTrack?: (track: Track) => void | Promise<void>; onFavoriteTrack?: (track: Track) => void | Promise<void>;
      onQueueTracks?: (tracks: Track[], mode: "next" | "append") => void | Promise<void>; onAddTrackToPlaylist?: (track: Track) => void | Promise<void>; onTrackContext?: (track: Track, event: MouseEvent) => void;
      onInfoTrack?: (track: Track) => void | Promise<void>;
      onOpenHistory?: (stationId: string) => void;
      playingTrack?: Track | null; bufferingTrack?: Track | null;
      playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
      favoriteTrackIds?: string[];
    } = $props();
  let stations = $state<RadioStation[]>([]);
  let activeStationId = $state("main");
  $effect(()=>{
    if(savedStations === undefined)return;
    stations=savedStations;
    if(!savedStations.some(station=>station.id===activeStationId))activeStationId=savedStations[0]?.id??"";
  });
  let metadata = $state<RadioMetadata | null>(null);
  let historyByStation = $state<Record<string, RadioTrackMetadata[]>>({});
  let radioTracksByIdentity = $state<Record<string, Track>>({});
  const radioTrackLoads = new Map<string, Promise<Track>>();
  let historyAvailabilityByStation = $state<Record<string, boolean>>({});
  let stationDetailsById = $state<Record<string, RadioStationDetails>>({});
  let loadingStations = $state(true);
  let error = $state("");
  let mounted = false;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let requestSerial = 0;
  let addOpen = $state(false);
  let dismissedExternalAdd = $state(false);
  let addMode = $state<"search" | "url">("search");
  let searchQuery = $state("");
  let countryFilter = $state("");
  let tagFilter = $state("");
  let searchResults = $state<RadioStationSearchResult[]>([]);
  let searching = $state(false);
  let addingStationId = $state("");
  let stationName = $state("");
  let stationUrl = $state("");
  let stationActionError = $state("");
  let historyQuery = $state("");
  let historySort = $state<"newest" | "oldest">("newest");
  let historyTrackSort = $state<TrackSortField>("sourceOrder");
  let historyTrackSortDirection = $state<SortDirection>("asc");
  let historyMinDuration = $state("");
  let historyMaxDuration = $state("");
  let recognizing = $state(false);
  const selectedStationId = $derived(activeStationId);
  const selectedStation = $derived(stations.find((station) => station.id === selectedStationId) ? {
    ...stations.find((station) => station.id === selectedStationId)!,
    ...(stationDetailsById[selectedStationId] ?? {}),
  } : null);
  const currentHistory = $derived(historyByStation[selectedStationId] ?? []);
  const displayedNowPlaying = $derived(metadata?.nowPlaying ?? (playingStationId !== selectedStationId ? stationDetailsById[selectedStationId]?.nowPlaying ?? null : currentHistory[0] ?? stationDetailsById[selectedStationId]?.nowPlaying ?? null));
  const showingLastIdentified = $derived(playingStationId === selectedStationId && !metadata?.nowPlaying && Boolean(displayedNowPlaying));
  const visibleHistory = $derived.by(() => {
    const query = historyQuery.trim().toLocaleLowerCase();
    return currentHistory
      .filter((item) => !query || [item.title, item.artist, item.album, stationNames[item.stationId]].some((value) => value?.toLocaleLowerCase().includes(query)))
      .map((item, index) => ({ item, index, time: Date.parse(item.observedAt ?? "") }))
      .sort((a, b) => {
        const aTime = Number.isFinite(a.time) ? a.time : (historySort === "newest" ? -Infinity : Infinity);
        const bTime = Number.isFinite(b.time) ? b.time : (historySort === "newest" ? -Infinity : Infinity);
        const delta = historySort === "oldest" ? aTime - bTime : bTime - aTime;
        return delta || a.index - b.index;
      })
      .map(({ item }) => item);
  });
  const selectedStationWebsite = $derived(selectedStation?.homepageUrl || (["main", "mellow", "rock", "global"].includes(selectedStation?.id ?? "") ? selectedStation?.sourceUrl : "") || "");
  const selectedStationFavicon = $derived(selectedStation?.faviconUrl || selectedStation?.favicon || "");
  let requestedDetailsStationId = "";
  $effect(() => {
    const stationId = selectedStation?.id;
    if (stationId && stationId !== requestedDetailsStationId) {
      requestedDetailsStationId = stationId;
      void loadStationDetails(stationId);
    }
  });

  let appliedRequestedStationId = "";
  let appliedPlayingStationId = "";
  $effect(() => {
    if (requestedStationId && requestedStationId !== appliedRequestedStationId) {
      activeStationId = requestedStationId;
      appliedRequestedStationId = requestedStationId;
      metadata = null;
      error = "";
      requestSerial++;
      if (refreshTimer) clearTimeout(refreshTimer);
    }
    if (playingStationId !== appliedPlayingStationId) {
      appliedPlayingStationId = playingStationId;
      requestSerial++;
      if (refreshTimer) clearTimeout(refreshTimer);
      if (playingStationId !== selectedStationId) metadata = null;
      else if (mounted && stations.some((station) => station.id === playingStationId)) void fetchMetadata(playingStationId);
    }
    if (showAdd) dismissedExternalAdd = false;
  });
  const addPanelVisible = $derived(addOpen || (showAdd && !dismissedExternalAdd));
  function toggleAddPanel() {
    if (addPanelVisible) { addOpen = false; dismissedExternalAdd = true; onAddClosed?.(); }
    else { dismissedExternalAdd = false; addOpen = true; }
  }
  function removable(_station: RadioStation) { return true; }
  function stationTags(station: RadioStation, max = 4) {
    return (station.tags ?? "").split(/[;,]/).map((tag) => tag.trim()).filter(Boolean).slice(0, max);
  }
  async function loadStationDetails(stationId: string) {
    try {
      const result = await call<RadioStationDetails>("get_station_details", { stationId });
      if (selectedStationId !== stationId) return;
      stationDetailsById = { ...stationDetailsById, [stationId]: result };
      if (result.historyAvailable) {
        historyByStation = { ...historyByStation, [stationId]: (result.history ?? []).slice(0, 20) };
        historyAvailabilityByStation = { ...historyAvailabilityByStation, [stationId]: true };
      }
    } catch {
      // Station details are an enhancement; the saved station remains usable offline.
    }
  }

  function metadataKey(item: RadioTrackMetadata) { return `${item.artist}\u0000${item.title}\u0000${item.album ?? ""}`.toLocaleLowerCase(); }
  function radioIdentityKey(item: RadioTrackMetadata) { return `${item.artist.trim().toLocaleLowerCase()}\n${item.title.trim().toLocaleLowerCase()}`; }
  function fallbackTrackForMetadata(item: RadioTrackMetadata): Track {
    const identity = radioIdentityKey(item);
    return {
      id: `radio_song:unresolved:${encodeURIComponent(identity)}`, provider: "radio_song", providerId: identity,
      sourceUrl: "", artist: item.artist.trim(), title: item.title.trim(), album: item.album ?? undefined,
      releaseDate: item.year ?? undefined, artworkUrl: item.artworkUrl ?? undefined, favorite: false,
      provenance: { sourceKind: "radioSong", identity: "unverified station metadata; canonical track lookup unavailable", radioStationId: item.stationId, radioMetadataConfidence: item.metadataConfidence, radioObservedAt: item.observedAt ?? null },
    };
  }
  function withOccurrence(track: Track, item: RadioTrackMetadata): Track {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    return {
      ...track,
      favorite: favoriteTrackIds.includes(track.id),
      provenance: { ...provenance, stationId: item.stationId, observedAt: item.observedAt ?? null, radioStationId: item.stationId, radioStationName: stationNames[item.stationId] ?? provenance.radioStationName ?? "Radio", radioObservedAt: item.observedAt ?? provenance.radioObservedAt ?? null, radioMetadataConfidence: item.metadataConfidence ?? provenance.radioMetadataConfidence },
    };
  }
  async function hydrateRadioSong(item: RadioTrackMetadata): Promise<Track> {
    const identity = radioIdentityKey(item);
    const cached = radioTracksByIdentity[identity];
    if (cached) return withOccurrence(cached, item);
    let pending = radioTrackLoads.get(identity);
    if (!pending) {
      pending = call<Track>("get_radio_song_track", { stationId: item.stationId, metadata: item })
        .catch(() => fallbackTrackForMetadata(item))
        .then((track) => {
          radioTracksByIdentity = { ...radioTracksByIdentity, [identity]: track };
          return track;
        })
        .finally(() => radioTrackLoads.delete(identity));
      radioTrackLoads.set(identity, pending);
    }
    return withOccurrence(await pending, item);
  }
  function trackForMetadata(item: RadioTrackMetadata): Track {
    return withOccurrence(radioTracksByIdentity[radioIdentityKey(item)] ?? fallbackTrackForMetadata(item), item);
  }
  function metadataForTrack(track: Track) { return visibleHistory.find(item => radioIdentityKey(item) === radioIdentityKey({ stationId: "", artist: track.artist, title: track.title, sourceUrl: "", metadataConfidence: "" })); }
  async function hydrateHistoryTrack(track: Track) {
    const item = metadataForTrack(track);
    return item ? hydrateRadioSong(item) : track;
  }
  async function playHistoryTrack(_track: Track, displayedTracks: Track[], index: number) {
    const tracks = await Promise.all(displayedTracks.map(hydrateHistoryTrack));
    if (tracks[index]) await onPlayTrack?.(tracks[index], tracks, index);
  }
  function downloadHistoryTrack(track: Track) { void hydrateHistoryTrack(track).then(canonical => onDownloadTrack?.(canonical)); }
  function favoriteHistoryTrack(track: Track) { void hydrateHistoryTrack(track).then(canonical => onFavoriteTrack?.(canonical)); }
  function playlistHistoryTrack(track: Track) { void hydrateHistoryTrack(track).then(canonical => onAddTrackToPlaylist?.(canonical)); }
  function queueHistoryTrack(track: Track, mode: "next" | "append") { void hydrateHistoryTrack(track).then(canonical => onQueueTracks?.([canonical], mode)); }
  function artistHistoryTrack(track: Track) { void hydrateHistoryTrack(track).then(canonical => onArtist(canonical)); }
  function contextHistoryTrack(track: Track, event: MouseEvent) { void hydrateHistoryTrack(track).then(canonical => onTrackContext?.(canonical, event)); }
  function infoHistoryTrack(track: Track) { void hydrateHistoryTrack(track).then(canonical => onInfoTrack?.(canonical)); }
  function openArtistForMetadata(item: RadioTrackMetadata) {
    void hydrateRadioSong(item).then(track => onArtist(track));
  }
  $effect(() => {
    for (const item of visibleHistory) {
      const identity = radioIdentityKey(item);
      if (!radioTracksByIdentity[identity] && !radioTrackLoads.has(identity)) void hydrateRadioSong(item);
    }
  });
  const stationHistoryTracks = $derived.by(() => {
    const lower = parseDurationBound(historyMinDuration), upper = parseDurationBound(historyMaxDuration);
    const tracks = filterTracks(visibleHistory.map(trackForMetadata), {
      ...(lower === null ? {} : { minDurationSeconds: lower }),
      ...(upper === null ? {} : { maxDurationSeconds: upper }),
    });
    return sortTracks(tracks, historyTrackSort, historyTrackSortDirection);
  });
  function historyTimestamp(track: Track) {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const observedAt = provenance.observedAt ?? provenance.radioObservedAt;
    if (typeof observedAt !== "string") return { label: "Listening time unavailable", full: "Listening time unavailable" };
    const date = new Date(observedAt);
    if (!Number.isFinite(date.getTime())) return { label: "Listening time unavailable", full: "Listening time unavailable" };
    const full = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit", timeZoneName: "long" }).format(date);
    const label = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit", timeZoneName: "short" }).format(date);
    return { label, full };
  }
  const sortAvailability = $derived(trackSortAvailability(stationHistoryTracks));
  function rememberPrevious(stationId: string, previous: RadioTrackMetadata | null, next: RadioTrackMetadata | null) {
    if (!previous || !next || metadataKey(previous) === metadataKey(next)) return;
    const history = historyByStation[stationId] ?? [];
    if (history.some((item) => metadataKey(item) === metadataKey(previous))) return;
    historyByStation = { ...historyByStation, [stationId]: [previous, ...history].slice(0, 12) };
  }
  async function fetchMetadata(stationId: string) {
    if (playingStationId !== stationId || selectedStationId !== stationId) return;
    const request = ++requestSerial;
    try {
      const result = await call<RadioMetadata>("poll_radio_station", { stationId, recognize: true });
      if (!mounted || request !== requestSerial || selectedStationId !== stationId || playingStationId !== stationId) return;
      if (!result.historyAvailable) rememberPrevious(stationId, metadata?.stationId === stationId ? metadata.nowPlaying : null, result.nowPlaying);
      else historyByStation = { ...historyByStation, [stationId]: result.history.slice(0, 20) };
      historyAvailabilityByStation = { ...historyAvailabilityByStation, [stationId]: result.historyAvailable };
      metadata = result;
      error = "";
      const station = stations.find((item) => item.id === stationId);
      if (station) onNowPlaying?.(station, result.nowPlaying);
    } catch (failure) {
      if (mounted && request === requestSerial) {
        error = String(failure);
        metadata = null;
        const station=stations.find(item=>item.id===stationId);
        if(station && playingStationId===stationId)onNowPlaying?.(station,null);
      }
    } finally {
      if (mounted && request === requestSerial && selectedStationId === stationId && playingStationId === stationId) {
        const wait = metadata?.secondsRemaining == null ? 30_000 : Math.max(15_000, Math.min(60_000, metadata.secondsRemaining * 1000 + 2_000));
        refreshTimer = setTimeout(() => void fetchMetadata(stationId), wait);
      }
    }
  }
  function chooseStation(station: RadioStation) {
    activeStationId = station.id;
    metadata = null;
    error = "";
    if (refreshTimer) clearTimeout(refreshTimer);
    requestSerial++;
    onPlayStation(station);
  }
  async function identifyCurrentSong(stationId: string) {
    if (playingStationId !== stationId || recognizing) return;
    recognizing = true;
    error = "";
    try {
      const result = await call<RadioMetadata>("identify_radio_station", { stationId });
      if (!mounted || selectedStationId !== stationId || playingStationId !== stationId) return;
      if (result.historyAvailable) historyByStation = { ...historyByStation, [stationId]: result.history.slice(0, 20) };
      historyAvailabilityByStation = { ...historyAvailabilityByStation, [stationId]: result.historyAvailable };
      metadata = result;
      const station = stations.find((item) => item.id === stationId);
      if (station) onNowPlaying?.(station, result.nowPlaying);
    } catch (failure) { error = String(failure); }
    finally { recognizing = false; }
  }
  async function loadStations() {
    loadingStations = true;
    try {
      const result = await call<RadioStation[]>("get_radio_stations");
      if (!mounted) return;
      stations = result;
      onStationsChanged?.(stations);
      if (!stations.some((station) => station.id === selectedStationId)) activeStationId = stations[0]?.id ?? "";
      if (mounted && selectedStationId && selectedStationId === playingStationId) void fetchMetadata(selectedStationId);
    } catch (failure) { error = String(failure); }
    finally { loadingStations = false; }
  }
  async function searchStations() {
    const query = searchQuery.trim();
    const country = countryFilter.trim();
    const tag = tagFilter.trim();
    if (!query && !country && !tag) { searchResults = []; return; }
    searching = true; stationActionError = "";
    try { searchResults = await call<RadioStationSearchResult[]>("search_radio_stations", { query, country: country || null, tag: tag || null, limit: 20 }); }
    catch (failure) { stationActionError = String(failure); }
    finally { searching = false; }
  }
  async function addSearchStation(station: RadioStationSearchResult) {
    addingStationId = station.id; stationActionError = "";
    try {
      stations = await call<RadioStation[]>("add_radio_station", { station });
      onStationsChanged?.(stations);
      activeStationId = station.id; addOpen = false; dismissedExternalAdd = true; onAddClosed?.();
    } catch (failure) { stationActionError = String(failure); }
    finally { addingStationId = ""; }
  }
  async function addCustomStation() {
    if (!stationName.trim() || !stationUrl.trim()) return;
    stationActionError = "";
    try {
      stations = await call<RadioStation[]>("add_custom_radio_station", { name: stationName.trim(), url: stationUrl.trim() });
      onStationsChanged?.(stations);
      const added = stations.find((item) => item.id.startsWith("custom:") && item.name === stationName.trim()) ?? stations[stations.length - 1];
      stationName = ""; stationUrl = ""; addOpen = false; dismissedExternalAdd = true; onAddClosed?.();
      if (added) activeStationId = added.id;
    } catch (failure) { stationActionError = String(failure); }
  }
  async function removeStation(station: RadioStation) {
    stationActionError = "";
    try {
      stations = await call<RadioStation[]>("remove_radio_station", { stationId: station.id });
      onStationsChanged?.(stations);
      if (activeStationId === station.id) {
        activeStationId = stations[0]?.id ?? "";
        metadata = null;
        if (activeStationId && activeStationId === playingStationId) void fetchMetadata(activeStationId);
      }
    } catch (failure) { stationActionError = String(failure); }
  }
  onMount(() => {
    mounted = true;
    void loadStations();
    return () => { mounted = false; requestSerial++; if (refreshTimer) clearTimeout(refreshTimer); };
  });
</script>

<section class="radio-page" aria-label="Radio">
  <header class="page-heading"><div class="station-identity">{#if selectedStationFavicon}<img class="station-favicon" src={selectedStationFavicon} alt="" />{/if}<div><p class="eyebrow">LIVE RADIO</p><h1>{selectedStation?.name ?? "Radio"}</h1><p>{selectedStation?.description || (selectedStation?.country ? `Live internet radio from ${selectedStation.country}.` : "Live internet radio stream.")}</p></div></div><div class="heading-actions">{#if selectedStationWebsite}<a href={selectedStationWebsite} target="_blank" rel="noreferrer">Station website ↗</a>{/if}<button class="add-button" onclick={toggleAddPanel} aria-expanded={addPanelVisible}>{addPanelVisible ? "Close" : "＋ Add station"}</button></div></header>
  {#if addPanelVisible}
    <section class="station-add" aria-label="Add radio station">
      <div class="add-tabs" role="tablist" aria-label="Add station method"><button role="tab" aria-selected={addMode === "search"} class:active={addMode === "search"} onclick={() => (addMode = "search")}>Search</button><button role="tab" aria-selected={addMode === "url"} class:active={addMode === "url"} onclick={() => (addMode = "url")}>Stream URL</button></div>
      {#if addMode === "search"}
        <form class="station-search" onsubmit={(event) => { event.preventDefault(); void searchStations(); }}><label>Search internet radio <span><input aria-label="Search internet radio" bind:value={searchQuery} placeholder="Station name or keyword" /><button disabled={searching || (!searchQuery.trim() && !countryFilter.trim() && !tagFilter.trim())}>{searching ? "Searching…" : "Search"}</button></span></label><div class="radio-filters"><label>Country<input aria-label="Station country filter" bind:value={countryFilter} placeholder="Any country" /></label><label>Genre<input aria-label="Station genre filter" bind:value={tagFilter} placeholder="Any genre" /></label></div></form>
        {#if searchResults.length}<ul class="search-results" aria-label="Station search results">{#each searchResults as station (station.id)}<li><div><strong>{station.name}</strong><small>{[station.country, stationTags(station, 3).join(" · "), station.codec, (station.bitrateKbps ?? station.bitrate) ? `${station.bitrateKbps ?? station.bitrate} kbps` : ""].filter(Boolean).join(" · ") || "Internet radio"}</small></div><button disabled={addingStationId === station.id} onclick={() => void addSearchStation(station)}>{addingStationId === station.id ? "Adding…" : "Add"}</button></li>{/each}</ul>{:else if (searchQuery.trim() || countryFilter.trim() || tagFilter.trim()) && !searching}<p class="radio-empty">No matching stations yet. Try a country or genre.</p>{/if}
      {:else}
        <form class="station-search custom-station" onsubmit={(event) => { event.preventDefault(); void addCustomStation(); }}><label>Station name<input aria-label="Station name" bind:value={stationName} /></label><label>Stream URL<input aria-label="Stream URL" type="url" bind:value={stationUrl} placeholder="https://…" /></label><button disabled={!stationName.trim() || !stationUrl.trim()}>Add stream</button><small>Use a direct audio stream URL. HLS playlists are supported when the app can play them.</small></form>
      {/if}
      {#if stationActionError}<p role="alert" class="error">{stationActionError}</p>{/if}
    </section>
  {/if}
  {#if loadingStations && !stations.length}<p role="status">Loading stations…</p>{/if}
  {#if stations.length && !requestedStationId}<nav class="stations" aria-label="Radio stations">
    {#each stations as station (station.id)}
      <button class="station-card" class:active={selectedStationId === station.id} aria-pressed={selectedStationId === station.id} onclick={() => void chooseStation(station)}>
        <span class="station-mark" aria-hidden="true">♫</span><span class="station-copy"><strong>{station.name}</strong><small>{[station.country, station.codec, (station.bitrateKbps ?? station.bitrate) ? `${station.bitrateKbps ?? station.bitrate} kbps` : ""].filter(Boolean).join(" · ") || (playingStationId === station.id ? "Playing" : "Live stream")}</small></span><span class="play-icon" aria-hidden="true">{playingStationId === station.id ? "♫" : "▶"}</span>
      </button>
      {#if removable(station)}<button class="remove-station" aria-label={`Remove ${station.name}`} onclick={() => void removeStation(station)}>Remove</button>{/if}
    {/each}
  </nav>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if selectedStation}
    <section class="radio-now-playing" aria-label={`Now playing on ${selectedStation.name}`} aria-live="polite">
      <p class="eyebrow">{showingLastIdentified ? "LAST IDENTIFIED" : "NOW PLAYING"} · {selectedStation.name.toLocaleUpperCase()}</p>
      {#if selectedStation.country || selectedStation.tags?.length || selectedStation.codec || selectedStation.bitrateKbps || selectedStation.bitrate || selectedStation.language}
        <p class="station-details">{[selectedStation.country, selectedStation.language, stationTags(selectedStation).join(" · "), selectedStation.codec, (selectedStation.bitrateKbps ?? selectedStation.bitrate) ? `${selectedStation.bitrateKbps ?? selectedStation.bitrate} kbps` : ""].filter(Boolean).join(" · ")}</p>
      {/if}
      <div class="station-actions"><button class="add-button" onclick={() => onPlayStation(selectedStation)}>▶ Play station</button>{#if removable(selectedStation)}<button class="remove-station" aria-label={`Remove ${selectedStation.name}`} onclick={() => void removeStation(selectedStation)}>Remove station</button>{/if}</div>
      {#if playingStationId === selectedStation.id}<button class="identify-button" disabled={recognizing} onclick={() => void identifyCurrentSong(selectedStation.id)}>{recognizing ? "Identifying…" : "Identify current song"}</button><small class="recognition-note">Identify the current song manually. A short stream sample is fingerprinted through SongRec and sent to Shazam. No microphone is used.</small>{/if}
      {#if displayedNowPlaying}
        {@const item = displayedNowPlaying}
        <div class="radio-track-card">
          {#if item.artworkUrl}<img src={item.artworkUrl} alt={`Album artwork for ${item.title}`} />{:else}<div class="art-placeholder" aria-label="No artwork available">♫</div>{/if}
          <div class="radio-track-copy"><h2>{item.title}</h2><button class="artist-link" aria-label={`Search artist information for ${item.artist}`} onclick={() => openArtistForMetadata(item)}>{item.artist}</button>{#if item.album}<p>{item.album}{item.year ? ` · ${item.year}` : ""}</p>{/if}<details class="confidence-details"><summary title={item.metadataConfidence}>Details</summary><small>{item.metadataConfidence}. Catalog identity is unverified.</small></details></div>
        </div>
      {:else}<p class="radio-empty">{playingStationId === selectedStation.id ? "Current track information is unavailable." : "Play this station to see its current track."}</p>{/if}
      {#if metadata?.note || stationDetailsById[selectedStationId]?.note}<details class="confidence-details"><summary title={metadata?.note ?? stationDetailsById[selectedStationId]?.note}>Station details</summary><small>{metadata?.note ?? stationDetailsById[selectedStationId]?.note}</small></details>{/if}
    </section>
  {/if}
  {#if selectedStation}
    <section class="history" aria-label={`Recently heard on ${selectedStation?.name ?? "Radio Paradise"}`}>
      <h2>Station history <small>your listening history</small></h2>
      <div class="history-tools"><label>Search station history<input aria-label="Search station history" type="search" bind:value={historyQuery} placeholder="Title or artist" /></label><label>History order<select aria-label="Sort station history" bind:value={historySort}><option value="newest">Newest first</option><option value="oldest">Oldest first</option></select></label><label>Sort tracks<select aria-label="Sort station history tracks" bind:value={historyTrackSort}><option value="sourceOrder">History order</option><option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabel(sortAvailability, "title", "Title")}</option><option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabel(sortAvailability, "artist", "Artist")}</option><option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabel(sortAvailability, "album", "Album")}</option><option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabel(sortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabel(sortAvailability, "releaseDate", "Release date")}</option><option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabel(sortAvailability, "dateAdded", "Date added")}</option><option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabel(sortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabel(sortAvailability, "key", "Key")}</option><option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabel(sortAvailability, "plays", "Plays")}</option><option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabel(sortAvailability, "views", "Views")}</option></select></label><label>Direction<select aria-label="Sort direction" bind:value={historyTrackSortDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label><label>Duration from<input aria-label="Minimum track duration" inputmode="numeric" placeholder="mm:ss" bind:value={historyMinDuration} /></label><label>to<input aria-label="Maximum track duration" inputmode="numeric" placeholder="mm:ss" bind:value={historyMaxDuration} /></label>{#if onOpenHistory}<button class="full-history" onclick={() => onOpenHistory?.(selectedStation.id)}>Full listening history →</button>{/if}</div>
      {#if stationHistoryTracks.length}<TrackList tracks={stationHistoryTracks} listLabel={`${selectedStation.name} listening history`} onPlay={playHistoryTrack} onDownload={downloadHistoryTrack} onFavorite={favoriteHistoryTrack} onNext={track => queueHistoryTrack(track, "next")} onQueue={track => queueHistoryTrack(track, "append")} onPlaylist={playlistHistoryTrack} onArtist={artistHistoryTrack} onInfo={infoHistoryTrack} onContext={contextHistoryTrack} timestamp={historyTimestamp} meta={() => `Via ${selectedStation.name}`} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />{:else if currentHistory.length}<p class="radio-empty">No recent tracks match this filter.</p>{:else}<p class="radio-empty">Songs identified while you listen will appear here.</p>{/if}
    </section>
  {/if}
</section>

<style>
  .radio-page{box-sizing:border-box;flex:1 1 auto;min-height:0;min-width:0;width:100%;overflow-y:auto;overscroll-behavior:contain;overflow-wrap:anywhere;max-width:1120px;margin:0 auto;padding:clamp(18px,4vw,42px);color:var(--text);container-type:inline-size}.page-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:16px;margin-bottom:24px}.page-heading h1{margin:3px 0;font-size:clamp(24px,4vw,34px)}.page-heading p:not(.eyebrow){margin:5px 0;color:var(--muted)}.heading-actions{display:flex;align-items:center;gap:12px}.heading-actions>a{color:var(--accent);font-size:13px}.add-button,.station-search button,.search-results button,.remove-station{border:1px solid var(--line);border-radius:7px;padding:7px 11px;background:var(--raised);color:var(--text)}.eyebrow{margin:0;color:var(--muted);font-size:10px;font-weight:650;letter-spacing:.12em}.stations{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,205px),1fr));gap:10px}.station-card{display:flex;align-items:center;gap:12px;min-height:78px;padding:14px;border:1px solid var(--line);border-radius:11px;background:var(--surface);color:var(--text);text-align:left}.station-card.active{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}.station-mark{display:grid;place-items:center;width:40px;height:40px;border-radius:9px;background:var(--raised);font-size:22px}.station-copy{display:grid;gap:5px;flex:1;min-width:0}.station-copy small,.radio-track-copy small{color:var(--muted);font-size:11px}.play-icon{color:var(--accent)}.station-add{margin:0 0 18px;padding:14px;border:1px solid var(--line);border-radius:12px;background:var(--surface)}.add-tabs{display:flex;gap:5px;margin-bottom:12px}.add-tabs button{border:0;border-radius:6px;padding:8px 12px;background:transparent;color:var(--muted)}.add-tabs button.active{background:var(--raised);color:var(--text)}.station-search label{display:grid;gap:6px;color:var(--muted);font-size:12px}.station-search label span{display:flex;gap:8px}.station-search input{min-width:0;width:100%;padding:9px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text)}.radio-filters{display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-top:9px}.custom-station{display:grid;grid-template-columns:1fr 1.4fr auto;align-items:end;gap:10px}.custom-station small{grid-column:1/-1;color:var(--muted)}.search-results{list-style:none;margin:10px 0 0;padding:0;max-height:280px;overflow:auto}.search-results li{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 0;border-top:1px solid var(--line)}.search-results li div{display:grid;gap:3px;min-width:0}.search-results li small{color:var(--muted);font-size:11px}.remove-station{padding:4px 7px;font-size:10px}.radio-now-playing,.history{margin-top:26px;padding:18px;border:1px solid var(--line);border-radius:12px;background:var(--surface)}.station-details{margin:7px 0;color:var(--muted);font-size:12px;line-height:1.5}.station-actions{display:flex;gap:8px;margin:10px 0}.radio-track-card{display:grid;grid-template-columns:128px minmax(0,1fr);gap:18px;align-items:center;margin-top:12px}.radio-track-card img,.art-placeholder{width:128px;aspect-ratio:1;object-fit:cover;border-radius:9px;background:var(--raised)}.art-placeholder{display:grid;place-items:center;color:var(--muted);font-size:44px}.radio-track-copy{min-width:0}.radio-track-copy h2{margin:0 0 6px;font-size:22px}.radio-track-copy p{margin:8px 0;color:var(--muted);font-size:13px}.artist-link{border:0;padding:0;background:transparent;color:var(--accent);text-align:left;font-size:16px;text-decoration:underline}.radio-empty{color:var(--muted);font-size:12px;line-height:1.5}.history h2{margin:0 0 9px;font-size:15px}.history h2 small{color:var(--muted);font-weight:400}.error{padding:12px;border:1px solid #a44;border-radius:8px;color:#ff9c9c}@media(max-width:560px){.page-heading{display:block}.heading-actions{margin-top:10px;flex-wrap:wrap}.radio-track-card{grid-template-columns:88px minmax(0,1fr);gap:12px}.radio-track-card img,.art-placeholder{width:88px}.radio-track-copy h2{font-size:18px}.radio-now-playing,.history{padding:13px}.custom-station{grid-template-columns:1fr}.custom-station small{grid-column:auto}.radio-filters{grid-template-columns:1fr}}
  @container (max-width: 640px){.page-heading{flex-wrap:wrap}.heading-actions{flex-wrap:wrap}.station-identity{flex:1 1 260px}.station-actions{flex-wrap:wrap}.radio-track-card{grid-template-columns:88px minmax(0,1fr);gap:12px}.radio-track-card img,.art-placeholder{width:88px}.radio-track-copy h2{overflow-wrap:anywhere;font-size:18px}.radio-now-playing,.history{padding:13px}}
  .station-identity{display:flex;align-items:center;gap:12px;min-width:0}.station-favicon{width:38px;height:38px;object-fit:contain;border-radius:7px}.station-identity>div{min-width:0}.confidence-details{margin-top:7px;color:var(--muted);font-size:11px}.confidence-details summary{cursor:pointer}.confidence-details small{display:block;margin-top:4px;max-width:42ch}.identify-button{border:1px solid var(--line);border-radius:7px;padding:7px 11px;background:var(--raised);color:var(--text)}.recognition-note{display:block;margin-top:5px;color:var(--muted);font-size:11px}
  .history-tools{display:flex;align-items:end;flex-wrap:wrap;gap:10px;margin:0 0 12px}.history-tools label{display:grid;gap:4px;color:var(--muted);font-size:11px}.history-tools input,.history-tools select{min-width:145px;padding:7px 9px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);font:inherit}.full-history{margin-left:auto;border:0;padding:7px 0;background:transparent;color:var(--accent);font:inherit;font-size:12px;cursor:pointer}
</style>
