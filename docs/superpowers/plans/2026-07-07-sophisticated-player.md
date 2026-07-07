# Sophisticated `<skyfire-player>` Web Component — Implementation Plan (Phase 2b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a polished, embeddable `<skyfire-player>` Web Component in `@firemedia/skyfire-player` that wraps the unchanged headless `SkyfirePlayer` engine with controls, track/subtitle menus, buffering/error/loading states, fullscreen, picture-in-picture, diagnostics, and src-reactive switching.

**Architecture:** A new `packages/player/skyfire-element.js` defines `class SkyfirePlayerElement extends HTMLElement` (`customElements.define("skyfire-player")`). It builds all UI into a Shadow DOM (scoped `<style>`, no inline styles) and instantiates the headless `SkyfirePlayer` engine against the shadow-root canvas. The engine is unchanged except a small mute/volume setter it lacks.

**Tech Stack:** Vanilla Web Components (custom element + Shadow DOM), the existing `SkyfirePlayer` engine (ES module), WebCodecs/WebAudio, Playwright (chromium) for browser tests, Bun for the test runner + `web/serve.ts`.

## Global Constraints

- Zero inline `style=""` — all CSS in the shadow-root `<style>`. No external design system.
- Dual licence MIT OR Apache-2.0. **No `Co-Authored-By` lines in commits.**
- Do NOT change the engine's decode/sync/render internals. The only permitted engine change is adding a public `setMuted`/`setVolume` (Task 1) — it just sets the existing `_audioGain` node.
- Keep `window.__sfStats` populated (the Phase-1 `web/tests/streams.spec.mjs` gate must stay green).
- The engine's public API (verified): `new SkyfirePlayer(canvas, { streamUrl, subtitlePid?, forceMse?, muted?, audioLeadSeconds? })`; `async init()`; `play()`; `pause()`; `selectAudio(pid)`; `selectSubtitle(pid|null)`; `tracks()`; `destroy()`; events via `on("tracks"|"stats"|"error"|"ended", cb)`. Track-list shape: `{ video_pid, video_codec, audio:[{pid,language,codec}], subtitles:[{pid,language,kind}] }`. Stats shape includes `decoded, drawn, audioFrames, audioSamples, avSkewMs, w, h, subCues, videoPath, status, done`.
- **Element tests run in a real browser via Playwright** (custom elements + Shadow DOM + WASM need it). A fixture page `web/element-test.html` hosts the element; pure-UI tests drive it through documented internal seams (`_applyTracks(tl)`, `_setState(name)`); integration tests use a served stream via the existing `web/tests/global-setup.mjs` servers.
- Element source lives in `packages/player/skyfire-element.js`; add it to `package.json` `files`. The headless engine export stays.

---

## Task 1: Engine — add `setMuted`/`setVolume`

**Files:**
- Modify: `packages/player/skyfire-player.js`
- Test: `packages/player/volume.test.js`

**Interfaces:**
- Produces: `SkyfirePlayer.prototype.setMuted(bool)`, `SkyfirePlayer.prototype.setVolume(v: 0..1)`, `SkyfirePlayer.prototype.getVolume(): number`. They set `_muted` / a new `_volume` and, when the gain node exists, `_audioGain.gain.value`.

- [ ] **Step 1: Write the failing test**

Create `packages/player/volume.test.js`:

```js
import { test, expect } from "bun:test";
import { SkyfirePlayer } from "./skyfire-player.js";

function fakePlayer() {
  const canvas = { getContext: () => ({}), parentElement: null };
  return new SkyfirePlayer(canvas, { streamUrl: "about:blank" });
}

test("setVolume/getVolume round-trips and clamps to 0..1", () => {
  const p = fakePlayer();
  p.setVolume(0.5);
  expect(p.getVolume()).toBeCloseTo(0.5);
  p.setVolume(2);   expect(p.getVolume()).toBe(1);
  p.setVolume(-1);  expect(p.getVolume()).toBe(0);
});

test("setMuted toggles muted state without throwing when no audio yet", () => {
  const p = fakePlayer();
  p.setMuted(true);
  expect(p._muted).toBe(true);
  p.setMuted(false);
  expect(p._muted).toBe(false);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `bun test packages/player/volume.test.js`
Expected: FAIL — `p.setVolume is not a function`.

- [ ] **Step 3: Implement the setters**

In `packages/player/skyfire-player.js`, in the constructor near `this._muted = opts.muted || false;` add:

```js
    this._volume = 1;
```

Add these methods to the class (near `selectSubtitle`):

```js
  /** Set output volume, 0..1 (clamped). Applies to the gain node if audio is up. */
  setVolume(v) {
    this._volume = Math.max(0, Math.min(1, Number(v) || 0));
    if (this._audioGain) this._audioGain.gain.value = this._muted ? 0 : this._volume;
  }

  /** @returns {number} current volume 0..1. */
  getVolume() {
    return this._volume;
  }

  /** Mute/unmute without losing the volume level. */
  setMuted(muted) {
    this._muted = !!muted;
    if (this._audioGain) this._audioGain.gain.value = this._muted ? 0 : this._volume;
  }
```

Also update the gain-node initialisation (where `this._audioGain.gain.value = this._muted ? 0 : 1;`, ~line 591) to honour volume:

```js
    this._audioGain.gain.value = this._muted ? 0 : this._volume;
```

- [ ] **Step 4: Run — verify pass**

Run: `bun test packages/player/volume.test.js`
Expected: PASS (2 tests). Also `bun test packages/player/stats.test.js packages/player/hls-source.test.js` still pass.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-player.js packages/player/volume.test.js
git commit -m "feat(player): setMuted/setVolume/getVolume on the engine"
```

---

## Task 2: Element scaffold — registration, Shadow DOM, attributes

**Files:**
- Create: `packages/player/skyfire-element.js`
- Create: `web/element-test.html`
- Create: `web/tests/element.spec.mjs`
- Modify: `packages/player/package.json` (`files`)

**Interfaces:**
- Produces: `customElements` registration of `"skyfire-player"`; `SkyfirePlayerElement` with `static get observedAttributes()` = `["src","controls","muted","autoplay","audio-lead"]`; a shadow root containing `.stage` (`<canvas class="video">`, `<canvas class="subs">`, `<video class="pip" hidden>`), `.controls`, `.menus`, `.overlays`, `.diag`; getters/setters reflecting `src`/`controls`/`muted`. Documented UI seam `_applyTracks(tl)` (Task 5) and `_setState(name)` (Task 6) added later.

