// skyfire.js — documented browser-side API for the Skyfire WASM engine.
//
// Exposes: init(canvas, streamUrl), play(), pause(),
//          selectAudio(idx), selectSubtitle(idx),
//          trackList (live introspection), version.
//
// Consumed by zenith as: <script type="module" src="/skyfire-releases/v0.1.0/skyfire.js"></script>
// Then: const player = new SkyfirePlayer(); player.init(canvas, "https://zenith/skyfire/bbc-one");
//
// ARCHITECTURE
// ────────────
// The WASM bridge (SkyfireBridge) demuxes the MPEG-TS stream, decodes AC-3 /
// E-AC-3 / MP2 audio in WASM, and hands Annex-B H.264 video access units up
// to WebCodecs. Audio drives an AudioWorklet ring-buffer; the audio-master
// clock drags video presentation (rAF poll) so A/V stays in sync.
// DVB subtitles (ETSI EN 300 743) are composited in WASM → RGBA regions
// and blitted onto an overlay canvas.
//
// No DOM dependencies except the canvas and an AudioContext — the caller
// provides both. The canvas is resize-observer-aware; the player adjusts
// the viewport on every decoded frame.

const VERSION = "0.1.1";
const PTS_HZ = 90_000;
const ticksToMicros = (t) => Number(t) * 1_000_000 / PTS_HZ;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Standard ITU-R BS.775 5.1 → stereo downmix matrix.
// Channel ordering (SMPTE/ITU): L, R, C, LFE, Ls, Rs
const _51_L = 0, _51_R = 1, _51_C = 2, _51_LFE = 3, _51_Ls = 4, _51_Rs = 5;
const DBM1 = 0.70710678;

function _downmix51ToStereo(interleaved, srcCh) {
  const frameCount = interleaved.length / srcCh;
  const out = new Float32Array(frameCount * 2);
  for (let i = 0; i < frameCount; i++) {
    const base = i * srcCh;
    out[i * 2]     = interleaved[base + _51_L] + DBM1 * (interleaved[base + _51_C] + interleaved[base + _51_Ls]);
    out[i * 2 + 1] = interleaved[base + _51_R] + DBM1 * (interleaved[base + _51_C] + interleaved[base + _51_Rs]);
  }
  return out;
}

export { VERSION as version };

// ── Public API ─────────────────────────────────────────────────────────────

/**
 * Create a player. Call `init(canvas, streamUrl)` to start playback.
 *
 * ```js
 * import { SkyfirePlayer } from "./skyfire.js";
 * const player = new SkyfirePlayer();
 * player.init(document.getElementById("video"), "https://example.com/stream.ts");
 * ```
 */
export class SkyfirePlayer {
  constructor() {
    this._bridge = null;
    this._canvas = null;
    this._ctx = null;
    this._sized = false;

    // Video decoder
    this._videoDecoder = null;
    this._decoderConfigured = false;
    this._sawKeyframe = false;

    // Audio
    this._audioCtx = null;
    this._audioNode = null;
    this._audioReady = false;
    this._audioStarting = false;
    this._audioGain = null;
    this._streamChannels = 0;
    this._outputChannels = 0;
    this._downmixActive = false;

    // A/V sync
    this._presentQueue = [];
    this._firstAudioPtsUs = null;
    this._audioFramesPlayed = 0;
    this._audioSampleRate = 48000;
    this._presentScheduled = false;
    this._lastVideoTs = 0;

    // Subs
    this._subQueue = [];
    this._subCtx = null;
    this._shownSubKey = null;

    // State
    this._playing = true;
    this._muted = false;
    this._trackList = null;

    // Stats (exposed for diagnostics)
    this.stats = {
      decoded: 0, drawn: 0, dropped: 0, aus: 0,
      audioChunks: 0, audioSamples: 0, audioFrames: 0, audioSec: 0,
      avSkewMs: 0, subCues: 0,
    };
  }

