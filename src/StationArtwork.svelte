<script lang="ts">
  let {
    src,
    alt,
    fallbackLabel,
    imageClass,
    placeholderClass,
    symbol = "♫",
    onImageError,
  }: {
    src?: string | null;
    alt: string;
    fallbackLabel: string;
    imageClass: string;
    placeholderClass: string;
    symbol?: string;
    onImageError?: () => void;
  } = $props();

  let failed = $state(false);
  $effect(() => {
    src;
    failed = false;
  });
</script>

{#if src && !failed}
  <img class={imageClass} {src} {alt} onerror={() => { failed = true; onImageError?.(); }} />
{:else}
  <div class={placeholderClass} role="img" aria-label={fallbackLabel}>{symbol}</div>
{/if}
