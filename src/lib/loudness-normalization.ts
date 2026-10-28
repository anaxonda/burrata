/** Constant playback gain policy and the Web Audio output adapter.
 *
 * This module does not inspect or guess at CORS. The caller must pass a
 * backend-confirmed same-origin route capability for the exact resolved
 * resource before attaching an audio element.
 */

import { startWebviewStereoDiagnosticTap, type StereoDiagnosticSession } from "./webview-stereo-diagnostic";

export type NormalizationMode = "off" | "track" | "album";
export type LoudnessStatus = "off" | "applied" | "album_fallback_track" | "unavailable";
export type LoudnessSettings = {
  mode: NormalizationMode;
  targetLufs: number;
  ceilingDbtp: number;
  maximumBoostDb: number;
  albumTrackFallback: boolean;
  analyzeWhenIdle: boolean;
};
export type LoudnessResourceIdentity = {
  scope: "track" | "album";
  members: Array<{
    assetId: string;
    fileHash: string;
    range: { startSeconds: number; endSeconds: number | null };
  }>;
  interTrackGapsSeconds: number[];
  resourceKey: string;
};
export type LoudnessAnalysisJobStatus = {
  jobId: string;
  status: "queued" | "running" | "completed" | "cancelled" | "failed" | string;
  total: number;
  completed: number;
  error: string | null;
  resource?: LoudnessResourceIdentity | null;
};
export type LoudnessPlaybackStatus = {
  mode?: NormalizationMode;
  appliedGainDb: number;
  status: LoudnessStatus;
  reason?: string | null;
  sourceResourceKey?: string | null;
  measurementSource?: string | null;
};

export function loudnessMeasurementSourceLabel(source?: string | null): string {
  const labels: Record<string, string> = {
    ffmpegAnalysis: "Audio analysis",
    replayGainTrack: "ReplayGain track tag",
    replayGainAlbum: "ReplayGain album tag",
    opusR128Track: "Opus track tag",
    opusR128Album: "Opus album tag",
  };
  if (source && Object.prototype.hasOwnProperty.call(labels, source)) return labels[source]!;
  if (source && Object.values(labels).includes(source)) return source;
  return "Audio measurement";
}

type AudioParamLike = {
  value: number;
  cancelScheduledValues(time: number): void;
  setValueAtTime(value: number, time: number): void;
  linearRampToValueAtTime(value: number, endTime: number): void;
};
type GainNodeLike = { gain: AudioParamLike; connect(destination: unknown): void; disconnect?(destination?: unknown): void };
type AnalyserNodeLike = {
  fftSize: number;
  smoothingTimeConstant?: number;
  connect(destination: unknown): void;
  disconnect?(destination?: unknown): void;
  getFloatTimeDomainData(data: Float32Array): void;
};
type AudioContextLike = {
  currentTime: number;
  state?: string;
  sampleRate?: number;
  destination: unknown;
  createMediaElementSource(element: HTMLAudioElement): { connect(destination: unknown): void; disconnect?(destination?: unknown): void };
  createGain(): GainNodeLike;
  createAnalyser?(): AnalyserNodeLike;
  audioWorklet?: { addModule(url: string): Promise<void> };
  createGain(): GainNodeLike;
  resume?(): Promise<void>;
  close?(): Promise<void>;
};
type ScriptProcessorNodeLike = {
  onaudioprocess: ((event: ScriptProcessorAudioEventLike) => void) | null;
  connect(destination: unknown): void;
  disconnect?(destination?: unknown): void;
};
type ScriptProcessorAudioBufferLike = {
  numberOfChannels: number;
  getChannelData(channel: number): Float32Array;
};
type ScriptProcessorAudioEventLike = { inputBuffer: ScriptProcessorAudioBufferLike; outputBuffer: ScriptProcessorAudioBufferLike };
export type WebAudioRouteStatus = "supported" | "unsupported" | "failed";
export type WebAudioAttachResult = { status: WebAudioRouteStatus; reason?: string };
export type NormalizationRoute = "same-origin-relay" | "unsupported";
export type WebAudioContextFactory = () => AudioContextLike;

