<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import SpotifyDurationRepair from "./SpotifyDurationRepair.svelte";

  type Provider = "spotify" | "youtube";
  type Account = { provider: Provider; connected: boolean; writeSyncEnabled?: boolean; accountId?: string | null; displayName?: string | null; checkedAt?: string | null; error?: string | null };
  type ManagedSession = { enabled: boolean; running: boolean; mode: string; transport: string; credentialStorage: string; spotifyAccountId?: string | null; youtubeAccountId?: string | null; youtubeLoginPage?: string | null; youtubeReconnectRequired?: boolean; pendingYoutubeAccountId?: string | null; pendingYoutubeDisplayName?: string | null; accountRevisions: Record<string, number>; connectionSeen: boolean; message: string };
  let { call, onBrowse, onChanged, onWriteSyncChange }: {
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onBrowse: (provider: Provider) => void;
    onChanged?: () => void;
    onWriteSyncChange?: (provider: Provider, enabled: boolean) => void;
  } = $props();

  let accounts = $state<Record<Provider, Account | null>>({ spotify: null, youtube: null });
  let busy = $state<Provider | null>(null);
  let message = $state<Record<Provider, string>>({ spotify: "", youtube: "" });
  let managed = $state<ManagedSession | null>(null);
  let managedBusy = $state(false);
  let managedMessage = $state("");
  let handoffCode = $state("");
  let handoffExpiresAt = $state("");
  let handoffTimer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    for (const provider of ["spotify", "youtube"] as const) void refresh(provider);
    void refreshManaged();
  });
  onDestroy(() => { if (handoffTimer) clearTimeout(handoffTimer); });

  async function refreshManaged() {
    try { setManaged(await call<ManagedSession>("get_managed_provider_session_status")); }
    catch (error) { managedMessage = `Could not check managed session: ${String(error)}`; }
  }
  function setManaged(session: ManagedSession) {
    managed = session;
    // Account status and managed-session status load concurrently. Reformat a pending
    // sign-in message when the owned-window mode arrives after the account result.
    for (const provider of ["spotify", "youtube"] as const) {
      const account = accounts[provider];
      if (!account?.connected && account?.error) message[provider] = connectionMessage(provider, account.error);
    }
  }
  async function managedAction(command: string, provider?: Provider) {
    if (managedBusy) return;
    managedBusy = true; managedMessage = "";
    try {
      const session = await call<ManagedSession>(command, provider ? { provider } : {});
      setManaged(session);
      if (command === "start_managed_provider_background" && session.transport === "native-webview-http") {
        await Promise.all([refresh("spotify"), refresh("youtube")]);
      }
      if (command === "confirm_managed_provider_account" && provider && !session.pendingYoutubeAccountId
        && (session.transport !== "native-webview-http" || session.mode === "native-ready")) await refresh(provider);
      else if (command === "confirm_managed_provider_account" && provider === "youtube" && session.mode === "native-login:youtube") {
        managedMessage = "Returning to YouTube Music. Wait for the page to finish loading, then choose continue again to verify your channel.";
      }
    }
    catch (error) { managedMessage = String(error); }
    finally { managedBusy = false; }
  }
  async function createYoutubeHandoffCode() {
    managedBusy = true; managedMessage = ""; handoffCode = ""; handoffExpiresAt = "";
    if (handoffTimer) clearTimeout(handoffTimer);
    try {
      const result = await call<{code: string; expiresAt: string}>("create_youtube_handoff_pairing_code");
      handoffCode = result.code; handoffExpiresAt = result.expiresAt;
      handoffTimer = setTimeout(() => { handoffCode = ""; handoffExpiresAt = ""; }, Math.max(0, Date.parse(result.expiresAt) - Date.now()));
    } catch (error) { managedMessage = String(error); }
    finally { managedBusy = false; }
  }

  async function refresh(provider: Provider) {
    try { setAccount(await call<Account>("remote_playlist_connection_status", { provider })); }
    catch (error) { message[provider] = `Could not check ${providerName(provider)}: ${String(error)}`; }
  }

  async function connect(provider: Provider) {
    busy = provider; message[provider] = "";
    try {
      const account = await call<Account>("connect_remote_playlist_provider", { provider });
      setAccount(account);
      if (!account.connected) message[provider] = connectionMessage(provider, account.error || "");
    } catch (error) {
      const detail = String(error);
      message[provider] = /sign.?in|required|unauthenticated/i.test(detail)
        ? connectionMessage(provider, detail)
        : `Could not connect: ${detail}`;
    } finally { busy = null; }
  }

  async function disconnect(provider: Provider) {
    busy = provider; message[provider] = "";
    try { setAccount(await call<Account>("disconnect_remote_playlist_provider", { provider })); }
    catch (error) { message[provider] = `Could not disconnect: ${String(error)}`; }
    finally { busy = null; }
  }

  async function setWriteSync(provider: Provider, enabled: boolean, input: HTMLInputElement) {
    busy = provider;
    try {
      const saved = await call<boolean>("set_remote_playlist_write_sync_enabled", { provider, enabled });
      if (accounts[provider]) accounts[provider] = {...accounts[provider]!, writeSyncEnabled: saved};
      onWriteSyncChange?.(provider, saved);
    } catch (error) { message[provider] = `Could not update sync setting: ${String(error)}`; }
    finally { input.checked = accounts[provider]?.writeSyncEnabled === true; busy = null; }
  }
  function setAccount(account: Account) {
    accounts[account.provider] = account;
    onWriteSyncChange?.(account.provider, account.writeSyncEnabled === true);
    onChanged?.();
    message[account.provider] = account.error
      ? connectionMessage(account.provider, account.error)
      : "";
  }
  function connectionMessage(provider: Provider, detail: string) {
    if (!/sign.?in|required|unauthenticated/i.test(detail)) return detail;
    if (nativeSession && managed?.mode === `native-login:${provider}`) {
      return `Finish signing in through Burrata’s ${providerName(provider)} sign-in window, then choose “I’ve signed in — continue.”`;
    }
    if (nativeSession && provider === "youtube" && managed?.youtubeReconnectRequired) {
      return "Choose Reconnect YouTube to open Burrata’s sign-in window, then choose “I’ve signed in — continue.”";
    }
    if (nativeSession) {
      return `Choose Connect ${providerName(provider)} to open Burrata’s sign-in window, then choose “I’ve signed in — continue.”`;
    }
    return `Choose Connect ${providerName(provider)} to open your browser and sign in.`;
  }
  const providerName = (provider: Provider) => provider === "spotify" ? "Spotify" : "YouTube";
  const nativeSession = $derived(managed?.enabled === true && managed.transport === "native-webview-http");
  const nativeAccessPaused = $derived(nativeSession && Boolean(managed?.spotifyAccountId || managed?.youtubeAccountId) && (managed?.mode === "stopped" || managed?.mode === "native-confirmed"));
  const activeNativeProvider = $derived<Provider | null>(managed?.mode === "native-login:spotify" ? "spotify" : managed?.mode === "native-login:youtube" || (managed?.mode === "native-awaiting-youtube-confirmation" && managed.pendingYoutubeAccountId) ? "youtube" : null);
  async function connectAccount(provider: Provider) {
    if (nativeSession) await managedAction("start_managed_provider_login", provider);
    else await connect(provider);
  }