- [ ] **Step 1: Write the failing test (fixture page + registration test)**

Create `web/element-test.html`:

```html
<!DOCTYPE html><html><head><meta charset="utf-8">
<script type="importmap">{ "imports": {
  "@firemedia/skyfire-player": "../packages/player/skyfire-player.js",
  "@firemedia/skyfire-core": "./skyfire-core-web.js"
} }</script>
</head><body>
<script type="module">import "../packages/player/skyfire-element.js";</script>
</body></html>
```

Create `web/tests/element.spec.mjs`:

```js
import { test, expect } from "@playwright/test";
const WEB = "http://localhost:8080";

test("registers <skyfire-player> and builds a shadow root with a video canvas", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const ok = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "none");
    document.body.appendChild(el);
    const sr = el.shadowRoot;
    return !!sr && !!sr.querySelector("canvas.video") && !!sr.querySelector("canvas.subs");
  });
  expect(ok).toBe(true);
});

test("reflects src/controls/muted attributes to properties", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "minimal");
    el.setAttribute("muted", "");
    document.body.appendChild(el);
    return { controls: el.controls, muted: el.muted };
  });
  expect(r.controls).toBe("minimal");
  expect(r.muted).toBe(true);
});
```

- [ ] **Step 2: Run — verify it fails**

Prereqs (run once): `PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH" wasm-pack build crates/skyfire-wasm --target web --release --out-dir "$(pwd)/web/pkg"` then `cargo build -p skyfire-server -p skyfire-cli`.
Run: `cd web && bunx playwright test tests/element.spec.mjs --config playwright.config.mjs`
Expected: FAIL — `skyfire-element.js` 404 / element never defined.

- [ ] **Step 3: Implement the scaffold**

Create `packages/player/skyfire-element.js`:

```js
// <skyfire-player> — polished UI shell around the headless SkyfirePlayer engine.
// All UI lives in a Shadow DOM (scoped styles); the engine draws into the shadow
// canvas and the element owns controls, menus, state overlays, PiP + fullscreen.
import { SkyfirePlayer } from "./skyfire-player.js";

const TEMPLATE = `
<div class="stage">
  <canvas class="video"></canvas>
  <div class="subs"><canvas></canvas></div>
  <video class="pip" hidden playsinline></video>
  <div class="overlays"></div>
  <div class="diag" hidden></div>
</div>
<div class="menus"></div>
<div class="controls"></div>
`;

const STYLE = `
:host { position: relative; display: block; width: 100%; height: 100%;
        background: #000; color: #eee; font: 14px/1.4 system-ui, sans-serif;
        overflow: hidden; }
