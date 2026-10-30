// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte's internal browser runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import FallbackSettings from "./FallbackSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 5; i++) await Promise.resolve(); await tick(); }

describe("FallbackSettings", () => {
  it("offers Bandcamp in the ordered fallback list and persists its position and enabled state", async () => {
    const onchange = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(FallbackSettings, { target, props: {
      label: "Streaming",
      enabled: true,
      primary: "youtube",
      providers: ["soundcloud", "bandcamp"],
      sources: [
        { id: "youtube", name: "YouTube" },
        { id: "soundcloud", name: "SoundCloud" },
        { id: "bandcamp", name: "Bandcamp" },
        { id: "monochrome", name: "Monochrome" },
      ],
      onchange,
    } });
    destroy = () => unmount(instance);
    await settle();

    const chain = target.querySelector(".fallback-chain")?.textContent?.replace(/\s+/g, " ") ?? "";
    expect(chain).toMatch(/YouTube→SoundCloud→Bandcamp/);
    expect(target.textContent).toContain("searches may run at the same time");
    expect(target.querySelector("details")?.open).toBe(false);
    expect(target.querySelector('[aria-label="Streaming fallback 2"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Enable streaming fallback"]')).toBeNull();

    const second = target.querySelector<HTMLSelectElement>('[aria-label="Streaming fallback 2"]')!;
    second.value = "monochrome";
    second.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onchange).toHaveBeenLastCalledWith(true, ["soundcloud", "monochrome"]);

    const enabled = target.querySelector<HTMLInputElement>('[aria-label="Try another source if playback fails"]')!;
    enabled.checked = false;
    enabled.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onchange).toHaveBeenLastCalledWith(false, ["soundcloud", "bandcamp"]);
  });
});
