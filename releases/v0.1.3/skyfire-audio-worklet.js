// skyfire-pcm AudioWorklet — streaming PCM ring buffer (ADR 0008 bridge client).
//
// The bridge (`SkyfireBridge.take_audio_pcm()`) yields interleaved **f32** PCM
// chunks as TS is fed. The main thread posts them here as `{type:"pcm", samples}`;
// this processor enqueues them in a ring and drains frame-by-frame in `process()`.
//
// It also reports `framesPlayed` (frames actually emitted to the output) back to
// the main thread — the audio-master clock (#32) is derived from that: media
// time = framesPlayed / sampleRate.

const RING_CAPACITY_FRAMES = 48000 * 4; // ~4 s of headroom at 48 kHz

class SkyfirePcmProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.channels = 2;
    this.sampleRate = 48000;
    this.playing = false;

    // Interleaved f32 ring buffer.
    this.ring = new Float32Array(RING_CAPACITY_FRAMES * this.channels);
    this.writeIdx = 0;   // sample index (interleaved)
    this.readIdx = 0;    // sample index (interleaved)
    this.available = 0;  // interleaved samples available to read

    this.framesPlayed = 0;
    this.reportCounter = 0;

    this.port.onmessage = (e) => {
      const msg = e.data;
      switch (msg.type) {
        case "config":
          this.configure(msg.sampleRate, msg.channels);
          break;
        case "pcm":
          this.enqueue(msg.samples);
          break;
        case "play":
          this.playing = true;
          break;
        case "pause":
          this.playing = false;
          break;
      }
    };
  }

  configure(sampleRate, channels) {
    this.sampleRate = sampleRate || 48000;
    this.channels = channels || 2;
    this.ring = new Float32Array(RING_CAPACITY_FRAMES * this.channels);
    this.writeIdx = 0;
    this.readIdx = 0;
    this.available = 0;
    this.framesPlayed = 0;
  }

  enqueue(samples) {
    // samples: Float32Array, interleaved, `channels` per frame.
    const cap = this.ring.length;
    for (let i = 0; i < samples.length; i++) {
      this.ring[this.writeIdx] = samples[i];
      this.writeIdx = (this.writeIdx + 1) % cap;
      if (this.available < cap) {
        this.available++;
      } else {
        // Overflow — advance read (drop oldest) to stay live.
        this.readIdx = (this.readIdx + 1) % cap;
      }
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    if (!out || out.length === 0) return true;

    const chCount = Math.min(this.channels, out.length);
    const frameLen = out[0].length;
    const cap = this.ring.length;

    if (!this.playing) {
      for (let ch = 0; ch < chCount; ch++) out[ch].fill(0);
      return true;
    }

    const framesAvail = Math.floor(this.available / this.channels);
    const framesToPlay = Math.min(frameLen, framesAvail);

    for (let i = 0; i < frameLen; i++) {
      if (i < framesToPlay) {
        for (let ch = 0; ch < chCount; ch++) {
          out[ch][i] = this.ring[(this.readIdx + i * this.channels + ch) % cap];
        }
      } else {
        // Underrun — pad with silence (don't advance clock past real audio).
        for (let ch = 0; ch < chCount; ch++) out[ch][i] = 0;
      }
    }

    if (framesToPlay > 0) {
      this.readIdx = (this.readIdx + framesToPlay * this.channels) % cap;
      this.available -= framesToPlay * this.channels;
      this.framesPlayed += framesToPlay;
    }

    // Report the clock ~every 10 quanta (~27 ms at 128-frame quanta).
    if (++this.reportCounter >= 10) {
      this.reportCounter = 0;
      this.port.postMessage({ type: "clock", framesPlayed: this.framesPlayed });
    }

    return true;
  }
}

registerProcessor("skyfire-pcm", SkyfirePcmProcessor);