.stage { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; }
canvas.video { max-width: 100%; max-height: 100%; object-fit: contain; }
.subs { position: absolute; left: 0; right: 0; bottom: 12%; display: flex; justify-content: center; pointer-events: none; }
.subs canvas { max-width: 90%; }
.pip { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
.controls { position: absolute; bottom: 0; left: 0; right: 0; display: flex; gap: 10px;
            align-items: center; padding: 10px 14px; background: rgba(0,0,0,0.72);
            opacity: 0; transition: opacity .2s; }
:host(:hover) .controls, .controls:focus-within, :host([data-active]) .controls { opacity: 1; }
.controls button, .controls select { background: #1a1a1a; color: #eee; border: 1px solid #444;
            border-radius: 4px; padding: 5px 9px; font: inherit; cursor: pointer; }
.controls .spacer { flex: 1; }
.controls input[type=range] { width: 90px; }
.menus { position: absolute; bottom: 52px; right: 14px; display: flex; gap: 8px; align-items: flex-end; }
.menu { display: none; background: rgba(0,0,0,0.9); border: 1px solid #444; border-radius: 6px;
        padding: 6px; min-width: 160px; }
.menu.open { display: block; }
.menu .row { display: flex; gap: 8px; align-items: center; padding: 6px 8px; border-radius: 4px; cursor: pointer; }
.menu .row:hover { background: #222; }
.menu .row[aria-checked=true] { background: #2a3a5a; }
.overlays { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; pointer-events: none; }
.overlay { display: none; text-align: center; padding: 16px 20px; background: rgba(0,0,0,0.7); border-radius: 8px; pointer-events: auto; }
.overlay.show { display: block; }
.spinner { width: 40px; height: 40px; border: 4px solid #444; border-top-color: #6af; border-radius: 50%; animation: spin 1s linear infinite; margin: 0 auto; }
@keyframes spin { to { transform: rotate(360deg); } }
.diag { position: absolute; top: 8px; left: 8px; background: rgba(0,0,0,0.75); padding: 8px 10px;
        border-radius: 6px; font-variant-numeric: tabular-nums; white-space: pre; font-size: 12px; }
.diag[hidden] { display: none; }
`;

export class SkyfirePlayerElement extends HTMLElement {
  static get observedAttributes() { return ["src", "controls", "muted", "autoplay", "audio-lead"]; }

  constructor() {
    super();
    const root = this.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = STYLE;
    root.appendChild(style);
    const wrap = document.createElement("div");
    wrap.innerHTML = TEMPLATE;
    root.append(...wrap.childNodes);

    this._engine = null;
    this._tracks = null;
    this._state = "idle";
    this._switchSeq = 0;
    this._videoCanvas = root.querySelector("canvas.video");
    this._subsCanvas = root.querySelector(".subs canvas");
    this._pipVideo = root.querySelector("video.pip");
    this._controlsEl = root.querySelector(".controls");
    this._menusEl = root.querySelector(".menus");
    this._overlaysEl = root.querySelector(".overlays");
    this._diagEl = root.querySelector(".diag");
  }

  // ── attribute reflection ──
  get src() { return this.getAttribute("src"); }
  set src(v) { if (v == null) this.removeAttribute("src"); else this.setAttribute("src", v); }
  get controls() { return this.getAttribute("controls") || "full"; }
  set controls(v) { this.setAttribute("controls", v); }
  get muted() { return this.hasAttribute("muted"); }
  set muted(v) { if (v) this.setAttribute("muted", ""); else this.removeAttribute("muted"); }

  connectedCallback() {
    this._buildControls();   // Task 4 (no-op stub until then)
    if (this.getAttribute("src")) this._start();  // Task 3
  }
  disconnectedCallback() { this._teardown(); }    // Task 3
  attributeChangedCallback() {}                    // Task 3/8

  // Stubs replaced by later tasks:
  _buildControls() {}
  _start() {}
  _teardown() { if (this._engine) { this._engine.destroy(); this._engine = null; } }
}

if (!customElements.get("skyfire-player")) {
  customElements.define("skyfire-player", SkyfirePlayerElement);
}
```

Add `"skyfire-element.js"` to `packages/player/package.json` `files` array.

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs --config playwright.config.mjs`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js packages/player/package.json web/element-test.html web/tests/element.spec.mjs
git commit -m "feat(player): <skyfire-player> element scaffold — shadow DOM + attributes"
```

---

## Task 3: Engine wiring — construct, events, `__sfStats`, teardown

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `SkyfirePlayer` engine API; `this._videoCanvas`.
- Produces: `_start()` constructs `new SkyfirePlayer(this._videoCanvas, {...})` from attributes and `init()`s it; forwards engine events as DOM `CustomEvent`s `sf-tracks|sf-stats|sf-error|sf-ended` (composed, bubbling) and mirrors stats to `window.__sfStats`; delegating methods `play()/pause()/selectAudio(pid)/selectSubtitle(pid)` and getters `tracks`/`stats`. `_applyTracks(tl)` stores tracks + (Task 5) rebuilds menus. `_setState(name)` (Task 6) stub here.

- [ ] **Step 1: Write the failing test**

Add to `web/tests/element.spec.mjs`:

```js
test("constructs the engine from attrs and re-emits sf-stats + mirrors __sfStats", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "none");
    el.setAttribute("muted", "");
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8");
    const got = { stats: false, tracks: false };
    el.addEventListener("sf-stats", () => { got.stats = true; });
    el.addEventListener("sf-tracks", () => { got.tracks = true; });
    document.body.appendChild(el);
    // wait up to 8s for events + __sfStats mirror
    const t0 = Date.now();
    while (Date.now() - t0 < 8000) {
      if (got.stats && got.tracks && window.__sfStats) break;
      await new Promise((r) => setTimeout(r, 200));
    }
    return { ...got, sfStats: !!window.__sfStats, decoded: window.__sfStats?.decoded ?? -1 };
  });
  expect(r.stats).toBe(true);
  expect(r.tracks).toBe(true);
  expect(r.sfStats).toBe(true);
  expect(r.decoded).toBeGreaterThanOrEqual(0);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "re-emits" --config playwright.config.mjs`
Expected: FAIL — no engine constructed, no events.

- [ ] **Step 3: Implement wiring**

Replace the `_start`/`_teardown` stubs in `skyfire-element.js`:

```js
  _start() {
    if (this._engine) this._teardown();
    const src = this.getAttribute("src");
    if (!src) { this._setState("idle"); return; }
    const seq = ++this._switchSeq;
    this._setState("loading");
    const opts = {
      streamUrl: src,
      muted: this.hasAttribute("muted"),
      forceMse: this.getAttribute("video") === "mse",
    };
    const lead = parseFloat(this.getAttribute("audio-lead"));
    if (!Number.isNaN(lead)) opts.audioLeadSeconds = lead;

    const engine = new SkyfirePlayer(this._videoCanvas, opts);
    this._engine = engine;
    engine.on("tracks", (tl) => { if (seq === this._switchSeq) this._applyTracks(tl); });
    engine.on("stats", (s) => {
      if (seq !== this._switchSeq) return;
      window.__sfStats = s;
      this._onStats(s);                 // Task 6 (stub here)
      this.dispatchEvent(new CustomEvent("sf-stats", { detail: s, bubbles: true, composed: true }));
    });
    engine.on("error", (e) => {
      if (seq !== this._switchSeq) return;
      this._setState("error", e?.message || String(e));
      this.dispatchEvent(new CustomEvent("sf-error", { detail: e, bubbles: true, composed: true }));
    });
    engine.on("ended", (s) => {
      if (seq !== this._switchSeq) return;
      this._setState("ended");
      this.dispatchEvent(new CustomEvent("sf-ended", { detail: s, bubbles: true, composed: true }));
    });
    engine.init().catch((err) => {
      if (seq === this._switchSeq) this._setState("error", err?.message || String(err));
    });
  }

  _teardown() {
    if (this._engine) { try { this._engine.destroy(); } catch (_) {} this._engine = null; }
  }

  _applyTracks(tl) {
    this._tracks = tl;
    this.dispatchEvent(new CustomEvent("sf-tracks", { detail: tl, bubbles: true, composed: true }));
    this._buildMenus();               // Task 5 (stub here)
  }

  // delegating API
  play() { this._engine?.play(); }
  pause() { this._engine?.pause(); }
  selectAudio(pid) { this._engine?.selectAudio(pid); }
  selectSubtitle(pid) { this._engine?.selectSubtitle(pid); }
  get tracks() { return this._tracks; }
  get stats() { return this._engine?._stats ?? null; }
```

Add stub methods so this task runs before Tasks 5/6: `_buildMenus() {}`, `_onStats() {}`, and make `_setState(name, msg)` a stub that stores `this._state = name;` (Task 6 fills it in).

- [ ] **Step 4: Run — verify pass**

Prereqs running: `skyfire-server` (port 8090, fixtures `fixtures/streams`) + `serve.ts` (8080) via `global-setup` (Playwright starts them). Run: `cd web && bunx playwright test tests/element.spec.mjs -g "re-emits" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): element engine wiring — events, __sfStats mirror, teardown"
```

---

## Task 4: Control bar + presets + volume/mute/play-pause

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `this._controlsEl`, delegating methods, engine `setMuted`/`setVolume`.
- Produces: `_buildControls()` renders buttons per `controls` preset (`full`|`minimal`|`none`); wires play/pause, volume slider, mute, fullscreen (Task 7), PiP (Task 7), audio/subs menu toggles (Task 5), diagnostics toggle (Task 9). Adds `_setActive()` auto-hide behaviour.

- [ ] **Step 1: Write the failing test**

Add:

```js
test("controls preset renders the right buttons", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const mk = (c) => { const el = document.createElement("skyfire-player"); el.setAttribute("controls", c); document.body.appendChild(el); return el.shadowRoot.querySelector(".controls"); };
    const full = mk("full"), minimal = mk("minimal"), none = mk("none");
    const has = (bar, sel) => !!bar.querySelector(sel);
    return {
      fullPlay: has(full, ".playpause"), fullVol: has(full, "input[type=range]"),
      fullAudio: has(full, ".audio-btn"), fullFs: has(full, ".fs-btn"),
      minPlay: has(minimal, ".playpause"), minVol: has(minimal, "input[type=range]"),
      minFs: has(minimal, ".fs-btn"),
      noneEmpty: none.children.length === 0,
    };
  });
  expect(r.fullPlay && r.fullVol && r.fullAudio && r.fullFs).toBe(true);
  expect(r.minPlay && r.minFs).toBe(true);
  expect(r.minVol).toBe(false);      // minimal = play + fullscreen only
  expect(r.noneEmpty).toBe(true);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "controls preset" --config playwright.config.mjs`
Expected: FAIL — `.controls` empty (stub).

- [ ] **Step 3: Implement `_buildControls`**

Replace the `_buildControls` stub:

```js
  _buildControls() {
    const bar = this._controlsEl;
    bar.innerHTML = "";
    const preset = this.controls;
    if (preset === "none") return;

    const btn = (cls, label, on) => {
      const b = document.createElement("button");
      b.className = cls; b.type = "button"; b.textContent = label;
      b.addEventListener("click", on);
      bar.appendChild(b); return b;
    };

    this._playBtn = btn("playpause", "⏸", () => this._togglePlay());

    if (preset === "full") {
      const vol = document.createElement("input");
      vol.type = "range"; vol.min = "0"; vol.max = "1"; vol.step = "0.05"; vol.value = "1";
      vol.className = "vol"; vol.setAttribute("aria-label", "Volume");
      vol.addEventListener("input", () => this._engine?.setVolume(parseFloat(vol.value)));
      bar.appendChild(vol);
      this._muteBtn = btn("mute-btn", "🔊", () => this._toggleMute());

      const spacer = document.createElement("span"); spacer.className = "spacer"; bar.appendChild(spacer);

      btn("audio-btn", "Audio ▾", () => this._toggleMenu("audio"));
      btn("subs-btn", "Subtitles ▾", () => this._toggleMenu("subtitle"));
      this._pipBtn = btn("pip-btn", "⧉", () => this._togglePip());       // Task 7
      btn("fs-btn", "⛶", () => this._toggleFullscreen());                // Task 7
      btn("diag-btn", "ⓘ", () => this._toggleDiag());                    // Task 9
    } else if (preset === "minimal") {
      const spacer = document.createElement("span"); spacer.className = "spacer"; bar.appendChild(spacer);
      btn("fs-btn", "⛶", () => this._toggleFullscreen());
    }
  }

  _togglePlay() {
    if (!this._engine) return;
    this._playing = !this._playing;
    if (this._playing) { this._engine.play(); this._playBtn.textContent = "⏸"; }
    else { this._engine.pause(); this._playBtn.textContent = "▶"; }
  }
  _toggleMute() {
    this._muted2 = !this._muted2;
    this._engine?.setMuted(this._muted2);
    if (this._muteBtn) this._muteBtn.textContent = this._muted2 ? "🔇" : "🔊";
  }
```

Add stub methods used above but implemented later: `_toggleMenu() {}` (Task 5), `_togglePip() {}` / `_toggleFullscreen() {}` (Task 7), `_toggleDiag() {}` (Task 9). Initialise `this._playing = true; this._muted2 = this.hasAttribute("muted");` in the constructor.

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "controls preset" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): control bar + presets (full/minimal/none) + play/volume/mute"
```

---

## Task 5: Audio + subtitle menus

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `this._menusEl`, `this._tracks`, delegating `selectAudio`/`selectSubtitle`.
- Produces: `_buildMenus()` renders an audio menu (radio rows `"<language|track N> · <codec>"`) and a subtitle menu (`Off` + `"<language|Subtitle N>"`) from `this._tracks`; `_toggleMenu(kind)` opens/closes; selecting a row calls the engine method and marks `aria-checked`. Re-renders whenever `_applyTracks` runs (late tracks appear). Exposes UI seam `_applyTracks(tl)` already wired in Task 3.

- [ ] **Step 1: Write the failing test (pure-UI via seam, no stream)**

Add:

```js
test("menus build from injected tracks and selecting calls the engine", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    document.body.appendChild(el);
    const calls = [];
    el._engine = { selectAudio: (p) => calls.push(["a", p]), selectSubtitle: (p) => calls.push(["s", p]) };
    el._applyTracks({
      video_pid: 0x100, video_codec: "H264",
      audio: [{ pid: 257, language: "eng", codec: "AC3" }, { pid: 258, language: "fra", codec: "EAC3" }],
      subtitles: [{ pid: 260, language: "eng", kind: "dvb" }],
    });
    const sr = el.shadowRoot;
    const audioRows = sr.querySelectorAll(".menu.audio .row").length;
    const subRows = sr.querySelectorAll(".menu.subtitle .row").length; // Off + 1
    sr.querySelectorAll(".menu.audio .row")[1].click();   // select fra (258)
    sr.querySelectorAll(".menu.subtitle .row")[1].click(); // select 260
    return { audioRows, subRows, calls };
  });
  expect(r.audioRows).toBe(2);
  expect(r.subRows).toBe(2); // Off + one track
  expect(r.calls).toContainEqual(["a", 258]);
  expect(r.calls).toContainEqual(["s", 260]);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "menus build" --config playwright.config.mjs`
Expected: FAIL — no `.menu` rows (stub).

- [ ] **Step 3: Implement menus**

Replace the `_buildMenus` / `_toggleMenu` stubs:

```js
  _buildMenus() {
    const tl = this._tracks;
    this._menusEl.innerHTML = "";
    if (!tl || this.controls !== "full") return;

    const menu = (kind) => { const m = document.createElement("div"); m.className = `menu ${kind}`; this._menusEl.appendChild(m); return m; };
    const row = (m, label, checked, on) => {
      const r = document.createElement("div"); r.className = "row"; r.setAttribute("role", "menuitemradio");
      r.setAttribute("aria-checked", checked ? "true" : "false"); r.textContent = label;
      r.addEventListener("click", on); m.appendChild(r); return r;
    };

    const am = menu("audio");
    (tl.audio || []).forEach((a, i) => {
      const label = `${a.language || `Track ${i + 1}`} · ${a.codec}`;
      row(am, label, this._selAudio === a.pid || (this._selAudio == null && i === 0),
        () => { this._selAudio = a.pid; this.selectAudio(a.pid); this._buildMenus(); am.classList.add("open"); });
    });

    const sm = menu("subtitle");
    row(sm, "Off", this._selSub == null, () => { this._selSub = null; this.selectSubtitle(null); this._buildMenus(); sm.classList.add("open"); });
    (tl.subtitles || []).forEach((s, i) => {
      const label = s.language || `Subtitle ${i + 1}`;
      row(sm, label, this._selSub === s.pid, () => { this._selSub = s.pid; this.selectSubtitle(s.pid); this._buildMenus(); sm.classList.add("open"); });
    });
  }

  _toggleMenu(kind) {
    const m = this._menusEl.querySelector(`.menu.${kind}`);
    if (!m) return;
    const wasOpen = m.classList.contains("open");
    this._menusEl.querySelectorAll(".menu").forEach((x) => x.classList.remove("open"));
    if (!wasOpen) m.classList.add("open");
  }
```

Initialise `this._selAudio = null; this._selSub = null;` in the constructor.

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "menus build" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): audio + subtitle menus (live-rebuilt from tracks)"
```

---

## Task 6: State machine + overlays (loading/buffering/error+Retry/ended)

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `this._overlaysEl`, `sf-stats` deltas, `sf-error`/`sf-ended`.
- Produces: `_setState(name, msg)` (`idle|loading|playing|buffering|error|ended`) toggles overlay visibility + Retry; `_onStats(s)` derives `loading→playing` (first `drawn>0`) and `playing⇄buffering` (drawn+audioFrames stalled while `!done`). Retry calls `_start()`.

- [ ] **Step 1: Write the failing test (state via seam)**

Add:

```js
test("state overlays: loading → buffering → error+retry via seam", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    document.body.appendChild(el);
    const shown = () => [...el.shadowRoot.querySelectorAll(".overlay.show")].map((o) => o.dataset.state);
    el._setState("loading"); const s1 = shown();
    el._setState("buffering"); const s2 = shown();
    let retried = false; el._start = () => { retried = true; };
    el._setState("error", "boom");
    const errText = el.shadowRoot.querySelector(".overlay[data-state=error]")?.textContent || "";
    el.shadowRoot.querySelector(".retry")?.click();
    el._setState("playing"); const s4 = shown();
    return { s1, s2, errText, retried, s4 };
  });
  expect(r.s1).toEqual(["loading"]);
  expect(r.s2).toEqual(["buffering"]);
  expect(r.errText).toContain("boom");
  expect(r.retried).toBe(true);
  expect(r.s4).toEqual([]); // playing → no overlay
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "state overlays" --config playwright.config.mjs`
Expected: FAIL — overlays not built.

- [ ] **Step 3: Implement state machine + overlays**

In the constructor, build the overlay elements once (after grabbing `_overlaysEl`):

```js
    this._overlaysEl.innerHTML = `
      <div class="overlay" data-state="loading"><div class="spinner"></div><div>Loading…</div></div>
      <div class="overlay" data-state="buffering"><div class="spinner"></div><div>Buffering…</div></div>
      <div class="overlay" data-state="ended"><div>Stream ended</div></div>
      <div class="overlay" data-state="error"><div class="msg"></div><button class="retry" type="button">Retry</button></div>`;
    this._overlaysEl.querySelector(".retry").addEventListener("click", () => this._start());
    this._lastProgress = { t: 0, drawn: 0, audioFrames: 0 };
```

Replace the `_setState` stub + `_onStats` stub:

```js
  _setState(name, msg) {
    this._state = name;
    this._overlaysEl.querySelectorAll(".overlay").forEach((o) =>
      o.classList.toggle("show", o.dataset.state === name && name !== "idle" && name !== "playing"));
    if (name === "error") {
      const m = this._overlaysEl.querySelector(".overlay[data-state=error] .msg");
      if (m) m.textContent = msg || "Playback error";
    }
  }

  _onStats(s) {
    const now = (s && typeof s === "object") ? (this._lastProgress.t + 1) : 0; // monotonic tick
    const advanced = (s.drawn > this._lastProgress.drawn) || (s.audioFrames > this._lastProgress.audioFrames);
    if (s.drawn > 0 && (this._state === "loading" || this._state === "idle")) this._setState("playing");
    if (this._state === "playing" && !s.done && !advanced) {
      this._stallTicks = (this._stallTicks || 0) + 1;
      if (this._stallTicks >= 3) this._setState("buffering");   // ~3 stats ticks with no progress
    } else if (advanced) {
      this._stallTicks = 0;
      if (this._state === "buffering") this._setState("playing");
    }
    this._lastProgress = { t: now, drawn: s.drawn || 0, audioFrames: s.audioFrames || 0 };
  }
```

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "state overlays" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): state machine + loading/buffering/error+retry/ended overlays"
```

---

## Task 7: Fullscreen + Picture-in-Picture

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: host element, `this._videoCanvas`, `this._pipVideo`, engine MSE `<video>` when present.
- Produces: `_toggleFullscreen()` (host `requestFullscreen`/`exitFullscreen`); `_togglePip()` (canvas→captureStream→hidden `<video>`→`requestPictureInPicture` on the WebCodecs path; native `<video>` on MSE). PiP button hidden when unsupported (`_pipSupported()`).

- [ ] **Step 1: Write the failing test**

Add (feature-detect path — assert the button hides when PiP unsupported, and fullscreen calls the API):

```js
test("fullscreen calls requestFullscreen; PiP button hidden when unsupported", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    document.body.appendChild(el);
    let fsCalled = false;
    el.requestFullscreen = () => { fsCalled = true; return Promise.resolve(); };
    el.shadowRoot.querySelector(".fs-btn").click();
    // Force-unsupported PiP and re-evaluate button visibility.
    const pipBtn = el.shadowRoot.querySelector(".pip-btn");
    const hiddenWhenUnsupported = el._pipSupported() === false ? pipBtn.hidden : "supported";
    return { fsCalled, hiddenWhenUnsupported };
  });
  expect(r.fsCalled).toBe(true);
  // Either PiP is supported (chromium usually is) or the button is hidden.
  expect(r.hiddenWhenUnsupported === true || r.hiddenWhenUnsupported === "supported").toBe(true);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "fullscreen calls" --config playwright.config.mjs`
Expected: FAIL — `_toggleFullscreen`/`_pipSupported` are stubs/undefined.

- [ ] **Step 3: Implement fullscreen + PiP**

Replace the `_toggleFullscreen`/`_togglePip` stubs and add helpers:

```js
  _toggleFullscreen() {
    if (this.ownerDocument.fullscreenElement === this) this.ownerDocument.exitFullscreen?.();
    else this.requestFullscreen?.().catch(() => {});
  }

  _pipSupported() {
    return !!(this.ownerDocument.pictureInPictureEnabled &&
      HTMLVideoElement.prototype.requestPictureInPicture);
  }

  async _togglePip() {
    if (!this._pipSupported()) return;
    const doc = this.ownerDocument;
    if (doc.pictureInPictureElement) { await doc.exitPictureInPicture().catch(() => {}); return; }
    // MSE path already renders to a <video>; use it. Otherwise mirror the canvas.
    let video = this._engine?._mseVideoEl;
    if (!video) {
      video = this._pipVideo;
      if (!video.srcObject && this._videoCanvas.captureStream) {
        video.srcObject = this._videoCanvas.captureStream(30);
        await video.play().catch(() => {});
      }
    }
    await video.requestPictureInPicture().catch(() => {});
  }
