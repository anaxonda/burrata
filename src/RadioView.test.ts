// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
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
  it("lists verified stations, shows now-playing metadata, and reports artist info without matching identity", async () => {
    vi.useFakeTimers();
    mocks.call.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_radio_stations") return stations;
      if (command === "get_station_details") return { stationId: args?.stationId };
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
    const onPlayStation = vi.fn(), onArtist = vi.fn(), onNowPlaying = vi.fn(), onPlayTrack = vi.fn();
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation, onArtist, onNowPlaying, onPlayTrack, playingStationId: "main" } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(document.querySelectorAll(".station-card")).toHaveLength(4);
    expect(document.querySelector('[aria-label="Now playing on Main Mix"]')?.textContent).toContain("Live Song");
    expect(document.querySelector('[aria-label^="Recently heard"]')?.textContent).toContain("Earlier Song");
    expect(document.querySelector('[aria-label^="Recently heard"] .track-subtitle')?.textContent).toContain("Via Main Mix");
    expect(document.querySelector('[aria-label^="Recently heard"] time')?.textContent).toContain("2026");
    expect(document.querySelector('[aria-label^="Recently heard"] time')?.getAttribute("title")).toContain("2026");
    document.querySelector<HTMLButtonElement>('[aria-label="Play Earlier Song"]')!.click();
    expect(onPlayTrack).toHaveBeenCalledWith(expect.objectContaining({ title: "Earlier Song", artist: "Older Artist" }), expect.any(Array), 0);
    expect(document.querySelector('[aria-label^="Recently heard"]')?.textContent).toContain("your listening history");
    expect(document.querySelector("h1")?.textContent).toBe("Main Mix");
    expect(document.querySelector('img[alt="Album artwork for Live Song"]')?.getAttribute("src")).toBe("https://img.radioparadise.com/covers/l/current.jpg");
    document.querySelector<HTMLButtonElement>(".artist-link")!.click();
    expect(onArtist).toHaveBeenCalledWith(expect.objectContaining({
      provider: "radio", artist: "Radio Artist", title: "Live Song",
      provenance: expect.objectContaining({ identity: "station-reported; catalog identity unverified" }),
    }));
    document.querySelectorAll<HTMLButtonElement>('[aria-label="Radio stations"] .station-card')[1]!.click();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    expect(onPlayStation).toHaveBeenCalledWith(stations[1]);
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "main", recognize: true });
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
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation, onArtist, onStationsChanged } });
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
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), onStationsChanged } });
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
    expect(document.querySelector<HTMLButtonElement>(".station-actions .add-button")?.textContent).toContain("Play station");
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "rock", recognize: true });
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

  it("offers explicit stream recognition only for the playing station and accepts the result", async () => {
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
      throw new Error(`Unexpected command ${command}`);
    });
    const instance = mount(RadioView, { target: document.body, props: { onPlayStation: vi.fn(), onArtist: vi.fn(), playingStationId: "main", selectedStationId: "main", onNowPlaying } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();
    // Recognition is requested by active playback polling without a button click.
    expect(document.querySelector(".identify-button")).toBeNull();
    expect(mocks.call).toHaveBeenCalledWith("poll_radio_station", { stationId: "main", recognize: true });
    expect(document.querySelector(".track-copy h2")?.textContent).toBe("Recognized Song");
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
    expect(document.querySelector(".track-copy")?.textContent).toContain("Gotas De Lluvia");
    expect(document.querySelector(".now-playing .eyebrow")?.textContent).toContain("LAST IDENTIFIED");
    expect(document.querySelector(".history")?.textContent).toContain("Gotas De Lluvia");
    expect(document.querySelector(".history")?.textContent).not.toContain("Songs identified while you listen will appear here");
  });

});
