// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Exercise the browser runtime in jsdom.
import { mount, unmount, tick } from "../node_modules/svelte/src/index-client.js";
// @ts-expect-error Svelte internal browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SoulseekDownloadPolicy from "./SoulseekDownloadPolicy.svelte";
let instance: ReturnType<typeof mount>;
afterEach(async () => { if (instance) await unmount(instance); document.body.innerHTML = ""; });
it("keeps waiting tied to source order while upgrade checking is independently selectable", async () => {
  const change = vi.fn();
  instance = mount(SoulseekDownloadPolicy, { target: document.body, props: { inSourceOrder:false, onchange:change } });
  const select = document.querySelector("select")!;
  expect(select.disabled).toBe(true);
  const checkbox = document.querySelector("input")!;
  checkbox.checked = true; checkbox.dispatchEvent(new Event("change", {bubbles:true})); await tick();
  expect(change).toHaveBeenCalledWith("continue", true);
});
it("explains source ordering and safe upgrades in keyboard-accessible disclosures", async () => {
  instance = mount(SoulseekDownloadPolicy, { target: document.body, props: { inSourceOrder:true, onchange:vi.fn() } });
  const details = [...document.querySelectorAll("details")];
  expect(details.every(item => !item.open)).toBe(true);
  details[0].open = true; await tick();
  expect(details[0].textContent).toContain("sources after Soulseek are held");
  window.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape"}));
  expect(details[0].open).toBe(false);
  details[1].open = true; document.body.click();
  expect(details[1].open).toBe(false);
});
