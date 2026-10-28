<script lang="ts">
  import { onDestroy } from "svelte";
  import { call } from "./lib/api";
  import type { Track } from "./lib/types";
  import type { LoudnessAnalysisJobStatus, LoudnessResourceIdentity } from "./lib/loudness-normalization";

  let { tracks, album, albumArtist, canReorder, light = false, focusReturn = null, onAnalyze, onPlay, onQueue, onClose }: {
    tracks: Track[]; album: string; albumArtist: string; canReorder: boolean; light?: boolean;
    focusReturn?: HTMLElement | null;
    onAnalyze: (orderedTracks: Track[]) => Promise<{ jobId: string; playbackTracks: Track[] }>;
    onPlay: (tracks: Track[], resource: LoudnessResourceIdentity) => void | Promise<void>;
    onQueue: (tracks: Track[], resource: LoudnessResourceIdentity) => void;
    onClose: () => void;
  } = $props();
  let orderedTracks = $state<Track[]>([]);
  let jobId = $state("");
  let jobState = $state<"idle" | "running" | "completed" | "failed" | "cancelled">("idle");
  let progress = $state("");
  let error = $state("");
  let resource = $state<LoudnessResourceIdentity | null>(null);
  let playbackTracks = $state<Track[]>([]);
  let generation = 0;
  $effect(() => {
    if (orderedTracks.length === 0 && tracks.length > 0) orderedTracks = [...tracks];
  });

  function move(index: number, offset: -1 | 1) {
    if (!canReorder || jobState === "running") return;
    const target = index + offset;
    if (target < 0 || target >= orderedTracks.length) return;
    const next = [...orderedTracks];
    [next[index], next[target]] = [next[target]!, next[index]!];
    orderedTracks = next; resource = null; playbackTracks = [];
    if (jobState === "completed") jobState = "idle";
  }
  async function analyze() {
    if (jobState === "running" || orderedTracks.length < 2) return;
    const request = ++generation;
    jobState = "running"; error = ""; progress = "Preparing the ordered local programme…";
    resource = null; playbackTracks = [];
    try {
      const prepared = await onAnalyze(orderedTracks);
      if (request !== generation) { void call("cancel_loudness_analysis", { jobId: prepared.jobId }).catch(() => {}); return; }
      jobId = prepared.jobId; playbackTracks = prepared.playbackTracks;
      while (request === generation) {
        const status = await call<LoudnessAnalysisJobStatus>("get_loudness_analysis_status", { jobId: prepared.jobId });
        if (request !== generation) return;
        progress = String(status.completed) + " of " + String(status.total) + " tracks analyzed";
        if (status.status === "completed") {
          const result = status.resource;
          if (!result || result.scope !== "album" || result.interTrackGapsSeconds.some(gap => gap !== 0)
            || result.members.length !== playbackTracks.length
            || result.members.some((member, index) => member.assetId !== playbackTracks[index]?.assetId
              || member.range.startSeconds !== 0 || member.range.endSeconds != null)) {
            throw new Error("Analysis completed without the exact ordered, gapless local album identity.");
          }
          resource = result; jobState = "completed";
          progress = "Album analyzed. Play or queue this order; its common gain applies when Album mode is selected in Playback settings.";
          jobId = "";
          return;
        }
        if (status.status === "failed") throw new Error(status.error || "Album loudness analysis failed.");
        if (status.status === "cancelled") { jobState = "cancelled"; progress = "Album analysis canceled."; jobId = ""; return; }
        await new Promise(resolve => window.setTimeout(resolve, 700));
      }
    } catch (failure) {
      if (request === generation) {
        jobState = "failed"; error = failure instanceof Error ? failure.message : String(failure);
        progress = ""; jobId = "";
      }
    }
  }
  async function cancel(): Promise<boolean> {
    const id = jobId;
    const request = generation;
    if (!id) {
      generation++;
      jobState = "cancelled";
      error = "";
      progress = "Analysis request canceled. If preparation completes, its native job will be canceled.";
      return true;
    }
    try {
      await call("cancel_loudness_analysis", { jobId: id });
    } catch (failure) {
      if (request !== generation || jobId !== id) return false;
      error = `Album analysis is still running because cancellation failed. Retry cancel or wait for it to finish. ${failure instanceof Error ? failure.message : String(failure)}`;
      return false;
    }
    if (request !== generation || jobId !== id) return false;
    generation++;
    jobId = "";
    jobState = "cancelled";
    error = "";
    progress = "Album analysis canceled.";
    return true;
  }
  async function close() { if (jobState === "running" && !(await cancel())) return; onClose(); }
  function activateDialog(node: HTMLDialogElement, closeDialog: () => void) {
    const returnFocus = focusReturn?.isConnected
      ? focusReturn
      : document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const onCancel = (event: Event) => { event.preventDefault(); closeDialog(); };
    node.addEventListener("cancel", onCancel);
    if (typeof node.showModal === "function") node.showModal();
    else node.setAttribute("open", "");
    node.focus();
    return {
      destroy() {
        node.removeEventListener("cancel", onCancel);
        if (node.open && typeof node.close === "function") node.close();
        if (returnFocus?.isConnected) returnFocus.focus();
      },
    };
  }
  async function play() {
    if (!resource) return;
    try { await onPlay(playbackTracks, resource); onClose(); }
    catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
  }
  function queue() {
    if (!resource) return;
    try { onQueue(playbackTracks, resource); onClose(); }
    catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
  }
  onDestroy(() => { generation++; if (jobId) void call("cancel_loudness_analysis", { jobId }).catch(() => {}); });
