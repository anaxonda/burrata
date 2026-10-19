import workletUrl from "./webview-radio-pcm-worklet.js?url&no-inline";

const SAMPLE_RATE = 48_000;
const MIN_SECONDS = 3;
const MAX_SECONDS = 12;
const MAX_FRAMES = SAMPLE_RATE * MAX_SECONDS;
const MAX_WAV_BYTES = 44 + MAX_FRAMES * 2;

export async function captureWebviewRadioWavBase64(
  audio: HTMLAudioElement,
  isCurrent: () => boolean,
  signal: AbortSignal,
  onProgress: (seconds: number) => void = () => {},
  options?: { onSample: (wavBase64: string, seconds: number) => Promise<boolean>; onCancel?: () => void },
): Promise<string> {
  const media = audio as HTMLAudioElement & { captureStream?: () => MediaStream; webkitCaptureStream?: () => MediaStream };
  const capture = media.captureStream ?? media.webkitCaptureStream;
  if (!capture) throw new Error("This WebView cannot capture the currently playing audio.");
  if (!isCurrent() || signal.aborted) throw abortError();
  if (audio.paused) throw new Error("Start playback before capturing audio for identification.");

  return await new Promise<string>((resolve, reject) => {
    let stream: MediaStream | undefined;
    let context: AudioContext | undefined;
    let source: MediaStreamAudioSourceNode | undefined;
    let node: AudioWorkletNode | undefined;
    let sink: GainNode | undefined;
    let interval: ReturnType<typeof setInterval> | undefined;
    let done = false;
    let elapsedFrames = 0;
    const steps = options ? [3, 6, 12] : [8];
    let step = 0;
    let recognizing = false;
    let requested = false;
    let lastRequestAt = -Infinity;
    const cleanup = () => {
      if (done) return;
      done = true;
      signal.removeEventListener("abort", onAbort);
      clearTimeout(deadlineTimer);
      if (interval) clearInterval(interval);
      node?.port.close();
      try { source?.disconnect(); } catch { /* disconnected */ }
      try { node?.disconnect(); } catch { /* disconnected */ }
      try { sink?.disconnect(); } catch { /* disconnected */ }
      stream?.getTracks().forEach((track) => track.stop());
      if (context) void context.close().catch(() => {});
    };
    const fail = (error: Error) => {
      if (done) return;
      // A provider can still be reading its private WAV when capture expires.
      // Signal its owner before releasing the capture and admitting another try.
      if (recognizing) options?.onCancel?.();
      cleanup(); reject(error);
    };
    const finish = (value: string) => { if (done) return; cleanup(); resolve(value); };
    const onAbort = () => fail(abortError());
    const deadlineTimer = setTimeout(() => fail(new Error(options ? "Audio identification timed out." : "Could not capture eight seconds of current audio within twelve seconds.")), options ? 90_000 : 12_000);
    signal.addEventListener("abort", onAbort, { once: true });

    void (async () => {
      try {
        stream = capture.call(audio);
        if (!stream.getAudioTracks().length) throw new Error("This WebView did not expose current audio; no stream was reopened.");
        const AudioContextCtor = globalThis.AudioContext;
        if (!AudioContextCtor || typeof AudioWorkletNode === "undefined") throw new Error("This WebView lacks AudioWorklet support for current audio capture.");
        context = new AudioContextCtor({ sampleRate: SAMPLE_RATE });
        if (context.sampleRate !== SAMPLE_RATE) throw new Error("This WebView cannot provide 48 kHz capture for current audio.");
        source = context.createMediaStreamSource(stream);
        await context.audioWorklet.addModule(workletUrl);
        if (done) return;
        if (!isCurrent() || signal.aborted) throw abortError();
        node = new AudioWorkletNode(context, "burrata-radio-pcm-ring", {
          numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1], processorOptions: { maxFrames: MAX_FRAMES },
        });
        sink = context.createGain(); sink.gain.value = 0;
        source.connect(node); node.connect(sink); sink.connect(context.destination);
        node.port.onmessage = (event: MessageEvent<{ type: string; id: number; frames?: number; samples?: ArrayBuffer }>) => {
          if (done) return;
          if (event.data.type === "progress") {
            elapsedFrames = event.data.frames ?? elapsedFrames;
            onProgress(Math.min(MAX_SECONDS, elapsedFrames / SAMPLE_RATE));
          } else if (event.data.id === 1 && event.data.type === "snapshot" && event.data.samples) {
            if (recognizing) return;
            requested = false;
            try {
              const samples = new Float32Array(event.data.samples);
              const seconds = steps[step]!;
              const wav = encodeMonoPcm16WavBase64(samples.subarray(Math.max(0, samples.length - seconds * SAMPLE_RATE)));
              if (!options) { finish(wav); return; }
              recognizing = true;
              lastRequestAt = Date.now();
              void options.onSample(wav, seconds).then(matched => {
                if (done) return;
                if (!isCurrent() || signal.aborted) { fail(abortError()); return; }
                if (matched || step === steps.length - 1) finish(wav);
                else { step++; recognizing = false; }
              }).catch(error => fail(error instanceof Error ? error : new Error(String(error))));
            } catch (error) { fail(error instanceof Error ? error : new Error(String(error))); }
          }
        };
        interval = setInterval(() => {
          if (!isCurrent() || signal.aborted) { fail(abortError()); return; }
          onProgress(Math.min(MAX_SECONDS, elapsedFrames / SAMPLE_RATE));
          if (!recognizing && !requested && Date.now() - lastRequestAt >= 10_000) {
            requested = true;
            node?.port.postMessage({ type: "snapshot", id: 1, minFrames: steps[step]! * SAMPLE_RATE });
            // The worklet ignores requests until enough current audio exists.
            requested = false;
          }
        }, 250);
        await context.resume();
      } catch (error) {
        fail(error instanceof Error ? error : new Error(String(error)));
      }
    })();
  });
}

export function encodeMonoPcm16WavBase64(samples: Float32Array): string {
  if (samples.length < MIN_SECONDS * SAMPLE_RATE || samples.length > MAX_FRAMES) throw new Error("Captured audio must contain three to twelve seconds.");
  const bytes = new Uint8Array(44 + samples.length * 2);
  if (bytes.byteLength > MAX_WAV_BYTES) throw new Error("Captured audio exceeds the twelve-second limit.");
  const view = new DataView(bytes.buffer);
  const tag = (offset: number, text: string) => [...text].forEach((char, index) => view.setUint8(offset + index, char.charCodeAt(0)));
  tag(0, "RIFF"); view.setUint32(4, bytes.length - 8, true); tag(8, "WAVE"); tag(12, "fmt ");
  view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, SAMPLE_RATE, true); view.setUint32(28, SAMPLE_RATE * 2, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  tag(36, "data"); view.setUint32(40, samples.length * 2, true);
  let peak = 0;
  for (let i = 0; i < samples.length; i++) {
    const value = Number.isFinite(samples[i]) ? Math.max(-1, Math.min(1, samples[i]!)) : 0;
    peak = Math.max(peak, Math.abs(value));
    view.setInt16(44 + i * 2, value < 0 ? Math.round(value * 32768) : Math.round(value * 32767), true);
  }
  if (peak < 1e-6) throw new Error("WebView capture received silence from the current audio; no stream was reopened.");
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + 0x8000, bytes.length)));
  return btoa(binary);
}

function abortError(): Error { const error = new Error("Audio capture cancelled because playback changed."); error.name = "AbortError"; return error; }
