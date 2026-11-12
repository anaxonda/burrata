// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({call: vi.fn()}));
vi.mock("./lib/api", () => ({call:mocks.call}));
// @ts-expect-error Match the browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AddRadioStationDialog from "./AddRadioStationDialog.svelte";
import type { RadioStation } from "./RadioView.svelte";
const stations: RadioStation[] = [{id:"main", name:"Main Mix", streamUrl:"https://stream.radioparadise.com/flac", sourceUrl:"https://radioparadise.com/listen/stream-links"}];
let cleanup: (() => void) | undefined;
beforeEach(() => {
  Object.defineProperty(HTMLDialogElement.prototype,"showModal",{configurable:true,value:function(this:HTMLDialogElement){this.setAttribute("open","");}});
  Object.defineProperty(HTMLDialogElement.prototype,"close",{configurable:true,value:function(this:HTMLDialogElement){this.removeAttribute("open");}});
});
afterEach(() => {cleanup?.();cleanup=undefined;vi.clearAllMocks();document.body.innerHTML="";});
describe("AddRadioStationDialog", () => {
  it("opens a separate dialog, searches a bounded catalog, and saves a result", async () => {
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
    const onClose = vi.fn();
    const instance = mount(AddRadioStationDialog, { target: document.body, props: { onStationsChanged, onClose } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
    await tick();

    expect(document.querySelector("dialog[open]")?.getAttribute("aria-labelledby")).toBe("add-radio-title");
    expect(document.querySelector(".radio-page")).toBeNull();
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
    expect(onClose).toHaveBeenCalledOnce();
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
    const instance = mount(AddRadioStationDialog, { target: document.body, props: { onStationsChanged, onClose: vi.fn() } });
    cleanup = () => { void unmount(instance); };
    for (let i = 0; i < 8; i++) await Promise.resolve();
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
    expect(document.querySelector(".radio-page")).toBeNull();
  });

  it("focuses the independent form and closes on Escape without loading a station page", async () => {
    const onClose=vi.fn();
    const instance=mount(AddRadioStationDialog,{target:document.body,props:{onClose,onStationsChanged:vi.fn()}});
    cleanup=()=>{void unmount(instance);};
    await tick();
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Search internet radio");
    const event=new Event("cancel",{cancelable:true});
    document.querySelector("dialog")!.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(onClose).toHaveBeenCalledOnce();
    expect(mocks.call).not.toHaveBeenCalled();
  });
});
