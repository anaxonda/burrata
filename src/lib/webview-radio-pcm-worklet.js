class BurrataRadioPcmRing extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.samples = new Float32Array(Math.max(1, Math.min(576000, options.processorOptions?.maxFrames ?? 576000)));
    this.write = 0;
    this.frames = 0;
    this.framesSinceProgress = 0;
    this.port.onmessage = ({ data }) => {
      if (data?.type !== "snapshot") return;
      if (this.frames < data.minFrames) return;
      const length = Math.min(this.frames, this.samples.length);
      const result = new Float32Array(length);
      const start = (this.write - length + this.samples.length) % this.samples.length;
      const first = Math.min(length, this.samples.length - start);
      result.set(this.samples.subarray(start, start + first));
      if (first < length) result.set(this.samples.subarray(0, length - first), first);
      this.port.postMessage({ type: "snapshot", id: data.id, samples: result.buffer }, [result.buffer]);
    };
  }
  process(inputs, outputs) {
    const channels = inputs[0] ?? [];
    const output = outputs[0]?.[0];
    if (output) output.fill(0);
    if (!channels.length) return true;
    const frames = channels[0]?.length ?? 0;
    for (let frame = 0; frame < frames; frame++) {
      let value = 0;
      for (const channel of channels) value += channel?.[frame] ?? 0;
      value /= channels.length;
      this.samples[this.write] = Number.isFinite(value) ? value : 0;
      this.write = (this.write + 1) % this.samples.length;
      this.frames = Math.min(this.samples.length, this.frames + 1);
    }
    this.framesSinceProgress += frames;
    if (this.framesSinceProgress >= 12000) {
      this.framesSinceProgress = 0;
      this.port.postMessage({ type: "progress", frames: this.frames });
    }
    return true;
  }
}
registerProcessor("burrata-radio-pcm-ring", BurrataRadioPcmRing);
