// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Candidate, LocalAsset } from "./lib/types";
import DownloadSourceChooser from "./DownloadSourceChooser.svelte";
import { groupDownloadCandidates } from "./lib/download-source-chooser";

const candidate = (id: string, provider: string, overrides: Partial<Candidate> = {}): Candidate => ({
  id, provider, url: `https://${provider}.example/${id}`, title: `${provider} ${id}`, formats: [], permission: "allowed",
  confidence: "medium", score: 0.7, reasons: [], official: false, ...overrides,
});
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

describe("download source grouping", () => {
  it("puts the preferred and configured providers first, then sorts candidates by match inside each group", () => {
    const candidates = [candidate("a", "youtube", { confidence: "high", score: .9 }), candidate("b", "slskd"), candidate("c", "soundcloud")];
    expect(groupDownloadCandidates(candidates, { preferredProvider: "slskd", providerOrder: ["youtube", "soundcloud"], sort: "match" }).map((group) => group.provider)).toEqual(["slskd", "youtube", "soundcloud"]);
  });

  it("uses explicit lossless format metadata and never compares bitrate across different codecs", () => {
    const flac = candidate("flac", "source", { formats: [{ id: "f", codec: "flac", extension: "flac", bitrate: 700, lossless: true }] });
    const opus = candidate("opus", "source", { formats: [{ id: "o", codec: "opus", extension: "webm", bitrate: 160, lossless: false }] });
    expect(groupDownloadCandidates([opus, flac], { sort: "quality" })[0].candidates.map((item) => item.id)).toEqual(["flac", "opus"]);
    expect(groupDownloadCandidates([opus, candidate("aac", "source", { formats: [{ id: "a", codec: "aac", extension: "m4a", bitrate: 256, lossless: false }] })], { sort: "quality" })[0].candidates.map((item) => item.id)).toEqual(["opus", "aac"]);
  });

  it("sorts known durations in either direction and keeps unknown lengths last", () => {
    const candidates = [
      candidate("short", "source", { durationSeconds: 60 }),
      candidate("unknown", "source"),
      candidate("long", "source", { durationSeconds: 180 }),
    ];
    expect(groupDownloadCandidates(candidates, { sort: "duration" })[0]!.candidates.map(item => item.id)).toEqual(["short", "long", "unknown"]);
    expect(groupDownloadCandidates(candidates, { sort: "duration", direction: "desc" })[0]!.candidates.map(item => item.id)).toEqual(["long", "short", "unknown"]);
  });
});

describe("DownloadSourceChooser", () => {
  it("keeps selection owned by the parent while the source list is loading", async () => {
    const first = candidate("first", "youtube");
    const onSelect = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(DownloadSourceChooser, { target, props: { candidates: [first], selected: first, loading: true, onSelect } });
    destroy = () => unmount(instance);
    await tick();
    const selected = target.querySelector<HTMLButtonElement>("button.candidate[aria-pressed='true']");
    expect(selected?.textContent).toContain("youtube first");
    expect(target.textContent).toContain("Searching for more sources");
    expect(target.querySelectorAll("button.candidate[aria-pressed='true']")).toHaveLength(1);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("shows peer size, availability, and clearly unverified lossless claims", async () => {
    const peer = candidate("peer", "slskd", {
      source: { kind: "soulseek", username: "music-peer", filename: "Tune.flac", size_bytes: 25_000_000 },
      formats: [{ id: "flac", codec: "FLAC", extension: "flac", lossless: true }],
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(DownloadSourceChooser, { target, props: { candidates: [peer], onSelect: vi.fn() } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.textContent).toContain("23.8 MB");
    expect(target.textContent).toContain("Peer source · unverified");
    expect(target.textContent?.replace(/\s+/g, " ")).toMatch(/Lossless listed\s*·\s*unverified/);
  });

  it("filters source rows by a duration bound", async () => {
    const candidates = [candidate("short", "source", { durationSeconds: 60 }), candidate("long", "source", { durationSeconds: 180 })];
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(DownloadSourceChooser, { target, props: { candidates, onSelect: vi.fn() } });
    destroy = () => unmount(instance);
    await tick();
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum source duration"]')!;
    minimum.value = "2:00"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await tick();
    expect(target.querySelectorAll("button.candidate")).toHaveLength(1);
    expect(target.querySelector("button.candidate")?.textContent).toContain("source long");
  });

  it("shows local file facts and marks the active file instead of offering it again", async () => {
    const asset: LocalAsset = { id: "asset-1", recordingId: "t1", path: "/music/song.flac", hash: "h", sourceCodec: "flac", finalCodec: "FLAC", finalBitrate: 900, processing: "complete", track: { id: "t1", provider: "library", providerId: "t1", sourceUrl: "", artist: "Artist", title: "Song", favorite: false, provenance: {} } };
    const onReplace = vi.fn(), onPreferred = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(DownloadSourceChooser, { target, props: { candidates: [], onSelect: vi.fn(), existingFiles: [asset], preferredAssetId: "asset-1", onReplace, onPreferred } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.textContent).toContain("song.flac");
    expect(target.textContent).toContain("FLAC · 900 kbps");
    expect(target.querySelector<HTMLButtonElement>(".file-actions button[aria-current='true']")?.textContent).toContain("Current");
    expect(target.querySelector<HTMLButtonElement>(".file-actions button:not(.replace)")?.disabled).toBe(true);
    target.querySelector<HTMLButtonElement>(".file-actions .replace")?.click();
    expect(onReplace).toHaveBeenCalledWith("asset-1");
    expect(onPreferred).not.toHaveBeenCalled();
  });
});
