import init, { WasmEngine, WasmVideoDecoder } from "./pkg/skyfire_wasm.js";

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
    gl.useProgram(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    videoFramesDrawn++;
  } finally {
    frame.close();
  }
}

// ── WebGL quad program (RGBA path) ────────────────────────────────────────

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

// ── WebGL YUV program (software I420 path, BT.709 limited-range) ─────────
//
// Three R8 textures: Y (width×height), U (width/2 × height/2),
// V (width/2 × height/2).  BT.709 limited-range matrix:
//   Y'  ∈ [16,235], Cb/Cr ∈ [16,240]
//   y = (Y-16)/219, u = (U-128)/224, v = (V-128)/224
//   R = y + 1.5748*v
//   G = y - 0.1873*u - 0.4681*v
//   B = y + 1.8556*u

const vsYuvSrc = `#version 300 es
in vec2 a_pos;
out vec2 v_tex;
void main() {
  gl_Position = vec4(a_pos, 0.0, 1.0);
  v_tex = a_pos * 0.5 + 0.5;
}`;
const fsYuvSrc = `#version 300 es
precision highp float;
in vec2 v_tex;
out vec4 outColor;
uniform sampler2D u_y;
uniform sampler2D u_u;
uniform sampler2D u_v;
void main() {
  float yRaw = texture(u_y, v_tex).r;
  float uRaw = texture(u_u, v_tex).r;
  float vRaw = texture(u_v, v_tex).r;
  float y = (yRaw * 255.0 - 16.0) / 219.0;
  float u = (uRaw * 255.0 - 128.0) / 224.0;
  float v = (vRaw * 255.0 - 128.0) / 224.0;
  float r = y + 1.5748 * v;
  float g = y - 0.1873 * u - 0.4681 * v;
  float b = y + 1.8556 * u;
  outColor = vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0);
}`;

const progYuv = gl.createProgram();
gl.attachShader(progYuv, compile(gl.VERTEX_SHADER, vsYuvSrc));
gl.attachShader(progYuv, compile(gl.FRAGMENT_SHADER, fsYuvSrc));
gl.linkProgram(progYuv);
if (!gl.getProgramParameter(progYuv, gl.LINK_STATUS))
  throw new Error(gl.getProgramInfoLog(progYuv));

// Cache uniform/attrib locations for the YUV program.
const aPosYuv = gl.getAttribLocation(progYuv, "a_pos");
const uLocY = gl.getUniformLocation(progYuv, "u_y");
const uLocU = gl.getUniformLocation(progYuv, "u_u");
const uLocV = gl.getUniformLocation(progYuv, "u_v");

// Three persistent R8 textures reused across frames.
let yuvTexY = null, yuvTexU = null, yuvTexV = null;
let yuvTexW = 0, yuvTexH = 0;

function ensureYuvTextures(w, h) {
  if (yuvTexW === w && yuvTexH === h) return;

  if (yuvTexY) { gl.deleteTexture(yuvTexY); gl.deleteTexture(yuvTexU); gl.deleteTexture(yuvTexV); }

  function makeR8(tw, th) {
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, tw, th, 0, gl.RED, gl.UNSIGNED_BYTE, null);
    return t;
  }

  yuvTexY = makeR8(w, h);
  yuvTexU = makeR8(w >> 1, h >> 1);
  yuvTexV = makeR8(w >> 1, h >> 1);
  yuvTexW = w;
  yuvTexH = h;

  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

/**
 * Draw an I420 frame (from the software decoder present queue).
 * entry: { width, height, ptsTicks, y: Uint8Array, u: Uint8Array, v: Uint8Array }
 */
function drawFrameYuv(entry) {
  const { width: w, height: h, y, u, v } = entry;
  ensureYuvTextures(w, h);

  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);

  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, yuvTexY);
  gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RED, gl.UNSIGNED_BYTE, y);

  gl.activeTexture(gl.TEXTURE1);
  gl.bindTexture(gl.TEXTURE_2D, yuvTexU);
  gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w >> 1, h >> 1, gl.RED, gl.UNSIGNED_BYTE, u);

  gl.activeTexture(gl.TEXTURE2);
  gl.bindTexture(gl.TEXTURE_2D, yuvTexV);
  gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w >> 1, h >> 1, gl.RED, gl.UNSIGNED_BYTE, v);

  gl.useProgram(progYuv);
  gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
  gl.enableVertexAttribArray(aPosYuv);
  gl.vertexAttribPointer(aPosYuv, 2, gl.FLOAT, false, 0, 0);

  gl.uniform1i(uLocY, 0);
  gl.uniform1i(uLocU, 1);
  gl.uniform1i(uLocV, 2);

  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

  videoFramesDrawn++;
  window.__sfStats = { decoded: videoFramesDecoded, drawn: videoFramesDrawn, w, h, path: "sw" };
}

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

// ── software decode state ─────────────────────────────────────────────────

