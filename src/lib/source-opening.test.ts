import { describe, expect, it } from "vitest";
import { sourcePositionUrl } from "./source-opening";
import type { Track } from "./types";
describe("source opening position", () => {
  it("adds chapter offsets to YouTube positions and retains existing parameters", () => {
    const track = {playbackRange:{startTime:120}} as Track;
    const url = new URL(sourcePositionUrl("https://www.youtube.com/watch?v=abc&list=x",track,35.9));
    expect(url.searchParams.get("t")).toBe("155s");
    expect(url.searchParams.get("list")).toBe("x");
  });
  it("uses SoundCloud timestamp fragments and leaves unsupported sources unchanged", () => {
    expect(sourcePositionUrl("https://soundcloud.com/a/b",{} as Track,65)).toBe("https://soundcloud.com/a/b#t=1:05");
    expect(sourcePositionUrl("https://open.spotify.com/track/a",{} as Track,65)).toBe("https://open.spotify.com/track/a");
  });
});
