// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LibraryCleanup from "./LibraryCleanup.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
const entry = (id: string, title: string) => ({
  id, selectionToken: `token-${id}`, classification: "unsortedLibraryEntry", provider: "youtube", providerId: id,
  title, artist: "Possible Artist", album: "Possible Album",
});

describe("LibraryCleanup", () => {
  it("requires selecting exact entries and a second explicit confirmation before removing library membership", async () => {
    const first = entry("legacy-a", "Possible old search result");
    const call = vi.fn(async (command: string) => {
      if (command === "preview_unsorted_library_entries") return { entries: [first], total: 1, limit: 25, offset: 0 };
      if (command === "remove_unsorted_library_entries") return { removedIds: ["legacy-a"], skipped: [] };
      throw new Error(`Unexpected command: ${command}`);
    });
    const onRemoved = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(LibraryCleanup, { target, props: { open: true, call, onRemoved } as never });
    destroy = () => unmount(instance); await settle();

    expect(target.textContent).toContain("may include old search results");
    expect(target.textContent).toContain("Source cache, downloaded files, favorites, playlists, and assets are kept");
    const checkbox = target.querySelector<HTMLInputElement>("input[type=checkbox]")!;
    expect(checkbox.checked).toBe(false);
    expect(call).toHaveBeenCalledTimes(1);
    expect(call).toHaveBeenCalledWith("preview_unsorted_library_entries", { limit: 25, offset: 0 });

    checkbox.click(); await settle();
    target.querySelector<HTMLButtonElement>(".review-removal")!.click(); await settle();
    expect(target.textContent).toContain("Remove 1 selected entry from the library?");
    expect(call).toHaveBeenCalledTimes(1);
    target.querySelector<HTMLButtonElement>(".remove-confirm")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("remove_unsorted_library_entries", { selections: [{ id: "legacy-a", selectionToken: "token-legacy-a" }] });
    expect(onRemoved).toHaveBeenCalledWith(["legacy-a"]);
    expect(target.textContent).toContain("Source cache and files were not deleted");
  });

  it("keeps selections across review pages and reports entries rejected by the backend", async () => {
    const first = entry("legacy-a", "First possible result"), second = entry("legacy-b", "Second possible result");
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "preview_unsorted_library_entries") {
        return args?.offset === 0 ? { entries: [first], total: 2, limit: 1, offset: 0 } : { entries: [second], total: 2, limit: 1, offset: 1 };
      }
      return { removedIds: [], skipped: [{ id: "legacy-a", reason: "entry changed after review" }] };
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(LibraryCleanup, { target, props: { open: true, call, pageSize: 1 } as never });
    destroy = () => unmount(instance); await settle();
    target.querySelector<HTMLInputElement>("input[type=checkbox]")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".cleanup-pages button:nth-child(2)")!.click(); await settle();
    expect(target.textContent).toContain("Second possible result");
    expect(target.textContent).toContain("1 selected");
    target.querySelector<HTMLButtonElement>(".review-removal")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".remove-confirm")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("remove_unsorted_library_entries", { selections: [{ id: "legacy-a", selectionToken: "token-legacy-a" }] });
    expect(target.textContent).toContain("entry changed after review");
  });
});