type Graph = { context: AudioContextLike; source: { connect(destination: unknown): void; disconnect?(destination?: unknown): void }; gain: GainNodeLike; userVolume: GainNodeLike; analyser?: AnalyserNodeLike; observationBuffer?: Float32Array };
const graphs = new WeakMap<HTMLAudioElement, Graph>();

function setGainParam(param: AudioParamLike, value: number, now: number, rampMs: number): void {
  param.cancelScheduledValues(now);
  if (rampMs === 0) param.setValueAtTime(value, now);
  else {
    param.setValueAtTime(param.value, now);
    param.linearRampToValueAtTime(value, now + rampMs / 1000);
  }
}

/** Set user playback volume on the exact element's graph, if attached. */
export function setWebAudioUserVolume(audio: HTMLAudioElement, volume: number, rampMs = 30): boolean {
  const graph = graphs.get(audio);
  if (!graph || !Number.isFinite(volume) || volume < 0 || volume > 1 || !Number.isFinite(rampMs) || rampMs < 0 || rampMs > 2000) return false;
  setGainParam(graph.userVolume.gain, volume, graph.context.currentTime, rampMs);
  return true;
}

export type WebAudioPcmBatch = { sequence: number; sampleRate: 22050; samples: Float32Array };
export type WebAudioPcmTapEvent = { kind: "batch"; batch: WebAudioPcmBatch } | { kind: "gap"; reason: string };
export type WebAudioPcmTap = { finish(): Promise<void>; stop(): void };

export function encodeWebAudioPcm16Base64(samples: Float32Array): string {
  const bytes = new Uint8Array(samples.length * 2);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < samples.length; index++) {
    const value = Number.isFinite(samples[index]) ? Math.max(-1, Math.min(1, samples[index]!)) : 0;
    view.setInt16(index * 2, Math.round(value * 32767), true);
  }
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

const PCM_OBSERVER_PROCESSOR = `
class BurrataObservedPcm extends AudioWorkletProcessor {
  constructor() {
    super(); this.enabled = false; this.targetRate = 22050; this.outputTimeUnits = 0; this.weightedSum = 0;
    this.buffer = new Float32Array(22050); this.offset = 0; this.sequence = 0;
    this.awaitingAck = false; this.active = true; this.finishPending = false;
    this.port.onmessage = event => {
      if (event.data === "start") this.enabled = true;
      if (event.data === "stop") this.active = false;
      if (event.data === "finish") {
        this.enabled = false; this.finishPending = true;
        if (!this.awaitingAck) this.flushFinal();
      }
      if (event.data === "ack") {
        this.awaitingAck = false;
        if (this.finishPending) this.flushFinal();
      }
    };
  }
  flushFinal() {
    if (this.offset) {
      if (this.awaitingAck) { this.port.postMessage({kind:"gap",reason:"WebAudio PCM observer exceeded its bounded backpressure queue"}); this.active = false; return; }
      const samples = this.buffer.slice(0,this.offset); this.offset = 0; this.awaitingAck = true;
      this.port.postMessage({kind:"batch",sequence:this.sequence++,samples,final:true},[samples.buffer]);
    } else { this.port.postMessage({kind:"finished"}); this.active = false; }
  }
  pushSample(sample) {
    if (this.awaitingAck && this.offset === this.buffer.length) {
      this.port.postMessage({kind:"gap",reason:"WebAudio PCM observer exceeded its bounded backpressure queue"});
      this.active = false; return false;
    }
    this.buffer[this.offset++] = sample;
    if (this.offset === this.buffer.length) {
      if (this.awaitingAck) {
        this.port.postMessage({kind:"gap",reason:"WebAudio PCM observer exceeded its bounded backpressure queue"});
        this.active = false; return false;
      }
      const samples = this.buffer;
      this.buffer = new Float32Array(22050); this.offset = 0; this.awaitingAck = true;
      this.port.postMessage({kind:"batch",sequence:this.sequence++,samples},[samples.buffer]);
    }
    return true;
  }
  process(inputs, outputs) {
    for (const channel of outputs[0] || []) channel.fill(0);
    if (!this.active) return false;
    const channels = inputs[0] || [];
    if (!this.enabled || !channels.length) return true;
    const frames = channels[0].length;
    for (let frame = 0; frame < frames; frame++) {
      let mono = 0;
      for (const channel of channels) mono += channel[frame] || 0;
      mono = Math.max(-1, Math.min(1, mono / channels.length));
      // Area-average each input frame into the target-rate timeline. This
      // matches the native observer's anti-aliasing box filter instead of
      // linearly interpolating high-frequency energy into the key lane.
      let inputRemaining = this.targetRate;
      while (inputRemaining > 0) {
        const outputRemaining = sampleRate - this.outputTimeUnits;
        const overlap = Math.min(inputRemaining, outputRemaining);
        this.weightedSum += mono * overlap;
        this.outputTimeUnits += overlap;
        inputRemaining -= overlap;
        if (this.outputTimeUnits === sampleRate) {
          if (!this.pushSample(this.weightedSum / sampleRate)) return false;
          this.outputTimeUnits = 0; this.weightedSum = 0;
        }
      }
    }
    return true;
  }
}
registerProcessor("burrata-observed-pcm", BurrataObservedPcm);
`;

