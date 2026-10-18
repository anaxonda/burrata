<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  let { call }: { call:<T=unknown>(command:string,args?:Record<string,unknown>)=>Promise<T> } = $props();
  let running=$state(false), localRequest=$state(false), checking=$state(false), message=$state("");
  let alive=true, statusRevision=0, repairRevision=0;
  onDestroy(()=>{alive=false;statusRevision++;repairRevision++;});
  onMount(()=>{void refresh();});
  async function refresh(){
    const request=++statusRevision;checking=true;
    try{const active=await call<boolean>("public_spotify_duration_repair_running");if(alive&&request===statusRevision)running=active;}
    catch(failure){if(alive&&request===statusRevision)message=String(failure);}
    finally{if(alive&&request===statusRevision)checking=false;}
  }
  async function repair(){
    const request=++repairRevision;statusRevision++;localRequest=true;running=true;checking=false;message="Checking missing Spotify durations…";
    try{
      const result=await call<{changed:number;processed:number;complete:boolean}>("repair_spotify_durations_public");
      if(alive&&request===repairRevision)message=`Checked ${result.processed} tracks; repaired ${result.changed} durations. ${result.complete?"Scan finished. Unavailable tracks may still have no duration.":"More tracks may remain. Run again to continue."}`;
    }catch(failure){if(alive&&request===repairRevision)message=/cancelled/i.test(String(failure))?"Repair stopped. Completed updates are kept.":String(failure);}
    finally{if(alive&&request===repairRevision){localRequest=false;running=false;void refresh();}}
  }
  async function cancel(){
    try{const accepted=await call<boolean>("cancel_public_spotify_duration_repair");if(alive)message=accepted?"Stopping repair…":"No repair is running.";}
    catch(failure){if(alive)message=String(failure);}
    finally{if(alive)void refresh();}
  }
</script>
<details class="duration-repair">
  <summary>Repair missing Spotify durations</summary>
  <p>Check public Spotify metadata for imported tracks with missing durations. No sign-in is needed. Each run checks up to 300 tracks for at most one minute; saved durations and manual edits are preserved.</p>
  <div class="actions">
    <button disabled={running||localRequest||checking} onclick={()=>void repair()}>Check missing durations</button>
    <button disabled={!running&&!localRequest} onclick={()=>void cancel()}>Stop repair</button>
    <button disabled={checking} onclick={()=>void refresh()}>Refresh status</button>
  </div>
  {#if running||localRequest}<p role="status">Duration repair is running.</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</details>
<style>
  .duration-repair{border-top:1px solid var(--line);padding-top:10px;min-width:0}
  summary{cursor:pointer;font-size:12px;font-weight:600}
  p{margin:7px 0;color:var(--muted);font-size:12px;line-height:1.45;overflow-wrap:anywhere}
  .actions{display:flex;flex-wrap:wrap;gap:6px}.actions button{min-height:30px;font-size:11px}
</style>
