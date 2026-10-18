<script lang="ts">
  let { name }: { name: "plus" | "heart" | "queue" | "play" | "radio" | "external" | "download" | "info" | "window" | "search" | "select" | "remove" | "edit" | "refresh" | "copy" | "export" | "arrow-up" | "arrow-down" } = $props();
</script>

<svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
  {#if name === "plus"}
    <path d="M12 5v14M5 12h14" />
  {:else if name === "heart"}
    <path d="M20.8 8.7c0 5-8.8 10-8.8 10s-8.8-5-8.8-10a4.5 4.5 0 0 1 8.8-1.2 4.5 4.5 0 0 1 8.8 1.2Z" />
  {:else if name === "queue"}
    <path d="M4 6h16M4 11h16M4 16h10" /><path d="m17 15 4 3-4 3z" />
  {:else if name === "play"}
    <path d="m8 5 11 7-11 7z" />
  {:else if name === "radio"}
    <circle cx="12" cy="12" r="2" /><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M4.9 4.9a10 10 0 0 0 0 14.2M19.1 4.9a10 10 0 0 1 0 14.2" />
  {:else if name === "external"}
    <path d="M14 4h6v6M20 4l-9 9" /><path d="M18 13v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h6" />
  {:else if name === "download"}
    <path d="M12 3v12m-5-5 5 5 5-5" /><path d="M5 17v3h14v-3" />
  {:else if name === "info"}
    <circle cx="12" cy="12" r="9" /><path d="M12 11v5m0-8h.01" />
  {:else if name === "window"}
    <rect x="4" y="5" width="16" height="14" rx="2" /><path d="M4 9h16M8 7h.01M11 7h.01" />
  {:else if name === "search"}
    <circle cx="10.8" cy="10.8" r="6.8" /><path d="m16 16 4 4" />
  {:else if name === "select"}
    <rect x="4" y="4" width="16" height="16" rx="2" /><path d="m8 12 2.5 2.5L16 9" />
  {:else if name === "remove"}
    <circle cx="12" cy="12" r="9" /><path d="M8 12h8" />
  {:else if name === "edit"}
    <path d="m4 16.5-.8 4.3 4.3-.8L19 8.5 15.5 5z" /><path d="m13.8 6.7 3.5 3.5" />
  {:else if name === "refresh"}
    <path d="M20 7v5h-5" /><path d="M19 12a7 7 0 1 0 1.1 4" />
  {:else if name === "copy"}
    <rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
  {:else if name === "export"}
    <path d="M12 15V3m-5 5 5-5 5 5" /><path d="M5 13v7h14v-7" />
  {:else if name === "arrow-up"}
    <path d="m6 14 6-6 6 6" />
  {:else if name === "arrow-down"}
    <path d="m6 10 6 6 6-6" />
  {/if}
</svg>
