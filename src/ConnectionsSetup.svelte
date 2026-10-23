<script lang="ts">
  import { onMount } from "svelte";
  import { call, desktop } from "./lib/api";
  interface LyricsTranslationConfig { enabled: boolean; baseUrl: string; model: string; targetLanguage: string }
  interface Config { slskd: { enabled: boolean; baseUrl: string; downloadsPath: string }; discogsEnabled: boolean; lastfmEnabled: boolean; acoustidEnabled?: boolean; lyricsTranslation?: LyricsTranslationConfig }
  interface Credential { present: boolean; sessionOnly: boolean; error?: string | null }
  interface Status { settings: Config; slskdCredential: Credential; discogsCredential: Credential; lastfmCredential: Credential; acoustidCredential?: Credential; lyricsTranslationCredential?: Credential }
  interface SlskdShare { id: string; alias: string; localPath?: string | null; remotePath?: string | null; files: number; directories: number; excluded: boolean }
  interface SlskdSharesStatus { managed: boolean; canEdit: boolean; shares: SlskdShare[]; guidance: string; warning?:string|null }
  interface ManagedDownloadSettings { slots: number; speedLimitKib: number | null; retryAttempts: number; retryDelayMs: number; retryMaxDelayMs: number; partialPolicy: "resume" | "overwrite" }
  interface ManagedDownloadState { managed: boolean; canEdit: boolean; settings: ManagedDownloadSettings | null; guidance: string }
  let { onChanged = (_status: Status) => {} }: { onChanged?: (status: Status) => void } = $props();
  let config = $state<Config>({ slskd: { enabled: false, baseUrl: "http://localhost:5030", downloadsPath: "" }, discogsEnabled: false, lastfmEnabled: false, lyricsTranslation: { enabled: false, baseUrl: "https://api.openai.com/v1", model: "", targetLanguage: "en" } });
  let status = $state<Status | null>(null), slskdKey = $state(""), discogsToken = $state(""), lastfmKey = $state(""), acoustidKey = $state(""), lyricsTranslationKey = $state(""), sessionOnly = $state(false);
  let clear = $state<Record<string, boolean>>({}), busy = $state(false), message = $state("");
  let checks = $state<{ provider: string; available: boolean; message: string }[]>([]);
  let slskdSharesStatus = $state<SlskdSharesStatus | null>(null), shareDirectoryPath = $state(""), sharesBusy = $state(false), sharesLoading = $state(false), sharesMessage = $state("");
  let downloadState = $state<ManagedDownloadState | null>(null), downloadDraft = $state<ManagedDownloadSettings>({ slots: 32, speedLimitKib: null, retryAttempts: 3, retryDelayMs: 5000, retryMaxDelayMs: 60000, partialPolicy: "resume" }), downloadBusy = $state(false), downloadLoading = $state(false), downloadMessage = $state("");
  function apply(value: Status) {
    status = value; config = structuredClone(value.settings);
    config.lyricsTranslation ??= { enabled: false, baseUrl: "https://api.openai.com/v1", model: "", targetLanguage: "en" };
    config.slskd.baseUrl ||= "http://localhost:5030";
    onChanged(value);
    void loadShares(value);
    void loadDownloadSettings(value);
  }
  function applyDownloadState(value: ManagedDownloadState) {
    // Clone the plain IPC response before assigning it to reactive state.
    const draft = value.settings ? structuredClone(value.settings) : null;
    downloadState = value;
    if (draft) downloadDraft = draft;
  }
  async function loadDownloadSettings(current = status) {
    if (!current?.settings.slskd.enabled) { downloadState = null; return; }
    downloadLoading = true; downloadMessage = ""; downloadState = null;
    try {
      applyDownloadState(await call<ManagedDownloadState>("get_managed_slskd_download_settings"));
    } catch (error) { downloadMessage = `Could not read download settings: ${String(error)}`; }
    finally { downloadLoading = false; }
  }
  async function saveDownloadSettings() {
    if (!downloadState?.canEdit || downloadBusy || downloadLoading) return;
    downloadBusy = true; downloadMessage = "";
    try {
      const updated = await call<ManagedDownloadState>("save_managed_slskd_download_settings", { settings: downloadDraft });
      applyDownloadState(updated);
      downloadMessage = updated.guidance;
    } catch (error) { downloadMessage = String(error); }
    finally { downloadBusy = false; }
  }
  onMount(() => { if (desktop()) void call<Status>("get_connections").then(apply).catch((error) => message = String(error)); });
  async function loadShares(current = status) {
    if (!current?.settings.slskd.enabled || !current.slskdCredential.present) {
      slskdSharesStatus = null;
      sharesMessage = current?.settings.slskd.enabled ? "Save a slskd API key to inspect its configured shares." : "Enable and connect slskd to inspect its configured shares.";
      return;
    }
    sharesLoading = true; sharesMessage = "";
    try { slskdSharesStatus = await call<SlskdSharesStatus>("get_slskd_shares"); }
    catch (error) { sharesMessage = `Could not read slskd shares: ${String(error)}`; }
    finally { sharesLoading = false; }
  }
  async function rescanShares() {
    sharesBusy = true; sharesMessage = "";
    try {
      slskdSharesStatus = await call<SlskdSharesStatus>("rescan_slskd_shares");
      sharesMessage = "slskd share scan requested.";
    } catch (error) { sharesMessage = `Could not start a share scan: ${String(error)}`; }
    finally { sharesBusy = false; }
  }
  async function addManagedShare() {
    const directoryPath = shareDirectoryPath.trim();
    if (!directoryPath) return;
    sharesBusy = true; sharesMessage = "";
    try {
      slskdSharesStatus = await call<SlskdSharesStatus>("add_managed_slskd_share", { directoryPath });
      shareDirectoryPath = "";
      sharesMessage = "Folder added to the managed slskd shares.";
    } catch (error) { sharesMessage = `Could not add folder: ${String(error)}`; }
    finally { sharesBusy = false; }
  }
  async function removeManagedShare(shareId: string) {
    sharesBusy = true; sharesMessage = "";
    try {
      slskdSharesStatus = await call<SlskdSharesStatus>("remove_managed_slskd_share", { shareId });
      sharesMessage = "Folder removed from the managed slskd shares.";
    } catch (error) { sharesMessage = `Could not remove folder: ${String(error)}`; }
    finally { sharesBusy = false; }
  }
  async function save() {
    busy = true; message = "";
    try {
      apply(await call<Status>("save_connections", { update: {
        settings: config, sessionOnly,
        ...(slskdKey || clear.slskd ? { slskdApiKey: clear.slskd ? "" : slskdKey } : {}),
        ...(discogsToken || clear.discogs ? { discogsToken: clear.discogs ? "" : discogsToken } : {}),
        ...(lastfmKey || clear.lastfm ? { lastfmApiKey: clear.lastfm ? "" : lastfmKey } : {}),
        ...(acoustidKey || clear.acoustid ? { acoustidApiKey: clear.acoustid ? "" : acoustidKey } : {}),
        ...(lyricsTranslationKey || clear.lyricsTranslation ? { lyricsTranslationApiKey: clear.lyricsTranslation ? "" : lyricsTranslationKey } : {}),
      } }));
      slskdKey = ""; discogsToken = ""; lastfmKey = ""; acoustidKey = ""; lyricsTranslationKey = ""; clear = {};
      message = "Connections saved.";
    } catch (error) { message = String(error); }
    finally { busy = false; }
  }
  async function importOneTagger() {
    busy = true; message = "";
    try { apply(await call<Status>("import_onetagger_discogs", { sessionOnly })); message = "Discogs token imported from OneTagger."; }
    catch (error) { message = String(error); }
    finally { busy = false; }
  }
  async function test() {
    busy = true;
    try { checks = await call("test_connections"); }
    catch (error) { message = String(error); }
    finally { busy = false; }
  }
  async function guide(url: string) { try { await call("open_external_url", { url }); } catch (error) { message = String(error); } }
  const credentialLabel = (value?: Credential) => value?.error ? "Secure store unavailable — use session-only" : value?.present ? value.sessionOnly ? "Connected for this session" : "Credential saved securely" : "Not configured";