// Set to true in main() when using the WASM software decoder.
let useSwDecode = false;
// WASM software decoder instance (WasmVideoDecoder).
let swDecoder = null;
// Array of all access-unit objects from the engine, to be consumed lazily.
let swAuQueue = [];
// Index into swAuQueue for the next AU to send.
let swAuIndex = 0;
// True when all AUs have been sent and dec.flush() called.
let swFlushed = false;
// Maximum decoded frames to hold in presentQueue before pausing decode.
const SW_QUEUE_MAX = 8;
// How many AUs to process per incremental batch.
const SW_BATCH_SIZE = 4;

/**
 * Drain decoded frames from the WASM decoder into presentQueue (as I420
 * plain-object copies so wasm memory is released immediately).
 */
function drainSwDecoder() {
  let f;
  while ((f = swDecoder.receive()) !== undefined) {
    const w = f.width;
    const h = f.height;
    const ptsTicks = f.pts_ticks;
    const yLen = w * h;
    const cLen = (w >> 1) * (h >> 1);

    // Slice and copy each plane out of the wasm Uint8Array.
    const y = f.data.slice(0, yLen);
    const u = f.data.slice(yLen, yLen + cLen);
    const v = f.data.slice(yLen + cLen, yLen + 2 * cLen);
    f.free();

    videoFramesDecoded++;
    if (syncFirstVideoPts === null) syncFirstVideoPts = ptsTicks;
    presentQueue.push({ width: w, height: h, ptsTicks, y, u, v });
  }
}

/**
 * Process a small batch of access units from swAuQueue, then drain decoded
 * frames.  Reschedules itself via setTimeout(0) until all AUs are consumed and
 * flushed.  Back-pressure: if presentQueue is already at SW_QUEUE_MAX, yields
 * without consuming more AUs.
 */
function swDecodeTick() {
  if (!swDecoder) return;

  // Back-pressure: wait for the render loop to consume some frames first.
  if (presentQueue.length >= SW_QUEUE_MAX) {
    scheduleRender();
    setTimeout(swDecodeTick, 16);
    return;
  }

  // Send a batch of AUs to the decoder.
  let sent = 0;
  while (swAuIndex < swAuQueue.length && sent < SW_BATCH_SIZE) {
    const au = swAuQueue[swAuIndex++];
    if (!au) continue;
    const pts = au.pts_ticks != null ? Number(au.pts_ticks) : 0;
    swDecoder.send(au.bytes, pts);
    sent++;
  }

  // If all AUs sent and not yet flushed, flush to drain reordered frames.
  if (swAuIndex >= swAuQueue.length && !swFlushed) {
    swDecoder.flush();
    swFlushed = true;
  }

  // Collect any newly decoded frames.
  drainSwDecoder();
  scheduleRender();

  // Keep going if there are more AUs to process.
  if (!swFlushed || swAuIndex < swAuQueue.length) {
    setTimeout(swDecodeTick, 0);
  }
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
      // Frame is more than 100ms late — drop it (no close() needed for I420
      // plain objects; WebCodecs VideoFrame objects have .close()).
      if (entry.frame) entry.frame.close();
      continue;
    }

    if (useSwDecode) {
      drawFrameYuv(entry);
    } else {
      drawFrame(entry.frame);
    }
  }

  // Status line updated ~1 Hz.
  if (now - lastStatusUpdate > 1.0) {
    lastStatusUpdate = now;
    const q = presentQueue.length;
    const aState = audioWorkletNode ? "playing" : (audioCtx ? "init" : "none");
    const path = useSwDecode ? "sw" : "wc";
    status(`decoded ${videoFramesDecoded} video | drawn ${videoFramesDrawn} | queue ${q} | audio ${aState} | path ${path}`);
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

    if (engine.video_is_interlaced()) {
      // ── Software path: WASM H.264 decoder for interlaced (1080i) ──────────
      status(`Interlaced video detected — using software decoder (${codec})`);
      useSwDecode = true;

      // Collect all access units from the engine up front, then decode lazily.
      const count = engine.video_unit_count();
      for (let i = 0; i < count; i++) {
        const au = engine.video_unit(i);
        if (au && au.pts_ticks != null) swAuQueue.push(au);
      }

      swDecoder = new WasmVideoDecoder();
      startPlayback();
      // Begin incremental decode (yields via setTimeout to keep page responsive).
      setTimeout(swDecodeTick, 0);
    } else {
      // ── WebCodecs path: progressive / hardware-decoded ────────────────────
      const support = await VideoDecoder.isConfigSupported({ codec });
      if (!support.supported) {
        fatal(`Video codec not supported: ${codec}`);
        return;
      }

      await configureVideoDecoder(codec);
      startPlayback();
      feedVideoAccessUnits(engine);
    }
  } else {
    status("No video stream found");
  }

  status("Pipeline running");
}

main().catch((err) => fatal("startup failed", err));
