// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ImportSources from "./ImportSources.svelte";
import ImportHub, { type ImportChoice } from "./ImportHub.svelte";
import type { Track } from "./lib/types";

let destroy: (() => void) | undefined;
const initialDevicePixelRatio = window.devicePixelRatio;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; delete window.__TAURI__; Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: initialDevicePixelRatio }); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
async function settlePortable() { await new Promise(resolve => setTimeout(resolve, 500)); await settle(); }
const track = (id: string, title = "Song"): Track => ({ id, provider: "import", providerId: id, sourceUrl: "", artist: "Artist", title, favorite: false, provenance: {} });
const collections = [{ id: "playlist-1", name: "Road trip", trackCount: 2 }];
function mountSource(choice: ImportChoice, call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, onApply = vi.fn(), onBackupSelected = vi.fn(), onBackupPreview = vi.fn()) {
  const instance = mount(ImportSources, { target: document.body, props: { choice, call, collections, desktop: true, onApply, onBackupSelected, onBackupPreview } as never });
  destroy = () => unmount(instance);
  return { onApply, onBackupSelected, onBackupPreview };
}
function button(label: string) { return [...document.querySelectorAll<HTMLButtonElement>("button")].find(item => item.textContent?.trim() === label)!; }
let nativeDropListener: ((event: { event: string; id: number; payload: { token: string; kind: "backup" | "playlist" | "unsupported"; coordinateSpace: "logical" | "physical"; position: { x: number; y: number } } }) => void) | undefined;
const nativeDropUnlisten = vi.fn();
function installNativeDropListener() {
  nativeDropUnlisten.mockClear();
  nativeDropListener = undefined;
  Object.defineProperty(window, "__TAURI__", { configurable: true, value: { event: { listen: vi.fn(async (_event: string, handler: typeof nativeDropListener) => { nativeDropListener = handler!; return nativeDropUnlisten; }) } } });
}
function setImportBounds(left = 10, top = 20, right = 210, bottom = 220) {
  Object.defineProperty(document.querySelector<HTMLElement>(".source-import")!, "getBoundingClientRect", { configurable: true, value: () => ({ left, top, right, bottom, width: right - left, height: bottom - top, x: left, y: top, toJSON: () => ({}) }) });
}
async function emitNativeDrop(token = "drop-token", position = { x: 80, y: 90 }, kind: "backup" | "playlist" | "unsupported" = "playlist", coordinateSpace: "logical" | "physical" = "logical") {
  expect(nativeDropListener).toBeTruthy();
  nativeDropListener!({ event: "import-file-drop", id: 1, payload: { token, kind, coordinateSpace, position } });
  await settle();
}

