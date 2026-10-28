<script lang="ts">
  import { untrack } from "svelte";
  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  type Preview = { entryCount: number; identityPlaceholders: number; searchLinks: number; excludedPrivateFields: string[] };
  let { call, includeYtdlpSearchDefault = false, onClose = () => {} }: { call: Call; includeYtdlpSearchDefault?: boolean; onClose?: () => void } = $props();
  let includeHistoryDates = $state(false);
  let includeYtdlpSearch = $state(untrack(() => includeYtdlpSearchDefault));
  let preview = $state<Preview | null>(null);
  let busy = $state(false);
  let message = $state("");
  const options = () => ({ purpose: "history", includeYtdlpSearch, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates });

  async function loadPreview(): Promise<Preview | null> {
    busy = true; message = "";
    try {
      preview = await call<Preview>("preview_history_playlist_export", { options: options() });
      return preview;
    } catch (error) { message = String(error); return null; }
    finally { busy = false; }
  }

  async function createFile() {
    busy = true; message = "";
    try {
      preview = await call<Preview>("preview_history_playlist_export", { options: options() });
      const text = await call<string>("render_history_playlist_export", { options: options() });
      const url = URL.createObjectURL(new Blob([text], { type: "audio/x-mpegurl;charset=utf-8" }));
      const anchor = document.createElement("a"); anchor.href = url; anchor.download = "Listening History.m3u8"; anchor.click(); URL.revokeObjectURL(url);
      message = "Listening history saved as a playlist file.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  async function copyText() {
    busy = true; message = "";
    try {
      preview = await call<Preview>("preview_history_playlist_export", { options: options() });
      const text = await call<string>("render_history_playlist_export", { options: options() });
      await navigator.clipboard.writeText(text);
      message = "Playlist text copied to clipboard.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  function handleOutsideClick(event: MouseEvent) {
    const node = event.currentTarget as HTMLDialogElement;
    if (event.target !== node) return;
    const bounds = node.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose();
  }

  function activateHistoryDialog(node: HTMLDialogElement) {
    const restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const onCancel = (event: Event) => { event.preventDefault(); onClose(); };
    node.addEventListener("cancel", onCancel);
    if (typeof node.showModal === "function") node.showModal();
    else node.setAttribute("open", "");
    return { destroy() {
      node.removeEventListener("cancel", onCancel);
      if (node.open && typeof node.close === "function") node.close();
      if (restoreFocus?.isConnected) restoreFocus.focus();
    } };
  }
</script>

  <dialog class="download-dialog portable-history-dialog" use:activateHistoryDialog aria-label="Export listening history" aria-labelledby="portable-history-title" aria-modal="true" tabindex="-1" onclick={handleOutsideClick}>
    <header><div><h2 id="portable-history-title">Export listening history</h2></div><button onclick={onClose} aria-label="Close">×</button></header>
    <p>Save the ordered track list from your listening history. Music files aren’t included.</p>
    <p>Repeat plays are kept. {#if includeHistoryDates}Listening dates are included in this file.{:else}Listening dates are left out by default.{/if}</p>
    <div class="actions"><button class="primary" disabled={busy} onclick={() => void createFile()}>Save playlist file</button></div>
    {#if preview}<section class="preview" aria-label="Export preview"><strong>{preview.entryCount} tracks</strong>{#if preview.identityPlaceholders}<span>{preview.identityPlaceholders} {preview.identityPlaceholders === 1 ? "track needs" : "tracks need"} a playable link. Burrata can match them when imported.</span>{/if}</section>{/if}
    <details class="advanced"><summary>Advanced export options</summary>
      <p>Add optional search links, include listening dates, or copy the playlist text.</p>
      <label class="check-row"><input type="checkbox" aria-label="Include search links for this export" bind:checked={includeYtdlpSearch} onchange={() => (preview = null)} /><span>Add optional search links</span></label>
      <label class="check-row"><input type="checkbox" aria-label="Include listening dates" bind:checked={includeHistoryDates} onchange={() => (preview = null)} /><span>Include listening dates</span></label>
      <button disabled={busy} onclick={() => void copyText()}>Copy playlist file contents</button>
      <p>Copies the playlist text to your clipboard for pasting into a compatible app.</p>
      <button disabled={busy} onclick={() => void loadPreview()}>Update preview</button>
      {#if preview}<details class="privacy-details"><summary>Technical privacy details</summary><p>Fields excluded: {preview.excludedPrivateFields.join(", ") || "none"}.</p></details>{/if}
    </details>
    {#if message}<p role="status">{message}</p>{/if}
  </dialog>

<style>
  .portable-history-dialog { width:min(520px,calc(100vw - 32px)); max-height:calc(100vh - 32px); background:var(--surface,#181818); color:var(--text,#eee); }
  .portable-history-dialog::backdrop { background:color-mix(in srgb,#050806 76%,transparent); backdrop-filter:blur(5px); }
  .portable-history-dialog header { display:flex; align-items:center; justify-content:space-between; }
  .portable-history-dialog h2 { margin:0; }
  .actions { display:flex; justify-content:flex-start; margin:18px 0 12px; }
  .primary { background:var(--accent,#7aa2ff); color:var(--accent-contrast,#111); font-weight:650; }
  .preview { display:grid; gap:4px; margin:12px 0 18px; padding:12px; border:1px solid var(--line,#444); border-radius:8px; }
  .preview span { color:var(--muted); font-size:.9rem; }
  .advanced,.privacy-details { border-top:1px solid var(--line,#444); padding-top:12px; margin-top:12px; }
  summary { cursor:pointer; font-weight:600; }
  .check-row { display:flex; align-items:center; gap:10px; margin:12px 0; }
  .portable-history-dialog .check-row input[type="checkbox"] { width:18px; height:18px; min-height:18px; flex:none; margin:0; }
  .check-row span { flex:1; }
</style>
