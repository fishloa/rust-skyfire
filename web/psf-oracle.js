// psf-oracle.js — decoder-verification gate for zenith's PsF re-signal (#38).
//
// Feeds a TS (a zenith re-signaled progressive-from-PsF sample) through the
// SkyfireBridge demux → WebCodecs VideoDecoder and emits a clear PASS/FAIL:
//   PASS  = frames decoded, no decoder error
//   FAIL  = a VideoDecoder error (e.g. PIPELINE_ERROR_DECODE from a still-field-
//           coded slice header) or zero frames out.
// window.__sfOracle = { verdict, frames, error } is set for headless harnesses.

import init, { SkyfireBridge } from "./pkg/skyfire_wasm.js";

const verdictEl = document.getElementById("verdict");
const logEl = document.getElementById("log");
const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d", { alpha: false });

function log(m) { logEl.textContent += m + "\n"; console.log("[oracle]", m); }
function setVerdict(cls, text) { verdictEl.className = cls; verdictEl.textContent = text; }

const PTS_HZ = 90_000;
const ticksToMicros = (t) => Number(t) * 1e6 / PTS_HZ;

let frames = 0;
let decodeError = null;

async function run() {
  await init();
  const bridge = new SkyfireBridge();
  const src = new URLSearchParams(location.search).get("src");
  if (!src) { setVerdict("fail", "FAIL — no ?src= given"); window.__sfOracle = { verdict: "fail", error: "no src" }; return; }

  log("fetching " + src);
  let resp;
  try { resp = await fetch(src); if (!resp.ok) throw new Error("HTTP " + resp.status); }
  catch (e) { setVerdict("fail", "FAIL — fetch " + e.message); window.__sfOracle = { verdict: "fail", error: String(e) }; return; }

  let decoder = null, configured = false, sawKey = false;

  const ensure = (codec) => {
    if (configured) return;
    decoder = new VideoDecoder({
      output(f) {
        frames++;
        if (canvas.width !== f.displayWidth) { canvas.width = f.displayWidth; canvas.height = f.displayHeight; }
        ctx.drawImage(f, 0, 0, canvas.width, canvas.height);
        f.close();
      },
      error(e) { decodeError = e?.message ?? String(e); log("DECODER ERROR: " + decodeError); },
    });
    decoder.configure({ codec, optimizeForLatency: true, description: bridge.video_config_description() });
    configured = true;
    log("configured " + codec);
  };

  const pump = () => {
    const cs = bridge.video_codec();
    if (!cs) return;
    ensure(cs);
    for (const au of bridge.take_video_aus()) {
      const key = au.is_keyframe;
      if (!sawKey) { if (!key) { au.free?.(); continue; } sawKey = true; }
      try {
        decoder.decode(new EncodedVideoChunk({
          type: key ? "key" : "delta",
          timestamp: au.pts_ticks !== undefined ? ticksToMicros(au.pts_ticks) : 0,
          data: au.bytes,
        }));
      } catch (e) { decodeError = e?.message ?? String(e); log("decode() threw: " + decodeError); }
      au.free?.();
    }
  };

  const reader = resp.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    bridge.feed(value);
    pump();
    if (decodeError) break;
  }
  bridge.flush();
  pump();
  if (decoder && configured) { try { await decoder.flush(); } catch (e) { decodeError ||= String(e); } }

  log(`frames=${frames} error=${decodeError ?? "none"}`);
  if (decodeError) {
    setVerdict("fail", `FAIL — decoder error after ${frames} frames`);
    window.__sfOracle = { verdict: "fail", frames, error: decodeError };
  } else if (frames > 0) {
    setVerdict("pass", `PASS — ${frames} frames decoded clean`);
    window.__sfOracle = { verdict: "pass", frames, error: null };
  } else {
    setVerdict("fail", "FAIL — 0 frames, no error (no keyframe / empty?)");
    window.__sfOracle = { verdict: "fail", frames: 0, error: "no frames" };
  }
}

run().catch((e) => { setVerdict("fail", "FAIL — " + (e?.message ?? e)); window.__sfOracle = { verdict: "fail", error: String(e) }; });
