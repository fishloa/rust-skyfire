import init, { WasmEngine } from "./pkg/skyfire_wasm.js";

const overlay = document.getElementById("overlay");
const errorEl = document.getElementById("error");

function status(msg) {
  overlay.textContent = msg;
  console.log("[skyfire]", msg);
}

function fatal(msg, err) {
  const text = msg + (err ? "\n" + (err.message || err) : "");
  errorEl.textContent = text;
  errorEl.style.display = "block";
  console.error("[skyfire]", msg, err);
}

// ── canvas + WebGL2 ───────────────────────────────────────────────────────

const canvas = document.getElementById("canvas");
const gl = canvas.getContext("webgl2", {
  premultipliedAlpha: false,
  alpha: false,
  antialias: false,
  powerPreference: "high-performance",
});
if (!gl) fatal("WebGL2 not available");

const PTS_TICKS_PER_SEC = 90_000;

// ── shared state ──────────────────────────────────────────────────────────

let canvasWidth = 0, canvasHeight = 0;
let tex = null;
let audioCtx = null;
let audioWorkletNode = null;
let videoDecoder = null;
let videoFramesDecoded = 0;
let videoFramesDrawn = 0;
let lastStatusUpdate = 0;

// ── texture ───────────────────────────────────────────────────────────────

function ensureTexture(w, h) {
  if (tex && canvasWidth === w && canvasHeight === h) return;
  if (tex) gl.deleteTexture(tex);
  tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
  canvasWidth = w;
  canvasHeight = h;
  canvas.width = w;
  canvas.height = h;
}

function drawFrame(frame) {
  try {
    const w = frame.displayWidth, h = frame.displayHeight;
    ensureTexture(w, h);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, frame);
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    videoFramesDrawn++;
  } finally {
    frame.close();
  }
}

// ── WebGL quad program ────────────────────────────────────────────────────

const vsSrc = `#version 300 es
in vec2 a_pos;
out vec2 v_tex;
void main() {
  gl_Position = vec4(a_pos, 0.0, 1.0);
  v_tex = a_pos * 0.5 + 0.5;
}`;
const fsSrc = `#version 300 es
precision highp float;
in vec2 v_tex;
out vec4 outColor;
uniform sampler2D u_tex;
void main() {
  outColor = texture(u_tex, v_tex);
}`;
function compile(t, s) {
  const sh = gl.createShader(t);
  gl.shaderSource(sh, s);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS))
    throw new Error(gl.getShaderInfoLog(sh));
  return sh;
}
const prog = gl.createProgram();
gl.attachShader(prog, compile(gl.VERTEX_SHADER, vsSrc));
gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, fsSrc));
gl.linkProgram(prog);
if (!gl.getProgramParameter(prog, gl.LINK_STATUS))
  throw new Error(gl.getProgramInfoLog(prog));
gl.useProgram(prog);

const quad = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]);
const vbuf = gl.createBuffer();
gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
gl.bufferData(gl.ARRAY_BUFFER, quad, gl.STATIC_DRAW);
const aPos = gl.getAttribLocation(prog, "a_pos");
gl.enableVertexAttribArray(aPos);
gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
gl.bindTexture(gl.TEXTURE_2D, null);

// ── sync: audio-master clock ──────────────────────────────────────────────

const presentQueue = [];
let syncAudioStartTime = 0;    // audioCtx.currentTime when audio began
let syncFirstVideoPts = null;  // first video frame's PTS in 90kHz ticks

function ptsTicksToSec(pts) {
  return Number(pts) / PTS_TICKS_PER_SEC;
}

function ptsTicksToWallTime(pts) {
  // Audio starts at syncAudioStartTime. Video PTS offset from first video PTS
  // maps to wall time offset from audio start.
  if (syncFirstVideoPts === null) return syncAudioStartTime;
  const offsetSec = ptsTicksToSec(pts - syncFirstVideoPts);
  return syncAudioStartTime + offsetSec;
}

let renderScheduled = false;

function scheduleRender() {
  if (renderScheduled) return;
  renderScheduled = true;
  requestAnimationFrame(onRaf);
}

function onRaf() {
  renderScheduled = false;
  if (!audioCtx) return;
  const now = audioCtx.currentTime;
  let next = false;

  while (presentQueue.length > 0) {
    const entry = presentQueue[0];
    const target = ptsTicksToWallTime(entry.ptsTicks);
    const delta = target - now;

    if (delta > 0.016) {
      scheduleRender();
      next = true;
      break;
    }

    presentQueue.shift();
    if (delta < -0.100) {
      entry.frame.close();
      continue;
    }
    drawFrame(entry.frame);
  }

  // Status line updated ~1 Hz.
  if (now - lastStatusUpdate > 1.0) {
    lastStatusUpdate = now;
    const q = presentQueue.length;
    const aState = audioWorkletNode ? "playing" : (audioCtx ? "init" : "none");
    status(`decoded ${videoFramesDecoded} video | drawn ${videoFramesDrawn} | queue ${q} | audio ${aState}`);
  }

  if (!next && presentQueue.length > 0) scheduleRender();
}

function enqueueVideoFrame(frame, ptsTicks) {
  videoFramesDecoded++;
  presentQueue.push({ frame, ptsTicks });
  if (syncFirstVideoPts === null) syncFirstVideoPts = ptsTicks;
  scheduleRender();
}

