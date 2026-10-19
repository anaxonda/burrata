import { afterEach, describe, expect, it, vi } from "vitest";
import { WebviewStallWatchdog, type WebviewStallOwner } from "./webview-stall-watchdog";

describe("WebView steady stall watchdog", () => {
  afterEach(() => vi.useRealTimers());

  function setup() {
    vi.useFakeTimers();
    const node = { paused: false, seeking: false, ended: false, error: null, currentTime: 30 } as unknown as HTMLAudioElement;
    let current = true;
    const owner: WebviewStallOwner = { node, sourceGeneration: 3, seekSerial: 1, queueOccurrenceId: "occ-a" };
    const watchdog = new WebviewStallWatchdog();
    const onExpired = vi.fn();
    const arm = (duration = 240, live = false) => watchdog.arm({ owner, currentNode: current ? node : null, isCurrent: () => current,
      mediaTime: node.currentTime, duration, paused: node.paused, seeking: node.seeking, ended: node.ended, hasError: Boolean(node.error), live, onExpired });
    return { node, owner, watchdog, onExpired, arm, setCurrent: (value: boolean) => { current = value; } };
  }

  it("expires once after 20 seconds only while the owned finite-media clock is stopped", () => {
    const { watchdog, onExpired, arm } = setup();
    arm();
    vi.advanceTimersByTime(19_999);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpired).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(60_000);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
  });

  it("does not extend the same stall episode when waiting and stalled repeat", () => {
    const { watchdog, onExpired, arm } = setup();
    arm();
    vi.advanceTimersByTime(15_000);
    arm();
    vi.advanceTimersByTime(4_999);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
  });

  it.each(["paused", "stale", "unknown duration"])("does not recover for %s ownership/state", (caseName) => {
    const { node, watchdog, onExpired, arm, setCurrent } = setup();
    if (caseName === "paused") (node as unknown as { paused: boolean }).paused = true;
    if (caseName === "stale") setCurrent(false);
    if (caseName === "unknown duration") arm(Number.NaN);
    else if (caseName === "live") arm(240, true);
    else arm();
    vi.advanceTimersByTime(20_000);
    expect(onExpired).not.toHaveBeenCalled();
    watchdog.reset();
  });

  it("bounds a live source with infinite duration to five seconds without clock progress", () => {
    const { node, owner, watchdog, onExpired, arm } = setup();
    arm(Infinity, true);
    vi.advanceTimersByTime(4_000);
    (node as unknown as {currentTime:number}).currentTime += 4;
    watchdog.observeProgress(owner, node.currentTime);
    vi.advanceTimersByTime(4_999);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
  });

  it("rechecks owner identity at expiry after a queue/source change", () => {
    const { watchdog, onExpired, arm, setCurrent } = setup();
    arm();
    setCurrent(false);
    vi.advanceTimersByTime(20_000);
    expect(onExpired).not.toHaveBeenCalled();
    watchdog.reset();
  });

  it("moves the deadline on real progress and rechecks the clock after timer clamping", () => {
    const { node, owner, watchdog, onExpired, arm } = setup();
    arm();
    vi.advanceTimersByTime(500);
    (node as unknown as { currentTime: number }).currentTime += 0.5;
    watchdog.observeProgress(owner, node.currentTime);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(19_999);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("restarts a bounded deadline when a clamped timer finds actual clock movement", () => {
    const { node, watchdog, onExpired, arm } = setup();
    arm();
    (node as unknown as { currentTime: number }).currentTime += 20;
    vi.advanceTimersByTime(20_000);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(19_999);
    expect(onExpired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
  });

  it("recovers when a long-window trickle is too slow to count as plausible progress", () => {
    const { node, watchdog, onExpired, arm } = setup();
    arm();
    (node as unknown as { currentTime: number }).currentTime += 0.5;
    vi.advanceTimersByTime(20_000);
    expect(onExpired).toHaveBeenCalledTimes(1);
    watchdog.reset();
  });
});
