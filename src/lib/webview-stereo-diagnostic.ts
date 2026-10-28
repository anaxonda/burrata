/**
 * Bounded diagnostic-only tap for comparing decoded Web Audio output with a
 * downstream OS audio capture. The caller supplies the final user-volume node
 * immediately before AudioContext.destination. Its normal destination branch
 * remains connected; this helper adds only a zero-filled side branch.
 *
 * This module is deliberately not wired into the application UI. Callers must
 * opt in for a specific fixture/source and keep the capture result local.
 */

export const MAX_STEREO_DIAGNOSTIC_SECONDS = 4;
const MIN_DIAGNOSTIC_SECONDS = 0.05;
const SCRIPT_PROCESSOR_BUFFER_SIZE = 4096;

export type StereoDiagnosticCapture = {
  status: "complete" | "cancelled" | "unavailable" | "gap";
  sampleRate: number;
  channels: 2;
  frames: number;
  firstFrame: number | null;
  firstContextTime: number | null;
  /** Interleaved Float32 samples in frame-major L,R order. */
  samples: Float32Array | null;
  reason?: string;
};

export type StereoDiagnosticSession = {
  capture: Promise<StereoDiagnosticCapture>;
  stop(): void;
};

export type StereoDiagnosticPlaybackIdentity = {
  requestId: string;
  trackId: string;
  sessionId: string;
  sourceGeneration: number;
  seekSerial: number;
  currentIndex: number;
  queueTotal: number;
  entryId: string;
  queueTrackId: string;
};

/** Keep Tauri's identity arguments and native capture payload projection aligned. */
export function stereoDiagnosticCompletionArguments(
  identity: StereoDiagnosticPlaybackIdentity,
  capture: Record<string, unknown>,
): StereoDiagnosticPlaybackIdentity & { capture: Record<string, unknown> } {
  return { ...identity, capture };
}

type CaptureMessage = {
  kind?: unknown;
  sampleRate?: unknown;
  channels?: unknown;
  frames?: unknown;
  firstFrame?: unknown;
  firstContextTime?: unknown;
  samples?: unknown;
  reason?: unknown;
};

const STEREO_WORKLET_SOURCE = `
class BurrataStereoDiagnostic extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.maxFrames = options.processorOptions.maxFrames;
    this.samples = new Float32Array(this.maxFrames * 2);
    this.frames = 0;
    this.firstFrame = null;
    this.firstContextTime = null;
    this.active = true;
    this.port.onmessage = event => {
      if (event.data === "stop") {
        this.active = false;
        this.port.postMessage({kind:"cancelled"});
      }
    };
  }
  process(inputs, outputs) {
    const output = outputs[0] || [];
    for (const channel of output) channel.fill(0);
    if (!this.active) return false;
    const input = inputs[0] || [];
    if (input.length !== 2 || !input[0] || !input[1] || input[0].length !== input[1].length) {
      this.active = false;
      this.port.postMessage({kind:"gap", reason:"The pre-destination tap did not receive two aligned channels."});
      return false;
    }
    if (this.firstFrame === null) {
      this.firstFrame = currentFrame;
      this.firstContextTime = currentTime;
    }
    const frames = Math.min(input[0].length, this.maxFrames - this.frames);
    for (let frame = 0; frame < frames; frame++) {
      const target = (this.frames + frame) * 2;
      this.samples[target] = input[0][frame];
      this.samples[target + 1] = input[1][frame];
    }
    this.frames += frames;
    if (this.frames >= this.maxFrames) {
      this.active = false;
      const samples = this.samples.slice(0, this.frames * 2);
      this.port.postMessage({kind:"complete", sampleRate, channels:2, frames:this.frames,
        firstFrame:this.firstFrame, firstContextTime:this.firstContextTime, samples}, [samples.buffer]);
      return false;
    }
    return true;
  }
}
registerProcessor("burrata-stereo-diagnostic", BurrataStereoDiagnostic);
`;

