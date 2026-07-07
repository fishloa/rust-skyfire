import { test, expect } from "@playwright/test";
const WEB = "http://localhost:8080";

// ── Task 2: registration + attribute reflection ──
test("registers <skyfire-player> and builds a shadow root with a video canvas", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const ok = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "none");
    document.body.appendChild(el);
    const sr = el.shadowRoot;
    return !!sr && !!sr.querySelector("canvas.video") && !!sr.querySelector(".subs canvas");
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

// ── Task 3: engine wiring ──
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

// ── Task 4: control bar + presets ──
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
  expect(r.minVol).toBe(false);
  expect(r.noneEmpty).toBe(true);
});

// ── Task 5: audio + subtitle menus ──
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
    const subRows = sr.querySelectorAll(".menu.subtitle .row").length;
    sr.querySelectorAll(".menu.audio .row")[1].click();
    sr.querySelectorAll(".menu.subtitle .row")[1].click();
    return { audioRows, subRows, calls };
  });
  expect(r.audioRows).toBe(2);
  expect(r.subRows).toBe(2);
  expect(r.calls).toContainEqual(["a", 258]);
  expect(r.calls).toContainEqual(["s", 260]);
});

// ── Task 6: state machine + overlays ──
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
  expect(r.s4).toEqual([]);
});

// ── Task 7: fullscreen + PiP ──
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
    const pipBtn = el.shadowRoot.querySelector(".pip-btn");
    const hiddenWhenUnsupported = el._pipSupported() === false ? pipBtn.hidden : "supported";
    return { fsCalled, hiddenWhenUnsupported };
  });
  expect(r.fsCalled).toBe(true);
  expect(r.hiddenWhenUnsupported === true || r.hiddenWhenUnsupported === "supported").toBe(true);
});

// ── Task 8: src-reactive switching ──
test("changing src tears down the old engine and starts a new one", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "none");
    let destroyed = 0, started = 0;
    el._teardown = function () { if (this._engine) { destroyed++; this._engine = null; } };
    el._start = function () { started++; this._engine = { destroy() {} }; };
    document.body.appendChild(el);
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/rai-1/index.m3u8");
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/arte/index.m3u8");
    return { destroyed, started };
  });
  expect(r.started).toBeGreaterThanOrEqual(2);
  expect(r.destroyed).toBeGreaterThanOrEqual(1);
});

// ── Task 9: diagnostics toggle ──
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
