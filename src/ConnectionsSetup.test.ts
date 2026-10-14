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
  it("shows managed shares and warns before explicit add/remove actions", async () => {
    const managed = { managed: true, canEdit: true, guidance: "Managed service", shares: [{ id: "0123456789abcdef", directory: "/music/public", localPath: "/music/public", remotePath: "/burrata-shares/0123456789abcdef", alias: "Public music", files: 42, directories: 3, excluded: false }] };
    callMock.mockImplementation(async (command: string) => {
      if (command === "get_connections") return status;
      if (command === "get_slskd_shares") return managed;
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
    } : command === "get_slskd_shares" ? ({ managed: false, canEdit: false, guidance: "Edit shares in slskd", shares: [] }) : command === "rescan_slskd_shares" ? ({ managed: false, canEdit: false, guidance: "Edit shares in slskd", shares: [] }) : []);
    const target = render(); await settle();
    expect(callMock).toHaveBeenCalledWith("get_slskd_shares");
    expect(target.textContent).toContain("No shared folders reported by slskd.");
    expect(target.querySelector<HTMLInputElement>('[aria-label="slskd API key"]')?.type).toBe("password");
    expect(target.querySelector('[aria-label="Folder path on this computer"]')).toBeNull();
    target.querySelector<HTMLButtonElement>(".slskd-shares button")!.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("rescan_slskd_shares");
  });
});