function unavailable(sampleRate: number, reason: string): StereoDiagnosticCapture {
  return { status: "unavailable", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null, reason };
}

function validCapture(message: CaptureMessage, maxFrames: number, expectedSampleRate: number): StereoDiagnosticCapture | null {
  const samples = message.samples;
  const frames = message.frames;
  if (message.kind !== "complete" || message.channels !== 2 || message.sampleRate !== expectedSampleRate ||
      !Number.isSafeInteger(frames) || (frames as number) <= 0 || (frames as number) > maxFrames ||
      !(samples instanceof Float32Array) || samples.length !== (frames as number) * 2 ||
      !Number.isSafeInteger(message.firstFrame) || !Number.isFinite(message.firstContextTime)) return null;
  return {
    status: "complete", sampleRate: expectedSampleRate, channels: 2, frames: frames as number,
    firstFrame: message.firstFrame as number, firstContextTime: message.firstContextTime as number, samples,
  };
}

/** Encode bounded interleaved samples as little-endian Float32 bytes. */
export function encodeStereoDiagnosticSamples(samples: Float32Array): string {
  const bytes = new Uint8Array(samples.length * Float32Array.BYTES_PER_ELEMENT);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < samples.length; index++) {
    const value = samples[index]!;
    if (!Number.isFinite(value) || value < -2 || value > 2) throw new Error("The stereo diagnostic sample is outside its finite PCM range.");
    view.setFloat32(index * Float32Array.BYTES_PER_ELEMENT, value, true);
  }
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

