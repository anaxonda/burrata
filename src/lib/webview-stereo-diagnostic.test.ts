// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { encodeStereoDiagnosticSamples, MAX_STEREO_DIAGNOSTIC_SECONDS, startWebviewStereoDiagnosticTap, stereoDiagnosticCompletionArguments } from "./webview-stereo-diagnostic";

type FakeNode = {
  connections: unknown[];
  disconnected: number;
  connect(target: unknown): void;
  disconnect(target?: unknown): void;
};

function fakeNode(): FakeNode {
  return {
    connections: [], disconnected: 0,
    connect(target) { this.connections.push(target); },
    disconnect() { this.disconnected++; },
  };
}

function contextWithScriptProcessor(sampleRate: number, channelCount = 2) {
  const source = fakeNode();
  const destination = fakeNode();
  const processor = Object.assign(fakeNode(), { onaudioprocess: null as ((event: AudioProcessingEvent) => void) | null });
  const gains: Array<ReturnType<typeof fakeNode> & { gain: { setValueAtTime: ReturnType<typeof vi.fn> } }> = [];
  const context = {
    sampleRate, currentTime: 2, destination,
    createGain() {
      const node = Object.assign(fakeNode(), { gain: { setValueAtTime: vi.fn() } });
      gains.push(node);
      return node;
    },
    createScriptProcessor: vi.fn(() => processor),
  };
  return { source, destination, processor, gains, context, channelCount };
}

function audioEvent(left: Float32Array, right: Float32Array, outputFrames = left.length): AudioProcessingEvent {
  const inputChannels = [left, right];
  const outputChannels = [new Float32Array(outputFrames).fill(0.75), new Float32Array(outputFrames).fill(-0.75)];
  return {
    playbackTime: 3,
    inputBuffer: {
      numberOfChannels: inputChannels.length,
      getChannelData: (index: number) => inputChannels[index]!,
    },
    outputBuffer: {
      numberOfChannels: outputChannels.length,
      getChannelData: (index: number) => outputChannels[index]!,
    },
  } as unknown as AudioProcessingEvent;
}

