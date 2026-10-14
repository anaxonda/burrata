// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte's internal browser runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LibrarySidebar from "./LibrarySidebar.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(LibrarySidebar, { target, props: {
    view: "library", collections: [], browserImports: [], activeCollectionId: "", onNavigate: vi.fn(),
    onPlaylist: vi.fn(), onSource: vi.fn(), onImport: vi.fn(), onSettings: vi.fn(), onDownloads: vi.fn(), activeJobs: 0,
    ...props,
  } as never });
  destroy = () => unmount(instance);
  return target;
}

describe("LibrarySidebar provider presets", () => {
  it("shows permanent Shazam, Last.fm, and Bandcamp groups with the right browse actions", async () => {
    const onSource = vi.fn();
    const target = render({ onSource });
    await settle();
    for (const provider of ["beatport", "traxsource", "spotify", "shazam", "lastfm", "bandcamp", "local"]) {
      expect(target.querySelector(`.source-heading.${provider}`), `${provider} source group`).not.toBeNull();
    }
    for (const [provider, label] of [["shazam", "Shazam"], ["lastfm", "Last.fm"], ["bandcamp", "Bandcamp"]]) {
      const logo = target.querySelector<HTMLImageElement>(`.source-mark.${provider} img`);
      expect(logo, `${provider} provider artwork`).not.toBeNull();
      expect(logo?.getAttribute("src")).toBeTruthy();
      expect(logo?.parentElement?.getAttribute("title")).toBe(label);
    }
    for (const provider of ["shazam", "lastfm", "bandcamp"]) {
      const group = target.querySelector<HTMLDetailsElement>(`.source-heading.${provider}`)!.parentElement as HTMLDetailsElement;
      group.querySelector("summary")!.click();
      await settle();
      const action = group.querySelector<HTMLButtonElement>(".browse-source")!;
      expect(action.getAttribute("aria-label")).toBe(`Browse ${provider === "lastfm" ? "Last.fm" : provider[0]!.toUpperCase() + provider.slice(1)} ${provider === "bandcamp" ? "music" : "charts"}`);
      expect(action.textContent).toContain(provider === "bandcamp" ? "Browse music" : "Browse charts");
      action.click();
      expect(onSource).toHaveBeenLastCalledWith(provider);
    }
  });

  it("keeps new provider browse actions reachable in the narrow drawer and closes it after selection", async () => {
    const onSource = vi.fn();
    const target = render({ width: 160, onSource });
    await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Expand library menu"]')!.click();
    await settle();
    expect(target.querySelector(".library-sidebar")?.classList.contains("drawer-open")).toBe(true);
    const group = target.querySelector<HTMLDetailsElement>(".source-heading.shazam")!.parentElement as HTMLDetailsElement;
    group.querySelector("summary")!.click();
    group.querySelector<HTMLButtonElement>('[aria-label="Browse Shazam charts"]')!.click();
    await settle();
    expect(onSource).toHaveBeenCalledWith("shazam");
    expect(target.querySelector(".library-sidebar")?.classList.contains("drawer-open")).toBe(false);
  });
});
