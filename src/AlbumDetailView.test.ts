// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AlbumDetailView from "./AlbumDetailView.svelte";
import AlbumDetailViewHarness from "./AlbumDetailViewHarness.svelte";
import { call } from "./lib/api";
import type { Track } from "./lib/types";

vi.mock("./lib/api", () => ({ call: vi.fn(async () => ({ status: "running", completed: 0, total: 2 })) }));

const trackA: Track = { id: "a", provider: "local", providerId: "a", sourceUrl: "local:a", artist: "Example", title: "A", durationSeconds: 100, favorite: false, provenance: {} };
const trackB: Track = { id: "b", provider: "local", providerId: "b", sourceUrl: "local:b", artist: "Example", title: "B", durationSeconds: 110, favorite: false, provenance: {} };
let cleanup: (() => void) | undefined;

beforeEach(() => {
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
});
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; vi.clearAllMocks(); });
async function settle() { for (let index = 0; index < 8; index++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(AlbumDetailView, { target, props: {
    metadata: { title: "Example Album", artist: "Example Artist", sourceLabel: "Saved library", scopeLabel: "Saved album" },
    albumTracks: [trackA], analysisIdentity: "album:v1", trackListProps: { tracks: [trackA] },
    onPlayAlbum: vi.fn(), onQueueAlbum: vi.fn(), ...props,
  } as never });
  cleanup = () => unmount(instance);
  return target;
}

describe("AlbumDetailView", () => {
  it("resolves full canonical scope for album-wide play, queue, and download despite a one-row page", async () => {
    const resolveAlbumTracks = vi.fn(async () => [trackB, trackA]);
    const onPlayAlbum = vi.fn(); const onQueueAlbum = vi.fn(); const onDownloadAlbum = vi.fn();
    const target = render({ actionTrackCount: 2, resolveAlbumTracks, onPlayAlbum, onQueueAlbum, onDownloadAlbum });
    await settle();
    target.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".hero-actions button:not(.primary)")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".download-menu .download-album-action")!.click(); await settle();
    expect(resolveAlbumTracks).toHaveBeenCalledTimes(3);
    expect(onPlayAlbum).toHaveBeenCalledWith([trackB, trackA], null);
    expect(onQueueAlbum).toHaveBeenCalledWith([trackB, trackA], null);
    expect(onDownloadAlbum).toHaveBeenCalledWith([trackB, trackA]);
  });

  it("shows stale-snapshot errors and never invokes album playback on a rejected full-scope read", async () => {
    const resolveAlbumTracks = vi.fn(async () => { throw new Error("The saved album changed while loading."); });
    const onPlayAlbum = vi.fn();
    const target = render({ actionTrackCount: 2, resolveAlbumTracks, onPlayAlbum });
    target.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click(); await settle();
    expect(onPlayAlbum).not.toHaveBeenCalled();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Could not play album: The saved album changed while loading.");
  });

  it("drops a deferred old-album action when navigation changes the analysis identity", async () => {
    let finishRead!: (tracks: Track[]) => void;
    const resolveOldAlbum = vi.fn(() => new Promise<Track[]>(resolve => { finishRead = resolve; }));
    const onPlayOld = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(AlbumDetailViewHarness, { target }) as unknown as { updateAlbum(next: { identity: string; tracks: Track[]; resolve: () => Promise<Track[]>; onPlay: (tracks: Track[]) => void }): void };
    cleanup = () => unmount(instance as never);
    instance.updateAlbum({ identity: "album:old", tracks: [trackA], resolve: resolveOldAlbum, onPlay: onPlayOld }); await settle();
    target.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click(); await settle();
    instance.updateAlbum({ identity: "album:new", tracks: [trackB], resolve: async () => [trackB], onPlay: vi.fn() }); await settle();
    finishRead([trackA, trackB]); await settle();
    expect(onPlayOld).not.toHaveBeenCalled();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("This album changed while loading its tracks");
  });

  it("closes More with Escape and returns focus to its summary", async () => {
    const target = render({ onAnalyzeLoudness: vi.fn(async () => ({ jobId: "job", playbackTracks: [trackA, trackB] })) });
    const menu = target.querySelector<HTMLDetailsElement>(".more-menu")!;
    const summary = menu.querySelector<HTMLElement>("summary")!;
    summary.focus(); summary.click(); await settle();
    expect(menu.open).toBe(true);
    target.querySelector<HTMLButtonElement>(".more-menu button")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(menu.open).toBe(false);
    expect(document.activeElement).toBe(summary);
  });

  it("cancels an analysis job that resolves after the shared view unmounts", async () => {
    let finishStart!: (started: { jobId: string; playbackTracks: Track[] }) => void;
    const onAnalyzeLoudness = vi.fn(() => new Promise<{ jobId: string; playbackTracks: Track[] }>(resolve => { finishStart = resolve; }));
    const target = render({ albumTracks: [trackA, trackB], actionTrackCount: 2, trackListProps: { tracks: [trackA, trackB] }, onAnalyzeLoudness });
    const summary = target.querySelector<HTMLElement>(".more-menu summary")!;
    summary.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".more-menu button")].find(button => button.textContent?.includes("Analyze album loudness"))!.click();
    await settle();
    expect(onAnalyzeLoudness).toHaveBeenCalledOnce();
    cleanup?.(); cleanup = undefined;
    finishStart({ jobId: "late-job", playbackTracks: [trackA, trackB] }); await settle();
    expect(call).toHaveBeenCalledWith("cancel_loudness_analysis", { jobId: "late-job" });
  });
});
