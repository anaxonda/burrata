<script lang="ts">
  import { onMount, untrack } from "svelte";
  export type BackupComponentSummary = {
    id: string;
    bytes: number;
    records?: number | null;
    description?: string;
  };

  export type BackupInspection = {
    previewId: string;
    schemaVersion: number;
    createdAt: string;
    components: BackupComponentSummary[];
    media: { included: boolean; assets: number; missing: number; changed: number };
    mediaRoots?: BackupMediaRootPreview[];
    warnings: string[];
    estimatedBytes?: number;
    archiveBytes?: number;
    summary?: RestoreStateSummary;
    settingsProjection?: {
      omittedCacheRows: number;
      omittedJobs: number;
      omittedPrivateSettings: number;
      excludedApplicationFields: number;
      assetPathHints: number;
    };
  };

  export type RestoreStateSummary = {
    alreadyApplied?: boolean;
    importedRecordings?: number;
    importedCatalogListings?: number;
    importedCollections?: number;
    importedPlaylistOccurrences?: number;
    importedAssets?: number;
    importedHistoryEvents?: number;
    skippedConflicts?: number;
    unavailableAssets?: number;
    omittedStaleAnalyses?: number;
    deviceSettingsSkipped?: number;
    privateSettingsOmitted?: number;
    warnings?: string[];
  };

  export type BackupMediaRootPreview = {
    rootKey: string;
    label: string;
    assets: number;
    requiredBytes: number;
    destinationToken: string | null;
    destinationLabel: string | null;
    reusableSameHash: number;
    copyCount: number;
    conflicts: number;
    missingDestination: boolean;
    availableBytes: number | null;
    lyricsSidecars?: number;
    lyricsCopyCount?: number;
    lyricsReusableSameHash?: number;
    lyricsConflicts?: number;
  };

  export type BackupProgress = {
    jobId?: string;
    state: "queued" | "running" | "complete" | "failed" | "canceled";
    automatic?: boolean;
    phase?: string;
    completed?: number;
    total?: number;
    message?: string;
  };

  export type BackupSchedule = {
    enabled: boolean;
    destination: string | null;
    interval: "daily" | "weekly" | "custom";
    intervalDays: number;
    includeMedia: boolean;
    destinationAvailableBytes?: number | null;
    retentionDaily: number;
    retentionWeekly: number;
    lastSuccessAt: string | null;
    nextDueAt: string | null;
    lastArchiveBytes: number | null;
    retryAfterAt: string | null;
    lastStatus: string | null;
    lastError: string | null;
  };

  export type SettingsImportPreview = {
    previewId: string;
    schemaVersion: number;
    archiveBytes: number;
    applicationFields: string[];
    preferenceKeys: string[];
    frontendPreferenceKeys: string[];
    excludedApplicationFields: number;
    excludedPreferenceKeys: number;
    excludedFrontendPreferenceKeys: number;
    excludedAssetPathHints: number;
    omittedCacheRows: number;
    omittedJobs: number;
    omittedPrivateSettings: number;
    warnings: string[];
  };

  export type SettingsImportSelection = {
    application: boolean;
    preferenceKeys: string[];
    frontendPreferenceKeys: string[];
  };

  export type BackupMediaEstimate = {
    estimatedBytes: number;
    registeredAssets: number;
    eligibleMediaAssets: number;
    missingAssets: number;
    changedAssets: number;
    mediaIncluded: boolean;
    eligibleSidecars: number;
    missingSidecars: number;
    changedSidecars: number;
    sidecarBytes: number;
  };

  type RestoreMode = "merge" | "replace";
  type ProgressReporter = (progress: BackupProgress) => void;

  let {
    inspectBackup,
    createBackup,
    restoreBackup,
    chooseMediaRoot,
    estimateBackup,
    loadSchedule,
    saveSchedule,
    chooseScheduleDestination,
    exportSettings,
    inspectSettings,
    applySettings,
    previewRestore,
    cancelJob,
    loadActiveBackupJob,
    subscribeBackupProgress,
    allowReplace = false,
    initialPreview = null,
  }: {
    /** Opens the native file picker, validates the selected archive, and returns a reviewable preview. */
    inspectBackup: () => Promise<BackupInspection | null>;
    /** Opens the destination picker and creates a verified backup. */
    createBackup: (options: { includeMedia: boolean }, report: ProgressReporter) => Promise<{ jobId?: string } | void>;
    restoreBackup: (previewId: string, options: { mode: RestoreMode; componentIds: string[]; pathMappings: { rootKey: string; destinationToken: string }[] }, report: ProgressReporter) => Promise<{ jobId?: string; restartRequired?: boolean } | void>;
    chooseMediaRoot: (previewId: string, rootKey: string) => Promise<BackupMediaRootPreview | null>;
    estimateBackup?: (includeMedia: boolean) => Promise<BackupMediaEstimate>;
    loadSchedule?: () => Promise<BackupSchedule>;
    saveSchedule?: (schedule: BackupSchedule) => Promise<BackupSchedule>;
    chooseScheduleDestination?: () => Promise<string | null>;
    exportSettings?: () => Promise<string | null | void | boolean>;
    inspectSettings?: () => Promise<SettingsImportPreview | null>;
    applySettings?: (previewId: string, selection: SettingsImportSelection, preview: SettingsImportPreview) => Promise<void | { frontendPreferencesPending?: boolean }>;
    previewRestore?: (previewId: string, options: { mode: RestoreMode; componentIds: string[]; pathMappings: { rootKey: string; destinationToken: string }[] }) => Promise<RestoreStateSummary>;
    cancelJob?: (jobId: string) => Promise<void>;
    /** Current active scheduler job, if one exists; used when this view opens mid-run. */
    loadActiveBackupJob?: () => Promise<BackupProgress | null>;
    /** Subscribe to backend backup progress without taking ownership of the job lifecycle. */
    subscribeBackupProgress?: (handler: (progress: BackupProgress) => void) => Promise<() => void>;
    /** Keep false until the backend has safe writer quiescence and journaled replacement recovery. */
    allowReplace?: boolean;
    /** A backup already inspected from a trusted native file drop. */
    initialPreview?: BackupInspection | null;
  } = $props();

  const initialPreviewSeed = untrack(() => initialPreview);
  let preview = $state<BackupInspection | null>(initialPreviewSeed);
  let selectedComponents = $state<string[]>(initialPreviewSeed?.components
    .filter(component => !component.id.startsWith("media:") && !component.id.startsWith("aux-lyrics:"))
    .map(component => component.id) ?? []);
  let mode = $state<RestoreMode>("merge");
  let includeMedia = $state(false);
  let restoreMedia = $state(false);
  let mediaRoots = $state<BackupMediaRootPreview[]>(initialPreviewSeed?.mediaRoots ?? []);
  let replaceAcknowledged = $state(false);
  let inspecting = $state(false);
  let busy = $state(false);
  let jobId = $state<string | null>(null);
  let progress = $state<BackupProgress | null>(null);
  let automaticJob = $state<BackupProgress | null>(null);
  let automaticCancelRequested = $state(false);
  let error = $state("");
  let notice = $state("");
  let schedule = $state<BackupSchedule>({ enabled: false, destination: null, interval: "daily", intervalDays: 1, includeMedia: false, retentionDaily: 7, retentionWeekly: 4, lastSuccessAt: null, nextDueAt: null, lastArchiveBytes: null, retryAfterAt: null, lastStatus: null, lastError: null });
  let scheduleBusy = $state(false);
  let mediaEstimate = $state<BackupMediaEstimate | null>(null);
  let scheduleEstimate = $state<BackupMediaEstimate | null>(null);
  let estimating = $state(false);
  let settingsPreview = $state<SettingsImportPreview | null>(null);
  let settingsSelection = $state<SettingsImportSelection>({ application: false, preferenceKeys: [], frontendPreferenceKeys: [] });
  let settingsBusy = $state(false);
  let summaryBusy = $state(false);
  let summaryRequest = 0;

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "size unavailable";
    if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  async function refreshSchedule() {
    if (!loadSchedule) return;
    try {
      const value = await loadSchedule();
      schedule = { ...schedule, ...value };
      if (estimateBackup) void updateScheduleMedia(value.includeMedia);
    } catch (failure) {
      error = `Could not load backup schedule: ${String(failure)}`;
    }
  }

  function isTerminalBackupState(state: BackupProgress["state"]) {
    return state === "complete" || state === "failed" || state === "canceled";
  }

  let automaticEventRevision = 0;
  let latestAutomaticJobId: string | null = null;
  let latestAutomaticJobState: BackupProgress["state"] | null = null;
  const retiredAutomaticJobIds = new Set<string>();
  function reportAutomaticJob(next: BackupProgress) {
    // Any progress event means the active-job snapshot request is stale, even
    // if that event is for a manual operation we do not display here.
    automaticEventRevision++;
    if (!next.automatic || !next.jobId) return;
    if (retiredAutomaticJobIds.has(next.jobId)) return;
    if (latestAutomaticJobId && latestAutomaticJobId !== next.jobId) {
      // The backend only permits one backup/restore at a time. A different
      // job cannot supersede a still-running one, so treat that event as stale.
      if (!latestAutomaticJobState || !isTerminalBackupState(latestAutomaticJobState)) return;
      retiredAutomaticJobIds.add(latestAutomaticJobId);
    }
    latestAutomaticJobId = next.jobId;
    latestAutomaticJobState = next.state;
    if (automaticJob?.jobId !== next.jobId) automaticCancelRequested = false;
    automaticJob = next;
    if (isTerminalBackupState(next.state)) {
      retiredAutomaticJobIds.add(next.jobId);
      automaticCancelRequested = false;
      void refreshSchedule();
    }
  }

  onMount(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void refreshSchedule();
    const loadCurrentAutomaticJob = async () => {
      if (!loadActiveBackupJob) return;
      const revisionAtRequest = automaticEventRevision;
      try {
        const active = await loadActiveBackupJob();
        if (!disposed && revisionAtRequest === automaticEventRevision && active?.automatic && active.jobId) {
          if (!retiredAutomaticJobIds.has(active.jobId)) {
            latestAutomaticJobId = active.jobId;
            latestAutomaticJobState = active.state;
            if (isTerminalBackupState(active.state)) retiredAutomaticJobIds.add(active.jobId);
            automaticJob = active;
            if (isTerminalBackupState(active.state)) void refreshSchedule();
          }
        }
      } catch (failure) {
        if (!disposed) error = `Could not load active automatic backup: ${String(failure)}`;
      }
    };
    if (subscribeBackupProgress) {
      void subscribeBackupProgress(next => {
        if (!disposed) reportAutomaticJob(next);
      }).then(stop => {
        if (disposed) stop();
        else {
          unsubscribe = stop;
          // Subscribe first so an event cannot fall into a snapshot/listener gap.
          void loadCurrentAutomaticJob();
        }
      }).catch(failure => {
        if (!disposed) {
          error = `Could not monitor automatic backup progress: ${String(failure)}`;
          void loadCurrentAutomaticJob();
        }
      });
    } else {
      void loadCurrentAutomaticJob();
    }
    return () => {
      disposed = true;
      unsubscribe?.();
      // Automatic jobs keep running when this view closes. The subscription is
      // view-owned; the job and its cancellation state belong to the manager.
    };
  });

  async function pickScheduleDestination() {
    if (!chooseScheduleDestination) return;
    try {
      const destination = await chooseScheduleDestination();
      if (destination) schedule = { ...schedule, destination };
    } catch (failure) { error = `Could not choose a backup folder: ${String(failure)}`; }
  }

  async function persistSchedule() {
    if (!saveSchedule) return;
    scheduleBusy = true;
    try { schedule = await saveSchedule(schedule); notice = schedule.enabled ? "Automatic backups are enabled." : "Automatic backups are paused."; }
    catch (failure) { error = `Could not save backup schedule: ${String(failure)}`; }
    finally { scheduleBusy = false; }
  }

  async function saveSettingsArchive() {
    if (!exportSettings) return;
    settingsBusy = true; error = ""; notice = "";
    try {
      const path = await exportSettings();
      notice = path === false ? "Settings export canceled." : typeof path === "string" && path ? `Settings archive saved: ${path}` : "Settings archive exported.";
    } catch (failure) { error = `Could not export settings: ${String(failure)}`; }
    finally { settingsBusy = false; }
  }

  async function inspectSettingsArchive() {
    if (!inspectSettings) return;
    settingsBusy = true; error = ""; notice = "";
    try {
      const next = await inspectSettings();
      if (!next) return;
      settingsPreview = next;
      settingsSelection = {
        application: next.applicationFields.length > 0,
        preferenceKeys: [...next.preferenceKeys],
        frontendPreferenceKeys: [...next.frontendPreferenceKeys],
      };
    } catch (failure) { error = `Could not inspect settings archive: ${String(failure)}`; }
    finally { settingsBusy = false; }
  }

  async function applySettingsArchive() {
    if (!settingsPreview || !applySettings) return;
    settingsBusy = true; error = ""; notice = "";
    try {
      const result = await applySettings(settingsPreview.previewId, settingsSelection, settingsPreview);
      notice = result?.frontendPreferencesPending
        ? "Selected settings were applied. We’ll confirm the imported interface preferences were saved the next time Burrata starts."
        : "Selected settings were applied.";
      settingsPreview = null;
    } catch (failure) { error = `Could not apply settings archive: ${String(failure)}`; }
    finally { settingsBusy = false; }
  }

  function toggleKey(keys: string[], key: string, checked: boolean): string[] {
    return checked ? [...new Set([...keys, key])] : keys.filter(value => value !== key);
  }

  async function updateIncludeMedia(enabled: boolean) {
    includeMedia = enabled;
    mediaEstimate = null;
    if (!enabled || !estimateBackup) return;
    estimating = true;
    try { mediaEstimate = await estimateBackup(true); }
    catch (failure) { error = `Could not estimate backup size: ${String(failure)}`; }
    finally { estimating = false; }
  }

  async function updateScheduleMedia(enabled: boolean) {
    schedule.includeMedia = enabled;
    scheduleEstimate = null;
    if (!estimateBackup) return;
    estimating = true;
    try { scheduleEstimate = await estimateBackup(true); }
    catch (failure) { error = `Could not estimate automatic backup size: ${String(failure)}`; }
    finally { estimating = false; }
  }

  async function inspect() {
    inspecting = true;
    error = "";
    notice = "";
    try {
      const next = await inspectBackup();
      if (!next) return;
      preview = next;
      selectedComponents = next.components
        .filter(component => !component.id.startsWith("media:") && !component.id.startsWith("aux-lyrics:"))
        .map(component => component.id);
      mediaRoots = next.mediaRoots ?? [];
      restoreMedia = false;
      mode = "merge";
      replaceAcknowledged = false;
    } catch (failure) {
      // Keep the prior preview visible so a picker or validation error never discards the user's selection.
      error = `Could not inspect this backup: ${String(failure)}`;
    } finally {
      inspecting = false;
    }
  }

  function report(next: BackupProgress) {
    progress = next;
    if (next.jobId) jobId = next.jobId;
  }

  async function backUpNow() {
    busy = true;
    error = "";
    notice = "";
    progress = { state: "queued", phase: "Preparing backup" };
    try {
      const result = await createBackup({ includeMedia }, report);
      if (result && result.jobId) jobId = result.jobId;
      if (progress?.state !== "canceled") notice = "Backup completed and verified.";
    } catch (failure) {
      error = `Backup did not finish: ${String(failure)}`;
    } finally {
      busy = false;
      jobId = null;
    }
  }

  async function restore() {
    if (!preview || (!selectedComponents.some(id => !id.startsWith("media:")) && !restoreMedia)) return;
    const replaceReady = ["library", "history", "settings", "playback"].every(value => selectedComponents.includes(value));
    if (mode === "replace" && (!replaceAcknowledged || !replaceReady)) return;
    busy = true;
    error = "";
    notice = "";
    progress = { state: "queued", phase: "Preparing restore" };
    try {
      const mediaIds = restoreMedia ? preview.components.filter(component => component.id.startsWith("media:")).map(component => component.id) : [];
      const auxiliaryIds = restoreMedia ? preview.components.filter(component => component.id.startsWith("aux-lyrics:")).map(component => component.id) : [];
      const pathMappings = selectedComponents.includes("library") ? selectedPathMappings() : [];
      const componentIds = [...new Set([...selectedComponents.filter(id => !id.startsWith("media:")), ...mediaIds, ...auxiliaryIds])];
      const result = await restoreBackup(preview.previewId, { mode, componentIds, pathMappings }, report);
      if (result && result.jobId) jobId = result.jobId;
      if (progress?.state !== "canceled") notice = result && "restartRequired" in result && result.restartRequired ? "Restore staged. Burrata will restart to apply the backup." : "Restore completed.";
      preview = null;
      selectedComponents = [];
      replaceAcknowledged = false;
    } catch (failure) {
      // Keep the inspected archive and component choices available for retry after a failure.
      error = `Restore did not finish: ${String(failure)}`;
    } finally {
      busy = false;
      jobId = null;
    }
  }

  async function chooseRoot(root: BackupMediaRootPreview) {
    if (!preview) return;
    try {
      const next = await chooseMediaRoot(preview.previewId, root.rootKey);
      if (next) {
        mediaRoots = mediaRoots.map(item => item.rootKey === next.rootKey ? next : item);
        void refreshRestoreSummary();
      }
    } catch (failure) {
      error = `Could not select an audio folder: ${String(failure)}`;
    }
  }

  async function cancel() {
    if (!busy || !jobId || !cancelJob) return;
    try {
      await cancelJob(jobId);
      notice = "Cancellation requested. The current library remains available until the operation stops.";
    } catch (failure) {
      error = `Could not cancel the operation: ${String(failure)}`;
    }
  }

  async function cancelAutomaticBackup() {
    if (!automaticJob?.automatic || !automaticJob.jobId || !cancelJob || isTerminalBackupState(automaticJob.state) || automaticCancelRequested) return;
    automaticCancelRequested = true;
    error = "";
    notice = "Cancellation requested for the automatic backup.";
    try {
      await cancelJob(automaticJob.jobId);
    } catch (failure) {
      automaticCancelRequested = false;
      error = `Could not cancel automatic backup: ${String(failure)}`;
      notice = "";
    }
  }

  function toggleComponent(id: string, checked: boolean) {
    selectedComponents = checked
      ? [...new Set([...selectedComponents, id])]
      : selectedComponents.filter(value => value !== id);
    if (id === "library" && !checked) restoreMedia = false;
    const replaceReady = ["library", "history", "settings", "playback"].every(value => selectedComponents.includes(value));
    if (!replaceReady && mode === "replace") { mode = "merge"; replaceAcknowledged = false; }
    void refreshRestoreSummary();
  }

  async function refreshRestoreSummary() {
    if (!preview || !previewRestore) return;
    const request = ++summaryRequest;
    const mediaIds = restoreMedia ? preview.components.filter(component => component.id.startsWith("media:")).map(component => component.id) : [];
    const auxiliaryIds = restoreMedia ? preview.components.filter(component => component.id.startsWith("aux-lyrics:")).map(component => component.id) : [];
    const componentIds = [...new Set([...selectedComponents, ...mediaIds, ...auxiliaryIds])];
    if (!componentIds.length) {
      preview = { ...preview, summary: {} };
      summaryBusy = false;
      return;
    }
    const pathMappings = selectedComponents.includes("library") ? selectedPathMappings() : [];
    summaryBusy = true;
    try {
      const summary = await previewRestore(preview.previewId, { mode, componentIds, pathMappings });
      if (request === summaryRequest && preview) preview = { ...preview, summary };
    } catch (failure) {
      if (request === summaryRequest) error = `Could not update restore preview: ${String(failure)}`;
    } finally {
      if (request === summaryRequest) summaryBusy = false;
    }
  }

  function selectedPathMappings() {
    return mediaRoots.filter(root => root.destinationToken)
      .map(root => ({ rootKey: root.rootKey, destinationToken: root.destinationToken! }));
  }
