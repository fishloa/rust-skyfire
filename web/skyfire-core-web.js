// web/skyfire-core-web.js — web-target WASM facade for the example app.
//
// WHY THIS FILE EXISTS (bundler-target vs web-target split):
//
//   packages/core/skyfire-core.js  → imports from ./pkg/skyfire_wasm.js
//     pkg/ was built with:  wasm-pack build --target bundler
//     That build uses ES module syntax with a __wbindgen_placeholder__ import
//     that only resolves inside a bundler (webpack, vite, rollup, etc.).
//     It will NOT load raw in a browser via a plain <script type="module">.
//
//   web/skyfire-core-web.js  → imports from ./pkg/skyfire_wasm.js  (THIS file)
//     web/pkg/ is built with:  wasm-pack build --target web
//     That build exports an explicit default init() function and loads the .wasm
//     via a fetch() relative URL, so it works raw in a browser with no bundler.
//
// CONSUMERS:
//   - This file: used ONLY by the example app in web/ (served raw by web/serve.ts).
//   - packages/core: used by npm consumers that bundle their app (not this file).
//
// The import map in web/index.html points @skyfire/core at THIS file so that
// the bare specifier resolves correctly without a bundler.  npm/bundler consumers
// resolve @skyfire/core via packages/core/package.json as usual.

import initWasm, { SkyfireBridge } from "./pkg/skyfire_wasm.js";

let _ready = null;

/** Initialize the WASM module (web-target build in web/pkg/). Idempotent. */
export function initSkyfire() {
  if (!_ready) _ready = initWasm();
  return _ready;
}

export { SkyfireBridge };
