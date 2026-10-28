<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import type { Collection, Track } from "./lib/types";
  import type { ImportChoice } from "./ImportHub.svelte";
  import type { BackupInspection } from "./BackupRestore.svelte";
  import { listen } from "./lib/api";

  type Call = <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  type LocalAudioFileSeal = { path: string; sizeBytes: string; modifiedNs: string | null; fileIdentity: string };
  type ApplyOptions = { destinationId: string; newPlaylistName: string; folderPath: string; sectionName: string; duplicatePolicy: "keep" | "skip-repeated"; localAssetsImported?: boolean };
  type PortableMatchingLevel = "exact" | "balanced" | "flexible";
  type PortableImportMode = "create" | "update" | "merge";
  type PortableDuplicatePolicy = "keep" | "skip";
  type PortableProviderOption = { provider: string; sourceId: string; title: string; artist?: string | null; version?: string | null; durationSeconds?: number | null; relationship: string; action: string; evidence: string[] };
  type PortableProviderSelection = { entryOrder: number; provider: string; sourceId: string };
  type NativeImportFileDrop = { token: string; kind: "backup" | "playlist" | "unsupported"; coordinateSpace: "logical" | "physical"; position: { x: number; y: number } };
  type ConsumedImportDrop = { kind: "playlist"; name: string; text: string } | { kind: "backup"; preview: BackupInspection };
  type PortableEntryPreview = { entryOrder: number; order: number; title: string; artists: string[]; version?: string | null; durationSeconds?: number | null; sectionName?: string | null; status: "exactLocal" | "providerSuggestion" | "providerAmbiguous" | "providerConflict" | "unresolved"; provider?: string | null; providerTitle?: string | null; providerArtist?: string | null; providerVersion?: string | null; providerDurationSeconds?: number | null; relationship?: string | null; reason?: string | null; conflicts: string[]; providerOptions?: PortableProviderOption[]; suggestedSelection?: PortableProviderSelection | null };
  type PortablePreview = {
    previewId: string; sourceHash: string; name: string; entryCount: number; duplicateOccurrences: number;
    exactLocalMatches: number; ambiguousLocalMatches: number; unresolvedEntries: number;
    providerMatchSuggestions: number; providerAmbiguousEntries: number; providerConflictEntries: number;
    targetName?: string | null; targetEntryCount: number;
    projectedAddedEntries: number; projectedSkippedDuplicates: number;
    folderAncestry?: string[]; sections?: { name: string; entryCount: number }[];
    entries?: PortableEntryPreview[]; warnings: string[];
  };
  let { choice, call, collections, currentCollectionId = null, desktop = false, onApply, onPortableImported = async () => {}, onBackupSelected = () => {}, onBackupPreview, onClose = () => {} }: {
    choice: ImportChoice;
    call: Call;
    collections: Collection[];
    currentCollectionId?: string | null;
    desktop?: boolean;
    onApply: (tracks: Track[], options: ApplyOptions) => Promise<void>;
    onPortableImported?: (collection: Collection) => Promise<void>;
    onBackupSelected?: () => void;
    onBackupPreview?: (preview: BackupInspection) => void;
    onClose?: () => void;
  } = $props();

  let text = $state("");
  let format = $state<"auto" | "tracklist" | "csv" | "m3u">("auto");
  let detectedFormat = $state<"tracklist" | "csv" | "m3u">("tracklist");
  let tracks = $state<Track[]>([]);
  let paths = $state<string[]>([]);
  let seals = $state<LocalAudioFileSeal[]>([]);
  let destinationId = $state("");
  let newPlaylistName = $state("");
  let folderPath = $state("");
  let sectionName = $state("");
  let duplicatePolicy = $state<"keep" | "skip-repeated">("skip-repeated");
  let busy = $state(false);
  let message = $state("");
  let previewRequest = 0;
  let portablePreviewTimer: ReturnType<typeof setTimeout> | undefined;
  let portablePreview = $state<PortablePreview | null>(null);
  let portableDestinationName = $state("");
  let portableFolderOverride = $state("");
  let portableHasFolderOverride = $state(false);
  let portableMatchingLevel = $state<PortableMatchingLevel>("balanced");
  let portableMode = $state<PortableImportMode>("create");
  let portableTargetCollectionId = $state("");
  let portableDuplicatePolicy = $state<PortableDuplicatePolicy>("keep");
  let portableHash = $state("");
  let portablePreviewId = $state("");
  let portableInputText = $state("");
  let portableFileName = $state("");
  let portableProviderSelections = $state<Record<number, string>>({});
  let sourceSection = $state<HTMLElement | null>(null);
  let nativeDropRequest = 0;
  let recentDrop: { origin: "native" | "dom"; x: number; y: number; at: number } | null = null;
  let repeatedCount = $derived(tracks.length - new Set(tracks.map(track => track.id)).size);
  let portableInput = $derived(detectedFormat === "m3u" && /^#BURRATA-PLAYLIST:/im.test(text));
  let portableReviewReady = $derived(Boolean(portablePreview && portablePreviewId && portableHash));

  let local = $derived(choice === "local-audio");
  let paste = $derived(choice === "paste-playlist");
  let title = $derived(local ? "Add local audio" : paste ? "Paste a playlist" : "Import a playlist file");
  const applyOptions = () => ({ destinationId, newPlaylistName, folderPath, sectionName, duplicatePolicy });

  function portableReviewChanged() {
    if (portablePreviewTimer) clearTimeout(portablePreviewTimer);
    if (text.trim()) void previewText(text);
  }

  function providerOptionKey(option: PortableProviderOption) { return `${option.provider}|${option.sourceId}`; }
  function providerOptionsForApply(): PortableProviderSelection[] {
    if (!portablePreview?.entries) return [];
    return portablePreview.entries.flatMap((entry) => {
      const key = portableProviderSelections[entry.entryOrder];
      if (!key || entry.status === "exactLocal") return [];
      const option = entry.providerOptions?.find((item) => providerOptionKey(item) === key && item.action !== "Reject");
      return option ? [{ entryOrder: entry.entryOrder, provider: option.provider, sourceId: option.sourceId }] : [];
    });
  }

  function setPortableProviderSelection(entry: PortableEntryPreview, value: string) {
    portableProviderSelections = { ...portableProviderSelections, [entry.entryOrder]: value };
  }

  function scheduleTextPreview() {
    if (detect(text) !== "m3u" || !/^#BURRATA-PLAYLIST:/im.test(text)) {
      void previewText(text);
      return;
    }
    if (portablePreviewTimer) clearTimeout(portablePreviewTimer);
    const request = ++previewRequest;
    portablePreviewTimer = setTimeout(() => {
      portablePreviewTimer = undefined;
      if (request === previewRequest) void previewText(text);
    }, 450);
  }

  function closeImport() {
    if (portablePreviewTimer) clearTimeout(portablePreviewTimer);
    portablePreviewTimer = undefined;
    previewRequest += 1;
    onClose();
  }

  // Closing the parent dialog can unmount this flow while a delayed preview is
  // pending. Invalidate it so an older request cannot update a later import.
  onDestroy(() => {
    if (portablePreviewTimer) clearTimeout(portablePreviewTimer);
    portablePreviewTimer = undefined;
    previewRequest += 1;
  });

  function setPortableMode(mode: PortableImportMode) {
    portableMode = mode;
    if (mode !== "create" && !portableTargetCollectionId && currentCollectionId) {
      portableTargetCollectionId = currentCollectionId;
    }
    portableReviewChanged();
  }

  function artistCredit(entry: PortableEntryPreview) {
    return entry.artists.filter(Boolean).join(", ") || "Unknown artist";
  }

  function resolutionLabel(entry: PortableEntryPreview) {
    if (entry.status === "exactLocal") return "Local asset";
    if (entry.status === "providerSuggestion") return entry.provider ? "Provider suggestion · " + entry.provider : "Provider suggestion";
    if (entry.status === "providerAmbiguous") return "Ambiguous provider matches";
    if (entry.status === "providerConflict") return "Provider conflict";
    return "Unresolved metadata";
  }

  function sectionSummary() {
    const sections = portablePreview?.sections ?? [];
    if (sections.length) return sections.map(section => section.name + " (" + section.entryCount + ")").join(" · ");
    const byName = new Map<string, number>();
    for (const entry of portablePreview?.entries ?? []) {
      if (entry.sectionName) byName.set(entry.sectionName, (byName.get(entry.sectionName) ?? 0) + 1);
    }
    return [...byName].map(([name, count]) => name + " (" + count + ")").join(" · ");
  }

  function entryDisplay(entry: PortableEntryPreview) {
    return artistCredit(entry) + " — " + entry.title + (entry.version ? " (" + entry.version + ")" : "");
  }

  function providerEvidence(entry: PortableEntryPreview) {
    const name = [entry.providerArtist, entry.providerTitle].filter(Boolean).join(" — ");
    return name + (entry.providerVersion ? " (" + entry.providerVersion + ")" : "") + (entry.provider ? " · " + entry.provider : "") + (entry.providerDurationSeconds ? " · " + Math.round(entry.providerDurationSeconds) + " sec" : "");
  }

  function humanizeEvidence(value: string) {
    return value.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replaceAll("_", " ");
  }

  $effect(() => { if (currentCollectionId) destinationId = currentCollectionId; });

  function detect(value: string): "tracklist" | "csv" | "m3u" {
    const sample = value.trimStart().replace(/^\uFEFF/, "");
    if (/^#EXTM3U\b/im.test(sample) || /^#BURRATA-PLAYLIST:/im.test(sample)) return "m3u";
    const first = sample.split(/\r?\n/, 1)[0] ?? "";
    if (/\b(artist|artists|artist name)\b/i.test(first) && /\b(title|track|track name)\b/i.test(first) && first.includes(",")) return "csv";
    return "tracklist";
  }

  async function previewText(value = text) {
    const request = ++previewRequest;
    const sourceChanged = value !== portableInputText;
    if (sourceChanged) {
      portableInputText = value;
      portableProviderSelections = {};
      portableDestinationName = "";
      portableFolderOverride = "";
      portableHasFolderOverride = false;
    }
    tracks = []; portablePreview = null; portableHash = ""; portablePreviewId = ""; message = "";
    if (!value.trim()) { portableHash = ""; portablePreviewId = ""; busy = false; return; }
    detectedFormat = detect(value);
    busy = true;
    try {
      if (detectedFormat === "m3u" && /^#BURRATA-PLAYLIST:/im.test(value)) {
        const requestedName = portableDestinationName.trim();
        const preview = await call<PortablePreview>("preview_portable_playlist_import", {
          text: value,
          options: {
            matchingLevel: portableMatchingLevel,
            mode: portableMode,
            targetCollectionId: portableMode === "create" ? null : portableTargetCollectionId || null,
            duplicatePolicy: portableMode === "merge" ? portableDuplicatePolicy : "keep",
            destinationName: portableMode === "create" ? requestedName || null : null,
            destinationFolderPath: portableHasFolderOverride ? portableFolderOverride.trim() || null : null,
          },
        });
        if (request === previewRequest) {
          portablePreview = preview;
          const nextSelections: Record<number, string> = {};
          for (const entry of preview.entries ?? []) {
            const prior = portableProviderSelections[entry.entryOrder];
            const priorStillAvailable = entry.providerOptions?.some((option) => providerOptionKey(option) === prior && option.action !== "Reject");
            const suggested = entry.suggestedSelection;
            const suggestionStillAvailable = suggested && entry.providerOptions?.some((option) => option.provider === suggested.provider && option.sourceId === suggested.sourceId && option.action !== "Reject");
            if (priorStillAvailable) nextSelections[entry.entryOrder] = prior;
            else if (suggestionStillAvailable) nextSelections[entry.entryOrder] = `${suggested.provider}|${suggested.sourceId}`;
          }
          portableProviderSelections = nextSelections;
          if (sourceChanged || !portableDestinationName) portableDestinationName = preview.name;
          portableHash = preview.sourceHash;
          portablePreviewId = preview.previewId;
          if (portableMode !== "create" && !portableTargetCollectionId) {
            message = "Choose a playlist to update or merge into.";
          }
          if (!requestedName && portableMode === "create") {
            queueMicrotask(() => {
              if (request === previewRequest) void previewText(value);
            });
          }
        }
      } else {
        const preview = await call<Track[]>("preview_import_tracks", { text: value, kind: format === "auto" ? detectedFormat : format });
        if (request === previewRequest) tracks = preview;
      }
    }
    catch (error) { if (request === previewRequest) message = String(error); }
    finally { if (request === previewRequest) busy = false; }
  }

  async function loadFileObject(file: File | undefined, input?: HTMLInputElement) {
    tracks = []; portablePreview = null; portableHash = ""; text = ""; message = "";
    if (!file) return;
    if (file.name.toLowerCase().endsWith(".burrata-backup")) { portableFileName = ""; message = "Backup file selected. Opening Restore Backup…"; onBackupSelected(); if (input) input.value = ""; return; }
    if (file.size > 2 * 1024 * 1024) { message = "Imports are limited to 2 MB."; if (input) input.value = ""; return; }
    try {
      portableFileName = file.name; text = await file.text(); detectedFormat = detect(text);
      format = "auto";
      await previewText(text);
    } catch (error) { message = String(error); }
    if (input) input.value = "";
  }

  async function loadDroppedPlaylist(name: string, contents: string) {
    tracks = []; portablePreview = null; portableHash = ""; text = ""; message = "";
    portableFileName = name;
    text = contents;
    detectedFormat = detect(contents);
    format = "auto";
    await previewText(contents);
  }

  async function loadFile(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    await loadFileObject(input.files?.[0], input);
  }

  async function dropFile(event: DragEvent) {
    event.preventDefault();
    const point = { x: event.clientX, y: event.clientY };
    if (isDuplicateDrop("native", point)) return;
    if (!event.dataTransfer?.files?.length) return;
    recentDrop = { origin: "dom", ...point, at: Date.now() };
    await loadFileObject(event.dataTransfer.files[0]);
  }

  function isDuplicateDrop(origin: "native" | "dom", point: { x: number; y: number }) {
    const prior = recentDrop;
    return Boolean(prior && prior.origin === origin && Date.now() - prior.at <= 1500 && Math.hypot(prior.x - point.x, prior.y - point.y) <= 12);
  }

  function nativeDropCssPoint(drop: NativeImportFileDrop) {
    const ratio = Number.isFinite(window.devicePixelRatio) && window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
    return drop.coordinateSpace === "physical"
      ? { x: drop.position.x / ratio, y: drop.position.y / ratio }
      : { x: drop.position.x, y: drop.position.y };
  }

  function insideImportZone(point: { x: number; y: number }) {
    const bounds = sourceSection?.getBoundingClientRect();
    if (!bounds || !Number.isFinite(point.x) || !Number.isFinite(point.y)) return false;
    return point.x >= bounds.left && point.x <= bounds.right && point.y >= bounds.top && point.y <= bounds.bottom;
  }

  onMount(() => {
    if (!desktop || choice !== "playlist-file" || !window.__TAURI__?.event?.listen) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<NativeImportFileDrop>("import-file-drop", ({ payload }) => {
      if (!payload || payload.kind === "backup" || typeof payload.token !== "string" || !payload.token || !payload.position || !["logical", "physical"].includes(payload.coordinateSpace)) return;
      const point = nativeDropCssPoint(payload);
      if (!insideImportZone(point)) return;
      const pairedDomDrop = Boolean(recentDrop?.origin === "dom" && Date.now() - recentDrop.at <= 1500 && Math.hypot(recentDrop.x - point.x, recentDrop.y - point.y) <= 12);
      recentDrop = { origin: "native", x: point.x, y: point.y, at: Date.now() };
      const request = ++nativeDropRequest;
      void call<ConsumedImportDrop>("consume_import_drop", { token: payload.token }).then(async (result) => {
        if (disposed || request !== nativeDropRequest || pairedDomDrop) return;
        if (result.kind === "playlist") {
          await loadDroppedPlaylist(result.name, result.text);
          return;
        }
        if (result.kind === "backup") {
          if (!onBackupPreview) throw new Error("Backup drop review is unavailable.");
          onBackupPreview(result.preview);
          return;
        }
        throw new Error("The dropped item is not a supported playlist or backup file.");
      }).catch((failure) => {
        if (!disposed && request === nativeDropRequest && !pairedDomDrop) message = `Could not read dropped file: ${String(failure)}`;
      });
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((failure) => {
      if (!disposed) message = `Could not listen for dropped files: ${String(failure)}`;
    });
    return () => {
      disposed = true;
      nativeDropRequest += 1;
      unlisten?.();
    };
  });

  async function chooseAudio(kind: "files" | "folder") {
    busy = true; tracks = []; paths = []; message = "";
    try {
      paths = await call<string[]>("pick_import_audio_sources", { sourceKind: kind });
      if (!paths.length) return;
      const preview = await call<{ tracks: Track[]; seals: LocalAudioFileSeal[] }>("preview_local_audio_import", { paths });
      tracks = preview.tracks; seals = preview.seals;
      if (!tracks.length) message = "No supported audio files were found.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }

  async function apply() {
    if (portablePreview) {
      if (busy) return;
      if (!portableReviewReady) return;
      if (portableMode !== "create" && !portableTargetCollectionId) {
        message = "Choose the playlist to update or merge into.";
        return;
      }
      busy = true; message = "";
      try {
        const result = await call<{ collection: Collection }>("apply_portable_playlist_import", {
          text,
          previewId: portablePreviewId,
          expectedSourceHash: portableHash,
          destinationName: portableMode === "create" ? portableDestinationName.trim() || null : null,
          destinationFolderPath: portableHasFolderOverride ? portableFolderOverride.trim() || null : null,
          providerSelections: providerOptionsForApply(),
        });
        await onPortableImported(result.collection);
        message = portableMode === "create"
          ? "Portable playlist created with its reviewed structure and entry metadata."
          : portableMode === "update"
            ? "The selected playlist was updated with the reviewed entries."
            : "The reviewed entries were merged into the selected playlist.";
      } catch (error) { message = String(error); }
      finally { busy = false; }
      return;
    }
    if (!tracks.length || busy) return;
    if (!destinationId && !newPlaylistName.trim() && !currentCollectionId) { message = "Choose a destination playlist or enter a new playlist name."; return; }
    busy = true; message = "";
    try {
      const prepared = local ? await call<Track[]>("apply_local_audio_import", { seals }) : tracks;
      await onApply(prepared, { ...applyOptions(), ...(local ? { localAssetsImported: true } : {}) });
      message = `Applied ${prepared.length} reviewed tracks.`;
    }
    catch (error) { message = String(error); }
    finally { busy = false; }
  }
</script>

<section bind:this={sourceSection} class="source-import" aria-labelledby="source-import-title" ondragover={(event) => event.preventDefault()} ondrop={dropFile}>
  <header><h3 id="source-import-title">{title}</h3><p>Review the detected tracks and destination before applying this import.</p></header>
  {#if local}
    {#if desktop}
      <div class="actions"><button type="button" disabled={busy} onclick={() => void chooseAudio("files")}>Choose audio files…</button><button type="button" disabled={busy} onclick={() => void chooseAudio("folder")}>Choose a folder…</button></div>
      {#if paths.length}<p>{paths.length} audio files selected for review.</p>{/if}
    {:else}<p>Local audio import requires the Burrata desktop app.</p>{/if}
  {:else if paste}
    <label>Playlist text<textarea bind:value={text} rows="8" placeholder="Paste Artist — Title tracklist, CSV, or M3U text" oninput={scheduleTextPreview}></textarea></label>
  {:else}
    <label>Playlist or data file<input type="file" accept=".m3u,.m3u8,.csv,.burrata-backup,text/csv,audio/x-mpegurl,application/vnd.apple.mpegurl,application/octet-stream" onchange={loadFile} /></label>
    {#if portableFileName}<small>Selected file: {portableFileName}</small>{/if}
    {#if text}<p>Detected format: <strong>{detectedFormat === "m3u" ? "M3U / M3U8" : detectedFormat === "csv" ? "CSV" : "Tracklist"}</strong></p>{/if}
    {#if text}<details><summary>Source text</summary><pre>{text}</pre></details>{/if}
  {/if}
  {#if !local}<label>Format<select bind:value={format} onchange={() => void previewText()}><option value="auto">Automatic ({detectedFormat})</option><option value="tracklist">Artist and title tracklist</option><option value="csv">CSV / Spotify export</option><option value="m3u">M3U / M3U8</option></select></label>{/if}
  {#if message}<p class="message" role="status">{message}</p>{/if}
  {#if portablePreview}
    <section class="preview portable-preview" aria-label="Portable playlist preview">
      <div class="preview-heading"><strong>{portablePreview.entryCount} entries ready to review</strong><span>{portablePreview.duplicateOccurrences} repeated occurrences retained</span></div>
      <div class="match-summary" aria-label="Matching summary">
        <span>{portablePreview.exactLocalMatches} local asset matches</span>
        <span>{portablePreview.providerMatchSuggestions} provider suggestions</span>
        <span>{portablePreview.ambiguousLocalMatches + portablePreview.providerAmbiguousEntries} ambiguous</span>
        <span>{portablePreview.unresolvedEntries} unresolved</span>
        {#if portablePreview.providerConflictEntries}<span>{portablePreview.providerConflictEntries} provider conflicts</span>{/if}
      </div>
      <p class="projection">{portablePreview.projectedAddedEntries} entries will be added · {portablePreview.projectedSkippedDuplicates} skipped by the selected merge policy.</p>
      {#if portablePreview.targetName}<p>Target: <strong>{portablePreview.targetName}</strong> · {portablePreview.targetEntryCount} existing entries</p>{/if}
      {#if portablePreview.folderAncestry?.length}<p>{portableMode === "create" ? "Source folder path" : "Destination folder path"}: <strong>{portablePreview.folderAncestry.join(" / ")}</strong></p>{/if}
      {#if portablePreview.sections?.length}<p>Sections: {sectionSummary()}</p>{/if}
      <p>Order, repeated entries, sections, occurrence notes, cues, and playback ranges stay attached to their entries.</p>
      {#if portablePreview.entries?.length}
        <details class="entry-review" open>
          <summary>Review entries and provider conflicts ({portablePreview.entries.length})</summary>
          <ol>
            {#each portablePreview.entries.slice(0, 30) as entry}
              <li>
                <div class="entry-title"><strong>{entry.order}. {entryDisplay(entry)}</strong><span class="resolution">{resolutionLabel(entry)}</span></div>
                {#if entry.sectionName}<small>Section: {entry.sectionName}</small>{/if}
                {#if entry.providerTitle || entry.providerArtist || entry.providerVersion}<p class="provider-evidence">Provider listing: {providerEvidence(entry)}</p>{/if}
                {#if entry.relationship}<small>Match relationship: {humanizeEvidence(entry.relationship)}</small>{/if}
                {#if entry.status !== "exactLocal" && entry.providerOptions?.some((option) => option.action !== "Reject")}
                  <label class="provider-selection">Playback source<select value={portableProviderSelections[entry.entryOrder] ?? ""} disabled={busy} onchange={(event) => setPortableProviderSelection(entry, (event.currentTarget as HTMLSelectElement).value)}>
                    <option value="">Keep metadata only</option>
                    {#each (entry.providerOptions ?? []).filter((option) => option.action !== "Reject") as option}
                      <option value={providerOptionKey(option)}>{option.provider} · {option.artist || "Unknown artist"} — {option.title}{option.version ? ` (${option.version})` : ""}{option.durationSeconds ? ` · ${Math.round(option.durationSeconds)} sec` : ""}</option>
                    {/each}
                  </select></label>
                {/if}
                {#if entry.reason}<small>{humanizeEvidence(entry.reason)}</small>{/if}
                {#each entry.conflicts ?? [] as conflict}<small class="conflict">{humanizeEvidence(conflict)}</small>{/each}
              </li>
            {/each}
          </ol>
          {#if portablePreview.entries.length > 30}<small>Showing the first 30 entries. The full reviewed playlist is retained.</small>{/if}
        </details>
      {/if}
      {#each portablePreview.warnings as warning}<p class="warning">{warning}</p>{/each}
    </section>
  {/if}
  {#if portableInput}
    <fieldset><legend>Matching policy</legend>
      <label>Resolve entries<select bind:value={portableMatchingLevel} disabled={busy} onchange={portableReviewChanged}>
        <option value="exact">Exact — require structured performer credit and recording evidence</option>
        <option value="balanced">Balanced — allow reasonable same-recording catalog evidence</option>
        <option value="flexible">Flexible — consider missing guest evidence and identified alternatives</option>
      </select></label>
      <small>Changing this policy reruns matching against registered local assets and configured providers. Selected provider recordings are rechecked before they are bound as playback sources; no audio is downloaded.</small>
    </fieldset>
    <fieldset><legend>Destination and repeat behavior</legend>
      <label>Import mode<select value={portableMode} disabled={busy} onchange={(event) => setPortableMode((event.currentTarget as HTMLSelectElement).value as PortableImportMode)}>
        <option value="create">Create a new playlist</option>
        <option value="update">Update an existing playlist</option>
        <option value="merge">Merge into an existing playlist</option>
      </select></label>
      {#if portableMode === "create"}
        <label>New playlist name<input bind:value={portableDestinationName} disabled={busy} onchange={portableReviewChanged} /></label>
      {:else}
        <label>Playlist to {portableMode === "update" ? "update" : "merge into"}<select bind:value={portableTargetCollectionId} disabled={busy} onchange={portableReviewChanged}>
          <option value="">Choose a playlist</option>
          {#each collections.filter(item => !["favorites", "history", "downloads"].includes(item.id)) as collection}
            <option value={collection.id}>{collection.name}{collection.trackCount ? " · " + collection.trackCount + " entries" : ""}</option>
          {/each}
        </select></label>
        <p>{portableMode === "update" ? "The reviewed playlist contents replace the target entries while keeping its identity." : "Reviewed entries are appended to the target playlist."}</p>
      {/if}
      {#if portableMode === "merge"}
        <label>Repeated entries in the target<select bind:value={portableDuplicatePolicy} disabled={busy} onchange={portableReviewChanged}>
          <option value="keep">Keep every source occurrence</option>
          <option value="skip">Skip entries already in the target</option>
        </select></label>
        <small>Repeated occurrences within the imported file remain in order. Skip applies only to matches already in the target.</small>
      {:else}
        <p>{portableMode === "create" ? "All source occurrences are retained." : "Updating replaces the target contents and retains all source occurrences."}</p>
      {/if}
      <label><input type="checkbox" bind:checked={portableHasFolderOverride} disabled={busy} onchange={portableReviewChanged} /> {portableMode === "create" ? "Place under a different folder path" : "Move the target playlist to a different folder path"}</label>
      {#if portableHasFolderOverride}<label>Destination folder path<input bind:value={portableFolderOverride} disabled={busy} placeholder="Optional, e.g. Road trips/2026" onchange={portableReviewChanged} /></label>{/if}
      {#if portableMode === "create" && !portableHasFolderOverride && portablePreview?.folderAncestry?.length}
        <small>Will use source folder path: {portablePreview.folderAncestry.join(" / ")}</small>
      {/if}
    </fieldset>
    <div class="actions"><button type="button" disabled={busy || !portableReviewReady || (portableMode !== "create" && !portableTargetCollectionId)} onclick={() => void apply()}>{busy ? "Refreshing review…" : portableMode === "create" ? "Create reviewed playlist" : portableMode === "update" ? "Update reviewed playlist" : "Merge reviewed playlist"}</button><button type="button" disabled={busy} onclick={closeImport}>Cancel</button></div>
  {:else if tracks.length}
    <section class="preview" aria-label="Import preview"><strong>{tracks.length} tracks ready to review</strong>{#if repeatedCount}<p>{repeatedCount} repeated entries in this import.</p>{/if}<ol>{#each tracks.slice(0, 12) as track}<li>{track.artist || "Unknown artist"} — {track.title}{track.version ? ` (${track.version})` : ""}</li>{/each}</ol>{#if tracks.length > 12}<small>Showing the first 12 tracks.</small>{/if}</section>
    <fieldset><legend>Destination</legend>
      {#if currentCollectionId}<p>Current playlist: <strong>{collections.find(item => item.id === currentCollectionId)?.name ?? "Selected playlist"}</strong></p>
      {:else}<label>Existing playlist<select bind:value={destinationId}><option value="">Choose later / create new</option>{#each collections.filter(item => !["favorites", "history", "downloads"].includes(item.id)) as collection}<option value={collection.id}>{collection.name}</option>{/each}</select></label>
        <label>Or create a playlist<input bind:value={newPlaylistName} placeholder="New playlist name" /></label>{/if}
      <label>Playlist folder path <input bind:value={folderPath} placeholder="Optional, e.g. Road trips/2026" /></label>
      <label>Section name <input bind:value={sectionName} placeholder="Optional section for these tracks" /></label>
      <label>Duplicate handling<select bind:value={duplicatePolicy}><option value="skip-repeated">Skip repeats and destination duplicates</option><option value="keep">Keep every occurrence</option></select></label>
      {#if duplicatePolicy === "skip-repeated"}<small>Existing destination duplicates are skipped when applied. The count above covers repeats within this source.</small>{/if}
    </fieldset>
    <div class="actions"><button type="button" disabled={busy} onclick={() => void apply()}>Apply import</button><button type="button" disabled={busy} onclick={closeImport}>Cancel</button></div>
  {/if}
</section>

<style>
  .source-import{display:grid;gap:13px;max-width:760px;color:var(--text)}header h3,p{margin:0}header p,.source-import>p{color:var(--muted);font-size:.86rem;line-height:1.45}label{display:grid;gap:5px;font-size:.84rem}input,textarea,select{width:100%;box-sizing:border-box;padding:9px;border:1px solid var(--line);border-radius:7px;background:var(--surface);color:var(--text);font:inherit}textarea{resize:vertical}fieldset{display:grid;gap:10px;border:1px solid var(--line);border-radius:8px;padding:12px}legend{padding:0 5px;font-weight:650}.preview{border:1px solid var(--line);border-radius:8px;padding:11px}.preview ol{margin:8px 0;padding-left:22px}.preview li{padding:7px 0;font-size:.86rem;border-top:1px solid color-mix(in srgb,var(--line) 55%,transparent)}.preview small{color:var(--muted)}.preview-heading,.match-summary,.entry-title{display:flex;align-items:center;gap:8px;flex-wrap:wrap;justify-content:space-between}.preview-heading span,.match-summary span,.resolution{font-size:.78rem;color:var(--muted)}.match-summary{justify-content:flex-start;margin:10px 0;gap:6px}.match-summary span{border:1px solid var(--line);border-radius:999px;padding:3px 8px}.projection{font-weight:650}.entry-review{margin-top:10px}.entry-review summary{cursor:pointer;font-weight:650}.entry-review ol{max-height:380px;overflow:auto}.entry-title{align-items:flex-start}.provider-evidence{margin:4px 0;color:var(--muted);font-size:.82rem}.conflict,.warning{color:var(--warning,#a96b00)}.actions{display:flex;gap:8px;flex-wrap:wrap}.actions button{padding:8px 12px}.message{color:var(--warning,#a96b00);font-size:.86rem}
</style>
