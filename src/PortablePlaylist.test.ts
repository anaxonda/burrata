// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PortablePlaylist from "./PortablePlaylist.svelte";

const emptyPreview = { entryCount: 0, localEntries: 0, identityPlaceholders: 0, searchLinks: 0, requiresBurrata: false, excludedPrivateFields: [] };

const collection = { id: "playlist-1", name: "Evening set", trackCount: 2 };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let index = 0; index < 8; index++) await Promise.resolve(); await tick(); }
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, includeYtdlpSearchDefault = false, initialMode: "export" | "folder" = "export", onClose = vi.fn()) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PortablePlaylist, { target, props: { collection, call, initialMode, includeYtdlpSearchDefault, onClose } as never });
  destroy = () => unmount(instance);
  return target;
}
function button(target: HTMLElement, label: string) {
  return [...target.querySelectorAll("button")].find((element) => element.textContent?.trim() === label)!;
}

describe("PortablePlaylist", () => {
  it("keeps sharing private by default and shows a plain preview", async () => {
    const call = vi.fn(async (command: string) => command === "preview_portable_playlist_export"
      ? { entryCount: 2, localEntries: 0, identityPlaceholders: 2, searchLinks: 0, requiresBurrata: true, excludedPrivateFields: ["private provider URLs", "filesystem paths", "personal tags"] }
      : "#EXTM3U\n#BURRATA-PLAYLIST:{}\n");
    const target = render(call);
    await settle();
    expect(target.textContent).toContain("Save playlist file");
    expect(target.textContent).toContain("Shared files leave out local paths, private provider links, and personal details.");
    expect(target.textContent).toContain("Music files aren’t included.");
    expect(target.querySelector<HTMLDetailsElement>(".advanced")?.open).toBe(false);
    const checkboxes = [...target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes.every((input) => !input.checked)).toBe(true);
    target.querySelector(".advanced summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    await settle();
    expect(call).toHaveBeenCalledWith("preview_portable_playlist_export", {
      collectionId: "playlist-1",
      options: { purpose: "share", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    expect(target.textContent).toContain("2 tracks need a playable link. Burrata can match them when imported.");
    expect(target.querySelector<HTMLDetailsElement>(".privacy-details")?.open).toBe(false);
  });

  it("loads the persisted search-link default and keeps the checkbox as a per-export override", async () => {
    const call = vi.fn(async (command: string) => command === "preview_portable_playlist_export"
      ? { entryCount: 2, localEntries: 0, identityPlaceholders: 0, searchLinks: 2, requiresBurrata: false, excludedPrivateFields: [] }
      : "#EXTM3U\n");
    const target = render(call, true);
    await settle();
    const search = target.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')!;
    expect(search.checked).toBe(true);
    target.querySelector(".advanced summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    expect(call).toHaveBeenLastCalledWith("preview_portable_playlist_export", {
      collectionId: "playlist-1",
      options: { purpose: "share", includeYtdlpSearch: true, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    search.checked = false; search.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(call).toHaveBeenLastCalledWith("preview_portable_playlist_export", {
      collectionId: "playlist-1",
      options: { purpose: "share", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
  });

  it("uses an updated saved default when a new playlist dialog is opened", async () => {
    const call = vi.fn(async () => emptyPreview);
    const first = render(call, false);
    await settle();
    expect(first.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(false);
    destroy?.(); destroy = undefined; document.body.innerHTML = "";
    const reopened = render(call, true);
    await settle();
    expect(reopened.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(true);
  });

  it("keeps imports out of the export dialog", async () => {
    const target = render(vi.fn(async () => emptyPreview));
    await settle();
    expect(target.querySelector('input[type="file"]')).toBeNull();
    expect([...target.querySelectorAll("button")].map(item => item.textContent?.trim())).not.toContain("Import");
  });

  it("copies playlist text only from the closed advanced options", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const text = "#EXTM3U\n#EXTINF:-1,Song\n";
    const call = vi.fn(async (command: string) => command === "render_portable_playlist_export" ? text : command === "preview_portable_playlist_export" ? { entryCount: 1, localEntries: 0, identityPlaceholders: 1, searchLinks: 0, requiresBurrata: true, excludedPrivateFields: [] } : {});
    const target = render(call);
    await settle();
    expect(target.querySelector<HTMLDetailsElement>(".advanced")?.open).toBe(false);
    target.querySelector(".advanced summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    await button(target, "Copy playlist file contents").click(); await settle();
    expect(writeText).toHaveBeenCalledWith(text);
    expect(target.textContent).toContain("Portable playlist copied to clipboard.");
  });

  it("opens the folder move action directly", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => command === "get_playlist_hierarchy"
      ? { schemaVersion: 1, folders: [{ id: "folder-old", name: "Old", parentId: null, position: 0 }, { id: "folder-set", name: "Evening", parentId: "folder-sets", position: 0 }, { id: "folder-sets", name: "Sets", parentId: null, position: 1 }], playlists: [{ collectionId: collection.id, folderId: "folder-old", position: 0, sections: [{ id: "section-1", name: "Side A", position: 0 }], occurrences: [{ id: "occ-1", sectionId: "section-1" }] }] }
      : args?.hierarchy ?? {});
    const target = render(call, false, "folder");
    await settle();
    expect(target.querySelector("h2")?.textContent).toBe("Move to folder");
    expect(call).toHaveBeenCalledWith("get_playlist_hierarchy");
    expect(target.textContent).toContain("Folder path");
    expect(target.textContent).not.toContain("Sections, one per line");
    const path = target.querySelector<HTMLInputElement>(".field-row input")!;
    expect(path.value).toBe("Old");
    path.value = "Sets/Evening"; path.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    await button(target, "Move playlist").click(); await settle();
    const saved = call.mock.calls.find(([command]) => command === "save_playlist_hierarchy")?.[1]?.hierarchy as { playlists: { folderId: string | null; sections: unknown[]; occurrences: unknown[] }[] };
    expect(saved.playlists[0].folderId).toBe("folder-set");
    expect(saved.playlists[0].sections).toEqual([{ id: "section-1", name: "Side A", position: 0 }]);
    expect(saved.playlists[0].occurrences).toEqual([{ id: "occ-1", sectionId: "section-1" }]);
  });

  it("closes on Escape and restores focus when dismissed", async () => {
    const opener = document.createElement("button"); document.body.append(opener); opener.focus();
    const onClose = vi.fn();
    const target = render(vi.fn(async () => emptyPreview), false, "export", onClose);
    await settle();
    const cancel = new Event("cancel", { cancelable: true });
    target.querySelector("dialog")!.dispatchEvent(cancel);
    expect(cancel.defaultPrevented).toBe(true);
    expect(onClose).toHaveBeenCalledOnce();
    destroy?.(); destroy = undefined;
    expect(document.activeElement).toBe(opener);
  });

});
