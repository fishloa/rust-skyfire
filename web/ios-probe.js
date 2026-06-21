// ios-probe.js — WebCodecs interlaced H.264 hardware-decode diagnostic.
// Served by web/serve.ts; loaded as ES module from ios-probe.html.
// Fixture: /fixtures/gulli-15s.ts  (1080i H.264 DVB broadcast)

import init, { WasmEngine } from "./pkg/skyfire_wasm.js";

// ── logging helpers ────────────────────────────────────────────────────────

const logEl = document.getElementById("log");

function log(text, cls = "info") {
  const d = document.createElement("div");
  d.className = "entry " + cls;
  d.textContent = text;
  logEl.appendChild(d);
  console.log("[probe]", text);
  return d;
}

function stepHead(n, label) {
  const d = document.createElement("div");
  d.className = "entry info step-head";
  d.textContent = `── Step ${n}: ${label} ──`;
  logEl.appendChild(d);
  console.log(`[probe] step ${n}: ${label}`);
}

// ── NAL start-code scanner ─────────────────────────────────────────────────
// Returns true if the AU contains a NAL of type 7 (SPS) or 8 (PPS),
// which marks it as a keyframe in Annex-B H.264.

function isKeyframe(bytes) {
  let i = 0;
  const end = bytes.length;
  while (i < end - 3) {
    // look for 00 00 01 (3-byte) or 00 00 00 01 (4-byte) start code
    if (bytes[i] === 0 && bytes[i + 1] === 0) {
      let nalStart = -1;
      if (bytes[i + 2] === 1) {
        nalStart = i + 3;
      } else if (bytes[i + 2] === 0 && i + 3 < end && bytes[i + 3] === 1) {
        nalStart = i + 4;
      }
      if (nalStart !== -1 && nalStart < end) {
        const nalType = bytes[nalStart] & 0x1f;
        if (nalType === 7 || nalType === 8) return true;
        // skip past this start code
        i = nalStart;
        continue;
      }
    }
    i++;
  }
  return false;
}

// ── main probe ─────────────────────────────────────────────────────────────

