// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PortableHistory from "./PortableHistory.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function settle() { for (let index = 0; index < 8; index++) await Promise.resolve(); await tick(); }
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, includeYtdlpSearchDefault = false, onClose = vi.fn()) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PortableHistory, { target, props: { call, includeYtdlpSearchDefault, onClose } as never });
  destroy = () => unmount(instance);
  return target;
}
function button(target: HTMLElement, label: string) {
  return [...target.querySelectorAll("button")].find((element) => element.textContent?.trim() === label)!;
}
function openAdvanced(target: HTMLElement) {
  target.querySelector(".advanced summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

describe("PortableHistory", () => {
  it("previews the ordered track list with dates and search links off by default", async () => {
    const call = vi.fn(async (command: string) => command === "preview_history_playlist_export"
      ? { entryCount: 4, identityPlaceholders: 2, searchLinks: 0, excludedPrivateFields: ["history dates and session IDs"] }
      : "#EXTM3U\n");
    const target = render(call);
    await settle();
    expect(target.textContent).toContain("Save the ordered track list from your listening history.");
    expect(target.textContent).toContain("Music files aren’t included.");
    expect(target.querySelector<HTMLDetailsElement>(".advanced")?.open).toBe(false);
    expect(target.querySelectorAll("input[type=checkbox]")).toHaveLength(2);
    expect([...target.querySelectorAll<HTMLInputElement>("input[type=checkbox]")].every(input => !input.checked)).toBe(true);
    expect([...target.querySelectorAll("button")].map(item => item.textContent?.trim())).not.toContain("Resume local history mirror");

    openAdvanced(target); await settle();
    await button(target, "Update preview").click(); await settle();
    expect(call).toHaveBeenCalledWith("preview_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    expect(target.textContent).toContain("4 tracks");
    expect(target.textContent).toContain("2 tracks need a playable link. Burrata can match them when imported.");
    expect(target.querySelector<HTMLDetailsElement>(".privacy-details")?.open).toBe(false);

    const dates = target.querySelector<HTMLInputElement>('[aria-label="Include listening dates"]')!;
    dates.checked = true; dates.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(target.querySelector(".preview")).toBeNull();
    await button(target, "Update preview").click(); await settle();
    expect(call).toHaveBeenLastCalledWith("preview_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: true },
    });
    expect(target.textContent).toContain("Listening dates are included in this file.");
  });

  it("previews current history date options before saving the playlist file", async () => {
    const createObjectURL = vi.fn(() => "blob:test");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", { ...URL, createObjectURL, revokeObjectURL });
    const anchorClick = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const call = vi.fn(async (command: string) => {
      if (command === "preview_history_playlist_export") return { entryCount: 1, identityPlaceholders: 1, searchLinks: 0, excludedPrivateFields: [] };
      if (command === "render_history_playlist_export") return "#EXTM3U\n#BURRATA-PLAYLIST:{}\n";
      return {};
    });
    const target = render(call);
    await settle();
    await button(target, "Save playlist file").click(); await settle();
    expect(call).toHaveBeenCalledWith("preview_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    expect(call).toHaveBeenCalledWith("render_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    expect(createObjectURL).toHaveBeenCalledOnce();
    expect(anchorClick).toHaveBeenCalledOnce();
    expect(target.textContent).toContain("Listening history saved as a playlist file.");
    expect(target.textContent).toContain("1 track needs a playable link.");
  });

  it("copies history text only from Advanced and uses the saved search-link default", async () => {
    const writeText = vi.fn(async () => {});
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const call = vi.fn(async (command: string) => {
      if (command === "preview_history_playlist_export") return { entryCount: 1, identityPlaceholders: 0, searchLinks: 1, excludedPrivateFields: [] };
      if (command === "render_history_playlist_export") return "#EXTM3U\n";
      return {};
    });
    const target = render(call, true);
    await settle();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(true);
    expect(target.querySelector<HTMLDetailsElement>(".advanced")?.open).toBe(false);
    openAdvanced(target); await settle();
    const search = target.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')!;
    search.checked = false; search.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    await button(target, "Update preview").click(); await settle();
    expect(call).toHaveBeenLastCalledWith("preview_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    await button(target, "Copy playlist file contents").click(); await settle();
    expect(call).toHaveBeenLastCalledWith("render_history_playlist_export", {
      options: { purpose: "history", includeYtdlpSearch: false, includeNotes: false, includeTags: false, includeRatings: false, includeHistoryDates: false },
    });
    expect(writeText).toHaveBeenCalledWith("#EXTM3U\n");
    expect(target.textContent).toContain("Playlist text copied to clipboard.");
  });

  it("uses an updated saved search-link default when the history dialog is reopened", async () => {
    const call = vi.fn(async () => ({}));
    const first = render(call, false);
    await settle();
    expect(first.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(false);
    destroy?.(); destroy = undefined; document.body.innerHTML = "";
    const reopened = render(call, true);
    await settle();
    expect(reopened.querySelector<HTMLInputElement>('[aria-label="Include search links for this export"]')?.checked).toBe(true);
  });

  it("closes on Escape or an outside click and restores focus", async () => {
    const opener = document.createElement("button"); document.body.append(opener); opener.focus();
    const onClose = vi.fn();
    const target = render(vi.fn(async () => ({})), false, onClose);
    await settle();
    const dialog = target.querySelector<HTMLDialogElement>("dialog")!;
    dialog.getBoundingClientRect = () => ({ left: 20, top: 20, right: 320, bottom: 260, width: 300, height: 240, x: 20, y: 20, toJSON: () => ({}) });
    target.querySelector(".advanced summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    dialog.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 30, clientY: 30 }));
    expect(onClose).not.toHaveBeenCalled();
    const cancel = new Event("cancel", { cancelable: true });
    dialog.dispatchEvent(cancel);
    expect(cancel.defaultPrevented).toBe(true);
    expect(onClose).toHaveBeenCalledOnce();
    dialog.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 0, clientY: 0 }));
    expect(onClose).toHaveBeenCalledTimes(2);
    destroy?.(); destroy = undefined;
    expect(document.activeElement).toBe(opener);
  });
});
