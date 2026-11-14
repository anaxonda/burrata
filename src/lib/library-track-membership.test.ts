import { describe, expect, it, vi } from "vitest";
import { readLibraryTrackMembership } from "./library-track-membership";
import type { Track } from "./types";

describe("album library status", () => {
  it("reads bounded sequential batches and ignores IDs outside each batch", async () => {
    const tracks = Array.from({ length: 401 }, (_, id) => ({ id: String(id) }) as Track);
    const call = vi.fn(async (_command, args) => [args.tracks[0].id, "outside-album"]);
    const saved = await readLibraryTrackMembership(tracks, call as never, () => true);
    expect([...saved]).toEqual(["0", "200", "400"]);
    expect(call.mock.calls.map(([, args]) => args.tracks.length)).toEqual([200, 200, 1]);
  });

  it("stops subsequent batches after navigation away", async () => {
    let active = true;
    const call = vi.fn(async () => { active = false; return ["0"]; });
    expect(await readLibraryTrackMembership(Array.from({ length: 401 }, (_, id) => ({ id: String(id) }) as Track), call as never, () => active)).toEqual(new Set());
    expect(call).toHaveBeenCalledTimes(1);
  });
});
