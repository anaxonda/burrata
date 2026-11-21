<script lang="ts">
  import type { Snippet } from "svelte";
  import { safeArtworkUrl } from "./lib/artwork-url";

  let {
    name, label, artworkUrl, heading = "h2", class: className = "", details, actions, onArtworkError,
  }: {
    name: string;
    label: string;
    artworkUrl?: string | null;
    heading?: "h1" | "h2";
    class?: string;
    details?: Snippet;
    actions?: Snippet;
    onArtworkError?: (url: string) => void;
  } = $props();
  let failedArtwork = $state<string | null>(null);
  const source = $derived(artworkUrl !== failedArtwork ? safeArtworkUrl(artworkUrl) : null);

  function rejectArtwork(event: Event) {
    const url = (event.currentTarget as HTMLImageElement).getAttribute("src");
    if (!url) return;
    failedArtwork = url;
    onArtworkError?.(url);
  }
</script>

<header class={`artist-banner ${className}`} style:--artist-artwork={source ? `url(${JSON.stringify(source)})` : undefined}>
  <div class="portrait" aria-hidden="true">
    {#if source}<img src={source} alt="" onerror={rejectArtwork} />{:else}<span class="placeholder">♫</span>{/if}
  </div>
  <div class="identity">
    <p class="eyebrow">{label}</p>
    <svelte:element this={heading} class="artist-name">{name}</svelte:element>
    {@render details?.()}
    {#if actions}<div class="artist-actions">{@render actions()}</div>{/if}
  </div>
</header>

<style>
  .artist-banner {
    position:relative; isolation:isolate; display:flex; align-items:flex-end;
    gap:clamp(12px,2vw,20px); flex:none; min-width:0; min-height:clamp(132px,16vw,198px);
    box-sizing:border-box; padding:clamp(12px,2vw,20px); border-radius:9px;
    background:linear-gradient(145deg,color-mix(in srgb,var(--accent) 13%,var(--surface)),var(--base));
  }
  .artist-banner::before {
    content:""; position:absolute; inset:0; z-index:-1; border-radius:inherit;
    background:linear-gradient(90deg,var(--base),transparent),var(--artist-artwork) center / cover no-repeat;
    filter:blur(24px); clip-path:inset(0 round 9px); opacity:.35; pointer-events:none;
  }
  .portrait {
    display:grid; place-items:center; flex:none; width:clamp(72px,14vw,148px);
    aspect-ratio:1; overflow:hidden; border-radius:8px; background:var(--surface); box-shadow:0 8px 24px #0004;
  }
  .portrait img { width:100%; height:100%; object-fit:cover; }
  .placeholder { color:var(--muted); font-size:clamp(24px,5vw,46px); }
  .identity { flex:1; min-width:0; overflow-wrap:anywhere; }
  .identity > p { margin:0; color:var(--muted); font-size:.76rem; }
  .artist-name { margin:.2rem 0 .35rem; font-size:clamp(22px,3vw,42px); line-height:1.08; }
  .artist-actions { display:flex; flex-wrap:wrap; align-items:center; gap:.35rem; margin-top:.4rem; }
  .artist-actions :global(.source-actions-trigger) { margin-inline-start:auto; }
  @media(max-width:620px) { .artist-banner { align-items:center; } }
</style>
