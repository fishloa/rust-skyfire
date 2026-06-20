/**
 * decode-worker.js — off-main-thread H.264 WASM software decode worker.
 *
 * Protocol (main → worker):
 *   { type: "init" }
 *     Initialise the WASM module and decoder.  Worker replies { type: "ready" }.
 *
 *   { type: "au", bytes: ArrayBuffer, pts: number }
 *     Submit one Annex-B access unit (bytes is transferred, not copied).
 *     Worker drains decoded frames immediately and posts each as:
 *       { type: "frame", width, height, ptsTicks, buf: ArrayBuffer }
 *     buf is the raw I420 data (Y plane first, then U, then V), transferred.
 *
 *   { type: "flush" }
 *     Flush the decoder.  Worker drains remaining frames (same posts as above),
 *     then replies { type: "done" }.
 */

import init, { WasmVideoDecoder } from "./pkg/skyfire_wasm.js";

let dec = null;
let __auCount = 0;
let __frameCount = 0;

/**
 * Drain any decoded frames from dec, posting each as a transferred ArrayBuffer.
 */
function drainFrames() {
  let f;
  while ((f = dec.receive()) !== undefined) {
    const w = f.width;
    const h = f.height;
    const ptsTicks = f.pts_ticks;

    // Copy I420 data out of wasm memory before calling f.free().
    // Layout: Y[w*h] U[w/2*h/2] V[w/2*h/2]
    const i420Len = w * h + 2 * ((w >> 1) * (h >> 1));
    const buf = new ArrayBuffer(i420Len);
    new Uint8Array(buf).set(f.data.subarray(0, i420Len));
    f.free();

    __frameCount++;
    // Transfer the buffer — zero-copy to main thread.
    self.postMessage({ type: "frame", width: w, height: h, ptsTicks, buf }, [buf]);
  }
}

self.onmessage = async (evt) => {
  const msg = evt.data;

  switch (msg.type) {
    case "init": {
      await init();
      dec = new WasmVideoDecoder();
      self.postMessage({ type: "ready" });
      break;
    }

    case "au": {
      if (!dec) return;
      __auCount++;
      try {
        dec.send(new Uint8Array(msg.bytes), msg.pts);
        drainFrames();
      } catch (e) {
        self.postMessage({ type: "dbg", msg: "decode error au#" + __auCount + ": " + e });
      }
      break;
    }

    case "flush": {
      if (!dec) return;
      dec.flush();
      drainFrames();
      self.postMessage({ type: "done" });
      break;
    }

    default:
      console.warn("[decode-worker] unknown message type:", msg.type);
  }
};
