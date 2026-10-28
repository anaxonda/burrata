<script lang="ts">
  import { onMount } from "svelte";

  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  type HistoryMirrorOptions = { schemaVersion: number; recentDays: number | null };

  let { call, desktop = false }: { call: Call; desktop?: boolean } = $props();
  let options = $state<HistoryMirrorOptions>({ schemaVersion: 1, recentDays: null });
  let scope = $state<"all" | "recent">("all");
  let recentDays = $state(30);
  let busy = $state(false);
  let message = $state("");

  onMount(() => {
    if (!desktop) return;
    void call<HistoryMirrorOptions>("get_history_mirror_options")
      .then((saved) => {
        options = saved;
        scope = saved.recentDays === null ? "all" : "recent";
        recentDays = saved.recentDays ?? 30;
      })
      .catch((error) => { message = String(error); });
  });

  async function save() {
    if (!desktop) return;
    busy = true;
    message = "";
    try {
      const requested = { schemaVersion: 1, recentDays: scope === "recent" ? recentDays : null };
      options = await call<HistoryMirrorOptions>("set_history_mirror_options", { options: requested });
      try {
        await call("sync_playlists");
      } catch (error) {
        message = `History options saved, but playlist sync failed: ${String(error)}`;
        return;
      }
      message = scope === "all"
        ? "History options saved: all retained history will be included when Listening history is mirrored. Oversized histories are partitioned by month."
        : `History options saved: the last ${recentDays} days will be included when Listening history is mirrored. This scope is identified in its playlist title and metadata.`;
    } catch (error) {
      message = String(error);
    } finally {
      busy = false;
    }
  }
</script>

<section class="history-mirror-settings" aria-labelledby="history-mirror-settings-title">
  <h3 id="history-mirror-settings-title">Listening history mirror</h3>
  {#if !desktop}
    <p>History mirror settings require the Burrata desktop app.</p>
  {:else}
    <p>Choose which retained history to include when the Listening history playlist is written to a local M3U copy. These options apply when global mirroring is on and Listening history is included.</p>
    <label><input type="radio" name="history-mirror-scope" value="all" bind:group={scope} /> All retained history</label>
    <label><input type="radio" name="history-mirror-scope" value="recent" bind:group={scope} /> Recent history</label>
    {#if scope === "recent"}
      <label for="history-mirror-recent-days">Days to include</label>
      <input id="history-mirror-recent-days" type="number" min="1" max="3650" step="1" bind:value={recentDays} />
      <small>The limit is shown in the exported playlist name and metadata.</small>
    {/if}
    <button type="button" disabled={busy || scope === "recent" && (!Number.isInteger(recentDays) || recentDays < 1 || recentDays > 3650)} onclick={() => void save()}>{busy ? "Saving…" : "Save history options"}</button>
    {#if message}<p role="status">{message}</p>{/if}
  {/if}
</section>

<style>
  .history-mirror-settings{display:grid;gap:9px;max-width:640px;padding:12px;border:1px solid var(--line);border-radius:9px;margin-top:12px}.history-mirror-settings h3,.history-mirror-settings p{margin:0}.history-mirror-settings p,.history-mirror-settings small{color:var(--muted);font-size:.86rem;line-height:1.45}.history-mirror-settings label{display:flex;align-items:center;gap:8px}.history-mirror-settings input[type="number"]{max-width:150px;padding:8px;border:1px solid var(--line);border-radius:7px;background:var(--surface);color:var(--text);font:inherit}
</style>
