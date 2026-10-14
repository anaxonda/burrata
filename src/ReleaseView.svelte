<script lang="ts">
  import type { ArtistReleaseRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";

  let {
    release, tracks = [], loading = false, error = "", onBack = () => {}, onPlay = () => {}, onQueue = () => {},
    onDownload = () => {}, onDownloadAlbum, onArtist = () => {}, onContext = () => {}, onPlaylist = () => {},
  }: {
    release: ArtistReleaseRef & { artworkUrl?: string | null };
    tracks?: Track[]; loading?: boolean; error?: string;
    onBack?: () => void; onPlay?: (track: Track, tracks: Track[], index: number) => void;
    onQueue?: (tracks: Track[], mode: "next" | "append") => void; onDownload?: (track: Track) => void; onDownloadAlbum?: (tracks: Track[]) => void;
    onArtist?: (track: Track) => void; onContext?: (track: Track, event: MouseEvent) => void; onPlaylist?: (track: Track) => void;
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
  function duration(track: Track) { return track.durationSeconds == null ? "" : `${Math.floor(track.durationSeconds / 60)}:${String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}`; }
  function playAlbum() { if (albumTracks.length) onPlay(albumTracks[0], albumTracks, 0); }
</script>

<section class="release-view" aria-label={`${release.title} release`}>
  <button class="back" onclick={onBack}>← Back to search</button>
  <header class="release-hero">
    {#if cover}<img class="cover" src={cover} alt="" />{:else}<div class="cover placeholder" aria-hidden="true">♫</div>{/if}
    <div class="hero-copy">
      <p class="eyebrow">{release.primaryType ?? release.format ?? "Release"}</p>
      <h1>{release.title}</h1>
      {#if artists.length}<p class="artists">{#each artists as artist, i}<span>{#if i > 0}<span aria-hidden="true"> · </span>{/if}{#if tracks[0]}<button onclick={() => onArtist(tracks[0])}>{artist}</button>{:else}{artist}{/if}</span>{/each}</p>{/if}
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
    <ol class="track-list">{#each visibleTracks as row, index (`${row.track.id}:${row.index}`)}
      <li oncontextmenu={(event) => { event.preventDefault(); onContext(row.track, event); }}>
        <span class="position">{trackNumber(row.track, row.index)}</span>
        <div class="track-copy"><button class="track-title" aria-label={`Play ${row.track.title}`} onclick={() => onPlay(row.track, visibleTracks.map(item => item.track), index)}><strong>{row.track.title}{#if row.track.version}<small>({row.track.version})</small>{/if}</strong></button><span class="track-artist"><button class="artist-link" onclick={() => onArtist(row.track)}>{row.track.artist}</button></span></div>
        {#if duration(row.track)}<span class="duration">{duration(row.track)}</span>{/if}
        <div class="row-actions"><button class="icon-action" aria-label={`Download ${row.track.title}`} title="Download" onclick={() => onDownload(row.track)}>↓</button><details class="more"><summary aria-label={`More actions for ${row.track.title}`} title="More">⋯</summary><div class="more-menu"><button onclick={() => onQueue([row.track], "next")}>Play next</button><button onclick={() => onQueue([row.track], "append")}>Add to queue</button><button onclick={() => onPlaylist(row.track)}>Add to playlist</button></div></details></div>
      </li>
    {/each}</ol>
  {/if}
</section>

<style>
  .release-view{display:flex;flex-direction:column;gap:1rem;min-width:0;min-height:0;height:100%;overflow:auto;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px}.release-view button,.release-view input,.release-view select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}.release-view button{cursor:pointer}.release-view button:disabled{opacity:.5;cursor:default}.back{align-self:flex-start;border:0!important;background:transparent!important;color:var(--muted,#a3a8ae)!important;padding:.2rem!important}.release-hero{display:flex;align-items:flex-end;gap:1.15rem;padding:.6rem 0 1rem;border-bottom:1px solid var(--border,#383d43)}.cover{width:clamp(120px,20vw,190px);height:clamp(120px,20vw,190px);flex:none;object-fit:cover;border-radius:9px;box-shadow:0 8px 24px #0005}.placeholder{display:grid;place-items:center;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:4rem}.hero-copy{min-width:0}.eyebrow{margin:0;color:var(--muted,#a3a8ae);font-size:.75rem;text-transform:uppercase;letter-spacing:.1em}.hero-copy h1{margin:.35rem 0;font-size:clamp(1.6rem,4vw,2.65rem);line-height:1.1}.artists,.release-meta{margin:.35rem 0;color:var(--muted,#a3a8ae)}.artists button{border:0;background:none;padding:0;color:var(--text,#eee);text-decoration:underline}.hero-actions{display:flex;gap:.45rem;margin-top:.85rem;flex-wrap:wrap}.primary{background:var(--accent,#4c78c8)!important;border-color:var(--accent,#4c78c8)!important;color:white!important}.track-heading{display:flex;justify-content:space-between;align-items:center;gap:.75rem}.track-heading h2{margin:0;font-size:1.1rem}.track-heading h2 span{color:var(--muted,#a3a8ae);font-size:.8rem;font-weight:400}.track-controls{display:flex;gap:.4rem}.track-controls input{min-width:8rem}.track-controls select{color-scheme:dark}.track-list{list-style:none;margin:0;padding:0}.track-list li{display:flex;align-items:center;gap:.65rem;padding:.5rem .35rem;border-bottom:1px solid var(--border,#30343a)}.track-list li:hover{background:color-mix(in srgb,var(--surface,#202328) 65%,transparent)}.position{width:1.5rem;text-align:right;color:var(--muted,#a3a8ae);font-size:.82rem}.track-copy{display:flex;flex:1;min-width:0;flex-direction:column;align-items:flex-start}.track-title{display:flex;width:100%;min-width:0;flex-direction:column;align-items:flex-start;border:0!important;background:transparent!important;padding:.2rem!important;text-align:left}.track-title strong{max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:550}.track-title strong small{margin-left:.35rem;color:var(--muted,#a3a8ae);font-weight:400}.track-artist{font-size:.78rem;color:var(--muted,#a3a8ae)}.artist-link{border:0!important;background:transparent!important;padding:0!important;color:inherit!important;text-align:left;text-decoration:underline}.duration{width:2.8rem;text-align:right;color:var(--muted,#a3a8ae);font-size:.8rem}.row-actions{display:flex;align-items:center;gap:.2rem}.icon-action{width:2rem;height:2rem;padding:.2rem!important;border-radius:50%!important}.more{position:relative}.more summary{list-style:none;cursor:pointer;border:1px solid var(--border,#454a50);border-radius:50%;width:2rem;height:2rem;display:grid;place-items:center;color:var(--muted,#a3a8ae)}.more summary::-webkit-details-marker{display:none}.more-menu{position:absolute;right:0;top:2.2rem;z-index:2;display:flex;flex-direction:column;min-width:9rem;padding:.3rem;background:var(--panel,#17191c);border:1px solid var(--border,#454a50);border-radius:8px;box-shadow:0 8px 25px #0008}.more-menu button{text-align:left;border:0;background:transparent;padding:.45rem}.error{color:#ff9b9b}.muted{color:var(--muted,#a3a8ae)}
  @media(max-width:620px){.release-view{padding:.7rem}.release-hero{align-items:center;gap:.75rem}.cover{width:105px;height:105px}.hero-actions button{padding:.4rem .5rem}.track-heading{align-items:flex-start;flex-direction:column}.track-controls{width:100%}.track-controls input{flex:1;min-width:0}.track-list li{gap:.35rem}.track-title{font-size:.88rem}}
</style>
