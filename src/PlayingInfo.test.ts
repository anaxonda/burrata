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
  it("keeps station links behind an accessible station-name disclosure", async () => {
    const target = render(radioTrack); await settle();
    const details = target.querySelector(".station-info")!;
    expect(details.querySelector("summary")?.getAttribute("aria-label")).toBe("Station information for Custom Jazz");
    expect((details as HTMLDetailsElement).open).toBe(false);
    details.querySelector("summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(details.textContent).toContain("Station website");
    expect(details.querySelector('a[href="https://station.example/about"]')?.getAttribute("rel")).toBe("noopener noreferrer");
    expect(details.textContent).toContain("Stream URL");
    expect(details.querySelector('a[href="https://stream.example/live.mp3"]')).not.toBeNull();
    expect(target.textContent).not.toContain("example-implementation-secret");
    expect(target.textContent).not.toContain("rawResponse");
    expect(callMock).not.toHaveBeenCalled();
  });

  it("does not render or open unsafe stream and station URLs", async () => {
    const track = { ...radioTrack, sourceUrl: "javascript:alert(1)", provenance: { sourceKind: "radio", radioStreamUrl: "file:///etc/passwd" } };
    const target = render(track); await settle();
    expect(target.querySelector(".station-info")).toBeNull();
    expect(callMock).not.toHaveBeenCalled();
  });
});

describe("PlayingInfo compact metadata", () => {
  it("separates catalog provenance from the selected audio provider", async () => {
    const target = render({ ...radioTrack, provider: "catalog", provenance: { catalogProvider: "musicbrainz" } }, {
      candidate: { id: "yt-one", provider: "youtube", title: "Selected upload", url: "https://youtube.com/watch?v=one", formats: [], reasons: [] },
    }); await settle();
    const details = target.querySelector("dl")?.textContent;
    expect(details).toContain("Metadata sourceMusicBrainz");
    expect(details).toContain("Audio providerYouTube");
    expect(details).not.toContain("Catalogcatalog");
  });

  it("does not invent provenance for a generic catalog entry", async () => {
    const target = render({ ...radioTrack, provider: "catalog", provenance: {} }); await settle();
    expect(target.querySelector("dl")?.textContent).not.toContain("Metadata source");
    expect(target.querySelector("dl")?.textContent).not.toContain("Catalogcatalog");
  });

  it("shows a tempo alternative only for the current non-manual analysis", async () => {
    const provenance = { bpmAnalysis: { bpm: 60.2, relatedTempoAlternativeBpm: 120 } };
    const target = render({...radioTrack, bpm: 60.2, provenance}); await settle();
    expect(target.textContent).toContain("60.2 BPM · estimated · alternative 120 BPM");
  });

  it("does not label a manual value using stale analysis", async () => {
    const target = render({...radioTrack, bpm: 124, key: "Gm", provenance: {
      manualFields: {bpm: true}, bpmAnalysis: {bpm: 124, relatedTempoAlternativeBpm: 62}, keyAnalysis: {key: "Am"},
    }}); await settle();
    expect(target.textContent).not.toContain("estimated");
    expect(target.textContent).not.toContain("alternative");
  });

  it("shows approximate live BPM separately from the catalog tempo", async () => {
    const target = render({ ...radioTrack, bpm: 96, provenance: {} }, { liveBpm: {
      generation: 5, seekSerial: 2, trackId: radioTrack.id, bpm: 128.4, confidence: 0.78,
      relatedTempoAlternativeBpm: 64.2, windowSeconds: 8,
    } }); await settle();
    expect(target.querySelector(".live-bpm")?.textContent).toContain("Live BPM128.4 BPM");
    expect(target.querySelector(".live-bpm")?.textContent).toContain("approx. · 78% rhythm strength");
    expect(target.querySelector(".live-bpm")?.textContent).toContain("half/double-time readings can vary");
    const rows = [...target.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(rows).toContain("Tempo96 BPM");
    expect(rows.find(row => row.includes("Live BPM"))).toContain("128.4 BPM");
    expect(rows.find(row => row.startsWith("Tempo"))).not.toContain("128.4 BPM");
  });

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

describe("PlayingInfo automatic recording verification", () => {
  it("keeps internal matching methods out of track details", async () => {
    const spotifyId = "1234567890123456789012";
    const target = render({ ...radioTrack, provider:"spotify", providerId:spotifyId,
      sourceUrl:`https://open.spotify.com/track/${spotifyId}`,
      provenance:{recordingContext:{albumId:"abcdefghijklmnopqrstuv"}} });
    await settle();
    expect(target.querySelector(".identity-check")).toBeNull();
    expect(target.querySelector('input[type="radio"]')).toBeNull();
    expect(target.textContent).not.toMatch(/Verification method|Verify recording|MusicBrainz recording URL|UUID/);
    expect(callMock).not.toHaveBeenCalledWith("prepare_spotify_recording_crosswalk",expect.anything());
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
  it.each([
    ["disabled", "Disabled · enable"], ["unsupported", "Requires Native audio"],
    ["paused", "Paused"], ["warming", "collecting a ten-second"],
    ["insufficient", "No confident rhythm"], ["unavailable", "temporarily unavailable"],
  ])("explains live BPM state %s without inventing an estimate", async (status, message) => {
    const target = render({...radioTrack, bpm:96}, {liveBpmStatus:status}); await settle();
    expect(target.querySelector(".live-bpm")?.textContent).toContain(message);
    expect(target.querySelector(".live-bpm")?.textContent).not.toContain("96 BPM");
  });

});
