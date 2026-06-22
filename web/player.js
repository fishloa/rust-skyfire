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
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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
  decoded: 0, drawn: 0, dropped: 0, w: 0, h: 0, aus: 0, path: "wc",
  audioChunks: 0, audioSamples: 0, audioFrames: 0, audioSec: 0, avSkewMs: 0,
};
let videoDecoder = null;
let decoderConfigured = false;
let sawKeyframe = false;

// ── audio-master A/V sync (#32) ─────────────────────────────────────────────
//
// Audio is the clock. Video PTS and audio PTS share the broadcaster 90 kHz
// timeline (PCR/PTS preserved by zenith), so the position currently being heard
// is `firstAudioPtsUs + framesPlayed / sampleRate`. A video frame is presented
// when its timestamp reaches that clock; frames that fall too far behind are
// dropped, frames ahead are held.

const presentQueue = [];          // { frame, ts(µs) }, ascending by ts
let firstAudioPtsUs = null;       // µs PTS of the first audio sample heard
let audioFramesPlayed = 0;        // from the worklet clock
let audioSampleRate = 48000;
let presentScheduled = false;

const LATE_DROP_US = 80_000;      // >80 ms behind the clock → drop
const LEAD_US = 12_000;           // present up to ~12 ms early (one frame)

function audioClockUs() {
  if (firstAudioPtsUs === null || audioFramesPlayed === 0) return null;
  return firstAudioPtsUs + (audioFramesPlayed / audioSampleRate) * 1_000_000;
}

function schedulePresent() {
  if (presentScheduled) return;
  presentScheduled = true;
  requestAnimationFrame(present);
}

function present() {
  presentScheduled = false;
  const clock = audioClockUs();

  // Before audio is actually playing, present at the display's pace (one frame
  // per rAF) so video isn't frozen waiting on a clock that hasn't started.
  if (clock === null) {
    const e = presentQueue.shift();
    if (e) drawFrame(e.frame);
    if (presentQueue.length) schedulePresent();
    return;
  }

  while (presentQueue.length) {
    const e = presentQueue[0];
    if (e.ts > clock + LEAD_US) break;          // ahead of the clock — hold
    presentQueue.shift();
    if (e.ts < clock - LATE_DROP_US) {           // too far behind — drop
      e.frame.close();
      stats.dropped++;
      continue;
    }
    drawFrame(e.frame);
    stats.avSkewMs = Math.round((clock - e.ts) / 1000);
  }
  if (presentQueue.length) schedulePresent();
}

// ── WebCodecs video decoder ─────────────────────────────────────────────────

