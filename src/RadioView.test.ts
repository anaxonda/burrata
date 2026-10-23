// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
beforeEach(() => {
  globalThis.ResizeObserver = class {observe() {} unobserve() {} disconnect() {}} as unknown as typeof ResizeObserver;
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {configurable: true, value: function(this: HTMLDialogElement) {this.setAttribute("open", "");}});
});

const mocks = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("./lib/api", () => ({ call: mocks.call }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RadioView from "./RadioView.svelte";
import type { RadioStation, RadioTrackMetadata } from "./RadioView.svelte";

const stations: RadioStation[] = [
  { id: "main", name: "Main Mix", streamUrl: "https://stream.radioparadise.com/flac", sourceUrl: "https://radioparadise.com/listen/stream-links" },
  { id: "mellow", name: "Mellow Mix", streamUrl: "https://stream.radioparadise.com/mellow-flac", sourceUrl: "https://radioparadise.com/listen/stream-links" },
  { id: "rock", name: "Rock Mix", streamUrl: "https://stream.radioparadise.com/rock-flac", sourceUrl: "https://radioparadise.com/listen/stream-links" },
  { id: "global", name: "Global Mix", streamUrl: "https://stream.radioparadise.com/eclectic-flac", sourceUrl: "https://radioparadise.com/listen/stream-links" },
];
let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; vi.useRealTimers(); vi.clearAllMocks(); document.body.innerHTML = ""; });

