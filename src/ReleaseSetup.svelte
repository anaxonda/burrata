<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  import { call, desktop, listen } from "./lib/api";
  import { browserCompanionStatus } from "./lib/browser-companion-status";
  import type { BrowserStatus } from "./lib/types";

  type BrowserInfo = { browser: string; installed: boolean };
  type InstallStatus = { browsers: BrowserInfo[] };
  type Setup = {
    setupComplete: boolean;
    musicDirectory: string | null;
    radioRecognition: boolean;
    mcpCommand: string[];
    browser: InstallStatus;
    toolPaths: { name: string; path: string; origin: string }[];
    usesManagedYtDlp?: boolean;
  };
  type YoutubeStatus = {
    managedVersion: string | null;
    activeVersion: string | null;
    activeSource: "bundled" | "managed";
  };
  type UpdateChannel = "stable" | "nightly";
  type UpdateCheck = { version: string | null; available: boolean };
  type UpdateProgress =
    | { stage: "checking" | "verifying" }
    | { stage: "downloading"; receivedBytes: number; totalBytes: number | null };
  type UpdateAvailability = { result?: UpdateCheck; checking: boolean; error?: string };
  type Diagnosis = { ok: boolean; category: string; ytDlpVersion: string | null; providerAvailable: boolean; audioAccessible: boolean; message: string };
  let { onSettingsChanged = () => {}, onComplete = () => {}, onboarding = false, category = "all", browserStatus = null }: {
    category?: string;
    browserStatus?: BrowserStatus | null;
    onSettingsChanged?: () => void; onComplete?: () => void; onboarding?: boolean;
  } = $props();

  let setup = $state<Setup | null>(null), youtube = $state<YoutubeStatus | null>(null);
  let loading = $state(true), busy = $state(false), error = $state(""), message = $state("");
  let radioRecognition = $state(false), musicDirectory = $state("");
  let diagnosis = $state<Diagnosis | null>(null), diagnosisUrl = $state("");
  let browserBusy = $state<Record<string, boolean>>({});

  let availability = $state<Partial<Record<UpdateChannel, UpdateAvailability>>>({});
  let updateProgress = $state<UpdateProgress | null>(null);

  async function checkYoutube(channel: UpdateChannel) {
    if (availability[channel]?.checking) return;
    availability[channel] = { checking: true };
    try {
      const result = await call<UpdateCheck>("youtube_support_check", { channel });
      availability[channel] = { result, checking: false };
    } catch {
      availability[channel] = { checking: false, error: "Could not check for updates." };
    }
  }

  let refreshEpoch = 0;
  async function refresh(currentCategory: string) {
    const epoch = ++refreshEpoch;
    loading = true; error = "";
    try {
      const [nextSetup, nextYoutube] = await Promise.all([
        call<Setup>("get_release_setup"), currentCategory === "all" || currentCategory === "advanced" ? call<YoutubeStatus>("youtube_support_status") : Promise.resolve(null),
      ]);
      if (epoch !== refreshEpoch) return;
      setup = nextSetup; youtube = nextYoutube;
      radioRecognition = nextSetup.radioRecognition;
      musicDirectory = nextSetup.musicDirectory ?? "";
      if (!onboarding && nextYoutube && nextSetup.usesManagedYtDlp !== false) {
        void checkYoutube("stable");
      }
    } catch (cause) { if (epoch === refreshEpoch) error = String(cause); }
    finally { if (epoch === refreshEpoch) loading = false; }
  }
  $effect(() => { const currentCategory = category; if (desktop()) void refresh(currentCategory); else { loading = false; error = "Setup is available in the Burrata desktop app."; } });

  async function chooseDirectory() {
    busy = true; error = "";
    try {
      const selected = await call<string | null>("choose_music_directory");
      if (selected) { musicDirectory = selected; if (setup) setup = { ...setup, musicDirectory: selected }; onSettingsChanged(); }
    } catch (cause) { error = `Could not choose a music folder: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function completeSetup() {
    busy = true; error = "";
    try {
      await call<void>("complete_release_setup", { radioRecognition });
      if (setup) setup = { ...setup, setupComplete: true, musicDirectory: musicDirectory || null, radioRecognition };
      message = "Setup saved."; onComplete(); onSettingsChanged();
    } catch (cause) { error = `Could not save setup: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function saveRecognition(enabled: boolean) {
    const previous = setup?.radioRecognition ?? !enabled;
    radioRecognition = enabled; busy = true; error = ""; message = "";
    try {
      await call<void>("set_radio_recognition", { enabled });
      if (setup) setup = { ...setup, radioRecognition: enabled };
      message = "Radio recognition setting saved."; onSettingsChanged();
    } catch (cause) { radioRecognition = previous; error = `Could not save radio recognition setting: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function updateBrowserHost(remove: boolean) {
    browserBusy.all = true; error = "";
    try {
      await call<InstallStatus>(remove ? "unregister_browser_host" : "register_browser_host");
      setup = await call<Setup>("get_release_setup");
      message = remove ? "Browser host registration removed." : "Browser host registered. Open the extension popup to connect.";
      onSettingsChanged();
    } catch (cause) { error = `Could not update browser companion: ${String(cause)}`; }
    finally { browserBusy.all = false; }
  }
  async function updateYoutube(channel: UpdateChannel) {
    if (busy || setup?.usesManagedYtDlp === false) return;
    const previousVersion = youtube?.activeVersion;
    busy = true; error = "";
    message = channel === "stable" ? "Checking for a stable yt-dlp update…" : "Installing the latest yt-dlp nightly…";
    let unlisten: (() => void) | undefined;
    try {
      const operationId = crypto.randomUUID();
      unlisten = await listen<{ operationId: string; progress: UpdateProgress }>("youtube-support-update-progress", ({ payload }) => {
        if (payload.operationId !== operationId) return;
        updateProgress = payload.progress;
        if (payload.progress.stage !== "checking") message = "";
      });
      updateProgress = { stage: "checking" };
      youtube = await call<YoutubeStatus>("youtube_support_update", { channel, operationId });
      if (youtube.activeVersion && youtube.activeVersion === previousVersion) {
        message = channel === "stable"
          ? "Your yt-dlp is already as recent as the latest stable release."
          : "The latest nightly is already installed.";
      } else {
        const build = channel === "stable" ? "stable release" : "nightly build";
        message = youtube.activeVersion ? `Installed yt-dlp ${youtube.activeVersion} (${build}).` : `Installed the yt-dlp ${build}.`;
      }
      void checkYoutube(channel);
      if (channel === "nightly") void checkYoutube("stable");
    } catch (cause) {
      message = "";
      error = `Could not update yt-dlp: ${String(cause)}`;
    }
    finally {
      unlisten?.();
      updateProgress = null;
      busy = false;
    }
  }
  async function rollbackYoutube() {
    if (busy || setup?.usesManagedYtDlp === false) return;
    busy = true; error = ""; message = "";
    try {
      youtube = await call<YoutubeStatus>("youtube_support_rollback");
      message = youtube.activeVersion ? `Restored yt-dlp ${youtube.activeVersion}.` : "Restored the previous yt-dlp version.";
      void checkYoutube("stable");
      if (availability.nightly) void checkYoutube("nightly");
    } catch (cause) { error = `Could not restore the previous yt-dlp version: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function diagnose() {
    busy = true; error = ""; diagnosis = null;
    try {
      diagnosis = await call<Diagnosis>("youtube_support_diagnose", { ...(diagnosisUrl.trim() ? { url: diagnosisUrl.trim() } : {}) });
      youtube = await call<YoutubeStatus>("youtube_support_status");
    } catch (cause) { error = `Could not diagnose YouTube support: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function openUrl(url: string) {
    try { await call("open_external_url", { url }); }
    catch (cause) { error = `Could not open link: ${String(cause)}`; }
  }
  function diagnosisNextStep(category: string): string {
    switch (category) {
      case "network": return "Check your internet connection and try again.";
      case "invalidUrl": case "unsupported": return "Check the YouTube link in troubleshooting options and try again.";
      case "videoUnavailable": return "Try another public YouTube video.";
      case "loginRequired": return "Try a public video that does not require signing in.";
      case "http403": case "expiredStream": return "Try the check again. If it still fails, update yt-dlp.";
      case "missingRuntime": case "missingEjs": case "tokenProvider": return "Check the included tools or try updating yt-dlp.";
      default: return "Try the check again or review the troubleshooting options.";
    }
  }
  function updateLabel(channel: UpdateChannel) {
    const build = channel === "stable" ? "stable release" : "nightly build";
    const state = availability[channel];
    if (state?.checking) return `Checking for the latest ${build}…`;
    if (state?.error) return state.error;
    const version = state?.result?.version;
    if (!version) return "";
    return state.result?.available
      ? `yt-dlp ${version} is available (${build}).`
      : channel === "stable"
        ? `Latest stable release is ${version}. The installed version is current or newer.`
        : `The latest nightly (${version}) is already installed.`;
  }
  const browserLabel = (name: string) => name.charAt(0).toUpperCase() + name.slice(1);
</script>

<section class="release-setup settings-card" class:troubleshooting={category === "advanced"} aria-labelledby="release-setup-title">
  <h2 id="release-setup-title">{onboarding ? "Welcome to Burrata" : category === "connections" ? "Browser companion" : category === "playback" ? "Radio" : category === "integrations" ? "Local integrations" : "YouTube support"}</h2>
  {#if onboarding}<p class="intro">Choose where Burrata keeps music and connect optional desktop tools.</p>{/if}
  {#if loading}<p role="status">Loading setup…</p>
  {:else if error && !setup && !youtube}<p role="alert">{error}</p>
  {:else}
    {#if onboarding && !setup?.setupComplete}
      <section class="setup-step" aria-labelledby="first-run-title">
        <h3 id="first-run-title">Choose your music folder</h3>
        <p>Burrata uses this folder for music you choose to download. You can skip this and set it later.</p>
        <div class="directory-row"><span title={musicDirectory || "No folder selected"}>{musicDirectory || "No folder selected"}</span><button disabled={busy} onclick={() => void chooseDirectory()}>Choose folder</button></div>
        <label class="check"><input type="checkbox" bind:checked={radioRecognition} /> Enable radio track recognition</label>
        <button class="primary" disabled={busy} onclick={() => void completeSetup()}>{busy ? "Saving…" : "Complete setup"}</button>
        <button class="quiet" disabled={busy} onclick={() => void completeSetup()}>Skip for now</button>
      </section>
    {/if}

    {#if category === "all" || category === "connections"}<section class="tool-section" aria-label="Browser companion">
      {#if category === "all"}<h3 id="browser-title">Browser companion</h3>{/if}
      <p>Connect a browser to import charts and playlists. Burrata reads pages only when you use the extension; it does not read cookies or logins.</p>
      <p role="status">{browserCompanionStatus(browserStatus)}</p>
      {#if setup?.browser.browsers.length}
        <div class="browser-list">{#each setup.browser.browsers as item (item.browser)}
          <div class="browser-row"><div><strong>{browserLabel(item.browser)}</strong><small>{item.installed ? "Browser host registered" : "Browser host not registered"}</small></div></div>
        {/each}</div>
      {:else}<p class="muted">No supported browsers were detected.</p>{/if}
      <div class="browser-actions"><button disabled={!!browserBusy.all} onclick={() => void updateBrowserHost(false)}>{browserBusy.all ? "Working…" : "Connect browser companion"}</button><button class="quiet" disabled={!!browserBusy.all || !setup?.browser.browsers.some((item) => item.installed)} onclick={() => void updateBrowserHost(true)}>Disconnect browser companion</button></div>
      <details><summary>Install the browser extension</summary>
        <p>For Chromium browsers, open Extensions, enable developer mode, then choose “Load unpacked” and select the Burrata extension folder. For Firefox, install the signed XPI from Burrata releases.</p>
        {#if setup?.toolPaths.length}<ul class="paths">{#each setup.toolPaths as tool (tool.name)}<li><strong>{tool.name}</strong><code>{tool.path}</code><small>{tool.origin}</small></li>{/each}</ul>{/if}
        <button onclick={() => void openUrl("https://github.com/anaxonda/burrata/releases")}>Open Burrata releases ↗</button>
      </details>
      {#if category === "all" && setup?.mcpCommand?.length}<details><summary>MCP command</summary><p>Use this command to connect Burrata tools to an MCP-compatible client.</p><code class="command">{setup.mcpCommand.join(" ")}</code></details>{/if}
    </section>

    {/if}
    {#if (category === "connections" || category === "integrations") && setup?.mcpCommand?.length}<details><summary>Connect an assistant</summary><p>Use this command to connect a compatible assistant to Burrata.</p><code class="command">{setup.mcpCommand.join(" ")}</code></details>{/if}
    {#if category === "all" || category === "playback"}<section class="tool-section" aria-labelledby="recognition-title">
      <div class="section-heading"><h3 id="recognition-title">Radio recognition</h3><InfoPopover label="About radio recognition">Burrata sends a short sample of the radio stream for identification. This uses network data and processing.</InfoPopover></div>
      <label class="check"><input type="checkbox" checked={radioRecognition} disabled={busy} onchange={(event) => void saveRecognition(event.currentTarget.checked)} /> Enable radio track recognition</label>
    </section>

    {/if}
    {#if category === "all" || category === "advanced"}<section class="tool-section" aria-label="YouTube support">
      {#if category === "all"}<h3 id="youtube-title">YouTube support</h3>{/if}
      {#if youtube}
        {#if setup?.usesManagedYtDlp === false}
          <p>Update your configured yt-dlp outside Burrata.</p>
        {:else}
          <p class="tool-version">
            yt-dlp {youtube.activeVersion ?? "version unavailable"} · {youtube.activeSource === "bundled" ? "Included with Burrata" : "Installed update"}
          </p>
          <button class="primary" disabled={busy} aria-describedby="yt-dlp-stable-description" onclick={() => void updateYoutube("stable")}>Update yt-dlp</button>
          <p class="update-description" id="yt-dlp-stable-description">Installs a newer stable release when available.</p>
          {#if updateLabel("stable")}<p role="status">{updateLabel("stable")}</p>{/if}
          {#if availability.stable?.error}<button disabled={busy} onclick={() => void checkYoutube("stable")}>Check again</button>{/if}
        {/if}
        {#if updateProgress?.stage === "downloading"}
          {@const received = updateProgress.receivedBytes}
          {@const total = updateProgress.totalBytes}
          <div class="update-progress">
            <label>Downloading yt-dlp
              {#if total}<progress max={total} value={Math.min(received, total)}></progress>
              {:else}<progress></progress>{/if}
            </label>
            {#if total}
              <span>{Math.round(Math.min(received / total, 1) * 100)}%</span>
            {:else}<span>{(received / 1024 / 1024).toFixed(1)} MiB downloaded</span>{/if}
          </div>
        {:else if updateProgress?.stage === "verifying"}
          <p role="status">Verifying the download and checking that it works…</p>
        {/if}
        <details class="youtube-details" ontoggle={({ currentTarget }) => {
          if (currentTarget.open && setup?.usesManagedYtDlp !== false && !availability.nightly) void checkYoutube("nightly");
        }}><summary>Advanced options</summary>
          {#if setup?.usesManagedYtDlp !== false}
            <div class="update-actions">
              <button disabled={busy} onclick={() => void updateYoutube("nightly")}>Install latest nightly build</button>
              <small>Nightly builds get fixes sooner and may be less reliable.</small>
              {#if updateLabel("nightly")}<p role="status">{updateLabel("nightly")}</p>{/if}
              {#if availability.nightly?.error}<button disabled={busy} onclick={() => void checkYoutube("nightly")}>Check nightly again</button>{/if}
              {#if youtube.managedVersion}<button disabled={busy} onclick={() => void rollbackYoutube()}>Restore previous version</button>{/if}
            </div>
          {/if}
          <details><summary>Test YouTube audio access</summary>
            <p>Test a public video without signing in.</p>
            <label>Optional YouTube URL<input aria-label="YouTube URL for diagnosis" bind:value={diagnosisUrl} placeholder="https://www.youtube.com/watch?v=…" /></label>
            <button disabled={busy} onclick={() => void diagnose()}>{busy ? "Checking…" : "Check YouTube audio access"}</button>
            {#if diagnosis}
              <p role="status">{diagnosis.message}</p>
              {#if !diagnosis.ok}<p>{diagnosisNextStep(diagnosis.category)}</p>{/if}
            {/if}
          </details>
        </details>
      {:else}<p role="status">YouTube support status is unavailable.</p>{/if}
    </section>{/if}
  {/if}
  {#if error && setup}<p role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</section>

<style>
  .update-progress { display: flex; align-items: center; gap: 0.75rem; }
  .update-progress label { display: grid; gap: 0.25rem; }
  .update-progress progress { width: min(19rem, 50vw); accent-color: var(--accent); }

  .release-setup{min-width:0}.intro,.tool-section>p,.setup-step>p{color:var(--muted);line-height:1.5}
  .setup-step,.tool-section{min-width:0;margin:14px 0;padding:14px;border:1px solid var(--line);border-radius:10px;background:color-mix(in srgb,var(--surface) 80%,transparent)}
  .troubleshooting > h2{font-size:16px;margin:0 0 8px}.troubleshooting > .tool-section{margin:0;padding:0;border:0;background:transparent}.troubleshooting .tool-section > p{font-size:.82rem;margin:10px 0;line-height:1.45}.troubleshooting .youtube-details{margin:12px 0 0;padding:0;border:0;border-radius:0}.troubleshooting .youtube-details > summary{font-size:12px}
  .section-heading{display:flex;align-items:center;gap:8px}.section-heading h3{margin:0}
  h3{margin:0 0 8px;font-size:15px}.directory-row,.browser-row{display:flex;align-items:center;justify-content:space-between;gap:10px;min-width:0}
  .directory-row{margin:12px 0}.directory-row span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted)}
  .browser-list{display:grid;gap:8px}.browser-row{padding:8px 10px;border-radius:7px;background:var(--raised)}.browser-row>div{display:grid;gap:3px}.browser-row small,.paths small,.muted{color:var(--muted)}
  label{display:grid;gap:6px;margin:12px 0}.check{display:flex;flex-flow:row nowrap;align-items:flex-start;gap:8px;min-width:0}.check input[type="checkbox"]{appearance:auto;flex:0 0 16px;width:16px;height:16px;min-width:16px;margin:2px 0 0;padding:0;border-radius:3px}.tool-version{font-size:13px}
  details{margin:12px 0;padding:10px;border:1px solid var(--line);border-radius:8px}summary{cursor:pointer;font-weight:600}code{overflow-wrap:anywhere}.command{display:block;padding:9px;background:var(--raised);border-radius:6px}
  .paths{padding-left:20px}.paths li{display:grid;gap:3px;margin:10px 0}.paths code{color:var(--muted);font-size:11px}.update-actions{display:grid;justify-items:start;gap:8px;margin-top:12px}
  .quiet{background:transparent}.primary{font-weight:600}.release-setup button{max-width:100%}
  @media(max-width:520px){.setup-step,.tool-section{padding:11px}.directory-row,.browser-row{align-items:flex-start}.directory-row{flex-direction:column}}
</style>
