<script lang="ts">
  import { onMount } from "svelte";
  import { call, desktop } from "./lib/api";
  type Engine = "webview" | "native";
  type State = {selected:Engine;active:Engine;locked:boolean;restartRequired:boolean};
  let engineSettings=$state<State|null>(null), selected=$state<Engine>("webview"), busy=$state(false), message=$state("");
  function apply(value:State) {
    if(!value || !["webview","native"].includes(value.selected) || !["webview","native"].includes(value.active) || typeof value.locked!=="boolean")throw new Error("Invalid engine settings");
    engineSettings=value;selected=value.selected;
  }
  onMount(()=>{if(desktop())void call<State>("get_playback_engine_settings").then(apply).catch(()=>message="Audio playback settings are unavailable in this build.");});
  async function save() {
    if(!engineSettings || engineSettings.locked || busy)return;
    busy=true;message="";
    try{apply(await call<State>("save_playback_engine_settings",{engine:selected}));}
    catch{selected=engineSettings.selected;message="Could not save audio playback settings. Try again.";}
    finally{busy=false;}
  }
</script>
<section class="engine-settings" aria-label="Audio playback engine">
  <label>Audio playback<select aria-label="Audio playback engine" bind:value={selected} disabled={!engineSettings || engineSettings.locked || busy} onchange={()=>void save()}><option value="webview">Compatibility (default)</option><option value="native">Native audio · live BPM</option></select></label>
  <small>Native audio supports live BPM. Engine changes take effect after restarting Burrata.</small>
  {#if engineSettings?.locked}<small>This launch uses a fixed audio engine. Start Burrata normally to change it.</small>
  {:else if engineSettings?.restartRequired}<p role="status">Restart Burrata to use {selected==="native" ? "native audio" : "compatibility playback"}.</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</section>
<style>.engine-settings{display:grid;gap:.45rem;margin:1rem 0}.engine-settings label{display:grid;gap:.35rem;max-width:28rem}.engine-settings select{padding:.5rem;border:1px solid var(--line,#454a50);border-radius:7px;background:var(--surface,#202723);color:var(--text,#e5ebe7)}small{color:var(--muted,#94a399);line-height:1.5}p{font-size:.85rem;margin:0}</style>
