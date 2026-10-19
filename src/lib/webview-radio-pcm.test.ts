import { afterEach, describe, expect, it, vi } from "vitest";
import { captureWebviewRadioWavBase64, encodeMonoPcm16WavBase64 } from "./webview-radio-pcm";

describe("captured WebView PCM WAV", () => {
  it("encodes an eight-second mono 48 kHz PCM16 sample within the native cap", () => {
    const frames = 8 * 48_000;
    const samples = Float32Array.from({ length: frames }, (_, index) => Math.sin(index / 12));
    const encoded = encodeMonoPcm16WavBase64(samples);
    const bytes = Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));
    const view = new DataView(bytes.buffer);
    expect(new TextDecoder().decode(bytes.subarray(0, 4))).toBe("RIFF");
    expect(new TextDecoder().decode(bytes.subarray(8, 12))).toBe("WAVE");
    expect(view.getUint32(24, true)).toBe(48_000);
    expect(view.getUint16(22, true)).toBe(1);
    expect(view.getUint16(34, true)).toBe(16);
    expect(view.getUint32(40, true)).toBe(frames * 2);
    expect(bytes.byteLength).toBe(44 + frames * 2);
    expect(bytes.byteLength).toBeLessThanOrEqual(1_200_000);
  });

  it("rejects short, oversized, and silent captures before IPC", () => {
    expect(() => encodeMonoPcm16WavBase64(new Float32Array(2 * 48_000).fill(.1))).toThrow(/three to twelve seconds/);
    expect(() => encodeMonoPcm16WavBase64(new Float32Array(13 * 48_000).fill(.1))).toThrow(/three to twelve seconds/);
    expect(() => encodeMonoPcm16WavBase64(new Float32Array(8 * 48_000))).toThrow(/silence/);
  });

  it("reports unsupported capture explicitly and skips stale playback generations", async () => {
    await expect(captureWebviewRadioWavBase64({} as HTMLAudioElement, () => true, new AbortController().signal))
      .rejects.toThrow(/cannot capture the currently playing audio/);
    const captureStream = vi.fn();
    const audio = { captureStream } as unknown as HTMLAudioElement;
    await expect(captureWebviewRadioWavBase64(audio, () => false, new AbortController().signal))
      .rejects.toMatchObject({ name: "AbortError" });
    expect(captureStream).not.toHaveBeenCalled();
  });

  it("captures the already-playing element once, with an isolated muted processing branch", async () => {
    const stop = vi.fn(), close = vi.fn(), connect = vi.fn(), disconnect = vi.fn();
    const track = { stop };
    const stream = { getAudioTracks: () => [track], getTracks: () => [track] };
    let worklet: { port: { onmessage: ((event: MessageEvent) => void) | null; postMessage: (message: {id:number}) => void; close: () => void } };
    class FakeContext {
      sampleRate = 48_000;
      destination = {};
      audioWorklet = { addModule: vi.fn(async () => {}) };
      createMediaStreamSource = vi.fn(() => ({ connect, disconnect }));
      createGain = vi.fn(() => ({ gain: { value: 1 }, connect, disconnect }));
      resume = vi.fn(async () => {});
      close = vi.fn(async () => { close(); });
    }
    class FakeWorklet {
      port = {
        onmessage: null as ((event: MessageEvent) => void) | null,
        postMessage: ({ id }: { id: number }) => queueMicrotask(() => {
          const samples = new Float32Array(8 * 48_000).fill(.2);
          worklet.port.onmessage?.({ data: { type: "snapshot", id, samples: samples.buffer } } as MessageEvent);
        }),
        close: vi.fn(),
      };
      constructor() { worklet = this; }
      connect = connect;
      disconnect = disconnect;
    }
    vi.stubGlobal("AudioWorkletNode", FakeWorklet);
    vi.stubGlobal("AudioContext", FakeContext);
    const capture = vi.fn(() => stream);
    const audio = { captureStream: capture } as unknown as HTMLAudioElement;
    try {
      const result = await captureWebviewRadioWavBase64(audio, () => true, new AbortController().signal);
      expect(result.length).toBeLessThan(1_600_000);
      expect(capture).toHaveBeenCalledTimes(1);
      expect(stop).toHaveBeenCalledTimes(1);
      expect(connect).toHaveBeenCalledTimes(3);
      expect(disconnect).toHaveBeenCalledTimes(3);
      expect(close).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("cancels capture and releases its capture tracks when playback identity goes stale", async () => {
    const stop = vi.fn(), close = vi.fn();
    const track = { stop }, stream = { getAudioTracks: () => [track], getTracks: () => [track] };
    class FakeContext {
      sampleRate = 48_000; destination = {};
      audioWorklet = { addModule: vi.fn(async () => {}) };
      createMediaStreamSource = () => ({ connect: vi.fn(), disconnect: vi.fn() });
      createGain = () => ({ gain: { value: 1 }, connect: vi.fn(), disconnect: vi.fn() });
      resume = vi.fn(async () => {});
      close = vi.fn(async () => { close(); });
    }
    class FakeWorklet { port = { onmessage: null, postMessage: vi.fn(), close: vi.fn() }; connect = vi.fn(); disconnect = vi.fn(); }
    vi.stubGlobal("AudioWorkletNode", FakeWorklet); vi.stubGlobal("AudioContext", FakeContext);
    const controller = new AbortController();
    const pending = captureWebviewRadioWavBase64({ captureStream: () => stream } as unknown as HTMLAudioElement, () => true, controller.signal);
    setTimeout(() => controller.abort(), 5);
    try {
      await expect(pending).rejects.toMatchObject({ name: "AbortError" });
      expect(stop).toHaveBeenCalledTimes(1);
      expect(close).toHaveBeenCalledTimes(1);
    } finally { vi.unstubAllGlobals(); }
  });

  it("cancels a stalled worklet setup and enforces the total deadline before setup completes", async () => {
    const stop = vi.fn(), close = vi.fn();
    const track = { stop }, stream = { getAudioTracks: () => [track], getTracks: () => [track] };
    let setupStarted!: () => void;
    const started = new Promise<void>((resolve) => { setupStarted = resolve; });
    class FakeContext {
      sampleRate = 48_000; destination = {};
      audioWorklet = { addModule: vi.fn(() => { setupStarted(); return new Promise<void>(() => {}); }) };
      createMediaStreamSource = () => ({ connect: vi.fn(), disconnect: vi.fn() });
      createGain = () => ({ gain: { value: 1 }, connect: vi.fn(), disconnect: vi.fn() });
      resume = vi.fn(async () => {});
      close = vi.fn(async () => { close(); });
    }
    class FakeWorklet { port = { onmessage: null, postMessage: vi.fn(), close: vi.fn() }; connect = vi.fn(); disconnect = vi.fn(); }
    vi.stubGlobal("AudioWorkletNode", FakeWorklet); vi.stubGlobal("AudioContext", FakeContext);
    try {
      const controller = new AbortController();
      const pendingAbort = captureWebviewRadioWavBase64({ captureStream: () => stream } as unknown as HTMLAudioElement, () => true, controller.signal);
      await started; controller.abort();
      await expect(pendingAbort).rejects.toMatchObject({ name: "AbortError" });
      expect(stop).toHaveBeenCalledTimes(1);
      expect(close).toHaveBeenCalledTimes(1);

      vi.useFakeTimers();
      const pendingDeadline = captureWebviewRadioWavBase64({ captureStream: () => stream } as unknown as HTMLAudioElement, () => true, new AbortController().signal);
      const deadlineAssertion = expect(pendingDeadline).rejects.toThrow(/within twelve seconds/);
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(12_000);
      await deadlineAssertion;
      expect(stop).toHaveBeenCalledTimes(2);
      expect(close).toHaveBeenCalledTimes(2);
    } finally { vi.useRealTimers(); vi.unstubAllGlobals(); }
  });

  it("cancels an in-flight provider when the overall capture deadline expires", async () => {
    vi.useFakeTimers();
    const stop = vi.fn(), onCancel = vi.fn();
    class FakeContext {
      sampleRate = 48_000; destination = {};
      audioWorklet = {addModule: async () => {}};
      createMediaStreamSource = () => ({connect: vi.fn(), disconnect: vi.fn()});
      createGain = () => ({gain: {value: 1}, connect: vi.fn(), disconnect: vi.fn()});
      resume = async () => {}; close = async () => {};
    }
    class FakeWorklet {
      port = {onmessage: null as ((event: MessageEvent) => void) | null, close: vi.fn(), postMessage: ({id, minFrames}: {id: number; minFrames: number}) => {
        queueMicrotask(() => this.port.onmessage?.({data: {type: "snapshot", id, samples: new Float32Array(minFrames).fill(.2).buffer}} as MessageEvent));
      }};
      connect = vi.fn(); disconnect = vi.fn();
    }
    vi.stubGlobal("AudioContext", FakeContext); vi.stubGlobal("AudioWorkletNode", FakeWorklet);
    const onSample = vi.fn(() => new Promise<boolean>(() => {}));
    try {
      const pending = captureWebviewRadioWavBase64({captureStream: () => ({getAudioTracks: () => [{}], getTracks: () => [{stop}]})} as unknown as HTMLAudioElement,
        () => true, new AbortController().signal, undefined, {onSample, onCancel});
      const rejected = expect(pending).rejects.toThrow(/timed out/);
      await vi.advanceTimersByTimeAsync(250);
      expect(onSample).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(90_000);
      await rejected;
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(stop).toHaveBeenCalledTimes(1);
      expect(onSample).toHaveBeenCalledTimes(1);
    } finally {vi.useRealTimers(); vi.unstubAllGlobals();}
  });

  it.each([6, 12])("tries progressive heard-audio samples sequentially and stops at %s seconds", async (matchAt) => {
    vi.useFakeTimers();
    const stop = vi.fn(), capture = vi.fn(() => ({getAudioTracks: () => [{}], getTracks: () => [{stop}]}));
    const starts: number[] = [], lengths: number[] = [];
    class FakeContext {
      sampleRate = 48_000; destination = {};
      audioWorklet = {addModule: async () => {}};
      createMediaStreamSource = () => ({connect: vi.fn(), disconnect: vi.fn()});
      createGain = () => ({gain: {value: 1}, connect: vi.fn(), disconnect: vi.fn()});
      resume = async () => {}; close = async () => {};
    }
    class FakeWorklet {
      port = {onmessage: null as ((event: MessageEvent) => void) | null, close: vi.fn(), postMessage: ({id, minFrames}: {id: number; minFrames: number}) => {
        queueMicrotask(() => this.port.onmessage?.({data: {type: "snapshot", id, samples: new Float32Array(minFrames).fill(.2).buffer}} as MessageEvent));
      }};
      connect = vi.fn(); disconnect = vi.fn();
    }
    vi.stubGlobal("AudioContext", FakeContext); vi.stubGlobal("AudioWorkletNode", FakeWorklet);
    const onSample = vi.fn(async (_wav: string, seconds: number) => {starts.push(Date.now()); lengths.push(seconds); return seconds === matchAt;});
    try {
      const pending = captureWebviewRadioWavBase64({captureStream: capture} as unknown as HTMLAudioElement, () => true, new AbortController().signal, undefined, {onSample});
      await vi.advanceTimersByTimeAsync(250);
      expect(lengths).toEqual([3]);
      await vi.advanceTimersByTimeAsync(matchAt === 6 ? 10_000 : 20_000);
      await pending;
      expect(lengths).toEqual(matchAt === 6 ? [3, 6] : [3, 6, 12]);
      expect(starts[1]! - starts[0]!).toBeGreaterThanOrEqual(10_000);
      expect(capture).toHaveBeenCalledTimes(1);
      expect(stop).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(20_000);
      expect(onSample).toHaveBeenCalledTimes(matchAt === 6 ? 2 : 3);
    } finally {vi.useRealTimers(); vi.unstubAllGlobals();}
  });

});
