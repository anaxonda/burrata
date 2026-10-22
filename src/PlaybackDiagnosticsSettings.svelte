<script lang="ts">
  import { onMount } from "svelte";

  export type PlaybackDiagnosticsStatus = { enabled: boolean; eventCount: number; maxEvents: number };
  type ExportResult = { cancelled: boolean; path?: string; eventCount?: number };
  type NativeCall = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  let { enabled, onEnabledChange, call }: {
    enabled: boolean;
    onEnabledChange: (enabled: boolean) => void | Promise<void>;
    call: NativeCall;
  } = $props();

  let status = $state<PlaybackDiagnosticsStatus | null>(null);
  let loading = $state(false);
  let working = $state(false);
  let error = $state("");
  let message = $state("");
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
      await refresh();
      message = "Playback diagnostic events cleared.";
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    finally { working = false; }
  }
  async function exportReport() {
    working = true; error = ""; message = "";
    try {
      const result = await call<ExportResult>("export_playback_diagnostics");
      if (result?.cancelled) message = "Export cancelled.";
      else if (result?.path) message = `Exported ${result.eventCount ?? status?.eventCount ?? 0} playback diagnostic events to ${result.path}. No report was uploaded.`;
      else message = "Playback diagnostics exported.";
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    finally { working = false; }
  }
</script>

<section class="playback-diagnostics" aria-labelledby="playback-diagnostics-title">
  <h3 id="playback-diagnostics-title">Playback diagnostics</h3>
  <p>Optional playback logging records player stages, timing, and source failure categories. Reports include timestamps and hashed track/source references; they exclude credentials and full source URLs. Events are kept in memory; export the report before closing Burrata.</p>
  <label class="diagnostics-toggle">
    <input type="checkbox" aria-label="Enable playback diagnostics" checked={enabled} disabled={working} onchange={event => void toggle(event.currentTarget.checked)} />
    <span>Enable playback diagnostics</span>
  </label>
  {#if loading}<p role="status">Loading diagnostics status…</p>
  {:else if status}<p class="diagnostics-count" role="status">{status.eventCount} of {status.maxEvents} diagnostic events stored · {status.enabled ? "logging enabled" : "logging disabled"}</p>
  {:else}<p class="diagnostics-count">Diagnostics status is unavailable.</p>{/if}
  <div class="diagnostics-actions">
    <button type="button" disabled={working || loading} onclick={() => void refresh()}>Refresh status</button>
    <button type="button" disabled={working || !status?.eventCount} onclick={() => void clearEvents()}>Clear diagnostic events</button>
    <button type="button" disabled={working} onclick={() => void exportReport()}>{working ? "Working…" : "Export report…"} </button>
  </div>
  {#if message}<p role="status" class="diagnostics-message">{message}</p>{/if}
  {#if error}<p role="alert" class="diagnostics-error">{error}</p>{/if}
</section>

<style>
  .playback-diagnostics{display:grid;gap:.55rem;padding:.85rem 0;border-top:1px solid var(--line,#454a50)}
  h3{margin:0;font-size:1rem}p{margin:0;color:var(--muted,#a3a8ae);font-size:.82rem;line-height:1.45}
  .diagnostics-toggle{display:flex;align-items:center;gap:.55rem;color:var(--text,#eee);font-size:.86rem}
  .diagnostics-count{font-size:.78rem}.diagnostics-actions{display:flex;gap:.45rem;flex-wrap:wrap}
  button{border:1px solid var(--line,#454a50);border-radius:7px;padding:.4rem .6rem;color:inherit;background:#ffffff0a;cursor:pointer}
  button:disabled{opacity:.55;cursor:not-allowed}.diagnostics-message{color:var(--success,#75d49a)}.diagnostics-error{color:var(--danger,#ff8c8c)}
</style>