// ── NAL unit scanner for Annex-B key-frame detection ──────────────────────

/**
 * Scan H.264 Annex-B access unit bytes for NAL unit types.
 * Returns true if the AU contains SPS (7) or PPS (8) — an open-GOP
 * random-access point suitable for `type: "key"`.
 */
function containsSpsOrPps(bytes) {
  const len = bytes.length;
  for (let i = 0; i < len - 4; i++) {
    if (bytes[i] === 0x00 && bytes[i + 1] === 0x00) {
      if (bytes[i + 2] === 0x01) {
        const nalType = bytes[i + 3] & 0x1f;
        if (nalType === 7 || nalType === 8) return true;
        i += 3;
      } else if (bytes[i + 2] === 0x00 && bytes[i + 3] === 0x01) {
        const nalType = bytes[i + 4] & 0x1f;
        if (nalType === 7 || nalType === 8) return true;
        i += 4;
      }
    }
  }
  return false;
}

// ── video decoder ─────────────────────────────────────────────────────────

function configureVideoDecoder(codec, _description) {
  // Do NOT pass avcC description — the engine produces Annex-B access units.
  // WebCodecs: description → AVCC data; no description → Annex-B data.
  return new Promise((resolve, reject) => {
    videoDecoder = new VideoDecoder({
      output(frame) {
        const ptsTicks = BigInt(Math.round(frame.timestamp * PTS_TICKS_PER_SEC / 1_000_000));
        enqueueVideoFrame(frame, ptsTicks);
      },
      error(e) { fatal("VideoDecoder error", e); },
    });

    videoDecoder.configure({ codec, optimizeForLatency: false });
    status(`VideoDecoder configured: ${codec} (Annex-B)`);
    resolve();
  });
}

function feedVideoAccessUnits(engine) {
  const count = engine.video_unit_count();
  for (let i = 0; i < count; i++) {
    const au = engine.video_unit(i);
    if (!au) continue;
    const ptsRaw = au.pts_ticks;
    if (ptsRaw == null) continue;

    const ptsTicks = BigInt(ptsRaw);
    const timestampUs = Number(ptsTicks) * 1_000_000 / PTS_TICKS_PER_SEC;

    // Detect open-GOP random-access points: AU containing SPS(7) or PPS(8).
    const chunkType = containsSpsOrPps(au.bytes) ? "key" : "delta";

    const chunk = new EncodedVideoChunk({
      type: chunkType,
      timestamp: timestampUs,
      duration: 0,
      data: au.bytes,
    });
    videoDecoder.decode(chunk);
  }
  videoDecoder.flush();
}

// ── audio ─────────────────────────────────────────────────────────────────

async function initAudio(pcm, sampleRate, channels) {
  audioCtx = new AudioContext({ sampleRate, latencyHint: 0.04 });
  await audioCtx.audioWorklet.addModule("./audio-worklet.js");

  audioWorkletNode = new AudioWorkletNode(audioCtx, "skyfire-pcm", {
    numberOfOutputs: 1,
    outputChannelCount: [channels],
  });

  // Copy PCM out of wasm heap into a fresh ArrayBuffer for transfer.
  const wasmView = new Uint8Array(pcm);
  const pcmCopy = new ArrayBuffer(wasmView.byteLength);
  new Uint8Array(pcmCopy).set(wasmView);
  audioWorkletNode.port.postMessage(
    { type: "init", pcm: pcmCopy, sampleRate, channels },
    [pcmCopy]
  );
  audioWorkletNode.connect(audioCtx.destination);
  status(`Audio: ${sampleRate} Hz, ${channels} ch, ${pcm.length} bytes`);
}

function startPlayback() {
  if (audioCtx && audioCtx.state === "suspended") audioCtx.resume();
  if (audioWorkletNode) audioWorkletNode.port.postMessage({ type: "start" });
  syncAudioStartTime = audioCtx ? audioCtx.currentTime : 0;
}

// ── main ──────────────────────────────────────────────────────────────────

async function main() {
  status("Loading WASM...");
  await init();

  status("Fetching fixture...");
  const resp = await fetch("/fixtures/gulli-15s.ts");
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  const tsBytes = new Uint8Array(await resp.arrayBuffer());
  status(`Loaded ${tsBytes.length} bytes`);

  const engine = new WasmEngine();

  const ch = engine.probe(tsBytes);
  if (!ch) throw new Error("Probe failed");
  status(`Probe: video=${ch.video_codec}@0x${ch.video_pid.toString(16)}, audio=${JSON.stringify(ch.audio_codecs)}`);

  engine.init_with_channel(ch.video_pid, ch.video_codec, ch.audio_pids, ch.audio_codecs);

  status("Decoding...");
  engine.feed(tsBytes);
  engine.flush();
  engine.finalize();

  if (engine.has_audio()) {
    await initAudio(engine.audio_pcm(), engine.audio_sample_rate(), engine.audio_channels());
  }

  if (engine.has_video()) {
    const codec = engine.video_config_codec();

    // Probe without description — Annex-B mode.
    const support = await VideoDecoder.isConfigSupported({ codec });
    if (!support.supported) {
      fatal(`Video codec not supported: ${codec}`);
      return;
    }

    await configureVideoDecoder(codec);
    startPlayback();
    feedVideoAccessUnits(engine);
  } else {
    status("No video stream found");
  }

  status("Pipeline running");
}

main().catch((err) => fatal("startup failed", err));
