// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import BrowserConnections from "./BrowserConnections.svelte";

let destroy: (() => void) | undefined;
function disposeMounted() { const cleanup = destroy; destroy = undefined; cleanup?.(); }
afterEach(() => { disposeMounted(); document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, onBrowse = vi.fn(), onWriteSyncChange = vi.fn()) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(BrowserConnections, { target, props: { call, onBrowse, onWriteSyncChange } });
  destroy = () => unmount(instance);
  return { target, onBrowse };
}

describe("BrowserConnections", () => {
  it("keeps stopped native access paused despite cached connected cards and resumes only once on user action", async () => {
    let mode="stopped";
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="public_spotify_duration_repair_running")return false;
      if(command==="remote_playlist_connection_status")return {provider:args?.provider,connected:true,displayName:"Saved account"};
      const status={enabled:true,running:mode==="native-ready",mode,transport:"native-webview-http",credentialStorage:"os-keyring",spotifyAccountId:"saved-spotify",youtubeAccountId:"saved-youtube",accountRevisions:{spotify:2,youtube:3},connectionSeen:false,message:"state"};
      if(command==="get_managed_provider_session_status")return status;
      if(command==="start_managed_provider_background"){mode="native-ready";return {...status,mode,running:true};}
      throw new Error(`Unexpected command ${command}`);
    });
    const {target,onBrowse}=render(call);await settle();
    expect(call).not.toHaveBeenCalledWith("start_managed_provider_background",expect.anything());
    const banner=target.querySelector('.native-access-paused')!;expect(banner).not.toBeNull();
    const resume=[...target.querySelectorAll<HTMLButtonElement>('button')].filter(button=>button.textContent==="Resume provider access");expect(resume).toHaveLength(1);
    expect(target.querySelectorAll('.remote-account-state.connected')).toHaveLength(0);
    for(const card of target.querySelectorAll('.remote-account-card')){
      expect(card.querySelector('.remote-account-state')?.textContent).toContain("Access paused");
      const browse=[...card.querySelectorAll<HTMLButtonElement>('button')].find(button=>button.textContent==="Search playlists")!;expect(browse.disabled).toBe(true);browse.click();
      expect([...card.querySelectorAll<HTMLButtonElement>('button')].find(button=>button.textContent==="Forget connection")?.disabled).toBe(false);
    }
    expect(onBrowse).not.toHaveBeenCalled();
    resume[0].click();resume[0].click();await settle();expect(call.mock.calls.filter(([command])=>command==="start_managed_provider_background")).toHaveLength(1);
    expect(target.querySelector('.native-access-paused')).toBeNull();expect(target.querySelectorAll('.remote-account-state.connected')).toHaveLength(2);
    expect(target.textContent).toContain("provider requests run only when needed");
    expect(target.textContent).not.toContain("Start background session");
    expect([...target.querySelectorAll<HTMLButtonElement>('button')].some(button=>button.textContent==="Pause provider access")).toBe(true);
  });
  it("shows paused native-confirmed accounts without reconnecting and preserves Chromium controls",async()=>{
    for(const transport of ["native-webview-http","isolated-chromium"]){
      const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
        if(command==="public_spotify_duration_repair_running")return false;
        if(command==="remote_playlist_connection_status")return {provider:args?.provider,connected:false};
        if(command==="get_managed_provider_session_status")return {enabled:true,running:false,mode:"native-confirmed",transport,credentialStorage:"os-keyring",spotifyAccountId:"saved",accountRevisions:{spotify:2},connectionSeen:false,message:"state"};
        throw new Error(`Unexpected command ${command}`);
      });
      const {target}=render(call);await settle();
      expect(call).not.toHaveBeenCalledWith("start_managed_provider_background",expect.anything());
      if(transport==="native-webview-http"){
        expect(target.querySelector('[aria-label="Spotify account"] .remote-account-state')?.textContent).toContain("Access paused");
        expect([...target.querySelectorAll<HTMLButtonElement>('button')].filter(button=>button.textContent==="Resume provider access")).toHaveLength(1);
        expect(target.textContent).not.toContain("Start background session");
      }else{
        expect(target.querySelector('.native-access-paused')).toBeNull();
        expect(target.textContent).toContain("Start background session");expect(target.textContent).toContain("Confirm Spotify account");expect(target.textContent).toContain("Stop session");
      }
      expect(target.textContent).toContain("Pair an existing YouTube session");disposeMounted();
    }
  });

  it("loads account states and opens browsing only for a connected provider", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: provider === "spotify", displayName: "DJ Example" };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target, onBrowse } = render(call); await settle();
    expect(call).toHaveBeenCalledWith("remote_playlist_connection_status", { provider: "spotify" });
    expect(target.textContent).toContain("DJ Example");
    expect(target.textContent).toContain("Connected");
    expect(target.textContent).toContain("Not connected");
    expect(target.querySelectorAll("input[type=password]")).toHaveLength(0);
    const spotify = target.querySelector<HTMLElement>('[aria-label="Spotify account"]')!;
    spotify.querySelector("button")!.click();
    expect(onBrowse).toHaveBeenCalledWith("spotify");
  });

  it("requests browser sign-in and disconnects through provider commands", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: false };
      if (command === "connect_remote_playlist_provider") return { provider, connected: false, error: "SIGN_IN_REQUIRED" };
      if (command === "disconnect_remote_playlist_provider") return { provider, connected: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    youtube.querySelector("button")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("connect_remote_playlist_provider", { provider: "youtube" });
    expect(target.textContent).toContain("Choose Connect YouTube to open your browser and sign in.");
    expect(target.textContent).not.toContain("Check again");
  });

  it("clears only the selected browser account connection", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: true, displayName: "DJ Example" };
      if (command === "disconnect_remote_playlist_provider") return { provider, connected: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const spotify = target.querySelector<HTMLElement>('[aria-label="Spotify account"]')!;
    const buttons = spotify.querySelectorAll("button");
    buttons[1].click(); await settle();
    expect(call).toHaveBeenCalledWith("disconnect_remote_playlist_provider", { provider: "spotify" });
    expect(spotify.textContent).toContain("Not connected");
  });

  it("uses the backend write-sync value and keeps the saved state when an update fails", async () => {
    const onWriteSyncChange = vi.fn();
    const successfulCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: true, writeSyncEnabled: false };
      if (command === "set_remote_playlist_write_sync_enabled") return false; // Backend remains authoritative.
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(successfulCall, vi.fn(), onWriteSyncChange); await settle();
    onWriteSyncChange.mockClear();
    const authoritativeCheckbox = target.querySelector<HTMLInputElement>(".write-sync-setting input")!;
    authoritativeCheckbox.click(); await settle();
    expect(successfulCall).toHaveBeenCalledWith("set_remote_playlist_write_sync_enabled", { provider: "spotify", enabled: true });
    expect(onWriteSyncChange).toHaveBeenLastCalledWith("spotify", false);
    expect(authoritativeCheckbox.checked).toBe(false);

    disposeMounted();
    onWriteSyncChange.mockClear();
    const failingCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: true, writeSyncEnabled: true };
      if (command === "set_remote_playlist_write_sync_enabled") throw new Error("storage unavailable");
      throw new Error(`Unexpected command ${command}`);
    });
    const failed = render(failingCall, vi.fn(), onWriteSyncChange); await settle();
    onWriteSyncChange.mockClear();
    const failedCheckbox = failed.target.querySelector<HTMLInputElement>(".write-sync-setting input")!;
    failedCheckbox.click(); await settle();
    expect(failingCall).toHaveBeenCalledWith("set_remote_playlist_write_sync_enabled", { provider: "spotify", enabled: false });
    expect(onWriteSyncChange).not.toHaveBeenCalled();
    expect(failedCheckbox.checked).toBe(true);
    expect(failed.target.textContent).toContain("Could not update sync setting:");
    expect(failed.target.textContent).toContain("storage unavailable");
  });

  it("lets a disconnected YouTube account turn off saved write permission", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: false, writeSyncEnabled: true };
      if (command === "set_remote_playlist_write_sync_enabled") return false;
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const checkbox = target.querySelector<HTMLInputElement>('[aria-label="YouTube account"] .write-sync-setting input')!;
    expect(checkbox.disabled).toBe(false);
    checkbox.click(); await settle();
    expect(call).toHaveBeenCalledWith("set_remote_playlist_write_sync_enabled", { provider: "youtube", enabled: false });
    expect(checkbox.checked).toBe(false);
  });

  it("updates sign-in guidance when native session status arrives after account status", async () => {
    let resolveManaged!: (status: unknown) => void;
    const managedResult = new Promise(resolve => { resolveManaged = resolve; });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider: args?.provider, connected: false, error: "SIGN_IN_REQUIRED" };
      if (command === "get_managed_provider_session_status") return managedResult;
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    expect(target.textContent).toContain("Choose Connect YouTube to open your browser and sign in.");
    resolveManaged({
      enabled:true, running:false, mode:"native-ready", transport:"native-webview-http", credentialStorage:"os-keyring",
      accountRevisions:{}, connectionSeen:false, message:"ready",
    });
    await settle();
    expect(target.textContent).toContain("Choose Connect YouTube to open Burrata’s sign-in window");
    expect(target.textContent).toContain("I’ve signed in — continue.");
    expect(target.textContent).not.toContain("Check again");
  });

  it("keeps YouTube write opt-in independent from Spotify", async () => {
    const onWriteSyncChange = vi.fn();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: true, writeSyncEnabled: false };
      if (command === "set_remote_playlist_write_sync_enabled") return true;
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call, vi.fn(), onWriteSyncChange); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    expect(youtube.textContent).toContain("Writes are experimental");
    expect(youtube.textContent).not.toContain("applying writes remains disabled");
    const checkbox = youtube.querySelector<HTMLInputElement>(".write-sync-setting input")!;
    checkbox.click(); await settle();
    expect(call).toHaveBeenCalledWith("set_remote_playlist_write_sync_enabled", { provider: "youtube", enabled: true });
    expect(onWriteSyncChange).toHaveBeenLastCalledWith("youtube", true);
    expect(youtube.textContent).toContain("Writes are experimental");
  });

  it("can resume a stopped native YouTube-only session without Spotify", async () => {
    const status = { enabled:true, running:false, mode:"stopped", transport:"native-webview-http", credentialStorage:"os-keyring", accountRevisions:{}, connectionSeen:false, message:"stopped", youtubeAccountId:"UC1234567890123456789012" };
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="public_spotify_duration_repair_running")return false;
      if(command==="remote_playlist_connection_status")return {provider:args?.provider,connected:false};
      if(command==="get_managed_provider_session_status")return status;
      if(command==="start_managed_provider_background")return {...status,running:true,mode:"native-ready"};
      throw new Error(`Unexpected command ${command}`);
    });
    const {target}=render(call);await settle();
    expect(target.textContent).toContain("After you confirm an account, Burrata uses it for provider requests when needed.");
    const resume=[...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent==="Resume provider access")!;
    expect(resume).toBeDefined();resume.click();await settle();
    expect(call).toHaveBeenCalledWith("start_managed_provider_background", {});
    expect([...target.querySelectorAll("button")].some(button=>button.textContent==="Resume provider access")).toBe(false);
  });

  it("offers provider access activation only for stopped or legacy confirmed states", async () => {
    const status = { enabled:true, running:false, mode:"native-confirmed", transport:"native-webview-http", credentialStorage:"os-keyring", accountRevisions:{}, connectionSeen:false, message:"confirmed", spotifyAccountId:"spotify-user" };
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="public_spotify_duration_repair_running")return false;
      if(command==="remote_playlist_connection_status")return {provider:args?.provider,connected:false};
      if(command==="get_managed_provider_session_status")return status;
      if(command==="start_managed_provider_background")return {...status,running:true,mode:"native-ready"};
      throw new Error(`Unexpected command ${command}`);
    });
    const {target}=render(call);await settle();
    const enable=[...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent==="Resume provider access")!;
    expect(enable).toBeDefined();
    enable.click();await settle();
    expect(call).toHaveBeenCalledWith("start_managed_provider_background", {});

    disposeMounted();
    for (const mode of ["native-login:spotify", "native-awaiting-youtube-confirmation"]) {
      const activeStatus = { ...status, running:true, mode, youtubeAccountId:"UC1234567890123456789012", pendingYoutubeAccountId:mode === "native-awaiting-youtube-confirmation" ? "UC1234567890123456789012" : null };
      const activeCall=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
        if(command==="public_spotify_duration_repair_running")return false;
        if(command==="remote_playlist_connection_status")return {provider:args?.provider,connected:false};
        if(command==="get_managed_provider_session_status")return activeStatus;
        throw new Error(`Unexpected command ${command}`);
      });
      const active=render(activeCall);await settle();
      const accessControls=[...active.target.querySelectorAll<HTMLButtonElement>("button")].filter(button=>button.textContent==="Resume provider access"||button.textContent==="Resume provider access");
      expect(accessControls).toHaveLength(0);
      disposeMounted();
    }
  });

  it("shows only an app-generated short-lived pairing code for the explicit YouTube handoff", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: false };
      if (command === "get_managed_provider_session_status") return {
        enabled: true, running: true, mode: "native-ready", transport: "native-webview-http",
        credentialStorage: "os-keyring", accountRevisions: {}, connectionSeen: false, message: "ready",
      };
      if (command === "create_youtube_handoff_pairing_code") return {
        code: "49152-0123456789abcdef0123456789abcdef", expiresAt: new Date(Date.now() + 120_000).toISOString(),
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const pair = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Pair an existing YouTube session")!;
    pair.click(); await settle();
    expect(call).toHaveBeenCalledWith("create_youtube_handoff_pairing_code");
    expect(target.textContent).toContain("49152-0123456789abcdef0123456789abcdef");
    expect(target.textContent).toContain("works once for this app");
  });

  it("requires a second confirmation after showing the verified native YouTube channel", async () => {
    let mode = "native-login:youtube";
    let pending = false;
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: provider === "youtube" && mode === "native-ready", displayName:"My channel" };
      if (command === "get_managed_provider_session_status") return {
        enabled: true, running: true, mode, transport: "native-webview-http",
        credentialStorage: "not-confirmed", accountRevisions: {}, connectionSeen: false, message: "login",
      };
      if (command === "confirm_managed_provider_account") {
        expect(args?.provider).toBe("youtube");
        if (!pending) { pending = true; mode = "native-awaiting-youtube-confirmation"; return {
          enabled: true, running: true, mode, transport: "native-webview-http",
          credentialStorage: "not-confirmed", accountRevisions: {}, connectionSeen: false, message: "review",
          pendingYoutubeAccountId: "UC1234567890123456789012", pendingYoutubeDisplayName: "My channel",
        }; }
        mode = "native-ready";
        return { enabled: true, running: true, mode, transport: "native-webview-http", credentialStorage: "os-keyring", accountRevisions: {}, connectionSeen: false, message: "ready", youtubeAccountId: "UC1234567890123456789012" };
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    const spotify = target.querySelector<HTMLElement>('[aria-label="Spotify account"]')!;
    expect([...spotify.querySelectorAll("button")].some(button => button.textContent?.includes("continue"))).toBe(false);
    const check = [...youtube.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "I’ve signed in — continue")!;
    check.click(); await settle();
    expect(youtube.textContent).toContain("Verified My channel (UC1234567890123456789012)");
    expect([...youtube.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent === "Use this account")).toBe(true);
    expect([...spotify.querySelectorAll("button")].some(button => button.textContent?.includes("continue"))).toBe(false);
    youtube.querySelector<HTMLButtonElement>('button')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("remote_playlist_connection_status", { provider: "youtube" });
    expect(youtube.textContent).toContain("Connected");
  });

  it("uses explicit Continue to return from Google, skips playlist refresh, and waits for Music", async () => {
    const status = {
      enabled:true, running:true, mode:"native-login:youtube", transport:"native-webview-http",
      credentialStorage:"not-confirmed", accountRevisions:{}, connectionSeen:false, message:"login",
      youtubeLoginPage:"google-accounts",
    };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider:args?.provider, connected:false };
      if (command === "get_managed_provider_session_status") return status;
      if (command === "confirm_managed_provider_account") return { ...status, youtubeLoginPage:"music" };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    expect(youtube.textContent).toContain("return to YouTube Music");
    const refreshCount = call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status").length;
    youtube.querySelector<HTMLButtonElement>("button.primary")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("confirm_managed_provider_account", { provider:"youtube" });
    expect(call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status")).toHaveLength(refreshCount);
    expect(target.textContent).toContain("Returning to YouTube Music");
    expect(target.textContent).toContain("choose continue again to verify your channel");
    expect(target.textContent).not.toContain("Verified");
  });

  it("keeps the Google sign-in prompt visible and does not refresh playlists when auth cookie is missing", async () => {
    const status = {
      enabled:true, running:true, mode:"native-login:youtube", transport:"native-webview-http",
      credentialStorage:"not-confirmed", accountRevisions:{}, connectionSeen:false, message:"login",
      youtubeLoginPage:"google-accounts",
    };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider:args?.provider, connected:false };
      if (command === "get_managed_provider_session_status") return status;
      if (command === "confirm_managed_provider_account") throw new Error("Finish signing in to Google before returning to Music");
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    const refreshCount = call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status").length;
    youtube.querySelector<HTMLButtonElement>("button.primary")!.click(); await settle();
    expect(target.textContent).toContain("Finish signing in to Google before returning to Music");
    expect(target.textContent).toContain("After Google sign-in finishes");
    expect(call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status")).toHaveLength(refreshCount);
  });

  it("reconnects expired YouTube session without forgetting the account and only clears after confirmation", async () => {
    let managedStatus: Record<string, any> = {
      enabled:true, running:true, mode:"native-ready", transport:"native-webview-http",
      credentialStorage:"os-keyring", accountRevisions:{youtube:4}, connectionSeen:false,
      message:"YouTube needs to reconnect", youtubeAccountId:"UC1234567890123456789012",
      youtubeReconnectRequired:true,
    };
    let connectedChannel = "My channel";
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider:args?.provider, connected:args?.provider === "youtube", displayName:connectedChannel };
      if (command === "get_managed_provider_session_status") return managedStatus;
      if (command === "start_managed_provider_login") {
        expect(args?.provider).toBe("youtube");
        managedStatus = { ...managedStatus, mode:"native-login:youtube", youtubeReconnectRequired:true };
        return managedStatus;
      }
      if (command === "confirm_managed_provider_account" && managedStatus.mode === "native-login:youtube") {
        managedStatus = { ...managedStatus, mode:"native-awaiting-youtube-confirmation", pendingYoutubeAccountId:"UC9999999999999999999999", pendingYoutubeDisplayName:"Replacement channel" };
        return managedStatus;
      }
      if (command === "confirm_managed_provider_account") {
        managedStatus = { ...managedStatus, mode:"native-ready", youtubeAccountId:"UC9999999999999999999999", youtubeReconnectRequired:false, pendingYoutubeAccountId:null };
        connectedChannel = "Replacement channel";
        return managedStatus;
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    expect(youtube.textContent).toContain("The saved YouTube account is retained");
    expect(youtube.textContent).toContain("Reconnect needed");
    expect(youtube.textContent).not.toContain("Search playlists");
    expect([...youtube.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent === "Reconnect YouTube")).toBe(true);
    youtube.querySelector<HTMLButtonElement>("button.primary")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("start_managed_provider_login", { provider:"youtube" });
    expect(youtube.textContent).toContain("Reconnect needed");
    expect([...youtube.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent === "I’ve signed in — continue")).toBe(true);
    youtube.querySelector<HTMLButtonElement>("button.primary")!.click(); await settle();
    expect(youtube.textContent).toContain("Verified Replacement channel (UC9999999999999999999999)");
    expect(managedStatus.youtubeAccountId).toBe("UC1234567890123456789012");
    expect([...youtube.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent === "Use this account")).toBe(true);
    youtube.querySelector<HTMLButtonElement>("button.primary")!.click(); await settle();
    expect(managedStatus.youtubeReconnectRequired).toBe(false);
    expect(managedStatus.youtubeAccountId).toBe("UC9999999999999999999999");
    expect(youtube.textContent).toContain("Connected");
    expect(youtube.textContent).toContain("Replacement channel");
    expect(call).toHaveBeenCalledWith("remote_playlist_connection_status", { provider:"youtube" });
  });

  it("still refreshes playlist status after a successful managed Chromium confirmation", async () => {
    const status = {
      enabled:true, running:true, mode:"background", transport:"isolated-chromium",
      credentialStorage:"os-keyring", accountRevisions:{}, connectionSeen:true, message:"background",
    };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider:args?.provider, connected:true, displayName:"Managed channel" };
      if (command === "get_managed_provider_session_status") return status;
      if (command === "confirm_managed_provider_account") return status;
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const details = target.querySelector<HTMLDetailsElement>("details.managed-provider")!;
    details.open = true;
    await settle();
    const refreshCount = call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status").length;
    const confirm = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Confirm YouTube account")!;
    confirm.click(); await settle();
    expect(call.mock.calls.filter(([command]) => command === "remote_playlist_connection_status")).toHaveLength(refreshCount + 1);
    expect(target.querySelector('[aria-label="YouTube account"]')?.textContent).toContain("Managed channel");
  });

  it("starts one native sign-in action and keeps alternate pairing disclosed", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      const provider = args?.provider as string;
      if (command === "public_spotify_duration_repair_running") return false;
      if (command === "remote_playlist_connection_status") return { provider, connected: false };
      if (command === "get_managed_provider_session_status") return { enabled:true, running:false, mode:"idle", transport:"native-webview-http", credentialStorage:"os-keyring", accountRevisions:{}, connectionSeen:false, message:"ready" };
      if (command === "start_managed_provider_login") return { enabled:true, running:true, mode:`native-login:${provider}`, transport:"native-webview-http", credentialStorage:"os-keyring", accountRevisions:{}, connectionSeen:false, message:"login" };
      throw new Error(`Unexpected command ${command}`);
    });
    const { target } = render(call); await settle();
    const spotify = target.querySelector<HTMLElement>('[aria-label="Spotify account"]')!;
    expect(spotify.querySelectorAll("button")).toHaveLength(1);
    spotify.querySelector("button")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("start_managed_provider_login", { provider:"spotify" });
    expect(spotify.textContent).toContain("I’ve signed in — continue");
    expect(spotify.textContent).not.toContain("Check again");
    const youtube = target.querySelector<HTMLElement>('[aria-label="YouTube account"]')!;
    const youtubeConnect = youtube.querySelector<HTMLButtonElement>("button")!;
    expect(youtubeConnect.disabled).toBe(true);
    expect(youtube.textContent).toContain("Finish the Spotify connection first.");
    youtubeConnect.click(); await settle();
    expect(call).not.toHaveBeenCalledWith("start_managed_provider_login", { provider:"youtube" });
    const options = [...target.querySelectorAll("summary")].find(summary => summary.textContent === "Other connection options")!.parentElement!;
    expect([...options.querySelectorAll("summary")].map(summary => summary.textContent)).toContain("Other YouTube connection options");
    expect(target.textContent).not.toContain("Transport:");
  });
});
