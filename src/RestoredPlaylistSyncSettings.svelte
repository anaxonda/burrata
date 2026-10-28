<script lang="ts">
  import { onMount } from "svelte";

  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  type Review = {
    reviewToken: string;
    currentMirrorDirectory: string | null;
    sourceCount: number;
    mirrorCount: number;
    protectedCopyCount: number;
    sectionMirrorRelinks?: { playlistName: string; sectionName: string; relativePath: string }[];
  };

  let { call, desktop = false }: { call: Call; desktop?: boolean } = $props();
  let review = $state<Review | null>(null);
  let acknowledged = $state(false);
  let busy = $state(false);
  let message = $state("");
  let unavailable = $state(false);

  async function refresh() {
    if (!desktop) return;
    try {
      review = await call<Review>("preview_restored_playlist_sync");
      unavailable = false;
      acknowledged = false;
      message = "";
    } catch (error) {
      review = null;
      // No restored projection is the ordinary state for profiles that were
      // not restored from an archive. Surface other failures for diagnosis.
      const reason = String(error).toLowerCase();
      unavailable = !reason.includes("no restored playlist projection")
        && !reason.includes("restored playlist synchronization is already active");
      message = unavailable ? String(error) : "";
    }
  }

  onMount(() => { void refresh(); });

  async function resume() {
    if (!review || !acknowledged || busy) return;
    busy = true;
    message = "";
    try {
      await call("resume_reviewed_restored_playlist_sync", { reviewToken: review.reviewToken });
      const status = await call<{ warnings?: string[] }>("sync_playlists");
      review = null;
      acknowledged = false;
      message = status.warnings?.length
        ? `Synchronization resumed. ${status.warnings.join(" ")}`
        : "Playlist files will update using this device’s current music folder.";
    } catch (error) {
      message = String(error);
      // A stale review token means the user must inspect the current root again.
      if (message.toLowerCase().includes("review it again")) {
        await refresh();
        if (review) message = "The folder review changed. Check the updated details before resuming.";
      }
    } finally {
      busy = false;
    }
  }
</script>

{#if desktop && review}
  <section class="restored-sync-review" aria-labelledby="restored-sync-review-title">
    <h3 id="restored-sync-review-title">Resume saving playlist files after restore</h3>
    <p>Saving playlist files is paused after restore. Review the current folder and saved files below before resuming. Restored playlists will save under this device’s current music folder.</p>
    <dl>
      <div><dt>Current music folder</dt><dd>{review.currentMirrorDirectory ?? "No music folder is configured"}</dd></div>
      <div><dt>Saved playlist sources</dt><dd>{review.sourceCount}</dd></div>
      <div><dt>Saved playlist files</dt><dd>{review.mirrorCount}</dd></div>
      <div><dt>Protected existing copies</dt><dd>{review.protectedCopyCount}</dd></div>
    </dl>
    {#if review.sectionMirrorRelinks?.length}
      <div class="section-relink-review" aria-label="Section files that match the backup">
        <h4>Section files that match your backup</h4>
        <p>These files match your backup and can resume updating with your playlists.</p>
        <ul>
          {#each review.sectionMirrorRelinks as relink}
            <li>
              <span class="section-relink-name">{relink.playlistName} · {relink.sectionName}</span>
              <code>{relink.relativePath}</code>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    <label class="review-confirm"><input type="checkbox" bind:checked={acknowledged} /> I reviewed this folder and want to resume saving playlist files.</label>
    <button type="button" disabled={busy || !acknowledged || !review.currentMirrorDirectory} onclick={() => void resume()}>{busy ? "Resuming…" : "Resume saving playlist files"}</button>
  </section>
{/if}
{#if unavailable && message}
  <p role="alert" class="restored-sync-message">Could not review restored playlist files: {message}</p>
{:else if message}
  <p role="status" class="restored-sync-message">{message}</p>
{/if}

<style>
  .restored-sync-review{display:grid;gap:10px;max-width:680px;padding:14px;border:1px solid var(--line);border-radius:9px;margin-top:12px}.restored-sync-review h3,.restored-sync-review p,.section-relink-review h4{margin:0}.restored-sync-review p,.restored-sync-review dt,.restored-sync-message{color:var(--muted);font-size:.88rem;line-height:1.45}.restored-sync-review dl{display:grid;gap:7px;margin:0}.restored-sync-review dl div{display:grid;grid-template-columns:minmax(145px,1fr) 2fr;gap:10px}.restored-sync-review dd{margin:0;overflow-wrap:anywhere}.section-relink-review{display:grid;gap:6px}.section-relink-review ul{display:grid;gap:6px;margin:0;padding-left:20px}.section-relink-review li{display:grid;gap:2px}.section-relink-name{font-weight:600}.section-relink-review code{overflow-wrap:anywhere;font-size:.82rem}.review-confirm{display:flex;align-items:flex-start;gap:8px}.restored-sync-review button{justify-self:start}
</style>
