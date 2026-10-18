<script lang="ts">
  import {tick} from "svelte";
  import { trackSortAvailability, trackSortOptionLabel } from "./lib/track-sort-availability";
  import type { ArtistReleaseRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";
  import TrackList from "./TrackList.svelte";
  import { filterTracks, parseDurationBound, sortTracks, type SortDirection, type TrackSortField } from "./lib/track-sort";

  let {
    release, tracks = [], loading = false, error = "", onBack = () => {}, onPlay = () => {}, onQueue = () => {},
    onDownload = () => {}, onDownloadAlbum, onSaveToLibrary, onArtist, onContext, onPlaylist, onAddSelectedToPlaylist, onFavorite,
    playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false,
  }: {
    release: ArtistReleaseRef & { artworkUrl?: string | null };
    tracks?: Track[]; loading?: boolean; error?: string;
    onBack?: () => void; onPlay?: (track: Track, tracks: Track[], index: number) => void;
    onQueue?: (tracks: Track[], mode: "next" | "append") => void; onDownload?: (track: Track) => void; onDownloadAlbum?: (tracks: Track[]) => void; onSaveToLibrary?: (tracks: Track[]) => Promise<void>;
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
      query: filter,
      ...(lower === null ? {} : { minDurationSeconds: lower }),
      ...(upper === null ? {} : { maxDurationSeconds: upper }),
    });
    return sortTracks(filtered, sort, sortDirection).map(row => bySortObject.get(row)!);
  });
  const visibleTracks = $derived(visibleOccurrences.map(row => row.track));
  const artists = $derived([...new Set(release.credits.map(credit => credit.name).filter(Boolean))]);
  const cover = $derived(release.artworkUrl || tracks.find(track => track.artworkUrl)?.artworkUrl || null);
  function playAlbum() { if (albumTracks.length) onPlay(albumTracks[0], albumTracks, 0); }
  async function saveAlbumToLibrary() {
    if (!onSaveToLibrary || !albumTracks.length || savingToLibrary) return;
    const identity = saveReleaseIdentity;
    syncSaveReleaseIdentity(identity);
    const generation = ++saveReleaseGeneration;
    const isCurrentSave = () => generation === saveReleaseGeneration && identity === saveReleaseIdentity;
    savingToLibrary = true; saveLibraryError = ""; saveLibraryNotice = "";
    try {
      await onSaveToLibrary(albumTracks);
      if (isCurrentSave()) saveLibraryNotice = "Album tracks saved to Library.";
    } catch (reason) {
      if (isCurrentSave()) saveLibraryError = reason instanceof Error ? reason.message : String(reason || "Could not save tracks to Library.");
    } finally {
      if (isCurrentSave()) savingToLibrary = false;
    }
  }
</script>

