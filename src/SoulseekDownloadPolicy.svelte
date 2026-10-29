<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  let { unavailable = "continue", upgrade = false, inSourceOrder = false, onchange }: {
    unavailable?: "continue" | "wait"; upgrade?: boolean; inSourceOrder?: boolean;
    onchange: (unavailable: "continue" | "wait", upgrade: boolean) => void;
  } = $props();
</script>

<div class="soulseek-policy">
  <div class="policy-row">
    <label for="soulseek-unavailable">When Soulseek has no match</label>
    <InfoPopover label="About waiting for a Soulseek source">
      Sources before Soulseek are tried normally. Wait keeps the recording in Downloads and checks Soulseek again while Burrata is open; sources after Soulseek are held. Waiting uses no transfer slot. Other recordings continue downloading. Matching rules stay attached to the request. Checks use the slskd connection in Settings → Connections.
    </InfoPopover>
  </div>
  <select id="soulseek-unavailable" aria-label="When Soulseek has no match" value={unavailable} disabled={!inSourceOrder}
    onchange={(event) => onchange(event.currentTarget.value as "continue" | "wait", upgrade)}>
    <option value="continue">Continue to the next source</option>
    <option value="wait">Wait for Soulseek</option>
  </select>
  {#if !inSourceOrder}<small>Add Soulseek to the source order to enable waiting.</small>{/if}
  <div class="policy-row upgrade-row">
    <label class="check"><input type="checkbox" aria-label="Keep looking for a lossless copy" checked={upgrade}
      onchange={(event) => onchange(unavailable, event.currentTarget.checked)} />Keep looking for a lossless copy</label>
    <InfoPopover label="About lossless upgrade checks">
      After a permitted lossy download, check Soulseek for a lossless copy of the same recording, independently of source order. Your saved copy stays playable until a validated upgrade is ready. Uncertain matches need review. MP3 output and chapter downloads are excluded. Checks pause when Burrata closes; you can pause or cancel them in Downloads. Checks use your configured slskd connection.
    </InfoPopover>
  </div>
</div>

<style>
  .soulseek-policy { border-top:1px solid var(--line); padding-top:12px; margin-top:12px; }
  .policy-row { display:flex; align-items:center; justify-content:space-between; gap:10px; }
  .policy-row label { margin:0; font-size:12px; }
  select { width:100%; margin-top:7px; }
  small { display:block; color:var(--muted); font-size:11px; margin-top:5px; }
  .upgrade-row { margin-top:13px; }
  label.check { display:flex; flex-direction:row; align-items:center; gap:8px; }
  input[type=checkbox] { flex:none; width:auto; margin:0; }
</style>
