// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { attachAudioStream } from "./audio-stream";

const attached = "media-attached";
const errorEvent = "hls-error";

class FakeHls {
  static supported = true;
  static Events = { MEDIA_ATTACHED: attached, ERROR: errorEvent };
  static instance: FakeHls;
  listeners = new Map<string, Set<(event: string, data?: { fatal?: boolean; details?: string }) => void>>();
  attachMedia = vi.fn();
  loadSource = vi.fn();
  destroy = vi.fn();
  constructor() { FakeHls.instance = this; }
  static isSupported() { return FakeHls.supported; }
  on(event: string, listener: (event: string, data?: { fatal?: boolean; details?: string }) => void) {
    const listeners = this.listeners.get(event) ?? new Set();
    listeners.add(listener); this.listeners.set(event, listeners);
  }
  off(event: string, listener: (event: string, data?: { fatal?: boolean; details?: string }) => void) {
    this.listeners.get(event)?.delete(listener);
  }
  emit(event: string, data?: { fatal?: boolean; details?: string }) {
    for (const listener of this.listeners.get(event) ?? []) listener(event, data);
  }
}

function audioElement() {
  const audio = document.createElement("audio");
  vi.spyOn(audio, "load").mockImplementation(() => {});
  vi.spyOn(audio, "pause").mockImplementation(() => {});
  vi.spyOn(audio, "canPlayType").mockReturnValue("");
  return audio;
}

function fakeLoader() { return vi.fn(async () => FakeHls as never); }

afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

describe("attachAudioStream", () => {
  it("keeps ordinary audio streams on the direct media-element path", async () => {
    const audio = audioElement();
    const handle = await attachAudioStream(audio, "https://cdn.test/song.mp3", "audio/mpeg");
    expect(handle.kind).toBe("direct");
    expect(audio.getAttribute("src")).toBe("https://cdn.test/song.mp3");
    handle.destroy();
    expect(audio.hasAttribute("src")).toBe(false);
  });

  it("prefers native HLS when the browser advertises support", async () => {
    const audio = audioElement();
    vi.mocked(audio.canPlayType).mockReturnValue("maybe");
    const loadHls = fakeLoader();
    const handle = await attachAudioStream(audio, "https://cdn.test/live.m3u8", null, { loadHls });
    expect(handle.kind).toBe("native-hls");
    expect(audio.getAttribute("src")).toBe("https://cdn.test/live.m3u8");
    expect(loadHls).not.toHaveBeenCalled();
    handle.destroy();
  });

  it("attaches MSE before loading the manifest and forwards fatal errors once", async () => {
    const audio = audioElement();
    const loadHls = fakeLoader();
    const onError = vi.fn();
    const mediaError = vi.fn();
    audio.addEventListener("error", mediaError);
    const preparing = attachAudioStream(audio, "https://cdn.test/live", "application/vnd.apple.mpegurl", { loadHls, onError });
    await Promise.resolve();
    const hls = FakeHls.instance;
    expect(hls.attachMedia).toHaveBeenCalledWith(audio);
    expect(hls.loadSource).not.toHaveBeenCalled();
    hls.emit(attached);
    const handle = await preparing;
    expect(handle.kind).toBe("mse-hls");
    expect(hls.loadSource).toHaveBeenCalledWith("https://cdn.test/live");
    hls.emit(errorEvent, { fatal: true, details: "networkError" });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: "networkError" }));
    expect(mediaError).toHaveBeenCalledTimes(1);
    expect(hls.destroy).toHaveBeenCalledTimes(1);
    handle.destroy();
    expect(hls.destroy).toHaveBeenCalledTimes(1);
  });

  it("cancels stale setup before media attach without leaving a pending promise", async () => {
    vi.useFakeTimers();
    const audio = audioElement();
    const loadHls = fakeLoader();
    let current = true;
    const preparing = attachAudioStream(audio, "https://cdn.test/live.m3u8", null, { loadHls, isCurrent: () => current });
    const rejected = expect(preparing).rejects.toMatchObject({ name: "AbortError" });
    await Promise.resolve();
    current = false;
    await vi.advanceTimersByTimeAsync(51);
    await rejected;
    expect(FakeHls.instance.destroy).toHaveBeenCalledTimes(1);
    expect(audio.hasAttribute("src")).toBe(false);
  });

  it("rejects fatal setup errors before media is ready and unsupported MSE", async () => {
    const audio = audioElement();
    const loadHls = fakeLoader();
    const preparing = attachAudioStream(audio, "https://cdn.test/live.m3u8", null, { loadHls });
    await Promise.resolve();
    FakeHls.instance.emit(errorEvent, { fatal: true, details: "manifestLoadError" });
    await expect(preparing).rejects.toThrow("manifestLoadError");

    FakeHls.supported = false;
    await expect(attachAudioStream(audio, "https://cdn.test/live.m3u8", null, { loadHls })).rejects.toThrow("cannot play HLS");
    FakeHls.supported = true;
  });
});
