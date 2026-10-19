// @vitest-environment jsdom
// @ts-expect-error Exercise the same browser runtime as the app.
import { createRawSnippet, mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ActionMenu from "./ActionMenu.svelte";

let dispose: (() => void) | undefined;
let originalShowModal: PropertyDescriptor | undefined;
beforeEach(() => {
  originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal("innerWidth", 420); vi.stubGlobal("innerHeight", 700);
});
afterEach(() => {
  dispose?.(); dispose = undefined;
  document.body.innerHTML = "";
  if (originalShowModal) Object.defineProperty(HTMLDialogElement.prototype, "showModal", originalShowModal);
  else Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal");
  vi.unstubAllGlobals(); vi.restoreAllMocks();
});
const content = () => createRawSnippet(() => ({ render: () => '<button class="primary-action">Run action</button>' }));

describe("shared action menu frame", () => {
  it("clamps a point-anchored menu to the viewport and the supplied expanded player height", async () => {
    const instance = mount(ActionMenu, { target: document.body, props: { label: "Shared actions", title: "A title", anchor: { x: 410, y: 690 }, playerHeight: 140, onClose: vi.fn(), children: content() } });
    dispose = () => unmount(instance); await tick();
    const dialog = document.querySelector<HTMLDialogElement>(".action-menu")!;
    expect(dialog.open).toBe(true);
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(Number.parseFloat(dialog.style.left)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(dialog.style.left)).toBeLessThanOrEqual(92);
    expect(Number.parseFloat(dialog.style.top)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(dialog.style.top)).toBeLessThanOrEqual(312);
    expect(dialog.style.getPropertyValue("--menu-player-height")).toBe("140px");
    expect(document.activeElement?.textContent).toBe("Run action");
  });

  it("restores the opener after Escape but preserves focus moved outside before layout dismissal", async () => {
    const opener = document.createElement("button"), sentinel = document.createElement("button");
    document.body.append(opener, sentinel); opener.focus();
    const onClose = vi.fn();
    let instance = mount(ActionMenu, { target: document.body, props: { label: "Queue actions", role: "menu", onClose, children: content() } });
    dispose = () => unmount(instance); await tick();
    const action = document.querySelector<HTMLButtonElement>(".primary-action")!;
    action.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(onClose).toHaveBeenCalledOnce();
    unmount(instance); dispose = undefined;
    expect(document.activeElement).toBe(opener);
    instance = mount(ActionMenu, { target: document.body, props: { label: "Queue actions", onClose, children: content() } });
    dispose = () => unmount(instance); await tick(); sentinel.focus();
    window.dispatchEvent(new Event("resize"));
    expect(onClose).toHaveBeenCalledTimes(2);
    unmount(instance); dispose = undefined;
    expect(document.activeElement).toBe(sentinel);
  });

  it("keeps internal scrolling open and dismisses on outside scrolling", async () => {
    const onClose = vi.fn();
    const instance = mount(ActionMenu, { target: document.body, props: { label: "Shared actions", onClose, children: content() } });
    dispose = () => unmount(instance); await tick();
    document.querySelector(".action-menu")!.dispatchEvent(new Event("scroll"));
    expect(onClose).not.toHaveBeenCalled();
    window.dispatchEvent(new Event("scroll"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
