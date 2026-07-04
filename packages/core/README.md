# @firemedia/skyfire-core

In-browser DVB TV player bridge: MPEG-TS demux + WASM AC-3/E-AC-3 decode + audio-master A/V sync + DVB-subtitle compositing. The host wires WebCodecs (video), WebAudio (decoded PCM audio), and canvas (subtitle overlay).

## Installation

```bash
npm install @firemedia/skyfire-core
```

## Usage

`@firemedia/skyfire-core` exports:
- `initSkyfire()`: Async initializer for the WASM module (call once before constructing a bridge).
- `SkyfireBridge`: The main class that demuxes MPEG-TS, decodes selected audio tracks, and composites subtitles.

### Samples-In, Host-Renders Model

The bridge is a stateful processor: feed raw TS bytes, poll the outputs, and wire the samples to WebCodecs/WebAudio/canvas.

```js
import { initSkyfire, SkyfireBridge } from "@firemedia/skyfire-core";

// Initialize the WASM module once
await initSkyfire();

// Construct the bridge
const bridge = new SkyfireBridge();

// Fetch and feed TS chunks
const res = await fetch("/path/to/stream.ts");
const reader = res.body.getReader();

while (true) {
  const { done, value } = await reader.read();
  if (done) {
    bridge.flush(); // Signal end-of-stream
    break;
  }
  bridge.feed(value); // Feed TS bytes

  // Drain outputs each cycle
  const videoAUs = bridge.take_video_aus();
  const pcmChunks = bridge.take_audio_pcm();
  const subtitleCues = bridge.take_subtitle_cues();

  // Wire videoAUs → VideoDecoder.decode()
  for (const au of videoAUs) {
    videoDecoder.decode(new EncodedVideoChunk({
      type: au.isKeyframe ? "key" : "delta",
      timestamp: Number(au.ptsTicks ?? 0n) * 1_000_000 / 90_000,
      data: au.bytes,
    }));
  }

  // Wire pcmChunks → AudioWorklet / AudioBuffer
  for (const chunk of pcmChunks) {
    audioContext.getAudioWorkletNode().port.postMessage({
      samples: chunk.samples,
      sampleRate: chunk.sampleRate,
    });
  }

  // Wire subtitleCues → canvas overlay (draw regions at x, y)
  for (const cue of subtitleCues) {
    for (const region of cue.regions) {
      drawSubtitleRegion(region.x, region.y, region.width, region.height, region.rgba);
    }
  }
}
```

## Stream Contract

The input stream must be progressive H.264 MPEG-TS with:
- **Video**: H.264 (progressive, no interlacing; `frame_mbs_only=1`)
- **Audio**: AC-3 or E-AC-3 (untouched, no re-encoding)
- **Subtitles**: DVB subtitles (separate PID)
- **Timing**: PCR and PTS preserved (90 kHz clock)

See the [DVB stream architecture](https://github.com/fishloa/rust-skyfire/issues/27) for details on how the server (zenith) prepares streams for the browser.

## API Reference

### `initSkyfire(): Promise<void>`

Initialize the WASM module. Idempotent — calling it multiple times is safe.

### `SkyfireBridge`

#### Constructor

```ts
new SkyfireBridge()
```

#### Methods

- **`feed(bytes: Uint8Array): void`** — Push raw TS chunk into the bridge. Demuxes PAT/PMT on the fly and accumulates video AUs.

- **`flush(): void`** — Signal end-of-stream. Flushes any partial PES packets and processes remaining access units. Safe to call once at stream end.

- **`track_list(): TrackList | undefined`** — Drain the parsed track list (video + audio + subtitle PIDs and codecs). Returns `undefined` until PAT+PMT have been parsed.

- **`select_audio(pid: number): void`** — Select which audio PID to route and decode.

- **`select_subtitle(pid?: number): void`** — Select a subtitle PID, or omit/`null` to disable subtitles.

- **`set_audio_downmix(enabled: boolean): void`** — Enable (default) or disable the WASM stereo downmix. Disable to emit native multichannel PCM for discrete output on a capable device.

- **`audio_native_channels(): number`** — Native channel count of the most recently decoded audio (before any downmix), or 0 if none yet. Use this to decide whether the output device can render discrete multichannel.

- **`take_video_aus(): VideoAu[]`** — Drain all completed video access units since the last call. Returns AVCC length-prefixed bytes with PTS/DTS and keyframe flag.

- **`video_codec(): string | undefined`** — WebCodecs codec string (e.g. `"avc1.640028"`), or `undefined` until SPS has been seen.

- **`video_config_description(): Uint8Array`** — WebCodecs `avcC` description bytes, or empty if not yet available.

- **`video_init_segment(): Uint8Array`** — CMAF initialization segment for MSE playback.

- **`take_video_media_segment(): MediaSegment | undefined`** — Drain the next complete GOP as a CMAF media segment, or `undefined` until a full GOP is buffered.

- **`take_audio_pcm(): PcmChunk[]`** — Drain all decoded PCM chunks since the last call. Each chunk corresponds to one audio access unit. See `PcmChunk` interface below.

- **`take_subtitle_cues(): SubtitleCue[]`** — Drain all composited subtitle cues since the last call. Each cue contains RGBA region bitmaps ready for overlay.

- **`pcr_pts(): bigint | undefined`** — Latest PCR-derived clock value in 90 kHz ticks.

### Type reference

```ts
interface PcmChunk {
  samples: Float32Array;  // Interleaved PCM samples (e.g. L,R,L,R for stereo)
  sample_rate: number;    // Sample rate in Hz
  channels: number;       // Number of audio channels
  pts_ticks?: bigint;     // PTS in 90 kHz ticks
}

interface MediaSegment {
  bytes: Uint8Array;
  base_media_decode_time: bigint;
  sample_count: number;
}
```

## License

Dual licensed under MIT OR Apache-2.0.
