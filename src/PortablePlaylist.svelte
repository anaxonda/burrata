<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { Collection } from "./lib/types";

  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  type Options = {
    purpose: "share" | "mirror" | "history";
    includeYtdlpSearch: boolean;
    includeNotes: boolean;
    includeTags: boolean;
    includeRatings: boolean;
    includeHistoryDates: boolean;
  };
  type Preview = {
    entryCount: number;
    localEntries: number;
    identityPlaceholders: number;
    searchLinks: number;
    requiresBurrata: boolean;
    excludedPrivateFields: string[];
  };
  type Hierarchy = {
    schemaVersion: number;
    folders: { id: string; name: string; parentId: string | null; position: number }[];
    playlists: { collectionId: string; folderId: string | null; position: number; sections: { id: string; name: string; position: number }[]; occurrences: { id: string; sectionId: string | null }[] }[];
  };

  let {
    collection,
    call,
    includeYtdlpSearchDefault = false,
    initialMode = "export",
    onClose = () => {},
  }: {
    collection: Collection;
    call: Call;
    includeYtdlpSearchDefault?: boolean;
    initialMode?: "export" | "folder";
    onClose?: () => void;
  } = $props();

  let mode = $state<"export" | "folder">(untrack(() => initialMode));
  let options = $state<Options>({ purpose: "share", includeYtdlpSearch: untrack(() => includeYtdlpSearchDefault), includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false });
  let preview = $state<Preview | null>(null);
  let hierarchy = $state<Hierarchy | null>(null);
  let folderPath = $state("");
  let busy = $state(false);
  let message = $state("");
  let previewRequest = 0;

  onMount(() => {
    if (mode === "export") void refreshExportPreview();
  });

  $effect(() => {
    if (mode === "folder" && !hierarchy) void loadHierarchy();
  });

  function activatePortableDialog(node: HTMLDialogElement) {
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

  async function refreshExportPreview() {
    const request = ++previewRequest;
    busy = true; message = "";
    try {
      const result = await call<Preview>("preview_portable_playlist_export", { collectionId: collection.id, options: { ...options } });
      if (request === previewRequest) preview = result;
    }
    catch (error) { if (request === previewRequest) message = String(error); }
    finally { if (request === previewRequest) busy = false; }
  }

  async function exportFile() {
    busy = true; message = "";
    try {
      preview = await call<Preview>("preview_portable_playlist_export", { collectionId: collection.id, options });
      const path = await call<string>("export_portable_playlist", { collectionId: collection.id, options });
      message = `Exported to ${path}`;
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  async function copyText() {
    busy = true; message = "";
    try {
      preview = await call<Preview>("preview_portable_playlist_export", { collectionId: collection.id, options });
      const text = await call<string>("render_portable_playlist_export", { collectionId: collection.id, options });
      await navigator.clipboard.writeText(text);
      message = "Portable playlist copied to clipboard.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  async function loadHierarchy() {
    busy = true; message = "";
    try {
      hierarchy = await call<Hierarchy>("get_playlist_hierarchy");
      const placement = hierarchy.playlists.find((item) => item.collectionId === collection.id);
      const names: string[] = [];
      let folderId = placement?.folderId ?? null;
      while (folderId) {
        const folder = hierarchy.folders.find((item) => item.id === folderId);
        if (!folder) break;
        names.unshift(folder.name); folderId = folder.parentId;
      }
      folderPath = names.join("/");
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  async function saveHierarchy() {
    if (!hierarchy) return;
    busy = true; message = "";
    try {
      const names = folderPath.split("/").map((name) => name.trim()).filter(Boolean);
      let parentId: string | null = null;
      for (const [position, name] of names.entries()) {
        let folder = hierarchy.folders.find((item) => item.parentId === parentId && item.name.toLocaleLowerCase() === name.toLocaleLowerCase());
        if (!folder) {
          folder = { id: crypto.randomUUID(), name, parentId, position };
          hierarchy.folders.push(folder);
        }
        parentId = folder.id;
      }
      let placement = hierarchy.playlists.find((item) => item.collectionId === collection.id);
      if (!placement) {
        placement = { collectionId: collection.id, folderId: parentId, position: hierarchy.playlists.length, sections: [], occurrences: [] };
        hierarchy.playlists.push(placement);
      }
      placement.folderId = parentId;
      hierarchy = await call<Hierarchy>("save_playlist_hierarchy", { hierarchy });
      message = "Playlist folder updated.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

</script>

<dialog class="download-dialog portable-playlist-dialog" use:activatePortableDialog aria-label="Portable playlist" aria-labelledby="portable-playlist-title" aria-modal="true" tabindex="-1" onclick={(event) => {
  if (event.target !== event.currentTarget) return;
  const bounds = event.currentTarget.getBoundingClientRect();
  if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose();
}}>
  <header><div><h2 id="portable-playlist-title">{mode === "export" ? "Export playlist" : "Move to folder"}</h2><p>{collection.name}</p></div><button onclick={onClose} aria-label="Close">×</button></header>

  {#if mode === "export"}
    <p class="purpose">Save the track list to move it to another device or keep a backup. Music files aren’t included.</p>
    {#if options.purpose === "share"}<p class="privacy">Shared files leave out local paths, private provider links, and personal details.</p>{/if}
    <div class="actions"><button class="primary" disabled={busy} onclick={() => void exportFile()}>Save playlist file</button></div>
    {#if preview}<section class="preview" aria-label="Export preview"><strong>{preview.entryCount} tracks</strong>{#if preview.identityPlaceholders}<span>{preview.identityPlaceholders} {preview.identityPlaceholders === 1 ? "track needs" : "tracks need"} a playable link. Burrata can match them when imported.</span>{/if}</section>{/if}
    <details class="advanced"><summary>Advanced export options</summary>
      <p>Add optional search links, use audio files from this computer, or copy the playlist text.</p>
      <button disabled={busy} onclick={() => void copyText()}>Copy playlist file contents</button>
      <p>Copies the exported text to your clipboard for pasting into a compatible app.</p>
      <label class="check-row"><input type="checkbox" aria-label="Include search links for this export" bind:checked={options.includeYtdlpSearch} onchange={() => void refreshExportPreview()} /><span>Add optional search links</span></label>
      <label class="check-row"><input type="checkbox" checked={options.purpose === "mirror"} onchange={event => { options.purpose = event.currentTarget.checked ? "mirror" : "share"; void refreshExportPreview(); }} /><span>Use audio files on this computer</span></label>
      {#if options.purpose === "mirror"}<p>This file uses local paths and only works on this computer.</p>{/if}
      {#if options.includeYtdlpSearch}<p>Search links help find playable versions. Burrata still checks recording matches when importing.</p>{/if}
      {#if preview}<details class="privacy-details"><summary>Technical privacy details</summary><p>Fields excluded: {preview.excludedPrivateFields.join(", ") || "none"}.</p></details>{/if}
    </details>
  {:else if mode === "folder"}
    <p>Choose where this playlist appears in your library. Leave the folder name blank to remove it from its current folder.</p>
    <label class="field-row"><span>Folder path</span><input bind:value={folderPath} placeholder="For example, Sets/Evening" /></label>
    <button disabled={busy} onclick={() => void saveHierarchy()}>Move playlist</button>
  {/if}
  {#if message}<p role="status">{message}</p>{/if}
</dialog>

<style>
  .portable-playlist-dialog { background:var(--surface,#191e1b); color:var(--text,#e5ebe7); }
  .portable-playlist-dialog::backdrop { background:color-mix(in srgb,#050806 76%,transparent); backdrop-filter:blur(5px); }
  header,.actions { display:flex; align-items:center; gap:10px; justify-content:space-between; }
  header p { margin:3px 0 0; color:var(--muted); } h2 { margin:0; font-size:1.2rem; }
  .actions { justify-content:flex-start; margin:18px 0 12px; } .primary { background:var(--accent,#7aa2ff); color:var(--accent-contrast,#111); font-weight:650; }
  p { line-height:1.45; } .privacy { margin:16px 0 0; color:var(--muted); }
  .preview { display:grid; gap:4px; margin:12px 0 18px; padding:12px; border:1px solid var(--line,#444); border-radius:8px; } .preview span { color:var(--muted); font-size:.9rem; }
  .advanced,.privacy-details { border-top:1px solid var(--line,#444); padding-top:12px; margin-top:12px; } summary { cursor:pointer; font-weight:600; } details > p { color:var(--muted); font-size:.9rem; }
  .check-row,.field-row { display:flex; align-items:center; gap:10px; margin:12px 0; } .portable-playlist-dialog .check-row input[type="checkbox"] { width:18px; height:18px; min-height:18px; flex:none; margin:0; } .check-row span { flex:1; }
  .portable-playlist-dialog .check-row { flex-direction:row; justify-content:flex-start; text-align:left; }
  .field-row { align-items:flex-start; flex-direction:column; } input { max-width:100%; } .field-row input { width:100%; box-sizing:border-box; }
</style>