</script>

<div class="modal-backdrop local-album-loudness-backdrop">
  <dialog class="download-dialog local-album-loudness" class:light use:activateDialog={() => void close()} aria-labelledby="local-album-loudness-title" aria-modal="true" tabindex="-1">
    <header>
      <p class="eyebrow">LOCAL ALBUM PROGRAMME</p>
      <h2 id="local-album-loudness-title">{album}</h2>
      <p>{albumArtist} · {orderedTracks.length} downloaded tracks</p>
      <p>Review the exact order. Analysis uses zero added inter-track silence and changes neither audio files nor recording metadata.</p>
    </header>
    {#if canReorder}<p class="order-note">Track numbers are incomplete or repeated, so the current selection order is the starting order. Move tracks to set the programme order.</p>
    {:else}<p class="order-note">Ordered by the selected tracks’ complete track-number metadata. The measured order is shown below.</p>{/if}
    <ol class="programme-order" aria-label="Album analysis order">
      {#each orderedTracks as track, index (track.assetId ?? track.id)}
        <li>
          <span class="order-index">{String(index + 1).padStart(2, "0")}</span>
          <span class="track-copy"><strong>{track.title}</strong><small>{track.artist}{#if track.discNumber != null || track.trackNumber != null} · {track.discNumber ?? 1}-{track.trackNumber ?? "?"}{/if}</small></span>
          {#if canReorder}<span class="order-buttons"><button type="button" aria-label={"Move " + track.title + " up"} disabled={index === 0 || jobState === "running"} onclick={() => move(index, -1)}>↑</button><button type="button" aria-label={"Move " + track.title + " down"} disabled={index === orderedTracks.length - 1 || jobState === "running"} onclick={() => move(index, 1)}>↓</button></span>{/if}
        </li>
      {/each}
    </ol>
    {#if jobState === "running"}<p role="status">{progress}</p><progress max={orderedTracks.length} value={Number(progress.match(/^(\d+)/)?.[1] ?? 0)} aria-label="Album loudness analysis progress"></progress>{/if}
    {#if progress && jobState !== "running"}<p role="status">{progress}</p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    <footer>
      {#if jobState === "running"}<button type="button" onclick={() => void cancel()}>Cancel analysis</button>{/if}
      <button type="button" onclick={() => void close()}>{jobState === "running" ? "Close and cancel" : "Close"}</button>
      <button type="button" disabled={jobState === "running"} onclick={() => void analyze()}>{jobState === "completed" ? "Reanalyze album" : jobState === "failed" || jobState === "cancelled" ? "Retry analysis" : "Analyze album"}</button>
      {#if resource}<button type="button" onclick={queue}>Queue analyzed order</button><button type="button" class="primary" onclick={() => void play()}>Play analyzed order</button>{/if}
    </footer>
  </dialog>
</div>

<style>
  .local-album-loudness-backdrop { position:fixed; inset:0; z-index:120; display:grid; place-items:center; padding:16px; }
  .local-album-loudness { --surface:#191e1b; --raised:#202723; --line:#2c352f; --text:#e5ebe7; --muted:#94a399; --accent:#b8ec8c; color-scheme:dark; }
  .local-album-loudness.light { --surface:#fff; --raised:#f0f4ec; --line:#cbd3c5; --text:#203021; --muted:#5a6d59; --accent:#527e27; color-scheme:light; }
  .local-album-loudness::backdrop { background:#050806b8; backdrop-filter:blur(4px); }
  .local-album-loudness header { display:block; padding:0 0 14px; margin:0 0 16px; border-bottom:1px solid var(--line); }

  .local-album-loudness { width: min(620px, calc(100vw - 32px)); max-height: min(88vh, 900px); overflow: auto; }
  .local-album-loudness header h2 { margin: 0 0 4px; }
  .local-album-loudness header p { margin: 5px 0; color: var(--muted, #9da4b1); }
  .local-album-loudness .eyebrow { font-size: 11px; letter-spacing: .08em; color: var(--accent, #8eb8ff); }
  .order-note { font-size: 13px; color: var(--muted, #9da4b1); }
  .programme-order { display: grid; gap: 6px; padding: 0; margin: 14px 0; list-style: none; }
  .programme-order li { display: grid; grid-template-columns: 32px minmax(0,1fr) auto; align-items: center; gap: 10px; padding: 9px 10px; border: 1px solid var(--border, #303642); border-radius: 8px; }
  .order-index { color: var(--muted, #9da4b1); font-variant-numeric: tabular-nums; }
  .track-copy { display: grid; min-width: 0; gap: 3px; }
  .track-copy strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track-copy small { color: var(--muted, #9da4b1); }
  .order-buttons { display: flex; gap: 4px; }
  .order-buttons button { min-width: 32px; padding: 3px; }
  .local-album-loudness footer { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; margin-top: 18px; }
  .local-album-loudness progress { width: 100%; }
</style>
