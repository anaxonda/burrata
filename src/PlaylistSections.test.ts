// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte's internal browser runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaylistSections from "./PlaylistSections.svelte";
import { PLAYLIST_SECTION_DRAG_TYPE } from "./lib/playlist-sections";

const sections = [{ id: "workout", name: "Workout" }, { id: "late", name: "Late Night" }];
const collections = [
  { id: "p1", name: "House Set", trackCount: 12 },
  { id: "p2", name: "Jazz Mix", trackCount: 4 },
];
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 7; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const callbacks = {
    onPlaylist: vi.fn(), onPlaylistContext: vi.fn(), onCreateSection: vi.fn(), onRenameSection: vi.fn(),
    onDeleteSection: vi.fn(), onMovePlaylist: vi.fn(), onCreatePlaylist: vi.fn(),
  };
  const instance = mount(PlaylistSections, { target, props: {
    sections, assignments: { p1: "workout", gone: "late" }, collections, activeCollectionId: "", ...callbacks, ...props,
  } });
  destroy = () => unmount(instance);
  return { target, ...callbacks };
}
function dragEvent(type: string, transfer: DataTransfer) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: transfer });
  return event;
}
function transferFor(id: string, type = PLAYLIST_SECTION_DRAG_TYPE): DataTransfer {
  return { types: [type], effectAllowed: "", dropEffect: "", setData: vi.fn(), getData: vi.fn((requested: string) => requested === type ? id : "") } as unknown as DataTransfer;
}

describe("PlaylistSections", () => {
  it("renders unassigned and assigned playlists once with playlist creation in each section", async () => {
    const { target, onCreatePlaylist } = render();
    await settle();
    expect(target.querySelector('[aria-label="Playlist sections"]')?.textContent).toContain("Unsorted");
    expect(target.querySelector('[aria-label="Playlist sections"]')?.textContent).toContain("House Set");
    expect(target.querySelector('[aria-label="Playlist sections"]')?.textContent).toContain("Jazz Mix");
    expect(target.querySelectorAll('[aria-label="Move House Set"]')).toHaveLength(1);
    target.querySelector<HTMLButtonElement>('[aria-label="Create playlist in Workout"]')!.click();
    expect(onCreatePlaylist).toHaveBeenCalledWith("workout");
    target.querySelector<HTMLButtonElement>('[aria-label="Create playlist in Unsorted"]')!.click();
    expect(onCreatePlaylist).toHaveBeenLastCalledWith(undefined);
  });

  it("creates, renames, and deletes a section through explicit controls", async () => {
    const { target, onCreateSection, onRenameSection, onDeleteSection } = render();
    await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Create playlist section"]')!.click();
    await settle();
    const createInput = target.querySelector<HTMLInputElement>('[aria-label="New section name"]')!;
    createInput.value = "  Driving  "; createInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    target.querySelector<HTMLButtonElement>('.section-form button[type="submit"]')!.click();
    expect(onCreateSection).toHaveBeenCalledWith("Driving");
    target.querySelector<HTMLButtonElement>('[aria-label="Rename section Workout"]')!.click();
    await settle();
    const renameInput = target.querySelector<HTMLInputElement>('[aria-label="New name for Workout"]')!;
    renameInput.value = "Warmup"; renameInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    target.querySelector<HTMLButtonElement>('.rename-form button[type="submit"]')!.click();
    expect(onRenameSection).toHaveBeenCalledWith("workout", "Warmup");
    target.querySelector<HTMLButtonElement>('[aria-label="Delete section Late Night"]')!.click();
    expect(onDeleteSection).toHaveBeenCalledWith("late");
  });

  it("supports keyboard section moves and rejects external or unknown drag payloads", async () => {
    const { target, onMovePlaylist } = render({ assignments: {} });
    await settle();
    const move = target.querySelector<HTMLSelectElement>('[aria-label="Move House Set"]')!;
    move.value = "workout"; move.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onMovePlaylist).toHaveBeenCalledWith("p1", "workout");
    const unsorted = target.querySelector<HTMLDetailsElement>(".playlist-section")!;
    unsorted.dispatchEvent(dragEvent("drop", transferFor("outside-id")));
    unsorted.dispatchEvent(dragEvent("drop", transferFor("p1", "text/plain")));
    expect(onMovePlaylist).toHaveBeenCalledTimes(1);
  });

  it("moves a playlist through validated internal drag data and provides a context callback", async () => {
    const { target, onMovePlaylist, onPlaylistContext } = render({ assignments: {} });
    await settle();
    const row = target.querySelector<HTMLElement>('[aria-label="Move House Set"]')!.closest(".playlist-row")!;
    const transfer = transferFor("p1");
    row.dispatchEvent(dragEvent("dragstart", transfer));
    expect(transfer.setData).toHaveBeenCalledWith(PLAYLIST_SECTION_DRAG_TYPE, "p1");
    const destination = target.querySelector<HTMLDetailsElement>('.playlist-section:nth-of-type(2)')!;
    destination.dispatchEvent(dragEvent("dragover", transfer));
    destination.dispatchEvent(dragEvent("drop", transfer));
    expect(onMovePlaylist).toHaveBeenCalledWith("p1", "workout");
    row.querySelector<HTMLButtonElement>(".playlist-title")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 80, clientY: 90 }));
    expect(onPlaylistContext).toHaveBeenCalledWith("p1", expect.any(MouseEvent));
  });
});
