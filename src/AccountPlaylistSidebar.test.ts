// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AccountPlaylistSidebar from "./AccountPlaylistSidebar.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
function render(props: { provider?: "spotify" | "youtube"; enabled: boolean; call: (command: string, args?: Record<string, unknown>) => Promise<unknown>; importedUrls?: string[]; onOpen?: (url: string) => void }) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(AccountPlaylistSidebar, { target, props: {
    provider: "spotify", onOpen: vi.fn(), ...props,
  } });
  destroy = () => unmount(instance);
  return target;
}

describe("AccountPlaylistSidebar", () => {
  it("makes no account request while its privacy setting is off", async () => {
    const call = vi.fn(async () => ({ items: [], nextCursor: null }));
    const target = render({ enabled: false, call }); await settle();
    expect(target.textContent).toContain("Enable account playlist listing in Settings");
    expect(target.textContent).toContain("Use Search to find other public playlists");
    expect(call).not.toHaveBeenCalled();
  });

  it("loads pages, hides imported URLs, and opens a selected playlist without importing", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "list_remote_playlists") throw new Error(`Unexpected command ${command}`);
      if (args?.cursor === "page-2") return { items: [
        { id: "duplicate", title: "Duplicate URL", url: "https://open.spotify.com/playlist/one?si=tracking" },
        { id: "two", title: "Second mix", url: "https://open.spotify.com/playlist/two", trackCount: 8 },
      ], nextCursor: null };
      return { items: [
        { id: "one", title: "First mix", url: "https://open.spotify.com/playlist/one", ownerName: "Owner", trackCount: 4 },
        { id: "already-imported", title: "Already local", url: "https://open.spotify.com/playlist/local" },
        { id: "imported-flag", title: "Imported flag", url: "https://open.spotify.com/playlist/flag", imported: true },
      ], nextCursor: "page-2" };
    });
    const onOpen = vi.fn();
    const target = render({ enabled: true, call, onOpen, importedUrls: ["https://open.spotify.com/playlist/local?si=old"] }); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", requestId: expect.any(String) }));
    expect(target.textContent).toContain("First mix");
    expect(target.textContent).not.toContain("Already local");
    expect(target.textContent).not.toContain("Imported flag");

    target.querySelector<HTMLButtonElement>(".load-more")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", cursor: "page-2", requestId: expect.any(String) }));
    expect(target.textContent).toContain("Second mix");
    expect(target.textContent).not.toContain("Duplicate URL");
    target.querySelector<HTMLButtonElement>(".playlist-title")!.click();
    expect(onOpen).toHaveBeenCalledWith("https://open.spotify.com/playlist/one");
    expect(call.mock.calls.every(([command]) => command === "list_remote_playlists")).toBe(true);
  });
});