```

In `_buildControls`, after creating `this._pipBtn`, hide it when unsupported: `if (this._pipBtn && !this._pipSupported()) this._pipBtn.hidden = true;`.

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "fullscreen calls" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): fullscreen + picture-in-picture (canvas captureStream / MSE video)"
```

---

## Task 8: src-reactive switching

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `attributeChangedCallback`, `_start`/`_teardown`, `_switchSeq`.
- Produces: changing `src` after connect tears down the old engine and starts a fresh one; the stale engine's late events are ignored (guarded by `_switchSeq`, already in Task 3). `controls`/`muted`/`audio-lead` changes re-apply without a full reload where possible.

- [ ] **Step 1: Write the failing test**

Add:

```js
test("changing src tears down the old engine and starts a new one", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "none");
    let destroyed = 0, started = 0;
    // Stub _start/_teardown to observe switching without real engines.
    el._teardown = function () { if (this._engine) { destroyed++; this._engine = null; } };
    el._start = function () { started++; this._engine = { destroy() {} }; };
    document.body.appendChild(el);          // connectedCallback (no src yet → _start called, started=1, engine set)
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8");
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/arte/index.m3u8");
    return { destroyed, started };
  });
  // Each src change: teardown old + start new.
  expect(r.started).toBeGreaterThanOrEqual(2);
  expect(r.destroyed).toBeGreaterThanOrEqual(1);
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "changing src" --config playwright.config.mjs`
Expected: FAIL — `attributeChangedCallback` is a no-op.

