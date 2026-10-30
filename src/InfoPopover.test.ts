// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import InfoPopover from "./InfoPopover.svelte";

afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });
it("keeps the panel inside a scrolling ancestor and updates on scroll", async () => {
  const container = document.createElement("div");
  container.style.overflow = "auto";
  container.style.overflowX = "auto"; container.style.overflowY = "auto";
  document.body.append(container);
  const rect = (left: number, top: number, width: number, height: number) => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} });
  vi.spyOn(container, "getBoundingClientRect").mockReturnValue(rect(64, 100, 250, 300));
  const instance = mount(InfoPopover, { target: container, props: { label: "Help", children: () => {} } });
  await tick();
  const anchor = container.querySelector("details")!;
  vi.spyOn(anchor, "getBoundingClientRect").mockReturnValue(rect(80, 120, 22, 22));
  anchor.open = true; anchor.dispatchEvent(new Event("toggle")); await tick();
  const panel = container.querySelector<HTMLElement>(".info-content")!;
  expect(panel.style.width).toBe("234px");
  expect(panel.style.transform).toBe("translateX(204px)");
  expect(panel.style.maxHeight).toBe("250px");
  vi.mocked(anchor.getBoundingClientRect).mockReturnValue(rect(260, 120, 22, 22));
  container.dispatchEvent(new Event("scroll")); await tick();
  expect(panel.style.transform).toBe("translateX(24px)");
  await unmount(instance);
});
