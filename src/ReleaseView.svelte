<script lang="ts">
  import type { ArtistReleaseRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";
  import TrackList from "./TrackList.svelte";

  let {
    release, tracks = [], loading = false, error = "", onBack = () => {}, onPlay = () => {}, onQueue = () => {},
    onDownload = () => {}, onDownloadAlbum, onArtist, onContext, onPlaylist, onFavorite,
    playingTrackId = "", bufferingTrackId = "", paused = false,
  }: {
    release: ArtistReleaseRef & { artworkUrl?: string | null };
    tracks?: Track[]; loading?: boolean; error?: string;
    onBack?: () => void; onPlay?: (track: Track, tracks: Track[], index: number) => void;
    onQueue?: (tracks: Track[], mode: "next" | "append") => void; onDownload?: (track: Track) => void; onDownloadAlbum?: (tracks: Track[]) => void;
    onArtist?: (track: Track) => void; onContext?: (track: Track, event: MouseEvent) => void; onPlaylist?: (track: Track) => void; onFavorite?: (track: Track) => void;
    playingTrackId?: string; bufferingTrackId?: string; paused?: boolean;
  } = $props();

  let filter = $state("");
  let sort = $state<"track" | "title" | "duration">("track");
  const normalized = (value: string) => value.normalize("NFKC").trim().toLocaleLowerCase();
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
  const visibleTracks = $derived.by(() => {
    const rows = tracks.map((track, index) => ({ track, index })).filter(({ track }) => !filter.trim() || normalized(`${track.title} ${track.artist} ${track.version ?? ""}`).includes(normalized(filter)));
    return rows.sort((a, b) => sort === "title" ? a.track.title.localeCompare(b.track.title) || a.index - b.index
      : sort === "duration" ? (a.track.durationSeconds ?? Number.MAX_SAFE_INTEGER) - (b.track.durationSeconds ?? Number.MAX_SAFE_INTEGER) || a.index - b.index
        : trackNumber(a.track, a.index) - trackNumber(b.track, b.index) || a.index - b.index);
  });
  const albumTracks = $derived(tracks.map((track, index) => ({ track, index })).sort((a, b) => trackNumber(a.track, a.index) - trackNumber(b.track, b.index) || a.index - b.index).map(row => row.track));
  const artists = $derived([...new Set(release.credits.map(credit => credit.name).filter(Boolean))]);
  const cover = $derived(release.artworkUrl || tracks.find(track => track.artworkUrl)?.artworkUrl || null);
  function playAlbum() { if (albumTracks.length) onPlay(albumTracks[0], albumTracks, 0); }
</script>

<section class="release-view" aria-label={`${release.title} release`}>
  <button class="back" onclick={onBack}>← Back to search</button>
  <header class="release-hero">
    {#if cover}<img class="cover" src={cover} alt="" />{:else}<div class="cover placeholder" aria-hidden="true">♫</div>{/if}
    <div class="hero-copy">
      <p class="eyebrow">{release.primaryType ?? release.format ?? "Release"}</p>
      <h1>{release.title}</h1>
      {#if artists.length}<p class="artists">{#each artists as artist, i}<span>{#if i > 0}<span aria-hidden="true"> · </span>{/if}{#if tracks[0] && onArtist}<button onclick={() => onArtist?.(tracks[0])}>{artist}</button>{:else}{artist}{/if}</span>{/each}</p>{/if}
      <p class="release-meta">{#if release.year}{release.year}{:else if release.releaseDate}{release.releaseDate}{/if}{#if release.label}<span> · {release.label}</span>{/if}{#if release.country}<span> · {release.country}</span>{/if}</p>
      <div class="hero-actions"><button class="primary" disabled={!tracks.length || loading} onclick={playAlbum}>▶ Play album</button><button disabled={!tracks.length || loading} onclick={() => onQueue(albumTracks, "append")}>＋ Queue album</button>{#if onDownloadAlbum}<button disabled={!tracks.length || loading} onclick={() => onDownloadAlbum(albumTracks)}>↓ Download album</button>{/if}</div>
    </div>
  </header>
  <div class="track-heading"><h2>Tracks <span>{tracks.length}</span></h2><div class="track-controls"><input aria-label="Filter tracks" placeholder="Filter tracks" bind:value={filter} /><select aria-label="Sort tracks" bind:value={sort}><option value="track">Track number</option><option value="title">Title</option><option value="duration">Duration</option></select></div></div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if loading && !tracks.length}<p class="muted" role="status">Loading release tracks…</p>
  {:else if !tracks.length}<p class="muted">No track details available for this release.</p>
  {:else if !visibleTracks.length}<p class="muted">No tracks match this filter.</p>
  {:else}
    <TrackList tracks={visibleTracks.map(row => row.track)} listLabel="Release tracks" position={(track, index) => trackNumber(track, visibleTracks[index]?.index ?? index)} onPlay={(track, list, index) => onPlay(track, list, index)} onDownload={onDownload} onFavorite={onFavorite} onNext={(track) => onQueue([track], "next")} onQueue={(track) => onQueue([track], "append")} onPlaylist={onPlaylist} onArtist={onArtist} onContext={onContext} {playingTrackId} {bufferingTrackId} {paused} />
  {/if}
</section>

<style>
  .release-view{display:flex;flex-direction:column;gap:1rem;min-width:0;min-height:0;height:100%;overflow:auto;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px}.release-view button,.release-view input,.release-view select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}.release-view button{cursor:pointer}.release-view button:disabled{opacity:.5;cursor:default}.back{align-self:flex-start;border:0!important;background:transparent!important;color:var(--muted,#a3a8ae)!important;padding:.2rem!important}.release-hero{display:flex;align-items:flex-end;gap:1.15rem;padding:.6rem 0 1rem;border-bottom:1px solid var(--border,#383d43)}.cover{width:clamp(120px,20vw,190px);height:clamp(120px,20vw,190px);flex:none;object-fit:cover;border-radius:9px;box-shadow:0 8px 24px #0005}.placeholder{display:grid;place-items:center;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:4rem}.hero-copy{min-width:0}.eyebrow{margin:0;color:var(--muted,#a3a8ae);font-size:.75rem;text-transform:uppercase;letter-spacing:.1em}.hero-copy h1{margin:.35rem 0;font-size:clamp(1.6rem,4vw,2.65rem);line-height:1.1}.artists,.release-meta{margin:.35rem 0;color:var(--muted,#a3a8ae)}.artists button{border:0;background:none;padding:0;color:var(--text,#eee);text-decoration:underline}.hero-actions{display:flex;gap:.45rem;margin-top:.85rem;flex-wrap:wrap}.primary{background:var(--accent,#4c78c8)!important;border-color:var(--accent,#4c78c8)!important;color:white!important}.track-heading{display:flex;justify-content:space-between;align-items:center;gap:.75rem}.track-heading h2{margin:0;font-size:1.1rem}.track-heading h2 span{color:var(--muted,#a3a8ae);font-size:.8rem;font-weight:400}.track-controls{display:flex;gap:.4rem}.track-controls input{min-width:8rem}.track-controls select{color-scheme:dark}.error{color:#ff9b9b}.muted{color:var(--muted,#a3a8ae)}
  @media(max-width:620px){.release-view{padding:.7rem}.release-hero{align-items:center;gap:.75rem}.cover{width:105px;height:105px}.hero-actions button{padding:.4rem .5rem}.track-heading{align-items:flex-start;flex-direction:column}.track-controls{width:100%}.track-controls input{flex:1;min-width:0}}
</style>
