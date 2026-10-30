<script lang="ts">
  import { onMount } from "svelte";
  import InfoPopover from "./InfoPopover.svelte";

  export type PlaybackDiagnosticsStatus = { enabled: boolean; eventCount: number; maxEvents: number };
  type ExportResult = { cancelled: boolean; path?: string; eventCount?: number };
  type NativeCall = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  let { enabled, onEnabledChange, call, getWebviewDiagnostics, onClearWebviewDiagnostics }: {
    enabled: boolean;
    onEnabledChange: (enabled: boolean) => void | Promise<void>;
    call: NativeCall;
    getWebviewDiagnostics?: () => unknown;
    onClearWebviewDiagnostics?: () => void;
  } = $props();

  let status = $state<PlaybackDiagnosticsStatus | null>(null);
  let loading = $state(false);
  let working = $state(false);
  let error = $state("");
  let message = $state("");
  let browserPlayback = $state("not_tested");
  let alternateNetworkPlayback = $state("not_tested");
  let vpn = $state("unknown");
  let requestEpoch = 0;
  onMount(() => { void refresh(); });

  async function refresh() {
    const epoch = ++requestEpoch;
    loading = true;
    error = "";
    try {
      const result = await call<PlaybackDiagnosticsStatus>("get_playback_diagnostics_status");
      if (epoch === requestEpoch) status = result;
    } catch (failure) {
      if (epoch === requestEpoch) error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      if (epoch === requestEpoch) loading = false;
    }
  }
  async function toggle(enabledNow: boolean) {
    working = true; error = ""; message = "";
    try {
      await onEnabledChange(enabledNow);
      await refresh();
      message = enabledNow ? "Playback diagnostics enabled." : "Playback diagnostics disabled.";
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    finally { working = false; }
  }
  async function clearEvents() {
    working = true; error = ""; message = "";
    try {
      await call("clear_playback_diagnostics");
      onClearWebviewDiagnostics?.();
      await refresh();
      message = "Playback diagnostic events cleared.";
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    finally { working = false; }
  }
  async function exportReport() {
    working = true; error = ""; message = "";
    try {
      const original = getWebviewDiagnostics?.() ?? null;
      const webviewContext = browserPlayback === "not_tested" && alternateNetworkPlayback === "not_tested" && vpn === "unknown"
        ? original : {...(original && typeof original === "object" ? original : {}), testerObservations:{browserPlayback,alternateNetworkPlayback,vpn}};
      const result = await call<ExportResult>("export_playback_diagnostics", {webviewContext});
      if (result?.cancelled) message = "Export cancelled.";
      else if (result?.path) message = `Exported ${result.eventCount ?? status?.eventCount ?? 0} playback diagnostic events to ${result.path}. No report was uploaded.`;
      else message = "Playback diagnostics exported.";
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    finally { working = false; }
  }
</script>

<section class="playback-diagnostics" aria-labelledby="playback-diagnostics-title">
  <div class="heading"><h2 id="playback-diagnostics-title">Playback reports</h2><InfoPopover label="About playback reports">Reports include timestamps, public source links, playback stages and redacted errors. They exclude credentials, signed stream URLs, private-link tokens, titles and user-agent strings. Events stay in memory until exported. Export saves a file; it does not upload a report.</InfoPopover></div>
  <p>Turn on recording, reproduce the playback problem, then export a report before closing Burrata.</p>
  <label class="diagnostics-toggle">
    <input type="checkbox" aria-label="Enable playback diagnostics" checked={enabled} disabled={working} onchange={event => void toggle(event.currentTarget.checked)} />
    <span>Record playback problems</span>
  </label>
  <div><button class="export" type="button" disabled={working} onclick={() => void exportReport()}>{working ? "Working…" : "Export report…"}</button></div>
  <details class="report-details"><summary>Report details</summary>
    {#if loading}<p role="status">Loading diagnostics status…</p>
    {:else if status}<p class="diagnostics-count" role="status">{status.eventCount} of {status.maxEvents} diagnostic events stored · {status.enabled ? "logging enabled" : "logging disabled"}</p>
    {:else}<p class="diagnostics-count">Diagnostics status is unavailable.</p>{/if}
    <div class="diagnostics-actions">
      <button type="button" disabled={working || loading} onclick={() => void refresh()}>Refresh status</button>
      <button type="button" disabled={working || !status?.eventCount} onclick={() => void clearEvents()}>Clear diagnostic events</button>
    </div>
    <details><summary>Optional reproduction details</summary>
      <p>For the same failing source, record comparisons you have tried. These choices are included in the exported report.</p>
      <label>Same source in browser<select aria-label="Same source in browser" bind:value={browserPlayback}><option value="not_tested">Not tested</option><option value="works">Plays</option><option value="fails">Fails</option></select></label>
      <label>Same source on another network<select aria-label="Same source on another network" bind:value={alternateNetworkPlayback}><option value="not_tested">Not tested</option><option value="works">Plays</option><option value="fails">Fails</option></select></label>
      <label>VPN during reproduction<select aria-label="VPN during reproduction" bind:value={vpn}><option value="unknown">Unknown</option><option value="enabled">Enabled</option><option value="disabled">Disabled</option></select></label>
    </details>
  </details>
  {#if message}<p role="status" class="diagnostics-message">{message}</p>{/if}
  {#if error}<p role="alert" class="diagnostics-error">{error}</p>{/if}
</section>

<style>
  .playback-diagnostics{display:grid;gap:.55rem;padding:15px 16px;border-top:1px solid var(--line,#454a50)}
  .heading{display:flex;align-items:center;gap:.4rem}.report-details{font-size:.82rem}.report-details > :not(summary){margin-top:.55rem}.report-details label{display:flex;flex-direction:column;align-items:stretch;gap:.35rem;margin:.55rem 0 0}
  h2{margin:0;font-size:1rem}p{margin:0;color:var(--muted,#a3a8ae);font-size:.82rem;line-height:1.45}
  .diagnostics-toggle{display:flex;flex-direction:row;align-items:center;gap:.55rem;margin:0;color:var(--text,#eee);font-size:.86rem}
  .playback-diagnostics .diagnostics-toggle input[type="checkbox"]{width:auto;flex:none;margin:0}
  .diagnostics-count{font-size:.78rem}.diagnostics-actions{display:flex;gap:.45rem;flex-wrap:wrap}
  button{border:1px solid var(--line,#454a50);border-radius:7px;padding:.4rem .6rem;color:inherit;background:#ffffff0a;cursor:pointer}
  button:disabled{opacity:.55;cursor:not-allowed}.diagnostics-message{color:var(--success,#75d49a)}.diagnostics-error{color:var(--danger,#ff8c8c)}
</style>