describe("bounded WebView stereo diagnostic tap", () => {
  it("captures post-gain L/R samples in order, caps duration, and keeps its side branch silent", async () => {
    const fixture = contextWithScriptProcessor(10_000);
    const session = await startWebviewStereoDiagnosticTap(fixture.source as unknown as AudioNode, fixture.context as unknown as AudioContext, 0.1);
    expect(fixture.context.createScriptProcessor).toHaveBeenCalledWith(4096, 2, 2);
    expect(fixture.source.connections).toContain(fixture.processor);
    expect(fixture.processor.connections).toContain(fixture.gains[0]);
    expect(fixture.gains[0]?.connections).toContain(fixture.destination);
    expect(fixture.gains[0]?.gain.setValueAtTime).toHaveBeenCalledWith(0, 2);

    const left = Float32Array.from({ length: 1024 }, (_, i) => i / 2048);
    const right = Float32Array.from({ length: 1024 }, (_, i) => -i / 1024);
    const event = audioEvent(left, right);
    fixture.processor.onaudioprocess!(event);
    const result = await session.capture;

    expect(result).toMatchObject({ status: "complete", sampleRate: 10_000, channels: 2, frames: 1000, firstFrame: 30_000, firstContextTime: 3 });
    expect(result.samples).toHaveLength(2000);
    expect(result.samples?.[0]).toBe(0);
    expect(result.samples?.[1]).toBeCloseTo(0);
    expect(result.samples?.[2]).toBeCloseTo(left[1]!);
    expect(result.samples?.[3]).toBeCloseTo(right[1]!);
    expect(result.samples?.[1998]).toBeCloseTo(left[999]!);
    expect(result.samples?.[1999]).toBeCloseTo(right[999]!);
    expect(event.outputBuffer.getChannelData(0).every(value => value === 0)).toBe(true);
    expect(event.outputBuffer.getChannelData(1).every(value => value === 0)).toBe(true);
    expect(fixture.processor.disconnected).toBe(1);
    expect(fixture.gains[0]?.disconnected).toBe(1);
  });

  it("reports a channel discontinuity rather than duplicating or downmixing", async () => {
    const fixture = contextWithScriptProcessor(10_000);
    const session = await startWebviewStereoDiagnosticTap(fixture.source as unknown as AudioNode, fixture.context as unknown as AudioContext, 0.1);
    const mono = new Float32Array(64).fill(0.2);
    fixture.processor.onaudioprocess!({
      playbackTime: 3,
      inputBuffer: { numberOfChannels: 1, getChannelData: () => mono },
      outputBuffer: { numberOfChannels: 2, getChannelData: (index: number) => new Float32Array(64).fill(index + 1) },
    } as unknown as AudioProcessingEvent);
    expect(await session.capture).toMatchObject({ status: "gap", samples: null, frames: 0 });
  });

  it("uses an explicit two-channel silent AudioWorklet branch when available", async () => {
    const source = fakeNode();
    const destination = fakeNode();
    const gains: Array<ReturnType<typeof fakeNode> & { gain: { setValueAtTime: ReturnType<typeof vi.fn> } }> = [];
    const addModule = vi.fn(async () => {});
    const context = {
      sampleRate: 48_000, currentTime: 4, destination,
      audioWorklet: { addModule },
      createGain() {
        const node = Object.assign(fakeNode(), { gain: { setValueAtTime: vi.fn() } });
        gains.push(node);
        return node;
      },
      createScriptProcessor: vi.fn(() => { throw new Error("fallback should not run"); }),
    };
    type WorkletStub = {
      port: { onmessage: ((event: { data: unknown }) => void) | null; postMessage: ReturnType<typeof vi.fn> };
      options: AudioWorkletNodeOptions;
      connections: unknown[];
      disconnected: number;
      connect(target: unknown): void;
      disconnect(target?: unknown): void;
    };
    const createdHolder: { instance?: WorkletStub } = {};
    class FakeWorkletNode {
      connections: unknown[] = [];
      disconnected = 0;
      port = { onmessage: null as ((event: { data: unknown }) => void) | null, postMessage: vi.fn() };
      constructor(_context: unknown, _name: string, public options: AudioWorkletNodeOptions) { createdHolder.instance = this; }
      connect(target: unknown) { this.connections.push(target); }
      disconnect() { this.disconnected++; }
    }
    vi.stubGlobal("AudioWorkletNode", FakeWorkletNode);
    vi.stubGlobal("URL", { createObjectURL: vi.fn(() => "blob:stereo-probe"), revokeObjectURL: vi.fn() });

    const session = await startWebviewStereoDiagnosticTap(source as unknown as AudioNode, context as unknown as AudioContext, 0.1);
    const created = createdHolder.instance;
    if (!created) throw new Error("The AudioWorklet node was not created");
    expect(addModule).toHaveBeenCalledOnce();
    expect(context.createScriptProcessor).not.toHaveBeenCalled();
    expect(created.options).toMatchObject({ numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [2], channelCount: 2, channelCountMode: "explicit" });
    expect(source.connections).toContain(created);
    expect(created.connections).toContain(gains[0]);
    expect(gains[0]?.gain.setValueAtTime).toHaveBeenCalledWith(0, 4);
    const samples = new Float32Array([0.1, -0.2, 0.3, -0.4]);
    created.port.onmessage?.({ data: { kind: "complete", sampleRate: 48_000, channels: 2, frames: 2, firstFrame: 192_000, firstContextTime: 4, samples } });
    expect(await session.capture).toMatchObject({ status: "complete", sampleRate: 48_000, channels: 2, frames: 2, firstFrame: 192_000, firstContextTime: 4 });
    expect((await session.capture).samples).toEqual(new Float32Array([0.1, -0.2, 0.3, -0.4]));
    vi.unstubAllGlobals();
  });

  it("enforces the four-second limit and cancels cleanly", async () => {
    const fixture = contextWithScriptProcessor(48_000);
    const tooLong = await startWebviewStereoDiagnosticTap(fixture.source as unknown as AudioNode, fixture.context as unknown as AudioContext, MAX_STEREO_DIAGNOSTIC_SECONDS + 0.01);
    expect(await tooLong.capture).toMatchObject({ status: "unavailable", frames: 0 });
    expect(fixture.context.createScriptProcessor).not.toHaveBeenCalled();

    const session = await startWebviewStereoDiagnosticTap(fixture.source as unknown as AudioNode, fixture.context as unknown as AudioContext, 0.1);
    session.stop();
    expect(await session.capture).toMatchObject({ status: "cancelled", samples: null, frames: 0 });
    expect(fixture.processor.disconnected).toBe(1);
    expect(fixture.gains[0]?.disconnected).toBe(1);
  });

  it("encodes interleaved PCM as bounded little-endian float data", () => {
    const encoded = encodeStereoDiagnosticSamples(new Float32Array([0.5, -0.25]));
    const bytes = Uint8Array.from(atob(encoded), character => character.charCodeAt(0));
    expect(bytes).toHaveLength(8);
    const view = new DataView(bytes.buffer);
    expect(view.getFloat32(0, true)).toBe(0.5);
    expect(view.getFloat32(4, true)).toBe(-0.25);
    expect(() => encodeStereoDiagnosticSamples(new Float32Array([Number.NaN]))).toThrow(/finite PCM/);
  });

  it("projects the App completion into separate Tauri identity fields and nested capture data", () => {
    const identity = {
      requestId: "webview-stereo-diagnostic:request",
      trackId: "local:synthetic",
      sessionId: "fixture-session",
      sourceGeneration: 7,
      seekSerial: 2,
      currentIndex: 1,
      queueTotal: 2,
      entryId: "fixture-entry-1",
      queueTrackId: "local:synthetic",
    };
    const capture = { status: "complete", frames: 96_000, sourceGeneration: 7, samplesBase64: "AAAA" };
    const args = stereoDiagnosticCompletionArguments(identity, capture);
    expect(args).toEqual({ ...identity, capture });
    expect(args.capture).toBe(capture);
    expect(args).not.toHaveProperty("payload");
  });
});
