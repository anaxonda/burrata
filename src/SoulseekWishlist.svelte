<script lang="ts">
  import { onMount } from "svelte";
  import { call, desktop } from "./lib/api";
  import { SoulseekWishlist, type WishlistEntry } from "./lib/soulseek-wishlist";
  import { searchWishlist } from "./lib/soulseek-wishlist-search";
  let { visible=false, query="", startupStorage, onClose=()=>{}, onReview=(_query:string)=>{} }:{visible?:boolean;query?:string;startupStorage?:Pick<Storage,"getItem"|"setItem">;onClose?:()=>void;onReview?:(query:string)=>void}=$props();
  let dialog = $state<HTMLDialogElement>();
  $effect(()=>{ if (!dialog) return; if(visible && !dialog.open) dialog.showModal(); else if(!visible && dialog.open) dialog.close(); });
  let manager=$state.raw<SoulseekWishlist | null>(null);
  let entries=$state<WishlistEntry[]>([]), message=$state("");
  function act(action:()=>void) {try{action();message="";}catch{message="Could not save wishlist changes. Check available storage and try again.";}}
  onMount(()=>{
    if(!desktop())return;
    try{manager=new SoulseekWishlist(startupStorage??localStorage,Date.now,()=>{entries=[...manager!.entries];});entries=[...manager.entries];}
    catch{message="Wishlist storage is unavailable.";return;}
    let stopped=false;
    const check=()=>{if(!stopped)void manager!.tick((value,signal)=>searchWishlist(call,value,signal)).catch(()=>{message="Could not save wishlist status. Searches are waiting for storage to become available.";});};
    const timer=setInterval(check,10_000);check();
    return ()=>{stopped=true;clearInterval(timer);manager?.stop();};
  });
</script>
{#if desktop()}
<dialog bind:this={dialog} class="wishlist" aria-label="Saved Soulseek searches" onclose={onClose} oncancel={onClose}>
  <header><h2>Saved Soulseek searches</h2><button aria-label="Close saved searches" onclick={onClose}>×</button></header>
  <p>Save troublesome searches to check hourly while Burrata is open. Candidates still need review; searches do not download automatically.</p>
  <label>Search to watch<input aria-label="Soulseek wishlist query" type="text" maxlength="500" bind:value={query} placeholder="Artist and track or album" /></label>
  <button disabled={!query.trim() || !manager} onclick={()=>act(()=>manager!.add(query))}>Save search to wishlist</button>
  {#if message}<p role="status">{message}</p>{/if}
  {#each entries as entry (entry.id)}
    <div class="entry">
      <label><input type="checkbox" checked={entry.enabled} onchange={(event)=>act(()=>manager!.enable(entry.id,event.currentTarget.checked))} />{entry.query}</label>
      <small>{entry.outcome==="unchecked" ? "Not checked yet" : entry.outcome==="unavailable" ? "Search unavailable; will retry" : `${entry.resultCount ?? 0} candidates at last check`}{!entry.enabled ? " · Paused" : ""}</small>
      <button onclick={()=>onReview(entry.query)}>Search now</button>
      <button aria-label={`Remove wishlist search ${entry.query}`} onclick={()=>act(()=>manager!.remove(entry.id))}>Remove</button>
    </div>
  {/each}
</dialog>
{/if}
<style>
.wishlist{width:min(620px,85vw);max-height:75vh;overflow:auto;background:var(--panel,#18201b);color:var(--text);padding:1rem;border:1px solid var(--border,#454a50);border-radius:8px}.wishlist::backdrop{background:#0008}.wishlist header{display:flex;align-items:center;justify-content:space-between;gap:1rem}.wishlist h2{font-size:18px;margin:0}.wishlist p,.entry small{color:var(--muted,#a3a8ae);font-size:.82rem}.entry{display:flex;align-items:center;flex-wrap:wrap;gap:.5rem;margin-top:.6rem}.entry label{display:flex;align-items:center;gap:.4rem;flex:1;min-width:180px;overflow-wrap:anywhere}.entry input{width:auto;flex:none;margin-right:0}.entry small{flex:1;min-width:140px}
</style>
