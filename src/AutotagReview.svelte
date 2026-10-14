<script lang="ts">
  import type { LocalAsset, Track } from "./lib/types";

  export type AutotagCandidate = {
    provider: string;
    providerId: string;
    sourceUrl?: string | null;
    title?: string | null;
    artist?: string | null;
    durationSeconds?: number | null;
    score?: number | null;
    fields: Partial<Track>;
    evidence: string[];
  };
  export type AutotagJob = {
    assetId: string;
    inputHash?: string;
    version?: string;
    status: string;
    attempts?: number;
    nextAttemptAt?: string | null;
    message?: string | null;
    candidates: AutotagCandidate[];
    chosen?: AutotagCandidate | string | null;
    approvedFields?: string[];
    excerptOnly?: boolean;
    updatedAt?: string;
  };
  type FieldKey = "artist" | "title" | "album" | "albumArtist" | "releaseDate" | "isrc" | "genre" | "label" | "catalogNumber" | "trackNumber" | "trackTotal" | "discNumber" | "discTotal";
  const fields: { key: FieldKey; label: string }[] = [
    { key: "artist", label: "Artist" },
    { key: "title", label: "Title" },
    { key: "album", label: "Album" }, { key: "albumArtist", label: "Album artist" },
    { key: "releaseDate", label: "Release date" }, { key: "isrc", label: "ISRC" },
    { key: "genre", label: "Genre" }, { key: "label", label: "Label" }, { key: "catalogNumber", label: "Catalog number" },
    { key: "trackNumber", label: "Track number" }, { key: "trackTotal", label: "Track total" }, { key: "discNumber", label: "Disc number" }, { key: "discTotal", label: "Disc total" },
  ];

  let {
    job,
    asset,
    onReview,
    onApply,
    onRetry,
    busy = false,
    error = "",
  }: {
    job: AutotagJob;
    asset: LocalAsset;
    onReview: (assetId: string, providerId: string) => Promise<AutotagJob | void> | void;
    onApply: (assetId: string, fieldKeys: FieldKey[]) => Promise<unknown> | unknown;
    onRetry?: (assetId: string) => Promise<unknown> | unknown;
    busy?: boolean;
    error?: string;
  } = $props();

  let selectedProviderId = $state("");
  let selectedFields = $state<FieldKey[]>([]);
  let localJob = $state<AutotagJob | null>(null);
  let reviewing = $state(false);
  let applying = $state(false);
  const currentJob = $derived(localJob ?? job);
  const candidates = $derived(currentJob.candidates ?? []);
  const chosenId = $derived(typeof currentJob.chosen === "string" ? currentJob.chosen : currentJob.chosen?.providerId ?? "");
  const activeProviderId = $derived(selectedProviderId || chosenId || candidates[0]?.providerId || "");
  const candidate = $derived(candidates.find((item) => item.providerId === activeProviderId));
  const permittedFields = $derived(currentJob.excerptOnly ? fields.filter(({ key }) => ["artist", "title", "genre"].includes(key)) : fields);
  const diffs = $derived(permittedFields.filter(({ key }) => {
    const value = candidate?.fields?.[key];
    return value != null && String(value).trim() !== "" && String(value).trim() !== String(asset.track[key] ?? "").trim();
  }));
  const differenceValue = (key: FieldKey, candidateValue: unknown) => {
    const currentValue = asset.track[key] || "Not set";
    return { before: currentValue, after: typeof candidateValue === "string" ? candidateValue : "Not provided" };
  };

  $effect(() => {
    const id = chosenId || candidates[0]?.providerId || "";
    if (!selectedProviderId || !candidates.some((item) => item.providerId === selectedProviderId)) {
      selectedProviderId = id;
      selectedFields = [];
    }
  });

  async function selectCandidate(providerId: string) {
    selectedProviderId = providerId;
    selectedFields = [];
    reviewing = true;
    try {
      const result = await onReview(currentJob.assetId, providerId);
      if (result) localJob = result;
    } finally {
      reviewing = false;
    }
  }
  function toggleField(key: FieldKey, checked: boolean) {
    selectedFields = checked ? [...selectedFields, key] : selectedFields.filter((item) => item !== key);
  }
  async function apply() {
    if (!selectedFields.length) return;
    applying = true;
    try { await onApply(currentJob.assetId, selectedFields); }
    finally { applying = false; }
  }
</script>

