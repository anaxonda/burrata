// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
const { callMock } = vi.hoisted(() => ({ callMock: vi.fn() }));
vi.mock("./lib/api", () => ({ call: callMock, desktop: () => true }));
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlayingInfo from "./PlayingInfo.svelte";
import PlayingInfoArtworkTestHost from "./PlayingInfoArtworkTestHost.svelte";
import type { Track } from "./lib/types";

const radioTrack: Track = {
  id: "radio:station-1", provider: "internet-radio", providerId: "station-1", title: "Custom Jazz",
  artist: "Internet radio", sourceUrl: "https://station.example/about", favorite: false,
  provenance: { sourceKind: "radio", stationId: "station-1", radioStreamUrl: "https://stream.example/live.mp3", apiKey: "example-implementation-secret", rawResponse: { token: "hidden" } },
};
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; callMock.mockReset(); });

function render(track: Track, extra: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlayingInfo, { target, props: {
    track, onClose: vi.fn(), onArtist: vi.fn(), onDownload: vi.fn(), onPlaylist: vi.fn(), ...extra,
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

describe("PlayingInfo compact metadata", () => {
  it("shows source view and play counts, including zero, without fetching metadata", async () => {
    const candidate = {
      id: "candidate-1", provider: "youtube", url: "https://youtube.com/watch?v=one", title: "A video",
      formats: [], permission: "unknown", confidence: "high", score: 1, reasons: [], official: false,
      sourceMetadata: { remoteCatalog: { viewCount: 0, playCount: 1_250 } },
    };
    const target = render({ ...radioTrack, provider: "youtube", provenance: { remoteCatalog: { views: 92, plays: 0 } } }, { candidate }); await settle();
    expect(target.querySelector("dl")?.textContent).toContain("Views0");
    expect(target.querySelector("dl")?.textContent).toContain("Plays1,250");
    expect(callMock).not.toHaveBeenCalled();
  });

  it("shows a useful fallback when artist and album metadata are missing", async () => {
    const target = render({ ...radioTrack, title: "Untitled recording", artist: "", album: undefined });
    await settle();
    expect(target.querySelector(".unknown-artist")?.textContent).toBe("Artist unknown");
    expect(target.querySelector(".album")).toBeNull();
    expect(target.querySelector("h2")?.textContent).toContain("Untitled recording");
  });

  it("keeps one download action and removes the duplicate quality-upgrade action", async () => {
    const target = render(radioTrack); await settle();
    expect(target.querySelectorAll(".actions .download")).toHaveLength(1);
    expect(target.textContent).not.toContain("Find higher-quality download");
  });

  it("shows the available on-device files and disables selection for the current one", async () => {
    const asset = { id: "asset-1", recordingId: radioTrack.id, path: "/music/radio.flac", hash: "h", sourceCodec: "flac", finalCodec: "FLAC", finalBitrate: 900, processing: "complete", track: radioTrack };
    const onPreferredFile = vi.fn();
    const target = render(radioTrack, { asset, availableFiles: [asset], preferredAssetId: asset.id, onPreferredFile }); await settle();
    expect(target.textContent).toContain("On this device");
    expect(target.textContent).toContain("radio.flac");
    const current = target.querySelector<HTMLButtonElement>('.file-actions button[aria-current="true"]');
    expect(current?.textContent).toBe("Current");
    expect(current?.disabled).toBe(true);
    current?.click();
    expect(onPreferredFile).not.toHaveBeenCalled();
  });
});

describe("PlayingInfo Spotify recording verification", () => {
  it("exposes verification in the reachable source actions only for exact Spotify tracks", async () => {
    const spotifyId = "1234567890123456789012";
    const albumId = "abcdefghijklmnopqrstuv";
    callMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "prepare_spotify_recording_crosswalk") return { verified: true, spotifyTrackId: args?.trackId, musicbrainzReleaseId: "release-id", spotifyPosition: 4 };
      return true;
    });
    const onRetryPreview = vi.fn();
    const spotify = render({ ...radioTrack, id: "spotify-local-occurrence", provider: "spotify", providerId: spotifyId,
      sourceUrl: `https://open.spotify.com/track/${spotifyId}`, provenance: { recordingContext: { albumId } } }, { onRetryPreview });
    await settle();
    const verify = spotify.querySelector<HTMLButtonElement>(".identity-check button:not(.retry)");
    expect(verify?.textContent).toContain("Verify recording identity");
    verify!.click(); await settle();
    expect(callMock).toHaveBeenCalledWith("prepare_spotify_recording_crosswalk", {
      trackId: spotifyId, albumIdHint: albumId, requestId: expect.stringMatching(/^[0-9a-f-]{36}$/i),
    });
    spotify.querySelector<HTMLButtonElement>(".identity-check .retry")!.click();
    expect(onRetryPreview).toHaveBeenCalledTimes(1);

    const unmountSpotify = destroy; destroy = undefined; unmountSpotify?.();
    const youtube = render({ ...radioTrack, provider: "youtube", providerId: "youtube-video" }, { onRetryPreview }); await settle();
    expect(youtube.querySelector(".identity-check")).toBeNull();
  });
});

describe("PlayingInfo cached asset artwork recovery", () => {
  it("requests the asset fallback once, renders the display URL without changing track metadata, and ignores a stale image error", async () => {
    const track: Track = {
      ...radioTrack,
      id: "asset-artwork-track",
      title: "Cached Cover",
      artworkUrl: "asset://localhost/%2Fown%2Fartwork%2F" + "a".repeat(64) + ".jpg",
    };
    const originalTrack = JSON.stringify(track);
    const onArtworkError = vi.fn();
    const target = document.createElement("div");
    document.body.append(target);
    const instance = mount(PlayingInfoArtworkTestHost, {
      target,
      props: { initialTrack: track, onArtworkError },
    }) as unknown as {
      setArtworkDisplayUrl: (value: string) => void;
      currentTrack: () => Track;
    };
    destroy = () => unmount(instance);
    await settle();

    const failedImage = target.querySelector<HTMLImageElement>(".track-hero img")!;
    failedImage.dispatchEvent(new Event("error"));
    await settle();
    expect(onArtworkError).toHaveBeenCalledTimes(1);
    expect(onArtworkError).toHaveBeenCalledWith(track, track.artworkUrl);
    expect(target.querySelector(".cover-placeholder")).not.toBeNull();

    const displayUrl = "data:image/jpeg;base64,/9j/AA==";
    instance.setArtworkDisplayUrl(displayUrl);
    await settle();
    expect(target.querySelector(".cover-placeholder")).toBeNull();
    expect(target.querySelector<HTMLImageElement>(".track-hero img")?.getAttribute("src")).toBe(displayUrl);
    expect(JSON.stringify(instance.currentTrack())).toBe(originalTrack);
    expect(failedImage.isConnected).toBe(false);
    failedImage.dispatchEvent(new Event("error"));
    await settle();
    expect(onArtworkError).toHaveBeenCalledTimes(1);
  });
});
