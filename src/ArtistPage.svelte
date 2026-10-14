<script lang="ts">
  import type { Track } from "./lib/types";
  import type {
    ArtistPageProps, ArtistRef, ArtistSearchResult, ArtistProfile, ArtistReleaseRef,
    ArtistReleasePage, ArtistTrackPage, ArtistCollaborationPage, MetadataReview,
    MetadataFieldKey,
  } from "./lib/artist-types";
  import { onMount } from "svelte";

  let {
    activeTrack,
    artistName = null,
    initialArtistRef = null,
    initialSection = "overview",
    initialScroll = 0,
    onSectionChange,
    onArtistSelected,
    onScroll,
    localTracks,
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
    onDownloadAlbum,
    onAddTrackToPlaylist,
    onArtistNavigate,
  }: ArtistPageProps & { onDownloadAlbum?: (tracks: Track[], release: ArtistReleaseRef) => void | Promise<void> } = $props();

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
  let trackQuery = $state("");
  let trackSort = $state<"source" | "title" | "newest" | "oldest" | "duration">("source");
  let trackAvailability = $state<"all" | "on-device" | "not-on-device">("all");
  let expandedRelease = $state("");
  let editionByGroup = $state<Record<string, string>>({});
  let releaseTrackCache = $state<Record<string, Track[]>>({});
  let loadingRelease = $state<Record<string, boolean>>({});
  let releaseLoadError = $state("");
  const releaseTrackLoads = new Map<string, Promise<Track[]>>();
  let lastInitialSection = $state<"overview" | "releases" | "tracks" | "collaborations" | "local">("overview");
  let pageElement = $state<HTMLElement | null>(null);
  let lastInitialScroll = $state(0);
  let scrollRestorePending = $state(true);
  let lastInitialArtistKey = $state<string | null>(null);

  onMount(() => {
    if (!initialArtistRef) {
      const initialName = artistName?.trim() || activeTrack?.artist?.trim();
      if (initialName) { query = initialName; void runSearch(); }
    }
  });

  $effect(() => {
    if (lastInitialSection !== initialSection) {
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
    if (!initialArtistRef && artistName?.trim() && artistName.trim() !== query.trim()) {
      query = artistName.trim();
      void runSearch();
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

  const localMatches = $derived.by(() => {
    if (!selectedArtist) return [] as Track[];
    const names = [selectedArtist.name];
    if (profile) names.push(...profile.aliases);
    const normalized = names.map(normalizeName);
    return localTracks.filter((track) => normalized.includes(normalizeName(track.artist)));
  });

  function normalizeName(value: string): string {
    return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase();
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
    const chronology = (group: typeof result[number]) => Math.min(...group.editions.map((edition) => edition.year ?? Number(String(edition.releaseDate ?? "").slice(0, 4))).filter((year) => Number.isFinite(year) && year > 0), Number.MAX_SAFE_INTEGER);
    return result.sort((a,b) => {
      if (releaseSort === "title") return a.editions[0].title.localeCompare(b.editions[0].title);
      const aYear = chronology(a), bYear = chronology(b);
      if (aYear === Number.MAX_SAFE_INTEGER) return bYear === aYear ? a.editions[0].title.localeCompare(b.editions[0].title) : 1;
      if (bYear === Number.MAX_SAFE_INTEGER) return -1;
      return releaseSort === "oldest" ? aYear - bYear : bYear - aYear;
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

  const visibleSourceTracks = $derived.by(() => {
    const items = (tracks?.items ?? []).filter((track) => !trackQuery.trim() || normalizeName(`${track.title} ${track.artist} ${track.version ?? ""} ${track.releaseDate ?? ""}`).includes(normalizeName(trackQuery)))
      .filter((track) => trackAvailability === "all" || (trackAvailability === "on-device" ? Boolean(track.localPath || track.localAvailability === "available") : !track.localPath && track.localAvailability !== "available"));
    return [...items].sort((a,b) => trackSort === "title" ? a.title.localeCompare(b.title) : trackSort === "newest" ? (b.releaseDate ?? "").localeCompare(a.releaseDate ?? "") : trackSort === "oldest" ? (a.releaseDate ?? "").localeCompare(b.releaseDate ?? "") : trackSort === "duration" ? (a.durationSeconds ?? Number.MAX_SAFE_INTEGER) - (b.durationSeconds ?? Number.MAX_SAFE_INTEGER) : 0);
  });

  async function runSearch() {
    const term = query.trim();
    if (!term) return;
    const generation = ++requestGeneration;
    busy = true; error = ""; selectedArtist = null; profile = null;
    scrollRestorePending = true;
    try {
      const page = await searchArtists(term, 1);
      if (generation !== requestGeneration) return;
      results = page.results;
      searchPage = page.page;
      searchHasMore = page.hasMore;
      searchWarnings = page.warnings;
      if (!results.length) searchWarnings = [...searchWarnings, "No artists found."];
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function openArtist(artist: ArtistSearchResult) {
    const generation = ++requestGeneration;
    busy = true; error = ""; review = null;
    releaseTrackCache = {}; loadingRelease = {}; releaseLoadError = ""; expandedRelease = ""; editionByGroup = {};
    selectedArtist = artist; section = initialSection;
    scrollRestorePending = true;
    onArtistSelected?.(artist.artist, artist.name);
    releases = null; tracks = null; collaborations = null;
    try {
      const result = await getArtistProfile(artist.artist);
      if (generation !== requestGeneration) return;
      selectedArtist = result.artist;
      profile = result;
      releases = result.releases;
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
      results = [...results, ...page.results];
      searchPage = page.page;
      searchHasMore = page.hasMore;
      searchWarnings = page.warnings;
    } catch (cause) { if (generation === requestGeneration) error = message(cause); }
    finally { if (generation === requestGeneration) busy = false; }
  }

  async function loadSection(next: typeof section) {
    section = next;
    onSectionChange?.(next);
    if (!selectedArtist) return;
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
    } catch (cause) { error = message(cause); }
    finally { busy = false; }
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
    <button class="back" onclick={() => { selectedArtist = null; profile = null; onArtistSelected?.(null, query); }}>← Search results</button>
    <div class="artist-heading">
      {#if profile?.imageUrl}<img src={profile.imageUrl} alt="" />{/if}
      <div><p class="eyebrow">{providerLabel(selectedArtist.artist.provider)} identity</p>
        <h2>{selectedArtist.name}</h2>
        {#if selectedArtist.disambiguation}<p>{selectedArtist.disambiguation}</p>{/if}
        <a href={selectedArtist.sourceUrl} target="_blank" rel="noreferrer">Open source profile ↗</a>
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
        {#if profile.topTracks.length}<ul class="track-list">{#each profile.topTracks as track, index (track.id)}<li oncontextmenu={(event) => { if (onTrackContext) { event.preventDefault(); onTrackContext(track, event); } }}><div><span>{track.title}</span><small>{track.artist}</small></div><div class="track-actions">{#if onPlayTrack}<button class="secondary" onclick={() => void onPlayTrack(track, profile!.topTracks, index)}>Play</button>{/if}{#if onQueueTracks}<button class="secondary" onclick={() => void onQueueTracks?.([track], "append")}>Queue</button><button class="secondary" onclick={() => void onQueueTracks?.([track], "next")}>Next</button>{/if}{#if onDownloadTrack}<button class="secondary compact-action" aria-label={`Download ${track.title}`} onclick={() => void onDownloadTrack(track)}>↓</button>{/if}<details class="row-more"><summary>More</summary>{#if onDownloadTrack}<button class="secondary" onclick={() => void onDownloadTrack(track)}>Find sources</button>{/if}{#if onAddTrackToPlaylist}<button class="secondary" onclick={() => void onAddTrackToPlaylist(track)}>Add to playlist</button>{/if}</details></div></li>{/each}</ul>{:else}<p class="muted">Last.fm top tracks are unavailable for this artist or no API key is configured.</p>{/if}
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
      <div class="release-list">{#each releaseGroups as group (group.key)}
        {@const release = selectedEdition(group)}
        {@const artworkUrl = (releaseTrackCache[release.sourceUrl] ?? []).find((track) => track.artworkUrl)?.artworkUrl}
        <article class="release-card"><div class="release-card-main"><div class="release-art" aria-hidden="true">♫{#if artworkUrl}<img src={artworkUrl} alt="" onerror={(event) => { (event.currentTarget as HTMLImageElement).style.display = "none"; }} />{/if}</div><div class="release-info"><small class="release-source">{providerLabel(release.release.provider)}</small><button class="album-title" aria-expanded={expandedRelease === group.key} onclick={() => void toggleRelease(group)}><strong>{release.title}</strong></button><p>{release.year ?? "Year unknown"}{#if release.country} · {release.country}{/if}{#if release.label} · {release.label}{/if}</p><small>{release.format ?? release.primaryType ?? "Format unknown"}{#if release.catalogNumber} · {release.catalogNumber}{/if} · {release.trackCount ?? "?"} tracks</small></div></div>
          <div class="release-actions"><button class="secondary" aria-label={`Play album ${release.title}`} onclick={() => void playRelease(release)} disabled={!getReleaseTracks || !onPlayTrack || loadingRelease[release.sourceUrl]}>Play album</button><button class="secondary" aria-label={`Queue album ${release.title}`} onclick={() => void queueRelease(release, "append")} disabled={!getReleaseTracks || !onQueueTracks || loadingRelease[release.sourceUrl]}>Queue</button><button class="secondary" aria-label={`Play album ${release.title} next`} onclick={() => void queueRelease(release, "next")} disabled={!getReleaseTracks || !onQueueTracks || loadingRelease[release.sourceUrl]}>Play next</button>{#if onDownloadAlbum}<button class="secondary" aria-label={`Download album ${release.title}`} onclick={() => void downloadRelease(release)} disabled={!getReleaseTracks || loadingRelease[release.sourceUrl]}>Download album</button>{/if}<button class="secondary" aria-expanded={expandedRelease === group.key} onclick={() => void toggleRelease(group)}>Tracks</button></div>
          {#if release.tracks.length && !getReleaseTracks}<details class="source-track-details"><summary>Track titles from source · {release.tracks.length}</summary><ol class="release-tracklist">{#each release.tracks as sourceTrack, trackIndex (trackIndex)}<li>{sourceTrack.position ? `${sourceTrack.position}. ` : ""}{sourceTrack.title}</li>{/each}</ol></details>{/if}
          {#if group.editions.length > 1}<label class="edition-select">Edition <select value={release.sourceUrl} onchange={(event) => { editionByGroup = { ...editionByGroup, [group.key]: event.currentTarget.value }; const chosen = group.editions.find((item) => item.sourceUrl === event.currentTarget.value); if (chosen && expandedRelease === group.key) void loadReleaseTracks(chosen); }} aria-label={`Edition of ${release.title}`}>{#each group.editions as edition (edition.sourceUrl)}<option value={edition.sourceUrl}>{edition.year ?? "Year unknown"} · {providerLabel(edition.release.provider)}{edition.country ? ` · ${edition.country}` : ""}</option>{/each}</select></label>{/if}
          {#if expandedRelease === group.key}<div class="expanded-tracks">{#if releaseLoadError}<p class="notice error">{releaseLoadError}</p>{/if}{#if loadingRelease[release.sourceUrl]}<p class="muted">Loading this edition’s tracks…</p>{:else if (releaseTrackCache[release.sourceUrl] ?? []).length}<ol class="release-tracklist">{#each releaseTrackCache[release.sourceUrl] as track, trackIndex (trackIndex)}<li oncontextmenu={(event) => { if (onTrackContext) { event.preventDefault(); onTrackContext(track, event); } }}><span class="album-track-position">{trackIndex + 1}</span><button class="track-title" aria-label={`Play ${track.title}`} onclick={() => void playRelease(release, trackIndex)}><span>{track.title}{#if track.version} ({track.version}){/if}</span><small>{track.artist}</small></button>{#if track.durationSeconds}<span class="album-track-duration">{Math.floor(track.durationSeconds / 60)}:{String(Math.floor(track.durationSeconds % 60)).padStart(2,"0")}</span>{/if}<button class="secondary compact-action" onclick={() => void onQueueTracks?.([track], "append")}>Queue</button><button class="secondary compact-action" onclick={() => void onQueueTracks?.([track], "next")}>Next</button>{#if onDownloadTrack}<button class="secondary compact-action" aria-label={`Download ${track.title}`} onclick={() => void onDownloadTrack(track)}>↓</button>{/if}<details class="row-more"><summary>More</summary>{#if track.sourceUrl}<a href={track.sourceUrl} target="_blank" rel="noreferrer">Source ↗</a>{/if}{#if onDownloadTrack}<button class="secondary" onclick={() => void onDownloadTrack(track)}>Find sources</button>{/if}{#if onAddTrackToPlaylist}<button class="secondary" onclick={() => void onAddTrackToPlaylist(track)}>Add to playlist</button>{/if}</details></li>{/each}</ol>{:else}<p class="muted">No track details available for this edition.</p>{/if}</div>{/if}
          <div class="links release-source-link"><a href={release.sourceUrl} target="_blank" rel="noreferrer">Open edition source ↗</a>{#if activeTrack}<button class="secondary" disabled={busy} onclick={() => void inspectRelease(release)}>Review metadata</button>{/if}</div></article>
      {/each}</div>
      {#if releases?.items.length && !releaseGroups.length}<p class="muted">No loaded releases match these filters. <button class="secondary" onclick={() => { releaseQuery = ""; releaseFilter = "All"; releaseProvider = "All sources"; }}>Clear filters</button></p>{/if}
      {#if releases?.nextCursor}<button class="secondary" onclick={() => void moreReleases()} disabled={busy}>Load more releases</button>{/if}
      {#if !(releases?.items.length)}<p class="muted">No releases are available from this source.</p>{/if}
    {:else if section === "tracks"}
      <p class="muted">{tracks?.source ?? "Loading source recordings…"} · {visibleSourceTracks.length} of {(tracks?.items ?? []).length} loaded recordings</p>
      <div class="catalog-controls"><input bind:value={trackQuery} placeholder="Search loaded tracks" aria-label="Search loaded tracks" /><select bind:value={trackAvailability} aria-label="Track availability"><option value="all">All tracks</option><option value="on-device">On this device</option><option value="not-on-device">Not on this device</option></select><select bind:value={trackSort} aria-label="Sort tracks"><option value="source">Source order</option><option value="title">Title A–Z</option><option value="newest">Newest first</option><option value="oldest">Oldest first</option><option value="duration">Duration</option></select></div>
      <ul class="track-list">{#each visibleSourceTracks as track, index (`${track.id}:${index}`)}<li oncontextmenu={(event) => { if (onTrackContext) { event.preventDefault(); onTrackContext(track, event); } }}><div><span>{track.title}{#if track.version} ({track.version}){/if}</span><small>{track.artist} · {track.releaseDate ?? "date unknown"}</small></div><div class="track-actions">{#if onPlayTrack}<button class="secondary" onclick={() => void onPlayTrack(track, visibleSourceTracks, index)}>Play</button>{/if}{#if onQueueTracks}<button class="secondary" onclick={() => void onQueueTracks?.([track], "append")}>Queue</button><button class="secondary" onclick={() => void onQueueTracks?.([track], "next")}>Next</button>{/if}{#if onDownloadTrack}<button class="secondary compact-action" aria-label={`Download ${track.title}`} onclick={() => void onDownloadTrack(track)}>↓</button>{/if}<details class="row-more"><summary>More</summary>{#if onDownloadTrack}<button class="secondary" onclick={() => void onDownloadTrack(track)}>Find sources</button>{/if}{#if onAddTrackToPlaylist}<button class="secondary" onclick={() => void onAddTrackToPlaylist(track)}>Add to playlist</button>{/if}</details></div></li>{/each}</ul>
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
      <p class="muted">Local matches use exact artist-credit text after case and whitespace normalization. Same-name identities are not merged.</p>
      <ul class="track-list">{#each localMatches as track, index (track.id)}<li oncontextmenu={(event) => { if (onTrackContext) { event.preventDefault(); onTrackContext(track, event); } }}><div><span>{track.title}{#if track.version} ({track.version}){/if}</span><small>{track.album ?? "Unknown release"} · {track.localPath ?? track.localAvailability ?? "Library track"}</small></div><div class="track-actions">{#if onPlayTrack}<button class="secondary" onclick={() => void onPlayTrack(track, localMatches, index)}>Play</button>{/if}{#if onQueueTracks}<button class="secondary" onclick={() => void onQueueTracks?.([track], "append")}>Queue</button><button class="secondary" onclick={() => void onQueueTracks?.([track], "next")}>Next</button>{/if}</div></li>{/each}</ul>
      {#if !localMatches.length}<p class="muted">No exact artist-credit matches in the local library.</p>{/if}
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
  .artist-page{display:flex;flex:1;flex-direction:column;gap:1rem;padding:1.25rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px;min-width:0;min-height:0;height:100%;overflow:auto}
  .artist-page > * { flex-shrink: 0; }
  header,.artist-heading,.artist-result,.release-list article,.candidate{display:flex;align-items:center;justify-content:space-between;gap:1rem}
  h2,h3,h4,p{margin:.25rem 0}h2{font-size:1.4rem}h3{font-size:1rem}.eyebrow,.source,small,.muted{color:var(--muted,#a3a8ae);font-size:.82rem}.eyebrow{letter-spacing:.09em;font-weight:700}
  .search{display:flex;gap:.5rem}.search input{flex:1;min-width:80px}.search input,input[type=checkbox]{accent-color:var(--accent,#67c3a5)}
  input:not([type=checkbox]){background:var(--input,#22262b);border:1px solid var(--border,#383d43);color:inherit;border-radius:7px;padding:.65rem}
  button{border:0;border-radius:7px;padding:.6rem .8rem;background:var(--accent,#67c3a5);color:#111;font-weight:650;cursor:pointer}button:disabled{opacity:.5;cursor:not-allowed}.secondary,.back{background:transparent;color:inherit;border:1px solid var(--border,#383d43)}.back{align-self:start}
  .results,.release-list{display:flex;flex-direction:column;gap:.5rem}.artist-result,.release-list article{width:100%;text-align:left;background:var(--input,#22262b);color:inherit;border:1px solid var(--border,#383d43);padding:.8rem;border-radius:8px}.artist-result span:first-child{display:flex;flex-direction:column;gap:.2rem}.artist-heading{justify-content:flex-start}.artist-heading img{width:84px;height:84px;object-fit:cover;border-radius:8px}.tabs{display:flex;gap:.35rem;overflow:auto;border-bottom:1px solid var(--border,#383d43)}.tabs button{white-space:nowrap;background:transparent;color:inherit;border-radius:6px 6px 0 0}.tabs button.active{background:var(--accent,#67c3a5);color:#111}
  .notice{padding:.65rem .8rem;border-left:3px solid var(--accent,#67c3a5);background:var(--input,#22262b);border-radius:4px}.error{border-color:#ec7373}.overview-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:.75rem}.overview-grid article,.review{padding:.8rem;border:1px solid var(--border,#383d43);border-radius:8px}.bio{white-space:pre-wrap;max-height:12rem;overflow:auto}.links{display:flex;gap:.75rem;flex-wrap:wrap}.track-list{list-style:none;margin:0;padding:0}.track-list li{display:flex;justify-content:space-between;gap:.5rem;padding:.55rem 0;border-bottom:1px solid var(--border,#383d43)}.release-list article{align-items:flex-start}.release-list article>div{min-width:0}.review{display:flex;flex-direction:column;gap:.6rem}.field{display:flex;gap:.65rem;align-items:flex-start;padding:.5rem;border:1px solid var(--border,#383d43);border-radius:6px}.field>span{display:flex;flex-direction:column;gap:.2rem}.candidate{justify-content:space-between;padding:.5rem 0}
  .release-source{display:block;margin-bottom:.35rem}
  .catalog-provider-details{font-size:.82rem;color:var(--muted)}.catalog-provider-details summary{cursor:pointer}.edition-select{flex-direction:row;justify-content:flex-start;margin:0}.expanded-tracks .release-tracklist{max-height:none;overflow:visible;padding-left:0}.compact-action+.compact-action{margin-left:0}.release-info{overflow-wrap:anywhere}
  .album-title{background:transparent;color:inherit;text-align:left;padding:0}.track-title{flex:1;min-width:0;display:flex;flex-direction:column;overflow-wrap:anywhere}.album-track-position{color:var(--muted);font-size:.8rem;min-width:1.5rem}.album-track-duration{font-size:.8rem;color:var(--muted);white-space:nowrap}
  .catalog-controls{display:flex;gap:.55rem;align-items:center;flex-wrap:wrap}.catalog-controls input{flex:1;min-width:12rem}.catalog-controls select,.edition-select select{background:var(--input,#22262b);color:var(--text,#eee);color-scheme:dark;border:1px solid var(--border,#383d43);border-radius:7px;padding:.6rem;max-width:100%}.catalog-count{margin-top:-.6rem}.release-card{display:grid!important;grid-template-columns:minmax(0,1fr) auto;align-items:center!important;gap:.65rem!important}.release-card-main{display:flex;align-items:center;gap:.8rem}.release-art{position:relative;width:62px;height:62px;flex:none;display:grid;place-items:center;border-radius:7px;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:1.8rem;overflow:hidden}.release-art img{position:absolute;inset:0;width:100%;height:100%;object-fit:cover}.release-info{min-width:0}.release-info strong{font-size:1rem}.release-info p{color:var(--muted,#a3a8ae);font-size:.86rem}.release-actions{display:flex;gap:.3rem;flex-wrap:wrap;justify-content:flex-end}.release-actions button,.compact-action{padding:.43rem .58rem;font-size:.8rem}.edition-select{grid-column:1/-1;display:flex;align-items:center;gap:.5rem;font-size:.82rem;color:var(--muted,#a3a8ae)}.source-track-details,.expanded-tracks,.release-source-link{grid-column:1/-1}.source-track-details{font-size:.85rem;color:var(--muted,#a3a8ae)}.expanded-tracks{border-top:1px solid var(--border,#383d43);padding-top:.4rem}.expanded-tracks .release-tracklist{list-style:decimal}.expanded-tracks .release-tracklist li{display:flex;align-items:center;gap:.35rem}.track-title{background:transparent;color:inherit;text-align:left;padding:.3rem;font-weight:500}.compact-action{margin-left:auto}.release-source-link{justify-content:space-between}.track-actions{display:flex;align-items:center;gap:.3rem;flex-wrap:wrap}.track-actions button{padding:.4rem .55rem;font-size:.78rem}.row-more{position:relative}.row-more summary{cursor:pointer;list-style:none;border:1px solid var(--border,#383d43);border-radius:6px;padding:.35rem .5rem;color:var(--muted,#a3a8ae);font-size:.78rem}.row-more summary::-webkit-details-marker{display:none}.row-more[open]{display:flex;gap:.3rem;align-items:center;flex-wrap:wrap}.row-more a{font-size:.78rem}
  .release-tracklist{max-height:14rem;overflow:auto;padding-left:1.7rem}.release-tracklist li{padding:.3rem}.release-tracklist li.possible-match{font-weight:700;color:var(--accent,#67c3a5)}.release-tracklist small{display:block}.source-tags,.source-coverage{list-style:none;padding:0;margin:.5rem 0;display:flex;flex-direction:column;gap:.45rem}.source-tags li,.source-coverage li{display:flex;flex-wrap:wrap;gap:.45rem;align-items:baseline}.source-tags small,.source-coverage small{flex-basis:100%;padding-left:.2rem}.source-coverage li{padding:.35rem 0;border-bottom:1px solid var(--border,#383d43)}
  @media(max-width:650px){.release-card{grid-template-columns:minmax(0,1fr)}.release-actions{justify-content:flex-start}.release-list article,.track-list li{align-items:flex-start;flex-direction:column}.search{flex-wrap:wrap}.search input{flex-basis:100%}.catalog-controls>*{flex:1;min-width:9rem}.release-source-link{flex-direction:row!important}}
</style>
