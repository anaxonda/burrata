<script lang="ts">
  export type AutotagDependency = {
    name: string;
    available: boolean;
    version?: string | null;
    message?: string;
  };

  let {
    enabled = true,
    shazamFallback = true,
    dependencies = [],
    fpcalcAvailable,
    songrecAvailable,
    acoustIdConfigured = false,
    busy = false,
    error = "",
    onChange,
    onChanged,
    onShazamChange,
    onSave,
    onConfigureAcoustId,
  }: {
    enabled?: boolean;
    shazamFallback?: boolean;
    dependencies?: AutotagDependency[];
    fpcalcAvailable?: boolean;
    songrecAvailable?: boolean;
    acoustIdConfigured?: boolean;
    busy?: boolean;
    error?: string;
    onChange?: (enabled: boolean) => void;
    onChanged?: (settings: { enabled: boolean; shazamFallback: boolean }) => void;
    onShazamChange?: (enabled: boolean) => void;
    onSave?: () => void;
    onConfigureAcoustId?: () => void;
  } = $props();
  const toolStatuses = $derived(dependencies.length ? dependencies : [
    ...(fpcalcAvailable === undefined ? [] : [{ name: "fpcalc", available: fpcalcAvailable }]),
    ...(songrecAvailable === undefined ? [] : [{ name: "songrec", available: songrecAvailable }]),
  ]);
  function updateEnabled(value: boolean) { onChange?.(value); onChanged?.({ enabled: value, shazamFallback }); }
  function updateFallback(value: boolean) { onShazamChange?.(value); onChanged?.({ enabled, shazamFallback: value }); }
</script>

<section class="autotag-settings settings-card" aria-labelledby="autotag-title">
  <div class="section-heading">
    <div><p class="eyebrow">LIBRARY METADATA</p><h3 id="autotag-title">Automatic tagging</h3></div>
    <span class="status" class:ready={toolStatuses.length === 0 || toolStatuses.every((item) => item.available)}>
      {toolStatuses.length === 0 || toolStatuses.every((item) => item.available) ? "Ready" : "Needs setup"}
    </span>
  </div>
  <p class="intro">When a new download finishes, Burrata can look up its artist, title, and recording date. You review each match and approve the fields before they are written.</p>

  <label class="check-row">
    <span><strong>Tag new downloads</strong><small>Check completed downloads for missing or incorrect metadata.</small></span>
    <input type="checkbox" checked={enabled} disabled={busy} onchange={(event) => updateEnabled(event.currentTarget.checked)} />
  </label>
  <label class="check-row subordinate">
    <span><strong>Use Shazam as a fallback</strong><small>Try an audio fingerprint when text matching cannot find a confident result.</small></span>
    <input type="checkbox" checked={shazamFallback} disabled={!enabled || busy || (!onShazamChange && !onChanged)} onchange={(event) => updateFallback(event.currentTarget.checked)} />
  </label>

  <div class="credential-row">
    <div><strong>AcoustID</strong><small>{acoustIdConfigured ? "Connection configured" : "Optional audio fingerprint lookup"}</small></div>
    <button type="button" onclick={onConfigureAcoustId} disabled={!onConfigureAcoustId}>{acoustIdConfigured ? "Manage connection" : "Set up connection"}</button>
  </div>

  {#if toolStatuses.length}
    <details class="dependencies">
      <summary>Fingerprint tools</summary>
      <ul>
        {#each toolStatuses as dependency (dependency.name)}
          <li class:missing={!dependency.available}>
            <span class="indicator" aria-hidden="true">{dependency.available ? "✓" : "!"}</span>
            <span><strong>{dependency.name}</strong>{#if dependency.version}<small>{dependency.version}</small>{:else if dependency.message}<small>{dependency.message}</small>{/if}</span>
            <span class="dependency-status">{dependency.available ? "Available" : "Not found"}</span>
          </li>
        {/each}
      </ul>
    </details>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if onSave}<div class="actions"><button class="primary" type="button" disabled={busy} onclick={onSave}>{busy ? "Saving…" : "Save tagging settings"}</button></div>{/if}
</section>

<style>
  .autotag-settings { min-width:0; }
  .section-heading { display:flex; align-items:center; justify-content:space-between; gap:12px; }
  .eyebrow { margin:0 0 4px; color:var(--muted); font-size:9px; font-weight:650; letter-spacing:.12em; }
  h3 { margin:0; font-size:16px; }
  .status { flex:0 0 auto; padding:5px 8px; border-radius:999px; background:var(--raised); color:var(--muted); font-size:10px; }
  .status.ready { color:var(--accent); }
  .intro { max-width:70ch; margin:10px 0 16px; color:var(--muted); font-size:12px; line-height:1.55; }
  .check-row { display:flex; align-items:center; justify-content:space-between; gap:18px; min-height:57px; padding:10px 0; border-top:1px solid var(--line); cursor:pointer; }
  .check-row span,.credential-row>div { display:grid; gap:4px; min-width:0; }
  .check-row strong,.credential-row strong { color:var(--text); font-size:12px; font-weight:600; }
  .check-row small,.credential-row small { color:var(--muted); font-size:11px; line-height:1.4; }
  .check-row input { flex:0 0 auto; width:17px; height:17px; accent-color:var(--accent); }
  .subordinate { padding-left:12px; }
  .credential-row { display:flex; align-items:center; justify-content:space-between; gap:12px; padding:12px 0; border-top:1px solid var(--line); }
  .credential-row button { min-height:34px; padding:7px 10px; white-space:nowrap; font-size:11px; }
  .dependencies { border-top:1px solid var(--line); padding:12px 0 0; color:var(--muted); }
  .dependencies summary { cursor:pointer; font-size:11px; }
  .dependencies ul { display:grid; gap:10px; margin:12px 0 0; padding:0; list-style:none; }
  .dependencies li { display:grid; grid-template-columns:18px minmax(0,1fr) auto; align-items:center; gap:8px; font-size:11px; }
  .dependencies li>span:nth-child(2) { display:grid; gap:2px; min-width:0; }
  .dependencies li strong { color:var(--text); font-weight:550; }
  .dependencies li small { color:var(--muted); }
  .indicator { color:var(--accent); font-size:13px; }.missing .indicator,.missing .dependency-status { color:#e6a38f; }
  .dependency-status { font-size:10px; }
  .error { color:#e6a38f; font-size:11px; }
  .actions { display:flex; justify-content:flex-end; margin-top:13px; }
  .actions button { min-height:36px; padding:8px 12px; }
  @media(max-width:500px) { .credential-row { align-items:flex-start; flex-direction:column; }.credential-row button { width:100%; }.check-row { gap:10px; } }
</style>
