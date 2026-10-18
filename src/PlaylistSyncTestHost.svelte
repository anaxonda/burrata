<script lang="ts">
  import PlaylistSync from "./PlaylistSync.svelte";

  let { call }: {
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  } = $props();
  let bindingId = $state("binding-1");
  let enabled = $state(true);
</script>

<button class="switch-binding" onclick={() => bindingId = "binding-2"}>Switch source playlist</button>
<PlaylistSync {bindingId} {enabled} {call} />