const LIVE_PCM_RATE = 22_050;
const LIVE_PCM_BATCH_SAMPLES = LIVE_PCM_RATE;
const SCRIPT_PROCESSOR_BUFFER_SIZE = 4096;

function attachScriptProcessorPcmTap(
  graph: Graph,
  context: AudioContextLike,
  onEvent: (event: WebAudioPcmTapEvent) => void | Promise<void>,
): { tap: WebAudioPcmTap | null; reason?: string } {
  const legacyContext = context as AudioContextLike & {
    createScriptProcessor?: (bufferSize?: number, numberOfInputChannels?: number, numberOfOutputChannels?: number) => ScriptProcessorNodeLike;
  };
  if (typeof legacyContext.createScriptProcessor !== "function") {
    return { tap: null, reason: "This WebView cannot observe decoded PCM with AudioWorklet or ScriptProcessor." };
  }
  const sampleRate = context.sampleRate;
  if (!Number.isFinite(sampleRate) || sampleRate! < 8_000 || sampleRate! > 192_000) {
    return { tap: null, reason: "The Web Audio sample rate is outside the PCM observer's supported range." };
  }
  let processor: ScriptProcessorNodeLike | null = null;
  let silentGain: GainNodeLike | null = null;
  let active = true;
  let enabled = true;
  let sequence = 0;
  let offset = 0;
  let outputTimeUnits = 0;
  let weightedSum = 0;
  let awaitingAck = false;
  let finishRequested = false;
  let finishPromise: Promise<void> | null = null;
  let finishResolve: (() => void) | null = null;
  let gapSent = false;
  const buffer = new Float32Array(LIVE_PCM_BATCH_SAMPLES);
  const detach = () => {
    if (!processor) return;
    processor.onaudioprocess = null;
    try { graph.gain.disconnect?.(processor); } catch {}
    try { processor.disconnect?.(); } catch {}
    try { silentGain?.disconnect?.(); } catch {}
  };
  const completeFinish = () => {
    detach();
    finishResolve?.(); finishResolve = null;
  };
  const failGap = (reason: string) => {
    if (!active) return;
    active = false;
    detach();
    if (!gapSent) {
      gapSent = true;
      try { void Promise.resolve(onEvent({ kind: "gap", reason })).catch(() => {}); } catch {}
    }
    finishResolve?.(); finishResolve = null;
  };
  const dispatch = (final: boolean) => {
    if (!active || awaitingAck || offset === 0) return;
    awaitingAck = true;
    const samples = buffer.slice(0, offset);
    offset = 0;
    const event: WebAudioPcmTapEvent = { kind: "batch", batch: { sequence: sequence++, sampleRate: LIVE_PCM_RATE, samples } };
    let result: void | Promise<void>;
    try { result = onEvent(event); }
    catch { failGap("The Web Audio PCM consumer could not accept a sample batch."); return; }
    void Promise.resolve(result).then(() => {
      awaitingAck = false;
      if (finishRequested) {
        if (offset) dispatch(true);
        else completeFinish();
      }
    }, () => failGap("The Web Audio PCM consumer could not accept a sample batch."));
    if (final) finishRequested = true;
  };
  const pushSample = (sample: number) => {
    if (awaitingAck && offset === buffer.length) {
      failGap("The Web Audio PCM observer exceeded its bounded backpressure queue.");
      return false;
    }
    buffer[offset++] = sample;
    if (offset === buffer.length) {
      if (awaitingAck) {
        failGap("The Web Audio PCM observer exceeded its bounded backpressure queue.");
        return false;
      }
      dispatch(false);
    }
    return true;
  };
  try {
    processor = legacyContext.createScriptProcessor.call(context, SCRIPT_PROCESSOR_BUFFER_SIZE, 2, 1);
    silentGain = context.createGain();
    silentGain.gain.value = 0;
    graph.gain.connect(processor);
    processor.connect(silentGain);
    silentGain.connect(context.destination);
    processor.onaudioprocess = event => {
      for (let channel = 0; channel < event.outputBuffer.numberOfChannels; channel++) event.outputBuffer.getChannelData(channel).fill(0);
      if (!active || !enabled) return;
      const channels = Math.min(2, Math.max(0, event.inputBuffer.numberOfChannels));
      if (!channels) return;
      const input = Array.from({ length: channels }, (_, channel) => event.inputBuffer.getChannelData(channel));
      const frames = input[0]!.length;
      for (let frame = 0; frame < frames; frame++) {
        let mono = 0;
        for (const samples of input) mono += samples[frame] || 0;
        mono = Math.max(-1, Math.min(1, mono / channels));
        let inputRemaining = LIVE_PCM_RATE;
        while (inputRemaining > 0) {
          const outputRemaining = sampleRate! - outputTimeUnits;
          const overlap = Math.min(inputRemaining, outputRemaining);
          weightedSum += mono * overlap;
          outputTimeUnits += overlap;
          inputRemaining -= overlap;
          if (outputTimeUnits === sampleRate) {
            if (!pushSample(weightedSum / sampleRate!)) return;
            outputTimeUnits = 0;
            weightedSum = 0;
          }
        }
      }
    };
  } catch {
    detach();
    return { tap: null, reason: "The Web Audio ScriptProcessor PCM observer could not start." };
  }
  return {
    tap: {
      finish() {
        if (finishPromise) return finishPromise;
        finishRequested = true;
        enabled = false;
        finishPromise = new Promise<void>(resolve => { finishResolve = resolve; });
        if (!awaitingAck) {
          if (offset) dispatch(true);
          else completeFinish();
        }
        return finishPromise;
      },
      stop() {
        if (!active) return;
        active = false;
        enabled = false;
        detach();
        finishResolve?.(); finishResolve = null;
      },
    },
  };
}

