// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaylistMenu from "./PlaylistMenu.svelte";

let cleanup: (() => void) | undefined;
let originalShowModal: PropertyDescriptor | undefined;
afterEach(() => {
  cleanup?.(); cleanup = undefined;
  if (originalShowModal) Object.defineProperty(HTMLDialogElement.prototype, "showModal", originalShowModal);
  else Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal");
  originalShowModal = undefined;
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("PlaylistMenu", () => {
  it("offers the separate Move to folder action when the playlist can be organized", async () => {
    vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
    originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const target = document.createElement("div"); document.body.append(target);
    const onMoveToFolder = vi.fn();
    const onClose = vi.fn();
    const instance = mount(PlaylistMenu, { target, props: {
      name: "Evening set", anchor: { x: 40, y: 40 }, playerHeight: 90,
      onClose, onOpen: vi.fn(), onMoveToFolder,
    } as never });
    cleanup = () => unmount(instance);
    await tick();
    const move = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Move to folder…")!;
    expect(move).toBeTruthy();
    move.click();
    expect(onMoveToFolder).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("uses contained dialog focus, restores the opener on unmount, and closes on Escape", async () => {
    originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
    const showModal = vi.fn(function (this: HTMLDialogElement) { this.setAttribute("open", ""); });
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value: showModal,
    });
    vi.stubGlobal("ResizeObserver", class {
      observe() {}
      unobserve() {}
      disconnect() {}
    });
    const opener = document.createElement("button"); opener.textContent = "Open playlist actions"; document.body.append(opener); opener.focus();
    const target = document.createElement("div"); document.body.append(target);
    const onClose = vi.fn();
    const instance = mount(PlaylistMenu, { target, props: {
      name: "Test playlist", anchor: { x: 40, y: 40 }, playerHeight: 90,
      onClose, onOpen: vi.fn(),
    } as never });
    cleanup = () => unmount(instance);
    await tick();
    const menu = target.querySelector("dialog")!;
    expect(showModal).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(target.querySelector(".primary-action"));
    const appShortcut = vi.fn(); window.addEventListener("keydown", appShortcut);
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    menu.dispatchEvent(escape);
    window.removeEventListener("keydown", appShortcut);
    expect(escape.defaultPrevented).toBe(true);
    expect(appShortcut).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
    unmount(instance); cleanup = undefined;
    expect(document.activeElement).toBe(opener);
  });

  it("keeps click-away dismissal when the dialog uses modal focus containment", async () => {
    originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value(this: HTMLDialogElement) { this.setAttribute("open", ""); },
    });
    const outside = document.createElement("button"); document.body.append(outside);
    const target = document.createElement("div"); document.body.append(target);
    const onClose = vi.fn();
    const instance = mount(PlaylistMenu, { target, props: {
      name: "Test playlist", anchor: { x: 40, y: 40 }, playerHeight: 90,
      onClose, onOpen: vi.fn(),
    } as never });
    cleanup = () => unmount(instance);
    await tick();
    outside.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("wraps Tab at the visible focus boundaries and leaves interior Tab movement to the dialog", async () => {
    originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(PlaylistMenu, { target, props: {
      name: "Test playlist", anchor: { x: 40, y: 40 }, playerHeight: 90,
      onClose: vi.fn(), onOpen: vi.fn(), onDelete: vi.fn(), onRename: vi.fn(), onDownload: vi.fn(),
    } as never });
    cleanup = () => unmount(instance);
    await tick();
    const menu = target.querySelector("dialog")!;
    const buttons = [...menu.querySelectorAll<HTMLButtonElement>("button")];
    for (const button of buttons) button.getClientRects = () => { const rects = [new DOMRect(0, 0, 1, 1)]; return Object.assign(rects, { item: (index: number) => rects[index] ?? null }); };
    const first = buttons[0], disabledLast = buttons[buttons.length - 1]!;
    disabledLast.disabled = true; disabledLast.tabIndex = 0;
    const excluded = document.createElement("button"); excluded.tabIndex = -1; menu.append(excluded); excluded.getClientRects = () => { const rects = [new DOMRect(0, 0, 1, 1)]; return Object.assign(rects, { item: (index: number) => rects[index] ?? null }); };
    const last = buttons[buttons.length - 2]!;
    last.focus();
    const forward = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    menu.dispatchEvent(forward);
    expect(forward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    expect(last.disabled).toBe(false);
    const interior = buttons[1]; interior.focus();
    const normal = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    menu.dispatchEvent(normal);
    expect(normal.defaultPrevented).toBe(false);
    first.focus();
    const reverse = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    menu.dispatchEvent(reverse);
    expect(reverse.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
    expect(document.activeElement).not.toBe(disabledLast);
  });
});
