// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import FolderTree from "./FolderTree.svelte";
// @ts-expect-error Reuse Svelte's reactive prop proxy to model parent revision updates.
import { proxy } from "../node_modules/svelte/src/internal/client/proxy.js";

let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(FolderTree, { target, props: props as never });
  cleanup = () => unmount(instance);
  return target;
}
const root = { id: "root-1", name: "Music", path: "/music" };
const children = [{ name: "Albums", relativePath: "Albums" }, { name: "Mixes", relativePath: "Mixes" }];

describe("FolderTree", () => {
  it("loads children lazily and keeps expansion separate from folder selection", async () => {
    const call = vi.fn(async (command: string) => command === "list_directory_browser_roots" ? [root] : { rootId: root.id, relativePath: "", directories: children });
    const onSelectFolder = vi.fn();
    const target = render({ call, selectedFolder: { rootId: root.id, relativePath: "Albums" }, onSelectFolder });
    await settle();
    expect(target.querySelectorAll('[role="treeitem"]')).toHaveLength(1);
    const rootRow = target.querySelector<HTMLButtonElement>('[role="treeitem"]')!;
    rootRow.querySelector(".folder-tree-chevron")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(call).toHaveBeenCalledWith("list_directory_browser_children", { rootId: root.id, relativePath: "" });
    expect(target.querySelectorAll('[role="treeitem"]')).toHaveLength(3);
    expect(onSelectFolder).not.toHaveBeenCalled();
    const selected = target.querySelector<HTMLElement>('[role="treeitem"][aria-current="location"]');
    expect(selected?.getAttribute("aria-label")).toBe("Albums");
    selected!.click();
    expect(onSelectFolder).toHaveBeenCalledWith({ rootId: root.id, relativePath: "Albums" });
  });

  it("opens the folder browser from its title and collapses only the tree body", async () => {
    const call = vi.fn(async () => [root]);
    const onOpenFolders = vi.fn();
    const target = render({ call, onOpenFolders });
    await settle();
    target.querySelector<HTMLButtonElement>('button[aria-label="Folders"]')!.click();
    expect(onOpenFolders).toHaveBeenCalledTimes(1);
    target.querySelector<HTMLButtonElement>('[aria-label="Collapse folder tree"]')!.click();
    await settle();
    expect(target.querySelector('[aria-label="Expand folder tree"]')?.getAttribute("aria-expanded")).toBe("false");
    expect(target.querySelector('[role="tree"]')).toBeNull();
    target.querySelector<HTMLButtonElement>('[aria-label="Expand folder tree"]')!.click();
    await settle();
    expect(target.querySelectorAll('[role="treeitem"]')).toHaveLength(1);
  });

  it("supports tree navigation keys and selects with Enter", async () => {
    const call = vi.fn(async (command: string) => command === "list_directory_browser_roots" ? [root] : { rootId: root.id, relativePath: "", directories: children });
    const onSelectFolder = vi.fn();
    const target = render({ call, onSelectFolder });
    await settle();
    let item = target.querySelector<HTMLButtonElement>('[role="treeitem"]')!;
    item.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(target.querySelectorAll('[role="treeitem"]')).toHaveLength(3);
    item.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await settle();
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Albums");
    item = document.activeElement as HTMLButtonElement;
    item.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
    await settle();
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Mixes");
    (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(onSelectFolder).toHaveBeenCalledWith({ rootId: root.id, relativePath: "Mixes" });
    (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true }));
    await settle();
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Music");
  });

  it("ignores child results made stale by a root registry refresh", async () => {
    let finishChildren!: (value: { rootId: string; relativePath: string; directories: typeof children }) => void;
    let rootCalls = 0;
    const call = vi.fn((command: string) => {
      if (command === "list_directory_browser_roots") { rootCalls++; return Promise.resolve([root]); }
      return new Promise<{ rootId: string; relativePath: string; directories: typeof children }>(resolve => { finishChildren = resolve; });
    });
    const props = proxy({ call, rootsRevision: 0 });
    const target = render(props);
    await settle();
    target.querySelector<HTMLElement>(".folder-tree-chevron")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    props.rootsRevision = 1;
    await settle();
    expect(rootCalls).toBeGreaterThanOrEqual(2);
    finishChildren({ rootId: root.id, relativePath: "", directories: children });
    await settle();
    expect(target.querySelectorAll('[role="treeitem"]')).toHaveLength(1);
  });

  it("adds and unregisters roots without deleting their files", async () => {
    const roots = [root];
    const call = vi.fn(async (command: string) => {
      if (command === "list_directory_browser_roots") return [...roots];
      if (command === "choose_directory_browser_root") return { path: "/archive" };
      if (command === "add_directory_browser_root") { const added = { id: "root-2", name: "Archive", path: "/archive" }; roots.push(added); return added; }
      if (command === "remove_directory_browser_root") { roots.splice(roots.findIndex(item => item.id === "root-2"), 1); return undefined; }
      throw new Error(`Unexpected command ${command}`);
    });
    const onSelectFolder = vi.fn(); const onRootsChanged = vi.fn();
    const props = proxy({ call, onSelectFolder, onRootsChanged, selectedFolder: null as { rootId: string; relativePath: string } | null });
    const target = render(props);
    await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Add folder"]')!.click();
    await settle();
    expect(onSelectFolder).toHaveBeenCalledWith({ rootId: "root-2", relativePath: "" });
    expect(onRootsChanged).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("Archive");
    props.selectedFolder = { rootId: "root-2", relativePath: "" };
    await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Unregister folder Archive"]')!.click();
    await settle();
    expect(call).toHaveBeenCalledWith("remove_directory_browser_root", { rootId: "root-2" });
    expect(target.textContent).not.toContain("Archive");
    expect(onRootsChanged).toHaveBeenCalledTimes(2);
    expect(onSelectFolder).toHaveBeenLastCalledWith({ rootId: root.id, relativePath: "" });
  });
});
