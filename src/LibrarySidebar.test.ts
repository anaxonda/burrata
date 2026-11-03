// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte's internal browser runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LibrarySidebar from "./LibrarySidebar.svelte";
import LibrarySidebarRadioArtworkHarness from "./LibrarySidebarRadioArtworkHarness.svelte";

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
function renderWithStationUpdates(initialStations: Record<string, unknown>[], call: (command: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(LibrarySidebarRadioArtworkHarness, { target, props: { initialStations, call } as never }) as unknown as {
    updateStations: (stations: Record<string, unknown>[]) => void;
  };
  destroy = () => unmount(instance);
  return { target, updateStations: instance.updateStations };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
const recoveredImage = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/3S8AAAAASUVORK5CYII=";

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

  it("recovers all four Radio Paradise built-ins from one shared homepage request", async () => {
    const northImage = "data:image/png;base64,bm9ydGg=";
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "resolve_radio_station_artwork") return [];
      return args?.stationId === "the-north" ? northImage : recoveredImage;
    });
    const stations = [
      ...["main", "mellow", "rock", "global"].map(id => ({
        id, name: `Radio Paradise · ${id}`, streamUrl: `https://stream.radioparadise.com/${id}`,
        homepageUrl: "https://radioparadise.com/",
      })),
      { id: "the-north", name: "The North 103.3", streamUrl: "https://thenorth1033.streamguys1.com/live", homepageUrl: "https://www.thenorth1033.org/" },
    ];
    const target = render({ call, radioStations: stations });
    await settle();
    const artworkCalls = call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork");
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(artworkCalls).toEqual([
      ["resolve_radio_station_artwork", { stationId: "main" }],
      ["resolve_radio_station_artwork", { stationId: "the-north" }],
    ]);
    expect(buttons.slice(0, 4).map(button => button.querySelector("img")?.getAttribute("src"))).toEqual(Array(4).fill(recoveredImage));
    expect(buttons[4].querySelector("img")?.getAttribute("src")).toBe(northImage);
  });

  it("keeps explicit station favicons ahead of shared homepage artwork and falls back after an error", async () => {
    const call = vi.fn(async (command: string) => command === "resolve_radio_station_artwork" ? recoveredImage : []);
    const target = render({ call, radioStations: [
      { id: "missing", name: "Missing Logo FM", homepageUrl: "https://radio.example/" },
      { id: "explicit", name: "Explicit Logo FM", faviconUrl: "https://cdn.example/logo.png", homepageUrl: "https://radio.example/" },
    ] });
    await settle();
    const rows = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(rows[1].querySelector("img")?.getAttribute("src")).toBe("https://cdn.example/logo.png");
    rows[1].querySelector("img")!.dispatchEvent(new Event("error"));
    await settle();
    expect(rows[1].querySelector("img")?.getAttribute("src")).toBe(recoveredImage);
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(1);
  });

  it("isolates explicit-favicon recovery from missing-favicon stations on the same homepage", async () => {
    const homepageImage = "data:image/png;base64,aG9tZXBhZ2U=";
    const explicitImage = "data:image/png;base64,ZXhwbGljaXQ=";
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "resolve_radio_station_artwork") return [];
      return args?.stationId === "explicit" ? explicitImage : homepageImage;
    });
    const { target, updateStations } = renderWithStationUpdates([
      { id: "explicit", name: "Explicit Radio", faviconUrl: "https://cdn.example/logo.png", homepageUrl: "https://same.example/" },
    ], call);
    await settle();
    target.querySelector<HTMLButtonElement>(".radio-section .playlist-card img")!.dispatchEvent(new Event("error"));
    await settle();
    expect(target.querySelector(".radio-section .playlist-card img")?.getAttribute("src")).toBe(explicitImage);
    updateStations([
      { id: "explicit", name: "Explicit Radio", faviconUrl: "https://cdn.example/logo.png", homepageUrl: "https://same.example/" },
      { id: "missing", name: "Missing Radio", homepageUrl: "https://same.example/" },
    ]);
    await settle();
    const rows = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(2);
    expect(rows[0].querySelector("img")?.getAttribute("src")).toBe(explicitImage);
    expect(rows[1].querySelector("img")?.getAttribute("src")).toBe(homepageImage);
  });

  it("discards a stale homepage result and retries once through a still-current station", async () => {
    const stale = deferred<string | null>();
    const oldHomepageImage = "data:image/png;base64,b2xk";
    const currentHomepageImage = "data:image/png;base64,bmV3";
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "resolve_radio_station_artwork") return [];
      return args?.stationId === "main" && call.mock.calls.filter(([name]) => name === command).length === 1
        ? stale.promise
        : currentHomepageImage;
    });
    const { target, updateStations } = renderWithStationUpdates([
      { id: "main", name: "Main", streamUrl: "https://stream.example/old", homepageUrl: "https://old.example/" },
      { id: "mellow", name: "Mellow", streamUrl: "https://stream.example/mellow", homepageUrl: "https://old.example/" },
    ], call);
    await settle();
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(1);
    updateStations([
      { id: "main", name: "Main", streamUrl: "https://stream.example/new", homepageUrl: "https://new.example/" },
      { id: "mellow", name: "Mellow", streamUrl: "https://stream.example/mellow", homepageUrl: "https://old.example/" },
    ]);
    await settle();
    stale.resolve(oldHomepageImage);
    await settle();
    const rows = [...target.querySelectorAll<HTMLButtonElement>(".radio-section .playlist-card")];
    expect(rows[0].querySelector("img")?.getAttribute("src")).toBe(currentHomepageImage);
    expect(rows[1].querySelector("img")?.getAttribute("src")).toBe(currentHomepageImage);
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(3);
  });

  it("does not spin on a null result or metadata identity changes during the 600 second negative-cache window", async () => {
    const call = vi.fn(async (command: string) => command === "resolve_radio_station_artwork" ? null : []);
    const { target, updateStations } = renderWithStationUpdates([
      { id: "station", name: "Station", streamUrl: "https://stream.example/one", homepageUrl: "https://radio.example/" },
    ], call);
    await settle();
    updateStations([{ id: "station", name: "Station", streamUrl: "https://stream.example/two", homepageUrl: "https://radio.example/" }]);
    await settle();
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(1);
    expect(target.querySelector(".radio-station-placeholder")).not.toBeNull();
  });

  it("does not apply an in-flight artwork result after the sidebar unmounts", async () => {
    const pending = deferred<string | null>();
    const call = vi.fn(async (command: string) => command === "resolve_radio_station_artwork" ? pending.promise : []);
    const target = render({ call, radioStations: [{ id: "station", name: "Station", homepageUrl: "https://radio.example/" }] });
    await settle();
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(1);
    destroy?.(); destroy = undefined;
    pending.resolve(recoveredImage);
    await settle();
    expect(target.querySelector(".radio-station-icon")).toBeNull();
    expect(target.querySelector(".radio-station-placeholder")).toBeNull();
  });

  it("does not resolve unsafe or malformed homepage URLs", async () => {
    const call = vi.fn(async (command: string) => command === "resolve_radio_station_artwork" ? recoveredImage : []);
    const target = render({ call, radioStations: [
      { id: "active", name: "Active URL", homepageUrl: "javascript:alert(1)" },
      { id: "credentials", name: "Credential URL", homepageUrl: "https://user:secret@radio.example/" },
    ] });
    await settle();
    expect(call.mock.calls.filter(([command]) => command === "resolve_radio_station_artwork")).toHaveLength(0);
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