/** Attach an opt-in, bounded PCM observer to an existing trusted WebAudio
 * graph. Audio remains on the graph's original audible branch; the observer's
 * output is silent and its side channel holds at most two one-second
 * buffers before it reports a gap and stops. */
export async function attachWebAudioPcmTap(
  audio: HTMLAudioElement,
  onEvent: (event: WebAudioPcmTapEvent) => void | Promise<void>,
): Promise<{ tap: WebAudioPcmTap | null; reason?: string }> {
  const graph = graphs.get(audio);
  const context = graph?.context as (AudioContextLike & AudioContext) | undefined;
  if (!graph || !context) return { tap: null, reason: "No trusted Web Audio playback graph is attached to this source." };
  if (!context.audioWorklet || typeof AudioWorkletNode === "undefined") {
    return attachScriptProcessorPcmTap(graph, context, onEvent);
  }
  let moduleUrl: string | null = null;
  let worklet: AudioWorkletNode | null = null;
  let silentGain: GainNodeLike | null = null;
  try {
    moduleUrl = URL.createObjectURL(new Blob([PCM_OBSERVER_PROCESSOR], { type: "text/javascript" }));
    await context.audioWorklet.addModule(moduleUrl);
    worklet = new AudioWorkletNode(context, "burrata-observed-pcm", {
      numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1], channelCountMode: "max",
    });
    silentGain = context.createGain();
    silentGain.gain.value = 0;
    graph.gain.connect(worklet);
    worklet.connect(silentGain as unknown as AudioNode);
    silentGain.connect(context.destination);
    worklet.port.onmessage = async event => {
      const value = event.data as { kind?: unknown; sequence?: unknown; samples?: unknown; reason?: unknown };
      if (value.kind === "gap") {
        finishResolve?.(); finishResolve = null;
        onEvent({ kind: "gap", reason: typeof value.reason === "string" ? value.reason : "WebAudio PCM observation lost continuity." });
      } else if (value.kind === "finished") {
        finishResolve?.(); finishResolve = null;
      } else if (value.kind === "batch" && Number.isSafeInteger(value.sequence) && value.samples instanceof Float32Array) {
        try {
          await onEvent({ kind: "batch", batch: { sequence: value.sequence as number, sampleRate: 22050, samples: value.samples } });
          worklet?.port.postMessage("ack");
        } catch (failure) {
          onEvent({ kind: "gap", reason: failure instanceof Error ? failure.message : "WebAudio PCM capture did not keep up." });
          worklet?.port.postMessage("stop");
          finishResolve?.(); finishResolve = null;
        }
      }
    };
    worklet.port.postMessage("start");
    let stopped = false;
    let finishResolve: (() => void) | null = null;
    let finishPromise: Promise<void> | null = null;
    return {
      tap: {
        finish() {
          if (!finishPromise) {
            finishPromise = new Promise<void>(resolve => { finishResolve = resolve; });
            worklet?.port.postMessage("finish");
          }
          return finishPromise;
        },
        stop() {
          if (stopped) return;
          stopped = true;
          finishResolve?.(); finishResolve = null;
          worklet?.port.postMessage("stop");
          try { graph.gain.disconnect?.(worklet as unknown as AudioNode); } catch {}
          try { worklet?.disconnect?.(); } catch {}
          try { silentGain?.disconnect?.(); } catch {}
          if (moduleUrl) URL.revokeObjectURL(moduleUrl);
        },
      },
    };
  } catch {
    if (worklet) {
      try { graph.gain.disconnect?.(worklet as unknown as AudioNode); } catch {}
    }
    try { worklet?.disconnect?.(); } catch {}
    try { silentGain?.disconnect?.(); } catch {}
    if (moduleUrl) URL.revokeObjectURL(moduleUrl);
    const fallback = attachScriptProcessorPcmTap(graph, context, onEvent);
    if (fallback.tap) return fallback;
    return { tap: null, reason: fallback.reason ?? "Neither bounded Web Audio PCM observer could start." };
  }
}

