// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
const apiCall = vi.hoisted(() => vi.fn());
vi.mock("./lib/api", () => ({ call: apiCall, desktop: () => true }));
import SearchPanel from "./SearchPanel.svelte";

let destroy: (() => void) | undefined;
const storage = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() } });
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; storage.clear(); apiCall.mockReset(); vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(SearchPanel, { target, props: { initialSources: ["library"], ...props } as never });
  destroy = () => unmount(instance);
  return target;
}

describe("SearchPanel", () => {
  it("searches the selected source progressively and renders compact track actions", async () => {
    const track = { id: "t1", provider: "library", providerId: "t1", sourceUrl: "local:t1", artist: "A Artist", title: "A Song", favorite: false, provenance: {} };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [{ id: "t1", kind: "track", sourceId: "library", title: "A Song", track }] }));
    const onPlay = vi.fn();
    const target = render({ searchSource, onPlay });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "A Song"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "A Song", sourceIds: ["library"], limit: 30 }));
    expect(target.textContent).toContain("A Song");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Download A Song"]')).not.toBeNull();
    target.querySelector<HTMLButtonElement>(".play-action")!.click();
    expect(onPlay).toHaveBeenCalledWith(track);
  });

  it("cancels a stale search response", async () => {
    let finish!: (value: { complete: boolean; results: { id: string; kind: string; sourceId: string; title: string }[] }) => void;
    const searchSource = vi.fn(() => new Promise<{ complete: boolean; results: { id: string; kind: string; sourceId: string; title: string }[] }>(resolve => { finish = resolve; }));
    const target = render({ searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>(".search-form input")!;
    input.value = "first"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await tick();
    target.querySelector<HTMLButtonElement>(".search-form button[type=button]")?.click();
    finish({ complete: true, results: [{ id: "stale", kind: "track", sourceId: "library", title: "Stale result" }] });
    await settle();
    expect(target.textContent).toContain("Cancelled");
    expect(target.textContent).not.toContain("Stale result");
    expect(target.textContent).not.toContain("No results from Library");
  });

  it("migrates the old source list to all online while excluding unsupported providers", async () => {
    storage.set("burrata.discovery.sources.v1", JSON.stringify(["library", "musicbrainz", "spotify", "beatport", "soundcloud"]));
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "spotify", name: "Spotify (imported)", kinds: ["track"] },
      { id: "beatport", name: "Beatport", kinds: ["track"] },
      { id: "soundcloud", name: "SoundCloud", kinds: ["track", "set"], enabled: false },
    ] });
    await settle();
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Online"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(target.querySelector<HTMLButtonElement>('[aria-label="My library"]')?.getAttribute("aria-pressed")).toBe("false");
    const select = target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    expect(select.value).toBe("all");
    expect([...select.options].map(option => option.value)).toEqual(["all", "musicbrainz", "discogs"]);
    expect(storage.get("burrata.discovery.search-scope.v2")).toBe(JSON.stringify({ mode: "online", sourceId: "all" }));
  });

  it("defaults to all enabled supported online sources", async () => {
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"], enabled: false },
    ] });
    await settle();
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Online"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("all");
    expect([...target.querySelectorAll<HTMLOptionElement>('[aria-label="Search in"] option')].map(option => option.value))
      .toEqual(["all", "musicbrainz", "discogs", "youtube"]);
  });

  it("keeps My library controls focused on local tracks", async () => {
    const target = render({ initialSources: ["library"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["artist", "album", "track"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
    ] });
    await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')).toBeNull();
    expect(target.textContent).toContain("Saved tracks, imports and downloads");
    expect([...target.querySelectorAll<HTMLButtonElement>(".filters button")].map(button => button.textContent?.trim()))
      .toEqual(["All", "Tracks"]);
    expect(target.querySelector<HTMLButtonElement>('[aria-label="My library"]')?.getAttribute("aria-pressed")).toBe("true");
  });

  it("shows only online sources that support the active result type", async () => {
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [] }));
    const target = render({ initialSources: undefined, availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
      { id: "soundcloud", name: "SoundCloud", kinds: ["track", "set"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"] },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "artist"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Artists")!.click();
    await settle();
    expect(searchSource.mock.calls.slice(-2).map(([request]) => request.sourceIds[0])).toEqual(["musicbrainz", "discogs"]);
    expect(target.querySelector('[aria-label="YouTube results"]')).toBeNull();
    expect(target.querySelector('[aria-label="SoundCloud results"]')).toBeNull();
    expect(target.querySelector('[aria-label="Soulseek results"]')).toBeNull();
    expect(target.querySelector('[aria-label="MusicBrainz results"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Discogs results"]')).not.toBeNull();
  });

  it("searches library, all online sources, and one chosen source immediately on scope changes", async () => {
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({
      sourceId: sourceIds[0], complete: true,
      results: [{ id: `${sourceIds[0]}-hit`, kind: "track", sourceId: sourceIds[0], title: `${sourceIds[0]} hit` }],
    }));
    const target = render({ initialSources: ["library"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "musicbrainz", name: "MusicBrainz", kinds: ["track", "artist", "album"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
      { id: "youtube", name: "YouTube", kinds: ["track", "video"] },
      { id: "spotify", name: "Spotify (imported)", kinds: ["track"] },
      { id: "soulseek", name: "Soulseek", kinds: ["track"], enabled: false },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "artist title"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["library"] }));

    target.querySelector<HTMLButtonElement>('[aria-label="Online"]')!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["musicbrainz"] }));
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["discogs"] }));
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["youtube"] }));
    const select = target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')!;
    expect([...select.options].map(option => option.value)).toEqual(["all", "musicbrainz", "discogs", "youtube"]);
    expect(target.textContent).not.toContain("Spotify (imported)");
    expect(target.textContent).not.toContain("Soulseek");

    select.value = "youtube"; select.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["youtube"] }));
    expect(storage.get("burrata.discovery.search-scope.v2")).toBe(JSON.stringify({ mode: "online", sourceId: "youtube" }));
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Artists")!.click();
    await settle();
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ sourceIds: ["youtube"] }));
    target.querySelector<HTMLButtonElement>('[aria-label="My library"]')!.click();
    await settle();
    expect(target.querySelector<HTMLButtonElement>('.filters button[aria-pressed="true"]')?.textContent).toBe("Tracks");
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ query: "artist title", sourceIds: ["library"], kinds: ["track"] }));
  });

  it("keeps a selected source through kind changes and explains unsupported kinds", async () => {
    const searchSource = vi.fn(async () => ({ complete: true, results: [] }));
    const target = render({ initialSources: ["discogs"], availableSources: [
      { id: "library", name: "My library", kinds: ["track"] },
      { id: "discogs", name: "Discogs", kinds: ["artist", "album"] },
    ], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "some artist"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(1);
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Tracks")!.click();
    await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Search in"]')?.value).toBe("discogs");
    expect(searchSource).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("Discogs does not support tracks");
    expect(target.querySelector('[aria-label="Discogs results"]')).toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>(".filters button")].find(button => button.textContent?.trim() === "Albums")!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({ sourceIds: ["discogs"], kinds: ["album"] }));
  });

  it("polls a partial Soulseek search through page completion and loads its next cursor", async () => {
    const firstBatch = Array.from({ length: 50 }, (_, index) => ({
      id: `peer-${index + 1}`, kind: "track", sourceId: "soulseek", title: `Peer Track ${index + 1}`,
    }));
    const searchSource = vi.fn()
      .mockResolvedValueOnce({ sourceId: "soulseek", complete: false, progress: "searching", nextCursor: "slsk-search-1", results: [] })
      .mockResolvedValueOnce({ sourceId: "soulseek", complete: true, nextCursor: null, results: [
        { id: "peer-51", kind: "track", sourceId: "soulseek", title: "Peer Track 51" },
      ] });
    apiCall.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "poll_search_source") {
        expect(args).toEqual({ searchId: "slsk-search-1", sourceId: "soulseek" });
        return { sourceId: "soulseek", complete: true, nextCursor: "slsk-search-1@50", results: firstBatch };
      }
      throw new Error(`Unexpected command: ${command}`);
    });
    const target = render({ initialSources: ["soulseek"], searchSource });
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search music"]')!;
    input.value = "peer track"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("searching");

    await new Promise((resolve) => setTimeout(resolve, 950));
    await settle();
    expect(apiCall).toHaveBeenCalledWith("poll_search_source", { searchId: "slsk-search-1", sourceId: "soulseek" });
    expect(target.textContent).toContain("Peer Track 1");
    expect(target.textContent).toContain("Peer Track 50");
    expect(target.querySelector<HTMLButtonElement>(".load-more")).not.toBeNull();

    target.querySelector<HTMLButtonElement>(".load-more")!.click();
    await settle();
    expect(searchSource).toHaveBeenCalledTimes(2);
    expect(searchSource).toHaveBeenLastCalledWith(expect.objectContaining({
      query: "peer track", sourceIds: ["soulseek"], cursor: "slsk-search-1@50", limit: 30,
    }));
    expect(target.textContent).toContain("Peer Track 51");
    expect(target.textContent).toContain("Peer Track 50");
  });

  it("renders cached chapters as a disclosure and sends chapter playback through the callback", async () => {
    const chapter = { id: "c1", title: "First track", startTime: 0, endTime: 90, confidence: "high" };
    const track = { id: "video1", provider: "youtube", providerId: "video1", sourceUrl: "https://youtube.com/watch?v=1", artist: "DJ", title: "Live set", favorite: false, provenance: { audioCandidate: { id: "candidate1", provider: "youtube", url: "https://youtube.com/watch?v=1", title: "Live set", formats: [], permission: "unknown", confidence: "high", score: 1, reasons: [], official: false, chapters: [chapter] } } };
    const searchSource = vi.fn(async ({ sourceIds }: { sourceIds: string[] }) => ({ sourceId: sourceIds[0], complete: true, results: [{ id: "video1", kind: "video", sourceId: "youtube", title: "Live set", url: track.sourceUrl, track }] }));
    const onPlayChapter = vi.fn();
    const target = render({ initialSources: ["youtube"], searchSource, onPlayChapter });
    await settle();
    const input = target.querySelector<HTMLInputElement>(".search-form input")!;
    input.value = "Live set"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const badge = target.querySelector<HTMLElement>(".chapter-badge.available")!;
    expect(badge.textContent).toContain("Chapters available · 1");
    badge.click();
    await settle();
    target.querySelector<HTMLButtonElement>(".chapter-row button")!.click();
    expect(onPlayChapter).toHaveBeenCalledWith(track, track.provenance.audioCandidate, chapter);
  });
});