  /**
   * Initialise the player and start streaming.
   *
   * @param {HTMLCanvasElement} canvas — the video render target.
   * @param {string} streamUrl — URL to an MPEG-TS or HLS stream (HTTP/HTTPS).
   * @param {object} [opts]
   * @param {boolean} [opts.live=false] — true for live streams (auto reconnect).
   * @param {number} [opts.maxReconnect=5] — max reconnect attempts for live.
   * @returns {Promise<void>}
   */
  async init(canvas, streamUrl, opts = {}) {
    this._canvas = canvas;
    this._ctx = canvas.getContext("2d", { alpha: false });
    this._streamUrl = streamUrl;
    this._live = !!opts.live;
    this._maxReconnect = opts.maxReconnect ?? 5;

    // Lazy-load the WASM module (bundled alongside this script).
    const wasmBase = import.meta.url.replace(/\/[^/]+$/, "");
    const { default: initWasm, SkyfireBridge } = await import(`${wasmBase}/skyfire_wasm.js`);
    await initWasm();
    this._bridge = new SkyfireBridge();

    // Wire resize observer so canvas matches decoded frame size.
    new ResizeObserver(() => { this._sized = false; }).observe(canvas);

    this._run();
  }

  /** Pause playback (mutes audio, pauses decoder feeding). */
  pause() {
    this._playing = false;
    if (this._bridge) this._bridge.set_playing(false);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "pause" });
  }

  /** Resume playback. */
  play() {
    this._playing = true;
    if (this._bridge) this._bridge.set_playing(true);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "play" });
    this._resumeAudio();
  }

  /**
   * Select audio track by index (0-based, from trackList.audio).
   * @param {number} idx
   */
  selectAudio(idx) {
    const list = this._trackList ?? this._bridge.track_list();
    if (!list || idx >= list.audio.length) return;
    this._bridge.select_audio(list.audio[idx].pid);
  }

  /**
   * Select subtitle track by index (0-based, from trackList.subtitles).
   * Pass -1 to disable subtitles.
   * @param {number} idx
   */
  selectSubtitle(idx) {
    const list = this._trackList ?? this._bridge.track_list();
    if (idx < 0 || !list || idx >= list.subtitles.length) {
      this._bridge.select_subtitle(undefined);
      return;
    }
    this._bridge.select_subtitle(list.subtitles[idx].pid);
  }

  /**
   * Live track list (updated as soon as PMT is parsed).
   * @returns {{ video: {pid, codec}, audio: [{pid, codec, language}], subtitles: [{pid, kind, language}] } | null}
   */
  get trackList() {
    const tl = this._bridge ? this._bridge.track_list() : undefined;
    if (!tl) return null;
    return {
      video: { pid: tl.video_pid, codec: tl.video_codec },
      audio: tl.audio.map((a) => ({ pid: a.pid, codec: a.codec, language: a.language ?? null })),
      subtitles: tl.subtitles.map((s) => ({ pid: s.pid, kind: s.kind, language: s.language ?? null })),
    };
  }

  /** Dispose the player and free WASM memory. */
  destroy() {
    this._playing = false;
    if (this._videoDecoder) { try { this._videoDecoder.close(); } catch (_) {} }
    if (this._audioNode) { try { this._audioNode.disconnect(); } catch (_) {} }
    if (this._audioCtx) { try { this._audioCtx.close(); } catch (_) {} }
    while (this._presentQueue.length) this._presentQueue.shift().frame?.close();
    if (this._bridge) { try { this._bridge.free(); } catch (_) {} }
    this._bridge = null;
  }

  // ── internals ──────────────────────────────────────────────────────────

  async _run() {
    const MAX_RECONNECT = this._maxReconnect;
    let attempt = 0;

    for (;;) {
      try {
        await this._consumeStream(this._streamUrl);
      } catch (e) {
        if (this._live && attempt < MAX_RECONNECT) {
          attempt++;
          await sleep(Math.min(1500 * attempt, 8000));
          this._sawKeyframe = false;
          continue;
        }
        this._emit("error", e);
        return;
      }
      if (this._live && attempt < MAX_RECONNECT) {
        attempt++;
        this._sawKeyframe = false;
        await sleep(1000);
        continue;
      }
      break;
    }

    this._bridge.flush();
    this._pumpVideo();
    this._pumpAudio();
    this._pumpSubtitles();
    if (this._videoDecoder && this._decoderConfigured) {
      try { await this._videoDecoder.flush(); } catch (_) {}
    }
    this._emit("end");
  }

  async _consumeStream(src) {
    const resp = await fetch(src);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const reader = resp.body.getReader();
    let trackLogged = false;

    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      this._bridge.feed(value);

      if (!trackLogged) {
        const tl = this._bridge.track_list();
        if (tl) {
          trackLogged = true;
          this._trackList = tl;
          this._emit("tracklist", this.trackList);
        }
      }
      this._pumpVideo();
      this._pumpAudio();
      this._pumpSubtitles();

      while (this._presentQueue.length > 60) await sleep(40);
    }
  }

  _pumpVideo() {
    const codec = this._bridge.video_codec();
    if (!codec) return;
    this._ensureDecoder(codec);

    for (const au of this._bridge.take_video_aus()) {
      this.stats.aus++;
      if (!this._sawKeyframe) {
        if (!au.is_keyframe) { au.free?.(); continue; }
        this._sawKeyframe = true;
      }
      const ts = au.pts_ticks !== undefined ? ticksToMicros(au.pts_ticks) : 0;
      try {
        this._videoDecoder.decode(new EncodedVideoChunk({
          type: au.is_keyframe ? "key" : "delta",
          timestamp: ts,
          data: au.bytes,
        }));
      } catch (e) {
        this._emit("error", e);
      }
      au.free?.();
    }
  }

  _ensureDecoder(codec) {
    if (this._decoderConfigured) return;
    this._videoDecoder = new VideoDecoder({
      output: (frame) => {
        this.stats.decoded++;
        this._presentQueue.push({ frame, ts: frame.timestamp });
        this._schedulePresent();
      },
      error: (e) => this._emit("error", e),
    });
    this._videoDecoder.configure({ codec, optimizeForLatency: true });
    this._decoderConfigured = true;
  }

  _pumpAudio() {
    const chunks = this._bridge.take_audio_pcm();
    for (const c of chunks) {
      if (!this._audioReady) {
        this._ensureAudio(c.sample_rate, c.channels);
      }
      if (this._firstAudioPtsUs === null && c.pts_ticks !== undefined) {
        this._firstAudioPtsUs = ticksToMicros(c.pts_ticks);
      }
      let samples = c.samples;
      if (this._downmixActive && this._streamChannels === 6) {
        samples = _downmix51ToStereo(samples, this._streamChannels);
      }
      this.stats.audioChunks++;
      this.stats.audioSamples += samples.length;
      if (this._audioNode) {
        this._audioNode.port.postMessage({ type: "pcm", samples }, [samples.buffer]);
      }
      c.free?.();
    }
  }

  _ensureAudio(sampleRate, channels) {
    if (this._audioReady || this._audioStarting) return;
    this._audioStarting = true;
    this._streamChannels = channels;

    this._audioCtx = new AudioContext({ sampleRate });
    this._audioSampleRate = this._audioCtx.sampleRate || sampleRate;

    const maxCh = this._audioCtx.destination.maxChannelCount;
    if (channels <= maxCh) {
      this._outputChannels = channels;
      this._downmixActive = false;
    } else {
      this._outputChannels = Math.min(2, maxCh);
      this._downmixActive = true;
    }

    this._audioCtx.audioWorklet.addModule(
      new URL("./skyfire-audio-worklet.js", import.meta.url)
    ).then(() => {
      this._audioNode = new AudioWorkletNode(this._audioCtx, "skyfire-pcm", {
        numberOfOutputs: 1,
        outputChannelCount: [this._outputChannels],
      });
      this._audioNode.port.onmessage = (e) => {
        if (e.data.type === "clock") {
          this._audioFramesPlayed = e.data.framesPlayed;
          this.stats.audioFrames = this._audioFramesPlayed;
          this.stats.audioSec = this._audioFramesPlayed / this._audioSampleRate;
          this._schedulePresent();
        }
      };
      this._audioGain = this._audioCtx.createGain();
      this._audioGain.gain.value = this._muted ? 0 : 1;
      this._audioNode.connect(this._audioGain).connect(this._audioCtx.destination);
      this._audioNode.port.postMessage({ type: "config", sampleRate: this._audioSampleRate, channels: this._outputChannels });
      this._audioNode.port.postMessage({ type: "play" });
      this._audioReady = true;
      this._audioStarting = false;
      this._resumeAudio();
    }).catch((e) => this._emit("error", e));
  }

  _resumeAudio() {
    if (this._audioCtx && this._audioCtx.state === "suspended")
      this._audioCtx.resume().catch(() => {});
  }

  _pumpSubtitles() {
    if (!this._bridge.take_subtitle_cues) return;
    let added = false;
    for (const cue of this._bridge.take_subtitle_cues()) {
      const start = Number(cue.start_pts);
      const end = Number(cue.end_pts);
      const regions = cue.regions.map((r) => {
        const o = { x: r.x, y: r.y, width: r.width, height: r.height, rgba: r.rgba };
        r.free?.();
        return o;
      });
      this._subQueue.push({
        startUs: ticksToMicros(start),
        endUs: ticksToMicros(end > start ? end : start + 3 * PTS_HZ),
        key: `${start}:${regions.length}`,
        regions,
      });
      this.stats.subCues++;
      added = true;
      cue.free?.();
    }
    if (added) this._schedulePresent();
  }

  // ── render loop ────────────────────────────────────────────────────────

  _schedulePresent() {
    if (this._presentScheduled) return;
    this._presentScheduled = true;
    requestAnimationFrame(() => this._present());
  }

  _present() {
    this._presentScheduled = false;
    const clock = this._audioClockUs();

    if (clock === null) {
      const e = this._presentQueue.shift();
      if (e) this._drawFrame(e.frame);
      this._renderSubs(this._lastVideoTs || 0);
      if (this._presentQueue.length || this._subQueue.length) this._schedulePresent();
      return;
    }

    const LEAD_US = 12_000;
    const LATE_DROP_US = 80_000;

    while (this._presentQueue.length) {
      const e = this._presentQueue[0];
      if (e.ts > clock + LEAD_US) break;
      this._presentQueue.shift();
      if (e.ts < clock - LATE_DROP_US) { e.frame.close(); this.stats.dropped++; continue; }
      this._drawFrame(e.frame);
      this.stats.avSkewMs = Math.round((clock - e.ts) / 1000);
    }
    this._renderSubs(clock);
    if (this._presentQueue.length || this._subQueue.length) this._schedulePresent();
  }

  _drawFrame(frame) {
    try {
      if (!this._sized || this._canvas.width !== frame.displayWidth || this._canvas.height !== frame.displayHeight) {
        this._canvas.width = frame.displayWidth;
        this._canvas.height = frame.displayHeight;
        this._sized = true;
      }
      this._ctx.drawImage(frame, 0, 0, this._canvas.width, this._canvas.height);
      this.stats.drawn++;
      this._lastVideoTs = frame.timestamp;
    } finally {
      frame.close();
    }
  }

  _audioClockUs() {
    if (this._firstAudioPtsUs === null || this._audioFramesPlayed === 0) return null;
    return this._firstAudioPtsUs + (this._audioFramesPlayed / this._audioSampleRate) * 1_000_000;
  }

  _renderSubs(clockUs) {
    while (this._subQueue.length && this._subQueue[0].endUs <= clockUs) {
      if (this._shownSubKey === this._subQueue[0].key) this._clearSubs();
      this._subQueue.shift();
    }
    const active = this._subQueue.find((c) => c.startUs <= clockUs && clockUs < c.endUs);
    if (active) {
      if (this._shownSubKey !== active.key) { this._drawSubCue(active); this._shownSubKey = active.key; }
    } else if (this._shownSubKey !== null) {
      this._clearSubs();
    }
  }

  _clearSubs() {
    if (this._subCtx) this._subCtx.clearRect(0, 0, this._subCtx.canvas.width, this._subCtx.canvas.height);
    this._shownSubKey = null;
  }

  _drawSubCue(cue) {
    const cx = this._ensureSubsCtx();
    cx.clearRect(0, 0, cx.canvas.width, cx.canvas.height);
    for (const r of cue.regions) {
      if (!r.rgba || !r.width || !r.height) continue;
      cx.putImageData(new ImageData(new Uint8ClampedArray(r.rgba), r.width, r.height), r.x, r.y);
    }
  }

  _ensureSubsCtx() {
    if (this._subCtx) return this._subCtx;
    const c = document.createElement("canvas");
    c.style.cssText = "position:fixed;left:0;right:0;bottom:12%;z-index:15;display:flex;justify-content:center;pointer-events:none;max-width:90vw;margin:0 auto;";
    document.body.appendChild(c);
    this._subCtx = c.getContext("2d");
    return this._subCtx;
  }

  _emit(type, detail) {
    this._canvas?.dispatchEvent(new CustomEvent(`skyfire:${type}`, { detail }));
  }
}
