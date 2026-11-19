<script lang="ts">
  import HeartIcon from "./HeartIcon.svelte";
  import { dismissDetails } from "./lib/dismiss-details";
  import { onMount } from "svelte";
  import ArtworkImage from "./ArtworkImage.svelte";
  import ArtistInformation from "./ArtistInformation.svelte";
  import RecordingInformation from "./RecordingInformation.svelte";
  import LyricsPanel, { type CurrentTrackLyrics, type LyricsSyncResult, type LyricCandidates } from "./LyricsPanel.svelte";
  import type {RecordingInformationResult, RecordingReleaseInfo} from "./lib/recording-information";
  import type { Track } from "./lib/types";
  import { fullscreenArtworkUpgradeCandidates } from "./lib/fullscreen-artwork";
  import type { ArtistProfile, ArtistRef } from "./lib/artist-types";
  let { track, setTrack = track, lyricsTrack = track, contextLookupConfigured = false, contextLookupRevision = 0, absolutePosition = 0, loadLyrics, findLyrics, chooseLyrics, saveLyrics, exportLyrics, translationLanguage, changeTranslationLanguage, syncLyrics, syncScope, translationEnabled = false, translationCredentialPresent = false, translateLyrics, onSeek, nextTrack = null, title, artworkUrl, sourceArtworkUrl, detectedArtworkUrl = null, artworkMode = "set", onArtworkModeChange, bottom = 100, context = "Now playing", loadArtist, loadRecording, onRecordingRelease, onClose, onArtist, onArtistRef, onAlbum, onQueue, queueOpen = false, onFavorite }: {
    track: Track; setTrack?: Track; contextLookupConfigured?: boolean; contextLookupRevision?: number; nextTrack?: Track | null; title: string; artworkUrl?: string | null; sourceArtworkUrl?: string | null; bottom?: number; context?: string;
    detectedArtworkUrl?: string | null; artworkMode?: "set" | "detected" | "both"; onArtworkModeChange?: (mode: "set" | "detected" | "both") => void;
    onSeek?: (seconds: number) => void;
    saveLyrics?: (sourceHash: string) => Promise<unknown>; exportLyrics?: (sourceHash: string,text:string) => Promise<{cancelled?:boolean}>; translationLanguage?:string; changeTranslationLanguage?: (language:string)=>Promise<unknown>;
    lyricsTrack?: Track; absolutePosition?: number; loadLyrics?: () => Promise<CurrentTrackLyrics>; findLyrics?: () => Promise<LyricCandidates>; chooseLyrics?: (scopeId: string, id: string) => Promise<CurrentTrackLyrics>; syncLyrics?: () => Promise<LyricsSyncResult | null>; syncScope?: string;
    translationEnabled?: boolean; translationCredentialPresent?: boolean; translateLyrics?: (sourceHash: string, lines: {id:string;timeSeconds:number|null;text:string}[]) => Promise<import("./lib/lyrics-translation").LyricsTranslationResult>;
    loadArtist: (track: Track) => Promise<ArtistProfile | null>;
    loadRecording?: (track: Track) => Promise<RecordingInformationResult>;
    onRecordingRelease?: (release:RecordingReleaseInfo, artist:ArtistRef|null) => void; onClose: () => void; onArtist: () => void; onArtistRef?: (artist: ArtistRef) => void;
    onAlbum?: () => void; onQueue: () => void; queueOpen?: boolean; onFavorite: () => void;
  } = $props();
  function contextTrackIdentity(value: Track): string { const range=value.playbackRange; return JSON.stringify([value.id,value.provider,value.title,value.artist,value.album ?? "",value.version ?? "",value.recordingDate ?? "",range ? [range.parentTrackId,range.chapterId,range.startTime,range.endTime] : null]); }
  const contextTargets = $derived.by(() => contextTrackIdentity(lyricsTrack) !== contextTrackIdentity(setTrack)
    ? [{track:lyricsTrack,label:"Detected song",targetKind:"song" as const},{track:setTrack,label:"Full set",targetKind:"set" as const}]
    : undefined);
  const credits = $derived.by(() => {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const catalog = provenance.catalog && typeof provenance.catalog === "object" ? provenance.catalog as Record<string, unknown> : {};
    const raw = Array.isArray(provenance.artistCredits) && provenance.artistCredits.length ? provenance.artistCredits : catalog.artists;
    if (!Array.isArray(raw)) return [];
    return raw.slice(0, 50).flatMap(value => {
      if (!value || typeof value !== "object" || typeof value.name !== "string" || !value.name.trim()) return [];
      return [{name: value.name.trim(), role: typeof value.role === "string" && value.role.trim() ? value.role : "Credited performer"}];
    });
  });
  let view: HTMLElement;
  let artworkMenu = $state<HTMLDetailsElement>();
  const playerInset = $derived(Math.max(0, Number.isFinite(bottom) ? bottom : 0));
  const showSetArtwork = $derived(artworkMode !== "detected" || !detectedArtworkUrl);
  onMount(() => {
    const previous = document.activeElement;
    view.scrollTop = 0;
    view.focus({ preventScroll: true });
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  });
</script>
<svelte:window onkeydown={(event) => { if (event.key === "Escape" && !queueOpen && !event.defaultPrevented) { event.preventDefault(); onClose(); } }} />
<section bind:this={view} class="now-playing-view" style:bottom={`${playerInset}px`} style:--player-inset={`${playerInset}px`} aria-label="Expanded Now Playing" tabindex="-1">
  <div class="artwork-stage">
    {#if artworkMode !== "set" && detectedArtworkUrl}
      <img class="artwork-ambient detected-ambient" src={detectedArtworkUrl} alt="" aria-hidden="true" loading="lazy" decoding="async" />
    {/if}
    <div class="hero-art" class:both-art={artworkMode === "both" && Boolean(detectedArtworkUrl)}>
      {#if showSetArtwork}
        <ArtworkImage urls={[artworkUrl, track.artworkUrl]} upgradeCandidates={fullscreenArtworkUpgradeCandidates(sourceArtworkUrl ? {...track, artworkUrl:sourceArtworkUrl} : track)} ambient />
      {/if}
      {#if (artworkMode === "detected" || artworkMode === "both") && detectedArtworkUrl}
        <img class="detected-artwork" src={detectedArtworkUrl} alt="Detected track artwork" />
      {/if}
    </div>
  </div>
  <div class="content">
    {#if context !== "Now playing" || (detectedArtworkUrl && onArtworkModeChange)}
      <header>
        {#if context !== "Now playing"}<div><small>PLAYING FROM</small><strong>{context}</strong></div>{/if}
        {#if detectedArtworkUrl && onArtworkModeChange}
          <details use:dismissDetails class="artwork-menu" bind:this={artworkMenu}>
            <summary aria-label="Fullscreen artwork options" title="Artwork display">⋯</summary>
            <div class="artwork-choice" role="group" aria-label="Artwork display">
              <strong class="artwork-choice-heading">Artwork display</strong>
              <label><input type="radio" name="fullscreen-artwork" checked={artworkMode === "set"} onchange={() => { onArtworkModeChange?.("set"); if (artworkMenu) artworkMenu.open = false; }} /> Set artwork</label>
              <label><input type="radio" name="fullscreen-artwork" checked={artworkMode === "detected"} onchange={() => { onArtworkModeChange?.("detected"); if (artworkMenu) artworkMenu.open = false; }} /> Detected track</label>
              <label><input type="radio" name="fullscreen-artwork" checked={artworkMode === "both"} onchange={() => { onArtworkModeChange?.("both"); if (artworkMenu) artworkMenu.open = false; }} /> Side by side</label>
            </div>
          </details>
        {/if}
      </header>
    {/if}
    <div class="recording">
      <h1>{title}</h1>
      {#if track.artist.trim()}<button class="artist-link" onclick={onArtist}>{track.artist}</button>
      {:else}<p class="artist-caption">Artist unknown</p>{/if}
      {#if track.album}
        {#if onAlbum}<button class="album-link" onclick={onAlbum}>{track.album}</button>
        {:else}<p class="album-caption">{track.album}</p>{/if}
      {/if}
      <button class="favorite" onclick={onFavorite} aria-label={track.favorite ? "Remove from favorites" : "Add to favorites"} aria-pressed={track.favorite}><HeartIcon filled={track.favorite} /></button>
    </div>
    <div class="cards">
      <article class="artist-card"><ArtistInformation {track} {loadArtist} {onArtist} {onArtistRef} />{#if loadRecording}<RecordingInformation {track} {loadRecording} {contextLookupConfigured} {contextLookupRevision} {contextTargets} onRelease={onRecordingRelease} onArtist={onArtistRef} />{/if}</article>
      <div class="right-cards"><article><h2>Credits</h2>{#if credits.length}{#each credits as credit}<p class="credit-name">{credit.name}</p><small>{credit.role}</small>{/each}{:else}<p class="credit-name">{track.artist || "Artist unknown"}</p><small>Credited performers</small>{/if}{#if track.remixers?.length}<p class="credit-name">{track.remixers.join(", ")}</p><small>Remix</small>{/if}{#if track.label}<p class="credit-name">{track.label}</p><small>Label</small>{/if}</article>
      <article><div class="card-heading"><h2>Next in queue</h2><button class="subtle" onclick={onQueue}>Open queue</button></div>{#if nextTrack}<div class="next-track"><div class="next-art"><ArtworkImage urls={[nextTrack.artworkUrl]} /></div><div><strong>{nextTrack.title}</strong><p>{nextTrack.artist}</p></div></div>{:else}<p>No more tracks queued.</p>{/if}</article></div>
      {#if loadLyrics}<article class="lyrics-card"><LyricsPanel track={lyricsTrack} {absolutePosition} bottomInset={bottom} {onSeek} {loadLyrics} {findLyrics} {chooseLyrics} {saveLyrics} {exportLyrics} {translationLanguage} {changeTranslationLanguage} {syncLyrics} {syncScope} {translationEnabled} {translationCredentialPresent} {translateLyrics} /></article>{/if}
    </div>
  </div>
</section>
<style>
  .now-playing-view {
    --cover-gutter: clamp(12px, 3vmin, 32px);
    --stage-height: max(0px, calc(100dvh - var(--player-inset)));
    --cover-height: max(0px, calc(var(--stage-height) - 2 * var(--cover-gutter)));
    position: fixed;
    inset: 0;
    z-index: 55;
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    background: var(--base, #101512);
    color: var(--text);
    outline: none;
  }
  .artwork-stage {
    --artwork-ambient-blur: 90px;
    --artwork-ambient-opacity: .38;
    box-sizing: border-box;
    display: grid;
    place-items: center;
    position: relative;
    isolation: isolate;
    overflow: hidden;
    height: var(--stage-height);
    padding: var(--cover-gutter);
  }
  .artwork-stage :global(.artwork-ambient) {
    mask-image: linear-gradient(#000 59%, transparent 79%);
  }
  .artwork-stage:has(.hero-art :global(.artwork-ambient)) > .detected-ambient {
    display: none;
  }
  .hero-art {
    width: min(100%, 1000px, var(--cover-height));
    min-width: 0;
    aspect-ratio: 1;
    border-radius: 16px;
    overflow: hidden;
    box-shadow: 0 18px 60px #0003;
  }
  .content {
    container-type: inline-size;
    width: min(calc(100% - 64px), 1300px);
    margin: 0 auto;
    padding: 24px 0;
  }
  header {
    position: relative;
    z-index: 1;
    min-height: 44px;
    padding: 0;
    gap: 16px;
  }
  header div { display: grid; gap: 5px; min-width: 0; overflow-wrap: anywhere; }
  header small { font-size: 10px; letter-spacing: .14em; }
  .recording{width:100%;box-sizing:border-box;align-self:stretch;max-width:650px;margin:16px auto 0;position:relative;padding-right:55px}.recording h1{font-size:clamp(22px,3vw,36px);margin:0 0 10px;overflow-wrap:anywhere}.artist-link,.album-link{display:block;max-width:100%;border:0;background:transparent;color:inherit;text-align:left;padding:0;white-space:normal;overflow-wrap:anywhere;cursor:pointer;text-decoration:underline;text-underline-offset:.2em}.artist-link,.artist-caption{font-size:18px}.artist-caption{margin:0}.album-link,.album-caption{font-size:14px;color:var(--muted);margin:8px 0 0}.favorite{position:absolute;right:0;top:8px;background:transparent;border:0;color:var(--accent);font-size:30px}.cards{align-items:start;display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:18px;margin-top:24px}.right-cards{display:grid;grid-template-columns:minmax(0,1fr);align-content:start;gap:18px}article{background:#202522eF;border-radius:18px;padding:28px;min-width:0}.lyrics-card{grid-column:1/-1}h2{font-size:24px;margin:0 0 22px}p{line-height:1.6;color:#b5beb8;overflow-wrap:anywhere}.credit-name{color:var(--text);margin:18px 0 2px}small{color:#b5beb8}.subtle{background:transparent;border:0;color:#c3cec7;padding:0;font-size:13px;cursor:pointer}.card-heading{display:flex;flex-wrap:wrap;align-items:baseline;justify-content:space-between;gap:16px}.next-track{display:flex;align-items:center;gap:16px}.next-art{width:64px;height:64px;flex-shrink:0;border-radius:5px;overflow:hidden}.next-track strong{overflow-wrap:anywhere}.next-track p{margin:5px 0 0}button:hover:not(:disabled){text-decoration:underline}button:focus-visible{outline:2px solid var(--accent);outline-offset:5px}@media(max-width:700px){.content{width:calc(100% - 36px)}}@media(max-height:600px){.recording{margin-top:12px}.recording h1{font-size:18px;line-height:1.15;margin-bottom:6px}.artist-link,.artist-caption{font-size:15px}.album-link,.album-caption{font-size:12px;margin-top:5px}}
  @container (max-width: 630px) {
    .cards { grid-template-columns: minmax(0, 1fr); }
    .lyrics-card { grid-column: auto; }
    article { padding: 22px; }
    h2 { font-size: 21px; }
  }
  .artwork-menu{position:relative;margin-left:auto;flex:none}.artwork-menu summary{display:grid;place-items:center;list-style:none;width:44px;height:44px;border-radius:50%;background:#103c35e8;color:inherit;font-size:18px;letter-spacing:1px;cursor:pointer}.artwork-menu summary::-webkit-details-marker{display:none}.artwork-menu summary:focus-visible{outline:2px solid var(--accent);outline-offset:3px}.artwork-choice{position:absolute;top:50px;right:0;display:grid;gap:8px;margin:0;padding:12px 14px;border:1px solid var(--line);border-radius:10px;background:var(--surface,#202522);box-shadow:0 12px 32px #0006;width:210px;max-width:calc(100vw - 110px);box-sizing:border-box}.artwork-choice-heading{display:block;padding:0 0 4px;color:var(--muted);font-size:11px;font-weight:600}.artwork-choice label{display:flex;flex-direction:row;align-items:center;gap:8px;margin:0;min-height:32px;font-size:13px;color:var(--text);white-space:nowrap;cursor:pointer}.artwork-choice input{flex:0 0 auto;width:14px;height:14px;margin:0;padding:0;accent-color:var(--accent)}.hero-art.both-art{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));width:min(100%,1200px,calc(2 * var(--cover-height) + 12px));aspect-ratio:auto;gap:12px;overflow:visible;box-shadow:none;border-radius:0}.hero-art.both-art > :global(*){width:100%;min-width:0;height:auto;aspect-ratio:1;justify-self:center;border-radius:12px;overflow:hidden;object-fit:cover}
  .artist-card :global(.artist-name){max-width:100%;white-space:normal;overflow-wrap:anywhere}
  .detected-artwork{width:100%;height:100%;object-fit:cover}
</style>
