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

const VERSION = "0.1.4";
const PTS_HZ = 90_000;
const ticksToMicros = (t) => Number(t) * 1_000_000 / PTS_HZ;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── Re-entrancy guard for SkyfireBridge ───────────────────────────────────
//
// wasm-bindgen structs internally use a RefCell — concurrent method calls
// from overlapping JS contexts (trackList poll, UI events, worklet onmessage)
// trigger "recursive use of an object" panics.  The guard wraps every bridge
// method so no two can execute at the same time.
//
// STRUCTURAL GUARANTEE: All bridge access goes through _callBridge().  Methods
// that are called from outside the main _consumeStream loop (selectAudio,
// selectSubtitle, play/pause, trackList getter) defer to the loop via flags or
// cached state — they never call the bridge directly.  The only direct bridge
// calls happen inside _consumeStream, where they execute sequentially in the
// same synchronous tick with no awaits between them.


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

    // Re-entrancy guard (see §Guard above)
    this._bridgeLocked = false;
    this._pendingBridgeQueue = [];
    // Pending track selections — applied in the main loop, not from event handlers.
    this._pendingAudioPid = null;     // null=no change, pid=select this
    this._pendingSubtitlePid = null;  // null=no change, -1=disable, pid=select this

    // Stats (exposed for diagnostics)
    this.stats = {
      decoded: 0, drawn: 0, dropped: 0, aus: 0,
      audioChunks: 0, audioSamples: 0, audioFrames: 0, audioSec: 0,
      avSkewMs: 0, subCues: 0,
    };
  }

  // ── bridge serialization helpers ───────────────────────────────────────

  /**
   * Execute a bridge method through the re-entrancy guard.
   *
   * If the bridge is free, the call executes immediately and the return value
   * is passed through.  If the bridge is locked (another call is in progress),
   * the call is deferred to a queue and executed (with its return value
   * discarded) after the current borrow is released.
   *
   * This is a safety net — by design no re-entrant calls should happen after
   * this patch (external callers use flags); if they do, we warn and queue
   * rather than crashing.
   *
   * @param {string|function} method - method name on the bridge, or a fn
   * @param {...any} args
   * @returns {any}
   */
  _callBridge(method, ...args) {
    if (!this._bridge) return;

    if (this._bridgeLocked) {
      // Re-entrant call detected — queue it.
      // Feature: count these in stats so we know the guard is working.
      this.stats._bridgeReentries = (this.stats._bridgeReentries || 0) + 1;
      if (typeof method === 'function') {
        this._pendingBridgeQueue.push(method);
      } else {
        const m = method;
        this._pendingBridgeQueue.push(() => this._bridge[m](...args));
      }
      return undefined;
    }

    this._bridgeLocked = true;
    try {
      if (typeof method === 'function') return method();
      return this._bridge[method](...args);
    } finally {
      this._bridgeLocked = false;
      // Drain any calls that arrived while we were locked.
      this._drainBridgeQueue();
    }
  }

  /**
   * Drain the pending bridge call queue.  Called after the primary call
   * completes so any calls deferred during the locked window eventually run.
   */
  _drainBridgeQueue() {
    while (this._pendingBridgeQueue.length > 0) {
      const fn = this._pendingBridgeQueue.shift();
      if (this._bridgeLocked) {
        // Nested drain — re-queue to avoid infinite recursion.
        this._pendingBridgeQueue.unshift(fn);
        break;
      }
      this._bridgeLocked = true;
      try {
        fn();
      } finally {
        this._bridgeLocked = false;
      }
    }
  }

  // ── public API ────────────────────────────────────────────────────────

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
    this._callBridge("set_playing", false);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "pause" });
  }

  /** Resume playback. */
  play() {
    this._playing = true;
    this._callBridge("set_playing", true);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "play" });
    this._resumeAudio();
  }

  /**
   * Select audio track by index (0-based, from trackList.audio).
   *
   * The actual bridge call is deferred to the main _consumeStream loop;
   * this method sets a flag so the loop picks up the change on the next
   * iteration, avoiding re-entrancy with any bridge borrow held by
   * feed()/take_*() in that loop.
   *
   * @param {number} idx
   */
  selectAudio(idx) {
    const list = this._trackList;
    if (!list || idx < 0 || idx >= list.audio.length) return;
    this._pendingAudioPid = list.audio[idx].pid;
  }

  /**
   * Select subtitle track by index (0-based, from trackList.subtitles).
   * Pass -1 to disable subtitles.
   *
   * See `selectAudio()` — deferred to the main loop.
   *
   * @param {number} idx
   */
  selectSubtitle(idx) {
    const list = this._trackList;
    if (idx < 0 || !list || idx >= list.subtitles.length) {
      this._pendingSubtitlePid = -1;
      return;
    }
    this._pendingSubtitlePid = list.subtitles[idx].pid;
  }

  /**
   * Live track list (cached copy — never calls the bridge directly).
   *
   * Populated from the main _consumeStream loop when PMT is first parsed.
   * Consumer callers that previously polled `bridge.track_list()` externally
   * no longer re-enter the bridge; they see the cached snapshot.
   *
   * @returns {{ video: {pid, codec}, audio: [{pid, codec, language}], subtitles: [{pid, kind, language}] } | null}
   */
  get trackList() {
    const tl = this._trackList;
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
    if (this._bridge) { try { this._callBridge("free"); } catch (_) {} }
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

    this._callBridge(() => {
      this._bridge.flush();
      this._pumpVideoInner();
      this._pumpAudioInner();
      this._pumpSubtitlesInner();
    });
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

      // ── single synchronous bridge tick ────────────────────────────────
      // All bridge access in this tick is sequential with no awaits between
      // calls, so borrows are released before the next method runs.
      // Pending track selections from selectAudio/selectSubtitle are applied
      // here, inside the locked window where the bridge is held exclusively.

      this._callBridge(() => {
        this._bridge.feed(value);

        if (!trackLogged) {
          const tl = this._bridge.track_list();
          if (tl) {
            trackLogged = true;
            this._trackList = tl;
            this._emit("tracklist", this.trackList);
          }
        }

        // Apply deferred track selections.
        if (this._pendingAudioPid !== null) {
          this._bridge.select_audio(this._pendingAudioPid);
          this._pendingAudioPid = null;
        }
        if (this._pendingSubtitlePid !== null) {
          this._bridge.select_subtitle(
            this._pendingSubtitlePid === -1
              ? undefined
              : this._pendingSubtitlePid
          );
          this._pendingSubtitlePid = null;
        }

        // Drain outputs (these are &mut self → must be in the locked window).
        this._pumpVideoInner();
        this._pumpAudioInner();
        this._pumpSubtitlesInner();
      });

      while (this._presentQueue.length > 60) await sleep(40);
    }
  }

  _pumpVideoInner() {
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
    // Called from _pumpVideoInner which runs inside _callBridge — the bridge
    // is already locked by the caller, so access it directly.
    if (this._decoderConfigured) return;
    this._videoDecoder = new VideoDecoder({
      output: (frame) => {
        this.stats.decoded++;
        this._presentQueue.push({ frame, ts: frame.timestamp });
        this._schedulePresent();
      },
      error: (e) => this._emit("error", e),
    });
    this._videoDecoder.configure({ codec, optimizeForLatency: true, description: this._bridge.video_config_description() });
    this._decoderConfigured = true;
  }

  _pumpAudioInner() {
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

  _pumpSubtitlesInner() {
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
