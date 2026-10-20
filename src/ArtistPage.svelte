<script lang="ts">
  import TrackDurationFilter from "./TrackDurationFilter.svelte";
  import { allTrackSortFieldsAvailable, trackSortAvailability, trackSortAvailabilityForSources, trackSortOptionLabel, trackSortOptionLabelForSources } from "./lib/track-sort-availability";
  import type { Track } from "./lib/types";
  import TrackList from "./TrackList.svelte";
  import ArtworkImage from "./ArtworkImage.svelte";
  import { musicBrainzCover, trackArtworkSources } from "./lib/catalog-artwork";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";
  import type {
    ArtistPageProps, ArtistRef, ArtistSearchResult, ArtistProfile, ArtistReleaseRef,
    ArtistReleasePage, ArtistTrackPage, ArtistCollaborationPage, MetadataReview,
    ArtistLocalAssetsPageRequest, ArtistLocalAssetsPage, MetadataFieldKey,
  } from "./lib/artist-types";
  import { onMount, onDestroy, tick } from "svelte";
  import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";

  let {
    activeTrack,
    artistName = null,
    initialArtistRef = null,
    initialSection = "overview",
    initialScroll = 0,
    onSectionChange,
    onArtistSelected,
    showSearchResultsButton = true,
    onScroll,
    localTracks,
    onLoadLocalAssetsPage,
    searchArtists,
    getArtistProfile,
    getArtistReleases,
    getReleaseTracks,
    getArtistTracks,
    getArtistCollaborations,
    resolveRecordingMetadata,
    onApplyMetadata,
    onPlayTrack,
    onTrackContext,
    onQueueTracks,
    onDownloadTrack,
    onFavoriteTrack,
    onDownloadAlbum,
    onSaveAlbumToLibrary,
    onBrowseArtistProvider,
    onAddTrackToPlaylist,
    onAddSelectedToPlaylist,
    onArtistNavigate,
    onTrackAlbum,
    playingTrack = null,
    bufferingTrack = null,
    playingTrackId = "",
    bufferingTrackId = "",
    paused = false,
  }: ArtistPageProps & { showSearchResultsButton?: boolean; onDownloadAlbum?: (tracks: Track[], release: ArtistReleaseRef) => void | Promise<void>; onSaveAlbumToLibrary?: (tracks: Track[], release: ArtistReleaseRef) => Promise<void>; onBrowseArtistProvider?: (name: string, provider: "spotify" | "musicbrainz") => void | Promise<void>; onTrackAlbum?: (track: Track) => void | Promise<void> } = $props();

  let query = $state("");
  let results = $state<ArtistSearchResult[]>([]);
  let searchWarnings = $state<string[]>([]);
  let searchPage = $state(0);
  let searchHasMore = $state(false);
  let selectedArtist = $state<ArtistSearchResult | null>(null);
  let profile = $state<ArtistProfile | null>(null);
  const providerLabels: Record<string, string> = { musicbrainz: "MusicBrainz", discogs: "Discogs", lastfm: "Last.fm" };
  const providerLabel = (value: string) => providerLabels[value] ?? value;
  const coverageLabel = (value: string) => value.replaceAll("_", " ");
  let releases = $state<ArtistReleasePage | null>(null);
  let tracks = $state<ArtistTrackPage | null>(null);
  let collaborations = $state<ArtistCollaborationPage | null>(null);
  let section = $state<"overview" | "releases" | "tracks" | "collaborations" | "local">("overview");
  let busy = $state(false);
  let error = $state("");
  let review = $state<MetadataReview | null>(null);
  let selectedFields = $state<MetadataFieldKey[]>([]);
  let requestGeneration = 0;
  let releaseFilter = $state<"Albums" | "EPs" | "Singles" | "All">("Albums");
  let releaseQuery = $state("");
  let releaseProvider = $state("All sources");
  let releaseSort = $state<"newest" | "oldest" | "title">("newest");
  let loadingReleaseOrder = $state(false);
  let trackQuery = $state("");
  let trackSort = $state<TrackSortField>("sourceOrder");
  let trackSortDirection = $state<SortDirection>("asc");
  let trackMinDuration = $state("");
  let trackMaxDuration = $state("");
  let releaseTrackQuery = $state("");
  let releaseTrackSort = $state<TrackSortField>("sourceOrder");
  let releaseTrackSortDirection = $state<SortDirection>("asc");
  let releaseTrackMinDuration = $state("");
  let releaseTrackMaxDuration = $state("");
  let trackAvailability = $state<"all" | "on-device" | "not-on-device">("all");
  let topTrackQuery = $state("");
  let topTrackSort = $state<TrackSortField>("sourceOrder");
  let topTrackDirection = $state<SortDirection>("asc");
  let topTrackMinDuration = $state("");
  let topTrackMaxDuration = $state("");
  let localArtistTracks = $state<Track[]>([]);
  let localArtistTotal = $state(0);
  let localArtistSortAvailability = $state<Partial<Record<TrackSortField, boolean>> | null>(null);
  let localArtistQuery = $state("");
  let localArtistSort = $state<TrackSortField>("sourceOrder");
  let localArtistDirection = $state<SortDirection>("asc");
  let localArtistMinDuration = $state("");
  let localArtistMaxDuration = $state("");
  let localArtistAvailability = $state<"all" | "on-device" | "not-on-device">("all");
  let localArtistLoading = $state(false);
  let localArtistError = $state("");
  let localArtistRequestGeneration = 0;
  let expandedRelease = $state("");
  let editionByGroup = $state<Record<string, string>>({});
  let releaseTrackCache = $state<Record<string, Track[]>>({});
  let loadingRelease = $state<Record<string, boolean>>({});
  let albumSaveState = $state<Record<string, { status: "saving" | "saved" | "error"; message: string }>>({});
  let releaseLoadError = $state("");
  const releaseTrackLoads = new Map<string, Promise<Track[]>>();
  let lastInitialSection = $state<"overview" | "releases" | "tracks" | "collaborations" | "local">("overview");
  let pageElement = $state<HTMLElement | null>(null);
  let lastInitialScroll = $state(0);
  let scrollRestorePending = $state(true);
  let lastInitialArtistKey = $state<string | null>(null);
  let lastInitialArtistName = $state("");

  onMount(() => {
    if (!initialArtistRef && !artistName?.trim()) {
      const initialName = activeTrack?.artist?.trim();
      if (initialName) { query = initialName; void runSearch(); }
    }
  });

  let navigationRestoreGeneration = 0;
  let navigationDestroyed = false;
  onDestroy(() => { navigationDestroyed=true; navigationRestoreGeneration++; requestGeneration++; localArtistRequestGeneration++; });
  function nativeNavigationSnapshot() {
    return { controls:{query, section, releaseFilter, releaseQuery, releaseProvider, releaseSort, trackQuery, trackSort, trackSortDirection, trackMinDuration, trackMaxDuration, releaseTrackQuery, releaseTrackSort, releaseTrackSortDirection, releaseTrackMinDuration, releaseTrackMaxDuration, trackAvailability, topTrackQuery, topTrackSort, topTrackDirection, topTrackMinDuration, topTrackMaxDuration, localArtistQuery, localArtistSort, localArtistDirection, localArtistMinDuration, localArtistMaxDuration, localArtistAvailability, expandedRelease, editionByGroup:{...editionByGroup}}, selectedArtist, profile, releases, tracks, collaborations, results, searchPage, searchHasMore, searchWarnings,
      localArtistTracks,localArtistTotal,localArtistSortAvailability,releaseTrackCache:{...releaseTrackCache},
      loadedReleases:releases?.items.length??0,loadedTracks:tracks?.items.length??0,loadedCollaborations:collaborations?.items.length??0,loadedLocal:localArtistTracks.length,loadedSearch:results.length,
      scroll:pageElement?.scrollTop??0,needsRefetch:false };
  }
  export function captureNavigationSnapshot(key:string):string {
    const snapshot=nativeNavigationSnapshot();
    const rows=results.length+(profile?.topTracks.length??0)+(releases?.items.length??0)+(tracks?.items.length??0)+(collaborations?.items.length??0)+localArtistTracks.length+Object.values(releaseTrackCache).reduce((sum,rows)=>sum+rows.length,0);
    providerNavigationSnapshots.put({kind:"artist-page",key,accountScope:"public-catalog",snapshot,rows,stripRows:value=>({...value,profile:null,releases:null,tracks:null,collaborations:null,results:[],localArtistTracks:[],releaseTrackCache:{},needsRefetch:true})});
    return key;
  }
  export async function restoreNavigationSnapshot(key:string) {
    const saved=providerNavigationSnapshots.get<ReturnType<typeof nativeNavigationSnapshot>>("artist-page",key,"public-catalog");
    if(!saved)return;
    const restore=++navigationRestoreGeneration;
    requestGeneration++;localArtistRequestGeneration++;localArtistLoading=false;busy=false;
    const isCurrent=()=>!navigationDestroyed&&restore===navigationRestoreGeneration;
    const guarded = async (operation:()=>Promise<unknown>) => {
      const pending=operation();
      const controls=JSON.stringify(nativeNavigationSnapshot().controls),generation=requestGeneration;
      await pending;
      if(!isCurrent()||generation!==requestGeneration||controls!==JSON.stringify(nativeNavigationSnapshot().controls)){navigationRestoreGeneration++;return false;}
      return true;
    };
    const applyControls=()=>{ ({query, section, releaseFilter, releaseQuery, releaseProvider, releaseSort, trackQuery, trackSort, trackSortDirection, trackMinDuration, trackMaxDuration, releaseTrackQuery, releaseTrackSort, releaseTrackSortDirection, releaseTrackMinDuration, releaseTrackMaxDuration, trackAvailability, topTrackQuery, topTrackSort, topTrackDirection, topTrackMinDuration, topTrackMaxDuration, localArtistQuery, localArtistSort, localArtistDirection, localArtistMinDuration, localArtistMaxDuration, localArtistAvailability, expandedRelease}=saved.controls);editionByGroup={...saved.controls.editionByGroup}; };
    selectedArtist=saved.selectedArtist;applyControls();
    if(saved.needsRefetch){
      if(saved.selectedArtist){
        if(!await guarded(()=>openArtist(saved.selectedArtist!)))return;applyControls();
        if(!await guarded(()=>loadSection(saved.controls.section)))return;
        const deadline=Date.now()+15_000;
        while(isCurrent()&&Date.now()<deadline&&releases?.nextCursor&&(releases.items.length<saved.loadedReleases)){const before=releases.items.length;if(!await guarded(moreReleases))return;if(!isCurrent()||releases.items.length<=before)break;}
        while(isCurrent()&&Date.now()<deadline&&tracks?.nextCursor&&(tracks.items.length<saved.loadedTracks)){const before=tracks.items.length;if(!await guarded(moreTracks))return;if(!isCurrent()||tracks.items.length<=before)break;}
        while(isCurrent()&&Date.now()<deadline&&collaborations?.nextCursor&&(collaborations.items.length<saved.loadedCollaborations)){const before=collaborations.items.length;if(!await guarded(moreCollaborations))return;if(collaborations.items.length<=before)break;}
        if(saved.controls.section==="local"&&onLoadLocalAssetsPage){if(!await guarded(()=>loadLocalArtistPage(false)))return;while(isCurrent()&&Date.now()<deadline&&localArtistTracks.length<Math.min(saved.loadedLocal,localArtistTotal)){const before=localArtistTracks.length;if(!await guarded(()=>loadLocalArtistPage(true)))return;if(localArtistTracks.length<=before)break;}}
        if(!isCurrent())return;applyControls();
        const group=releaseGroups.find(group=>group.key===saved.controls.expandedRelease);
        if(group){if(!await guarded(()=>loadReleaseTracks(selectedEdition(group))))return;}
      } else {if(!await guarded(runSearch))return;const deadline=Date.now()+15_000;while(isCurrent()&&Date.now()<deadline&&searchHasMore&&results.length<saved.loadedSearch){const before=results.length;if(!await guarded(moreSearchResults))return;if(results.length<=before)break;}}
    } else {
      profile=saved.profile;releases=saved.releases;tracks=saved.tracks;collaborations=saved.collaborations;results=saved.results;searchPage=saved.searchPage;searchHasMore=saved.searchHasMore;searchWarnings=saved.searchWarnings;
      localArtistTracks=saved.localArtistTracks;localArtistTotal=saved.localArtistTotal;localArtistSortAvailability=saved.localArtistSortAvailability;releaseTrackCache={...saved.releaseTrackCache};
    }
    if(!isCurrent())return;scrollRestorePending=false;await tick();if(isCurrent()&&pageElement)pageElement.scrollTop=saved.scroll;
  }

  $effect(() => {
    const initialSectionChanged = lastInitialSection !== initialSection;
    if (initialSectionChanged) {
      lastInitialSection = initialSection;
      section = initialSection;
    }
    const artistKey = initialArtistRef ? `${initialArtistRef.provider}:${initialArtistRef.id}` : null;
    const artistChanged = artistKey !== lastInitialArtistKey;
    if (artistChanged) {
      const previousArtistKey = lastInitialArtistKey;
      lastInitialArtistKey = artistKey;
      if (!artistKey && previousArtistKey) {
        requestGeneration++;
        selectedArtist = null; profile = null; releases = null; tracks = null; collaborations = null;
        const restoreQuery = artistName?.trim() || query.trim();
        if (restoreQuery) { query = restoreQuery; void runSearch(); }
        return;
      }
    }
    if (initialArtistRef && (selectedArtist?.artist.provider !== initialArtistRef.provider || selectedArtist?.artist.id !== initialArtistRef.id)) {
      void openArtist({ artist: initialArtistRef, name: artistName?.trim() || activeTrack?.artist || "Artist", sourceUrl: sourceUrlFor(initialArtistRef) });
      return;
    }
    if (initialSectionChanged && profile && (initialSection === "releases" || initialSection === "tracks")) {
      void loadSection(initialSection);
    }
    const incomingArtistName = artistName?.trim() ?? "";
    if (incomingArtistName !== lastInitialArtistName) {
      lastInitialArtistName = incomingArtistName;
      if (incomingArtistName && !initialArtistRef && incomingArtistName !== query.trim()) {
        query = incomingArtistName;
        void runSearch();
      }
    }
  });

  $effect(() => {
    if (lastInitialScroll !== initialScroll) {
      lastInitialScroll = initialScroll;
      scrollRestorePending = true;
    }
    if (scrollRestorePending && pageElement && !busy && (!selectedArtist || profile)) {
      pageElement.scrollTop = Math.max(0, initialScroll);
      scrollRestorePending = false;
    }
  });

  function localArtistNames(): string[] {
    if (!selectedArtist) return [];
    return [...new Set([selectedArtist.name, ...(profile?.aliases ?? [])].map(name => name.trim()).filter(Boolean))];
  }

  const fallbackLocalMatches = $derived.by(() => {
    const normalized = localArtistNames().map(normalizeName);
    const minimum = parseDurationBound(localArtistMinDuration), maximum = parseDurationBound(localArtistMaxDuration);
    const sourceOccurrences = localTracks.map((track, sourceIndex) => ({ track, sourceIndex, sortTrack: { ...track } })).filter(row => normalized.includes(normalizeName(row.track.artist)));
    const filtered = filterTracks(sourceOccurrences.map(row => row.sortTrack), {
      query: localArtistQuery,
      ...(minimum === null ? {} : { minDurationSeconds: minimum }),
      ...(maximum === null ? {} : { maxDurationSeconds: maximum }),
    }).filter(track => localArtistAvailability === "all" || (localArtistAvailability === "on-device" ? Boolean(track.localPath || track.localAvailability === "available") : !track.localPath && track.localAvailability !== "available"));
    const bySortTrack = new Map(sourceOccurrences.map(row => [row.sortTrack, row]));
    return sortTracks(filtered, localArtistSort, localArtistDirection).map(row => { const item = bySortTrack.get(row)!; return { track: item.track, sourceIndex: item.sourceIndex }; });
  });
  const visibleLocalArtistOccurrences = $derived(onLoadLocalAssetsPage
    ? localArtistTracks.map((track, sourceIndex) => ({ track, sourceIndex }))
    : fallbackLocalMatches);
  const visibleLocalArtistTracks = $derived(visibleLocalArtistOccurrences.map(row => row.track));
  const localArtistSortOptions = $derived({
    sourceOrder: true,
    title: localArtistSortAvailability?.title ?? false,
    artist: localArtistSortAvailability?.artist ?? false,
    album: localArtistSortAvailability?.album ?? false,
    duration: localArtistSortAvailability?.duration ?? false,
    releaseDate: localArtistSortAvailability?.releaseDate ?? false,
    dateAdded: localArtistSortAvailability?.dateAdded ?? false,
    bpm: localArtistSortAvailability?.bpm ?? false,
    key: localArtistSortAvailability?.key ?? false,
    plays: localArtistSortAvailability?.plays ?? false,
    views: localArtistSortAvailability?.views ?? false,
  });
  const visibleTopTrackOccurrences = $derived.by(() => {
    const minimum = parseDurationBound(topTrackMinDuration), maximum = parseDurationBound(topTrackMaxDuration);
    const source = profile?.topTracks ?? [];
    const occurrences = source.map((track, sourceIndex) => ({ track, sourceIndex, sortTrack: { ...track } }));
    const bySortTrack = new Map(occurrences.map(row => [row.sortTrack, row]));
    const filtered = filterTracks(occurrences.map(row => row.sortTrack), {
      query: topTrackQuery,
      ...(minimum === null ? {} : { minDurationSeconds: minimum }),
      ...(maximum === null ? {} : { maxDurationSeconds: maximum }),
    });
    return sortTracks(filtered, topTrackSort, topTrackDirection).map(row => bySortTrack.get(row)!);
  });
  const visibleTopTracks = $derived(visibleTopTrackOccurrences.map(row => row.track));

  async function loadLocalArtistPage(append = false) {
    if (!selectedArtist || !onLoadLocalAssetsPage || localArtistLoading) return;
    const generation = ++localArtistRequestGeneration;
    const artistKey = `${selectedArtist.artist.provider}:${selectedArtist.artist.id}`;
    const offset = append ? localArtistTracks.length : 0;
    const minimum = parseDurationBound(localArtistMinDuration), maximum = parseDurationBound(localArtistMaxDuration);
    const request: ArtistLocalAssetsPageRequest = {
      names: localArtistNames(), limit: 200, offset, query: localArtistQuery,
      sortField: localArtistSort, direction: localArtistDirection, availability: localArtistAvailability,
      ...(minimum === null ? {} : { minDurationSeconds: minimum }),
      ...(maximum === null ? {} : { maxDurationSeconds: maximum }),
    };
    localArtistLoading = true; localArtistError = "";
    try {
      const page: ArtistLocalAssetsPage = await onLoadLocalAssetsPage(request);
      if (generation !== localArtistRequestGeneration || section !== "local" || !selectedArtist || artistKey !== `${selectedArtist.artist.provider}:${selectedArtist.artist.id}`) return;
      localArtistTracks = append ? [...localArtistTracks, ...page.items] : page.items;
      localArtistTotal = page.total;
      localArtistSortAvailability = page.sortAvailability ?? null;
    } catch (cause) {
      if (generation === localArtistRequestGeneration) localArtistError = message(cause);
    } finally {
      if (generation === localArtistRequestGeneration) localArtistLoading = false;
    }
  }

  function resetLocalArtistQuery() {
    localArtistRequestGeneration++;
    localArtistLoading = false;
    localArtistError = "";
    localArtistTracks = [];
    localArtistTotal = 0;
    localArtistSortAvailability = null;
    void loadLocalArtistPage(false);
  }

  function normalizeName(value: string): string {
    return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase();
  }

  function navigateTrackArtist(track: Track) {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as { catalog?: { artists?: { provider?: string; id?: string; name?: string }[] } } : {};
    const matches = (provenance.catalog?.artists ?? []).filter((artist) => artist.name && normalizeName(artist.name) === normalizeName(track.artist) && (artist.provider === "musicbrainz" || artist.provider === "discogs") && artist.id);
    if (matches.length === 1 && onArtistNavigate) {
      void onArtistNavigate({ provider: matches[0].provider as ArtistRef["provider"], id: matches[0].id! });
      return;
    }
    query = track.artist;
    selectedArtist = null;
    profile = null;
    void runSearch();
  }

  function dedupeReleases(items: ArtistReleaseRef[]): ArtistReleaseRef[] {
    const result = new Map<string, ArtistReleaseRef>();
    for (const item of items) {
      const key = `${item.release.provider}:${item.sourceUrl}`;
      const prior = result.get(key);
      if (!prior) { result.set(key, item); continue; }
      const credits = new Map([...prior.credits, ...item.credits].map((credit) => [`${credit.name}\u0000${credit.role}\u0000${credit.creditedAs ?? ""}`, credit]));
      result.set(key, { ...prior, credits: [...credits.values()] });
    }
    return [...result.values()];
  }

  function matchesReleaseFilter(release: ArtistReleaseRef): boolean {
    if (releaseFilter === "All") return true;
    const type = release.primaryType?.toLocaleLowerCase();
    const format = release.format?.toLocaleLowerCase() ?? "";
    const expected = releaseFilter === "Albums" ? "album" : releaseFilter === "EPs" ? "ep" : "single";
    return type === expected || (!type && (format === expected || format.includes(expected)));
  }

  const releaseGroups = $derived.by(() => {
    const items = (releases?.items ?? []).filter(matchesReleaseFilter)
      .filter((release) => releaseProvider === "All sources" || release.release.provider === releaseProvider)
      .filter((release) => !releaseQuery.trim() || normalizeName(`${release.title} ${release.year ?? ""} ${release.format ?? ""} ${release.tracks.map(track => track.title).join(" ")} ${(releaseTrackCache[release.sourceUrl] ?? []).map((track) => track.title).join(" ")}`).includes(normalizeName(releaseQuery)));
    const groups = new Map<string, ArtistReleaseRef[]>();
    for (const item of items) {
      const key = `${normalizeName(item.title)}|${normalizeName(item.primaryType ?? item.format ?? "release")}`;
      groups.set(key, [...(groups.get(key) ?? []), item]);
    }
    const result = [...groups.entries()].map(([key, editions]) => ({ key, editions: editions.sort((a,b) => (b.releaseDate ?? `${b.year ?? 0}`).localeCompare(a.releaseDate ?? `${a.year ?? 0}`)) }));
    const chronology = (group: typeof result[number]) => {
      const dates = group.editions.map(edition => {
        if (edition.releaseDate && /^\d{4}(?:-\d{2})?(?:-\d{2})?$/.test(edition.releaseDate)) return `${edition.releaseDate}-00-00`.slice(0,10);
        return edition.year && edition.year > 0 ? `${String(edition.year).padStart(4,"0")}-00-00` : "";
      }).filter(Boolean).sort();
      return dates[0] ?? "";
    };
    return result.sort((a,b) => {
      if (releaseSort === "title") return a.editions[0].title.localeCompare(b.editions[0].title);
      const aYear = chronology(a), bYear = chronology(b);
      if (!aYear) return !bYear ? a.editions[0].title.localeCompare(b.editions[0].title) : 1;
      if (!bYear) return -1;
      return (releaseSort === "oldest" ? aYear.localeCompare(bYear) : bYear.localeCompare(aYear)) || a.editions[0].title.localeCompare(b.editions[0].title);
    });
  });

  function selectedEdition(group: { key: string; editions: ArtistReleaseRef[] }): ArtistReleaseRef {
    return group.editions.find((edition) => edition.sourceUrl === editionByGroup[group.key]) ?? group.editions[0];
  }

  async function loadReleaseTracks(release: ArtistReleaseRef) {
    if (!selectedArtist || !getReleaseTracks) return releaseTrackCache[release.sourceUrl] ?? [];
    if (releaseTrackCache[release.sourceUrl]) return releaseTrackCache[release.sourceUrl];
    const generation = requestGeneration;
    const artist = selectedArtist.artist;
    const key = `${generation}:${artist.provider}:${artist.id}:${release.sourceUrl}`;
    const existing = releaseTrackLoads.get(key);
    if (existing) return existing;
    loadingRelease = { ...loadingRelease, [release.sourceUrl]: true }; releaseLoadError = "";
    let task!: Promise<Track[]>;
    task = (async () => {
      try {
        const result = await getReleaseTracks(release, artist);
        if (generation !== requestGeneration) return [];
        releaseTrackCache = { ...releaseTrackCache, [release.sourceUrl]: result };
        return result;
      } catch (cause) { if (generation === requestGeneration) releaseLoadError = message(cause); return []; }
      finally {
        if (releaseTrackLoads.get(key) === task) releaseTrackLoads.delete(key);
        if (generation === requestGeneration) { const next = { ...loadingRelease }; delete next[release.sourceUrl]; loadingRelease = next; }
      }
    })();
    releaseTrackLoads.set(key, task);
    return task;
  }

  async function toggleRelease(group: { key: string; editions: ArtistReleaseRef[] }) {
    if (expandedRelease === group.key) { expandedRelease = ""; return; }
    expandedRelease = group.key;
    const edition = selectedEdition(group);
    if (!releaseTrackCache[edition.sourceUrl]) await loadReleaseTracks(edition);
  }

  async function playRelease(release: ArtistReleaseRef, index = 0) {
    const generation = requestGeneration;
    const list = releaseTrackCache[release.sourceUrl] ?? await loadReleaseTracks(release);
    if (generation === requestGeneration && list.length && onPlayTrack) await onPlayTrack(list[index], list, index);
  }

  function filteredReleaseOccurrences(source: Track[]): { track: Track; sourceIndex: number }[] {
    const minimum = parseDurationBound(releaseTrackMinDuration), maximum = parseDurationBound(releaseTrackMaxDuration);
    const occurrences = source.map((track, sourceIndex) => ({ track, sourceIndex, sortTrack: { ...track } }));
    const bySortTrack = new Map<Track, (typeof occurrences)[number]>();
    for (const occurrence of occurrences) bySortTrack.set(occurrence.sortTrack, occurrence);
    const filtered = filterTracks(occurrences.map(occurrence => occurrence.sortTrack), {
      query: releaseTrackQuery,
      ...(minimum === null ? {} : { minDurationSeconds: minimum }),
      ...(maximum === null ? {} : { maxDurationSeconds: maximum }),
    });
    return sortTracks(filtered, releaseTrackSort, releaseTrackSortDirection).map(sortTrack => {
      const occurrence = bySortTrack.get(sortTrack)!;
      return { track: occurrence.track, sourceIndex: occurrence.sourceIndex };
    });
  }

  async function playReleaseOccurrence(release: ArtistReleaseRef, sourceIndex: number) {
    const source = releaseTrackCache[release.sourceUrl] ?? [];
    if (sourceIndex >= 0 && sourceIndex < source.length && onPlayTrack) await onPlayTrack(source[sourceIndex], source, sourceIndex);
  }

  async function queueRelease(release: ArtistReleaseRef, mode: "append" | "next") {
    const generation = requestGeneration;
    const list = releaseTrackCache[release.sourceUrl] ?? await loadReleaseTracks(release);
    if (generation === requestGeneration && list.length) await onQueueTracks?.(list, mode);
  }

  async function downloadRelease(release: ArtistReleaseRef) {
    if (!onDownloadAlbum) return;
    const generation = requestGeneration;
    const list = releaseTrackCache[release.sourceUrl] ?? await loadReleaseTracks(release);
    if (generation === requestGeneration && list.length) await onDownloadAlbum(list, release);
  }

  async function saveAlbumToLibrary(release: ArtistReleaseRef, groupKey: string) {
    if (!onSaveAlbumToLibrary || albumSaveState[release.sourceUrl]?.status === "saving") return;
    const generation = requestGeneration;
    const artistIdentity = selectedArtist ? `${selectedArtist.artist.provider}:${selectedArtist.artist.id}` : "";
    const sourceUrl = release.sourceUrl;
    const isCurrent = () => !navigationDestroyed && generation === requestGeneration
      && `${selectedArtist?.artist.provider ?? ""}:${selectedArtist?.artist.id ?? ""}` === artistIdentity
      && releaseGroups.some(group => group.key === groupKey && selectedEdition(group).sourceUrl === sourceUrl);
    albumSaveState = { ...albumSaveState, [sourceUrl]: { status: "saving", message: "Saving album to library…" } };
    try {
      const sourceTracks = releaseTrackCache[sourceUrl] ?? await loadReleaseTracks(release);
      if (!isCurrent()) return;
      if (!sourceTracks.length) throw new Error("No album tracks are available to save.");
      await onSaveAlbumToLibrary(sourceTracks, release);
      if (!isCurrent()) return;
      const expectedCount = release.trackCount;
      const messageText = typeof expectedCount === "number" && expectedCount > sourceTracks.length
        ? `Album saved to library with ${sourceTracks.length} of ${expectedCount} source tracks available.`
        : "Album saved to library.";
      albumSaveState = { ...albumSaveState, [sourceUrl]: { status: "saved", message: messageText } };
    } catch (cause) {
      if (isCurrent()) albumSaveState = { ...albumSaveState, [sourceUrl]: { status: "error", message: message(cause) } };
    }
  }

  const visibleSourceOccurrences = $derived.by(() => {
    const source = tracks?.items ?? [];
    const items = source.map((track, sourceIndex) => ({ track, sourceIndex, sortTrack: { ...track } })).filter((row) => !trackQuery.trim() || normalizeName(`${row.track.title} ${row.track.artist} ${row.track.album ?? ""} ${row.track.version ?? ""} ${row.track.releaseDate ?? ""}`).includes(normalizeName(trackQuery)))
      .filter(({track}) => trackAvailability === "all" || (trackAvailability === "on-device" ? Boolean(track.localPath || track.localAvailability === "available") : !track.localPath && track.localAvailability !== "available"));
    const minimum = parseDurationBound(trackMinDuration), maximum = parseDurationBound(trackMaxDuration);
    const filtered = filterTracks(items.map(row => row.sortTrack), {
      ...(minimum === null ? {} : { minDurationSeconds: minimum }),
      ...(maximum === null ? {} : { maxDurationSeconds: maximum }),
    });
    const bySortTrack = new Map(items.map(row => [row.sortTrack, row]));
    return sortTracks(filtered, trackSort, trackSortDirection).map(row => { const item = bySortTrack.get(row)!; return { track: item.track, sourceIndex: item.sourceIndex }; });
  });
  const visibleSourceTracks = $derived(visibleSourceOccurrences.map(row => row.track));

  async function runSearch() {
    const term = query.trim();
    if (!term) return;
    const generation = ++requestGeneration;
    busy = true; error = ""; selectedArtist = null; profile = null; results = []; searchPage = 0; searchHasMore = false; searchWarnings = [];
    scrollRestorePending = true;
    try {
      const page = await searchArtists(term, 1);
      if (generation !== requestGeneration) return;
      results = dedupeArtistResults(page.results);
      searchPage = page.page;
      searchHasMore = page.hasMore;
      searchWarnings = page.warnings;
      if (!results.length) searchWarnings = [...searchWarnings, "No artists found."];
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }
  const sortAvailability = $derived(tracks?.nextCursor
    ? allTrackSortFieldsAvailable()
    : trackSortAvailability(tracks?.items ?? []));

  async function openArtist(artist: ArtistSearchResult) {
    onArtistSelected?.(artist.artist, artist.name);
    const generation = ++requestGeneration;
    busy = true; error = ""; review = null;
    loadingReleaseOrder = false;
    releaseTrackCache = {}; loadingRelease = {}; releaseLoadError = ""; expandedRelease = ""; editionByGroup = {};
    localArtistRequestGeneration++; localArtistTracks = []; localArtistTotal = 0; localArtistSortAvailability = null; localArtistLoading = false;
    selectedArtist = artist; profile = null; section = initialSection;
    scrollRestorePending = true;
    releases = null; tracks = null; collaborations = null;
    try {
      const result = await getArtistProfile(artist.artist);
      if (generation !== requestGeneration) return;
      selectedArtist = result.artist;
      profile = result;
      releases = result.releases;
      if (section === "releases" || section === "tracks") await loadSection(section);
      if (section === "local" && onLoadLocalAssetsPage) resetLocalArtistQuery();
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function moreSearchResults() {
    if (!searchHasMore || busy) return;
    const generation = requestGeneration;
    busy = true;
    try {
      const page = await searchArtists(query.trim(), searchPage + 1);
      if (generation !== requestGeneration) return;
      results = dedupeArtistResults([...results, ...page.results]);
      searchPage = page.page;
      searchHasMore = page.hasMore;
      searchWarnings = page.warnings;
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  function dedupeArtistResults(items: ArtistSearchResult[]): ArtistSearchResult[] {
    const seen = new Set<string>();
    return items.filter((item) => {
      const key = `${item.artist.provider}:${item.artist.id}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }

  function backToResults() {
    onArtistSelected?.(null, query);
    requestGeneration++;
    localArtistRequestGeneration++;
    localArtistLoading = false;
    busy = false;
    selectedArtist = null;
    profile = null;
    releases = null;
    tracks = null;
    collaborations = null;
  }

  async function loadSection(next: typeof section) {
    onSectionChange?.(next);
    section = next;
    if (next !== "local") localArtistRequestGeneration++;
    if (!selectedArtist) return;
    if (next === "local" && onLoadLocalAssetsPage) {
      localArtistLoading = false;
      void loadLocalArtistPage(false);
      return;
    }
    const generation = requestGeneration;
    busy = true; error = "";
    try {
      if (next === "releases" && (!releases || !releases.items.length)) {
        const result = await getArtistReleases(selectedArtist.artist);
        if (generation === requestGeneration) releases = result;
      }
      if ((next === "tracks" || next === "overview") && (!tracks || (!tracks.items.length && tracks.warnings.length))) {
        const result = await getArtistTracks(selectedArtist.artist);
        if (generation === requestGeneration) tracks = result;
      }
      if (next === "collaborations" && (!collaborations || (!collaborations.items.length && collaborations.warnings.length))) {
        const result = await getArtistCollaborations(selectedArtist.artist);
        if (generation === requestGeneration) collaborations = result;
      }
      if (generation !== requestGeneration) return;
      if (next === "releases") await completeReleaseOrder(generation);
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function completeReleaseOrder(generation: number) {
    if (!selectedArtist || !releases?.nextCursor) return;
    const artist = selectedArtist.artist;
    const seen = new Set<string>();
    loadingReleaseOrder = true;
    try {
      for (let page = 0; page < 20 && generation === requestGeneration && section === "releases" && releases?.nextCursor; page++) {
        const cursor = releases.nextCursor;
        if (seen.has(cursor)) break;
        seen.add(cursor);
        const next = await getArtistReleases(artist, cursor);
        if (generation !== requestGeneration) return;
        releases = { ...next, items: dedupeReleases([...releases.items, ...next.items]) };
      }
    } finally { if (generation === requestGeneration) loadingReleaseOrder = false; }
  }

  async function moreReleases() {
    if (!selectedArtist || !releases?.nextCursor || busy) return;
    const generation = requestGeneration;
    const artist = selectedArtist.artist;
    busy = true;
    try {
      const next = await getArtistReleases(artist, releases.nextCursor);
      if (generation !== requestGeneration) return;
      releases = { ...next, items: dedupeReleases([...releases.items, ...next.items]) };
      await completeReleaseOrder(generation);
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function moreTracks() {
    if (!selectedArtist || !tracks?.nextCursor || busy) return;
    const generation = requestGeneration;
    const artist = selectedArtist.artist;
    busy = true;
    try {
      const next = await getArtistTracks(artist, tracks.nextCursor);
      if (generation !== requestGeneration) return;
      tracks = { ...next, items: [...tracks.items, ...next.items] };
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function moreCollaborations() {
    if (!selectedArtist || !collaborations?.nextCursor || busy) return;
    const generation = requestGeneration;
    const artist = selectedArtist.artist;
    busy = true;
    try {
      const next = await getArtistCollaborations(artist, collaborations.nextCursor);
      if (generation !== requestGeneration) return;
      collaborations = { ...next, items: [...collaborations.items, ...next.items] };
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function inspectRelease(release: ArtistReleaseRef) {
    if (!activeTrack) { error = "Select a library track first to review metadata suggestions."; return; }
    busy = true; error = ""; selectedFields = [];
    try { review = await resolveRecordingMetadata(activeTrack, release, null); }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }

  function toggleField(key: MetadataFieldKey, checked: boolean) {
    selectedFields = checked ? [...new Set([...selectedFields, key])] : selectedFields.filter((item) => item !== key);
  }

  async function applyReview() {
    if (!activeTrack || !review || !selectedFields.length) return;
    busy = true;
    try { await onApplyMetadata(activeTrack, review, selectedFields); review = null; }
    catch (cause) { error = message(cause); }
    finally { busy = false; }
  }

  function message(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }
  function sourceUrlFor(artist: ArtistRef): string {
    return artist.provider === "musicbrainz" ? `https://musicbrainz.org/artist/${artist.id}` : `https://www.discogs.com/artist/${artist.id}`;
  }
  function browseCurrentArtistOnSpotify() {
    const name = selectedArtist?.name;
    if (name && onBrowseArtistProvider) void onBrowseArtistProvider(name, "spotify");
  }
</script>

<section bind:this={pageElement} class="artist-page" aria-label="Artist discovery" onscroll={(event) => onScroll?.(event.currentTarget.scrollTop)}>
  {#if !selectedArtist}
  <header>
    <div><p class="eyebrow">MUSICBRAINZ · DISCOGS · LAST.FM</p><h2>Artist discovery</h2>
      <p class="muted">Find an artist identity, explore source credits, and review metadata suggestions.</p></div>
  </header>
  <form class="search" onsubmit={(event) => { event.preventDefault(); void runSearch(); }}>
    <input bind:value={query} placeholder="Search artist name" aria-label="Search artist name" />
    <button type="submit" disabled={busy || !query.trim()}>{busy ? "Loading…" : "Search"}</button>
  </form>
  {/if}
  {#if error}<p class="notice error" role="alert">{error}</p>{/if}
  {#if searchWarnings.length}<div class="notice">{#each searchWarnings as warning}<p>{warning}</p>{/each}</div>{/if}

  {#if !selectedArtist}
    <div class="results" aria-live="polite">
      {#each results as result (result.artist.provider + result.artist.id)}
        <button class="artist-result" onclick={() => void openArtist(result)}>
          <span><strong>{result.name}</strong>{#if result.disambiguation}<small>{result.disambiguation}</small>{/if}</span>
          <span class="source">{result.artist.provider} · {result.country ?? "country unknown"}</span>
        </button>
      {/each}
    </div>
    {#if searchHasMore}<button class="secondary" onclick={() => void moreSearchResults()} disabled={busy}>Load more artists</button>{/if}
  {:else}
    {#if showSearchResultsButton}<button class="back" onclick={backToResults}>← Search results</button>{/if}
    <div class="artist-heading">
      {#if profile?.imageUrl}<span class="artist-portrait"><ArtworkImage urls={[profile.imageUrl]} /></span>{/if}
      <div><p class="eyebrow">{providerLabel(selectedArtist.artist.provider)} identity</p>
        <h2>{selectedArtist.name}</h2>
        {#if selectedArtist.disambiguation}<p>{selectedArtist.disambiguation}</p>{/if}
        <a href={selectedArtist.sourceUrl} target="_blank" rel="noreferrer">Open source profile ↗</a>
        {#if onBrowseArtistProvider}<button class="secondary artist-provider-action" type="button" aria-label={`Find ${selectedArtist.name} on Spotify`} onclick={browseCurrentArtistOnSpotify}>Find on Spotify</button>{/if}
      </div>
    </div>
    <nav class="tabs" aria-label="Artist sections">
      {#each [["overview", "Overview"], ["releases", "Discography"], ["tracks", "Tracks"], ["collaborations", "Collaborations"], ["local", "Local"]] as item}
        <button class:active={section === item[0]} onclick={() => void loadSection(item[0] as typeof section)}>{item[1]}</button>
      {/each}
    </nav>
    {#if profile?.warnings.length}<div class="notice">{#each profile.warnings as warning}<p>{warning}</p>{/each}</div>{/if}
    {#if section === "overview"}
      {#if profile}
        <div class="overview-grid">
          <article><h3>Genres and tags by source</h3>{#if (profile.sourceTags ?? []).length}<ul class="source-tags">{#each profile.sourceTags ?? [] as tag (tag.provider + tag.kind + tag.name)}<li><span>{tag.name}</span><small>{tag.kind} · {providerLabel(tag.provider)}</small></li>{/each}</ul>{:else}<p>No source tags available.</p>{/if}</article>
          <article><h3>Aliases</h3><p>{profile.aliases.join(" · ") || "No aliases listed."}</p></article>
          <article><h3>Provider coverage</h3><ul class="source-coverage">{#each profile.sourceCoverage ?? [] as coverage (coverage.provider)}<li><strong>{providerLabel(coverage.provider)}</strong><span>{coverageLabel(coverage.status)}</span><small>{coverage.detail}</small>{#if coverage.sourceUrl}<a href={coverage.sourceUrl} target="_blank" rel="noreferrer">Source ↗</a>{/if}</li>{/each}</ul></article>
        </div>
        {#if profile.biography}<h3>Biography {#if profile.biographySource}<small>· {profile.biographySource}</small>{/if}</h3><p class="bio">{profile.biography}</p>{/if}
        {#if profile.links.length}<div class="links">{#each profile.links as link}<a href={link.url} target="_blank" rel="noreferrer">{link.label} ↗</a>{/each}</div>{/if}
        <h3>Top tracks {#if profile.topTracks.length}<small>· Last.fm</small>{/if}</h3>
        {#if profile.topTracks.length}
          {@const topAvailability = trackSortAvailabilityForSources(profile.topTracks, ["lastfm-top-tracks"])}
          <div class="catalog-controls" aria-label="Artist top tracks controls">
            <input aria-label="Filter artist top tracks" placeholder="Filter loaded top tracks" bind:value={topTrackQuery} />
            <label>Sort loaded tracks<select aria-label="Sort artist top tracks" bind:value={topTrackSort}>
              <option value="sourceOrder">Source order</option>
              <option value="title" disabled={!topAvailability.title}>{trackSortOptionLabelForSources(topAvailability, "title", "Title", ["lastfm-top-tracks"])}</option>
              <option value="artist" disabled={!topAvailability.artist}>{trackSortOptionLabelForSources(topAvailability, "artist", "Artist", ["lastfm-top-tracks"])}</option>
              <option value="album" disabled={!topAvailability.album}>{trackSortOptionLabelForSources(topAvailability, "album", "Album", ["lastfm-top-tracks"])}</option>
              <option value="duration" disabled={!topAvailability.duration}>{trackSortOptionLabelForSources(topAvailability, "duration", "Duration", ["lastfm-top-tracks"])}</option>
              <option value="releaseDate" disabled={!topAvailability.releaseDate}>{trackSortOptionLabelForSources(topAvailability, "releaseDate", "Release date", ["lastfm-top-tracks"])}</option>
              <option value="dateAdded" disabled={!topAvailability.dateAdded}>{trackSortOptionLabelForSources(topAvailability, "dateAdded", "Date added", ["lastfm-top-tracks"])}</option>
              <option value="bpm" disabled={!topAvailability.bpm}>{trackSortOptionLabelForSources(topAvailability, "bpm", "BPM", ["lastfm-top-tracks"])}</option>
              <option value="key" disabled={!topAvailability.key}>{trackSortOptionLabelForSources(topAvailability, "key", "Key", ["lastfm-top-tracks"])}</option>
              <option value="plays" disabled={!topAvailability.plays}>{trackSortOptionLabelForSources(topAvailability, "plays", "Plays", ["lastfm-top-tracks"])}</option>
              <option value="views" disabled={!topAvailability.views}>{trackSortOptionLabelForSources(topAvailability, "views", "Views", ["lastfm-top-tracks"])}</option>
            </select></label>
            <label>Direction<select aria-label="Artist top track sort direction" bind:value={topTrackDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
            <TrackDurationFilter bind:minDuration={topTrackMinDuration} bind:maxDuration={topTrackMaxDuration} minLabel="Minimum artist top track duration" maxLabel="Maximum artist top track duration" />
            <span>{visibleTopTracks.length} of {profile.topTracks.length} loaded tracks</span>
          </div>
          {#if visibleTopTracks.length}<TrackList presentation="playlist" tracks={visibleTopTracks} selectionContext={`artist-top:${selectedArtist?.artist.provider}:${selectedArtist?.artist.id}`} selectionKeys={visibleTopTrackOccurrences.map(row => String(row.sourceIndex))} selectionTracks={profile.topTracks} selectionTrackKeys={profile.topTracks.map((_, index) => String(index))} listLabel="Artist top tracks" onPlay={onPlayTrack} onDownload={onDownloadTrack} onFavorite={onFavoriteTrack} onNext={track => onQueueTracks?.([track], "next")} onQueue={track => onQueueTracks?.([track], "append")} onPlaylist={onAddTrackToPlaylist} {onAddSelectedToPlaylist} onArtist={navigateTrackArtist} onAlbum={onTrackAlbum} onContext={onTrackContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />{:else}<p class="muted">No loaded top tracks match these filters.</p>{/if}{:else}<p class="muted">Last.fm top tracks are unavailable for this artist or no API key is configured.</p>{/if}
      {:else}<p class="muted">Loading artist profile…</p>{/if}
    {:else if section === "releases"}
      {#if releases?.sourceCoverage?.length}<details class="catalog-provider-details"><summary>Catalog sources · {releases.sourceCoverage.map(coverage => providerLabel(coverage.provider)).join(" · ")}</summary><ul class="source-coverage">{#each releases.sourceCoverage ?? [] as coverage (coverage.provider)}<li><strong>{providerLabel(coverage.provider)}</strong><span>{coverageLabel(coverage.status)}</span><small>{coverage.detail}</small></li>{/each}</ul></details>{/if}
      {#if releases?.warnings.length}<div class="notice">{#each releases.warnings as warning}<p>{warning}</p>{/each}</div>{/if}
      <div class="catalog-controls" aria-label="Discography filters">
        <input bind:value={releaseQuery} placeholder="Search releases and loaded tracks" aria-label="Search discography" />
        <select bind:value={releaseFilter} aria-label="Release type"><option>Albums</option><option>EPs</option><option>Singles</option><option>All</option></select>
        <select bind:value={releaseProvider} aria-label="Source provider"><option>All sources</option>{#each [...new Set((releases?.items ?? []).map((item) => item.release.provider))] as provider}<option value={provider}>{providerLabel(provider)}</option>{/each}</select>
        <select bind:value={releaseSort} aria-label="Sort releases"><option value="newest">Newest first</option><option value="oldest">Oldest first</option><option value="title">Title A–Z</option></select>
      </div>
      <p class="muted catalog-count">{releaseGroups.length} {releaseGroups.length === 1 ? "album" : "albums"} · {releaseGroups.reduce((count, group) => count + group.editions.length, 0)} editions loaded</p>
      {#if loadingReleaseOrder}<p class="muted" role="status">Loading remaining releases to order albums…</p>{:else if releases?.nextCursor}<p class="muted">More releases are available; order applies to the albums loaded so far.</p>{/if}
      <div class="release-list">{#each releaseGroups as group (group.key)}
        {@const release = selectedEdition(group)}
        {@const artworkTrack = (releaseTrackCache[release.sourceUrl] ?? []).find((track) => track.artworkUrl)}
        {@const artworkSources = [...(artworkTrack ? trackArtworkSources(artworkTrack) : []), ...(release.artworkUrls ?? []), musicBrainzCover(release.sourceUrl)]}
        <article class="release-card"><div class="release-card-header"><div class="release-card-main"><div class="release-art" aria-hidden="true"><ArtworkImage urls={artworkSources} /></div><div class="release-info"><small class="release-source">{providerLabel(release.release.provider)}</small><button class="album-title" aria-expanded={expandedRelease === group.key} onclick={() => void toggleRelease(group)}><strong>{release.title}</strong></button><p>{release.year ?? "Year unknown"}{#if release.country} · {release.country}{/if}{#if release.label} · {release.label}{/if}</p><small>{release.format ?? release.primaryType ?? "Format unknown"}{#if release.catalogNumber} · {release.catalogNumber}{/if} · {release.trackCount ?? "?"} tracks</small></div></div>
          <div class="release-actions"><button class="secondary" aria-label={`Play album ${release.title}`} onclick={() => void playRelease(release)} disabled={!getReleaseTracks || !onPlayTrack || loadingRelease[release.sourceUrl]}>Play album</button><button class="secondary" aria-label={`Queue album ${release.title}`} onclick={() => void queueRelease(release, "append")} disabled={!getReleaseTracks || !onQueueTracks || loadingRelease[release.sourceUrl]}>Queue</button><button class="secondary" aria-label={`Play album ${release.title} next`} onclick={() => void queueRelease(release, "next")} disabled={!getReleaseTracks || !onQueueTracks || loadingRelease[release.sourceUrl]}>Play next</button>{#if onDownloadAlbum}<button class="secondary" aria-label={`Download album ${release.title}`} onclick={() => void downloadRelease(release)} disabled={!getReleaseTracks || loadingRelease[release.sourceUrl]}>Download album</button>{/if}{#if onSaveAlbumToLibrary}<button class="secondary" aria-label={`Save album ${release.title} to library`} onclick={() => void saveAlbumToLibrary(release, group.key)} disabled={!getReleaseTracks || loadingRelease[release.sourceUrl] || albumSaveState[release.sourceUrl]?.status === "saving"}>{albumSaveState[release.sourceUrl]?.status === "saving" ? "Saving…" : albumSaveState[release.sourceUrl]?.status === "saved" ? "Saved" : "Save to library"}</button>{/if}<button class="secondary" aria-expanded={expandedRelease === group.key} onclick={() => void toggleRelease(group)}>Tracks</button></div></div>
          {#if onSaveAlbumToLibrary && albumSaveState[release.sourceUrl]}<p class={`notice album-save-status ${albumSaveState[release.sourceUrl].status === "error" ? "error" : ""}`} aria-live="polite">{albumSaveState[release.sourceUrl].message}</p>{/if}
          {#if release.tracks.length && !getReleaseTracks}<details class="source-track-details"><summary>Track titles from source · {release.tracks.length}</summary><ol class="release-tracklist">{#each release.tracks as sourceTrack, trackIndex (trackIndex)}<li>{sourceTrack.position ? `${sourceTrack.position}. ` : ""}{sourceTrack.title}</li>{/each}</ol></details>{/if}
          {#if group.editions.length > 1}<label class="edition-select">Edition <select value={release.sourceUrl} onchange={(event) => { editionByGroup = { ...editionByGroup, [group.key]: event.currentTarget.value }; const chosen = group.editions.find((item) => item.sourceUrl === event.currentTarget.value); if (chosen && expandedRelease === group.key) void loadReleaseTracks(chosen); }} aria-label={`Edition of ${release.title}`}>{#each group.editions as edition (edition.sourceUrl)}<option value={edition.sourceUrl}>{edition.year ?? "Year unknown"} · {providerLabel(edition.release.provider)}{edition.country ? ` · ${edition.country}` : ""}</option>{/each}</select></label>{/if}
          {#if expandedRelease === group.key}<div class="expanded-tracks">
            {#if releaseLoadError}<p class="notice error">{releaseLoadError}</p>{/if}
            {#if loadingRelease[release.sourceUrl]}<p class="muted">Loading this edition’s tracks…</p>
            {:else if (releaseTrackCache[release.sourceUrl] ?? []).length}
              {@const sourceTracks = releaseTrackCache[release.sourceUrl] ?? []}
              {@const visibleReleaseOccurrences = filteredReleaseOccurrences(sourceTracks)}
              {@const visibleReleaseTracks = visibleReleaseOccurrences.map(row => row.track)}
              {@const releaseAvailability = trackSortAvailability(sourceTracks)}
              <div class="catalog-controls release-track-controls" aria-label="Loaded release tracks">
                <input aria-label="Filter loaded release tracks" bind:value={releaseTrackQuery} placeholder="Filter loaded tracks" />
                <label>Sort loaded tracks
                  <select aria-label="Sort loaded release tracks" bind:value={releaseTrackSort}>
                    <option value="sourceOrder">Album order</option>
                    <option value="title" disabled={!releaseAvailability.title}>{trackSortOptionLabel(releaseAvailability, "title", "Title")}</option>
                    <option value="artist" disabled={!releaseAvailability.artist}>{trackSortOptionLabel(releaseAvailability, "artist", "Artist")}</option>
                    <option value="album" disabled={!releaseAvailability.album}>{trackSortOptionLabel(releaseAvailability, "album", "Album")}</option>
                    <option value="duration" disabled={!releaseAvailability.duration}>{trackSortOptionLabel(releaseAvailability, "duration", "Duration")}</option>
                    <option value="releaseDate" disabled={!releaseAvailability.releaseDate}>{trackSortOptionLabel(releaseAvailability, "releaseDate", "Release date")}</option>
                    <option value="dateAdded" disabled={!releaseAvailability.dateAdded}>{trackSortOptionLabel(releaseAvailability, "dateAdded", "Date added")}</option>
                    <option value="bpm" disabled={!releaseAvailability.bpm}>{trackSortOptionLabel(releaseAvailability, "bpm", "BPM")}</option>
                    <option value="key" disabled={!releaseAvailability.key}>{trackSortOptionLabel(releaseAvailability, "key", "Key")}</option>
                    <option value="plays" disabled={!releaseAvailability.plays}>{trackSortOptionLabel(releaseAvailability, "plays", "Plays")}</option>
                    <option value="views" disabled={!releaseAvailability.views}>{trackSortOptionLabel(releaseAvailability, "views", "Views")}</option>
                  </select>
                </label>
                <label>Direction<select aria-label="Release track sort direction" bind:value={releaseTrackSortDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
                <TrackDurationFilter bind:minDuration={releaseTrackMinDuration} bind:maxDuration={releaseTrackMaxDuration} minLabel="Minimum loaded release track duration" maxLabel="Maximum loaded release track duration" />
                <span>{visibleReleaseTracks.length} of {sourceTracks.length} loaded tracks</span>
              </div>
              {#if visibleReleaseTracks.length}
                <TrackList presentation="playlist" tracks={visibleReleaseTracks} selectionContext={`artist-release:${release.sourceUrl}`} selectionKeys={visibleReleaseOccurrences.map(row => String(row.sourceIndex))} selectionTracks={sourceTracks} selectionTrackKeys={sourceTracks.map((_, index) => String(index))} listLabel={`${release.title} tracks`} position={(_track, index) => (visibleReleaseOccurrences[index]?.sourceIndex ?? index) + 1} onPlay={(_track, _list, index) => void playReleaseOccurrence(release, visibleReleaseOccurrences[index]?.sourceIndex ?? -1)} onDownload={onDownloadTrack} onFavorite={onFavoriteTrack} onNext={track => onQueueTracks?.([track], "next")} onQueue={track => onQueueTracks?.([track], "append")} onPlaylist={onAddTrackToPlaylist} {onAddSelectedToPlaylist} onArtist={navigateTrackArtist} onAlbum={onTrackAlbum} onContext={onTrackContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
              {:else}<p class="muted">No loaded tracks match these filters.</p>{/if}
            {:else}<p class="muted">No track details available for this edition.</p>{/if}
          </div>{/if}
          <div class="links release-source-link"><a href={release.sourceUrl} target="_blank" rel="noreferrer">Open edition source ↗</a>{#if activeTrack}<button class="secondary" disabled={busy} onclick={() => void inspectRelease(release)}>Review metadata</button>{/if}</div></article>
      {/each}</div>
      {#if releases?.items.length && !releaseGroups.length}<p class="muted">No loaded releases match these filters. <button class="secondary" onclick={() => { releaseQuery = ""; releaseFilter = "All"; releaseProvider = "All sources"; }}>Clear filters</button></p>{/if}
      {#if releases?.nextCursor}<button class="secondary" onclick={() => void moreReleases()} disabled={busy}>Load more releases</button>{/if}
      {#if !(releases?.items.length)}<p class="muted">No releases are available from this source.</p>{/if}
    {:else if section === "tracks"}
      <p class="muted">{tracks?.source ?? "Loading source recordings…"} · {visibleSourceTracks.length} of {(tracks?.items ?? []).length} loaded recordings</p>
      <div class="catalog-controls"><input bind:value={trackQuery} placeholder="Search loaded tracks" aria-label="Search loaded tracks" /><select bind:value={trackAvailability} aria-label="Track availability"><option value="all">All tracks</option><option value="on-device">On this device</option><option value="not-on-device">Not on this device</option></select><select bind:value={trackSort} aria-label="Sort tracks"><option value="sourceOrder">Source order</option><option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabel(sortAvailability, "title", "Title")}</option><option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabel(sortAvailability, "artist", "Artist")}</option><option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabel(sortAvailability, "album", "Album")}</option><option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabel(sortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabel(sortAvailability, "releaseDate", "Release date")}</option><option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabel(sortAvailability, "dateAdded", "Date added")}</option><option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabel(sortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabel(sortAvailability, "key", "Key")}</option><option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabel(sortAvailability, "plays", "Plays")}</option><option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabel(sortAvailability, "views", "Views")}</option></select><select bind:value={trackSortDirection} aria-label="Sort direction"><option value="asc">Ascending</option><option value="desc">Descending</option></select><TrackDurationFilter bind:minDuration={trackMinDuration} bind:maxDuration={trackMaxDuration} minLabel="Minimum track duration" maxLabel="Maximum track duration" /></div>
      <TrackList presentation="playlist" tracks={visibleSourceTracks} selectionContext={`artist-source:${selectedArtist?.artist.provider}:${selectedArtist?.artist.id}:${tracks?.source ?? ""}`} selectionKeys={visibleSourceOccurrences.map(row => String(row.sourceIndex))} selectionTracks={tracks?.items ?? []} selectionTrackKeys={(tracks?.items ?? []).map((_, index) => String(index))} listLabel="Artist recordings" meta={track => track.releaseDate ?? "Date unknown"} onPlay={onPlayTrack} onDownload={onDownloadTrack} onFavorite={onFavoriteTrack} onNext={track => onQueueTracks?.([track], "next")} onQueue={track => onQueueTracks?.([track], "append")} onPlaylist={onAddTrackToPlaylist} {onAddSelectedToPlaylist} onArtist={navigateTrackArtist} onAlbum={onTrackAlbum} onContext={onTrackContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
      {#if tracks && !visibleSourceTracks.length}<p class="muted">{tracks.items.length ? "No loaded tracks match these filters." : "No tracks have been returned by this source."}</p>{/if}
      {#if tracks?.nextCursor}<button class="secondary" onclick={() => void moreTracks()} disabled={busy}>Load more tracks</button>{/if}
      {#if tracks?.warnings.length}<div class="notice">{#each tracks.warnings as warning}<p>{warning}</p>{/each}</div>{/if}
    {:else if section === "collaborations"}
      {#if collaborations?.sourceCoverage?.length}<ul class="source-coverage">{#each collaborations.sourceCoverage ?? [] as coverage (coverage.provider)}<li><strong>{providerLabel(coverage.provider)}</strong><span>{coverageLabel(coverage.status)}</span><small>{coverage.detail}</small></li>{/each}</ul>{/if}
      <div class="release-list">{#each collaborations?.items ?? [] as item (item.sourceUrl + item.credits[1]?.artist?.id + item.credits[1]?.role)}<article><div><strong>{item.title ?? item.description ?? "Credit"}</strong><p>{item.date ?? "Date unknown"}</p><small>{item.credits.map((credit) => `${credit.name} — ${credit.role}`).join(" · ")}</small></div><div class="links"><a href={item.sourceUrl} target="_blank" rel="noreferrer">Source ↗</a>{#each item.credits as credit (credit.name + credit.role)}{#if credit.artist && credit.artist.id !== selectedArtist?.artist.id && onArtistNavigate}<button class="secondary" onclick={() => void onArtistNavigate?.(credit.artist!)}>{credit.name}</button>{/if}{/each}</div></article>{/each}</div>
      {#if collaborations?.nextCursor}<button class="secondary" onclick={() => void moreCollaborations()} disabled={busy}>Load more collaborations</button>{/if}
      {#if collaborations?.warnings.length}<div class="notice">{#each collaborations.warnings as warning}<p>{warning}</p>{/each}</div>{/if}
      {#if collaborations && !collaborations.items.length && !collaborations.nextCursor}<p class="muted">No relationships are listed by the available sources.</p>{/if}
    {:else}
      <p class="muted">Local recordings use exact artist-credit text after case and whitespace normalization. Same-name identities are not merged.</p>
      {#if onLoadLocalAssetsPage}
        <div class="catalog-controls" aria-label="Local artist recordings controls">
          <input aria-label="Filter local artist recordings" placeholder="Filter local tracks" bind:value={localArtistQuery} oninput={resetLocalArtistQuery} />
          <select aria-label="Local artist track availability" bind:value={localArtistAvailability} onchange={resetLocalArtistQuery}><option value="all">All tracks</option><option value="on-device">On this device</option><option value="not-on-device">Not on this device</option></select>
          <label>Sort local tracks<select aria-label="Sort local artist tracks" bind:value={localArtistSort} onchange={resetLocalArtistQuery}>
            <option value="sourceOrder">Library order</option>
            <option value="title" disabled={!localArtistSortOptions.title}>{trackSortOptionLabel(localArtistSortOptions, "title", "Title")}</option>
            <option value="artist" disabled={!localArtistSortOptions.artist}>{trackSortOptionLabel(localArtistSortOptions, "artist", "Artist")}</option>
            <option value="album" disabled={!localArtistSortOptions.album}>{trackSortOptionLabel(localArtistSortOptions, "album", "Album")}</option>
            <option value="duration" disabled={!localArtistSortOptions.duration}>{trackSortOptionLabel(localArtistSortOptions, "duration", "Duration")}</option>
            <option value="releaseDate" disabled={!localArtistSortOptions.releaseDate}>{trackSortOptionLabel(localArtistSortOptions, "releaseDate", "Release date")}</option>
            <option value="dateAdded" disabled={!localArtistSortOptions.dateAdded}>{trackSortOptionLabel(localArtistSortOptions, "dateAdded", "Date added")}</option>
            <option value="bpm" disabled={!localArtistSortOptions.bpm}>{trackSortOptionLabel(localArtistSortOptions, "bpm", "BPM")}</option>
            <option value="key" disabled={!localArtistSortOptions.key}>{trackSortOptionLabel(localArtistSortOptions, "key", "Key")}</option>
            <option value="plays" disabled={!localArtistSortOptions.plays}>{trackSortOptionLabel(localArtistSortOptions, "plays", "Plays")}</option>
            <option value="views" disabled={!localArtistSortOptions.views}>{trackSortOptionLabel(localArtistSortOptions, "views", "Views")}</option>
          </select></label>
          <label>Direction<select aria-label="Local artist sort direction" bind:value={localArtistDirection} onchange={resetLocalArtistQuery}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
          <TrackDurationFilter bind:minDuration={localArtistMinDuration} bind:maxDuration={localArtistMaxDuration} minLabel="Minimum local artist duration" maxLabel="Maximum local artist duration" onchange={resetLocalArtistQuery} />
        </div>
        <p class="muted">{visibleLocalArtistTracks.length} of {localArtistTotal} exact artist-credit matches</p>
        {#if localArtistError}<p class="notice error" role="alert">{localArtistError}</p>{/if}
        {#if localArtistLoading}<p class="muted" role="status">Loading local artist recordings…</p>{/if}
      {:else}
        <p class="muted">Local matches use exact artist-credit text after case and whitespace normalization. Same-name identities are not merged.</p>
      {/if}
      <TrackList presentation="playlist" tracks={visibleLocalArtistTracks} selectionContext={`artist-local:${selectedArtist?.artist.provider}:${selectedArtist?.artist.id}:${onLoadLocalAssetsPage ? "paged" : "fallback"}`} selectionKeys={visibleLocalArtistOccurrences.map(row => String(row.sourceIndex))} selectionTracks={onLoadLocalAssetsPage ? localArtistTracks : localTracks} selectionTrackKeys={(onLoadLocalAssetsPage ? localArtistTracks : localTracks).map((_, index) => String(index))} listLabel="Local artist recordings" meta={track => track.localAvailability ?? "Library track"} onPlay={onPlayTrack} onDownload={onDownloadTrack} onFavorite={onFavoriteTrack} onNext={track => onQueueTracks?.([track], "next")} onQueue={track => onQueueTracks?.([track], "append")} onPlaylist={onAddTrackToPlaylist} {onAddSelectedToPlaylist} onArtist={navigateTrackArtist} onAlbum={onTrackAlbum} onContext={onTrackContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
      {#if onLoadLocalAssetsPage && localArtistTracks.length < localArtistTotal}<button class="secondary" disabled={localArtistLoading} onclick={() => void loadLocalArtistPage(true)}>Load more local recordings</button>{/if}
      {#if !visibleLocalArtistTracks.length && !localArtistLoading}<p class="muted">No exact artist-credit matches in the local library.</p>{/if}
    {/if}
  {/if}

  {#if review}
    <section class="review" aria-label="Review metadata suggestions">
      <h3>Review metadata suggestions</h3>
      <p class="muted">Every field needs your explicit selection. The selected track version is preserved; release edition year and recording first-release date are shown separately.</p>
      {#if review.recordingFirstReleaseDate}<p>Recording first release: {review.recordingFirstReleaseDate}</p>{/if}
      {#if review.releaseYear}<p>Selected edition year: {review.releaseYear}</p>{/if}
      {#if review.selectedVersionPreserved}<p>Version kept: {review.selectedVersionPreserved}</p>{/if}
      {#if review.releaseTracklist.length}<h4>Tracklist for selected edition</h4><ol class="release-tracklist">{#each review.releaseTracklist as track, trackIndex (trackIndex)}<li class:possible-match={track.title === review.matchedReleaseTrack}><span>{track.position ? `${track.position}. ` : ""}{track.title}</span>{#if track.title === review.matchedReleaseTrack}<small>Possible match for the selected library track · verify the mix</small>{/if}</li>{/each}</ol>{:else if review.releaseRef?.release.provider === "discogs"}<p class="muted">The Discogs artist release list does not include individual track titles. The selected release page has been checked for a matching track; review the result below.</p>{/if}
      {#each review.warnings as warning}<p class="notice">{warning}</p>{/each}
      {#if review.recordingCandidates.length}<h4>Choose a recording identity</h4>{#each review.recordingCandidates as candidate (candidate.recording.id)}
        <article class="candidate"><span>{candidate.title} — {candidate.artist} · score {Math.round(candidate.score)}</span><button disabled={!activeTrack || busy} onclick={async () => { if (!activeTrack) return; busy = true; try { review = await resolveRecordingMetadata(activeTrack, null, candidate.recording); selectedFields = []; } catch (cause) { error = message(cause); } finally { busy = false; } }}>Select recording</button>
      </article>{/each}{/if}
      {#each review.fields as field (field.key)}<label class="field"><input type="checkbox" checked={selectedFields.includes(field.key as MetadataFieldKey)} onchange={(event) => toggleField(field.key as MetadataFieldKey, event.currentTarget.checked)} /><span><strong>{field.key}</strong><span>{field.value ?? "(empty)"}</span><small><a href={field.sourceUrl} target="_blank" rel="noreferrer">{field.source} ↗</a>{#if field.note} · {field.note}{/if}</small></span></label>{/each}
      {#if review.credits.length}<h4>Credits (source roles retained)</h4><p>{review.credits.map((credit) => `${credit.name} — ${credit.role}`).join(" · ")}</p>{/if}
      <button onclick={() => void applyReview()} disabled={busy || !selectedFields.length || !activeTrack}>Apply selected fields</button>
      <button class="secondary" onclick={() => { review = null; }}>Dismiss</button>
    </section>
  {/if}
</section>

<style>
  .artist-page{display:flex;flex:1;flex-direction:column;gap:1rem;padding:1.25rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px;min-width:0;min-height:0;height:100%;overflow:auto;overflow-x:hidden;container-name:artist-pane;container-type:inline-size}
  .artist-page > * { flex-shrink: 0;min-width:0;max-width:100% }
  header,.artist-heading,.artist-result,.release-list article,.candidate{display:flex;align-items:center;justify-content:space-between;gap:1rem}
  h2,h3,h4,p{margin:.25rem 0}h2{font-size:1.4rem}h3{font-size:1rem}.eyebrow,.source,small,.muted{color:var(--muted,#a3a8ae);font-size:.82rem}.eyebrow{letter-spacing:.09em;font-weight:700}
  .search{display:flex;gap:.5rem}.search input{flex:1;min-width:80px}.search input,input[type=checkbox]{accent-color:var(--accent,#67c3a5)}
  input:not([type=checkbox]){background:var(--input,#22262b);border:1px solid var(--border,#383d43);color:inherit;border-radius:7px;padding:.65rem}
  button{border:0;border-radius:7px;padding:.6rem .8rem;background:var(--accent,#67c3a5);color:#111;font-weight:650;cursor:pointer}button:disabled{opacity:.5;cursor:not-allowed}.secondary,.back{background:transparent;color:inherit;border:1px solid var(--border,#383d43)}.back{align-self:start}
  .results,.release-list{display:flex;flex-direction:column;gap:.5rem;min-width:0}.artist-result,.release-list article{width:100%;min-width:0;text-align:left;background:var(--input,#22262b);color:inherit;border:1px solid var(--border,#383d43);padding:.8rem;border-radius:8px;overflow-wrap:anywhere}.artist-result span{min-width:0;overflow-wrap:anywhere}.artist-result span:first-child{display:flex;flex-direction:column;gap:.2rem}.artist-result .source{text-align:right}.artist-heading{justify-content:flex-start;min-width:0}.artist-heading>div{min-width:0;overflow-wrap:anywhere}.artist-portrait{width:84px;height:84px;object-fit:cover;border-radius:8px}.tabs{display:flex;gap:.35rem;overflow:auto;border-bottom:1px solid var(--border,#383d43);max-width:100%}.tabs button{white-space:nowrap;background:transparent;color:inherit;border-radius:6px 6px 0 0}.tabs button.active{background:var(--accent,#67c3a5);color:#111}
  .artist-provider-action{display:block;margin-top:.5rem}.notice{padding:.65rem .8rem;border-left:3px solid var(--accent,#67c3a5);background:var(--input,#22262b);border-radius:4px}.error{border-color:#ec7373}.overview-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:.75rem}.overview-grid article,.review{padding:.8rem;border:1px solid var(--border,#383d43);border-radius:8px}.bio{white-space:pre-wrap;max-height:12rem;overflow:auto}.links{display:flex;gap:.75rem;flex-wrap:wrap}.release-list article{align-items:flex-start}.release-list article>div{min-width:0}.review{display:flex;flex-direction:column;gap:.6rem}.field{display:flex;gap:.65rem;align-items:flex-start;padding:.5rem;border:1px solid var(--border,#383d43);border-radius:6px}.field>span{display:flex;flex-direction:column;gap:.2rem}.candidate{justify-content:space-between;padding:.5rem 0}
  .release-source{display:block;margin-bottom:.35rem}
  .catalog-provider-details{font-size:.82rem;color:var(--muted)}.catalog-provider-details summary{cursor:pointer}.edition-select{flex-direction:row;justify-content:flex-start;margin:0}.release-info{overflow-wrap:anywhere}
  .album-title{display:block;max-width:100%;min-width:0;background:transparent;color:inherit;text-align:left;padding:0;white-space:normal;overflow-wrap:anywhere}.album-title strong{overflow-wrap:anywhere}
  .catalog-controls{display:flex;gap:.55rem;align-items:center;flex-wrap:wrap;min-width:0}.catalog-controls input{flex:1;min-width:min(12rem,100%)}.catalog-controls select,.edition-select select{background:var(--input,#22262b);color:var(--text,#eee);color-scheme:dark;border:1px solid var(--border,#383d43);border-radius:7px;padding:.6rem;max-width:100%;min-width:0}.catalog-count{margin-top:-.6rem}.release-card{display:grid!important;grid-template-columns:minmax(0,1fr);align-items:center!important;gap:.65rem!important}.release-card-header{display:flex;flex-wrap:wrap;align-items:center;gap:.65rem;min-width:0}.release-card-main{display:flex;flex:1 1 20rem;align-items:center;gap:.8rem;min-width:0}.release-art{position:relative;width:62px;height:62px;flex:none;display:grid;place-items:center;border-radius:7px;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:1.8rem;overflow:hidden}.release-art :global(img){position:absolute;inset:0;width:100%;height:100%;object-fit:cover}.release-info{flex:1 1 0;min-width:0;overflow-wrap:anywhere}.release-info strong{font-size:1rem}.release-info p{color:var(--muted,#a3a8ae);font-size:.86rem;overflow-wrap:anywhere}.release-actions{display:flex;flex:1 1 22rem;gap:.3rem;flex-wrap:wrap;justify-content:flex-end;min-width:0;max-width:100%}.release-actions button{max-width:100%;white-space:normal;overflow-wrap:anywhere;padding:.43rem .58rem;font-size:.8rem}.edition-select{grid-column:1/-1;display:flex;align-items:center;gap:.5rem;font-size:.82rem;color:var(--muted,#a3a8ae);min-width:0}.edition-select select{flex:1}.source-track-details,.expanded-tracks,.release-source-link{grid-column:1/-1;min-width:0}.source-track-details{font-size:.85rem;color:var(--muted,#a3a8ae)}.expanded-tracks{border-top:1px solid var(--border,#383d43);padding-top:.4rem}.release-source-link{justify-content:space-between;overflow-wrap:anywhere}
  .album-save-status{grid-column:1/-1;margin:0;font-size:.82rem}
  .release-tracklist{max-height:14rem;overflow:auto;padding-left:1.7rem}.release-tracklist li{padding:.3rem}.release-tracklist li.possible-match{font-weight:700;color:var(--accent,#67c3a5)}.release-tracklist small{display:block}.source-tags,.source-coverage{list-style:none;padding:0;margin:.5rem 0;display:flex;flex-direction:column;gap:.45rem}.source-tags li,.source-coverage li{display:flex;flex-wrap:wrap;gap:.45rem;align-items:baseline}.source-tags small,.source-coverage small{flex-basis:100%;padding-left:.2rem}.source-coverage li{padding:.35rem 0;border-bottom:1px solid var(--border,#383d43)}
  @media(max-width:650px){.release-card{grid-template-columns:minmax(0,1fr)}.release-actions{justify-content:flex-start}.release-list article{align-items:flex-start;flex-direction:column}.search{flex-wrap:wrap}.search input{flex-basis:100%}.catalog-controls>*{flex:1;min-width:9rem}.release-source-link{flex-direction:row!important}}
  @container artist-pane (max-width:700px){.artist-result{align-items:flex-start;flex-direction:column;gap:.35rem}.artist-result .source{text-align:left}.artist-heading{align-items:flex-start;flex-wrap:wrap}.artist-heading h2{overflow-wrap:anywhere}.overview-grid{grid-template-columns:minmax(0,1fr)}.release-card{grid-template-columns:minmax(0,1fr)}.release-actions{justify-content:flex-start}.release-actions button{flex:1 1 auto}.search{flex-wrap:wrap}.search input{flex-basis:100%}.catalog-controls input{flex-basis:100%}.catalog-controls>select{flex:1 1 9rem}.candidate{align-items:flex-start;flex-direction:column;overflow-wrap:anywhere}}

  /* Nested controls must not inherit the pane rule's vertical 9rem flex basis. */
  @media (min-width:901px) {
    .catalog-controls { gap:6px 8px; align-items:end; }
    .catalog-controls > label { display:flex; flex-direction:column; align-items:stretch; flex:1 1 140px; min-width:0; margin:0; gap:3px; font-size:11px; color:var(--muted); }
    .catalog-controls > label > select { flex:none; box-sizing:border-box; width:100%; min-width:0; max-width:100%; height:34px; min-height:34px; padding:5px 8px; font-size:12px; line-height:normal; }
    .catalog-controls > input { box-sizing:border-box; min-height:34px; height:34px; padding:5px 8px; font-size:12px; }
    .catalog-controls > select { box-sizing:border-box; flex:1 1 140px; min-height:34px; height:auto; padding:5px 8px; font-size:12px; line-height:normal; }
    .release-track-controls { margin:0 0 6px; }
    .release-track-controls > input { flex:1 1 100%; min-width:0; }
    .release-track-controls > label:first-of-type { flex-basis:170px; }
    .release-track-controls > label:nth-of-type(2) { flex-basis:110px; }
    .release-track-controls > span { flex:0 1 auto; font-size:12px; line-height:1.3; align-self:center; }
  }
  .artist-page .artist-provider-action{box-sizing:border-box;min-height:34px;height:auto;padding:.3rem .6rem;font-size:.8rem;line-height:1.2}
  .artist-page .artist-provider-action{display:inline-flex;vertical-align:middle;margin-top:0;margin-left:.5rem}
  .artist-page .catalog-controls>select,.artist-page .catalog-controls>label>select,.artist-page .edition-select select{appearance:none;-webkit-appearance:none;box-sizing:border-box;display:block;height:34px;min-height:34px;max-height:34px;padding:0 30px 0 9px;font-size:12px;line-height:18px;text-align:left;background-image:linear-gradient(45deg,transparent 50%,var(--muted,#a3a8ae) 50%),linear-gradient(135deg,var(--muted,#a3a8ae) 50%,transparent 50%);background-position:calc(100% - 14px) 50%,calc(100% - 10px) 50%;background-size:4px 4px;background-repeat:no-repeat}
</style>
