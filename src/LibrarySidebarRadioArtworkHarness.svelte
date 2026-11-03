<script lang="ts">
  import LibrarySidebar from "./LibrarySidebar.svelte";
  import type { RadioStation } from "./RadioView.svelte";

  let { initialStations, call }: {
    initialStations: unknown[];
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  } = $props();
  let stations = $state<RadioStation[]>([]);
  $effect(() => { stations = initialStations as RadioStation[]; });
  export function updateStations(next: unknown[]) { stations = next as RadioStation[]; }
</script>

<LibrarySidebar
  view="library"
  collections={[]}
  browserImports={[]}
  activeCollectionId=""
  {call}
  radioStations={stations}
  onNavigate={() => {}}
  onPlaylist={() => {}}
  onSource={() => {}}
  onImport={() => {}}
  onSettings={() => {}}
/>