<article class="autotag-review" aria-label={`Metadata review for ${asset.track.title}`}>
  <div class="review-topline">
    <span class="status-dot" class:working={currentJob.status === "queued" || currentJob.status === "running"} class:failed={currentJob.status === "failed"} aria-hidden="true"></span>
    <span class="status-label">{currentJob.status === "needs_review" ? "Review match" : currentJob.status.replaceAll("_", " ")}</span>
    {#if onRetry && (currentJob.status === "failed" || currentJob.status === "needs_review")}
      <button class="retry" disabled={busy || reviewing} onclick={() => onRetry?.(currentJob.assetId)}>{currentJob.status === "failed" ? "Try again" : "Find another match"}</button>
    {/if}
  </div>
  <div class="asset-heading"><strong>{asset.track.title || "Untitled audio"}</strong><span>{asset.track.artist || "Artist unknown"}</span></div>
  {#if currentJob.message}<p class="job-message">{currentJob.message}</p>{/if}

  {#if candidates.length}
    <label class="candidate-select">Match to review
      <select aria-label="Choose an automatic metadata match" value={activeProviderId} disabled={busy || reviewing} onchange={(event) => void selectCandidate(event.currentTarget.value)}>
        {#each candidates as item (item.providerId)}
          <option value={item.providerId}>{item.provider}{item.title ? ` · ${item.title}` : ""}{item.score != null ? ` · ${Math.round(item.score)}%` : ""}</option>
        {/each}
      </select>
    </label>
    {#if candidate}
      <div class="match-summary">
        <div><strong>{candidate.fields.artist || candidate.artist || "Artist unknown"}</strong><span>{candidate.fields.title || candidate.title || "Untitled"}</span></div>
        <span class="provider">{candidate.provider}</span>
      </div>
      {#if candidate.evidence?.length}<ul class="evidence">{#each candidate.evidence as item}<li>{item}</li>{/each}</ul>{/if}
      {#if diffs.length}
        <div class="field-review">
          <div class="field-heading"><strong>Approve metadata fields</strong><span>Only selected fields will be applied</span></div>
          {#each diffs as field (field.key)}
            {@const value = differenceValue(field.key, candidate.fields[field.key])}
            <label class="field-diff">
              <input type="checkbox" checked={selectedFields.includes(field.key)} disabled={busy || applying || reviewing} onchange={(event) => toggleField(field.key, event.currentTarget.checked)} />
              <span class="field-name">{field.label}</span>
              <span class="values"><span class="before" title={String(value.before)}>{value.before}</span><span class="arrow" aria-hidden="true">→</span><strong title={String(value.after)}>{value.after}</strong></span>
            </label>
          {/each}
          <div class="review-actions"><button class="primary" disabled={!selectedFields.length || busy || applying || reviewing} onclick={() => void apply()}>{applying ? "Applying…" : `Apply ${selectedFields.length || "selected"} field${selectedFields.length === 1 ? "" : "s"}`}</button></div>
        </div>
      {:else}
        <p class="no-differences">This match has no new supported metadata fields to apply.</p>
      {/if}
    {/if}
  {:else if currentJob.status === "queued" || currentJob.status === "running"}
    <p class="job-message">Searching for a metadata match…</p>
  {:else}
    <p class="no-differences">No match is available yet.</p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</article>

<style>
  .autotag-review { min-width:0; padding:12px; border:1px solid var(--line); border-radius:10px; background:color-mix(in srgb,var(--raised) 35%,var(--surface)); color:var(--text); }
  .review-topline { display:flex; align-items:center; gap:7px; min-height:22px; }
  .status-dot { flex:0 0 7px; width:7px; height:7px; border-radius:50%; background:var(--muted); }
  .status-dot.working { background:var(--accent); }.status-dot.failed { background:#e6a38f; }
  .status-label { color:var(--muted); font-size:10px; text-transform:capitalize; }
  .retry { min-height:27px; margin-left:auto; padding:4px 8px; font-size:10px; }
  .asset-heading { display:grid; gap:3px; margin-top:7px; }.asset-heading strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; }.asset-heading span,.job-message { color:var(--muted); font-size:11px; }
  .candidate-select { display:grid; gap:5px; margin-top:12px; color:var(--muted); font-size:10px; }.candidate-select select { width:100%; min-width:0; min-height:34px; padding:6px 8px; font-size:11px; }
  .match-summary { display:flex; align-items:center; justify-content:space-between; gap:9px; margin-top:10px; }.match-summary>div { display:grid; min-width:0; gap:2px; }.match-summary strong,.match-summary>div>span { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.match-summary strong { font-size:12px; }.match-summary>div>span,.provider { color:var(--muted); font-size:10px; }.provider { flex:0 0 auto; }
  .evidence { display:flex; flex-wrap:wrap; gap:5px; margin:8px 0; padding:0; list-style:none; }.evidence li { padding:4px 7px; border-radius:6px; background:var(--raised); color:var(--muted); font-size:9px; }
  .field-review { margin-top:10px; border-top:1px solid var(--line); }.field-heading { display:grid; gap:3px; padding:9px 0 5px; }.field-heading strong { font-size:11px; }.field-heading span { color:var(--muted); font-size:9px; }
  .field-diff { display:grid; grid-template-columns:16px 72px minmax(0,1fr); align-items:center; gap:7px; min-height:39px; border-top:1px solid color-mix(in srgb,var(--line) 65%,transparent); cursor:pointer; }.field-diff input { width:15px; height:15px; margin:0; accent-color:var(--accent); }.field-name { color:var(--muted); font-size:10px; }.values { display:flex; align-items:center; gap:6px; min-width:0; font-size:10px; }.values .before,.values strong { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.values .before { color:var(--muted); }.values strong { color:var(--text); font-weight:600; }.arrow { flex:0 0 auto; color:var(--accent); }
  .review-actions { display:flex; justify-content:flex-end; padding-top:9px; }.review-actions button { min-height:32px; padding:6px 10px; font-size:10px; }
  .no-differences { margin:11px 0 0; color:var(--muted); font-size:10px; line-height:1.45; }.error { color:#e6a38f; font-size:10px; }
  @media(max-width:420px) { .field-diff { grid-template-columns:16px 62px minmax(0,1fr); gap:5px; }.values { gap:4px; font-size:9px; } }
</style>
