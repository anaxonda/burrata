import { describe, expect, it } from "vitest";
import { PlaybackQueue, loadPlaybackQueue, savePlaybackQueue } from "./playback-queue";
import type { Track } from "./types";

const track = (id: string) => ({ id, provider: "test", providerId: id, sourceUrl: "", artist: "A", title: id, favorite: false, provenance: {} }) as Track;

describe("PlaybackQueue", () => {
  it("follows the displayed order and supports play next, removal, and reordering", () => {
    const q = new PlaybackQueue(); q.setTracks([track("a"), track("b"), track("c")], 1);
    q.playNext(track("x")); expect(q.tracks.map((t) => t.id)).toEqual(["a", "b", "x", "c"]);
    expect(q.next()?.id).toBe("x"); q.remove("b"); expect(q.current?.id).toBe("x");
    q.reorder(2, 0); expect(q.current?.id).toBe("x"); expect(q.tracks.map((t) => t.id)).toEqual(["c", "a", "x"]);
  });

  it("implements repeat modes and shuffles only upcoming tracks", () => {
    const q = new PlaybackQueue(); q.setTracks(["a", "b", "c", "d"].map(track), 1);
    q.shuffle(() => 0); expect(q.tracks[0]?.id).toBe("a"); expect(q.tracks[1]?.id).toBe("b");
    q.setRepeat("one"); expect(q.next()?.id).toBe("b");
    q.setRepeat("all"); q.setTracks([track("a"), track("b")], 1); expect(q.next()?.id).toBe("a");
    q.setRepeat("off"); expect(q.next()?.id).toBe("b"); expect(q.next()).toBeNull();
  });

  it("persists and restores snapshots safely", () => {
    const values = new Map<string, string>();
    const storage = { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => { values.set(k, v); } } as Storage;
    const q = new PlaybackQueue(); q.setTracks([track("a"), track("b")], 1); q.setRepeat("all");
    expect(savePlaybackQueue(q, storage)).toBe(true);
    const restored = loadPlaybackQueue(storage)!;
    expect(restored.current?.id).toBe("b"); expect(restored.repeat).toBe("all");
  });

  it("restores legacy Track arrays and preserves optional chapter ranges", () => {
    const values = new Map<string, string>([["burrata.playback-queue.v1", JSON.stringify({ tracks: [track("legacy")], currentIndex: 0 })]]);
    const storage = { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => { values.set(k, v); } } as Storage;
    const legacy = loadPlaybackQueue(storage)!;
    expect(legacy.current?.id).toBe("legacy");
    expect(legacy.repeat).toBe("off");
    const ranged = { ...track("chapter"), playbackRange: { parentTrackId: "parent", chapterId: "c1", sourceUrl: "https://audio.test", startTime: 10, endTime: 20, parentTitle: "Mix" } } as Track;
    const queue = new PlaybackQueue(); queue.setTracks([ranged]);
    expect(savePlaybackQueue(queue, storage)).toBe(true);
    expect(loadPlaybackQueue(storage)?.current).toMatchObject({ id: "chapter", playbackRange: { startTime: 10, endTime: 20 } });
  });

  it("removes the chosen duplicate occurrence by index", () => {
    const duplicate = track("same");
    const q = new PlaybackQueue(); q.setTracks([duplicate, track("middle"), duplicate], 2);
    expect(q.removeAt(0)).toBe(duplicate);
    expect(q.currentIndex).toBe(1);
    expect(q.current).toBe(duplicate);
    expect(q.tracks.map((t) => t.id)).toEqual(["middle", "same"]);
    expect(q.removeAt(8)).toBeNull();
  });

  it("bounds automatic continuation after unavailable recordings", () => {
    const q = new PlaybackQueue(); q.setTracks(["a", "b", "c", "d", "e"].map(track));
    expect(q.nextAfterFailedStart(0, 3)?.id).toBe("b");
    expect(q.nextAfterFailedStart(1, 3)?.id).toBe("c");
    expect(q.nextAfterFailedStart(2, 3)?.id).toBe("d");
    expect(q.nextAfterFailedStart(3, 3)).toBeNull();
  });
});
