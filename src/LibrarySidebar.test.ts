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
  it("uses one folder header to open the browser and exposes the registered folder tree", async () => {
    const call = vi.fn(async (command: string) => command === "list_directory_browser_roots"
      ? [{ id: "folder-root", name: "Sample Music", path: "/sample" }]
      : { rootId: "folder-root", relativePath: "", directories: [] });
    const onSelectFolder = vi.fn();
    const onDirectoryBrowser = vi.fn();
    const target = render({ call, onSelectFolder, onDirectoryBrowser, selectedFolder: { rootId: "folder-root", relativePath: "" } });
    await settle();
    expect(target.querySelector(".folder-tree")?.getAttribute("aria-labelledby")).toBe("folder-tree-title");
    expect(target.querySelector('[role="treeitem"][aria-current="location"]')?.getAttribute("aria-label")).toBe("Sample Music");
    target.querySelector<HTMLButtonElement>('button[aria-label="Folders"]')!.click();
    expect(onDirectoryBrowser).toHaveBeenCalledTimes(1);
    expect(target.querySelector(".library-primary [aria-label='Folders']")).toBeNull();
    expect(target.querySelectorAll('button[aria-label="Folders"]')).toHaveLength(1);
  });

  it("uses one Playlists heading while keeping custom sections and source groups", async () => {
    const target = render({
      collections: [
        { id: "custom-1", name: "Road Mix", trackCount: 8 },
        { id: "spotify-1", name: "Imported Mix", trackCount: 3, sourceProvider: "spotify" },
      ],
      sections: [{ id: "custom", name: "Personal" }],
      assignments: { "custom-1": "custom" },
    });
    await settle();

    expect(target.querySelector(".playlist-heading")?.textContent).toContain("Playlists");
    expect(target.textContent).not.toContain("My playlists");
    expect(target.querySelector(".playlist-sections [aria-label='Create playlist section']")).not.toBeNull();
    expect(target.querySelector(".playlist-sections .section-name")?.textContent).toBe("Personal");
    expect(target.querySelector(".source-heading.spotify")).not.toBeNull();
    expect(target.textContent).toContain("Imported Mix");
  });

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

  it("exposes an accessible expandable library drawer in compact split-panel mode", async () => {
    const target = render({ compact: true, width: 240, collections: [{ id:"mix", name:"Night Drive", trackCount:4 }] });
    await settle();
    const sidebar = target.querySelector(".library-sidebar")!;
    const toggle = target.querySelector<HTMLButtonElement>('[aria-label="Expand library menu"]')!;
    expect(sidebar.classList.contains("compact")).toBe(true);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-controls")).toBe("library-sidebar-content");
    toggle.click();
    await settle();
    expect(target.querySelector<HTMLButtonElement>('[aria-label="Close library menu"]')?.getAttribute("aria-expanded")).toBe("true");
    expect(sidebar.classList.contains("drawer-open")).toBe(true);
    expect(target.querySelector("#library-sidebar-content")?.textContent).toContain("Night Drive");
  });

  it("filters playlist names throughout the left pane and scopes account rows to names or owners", async () => {
    const target = render({
      collections: [
        { id: "mix", name: "Night Drive", trackCount: 4 },
        { id: "spotify-import", name: "Imported Mix", trackCount: 2, sourceProvider: "spotify" },
      ],
      showAccountPlaylists: { spotify: true },
      call: vi.fn(async () => ({ items: [
        { id: "remote-one", title: "Sunrise Set", url: "https://open.spotify.com/playlist/a", ownerName: "DJ North" },
        { id: "remote-two", title: "Late Night", url: "https://open.spotify.com/playlist/b", ownerName: "Guest" },
      ], nextCursor: "spotify-next" })),
    });
    await settle();
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    filter.value = "north";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target.textContent).toContain("DJ North");
    expect(target.textContent).not.toContain("Night Drive");
    expect(target.textContent).not.toContain("Late Night");
    expect(target.textContent).not.toContain("Imported Mix");
    expect(target.querySelector(".account-playlists .load-more")?.textContent).toContain("Load more");
  });

  it("filters loaded collection owners while preserving custom and provider groups", async () => {
    const target = render({
      collections: [
        {id:"section-owned",name:"Section Mix",trackCount:2,ownerName:"DJ North",ownerId:"north-id"},
        {id:"source-owned",name:"Provider Mix",trackCount:3,sourceProvider:"spotify",ownerName:"DJ North",ownerId:"north-id"},
        {id:"other-owned",name:"Other Mix",trackCount:1,ownerName:"DJ South"},
      ],
      sections:[{id:"sets",name:"Sets"}], assignments:{"section-owned":"sets"},
    });
    await settle();
    const filter=target.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    for (const query of ["DJ NORTH", "north-id"]) {
      filter.value=query; filter.dispatchEvent(new Event("input",{bubbles:true})); await settle();
      expect(target.querySelector(".playlist-section")?.textContent).toContain("Section Mix");
      expect(target.querySelector(".source-heading.spotify")?.closest("details")?.textContent).toContain("Provider Mix");
      expect(target.textContent).not.toContain("Other Mix");
    }
  });

  it("filters assigned playlists and browser imports without showing stale empty messages", async () => {
    const target = render({
      collections: [
        { id: "section-match", name: "After Dark Local", trackCount: 4 },
        { id: "section-miss", name: "Morning Local", trackCount: 2 },
        { id: "spotify-import", name: "Stored Imported Mix", trackCount: 3, sourceProvider: "spotify" },
        { id: "youtube-import", name: "Morning YouTube Mix", trackCount: 2, sourceProvider: "youtube" },
      ],
      sections: [{ id: "live", name: "Live Sets" }, { id: "archive", name: "Archive" }],
      assignments: { "section-match": "live", "section-miss": "archive" },
      showAccountPlaylists: { spotify: true },
      call: vi.fn(async () => ({ items: [
        { id: "remote-unrelated", title: "Unrelated Account Mix", url: "https://open.spotify.com/playlist/unrelated", ownerName: "DJ Elsewhere" },
      ], nextCursor: "remote-next" })),
      browserImports: [
        { id: "import-match", provider: "spotify", title: "After Dark Playlist", collectionId: "spotify-import", sourceUrl: "https://open.spotify.com/playlist/after-dark" },
        { id: "import-miss", provider: "spotify", title: "Morning Playlist", collectionId: "spotify-other", sourceUrl: "https://open.spotify.com/playlist/morning" },
      ],
    });
    await settle();
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    filter.value = "after dark";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(target.querySelector(".source-heading.spotify .source-name")?.textContent).toBe("Spotify");
    expect(target.querySelector(".account-playlists")?.getAttribute("aria-label")).toBe("Spotify playlists");
    expect(target.querySelector(".account-playlists .scope-label")?.textContent).toBe("Loaded results");
    expect(target.textContent).toContain("After Dark Local");
    expect(target.textContent).toContain("After Dark Playlist");
    expect(target.textContent).not.toContain("Morning Local");
    expect(target.textContent).not.toContain("Morning Playlist");
    expect(target.querySelector(".playlist-section .empty-section")?.textContent).toBe("No matching playlists in this section.");
    expect(target.textContent).not.toContain("Drop a playlist here or create one.");
    expect(target.textContent).not.toContain("No playlists yet");
    expect(target.textContent).toContain("No imported playlists match this filter.");
    expect(target.querySelector('[aria-label="Create playlist section"]')).not.toBeNull();

    filter.value = "no loaded match";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target.querySelector(".account-playlists .hint")?.textContent).toContain("No loaded playlists match this filter.");
    expect(target.querySelector(".account-playlists .load-more")?.textContent).toContain("Load more");
  });
});
