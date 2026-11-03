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
  it("loads playlist artwork in one batch and preserves playlist navigation", async () => {
    const onPlaylist=vi.fn();
    const call=vi.fn(async(command:string)=> command==='get_playlist_artwork' ? { one:[1,2,3,4].map(i=>`https://example.test/${i}.jpg`),two:['https://example.test/single.jpg'] } : []);
    const target=render({call,onPlaylist,collections:[{id:'one',name:'Collage playlist',trackCount:4},{id:'two',name:'Single cover playlist',trackCount:1}]});
    await settle();
    const requests=call.mock.calls.filter(([command])=>command==='get_playlist_artwork');
    expect(requests).toHaveLength(1);
    expect(call).toHaveBeenCalledWith('get_playlist_artwork',{collectionIds:['one','two']});
    const row=[...target.querySelectorAll<HTMLButtonElement>('.collection-card')].find(row=>row.textContent?.includes('Collage playlist'))!;
    expect(row.querySelectorAll('.collage img')).toHaveLength(4);
    row.click();expect(onPlaylist).toHaveBeenCalledWith('one');
  });

  it("shows saved station icons from either favicon field and keeps station actions", async () => {
    const onRadioStation = vi.fn();
    const onRadioContext = vi.fn();
    const stations = [
      { id: "modern", name: "Modern FM", faviconUrl: "https://example.com/modern.png", favicon: "https://example.com/old.png" },
      { id: "legacy", name: "Legacy FM", faviconUrl: "", favicon: "https://example.com/legacy.png" },
    ];
    const target = render({ view: "radio", activeRadioStationId: "modern", radioStations: stations, onRadioStation, onRadioContext });
    await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(buttons.map(button => button.querySelector("img")?.getAttribute("src"))).toEqual([
      "https://example.com/modern.png", "https://example.com/legacy.png",
    ]);
    expect(buttons[0].classList.contains("active")).toBe(true);
    expect(buttons[0].querySelector("img")?.alt).toBe("");
    buttons[0].click();
    expect(onRadioStation).toHaveBeenCalledWith(stations[0]);
    buttons[1].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    expect(onRadioContext).toHaveBeenCalledWith(stations[1], expect.any(MouseEvent));
  });

  it("keeps the playing radio highlighted outside Radio, independently of the selected station", async () => {
    const stations = [
      { id: "playing", name: "Playing Radio", country: "United States" },
      { id: "selected", name: "Selected Radio", country: "France" },
    ];
    const target = render({ view: "library", radioStations: stations, activeRadioStationId: "selected", playingRadioStationId: "playing" });
    await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(buttons[0].classList.contains("playing")).toBe(true);
    expect(buttons[0].classList.contains("active")).toBe(true);
    expect(buttons[0].getAttribute("aria-current")).toBe("true");
    expect(buttons[0].textContent).toContain("Playing · United States");
    expect(buttons[1].classList.contains("playing")).toBe(false);
    expect(buttons[1].classList.contains("active")).toBe(false);
    expect(buttons[1].hasAttribute("aria-current")).toBe(false);
  });

  it("falls back to a radio symbol for missing or broken station icons", async () => {
    const target = render({ radioStations: [
      { id: "missing", name: "No Logo FM" },
      { id: "broken", name: "Broken Logo FM", faviconUrl: "https://example.com/broken.png" },
    ] });
    await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(buttons[0].querySelector(".radio-station-placeholder")?.textContent).toBe("◉");
    buttons[1].querySelector("img")!.dispatchEvent(new Event("error"));
    await settle();
    expect(buttons[1].querySelector("img")).toBeNull();
    expect(buttons[1].querySelector(".radio-station-placeholder")?.textContent).toBe("◉");
    expect(buttons[1].textContent).toContain("Broken Logo FM");
  });

  it("recovers missing or failed station artwork in the sidebar", async () => {
    const image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/3S8AAAAASUVORK5CYII=";
    const call = vi.fn(async (command: string) => command === "resolve_radio_station_artwork" ? image : null);
    const stations = [
      { id: "no-favicon", name: "No Favicon Radio", homepageUrl: "https://radio.example/" },
      { id: "bad-favicon", name: "Bad Favicon Radio", faviconUrl: "https://radio.example/bad.png", homepageUrl: "https://radio.example/" },
    ];
    const target = render({ call, radioStations: stations });
    await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    const artworkCalls = () => call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork");
    expect(artworkCalls()).toHaveLength(1);
    expect(call).toHaveBeenCalledWith("resolve_radio_station_artwork", { stationId: "no-favicon" });
    expect(buttons[0].querySelector("img")?.getAttribute("src")).toBe(image);

    buttons[1].querySelector("img")!.dispatchEvent(new Event("error"));
    await settle();
    expect(artworkCalls()).toHaveLength(2);
    expect(artworkCalls()[artworkCalls().length - 1]).toEqual(["resolve_radio_station_artwork", { stationId: "bad-favicon" }]);
    expect(buttons[1].querySelector("img")?.getAttribute("src")).toBe(image);
  });

  it("filters radio and playlists together without unrelated sections burying matches", async () => {
    const target = render({ collections: [{id:"bg", name:"north", trackCount:13}], radioStations: [
      {id:"north", name:"South Radio", country:"Southland"}, {id:"bg-radio", name:"Harbor FM", country:"Northland"}
    ] });
    await settle();
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    filter.value = "north"; filter.dispatchEvent(new Event("input", {bubbles:true})); await settle();
    expect(target.textContent).toContain("north");
    expect(target.textContent).toContain("Harbor FM");
    expect(target.textContent).not.toContain("South Radio");
    filter.value = "south"; filter.dispatchEvent(new Event("input", {bubbles:true})); await settle();
    expect(target.textContent).toContain("South Radio");
    expect(target.textContent).not.toContain("Harbor FM");
    expect(target.querySelector(".built-in-playlists")?.textContent).not.toContain("north");
  });

  it("labels Spotify and YouTube navigation as Search playlists and keeps provider identity", async () => {
    const onSource = vi.fn();
    const target = render({ onSource }); await settle();
    for (const [id, name] of [["spotify", "Spotify"], ["youtube", "YouTube"]]) {
      const group = target.querySelector<HTMLDetailsElement>(`.source-heading.${id}`)!.parentElement as HTMLDetailsElement;
      const action = group.querySelector<HTMLButtonElement>(".browse-source")!;
      expect(action.getAttribute("aria-label")).toBe(`Search ${name} playlists`);
      expect(action.textContent).toContain("Search playlists");
      expect(action.textContent).not.toContain("Browse playlists");
      action.click();
      expect(onSource).toHaveBeenLastCalledWith(id);
    }
  });

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

  it("keeps Downloads in the playlist list and places icon-only Settings beside the brand", async () => {
    const onPlaylist = vi.fn();
    const onDownloads = vi.fn();
    const onSettings = vi.fn();
    const target = render({ view: "settings", collections: [{ id: "00000000-0000-4000-8000-000000000001", name: "Downloads", trackCount: 5 }], onPlaylist, onDownloads, onSettings, activeJobs: 2 });
    await settle();
    expect(target.querySelector(".library-footer")).toBeNull();
    const settings = target.querySelector<HTMLButtonElement>(".library-topbar button[aria-label='Settings']")!;
    expect(settings).not.toBeNull();
    expect(settings.title).toBe("Settings");
    expect(settings.classList.contains("active")).toBe(true);
    expect(settings.querySelector("span")?.getAttribute("aria-hidden")).toBe("true");
    expect(target.querySelector(".sidebar-scroll-end")).toBeNull();
    settings.click();
    expect(onSettings).toHaveBeenCalledTimes(1);
    const downloads = [...target.querySelectorAll<HTMLButtonElement>(".built-in-playlists .playlist-card")]
      .find((button) => button.textContent?.includes("Downloads"))!;
    expect(downloads).toBeDefined();
    expect(downloads.querySelector(".jobs-count")?.textContent).toBe("2");
    expect(downloads.querySelector(".jobs-count")?.getAttribute("aria-label")).toBe("2 active download jobs");
    downloads.click();
    expect(onPlaylist).toHaveBeenCalledWith("00000000-0000-4000-8000-000000000001");
    expect(onDownloads).not.toHaveBeenCalled();
  });

  it("keeps header Settings available in compact mode and closes the drawer after navigation", async () => {
    const onSettings = vi.fn();
    const target = render({ compact: true, onSettings });
    await settle();
    const sidebar = target.querySelector<HTMLElement>(".library-sidebar")!;
    const settings = target.querySelector<HTMLButtonElement>(".header-settings")!;
    expect(settings.getAttribute("aria-label")).toBe("Settings");
    expect(settings.title).toBe("Settings");
    target.querySelector<HTMLButtonElement>(".rail-toggle")!.click();
    await settle();
    expect(sidebar.classList.contains("drawer-open")).toBe(true);
    settings.click();
    await settle();
    expect(onSettings).toHaveBeenCalledTimes(1);
    expect(sidebar.classList.contains("drawer-open")).toBe(false);
  });

  it("highlights Downloads while viewing the full jobs page even when no collection is selected", async () => {
    const target = render({
      view: "downloads",
      activeCollectionId: "",
      activeJobs: 1,
      collections: [{ id: "00000000-0000-4000-8000-000000000001", name: "Downloads", trackCount: 3 }],
    });
    await settle();
    const downloads = [...target.querySelectorAll<HTMLButtonElement>(".built-in-playlists .playlist-card")]
      .find((button) => button.querySelector("strong")?.textContent === "Downloads");
    expect(downloads?.classList.contains("active")).toBe(true);
    expect(downloads?.querySelector(".jobs-count")?.textContent).toBe("1");
  });

  it("keeps History, the bound Favorites playlist, and Downloads above ordinary playlists without duplicates", async () => {
    const onPlaylist = vi.fn();
    const target = render({
      favoritesPlaylistId: "favorites-bound",
      playlistNames: { "favorites-bound": "My Liked Collection" },
      collections: [
        { id: "ordinary", name: "Road Mix", trackCount: 4 },
        { id: "favorites-bound", name: "Old favorites name", trackCount: 9, sourceProvider: "spotify" },
        { id: "00000000-0000-4000-8000-000000000001", name: "Downloads", trackCount: 2 },
      ],
      browserImports: [
        { id: "fav-import", provider: "spotify", title: "Remote Favorite Alias", collectionId: "favorites-bound", sourceUrl: "https://example.test/favorites" },
      ],
      sections: [{ id: "sets", name: "Sets" }],
      assignments: { "favorites-bound": "sets" },
      onPlaylist,
    });
    await settle();

    const cards = [...target.querySelectorAll<HTMLButtonElement>(".built-in-playlists .playlist-card")];
    const names = cards.map((button) => button.querySelector("strong")?.textContent?.trim());
    expect(names).toEqual(["History", "My Liked Collection", "Downloads", "Road Mix"]);
    expect([...target.querySelectorAll<HTMLButtonElement>(".playlist-card")]
      .filter((button) => button.textContent?.includes("My Liked Collection"))).toHaveLength(1);
    expect(target.querySelectorAll(".playlist-sections .collection-card")).toHaveLength(0);
    expect(target.querySelectorAll(".source-groups .imported-card")).toHaveLength(0);

    cards[1]!.click();
    expect(onPlaylist).toHaveBeenCalledWith("favorites-bound");
  });

  it("opens the sidebar filter from its dropdown directly below main navigation", async () => {
    const target = render({ collections: [{ id: "mix", name: "Night Drive", trackCount: 4 }] });
    await settle();
    const dropdown = target.querySelector<HTMLDetailsElement>(".sidebar-filter-dropdown")!;
    expect(dropdown.open).toBe(false);
    dropdown.querySelector("summary")!.click();
    await settle();
    expect(dropdown.open).toBe(true);
    const filter = dropdown.querySelector<HTMLInputElement>('[aria-label="Filter playlists by name or owner"]')!;
    filter.value = "night";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target.querySelector(".built-in-playlists")?.textContent).toContain("Night Drive");
    expect(target.querySelector(".sidebar-filter-dropdown > summary")?.textContent).toContain("Active");
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
    target.querySelector<HTMLElement>(".sidebar-filter-dropdown summary")!.click();
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
    target.querySelector<HTMLElement>(".sidebar-filter-dropdown summary")!.click();
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
    target.querySelector<HTMLElement>(".sidebar-filter-dropdown summary")!.click();
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
