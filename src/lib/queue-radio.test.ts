import { describe, expect, it, vi } from "vitest";
import { QueueRadioController, queueRadioSeedKey, type QueueRadioFetch } from "./queue-radio";
import type { Track } from "./types";

function track(id: string, providerId = id): Track {
  return {
    id,
    provider: "youtube",
    providerId,
    sourceUrl: `https://youtube.com/watch?v=${providerId}`,
    artist: "Artist",
    title: id,
    favorite: false,
    provenance: {},
  };
}

describe("QueueRadioController", () => {
  it("changes its prepared seed identity when only the playable candidate changes", () => {
    const seed = track("same-track", "same-provider-id");
    const changedCandidate = {
      ...seed,
      provenance: { audioCandidate: { provider: "soundcloud", id: "candidate-2", url: "https://soundcloud.com/a/song" } },
    };
    expect(queueRadioSeedKey(changedCandidate)).not.toBe(queueRadioSeedKey(seed));
  });

  it("prepares only from the final queued track and takes only at natural end with repeat off", async () => {
    const fetch = vi.fn<QueueRadioFetch>().mockResolvedValue([track("suggested")]);
    const controller = new QueueRadioController(fetch);
    const first = track("first");
    const last = track("last");

    controller.prepare(first, [first, last], []);
    expect(controller.isPending).toBe(false);
    expect(fetch).not.toHaveBeenCalled();

    controller.prepare(last, [first, last], []);
    expect(controller.isPending).toBe(true);
    expect(await controller.take(last, [first, last], [], false, "off")).toEqual([]);
    expect(controller.isPending).toBe(false);

    controller.prepare(last, [first, last], []);
    expect(await controller.take(last, [first, last], [], true, "all")).toEqual([]);
    expect(controller.isPending).toBe(false);
  });

  it("filters current queue, recent tracks, and duplicates before returning recommendations", async () => {
    const seed = track("seed", "seed-video");
    const queued = track("queued", "queued-video");
    const recent = track("recent", "recent-video");
    const fetch = vi.fn<QueueRadioFetch>().mockResolvedValue([
      track("seed-copy", "seed-video"),
      track("queued", "other-video"),
      track("recent-copy", "recent-video"),
      track("recommended", "new-video"),
      track("recommended-copy", "new-video"),
    ]);
    const controller = new QueueRadioController(fetch);

    controller.prepare(seed, [queued, seed], [recent]);
    await Promise.resolve();
    const result = await controller.take(seed, [queued, seed], [recent], true, "off");

    expect(result.map((item) => item.id)).toEqual(["recommended"]);
  });

  it("uses source candidates without folding case-sensitive video IDs or distinct chapter ranges", async () => {
    const seed = track("seed", "SeedVideo01");
    const sourceTrack = {
      ...track("library-id", "SomeOtherId"),
      provenance: { audioCandidate: { id: "youtube:QueuedVideo1", provider: "youtube", url: "https://youtube.com/watch?v=QueuedVideo1" } },
    };
    const chapter = {
      ...track("same-chapter-track", "same-parent-video"),
      playbackRange: { parentTrackId: "parent", chapterId: "chapter-a", sourceUrl: "https://youtube.com/watch?v=ParentVideo1", startTime: 30, endTime: 60, parentTitle: "Parent" },
    };
    const anotherChapter = { ...chapter, playbackRange: { ...chapter.playbackRange!, chapterId: "chapter-b", startTime: 60, endTime: 90 } };
    const fetch = vi.fn<QueueRadioFetch>().mockResolvedValue([
      track("same-candidate", "QueuedVideo1"),
      track("case-different", "queuedvideo1"),
      anotherChapter,
    ]);
    const controller = new QueueRadioController(fetch);
    const queue = [sourceTrack, chapter, seed];

    controller.prepare(seed, queue, []);
    await Promise.resolve();
    const result = await controller.take(seed, queue, [], true, "off");

    expect(result.map((item) => item.id)).toEqual(["case-different", "same-chapter-track"]);
  });

  it("discards late responses after cancellation even when the request ignores AbortSignal", async () => {
    let resolveRequest!: (items: readonly Track[]) => void;
    const fetch = vi.fn<QueueRadioFetch>(() => new Promise((resolve) => { resolveRequest = resolve; }));
    const controller = new QueueRadioController(fetch);
    const seed = track("seed");

    controller.prepare(seed, [seed], []);
    await Promise.resolve();
    const oldGeneration = controller.generation;
    controller.cancel();
    expect(controller.generation).toBeGreaterThan(oldGeneration);
    expect(fetch.mock.calls[0]?.[1].aborted).toBe(true);
    resolveRequest([track("late")]);

    expect(await controller.take(seed, [seed], [], true, "off")).toEqual([]);
  });

  it("releases a pending natural-end wait immediately when stopped", async () => {
    const fetch = vi.fn<QueueRadioFetch>(() => new Promise(() => {}));
    const controller = new QueueRadioController(fetch);
    const seed = track("seed");
    controller.prepare(seed, [seed], []);
    const taking = controller.take(seed, [seed], [], true, "off");
    await Promise.resolve();
    controller.cancel();
    await expect(taking).resolves.toEqual([]);
  });

  it("rejects stale queue snapshots and invalidates failed or stopped playback", async () => {
    const seed = track("seed");
    const fetch = vi.fn<QueueRadioFetch>().mockResolvedValue([track("suggested")]);
    const controller = new QueueRadioController(fetch);

    controller.prepare(seed, [seed], []);
    await Promise.resolve();
    expect(await controller.take(seed, [track("new-tail")], [], true, "off")).toEqual([]);

    controller.prepare(seed, [seed], []);
    await Promise.resolve();
    controller.cancel(); // stop, failure, or an explicit queue change
    expect(await controller.take(seed, [seed], [], true, "off")).toEqual([]);
  });

  it("discards recommendations when a queue row keeps its ID but changes playable source", async () => {
    let resolveRequest!: (items: readonly Track[]) => void;
    const fetch = vi.fn<QueueRadioFetch>(() => new Promise((resolve) => { resolveRequest = resolve; }));
    const seed = track("seed", "seed-video");
    const controller = new QueueRadioController(fetch);

    controller.prepare(seed, [seed], []);
    await Promise.resolve();
    resolveRequest([track("recommendation")]);
    await Promise.resolve();

    const changedSeed = {
      ...seed,
      sourceUrl: "https://youtube.com/watch?v=replacement-video",
      provenance: { audioCandidate: { provider: "youtube", id: "replacement-video", url: "https://youtube.com/watch?v=replacement-video" } },
    };
    expect(await controller.take(changedSeed, [changedSeed], [], true, "off")).toEqual([]);
  });

  it("treats failed or exhausted recommendation lookups as no continuation", async () => {
    const seed = track("seed");
    const failed = new QueueRadioController(vi.fn<QueueRadioFetch>().mockRejectedValue(new Error("offline")));
    failed.prepare(seed, [seed], []);
    expect(await failed.take(seed, [seed], [], true, "off")).toEqual([]);
    expect(failed.isPending).toBe(false);

    const exhausted = new QueueRadioController(vi.fn<QueueRadioFetch>().mockResolvedValue([]));
    exhausted.prepare(seed, [seed], []);
    expect(await exhausted.take(seed, [seed], [], true, "off")).toEqual([]);
    expect(exhausted.isPending).toBe(false);
  });
});
