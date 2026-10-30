// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, unmount } from "../node_modules/svelte/src/index-client.js";
import { expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SettingsNavigation from "./SettingsNavigation.svelte";

it("labels troubleshooting clearly while preserving the advanced category", () => {
  const target = document.createElement("div");
  const onchange = vi.fn();
  const instance = mount(SettingsNavigation, { target, props: { active: "advanced", onchange } });
  const button = [...target.querySelectorAll("button")].find(item => item.textContent === "Troubleshooting")!;
  expect(button.getAttribute("aria-current")).toBe("page");
  button.click();
  expect(onchange).toHaveBeenCalledWith("advanced");
  unmount(instance);
});
