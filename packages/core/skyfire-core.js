// @firemedia/skyfire-core — WASM demux + AC-3/E-AC-3 decode + A/V-sync + DVB-sub composite.
// The host wires WebCodecs / WebAudio / canvas. See README for the API.
// pkg/ is wasm-pack's `--target bundler` output: it has NO default/init export —
// the bundler instantiates the wasm on import (use vite-plugin-wasm +
// vite-plugin-top-level-await, or webpack asyncWebAssembly), so SkyfireBridge is
// ready as soon as this module is imported. (The web-target facade
// web/skyfire-core-web.js keeps a real init() because `--target web` needs it.)
import { SkyfireBridge } from "./pkg/skyfire_wasm.js";

/** Kept for API parity with the web facade. The bundler auto-initialises the
 *  wasm on import, so there is nothing to await — resolves immediately. */
export function initSkyfire() {
  return Promise.resolve();
}

export { SkyfireBridge };

// ── Canonical timestamp helpers (single source of truth) ────────────────

/** MPEG-TS PTS clock frequency (90 kHz). */
export const PTS_HZ = 90_000;

/** Convert 90 kHz PTS ticks to microseconds. */
export const ticksToMicros = (t) => Number(t) * 1_000_000 / PTS_HZ;
