<script lang="ts">
  import { loudnessMeasurementSourceLabel, type LoudnessAnalysisJobStatus, type LoudnessPlaybackStatus, type LoudnessSettings as Settings, type NormalizationMode } from "./lib/loudness-normalization";
  import InfoPopover from "./InfoPopover.svelte";
  import { call } from "./lib/api";

  let { settings, onSettingsSaved, playbackStatus = null, webviewRouteStatus = null, engine = "webview", analysisJob = null, pendingAnalysisCount = 0, analysisMessage = "", onCancelAnalysis, onRetryAnalysis }: {
    settings: Settings;
    onSettingsSaved: (settings: Settings) => void | Promise<void>;
    playbackStatus?: LoudnessPlaybackStatus | null;
    webviewRouteStatus?: string | null;
    engine?: "native" | "webview" | "mpv";
    analysisJob?: LoudnessAnalysisJobStatus | null;
    pendingAnalysisCount?: number;
    analysisMessage?: string;
    onCancelAnalysis?: () => void;
    onRetryAnalysis?: () => void;
  } = $props();

  let mode = $derived(settings.mode);
  let saveError = $state("");

  async function changeMode(value: string): Promise<void> {
    if (value !== "off" && value !== "track" && value !== "album") return;
    saveError = "";
    const next: Settings = { ...settings, mode: value as NormalizationMode };
    try {
      await call("set_loudness_settings", { settings: next });
      await onSettingsSaved(next);
    } catch (error) {
      saveError = error instanceof Error ? error.message : "Could not save loudness settings.";
    }
  }

  async function changeAnalyzeWhenIdle(enabled: boolean): Promise<void> {
    saveError = "";
    const next: Settings = { ...settings, analyzeWhenIdle: enabled };
    try { await call("set_loudness_settings", { settings: next }); await onSettingsSaved(next); }
    catch (error) { saveError = error instanceof Error ? error.message : "Could not save the idle analysis setting."; }
  }

  function statusMessage(): string {
    if (mode === "off") return "Off: playback levels are unchanged.";
    if (engine === "mpv") return "Normalization is unavailable in external mpv playback.";
    if (engine === "webview" && webviewRouteStatus && !webviewRouteStatus.toLowerCase().includes("route active")) return webviewRouteStatus;
    if (playbackStatus?.status === "off") return "Waiting for playback to select normalization.";
    if (!playbackStatus || playbackStatus.status === "unavailable") return playbackStatus?.reason ?? "Waiting for a trusted measurement for this audio source.";
    const source = loudnessMeasurementSourceLabel(playbackStatus.measurementSource);
    if (playbackStatus.status === "album_fallback_track") {
      return playbackStatus.reason
        ? `Using track gain${gainText()} · ${source}. ${playbackStatus.reason}`
        : `Using track gain because trusted album analysis is unavailable${gainText()} · ${source}.`;
    }
    return `Applied ${gainText().trim()} · ${source}.${playbackStatus.reason ? ` ${playbackStatus.reason}` : ""}`;
  }

  function gainText(): string {
    const gain = playbackStatus?.appliedGainDb;
    if (typeof gain !== "number" || !Number.isFinite(gain)) return "";
    return ` ${gain > 0 ? "+" : ""}${gain.toFixed(1)} dB`;
  }
</script>

<section class="loudness-settings" aria-labelledby="loudness-title">
  <div class="heading"><h3 id="loudness-title">Even out volume</h3><InfoPopover label="About loudness normalization">Adjusts playback levels without changing your audio files or player volume.</InfoPopover></div>
  <label>
    Adjust volume by
    <select aria-label="Loudness normalization mode" value={mode} onchange={event => changeMode(event.currentTarget.value)}>
      <option value="off">Off</option>
      <option value="track">Each track</option>
      <option value="album">Album</option>
    </select>
  </label>
  <details class="advanced-options">
    <summary>Advanced</summary>
    <div class="advanced-content">
      <label class="idle-option"><input type="checkbox" checked={settings.analyzeWhenIdle} onchange={event => changeAnalyzeWhenIdle(event.currentTarget.checked)} /> Run selected analysis only while paused</label>
      <p>Whole-file analysis uses CPU and disk. Waiting keeps playback responsive.</p>
      <p>Pending selected work is kept for this session. Playback resuming cancels the current pass and queues those tracks again; nothing is scanned automatically at startup.</p>
    </div>
  </details>
  {#if saveError}<p role="alert">{saveError}</p>{/if}
  <p class="loudness-status" role="status">{statusMessage()}</p>
  {#if analysisJob || pendingAnalysisCount || analysisMessage}<section class="analysis-status" aria-label="Selected loudness analysis">
    {#if analysisJob}<p>{analysisJob.status === "completed" ? "Analysis complete" : analysisJob.status === "failed" ? "Analysis failed: " + (analysisJob.error ?? "unknown error") : analysisJob.status === "cancelled" ? "Analysis paused" : "Analyzing " + analysisJob.completed + " of " + analysisJob.total + " selected tracks"}</p>{#if analysisJob.status === "queued" || analysisJob.status === "running"}<progress max={analysisJob.total || 1} value={analysisJob.completed}></progress>{/if}{/if}
    {#if pendingAnalysisCount}<p>{pendingAnalysisCount} explicitly selected tracks queued until playback is idle.</p>{/if}
    {#if analysisMessage}<p>{analysisMessage}</p>{/if}
    <div>{#if onCancelAnalysis && (analysisJob?.status === "queued" || analysisJob?.status === "running" || pendingAnalysisCount)}<button type="button" onclick={onCancelAnalysis}>Cancel loudness analysis</button>{/if}{#if onRetryAnalysis && pendingAnalysisCount && (!analysisJob || analysisJob.status === "failed" || analysisJob.status === "cancelled")}<button type="button" onclick={onRetryAnalysis}>Retry selected analysis</button>{/if}</div>
  </section>{/if}
</section>

<style>
  .loudness-settings{display:grid;gap:.5rem;padding:.85rem 0;border-top:1px solid var(--line,#454a50)}
  .heading{display:flex;align-items:center;gap:8px}
  h3{margin:0;font-size:1rem}label{display:grid;gap:.35rem;max-width:28rem;font-size:.86rem}
  select{padding:.5rem;border:1px solid var(--line,#454a50);border-radius:7px;background:var(--surface,#202723);color:var(--text,#e5ebe7)}
  p{margin:0;color:var(--muted,#a3a8ae);font-size:.82rem;line-height:1.45}.loudness-status{color:var(--text,#e5ebe7)}.advanced-options{max-width:38rem;border-top:1px solid var(--line,#454a50);padding-top:.45rem}.advanced-options>summary{width:max-content;color:var(--text,#e5ebe7);font-size:.82rem;cursor:pointer}.advanced-content{display:grid;gap:.45rem;padding:.55rem 0 0}.idle-option{display:flex;align-items:flex-start;gap:.55rem;max-width:38rem}.idle-option input{width:auto;margin:.18rem 0 0}.analysis-status{display:grid;gap:.4rem;padding:.65rem;border:1px solid var(--line,#454a50);border-radius:7px}.analysis-status div{display:flex;gap:.5rem}.analysis-status progress{width:100%}
</style>
