// player.js — skyfire WASM bridge client (ADR 0008).
//
// The browser owns presentation + control; the WASM `SkyfireBridge` parses the
// MPEG-TS and hands progressive H.264 access units up to WebCodecs.
//
// Issue #30 scope = the VIDEO path: feed TS → bridge.take_video_aus() →
// WebCodecs `VideoDecoder` → canvas. Audio (WASM AC-3 → WebAudio) lands in #31,
// the audio-master PCR/PTS sync in #32, DVB subtitles in #34. Those hooks are
// marked `TODO(#NN)` below.

import init, { SkyfireBridge } from "./pkg/skyfire_wasm.js";

const overlay = document.getElementById("overlay");
const errorEl = document.getElementById("error");
const canvas = document.getElementById("canvas");

function status(msg) {
  if (overlay) overlay.textContent = msg;
  console.log("[skyfire]", msg);
}

function fatal(msg, err) {
  const text = msg + (err ? "\n" + (err.message || err) : "");
  if (errorEl) {
    errorEl.textContent = text;
    errorEl.style.display = "block";
  }
  console.error("[skyfire]", msg, err);
}

const PTS_HZ = 90_000;
const ticksToMicros = (ticks) => Number(ticks) * 1_000_000 / PTS_HZ;

// ── canvas (2D draw of WebCodecs VideoFrame) ────────────────────────────────

const ctx = canvas.getContext("2d", { alpha: false });
let sized = false;

function drawFrame(frame) {
  try {
    if (!sized || canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
      canvas.width = frame.displayWidth;
      canvas.height = frame.displayHeight;
      sized = true;
    }
    ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);
    stats.drawn++;
    stats.w = frame.displayWidth;
    stats.h = frame.displayHeight;
    window.__sfStats = { ...stats };
  } finally {
    frame.close();
  }
}

// ── shared state ────────────────────────────────────────────────────────────

const stats = {
  decoded: 0, drawn: 0, w: 0, h: 0, aus: 0, path: "wc",
  audioChunks: 0, audioSamples: 0, audioFrames: 0, audioSec: 0,
};
let videoDecoder = null;
let decoderConfigured = false;
let sawKeyframe = false;

// ── WebCodecs video decoder ─────────────────────────────────────────────────

function ensureDecoder(codec) {
  if (decoderConfigured) return true;

  videoDecoder = new VideoDecoder({
    output(frame) {
      stats.decoded++;
      // #30: present immediately (no clock yet). TODO(#32): queue + present
      // against the audio-master PCR/PTS clock instead of drawing on output.
      drawFrame(frame);
    },
    error(e) { fatal("VideoDecoder error", e); },
  });

  // Annex-B: configure with codec only, no description.
  videoDecoder.configure({ codec, optimizeForLatency: true });
  decoderConfigured = true;
  status(`VideoDecoder configured: ${codec}`);
  return true;
}

// Drain pending video AUs from the bridge into the decoder.
function pumpVideo() {
  const codec = videoDecoder ? null : undefined; // placeholder to keep lints quiet
  const cs = bridge.video_codec();               // "avc1.640028" once SPS seen
  if (!cs) return;                               // no config yet → wait
  if (!ensureDecoder(cs)) return;

  for (const au of bridge.take_video_aus()) {
    stats.aus++;
    const key = au.is_keyframe;
    // WebCodecs requires the first chunk after configure() to be a keyframe.
    if (!sawKeyframe) {
      if (!key) { au.free?.(); continue; }       // skip until the first IDR/SPS
      sawKeyframe = true;
    }
    const ts = au.pts_ticks !== undefined ? ticksToMicros(au.pts_ticks) : 0;
    try {
      videoDecoder.decode(new EncodedVideoChunk({
        type: key ? "key" : "delta",
        timestamp: ts,
        data: au.bytes,
      }));
    } catch (e) {
      fatal("decode() threw", e);
      return;
    }
    au.free?.();
  }
  void codec;
}

// ── audio: WASM PCM → WebAudio AudioWorklet (#31) ───────────────────────────

let audioCtx = null;
let audioNode = null;
let audioReady = false;
let audioStarting = false;

