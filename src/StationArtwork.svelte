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

  function imageSource(value: string | null | undefined) {
    if (!value || !/^http:\/\//i.test(value)) return value;
    try {
      const url = new URL(value);
      if (url.username || url.password || url.port) return value;
      url.protocol = "https:";
      return url.href;
    } catch {
      return value;
    }
  }

  const imageUrl = $derived(imageSource(src));
  let failed = $state(false);
  $effect(() => {
    src;
    failed = false;
  });
</script>

{#if imageUrl && !failed}
  <img class={imageClass} src={imageUrl} {alt} onerror={() => { failed = true; onImageError?.(); }} />
{:else}
  <div class={placeholderClass} role="img" aria-label={fallbackLabel}>{symbol}</div>
{/if}