export type WebAudioPeakWindow = {
  startPositionSeconds: number;
  endPositionSeconds: number;
  sampleRate: number;
  min: number;
  max: number;
};

/** True after the browser has permanently routed this element through WebAudio. */
export function hasWebAudioGainGraph(audio: HTMLAudioElement | null | undefined): boolean {
  return Boolean(audio && graphs.has(audio));
}

/** Start a finite diagnostic branch after the actual user-volume gain node.
 * This is deliberately separate from the ordinary playback and PCM-observer
 * paths; callers must apply their own synthetic-fixture authorization. */
export async function startWebAudioStereoDiagnosticTap(
  audio: HTMLAudioElement | null | undefined,
  durationSeconds = 2,
): Promise<StereoDiagnosticSession | null> {
  const graph = audio ? graphs.get(audio) : undefined;
  if (!graph) return null;
  return startWebviewStereoDiagnosticTap(
    graph.userVolume as unknown as AudioNode,
    graph.context as unknown as AudioContext,
    durationSeconds,
  );
}

/** Close the output graph for an audio element that the app is discarding. */
export function retireWebAudioGainGraph(audio: HTMLAudioElement | null | undefined): void {
  if (!audio) return;
  const graph = graphs.get(audio);
  if (!graph) return;
  graphs.delete(audio);
  try { graph.source.disconnect?.(); } catch { /* context may already be stopping */ }
  try { graph.gain.disconnect?.(); } catch { /* context may already be stopping */ }
  try { graph.analyser?.disconnect?.(); } catch { /* context may already be stopping */ }
  try { graph.userVolume.disconnect?.(); } catch { /* context may already be stopping */ }
  try {
    const closing = graph.context.close?.();
    if (closing) void closing.catch(() => {});
  } catch { /* teardown is best-effort */ }
}

export type WebAudioGainGraphInspection = {
  attached: boolean;
  gainLinear: number | null;
  userVolumeLinear: number | null;
  contextState: string | null;
  contextTimeSeconds: number | null;
};

