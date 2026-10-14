<script lang="ts">
  import { onMount } from "svelte";
  import type { Track } from "./lib/types";
  import type { ArtistRef, MetadataFieldKey, MetadataReview as Review } from "./lib/artist-types";

  let {
    track,
    resolve,
    onApply,
    onClose,
  }: {
    track: Track;
    resolve: (track: Track, recording?: ArtistRef | null) => Promise<Review>;
    onApply: (track: Track, review: Review, fields: MetadataFieldKey[]) => void | Promise<void>;
    onClose: () => void;
  } = $props();

  let review = $state<Review | null>(null);
  let checked = $state<MetadataFieldKey[]>([]);
  let loading = $state(true);
  let error = $state("");
  let applying = $state(false);

  onMount(() => { void load(); });
  async function load(recording?: ArtistRef | null) {
    loading = true; error = ""; checked = [];
    try { review = await resolve(track, recording); }
    catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
    finally { loading = false; }
  }
  function toggle(key: MetadataFieldKey, value: boolean) {
    checked = value ? [...new Set([...checked, key])] : checked.filter((item) => item !== key);
  }
  async function apply() {
    if (!review || !checked.length) return;
    applying = true;
    try { await onApply(track, review, checked); onClose(); }
    catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
    finally { applying = false; }
  }
</script>

<section class="metadata-review" aria-label="Review catalog metadata">
  <header><div><strong>Find catalog metadata</strong><small>Suggestions are unverified until you choose fields.</small></div>
    <button class="secondary" aria-label="Close metadata review" onclick={onClose}>Close</button>
  </header>
  {#if loading}<p role="status">Checking recording identities…</p>
  {:else if error}<p role="alert">{error}</p>
  {:else if review}
    <p>Current mix/version: <strong>{track.version || "No version tag"}</strong>. Keep the selected recording version when applying catalog fields.</p>
    {#if review.recordingCandidates.length}
      <fieldset><legend>Choose a recording identity</legend>
        {#each review.recordingCandidates as candidate (candidate.recording.id)}
          <label class="recording-choice"><input type="radio" name="recording-match" onchange={() => void load(candidate.recording)} />
            <span><strong>{candidate.title}</strong><small>{candidate.artist} · source score {candidate.score}{#if candidate.firstReleaseDate} · first release {candidate.firstReleaseDate}{/if}</small></span>
          </label>
        {/each}
      </fieldset>
    {/if}
    {#if review.recordingFirstReleaseDate}<p>Recording first-release date: {review.recordingFirstReleaseDate}</p>{/if}
    {#if review.releaseYear}<p>Selected edition year: {review.releaseYear}</p>{/if}
    {#if review.selectedVersionPreserved}<p>Version preserved: {review.selectedVersionPreserved}</p>{/if}
    {#each review.warnings as warning}<p class="warning">{warning}</p>{/each}
    {#if review.fields.length}
      <fieldset><legend>Fields to apply</legend>
        {#each review.fields as field (field.key)}
          <label class="field-choice"><input type="checkbox" checked={checked.includes(field.key)} onchange={(event) => toggle(field.key, event.currentTarget.checked)} />
            <span><strong>{field.key}: {field.value ?? "(empty)"}</strong><small><a href={field.sourceUrl} target="_blank" rel="noreferrer">{field.source} source ↗</a>{#if field.note} · {field.note}{/if}</small></span>
          </label>
        {/each}
      </fieldset>
    {:else if !review.recordingCandidates.length}<p>No matching catalog fields were found.</p>{/if}
    {#if review.credits.length}<p>Credits: {review.credits.map((credit) => `${credit.name} — ${credit.role}`).join(" · ")}</p>{/if}
    <button class="primary" disabled={applying || !checked.length} onclick={() => void apply()}>{applying ? "Saving…" : "Apply selected fields"}</button>
  {/if}
</section>

<style>
  .metadata-review{display:grid;gap:10px;margin:12px 0;padding:12px;border:1px solid var(--line);border-radius:9px;background:var(--base);font-size:12px}
  header{display:flex;justify-content:space-between;align-items:flex-start;gap:10px}
  header div, label span{display:grid;gap:4px;min-width:0}
  small{display:block;color:var(--muted);font-size:10px;line-height:1.45}
  p{margin:0;color:var(--muted);line-height:1.5}
  fieldset{display:grid;gap:6px;min-width:0;margin:0;padding:8px;border:1px solid var(--line);border-radius:7px}
  legend{padding:0 4px;font-size:11px;font-weight:650}
  label{display:flex;align-items:flex-start;gap:8px;margin:0;padding:7px;border-radius:6px;background:var(--surface)}
  input{flex:none;accent-color:var(--accent)}
  .warning{color:#dda984}
  a{color:var(--accent)}
</style>
