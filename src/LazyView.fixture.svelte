<script lang="ts">
  let { title }: { title: string } = $props();
  let clicks = $state(0);
</script>

<p>{title}</p>
<button type="button" onclick={() => clicks += 1}>Child clicks: {clicks}</button>