/** Read the already-owned normalization graph without creating or changing it. */
export function inspectWebAudioGainGraph(audio: HTMLAudioElement | null | undefined): WebAudioGainGraphInspection {
  const graph = audio ? graphs.get(audio) : undefined;
  if (!graph) return { attached: false, gainLinear: null, userVolumeLinear: null, contextState: null, contextTimeSeconds: null };
  const gainLinear = graph.gain.gain.value;
  const userVolumeLinear = graph.userVolume.gain.value;
  const contextTimeSeconds = graph.context.currentTime;
  return {
    attached: true,
    gainLinear: Number.isFinite(gainLinear) && gainLinear >= 0 && gainLinear <= 2 ? gainLinear : null,
    userVolumeLinear: Number.isFinite(userVolumeLinear) && userVolumeLinear >= 0 && userVolumeLinear <= 1 ? userVolumeLinear : null,
    contextState: typeof graph.context.state === "string" && graph.context.state.length <= 32 ? graph.context.state : null,
    contextTimeSeconds: Number.isFinite(contextTimeSeconds) && contextTimeSeconds >= 0 ? contextTimeSeconds : null,
  };
}

export function hasWebAudioWaveformTap(audio: HTMLAudioElement | null | undefined): boolean {
  if (!audio) return false;
  const graph = graphs.get(audio);
  return Boolean(graph?.analyser && graph.observationBuffer);
}

/** Read one bounded time-domain window from an already-owned graph. This is
 * passive: it never creates or reconfigures the media source. Callers must
 * aggregate timed windows as partial observations and must not infer coverage
 * for intervals they did not sample.
 */
export function readWebAudioPeakWindow(audio: HTMLAudioElement | null | undefined): WebAudioPeakWindow | null {
  if (!audio) return null;
  const graph = graphs.get(audio);
  const sampleRate = graph?.context.sampleRate;
  const analyser = graph?.analyser;
  const buffer = graph?.observationBuffer;
  if (!graph || !analyser || !buffer || !Number.isFinite(sampleRate) || !sampleRate ||
      audio.seeking || audio.readyState < 2 || !Number.isFinite(audio.currentTime)) return null;
  analyser.getFloatTimeDomainData(buffer);
  let min = Infinity, max = -Infinity;
  for (const value of buffer) {
    if (!Number.isFinite(value)) continue;
    min = Math.min(min, value);
    max = Math.max(max, value);
  }
  if (!Number.isFinite(min) || !Number.isFinite(max)) return null;
  const duration = buffer.length / sampleRate;
  const endPositionSeconds = audio.currentTime;
  return {
    startPositionSeconds: Math.max(0, endPositionSeconds - duration),
    endPositionSeconds,
    sampleRate,
    min: Math.max(-1, min),
    max: Math.min(1, max),
  };
}

function defaultContextFactory(): AudioContextLike {
  const Constructor = globalThis.AudioContext;
  if (!Constructor) throw new Error("This WebView does not provide Web Audio.");
  return new Constructor();
}

/**
 * A MediaElementSource permanently routes that element through Web Audio.
 * Graphs are retained in a WeakMap and reused because the platform permits
 * only one MediaElementSourceNode for each media element.
 */
export class WebAudioGainAdapter {
  private audio: WeakRef<HTMLAudioElement> | null = null;
  constructor(private readonly createContext: WebAudioContextFactory = defaultContextFactory) {}

  private get graph(): Graph | null {
    const audio = this.audio?.deref();
    return audio ? graphs.get(audio) ?? null : null;
  }

