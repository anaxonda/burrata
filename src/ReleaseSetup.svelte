<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  import { call, desktop } from "./lib/api";

  type BrowserInfo = { browser: string; installed: boolean };
  type InstallStatus = { browsers: BrowserInfo[] };
  type Setup = {
    setupComplete: boolean;
    musicDirectory: string | null;
    radioRecognition: boolean;
    mcpCommand: string[];
    browser: InstallStatus;
    toolPaths: { name: string; path: string; origin: string }[];
  };
  type YoutubeStatus = {
    bundledVersion: string | null; managedVersion: string | null; activeVersion: string | null;
    activeSource: string; updateAvailable: boolean | null; recommendedUpdate: string;
    advancedUpdate: string; provider: { version: string; available: boolean; helperRunning: boolean; message: string };
  };
  type UpdateCheck = { channel: string; version: string | null; publishedAt: string | null; available: boolean; message: string };
  type Diagnosis = { ok: boolean; category: string; ytDlpVersion: string | null; providerAvailable: boolean; audioAccessible: boolean; message: string };
  let { onSettingsChanged = () => {}, onComplete = () => {}, onboarding = false, category = "all" }: {
    category?: string;
    onSettingsChanged?: () => void; onComplete?: () => void; onboarding?: boolean;
  } = $props();

  let setup = $state<Setup | null>(null), youtube = $state<YoutubeStatus | null>(null);
  let loading = $state(true), busy = $state(false), error = $state(""), message = $state("");
  let radioRecognition = $state(false), musicDirectory = $state("");
  let checks = $state<Record<string, UpdateCheck | null>>({ recommended: null, nightly: null });
  let diagnosis = $state<Diagnosis | null>(null), diagnosisUrl = $state("");
  let browserBusy = $state<Record<string, boolean>>({});

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
      message = remove ? "Disconnected the browser companion." : "Connected the browser companion.";
      onSettingsChanged();
    } catch (cause) { error = `Could not update browser companion: ${String(cause)}`; }
    finally { browserBusy.all = false; }
  }
  async function checkUpdate(channel: "recommended" | "nightly") {
    busy = true; error = ""; checks[channel] = null;
    try { checks[channel] = await call<UpdateCheck>("youtube_support_check", { channel: channel === "nightly" ? "nightly" : "recommended" }); }
    catch (cause) { error = `Could not check ${channel === "nightly" ? "nightly" : "recommended"} updates: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function updateYoutube(channel: "recommended" | "nightly") {
    busy = true; error = "";
    try {
      youtube = await call<YoutubeStatus>("youtube_support_update", { channel });
      checks[channel] = null; message = "YouTube support update installed.";
    } catch (cause) { error = `Could not update YouTube support: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function rollbackYoutube() {
    busy = true; error = "";
    try { youtube = await call<YoutubeStatus>("youtube_support_rollback"); message = "YouTube support was rolled back."; }
    catch (cause) { error = `Could not roll back YouTube support: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function diagnose() {
    busy = true; error = ""; diagnosis = null;
    try {
      diagnosis = await call<Diagnosis>("youtube_support_diagnose", { ...(diagnosisUrl.trim() ? { url: diagnosisUrl.trim() } : {}) });
      youtube = await call<YoutubeStatus>("youtube_support_status");
      // A repair is offered only after diagnosis and remains an explicit user action.
      if (!diagnosis.ok) await checkUpdate("recommended");
    } catch (cause) { error = `Could not diagnose YouTube support: ${String(cause)}`; }
    finally { busy = false; }
  }
  async function openUrl(url: string) {
    try { await call("open_external_url", { url }); }
    catch (cause) { error = `Could not open link: ${String(cause)}`; }
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
      {#if setup?.browser.browsers.length}
        <div class="browser-list">{#each setup.browser.browsers as item (item.browser)}
          <div class="browser-row"><div><strong>{browserLabel(item.browser)}</strong><small>{item.installed ? "Browser host registered" : "Not connected"}</small></div></div>
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
    {#if category === "integrations" && setup?.mcpCommand?.length}<details><summary>Connect an assistant</summary><p>Use this command to connect a compatible assistant to Burrata.</p><code class="command">{setup.mcpCommand.join(" ")}</code></details>{/if}
    {#if category === "all" || category === "playback"}<section class="tool-section" aria-labelledby="recognition-title">
      <div class="section-heading"><h3 id="recognition-title">Radio recognition</h3><InfoPopover label="About radio recognition">Burrata sends a short sample of the radio stream for identification. This uses network data and processing.</InfoPopover></div>
      <label class="check"><input type="checkbox" checked={radioRecognition} disabled={busy} onchange={(event) => void saveRecognition(event.currentTarget.checked)} /> Enable radio track recognition</label>
    </section>

    {/if}
    {#if category === "all" || category === "advanced"}<section class="tool-section" aria-label="YouTube support">
      {#if category === "all"}<h3 id="youtube-title">YouTube support</h3>{/if}
      {#if youtube}
        <p>{diagnosis ? (diagnosis.ok ? "YouTube playback check passed." : "YouTube playback could not be confirmed.") : youtube.provider.available ? "YouTube support is included. Playback has not been checked yet." : "YouTube support needs attention. Check playback to find out more."}</p>
        <p>Check whether Burrata can play public YouTube audio. No account or cookies are used.</p>
        <button disabled={busy} onclick={() => void diagnose()}>{busy ? "Checking…" : "Check YouTube playback"}</button>
        {#if diagnosis}
          <span role="status" class="sr-only">{diagnosis.ok ? "YouTube playback check passed." : "YouTube playback could not be confirmed."}</span>
          {#if !diagnosis.ok && checks.recommended}
            <p role="status">{checks.recommended.message}</p>
            {#if checks.recommended.available}<button class="primary" disabled={busy} onclick={() => void updateYoutube("recommended")}>Install recommended update</button>{/if}
          {/if}
        {/if}
        <details class="youtube-details"><summary>More troubleshooting options</summary>
          <p class="tool-version">Active yt-dlp: {youtube.activeVersion ?? "version unavailable"} · {youtube.activeSource}</p>
          <p>{youtube.provider.message}</p>
          <label>Optional YouTube URL<input aria-label="YouTube URL for diagnosis" bind:value={diagnosisUrl} placeholder="https://www.youtube.com/watch?v=…" /></label>
          {#if diagnosis}<p>{diagnosis.message}</p><small>Result: {diagnosis.category} · yt-dlp {diagnosis.ytDlpVersion ?? "unknown"} · provider {diagnosis.providerAvailable ? "available" : "unavailable"} · audio {diagnosis.audioAccessible ? "accessible" : "not accessible"}</small>{/if}
          <div class="update-actions">
            <button disabled={busy} onclick={() => void checkUpdate("recommended")}>Check recommended update</button>
            {#if checks.recommended && (!diagnosis || diagnosis.ok)}<p role="status">{checks.recommended.message}</p>{#if checks.recommended.available}<button class="primary" disabled={busy} onclick={() => void updateYoutube("recommended")}>Install recommended update</button>{/if}{/if}
            <details><summary>Official yt-dlp nightly</summary><p>Nightly builds can change unexpectedly. Check only if you choose to try the advanced update channel.</p>
              <button disabled={busy} onclick={() => void checkUpdate("nightly")}>Check nightly</button>
              {#if checks.nightly}<p role="status">{#if checks.nightly.version}Latest nightly: {checks.nightly.version} · {checks.nightly.available ? "Update available" : "Already installed"}{:else}{checks.nightly.message}{/if}</p>{#if checks.nightly.available}<p>{checks.nightly.message}</p><button disabled={busy} onclick={() => void updateYoutube("nightly")}>Install nightly</button>{/if}{/if}
            </details>
            {#if youtube.managedVersion}<button class="quiet" disabled={busy} onclick={() => void rollbackYoutube()}>Roll back managed update</button>{/if}
          </div>
        </details>
      {:else}<p role="status">YouTube support status is unavailable.</p>{/if}
    </section>{/if}
  {/if}
  {#if error && setup}<p role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</section>

<style>
  .release-setup{min-width:0}.intro,.tool-section>p,.setup-step>p{color:var(--muted);line-height:1.5}
  .setup-step,.tool-section{min-width:0;margin:14px 0;padding:14px;border:1px solid var(--line);border-radius:10px;background:color-mix(in srgb,var(--surface) 80%,transparent)}
  .troubleshooting > h2{font-size:16px;margin:0 0 8px}.troubleshooting > .tool-section{margin:0;padding:0;border:0;background:transparent}.troubleshooting .tool-section > p{font-size:.82rem;margin:10px 0;line-height:1.45}.troubleshooting .youtube-details{margin:12px 0 0;padding:0;border:0;border-radius:0}.troubleshooting .youtube-details > summary{font-size:12px}
  .section-heading{display:flex;align-items:center;gap:8px}.section-heading h3{margin:0}
  h3{margin:0 0 8px;font-size:15px}.directory-row,.browser-row{display:flex;align-items:center;justify-content:space-between;gap:10px;min-width:0}
  .directory-row{margin:12px 0}.directory-row span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted)}
  .browser-list{display:grid;gap:8px}.browser-row{padding:8px 10px;border-radius:7px;background:var(--raised)}.browser-row>div{display:grid;gap:3px}.browser-row small,.paths small,.muted{color:var(--muted)}
  label{display:grid;gap:6px;margin:12px 0}.check{display:flex;flex-flow:row nowrap;align-items:flex-start;gap:8px;min-width:0}.check input[type="checkbox"]{appearance:auto;flex:0 0 16px;width:16px;height:16px;min-width:16px;margin:2px 0 0;padding:0;border-radius:3px}.sr-only{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}.tool-version{font-size:13px}
  details{margin:12px 0;padding:10px;border:1px solid var(--line);border-radius:8px}summary{cursor:pointer;font-weight:600}code{overflow-wrap:anywhere}.command{display:block;padding:9px;background:var(--raised);border-radius:6px}
  .paths{padding-left:20px}.paths li{display:grid;gap:3px;margin:10px 0}.paths code{color:var(--muted);font-size:11px}.update-actions{display:grid;justify-items:start;gap:8px;margin-top:12px}
  .quiet{background:transparent}.primary{font-weight:600}.release-setup button{max-width:100%}
  @media(max-width:520px){.setup-step,.tool-section{padding:11px}.directory-row,.browser-row{align-items:flex-start}.directory-row{flex-direction:column}}
</style>
