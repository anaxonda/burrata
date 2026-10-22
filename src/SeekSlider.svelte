<script lang="ts">
  import { onDestroy } from "svelte";
  let { position, duration, disabled, onSeek }: {position:number;duration:number;disabled:boolean;onSeek:(value:number)=>void} = $props();
  let target = $state<number | null>(null);
  let dragging = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function commit() {
    clearTimeout(timer);
    if (target !== null) onSeek(target);
    target = null;
  }
  onDestroy(() => clearTimeout(timer));
</script>
<input class="seek" type="range" min="0" max={duration || 1} step="0.1"
  value={target ?? position} {disabled} aria-label="Playback position"
  onpointerdown={(event) => { clearTimeout(timer); dragging = true; event.currentTarget.setPointerCapture(event.pointerId); }}
  oninput={(event) => { target = Number(event.currentTarget.value); clearTimeout(timer); if (!dragging) timer = setTimeout(commit, 150); }}
  onpointerup={() => { dragging = false; commit(); }}
  onlostpointercapture={() => { if (dragging) { dragging = false; commit(); } }}
  onpointercancel={() => { dragging = false; clearTimeout(timer); target = null; }}
  onblur={() => { dragging = false; commit(); }} />
