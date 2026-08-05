// @firemedia/skyfire-player — SkyfirePlayer: turnkey in-browser DVB TV player.
//
// Extracted from web/player.js (behaviour-preserving — ADR 0008).
// The browser owns presentation + control; SkyfireBridge parses the
// MPEG-TS and hands progressive H.264 access units up to WebCodecs.

import { initSkyfire, SkyfireBridge, PTS_HZ, ticksToMicros } from "@firemedia/skyfire-core";
import { makeSource } from "./hls-source.js";
import { trackSignature, diffTracks, pickFallbackAudio } from "./tracks.js";

// index.d.ts declares these as package exports; without re-exporting them
// here the entry point only ever exposes `SkyfirePlayer`, and a TS consumer
// following the typings gets a hard ESM link error at import time.
export { languageName, resolveLocale } from "./lang.js";
export { trackSignature, diffTracks, pickFallbackAudio } from "./tracks.js";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Decide whether to reconnect a dropped live stream, and how long to wait.
 *
 * The budget counts **consecutive** failures. Any progress since the last
 * failure resets it, so a long session punctuated by occasional transient
 * drops never exhausts it — only a stream that is genuinely not recovering
 * does. Previously the counter lived outside the retry loop and was never
 * reset, so five transient drops spread over an entire session ended playback
 * with "stream failed"; against a live packager that races its own playlist
 * (zenith#1205) that budget was gone within minutes.
 *
 * @param {{attempt: number, max: number, progressed: boolean}} state
 *   `attempt` consecutive failures so far, `max` the budget, `progressed`
 *   whether any media was consumed since the previous failure.
 * @returns {{reconnect: boolean, attempt: number, delayMs: number}}
 */
export function reconnectDecision({ attempt, max, progressed }) {
  const base = progressed ? 0 : attempt;
  if (base >= max) return { reconnect: false, attempt: base, delayMs: 0 };
  const next = base + 1;
  return { reconnect: true, attempt: next, delayMs: Math.min(1500 * next, 8000) };
}

/**
 * Turnkey in-browser DVB player.
 *
 * @example
 * const player = new SkyfirePlayer(canvas, { streamUrl: "/stream.ts" });
 * player.on("tracks", (tl) => console.log(tl));
 * await player.init();
 */
export class SkyfirePlayer {
  /**
   * @param {HTMLCanvasElement} canvas - The video canvas element.
   * @param {object} opts
   * @param {string}  opts.streamUrl       - TS stream URL.
   * @param {number}  [opts.audioPid]      - Pre-select audio PID.
   * @param {number}  [opts.subtitlePid]   - Pre-select subtitle PID (auto-starts compositing).
   * @param {boolean} [opts.muted]         - Start muted.
   * @param {boolean} [opts.forceMse]      - Force MSE video path (skip WebCodecs).
   */
  constructor(canvas, opts = {}) {
    if (!canvas) throw new Error("SkyfirePlayer: canvas required");
    if (!opts.streamUrl) throw new Error("SkyfirePlayer: opts.streamUrl required");

    this.canvas = canvas;
    this.opts = opts;
    this.streamUrl = opts.streamUrl;

    // ── event emitter ────────────────────────────────────────────────────────
    this._listeners = { tracks: [], stats: [], error: [], ended: [] };

    // ── bridge ───────────────────────────────────────────────────────────────
    this.bridge = null;

    // ── canvas 2D context ────────────────────────────────────────────────────
    this._ctx = canvas.getContext("2d", { alpha: false });
    this._sized = false;

    // ── shared stats object ───────────────────────────────────────────────────
    this._stats = {
      decoded: 0, drawn: 0, dropped: 0, w: 0, h: 0, aus: 0, path: "wc",
      audioChunks: 0, audioSamples: 0, audioFrames: 0, audioSec: 0, avSkewMs: 0,
      videoPath: "", mseSegments: 0, videoCurrentTime: 0,
      tracks: { audio: [], subtitle: [] },
      selectedAudio: null, decodedAudioPid: null, subCues: 0,
    };

    // ── video decoder ─────────────────────────────────────────────────────────
    this._videoDecoder = null;
    this._decoderConfigured = false;
    this._sawKeyframe = false;

    // ── MSE video fallback ────────────────────────────────────────────────────
    this._videoPath = null;          // "webcodecs" | "mse" | null
    this._mseVideoEl = null;
    this._mseMediaSource = null;
    this._mseSourceBuffer = null;
    this._mseBufferQueue = [];
    this._mseAppending = false;
    this._mseDriftRaf = null;

    // ── audio-master A/V sync ─────────────────────────────────────────────────
    this._presentQueue = [];
    this._firstAudioPtsUs = null;
    this._audioFramesPlayed = 0;
    this._audioSamplesFed = 0;   // interleaved PCM samples posted to the worklet
    this._videoWallAnchorMs = null;  // wall-clock anchor for video pacing sans audio
    this._videoWallAnchorUs = 0;
    this._postered = false;          // drew the held first frame while awaiting audio
    this._lastFp = 0;                // last observed framesPlayed (stall detection)
    this._lastFpAdvanceMs = 0;       // wall ms when framesPlayed last advanced
    this._lastMediaUs = null;        // last media time the audio clock reached
    this._firstFrameMs = null;       // wall ms the first video frame was queued
    this._AUDIO_STALL_MS = 600;      // audio clock considered stalled after this idle
    this._HOLD_MAX_MS = 1500;        // max initial hold waiting for audio to start
    this._audioSampleRate = 48000;
    this._presentScheduled = false;
    this._audioPcmQueue = []; // PCM chunks drained from the bridge, paced out to the worklet
    this._audioDraining = false; // end-of-stream drain in progress

    // ── audio ─────────────────────────────────────────────────────────────────
    this._audioCtx = null;
    this._audioNode = null;
    this._audioGain = null;
    this._audioReady = false;
    this._audioStarting = false;
    this._streamChannels = 0;
    this._outputChannels = 0;
    this._downmixActive = false;

    // ── transport ─────────────────────────────────────────────────────────────
    this._playing = true;
    this._volume = 1;
    this._muted = opts.muted || false;
    this._destroyed = false;
    this._fetchAbortController = null;

    // ── subtitle overlay ──────────────────────────────────────────────────────
    // The player creates a sibling canvas for the subtitle overlay. It is
    // inserted as an absolutely-positioned child of the canvas's parent so it
    // covers the video exactly. The parent must have position:relative (or any
    // non-static) for the overlay to track the canvas; the host is responsible
    // for that (or wrapping in a container).
    this._subsCanvas = null;
    this._subCtx = null;
    this._shownSubKey = null;
    this._subQueue = [];
    this._lastVideoTs = 0;

    // ── re-entrancy guard ─────────────────────────────────────────────────────
    this._bridgeLocked = false;
    this._pendingBridgeQueue = [];

    // ── track list ────────────────────────────────────────────────────────────
    this._trackList = null;
    this._trackSig = null;   // identity of the current track set — re-emit tracks when it changes

    // ── MSE drift constants ───────────────────────────────────────────────────
    this._MSE_DRIFT_SEEK_THRESH = 0.25;
    this._MSE_DRIFT_NUDGE_THRESH = 0.05;

    // ── late-drop / lead constants ────────────────────────────────────────────
    this._LATE_DROP_US = 80_000;
    this._LEAD_US = 12_000;

    // Feed backpressure: keep the pipeline at most this many seconds of audio
    // ahead of the play clock, so the worklet's fixed ring never overflows
    // (overflow drops audio and freezes the audio-master clock → video stalls).
    // 10 s absorbs network/segment jitter; must stay below the worklet ring
    // (16 s) minus one segment burst. Lower it for low-latency live via opts.
    this._AUDIO_LEAD_S = opts.audioLeadSeconds ?? 10;

    // Bind user-gesture audio resume so we can remove it on destroy.
    this._startAudioBound = () => this._startAudio();
  }

  // ── public event subscription ─────────────────────────────────────────────

  on(event, cb) {
    (this._listeners[event] ||= []).push(cb);
  }

  _emit(event, data, extra) {
    (this._listeners[event] || []).forEach((cb) => cb(data, extra));
  }

  // ── public transport ──────────────────────────────────────────────────────

  play() {
    if (this._destroyed) return;
    this._playing = true;
    this._callBridge("set_playing", true);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "play" });
    this._startAudio();
  }

  pause() {
    if (this._destroyed) return;
    this._playing = false;
    this._callBridge("set_playing", false);
    if (this._audioNode) this._audioNode.port.postMessage({ type: "pause" });
  }

  selectAudio(pid) {
    if (this._destroyed) return;
    this._callBridge("select_audio", pid);
    this._stats.selectedAudio = pid;
    this._status(`audio → pid ${pid}`);
  }

  selectSubtitle(pid) {
    if (this._destroyed) return;
    if (pid == null) {
      this._callBridge("select_subtitle", undefined);
    } else {
      this._callBridge("select_subtitle", pid);
    }
    // Clear current cue on switch/off.
    if (this._subCtx) {
      this._subCtx.clearRect(0, 0, this._subCtx.canvas.width, this._subCtx.canvas.height);
      this._shownSubKey = null;
    }
  }

  /** Set output volume, 0..1 (clamped). Applies to the gain node if audio is up. */
  setVolume(v) {
    this._volume = Math.max(0, Math.min(1, Number(v) || 0));
    if (this._audioGain) this._audioGain.gain.value = this._muted ? 0 : this._volume;
  }

  /** @returns {number} current volume 0..1. */
  getVolume() {
    return this._volume;
  }

  /** Mute/unmute without losing the volume level. */
  setMuted(muted) {
    this._muted = !!muted;
    if (this._audioGain) this._audioGain.gain.value = this._muted ? 0 : this._volume;
  }

  /** @returns {object|null} The current track list, or null if not yet available. */
  tracks() {
    return this._trackList || null;
  }

  // ── lifecycle ─────────────────────────────────────────────────────────────

  /**
   * Load WASM, construct the bridge, apply opts, start the stream.
   * @returns {Promise<void>}
   */
  async init() {
    if (this._destroyed) throw new Error("SkyfirePlayer: already destroyed");

    // Create the subtitle overlay canvas now (before the stream starts).
    this._createSubsCanvas();

    // Wire user-gesture audio resume.
    window.addEventListener("pointerdown", this._startAudioBound, { once: true });
    window.addEventListener("keydown", this._startAudioBound, { once: true });
    // Expose for Playwright/iOS verifier.
    window.sfStartAudio = () => this._startAudio();

    this._status("Loading WASM…");
    await initSkyfire();
    this.bridge = new SkyfireBridge();

    // Apply pre-selected subtitle PID from opts before streaming starts.
    if (this.opts.subtitlePid != null) {
      this.bridge.select_subtitle(this.opts.subtitlePid);
      this._status(`subtitle → pid ${this.opts.subtitlePid}`);
    }

    const src = this.streamUrl;
    this._isLive = false;

    this._videoPath = null;

    const MAX_RECONNECT = 5;
    let attempt = 0;

    for (;;) {
      // Progress marker: any video AU consumed during this attempt means the
      // stream was alive, so the next failure starts a fresh budget rather than
      // inheriting one spent hours ago. See reconnectDecision.
      const ausBefore = this._stats.aus;
      try {
        await this._consumeStream(src);
      } catch (e) {
        const d = this._isLive
          ? reconnectDecision({
              attempt,
              max: MAX_RECONNECT,
              progressed: this._stats.aus > ausBefore,
            })
          : { reconnect: false };
        if (d.reconnect) {
          attempt = d.attempt;
          this._status(`stream dropped — reconnecting (${attempt}/${MAX_RECONNECT})…`);
          this._sawKeyframe = false;
          await sleep(d.delayMs);
          continue;
        }
        this._fatal("stream failed", e);
        return;
      }
      {
        const d = this._isLive
          ? reconnectDecision({
              attempt,
              max: MAX_RECONNECT,
              progressed: this._stats.aus > ausBefore,
            })
          : { reconnect: false };
        if (d.reconnect) {
          attempt = d.attempt;
          this._status(`stream ended — reconnecting (${attempt}/${MAX_RECONNECT})…`);
          this._sawKeyframe = false;
          await sleep(1000);
          continue;
        }
      }
      break;
    }

    // End of stream — flush the bridge, drain, then flush the decoder.
    this._callBridge(() => {
      this.bridge.flush();
      if (this._videoPath === "webcodecs") this._pumpVideoInner();
      this._pumpSubtitlesInner();
    });
    // Drain every decoded PCM chunk into the worklet AND wait for it to be
    // actually played out (play-ahead drops to ~0). Reporting `done` while a
    // segment of audio was still queued-but-unplayed is the #91 tail-loss; this
    // is what makes end-of-stream distinguishable from a mid-stream freeze.
    await this._drainAudioToCompletion();
    if (this._videoPath === "webcodecs" && this._videoDecoder && this._decoderConfigured) {
      try { await this._videoDecoder.flush(); } catch (e) { console.warn("[skyfire] flush", e); }
    }
    if (this._videoPath === "mse" && this._mseSourceBuffer && this._mseMediaSource) {
      try {
        this._drainMseBufferQueue();
        if (this._mseMediaSource.readyState === "open") this._mseMediaSource.endOfStream();
      } catch (e) { console.warn("[skyfire] MSE endOfStream", e); }
    }

    const s = this._stats;
    // `done` is a persistent field so a later stats emission (drawFrame/status)
    // can never clear it — a host must be able to read "finished" at any time.
    this._stats.done = true;
    this._status(
      `done — video ${s.decoded}f/${s.drawn}drawn, audio ${s.audioChunks} chunks / ${s.audioSamples} samples, played ${s.audioSec.toFixed(1)}s`
    );
    this._emit("stats", { ...this._stats });
    this._emit("ended", { ...this._stats });
  }

  /**
   * Tear down all resources: VideoDecoder, MediaSource, AudioContext, rAF, fetch.
   */
  destroy() {
    if (this._destroyed) return;
    this._destroyed = true;

    // Abort in-flight fetch.
    if (this._fetchAbortController) {
      try { this._fetchAbortController.abort(); } catch (_) {}
      this._fetchAbortController = null;
    }
    try { this._source?.cancel(); } catch (_) {}

    // Cancel MSE drift rAF.
    if (this._mseDriftRaf) {
      cancelAnimationFrame(this._mseDriftRaf);
      this._mseDriftRaf = null;
    }

    // Close VideoDecoder.
    if (this._videoDecoder) {
      try { this._videoDecoder.close(); } catch (_) {}
      this._videoDecoder = null;
    }

    // Close MediaSource.
    if (this._mseMediaSource) {
      try {
        if (this._mseMediaSource.readyState === "open") {
          this._mseMediaSource.endOfStream();
        }
      } catch (_) {}
      this._mseMediaSource = null;
    }

    // Remove MSE video element.
    if (this._mseVideoEl) {
      try { this._mseVideoEl.remove(); } catch (_) {}
      this._mseVideoEl = null;
    }

    // Close AudioContext.
    if (this._audioCtx) {
      try { this._audioCtx.close(); } catch (_) {}
      this._audioCtx = null;
    }

    // Close open VideoFrames in the present queue.
    for (const e of this._presentQueue) {
      try { e.frame.close(); } catch (_) {}
    }
    this._presentQueue.length = 0;

    // Free PCM chunks still queued for the worklet (not yet posted).
    for (const c of this._audioPcmQueue) {
      try { c.free?.(); } catch (_) {}
    }
    this._audioPcmQueue.length = 0;

    // Remove subtitle overlay canvas.
    if (this._subsCanvas) {
      try { this._subsCanvas.remove(); } catch (_) {}
      this._subsCanvas = null;
      this._subCtx = null;
    }

    // Remove user-gesture listeners.
    window.removeEventListener("pointerdown", this._startAudioBound);
    window.removeEventListener("keydown", this._startAudioBound);

    // Nullify bridge last (other teardown may call callBridge which checks for null).
    this.bridge = null;
  }

  // ── status / error helpers ─────────────────────────────────────────────────

  _status(msg) {
    console.log("[skyfire]", msg);
    this._emit("stats", { ...this._stats, status: msg });
  }

  _fatal(msg, err) {
    const text = msg + (err ? "\n" + (err.message || err) : "");
    console.error("[skyfire]", msg, err);
    this._emit("error", { message: text, cause: err });
  }

  // ── subtitle overlay canvas ───────────────────────────────────────────────
  //
  // Instead of using a page-level getElementById("subs"), the player creates
  // its own overlay canvas and positions it absolutely over this.canvas.
  // The canvas's parent element must have position != static for this to work.

  _createSubsCanvas() {
    const c = document.createElement("canvas");
    c.width = this.canvas.width || 1920;
    c.height = this.canvas.height || 1080;
    c.style.position = "absolute";
    c.style.top = "0";
    c.style.left = "0";
    c.style.width = "100%";
    c.style.height = "100%";
    c.style.pointerEvents = "none";
    // Insert after the main canvas in the DOM.
    const parent = this.canvas.parentElement;
    if (parent) {
      const next = this.canvas.nextSibling;
      parent.insertBefore(c, next);
    }
    this._subsCanvas = c;
    this._subCtx = c.getContext("2d");
  }

  _ensureSubsCanvas() {
    if (!this._subCtx) return null;
    const cw = this.canvas.width || 1920;
    const ch = this.canvas.height || 1080;
    if (this._subCtx.canvas.width !== cw || this._subCtx.canvas.height !== ch) {
      this._subCtx.canvas.width = cw;
      this._subCtx.canvas.height = ch;
      this._shownSubKey = null;
    }
    return this._subCtx;
  }

  _clearSubs() {
    if (this._subCtx) {
      this._subCtx.clearRect(0, 0, this._subCtx.canvas.width, this._subCtx.canvas.height);
    }
    this._shownSubKey = null;
  }

  _drawSubCue(cue) {
    const cx = this._ensureSubsCanvas();
    if (!cx) return;
    cx.clearRect(0, 0, cx.canvas.width, cx.canvas.height);
    for (const r of cue.regions) {
      if (!r.rgba || !r.width || !r.height) continue;
      cx.putImageData(new ImageData(new Uint8ClampedArray(r.rgba), r.width, r.height), r.x, r.y);
    }
  }

  _renderSubs(clockUs) {
    if (clockUs == null) return;
    const subQueue = this._subQueue;
    while (subQueue.length && subQueue[0].endUs <= clockUs) {
      if (this._shownSubKey === subQueue[0].key) this._clearSubs();
      subQueue.shift();
    }
    const active = subQueue.find((c) => c.startUs <= clockUs && clockUs < c.endUs);
    if (active) {
      if (this._shownSubKey !== active.key) {
        this._drawSubCue(active);
        this._shownSubKey = active.key;
      }
    } else if (this._shownSubKey !== null) {
      this._clearSubs();
    }
  }

  // ── canvas frame draw ─────────────────────────────────────────────────────

  _drawFrame(frame) {
    try {
      const c = this.canvas;
      const s = this._stats;
      if (!this._sized || c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
        c.width = frame.displayWidth;
        c.height = frame.displayHeight;
        this._sized = true;
      }
      this._ctx.drawImage(frame, 0, 0, c.width, c.height);
      s.drawn++;
      s.w = frame.displayWidth;
      s.h = frame.displayHeight;
      this._lastVideoTs = frame.timestamp;
      s.videoCurrentTime = frame.timestamp / 1_000_000;
      // Refresh decoded-audio stat from the bridge on every frame draw so
      // the test harness sees the genuinely decoded PID, not just what was
      // requested. `_pumpAudioInner` also writes these, but it only runs on
      // feed ticks — a stalled feed (backpressure) produces no ticks, yet
      // the bridge's internal state (decoded_audio_pid, last_audio_channels)
      // is always current.
      this._refreshDecodedPid();
      this._emit("stats", { ...s });
    } finally {
      frame.close();
    }
  }

  /** Refresh decodedAudioPid and nativeChannels from the bridge. Called on
   *  every frame draw so stats stay current even when the feed is stalled. */
  _refreshDecodedPid() {
    if (!this.bridge) return;
    if (typeof this.bridge.current_decoded_pid === "function") {
      this._stats.decodedAudioPid = this.bridge.current_decoded_pid() ?? null;
    }
    if (typeof this.bridge.audio_native_channels === "function") {
      this._stats.nativeChannels = this.bridge.audio_native_channels();
    }
  }

  // ── audio-master clock ────────────────────────────────────────────────────

  _audioClockUs() {
    if (this._firstAudioPtsUs === null || this._audioFramesPlayed === 0) return null;
    return this._firstAudioPtsUs + (this._audioFramesPlayed / this._audioSampleRate) * 1_000_000;
  }

  /** True once audio playback has begun (framesPlayed advanced past 0). */
  _audioStarted() {
    return this._firstAudioPtsUs !== null && this._audioFramesPlayed > 0;
  }

  /**
   * True when the audio-master clock is actually ADVANCING (framesPlayed moved
   * within the last `_AUDIO_STALL_MS`). A started-but-frozen clock (audio underrun,
   * device hiccup, suspended context) is NOT live — the pipeline must then fall
   * back to wall-clock so video keeps playing and the feed keeps flowing, instead
   * of freezing on a dead clock (the "stops ~15s in" live halt, zenith #84).
   */
  _audioClockLive() {
    return this._audioStarted() && performance.now() - this._lastFpAdvanceMs < this._AUDIO_STALL_MS;
  }

  /** Seconds of decoded audio fed to the worklet but not yet played (buffer-ahead). */
  _audioAheadSeconds() {
    const ch = this._outputChannels || this._streamChannels || 2;
    const fedFrames = this._audioSamplesFed / ch;
    return (fedFrames - this._audioFramesPlayed) / this._audioSampleRate;
  }

  _schedulePresent() {
    if (this._presentScheduled) return;
    this._presentScheduled = true;
    requestAnimationFrame(() => this._present());
  }

  /**
   * Wall-clock media time, anchored to the first queued frame. Used to pace video
   * at 1× real time when there is no audio-master clock yet (audio not started, or
   * a video-only stream). Without this the present loop drew one frame per
   * animation-frame (~60 Hz) → video ran ~2× fast until audio took over.
   */
  _videoClockUs() {
    if (!this._presentQueue.length) return null;
    const now = performance.now();
    if (this._videoWallAnchorMs === null) {
      this._videoWallAnchorMs = now;
      // Anchor at the last media time the audio clock reached (so a mid-stream
      // audio stall continues video from where it was, not from the queue head),
      // else at the first queued frame.
      this._videoWallAnchorUs =
        this._lastMediaUs != null ? this._lastMediaUs : this._presentQueue[0].ts;
    }
    return this._videoWallAnchorUs + (now - this._videoWallAnchorMs) * 1000;
  }

  _present() {
    if (this._destroyed) return;
    this._presentScheduled = false;
    let clock;
    if (this._audioClockLive()) {
      // Audio master (clock actively advancing). Drop the wall anchor so a later
      // stall re-anchors from the current media time.
      clock = this._audioClockUs();
      this._videoWallAnchorMs = null;
      this._lastMediaUs = clock;
    } else if (this._audioSamplesFed > 0 && !this._audioStarted()
               && (this._firstFrameMs == null || performance.now() - this._firstFrameMs < this._HOLD_MAX_MS)) {
      // Audio is decoding but hasn't STARTED yet — hold briefly (up to _HOLD_MAX_MS)
      // so A/V begin together when audio kicks in. Bounded: if audio never starts
      // (autoplay blocked, no gesture), we fall through to wall-clock below rather
      // than freeze video forever.
      if (!this._postered && this._presentQueue.length) {
        this._postered = true;
        const e = this._presentQueue.shift();
        this._drawFrame(e.frame);
      }
      if (this._presentQueue.length || this._subQueue.length) this._schedulePresent();
      return;
    } else {
      // No audio, audio never started within the grace window, or audio started
      // then STALLED — pace video by wall clock so it never freezes (zenith #84).
      clock = this._videoClockUs();
    }

    if (clock === null) {
      // No clock at all yet (first frame not queued) — nothing to pace.
      if (this._presentQueue.length || this._subQueue.length) this._schedulePresent();
      return;
    }

    while (this._presentQueue.length) {
      const e = this._presentQueue[0];
      if (e.ts > clock + this._LEAD_US) break;
      this._presentQueue.shift();
      if (e.ts < clock - this._LATE_DROP_US) {
        e.frame.close();
        this._stats.dropped++;
        continue;
      }
      this._drawFrame(e.frame);
      this._stats.avSkewMs = Math.round((clock - e.ts) / 1000);
    }
    this._renderSubs(clock);
    if (this._presentQueue.length || this._subQueue.length) this._schedulePresent();
  }

  // ── WebCodecs video decoder ───────────────────────────────────────────────

  _ensureDecoder(codec) {
    if (this._decoderConfigured) return true;

    this._videoDecoder = new VideoDecoder({
      output: (frame) => {
        this._stats.decoded++;
        if (this._firstFrameMs == null) this._firstFrameMs = performance.now();
        this._presentQueue.push({ frame, ts: frame.timestamp });
        this._schedulePresent();
      },
      error: (e) => { this._fatal("VideoDecoder error", e); },
    });

    const avcc = this.bridge.video_config_description();
    this._videoDecoder.configure({ codec, description: avcc, optimizeForLatency: true });
    this._decoderConfigured = true;
    this._status(`VideoDecoder configured: ${codec} (AVCC, description ${avcc.length} bytes)`);
    return true;
  }

  _pumpVideoInner() {
    const cs = this.bridge.video_codec();
    if (!cs) return;
    if (!this._ensureDecoder(cs)) return;

    for (const au of this.bridge.take_video_aus()) {
      this._stats.aus++;
      const key = au.is_keyframe;
      if (!this._sawKeyframe) {
        if (!key) { au.free?.(); continue; }
        this._sawKeyframe = true;
      }
      const ts = au.pts_ticks !== undefined ? ticksToMicros(au.pts_ticks) : 0;
      try {
        this._videoDecoder.decode(new EncodedVideoChunk({
          type: key ? "key" : "delta",
          timestamp: ts,
          data: au.bytes,
        }));
      } catch (e) {
        this._fatal("decode() threw", e);
        return;
      }
      au.free?.();
    }
  }

  _pumpVideo() {
    this._callBridge(() => this._pumpVideoInner());
  }

  // ── audio: WASM PCM → WebAudio AudioWorklet ───────────────────────────────

  async _ensureAudio(sampleRate, nativeChannels) {
    if (this._audioReady || this._audioStarting) return;
    this._audioStarting = true;
    this._streamChannels = nativeChannels;

    this._audioCtx = new AudioContext({ sampleRate });
    const maxCh = this._audioCtx.destination.maxChannelCount;

    const passthrough = nativeChannels > 2 && nativeChannels <= maxCh;
    // Downmix ONLY genuinely multichannel audio (>2ch) that we cannot pass through.
    // Feeding already-stereo (or mono) audio through the 5.1→stereo matrix corrupts
    // a channel (left-channel full-scale clicks on france-2). ≤2ch must pass as-is.
    const downmix = nativeChannels > 2 && !passthrough;
    this.bridge.set_audio_downmix(downmix);
    this._outputChannels = passthrough ? nativeChannels : Math.min(nativeChannels, 2);
    this._downmixActive = downmix;

    // Resolve the worklet relative to THIS module (the package), not the page —
    // addModule() otherwise resolves against the document base, which 404s in any
    // consumer app that isn't serving its own ./audio-worklet.js. `new URL(...,
    // import.meta.url)` points at the shipped packages/player/audio-worklet.js and
    // is the pattern bundlers (vite/webpack) emit as an asset.
    await this._audioCtx.audioWorklet.addModule(new URL("./audio-worklet.js", import.meta.url));
    this._audioNode = new AudioWorkletNode(this._audioCtx, "skyfire-pcm", {
      numberOfOutputs: 1,
      outputChannelCount: [this._outputChannels],
      channelCountMode: "explicit",
      channelInterpretation: passthrough ? "discrete" : "speakers",
    });
    if (passthrough && this._audioCtx.destination.channelCount < this._outputChannels) {
      this._audioCtx.destination.channelCount = this._outputChannels;
    }
    this._audioSampleRate = this._audioCtx.sampleRate || sampleRate;
    this._audioNode.port.onmessage = (e) => {
      if (e.data.type === "clock") {
        if (e.data.framesPlayed > this._lastFp) {
          this._lastFp = e.data.framesPlayed;
          this._lastFpAdvanceMs = performance.now();   // clock is live
        }
        this._audioFramesPlayed = e.data.framesPlayed;
        this._audioUnderruns = e.data.underruns ?? 0;
        this._stats.audioFrames = this._audioFramesPlayed;
        this._stats.audioUnderruns = this._audioUnderruns;
        this._stats.audioSec = this._audioFramesPlayed / this._audioSampleRate;
        this._schedulePresent();
      }
    };
    this._audioGain = this._audioCtx.createGain();
    this._audioGain.gain.value = this._muted ? 0 : this._volume;
    this._audioNode.connect(this._audioGain).connect(this._audioCtx.destination);
    this._audioNode.port.postMessage({ type: "config", sampleRate, outputChannels: this._outputChannels });
    this._audioNode.port.postMessage({ type: "play" });
    this._audioCtx.resume().catch(() => {});
    this._audioReady = true;
    this._audioStarting = false;

    const label = this._downmixActive
      ? `${this._streamChannels}→${this._outputChannels} ch (WASM downmix)`
      : `${this._outputChannels} ch${this._outputChannels > 2 ? " (discrete passthrough)" : ""}`;
    this._status(`audio: ${sampleRate} Hz, ${label}`);
  }

  async _pumpAudioInner() {
    // Drain every decoded chunk out of the bridge and into our JS-side queue
    // so the bridge never accumulates PCM across ticks. Channel normalisation /
    // graph (re)creation happens here, once per chunk, at drain time.
    for (const c of this.bridge.take_audio_pcm()) {
      if (!this._audioReady) {
        // eslint-disable-next-line no-await-in-loop
        await this._ensureAudio(c.sample_rate, this.bridge.audio_native_channels() || c.channels);
      }
      if (c.channels !== this._outputChannels) {
        // Audio track switch can change channel layout (e.g. AC-3 5.1 → MP2
        // stereo on orf1). Tear down and recreate the audio graph.
        this._status(
          `audio channel change: ${this._outputChannels} → ${c.channels}; reconfiguring`
        );
        this._audioReady = false;
        this._audioStarting = false;
        if (this._audioCtx) {
          try { this._audioCtx.close(); } catch (_) {}
        }
        this._audioCtx = null;
        this._audioNode = null;
        this._audioGain = null;
        this._firstAudioPtsUs = null;
        this._audioFramesPlayed = 0;
        this._audioSamplesFed = 0;
        this._lastFp = 0;
        this._lastFpAdvanceMs = 0;
        this._lastMediaUs = null;
        await this._ensureAudio(c.sample_rate, this.bridge.audio_native_channels() || c.channels);
      }
      if (!this._audioReady) {
        // eslint-disable-next-line no-await-in-loop
        await this._ensureAudio(c.sample_rate, this.bridge.audio_native_channels() || c.channels);
      }
      if (c.channels !== this._outputChannels) {
        this._stats.audioDropped = (this._stats.audioDropped || 0) + 1;
        this._lastDropChannels = c.channels;
        c.free?.();
        continue;
      }
      this._audioPcmQueue.push(c);
    }

    // Post from the queue to the worklet, paced by the play-ahead so we NEVER
    // overflow the worklet's fixed ring (16 s). Posting everything in one burst
    // (what the feed did before) overflowed the ring and silently dropped the
    // oldest audio whenever a clip decoded faster than it played — the #91
    // tail-loss. Only post while the already-buffered audio stays below the
    // configured lead; the end-of-stream drain loop tops the queue back up on
    // later ticks as the playhead catches up.
    while ((this._audioAheadSeconds() < this._AUDIO_LEAD_S) && this._audioPcmQueue.length) {
      const c = this._audioPcmQueue.shift();
      if (this._firstAudioPtsUs === null && c.pts_ticks !== undefined) {
        this._firstAudioPtsUs = ticksToMicros(c.pts_ticks);
      }
      const samples = c.samples;
      this._stats.audioChunks++;
      this._stats.audioSamples += samples.length;
      if (this.bridge && typeof this.bridge.current_decoded_pid === "function") {
        this._stats.decodedAudioPid = this.bridge.current_decoded_pid() ?? null;
      }
      if (this.bridge && typeof this.bridge.audio_native_channels === "function") {
        this._stats.nativeChannels = this.bridge.audio_native_channels();
      }
      this._audioSamplesFed += samples.length;
      this._audioNode.port.postMessage({ type: "pcm", samples }, [samples.buffer]);
      c.free?.();
    }
  }

  async _pumpAudio() {
    this._callBridge(() => this._pumpAudioInner());
  }

  /**
   * At the end of a FINITE stream, wait for every decoded PCM chunk to be both
   * handed to the worklet AND actually played out (play-ahead drains to ~0),
   * before the caller reports the stream as ended. Without this the player
   * declared `done` while roughly a segment of buffered audio was still
   * queued-but-unplayed — the other half of #91. Bounded by a generous wall
   * timeout so a genuinely stuck clock cannot hang the caller forever.
   * Hard ends after `maxMs` and lets the caller proceed regardless.
   */
  async _drainAudioToCompletion(maxMs = 60_000) {
    if (this._audioDraining) return;
    this._audioDraining = true;
    const t0 = performance.now();
    try {
      for (;;) {
        if (performance.now() - t0 > maxMs) return;
        await this._pumpAudio();
        const allPosted = this._audioPcmQueue.length === 0;
        const nearZero =
          this._audioAheadSeconds() != null && this._audioAheadSeconds() <= 0.05;
        // All chunks handed to the worklet and played out, OR there is no audio
        // to play at all (video-only / autoplay-blocked) — either way the stream
        // is consumed, so report the end instead of waiting on a dead clock.
        if (allPosted && (nearZero || !this._audioStarted())) break;
        await sleep(40);
      }
    } finally {
      this._audioDraining = false;
    }
  }

  _startAudio() {
    if (this._audioCtx && this._audioCtx.state === "suspended") {
      this._audioCtx.resume().catch(() => {});
    }
  }

  // ── subtitles ─────────────────────────────────────────────────────────────

  _pumpSubtitlesInner() {
    if (!this.bridge.take_subtitle_cues) return;
    let added = false;
    for (const cue of this.bridge.take_subtitle_cues()) {
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
      this._stats.subCues = (this._stats.subCues || 0) + 1;
      added = true;
      cue.free?.();
    }
    if (added) this._schedulePresent();
  }

  _pumpSubtitles() {
    this._callBridge(() => this._pumpSubtitlesInner());
  }

  // ── re-entrancy guard ─────────────────────────────────────────────────────

  _callBridge(method, ...args) {
    if (!this.bridge) return;

    if (this._bridgeLocked) {
      this._stats._bridgeReentries = (this._stats._bridgeReentries || 0) + 1;
      if (typeof method === "function") {
        this._pendingBridgeQueue.push(method);
      } else {
        const m = method;
        this._pendingBridgeQueue.push(() => this.bridge[m](...args));
      }
      return undefined;
    }

    this._bridgeLocked = true;
    try {
      if (typeof method === "function") return method();
      return this.bridge[method](...args);
    } finally {
      this._bridgeLocked = false;
      while (this._pendingBridgeQueue.length > 0) {
        const fn = this._pendingBridgeQueue.shift();
        if (this._bridgeLocked) { this._pendingBridgeQueue.unshift(fn); break; }
        this._bridgeLocked = true;
        try { fn(); } finally { this._bridgeLocked = false; }
      }
    }
  }

  // ── capability gate: decide video path ───────────────────────────────────

  async _decideVideoPath(codec) {
    if (this._videoPath !== null) return;

    const forceMse = this.opts.forceMse || false;

    if (forceMse) {
      this._videoPath = "mse";
      this._stats.videoPath = "mse";
      this._status("MSE video fallback (forced via opts.forceMse)");
      this._setupMse(codec);
      return;
    }

    if (typeof VideoDecoder !== "undefined") {
      try {
        const cfg = { codec, optimizeForLatency: true };
        const sup = await VideoDecoder.isConfigSupported(cfg);
        if (sup.supported) {
          this._videoPath = "webcodecs";
          this._stats.videoPath = "webcodecs";
          this._status(`WebCodecs path: ${codec}`);
          return;
        }
      } catch (_) { /* fall through to MSE */ }
    }
    this._videoPath = "mse";
    this._stats.videoPath = "mse";
    this._status(`MSE video fallback: ${codec}`);
    this._setupMse(codec);
  }

  // ── MSE video fallback helpers ────────────────────────────────────────────

  _setupMse(codec) {
    const mime = `video/mp4; codecs="${codec}"`;
    if (!MediaSource.isTypeSupported(mime)) {
      this._fatal(`MSE: type not supported — ${mime}`);
      return;
    }

    this._mseVideoEl = document.createElement("video");
    this._mseVideoEl.muted = true;
    this._mseVideoEl.playsInline = true;
    this._mseVideoEl.style.display = "block";
    this._mseVideoEl.style.width = "100%";
    this._mseVideoEl.style.height = "auto";
    const container = this.canvas.parentElement || document.body;
    container.insertBefore(this._mseVideoEl, this.canvas.nextSibling || null);

    this._mseMediaSource = new MediaSource();
    this._mseVideoEl.src = URL.createObjectURL(this._mseMediaSource);

    this._mseMediaSource.addEventListener("sourceopen", () => {
      try {
        this._mseSourceBuffer = this._mseMediaSource.addSourceBuffer(mime);
      } catch (e) {
        this._fatal("MSE addSourceBuffer", e);
        return;
      }

      this._mseSourceBuffer.addEventListener("updateend", () => {
        this._drainMseBufferQueue();
      });

      const initSeg = this.bridge.video_init_segment();
      if (initSeg && initSeg.length > 0) {
        this._mseSourceBuffer.appendBuffer(initSeg);
      }

      this._mseVideoEl.play().catch((e) => console.warn("[skyfire] MSE play", e));
      this._startMseDriftCorrector();
    });
  }

  _drainMseBufferQueue() {
    if (!this._mseSourceBuffer || this._mseAppending) return;
    if (this._mseBufferQueue.length === 0) return;
    if (this._mseSourceBuffer.updating) return;

    this._mseAppending = true;
    const seg = this._mseBufferQueue.shift();
    try {
      this._mseSourceBuffer.appendBuffer(seg);
    } catch (e) {
      console.warn("[skyfire] MSE appendBuffer error", e);
    }
    this._mseAppending = false;
  }

  _queueMseAppend(buf) {
    this._mseBufferQueue.push(buf);
    if (this._mseSourceBuffer && !this._mseSourceBuffer.updating) {
      this._drainMseBufferQueue();
    }
  }

  _pumpVideoMseInner() {
    if (this._videoPath !== "mse") return;
    for (;;) {
      const seg = this.bridge.take_video_media_segment();
      if (!seg) break;
      this._stats.mseSegments++;
      this._queueMseAppend(seg.bytes);
    }
  }

  _pumpVideoMse() {
    this._callBridge(() => this._pumpVideoMseInner());
  }

  _startMseDriftCorrector() {
    if (this._mseDriftRaf) return;

    const corrector = () => {
      if (this._destroyed) return;
      if (!this._mseVideoEl || !this._mseSourceBuffer) return;

      const vt = this._mseVideoEl.currentTime;
      this._stats.videoCurrentTime = vt;

      if (this._mseVideoEl.buffered.length > 0 && vt < this._mseVideoEl.buffered.start(0)) {
        this._mseVideoEl.currentTime = this._mseVideoEl.buffered.start(0);
        this._mseVideoEl.playbackRate = 1.0;
        this._mseDriftRaf = requestAnimationFrame(corrector);
        return;
      }

      const clock = this._audioClockUs();
      if (clock === null || clock === 0) {
        this._mseDriftRaf = requestAnimationFrame(corrector);
        return;
      }

      const clockSec = clock / 1_000_000;
      const drift = vt - clockSec;
      const absDrift = Math.abs(drift);

      if (absDrift > this._MSE_DRIFT_SEEK_THRESH) {
        this._mseVideoEl.currentTime = clockSec;
        this._mseVideoEl.playbackRate = 1.0;
      } else if (absDrift > this._MSE_DRIFT_NUDGE_THRESH) {
        this._mseVideoEl.playbackRate = drift > 0 ? 0.98 : 1.02;
      } else {
        this._mseVideoEl.playbackRate = 1.0;
      }

      this._mseDriftRaf = requestAnimationFrame(corrector);
    };

    this._mseDriftRaf = requestAnimationFrame(corrector);
  }

  // ── stream consume loop ───────────────────────────────────────────────────

  async _consumeStream(src) {
    this._status(`tuning ${src} …`);

    this._fetchAbortController = new AbortController();
    const source = makeSource(src, { signal: this._fetchAbortController.signal, hls: this.opts.hls });
    this._source = source;
    let trackLogged = false;
    let pathDecided = false;

    for (;;) {
      // Backpressure BEFORE reading the next chunk: don't fetch/decode more than
      // _AUDIO_LEAD_S of audio ahead of the play clock (keeps the worklet's fixed
      // ring from overflowing). CRITICAL: only gate on the audio buffer while the
      // audio clock is LIVE — a stalled/frozen clock must NOT block the feed, or a
      // brief audio hiccup freezes the whole stream (the "stops ~15s in" live halt,
      // zenith #84). Video-only / stalled-audio streams fall back to the
      // presentQueue cap to bound memory.
      while (
        !this._destroyed &&
        ((this._audioClockLive() && this._audioAheadSeconds() > this._AUDIO_LEAD_S) ||
          (!this._audioClockLive() && this._presentQueue.length > 10) ||
          this._presentQueue.length > 300)
      ) {
        // eslint-disable-next-line no-await-in-loop
        await sleep(40);
      }

      let done, value;
      try {
        ({ done, value } = await source.read());
      } finally {
        this._isLive = source.isLive;
      }

      if (done) {
        if (!pathDecided && this.bridge.video_codec()) {
          pathDecided = true;
          await this._decideVideoPath(this.bridge.video_codec());
        }
        this._callBridge(() => {
          if (this._videoPath === "webcodecs") this._pumpVideoInner();
          else if (this._videoPath === "mse") this._pumpVideoMseInner();
          this._pumpAudioInner();
          this._pumpSubtitlesInner();
        });
        return;
      }

      this._callBridge(() => {
        this.bridge.feed(value);

        // Refresh the track list whenever it changes. DVB-subtitle (and late
        // audio) tracks resolve on their first sample — AFTER the initial
        // PAT/PMT/video snapshot — so a one-shot read misses them, and a PMT
        // reshuffle can swap a PID or correct a language without changing the
        // track count. Re-emit whenever any track's identity changes so hosts
        // (and __sfStats) see every track as discovered or altered.
        const tl = this.bridge.track_list();
        if (tl) {
          const sig = trackSignature(tl);
          if (sig !== this._trackSig) {
            const prev = this._trackList;
            this._trackSig = sig;
            this._trackList = tl;
            this._stats.tracks = { audio: tl.audio ?? [], subtitle: tl.subtitles ?? [] };

            const diff = diffTracks(prev, tl);

            // A PMT reshuffle must never leave audio permanently silent: if the
            // selected PID is gone, fall back to the lowest surviving track.
            // `this._stats.selectedAudio` starts null and is only written by an
            // explicit selectAudio()/opts.audioPid — but the bridge auto-selects
            // the first audio PID it sees on its own, and in the default embed
            // (host never opens the picker) the JS never otherwise learns that.
            // Fall back to the bridge's actual selection so the guard still
            // fires in that case.
            const sel = this._stats.selectedAudio ?? this.bridge.selected_audio_pid ?? null;
            if (sel != null && !(tl.audio ?? []).some((t) => t.pid === sel)) {
              const next = pickFallbackAudio(tl.audio, sel);
              if (next != null) {
                this.bridge.select_audio(next);
                this._stats.selectedAudio = next;
                diff.reselected = { from: sel, to: next };
                this._status(`audio pid ${sel} vanished → pid ${next}`);
              }
            }

            this._emit("tracks", tl, diff);
            if (!trackLogged) {
              trackLogged = true;
              this._status(
                `track: video pid 0x${tl.video_pid.toString(16)} ${tl.video_codec}, ${tl.audio.length} audio, ${tl.subtitles.length} sub`
              );
              // Apply pre-selected audio PID if provided.
              if (this.opts.audioPid != null) {
                this.bridge.select_audio(this.opts.audioPid);
                this._status(`audio → pid ${this.opts.audioPid}`);
              }
            }
          }
        }

        if (this._videoPath === "webcodecs") {
          this._pumpVideoInner();
        } else if (this._videoPath === "mse") {
          this._pumpVideoMseInner();
        }
        this._pumpAudioInner();
        this._pumpSubtitlesInner();
      });

      if (!pathDecided && this.bridge.video_codec()) {
        pathDecided = true;
        await this._decideVideoPath(this.bridge.video_codec());
      }

      // Yield to the event loop so VideoDecoder output callbacks
      // populate `_presentQueue` before the next backpressure check.
      await new Promise((r) => setTimeout(r, 0));

    }
  }
}