async function runProbe() {

  // Step 1 — User agent + CPU cores
  stepHead(1, "Environment");
  log("UA: " + navigator.userAgent);
  log("hardwareConcurrency: " + (navigator.hardwareConcurrency ?? "unknown"));

  // Step 2 — Load WASM, fetch fixture, demux
  stepHead(2, "WASM demux");

  log("Initialising WASM...");
  try {
    await init();
  } catch (e) {
    log("WASM init FAILED: " + (e?.message ?? e), "error");
    return;
  }
  log("WASM ready.", "ok");

  log("Fetching fixtures/gulli-15s.ts ...");
  let tsBytes;
  try {
    // Relative path so it works both under the local /ios-probe.html and
    // when served at a subpath (e.g. tv.icomb.place/skyfire-probe/).
    const resp = await fetch("fixtures/gulli-15s.ts");
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const buf = await resp.arrayBuffer();
    tsBytes = new Uint8Array(buf);
    log("Fixture loaded: " + tsBytes.length + " bytes.", "ok");
  } catch (e) {
    log("Fixture fetch FAILED: " + (e?.message ?? e), "error");
    return;
  }

  let engine;
  try {
    engine = new WasmEngine();
    const ch = engine.probe(tsBytes);
    if (!ch) {
      log("engine.probe() returned null — no PAT/PMT found.", "error");
      return;
    }
    log("probe: video_pid=" + ch.video_pid + "  video_codec=" + ch.video_codec);
    engine.init_with_channel(
      ch.video_pid,
      ch.video_codec,
      ch.audio_pids,
      ch.audio_codecs,
    );
    engine.feed(tsBytes);
    engine.flush();
    engine.finalize();
  } catch (e) {
    log("Demux error: " + (e?.message ?? e), "error");
    return;
  }

  const interlaced = engine.video_is_interlaced();
  const codec = engine.video_config_codec();
  const unitCount = engine.video_unit_count();

  log(
    "video_is_interlaced: " + interlaced
    + (interlaced ? "  ✓ (1080i confirmed)" : "  ✗ (progressive — wrong fixture?)"),
    interlaced ? "ok" : "warn",
  );
  log("video_config_codec: " + (codec ?? "(null)"), codec ? "ok" : "warn");
  log("video_unit_count: " + unitCount);

  if (!codec) {
    log("No codec string — cannot continue.", "error");
    return;
  }
  if (unitCount === 0) {
    log("No video access units — cannot continue.", "error");
    return;
  }

  // Step 3 — isConfigSupported
  stepHead(3, "VideoDecoder.isConfigSupported");

  if (typeof VideoDecoder === "undefined") {
    log("VideoDecoder is NOT defined in this browser — WebCodecs unsupported.", "error");
    return;
  }

  let support;
  try {
    support = await VideoDecoder.isConfigSupported({ codec });
  } catch (e) {
    log("isConfigSupported threw: " + (e?.message ?? e), "error");
    return;
  }
  log(
    "supported: " + support.supported,
    support.supported ? "ok" : "warn",
  );
  log("config returned: " + JSON.stringify(support.config));

  // Step 4 — Actual decode attempt
  stepHead(4, "Decode attempt (first 30 AUs, Annex-B, no description)");

  let frameCount = 0;
  let frameDim = null;
  let decodeError = null;

  const MAX_AUS = Math.min(30, unitCount);

  // Collect the AUs first so we can issue them synchronously after configure.
  const aus = [];
  for (let i = 0; i < MAX_AUS; i++) {
    const unit = engine.video_unit(i);
    if (!unit) continue;
    const pts = unit.pts_ticks;  // bigint | undefined
    const ts = pts !== undefined
      ? Number(pts) * 1e6 / 90000
      : i * (1e6 / 25);  // fallback: assume 25 fps
    aus.push({ bytes: unit.bytes, ts, key: isKeyframe(unit.bytes), idx: i });
  }

  log("AUs collected: " + aus.length
    + " (keyframes: " + aus.filter(a => a.key).length + ")");

  let decoder;
  try {
    decoder = new VideoDecoder({
      output(frame) {
        frameCount++;
        if (!frameDim) {
          frameDim = frame.displayWidth + "x" + frame.displayHeight;
        }
        frame.close();
      },
      error(e) {
        decodeError = e?.message ?? String(e);
        log("VideoDecoder error callback: " + decodeError, "error");
      },
    });
  } catch (e) {
    log("VideoDecoder constructor threw: " + (e?.message ?? e), "error");
    return;
  }

  // configure — Annex-B: codec only, no description
  try {
    decoder.configure({ codec });
    log("decoder.configure() accepted (state: " + decoder.state + ")", "ok");
  } catch (e) {
    log("decoder.configure() threw: " + (e?.message ?? e), "error");
    return;
  }

  // Feed AUs
  let chunkErrors = 0;
  for (const au of aus) {
    try {
      decoder.decode(new EncodedVideoChunk({
        type: au.key ? "key" : "delta",
        timestamp: au.ts,
        data: au.bytes,
      }));
    } catch (e) {
      chunkErrors++;
      if (chunkErrors === 1) {
        log("decoder.decode() threw on AU " + au.idx + ": " + (e?.message ?? e), "warn");
      }
    }
  }
  if (chunkErrors > 1) {
    log("... and " + (chunkErrors - 1) + " more decode() throws.", "warn");
  }

  // flush
  try {
    await decoder.flush();
    log("decoder.flush() resolved.", "ok");
  } catch (e) {
    const msg = e?.message ?? String(e);
    log("decoder.flush() rejected: " + msg, "warn");
    // Don't return — we still want to report the frame count.
    if (!decodeError) decodeError = msg;
  }

  // Step 5 — BIG RESULT
  stepHead(5, "Result");

  if (frameCount > 0) {
    log(
      "HARDWARE INTERLACED DECODE: WORKS ✅\n"
      + frameCount + " frame(s) produced, " + (frameDim ?? "?") + "\n"
      + "codec: " + codec,
      "result-ok",
    );
  } else if (decodeError) {
    log(
      "HARDWARE INTERLACED DECODE: FAILED ❌\n"
      + "0 frames produced\n"
      + "error: " + decodeError,
      "result-fail",
    );
  } else if (chunkErrors > 0) {
    log(
      "HARDWARE INTERLACED DECODE: FAILED ❌\n"
      + "0 frames produced (all decode() calls threw)\n"
      + "codec: " + codec,
      "result-fail",
    );
  } else {
    log(
      "HARDWARE INTERLACED DECODE: 0 FRAMES ⚠️\n"
      + "No error reported but no frames were output.\n"
      + "isConfigSupported.supported=" + support.supported + "  codec=" + codec,
      "result-fail",
    );
  }
}

// Run, catching any uncaught top-level throws.
runProbe().catch(e => {
  const d = document.createElement("div");
  d.className = "entry result-fail";
  d.textContent = "UNCAUGHT ERROR: " + (e?.message ?? e);
  logEl.appendChild(d);
  console.error("[probe] uncaught", e);
});