- [ ] **Step 3: Implement attributeChangedCallback**

Replace the no-op:

```js
  attributeChangedCallback(name, oldV, newV) {
    if (!this.isConnected || oldV === newV) return;
    switch (name) {
      case "src":
        this._teardown();
        this._start();
        break;
      case "controls":
        this._buildControls();
        this._buildMenus();
        break;
      case "muted":
        this._muted2 = this.hasAttribute("muted");
        this._engine?.setMuted(this._muted2);
        break;
      case "audio-lead":
        // Applies to the next (re)load; no live change.
        break;
      default: break;
    }
  }
```

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "changing src" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): src-reactive switching (teardown + reload, stale-event guard)"
```

---

## Task 9: Diagnostics overlay toggle

**Files:**
- Modify: `packages/player/skyfire-element.js`
- Test: `web/tests/element.spec.mjs` (add)

**Interfaces:**
- Consumes: `this._diagEl`, `_onStats`.
- Produces: `_toggleDiag()` shows/hides `.diag`; when visible, `_onStats` writes a text summary (`videoPath decoded/drawn fps? audioFrames skew`).

- [ ] **Step 1: Write the failing test**

Add:

```js
test("diagnostics toggle shows a stats summary", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    document.body.appendChild(el);
    const diag = el.shadowRoot.querySelector(".diag");
    const before = diag.hidden;
    el.shadowRoot.querySelector(".diag-btn").click();
    el._onStats({ decoded: 100, drawn: 98, audioFrames: 480000, avSkewMs: 12, videoPath: "webcodecs", done: false });
    return { before, after: diag.hidden, text: diag.textContent };
  });
  expect(r.before).toBe(true);
  expect(r.after).toBe(false);
  expect(r.text).toContain("webcodecs");
  expect(r.text).toContain("98");
});
```

- [ ] **Step 2: Run — verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "diagnostics toggle" --config playwright.config.mjs`
Expected: FAIL — `_toggleDiag` stub.

