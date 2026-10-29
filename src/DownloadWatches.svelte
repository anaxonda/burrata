<script lang="ts">
  import { onMount } from "svelte";
  import { call, listen, desktop } from "./lib/api";
  import type { Track } from "./lib/types";
  import InfoPopover from "./InfoPopover.svelte";
  type Item={id:string;kind:string;state:string;nextCheck:number;lastChecked?:number;jobId?:string;request:{track:Track;requiresLossless:boolean}};
  let {onReview,onCount=()=>{}}:{onReview:(track:Track)=>void;onCount?:(count:number)=>void}=$props();
  let items=$state<Item[]>([]),filter=$state("all"),message=$state("");
  const active=$derived(items.filter(item=>!["completed","cancelled","queued"].includes(item.state)));
  const visible=$derived(active.filter(item=>filter==="all" || (filter==="upgrade" ? item.kind==="upgrade" : item.state===filter)));
  const label=(item:Item)=>item.state==="paused"?"Paused":item.state==="review"?"Needs review":item.state==="checking"?"Checking Soulseek…":item.state==="queued"?"Source found · queued for download":item.kind==="upgrade"?"Downloaded · looking for lossless":"Waiting for source";
  let root=$state<HTMLElement>();
  function dismissMenus(event:MouseEvent){for(const menu of root?.querySelectorAll<HTMLDetailsElement>("details[open]")??[]){if(event.target instanceof Node && !menu.contains(event.target))menu.open=false;}}
  function closeMenus(){for(const menu of root?.querySelectorAll<HTMLDetailsElement>("details[open]")??[]){const focused=menu.contains(document.activeElement);menu.open=false;if(focused)menu.querySelector("summary")?.focus();}}
  function act(event:MouseEvent,action:()=>Promise<void>){const menu=(event.currentTarget as HTMLElement).closest("details");if(menu)menu.open=false;void action();}
  let revision=0;
  async function refresh(){const request=++revision;try{const result=await call<Item[]>("list_download_watches");if(request!==revision)return;items=result;onCount(result.filter(item=>!["completed","cancelled","queued"].includes(item.state)).length);}catch{message="Could not load waiting downloads.";}}
  async function control(item:Item,action:string){try{await call("control_download_watch",{id:item.id,action});message="";await refresh();}catch(error){message=String(error);}}
  async function choose(item:Item){try{if(item.state!=="paused" && item.state!=="review") await call("control_download_watch",{id:item.id,action:"pause"});await refresh();onReview(item.request.track);}catch(error){message=String(error);}}
  onMount(()=>{if(!desktop())return;let disposed=false;let stop:undefined|(()=>void);void listen("download-watches-changed",()=>void refresh()).then(fn=>{if(disposed)fn();else stop=fn;}).catch(()=>{message="Waiting download updates are unavailable. Reopen Downloads to refresh.";});void refresh();return()=>{disposed=true;revision++;stop?.();};});
</script>
<svelte:window onclick={dismissMenus} onkeydown={(event)=>{if(event.key==="Escape")closeMenus();}} />
{#if active.length || message}
<section bind:this={root} class="download-watches" aria-label="Waiting downloads and lossless checks">
  <header><h2>Waiting and upgrades <small>{active.length}</small></h2><InfoPopover label="About waiting downloads and lossless checks">These requests check Soulseek periodically while Burrata is open. Waiting does not use a transfer slot. Lossless checks keep your existing download playable. A source must meet the original recording and quality rules; uncertain results need review. Pausing or cancelling a check keeps any downloaded copy.</InfoPopover>
    <select aria-label="Filter waiting downloads" bind:value={filter}><option value="all">All requests</option><option value="waiting">Waiting for source</option><option value="upgrade">Lossless upgrades</option><option value="review">Needs review</option><option value="paused">Paused</option></select>
  </header>
  {#if message}<p role="status">{message}</p>{/if}
  {#each visible as item(item.id)}
    <article><div><strong>{item.request.track.title}</strong><p>{item.request.track.artist}</p><small>{label(item)}</small>{#if item.state==="waiting"}<small>Next check {new Date(item.nextCheck*1000).toLocaleString()}</small>{/if}</div>
      <details><summary aria-label={`Options for ${item.request.track.title}`}>•••</summary><div class="watch-menu">
        {#if item.state!=="queued"}<button onclick={(event)=>act(event,()=>control(item,item.state==="paused"?"resume":"check"))}>{item.state==="paused"?"Resume checks":"Check now"}</button>{/if}
        <button onclick={(event)=>act(event,()=>choose(item))}>Choose source…</button>
        {#if item.state!=="paused"}<button onclick={(event)=>act(event,()=>control(item,"pause"))}>Pause</button>{/if}
        <button onclick={(event)=>act(event,()=>control(item,"cancel"))}>Cancel request</button>
      </div></details>
    </article>
  {/each}
  {#if !visible.length}<p>No requests match this filter.</p>{/if}
</section>
{/if}
<style>
.download-watches{padding:16px 20px;border-bottom:1px solid var(--line)}header{display:flex;align-items:center;gap:10px;flex-wrap:wrap}h2{font-size:14px;margin:0}h2 small{display:inline;margin-left:6px;font-weight:400;color:var(--muted)}header select{margin-left:auto;width:auto;min-width:150px}article{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:12px 0;border-top:1px solid var(--line)}article:first-of-type{margin-top:12px}article>div{min-width:0}strong,p,small{overflow-wrap:anywhere}p{margin:4px 0;color:var(--muted);font-size:12px}small{display:block;color:var(--muted);font-size:11px}details{position:relative;align-self:flex-start}summary{cursor:pointer;list-style:none;padding:6px 10px;border-radius:6px}summary::-webkit-details-marker{display:none}.watch-menu{position:absolute;right:0;z-index:20;background:var(--panel,#18201b);border:1px solid var(--line);border-radius:8px;padding:5px;min-width:160px;box-shadow:0 6px 18px #0005}.watch-menu button{display:block;width:100%;text-align:left;border:0;background:transparent;padding:8px}.watch-menu button:hover{background:var(--line)}
</style>
