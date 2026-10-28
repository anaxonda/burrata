<script lang="ts">
  import { onMount } from "svelte";
  import { call } from "./lib/api";

  type WorkKind = "metadata_retag_analysis" | "asset_autotag" | "background_source_preparation" | "background_stream_media_analysis";
  type Category = {
    kind: WorkKind;
    paused: boolean;
    pendingCount: number;
    writesAudioTags: boolean;
    queriesProviders: boolean;
  };
  type Policy = {
    reviewRequired: boolean;
    snapshotId: string | null;
    reviewedAt: string | null;
    warning: string | null;
    categories: Category[];
  };
  type Resume = {
    metadataRetagAnalysis: boolean;
    assetAutotag: boolean;
    backgroundSourcePreparation: boolean;
    backgroundStreamMediaAnalysis: boolean;
  };

  let policy = $state<Policy | null>(null);
  let resume = $state<Resume>({
    metadataRetagAnalysis: false,
    assetAutotag: false,
    backgroundSourcePreparation: false,
    backgroundStreamMediaAnalysis: false,
  });
  let loading = $state(true);
  let saving = $state(false);
  let error = $state("");
  let notice = $state("");
  let { onReviewed }: { onReviewed?: (policy: Policy) => void } = $props();

  const copy: Record<WorkKind, { title: string; explanation: string; resumeKey: keyof Resume }> = {
    metadata_retag_analysis: {
      title: "Automatic metadata re-analysis",
      explanation: "Reads local audio and may write BPM/key tags into audio files.",
      resumeKey: "metadataRetagAnalysis",
    },
    asset_autotag: {
      title: "Automatic audio tagging",
      explanation: "May query recognition and catalog providers, then write approved tags into audio files.",
      resumeKey: "assetAutotag",
    },
    background_source_preparation: {
      title: "Saved radio source preparation",
      explanation: "Queries configured providers and caches playable source matches; it does not write audio tags.",
      resumeKey: "backgroundSourcePreparation",
    },
    background_stream_media_analysis: {
      title: "Full streamed-track analysis on playback",
      explanation: "Fetches the exact playing source for one bounded BPM/key/waveform analysis; it does not save audio or change library metadata.",
      resumeKey: "backgroundStreamMediaAnalysis",
    },
  };

  async function load() {
    loading = true;
    error = "";
    try {
      policy = await call<Policy>("get_restore_work_policy");
      for (const category of policy.categories) resume[copy[category.kind].resumeKey] = !category.paused;
      onReviewed?.(policy);
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      loading = false;
    }
  }

  async function saveReview() {
    if (saving || !policy) return;
    saving = true; error = ""; notice = "";
    try {
      policy = await call<Policy>("review_restored_automatic_work", { resume });
      onReviewed?.(policy);
      notice = Object.values(resume).some(Boolean)
        ? "Your selected automatic work was resumed. Unselected categories remain paused."
        : "Review saved. Automatic work remains paused.";
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
      await load();
    } finally {
      saving = false;
    }
  }

  onMount(() => { void load(); });
</script>

{#if policy?.snapshotId || policy?.reviewRequired || loading || error}
  <section class="restore-work-review" aria-labelledby="restore-work-review-title">
    <header>
      <p class="eyebrow">RESTORE REVIEW</p>
      <h2 id="restore-work-review-title">{policy?.reviewRequired ? "Automatic work is paused" : "Automatic work review"}</h2>
      {#if policy?.reviewRequired}
        <p>A restored profile can contain queued work that changes audio files or contacts providers. Review each category and explicitly select what may resume.</p>
      {:else}
        <p>Review which automatic work may run for this restored profile. Unselected categories stay paused.</p>
      {/if}
    </header>
    {#if loading}<p role="status">Loading pending work…</p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if policy}
      {#if policy.warning}<p role="alert">{policy.warning}</p>{/if}
      {#if policy.snapshotId}<p class="snapshot">Restored backup: {policy.snapshotId}</p>{/if}
      <fieldset disabled={loading || saving}>
        <legend>Choose work to resume</legend>
        {#each policy.categories as category (category.kind)}
          {@const item = copy[category.kind]}
          <label class="work-category">
            <input type="checkbox" bind:checked={resume[item.resumeKey]} />
            <span class="work-copy">
              <strong>{item.title}</strong>
              <small>{item.explanation}</small>
            </span>
            <span class="work-count">{category.kind === "background_stream_media_analysis" ? "On playback" : `${category.pendingCount} pending`}</span>
          </label>
        {/each}
      </fieldset>
      {#if notice}<p role="status">{notice}</p>{/if}
      <footer>
        <button type="button" disabled={loading || saving} onclick={() => void saveReview()}>
          {saving ? "Saving review…" : "Save review choices"}
        </button>
      </footer>
    {/if}
  </section>
{/if}

<style>
  .restore-work-review { margin: 18px 0; padding: 18px; border: 1px solid var(--line); border-radius: 12px; background: var(--surface); }
  .restore-work-review header h2 { margin: 0 0 6px; }
  .restore-work-review header p { margin: 6px 0 12px; color: var(--muted); }
  .restore-work-review .eyebrow { margin: 0 0 5px; font-size: 11px; letter-spacing: .08em; color: var(--accent); }
  .restore-work-review .snapshot { font-size: 12px; color: var(--muted); overflow-wrap: anywhere; }
  .restore-work-review fieldset { display: grid; gap: 8px; border: 0; border-top: 1px solid var(--line); padding: 12px 0 0; }
  .restore-work-review legend { padding: 0 8px 0 0; font-size: 13px; font-weight: 650; }
  .work-category { display: grid; grid-template-columns: 18px minmax(0,1fr) auto; align-items: start; gap: 10px; padding: 10px; border: 1px solid var(--line); border-radius: 9px; cursor: pointer; }
  .work-category input { margin: 3px 0 0; }
  .work-copy { display: grid; gap: 4px; }
  .work-copy small { color: var(--muted); line-height: 1.4; }
  .work-count { color: var(--muted); font-size: 12px; white-space: nowrap; }
  .restore-work-review footer { display: flex; justify-content: flex-end; margin-top: 12px; }
  @media (max-width: 600px) { .work-category { grid-template-columns: 18px minmax(0,1fr); } .work-count { grid-column: 2; } }
</style>