describe("RadioView", () => {
  it("replaces failed radio artwork and resets the station image when its source changes", async () => {
    const artworkStations: RadioStation[] = [
      { ...stations[0]!, faviconUrl: "https://images.example/invalid" },
      { ...stations[1]!, faviconUrl: "https://images.example/valid" },
    ];
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "search_playback_history") return { items: [], total: 0 };
      if (command === "get_station_details") return { stationId: args?.stationId };
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, {
      target: document.body,
      props: {
        onPlayStation: vi.fn(),
        onArtist: vi.fn(),
        savedStations: artworkStations,
        currentStationMetadata: {
          stationId: "main", artist: "Artist", title: "Live Song",
          artworkUrl: "https://images.example/bad-cover", sourceUrl: "https://radio.example",
          metadataConfidence: "station-reported",
        },
      },
    });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();

    const favicon = document.querySelector<HTMLImageElement>(".station-favicon")!;
    expect(favicon.getAttribute("src")).toBe("https://images.example/invalid");
    favicon.dispatchEvent(new Event("error"));
    await tick();
    expect(document.querySelector(".station-favicon")).toBeNull();
    expect(document.querySelector('[aria-label="Main Mix station artwork"]')?.textContent).toBe("♫");

    const cover = document.querySelector<HTMLImageElement>('.radio-track-card img[alt="Album artwork for Live Song"]')!;
    cover.dispatchEvent(new Event("error"));
    await tick();
    expect(document.querySelector('.radio-track-card img[alt="Album artwork for Live Song"]')).toBeNull();
    expect(document.querySelector('[aria-label="No artwork available for Live Song"]')?.textContent).toBe("♫");

    document.querySelectorAll<HTMLButtonElement>('[aria-label="Radio stations"] .station-card')[1]!.click();
    await tick();
    expect(document.querySelector<HTMLImageElement>(".station-favicon")?.getAttribute("src")).toBe("https://images.example/valid");
    expect(document.querySelector('[aria-label="Mellow Mix station artwork"]')).toBeNull();
  });

  it("recovers a failed station icon without delaying playback or changing its saved URL", async () => {
    const image = "data:image/png;base64,c2FuaXRpemVk";
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "resolve_radio_station_artwork") return image;
      if (command === "get_radio_stations") return [original];
      if (command === "search_playback_history") return { items: [], total: 0 };
      if (command === "get_station_details") return { stationId: args?.stationId };
      return null;
    });
    const original = { ...stations[0]!, faviconUrl: "https://old.example/gone.png" };
    const onPlayStation = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation, onArtist: vi.fn(), savedStations: [original] } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).not.toHaveBeenCalledWith("resolve_radio_station_artwork", expect.anything());
    document.querySelector<HTMLImageElement>(".station-favicon")!.dispatchEvent(new Event("error"));
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("resolve_radio_station_artwork", { stationId: "main" });
    expect(document.querySelector<HTMLImageElement>(".station-favicon")?.getAttribute("src")).toBe(image);
    expect(original.faviconUrl).toBe("https://old.example/gone.png");
    expect(onPlayStation).not.toHaveBeenCalled();
  });

  it("ignores stale artwork recovery after selecting a different station", async () => {
    let finish!: (value: string) => void;
    const pending = new Promise<string>((resolve) => { finish = resolve; });
    const artworkStations = [stations[0]!, { ...stations[1]!, faviconUrl: "https://images.example/mellow.png" }];
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "resolve_radio_station_artwork") return pending;
      if (command === "get_radio_stations") return artworkStations;
      if (command === "search_playback_history") return { items: [], total: 0 };
      if (command === "get_station_details") return { stationId: args?.stationId };
      return null;
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), savedStations: artworkStations } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("resolve_radio_station_artwork", { stationId: "main" });
    document.querySelectorAll<HTMLButtonElement>('[aria-label="Radio stations"] .station-card')[1]!.click();
    await tick();
    finish("data:image/png;base64,b2xkLXN0YXRpb24=");
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelector<HTMLImageElement>(".station-favicon")?.getAttribute("src")).toBe("https://images.example/mellow.png");
  });

  it("lists verified stations, shows now-playing metadata, and reports artist info without matching identity", async () => {
    vi.useFakeTimers();
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
      if (command === "get_radio_song_track") {
        const item = args?.metadata as RadioTrackMetadata;
        return { id: "radio_song:canonical", provider: "radio_song", providerId: "canonical", sourceUrl: "", artist: item.artist, title: item.title, album: item.album, durationSeconds: item.title === "Earlier Song" ? 180 : undefined, favorite: false, provenance: { sourceKind: "radioSong", identity: "unverified station metadata", radioStationId: item.stationId } };
      }
      if (command === "identify_radio_station") return {
        stationId: args?.stationId, nowPlaying: { stationId: args?.stationId, artist: "Radio Artist", title: "Live Song", album: "Current Album", year: "2025", artworkUrl: "https://img.radioparadise.com/covers/l/current.jpg", sourceUrl: stations[0]!.sourceUrl, metadataConfidence: "station-reported; catalog identity unverified" },
        secondsRemaining: 40, history: [{ stationId: args?.stationId, artist: "Older Artist", title: "Earlier Song", sourceUrl: stations[0]!.sourceUrl, metadataConfidence: "station-reported", observedAt: "2026-10-13T20:34:00.000Z" }], historyAvailable: true, note: "Station history is available.",
      };
      if (command === "poll_radio_station") return {
        stationId: args?.stationId, nowPlaying: {
          stationId: args?.stationId, artist: "Radio Artist", title: "Live Song", album: "Current Album", year: "2025",
          artworkUrl: "https://img.radioparadise.com/covers/l/current.jpg", sourceUrl: stations[0]!.sourceUrl,
          metadataConfidence: "station-reported; catalog identity unverified",
        }, secondsRemaining: 40, history: [{
          stationId: args?.stationId, artist: "Older Artist", title: "Earlier Song", sourceUrl: stations[0]!.sourceUrl,
          metadataConfidence: "station-reported", observedAt: "2026-10-13T20:34:00.000Z",
        }], historyAvailable: true, note: "Station history is available.",
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlayStation = vi.fn(), onArtist = vi.fn(), onNowPlaying = vi.fn(), onPlayTrack = vi.fn(), onFavoriteTrack = vi.fn(), onInfoTrack = vi.fn(), onOpenHistory = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation, onArtist, onNowPlaying, onPlayTrack, onFavoriteTrack, onInfoTrack, onOpenHistory, playingStationId: "main" } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelectorAll(".station-card")).toHaveLength(4);
    const radioScrollRegion = document.querySelector<HTMLElement>(".radio-page");
    expect(radioScrollRegion?.getAttribute("aria-label")).toBe("Radio");
    expect(document.querySelector(".history")?.closest(".radio-page")).toBe(radioScrollRegion);
    expect(document.querySelector('[aria-label="Now playing on Main Mix"]')?.textContent).toContain("Live Song");
    expect(document.querySelector<HTMLButtonElement>(".identify-button")?.textContent).toContain("Identify current song");
    expect(document.querySelector(".recognition-note, .confidence-details")).toBeNull();
    expect(document.querySelector(".radio-now-playing")?.textContent).not.toMatch(/SongRec|Shazam|Catalog identity|Station details/);
    expect(document.querySelector('[aria-label^="Recently heard"]')?.textContent).toContain("Earlier Song");
    expect(document.querySelector('[aria-label^="Recently heard"] .track-subtitle')?.textContent).toContain("Via Main Mix");
    expect(document.querySelector('[aria-label^="Recently heard"] time')?.textContent).toContain("2026");
    expect(document.querySelector('[aria-label^="Recently heard"] time')?.getAttribute("title")).toContain("2026");
    expect(document.querySelector('[aria-label^="Recently heard"] .track-title')?.textContent).toContain("Earlier Song");
    const minDuration = document.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    minDuration.value = "3:00"; minDuration.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(document.querySelectorAll('[aria-label="Radio stations"] .station-card')).toHaveLength(4);
    minDuration.value = "3:01"; minDuration.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(0);
    minDuration.value = ""; minDuration.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    const historySearch = document.querySelector<HTMLInputElement>('[aria-label="Search station history"]')!;
    historySearch.value = "no match"; historySearch.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    expect(document.querySelector(".history")?.textContent).toContain("No recent tracks match this filter.");
    historySearch.value = "older artist"; historySearch.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    expect(document.querySelector('[aria-label^="Recently heard"] .track-title')?.textContent).toContain("Earlier Song");
    document.querySelector<HTMLButtonElement>(".full-history")!.click();
    expect(onOpenHistory).toHaveBeenCalledWith("main");
    expect(document.querySelector('[aria-label="Add Earlier Song to favorites"]')).toBeTruthy();
    document.querySelector<HTMLButtonElement>('[aria-label="Add Earlier Song to favorites"]')!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(onFavoriteTrack).toHaveBeenCalledWith(expect.objectContaining({ id: "radio_song:canonical", sourceUrl: "" }));
    document.querySelector<HTMLButtonElement>(".identify-button")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(mocks.call).toHaveBeenCalledWith("identify_radio_station", { stationId: "main" });
    document.querySelector<HTMLButtonElement>('[aria-label="Play Earlier Song"]')!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(onPlayTrack).toHaveBeenCalledWith(expect.objectContaining({ id: "radio_song:canonical", title: "Earlier Song", artist: "Older Artist", sourceUrl: "" }), expect.any(Array), 0);
    document.querySelector<HTMLButtonElement>('[aria-label="Actions for Earlier Song"]')!.click(); await tick();
    [...document.querySelectorAll<HTMLButtonElement>(".track-menu button")].find(button => button.textContent === "Track information")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(onInfoTrack).toHaveBeenCalledWith(expect.objectContaining({ id: "radio_song:canonical", title: "Earlier Song", sourceUrl: "" }));
    expect(document.querySelector('[aria-label^="Recently heard"]')?.textContent).toContain("your listening history");
    expect(document.querySelector("h1")?.textContent).toBe("Main Mix");
    expect(document.querySelector('img[alt="Album artwork for Live Song"]')?.getAttribute("src")).toBe("https://img.radioparadise.com/covers/l/current.jpg");
    document.querySelector<HTMLButtonElement>(".artist-link")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(onArtist).toHaveBeenCalledWith(expect.objectContaining({
      id: "radio_song:canonical", provider: "radio_song", artist: "Radio Artist", title: "Live Song",
      provenance: expect.objectContaining({ identity: "unverified station metadata", radioStationId: "main" }),
    }));
    document.querySelectorAll<HTMLButtonElement>('[aria-label="Radio stations"] .station-card')[1]!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(onPlayStation).toHaveBeenCalledWith(stations[1]);
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "main", recognize: false });
    expect(onNowPlaying).toHaveBeenCalled();
    const metadataCallsBeforeUnmount = mocks.call.mock.calls.filter(([command]) => command === "poll_radio_station").length;
    cleanup?.(); cleanup = undefined;
    await vi.advanceTimersByTimeAsync(120_000);
    expect(mocks.call.mock.calls.filter(([command]) => command === "poll_radio_station")).toHaveLength(metadataCallsBeforeUnmount);
  });

  it("searches a bounded station catalog, saves a result, and removes a saved station", async () => {
    const searchResult: RadioStation = {
      id: "rb:station-1", name: "Jazz FM", streamUrl: "https://radio.example/jazz", sourceUrl: "https://radio-browser.info/",
      country: "Canada", tags: "jazz,instrumental", codec: "MP3", bitrate: 192,
    };
    const saved: RadioStation = { ...searchResult, isCustom: true };
    const onStationsChanged = vi.fn();
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
      if (command === "poll_radio_station") return { stationId: args?.stationId, nowPlaying: null, history: [], historyAvailable: false, note: "" };
      if (command === "search_radio_stations") return [searchResult];
      if (command === "add_radio_station") return [...stations, saved];
      if (command === "remove_radio_station") return stations;
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlayStation = vi.fn(), onArtist = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation, onArtist, onStationsChanged, showAdd: true } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();

    document.querySelector<HTMLButtonElement>(".add-button")!.click();
    await tick();
    document.querySelector<HTMLInputElement>('[aria-label="Search internet radio"]')!.value = "jazz";
    document.querySelector<HTMLInputElement>('[aria-label="Search internet radio"]')!.dispatchEvent(new Event("input", { bubbles: true }));
    const country = document.querySelector<HTMLInputElement>('[aria-label="Station country filter"]')!;
    country.value = "Canada"; country.dispatchEvent(new Event("input", { bubbles: true }));
    const genre = document.querySelector<HTMLInputElement>('[aria-label="Station genre filter"]')!;
    genre.value = "jazz"; genre.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".station-search")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();

    expect(mocks.call).toHaveBeenCalledWith("search_radio_stations", { query: "jazz", country: "Canada", tag: "jazz", limit: 20 });
    expect(document.querySelector('[aria-label="Station search results"]')?.textContent).toContain("Canada");
    expect(document.querySelector('[aria-label="Station search results"]')?.textContent).toContain("192 kbps");
    expect(document.querySelector('[aria-label="Station search results"]')?.textContent).toContain("instrumental");
    document.querySelector<HTMLButtonElement>('[aria-label="Station search results"] button')!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("add_radio_station", { station: searchResult });
    expect(onStationsChanged).toHaveBeenCalledWith([...stations, saved]);
    const savedStation = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Radio stations"] .station-card')].find((button) => button.textContent?.includes("Jazz FM"));
    expect(savedStation?.textContent).toContain("Jazz FM");
    expect(savedStation?.getAttribute("aria-pressed")).toBe("true");

    document.querySelector<HTMLButtonElement>('[aria-label="Remove Jazz FM"]')!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("remove_radio_station", { stationId: "rb:station-1" });
    expect(onStationsChanged).toHaveBeenLastCalledWith(stations);
  });

  it("adds a direct stream URL with its provided station name", async () => {
    const custom: RadioStation = { id: "custom:abc", name: "Night Drive", streamUrl: "https://stream.example/live", sourceUrl: "https://stream.example/live", isCustom: true };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
      if (command === "poll_radio_station") return { stationId: args?.stationId, nowPlaying: null, history: [], historyAvailable: false, note: "" };
      if (command === "add_custom_radio_station") return [...stations, custom];
      throw new Error(`Unexpected command ${command}`);
    });
    const onStationsChanged = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), onStationsChanged, showAdd: true } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    document.querySelector<HTMLButtonElement>(".add-button")!.click();
    await tick();
    document.querySelector<HTMLButtonElement>('[role="tab"]:nth-child(2)')!.click();
    await tick();
    const name = document.querySelector<HTMLInputElement>('[aria-label="Station name"]')!;
    const url = document.querySelector<HTMLInputElement>('[aria-label="Stream URL"]')!;
    name.value = "Night Drive"; name.dispatchEvent(new Event("input", { bubbles: true }));
    url.value = "https://stream.example/live"; url.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLFormElement>(".custom-station")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("add_custom_radio_station", { name: "Night Drive", url: "https://stream.example/live" });
    expect(onStationsChanged).toHaveBeenCalledWith([...stations, custom]);
    expect(document.querySelector('[aria-label="Radio stations"]')?.textContent).toContain("Night Drive");
  });

  it("hides the standalone station grid when the parent selects a sidebar station", async () => {
    mocks.call.mockImplementation(async (command: string) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: "rock" };
      if (command === "poll_radio_station") return { stationId: "rock", nowPlaying: null, history: [], historyAvailable: false, note: "" };
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), selectedStationId: "rock", playingStationId: "rock" } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelector('[aria-label="Radio stations"]')).toBeNull();
    expect(document.querySelector('[aria-label="Now playing on Rock Mix"]')).not.toBeNull();
    expect(document.querySelector(".radio-empty")?.textContent).toContain("Current track information is unavailable.");
    expect(document.querySelector(".radio-empty")?.classList.contains("empty")).toBe(false);
    expect(document.querySelector<HTMLButtonElement>(".station-actions .add-button")?.textContent).toContain("Play station");
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "rock", recognize: false });
  });

  it("keeps Add station in the sidebar flow and saves an alternate URL to the same station ID", async () => {
    const candidate: RadioStation = { id: "rb:alternate", name: "Main Mix", streamUrl: "https://alternate.example/main", sourceUrl: "https://directory.example", codec: "AAC", bitrateKbps: 256 };
    const updated = { ...stations[0]!, streamUrl: candidate.streamUrl };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId, streamUrl: "https://radio.example/stale-details" };
      if (command === "poll_radio_station") return { stationId: args?.stationId, nowPlaying: null, history: [], historyAvailable: false, note: "" };
      if (command === "search_radio_stations") return [stations[0], candidate];
      if (command === "edit_radio_station_url") return [updated, ...stations.slice(1)];
      throw new Error(`Unexpected command ${command}`);
    });
    const onStationsChanged = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), onStationsChanged, selectedStationId: "main" } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect([...document.querySelectorAll("button")].some(button => button.textContent?.includes("Add station"))).toBe(false);
    document.querySelector<HTMLButtonElement>(".station-actions button:nth-child(2)")!.click();
    await tick();
    const input = document.querySelector<HTMLInputElement>('[aria-label="Edit stream URL"]')!;
    expect(input.value).toBe(stations[0]!.streamUrl);
    document.querySelector<HTMLButtonElement>(".edit-station-url button[type=button]")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("search_radio_stations", { query: "Main Mix", country: null, tag: null, limit: 20 });
    expect(document.querySelector('[aria-label="Alternate streams"]')?.textContent).toContain("256 kbps");
    expect(document.querySelector('[aria-label="Alternate streams"]')?.textContent).toContain("AAC");
    expect(document.querySelector('[aria-label="Alternate streams"]')?.textContent).not.toContain(stations[0]!.streamUrl);
    expect(mocks.call).not.toHaveBeenCalledWith("edit_radio_station_url", expect.anything());
    document.querySelector<HTMLButtonElement>('[aria-label="Alternate streams"] button')!.click();
    await tick();
    expect(input.value).toBe(candidate.streamUrl);
    document.querySelector<HTMLFormElement>(".edit-station-url")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(mocks.call).toHaveBeenCalledWith("edit_radio_station_url", { stationId: "main", url: candidate.streamUrl });
    expect(onStationsChanged).toHaveBeenLastCalledWith([updated, ...stations.slice(1)]);
    expect(mocks.call).not.toHaveBeenCalledWith("add_radio_station", expect.anything());
  });

  it("cancels alternate-stream selection without changing the saved station", async () => {
    const candidate: RadioStation = { id: "rb:alternate", name: "Main Mix", streamUrl: "https://alternate.example/main", sourceUrl: "https://directory.example", codec: "AAC", bitrateKbps: 256 };
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
      if (command === "poll_radio_station") return { stationId: args?.stationId, nowPlaying: null, history: [], historyAvailable: false, note: "" };
      if (command === "search_radio_stations") return [candidate];
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), selectedStationId: "main" } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    document.querySelector<HTMLButtonElement>(".station-actions button:nth-child(2)")!.click();
    await tick();
    document.querySelector<HTMLButtonElement>(".edit-station-url button[type=button]")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    document.querySelector<HTMLButtonElement>('[aria-label="Alternate streams"] button')!.click();
    await tick();
    expect(document.querySelector<HTMLInputElement>('[aria-label="Edit stream URL"]')!.value).toBe(candidate.streamUrl);
    document.querySelector<HTMLButtonElement>('.edit-station-url button.remove-station')!.click();
    await tick();
    expect(document.querySelector('[aria-label="Edit stream URL"]')).toBeNull();
    expect(mocks.call).not.toHaveBeenCalledWith("edit_radio_station_url", expect.anything());
    expect(mocks.call).not.toHaveBeenCalledWith("add_radio_station", expect.anything());
  });

  it("uses the selected station’s own page identity and loads persistent history without polling while browsing", async () => {
    const station: RadioStation = {
      id: "rb:jazz", name: "Jazz One", streamUrl: "https://stream.example/jazz", sourceUrl: "https://stream.example/jazz",
      provider: "radio_browser", country: "Canada", tags: "jazz,swing", codec: "AAC", bitrateKbps: 128,
    };
    const history = [{ stationId: station.id, artist: "Earlier Artist", title: "Yesterday Song", sourceUrl: station.sourceUrl, metadataConfidence: "station-reported" }];
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return [station];
      if (command === "get_station_details") return {
        stationId: station.id, description: "A jazz station for late-night listening.", homepageUrl: "https://jazz.example/",
        faviconUrl: "https://jazz.example/favicon.png", country: "Canada", tags: "jazz,swing", codec: "AAC", bitrateKbps: 128,
        history, historyAvailable: true,
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), selectedStationId: station.id } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelector("h1")?.textContent).toBe("Jazz One");
    expect(document.querySelector("h1")?.textContent).not.toContain("Radio Paradise");
    expect(document.querySelector(".station-identity p:not(.eyebrow)")?.textContent).toContain("late-night listening");
    expect(document.querySelector<HTMLAnchorElement>('.heading-actions a')?.href).toBe("https://jazz.example/");
    expect(document.querySelector<HTMLImageElement>(".station-favicon")?.src).toBe("https://jazz.example/favicon.png");
    expect(document.querySelector(".station-details")?.textContent).toContain("128 kbps");
    expect(document.querySelector(".history")?.textContent).toContain("Yesterday Song");
    expect(document.querySelector(".history")?.textContent).toContain("your listening history");
    expect(mocks.call).toHaveBeenCalledWith("get_station_details", { stationId: station.id });
    expect(mocks.call.mock.calls.some(([command]) => command === "poll_radio_station")).toBe(false);
  });

  it("offers manual stream recognition for the playing station and accepts the result", async () => {
    const recognized: RadioTrackMetadata = {
      stationId: "main", artist: "Recognized Artist", title: "Recognized Song", sourceUrl: stations[0]!.sourceUrl,
      metadataConfidence: "SongRec fingerprint; catalog match unverified",
    };
    const onNowPlaying = vi.fn();
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: "main" };
      if (command === "poll_radio_station") return {
        stationId: "main", nowPlaying: args?.recognize ? recognized : null,
        history: args?.recognize ? [recognized] : [], historyAvailable: Boolean(args?.recognize), note: "",
      };
      if (command === "identify_radio_station") return { stationId: "main", nowPlaying: recognized, history: [recognized], historyAvailable: true, note: "" };
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main", selectedStationId: "main", onNowPlaying } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelector<HTMLButtonElement>(".identify-button")?.textContent).toContain("Identify current song");
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "main", recognize: false });
    document.querySelector<HTMLButtonElement>(".identify-button")!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(mocks.call).toHaveBeenCalledWith("identify_radio_station", { stationId: "main" });
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Recognized Song");
    expect(onNowPlaying).toHaveBeenCalledWith(stations[0], recognized);
  });
  it("keeps the latest identified song in history when a later poll has no current match", async()=>{
    vi.useFakeTimers();
    const known={stationId:"main",artist:"Grupo Niche",title:"Gotas De Lluvia",sourceUrl:stations[0]!.sourceUrl,metadataConfidence:"SongRec fingerprint"};
    let polls=0;
    mocks.call.mockImplementation(async(command:string)=>{
      if(command==="get_radio_stations")return stations;
      if(command==="get_station_details")return {stationId:"main",history:[known],historyAvailable:true};
      if(command==="poll_radio_station")return {stationId:"main",nowPlaying:++polls===1?known:null,history:[known],historyAvailable:true,note:""};
      throw new Error(command);
    });
    const instance=mount(RadioView,{target:document.body,props:{onPlayStation:vi.fn(),onArtist:vi.fn(),playingStationId:"main"}});
    cleanup=()=>{void unmount(instance);};
    for(let i=0;i<8;i++)await Promise.resolve();
    await tick();
    expect(document.querySelector(".history")?.textContent).toContain("Gotas De Lluvia");
    await vi.advanceTimersByTimeAsync(30000);await tick();
    expect(document.querySelector(".radio-track-copy")?.textContent).toContain("Gotas De Lluvia");
    expect(document.querySelector(".radio-now-playing .eyebrow")?.textContent).toContain("LAST IDENTIFIED");
    expect(document.querySelector(".history")?.textContent).toContain("Gotas De Lluvia");
    expect(document.querySelector(".history")?.textContent).not.toContain("Songs identified while you listen will appear here");
  });

  it("pages the full saved station history and ignores an old station response", async () => {
    let finishOld: ((value: unknown) => void) | undefined;
    const entry = (index: number, stationId = "main") => ({ playedAt: "2026-10-18T12:00:00Z", track: {
      id: `song:${stationId}:${index}`, provider: "radio_song", providerId: String(index),
      artist: "History Artist", title: `Saved song ${index}`, sourceUrl: "", favorite: false,
    } });
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
      if (command === "search_playback_history") {
        const options = args?.options as {stationId: string; offset: number};
        if (options.stationId === "mellow") return { items: [entry(999, "mellow")], total: 1 };
        if (options.offset === 100) return new Promise(resolve => { finishOld = resolve; });
        return { items: Array.from({length: 50}, (_, i) => entry(options.offset + i)), total: 125 };
      }
      throw new Error(command);
    });
    const instance = mount(RadioView, {target: document.body, props: {onPlayStation: vi.fn(), onArtist: vi.fn()}});
    cleanup = () => { void unmount(instance); };
    const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); await tick(); };
    await flush();
    expect(document.querySelectorAll(".shared-track-list li")).toHaveLength(50);
    expect(document.querySelector(".history-pagination")?.textContent).toContain("1–50 of 125 tracks");
    const next = () => [...document.querySelectorAll<HTMLButtonElement>(".history-pagination button")].find(button => button.textContent === "Next")!;
    next().click(); await flush();
    expect(document.querySelector(".history")?.textContent).toContain("Saved song 50");
    expect(document.querySelector(".history-pagination")?.textContent).toContain("51–100 of 125 tracks");
    next().click(); await flush();
    document.querySelectorAll<HTMLButtonElement>(".station-card")[1]!.click(); await flush();
    expect(document.querySelector(".history")?.textContent).toContain("Saved song 999");
    finishOld?.({items: [entry(100)], total: 125}); await flush();
    expect(document.querySelector(".history")?.textContent).toContain("Saved song 999");
    expect(document.querySelector(".history")?.textContent).not.toContain("Saved song 100");
    expect(document.querySelector(".history-pagination")).toBeNull();
  });

  it("delegates manual identification to the playing-audio owner and only polls metadata", async () => {
    const onIdentifyPlayingStation = vi.fn(async () => {});
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return {stationId: args?.stationId};
      if (command === "poll_radio_station") return {stationId: "main", nowPlaying: null, history: [], historyAvailable: false, note: ""};
      throw new Error(command);
    });
    const instance = mount(RadioView, {target: document.body, props: {onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main", onIdentifyPlayingStation}});
    cleanup = () => { void unmount(instance); };
    const flush = async () => {for (let i = 0; i < 12; i++) await Promise.resolve(); await tick();};
    await flush();
    const pollsBefore = mocks.call.mock.calls.filter(([command]) => command === "poll_radio_station").length;
    document.querySelector<HTMLButtonElement>(".identify-button")!.click(); await flush();
    expect(onIdentifyPlayingStation).toHaveBeenCalledTimes(1);
    expect(mocks.call.mock.calls.filter(([command]) => command === "poll_radio_station").length).toBeGreaterThan(pollsBefore);
    expect(mocks.call.mock.calls.some(([command]) => command === "identify_radio_station")).toBe(false);
    expect(mocks.call.mock.calls.filter(([command]) => command === "poll_radio_station").every(([,args]) => args.recognize === false)).toBe(true);
  });

  it.each([true, false])("keeps the newest identification above stale station details independently of history filters (current audio: %s)", async (hasCurrentAudio) => {
    const old = {stationId: "main", artist: "Cassius", title: "Old station song", sourceUrl: "", metadataConfidence: "station-reported", observedAt: "2026-10-18T12:00:00Z"};
    const heard = {...old, artist: "Ruthee", title: "Zoblazo", metadataConfidence: "SongRec fingerprint", observedAt: "2026-10-18T11:00:00Z"};
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return {stationId: "main", nowPlaying: old, history: [old], historyAvailable: true};
      if (command === "poll_radio_station") return {stationId: "main", nowPlaying: null, history: [old], historyAvailable: true, note: ""};
      if (command === "search_playback_history") {
        const options = args?.options as {limit: number; query?: string};
        return {items: options.limit === 1 ? [{track: {id: "heard", artist: heard.artist, title: heard.title, sourceUrl: "", provider: "radio_song", providerId: "heard", favorite: false, provenance: {radioMetadataConfidence: "SongRec fingerprint"}}, playedAt: heard.observedAt}] : [], total: options.limit === 1 ? 1 : 0};
      }
      throw new Error(command);
    });
    const instance = mount(RadioView, {target: document.body, props: {onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main", currentStationMetadata: hasCurrentAudio ? heard : null}});
    cleanup = () => {void unmount(instance);};
    const flush = async () => {for (let i = 0; i < 16; i++) await Promise.resolve(); await tick();};
    await flush();
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Zoblazo");
    if (!hasCurrentAudio) expect(document.querySelector(".radio-now-playing .eyebrow")?.textContent).toContain("LAST IDENTIFIED");
    const query = document.querySelector<HTMLInputElement>('[aria-label="Search station history"]')!;
    query.value = "Cassius"; query.dispatchEvent(new Event("input", {bubbles: true}));
    const order = document.querySelector<HTMLSelectElement>('[aria-label="Sort station history"]')!;
    order.value = "oldest"; order.dispatchEvent(new Event("change", {bubbles: true}));
    await flush();
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Zoblazo");
    expect(mocks.call).toHaveBeenCalledWith("search_playback_history", {options: {provider: "radio", stationId: "main", sort: "newest", sortField: "sourceOrder", direction: "desc", limit: 1, offset: 0}});
  });

  it("silently retries stale station lookups without erasing the newest identified song", async () => {
    vi.useFakeTimers();
    const known = {stationId: "main", artist: "Ruthee", title: "Zoblazo", sourceUrl: "", metadataConfidence: "SongRec fingerprint"};
    const next = {...known, title: "Next current song"};
    const onNowPlaying = vi.fn();
    let polls = 0;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return {stationId: args?.stationId};
      if (command === "poll_radio_station") {
        polls++;
        if (polls === 2) throw new Error("Playback changed during station metadata lookup");
        return {stationId: "main", nowPlaying: polls === 1 ? known : next, history: [known], historyAvailable: true, note: ""};
      }
      throw new Error(command);
    });
    const instance = mount(RadioView, {target: document.body, props: {onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main", onNowPlaying}});
    cleanup = () => {void unmount(instance);};
    for (let i = 0; i < 16; i++) await Promise.resolve(); await tick();
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Zoblazo");
    await vi.advanceTimersByTimeAsync(30_000); await tick();
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Zoblazo");
    expect(document.querySelector(".error")).toBeNull();
    expect(onNowPlaying.mock.calls.some(([, item]) => item === null)).toBe(false);
    await vi.advanceTimersByTimeAsync(30_000); await tick();
    expect(polls).toBe(3);
    expect(document.querySelector(".radio-track-copy h2")?.textContent).toBe("Next current song");
  });

  it("shows useful identification outcomes and real failures without technical station disclosures", async () => {
    let fail = false;
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return {stationId: "main", note: "SongRec identity is not catalog verified"};
      if (command === "poll_radio_station") return {stationId: "main", nowPlaying: null, history: [], historyAvailable: false, note: "Showing observed station metadata and SongRec; catalog identity unverified"};
      if (command === "identify_radio_station") {
        if (fail) throw new Error("Recognition service unavailable");
        return {stationId: "main", nowPlaying: null, history: [], historyAvailable: false, note: "No SongRec fingerprint match"};
      }
      throw new Error(command);
    });
    const instance = mount(RadioView, {target: document.body, props: {onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main"}});
    cleanup = () => {void unmount(instance);};
    const flush = async () => {for (let i = 0; i < 16; i++) await Promise.resolve(); await tick();};
    await flush();
    expect(document.querySelector(".radio-now-playing")?.textContent).not.toMatch(/SongRec|catalog|fingerprint/i);
    document.querySelector<HTMLButtonElement>(".identify-button")!.click(); await flush();
    expect(document.querySelector(".radio-now-playing [role=status]")?.textContent).toBe("No song identified. Try again during music.");
    fail = true; document.querySelector<HTMLButtonElement>(".identify-button")!.click(); await flush();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Recognition service unavailable");
  });

});

it("persists station playlist capture only after explicit opt-in and offers local targets", async () => {
  mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "get_radio_playlist_option") return {enabled: false, collectionId: null};
    if (command === "list_radio_playlist_targets") return [{id: "local-target", name: "My local songs"}];
    if (command === "set_radio_playlist_option") return {enabled: args?.enabled, collectionId: "local-target"};
    if (command === "search_playback_history") return {items: [], total: 0};
    if (command === "get_station_details") return {stationId: args?.stationId};
    throw new Error(`Unexpected command ${command}`);
  });
  const instance = mount(RadioView, {target: document.body, props: {savedStations: stations, selectedStationId: "main", onPlayStation: vi.fn(), onArtist: vi.fn()}});
  cleanup = () => unmount(instance);
  for (let i = 0; i < 8; i++) {await tick(); await Promise.resolve();}
  const checkbox = document.querySelector<HTMLInputElement>('.radio-playlist-capture input[type="checkbox"]')!;
  expect(checkbox.checked).toBe(false);
  expect(mocks.call.mock.calls.some(([command]) => command === "set_radio_playlist_option")).toBe(false);
  const target = document.querySelector<HTMLSelectElement>('select[aria-label="Radio capture playlist"]')!;
  expect(target.textContent).toContain("New playlist: Main Mix");
  target.value = "local-target"; target.dispatchEvent(new Event("change", {bubbles: true}));
  checkbox.checked = true; checkbox.dispatchEvent(new Event("change", {bubbles: true}));
  for (let i = 0; i < 8; i++) {await tick(); await Promise.resolve();}
  expect(mocks.call).toHaveBeenCalledWith("set_radio_playlist_option", {stationId: "main", enabled: true, collectionId: "local-target"});
  expect(checkbox.checked).toBe(true);
});

