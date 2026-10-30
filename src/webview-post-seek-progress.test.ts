import { afterEach, describe, expect, it, vi } from "vitest";
import { WebviewPostSeekProgress } from "./webview-post-seek-progress";

describe("post-seek progress verification", () => {
  afterEach(() => vi.useRealTimers());
  function setup() {
    vi.useFakeTimers();
    const state = { currentTime: 1, paused: false, seeking: false, ended: false, error: null };
    const node = state as unknown as HTMLAudioElement;
    const owner = { node, sourceGeneration: 1, seekSerial: 2, queueOccurrenceId: "entry" };
    const current = vi.fn(() => true), expired = vi.fn(), check = new WebviewPostSeekProgress();
    const begin = () => check.begin({ owner, isCurrent: current, onExpired: expired });
    return { node: state, current, expired, check, begin };
  }
  it("recovers a landed but frozen playing seek", () => {
    const f = setup(); f.begin(); vi.advanceTimersByTime(5_000); expect(f.expired).toHaveBeenCalledOnce();
  });
  it("accepts actual forward clock movement", () => {
    const f = setup(); f.begin(); f.node.currentTime = 2; vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
  });
  it("does not start while paused, or recover after a user pause", () => {
    const f = setup(); f.node.paused = true; f.begin(); vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
    f.node.paused = false; f.begin(); f.node.paused = true; vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
  });
  it("ignores stale owners, cancellation, and subsequent seeks", () => {
    const f = setup(); f.begin(); f.current.mockReturnValue(false); vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
    f.current.mockReturnValue(true); f.begin(); f.check.cancel(); vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
    f.begin(); f.node.seeking = true; vi.advanceTimersByTime(5_000); expect(f.expired).not.toHaveBeenCalled();
  });
});