</script>

<section class="settings-card connection-card" aria-labelledby="connections-title">
  <h2 id="connections-title">Sources & artist discovery</h2>
  <p>Connect once, then find music from Search or any track’s Download button.</p>
  <details>
    <summary>Soulseek / slskd · {credentialLabel(status?.slskdCredential)}</summary>
    <p>Run slskd on this computer as a separate background service. Burrata uses its API and copies completed files from its download folder; you do not need a public server.</p>
    <label class="check"><input type="checkbox" bind:checked={config.slskd.enabled} /> Enable Soulseek search and downloads</label>
    <label>slskd address<input aria-label="slskd address" bind:value={config.slskd.baseUrl} placeholder="http://localhost:5030" /></label>
    <label>Completed-download folder<input aria-label="slskd completed-download folder" bind:value={config.slskd.downloadsPath} placeholder="Absolute path to slskd’s downloads folder" /></label>
    <label>API key<input aria-label="slskd API key" type="password" autocomplete="off" bind:value={slskdKey} placeholder={status?.slskdCredential.present ? "Leave blank to keep saved key" : "Key from slskd configuration"} /></label>
    <label class="check"><input type="checkbox" bind:checked={clear.slskd} /> Remove saved slskd key</label>
    <section class="slskd-shares" aria-labelledby="slskd-shares-title">
      <h3 id="slskd-shares-title">Folders shared by slskd</h3>
      <p class="sharing-warning">Files in shared folders are visible to other Soulseek users. Share only music you intend to make public; never your home or personal-document folders.</p>
      <p class="share-setup-note">Burrata reads slskd’s configured shares. Use the folder controls below for the managed local service; for a separately configured service, edit its slskd configuration and rescan here.</p>
      {#if sharesLoading}<p role="status">Reading configured shares…</p>
      {:else if slskdSharesStatus?.shares.length}<ul aria-label="Configured shared folders">{#each slskdSharesStatus.shares as share (share.id)}<li><strong>{share.alias || share.localPath || share.remotePath || "Shared folder"}</strong>{#if share.localPath || share.remotePath}<small>{share.localPath ?? share.remotePath}</small>{/if}<small>{share.files} files · {share.directories} folders{#if share.excluded} · exclusions applied{/if}</small>{#if slskdSharesStatus.managed && slskdSharesStatus.canEdit && /^[a-f0-9]{16}$/i.test(share.id)}<button class="remove-share" disabled={sharesBusy} onclick={() => void removeManagedShare(share.id)}>Remove this folder</button>{/if}</li>{/each}</ul>
      {:else}<p class="empty-shares">No shared folders reported by slskd.</p>{/if}
      {#if slskdSharesStatus?.managed && slskdSharesStatus.canEdit}
        <p class="share-setup-note">Choose one music folder at a time. Burrata mounts managed shares read-only for slskd; it does not alter your files.</p>
        <label>Folder path on this computer<input aria-label="Folder path on this computer" type="text" autocomplete="off" bind:value={shareDirectoryPath} placeholder="Absolute path to a music folder" /></label>
        <p class="sharing-warning">Files in a folder you add become visible to other Soulseek users. Share only music you intend to make public; never your home or personal-document folders.</p>
        <button disabled={sharesBusy || sharesLoading || !shareDirectoryPath.trim()} onclick={() => void addManagedShare()}>{sharesBusy ? "Adding folder…" : "Add folder to shares"}</button>
      {:else if slskdSharesStatus?.managed && !slskdSharesStatus.canEdit}
        <p class="share-setup-note">Folder changes are unavailable for this managed service. {slskdSharesStatus.guidance}</p>
      {:else}
        <p class="share-setup-note">This slskd service is configured separately. Burrata can list and rescan its shares, but folder changes must be made in slskd configuration.</p>
      {/if}
      <button disabled={sharesBusy || sharesLoading || !status?.settings.slskd.enabled || !status?.slskdCredential.present} onclick={() => void rescanShares()}>{sharesBusy ? "Starting scan…" : "Rescan shares"}</button>
      {#if slskdSharesStatus?.guidance}<p class="share-setup-note">{slskdSharesStatus.guidance}</p>{/if}
      {#if slskdSharesStatus?.warning}<p role="status">Could not read the running daemon’s shares: {slskdSharesStatus.warning}</p>{/if}
      {#if sharesMessage}<p role="status">{sharesMessage}</p>{/if}
    </section>
    <section class="slskd-download-settings" aria-labelledby="slskd-download-settings-title">
      <h3 id="slskd-download-settings-title">Download behavior</h3>
      {#if downloadLoading}<p role="status">Reading download settings…</p>
      {:else if downloadState?.canEdit}
        <p>These settings apply to all downloads managed by this local slskd service. Speed is in KiB/s; leave it blank for no explicit limit.</p>
        <label>Concurrent download slots<input aria-label="Concurrent download slots" type="number" min="1" max="500" step="1" bind:value={downloadDraft.slots} /></label>
        <label>Download speed limit (KiB/s)<input aria-label="Download speed limit (KiB/s)" type="number" min="1" step="1" bind:value={downloadDraft.speedLimitKib} placeholder="No explicit limit" /></label>
        <label>Automatic retry attempts<input aria-label="Automatic retry attempts" type="number" min="1" max="100" step="1" bind:value={downloadDraft.retryAttempts} /></label>
        <label>Initial retry delay (ms)<input aria-label="Initial retry delay (ms)" type="number" min="0" max="3600000" step="100" bind:value={downloadDraft.retryDelayMs} /></label>
        <label>Maximum retry delay (ms)<input aria-label="Maximum retry delay (ms)" type="number" min="0" max="3600000" step="100" bind:value={downloadDraft.retryMaxDelayMs} /></label>
        <label>Partial download handling<select aria-label="Partial download handling" bind:value={downloadDraft.partialPolicy}><option value="resume">Resume partial files</option><option value="overwrite">Always overwrite partial files</option></select></label>
        <p class="share-setup-note">{downloadState.guidance}</p>
        <button disabled={downloadBusy} onclick={() => void saveDownloadSettings()}>{downloadBusy ? "Applying…" : "Apply download settings"}</button>
      {:else if downloadState}
        <p class="share-setup-note">{downloadState.guidance}</p>
        {#if downloadState.settings}<dl>{#each Object.entries(downloadState.settings) as [key, value]}<div><dt>{key}</dt><dd>{value ?? "No explicit limit"}</dd></div>{/each}</dl>{/if}
      {:else}
        <p class="share-setup-note">{downloadMessage || "Enable and connect slskd to inspect its download settings."}</p>
      {/if}
      {#if downloadMessage && downloadState?.canEdit}<p role="status">{downloadMessage}</p>{/if}
    </section>
    <button onclick={() => void guide("https://github.com/slskd/slskd#quick-start")}>slskd setup guide ↗</button>
  </details>
  <details>
    <summary>Discogs · {credentialLabel(status?.discogsCredential)}</summary>
    <p>Releases, versions, labels and credits. A personal token enables catalog search.</p>
    <button disabled={busy} onclick={() => void importOneTagger()}>Import token from OneTagger</button>
    <label class="check"><input type="checkbox" bind:checked={config.discogsEnabled} /> Use Discogs metadata</label>
    <label>Personal token<input aria-label="Discogs token" type="password" autocomplete="off" bind:value={discogsToken} placeholder={status?.discogsCredential.present ? "Leave blank to keep saved token" : "Discogs personal access token"} /></label>
    <label class="check"><input type="checkbox" bind:checked={clear.discogs} /> Remove saved Discogs token</label>
    <button onclick={() => void guide("https://www.discogs.com/settings/developers")}>Get a Discogs token ↗</button>
  </details>
  <details>
    <summary>Last.fm · {credentialLabel(status?.lastfmCredential)}</summary>
    <p>Popularity-ranked tracks on artist pages. MusicBrainz identities and credits work without a key.</p>
    <label class="check"><input type="checkbox" bind:checked={config.lastfmEnabled} /> Use Last.fm top tracks</label>
    <label>API key<input aria-label="Last.fm API key" type="password" autocomplete="off" bind:value={lastfmKey} placeholder={status?.lastfmCredential.present ? "Leave blank to keep saved key" : "Last.fm API key"} /></label>
    <label class="check"><input type="checkbox" bind:checked={clear.lastfm} /> Remove saved Last.fm key</label>
    <button onclick={() => void guide("https://www.last.fm/api/account/create")}>Get a Last.fm API key ↗</button>
  </details>
  <details id="acoustid-connection"><summary>AcoustID · {credentialLabel(status?.acoustidCredential)}</summary>
    <p>Optional audio-fingerprint identification for newly downloaded tracks. Requires a registered application key; fingerprints are generated locally and looked up online.</p>
    <label class="check"><input type="checkbox" bind:checked={config.acoustidEnabled} /> Use AcoustID recognition</label>
    <label>Application key<input aria-label="AcoustID application key" type="password" autocomplete="off" bind:value={acoustidKey} placeholder={status?.acoustidCredential?.present ? "Leave blank to keep saved key" : "AcoustID application key"} /></label>
    <label class="check"><input type="checkbox" bind:checked={clear.acoustid} /> Remove saved AcoustID key</label>
    <button onclick={()=>void guide("https://acoustid.org/webservice")}>AcoustID setup ↗</button>
  </details>
  <details id="lyrics-translation-connection"><summary>Lyrics translation · {credentialLabel(status?.lyricsTranslationCredential)}</summary>
    <p>Optional OpenAI-compatible translation for synchronized lyrics. The original stays visible, and translation runs only after you click Translate in Now Playing. Timed and plain lyrics are supported; source timestamps are kept unchanged. Your provider may charge for use.</p>
    <label class="check"><input type="checkbox" bind:checked={config.lyricsTranslation!.enabled} /> Enable lyrics translation</label>
    <label>API endpoint<input aria-label="Lyrics translation endpoint" type="url" autocomplete="url" bind:value={config.lyricsTranslation!.baseUrl} placeholder="https://api.openai.com/v1" /></label>
    <label>Model<input aria-label="Lyrics translation model" type="text" autocomplete="off" bind:value={config.lyricsTranslation!.model} placeholder="Model name from your provider" /></label>
    <label>Target language<input aria-label="Lyrics translation target language" type="text" autocomplete="language" bind:value={config.lyricsTranslation!.targetLanguage} placeholder="e.g. Spanish or es" /></label>
    <label>API key<input aria-label="Lyrics translation API key" type="password" autocomplete="new-password" bind:value={lyricsTranslationKey} placeholder={status?.lyricsTranslationCredential?.present ? "Leave blank to keep saved key" : "Provider API key"} /></label>
    <label class="check"><input type="checkbox" bind:checked={clear.lyricsTranslation} /> Remove saved translation key</label>
    <small>Endpoint, model, and language are saved as connection settings. The API key is kept in the secure credential store or in memory for this session; it is never returned to the UI.</small>
  </details>
  <label class="check"><input type="checkbox" bind:checked={sessionOnly} /> Keep new credentials only for this session</label>
  <small>Credentials use your operating system’s secure store. Choose session-only if it is unavailable; keys are never saved in the library database.</small>
  <div class="detail-actions"><button class="primary" disabled={busy} onclick={() => void save()}>Save connections</button><button disabled={busy} onclick={() => void test()}>Test saved connections</button></div>
  {#if message}<p role="status">{message}</p>{/if}
  {#each checks as check}<p>{check.available ? "✓" : "○"} {check.provider}: {check.message}</p>{/each}
</section>

<style>
  details { margin: 12px 0; padding: 12px; border: 1px solid var(--line); border-radius: 8px; }
  summary { cursor: pointer; font-weight: 600; }
  p, small { color: var(--muted); line-height: 1.5; }
  label { display: grid; gap: 6px; margin: 12px 0; }
  label.check { display: flex; align-items: center; }
  input:not([type="checkbox"]) { width: 100%; min-width: 0; }
  small { display: block; margin: 12px 0; }
  .slskd-shares{margin:16px 0 8px;padding:12px;border:1px solid var(--line);border-radius:8px;background:color-mix(in srgb,var(--surface) 70%,transparent)}
  .slskd-shares h3{margin:0;font-size:14px}
  .sharing-warning{padding:9px 11px;border-left:3px solid var(--warning,#d39c45);background:color-mix(in srgb,var(--warning,#d39c45) 10%,transparent);color:var(--text)!important;font-weight:600}
  .share-setup-note,.empty-shares{font-size:12px}
  .slskd-shares ul{display:grid;gap:7px;margin:10px 0;padding:0;list-style:none}
  .slskd-shares li{display:grid;gap:3px;min-width:0;padding:8px 10px;border-radius:6px;background:var(--raised)}
  .slskd-shares li strong,.slskd-shares li small{overflow-wrap:anywhere;margin:0}
  .slskd-shares li strong{font-size:12px}.slskd-shares li small{font-size:10px}
  .remove-share{justify-self:start;margin-top:5px;font-size:11px}
  .slskd-download-settings{margin:16px 0 8px;padding:12px;border:1px solid var(--line);border-radius:8px;background:color-mix(in srgb,var(--surface) 70%,transparent)}
  .slskd-download-settings h3{margin:0;font-size:14px}
  .slskd-download-settings dl{display:grid;grid-template-columns:max-content 1fr;gap:6px 12px;font-size:12px}.slskd-download-settings dl div{display:contents}.slskd-download-settings dt{font-weight:600}.slskd-download-settings dd{margin:0;overflow-wrap:anywhere}
</style>
