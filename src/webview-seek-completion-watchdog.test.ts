import { afterEach, describe, expect, it, vi } from "vitest";
import { WebviewSeekCompletionWatchdog, hasWebviewSeekLanded, type WebviewSeekCompletionOwner } from "./webview-seek-completion-watchdog";

describe("WebView seek completion watchdog", () => {
  afterEach(() => vi.useRealTimers());
  function setup() {
    vi.useFakeTimers();
    const node = {} as HTMLAudioElement;
    let current = true;
    const owner: WebviewSeekCompletionOwner = { node, sourceGeneration: 8, seekSerial: 3, queueOccurrenceId: "entry-a" };
    const watchdog = new WebviewSeekCompletionWatchdog();
    const expired = vi.fn();
    const begin = (paused = false, target = 44, thisOwner = owner) => watchdog.begin({ owner: thisOwner, target, paused, timeoutMs: 20_000,
      isCurrent: () => current, onExpired: expired });
    return { node, owner, watchdog, expired, begin, setCurrent: (value: boolean) => { current = value; } };
  }
  it("expires a held active seek once and carries the requested target", () => {
    const { watchdog, expired, begin } = setup();
    begin(false, 44);
    vi.advanceTimersByTime(19_999);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledOnce();
    expect(expired).toHaveBeenCalledWith(44);
    vi.advanceTimersByTime(60_000);
    expect(expired).toHaveBeenCalledOnce();
    watchdog.cancel();
  });
  it("lets a timely seeked completion cancel the deadline", () => {
    const { watchdog, owner, expired, begin } = setup();
    begin();
    vi.advanceTimersByTime(9_000);
    watchdog.complete(owner);
    vi.advanceTimersByTime(20_000);
    expect(expired).not.toHaveBeenCalled();
  });
  it("defers a paused seek deadline until the matching owner resumes", () => {
    const { watchdog, owner, expired, begin } = setup();
    begin(true, 71);
    vi.advanceTimersByTime(30_000);
    expect(expired).not.toHaveBeenCalled();
    watchdog.resume(owner);
    vi.advanceTimersByTime(19_999);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledWith(71);
  });
  it("cancels on pause and does not restart the bound until resume", () => {
    const { watchdog, owner, expired, begin } = setup();
    begin(false, 20);
    vi.advanceTimersByTime(10_000);
    watchdog.pause(owner);
    vi.advanceTimersByTime(30_000);
    expect(expired).not.toHaveBeenCalled();
    watchdog.resume(owner);
    vi.advanceTimersByTime(20_000);
    expect(expired).toHaveBeenCalledOnce();
  });
  it("replaces an older seek and ignores a stale owner at expiry", () => {
    const { watchdog, owner, expired, begin, setCurrent } = setup();
    begin(false, 20);
    vi.advanceTimersByTime(10_000);
    const newer = { ...owner, seekSerial: 4 };
    begin(false, 60, newer);
    vi.advanceTimersByTime(19_999);
    expect(expired).not.toHaveBeenCalled();
    setCurrent(false);
    vi.advanceTimersByTime(1);
    expect(expired).not.toHaveBeenCalled();
  });
});

describe("seek landing after queued media events", () => {
  const input = { target: 80, currentTime: 80.805442176, duration: 155.364716553, elapsedMs: 900, playing: true, playbackRate: 1 };
  it("accepts the observed successful landing after playback advanced past fixed tolerance", () => {
    expect(hasWebviewSeekLanded(input)).toBe(true);
    expect(hasWebviewSeekLanded({ ...input, currentTime: 81.6, playbackRate: 2 })).toBe(true);
  });
  it("accepts the observed coarse browser acknowledgment without inventing elapsed playback", () => {
    const actual = { ...input, currentTime: 81.037641723, elapsedMs: 5 };
    expect(hasWebviewSeekLanded(actual)).toBe(false);
    expect(hasWebviewSeekLanded({ ...actual, browserAcknowledged: true })).toBe(true);
    expect(hasWebviewSeekLanded({ ...actual, currentTime: 83, browserAcknowledged: true })).toBe(false);
    expect(hasWebviewSeekLanded({ ...actual, currentTime: 30, browserAcknowledged: true })).toBe(false);
  });
  it("does not forgive a wrong location merely because a seek event arrived", () => {
    expect(hasWebviewSeekLanded({ ...input, currentTime: 30 })).toBe(false);
    expect(hasWebviewSeekLanded({ ...input, currentTime: 90 })).toBe(false);
    expect(hasWebviewSeekLanded({ ...input, elapsedMs: 0 })).toBe(false);
    expect(hasWebviewSeekLanded({ ...input, elapsedMs: Number.NaN })).toBe(false);
  });
  it("keeps a paused seek bounded to its target without elapsed playback credit", () => {
    expect(hasWebviewSeekLanded({ ...input, elapsedMs: 10_000, playing: false })).toBe(false);
    expect(hasWebviewSeekLanded({ ...input, currentTime: 80.2, playing: false })).toBe(true);
  });
});