<section bind:this={releaseElement} class="release-view" aria-label={`${release.title} release`}>
  <button class="back" onclick={onBack}>← Back to search</button>
  <header class="release-hero">
    {#if cover}<img class="cover" src={cover} alt="" />{:else}<div class="cover placeholder" aria-hidden="true">♫</div>{/if}
    <div class="hero-copy">
      <p class="eyebrow">{release.primaryType ?? release.format ?? "Release"}</p>
      <h1>{release.title}</h1>
      {#if artists.length}<p class="artists">{#each artists as artist, i}<span>{#if i > 0}<span aria-hidden="true"> · </span>{/if}{#if tracks[0] && onArtist}<button onclick={() => onArtist?.(tracks[0])}>{artist}</button>{:else}{artist}{/if}</span>{/each}</p>{/if}
      <p class="release-meta">{#if release.year}{release.year}{:else if release.releaseDate}{release.releaseDate}{/if}{#if release.label}<span> · {release.label}</span>{/if}{#if release.country}<span> · {release.country}</span>{/if}</p>
      <div class="hero-actions"><button class="primary" disabled={!tracks.length || loading} onclick={playAlbum}>▶ Play album</button><button disabled={!tracks.length || loading} onclick={() => onQueue(albumTracks, "append")}>＋ Queue album</button>{#if onDownloadAlbum}<button disabled={!tracks.length || loading} onclick={() => onDownloadAlbum(albumTracks)}>↓ Download album</button>{/if}{#if onSaveToLibrary}<button class="save-album" disabled={!tracks.length || loading || savingToLibrary} onclick={() => void saveAlbumToLibrary()}>{savingToLibrary ? "Saving to Library…" : saveAlbumButtonLabel}</button>{/if}</div>
    </div>
  </header>
  <div class="track-heading"><h2>Tracks <span>{tracks.length}</span></h2><div class="track-controls"><input aria-label="Filter tracks" placeholder="Filter tracks" bind:value={filter} /><select aria-label="Sort tracks" bind:value={sort}><option value="sourceOrder">Track number</option><option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabel(sortAvailability, "title", "Title")}</option><option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabel(sortAvailability, "artist", "Artist")}</option><option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabel(sortAvailability, "album", "Album")}</option><option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabel(sortAvailability, "duration", "Duration")}</option><option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabel(sortAvailability, "releaseDate", "Release date")}</option><option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabel(sortAvailability, "dateAdded", "Date added")}</option><option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabel(sortAvailability, "bpm", "BPM")}</option><option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabel(sortAvailability, "key", "Key")}</option><option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabel(sortAvailability, "plays", "Plays")}</option><option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabel(sortAvailability, "views", "Views")}</option></select><select aria-label="Sort direction" bind:value={sortDirection}><option value="asc">Ascending</option><option value="desc">Descending</option></select><label>Duration from<input aria-label="Minimum track duration" inputmode="numeric" placeholder="mm:ss" bind:value={minDuration} /></label><label>to<input aria-label="Maximum track duration" inputmode="numeric" placeholder="mm:ss" bind:value={maxDuration} /></label></div></div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if saveLibraryError}<p class="error" role="alert">{saveLibraryError}</p>{/if}
  {#if saveLibraryNotice}<p class="muted" role="status">{saveLibraryNotice}</p>{/if}
  {#if loading && !tracks.length}<p class="muted" role="status">Loading release tracks…</p>
  {:else if !tracks.length}<p class="muted">No track details available for this release.</p>
  {:else if !visibleTracks.length}<p class="muted">No tracks match this filter.</p>
  {:else}
    <TrackList tracks={visibleTracks} selectionContext={`release:${release.sourceUrl}`} selectionKeys={visibleOccurrences.map(row => String(row.sourceIndex))} selectionTracks={albumTracks} selectionTrackKeys={albumOccurrences.map(row => String(row.sourceIndex))} listLabel="Release tracks" position={(track, index) => trackNumber(track, visibleOccurrences[index]?.sourceIndex ?? index)} onPlay={(track, list, index) => onPlay(track, list, index)} onDownload={onDownload} onFavorite={onFavorite} onNext={(track) => onQueue([track], "next")} onQueue={(track) => onQueue([track], "append")} onPlaylist={onPlaylist} {onAddSelectedToPlaylist} onArtist={onArtist} onContext={onContext} {playingTrack} {bufferingTrack} {playingTrackId} {bufferingTrackId} {paused} />
  {/if}
</section>

<style>
  .release-view{display:flex;flex-direction:column;gap:1rem;min-width:0;min-height:0;height:100%;overflow:auto;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px}.release-view button,.release-view input,.release-view select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}.release-view button{cursor:pointer}.release-view button:disabled{opacity:.5;cursor:default}.back{align-self:flex-start;border:0!important;background:transparent!important;color:var(--muted,#a3a8ae)!important;padding:.2rem!important}.release-hero{display:flex;align-items:flex-end;gap:1.15rem;padding:.6rem 0 1rem;border-bottom:1px solid var(--border,#383d43)}.cover{width:clamp(120px,20vw,190px);height:clamp(120px,20vw,190px);flex:none;object-fit:cover;border-radius:9px;box-shadow:0 8px 24px #0005}.placeholder{display:grid;place-items:center;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:4rem}.hero-copy{min-width:0}.eyebrow{margin:0;color:var(--muted,#a3a8ae);font-size:.75rem;text-transform:uppercase;letter-spacing:.1em}.hero-copy h1{margin:.35rem 0;font-size:clamp(1.6rem,4vw,2.65rem);line-height:1.1}.artists,.release-meta{margin:.35rem 0;color:var(--muted,#a3a8ae)}.artists button{border:0;background:none;padding:0;color:var(--text,#eee);text-decoration:underline}.hero-actions{display:flex;gap:.45rem;margin-top:.85rem;flex-wrap:wrap}.primary{background:var(--accent,#4c78c8)!important;border-color:var(--accent,#4c78c8)!important;color:var(--base,#101412)!important}.track-heading{display:flex;justify-content:space-between;align-items:flex-start;flex-wrap:wrap;gap:.75rem}.track-heading h2{margin:0;font-size:1.1rem}.track-heading h2 span{color:var(--muted,#a3a8ae);font-size:.8rem;font-weight:400}.track-controls{display:flex;flex-wrap:wrap;align-items:center;gap:.4rem;min-width:0}.release-view .track-controls input,.release-view .track-controls select{height:34px;min-height:34px;box-sizing:border-box;padding:.3rem .5rem;font-size:.8rem}.track-controls input{min-width:6.75rem}.track-controls select{color-scheme:dark}.track-controls label{display:flex;align-items:center;gap:.3rem;color:var(--muted,#a3a8ae);font-size:.8rem;white-space:nowrap}.release-view .track-controls label input{width:4.75rem;min-width:4.75rem}.release-view button:focus-visible,.release-view input:focus-visible,.release-view select:focus-visible{outline:2px solid var(--accent,#b8ec8c);outline-offset:2px}.error{color:#ff9b9b}.muted{color:var(--muted,#a3a8ae)}
  @media(max-width:620px){.release-view{padding:.7rem}.release-hero{align-items:center;gap:.75rem}.cover{width:105px;height:105px}.hero-actions button{padding:.4rem .5rem}.track-heading{align-items:flex-start;flex-direction:column}.track-controls{width:100%}.track-controls input{flex:1;min-width:0}}
  .release-view .hero-actions button{box-sizing:border-box;min-height:34px;height:auto;padding:.3rem .6rem;font-size:.8rem;line-height:1.2}
</style>