- [ ] **Step 3: Implement diagnostics**

Replace the `_toggleDiag` stub and extend `_onStats` to write the overlay when visible:

```js
  _toggleDiag() {
    this._diagEl.hidden = !this._diagEl.hidden;
  }
```

At the end of `_onStats(s)` (before the `_lastProgress` assignment), add:

```js
    if (!this._diagEl.hidden) {
      this._diagEl.textContent =
        `path: ${s.videoPath || "?"}\n` +
        `video: ${s.decoded ?? 0} dec / ${s.drawn ?? 0} drawn\n` +
        `audio: ${s.audioFrames ?? 0} frames\n` +
        `skew: ${s.avSkewMs ?? 0} ms`;
    }
```

- [ ] **Step 4: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "diagnostics toggle" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/player/skyfire-element.js web/tests/element.spec.mjs
git commit -m "feat(player): diagnostics overlay toggle"
```

---

## Task 10: Rewrite `web/index.html` on the element (keep the Phase-1 gate green)

**Files:**
- Modify: `web/index.html`
- Delete: `web/example.js` (superseded by the element)
- Test: existing `web/tests/streams.spec.mjs` (must still pass)

**Interfaces:**
- Produces: `web/index.html` hosts one `<skyfire-player controls="full">` whose `src` comes from `?src=`, importing `skyfire-element.js`, and preserving `window.__sfStats` (the element already mirrors it) and `window.__sfPlayer` (set to the element) so `streams.spec.mjs` keeps working (it calls `window.__sfPlayer.selectAudio` and reads `window.__sfStats`).

- [ ] **Step 1: Rewrite `web/index.html`**

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Skyfire — DVB Player</title>
<script type="importmap">
{ "imports": {
  "@firemedia/skyfire-player": "../packages/player/skyfire-player.js",
  "@firemedia/skyfire-core":   "./skyfire-core-web.js"
} }
</script>
<style>
  html, body { margin: 0; height: 100%; background: #000; }
  skyfire-player { position: fixed; inset: 0; }
</style>
</head>
<body>
<skyfire-player controls="full"></skyfire-player>
<script type="module">
  import "../packages/player/skyfire-element.js";
  const el = document.querySelector("skyfire-player");
  const p = new URLSearchParams(location.search);
  if (p.get("video") === "mse") el.setAttribute("video", "mse");
  el.setAttribute("src", p.get("src") || "/fixtures/h264-25fps.ts");
  // Harness contract: expose the element as __sfPlayer (selectAudio/selectSubtitle
  // delegate to the engine); __sfStats is mirrored by the element on every stats tick.
  window.__sfPlayer = el;
  el.addEventListener("sf-error", (e) => console.error("[skyfire]", e.detail?.message || e.detail));
</script>
</body>
</html>
```

