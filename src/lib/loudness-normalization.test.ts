// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { attachWebAudioPcmTap, encodeWebAudioPcm16Base64, hasWebAudioGainGraph, inspectWebAudioGainGraph, readWebAudioPeakWindow, retireWebAudioGainGraph, startWebAudioStereoDiagnosticTap, WebAudioGainAdapter, type WebAudioContextFactory } from "./loudness-normalization";

type FakeProcessEvent = {
  inputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array };
  outputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array };
};

function fakeGraph() {
  const calls: unknown[][] = [];
  const makeParam = (record: unknown[][]) => {
    const param = {
      value: 1,
      cancelScheduledValues: (time: number) => record.push(["cancel", time]),
      setValueAtTime: (value: number, time: number) => { record.push(["set", value, time]); param.value = value; },
      linearRampToValueAtTime: (value: number, end: number) => { record.push(["ramp", value, end]); param.value = value; },
    };
    return param;
  };
  const param = makeParam(calls);
  const userCalls: unknown[][] = [];
  const userParam = makeParam(userCalls);
  const destination = {};
  const source = { connect: vi.fn(), disconnect: vi.fn() };
  const gain = { gain: param, connect: vi.fn(), disconnect: vi.fn() };
  const userVolume = { gain: userParam, connect: vi.fn() };
  const analyser = { fftSize: 2048, smoothingTimeConstant: 0, connect: vi.fn(), getFloatTimeDomainData: vi.fn((buffer: Float32Array) => buffer.fill(0.5)) };
  const processor = { onaudioprocess: null as ((event: FakeProcessEvent) => void) | null, connect: vi.fn(), disconnect: vi.fn() };
  const extraGains = Array.from({length: 4}, () => ({gain: makeParam([]), connect: vi.fn()}));
  let gainIndex = 0;
  const context = { currentTime: 12, state: "running", sampleRate: 48_000, destination,
    createGain: vi.fn(() => [gain, userVolume, ...extraGains][gainIndex++]!),
    createScriptProcessor: vi.fn(() => processor),
    createAnalyser: vi.fn(() => analyser), createMediaElementSource: vi.fn(() => source), resume: vi.fn(async () => {}), close: vi.fn(async () => {}) };
  const factory: WebAudioContextFactory = () => context;
  return { calls, userCalls, param, userParam, source, gain, userVolume, analyser, processor, context, factory };
}

function processFallbackBlock(processor: ReturnType<typeof fakeGraph>["processor"], left = 0.25, right = 0.75) {
  const samples = 4096;
  const output = new Float32Array(samples);
  processor.onaudioprocess?.({
    inputBuffer: { numberOfChannels: 2, getChannelData: (channel: number) => new Float32Array(samples).fill(channel === 0 ? left : right) },
    outputBuffer: { numberOfChannels: 1, getChannelData: () => output },
  });
  return output;
}

