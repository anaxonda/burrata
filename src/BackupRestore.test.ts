// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match Svelte's browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import BackupRestore, { type BackupInspection, type BackupMediaRootPreview, type BackupProgress } from "./BackupRestore.svelte";
import ImportHub from "./ImportHub.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

const root: BackupMediaRootPreview = {
  rootKey: "root-1", label: "Music", assets: 2, requiredBytes: 2048,
  destinationToken: null, destinationLabel: null, reusableSameHash: 0,
  copyCount: 2, conflicts: 0, missingDestination: true, availableBytes: 1024,
  lyricsSidecars: 1, lyricsCopyCount: 1, lyricsReusableSameHash: 0, lyricsConflicts: 0,
};
const archive: BackupInspection = {
  previewId: "preview-1", schemaVersion: 1, createdAt: "2026-10-01T00:00:00Z",
  components: [
    { id: "library", bytes: 512, records: 3, description: "Library" },
    { id: "history", bytes: 64, records: 2, description: "History" },
    { id: "settings", bytes: 80, records: 4, description: "Settings" },
    { id: "playback", bytes: 32, records: 1, description: "Playback" },
    { id: "media:abc", bytes: 2048, records: 2, description: "Audio files" },
  ],
  media: { included: true, assets: 2, missing: 0, changed: 0 }, mediaRoots: [root], warnings: [], archiveBytes: 4096,
  summary: { importedRecordings: 4, importedCollections: 2, importedPlaylistOccurrences: 8, importedHistoryEvents: 5, skippedConflicts: 3, unavailableAssets: 1, omittedStaleAnalyses: 2 },
  settingsProjection: { omittedCacheRows: 6, omittedJobs: 2, omittedPrivateSettings: 4, excludedApplicationFields: 5, assetPathHints: 3 },
};
function mountRestore(overrides: Record<string, unknown> = {}) {
  const props = {
    inspectBackup: vi.fn().mockResolvedValue(archive),
    createBackup: vi.fn().mockResolvedValue(undefined),
    restoreBackup: vi.fn().mockResolvedValue({ restartRequired: true }),
    chooseMediaRoot: vi.fn().mockResolvedValue({ ...root, destinationToken: "opaque-root", destinationLabel: "Music" }),
    ...overrides,
  };
  const instance = mount(BackupRestore, { target: document.body, props: props as never });
  destroy = () => unmount(instance);
  return props;
}
async function inspect() {
  [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Choose backup"))!.click();
  await settle();
}

describe("BackupRestore", () => {
  it("opens a supplied native-drop inspection directly in review without reopening the picker", async () => {
    const inspectBackup = vi.fn().mockResolvedValue(null);
    const props = mountRestore({ initialPreview: archive, inspectBackup });
    await settle();
    expect(document.querySelector('[aria-label="Backup preview"]')?.textContent).toContain("5 components");
    expect(document.body.textContent).toContain("Merge into current library");
    expect(document.querySelectorAll<HTMLInputElement>('fieldset input[type="checkbox"]:checked').length).toBe(4);
    expect(inspectBackup).not.toHaveBeenCalled();
    expect(props.restoreBackup).not.toHaveBeenCalled();
  });

  it("previews archive contents and does not restore before an explicit selection and commit", async () => {
    const props = mountRestore();
    await inspect();
    expect(document.querySelector('[aria-label="Backup preview"]')?.textContent).toContain("5 components");
    expect(document.body.textContent).toContain("Restore a backup");
    expect(props.restoreBackup).not.toHaveBeenCalled();
    const cancel = [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === "Cancel")!;
    cancel.click(); await settle();
    expect(document.querySelector('[aria-label="Backup preview"]')).toBeNull();
    expect(props.restoreBackup).not.toHaveBeenCalled();
  });

  it("submits only selected components and the chosen opaque media root mapping", async () => {
    const props = mountRestore();
    await inspect();
    const settingsLabel = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Settings"))!;
    const settingsBox = settingsLabel.querySelector<HTMLInputElement>("input")!;
    settingsBox.click();
    for (const name of ["History", "Playback"]) {
      const label = [...document.querySelectorAll("label")].find(item => item.textContent?.includes(name))!;
      label.querySelector<HTMLInputElement>("input")!.click();
    }
    const mediaLabel = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Restore verified audio files"))!;
    mediaLabel.querySelector<HTMLInputElement>("input")!.click(); await settle();
    const choose = [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Choose folder"))!;
    choose.click(); await settle();
    const commit = [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === "Merge selected data")!;
    commit.click(); await settle();
    expect(props.chooseMediaRoot).toHaveBeenCalledWith("preview-1", "root-1");
    expect(props.restoreBackup).toHaveBeenCalledWith("preview-1", {
      mode: "merge", componentIds: ["library", "media:abc"],
      pathMappings: [{ rootKey: "root-1", destinationToken: "opaque-root" }],
    }, expect.any(Function));
  });

  it("maps hinted folders for a metadata-only library without selecting media or sidecars", async () => {
    const metadataOnly: BackupInspection = {
      ...archive,
      components: archive.components.filter(component => !component.id.startsWith("media:") && !component.id.startsWith("aux-lyrics:")),
      media: { included: false, assets: 0, missing: 0, changed: 0 },
      mediaRoots: [{ ...root, assets: 2, requiredBytes: 0, reusableSameHash: 1, copyCount: 0, conflicts: 0, lyricsSidecars: 0 }],
      settingsProjection: { ...archive.settingsProjection!, assetPathHints: 2 },
    };
    const previewRestore = vi.fn().mockResolvedValue({ unavailableAssets: 1 });
    const props = mountRestore({
      inspectBackup: vi.fn().mockResolvedValue(metadataOnly),
      chooseMediaRoot: vi.fn().mockResolvedValue({ ...metadataOnly.mediaRoots![0], destinationToken: "opaque-root", destinationLabel: "Music" }),
      previewRestore,
    });
    await inspect();

    expect(document.body.textContent).toContain("No audio files are included in this backup.");
    expect(document.body.textContent).toContain("No audio files or lyrics sidecars will be copied.");
    expect([...document.querySelectorAll("label")].find(label => label.textContent?.includes("Restore verified audio files"))?.querySelector("input")?.disabled).toBe(true);
    [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Link existing audio"))!.click();
    await settle();

    const expectedMapping = [{ rootKey: "root-1", destinationToken: "opaque-root" }];
    expect(props.chooseMediaRoot).toHaveBeenCalledWith("preview-1", "root-1");
    expect(previewRestore).toHaveBeenLastCalledWith("preview-1", {
      mode: "merge", componentIds: ["library", "history", "settings", "playback"], pathMappings: expectedMapping,
    });
    expect(document.body.textContent).toContain("1 matching audio file found in this folder");

    [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === "Merge selected data")!.click();
    await settle();
    expect(props.restoreBackup).toHaveBeenCalledWith("preview-1", {
      mode: "merge", componentIds: ["library", "history", "settings", "playback"], pathMappings: expectedMapping,
    }, expect.any(Function));
  });

  it("requires the replacement acknowledgement before committing selected data", async () => {
    const props = mountRestore({ allowReplace: true });
    await inspect();
    const replace = document.querySelector<HTMLInputElement>('input[name="restore-mode"][value="replace"]')!;
    expect(replace.disabled).toBe(false);
    replace.click(); await settle();
    const commit = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Replace selected data")!;
    expect(commit.disabled).toBe(true);
    commit.click(); await settle();
    expect(props.restoreBackup).not.toHaveBeenCalled();
    document.querySelector<HTMLInputElement>(".acknowledge input")!.click(); await settle();
    expect(commit.disabled).toBe(false);
    commit.click(); await settle();
    expect(props.restoreBackup).toHaveBeenCalledTimes(1);
    expect(props.restoreBackup).toHaveBeenCalledWith("preview-1", {
      mode: "replace", componentIds: ["library", "history", "settings", "playback"], pathMappings: [],
    }, expect.any(Function));
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel automatic backup") || button.textContent?.includes("Cancel operation"))).toBe(false);
  });

  it("returns to merge and clears acknowledgement if a required replace component is removed", async () => {
    const props = mountRestore({ allowReplace: true });
    await inspect();
    const replace = document.querySelector<HTMLInputElement>('input[name="restore-mode"][value="replace"]')!;
    replace.click(); await settle();
    const acknowledgement = document.querySelector<HTMLInputElement>(".acknowledge input")!;
    acknowledgement.click(); await settle();
    const playback = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Playback"))!;
    playback.querySelector<HTMLInputElement>("input")!.click(); await settle();
    expect(document.querySelector<HTMLInputElement>('input[name="restore-mode"][value="merge"]')!.checked).toBe(true);
    expect(document.querySelector(".acknowledge")).toBeNull();
    const commit = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Merge selected data")!;
    expect(commit.disabled).toBe(false);
    commit.click(); await settle();
    expect(props.restoreBackup).toHaveBeenCalledWith("preview-1", {
      mode: "merge", componentIds: ["library", "history", "settings"], pathMappings: [],
    }, expect.any(Function));
  });

  it("retains the preview, selected data, and media choice after a failed restore", async () => {
    const props = mountRestore({ restoreBackup: vi.fn().mockRejectedValue(new Error("disk full")) });
    await inspect();
    const mediaLabel = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Restore verified audio files"))!;
    mediaLabel.querySelector<HTMLInputElement>("input")!.click(); await settle();
    [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Choose folder"))!.click(); await settle();
    [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === "Merge selected data")!.click(); await settle();
    expect(document.querySelector('[aria-label="Backup preview"]')).not.toBeNull();
    expect(document.body.textContent).toContain("Restore did not finish: Error: disk full");
    expect((mediaLabel.querySelector<HTMLInputElement>("input")!).checked).toBe(true);
    expect(document.body.textContent).toContain("Folder: Music");
    expect(props.restoreBackup).toHaveBeenCalledTimes(1);
  });

  it("shows restore counts, conflicts, and insufficient destination space before applying", async () => {
    mountRestore();
    await inspect();
    const summary = document.querySelector('[aria-label="Restore preview summary"]')!;
    expect(summary.textContent).toContain("4 recordings");
    expect(summary.textContent).toContain("3 conflicts skipped");
    expect(summary.textContent).toContain("5 history events");
    expect(document.querySelector('[aria-label="Backup settings coverage"]')?.textContent).toContain("5 device-specific fields");
    const mediaLabel = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Restore verified audio files"))!;
    mediaLabel.querySelector<HTMLInputElement>("input")!.click(); await settle();
    expect(document.body.textContent).toMatch(/1 KB available\s*·\s*not enough space for all selected files/);
    expect(document.body.textContent).toContain("1 verified lyrics sidecars");
  });

  it("refreshes restore counts when the component selection changes", async () => {
    const previewRestore = vi.fn().mockResolvedValue({ importedRecordings: 0, skippedConflicts: 1 });
    mountRestore({ previewRestore });
    await inspect();
    const libraryLabel = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Library"))!;
    libraryLabel.querySelector<HTMLInputElement>("input")!.click(); await settle();
    expect(previewRestore).toHaveBeenCalledWith("preview-1", {
      mode: "merge", componentIds: ["history", "settings", "playback"], pathMappings: [],
    });
    expect(document.querySelector('[aria-label="Restore preview summary"]')?.textContent).toContain("1 conflicts skipped");
  });

  it("reports verified lyrics sidecars in the optional media size estimate", async () => {
    const estimateBackup = vi.fn().mockResolvedValue({
      estimatedBytes: 10240, registeredAssets: 2, eligibleMediaAssets: 2,
      missingAssets: 0, changedAssets: 0, mediaIncluded: true,
      eligibleSidecars: 1, missingSidecars: 0, changedSidecars: 0, sidecarBytes: 256,
    });
    mountRestore({ estimateBackup });
    const include = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Include verified audio files"))!;
    include.querySelector<HTMLInputElement>("input")!.click(); await settle();
    expect(estimateBackup).toHaveBeenCalledWith(true);
    expect(document.body.textContent).toContain("1 verified lyrics sidecars (1 KB)");
  });

  it("edits and saves custom schedule, media inclusion, and reports status and archive size", async () => {
    const saveSchedule = vi.fn(async schedule => ({ ...schedule }));
    const loadSchedule = vi.fn().mockResolvedValue({
      enabled: true, destination: "/backup", interval: "custom", intervalDays: 5, includeMedia: true,
      destinationAvailableBytes: 2048,
      retentionDaily: 7, retentionWeekly: 4, lastSuccessAt: "2026-10-20T12:00:00Z",
      nextDueAt: "2026-10-25T12:00:00Z", lastArchiveBytes: 1048576,
      retryAfterAt: "2026-10-26T12:00:00Z", lastStatus: "waiting", lastError: "Disk unavailable",
    });
    mountRestore({ loadSchedule, saveSchedule, chooseScheduleDestination: vi.fn() });
    await settle();
    expect(document.body.textContent).toContain("Repeat every (days)");
    expect(document.body.textContent).toContain("Next backup due:");
    expect(document.body.textContent).toContain("Last archive size: 1.0 MB");
    expect(document.body.textContent).toContain("Backup folder space available: 2 KB");
    expect(document.body.textContent).toContain("Retry after:");
    expect(document.body.textContent).toContain("Disk unavailable");
    const days = document.querySelector<HTMLInputElement>('input[type="number"]')!;
    days.value = "12"; days.dispatchEvent(new Event("input", { bubbles: true }));
    const save = [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Save backup schedule"))!;
    save.click(); await settle();
    expect(saveSchedule).toHaveBeenCalledWith(expect.objectContaining({ interval: "custom", intervalDays: 12, includeMedia: true }));
    expect(loadSchedule).toHaveBeenCalledTimes(1);
  });

  it("shows a running automatic job when the backup view opens late and requests cancellation", async () => {
    const active: BackupProgress = { jobId: "scheduled-1", automatic: true, state: "running", phase: "Writing backup archive", completed: 4, total: 6 };
    const loadActiveBackupJob = vi.fn().mockResolvedValue(active);
    const subscribeBackupProgress = vi.fn().mockResolvedValue(vi.fn());
    const cancelJob = vi.fn().mockResolvedValue(undefined);
    mountRestore({ loadActiveBackupJob, subscribeBackupProgress, cancelJob });
    await settle();

    expect(subscribeBackupProgress).toHaveBeenCalledWith(expect.any(Function));
    expect(loadActiveBackupJob).toHaveBeenCalledTimes(1);
    expect(document.querySelector('[aria-label="Automatic backup progress"]')?.textContent).toContain("Writing backup archive");
    const cancel = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Cancel automatic backup"))!;
    cancel.click(); await settle();
    expect(cancelJob).toHaveBeenCalledWith("scheduled-1");
    expect(document.body.textContent).toContain("Cancellation requested for the automatic backup.");
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel automatic backup"))).toBe(false);
  });

  it("attaches to automatic progress events, then refreshes schedule state on terminal failure", async () => {
    let publish: ((progress: BackupProgress) => void) | undefined;
    const loadSchedule = vi.fn()
      .mockResolvedValueOnce({ enabled: true, destination: "/backup", interval: "daily", intervalDays: 1, includeMedia: true, retentionDaily: 1, retentionWeekly: 1, lastSuccessAt: null, nextDueAt: null, lastArchiveBytes: null, retryAfterAt: null, lastStatus: "running", lastError: null })
      .mockResolvedValue({ enabled: true, destination: "/backup", interval: "daily", intervalDays: 1, includeMedia: true, retentionDaily: 1, retentionWeekly: 1, lastSuccessAt: null, nextDueAt: null, lastArchiveBytes: null, retryAfterAt: "2026-10-27T12:00:00Z", lastStatus: "failed", lastError: "No space left on device (os error 28)" });
    const subscribeBackupProgress = vi.fn(async (handler: (progress: BackupProgress) => void) => { publish = handler; return vi.fn(); });
    mountRestore({ loadSchedule, saveSchedule: vi.fn(), chooseScheduleDestination: vi.fn(), loadActiveBackupJob: vi.fn().mockResolvedValue(null), subscribeBackupProgress, cancelJob: vi.fn().mockResolvedValue(undefined) });
    await settle();
    publish!({ jobId: "scheduled-2", automatic: true, state: "running", phase: "Writing backup archive", completed: 4, total: 6 });
    await settle();
    expect(document.querySelector('[aria-label="Automatic backup progress"]')?.textContent).toContain("Writing backup archive");
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel automatic backup"))).toBe(true);

    publish!({ jobId: "scheduled-2", automatic: true, state: "failed", message: "No space left on device (os error 28)" });
    await settle();
    expect(loadSchedule).toHaveBeenCalledTimes(2);
    expect(document.body.textContent).toContain("Latest automatic backup failed: No space left on device (os error 28)");
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel automatic backup"))).toBe(false);
  });

  it("ignores stale active-job snapshots after a newer event and never cancels on view close", async () => {
    let publish: ((progress: BackupProgress) => void) | undefined;
    let resolveActive: ((progress: BackupProgress | null) => void) | undefined;
    const cancelJob = vi.fn();
    const unsubscribe = vi.fn();
    const loadActiveBackupJob = vi.fn(() => new Promise<BackupProgress | null>(resolve => { resolveActive = resolve; }));
    const subscribeBackupProgress = vi.fn(async (handler: (progress: BackupProgress) => void) => { publish = handler; return unsubscribe; });
    mountRestore({ loadActiveBackupJob, subscribeBackupProgress, cancelJob });
    await settle();
    publish!({ jobId: "scheduled-new", automatic: true, state: "running", phase: "Writing backup archive" });
    await settle();
    resolveActive!({ jobId: "scheduled-old", automatic: true, state: "running", phase: "Preparing a consistent snapshot" });
    await settle();
    expect(document.querySelector('[aria-label="Automatic backup progress"]')?.textContent).toContain("Writing backup archive");
    destroy?.(); destroy = undefined; await settle();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    expect(cancelJob).not.toHaveBeenCalled();
  });

  it("invalidates automatic snapshots on manual progress and ignores late events after close", async () => {
    let publish: ((progress: BackupProgress) => void) | undefined;
    let resolveActive: ((progress: BackupProgress | null) => void) | undefined;
    const loadSchedule = vi.fn().mockResolvedValue(null);
    const subscribeBackupProgress = vi.fn(async (handler: (progress: BackupProgress) => void) => { publish = handler; return vi.fn(); });
    mountRestore({
      loadSchedule,
      loadActiveBackupJob: vi.fn(() => new Promise<BackupProgress | null>(resolve => { resolveActive = resolve; })),
      subscribeBackupProgress,
    });
    await settle();
    publish!({ jobId: "manual-2", state: "running", phase: "Writing backup archive" });
    resolveActive!({ jobId: "stale-scheduled", automatic: true, state: "running", phase: "Preparing a consistent snapshot" });
    await settle();
    expect(document.querySelector('[aria-label="Automatic backup progress"]')).toBeNull();

    destroy?.(); destroy = undefined;
    publish!({ jobId: "late-scheduled", automatic: true, state: "complete", message: "Backup complete" });
    await settle();
    expect(loadSchedule).toHaveBeenCalledTimes(1);
  });

  it("ignores delayed progress from an older automatic job after a newer one starts", async () => {
    let publish: ((progress: BackupProgress) => void) | undefined;
    const subscribeBackupProgress = vi.fn(async (handler: (progress: BackupProgress) => void) => { publish = handler; return vi.fn(); });
    mountRestore({ loadActiveBackupJob: vi.fn().mockResolvedValue(null), subscribeBackupProgress });
    await settle();
    publish!({ jobId: "scheduled-old", automatic: true, state: "complete", message: "Old backup complete" });
    publish!({ jobId: "scheduled-new", automatic: true, state: "running", message: "New backup running" });
    publish!({ jobId: "scheduled-old", automatic: true, state: "running", message: "Stale old progress" });
    await settle();
    expect(document.querySelector('[aria-label="Automatic backup progress"]')?.textContent).toContain("New backup running");
    expect(document.querySelector('[aria-label="Automatic backup progress"]')?.textContent).not.toContain("Stale old progress");
  });

  it("keeps manual backup cancellation on its existing job flow", async () => {
    let reportManual: ((progress: BackupProgress) => void) | undefined;
    let finishManual: ((value: { jobId: string }) => void) | undefined;
    const createBackup = vi.fn((_options: { includeMedia: boolean }, report: (progress: BackupProgress) => void) => {
      reportManual = report;
      return new Promise<{ jobId: string }>(resolve => { finishManual = resolve; });
    });
    const cancelJob = vi.fn().mockResolvedValue(undefined);
    mountRestore({ createBackup, cancelJob });
    [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === "Back up now")!.click();
    reportManual!({ jobId: "manual-1", state: "running", phase: "Writing backup archive" });
    await settle();
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel operation"))).toBe(true);
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Cancel automatic backup"))).toBe(false);
    [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Cancel operation"))!.click();
    await settle();
    expect(cancelJob).toHaveBeenCalledWith("manual-1");
    finishManual!({ jobId: "manual-1" }); await settle();
    expect(createBackup).toHaveBeenCalledTimes(1);
  });

  it("previews and applies selected standalone settings, retaining choices after failure", async () => {
    const settingsPreview = {
      previewId: "settings-1", schemaVersion: 1,
      archiveBytes: 1024,
      applicationFields: ["theme"],
      preferenceKeys: ["playlistHierarchy.v1"],
      frontendPreferenceKeys: ["burrata.waveform-enabled.v1"],
      excludedApplicationFields: 1, excludedPreferenceKeys: 1,
      excludedFrontendPreferenceKeys: 1, excludedAssetPathHints: 2,
      omittedCacheRows: 3, omittedJobs: 1, omittedPrivateSettings: 2,
      warnings: ["Device paths will be skipped."],
    };
    const applySettings = vi.fn().mockRejectedValueOnce(new Error("write failed")).mockResolvedValue({ frontendPreferencesPending: true });
    const props = mountRestore({
      exportSettings: vi.fn().mockResolvedValue("settings.json"),
      inspectSettings: vi.fn().mockResolvedValue(settingsPreview),
      applySettings,
    });
    [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Import settings JSON"))!.click(); await settle();
    expect(document.querySelector('[aria-label="Settings import preview"]')?.textContent).toContain("Device paths will be skipped.");
    const appSettings = [...document.querySelectorAll("label")].find(label => label.textContent?.includes("Application settings"))!;
    appSettings.querySelector<HTMLInputElement>("input")!.click();
    const apply = [...document.querySelectorAll("button")].find(button => button.textContent?.includes("Apply selected settings"))!;
    apply.click(); await settle();
    expect(document.querySelector('[aria-label="Settings import preview"]')).not.toBeNull();
    expect(document.body.textContent).toContain("Could not apply settings archive: Error: write failed");
    apply.click(); await settle();
    expect(applySettings).toHaveBeenCalledTimes(2);
    expect(applySettings).toHaveBeenLastCalledWith("settings-1", expect.objectContaining({
      preferenceKeys: ["playlistHierarchy.v1"], frontendPreferenceKeys: ["burrata.waveform-enabled.v1"],
    }), settingsPreview);
    expect(document.querySelector('[aria-label="Settings import preview"]')).toBeNull();
    expect(document.body.textContent).toContain("We’ll confirm the imported interface preferences were saved the next time Burrata starts.");
  });
});

it("keeps Backup & Restore out of the playlist import choices", async () => {
  const onChoose = vi.fn();
  const instance = mount(ImportHub, { target: document.body, props: { onChoose } });
  destroy = () => unmount(instance); await settle();
  const choices = [...document.querySelectorAll("button")].map(button => button.textContent?.replace(/\s+/g, " ").trim());
  expect(choices).toHaveLength(4);
  expect(choices.join(" ")).toContain("Choose a playlist file");
  expect(choices.join(" ")).toContain("Paste a tracklist");
  expect(choices.join(" ")).toContain("Import a playlist or album link");
  expect(choices.join(" ")).toContain("Add music files or a folder");
  expect(document.body.textContent).not.toContain("Backup & restore");
  expect(document.body.textContent).not.toContain("Playlist exports share tracks");
  expect(onChoose).not.toHaveBeenCalled();
});