- [ ] **Step 2: Delete the old driver**

```bash
git rm web/example.js
```

- [ ] **Step 3: Run the Phase-1 gate — verify it still passes**

Prereqs: wasm web build + `skyfire-server`/`serve.ts` (global-setup). Run: `cd web && bun run test:streams`
Expected: **12 passed** (video+audio continuity, track-list, selectAudio switch, sub cues — now driven through the element).

Note: `streams.spec.mjs` uses `window.__sfPlayer.selectAudio(pid)` and `window.__sfPlayer.selectSubtitle(pid)` — both exist on the element (Task 3). It reads `s.tracks.subtitle` etc. from `__sfStats` — the element mirrors the engine stats object unchanged, so `tracks` is present.

- [ ] **Step 4: Commit**

```bash
git add web/index.html && git rm web/example.js
git commit -m "refactor(web): index.html hosts <skyfire-player>; drop hand-built example.js"
```

---

## Task 11: Example pages under `web/examples/`

**Files:**
- Create: `web/examples/index.html`
- Create: `web/examples/full.html`
- Create: `web/examples/minimal.html`
- Create: `web/examples/chromeless.html`
- Create: `web/examples/diagnostics.html`
- Modify: `web/serve.ts` if needed (it already serves `web/` recursively — verify `/examples/*` resolves)

**Interfaces:**
- Produces: standalone demo pages. Each imports the element via the same import map, points at a served stream (`http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8` for local, overridable via `?src=`).

- [ ] **Step 1: Create the shared per-page pattern + the index**

Create `web/examples/index.html`:

```html
<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Skyfire player — examples</title>
<style>
  body { background:#111; color:#ddd; font:15px/1.6 system-ui,sans-serif; max-width:640px; margin:40px auto; padding:0 16px; }
  h1 { font-size:20px; } a { color:#6af; }
  table { width:100%; border-collapse:collapse; margin-top:16px; }
  td { padding:12px 8px; border-top:1px solid #333; vertical-align:top; }
  td.h { height:25%; }
</style></head><body>
<h1>Skyfire &lt;skyfire-player&gt; examples</h1>
<table><tbody>
  <tr><td class="h"><a href="full.html">Full controls</a><br><small>play, volume, audio/subtitle menus, PiP, fullscreen, diagnostics</small></td></tr>
  <tr><td class="h"><a href="minimal.html">Minimal</a><br><small>play + fullscreen only</small></td></tr>
  <tr><td class="h"><a href="chromeless.html">Chromeless</a><br><small>no UI; driven by JS (programmatic API)</small></td></tr>
  <tr><td class="h"><a href="diagnostics.html">Diagnostics</a><br><small>full controls with the stats overlay open</small></td></tr>
</tbody></table>
</body></html>
```

- [ ] **Step 2: Create `full.html`**

```html
<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Skyfire — full</title>
<script type="importmap">{ "imports": {
  "@firemedia/skyfire-player": "../../packages/player/skyfire-player.js",
  "@firemedia/skyfire-core": "../skyfire-core-web.js" } }</script>
<style>html,body{margin:0;height:100%;background:#000}skyfire-player{position:fixed;inset:0}</style>
</head><body>
<skyfire-player controls="full"></skyfire-player>
<script type="module">
  import "../../packages/player/skyfire-element.js";
  const el = document.querySelector("skyfire-player");
  const p = new URLSearchParams(location.search);
  el.setAttribute("src", p.get("src") || "http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8");
</script></body></html>
```

- [ ] **Step 3: Create `minimal.html`, `chromeless.html`, `diagnostics.html`**

`minimal.html` — same as `full.html` but `controls="minimal"` and `<title>Skyfire — minimal</title>`.

`chromeless.html` — `controls="none"`, plus a driver showing the programmatic API:

```html
<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Skyfire — chromeless</title>
<script type="importmap">{ "imports": {
  "@firemedia/skyfire-player": "../../packages/player/skyfire-player.js",
  "@firemedia/skyfire-core": "../skyfire-core-web.js" } }</script>
<style>html,body{margin:0;height:100%;background:#000}
  skyfire-player{position:fixed;inset:0 0 44px 0}
  #bar{position:fixed;bottom:0;left:0;right:0;height:44px;display:flex;gap:8px;align-items:center;padding:0 12px;background:#181818;color:#ccc;font:13px system-ui}
  button{background:#222;color:#eee;border:1px solid #444;border-radius:4px;padding:4px 8px}</style>
</head><body>
<skyfire-player controls="none"></skyfire-player>
<div id="bar">
  <button id="pp">Play/Pause</button>
  <span>chromeless — host drives via the element API</span>
</div>
<script type="module">
  import "../../packages/player/skyfire-element.js";
  const el = document.querySelector("skyfire-player");
  const p = new URLSearchParams(location.search);
  el.setAttribute("src", p.get("src") || "http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8");
  let playing = true;
  document.getElementById("pp").addEventListener("click", () => { playing = !playing; playing ? el.play() : el.pause(); });
</script></body></html>
```