/** Start a fixed-duration stereo capture from the final Web Audio gain node. */
export async function startWebviewStereoDiagnosticTap(
  finalUserVolumeNode: AudioNode,
  context: AudioContext,
  durationSeconds = MAX_STEREO_DIAGNOSTIC_SECONDS,
): Promise<StereoDiagnosticSession> {
  const sampleRate = context.sampleRate;
  if (!Number.isFinite(sampleRate) || sampleRate < 8_000 || sampleRate > 192_000 ||
      !Number.isFinite(durationSeconds) || durationSeconds < MIN_DIAGNOSTIC_SECONDS ||
      durationSeconds > MAX_STEREO_DIAGNOSTIC_SECONDS) {
    return { capture: Promise.resolve(unavailable(Number.isFinite(sampleRate) ? sampleRate : 0, "The diagnostic sample rate or duration is outside the supported bound.")), stop() {} };
  }

  const maxFrames = Math.max(1, Math.floor(sampleRate * durationSeconds));
  let resolveCapture: (capture: StereoDiagnosticCapture) => void = () => {};
  let settled = false;
  let stopped = false;
  let source: AudioNode | null = null;
  let processor: AudioWorkletNode | ScriptProcessorNode | null = null;
  let workletPort: MessagePort | null = null;
  let silentGain: GainNode | null = null;
  let moduleUrl: string | null = null;
  const capture = new Promise<StereoDiagnosticCapture>(resolve => { resolveCapture = resolve; });

  const cleanup = () => {
    if (source && processor) {
      try { source.disconnect(processor); } catch { /* branch may already be gone */ }
    }
    try { processor?.disconnect(); } catch { /* context may already be stopping */ }
    try { silentGain?.disconnect(); } catch { /* context may already be stopping */ }
    if (moduleUrl) URL.revokeObjectURL(moduleUrl);
    source = null; processor = null; workletPort = null; silentGain = null; moduleUrl = null;
  };
  const finish = (result: StereoDiagnosticCapture) => {
    if (settled) return;
    settled = true;
    cleanup();
    resolveCapture(result);
  };
  const zeroBranch = (tapNode: AudioWorkletNode | ScriptProcessorNode): GainNode => {
    const mute = context.createGain();
    mute.gain.setValueAtTime(0, context.currentTime);
    tapNode.connect(mute);
    mute.connect(context.destination);
    silentGain = mute;
    return mute;
  };
  const connectSource = (tapNode: AudioWorkletNode | ScriptProcessorNode) => {
    source = finalUserVolumeNode;
    processor = tapNode;
    finalUserVolumeNode.connect(tapNode);
    zeroBranch(tapNode);
  };

  const workletSupported = Boolean(context.audioWorklet && typeof AudioWorkletNode !== "undefined");
  if (workletSupported) {
    try {
      moduleUrl = URL.createObjectURL(new Blob([STEREO_WORKLET_SOURCE], { type: "text/javascript" }));
      await context.audioWorklet.addModule(moduleUrl);
      if (!stopped) {
        const tap = new AudioWorkletNode(context, "burrata-stereo-diagnostic", {
          numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [2],
          channelCount: 2, channelCountMode: "explicit", processorOptions: { maxFrames },
        });
        workletPort = tap.port;
        tap.port.onmessage = event => {
          const value = event.data as CaptureMessage;
          if (value.kind === "complete") {
            finish(validCapture(value, maxFrames, sampleRate) ?? {
              status: "gap", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null,
              samples: null, reason: "The Web Audio stereo tap returned an invalid bounded capture.",
            });
          } else if (value.kind === "gap") {
            finish({ status: "gap", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null,
              reason: typeof value.reason === "string" ? value.reason : "The Web Audio stereo tap lost channel continuity." });
          } else if (value.kind === "cancelled") {
            finish({ status: "cancelled", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null });
          }
        };
        connectSource(tap);
      }
    } catch {
      cleanup();
      // A supported worklet API may still reject its module. Fall back below.
    }
  }

  if (!processor && !stopped) {
    try {
      const tap = context.createScriptProcessor(SCRIPT_PROCESSOR_BUFFER_SIZE, 2, 2);
      let samples = new Float32Array(maxFrames * 2);
      let frames = 0;
      let firstFrame: number | null = null;
      let firstContextTime: number | null = null;
      tap.onaudioprocess = event => {
        for (let channel = 0; channel < event.outputBuffer.numberOfChannels; channel++) event.outputBuffer.getChannelData(channel).fill(0);
        if (settled) return;
        if (event.inputBuffer.numberOfChannels !== 2) {
          finish({ status: "gap", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null,
            reason: "The pre-destination tap did not receive a stereo input buffer." });
          return;
        }
        const left = event.inputBuffer.getChannelData(0);
        const right = event.inputBuffer.getChannelData(1);
        if (left.length !== right.length) {
          finish({ status: "gap", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null,
            reason: "The pre-destination tap received channels with different frame counts." });
          return;
        }
        if (firstFrame === null) {
          firstContextTime = Number.isFinite(event.playbackTime) ? event.playbackTime : context.currentTime;
          firstFrame = Math.max(0, Math.round(firstContextTime * sampleRate));
        }
        const copiedFrames = Math.min(left.length, maxFrames - frames);
        for (let frame = 0; frame < copiedFrames; frame++) {
          const target = (frames + frame) * 2;
          samples[target] = left[frame]!;
          samples[target + 1] = right[frame]!;
        }
        frames += copiedFrames;
        if (frames >= maxFrames) {
          const completed = samples.slice(0, frames * 2);
          samples = new Float32Array(0);
          finish({ status: "complete", sampleRate, channels: 2, frames, firstFrame, firstContextTime, samples: completed });
        }
      };
      connectSource(tap);
    } catch {
      cleanup();
      finish(unavailable(sampleRate, "Neither AudioWorklet nor ScriptProcessor can observe stereo Web Audio output."));
    }
  }

  if (stopped && !settled) finish({ status: "cancelled", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null });
  return {
    capture,
    stop() {
      if (settled || stopped) return;
      stopped = true;
      workletPort?.postMessage("stop");
      finish({ status: "cancelled", sampleRate, channels: 2, frames: 0, firstFrame: null, firstContextTime: null, samples: null });
    },
  };
}
