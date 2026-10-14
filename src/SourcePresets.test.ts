// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte's internal browser runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SourcePresets from "./SourcePresets.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); }
function render(provider: string) {
  const target = document.createElement("div"); document.body.append(target);
  const onImport = vi.fn(async (_url: string) => {}), onOpen = vi.fn();
  const instance = mount(SourcePresets, { target, props: { provider, onImport, onOpen } });
  destroy = () => unmount(instance);
  return { target, onImport, onOpen };
}

describe("SourcePresets", () => {
  it("imports the default Shazam world chart and builds a selected region genre URL", async () => {
    const { target, onImport, onOpen } = render("shazam");
    await settle();
    target.querySelector<HTMLButtonElement>("button.primary")!.click();
    await settle();
    expect(onImport).toHaveBeenLastCalledWith("https://www.shazam.com/charts/top-200/world");
    const chart = target.querySelector<HTMLSelectElement>('[aria-label="Shazam chart"]')!;
    chart.value = "genre"; chart.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const region = target.querySelector<HTMLSelectElement>('[aria-label="Shazam region"]')!;
    region.value = "united-states"; region.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const genre = target.querySelector<HTMLSelectElement>('[aria-label="Shazam genre"]')!;
    genre.value = "house"; genre.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Open in browser"))!.click();
    expect(onOpen).toHaveBeenCalledWith("https://www.shazam.com/charts/genre/united-states/house");
  });

  it("builds Last.fm genre and profile URLs from the chosen genre, username, and period", async () => {
    const { target, onImport } = render("lastfm");
    await settle();
    const genre = target.querySelector<HTMLSelectElement>('[aria-label="Last.fm genre"]')!;
    genre.value = "jazz"; genre.dispatchEvent(new Event("change", { bubbles: true }));
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Import genre tracks"))!.click();
    await settle();
    expect(onImport).toHaveBeenLastCalledWith("https://www.last.fm/tag/jazz/tracks");
    target.querySelector<HTMLElement>("details summary")!.click();
    const username = target.querySelector<HTMLInputElement>('[aria-label="Last.fm username"]')!;
    username.value = "dj_example"; username.dispatchEvent(new Event("input", { bubbles: true }));
    username.dispatchEvent(new Event("change", { bubbles: true }));
    const period = target.querySelector<HTMLSelectElement>('[aria-label="Last.fm period"]')!;
    period.value = "3month"; period.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Import profile top tracks"))!.click();
    await settle();
    expect(onImport).toHaveBeenLastCalledWith("https://www.last.fm/user/dj_example/library/tracks?date_preset=LAST_90_DAYS");
  });

  it("opens genuine Bandcamp discovery destinations without issuing an import", async () => {
    const { target, onImport, onOpen } = render("bandcamp");
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Electronic"))!.click();
    expect(onOpen).toHaveBeenCalledWith("https://bandcamp.com/discover/electronic");
    expect(onImport).not.toHaveBeenCalled();
  });
});
