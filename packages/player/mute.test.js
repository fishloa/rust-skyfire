import { test, expect, mock } from "bun:test";
import { Window } from "happy-dom";

// ═══════════════════════════════════════════════════════════════════════════
// Mute-button initial-state + toggle tests (issue #115).
//
// The bug: _buildControls created the mute button with a hardcoded 🔊 glyph, so
// a <skyfire-player muted> element started muted but the button *looked*
// unmuted — glyph, ARIA and actual engine mute state disagreed, and the user's
// first click toggled from an already-muted state (a dead press).
//
// This is element UI and needs a DOM. The other suites exercise the (non-DOM)
// SkyfirePlayer behind WASM mocks; here happy-dom supplies the HTML DOM (shadow
// root, custom elements, events) so the element itself is constructed and its
// buttons asserted directly in bun.
//
// happy-dom timing note: it applies the 'muted' boolean attribute asynchronously
// (attributeChangedCallback fires after the constructor has already read
// hasAttribute("muted")), so to model "created muted" we drive the real
// attribute wiring first (muted = true → _muted2) and then (re)build the
// controls — the same method connectedCallback runs to build the initial bar.
// The attribute→_muted2 reflection itself is separately covered by
// web/tests/element.spec.mjs.
// ═══════════════════════════════════════════════════════════════════════════

// ── Pre-mock the WASM-loading modules before importing SkyfireElement ──
const MockBridge = class SkyfireBridgeMock {
  constructor() { this._props = {}; }
  get selected_audio_pid() { return this._props.selected_audio_pid ?? null; }
  select_audio() {}
  select_subtitle() {}
  video_codec() { return null; }
  audio_native_channels() { return null; }
  set_playing() {}
};

mock.module("@firemedia/skyfire-core/pkg/skyfire_wasm.js", () => ({
  SkyfireBridge: MockBridge,
  ProbeResult: class {},
  WasmAudioTrack: class {},
  WasmEngine: class {},
  WasmMediaSegment: class {},
  WasmPcmChunk: class {},
  WasmSubtitleCue: class {},
  WasmSubtitleRegion: class {},
  WasmSubtitleTrack: class {},
  WasmTrackList: class {},
  WasmVideoAu: class {},
  WasmVideoUnit: class {},
}));

mock.module("@firemedia/skyfire-core", () => ({
  initSkyfire: () => Promise.resolve(),
  SkyfireBridge: MockBridge,
  PTS_HZ: 90000,
  ticksToMicros: (t) => Math.floor((t * 1_000_000) / 90000),
}));

const win = new Window({ url: "http://localhost/" });
for (const k of ["HTMLElement", "document", "customElements", "Node", "Event", "CustomEvent"]) {
  globalThis[k] = win[k];
}
globalThis.window = win;

const { SkyfirePlayerElement } = await import("./skyfire-element.js");
const sleep = (ms) => new Promise((r) => win.setTimeout(r, ms));

/** Build a connected <skyfire-player controls="full"> element. */
function makeElement() {
  const el = new SkyfirePlayerElement();
  el.setAttribute("controls", "full");
  win.document.body.appendChild(el);
  return el;
}

test("default (no muted attribute): button renders 🔊 + aria-pressed=false", () => {
  const el = makeElement();                        // connectedCallback builds the bar
  expect(el._muted2).toBe(false);                  // absent attribute ⇒ unmuted default
  expect(el._muteBtn.textContent).toBe("🔊");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("false");
});

test("muted attribute present: button renders 🔇 + aria-pressed=true", async () => {
  const el = makeElement();
  el.muted = true;                                 // real attribute wiring → _muted2
  await sleep(0);
  expect(el._muted2).toBe(true);
  el._buildControls();                             // (re)build initial bar in muted state
  expect(el._muteBtn.textContent).toBe("🔇");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("true");
});

test("toggle from muted start: first click actually unmutes (icon, aria and engine agree)", async () => {
  const el = makeElement();
  el.muted = true;
  await sleep(0);
  expect(el._muted2).toBe(true);
  el._buildControls();
  const muted = mock((v) => {});
  el._engine = { setMuted: muted };

  // First press on an element that started muted must UNMUTE — the dead-press bug.
  el._toggleMute();

  expect(el._muted2).toBe(false);                  // engine state now unmuted
  expect(muted).toHaveBeenCalledWith(false);       // engine explicitly told to unmute
  expect(el._muteBtn.textContent).toBe("🔊");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("false");

  // Second press mutes again; all three stay in agreement.
  el._toggleMute();
  expect(el._muted2).toBe(true);
  expect(muted).toHaveBeenCalledWith(true);
  expect(el._muteBtn.textContent).toBe("🔇");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("true");
});

test("toggle from unmuted start: first click mutes (icon, aria and engine agree)", () => {
  const el = makeElement();
  const muted = mock((v) => {});
  el._engine = { setMuted: muted };

  el._toggleMute();

  expect(el._muted2).toBe(true);
  expect(muted).toHaveBeenCalledWith(true);
  expect(el._muteBtn.textContent).toBe("🔇");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("true");

  el._toggleMute();
  expect(el._muted2).toBe(false);
  expect(muted).toHaveBeenCalledWith(false);
  expect(el._muteBtn.textContent).toBe("🔊");
  expect(el._muteBtn.getAttribute("aria-pressed")).toBe("false");
});
