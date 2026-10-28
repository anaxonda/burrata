// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LocalAlbumLoudnessReview from "./LocalAlbumLoudnessReview.svelte";
import type { Track } from "./lib/types";

const mocks = vi.hoisted(() => ({ call: vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined) }));
vi.mock("./lib/api", () => ({ call: mocks.call }));

const tracks: Track[] = [
  { id: "recording-a", provider: "local", providerId: "a", sourceUrl: "local:a", artist: "Example", title: "First", album: "Set", albumArtist: "Example", assetId: "asset-a", trackNumber: null, favorite: false, provenance: {} },
  { id: "recording-b", provider: "local", providerId: "b", sourceUrl: "local:b", artist: "Example", title: "Second", album: "Set", albumArtist: "Example", assetId: "asset-b", trackNumber: null, favorite: false, provenance: {} },
];
const resource = {
  scope: "album" as const,
  members: tracks.map(track => ({ assetId: track.assetId!, fileHash: "a".repeat(64), range: { startSeconds: 0, endSeconds: null } })),
  interTrackGapsSeconds: [0], resourceKey: "b".repeat(64),
};
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let index = 0; index < 8; index++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(LocalAlbumLoudnessReview, { target, props: {
    tracks, album: "Set", albumArtist: "Example", canReorder: true,
    onAnalyze: vi.fn(async (ordered: Track[]) => ({ jobId: "job-1", playbackTracks: ordered })),
    onPlay: vi.fn(), onQueue: vi.fn(), onClose: vi.fn(), ...props,
  } as never });
  destroy = () => unmount(instance);
  return target;
}

describe("LocalAlbumLoudnessReview", () => {
  it("shows selected order, allows adjustments when track-number order is incomplete, and queues the analyzed exact resource", async () => {
    const reorderedResource = { ...resource, members: [...resource.members].reverse() };
    mocks.call.mockResolvedValue({ jobId: "job-1", status: "completed", total: 2, completed: 2, error: null, resource: reorderedResource });
    const onAnalyze = vi.fn(async (ordered: Track[]) => ({ jobId: "job-1", playbackTracks: ordered }));
    const onQueue = vi.fn();
    const target = render({ onAnalyze, onQueue }); await settle();
    expect(target.querySelectorAll(".programme-order li")).toHaveLength(2);
    expect(target.querySelector('[aria-label="Album analysis order"]')?.textContent).toContain("First");
    target.querySelector<HTMLButtonElement>('[aria-label="Move First down"]')!.click(); await settle();
    const orderText = target.querySelector('[aria-label="Album analysis order"]')?.textContent ?? "";
    expect(orderText.indexOf("Second")).toBeLessThan(orderText.indexOf("First"));
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Analyze album"))!.click();
    await settle();
    expect(onAnalyze.mock.calls[0]?.[0].map(track => track.id)).toEqual(["recording-b", "recording-a"]);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Queue analyzed order"))!.click();
    expect(onQueue).toHaveBeenCalledWith([tracks[1], tracks[0]], reorderedResource);
  });

  it("offers Play only after exact gapless resource completion and cancels a running job on close", async () => {
    mocks.call.mockResolvedValue({ jobId: "job-1", status: "running", total: 2, completed: 1, error: null });
    const onPlay = vi.fn(); const onClose = vi.fn();
    const summary = document.createElement("button"); summary.textContent = "Library options"; document.body.append(summary); summary.focus();
    const target = render({ onPlay, onClose, focusReturn: summary }); await settle();
    expect([...target.querySelectorAll("button")].some(button => button.textContent?.includes("Play analyzed order"))).toBe(false);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Analyze album"))!.click();
    await settle();
    expect(target.querySelector("progress")?.value).toBe(1);
    const cancelEvent = new Event("cancel", { cancelable: true });
    target.querySelector("dialog")!.dispatchEvent(cancelEvent);
    await settle();
    expect(cancelEvent.defaultPrevented).toBe(true);
    expect(mocks.call).toHaveBeenCalledWith("cancel_loudness_analysis", { jobId: "job-1" });
    expect(onClose).toHaveBeenCalledOnce();
    expect(onPlay).not.toHaveBeenCalled();
    destroy?.(); destroy = undefined;
    expect(document.activeElement).toBe(summary);
  });

  it("keeps a rejected cancellation running and retryable, then closes only after cancellation succeeds", async () => {
    let rejectNextCancel = true;
    mocks.call.mockImplementation(async (command: unknown, args: unknown) => {
      if (command === "get_loudness_analysis_status") return { jobId: "job-1", status: "running", total: 2, completed: 1, error: null };
      if (command === "cancel_loudness_analysis") {
        if (rejectNextCancel) { rejectNextCancel = false; throw new Error("worker is busy"); }
        return undefined;
      }
      return undefined;
    });
    const onClose = vi.fn();
    const target = render({ onClose }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Analyze album"))!.click();
    await settle();
    expect(target.querySelector("progress")?.value).toBe(1);

    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Close and cancel"))!.click();
    await settle();
    expect(onClose).not.toHaveBeenCalled();
    expect(target.querySelector("progress")?.value).toBe(1);
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("still running because cancellation failed");
    expect([...target.querySelectorAll("button")].some(button => button.textContent === "Cancel analysis")).toBe(true);

    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Cancel analysis")!.click();
    await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toBe("Album analysis canceled.");
    expect([...target.querySelectorAll("button")].some(button => button.textContent === "Cancel analysis")).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Close")!.click();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("closes immediately while preparation is pending and cancels the late native job", async () => {
    mocks.call.mockResolvedValue({ jobId: "job-1", status: "running", total: 2, completed: 0, error: null });
    let resolveAnalysis!: (value: { jobId: string; playbackTracks: Track[] }) => void;
    const onAnalyze = vi.fn(() => new Promise<{ jobId: string; playbackTracks: Track[] }>(resolve => { resolveAnalysis = resolve; }));
    const onClose = vi.fn();
    const canceledBefore = mocks.call.mock.calls.filter(([command]) => command === "cancel_loudness_analysis").length;
    const target = render({ onAnalyze, onClose }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Analyze album"))!.click();
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Close and cancel")!.click();
    await settle();
    expect(onClose).toHaveBeenCalledOnce();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Analysis request canceled");
    expect(mocks.call.mock.calls.filter(([command]) => command === "cancel_loudness_analysis")).toHaveLength(canceledBefore);

    resolveAnalysis({ jobId: "job-1", playbackTracks: tracks });
    await settle();
    expect(mocks.call).toHaveBeenCalledWith("cancel_loudness_analysis", { jobId: "job-1" });
    expect(mocks.call.mock.calls.filter(([command]) => command === "cancel_loudness_analysis")).toHaveLength(canceledBefore + 1);
  });
});
