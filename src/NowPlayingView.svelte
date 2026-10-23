<script lang="ts">
  import { onMount } from "svelte";
  import ArtworkImage from "./ArtworkImage.svelte";
  import ArtistInformation from "./ArtistInformation.svelte";
  import RecordingInformation from "./RecordingInformation.svelte";
  import LyricsPanel, { type CurrentTrackLyrics, type LyricsSyncResult } from "./LyricsPanel.svelte";
  import type {RecordingInformationResult, RecordingReleaseInfo} from "./lib/recording-information";
  import type { Track } from "./lib/types";
  import { fullscreenArtworkUpgradeCandidates } from "./lib/fullscreen-artwork";
  import type { ArtistProfile, ArtistRef } from "./lib/artist-types";
  let { track, lyricsTrack = track, absolutePosition = 0, loadLyrics, syncLyrics, syncScope, translationEnabled = false, translationCredentialPresent = false, translateLyrics, nextTrack = null, title, artworkUrl, bottom = 100, context = "Now playing", loadArtist, loadRecording, onRecordingRelease, onClose, onArtist, onArtistRef, onAlbum, onQueue, onFavorite }: {
    track: Track; nextTrack?: Track | null; title: string; artworkUrl?: string | null; bottom?: number; context?: string;
    lyricsTrack?: Track; absolutePosition?: number; loadLyrics?: () => Promise<CurrentTrackLyrics>; syncLyrics?: () => Promise<LyricsSyncResult | null>; syncScope?: string;
    translationEnabled?: boolean; translationCredentialPresent?: boolean; translateLyrics?: (sourceHash: string, lines: {id:string;timeSeconds:number|null;text:string}[]) => Promise<import("./lib/lyrics-translation").LyricsTranslationResult>;
    loadArtist: (track: Track) => Promise<ArtistProfile | null>;
    loadRecording?: (track: Track) => Promise<RecordingInformationResult>;
    onRecordingRelease?: (release:RecordingReleaseInfo, artist:ArtistRef|null) => void; onClose: () => void; onArtist: () => void; onArtistRef?: (artist: ArtistRef) => void;
    onAlbum?: () => void; onQueue: () => void; onFavorite: () => void;
  } = $props();
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
  let closeButton: HTMLButtonElement;
  onMount(() => {
    const previous = document.activeElement;
    closeButton.focus({preventScroll:true});
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  });
</script>
<svelte:window onkeydown={(event) => { if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); onClose(); } }} />
<section class="now-playing-view" style:padding-bottom={`${bottom + 48}px`} style:scroll-padding-bottom={`${bottom}px`} aria-label="Expanded Now Playing">
  <header><div><small>{context === "Now playing" ? "NOW PLAYING" : "PLAYING FROM"}</small>{#if context !== "Now playing"}<strong>{context}</strong>{/if}</div><button bind:this={closeButton!} onclick={onClose} aria-label="Close full-screen Now Playing" title="Close (Escape)">⤢</button></header>
  <div class="content">
    <div class="hero-art"><ArtworkImage urls={[artworkUrl, track.artworkUrl]} upgradeCandidates={fullscreenArtworkUpgradeCandidates(track)} /></div>
    <div class="recording"><h1>{title}</h1><button class="artist-link" onclick={onArtist}>{track.artist || "Artist unknown"}</button>{#if track.album}<button class="album-link" disabled={!onAlbum} onclick={onAlbum}>{track.album}</button>{/if}<button class="favorite" onclick={onFavorite} aria-label={track.favorite ? "Remove from favorites" : "Add to favorites"} aria-pressed={track.favorite}>{track.favorite ? "♥" : "♡"}</button></div>
    <div class="cards">
      <article class="artist-card"><ArtistInformation {track} {loadArtist} {onArtist} {onArtistRef} />{#if loadRecording}<RecordingInformation {track} {loadRecording} onRelease={onRecordingRelease} onArtist={onArtistRef} />{/if}</article>
      <div class="right-cards"><article><h2>Credits</h2>{#if credits.length}{#each credits as credit}<p class="credit-name">{credit.name}</p><small>{credit.role}</small>{/each}{:else}<p class="credit-name">{track.artist || "Artist unknown"}</p><small>Credited performers</small>{/if}{#if track.remixers?.length}<p class="credit-name">{track.remixers.join(", ")}</p><small>Remix</small>{/if}{#if track.label}<p class="credit-name">{track.label}</p><small>Label</small>{/if}</article>
      <article><div class="card-heading"><h2>Next in queue</h2><button class="subtle" onclick={onQueue}>Open queue</button></div>{#if nextTrack}<div class="next-track"><div class="next-art"><ArtworkImage urls={[nextTrack.artworkUrl]} /></div><div><strong>{nextTrack.title}</strong><p>{nextTrack.artist}</p></div></div>{:else}<p>No more tracks queued.</p>{/if}</article></div>
      {#if loadLyrics}<article class="lyrics-card"><LyricsPanel track={lyricsTrack} {absolutePosition} {loadLyrics} {syncLyrics} {syncScope} {translationEnabled} {translationCredentialPresent} {translateLyrics} /></article>{/if}
    </div>
  </div>
</section>
<style>
  .now-playing-view{position:fixed;inset:0;z-index:55;overflow:auto;scroll-padding-top:24px;overscroll-behavior:contain;background:linear-gradient(165deg,#175b51 0%,#102d29 50%,var(--bg,#101512) 90%);color:var(--text);padding:28px 32px 48px;box-sizing:border-box}header{min-height:44px;padding:10px 12px;display:flex;align-items:center;justify-content:space-between;gap:20px}header div{display:grid;gap:5px}header small{font-size:10px;letter-spacing:.14em}header button{position:fixed;top:24px;right:28px;z-index:1;display:grid;place-items:center;width:44px;height:44px;border-radius:50%;font-size:24px;background:#103c35e8;border:0;color:inherit;padding:0}.content{max-width:1300px;margin:40px auto 0}.hero-art{width:min(58vh,600px,100%);aspect-ratio:1;margin:0 auto;border-radius:16px;overflow:hidden;box-shadow:0 18px 60px #0003}.recording{max-width:650px;margin:24px auto 36px;position:relative;padding-right:55px}.recording h1{font-size:clamp(22px,3vw,36px);margin:0 0 10px;overflow-wrap:anywhere}.artist-link,.album-link{display:block;border:0;background:transparent;color:inherit;text-align:left;padding:0;cursor:pointer}.artist-link{font-size:18px}.album-link{font-size:14px;color:var(--muted);margin-top:8px}.album-link:disabled{cursor:default}.favorite{position:absolute;right:0;top:8px;background:transparent;border:0;color:var(--accent);font-size:30px}.cards{align-items:start;display:grid;grid-template-columns:1fr 1fr;gap:18px}.right-cards{display:grid;align-content:start;gap:18px}article{background:#202522eF;border-radius:18px;padding:28px;min-width:0}.lyrics-card{grid-column:1/-1}h2{font-size:24px;margin:0 0 22px}p{line-height:1.6;color:#b5beb8;overflow-wrap:anywhere}.credit-name{color:var(--text);margin:18px 0 2px}small{color:#b5beb8}.subtle{background:transparent;border:0;color:#c3cec7;padding:0;font-size:13px;cursor:pointer}.card-heading{display:flex;align-items:baseline;justify-content:space-between;gap:16px}.next-track{display:flex;align-items:center;gap:16px}.next-art{width:64px;height:64px;flex-shrink:0;border-radius:5px;overflow:hidden}.next-track strong{overflow-wrap:anywhere}.next-track p{margin:5px 0 0}button:hover:not(:disabled){text-decoration:underline}button:focus-visible{outline:2px solid var(--accent);outline-offset:5px}@media(max-width:700px){.now-playing-view{padding:18px 18px 30px}.content{margin-top:24px}.cards{grid-template-columns:1fr}.lyrics-card{grid-column:auto}article{padding:22px}.hero-art{width:min(55vh,100%)}h2{font-size:21px}}
</style>
