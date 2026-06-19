// skyfire-pcm AudioWorklet — plays a pre-decoded PCM ring buffer.
//
// Receives interleaved S16LE PCM as an ArrayBuffer via the port,
// samples the audio clock for A/V sync, and loops the buffer.

class SkyfirePcmProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buffer = null;      // Float32Array (interleaved)
    this.sampleRate = 0;
    this.channels = 0;
    this.frameCount = 0;
    this.playhead = 0;       // sample index
    this.playing = false;
    this.startTime = 0;
    this.startPlayhead = 0;

    this.port.onmessage = (e) => {
      const msg = e.data;
      if (msg.type === "init") {
        this.init(msg.pcm, msg.sampleRate, msg.channels);
      } else if (msg.type === "start") {
        this.playing = true;
        this.startTime = currentTime;
        this.startPlayhead = this.playhead;
      }
    };
  }

  init(pcmBuffer, sampleRate, channels) {
    // Convert S16LE bytes → Float32Array
    const bytes = new Uint8Array(pcmBuffer);
    const sampleCount = bytes.length / 2;
    const f32 = new Float32Array(sampleCount);
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    for (let i = 0; i < sampleCount; i++) {
      f32[i] = dv.getInt16(i * 2, true) / 32768.0;
    }
    this.buffer = f32;
    this.sampleRate = sampleRate;
    this.channels = channels;
    this.frameCount = sampleCount / channels;
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    if (!out || out.length === 0) return true;

    const chCount = Math.min(this.channels, out.length);
    const frameLen = out[0].length;

    if (!this.buffer || this.frameCount === 0) {
      // Output silence.
      for (let ch = 0; ch < chCount; ch++) out[ch].fill(0);
      return true;
    }

    if (!this.playing) {
      // Not started yet — output silence.
      for (let ch = 0; ch < chCount; ch++) out[ch].fill(0);
      return true;
    }

    for (let ch = 0; ch < chCount; ch++) {
      const chOut = out[ch];
      for (let i = 0; i < frameLen; i++) {
        const srcIdx = this.playhead * this.channels + ch;
        if (srcIdx < this.buffer.length) {
          chOut[i] = this.buffer[srcIdx];
        } else {
          chOut[i] = 0;
        }
      }
    }

    this.playhead += frameLen;

    // Loop the buffer when we hit the end.
    if (this.playhead >= this.frameCount) {
      this.playhead = 0;
    }

    return true;
  }
}

registerProcessor("skyfire-pcm", SkyfirePcmProcessor);
