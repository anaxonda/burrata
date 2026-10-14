<script lang="ts">
  import beatport from "./assets/providers/beatport.svg?url";
  import bandcamp from "./assets/providers/bandcamp.png?url";
  import discogs from "./assets/providers/discogs.svg?url";
  import lastfm from "./assets/providers/lastfm.svg?url";
  import local from "./assets/providers/local.svg?url";
  import musicbrainz from "./assets/providers/musicbrainz.svg?url";
  import soulseek from "./assets/providers/soulseek.svg?url";
  import soundcloud from "./assets/providers/soundcloud.svg?url";
  import shazam from "./assets/providers/shazam.png?url";
  import spotify from "./assets/providers/spotify.svg?url";
  import traxsource from "./assets/providers/traxsource.svg?url";
  import youtube from "./assets/providers/youtube.svg?url";

  let { provider, size = 20, class: className = "" }: { provider: string; size?: number; class?: string } = $props();
  const assets: Record<string, { src: string; label: string }> = {
    bandcamp: { src: bandcamp, label: "Bandcamp" },
    beatport: { src: beatport, label: "Beatport" },
    discogs: { src: discogs, label: "Discogs" },
    lastfm: { src: lastfm, label: "Last.fm" },
    local: { src: local, label: "Local files" },
    musicbrainz: { src: musicbrainz, label: "MusicBrainz" },
    soulseek: { src: soulseek, label: "Soulseek" },
    slskd: { src: soulseek, label: "Soulseek" },
    soundcloud: { src: soundcloud, label: "SoundCloud" },
    shazam: { src: shazam, label: "Shazam" },
    spotify: { src: spotify, label: "Spotify" },
    traxsource: { src: traxsource, label: "Traxsource" },
    youtube: { src: youtube, label: "YouTube" },
  };
  const normalized = $derived(provider.trim().toLowerCase());
  const asset = $derived(assets[normalized]);
  let failedProvider = $state("");
  const initials = $derived((asset?.label ?? provider).trim().slice(0, 1).toUpperCase() || "?");
</script>

<span class={`provider-logo ${className}`} style:--provider-logo-size={`${size}px`} aria-hidden="true" title={asset?.label ?? provider}>
  {#if asset && failedProvider !== normalized}
    <img src={asset.src} alt="" onerror={() => (failedProvider = normalized)} />
  {:else}
    <span class="provider-logo-fallback">{initials}</span>
  {/if}
</span>

<style>
  .provider-logo { display:inline-grid; flex:0 0 var(--provider-logo-size); place-items:center; width:var(--provider-logo-size); height:var(--provider-logo-size); overflow:hidden; }
  .provider-logo img { display:block; max-width:100%; max-height:100%; width:100%; height:100%; object-fit:contain; }
  .provider-logo-fallback { display:grid; place-items:center; width:100%; height:100%; border-radius:4px; color:var(--muted,#9aa0a6); font-size:calc(var(--provider-logo-size) * .65); font-weight:750; }
</style>
