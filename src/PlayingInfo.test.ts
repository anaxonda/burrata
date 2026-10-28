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

  it("shows estimated BPM and key without changing or overriding saved track metadata", async () => {
    const empty = render({ ...radioTrack, bpm: undefined, key: undefined }, {
      analysisEstimate: { bpm: { bpm: 118 }, key: { key: "A minor" } },
      analysisStatus: "complete",
    }); await settle();
    const rows = [...empty.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(rows).toContain("Tempo118 BPM · estimated from audio");
    expect(rows).toContain("KeyA minor · estimated from audio");

    destroy?.(); destroy = undefined;
    const saved = render({ ...radioTrack, bpm: 94, key: "G minor" }, {
      analysisEstimate: { bpm: { bpm: 118 }, key: { key: "A minor" } },
      analysisStatus: "complete",
    }); await settle();
    const savedRows = [...saved.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(savedRows).toContain("Tempo94 BPM");
    expect(savedRows).toContain("KeyG minor");
    expect(savedRows.some(row => row.includes("118 BPM") || row.includes("A minor"))).toBe(false);
  });

  it("explains missing estimates while audio analysis is in progress or complete without one", async () => {
    const running = render({ ...radioTrack, bpm: undefined, key: undefined }, { analysisStatus: "running" }); await settle();
    const runningRows = [...running.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(runningRows).toContain("TempoAnalyzing this audio for BPM and key…");
    expect(runningRows).toContain("KeyAnalyzing this audio for BPM and key…");
    destroy?.(); destroy = undefined;
    const complete = render({ ...radioTrack, bpm: undefined, key: undefined }, { analysisStatus: "complete" }); await settle();
    expect(complete.querySelector("dl")?.textContent).toContain("No estimate was detected for this audio.");
  });

  it("shows a missing key explicitly and distinguishes failed analysis from running work", async () => {
    const missing = render({ ...radioTrack, key: undefined }); await settle();
    expect(missing.querySelector("dl")?.textContent).toContain("KeyNot analyzed");
    destroy?.(); destroy = undefined; document.body.innerHTML = "";
    const failed = render({ ...radioTrack, key: undefined }, { analysisStatus: "Full waveform analysis is unavailable for this stream." });
    await settle();
    expect(failed.querySelector("dl")?.textContent).toContain("KeyAudio analysis is unavailable for this source.");
    expect(failed.querySelector("dl")?.textContent).not.toContain("Analyzing this audio");
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
    expect(target.querySelector(".live-bpm")?.textContent).toContain("Live BPM64.2 / 128.4 BPM");
    expect(target.querySelector(".live-bpm")?.textContent).not.toContain("rhythm strength");
    expect(target.querySelector(".live-bpm")?.textContent).not.toContain("%");
    const rows = [...target.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(rows).toContain("Tempo96 BPM");
    expect(rows.find(row => row.includes("Live BPM"))).toContain("128.4 BPM");
    expect(rows.find(row => row.startsWith("Tempo"))).not.toContain("128.4 BPM");
  });

  it("shows a provisional live key separately from saved key metadata", async () => {
    const target = render({ ...radioTrack, key: "C", provenance: {} }, { liveKey: {
      generation: 5, seekSerial: 2, trackId: radioTrack.id, key: "Am", profileFit: .34,
      profileMargin: .08, windowSeconds: 10,
    } }); await settle();
    const rows = [...target.querySelectorAll("dl > div")].map(row => row.textContent ?? "");
    expect(rows).toContain("KeyC");
    expect(rows.find(row => row.includes("Live key"))).toContain("Am · recent window");
    expect(target.querySelector('[title*="does not change saved track metadata"]')).not.toBeNull();
  });

  it("shows source view and play counts, including zero, without fetching metadata", async () => {
    const candidate = {
      id: "candidate-1", provider: "youtube", url: "https://youtube.com/watch?v=one", title: "A video",
      formats: [], permission: "unknown", confidence: "high", score: 1, reasons: [], official: false,
      sourceMetadata: { remoteCatalog: { viewCount: 0, playCount: 1_250 } },
    };
    const target = render({ ...radioTrack, provider: "youtube", provenance: { remoteCatalog: { views: 92, plays: 0 } } }, { candidate }); await settle();
    expect(target.querySelector("dl")?.textContent).toContain("Views0");
    expect(target.querySelector("dl")?.textContent).toContain("Source plays1,250");
    expect(callMock).not.toHaveBeenCalled();
  });

  it("shows actual local play history, status, and only existing playlist memberships", async () => {
    const target = render({ ...radioTrack, localPath: "/stale/path/track.flac", localAvailability: "available", provenance: { remoteCatalog: { playCount: 12 } } }, {
      trackLibrarySummary: {
        trackId: radioTrack.id,
        playCount: 2,
        playedAt: ["2026-10-26T19:10:00Z", "2026-10-24T08:30:00Z"],
        playlists: [{ id: "playlist-1", name: "Evening set" }],
        inLibrary: false,
        downloaded: false,
      },
    });
    await settle();
    const activity = target.querySelector('[aria-label="Library and listening activity"]')!;
    expect(activity.textContent).not.toContain("Not in library");
    expect(activity.textContent).not.toContain("Not downloaded");
    expect(activity.textContent).toContain("Plays2");
    expect(activity.textContent).toContain("In playlists");
    expect(activity.textContent).toContain("Evening set");
    expect(target.querySelector('[aria-label="Downloaded and available on this device"]')).toBeNull();
    const disclosure = activity.querySelector<HTMLDetailsElement>("details.plays-disclosure")!;
    expect(disclosure.open).toBe(false);
    const summary = disclosure.querySelector<HTMLElement>("summary")!;
    expect(summary.textContent).toContain("Plays2");
    expect(summary.querySelector(".plays-chevron")?.getAttribute("aria-hidden")).toBe("true");
    summary.click();
    expect(disclosure.open).toBe(true);
    expect(disclosure.querySelector(".played-history-body")?.textContent).toContain("Successful starts recorded in playback history");
    expect(disclosure.querySelector(".played-dates-note")?.textContent).toContain("newest first");
    expect([...disclosure.querySelectorAll("time")].map(time => time.dateTime)).toEqual([
      "2026-10-26T19:10:00Z", "2026-10-24T08:30:00Z",
    ]);
    const sourcePlays = [...target.querySelectorAll("dl > div")].find(row => row.querySelector("dt")?.textContent === "Source plays");
    expect(sourcePlays?.textContent).toContain("12");
  });

  it("shows an actual zero play count without inventing dates or empty playlist membership", async () => {
    const target = render(radioTrack, {
      trackLibrarySummary: {
        trackId: radioTrack.id,
        playCount: 0,
        playedAt: [],
        playlists: [],
        inLibrary: true,
        downloaded: true,
      },
    });
    await settle();
    const activity = target.querySelector('[aria-label="Library and listening activity"]')!;
    expect(activity.textContent).toContain("LibraryIn library");
    expect(activity.textContent).toContain("DownloadedAvailable on this device");
    expect(activity.textContent).toContain("Plays0");
    const disclosure = activity.querySelector<HTMLDetailsElement>("details.plays-disclosure")!;
    expect(disclosure.open).toBe(false);
    expect(disclosure.querySelector("summary")?.textContent).toContain("Plays0");
    expect(disclosure.querySelector(".played-dates-note")?.textContent).toContain("No play dates are available");
    expect(activity.querySelector(".playlist-membership")).toBeNull();
  });

  it("discloses when the date list is limited to the most recent plays", async () => {
    const playedAt = Array.from({ length: 100 }, (_, index) => new Date(Date.UTC(2026, 9, 27, 12 - index)).toISOString());
    const target = render(radioTrack, {
      trackLibrarySummary: {
        trackId: radioTrack.id,
        playCount: 147,
        playedAt,
        playedAtHasMore: true,
        playlists: [],
        inLibrary: false,
        downloaded: false,
      },
    });
    await settle();
    const disclosure = target.querySelector<HTMLDetailsElement>("details.plays-disclosure")!;
    expect(disclosure.open).toBe(false);
    expect(disclosure.querySelector("summary")?.textContent).toContain("Plays147");
    expect(disclosure.querySelector(".played-dates-note")?.textContent).toContain("Showing the 100 most recent of 147 recorded plays, newest first.");
    expect(disclosure.querySelectorAll("time")).toHaveLength(100);
    expect(target.querySelector(".playlist-membership")).toBeNull();
  });

  it("does not show a summary for a different track or invent zero while details are loading", async () => {
    const target = render(radioTrack, {
      trackLibrarySummaryLoading: true,
      trackLibrarySummary: {
        trackId: "other-track",
        playCount: 9,
        playedAt: ["2026-10-26T19:10:00Z"],
        playlists: [{ id: "playlist-1", name: "Other set" }],
        inLibrary: true,
        downloaded: true,
      },
    });
    await settle();
    expect(target.textContent).toContain("Loading listening and library details…");
    expect(target.textContent).not.toContain("Plays");
    expect(target.textContent).not.toContain("Other set");
  });

  it("does not fall back to possibly stale downloaded metadata after the real status request fails", async () => {
    const target = render({ ...radioTrack, localPath: "/old/path/track.flac", localAvailability: "available" }, {
      trackLibrarySummaryRequested: true,
    });
    await settle();
    expect(target.querySelector('[aria-label="Library and listening activity"]')).toBeNull();
    expect(target.querySelector('[aria-label="Downloaded and available on this device"]')).toBeNull();
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
  it("hides loudness playback details when normalization is Off", async () => {
    const track = { ...radioTrack, provider: "local", localPath: "/music/song.flac" };
    const target = render(track, {
      loudnessStatus: { mode: "off", status: "off", appliedGainDb: 0 },
    });
    await settle();
    expect(target.querySelector('[aria-label="Loudness normalization playback details"]')).toBeNull();

    destroy?.(); destroy = undefined;
    const otherTrack = render(track, { loudnessMode: "off", loudnessStatus: null });
    await settle();
    expect(otherTrack.querySelector('[aria-label="Loudness normalization playback details"]')).toBeNull();
  });

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

  it("shows applied loudness source, analysis progress, cancel, and a local track action", async () => {
    const onAnalyzeLoudness = vi.fn();
    const onCancelLoudnessAnalysis = vi.fn();
    const target = render({ ...radioTrack, provider: "local", localPath: "/music/song.flac" }, {
      loudnessStatus: { mode: "track", status: "applied", appliedGainDb: -2.4, measurementSource: "ReplayGain track tag" },
      loudnessAnalysisJob: { jobId: "job-1", status: "running", total: 2, completed: 1, error: null },
      pendingLoudnessAnalysisCount: 2, onAnalyzeLoudness, onCancelLoudnessAnalysis,
    });
    await settle();
    const panel = target.querySelector('[aria-label="Loudness normalization playback details"]')!;
    expect(panel.textContent).toContain("Applied · -2.4 dB · ReplayGain track tag");
    expect(panel.textContent).toContain("Analyzing 1 of 2 selected tracks");
    expect(panel.querySelector("progress")?.value).toBe(1);
    panel.querySelector<HTMLButtonElement>("button")!.click();
    [...panel.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Cancel loudness analysis"))!.click();
    expect(onAnalyzeLoudness).toHaveBeenCalledTimes(1);
    expect(onCancelLoudnessAnalysis).toHaveBeenCalledTimes(1);
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
    ["disabled", "Disabled · enable"], ["unsupported", "no supported Web Audio PCM route"],
    ["paused", "Paused"], ["warming", "Listening for audio…"],
    ["insufficient", "No live BPM reading yet"], ["waiting", "Waiting for the current playback buffer"],
    ["unavailable", "temporarily unavailable"],
  ])("explains live BPM state %s without inventing an estimate", async (status, message) => {
    const target = render({...radioTrack, bpm:96}, {liveBpmStatus:status}); await settle();
    expect(target.querySelector(".live-bpm")?.textContent).toContain(message);
    expect(target.querySelector(".live-bpm")?.textContent).not.toContain("96 BPM");
  });


  it.each([
    ["paused", "Paused"],
    ["warming", "Listening for audio…"],
    ["estimating", "Estimating key…"],
    ["insufficient", "No confident key estimate yet"],
    ["waiting", "Waiting for enough audio…"],
    ["unavailable", "Live key estimate temporarily unavailable"],
    ["unsupported", "Live key is unavailable for this audio route"],
  ])("explains live key state %s when no confident key is available", async (status, message) => {
    const target = render({ ...radioTrack, key: undefined }, { liveBpmStatus: status }); await settle();
    const row = [...target.querySelectorAll("dl > div")].find(item => item.textContent?.includes("Live key"));
    expect(row?.textContent).toContain(message);
    expect(row?.textContent).not.toMatch(/\b(?:C|Am|Gm|F#m)\b/);
  });

  it("reports an unresolved live key when BPM exists without a key estimate", async () => {
    const target = render({ ...radioTrack, key: undefined }, {
      liveBpm: { generation: 5, seekSerial: 2, trackId: radioTrack.id, bpm: 128.4, confidence: .78, windowSeconds: 8 },
      liveBpmStatus: "estimating",
    }); await settle();
    const row = [...target.querySelectorAll("dl > div")].find(item => item.textContent?.includes("Live key"));
    expect(row?.textContent).toContain("No confident key estimate yet");
    expect(row?.textContent).not.toContain("Estimating BPM");
  });

  it("hides disabled live-key status and rejects a stale key from another track", async () => {
    const disabled = render({ ...radioTrack, key: undefined }, { liveBpmStatus: "disabled" }); await settle();
    expect([...disabled.querySelectorAll("dl > div")].some(item => item.textContent?.includes("Live key"))).toBe(false);
    destroy?.(); destroy = undefined; document.body.innerHTML = "";
    const otherTrackId = "local:other-track";
    const stale = render({ ...radioTrack, id: otherTrackId, key: undefined }, { liveKey: {
      generation: 5, seekSerial: 2, trackId: radioTrack.id, key: "Am", profileFit: .34, profileMargin: .08, windowSeconds: 10,
    }, liveBpmStatus: "estimating" }); await settle();
    expect([...stale.querySelectorAll("dl > div")].some(item => item.textContent?.includes("Live key"))).toBe(false);
  });

  it("shows the bounded live BPM observer failure reason", async () => {
    const reason = "AudioWorklet is unavailable and ScriptProcessor could not start.";
    const target = render({ ...radioTrack, bpm: 96 }, { liveBpmStatus: "unsupported", liveBpmStatusReason: reason });
    await settle();
    const row = target.querySelector(".live-bpm");
    expect(row?.textContent).toContain(reason);
    expect(row?.getAttribute("title")).toBe(reason);
  });

});


describe("PlayingInfo detected source points", () => {
  it("only offers seek points inside the currently playing excerpt", async () => {
    const onSeek = vi.fn();
    const track: Track = { ...radioTrack, provider: "youtube", provenance: {}, durationSeconds: 10,
      playbackRange: { parentTrackId: "mix-parent", chapterId: "excerpt", sourceUrl: "https://youtube.com/watch?v=fixture",
        startTime: 100, endTime: 110, parentTitle: "Mix", parentDurationSeconds: 400 } };
    const observation = (title: string, detectedAtSeconds: number) => ({ title, artist: "Artist", detectedAtSeconds,
      capturedStartSeconds: detectedAtSeconds - 1, capturedEndSeconds: detectedAtSeconds + 1, observedAt: "2026-10-27T00:00:00Z" });
    const target = render(track, { trackDetectionAvailable: true, onTrackDetectionSeek: onSeek,
      trackDetectionView: { resourceKey: "a".repeat(64), enabled: false,
        observations: [observation("Before excerpt", 6), observation("Inside excerpt", 106), observation("After excerpt", 306)] } });
    await settle();
    const points = target.querySelectorAll<HTMLButtonElement>(".detected-track");
    expect(points).toHaveLength(1);
    expect(points[0]?.textContent).toContain("Inside excerpt");
    points[0]!.click();
    expect(onSeek).toHaveBeenCalledWith(106);
  });
});