it("selects radio history for playlists and keeps canonical song downloads", async () => {
  const metadata = {stationId: "main", artist: "Fixture Artist", title: "Heard Song", sourceUrl: "", metadataConfidence: "station"};
  const canonical = {id: "radio_song:heard", provider: "radio_song", providerId: "heard", title: "Heard Song", artist: "Fixture Artist", sourceUrl: "", provenance: {sourceKind: "radioSong"}};
  mocks.call.mockImplementation(async (command: string) => {
    if (command === "get_radio_playlist_option") return {enabled: false, collectionId: null};
    if (command === "list_radio_playlist_targets") return [];
    if (command === "get_station_details") return {stationId: "main", history: [metadata], historyAvailable: true};
    if (command === "search_playback_history") return {items: [], total: 0};
    if (command === "get_radio_song_track") return canonical;
    throw new Error(`Unexpected command ${command}`);
  });
  const onAddSelectedToPlaylist = vi.fn(), onDownloadTrack = vi.fn();
  const instance = mount(RadioView, {target: document.body, props: {savedStations: stations, selectedStationId: "main", onPlayStation: vi.fn(), onArtist: vi.fn(), onAddSelectedToPlaylist, onDownloadTrack}});
  cleanup = () => unmount(instance);
  for (let i = 0; i < 12; i++) {await tick(); await Promise.resolve();}
  document.querySelector<HTMLButtonElement>('.history .start-selection')!.click(); await tick();
  [...document.querySelectorAll<HTMLButtonElement>('.history button')].find(button => button.textContent === "Select visible")!.click(); await tick();
  document.querySelector<HTMLButtonElement>('.history .add-selected')!.click();
  for (let i = 0; i < 8; i++) {await tick(); await Promise.resolve();}
  expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([expect.objectContaining({id: "radio_song:heard", sourceUrl: ""})]);
  document.querySelector<HTMLButtonElement>('[aria-label="Download Heard Song"]')!.click();
  for (let i = 0; i < 8; i++) {await tick(); await Promise.resolve();}
  expect(onDownloadTrack).toHaveBeenCalledWith(expect.objectContaining({id: "radio_song:heard", sourceUrl: ""}));
});