describe("ImportSources", () => {
  it("previews pasted tracks without persistence, then applies to a selected destination", async () => {
    const parsed = [track("one"), track("two", "Other")];
    const call = vi.fn(async (command: string) => command === "preview_import_tracks" ? parsed : []);
    const { onApply } = mountSource("paste-playlist", call);
    const textarea = document.querySelector("textarea")!;
    textarea.value = "Artist — Song\nArtist — Other";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(call).toHaveBeenCalledWith("preview_import_tracks", { text: textarea.value, kind: "tracklist" });
    expect(document.body.textContent).toContain("2 tracks ready to review");
    expect(onApply).not.toHaveBeenCalled();
    expect(call).not.toHaveBeenCalledWith("import_tracks", expect.anything());
    const destination = document.querySelector<HTMLSelectElement>("fieldset select")!;
    destination.value = "playlist-1"; destination.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const policyFields = [...document.querySelectorAll<HTMLSelectElement>("fieldset select")];
    const policy = policyFields[policyFields.length - 1];
    policy.value = "keep"; policy.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    button("Apply import").click(); await settle();
    expect(onApply).toHaveBeenCalledWith(parsed, { destinationId: "playlist-1", newPlaylistName: "", folderPath: "", sectionName: "", duplicatePolicy: "keep" });
  });

  it("detects a playlist file format and retains its text when preview fails", async () => {
    const call = vi.fn(async () => { throw new Error("bad row 2"); });
    mountSource("playlist-file", call);
    const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
    const file = new File(["#EXTM3U\n#EXTINF:180,Artist - Song"], "set.m3u8");
    Object.defineProperty(file, "text", { value: async () => "#EXTM3U\n#EXTINF:180,Artist - Song" });
    Object.defineProperty(input, "files", { configurable: true, value: [file] });
    input.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(document.body.textContent).toContain("M3U / M3U8");
    expect(document.body.textContent).toContain("Selected file: set.m3u8");
    expect(document.body.textContent).toContain("bad row 2");
    expect(document.body.textContent).toContain("#EXTINF:180,Artist - Song");
    expect(call).toHaveBeenCalledWith("preview_import_tracks", { text: "#EXTM3U\n#EXTINF:180,Artist - Song", kind: "m3u" });
  });

  it("routes Burrata playlists through the typed, hash-reviewed path and keeps occurrences by default", async () => {
    const portable = "#EXTM3U\n#BURRATA-PLAYLIST:{\"version\":1}";
    const preview = { previewId: "review-1", sourceHash: "sha256:reviewed", name: "Foldered mix", entryCount: 3, duplicateOccurrences: 1, exactLocalMatches: 1, ambiguousLocalMatches: 0, unresolvedEntries: 1, providerMatchSuggestions: 1, providerAmbiguousEntries: 1, providerConflictEntries: 1, targetEntryCount: 0, projectedAddedEntries: 2, projectedSkippedDuplicates: 1, folderAncestry: ["Sets", "Night"], sections: [{ name: "Warm up", entryCount: 3 }], entries: [{ order: 1, title: "Song", artists: ["Artist"], version: "Club Mix", sectionName: "Warm up", status: "providerConflict", provider: "youtube", providerTitle: "Song (Live)", relationship: "balanced-title-performer", conflicts: ["Provider version differs"] }], warnings: [] };
    const call = vi.fn(async (command: string) => command === "preview_portable_playlist_import" ? preview : command === "apply_portable_playlist_import" ? { collection: { id: "created", name: "Foldered mix", trackCount: 3 } } : undefined);
    const { onApply } = mountSource("paste-playlist", call);
    const textarea = document.querySelector("textarea")!;
    textarea.value = portable; textarea.dispatchEvent(new Event("input", { bubbles: true })); await settlePortable();
    expect(call).toHaveBeenCalledWith("preview_portable_playlist_import", { text: portable, options: { matchingLevel: "balanced", mode: "create", targetCollectionId: null, duplicatePolicy: "keep", destinationName: null, destinationFolderPath: null } });
    expect(call).not.toHaveBeenCalledWith("preview_import_tracks", expect.anything());
    expect(document.body.textContent).toContain("3 entries ready to review");
    expect(document.body.textContent).toContain("Order, repeated entries, sections, occurrence notes, cues, and playback ranges stay attached to their entries.");
    expect(document.body.textContent).toContain("Provider conflict");
    expect(document.body.textContent).toContain("Provider version differs");
    expect(document.body.textContent).toContain("2 entries will be added");
    expect(document.body.textContent).toContain("1 repeated occurrences retained");
    button("Create reviewed playlist").click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_portable_playlist_import", { text: portable, previewId: "review-1", expectedSourceHash: "sha256:reviewed", destinationName: "Foldered mix", destinationFolderPath: null, providerSelections: [] });
    expect(onApply).not.toHaveBeenCalled();
  });

  it("previews update and merge targets and keeps source text when apply rejects a stale target", async () => {
    const portable = "#EXTM3U\n#BURRATA-PLAYLIST:{\"version\":1}";
    const preview = { previewId: "target-review", sourceHash: "sha256:target", name: "Imported", entryCount: 2, duplicateOccurrences: 0, exactLocalMatches: 1, ambiguousLocalMatches: 0, unresolvedEntries: 1, providerMatchSuggestions: 0, providerAmbiguousEntries: 0, providerConflictEntries: 0, targetEntryCount: 2, projectedAddedEntries: 2, projectedSkippedDuplicates: 0, warnings: [] };
    const call = vi.fn(async (command: string) => {
      if (command === "preview_portable_playlist_import") return preview;
      if (command === "apply_portable_playlist_import") throw new Error("Target changed; review again");
      return undefined;
    });
    mountSource("paste-playlist", call);
    const textarea = document.querySelector("textarea")!;
    textarea.value = portable; textarea.dispatchEvent(new Event("input", { bubbles: true })); await settlePortable();

    const matching = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "exact"))!;
    matching.value = "exact"; matching.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(call).toHaveBeenCalledWith("preview_portable_playlist_import", { text: portable, options: { matchingLevel: "exact", mode: "create", targetCollectionId: null, duplicatePolicy: "keep", destinationName: "Imported", destinationFolderPath: null } });
    matching.value = "balanced"; matching.dispatchEvent(new Event("change", { bubbles: true })); await settle();

    const mode = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "update"))!;
    mode.value = "update"; mode.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const target = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "playlist-1"))!;
    target.value = "playlist-1"; target.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(call).toHaveBeenCalledWith("preview_portable_playlist_import", { text: portable, options: { matchingLevel: "balanced", mode: "update", targetCollectionId: "playlist-1", duplicatePolicy: "keep", destinationName: null, destinationFolderPath: null } });
    button("Update reviewed playlist").click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_portable_playlist_import", { text: portable, previewId: "target-review", expectedSourceHash: "sha256:target", destinationName: null, destinationFolderPath: null, providerSelections: [] });
    expect(document.body.textContent).toContain("Target changed; review again");
    expect(textarea.value).toBe(portable);

    const modeForMerge = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "update"))!;
    modeForMerge.value = "merge"; modeForMerge.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const duplicate = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "skip"))!;
    duplicate.value = "skip"; duplicate.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(call).toHaveBeenCalledWith("preview_portable_playlist_import", { text: portable, options: { matchingLevel: "balanced", mode: "merge", targetCollectionId: "playlist-1", duplicatePolicy: "skip", destinationName: null, destinationFolderPath: null } });
    expect(document.body.textContent).toContain("Skip applies only to matches already in the target.");
  });

  it("keeps update and merge controls available after a missing-target preview fails, then applies a fresh review", async () => {
    const portable = "#EXTM3U\n#BURRATA-PLAYLIST:{\"version\":1}";
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "preview_portable_playlist_import") {
        const options = args?.options as { mode: string; targetCollectionId: string | null };
        if (options.mode !== "create" && !options.targetCollectionId) throw new Error("Choose an existing playlist to update or merge into.");
        return { previewId: `${options.mode}-fresh`, sourceHash: "fresh-hash", name: "Imported", entryCount: 1, duplicateOccurrences: 0, exactLocalMatches: 0, ambiguousLocalMatches: 0, unresolvedEntries: 1, providerMatchSuggestions: 0, providerAmbiguousEntries: 0, providerConflictEntries: 0, targetEntryCount: 1, projectedAddedEntries: 1, projectedSkippedDuplicates: 0, warnings: [] };
      }
      if (command === "apply_portable_playlist_import") return { collection: { id: "playlist-1", name: "Road trip", trackCount: 1 } };
      return undefined;
    });
    mountSource("paste-playlist", call);
    const textarea = document.querySelector("textarea")!;
    textarea.value = portable; textarea.dispatchEvent(new Event("input", { bubbles: true })); await settlePortable();

    for (const modeName of ["update", "merge"] as const) {
      const previousTarget = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === "playlist-1"));
      if (previousTarget) { previousTarget.value = ""; previousTarget.dispatchEvent(new Event("change", { bubbles: true })); await settlePortable(); }
      const mode = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => [...select.options].some(option => option.value === modeName))!;
      mode.value = modeName; mode.dispatchEvent(new Event("change", { bubbles: true })); await settle();
      const label = modeName === "update" ? "Playlist to update" : "Playlist to merge into";
      let destination = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => select.closest("label")?.textContent?.includes(label))!;
      expect(destination).toBeTruthy();
      expect(document.body.textContent).toContain("Choose an existing playlist");
      const applyLabel = modeName === "update" ? "Update reviewed playlist" : "Merge reviewed playlist";
      expect(button(applyLabel).disabled).toBe(true);
      destination.value = "playlist-1"; destination.dispatchEvent(new Event("change", { bubbles: true })); await settlePortable();
      destination = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => select.closest("label")?.textContent?.includes(label))!;
      expect(button(applyLabel).disabled).toBe(false);
      button(applyLabel).click(); await settle();
      expect(call).toHaveBeenCalledWith("apply_portable_playlist_import", expect.objectContaining({ text: portable, previewId: `${modeName}-fresh`, expectedSourceHash: "fresh-hash" }));
    }
  });

  it("lets the reviewer choose a provider source and submits only its stable identity", async () => {
    const portable = "#EXTM3U\n#BURRATA-PLAYLIST:{\"version\":1}";
    const preview = {
      previewId: "provider-review", sourceHash: "sha256:provider", name: "Imported", entryCount: 1,
      duplicateOccurrences: 0, exactLocalMatches: 0, ambiguousLocalMatches: 0, unresolvedEntries: 1,
      providerMatchSuggestions: 1, providerAmbiguousEntries: 0, providerConflictEntries: 0,
      targetEntryCount: 0, projectedAddedEntries: 1, projectedSkippedDuplicates: 0, warnings: [],
      entries: [{ entryOrder: 1, order: 1, title: "Song", artists: ["Artist"], status: "providerSuggestion", conflicts: [],
        providerOptions: [
          { provider: "youtube", sourceId: "video-1", title: "Song", artist: "Artist", relationship: "SameRecording", action: "Accept", evidence: [] },
          { provider: "soundcloud", sourceId: "track-2", title: "Song live", artist: "Artist", relationship: "PossibleAlternative", action: "Accept", evidence: [] },
        ], suggestedSelection: { entryOrder: 1, provider: "youtube", sourceId: "video-1" } }],
    };
    const call = vi.fn(async (command: string, _args?: Record<string, unknown>) => command === "preview_portable_playlist_import" ? preview : command === "apply_portable_playlist_import" ? { collection: { id: "created", name: "Imported", trackCount: 1 } } : undefined);
    mountSource("paste-playlist", call);
    const textarea = document.querySelector("textarea")!;
    textarea.value = portable; textarea.dispatchEvent(new Event("input", { bubbles: true })); await settlePortable();
    const source = [...document.querySelectorAll<HTMLSelectElement>("select")].find(select => select.closest("label")?.textContent?.includes("Playback source"))!;
    expect(source.value).toContain("youtube");
    source.value = [...source.options].find(option => option.textContent?.includes("soundcloud"))!.value;
    source.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    button("Create reviewed playlist").click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_portable_playlist_import", expect.objectContaining({
      providerSelections: [{ entryOrder: 1, provider: "soundcloud", sourceId: "track-2" }],
    }));
    const applyArgs = call.mock.calls.find(([command]) => command === "apply_portable_playlist_import")?.[1];
    expect(JSON.stringify(applyArgs)).not.toContain("https://");
  });

  it("routes a Burrata backup file to Restore Backup without parsing or consuming its contents", async () => {
    const onBackupSelected = vi.fn();
    const call = vi.fn(async () => []);
    mountSource("playlist-file", call, vi.fn(), onBackupSelected);
    const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
    const file = new File(["opaque backup bytes"], "library.burrata-backup");
    Object.defineProperty(input, "files", { configurable: true, value: [file] });
    input.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(onBackupSelected).toHaveBeenCalledOnce();
    expect(call).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("Backup file selected. Opening Restore Backup…");
  });

  it("routes dropped Burrata backup files to Restore Backup", async () => {
    const onBackupSelected = vi.fn();
    const call = vi.fn(async () => []);
    mountSource("playlist-file", call, vi.fn(), onBackupSelected);
    const file = new File(["opaque backup bytes"], "dropped.burrata-backup");
    const zone = document.querySelector<HTMLElement>(".source-import")!;
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { files: [file] } });
    zone.dispatchEvent(event); await settle();
    expect(event.defaultPrevented).toBe(true);
    expect(onBackupSelected).toHaveBeenCalledOnce();
    expect(call).not.toHaveBeenCalled();
  });

  it("consumes an in-zone native drop and previews the returned playlist text", async () => {
    installNativeDropListener();
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
    const content = "#EXTM3U\n#EXTINF:180,Artist - Song";
    const parsed = [track("native-song")];
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => command === "consume_import_drop" ? { kind: "playlist", name: "dropped.m3u", text: content } : command === "preview_import_tracks" ? parsed : undefined);
    mountSource("playlist-file", call);
    await settle(); setImportBounds();
    await emitNativeDrop();
    expect(call).toHaveBeenCalledWith("consume_import_drop", { token: "drop-token" });
    expect(call).toHaveBeenCalledWith("preview_import_tracks", { text: content, kind: "m3u" });
    expect(document.body.textContent).toContain("Selected file: dropped.m3u");
    expect(document.body.textContent).toContain("1 tracks ready to review");
    expect(call).not.toHaveBeenCalledWith("import_tracks", expect.anything());
  });

  it("does not consume a native drop outside the mounted import section", async () => {
    installNativeDropListener();
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
    const call = vi.fn(async () => undefined);
    mountSource("playlist-file", call);
    await settle(); setImportBounds();
    await emitNativeDrop("outside-token", { x: 400, y: 400 });
    expect(call).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain("Selected file:");
  });

  it("suppresses a matching DOM echo after a native drop", async () => {
    installNativeDropListener();
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
    const content = "#EXTM3U\n#EXTINF:180,Artist - Song";
    const call = vi.fn(async (command: string) => command === "consume_import_drop" ? { kind: "playlist", name: "native.m3u", text: content } : command === "preview_import_tracks" ? [track("native-song")] : undefined);
    mountSource("playlist-file", call);
    await settle(); setImportBounds(0, 0, 100, 100);
    await emitNativeDrop("native-token", { x: 20, y: 30 }, "playlist", "logical");
    const file = new File([content], "echo.m3u");
    Object.defineProperty(file, "text", { value: async () => content });
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperties(event, { clientX: { value: 20 }, clientY: { value: 30 }, dataTransfer: { value: { files: [file] } } });
    document.querySelector<HTMLElement>(".source-import")!.dispatchEvent(event); await settle();
    expect(event.defaultPrevented).toBe(true);
    expect(call.mock.calls.filter(([command]) => command === "consume_import_drop")).toHaveLength(1);
    expect(call.mock.calls.filter(([command]) => command === "preview_import_tracks")).toHaveLength(1);
    expect(document.body.textContent).toContain("Selected file: native.m3u");
  });

  it("consumes a native DOM echo token but applies only the already-previewed DOM file", async () => {
    installNativeDropListener();
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
    const content = "Artist — DOM song";
    const call = vi.fn(async (command: string) => command === "consume_import_drop" ? { kind: "playlist", name: "native.m3u", text: "Artist — Native song" } : command === "preview_import_tracks" ? [track("dom-song")] : undefined);
    mountSource("playlist-file", call);
    await settle(); setImportBounds(0, 0, 100, 100);
    const file = new File([content], "dom.txt");
    Object.defineProperty(file, "text", { value: async () => content });
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperties(event, { clientX: { value: 20 }, clientY: { value: 30 }, dataTransfer: { value: { files: [file] } } });
    document.querySelector<HTMLElement>(".source-import")!.dispatchEvent(event); await settle();
    await emitNativeDrop("native-echo", { x: 40, y: 60 }, "playlist", "physical");
    expect(call.mock.calls.filter(([command]) => command === "consume_import_drop")).toHaveLength(1);
    expect(call.mock.calls.filter(([command]) => command === "preview_import_tracks")).toHaveLength(1);
    expect(document.body.textContent).toContain("Selected file: dom.txt");
  });

  it("routes a consumed native backup preview directly to review", async () => {
    installNativeDropListener();
    const preview = { previewId: "drop-review", schemaVersion: 1, createdAt: "2026-10-01T00:00:00Z", components: [], media: { included: false, assets: 0, missing: 0, changed: 0 }, warnings: [] };
    const call = vi.fn(async (command: string) => command === "consume_import_drop" ? { kind: "backup", preview } : undefined);
    const { onBackupPreview } = mountSource("playlist-file", call);
    await settle(); setImportBounds();
    await emitNativeDrop("backup-token");
    expect(call).toHaveBeenCalledWith("consume_import_drop", { token: "backup-token" });
    expect(onBackupPreview).toHaveBeenCalledWith(preview);
    expect(call).not.toHaveBeenCalledWith("inspect_backup", expect.anything());
  });

  it("leaves native backup drops to the App-level backup router", async () => {
    installNativeDropListener();
    const call = vi.fn(async () => undefined);
    const { onBackupPreview } = mountSource("playlist-file", call);
    await settle(); setImportBounds();
    await emitNativeDrop("backup-token", { x: 80, y: 90 }, "backup");
    expect(call).not.toHaveBeenCalled();
    expect(onBackupPreview).not.toHaveBeenCalled();
  });

  it("shows native drop consume errors without applying an import", async () => {
    installNativeDropListener();
    const call = vi.fn(async () => { throw new Error("Dropped item expired"); });
    mountSource("playlist-file", call);
    await settle(); setImportBounds();
    await emitNativeDrop("expired-token");
    expect(document.body.textContent).toContain("Could not read dropped file: Error: Dropped item expired");
    expect(call).toHaveBeenCalledTimes(1);
    expect(call).toHaveBeenCalledWith("consume_import_drop", { token: "expired-token" });
  });

  it("ignores a native drop result that resolves after the import flow closes", async () => {
    installNativeDropListener();
    let resolveConsume: ((value: unknown) => void) | undefined;
    const preview = { previewId: "stale-review", schemaVersion: 1, createdAt: "2026-10-01T00:00:00Z", components: [], media: { included: false, assets: 0, missing: 0, changed: 0 }, warnings: [] };
    const call = vi.fn(() => new Promise(resolve => { resolveConsume = resolve; }));
    const { onBackupPreview } = mountSource("playlist-file", call as never);
    await settle(); setImportBounds();
    nativeDropListener!({ event: "import-file-drop", id: 1, payload: { token: "stale-token", kind: "playlist", coordinateSpace: "logical", position: { x: 80, y: 90 } } });
    await settle();
    destroy?.(); destroy = undefined;
    resolveConsume?.({ kind: "backup", preview });
    await settle();
    expect(onBackupPreview).not.toHaveBeenCalled();
    expect(nativeDropUnlisten).toHaveBeenCalledOnce();
  });

  it("unsubscribes if the import flow closes before asynchronous listener setup finishes", async () => {
    let resolveListen: ((stop: () => void) => void) | undefined;
    const stop = vi.fn();
    Object.defineProperty(window, "__TAURI__", { configurable: true, value: { event: { listen: vi.fn(() => new Promise(resolve => { resolveListen = resolve; })) } } });
    mountSource("playlist-file", vi.fn(async () => undefined));
    await settle();
    expect(resolveListen).toBeTruthy();
    destroy?.(); destroy = undefined;
    resolveListen?.(stop);
    await settle();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("uses native local file selection and delays asset import until Apply", async () => {
    const localTrack = { ...track("local-preview:file", "Local song"), provider: "local", localPath: "/music/song.flac" };
    const seals = [{ path: "/music/song.flac", sizeBytes: "128", modifiedNs: "10", fileIdentity: "dev:ino" }];
    const call = vi.fn(async (command: string) => {
      if (command === "pick_import_audio_sources") return ["/music/song.flac"];
      if (command === "preview_local_audio_import") return { tracks: [localTrack], seals };
      if (command === "apply_local_audio_import") return [localTrack];
      return [];
    });
    const { onApply } = mountSource("local-audio", call);
    button("Choose audio files…").click(); await settle();
    expect(call).toHaveBeenCalledWith("pick_import_audio_sources", { sourceKind: "files" });
    expect(call).toHaveBeenCalledWith("preview_local_audio_import", { paths: ["/music/song.flac"] });
    expect(document.body.textContent).toContain("1 audio files selected for review");
    expect(onApply).not.toHaveBeenCalled();
    expect(call).not.toHaveBeenCalledWith("apply_local_audio_import", expect.anything());
    expect(call).not.toHaveBeenCalledWith("import_local_file", expect.anything());
    button("Choose a folder…").click(); await settle();
    expect(call).toHaveBeenCalledWith("pick_import_audio_sources", { sourceKind: "folder" });
    expect(call).toHaveBeenCalledWith("preview_local_audio_import", { paths: ["/music/song.flac"] });
    document.querySelector<HTMLInputElement>("input[placeholder='New playlist name']")!.value = "Local set";
    document.querySelector<HTMLInputElement>("input[placeholder='New playlist name']")!.dispatchEvent(new Event("input", { bubbles: true }));
    button("Apply import").click(); await settle();
    expect(call).toHaveBeenCalledWith("apply_local_audio_import", { seals });
    expect(onApply).toHaveBeenCalledWith([localTrack], expect.objectContaining({ newPlaylistName: "Local set", localAssetsImported: true }));
  });

  it("keeps the hub choices distinct and routes each choice by its source type", async () => {
    const onChoose = vi.fn();
    const instance = mount(ImportHub, { target: document.body, props: { onChoose } as never });
    destroy = () => unmount(instance);
    for (const choice of ["playlist-file", "paste-playlist", "import-url", "local-audio"] as ImportChoice[]) {
      [...document.querySelectorAll<HTMLButtonElement>("button")].find(item => item.textContent?.includes(({ "playlist-file": "Choose a playlist file", "paste-playlist": "Paste a tracklist", "import-url": "Import a playlist or album link", "local-audio": "Add music files or a folder" } as Record<ImportChoice, string>)[choice]) )!.click();
    }
    expect(onChoose.mock.calls.map(([choice]) => choice)).toEqual(["playlist-file", "paste-playlist", "import-url", "local-audio"]);
  });
});
