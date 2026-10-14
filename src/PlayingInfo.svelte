<script lang="ts">
  import SourceLink from "./SourceLink.svelte";
  import { call } from "./lib/api";
  import PlaybackQuality from "./PlaybackQuality.svelte";
  import type { Track, Candidate, LocalAsset } from "./lib/types";

  let {
    track,
    candidate,
    onClose,
    onArtist,
    onDownload,
    onUpgrade = onDownload,
    asset = null,
    streamQuality = null,
    onPlaylist,
    onRadio,
    radioBusy = false,
    docked = false,
    activeChapter = "",
    onPlay,
    onMoreInfo,
    onFavorite,
    favorite = track.favorite,
    panelWidth = 340,
    onPanelWidthChange,
  }: {
    track: Track;
    candidate?: Candidate | null;
    onClose: () => void;
    onArtist: () => void;
    onDownload: () => void;
    onUpgrade?: () => void;
    asset?: LocalAsset | null;
    streamQuality?: { codec?: string; bitrateKbps?: number; live?: boolean } | null;
    onPlaylist: () => void;
    onRadio?: () => void;
    radioBusy?: boolean;
    docked?: boolean;
    activeChapter?: string;
    onPlay?: () => void;
    onMoreInfo?: () => void;
    onFavorite?: () => void;
    favorite?: boolean;
    panelWidth?: number;
    onPanelWidthChange?: (width: number) => void;
  } = $props();

  let resizing = $state(false);
  function resizePanel(event: PointerEvent) {
    if (!docked || event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = panelWidth;
    resizing = true;
    const move = (next: PointerEvent) => onPanelWidthChange?.(Math.max(280, Math.min(420, startWidth + startX - next.clientX)));
    const end = () => { resizing = false; window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", end); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end, { once: true });
  }
  let imageFailed = $state(false);
  let imageUrl = $state("");
  const coverUrl = $derived(track.artworkUrl || candidate?.artworkUrl || "");
  function safeHttpUrl(value: unknown): string {
    if (typeof value !== "string") return "";
    try {
      const url = new URL(value);
      return (url.protocol === "http:" || url.protocol === "https:") && !url.username && !url.password ? url.href : "";
    } catch { return ""; }
  }
  const radioLinks = $derived.by(() => {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const streamUrl = safeHttpUrl(provenance.radioStreamUrl);
    const isRadio = provenance.sourceKind === "radio" || Boolean(streamUrl);
    return { isRadio, streamUrl, stationUrl: isRadio ? safeHttpUrl(track.sourceUrl) : "" };
  });
  function openRadioUrl(url: string) {
    const safeUrl = safeHttpUrl(url);
    if (safeUrl) void call("open_external_url", { url: safeUrl });
  }
  $effect(() => {
    if (imageUrl !== coverUrl) {
      imageUrl = coverUrl;
      imageFailed = false;
    }
  });

  function focusPanel(node: HTMLElement) {
    if (docked) return { destroy() {} };
    const previous = document.activeElement as HTMLElement | null;
    node.querySelector<HTMLButtonElement>(".close")?.focus();
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    node.addEventListener("keydown", onKeydown);
    return {
      destroy() {
        node.removeEventListener("keydown", onKeydown);
        if (previous?.isConnected) previous.focus();
      },
    };
  }
</script>

<div class="playing-info-backdrop" class:docked class:resizing style={`--info-panel-width:${Math.max(280, Math.min(420, panelWidth))}px`} role="presentation" onclick={(event) => { if (!docked && event.target === event.currentTarget) onClose(); }}>
  <aside
    use:focusPanel
    class="playing-info"
    aria-label="Playing track information"
    tabindex="-1"
  >
    {#if docked}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (Focusable window splitter provides keyboard resizing.) -->
    <div tabindex="0" class="panel-resize" role="separator" aria-label="Resize information panel" aria-orientation="vertical" aria-valuemin="280" aria-valuemax="420" aria-valuenow={Math.max(280, Math.min(420, panelWidth))} onpointerdown={resizePanel} onkeydown={(event) => { if (event.key === "ArrowLeft") { event.preventDefault(); onPanelWidthChange?.(Math.min(420, panelWidth + 10)); } else if (event.key === "ArrowRight") { event.preventDefault(); onPanelWidthChange?.(Math.max(280, panelWidth - 10)); } }}></div>{/if}
    <header>
      <div><span class="eyebrow">NOW PLAYING</span><strong>Track details</strong></div>
      <button class="close" aria-label="Close playing track information" onclick={onClose}>✕</button>
    </header>
    <div class="track-hero">
      {#if coverUrl && !imageFailed}
        <img src={coverUrl} alt={"Artwork for " + track.title} onerror={() => (imageFailed = true)} />
      {:else}
        <div class="cover-placeholder" aria-label="No artwork available">♫</div>
      {/if}
      <div class="hero-copy">
      {#if candidate?.provider || track.provider}<p class="eyebrow">{candidate?.provider ?? track.provider} audio</p>{/if}
        <h2>{activeChapter || track.title}{!activeChapter && track.version ? " (" + track.version + ")" : ""}</h2>
      {#if track.artist}<button class="artist-link" onclick={onArtist}>{track.artist}</button>{:else}<p class="unknown-artist">Artist unknown</p>{/if}
        {#if track.album}<p class="album">{track.album}</p>{/if}
      {#if activeChapter}<p class="chapter"><span>From</span><strong>{track.playbackRange?.parentTitle || track.title}</strong></p>{/if}
      </div>
    </div>

    {#if track.localAvailability === "available" || (!track.localAvailability && track.localPath)}
      <p class="downloaded-badge" aria-label="Downloaded and available on this device"><span aria-hidden="true">✓</span> Downloaded</p>
    {/if}

    <PlaybackQuality {track} {asset} {streamQuality} />

    {#if radioLinks.isRadio && (radioLinks.stationUrl || radioLinks.streamUrl)}
      <section class="radio-links" aria-label="Internet radio links">
        {#if radioLinks.stationUrl}<div><span>Station website</span><code>{radioLinks.stationUrl}</code><button aria-label="Open station website" onclick={() => openRadioUrl(radioLinks.stationUrl)}>Open ↗</button></div>{/if}
        {#if radioLinks.streamUrl}<div><span>Stream URL</span><code>{radioLinks.streamUrl}</code><button aria-label="Open stream URL" onclick={() => openRadioUrl(radioLinks.streamUrl)}>Open ↗</button></div>{/if}
      </section>
    {/if}

    <dl>
      {#if track.provider}<div><dt>Catalog</dt><dd>{track.provider}</dd></div>{/if}
      {#if candidate?.title}<div><dt>Audio source</dt><dd>{candidate.title}</dd></div>{/if}
      {#if candidate?.uploader}<div><dt>Uploader</dt><dd>{candidate.uploader}</dd></div>{/if}
      {#if track.bpm}<div><dt>Tempo</dt><dd>{track.bpm} BPM{track.provenance && (track.provenance as Record<string, unknown>).bpmAnalysis ? " · estimated" : ""}</dd></div>{/if}
      {#if track.key}<div><dt>Key</dt><dd>{track.key}{track.provenance && (track.provenance as Record<string, unknown>).keyAnalysis ? " · estimated" : ""}</dd></div>{/if}
      {#if track.label}<div><dt>Label</dt><dd>{track.label}</dd></div>{/if}
      {#if track.releaseDate}<div><dt>Released</dt><dd>{track.releaseDate}</dd></div>{/if}
      {#if track.durationSeconds}<div><dt>Duration</dt><dd>{Math.floor(track.durationSeconds / 60)}:{String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}</dd></div>{/if}
      {#if candidate?.provider === "monochrome" && candidate.formats[0]}
        <div><dt>Audio source</dt><dd>Monochrome · {candidate.formats[0].extension.toUpperCase()}{candidate.formats[0].lossless ? " (lossless)" : ""}</dd></div>
      {/if}
    </dl>

    <div class="actions">
      {#if onFavorite}<button aria-label={favorite ? "Remove from favorites" : "Add to favorites"} aria-pressed={favorite} onclick={onFavorite}>{favorite ? "♥ Favorited" : "♡ Add to favorites"}</button>{/if}
      {#if onPlay}<button class="primary" onclick={onPlay}>Play</button>{/if}
      {#if !radioLinks.isRadio}<SourceLink {track} />{/if}
      {#if candidate?.url && /^https:\/\//.test(candidate.url)}
        <button onclick={() => void call("open_external_url", { url: candidate!.url })}>Open {candidate.provider} source ↗</button>
      {/if}
      {#if onMoreInfo}<button onclick={onMoreInfo}>Search for more info</button>{/if}
      {#if track.localPath}<button onclick={() => void call("open_in_file_manager", { path: track.localPath })}>Show local file</button>{/if}
      <button onclick={onPlaylist}>Add to playlist</button>
      {#if onRadio}<button disabled={radioBusy} onclick={onRadio}>{radioBusy ? "Finding radio…" : "Start song radio"}</button>{/if}
      <button class="download" onclick={onDownload}>Download</button>
      <button class="upgrade" onclick={onUpgrade}>Find higher-quality download</button>
    </div>
  </aside>
</div>

<style>
  .playing-info-backdrop { position: fixed; z-index: 41; inset: 0; display: flex; justify-content: flex-end; padding: 12px 12px calc(var(--player-height, 92px) + 12px + env(safe-area-inset-bottom)); pointer-events: none; }
  .playing-info-backdrop.docked { position:relative; z-index:auto; inset:auto; display:flex; justify-content:flex-end; min-width:0; min-height:0; height:100%; padding:0; pointer-events:auto; }
  .playing-info { display: flex; flex-direction: column; position: relative; width: min(510px, calc(100vw - 24px)); min-width:0; min-height: 0; max-height: 100%; margin: 0; padding: 18px; overflow: auto; overscroll-behavior: contain; border: 1px solid var(--line); border-radius: 16px; background: var(--surface); color: var(--text); box-shadow: 0 18px 58px #0008; pointer-events: auto; }
  .docked .playing-info { width:var(--info-panel-width,340px); max-width:none; border-radius:0; border-block:0; box-shadow:none; }
  .panel-resize { position:absolute; z-index:2; left:0; top:0; width:8px; height:100%; padding:0; border:0; background:transparent; cursor:col-resize; touch-action:none; }
  .panel-resize:hover,.panel-resize:focus-visible { background:color-mix(in srgb,var(--accent) 35%,transparent); outline:0; }
  .resizing { user-select:none; }
  header { display: flex; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 12px; padding-bottom: 14px; }
  header > div { display: grid; gap: 4px; }
  header strong { font-size: 13px; font-weight: 580; }
  .eyebrow { margin: 0; color: var(--muted); font-size: 9px; font-weight: 650; letter-spacing: .12em; }
  .close { display: grid; place-items: center; width: 33px; height: 33px; padding: 0; border-radius: 9px; background: transparent; color: var(--muted); }
  .track-hero { display: grid; grid-template-columns: 132px minmax(0, 1fr); align-items: center; gap: 16px; }
  img, .cover-placeholder { display: grid; width: 132px; aspect-ratio: 1; object-fit: cover; border-radius: 11px; box-shadow: 0 8px 24px #0004; }
  .cover-placeholder { place-items: center; background: var(--raised); color: var(--muted); font-size: 54px; }
  .hero-copy { min-width: 0; }
  .hero-copy .eyebrow { margin-bottom: 6px; }
  h2 { margin: 0 0 6px; overflow-wrap: anywhere; font-size: clamp(19px, 2vw, 26px); line-height: 1.15; letter-spacing: -.025em; }
  .artist-link { display: block; max-width: 100%; padding: 2px 0; overflow-wrap: anywhere; background: transparent; color: var(--accent); text-align: left; font-size: 15px; }
  .unknown-artist { margin:2px 0; color:var(--muted); font-size:13px; }
  .chapter { display:grid; gap:3px; margin:9px 0 0; font-size:11px; }.chapter span { color:var(--muted); }.chapter strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-weight:550; }
  .album { margin: 5px 0 0; color: var(--muted); font-size: 12px; }
  .downloaded-badge { display:flex;align-items:center;gap:6px;align-self:flex-start;margin:14px 0 0;padding:5px 9px;border:1px solid color-mix(in srgb,var(--accent) 55%,var(--line));border-radius:999px;color:var(--accent);font-size:11px;font-weight:650; }
  .downloaded-badge span { display:grid;place-items:center;width:16px;height:16px;border-radius:50%;background:color-mix(in srgb,var(--accent) 20%,transparent);font-size:11px; }
  .actions .upgrade { color:var(--accent);border:1px solid color-mix(in srgb,var(--accent) 45%,var(--line));background:transparent; }
  .radio-links{display:grid;gap:7px;margin-top:14px;padding:10px 11px;border:1px solid var(--line);border-radius:10px;background:var(--raised)}
  .radio-links>div{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:3px 9px;align-items:center;min-width:0}
  .radio-links span{grid-column:1/-1;color:var(--muted);font-size:10px;font-weight:650;letter-spacing:.04em;text-transform:uppercase}
  .radio-links code{min-width:0;overflow-wrap:anywhere;color:var(--text);font-size:10px}
  .radio-links button{min-height:28px;padding:4px 8px;color:var(--accent);font-size:10px}
  dl { display: grid; grid-template-columns: 1fr 1fr; gap: 0 20px; margin: 18px 0 0; }
  dl > div { display: flex; justify-content: space-between; gap: 10px; min-width: 0; padding: 8px 0; border-bottom: 1px solid var(--line); }
  dt { flex: 0 0 auto; color: var(--muted); font-size: 11px; }
  dd { min-width: 0; margin: 0; overflow: hidden; text-align: right; text-overflow: ellipsis; white-space: nowrap; font-size: 11px; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: auto; padding-top: 17px; }
  .actions button { min-height: 35px; padding: 8px 12px; }
  @media (max-width: 1199px) {
    .playing-info-backdrop { padding: 8px 8px calc(var(--player-height, 100px) + 8px + env(safe-area-inset-bottom)); }
    .playing-info { width: 100%; padding: 14px; border-radius: 15px; }
    .track-hero { grid-template-columns: 96px minmax(0, 1fr); gap: 12px; }
    img, .cover-placeholder { width: 96px; }
    .cover-placeholder { font-size: 40px; }
    h2 { font-size: 19px; }
    .artist-link { font-size: 13px; }
    dl { grid-template-columns: 1fr; margin-top: 14px; }
  }
  @media (min-width:1200px) { .docked .playing-info { width:var(--info-panel-width,340px); padding:16px; }.docked .track-hero { grid-template-columns:minmax(72px,104px) minmax(0,1fr); gap:12px; }.docked img,.docked .cover-placeholder { width:100%; }.docked dl { grid-template-columns:1fr; }.docked .actions { margin-top:12px; } }
</style>