async function ensureAudio(sampleRate, channels) {
  if (audioReady || audioStarting) return;
  audioStarting = true;
  audioCtx = new AudioContext({ sampleRate });
  await audioCtx.audioWorklet.addModule("./audio-worklet.js");
  audioNode = new AudioWorkletNode(audioCtx, "skyfire-pcm", {
    numberOfOutputs: 1,
    outputChannelCount: [channels],
  });
  audioNode.port.onmessage = (e) => {
    if (e.data.type === "clock") {
      stats.audioFrames = e.data.framesPlayed;
      // Audio-master media time (seconds). TODO(#32): present video against this.
      stats.audioSec = e.data.framesPlayed / (audioCtx.sampleRate || sampleRate);
    }
  };
  audioNode.connect(audioCtx.destination);
  audioNode.port.postMessage({ type: "config", sampleRate, channels });
  audioNode.port.postMessage({ type: "play" });
  // Autoplay policy: resume if a user gesture has been granted; otherwise it
  // stays suspended until startAudio() is called from a click.
  audioCtx.resume().catch(() => {});
  audioReady = true;
  audioStarting = false;
  status(`audio: ${sampleRate} Hz, ${channels} ch`);
}

// Drain decoded PCM chunks from the bridge into the worklet.
async function pumpAudio() {
  const chunks = bridge.take_audio_pcm();
  for (const c of chunks) {
    if (!audioReady) {
      // eslint-disable-next-line no-await-in-loop
      await ensureAudio(c.sample_rate, c.channels);
    }
    const samples = c.samples; // Float32Array, interleaved
    stats.audioChunks++;
    stats.audioSamples += samples.length;
    audioNode.port.postMessage({ type: "pcm", samples }, [samples.buffer]);
    c.free?.();
  }
}

// Resume audio on a user gesture (browsers gate AudioContext on one).
function startAudio() {
  if (audioCtx && audioCtx.state === "suspended") audioCtx.resume().catch(() => {});
}
window.addEventListener("pointerdown", startAudio, { once: true });
window.addEventListener("keydown", startAudio, { once: true });
window.sfStartAudio = startAudio; // exposed for the Playwright/iOS verifier

// ── bridge + stream ─────────────────────────────────────────────────────────

let bridge = null;

async function main() {
  status("Loading WASM…");
  await init();
  bridge = new SkyfireBridge();

  if (typeof VideoDecoder === "undefined") {
    fatal("WebCodecs VideoDecoder unavailable in this browser");
    return;
  }

  // Source TS. Default to the committed progressive fixture (WebCodecs can only
  // decode progressive H.264 — interlaced is deinterlaced server-side per ADR
  // 0008). Override with ?src= to point at the live /skyfire/<slug> endpoint or
  // another fixture.
  const src = new URLSearchParams(location.search).get("src") || "/fixtures/h264-25fps.ts";
  status(`Streaming ${src} …`);

  let resp;
  try {
    resp = await fetch(src);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  } catch (e) {
    fatal("fetch failed", e);
    return;
  }

  const reader = resp.body.getReader();
  let trackLogged = false;

  // Streaming read loop: feed each chunk, then drain video AUs.
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    bridge.feed(value);

    if (!trackLogged) {
      const tl = bridge.track_list();
      if (tl) {
        trackLogged = true;
        status(`track: video pid 0x${tl.video_pid.toString(16)} ${tl.video_codec}, ${tl.audio.length} audio`);
        // TODO(#31): select_audio + WASM AC-3 → WebAudio.
        // TODO(#34): select_subtitle + DVB-subtitle overlay.
      }
    }
    pumpVideo();
    await pumpAudio();
  }

  // Stream ended — drain any tail AUs and flush the decoder.
  pumpVideo();
  await pumpAudio();
  if (videoDecoder && decoderConfigured) {
    try { await videoDecoder.flush(); } catch (e) { console.warn("[skyfire] flush", e); }
  }

  status(`done — video ${stats.decoded}f/${stats.drawn}drawn, audio ${stats.audioChunks} chunks / ${stats.audioSamples} samples, played ${stats.audioSec.toFixed(1)}s`);
  window.__sfStats = { ...stats, done: true };
}

main().catch((err) => fatal("startup failed", err));
