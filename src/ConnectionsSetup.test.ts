// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";

const { callMock } = vi.hoisted(() => ({ callMock: vi.fn() }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ConnectionsSetup from "./ConnectionsSetup.svelte";

const status = {
  settings: { slskd: { enabled: true, baseUrl: "http://localhost:5030", downloadsPath: "/downloads" }, discogsEnabled: false, lastfmEnabled: false, acoustidEnabled: false },
  slskdCredential: { present: true, sessionOnly: false }, discogsCredential: { present: false, sessionOnly: false }, lastfmCredential: { present: false, sessionOnly: false },
};
const managedDownloadSettings = { slots: 32, speedLimitKib: null, retryAttempts: 3, retryDelayMs: 5000, retryMaxDelayMs: 60000, partialPolicy: "resume" as const };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; callMock.mockReset(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render() {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ConnectionsSetup, { target });
  destroy = () => unmount(instance);
  return target;
}

describe("ConnectionsSetup Soulseek shares", () => {
  it("stores translation configuration separately from the key and supports session-only credentials", async () => {
    const configured = {
      ...status,
      settings: { ...status.settings, lyricsTranslation: { enabled: false, baseUrl: "https://api.openai.com/v1", model: "", targetLanguage: "en" } },
      lyricsTranslationCredential: { present: false, sessionOnly: false },
    };
    let saveUpdate: any;
    callMock.mockImplementation(async (command: string, args?: any) => {
      if (command === "get_connections") return configured;
      if (command === "get_slskd_shares") return { managed: false, canEdit: false, guidance: "Separate service", shares: [] };
      if (command === "get_managed_slskd_download_settings") return { managed: false, canEdit: false, settings: null, guidance: "Separate service" };
      if (command === "save_connections") {
        saveUpdate = args.update;
        return { ...configured, settings: saveUpdate.settings, lyricsTranslationCredential: { present: true, sessionOnly: true } };
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(); await settle();
    const enabled = target.querySelector<HTMLInputElement>('#lyrics-translation-connection input[type="checkbox"]')!;
    enabled.checked = true; enabled.dispatchEvent(new Event("change", { bubbles: true }));
    const endpoint = target.querySelector<HTMLInputElement>('[aria-label="Lyrics translation endpoint"]')!;
    endpoint.value = "https://translator.example/v1"; endpoint.dispatchEvent(new Event("input", { bubbles: true }));
    const model = target.querySelector<HTMLInputElement>('[aria-label="Lyrics translation model"]')!;
    model.value = "synthetic-model"; model.dispatchEvent(new Event("input", { bubbles: true }));
    const language = target.querySelector<HTMLInputElement>('[aria-label="Lyrics translation target language"]')!;
    language.value = "Spanish"; language.dispatchEvent(new Event("input", { bubbles: true }));
    const key = target.querySelector<HTMLInputElement>('[aria-label="Lyrics translation API key"]')!;
    key.value = "synthetic-secret"; key.dispatchEvent(new Event("input", { bubbles: true }));
    [...target.querySelectorAll<HTMLLabelElement>("label.check")].find(label => label.textContent?.includes("Keep new credentials only for this session"))?.querySelector<HTMLInputElement>("input")?.click();
    target.querySelector<HTMLButtonElement>(".detail-actions button.primary")!.click(); await settle();
    expect(saveUpdate.settings.lyricsTranslation).toEqual({ enabled: true, baseUrl: "https://translator.example/v1", model: "synthetic-model", targetLanguage: "Spanish" });
    expect(saveUpdate.lyricsTranslationApiKey).toBe("synthetic-secret");
    expect(JSON.stringify(saveUpdate.settings)).not.toContain("synthetic-secret");
    expect(saveUpdate.sessionOnly).toBe(true);
  });

  it("shows managed shares and warns before explicit add/remove actions", async () => {
    const managed = { managed: true, canEdit: true, guidance: "Managed service", shares: [{ id: "0123456789abcdef", directory: "/music/public", localPath: "/music/public", remotePath: "/burrata-shares/0123456789abcdef", alias: "Public music", files: 42, directories: 3, excluded: false }] };
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_connections") return status;
      if (command === "get_slskd_shares") return managed;
      if (command === "get_managed_slskd_download_settings") return { managed: true, canEdit: true, settings: managedDownloadSettings, guidance: "Applying changes restarts the managed slskd service; active transfers may be interrupted." };
      if (command === "add_managed_slskd_share") return { ...managed, shares: [...managed.shares, { id: "fedcba9876543210", directory: "/music/new", localPath: "/music/new", remotePath: "/burrata-shares/fedcba9876543210", alias: "new", files: 3, directories: 0, excluded: false }] };
      if (command === "remove_managed_slskd_share") return { ...managed, shares: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(); await settle();
    expect(target.textContent).toContain("Folders shared by slskd");
    expect(target.textContent).toContain("Public music");
    expect(target.textContent).toContain("/music/public");
    expect(target.textContent).toContain("Files in shared folders are visible to other Soulseek users.");
    expect(target.textContent).toContain("never your home or personal-document folders");
    expect(target.querySelector<HTMLInputElement>('[aria-label="slskd API key"]')?.type).toBe("password");
    const pathInput = target.querySelector<HTMLInputElement>('[aria-label="Folder path on this computer"]')!;
    pathInput.value = "/music/new"; pathInput.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const warning = [...target.querySelectorAll(".sharing-warning")].find((node) => node.textContent?.includes("Files in a folder you add"))!;
    const add = target.querySelector<HTMLButtonElement>('[aria-label="Folder path on this computer"]')?.closest("section")?.querySelector<HTMLButtonElement>('button:not(.remove-share)');
    expect(warning.compareDocumentPosition(add!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    add!.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("add_managed_slskd_share", { directoryPath: "/music/new" });
    expect(target.textContent).toContain("new");
    target.querySelector<HTMLButtonElement>(".remove-share")!.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("remove_managed_slskd_share", { shareId: "0123456789abcdef" });
  });

  it("keeps separately configured slskd read-only and permits an explicit rescan", async () => {
    callMock.mockImplementation(async (command: string) => command === "get_connections" ? {
      ...status,
    } : command === "get_slskd_shares" ? ({ managed: false, canEdit: false, guidance: "Edit shares in slskd", shares: [] }) : command === "get_managed_slskd_download_settings" ? ({ managed: false, canEdit: false, settings: null, guidance: "Download settings are read-only for this separately configured slskd service." }) : command === "rescan_slskd_shares" ? ({ managed: false, canEdit: false, guidance: "Edit shares in slskd", shares: [] }) : []);
    const target = render(); await settle();
    expect(callMock).toHaveBeenCalledWith("get_slskd_shares");
    expect(target.textContent).toContain("No shared folders reported by slskd.");
    expect(target.querySelector<HTMLInputElement>('[aria-label="slskd API key"]')?.type).toBe("password");
    expect(target.querySelector('[aria-label="Folder path on this computer"]')).toBeNull();
    target.querySelector<HTMLButtonElement>(".slskd-shares button")!.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("rescan_slskd_shares");
  });

  it("loads managed controls, submits typed settings, and explains the restart", async () => {
    const managed = { managed: true, canEdit: true, settings: managedDownloadSettings, guidance: "Applying changes restarts the managed slskd service; active transfers may be interrupted." };
    const saved = { ...managed, settings: { ...managedDownloadSettings, slots: 24, retryAttempts: 5, partialPolicy: "overwrite" as const }, guidance: "Settings applied. The managed slskd service restarted; active transfers may have been interrupted." };
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_connections") return status;
      if (command === "get_slskd_shares") return { managed: true, canEdit: true, shares: [], guidance: "Managed service" };
      if (command === "get_managed_slskd_download_settings") return managed;
      if (command === "save_managed_slskd_download_settings") return saved;
      throw new Error(`Unexpected command ${command}`);
    });

    const target = render(); await settle();
    expect(callMock).toHaveBeenCalledWith("get_managed_slskd_download_settings");
    expect(target.querySelector<HTMLInputElement>('[aria-label="Concurrent download slots"]')?.value).toBe("32");
    expect(target.textContent).toContain("Applying changes restarts the managed slskd service");

    const slots = target.querySelector<HTMLInputElement>('[aria-label="Concurrent download slots"]')!;
    slots.value = "24"; slots.dispatchEvent(new Event("input", { bubbles: true }));
    const attempts = target.querySelector<HTMLInputElement>('[aria-label="Automatic retry attempts"]')!;
    attempts.value = "5"; attempts.dispatchEvent(new Event("input", { bubbles: true }));
    const policy = target.querySelector<HTMLSelectElement>('[aria-label="Partial download handling"]')!;
    policy.value = "overwrite"; policy.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    target.querySelector<HTMLButtonElement>(".slskd-download-settings button")!.click(); await settle();

    expect(callMock).toHaveBeenCalledWith("save_managed_slskd_download_settings", { settings: {
      slots: 24, speedLimitKib: null, retryAttempts: 5, retryDelayMs: 5000, retryMaxDelayMs: 60000, partialPolicy: "overwrite",
    } });
    expect(target.textContent).toContain("managed slskd service restarted");
  });

  it("keeps download settings read-only for a separately configured daemon", async () => {
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_connections") return status;
      if (command === "get_slskd_shares") return { managed: false, canEdit: false, shares: [], guidance: "Edit shares in slskd" };
      if (command === "get_managed_slskd_download_settings") return { managed: false, canEdit: false, settings: null, guidance: "Download settings are read-only for this separately configured slskd service." };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(); await settle();
    expect(target.textContent).toContain("Download settings are read-only for this separately configured slskd service.");
    expect(target.querySelector('[aria-label="Concurrent download slots"]')).toBeNull();
    expect([...target.querySelectorAll("button")].some((button) => button.textContent?.includes("Apply download settings"))).toBe(false);
    expect(callMock.mock.calls.some(([command]) => command === "save_managed_slskd_download_settings")).toBe(false);
  });
});
