<script lang="ts">
  let { filled = false }: { filled?: boolean } = $props();
</script>

<svg aria-hidden="true" width="1em" height="1em" viewBox="0 0 24 24" fill={filled ? "currentColor" : "none"} stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" style:color={filled ? "#f1768d" : undefined}>
  <path d="M20.8 8.7c0 5-8.8 10-8.8 10s-8.8-5-8.8-10a4.5 4.5 0 0 1 8.8-1.2 4.5 4.5 0 0 1 8.8 1.2Z" />
</svg>

<style>
  svg { display:inline-block; flex:none; vertical-align:middle; }
</style>