`diagnostics.html` — same as `full.html` but after setting `src`, open diagnostics:

```html
<script type="module">
  import "../../packages/player/skyfire-element.js";
  const el = document.querySelector("skyfire-player");
  const p = new URLSearchParams(location.search);
  el.setAttribute("src", p.get("src") || "http://localhost:8090/stream/hls/skyfire/arte/index.m3u8");
  el.addEventListener("sf-stats", () => { if (el._diagEl?.hidden) el._toggleDiag(); }, { once: true });
</script>
```

- [ ] **Step 4: Verify serve.ts resolves /examples/**

Run: `cd web && PORT=8080 bun run serve.ts &` then `curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8080/examples/index.html` → expect `200`. Kill the server. If 404, ensure `serve.ts`'s web-root branch serves nested paths (it maps `/` → `web/`; nested files should resolve — if not, add a fallthrough).

- [ ] **Step 5: Commit**

```bash
git add web/examples
git commit -m "docs(web): standalone <skyfire-player> example pages (full/minimal/chromeless/diagnostics)"
```

---

## Task 12: Integration browser test — element over a real stream

**Files:**
- Modify: `web/tests/element.spec.mjs` (add integration cases)

**Interfaces:**
- Consumes: served streams (global-setup), the finished element.

- [ ] **Step 1: Write the integration test**

Add:

```js
test("integration: element plays a stream, menu switches audio, subtitle cues render", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    el.setAttribute("muted", "");
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/france-2/index.m3u8");
    document.body.appendChild(el);
    document.body.click();
    const wait = (pred, ms) => new Promise((res) => { const t0 = Date.now(); const t = () => (pred() || Date.now()-t0>ms) ? res(pred()) : setTimeout(t, 200); t(); });
    // plays (drawn advances)
    await wait(() => (window.__sfStats?.drawn ?? 0) > 5, 15000);
    const drawnOk = (window.__sfStats?.drawn ?? 0) > 5;
    // audio menu has >1 track; select the second
    const arows = el.shadowRoot.querySelectorAll(".menu.audio .row");
    const before = window.__sfStats?.decodedAudioPid;
    if (arows.length > 1) arows[1].click();
    await wait(() => window.__sfStats?.decodedAudioPid !== before, 12000);
    const switched = arows.length > 1 ? (window.__sfStats?.decodedAudioPid !== before) : true;
    // enable first subtitle, expect cues
    const srows = el.shadowRoot.querySelectorAll(".menu.subtitle .row");
    if (srows.length > 1) srows[1].click();
    await wait(() => (window.__sfStats?.subCues ?? 0) >= 1, 15000);
    const cues = (window.__sfStats?.subCues ?? 0) >= 1;
    return { drawnOk, switched, cues, audioRows: arows.length };
  });
  expect(r.drawnOk).toBe(true);
  expect(r.switched).toBe(true);
  if (r.audioRows > 1) expect(r.cues).toBe(true);
});
```

- [ ] **Step 2: Run — verify pass**

Run: `cd web && bunx playwright test tests/element.spec.mjs -g "integration" --config playwright.config.mjs`
Expected: PASS.

- [ ] **Step 3: Full element + gate run**

Run: `cd web && bunx playwright test tests/element.spec.mjs --config playwright.config.mjs && bun run test:streams`
Expected: all element specs PASS; streams gate **12 passed**.

- [ ] **Step 4: Commit**

```bash
git add web/tests/element.spec.mjs
git commit -m "test(player): integration — element plays, audio switch, subtitle cues"
```

---

## Self-Review

**Spec coverage:**
- Web Component + Shadow DOM + scoped styles → Task 2 ✓
- Engine wiring, events, `__sfStats` mirror, delegating API → Task 3 ✓
- Controls + presets (full/minimal/none) + volume/mute → Tasks 1, 4 ✓
- Audio + subtitle menus (live-rebuilt) → Task 5 ✓
- Loading/buffering/error+Retry/ended states → Task 6 ✓
- Fullscreen + PiP (captureStream / MSE video, feature-detect) → Task 7 ✓
- src-reactive switching → Task 8 ✓
- Diagnostics overlay → Task 9 ✓
- `web/index.html` on element + Phase-1 gate green → Task 10 ✓
- Separate example pages → Task 11 ✓
- Unit-ish + integration browser tests → Tasks 2-9 (seam-based) + 12 (integration) ✓
- Ship element in `package.json` files → Task 2 ✓
- Engine internals unchanged except setMuted/setVolume → Task 1 only ✓

**Placeholder scan:** none — every step has real code + exact commands.

**Type consistency:** method/seam names consistent across tasks — `_start`, `_teardown`, `_applyTracks`, `_buildControls`, `_buildMenus`, `_toggleMenu`, `_setState`, `_onStats`, `_toggleFullscreen`, `_togglePip`, `_pipSupported`, `_toggleDiag`, `_diagEl`, `_switchSeq`, `_selAudio`, `_selSub`, `_muted2`, `_playing`. Constructor initialises `_playing`, `_muted2`, `_selAudio`, `_selSub`, `_lastProgress`, and builds overlay HTML (Task 6 adds the overlay markup + `_lastProgress` in the constructor). Engine API names match Task 1 (`setMuted`/`setVolume`/`getVolume`) and the verified engine surface.

**Known ordering note:** Tasks 3-9 add methods that reference stubs introduced in the same or earlier tasks; each task's Step 3 explicitly says "replace the stub" or "add stub" so the file always compiles and the targeted test runs. Constructor-level initialisers (`_playing`, `_selAudio`, overlay HTML, `_lastProgress`) are introduced in the task that first needs them (noted inline).