describe("WebAudioGainAdapter", () => {
  afterEach(() => { vi.restoreAllMocks(); });

  it("does not create a graph for unverified routes or mutate the existing source", async () => {
    const audio = document.createElement("audio");
    audio.src = "https://remote.invalid/song.mp3";
    const fake = fakeGraph();
    const adapter = new WebAudioGainAdapter(fake.factory);
    expect(await adapter.attach(audio, "unsupported")).toMatchObject({ status: "unsupported" });
    expect(fake.context.createMediaElementSource).not.toHaveBeenCalled();
    expect(audio.crossOrigin).toBeNull();
    expect(adapter.setGainDb(4)).toBe(false);

    expect(await adapter.attach(audio, "same-origin-relay")).toMatchObject({ status: "unsupported" });
    expect(fake.context.createMediaElementSource).not.toHaveBeenCalled();
    expect(audio.crossOrigin).toBeNull();
  });

  it("marks CORS before source assignment and separates normalization from user output volume", async () => {
    const audio = document.createElement("audio");
    audio.volume = 0.37;
    const fake = fakeGraph();
    fake.context.createMediaElementSource.mockImplementation(() => {
      expect(audio.crossOrigin).toBe("anonymous");
      expect(audio.getAttribute("src")).toBeNull();
      expect(fake.gain.gain.value).toBeCloseTo(10 ** (-3 / 20));
      expect(fake.userVolume.gain.value).toBe(0.37);
      return fake.source;
    });
    const adapter = new WebAudioGainAdapter(fake.factory);
    expect(await adapter.attach(audio, "same-origin-relay", -3, 0.37)).toEqual({ status: "supported" });
    expect(audio.volume).toBe(1);
    expect(fake.gain.gain.value).toBeCloseTo(10 ** (-3 / 20));
    expect(inspectWebAudioGainGraph(audio)).toEqual({ attached: true, gainLinear: 10 ** (-3 / 20), userVolumeLinear: 0.37, contextState: "running", contextTimeSeconds: 12 });
    expect(fake.source.connect).toHaveBeenCalledWith(fake.gain);
    expect(fake.gain.connect).toHaveBeenCalledWith(fake.analyser);
    expect(fake.analyser.connect).toHaveBeenCalledWith(fake.userVolume);
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.context.destination);
    expect(fake.context.resume).toHaveBeenCalledOnce();
    expect(adapter.setGainDb(6, 200)).toBe(true);
    expect(fake.calls).toEqual([["cancel", 12], ["set", 10 ** (-3 / 20), 12], ["ramp", 10 ** (6 / 20), 12.2]]);
    expect(adapter.setUserVolume(0, 0)).toBe(true);
    expect(fake.userParam.value).toBe(0);
    expect(fake.gain.gain.value).toBeCloseTo(10 ** (6 / 20));
    expect(audio.volume).toBe(1);
    expect(adapter.resetToUnity(0)).toBe(true);
    expect(fake.userParam.value).toBe(0);
  });

  it("keeps analysis upstream of the user-volume control, including at mute", async () => {
    const audio = document.createElement("audio");
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 12.5 });
    Object.defineProperty(audio, "readyState", { configurable: true, value: 4 });
    const fake = fakeGraph();
    const adapter = new WebAudioGainAdapter(fake.factory);
    expect((await adapter.attach(audio, "same-origin-relay", -6, 0.36)).status).toBe("supported");
    expect(fake.gain.connect).toHaveBeenCalledWith(fake.analyser);
    expect(fake.analyser.connect).toHaveBeenCalledWith(fake.userVolume);
    expect(adapter.setUserVolume(0, 0)).toBe(true);
    const observed = readWebAudioPeakWindow(audio);
    expect(observed).toMatchObject({ endPositionSeconds: 12.5, min: 0.5, max: 0.5 });
    expect(inspectWebAudioGainGraph(audio)).toMatchObject({ gainLinear: 10 ** (-6 / 20), userVolumeLinear: 0 });
  });

  it("branches the finite stereo diagnostic after user volume without replacing the audible destination", async () => {
    const audio = document.createElement("audio");
    const fake = fakeGraph();
    const adapter = new WebAudioGainAdapter(fake.factory);
    expect((await adapter.attach(audio, "same-origin-relay", -4, 0.36)).status).toBe("supported");

    const session = await startWebAudioStereoDiagnosticTap(audio, 0.05);
    expect(session).not.toBeNull();
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.context.destination);
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.processor);
    expect(fake.gain.connect).not.toHaveBeenCalledWith(fake.processor);
    expect(fake.processor.connect).toHaveBeenCalledWith(fake.context.createGain.mock.results[2]?.value);
    expect(fake.context.createGain.mock.results[2]?.value.gain.value).toBe(0);

    const left = Float32Array.from({ length: 4096 }, (_, index) => index / 4096);
    const right = Float32Array.from({ length: 4096 }, (_, index) => -index / 4096);
    const output = [new Float32Array(4096).fill(1), new Float32Array(4096).fill(1)];
    fake.processor.onaudioprocess?.({
      inputBuffer: { numberOfChannels: 2, getChannelData: channel => channel === 0 ? left : right },
      outputBuffer: { numberOfChannels: 2, getChannelData: channel => output[channel]! },
    });
    const capture = await session!.capture;
    expect(capture).toMatchObject({ status: "complete", sampleRate: 48_000, channels: 2, frames: 2400 });
    expect(capture.samples?.[2]).toBeCloseTo(left[1]!);
    expect(capture.samples?.[3]).toBeCloseTo(right[1]!);
    expect(output.every(channel => channel.every(value => value === 0))).toBe(true);
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.context.destination);
  });

  it.each([
    ["seeking", { seeking: true, readyState: 4 }],
    ["insufficient media data", { seeking: false, readyState: 1 }],
  ] as const)("does not timestamp a stale analyser window while %s", async (_reason, state) => {
    const audio = document.createElement("audio");
    Object.defineProperty(audio, "currentTime", { configurable: true, value: 42 });
    Object.defineProperty(audio, "seeking", { configurable: true, value: state.seeking });
    Object.defineProperty(audio, "readyState", { configurable: true, value: state.readyState });
    const fake = fakeGraph();
    const adapter = new WebAudioGainAdapter(fake.factory);
    expect((await adapter.attach(audio, "same-origin-relay")).status).toBe("supported");
    expect(readWebAudioPeakWindow(audio)).toBeNull();
    expect(fake.analyser.getFloatTimeDomainData).not.toHaveBeenCalled();
  });

  it("mutes the media element during an active graph handoff before restoring unity", async () => {
    const audio = document.createElement("audio");
    Object.defineProperty(audio, "paused", { configurable: true, value: false });
    audio.volume = 0.36;
    const volumeSetter = vi.spyOn(audio, "volume", "set");
    const fake = fakeGraph();
    fake.source.connect.mockImplementation(() => {
      expect(audio.volume).toBe(0);
      expect(fake.userVolume.gain.value).toBe(0.36);
    });
    expect((await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay", 0, 0.36)).status).toBe("supported");
    expect(volumeSetter.mock.calls.map(([value]) => value)).toEqual([0, 1]);
    expect(audio.volume).toBe(1);
  });

  it("passively reports absence or current graph state without creating a route", async () => {
    const audio = document.createElement("audio");
    expect(inspectWebAudioGainGraph(audio)).toEqual({ attached: false, gainLinear: null, userVolumeLinear: null, contextState: null, contextTimeSeconds: null });
    const fake = fakeGraph();
    expect(await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay", -6)).toMatchObject({ status: "supported" });
    expect(inspectWebAudioGainGraph(audio)).toMatchObject({ attached: true, contextState: "running", contextTimeSeconds: 12, userVolumeLinear: 1 });
    expect(fake.context.createMediaElementSource).toHaveBeenCalledOnce();
  });

  it("reuses the element's only MediaElementSource across adapters", async () => {
    const audio = document.createElement("audio");
    const fake = fakeGraph();
    expect(hasWebAudioGainGraph(audio)).toBe(false);
    const first = new WebAudioGainAdapter(fake.factory);
    const second = new WebAudioGainAdapter(fake.factory);
    expect((await first.attach(audio, "same-origin-relay", 0, 0.42)).status).toBe("supported");
    expect((await second.attach(audio, "same-origin-relay", -2)).status).toBe("supported");
    expect(fake.context.createMediaElementSource).toHaveBeenCalledOnce();
    expect(hasWebAudioGainGraph(audio)).toBe(true);
    expect(fake.userParam.value).toBe(0.42);
    expect(audio.volume).toBe(1);
    expect(second.setGainDb(-3, 0)).toBe(true);
    expect(inspectWebAudioGainGraph(audio).userVolumeLinear).toBe(0.42);
  });

  it("retires a discarded element graph without closing the replacement element graph", async () => {
    const oldAudio = document.createElement("audio");
    const currentAudio = document.createElement("audio");
    const oldFake = fakeGraph();
    const currentFake = fakeGraph();
    const oldAdapter = new WebAudioGainAdapter(oldFake.factory);
    const currentAdapter = new WebAudioGainAdapter(currentFake.factory);
    expect((await oldAdapter.attach(oldAudio, "same-origin-relay")).status).toBe("supported");
    expect((await currentAdapter.attach(currentAudio, "same-origin-relay")).status).toBe("supported");
    expect(hasWebAudioGainGraph(oldAudio)).toBe(true);
    expect(hasWebAudioGainGraph(currentAudio)).toBe(true);

    retireWebAudioGainGraph(oldAudio);

    expect(hasWebAudioGainGraph(oldAudio)).toBe(false);
    expect(hasWebAudioGainGraph(currentAudio)).toBe(true);
    expect(oldFake.source.disconnect).toHaveBeenCalledOnce();
    expect(oldFake.context.close).toHaveBeenCalledOnce();
    expect(currentFake.source.disconnect).not.toHaveBeenCalled();
    expect(currentFake.context.close).not.toHaveBeenCalled();
    expect(oldAdapter.setGainDb(-3, 0)).toBe(false);
    expect(currentAdapter.setGainDb(-3, 0)).toBe(true);
    expect(currentFake.param.value).toBeCloseTo(10 ** (-3 / 20));
  });

  it("reports Web Audio construction errors as unsupported output capability", async () => {
    const adapter = new WebAudioGainAdapter(() => { throw new Error("no output route"); });
    expect(await adapter.attach(document.createElement("audio"), "same-origin-relay"))
      .toEqual({ status: "failed", reason: "no output route" });
  });

  it("uses a passive ScriptProcessor fallback and emits continuous mono 22.05 kHz batches", async () => {
    const audio = document.createElement("audio");
    audio.volume = 0.42;
    const fake = fakeGraph();
    expect((await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay")).status).toBe("supported");
    const events: { kind: string; batch?: { sequence: number; sampleRate: number; samples: Float32Array } }[] = [];
    const result = await attachWebAudioPcmTap(audio, event => { events.push(event); });
    expect(result.tap).not.toBeNull();
    expect(fake.context.createScriptProcessor).toHaveBeenCalledWith(4096, 2, 1);
    expect(fake.gain.connect).toHaveBeenCalledWith(fake.processor);
    expect(fake.processor.connect).toHaveBeenCalled();
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.context.destination);
    for (let index = 0; index < 12; index++) processFallbackBlock(fake.processor);
    await Promise.resolve();
    const first = events.find(event => event.kind === "batch")?.batch;
    expect(first).toBeDefined();
    expect(first).toMatchObject({ sequence: 0, sampleRate: 22_050 });
    expect(first?.samples).toHaveLength(22_050);
    expect(first?.samples.every(sample => Math.abs(sample - 0.5) < 1e-6)).toBe(true);
    expect(result.tap?.stop).toBeTypeOf("function");
    expect(audio.volume).toBe(1);
    expect(fake.userParam.value).toBe(0.42);
    expect(fake.param.value).toBe(1);
    result.tap?.stop();
    expect(fake.gain.disconnect).toHaveBeenCalledWith(fake.processor);
    expect(fake.processor.onaudioprocess).toBeNull();
    expect(fake.processor.disconnect).toHaveBeenCalledOnce();
  });

  it("falls back to ScriptProcessor when AudioWorklet initialization fails", async () => {
    const audio = document.createElement("audio");
    const fake = fakeGraph();
    Object.assign(fake.context, { audioWorklet: { addModule: vi.fn(async () => { throw new Error("worklet denied"); }) } });
    const createDescriptor = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
    const revokeDescriptor = Object.getOwnPropertyDescriptor(URL, "revokeObjectURL");
    vi.stubGlobal("AudioWorkletNode", class {});
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => "blob:observer") });
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
    try {
      expect((await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay")).status).toBe("supported");
      const result = await attachWebAudioPcmTap(audio, () => {});
      expect(result.tap).not.toBeNull();
      expect(fake.context.createScriptProcessor).toHaveBeenCalledOnce();
      result.tap?.stop();
    } finally {
      if (createDescriptor) Object.defineProperty(URL, "createObjectURL", createDescriptor);
      else Reflect.deleteProperty(URL, "createObjectURL");
      if (revokeDescriptor) Object.defineProperty(URL, "revokeObjectURL", revokeDescriptor);
      else Reflect.deleteProperty(URL, "revokeObjectURL");
      vi.unstubAllGlobals();
    }
  });

  it("reports a bounded gap when a ScriptProcessor batch consumer falls behind", async () => {
    const audio = document.createElement("audio");
    const fake = fakeGraph();
    expect((await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay")).status).toBe("supported");
    const events: { kind: string; reason?: string }[] = [];
    let releaseBatch!: () => void;
    const result = await attachWebAudioPcmTap(audio, event => {
      events.push(event.kind === "gap" ? { kind: event.kind, reason: event.reason } : { kind: event.kind });
      if (event.kind === "batch") return new Promise<void>(resolve => { releaseBatch = resolve; });
    });
    for (let index = 0; index < 24; index++) processFallbackBlock(fake.processor);
    expect(events.filter(event => event.kind === "batch")).toHaveLength(1);
    expect(events.filter(event => event.kind === "gap")).toEqual([expect.objectContaining({ reason: expect.stringContaining("bounded backpressure queue") })]);
    expect(fake.processor.onaudioprocess).toBeNull();
    releaseBatch();
    result.tap?.stop();
  });

  it("flushes a short final PCM batch and retires only the observer branch", async () => {
    const audio = document.createElement("audio");
    const fake = fakeGraph();
    expect((await new WebAudioGainAdapter(fake.factory).attach(audio, "same-origin-relay")).status).toBe("supported");
    const events: { kind: string; batch?: { sequence: number; samples: Float32Array } }[] = [];
    const result = await attachWebAudioPcmTap(audio, event => { events.push(event); });
    for (let index = 0; index < 3; index++) processFallbackBlock(fake.processor, 0.2, 0.2);
    await result.tap?.finish();
    expect(events).toHaveLength(1);
    expect(events[0]?.kind).toBe("batch");
    expect(events[0]?.batch?.sequence).toBe(0);
    expect(events[0]?.batch?.samples.length).toBeGreaterThan(0);
    expect(events[0]?.batch?.samples.length).toBeLessThan(22_050);
    expect(fake.source.connect).toHaveBeenCalledWith(fake.gain);
    expect(fake.analyser.connect).toHaveBeenCalledWith(fake.userVolume);
    expect(fake.userVolume.connect).toHaveBeenCalledWith(fake.context.destination);
    expect(fake.gain.disconnect).toHaveBeenCalledWith(fake.processor);
    expect(fake.processor.disconnect).toHaveBeenCalledOnce();
  });

  it("encodes bounded WebView PCM as finite little-endian int16 samples", () => {
    const binary = atob(encodeWebAudioPcm16Base64(new Float32Array([-2, -1, 0, 0.5, 1, Number.NaN])));
    const bytes = Uint8Array.from(binary, character => character.charCodeAt(0));
    const view = new DataView(bytes.buffer);
    expect(bytes).toHaveLength(12);
    expect([0, 1, 2, 3, 4, 5].map(index => view.getInt16(index * 2, true))).toEqual([-32767, -32767, 0, 16384, 32767, 0]);
  });
});
