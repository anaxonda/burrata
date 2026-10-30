<script lang="ts">
  import { untrack } from "svelte";
  import ArtworkImage from "./ArtworkImage.svelte";
  import { call } from "./lib/api";
  import type { ArtistProfile, ArtistRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";
  let {track, loadArtist, onArtist, onArtistRef, compact = false}: {
    track: Track; loadArtist: (track: Track) => Promise<ArtistProfile | null>;
    onArtist: () => void; onArtistRef?: (artist: ArtistRef) => void; compact?: boolean;
  } = $props();
  let profile = $state<ArtistProfile | null>(null);
  let loading = $state(false);
  let expanded = $state(false);
  const identity = $derived(`${track.id}\n${track.artist}`);
  const wikipediaSource = $derived(profile?.biographySource === "Wikipedia" ? profile.links.find(link => link.provider === "wikipedia" && link.label === "Wikipedia biography")?.url : undefined);
  const biography = $derived((profile?.biography ?? "").replace(/\[a=([^\]]+)\]/gi, "$1").replace(/\[\/?(?:b|i|u)\]/gi, ""));
  $effect(() => {
    identity;
    const current = untrack(() => track);
    profile = null; loading = true; expanded = false;
    let cancelled = false;
    void loadArtist(current).then(value => {if (!cancelled) profile = value;}).catch(() => {}).finally(() => {if (!cancelled) loading = false;});
    return () => {cancelled = true;};
  });
  function browse() { if (profile && onArtistRef) onArtistRef(profile.artist.artist); else onArtist(); }
</script>
<section class="artist-information" class:compact aria-label="Artist information">
  <h2>About the artist</h2>
  {#if profile?.imageUrl}<div class="portrait"><ArtworkImage urls={[profile.imageUrl]} /></div>{/if}
  <button class="artist-name" onclick={browse}>{profile?.artist.name ?? (track.artist || "Find artist")}</button>
  {#if biography}<p class:clamped={!expanded}>{biography}</p><button class="subtle" onclick={() => expanded = !expanded}>{expanded ? "Show less" : "Read more"}</button>{#if profile?.biographySource}<small class="attribution">{#if wikipediaSource}<button class="source" onclick={() => void call("open_external_url", { url: wikipediaSource })}>Wikipedia ↗</button> · <button class="source" onclick={() => void call("open_external_url", { url: "https://creativecommons.org/licenses/by-sa/4.0/" })}>CC BY-SA ↗</button>{:else}{profile.biographySource}{/if}</small>{/if}
  {:else}<p>{loading ? "Loading artist information…" : "Artist information is not available for this recording."}</p>{/if}
  <button class="browse" onclick={browse}>Browse artist</button>
</section>
<style>
  h2{font-size:24px;margin:0 0 22px}.portrait{width:95px;height:95px;border-radius:50%;overflow:hidden;margin-bottom:22px}.artist-name{display:block;border:0;padding:0;background:transparent;color:var(--text);text-align:left;font-size:20px;font-weight:700;cursor:pointer}p{line-height:1.6;color:var(--muted);overflow-wrap:anywhere}.clamped{display:-webkit-box;-webkit-line-clamp:4;line-clamp:4;-webkit-box-orient:vertical;overflow:hidden}.subtle{background:transparent;border:0;color:var(--muted);padding:0;font-size:13px;cursor:pointer}.source{border:0;background:transparent;color:inherit;padding:0;font:inherit;cursor:pointer;text-decoration:underline}.attribution{display:block;margin-top:12px;color:var(--muted)}.browse{margin-top:18px;background:var(--raised);color:var(--text);border:1px solid var(--line);border-radius:8px;padding:7px 12px}.compact h2{font-size:15px;margin-bottom:12px}.compact .artist-name{font-size:14px}.compact .portrait{width:64px;height:64px;margin-bottom:12px}.compact p{font-size:12px}.compact .browse{font-size:12px}button:hover{text-decoration:underline}button:focus-visible{outline:2px solid var(--accent);outline-offset:4px}
</style>
