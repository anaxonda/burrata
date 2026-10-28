// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaylistSettings from "./PlaylistSettings.svelte";

const collections = [
  { id: "favorites", name: "Liked Songs", trackCount: 2 },
  { id: "set-a", name: "Warmup", trackCount: 8 },
];
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, includeYtdlpSearchDefault = false, onYtdlpSearchDefaultChange = vi.fn()) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlaylistSettings, { target, props: {
    preferences: { favoritesPlaylistId: "favorites", shortcuts: ["set-a", null, null, null, null, null, null, null, null], shortcutBindings: ["Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9"] },
    collections, call, onchange: vi.fn(), oncollections: vi.fn(), includeYtdlpSearchDefault, onYtdlpSearchDefaultChange,
  } });
  destroy = () => unmount(instance);
  return target;
}

describe("PlaylistSettings", () => {
  it("shows the Favorites destination, nine playlist slots, and editable key bindings", async () => {
    const target = render(vi.fn(async () => ({})));
    await settle();
    expect(target.textContent).toContain("Favorites playlist");
    expect(target.querySelectorAll('select[aria-label^="Playlist shortcut "]')).toHaveLength(9);
    expect(target.querySelector<HTMLInputElement>('[aria-label="Shortcut key 1"]')?.value).toBe("Alt+1");
  });

  it("persists destination and key changes through the supplied command bridge", async () => {
    const call = vi.fn(async (command: string) => command === "set_favorites_playlist"
      ? { favoritesPlaylistId: "set-a", shortcuts: ["set-a", null, null, null, null, null, null, null, null], shortcutBindings: ["Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9"] }
      : { favoritesPlaylistId: "favorites", shortcuts: ["set-a", null, null, null, null, null, null, null, null], shortcutBindings: ["Ctrl+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9"] });
    const target = render(call);
    target.querySelector<HTMLSelectElement>('[aria-label="Favorites playlist"]')!.value = "set-a";
    target.querySelector<HTMLSelectElement>('[aria-label="Favorites playlist"]')!.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(call).toHaveBeenCalledWith("set_favorites_playlist", { collectionId: "set-a" });
    const key = target.querySelector<HTMLInputElement>('[aria-label="Shortcut key 1"]')!;
    key.value = "Ctrl+1"; key.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(call).toHaveBeenCalledWith("set_playlist_shortcut_binding", { slot: 0, binding: "Ctrl+1" });
  });

  it("exposes the persisted yt-dlp default in common playlist settings", async () => {
    const change = vi.fn();
    const target = render(vi.fn(async () => ({})), true, change);
    await settle();
    const search = target.querySelector<HTMLInputElement>('[aria-label="Include search links by default"]')!;
    expect(search.checked).toBe(true);
    search.checked = false; search.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(change).toHaveBeenCalledWith(false);
  });
});