  async attach(audio: HTMLAudioElement, route: NormalizationRoute, initialGainDb = 0, initialUserVolume?: number): Promise<WebAudioAttachResult> {
    if (route !== "same-origin-relay") return { status: "unsupported", reason: "This audio source has no verified same-origin normalization route." };
    if (!Number.isFinite(initialGainDb) || initialGainDb < -60 || initialGainDb > 6) {
      return { status: "failed", reason: "The initial normalization gain is invalid." };
    }
    const existing = graphs.get(audio);
    if (existing) {
      this.audio = new WeakRef(audio);
      // Called before the next media source is assigned. Set synchronously so
      // the new track cannot start with the previous track's gain.
      const userVolume = initialUserVolume ?? existing.userVolume.gain.value;
      if (!Number.isFinite(userVolume) || userVolume < 0 || userVolume > 1) return { status: "failed", reason: "The Web Audio user volume is invalid." };
      if (!this.setGainDb(initialGainDb, 0) || !this.setUserVolume(userVolume, 0)) return { status: "failed", reason: "The Web Audio gain could not be initialized." };
      audio.volume = 1;
      try { await existing.context.resume?.(); }
      catch { return { status: "failed", reason: "The Web Audio output could not be resumed." }; }
      return { status: "supported" };
    }

    const hasAssignedSource = Boolean(audio.currentSrc || audio.getAttribute("src"));
    if (hasAssignedSource && audio.crossOrigin !== "anonymous") {
      return { status: "unsupported", reason: "Normalization must be connected before this audio source is assigned." };
    }
    const userVolume = initialUserVolume ?? audio.volume;
    if (!Number.isFinite(userVolume) || userVolume < 0 || userVolume > 1) return { status: "failed", reason: "The Web Audio user volume is invalid." };
    const originalElementVolume = audio.volume;
    try {
      // This must precede src assignment, so the app relay's allowed-origin
      // response is used for CORS-enabled media-element decoding.
      if (!hasAssignedSource) audio.crossOrigin = "anonymous";
      const context = this.createContext();
      const gain = context.createGain();
      gain.gain.value = 10 ** (initialGainDb / 20);
      const userVolumeGain = context.createGain();
      userVolumeGain.gain.value = userVolume;
      const analyser = context.createAnalyser?.();
      if (analyser) {
        analyser.fftSize = 2048;
        if (analyser.smoothingTimeConstant !== undefined) analyser.smoothingTimeConstant = 0;
      }
      const source = context.createMediaElementSource(audio);
      // A playing relay can be upgraded after playback starts. Mute the media
      // element during the graph handoff, then leave all user level in the
      // initialized gain node so the transition cannot briefly spike.
      const mediaWasPlaying = !audio.paused;
      if (mediaWasPlaying) audio.volume = 0;
      else audio.volume = 1;
      source.connect(gain);
      if (analyser) {
        gain.connect(analyser);
        analyser.connect(userVolumeGain);
      } else {
        gain.connect(userVolumeGain);
      }
      userVolumeGain.connect(context.destination);
      const graph: Graph = {
        context,
        source,
        gain,
        userVolume: userVolumeGain,
        ...(analyser && context.sampleRate ? { analyser, observationBuffer: new Float32Array(analyser.fftSize) } : {}),
      };
      graphs.set(audio, graph);
      this.audio = new WeakRef(audio);
      audio.volume = 1;
      await context.resume?.();
      return { status: "supported" };
    } catch (failure) {
      audio.volume = graphs.has(audio) ? 1 : originalElementVolume;
      return {
        status: "failed",
        reason: failure instanceof Error ? failure.message : "The Web Audio output route could not be created.",
      };
    }
  }

  /** Schedule a short linear gain ramp without changing HTMLMediaElement.volume. */
  setGainDb(dbGain: number, rampMs = 150): boolean {
    const graph = this.graph;
    if (!graph || !Number.isFinite(dbGain) || dbGain < -60 || dbGain > 6 || !Number.isFinite(rampMs) || rampMs < 0 || rampMs > 2000) return false;
    const now = graph.context.currentTime;
    const param = graph.gain.gain;
    const linear = 10 ** (dbGain / 20);
    param.cancelScheduledValues(now);
    if (rampMs === 0) param.setValueAtTime(linear, now);
    else {
      param.setValueAtTime(param.value, now);
      param.linearRampToValueAtTime(linear, now + rampMs / 1000);
    }
    return true;
  }

  /** Set user playback volume on the output branch after analysis taps. */
  setUserVolume(volume: number, rampMs = 30): boolean {
    const graph = this.graph;
    if (!graph || !Number.isFinite(volume) || volume < 0 || volume > 1 || !Number.isFinite(rampMs) || rampMs < 0 || rampMs > 2000) return false;
    setGainParam(graph.userVolume.gain, volume, graph.context.currentTime, rampMs);
    return true;
  }

  /** Off mode is unity gain. The graph cannot be disconnected from a media
   * element once constructed, so callers retain it at unity for future plays.
   */
  resetToUnity(rampMs = 150): boolean { return this.setGainDb(0, rampMs); }
}
