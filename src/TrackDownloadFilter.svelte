<script lang="ts">
  let {value=$bindable("all"),label="Downloaded status",onchange}:{onchange?:()=>void;value?:"all"|"downloaded"|"not-downloaded";label?:string}=$props();
</script>
<label class="download-filter">Files<select aria-label={label} bind:value {onchange}><option value="all">All tracks</option><option value="downloaded">Downloaded</option><option value="not-downloaded">Not downloaded</option></select></label>
<style>.download-filter{display:inline-flex;flex-direction:row;flex-wrap:wrap;align-items:center;gap:.4rem;margin:0;min-width:0;font-size:.8rem;color:var(--muted,#94a399)}select{min-width:0;max-width:min(180px,100%);padding:.4rem;border:1px solid var(--line,#454a50);border-radius:6px;background:var(--surface,#202723);color:var(--text,#e5ebe7)}</style>
