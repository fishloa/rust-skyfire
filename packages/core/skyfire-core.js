// @skyfire/core — WASM demux + AC-3/E-AC-3 decode + A/V-sync + DVB-sub composite.
// The host wires WebCodecs / WebAudio / canvas. See README for the API.
import initWasm, { SkyfireBridge } from "./pkg/skyfire_wasm.js";

let _ready = null;

/** Initialize the WASM module. Idempotent; await before constructing a bridge. */
export function initSkyfire() {
  if (!_ready) _ready = initWasm();
  return _ready;
}

export { SkyfireBridge };
