// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it } from "vitest";
import type { LocalAsset, Track } from "./lib/types";

import PlaybackQuality from "./PlaybackQuality.svelte";

const track: Track = { id: "t1", provider: "youtube", providerId: "v1", sourceUrl: "", artist: "Artist", title: "Song", favorite: false, provenance: {} };
const asset: LocalAsset = { id: "a1", recordingId: "r1", path: "/music/song.flac", hash: "h", sourceCodec: "opus", finalCodec: "flac", sourceBitrate: 128, finalBitrate: null, processing: "complete", track };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

function render(props: Record<string, unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlaybackQuality, { target, props });
  destroy = () => unmount(instance);
  return target;
}

describe("PlaybackQuality", () => {
  it("marks FLAC converted from Opus as lossy source without treating source bitrate as final bitrate", async () => {
    const target = render({ track, asset }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("FLAC · lossy source");
    expect(target.textContent).not.toContain("Lossless file");
    expect(target.querySelector(".quality-summary")?.textContent).not.toContain("128 kbps");
    expect(target.querySelector(".source-note")?.textContent?.replace(/\s+/g, " ")).toContain("Source: OPUS · 128 kbps");
  });

  it("labels lossless output as lossless when the source is also lossless", async () => {
    const losslessAsset = { ...asset, sourceCodec: "FLAC", finalCodec: "FLAC", finalBitrate: 900 };
    const target = render({ track, asset: losslessAsset }); await tick();
    expect(target.textContent).toContain("Lossless file");
    expect(target.textContent).toContain("900 kbps");
  });

  it("uses only actual live-stream facts and remains unknown when native facts are absent", async () => {
    const target = render({ track, asset, streamQuality: { codec: "AAC", bitrateKbps: 192, live: true } }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("AAC");
    expect(target.textContent).toContain("192 kbps");
    expect(target.textContent).toContain("Live stream");
    expect(target.textContent).not.toContain("FLAC");
  });

  it("does not fall back to download metadata when live-stream facts are absent", async () => {
    const unknown = render({ track, asset, streamQuality: { live: true } }); await tick();
    expect(unknown.querySelector(".quality-summary")?.textContent).toContain("Quality unknown");
    expect(unknown.textContent).not.toContain("OPUS");
  });

  it("shows resolved on-demand stream quality rather than downloaded asset quality", async () => {
    const target = render({ track, asset, streamQuality: { codec: "opus", bitrateKbps: 129.4, live: false } }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("OPUS");
    expect(target.textContent).toContain("129 kbps");
    expect(target.textContent).not.toContain("FLAC");
    expect(target.textContent).not.toContain("Live stream");
    expect(target.textContent).not.toContain("No downloaded file");
  });

  it("shows a known stream codec even when its bitrate is unavailable", async () => {
    const target = render({ track, streamQuality: { codec: "aac", live: false } }); await tick();
    expect(target.textContent).toContain("AAC");
    expect(target.textContent).not.toContain("kbps");
    expect(target.textContent).not.toContain("Quality unknown");
  });

  it("does not invent Opus bitrate, sample rate, or channel facts when the host omits them", async () => {
    const target = render({ track, streamQuality: { codec: "opus", live: false } }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("OPUS");
    expect(target.querySelector(".quality-summary")?.textContent).not.toContain("kbps");
    expect(target.querySelector(".quality-summary")?.textContent).not.toContain("kHz");
    expect(target.querySelector(".quality-summary")?.textContent).not.toContain("stereo");
  });

  it("marks reported average bitrates as approximate and only shows bit depth for lossless output", async () => {
    const approxStream = render({ track, streamQuality: { codec: "opus", bitrateKbps: 128, bitrateOrigin: "format_average", bitDepth: 16 } }); await tick();
    expect(approxStream.querySelector(".quality-summary")?.textContent).toContain("~128 kbps");
    expect(approxStream.querySelector(".quality-summary")?.textContent).not.toContain("16-bit");
    destroy?.(); destroy = undefined; document.body.innerHTML = "";
    const losslessAsset = { ...asset, sourceCodec: "FLAC", finalCodec: "FLAC", bitDepth: 24 };
    const target = render({ track, asset: losslessAsset }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("24-bit");
  });

  it("prefixes backend average file bitrate with an approximation marker", async () => {
    const target = render({ track, asset: { ...asset, sourceCodec: "FLAC", finalCodec: "FLAC", finalBitrate: 900, bitrateOrigin: "file_average" } }); await tick();
    expect(target.querySelector(".quality-summary")?.textContent).toContain("~900 kbps");
  });

  it("renders a compact bottom-player variant without a heading or box", async () => {
    const target = render({ track, streamQuality: { codec: "AAC", bitrateKbps: 192, live: true }, compact: true }); await tick();
    const quality = target.querySelector<HTMLElement>(".playback-quality")!;
    const summary = target.querySelector<HTMLElement>(".quality-summary")!;
    expect(quality.classList.contains("compact")).toBe(true);
    expect(quality.style.padding).toBe("0px");
    expect(quality.style.border).toBe("0px");
    expect(summary.style.fontSize).toBe("11px");
    expect(summary.style.fontWeight).toBe("400");
    expect(summary.style.gap).toBe("4px");
    expect(target.querySelector<HTMLElement>(".quality-icon")?.style.fontSize).toBe("11px");
    expect(target.querySelector<HTMLElement>(".quality-summary strong")?.style.fontWeight).toBe("400");
    expect(target.querySelector("h3")).toBeNull();
  });

  it("keeps compact lossless details in one readable resolution label", async () => {
    const target = render({ track, asset: { ...asset, sourceCodec: "FLAC", finalCodec: "FLAC", finalBitrate: 804, sampleRate: 44100, channels: 2, bitDepth: 16 }, compact: true }); await tick();
    const summary = target.querySelector<HTMLElement>(".quality-summary")!;
    expect(summary.querySelector("strong")?.textContent).toBe("FLAC · 16-bit / 44.1 kHz");
    expect(summary.title).toContain("804 kbps");
    expect(summary.title).toContain("stereo");
    expect(summary.querySelectorAll("span")).toHaveLength(1);
  });

  it("keeps quality unknown when no downloaded file details are available", async () => {
    const target = render({ track }); await tick();
    expect(target.textContent).toContain("Quality unknown");
    expect(target.textContent).not.toContain("lossless");
  });

  it("does not show a different track's asset as the playing file", async () => {
    const other = { ...track, id: "other" };
    const target = render({ track: other, asset }); await tick();
    expect(target.textContent).toContain("Quality unknown");
    expect(target.textContent).not.toContain("FLAC");
  });

  it("matches local assets by recording ID as well as track ID", async () => {
    const recordingAsset = { ...asset, recordingId: track.id, track: { ...track, id: "asset-track" } };
    const target = render({ track, asset: recordingAsset }); await tick();
    expect(target.textContent).toContain("FLAC · lossy source");
  });
});
