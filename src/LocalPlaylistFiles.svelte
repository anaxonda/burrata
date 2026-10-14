<script lang="ts">
  import { onMount } from "svelte";
  import { call } from "./lib/api";
  import type { Settings, Collection } from "./lib/types";

  interface PlaylistFile {
    path: string;
    name: string;
    trackCount: number;
    modified?: string | null;
    warning?: string | null;
  }
  interface ScanRoot { path: string; warning?: string | null; available?: boolean }
  interface ScanResult { files: PlaylistFile[]; roots?: ScanRoot[]; warnings?: string[] }

  let {
    mode = "browse",
    settings,
    onChange = (_value: Settings) => {},
    onImported = (_collection: Collection) => {},
    onSettings = () => {},
    onSave = () => {},
  }: {
    mode?: "browse" | "settings";
    settings?: Settings;
    onChange?: (value: Settings) => void;
    onImported?: (collection: Collection) => void;
    onSettings?: () => void;
    onSave?: () => void;
  } = $props();

  let files = $state<PlaylistFile[]>([]);
  let roots = $state<ScanRoot[]>([]);
  let warnings = $state<string[]>([]);
  let busy = $state(false);
  let message = $state("");
  let scanRevision = 0;
  const folders = $derived((settings?.playlistDirectories ?? []).join("\n"));
  const configuredRoots = $derived([
    ...(settings?.playlistDirectories ?? []).filter((path) => path.trim()),
    ...(settings?.musicDirectory?.trim() ? [settings.musicDirectory.trim()] : []),
    "~/Music",
  ]);
  const scanSettingsKey = $derived([
    ...(settings?.playlistDirectories ?? []),
    settings?.musicDirectory ?? "",
  ].join("\n"));

  async function scan() {
    const revision = ++scanRevision;
    busy = true;
    message = "";
    try {
      // Newer builds return scan diagnostics with their files. The array form is
      // retained for older app versions that expose only the file list.
      const result = await call<ScanResult | PlaylistFile[]>("scan_local_playlists");
      if (revision !== scanRevision) return;
      if (Array.isArray(result)) {
        files = result;
        roots = [];
        warnings = [];
      } else {
        files = result.files ?? [];
        roots = result.roots ?? [];
        warnings = result.warnings ?? [];
      }
      if (!files.length && !warnings.length && !roots.some((root) => root.warning)) {
        message = "No M3U playlists found in the scanned folders.";
      }
    } catch (error) {
      if (revision === scanRevision) message = String(error);
    } finally {
      if (revision === scanRevision) busy = false;
    }
  }

  async function openPlaylist(file: PlaylistFile, refresh = false) {
    busy = true;
    message = "";
    try {
      const result = await call<{ collection: Collection; importedCount: number; warning?: string }>(
        "import_local_playlist",
        { path: file.path, refresh },
      );
      message = result.warning || `Opened ${result.importedCount} playlist entries.`;
      onImported(result.collection);
    } catch (error) {
      message = String(error);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    mode;
    scanSettingsKey;
    if (mode === "browse") void scan();
  });

  onMount(() => {
    if (mode !== "browse") return;
    const refresh = () => {
      if (document.visibilityState === "visible") void scan();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  });
</script>

<section class="playlist-files settings-card" aria-labelledby={mode === "settings" ? "playlist-folders-title" : "playlist-files-title"}>
  {#if mode === "settings"}
    <h2 id="playlist-folders-title">Playlist folders</h2>
    <p>Burrata scans these folders and subfolders for M3U and M3U8 playlists. It also checks <code>~/Music</code> and your music directory.</p>
    <label>Folders, one per line<textarea aria-label="Playlist folders" rows="3" value={folders} placeholder={settings?.musicDirectory || "~/Music"} oninput={(event) => { if (settings) onChange({ ...settings, playlistDirectories: event.currentTarget.value.split("\n").map((value) => value.trim()).filter(Boolean) }); }}></textarea></label>
    <small>Scanned locations include {configuredRoots.join(", ")}. Opening a playlist adds its entries to Burrata and leaves the source file in place.</small>
    <button onclick={onSave}>Save playlist folders</button>
  {:else}
    <div class="playlist-heading">
      <div>
        <h2 id="playlist-files-title">Playlist files on this computer</h2>
        <p>Open an M3U or M3U8 playlist as a Burrata collection.</p>
      </div>
      <div class="playlist-actions">
        <button disabled={busy} onclick={() => void scan()}>{busy ? "Scanning…" : "Scan folders"}</button>
        <button onclick={onSettings}>Choose folders</button>
      </div>
    </div>
    {#if roots.length}
      <ul class="scan-roots" aria-label="Scanned folders">
        {#each roots as root (root.path)}
          <li><span>{root.path}</span>{#if root.warning}<small>{root.warning}</small>{:else if root.available === false}<small>Folder unavailable</small>{/if}</li>
        {/each}
      </ul>
    {:else}
      <small class="root-hint">Scans configured folders, ~/Music, and your saved music directory.</small>
    {/if}
    {#if warnings.length}
      <ul class="scan-warnings" aria-label="Playlist scan warnings">
        {#each warnings as warning}<li>{warning}</li>{/each}
      </ul>
    {/if}
    {#if files.length}
      <div class="playlist-list">
        {#each files as file (file.path)}
          <article>
            <div class="file-details">
              <strong>{file.name}</strong>
              <small class="path" title={file.path}>{file.path}</small>
              <small>{file.trackCount} entries, including URL entries</small>
              {#if file.warning}<small class="warning">{file.warning}</small>{/if}
            </div>
            <div class="playlist-actions"><button disabled={busy || !!file.warning} onclick={() => void openPlaylist(file)}>Open playlist</button><button disabled={busy || !!file.warning} title="Import a new version without changing the original file" onclick={() => void openPlaylist(file, true)}>Refresh import</button></div>
          </article>
        {/each}
      </div>
    {/if}
    {#if message}<p class="status" role="status">{message}</p>{/if}
  {/if}
</section>

<style>
  .playlist-files { margin: 12px 0; min-width: 0; }
  .playlist-heading, article { display: flex; gap: 12px; align-items: center; justify-content: space-between; }
  .playlist-heading > div:first-child, .file-details { min-width: 0; }
  .playlist-heading p { margin: 5px 0 0; }
  .playlist-actions { display: flex; gap: 8px; flex-wrap: wrap; }
  label { display: grid; gap: 8px; }
  textarea { width: 100%; box-sizing: border-box; resize: vertical; }
  small { display: block; color: var(--muted); overflow-wrap: anywhere; margin-top: 6px; }
  .root-hint { margin: 12px 0; }
  .scan-roots, .scan-warnings { margin: 12px 0; padding-left: 20px; color: var(--muted); }
  .scan-roots li, .scan-warnings li { margin: 5px 0; overflow-wrap: anywhere; }
  article { border-top: 1px solid var(--line); padding: 12px 0; }
  .file-details { flex: 1; }
  .path { font-size: 11px; }
  .warning, .scan-warnings { color: var(--warning, #dc9b43); }
  .status { color: var(--muted); }
  code { font-size: 0.95em; }
  @media (max-width: 620px) { .playlist-heading, article { align-items: flex-start; flex-direction: column; } }
</style>