function ensureDecoder(codec) {
  if (decoderConfigured) return true;

  videoDecoder = new VideoDecoder({
    output(frame) {
      stats.decoded++;
      // #32: queue by PTS; the rAF present loop draws against the audio clock.
      presentQueue.push({ frame, ts: frame.timestamp });
      schedulePresent();
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
  audioSampleRate = audioCtx.sampleRate || sampleRate;
  audioNode.port.onmessage = (e) => {
    if (e.data.type === "clock") {
      audioFramesPlayed = e.data.framesPlayed;
      stats.audioFrames = audioFramesPlayed;
      stats.audioSec = audioFramesPlayed / audioSampleRate;
      schedulePresent();   // wake the present loop on each clock tick
    }
  };
  audioGain = audioCtx.createGain();
  audioGain.gain.value = muted ? 0 : 1;
  audioNode.connect(audioGain).connect(audioCtx.destination);
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
    if (firstAudioPtsUs === null && c.pts_ticks !== undefined) {
      firstAudioPtsUs = ticksToMicros(c.pts_ticks);
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

// ── UI: track pickers + transport controls (#35) ────────────────────────────

const audioSelect = document.getElementById("audio-select");
const subSelect = document.getElementById("sub-select");
const playPauseBtn = document.getElementById("playpause");
const muteBtn = document.getElementById("mute");
const subsEl = document.getElementById("subs");

let playing = true;
let muted = false;
let uiWired = false;

function langLabel(code) { return code ? code : ""; }

function populateTracks(tl) {
  // Audio picker.
  audioSelect.innerHTML = "";
  tl.audio.forEach((a, i) => {
    const o = document.createElement("option");
    o.value = String(a.pid);
    o.textContent = `${langLabel(a.language) || "track " + (i + 1)} · ${a.codec}`;
    audioSelect.appendChild(o);
  });
  // Subtitle picker (keep the leading "Off").
  while (subSelect.options.length > 1) subSelect.remove(1);
  tl.subtitles.forEach((s) => {
    const o = document.createElement("option");
    o.value = String(s.pid);
    o.textContent = `${langLabel(s.language) || "sub"} · ${s.kind}`;
    subSelect.appendChild(o);
  });
}

function wireControls() {
  if (uiWired) return;
  uiWired = true;

  audioSelect.addEventListener("change", () => {
    bridge.select_audio(parseInt(audioSelect.value, 10));
    status(`audio → pid ${audioSelect.value}`);
  });

  subSelect.addEventListener("change", () => {
    const v = subSelect.value;
    bridge.select_subtitle(v === "" ? undefined : parseInt(v, 10));
    subsEl.replaceChildren(); // clear current cue on switch/off
  });

  playPauseBtn.addEventListener("click", () => {
    playing = !playing;
    bridge.set_playing(playing);
    if (audioNode) audioNode.port.postMessage({ type: playing ? "play" : "pause" });
    if (playing) startAudio();
    playPauseBtn.textContent = playing ? "⏸ Pause" : "▶ Play";
  });

  muteBtn.addEventListener("click", () => {
    muted = !muted;
    if (audioGain) audioGain.gain.value = muted ? 0 : 1;
    muteBtn.textContent = muted ? "🔇 Unmute" : "🔊 Mute";
  });
}

// Drain parsed subtitle cues into the overlay. The cue bitmap layout is defined
// by the bridge (#34); render is finalised once that lands.
function pumpSubtitles() {
  if (!bridge.take_subtitle_cues) return;
  for (const cue of bridge.take_subtitle_cues()) {
    stats.subCues = (stats.subCues || 0) + 1;
    // TODO(#34 render): decode cue.bytes → bitmap region → draw into #subs.
    cue.free?.();
  }
}

// ── bridge + stream ─────────────────────────────────────────────────────────

let bridge = null;
let audioGain = null;

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
        status(`track: video pid 0x${tl.video_pid.toString(16)} ${tl.video_codec}, ${tl.audio.length} audio, ${tl.subtitles.length} sub`);
        populateTracks(tl);
        wireControls();
      }
    }
    pumpVideo();
    await pumpAudio();
    pumpSubtitles();

    // Back-pressure: video presents at the audio (realtime) pace, so cap how
    // far decode runs ahead — otherwise the whole clip's frames queue as open
    // VideoFrames at once. (Bounded buffering is refined in #36.)
    while (presentQueue.length > 60) {
      // eslint-disable-next-line no-await-in-loop
      await sleep(40);
    }
  }

  // Stream ended — flush the bridge (emits the final buffered AU/PCM), drain,
  // then flush the decoder so its reordered tail frames come out.
  bridge.flush();
  pumpVideo();
  await pumpAudio();
  if (videoDecoder && decoderConfigured) {
    try { await videoDecoder.flush(); } catch (e) { console.warn("[skyfire] flush", e); }
  }

  status(`done — video ${stats.decoded}f/${stats.drawn}drawn, audio ${stats.audioChunks} chunks / ${stats.audioSamples} samples, played ${stats.audioSec.toFixed(1)}s`);
  window.__sfStats = { ...stats, done: true };
}

main().catch((err) => fatal("startup failed", err));
