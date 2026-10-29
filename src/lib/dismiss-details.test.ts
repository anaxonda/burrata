// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { dismissDetails } from "./dismiss-details";
let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); document.body.innerHTML = ""; });
it("closes a menu on Escape and restores its trigger without closing the parent view", () => {
  document.body.innerHTML = '<details open><summary>Options</summary><button>Choice</button></details>';
  const menu = document.querySelector("details")!;
  cleanup = dismissDetails(menu).destroy;
  const parentEscape = vi.fn();
  window.addEventListener("keydown", parentEscape);
  menu.querySelector("button")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  expect(menu.open).toBe(false);
  expect(document.activeElement).toBe(menu.querySelector("summary"));
  expect(parentEscape).not.toHaveBeenCalled();
  window.removeEventListener("keydown", parentEscape);
});
it("keeps inside interactions open, dismisses outside, and removes handlers on unmount", () => {
  document.body.innerHTML = '<details open><summary>Options</summary><button>Choice</button></details><button id="outside">Outside</button>';
  const menu = document.querySelector("details")!;
  const action = dismissDetails(menu);
  cleanup = action.destroy;
  menu.querySelector("button")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  expect(menu.open).toBe(true);
  document.querySelector("#outside")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  expect(menu.open).toBe(false);
  action.destroy();
  menu.open = true;
  document.querySelector("#outside")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  expect(menu.open).toBe(true);
});