</script>

<section class="backup-restore" aria-labelledby="backup-restore-title">
  <header>
    <div>
      <h2 id="backup-restore-title">Backup &amp; restore</h2>
      <p>Protect your library, playlists, listening history, and resumable queue.</p>
    </div>
  </header>

  <section class="backup-card" aria-labelledby="create-backup-title">
    <h3 id="create-backup-title">Create a backup</h3>
    <p>Metadata-only backups do not include the audio files in your library. Backup files contain private library data and are not encrypted.</p>
    <label class="option"><input type="checkbox" checked={includeMedia} onchange={(event) => void updateIncludeMedia(event.currentTarget.checked)} disabled={busy || inspecting || estimating} /><span>Include verified audio files and adjacent lyrics sidecars</span></label>
    {#if estimating}<small>Checking registered audio files and estimating archive size…</small>{/if}
    {#if mediaEstimate}<small>{mediaEstimate.eligibleMediaAssets} verified audio files · {formatBytes(mediaEstimate.estimatedBytes)} estimated · {mediaEstimate.missingAssets} missing · {mediaEstimate.changedAssets} changed and omitted · {mediaEstimate.eligibleSidecars} verified lyrics sidecars ({formatBytes(mediaEstimate.sidecarBytes)}) · {mediaEstimate.missingSidecars} missing · {mediaEstimate.changedSidecars} changed and omitted</small>{/if}
    <button type="button" class="primary" disabled={busy || inspecting || estimating} onclick={() => void backUpNow()}>{busy && progress?.phase?.includes("backup") ? "Creating backup…" : "Back up now"}</button>
  </section>

  {#if loadSchedule && saveSchedule && chooseScheduleDestination}
    <section class="backup-card" aria-labelledby="backup-schedule-title">
      <h3 id="backup-schedule-title">Automatic backups</h3>
      <p>Backups run while Burrata is open. After reopening, one overdue backup runs automatically.</p>
      <small>Daily repeats every 24 hours; weekly every 7 days. The local time may shift with daylight saving. Retention applies only to app-owned automatic archives.</small>
      <small>Choose another drive or a synced folder to protect against losing this disk.</small>
      <label class="option"><input type="checkbox" bind:checked={schedule.enabled} disabled={scheduleBusy || busy} /><span>Enable automatic backups</span></label>
      <div class="schedule-row"><span>{schedule.destination || "Choose a backup folder"}</span><button type="button" disabled={scheduleBusy || busy} onclick={() => void pickScheduleDestination()}>Choose folder…</button></div>
      <label>Frequency<select bind:value={schedule.interval} disabled={scheduleBusy || busy}><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="custom">Custom</option></select></label>
      {#if schedule.interval === "custom"}<label>Repeat every (days)<input type="number" min="1" max="365" bind:value={schedule.intervalDays} disabled={scheduleBusy || busy} /></label>{/if}
      <label class="option"><input type="checkbox" checked={schedule.includeMedia} onchange={event => void updateScheduleMedia(event.currentTarget.checked)} disabled={scheduleBusy || busy || estimating} /><span><strong>Include verified audio files and adjacent lyrics sidecars</strong><small>Scheduled archives can require substantial disk space.</small></span></label>
      {#if estimating}<small>Estimating automatic backup size…</small>{/if}
      {#if scheduleEstimate}<small>Scheduled {schedule.includeMedia ? "media-inclusive" : "metadata-only"} estimate: {formatBytes(scheduleEstimate.estimatedBytes)}{#if schedule.includeMedia} · {scheduleEstimate.eligibleMediaAssets} verified audio files · {scheduleEstimate.missingAssets} missing · {scheduleEstimate.changedAssets} changed and omitted · {scheduleEstimate.eligibleSidecars} lyrics sidecars ({formatBytes(scheduleEstimate.sidecarBytes)}) · {scheduleEstimate.missingSidecars} missing · {scheduleEstimate.changedSidecars} changed and omitted{/if}</small>{/if}
      {#if schedule.destinationAvailableBytes != null}<small>Backup folder space available: {formatBytes(schedule.destinationAvailableBytes)}{#if scheduleEstimate && schedule.destinationAvailableBytes < scheduleEstimate.estimatedBytes} · estimate exceeds available space{/if}</small>{/if}
      <div class="schedule-grid"><label>Daily copies<input type="number" min="1" max="365" bind:value={schedule.retentionDaily} disabled={scheduleBusy || busy} /></label><label>Weekly copies<input type="number" min="1" max="104" bind:value={schedule.retentionWeekly} disabled={scheduleBusy || busy} /></label></div>
      {#if schedule.lastSuccessAt}<small>Last successful backup: {new Date(schedule.lastSuccessAt).toLocaleString()}</small>{/if}
      {#if schedule.lastArchiveBytes != null}<small>Last archive size: {formatBytes(schedule.lastArchiveBytes)}</small>{/if}
      {#if schedule.nextDueAt}<small>Next backup due: {new Date(schedule.nextDueAt).toLocaleString()}</small>{/if}
      {#if schedule.retryAfterAt}<small>Retry after: {new Date(schedule.retryAfterAt).toLocaleString()}</small>{/if}
      {#if schedule.lastStatus}<small>Automatic backup status: {schedule.lastStatus}</small>{/if}
      {#if schedule.lastError}<p class="error" role="alert">Latest automatic backup failed: {schedule.lastError}</p>{/if}
      <button type="button" class="primary" disabled={scheduleBusy || busy || schedule.enabled && !schedule.destination} onclick={() => void persistSchedule()}>{scheduleBusy ? "Saving…" : "Save backup schedule"}</button>
    </section>
  {/if}

  <section class="backup-card" aria-labelledby="restore-backup-title">
    <h3 id="restore-backup-title">Restore a backup</h3>
    <p>Review the archive contents before choosing what to restore. Playback returns paused, and downloads do not resume automatically.</p>
    <button type="button" disabled={busy || inspecting} onclick={() => void inspect()}>{inspecting ? "Checking backup…" : preview ? "Choose another backup…" : "Choose backup…"}</button>

    {#if preview}
      <div class="preview" aria-label="Backup preview">
        <div class="preview-heading"><strong>Backup from {new Date(preview.createdAt).toLocaleString()}</strong><span>Format v{preview.schemaVersion}</span></div>
        <p>{preview.components.length} components · {formatBytes(preview.archiveBytes ?? preview.components.reduce((sum, item) => sum + item.bytes, 0))} archive{#if preview.estimatedBytes != null} · estimated restore space {formatBytes(preview.estimatedBytes)}{/if}</p>
        {#if preview.settingsProjection && preview.settingsProjection.excludedApplicationFields + preview.settingsProjection.assetPathHints + preview.settingsProjection.omittedPrivateSettings + preview.settingsProjection.omittedCacheRows + preview.settingsProjection.omittedJobs > 0}<section class="restore-summary" aria-label="Backup settings coverage"><strong>Settings excluded from this backup</strong><div><span>{preview.settingsProjection.excludedApplicationFields} device-specific fields</span><span>{preview.settingsProjection.assetPathHints} local path hints</span><span>{preview.settingsProjection.omittedPrivateSettings} private settings</span><span>{preview.settingsProjection.omittedCacheRows} cache records</span><span>{preview.settingsProjection.omittedJobs} jobs</span></div></section>{/if}
        {#if preview.summary}<section class="restore-summary" aria-label="Restore preview summary"><strong>Restore preview {#if summaryBusy}<small>Updating…</small>{/if}</strong><div>{#if preview.summary.importedRecordings != null}<span>{preview.summary.importedRecordings.toLocaleString()} recordings</span>{/if}{#if preview.summary.importedCatalogListings != null}<span>{preview.summary.importedCatalogListings.toLocaleString()} catalog listings</span>{/if}{#if preview.summary.importedCollections != null}<span>{preview.summary.importedCollections.toLocaleString()} playlists</span>{/if}{#if preview.summary.importedPlaylistOccurrences != null}<span>{preview.summary.importedPlaylistOccurrences.toLocaleString()} playlist entries</span>{/if}{#if preview.summary.importedHistoryEvents != null}<span>{preview.summary.importedHistoryEvents.toLocaleString()} history events</span>{/if}{#if preview.summary.importedAssets != null}<span>{preview.summary.importedAssets.toLocaleString()} assets</span>{/if}{#if preview.summary.skippedConflicts != null}<span>{preview.summary.skippedConflicts.toLocaleString()} conflicts skipped</span>{/if}{#if preview.summary.unavailableAssets != null}<span>{preview.summary.unavailableAssets.toLocaleString()} audio files currently unavailable</span>{/if}{#if preview.summary.omittedStaleAnalyses != null}<span>{preview.summary.omittedStaleAnalyses.toLocaleString()} stale analyses omitted</span>{/if}{#if preview.summary.deviceSettingsSkipped != null}<span>{preview.summary.deviceSettingsSkipped.toLocaleString()} device settings skipped</span>{/if}{#if preview.summary.privateSettingsOmitted != null}<span>{preview.summary.privateSettingsOmitted.toLocaleString()} private settings omitted</span>{/if}</div>{#if (preview.summary.unavailableAssets ?? 0) > 0}<small>{restoreMedia ? "Availability is checked before the audio files below are copied." : "Availability is checked in folders you choose. Matching files already on this device can be linked."}</small>{/if}<small>Device-specific paths and account settings are reviewed as warnings and are not applied automatically.</small></section>{/if}
        <fieldset disabled={busy}>
          <legend>Restore components</legend>
          {#each preview.components.filter(component => !component.id.startsWith("media:") && !component.id.startsWith("aux-lyrics:")) as component (component.id)}
            <label class="option"><input type="checkbox" checked={selectedComponents.includes(component.id)} onchange={(event) => toggleComponent(component.id, event.currentTarget.checked)} /><span><strong>{component.description || component.id}</strong><small>{formatBytes(component.bytes)}{#if component.records != null} · {component.records.toLocaleString()} records{/if}</small></span></label>
          {/each}
        </fieldset>
        <fieldset class="mode-choice" disabled={busy}>
          <legend>How should this backup be applied?</legend>
          <label class="option"><input type="radio" name="restore-mode" value="merge" bind:group={mode} onchange={() => void refreshRestoreSummary()} /><span><strong>Merge into current library</strong><small>Add the selected data using the reviewed conflict rules.</small></span></label>
          <label class="option" class:unavailable={!allowReplace}><input type="radio" name="restore-mode" value="replace" bind:group={mode} onchange={() => void refreshRestoreSummary()} disabled={!allowReplace || !["library", "history", "settings", "playback"].every(value => selectedComponents.includes(value))} /><span><strong>Replace library</strong><small>{!allowReplace ? "Unavailable until safe replacement and rollback are supported." : !["library", "history", "settings", "playback"].every(value => selectedComponents.includes(value)) ? "Select all four data components to replace." : "Replace all data components after a verified recovery snapshot."}</small></span></label>
        </fieldset>
        {#if mode === "replace" && allowReplace}
          <label class="option acknowledge"><input type="checkbox" bind:checked={replaceAcknowledged} /><span>I understand this replaces selected library data. A recovery snapshot will be created first.</span></label>
        {/if}
        <section class="media-restore" aria-label="Restore audio files">
          <label class="option"><input type="checkbox" checked={restoreMedia} onchange={event => { restoreMedia = event.currentTarget.checked; void refreshRestoreSummary(); }} disabled={busy || !preview.media.included || !selectedComponents.includes("library")} /><span><strong>Restore verified audio files</strong><small>{!selectedComponents.includes("library") ? "Select the library component to restore audio files." : preview.media.included ? `${preview.media.assets} assets in backup · ${preview.media.missing} missing · ${preview.media.changed} changed. Included lyrics sidecars restore beside their audio files; existing sidecars are never overwritten.` : "No audio files are included in this backup."}</small></span></label>
          {#if selectedComponents.includes("library") && mediaRoots.length}
            <p class="media-summary">Link existing audio from a folder on this device. Matching files are checked before linking; missing or changed files stay unavailable.</p>
            {#each mediaRoots as root (root.rootKey)}
              <div class="media-root"><span><strong>{root.label}</strong>{#if root.destinationLabel}<small>Folder: {root.destinationLabel}</small>{/if}{#if root.destinationToken}<small>{root.reusableSameHash} matching audio file{root.reusableSameHash === 1 ? "" : "s"} found in this folder</small>{/if}{#if restoreMedia}<small>{root.assets} backup audio files · {formatBytes(root.requiredBytes)} required</small>{#if root.availableBytes != null}<small>{formatBytes(root.availableBytes)} available{#if root.availableBytes < root.requiredBytes} · not enough space for all selected files{/if}</small>{/if}{#if root.destinationToken}<small>{root.copyCount} audio files will copy · {root.conflicts} occupied paths will be skipped</small>{/if}{#if root.lyricsSidecars}<small>{root.lyricsSidecars} verified lyrics sidecars{#if root.destinationToken} · {root.lyricsReusableSameHash ?? 0} already match · {root.lyricsCopyCount ?? 0} will copy · {root.lyricsConflicts ?? 0} conflicts skipped{/if}</small>{/if}{/if}</span><button type="button" disabled={busy} onclick={() => void chooseRoot(root)}>{root.destinationToken ? "Change folder…" : restoreMedia ? "Choose folder…" : "Link existing audio…"}</button></div>
            {/each}
          {:else if selectedComponents.includes("library") && preview.settingsProjection?.assetPathHints}
            <p class="media-summary">No usable audio folder hints are available in this backup.</p>
          {/if}
        </section>
        {#if !restoreMedia}<div class="media-summary">No audio files or lyrics sidecars will be copied. You can link matching audio already on this device; files that are missing or changed remain unavailable.</div>{/if}
        {#if preview.summary?.warnings?.length}<ul class="warnings">{#each preview.summary.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}
        {#if preview.warnings.length}<ul class="warnings">{#each preview.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}
        <div class="actions"><button type="button" disabled={busy} onclick={() => { summaryRequest++; summaryBusy = false; preview = null; selectedComponents = []; mediaRoots = []; restoreMedia = false; }}>Cancel</button><button type="button" class="primary" disabled={busy || summaryBusy || (!selectedComponents.some(id => !id.startsWith("media:")) && !restoreMedia) || mode === "replace" && (!replaceAcknowledged || !["library", "history", "settings", "playback"].every(value => selectedComponents.includes(value)))} onclick={() => void restore()}>{busy ? "Restoring…" : mode === "replace" ? "Replace selected data" : "Merge selected data"}</button></div>
      </div>
    {/if}
  </section>

  {#if exportSettings || inspectSettings}
    <section class="backup-card" aria-labelledby="settings-archive-title">
      <h3 id="settings-archive-title">Settings archive</h3>
      <p>Move approved preferences between Burrata installs. Device paths, account credentials, and session data are excluded.</p>
      <div class="actions settings-actions">
        {#if exportSettings}<button type="button" disabled={settingsBusy || busy} onclick={() => void saveSettingsArchive()}>Export settings JSON</button>{/if}
        {#if inspectSettings}<button type="button" disabled={settingsBusy || busy} onclick={() => void inspectSettingsArchive()}>{settingsPreview ? "Choose another settings file…" : "Import settings JSON…"}</button>{/if}
      </div>
      {#if settingsPreview}
        <section class="settings-preview" aria-label="Settings import preview">
          <strong>Settings archive v{settingsPreview.schemaVersion} · {formatBytes(settingsPreview.archiveBytes)}</strong>
          <p>{settingsPreview.applicationFields.length} application fields · {settingsPreview.preferenceKeys.length} saved preferences · {settingsPreview.frontendPreferenceKeys.length} frontend preferences</p>
          {#if settingsPreview.excludedApplicationFields + settingsPreview.excludedPreferenceKeys + settingsPreview.excludedFrontendPreferenceKeys + settingsPreview.excludedAssetPathHints + settingsPreview.omittedCacheRows + settingsPreview.omittedJobs + settingsPreview.omittedPrivateSettings > 0}<p>{settingsPreview.excludedApplicationFields} device/unsupported application fields · {settingsPreview.excludedAssetPathHints} path hints · {settingsPreview.omittedPrivateSettings} private settings · {settingsPreview.omittedCacheRows} cache records · {settingsPreview.omittedJobs} jobs excluded</p>{/if}
          {#if settingsPreview.applicationFields.length}<label class="option"><input type="checkbox" bind:checked={settingsSelection.application} /><span><strong>Application settings</strong><small>{settingsPreview.applicationFields.join(", ")}</small></span></label>{/if}
          {#each settingsPreview.preferenceKeys as key (key)}<label class="option"><input type="checkbox" checked={settingsSelection.preferenceKeys.includes(key)} onchange={event => settingsSelection = { ...settingsSelection, preferenceKeys: toggleKey(settingsSelection.preferenceKeys, key, event.currentTarget.checked) }} /><span>{key}</span></label>{/each}
          {#each settingsPreview.frontendPreferenceKeys as key (key)}<label class="option"><input type="checkbox" checked={settingsSelection.frontendPreferenceKeys.includes(key)} onchange={event => settingsSelection = { ...settingsSelection, frontendPreferenceKeys: toggleKey(settingsSelection.frontendPreferenceKeys, key, event.currentTarget.checked) }} /><span>{key}</span></label>{/each}
          {#if settingsPreview.warnings.length}<ul class="warnings">{#each settingsPreview.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}
          <div class="actions"><button type="button" disabled={settingsBusy} onclick={() => settingsPreview = null}>Cancel</button><button type="button" class="primary" disabled={settingsBusy || !settingsSelection.application && !settingsSelection.preferenceKeys.length && !settingsSelection.frontendPreferenceKeys.length} onclick={() => void applySettingsArchive()}>Apply selected settings</button></div>
        </section>
      {/if}
    </section>
  {/if}

  {#if automaticJob}<div class="progress automatic-progress" aria-label="Automatic backup progress" role="status" aria-live="polite"><span>{automaticJob.message || automaticJob.phase || (automaticJob.state === "complete" ? "Automatic backup complete." : automaticJob.state === "failed" ? "Automatic backup failed." : automaticJob.state === "canceled" ? "Automatic backup canceled." : "Automatic backup running…")}</span>{#if automaticJob.total != null}<progress max={Math.max(1, automaticJob.total)} value={Math.min(automaticJob.completed ?? 0, automaticJob.total)}></progress>{/if}{#if automaticJob.state === "running" || automaticJob.state === "queued"}{#if cancelJob && automaticJob.jobId}<button type="button" disabled={automaticCancelRequested} onclick={() => void cancelAutomaticBackup()}>{automaticCancelRequested ? "Cancellation requested…" : "Cancel automatic backup"}</button>{/if}{/if}</div>{/if}
  {#if busy && progress}<div class="progress" role="status" aria-live="polite"><span>{progress.message || progress.phase || (progress.state === "running" ? "Working…" : "Preparing…")}</span>{#if progress.total != null}<progress max={Math.max(1, progress.total)} value={Math.min(progress.completed ?? 0, progress.total)}></progress>{/if}{#if cancelJob && jobId}<button type="button" onclick={() => void cancel()}>Cancel operation</button>{/if}</div>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
</section>

<style>
  .backup-restore{display:grid;gap:16px;max-width:760px;color:var(--text)}
  h2,h3,p{margin:0}h2{font-size:1.25rem}h3{font-size:1rem}
  header p,.backup-card>p{margin-top:5px;color:var(--muted);font-size:.88rem;line-height:1.5}
  .backup-card{display:grid;gap:10px;padding:16px;border:1px solid var(--line);border-radius:10px;background:var(--surface)}
  button{justify-self:start;border:1px solid var(--line);border-radius:7px;padding:8px 12px;background:var(--surface-2);color:var(--text);font:inherit;cursor:pointer}
  button.primary{background:var(--accent);border-color:var(--accent);color:var(--on-accent,#172018);font-weight:650}
  button:disabled{opacity:.55;cursor:default}
  .option{display:flex;align-items:flex-start;gap:9px;color:var(--text);font-size:.88rem;line-height:1.4}
  .option input{margin-top:3px;accent-color:var(--accent)}.option span{display:grid;gap:2px}.option small{color:var(--muted);font-size:.8rem}.unavailable{opacity:.65}
  .preview{display:grid;gap:12px;margin-top:5px;padding-top:13px;border-top:1px solid var(--line)}
  .restore-summary,.settings-preview{display:grid;gap:8px;padding:10px;border:1px solid var(--line);border-radius:8px;font-size:.84rem}.restore-summary>div{display:flex;flex-wrap:wrap;gap:6px}.restore-summary span{padding:3px 7px;border-radius:999px;background:var(--surface-2)}.restore-summary small,.settings-preview p{color:var(--muted);font-size:.8rem;line-height:1.4}.settings-actions{justify-content:flex-start}.settings-preview .option{padding:4px 0}
  .preview-heading{display:flex;justify-content:space-between;gap:12px;flex-wrap:wrap}.preview-heading span,.preview>p,.media-summary{color:var(--muted);font-size:.82rem}
  .media-restore{display:grid;gap:9px;padding:10px;border:1px solid var(--line);border-radius:8px}.media-root{display:flex;justify-content:space-between;align-items:center;gap:12px;padding:8px 0 0 25px;border-top:1px solid var(--line);font-size:.84rem}.media-root>span{display:grid;gap:3px}.media-root small{color:var(--muted);font-size:.78rem}
  .schedule-row{display:flex;align-items:center;justify-content:space-between;gap:12px;color:var(--muted);font-size:.84rem}.schedule-grid{display:grid;grid-template-columns:1fr 1fr;gap:12px}.backup-card>label:not(.option),.schedule-grid label{display:grid;gap:5px;font-size:.84rem;color:var(--muted)}.backup-card select,.backup-card input[type=number]{max-width:180px;padding:7px;border:1px solid var(--line);border-radius:6px;background:var(--surface-2);color:var(--text);font:inherit}
  fieldset{display:grid;gap:8px;margin:0;padding:10px 12px;border:1px solid var(--line);border-radius:8px}legend{padding:0 5px;font-weight:600;font-size:.84rem}
  .mode-choice{grid-template-columns:1fr 1fr}.mode-choice legend{grid-column:1/-1}.mode-choice .option{padding:7px;border-radius:6px}.mode-choice .option:has(input:checked){background:color-mix(in srgb,var(--accent) 10%,transparent)}
  .acknowledge{padding:10px;border-left:3px solid var(--warning,#d39c45);background:color-mix(in srgb,var(--warning,#d39c45) 9%,transparent)}
  .warnings{margin:0;padding-left:20px;color:var(--warning,#d39c45);font-size:.83rem;line-height:1.5}
  .actions{display:flex;justify-content:flex-end;gap:8px;padding-top:6px;border-top:1px solid var(--line)}
  .progress{display:grid;gap:7px;padding:12px;border-radius:8px;background:var(--surface-2);font-size:.85rem}.progress progress{width:100%;height:7px;accent-color:var(--accent)}
  .error,.notice{margin:0;padding:10px 12px;border-radius:7px;font-size:.86rem;line-height:1.45}.error{color:var(--danger,#e48585);background:color-mix(in srgb,var(--danger,#e48585) 10%,transparent)}.notice{color:var(--success,#74c69d);background:color-mix(in srgb,var(--success,#74c69d) 10%,transparent)}
  @media(max-width:520px){.mode-choice{grid-template-columns:1fr}.mode-choice legend{grid-column:auto}.actions{flex-wrap:wrap}.actions button{flex:1}}
</style>