</script>

<section class="remote-account-panel" aria-labelledby="remote-account-title">
  <header>
    <h2 id="remote-account-title">Streaming playlist accounts</h2>
    <p>{nativeSession ? "Sign in through Burrata. Saved connections are reused after restarting, and provider requests run only when needed." : "Connect in your browser to browse playlists."} Burrata never asks for or stores your streaming password.</p>
  </header>
  {#if nativeAccessPaused}<div class="native-access-paused" role="status">
    <p>Provider access is paused. Your saved accounts are kept.</p>
    <button class="primary" disabled={!managed?.enabled || managedBusy} onclick={() => void managedAction("start_managed_provider_background")}>Resume provider access</button>
  </div>{/if}
  <div class="remote-account-grid">
    {#each ["spotify", "youtube"] as provider (provider)}
      {@const account = accounts[provider as Provider]}
      {@const pausedAccount = nativeAccessPaused && Boolean(account?.connected || (provider === "spotify" ? managed?.spotifyAccountId : managed?.youtubeAccountId))}
      <article class="remote-account-card" aria-label={`${providerName(provider as Provider)} account`}>
        <div class="remote-account-heading">
          <h3>{providerName(provider as Provider)}</h3>
          <span class:connected={account?.connected && !pausedAccount && !(nativeSession && provider === "youtube" && managed?.youtubeReconnectRequired)} class="remote-account-state" role="status">
            {pausedAccount ? "Access paused" : nativeSession && provider === "youtube" && managed?.youtubeReconnectRequired ? "Reconnect needed" : account ? account.connected ? "Connected" : "Not connected" : "Checking…"}
          </span>
        </div>
        {#if nativeSession && provider === "youtube" && managed?.youtubeReconnectRequired}
          <p class="remote-account-hint">The saved YouTube account is retained. Sign in again and confirm your channel to restore provider access.</p>
        {:else if account?.connected}
          <p class="remote-account-identity">{account.displayName || account.accountId || "Browser account"}</p>
        {:else if pausedAccount}
          <p class="remote-account-hint">Resume provider access to browse this saved account.</p>
        {:else if nativeSession && activeNativeProvider && activeNativeProvider !== provider}
          <p class="remote-account-hint">Finish the {providerName(activeNativeProvider)} connection first.</p>
        {:else if !nativeSession || managed?.mode !== `native-login:${provider}`}
          <p class="remote-account-hint">Sign in to {providerName(provider as Provider)} in your browser when prompted.</p>
        {:else}
          {#if provider === "youtube" && managed?.youtubeLoginPage === "google-accounts"}
            <p class="remote-account-hint">After Google sign-in finishes, choose continue to return to YouTube Music. Wait for Music to load, then choose continue again to verify your channel.</p>
          {:else}
            <p class="remote-account-hint">When you have finished signing in, continue to check the account.</p>
          {/if}
        {/if}
        {#if nativeSession && provider === "youtube" && managed?.pendingYoutubeAccountId}
          <p class="managed-confirmation" role="status">Verified <strong>{managed.pendingYoutubeDisplayName || managed.pendingYoutubeAccountId}</strong> ({managed.pendingYoutubeAccountId}). Confirm to use this account.</p>
        {/if}
        <div class="remote-account-actions">
          {#if nativeSession && managed?.mode === `native-login:${provider}`}
            <button class="primary" disabled={managedBusy} onclick={() => void managedAction("confirm_managed_provider_account", provider as Provider)}>I’ve signed in — continue</button>
          {:else if nativeSession && provider === "youtube" && managed?.pendingYoutubeAccountId}
            <button class="primary" disabled={managedBusy} onclick={() => void managedAction("confirm_managed_provider_account", "youtube")}>Use this account</button>
          {:else if nativeSession && provider === "youtube" && managed?.youtubeReconnectRequired}
            <button class="primary" disabled={busy !== null || managedBusy || (activeNativeProvider !== null && activeNativeProvider !== provider)} onclick={() => void connectAccount("youtube")}>Reconnect YouTube</button>
          {:else if account?.connected || pausedAccount}
            <button disabled={busy !== null || pausedAccount} onclick={() => onBrowse(provider as Provider)}>Browse playlists</button>
            <button class="quiet" disabled={busy !== null} onclick={() => void disconnect(provider as Provider)}>Forget connection</button>
          {:else}
            <button class="primary" disabled={busy !== null || managedBusy || (nativeSession && activeNativeProvider !== null && activeNativeProvider !== provider)} onclick={() => void connectAccount(provider as Provider)}>
              {busy === provider || managedBusy ? "Opening sign-in…" : `Connect ${providerName(provider as Provider)}`}
            </button>
          {/if}
        </div>
        {#if account}<label class="write-sync-setting"><input type="checkbox" checked={account.writeSyncEnabled === true} disabled={busy !== null || (provider === "youtube" && !account.connected && account.writeSyncEnabled !== true)} onchange={(event)=>void setWriteSync(provider as Provider, event.currentTarget.checked, event.currentTarget)}/><span>Allow writing playlist changes to {providerName(provider as Provider)}</span></label><p class="remote-account-hint">{provider === "youtube" ? "Writes are experimental. They run only after opt-in and review of a complete, editable preview." : "Changes are sent only after you choose Sync and review them."}</p>{/if}
        {#if message[provider as Provider] && !pausedAccount}<p class="remote-account-message" role="status">{message[provider as Provider]}</p>{/if}
      </article>
    {/each}
  </div>
</section>

<details class="managed-provider">
  <summary>Other connection options</summary>
  <p>After you confirm an account, Burrata uses it for provider requests when needed. If you stop access, use Resume provider access to enable it again. Spotify and YouTube support is still experimental; playlist writing remains separately opt-in and review-gated.</p>
  {#if managed}
    {#if managed.transport !== "native-webview-http"}
    <div class="remote-account-actions">
      <button disabled={!managed.enabled || managedBusy} onclick={() => void managedAction("start_managed_provider_login", "spotify")}>Sign in to Spotify</button>
      <button disabled={!managed.enabled || managedBusy || !managed.running} onclick={() => void managedAction("confirm_managed_provider_account", "spotify")}>{managed.transport === "native-webview-http" ? "Confirm and enable read access" : "Confirm Spotify account"}</button>
        <button disabled={!managed.enabled || managedBusy} onclick={() => void managedAction("start_managed_provider_login", "youtube")}>Sign in to YouTube</button>
        <button disabled={!managed.enabled || managedBusy || !managed.running} onclick={() => void managedAction("confirm_managed_provider_account", "youtube")}>Confirm YouTube account</button>
        <button disabled={!managed.enabled || managedBusy || !managed.spotifyAccountId} onclick={() => void managedAction("start_managed_provider_background")}>Start background session</button>
    </div>
    {/if}
    <details><summary>Diagnostics and advanced controls</summary>
      <p class="managed-state" role="status">{managed.enabled ? managed.message : "Experimental connection options are unavailable in this build."}{#if managed.spotifyAccountId} · Spotify connected{/if}{#if managed.youtubeAccountId} · YouTube connected{/if}{#if managed.credentialStorage === "memory-only"} · Session credentials remain in memory until Burrata closes.{/if}</p>
      <button disabled={!managed.enabled || managedBusy || !managed.running} onclick={() => void managedAction("stop_managed_provider_session")}>{nativeSession ? "Pause provider access" : "Stop session"}</button>
      <button class="quiet" disabled={managedBusy} onclick={() => void refreshManaged()}>Refresh status</button>
    </details>
    <details><summary>Other YouTube connection options</summary>
      <p>Pair an existing YouTube session from the Burrata browser extension.</p>
      <button disabled={!managed.enabled || managedBusy} onclick={() => void createYoutubeHandoffCode()}>Pair an existing YouTube session</button>
    </details>
    {#if handoffCode}<p class="handoff-code-copy" role="status">In the Burrata extension popup, confirm the active YouTube channel and enter this code: <code>{handoffCode}</code>. It expires at {new Date(handoffExpiresAt).toLocaleTimeString()} and works once for this app.</p>{/if}
  {/if}
  {#if managedMessage}<p class="remote-account-message" role="status">{managedMessage}</p>{/if}
</details>

<SpotifyDurationRepair {call} />

<style>
  .remote-account-panel { container-type:inline-size; min-width:0; display:grid; gap:12px; }
  .managed-provider { border-top:1px solid var(--line); padding-top:10px; }
  .native-access-paused { display:flex; flex-wrap:wrap; align-items:center; gap:8px; margin:10px 0; padding:10px 12px; border:1px solid var(--line); border-radius:8px; }
  .native-access-paused p { flex:1; margin:0; font-size:12px; }
  .managed-provider summary { cursor:pointer; font-size:12px; font-weight:600; }
  .managed-provider > p { margin:7px 0; color:var(--muted); font-size:12px; line-height:1.45; }
  .managed-confirmation { width:100%; }
  .remote-account-panel header h2 { margin:0; font-size:16px; }
  .remote-account-panel header p,.remote-account-card p { margin:5px 0 0; color:var(--muted); font-size:12px; line-height:1.45; }
  .remote-account-grid { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:10px; }
  .remote-account-card { min-width:0; display:grid; align-content:start; gap:8px; padding:12px; border:1px solid var(--line); border-radius:9px; background:var(--surface); }
  .remote-account-heading { display:flex; align-items:center; justify-content:space-between; gap:8px; }
  .remote-account-heading h3 { min-width:0; margin:0; font-size:14px; }
  .remote-account-state { flex:none; color:var(--muted); font-size:10px; }
  .remote-account-state.connected { color:var(--success,#5bab8a); }
  .remote-account-identity { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .remote-account-actions { display:flex; flex-wrap:wrap; gap:6px; }
  .remote-account-actions button { min-height:30px; font-size:11px; }
  .remote-account-actions .primary { color:var(--text); }
  .remote-account-actions .quiet { background:transparent; }
  .write-sync-setting { display:flex; gap:8px; align-items:flex-start; font-size:12px; line-height:1.4; }
  .write-sync-setting input { width:16px; height:16px; min-width:16px; padding:0; flex:0 0 16px; margin:2px 0 0; }
  .remote-account-message { overflow-wrap:anywhere; color:var(--warning,#d39c45)!important; }
  .handoff-code-copy { overflow-wrap:anywhere; padding:8px; border:1px solid var(--line); border-radius:7px; background:var(--raised); }
  .handoff-code-copy code { user-select:all; font-weight:700; letter-spacing:.08em; }
  @container (max-width:430px) { .remote-account-grid { grid-template-columns:minmax(0,1fr); } }
</style>
