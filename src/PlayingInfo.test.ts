// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
const { callMock } = vi.hoisted(() => ({ callMock: vi.fn() }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlayingInfo from "./PlayingInfo.svelte";
import type { Track } from "./lib/types";

const radioTrack: Track = {
  id: "radio:station-1", provider: "internet-radio", providerId: "station-1", title: "Custom Jazz",
  artist: "Internet radio", sourceUrl: "https://station.example/about", favorite: false,
  provenance: { sourceKind: "radio", stationId: "station-1", radioStreamUrl: "https://stream.example/live.mp3", apiKey: "example-implementation-secret", rawResponse: { token: "hidden" } },
};
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; callMock.mockReset(); });

function render(track: Track) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlayingInfo, { target, props: {
    track, onClose: vi.fn(), onArtist: vi.fn(), onDownload: vi.fn(), onPlaylist: vi.fn(),
  } as never });
  destroy = () => unmount(instance);
  return target;
}
async function settle() { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); }

describe("PlayingInfo internet-radio links", () => {
  it("labels the station website and actual stream URL separately and opens the stream safely", async () => {
    const target = render(radioTrack); await settle();
    expect(target.textContent).toContain("Station website");
    expect(target.textContent).toContain("https://station.example/about");
    expect(target.textContent).toContain("Stream URL");
    expect(target.textContent).toContain("https://stream.example/live.mp3");
    expect(target.textContent).not.toContain("example-implementation-secret");
    expect(target.textContent).not.toContain("rawResponse");
    target.querySelector<HTMLButtonElement>('[aria-label="Open stream URL"]')!.click();
    expect(callMock).toHaveBeenCalledWith("open_external_url", { url: "https://stream.example/live.mp3" });
  });

  it("does not render or open unsafe stream and station URLs", async () => {
    const track = { ...radioTrack, sourceUrl: "javascript:alert(1)", provenance: { sourceKind: "radio", radioStreamUrl: "file:///etc/passwd" } };
    const target = render(track); await settle();
    expect(target.querySelector(".radio-links")).toBeNull();
    expect(target.querySelector('[aria-label="Open stream URL"]')).toBeNull();
    expect(target.querySelector('[aria-label="Open station website"]')).toBeNull();
    expect(callMock).not.toHaveBeenCalled();
  });
});
