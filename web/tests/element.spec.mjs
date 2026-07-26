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
    await wait(() => (window.__sfStats?.drawn ?? 0) > 5, 15000);
    const drawnOk = (window.__sfStats?.drawn ?? 0) > 5;
    const arows = el.shadowRoot.querySelectorAll(".menu.audio .row");
    const before = window.__sfStats?.decodedAudioPid;
    if (arows.length > 1) arows[1].click();
    await wait(() => window.__sfStats?.decodedAudioPid !== before, 12000);
    const switched = arows.length > 1 ? (window.__sfStats?.decodedAudioPid !== before) : true;
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

// ── Task: rendered audio-menu labels are human-readable, not bare codes ──
// The picker label is the epic's only user-visible deliverable, and the
// unguarded Intl.DisplayNames#of() crash (lang.js item 1) sits on exactly
// this line, yet nothing asserted a rendered label before this test.
test("audio menu renders human-readable labels, not bare ISO 639-2 codes", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const labels = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    el.setAttribute("controls", "full");
    el.setAttribute("muted", "");
    el.setAttribute("src", "http://localhost:8090/stream/hls/skyfire/france-2/index.m3u8");
    document.body.appendChild(el);
    document.body.click();
    const rows = () => [...el.shadowRoot.querySelectorAll(".menu.audio .row")];
    const wait = (pred, ms) => new Promise((res) => {
      const t0 = Date.now();
      const t = () => (pred() || Date.now() - t0 > ms) ? res(pred()) : setTimeout(t, 200);
      t();
    });
    // Audio/subtitle tracks resolve their `language` on their first ES
    // sample — AFTER the initial PAT/PMT/video snapshot (see the comment in
    // skyfire-player.js's _consumeStream) — so waiting only for rows to
    // exist can catch the picker mid-population, before languages are known
    // and rows still read as positional "Track N" fallbacks. Wait for actual
    // decoded frames (as the existing audio-switch integration test above
    // does) so the track list has settled.
    await wait(() => (window.__sfStats?.drawn ?? 0) > 5, 15000);
    await wait(() => rows().length > 0, 5000);
    return rows().map((r) => r.textContent);
  });
  expect(labels.length).toBeGreaterThan(0);
  for (const label of labels) {
    expect(label).toMatch(/^.+ · (AC3|EAC3|MP2)( 5\.1| mono)?( \(\d+\))?$/);
    expect(label).not.toMatch(/^(fra|fre|deu|ita|eng|qaa|mis|oth)\b/);
  }
});

// ── Task 4 (issue #97): programmatic fullscreen API ──
test("fullscreen: exposes a programmatic API and reports state changes", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const api = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    return {
      enter: typeof el.enterFullscreen,
      exit: typeof el.exitFullscreen,
      toggle: typeof el.toggleFullscreen,
      state: typeof el.isFullscreen,
    };
  });
  expect(api).toEqual({ enter: "function", exit: "function", toggle: "function", state: "boolean" });
});

test("fullscreen: enterFullscreen returns a thenable", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const isThenable = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = () => Promise.resolve();
    const p = el.enterFullscreen();
    return typeof p?.then === "function";
  });
  expect(isThenable).toBe(true);
});

test("fullscreen: rejection from requestFullscreen reaches the caller, not swallowed", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  // Stub the underlying browser call directly so the outcome is deterministic
  // rather than at the mercy of headless gesture policy. A regression that
  // wraps the internal await in a try/catch-and-return would turn this
  // rejection into a silent resolve — this must fail if that happens.
  const outcome = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = () => Promise.reject(new Error("denied"));
    try {
      await el.enterFullscreen();
      return { rejected: false };
    } catch (e) {
      return { rejected: true, message: e?.message };
    }
  });
  expect(outcome.rejected).toBe(true);
  expect(outcome.message).toBe("denied");
});

test("fullscreen: resolves when requestFullscreen resolves", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const outcome = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = () => Promise.resolve();
    try {
      await el.enterFullscreen();
      return { resolved: true };
    } catch (e) {
      return { resolved: false, message: e?.message };
    }
  });
  expect(outcome.resolved).toBe(true);
});

test("fullscreen: falls back to pseudo-fullscreen when the API is absent", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  // iPhone Safari has no Element.requestFullscreen and skyfire paints to a
  // canvas, so there is no video element to promote. Simulate that.
  const res = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = undefined;
    document.body.style.overflow = "scroll"; // non-empty prior value the restore must not clobber
    const seen = [];
    el.addEventListener("sf-fullscreenchange", (e) => seen.push(e.detail));
    await el.enterFullscreen();
    const cls = el.classList.contains("sf-pseudo-fullscreen");
    const state = el.isFullscreen;
    const overflowDuring = document.body.style.overflow;
    await el.exitFullscreen();
    const clsAfterExit = el.classList.contains("sf-pseudo-fullscreen");
    const overflowAfter = document.body.style.overflow;
    return { seen, cls, state, after: el.isFullscreen, overflowDuring, clsAfterExit, overflowAfter };
  });
  expect(res.cls).toBe(true);
  expect(res.state).toBe(true);
  expect(res.after).toBe(false);
  expect(res.overflowDuring).toBe("hidden");
  expect(res.clsAfterExit).toBe(false);
  expect(res.overflowAfter).toBe("scroll");
  expect(res.seen[0]).toEqual({ fullscreen: true, mode: "pseudo" });
  expect(res.seen[1]).toEqual({ fullscreen: false, mode: "pseudo" });
});

test("fullscreen: disconnecting the element while pseudo-fullscreen is active releases the scroll lock", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  // If disconnectedCallback doesn't reverse an in-progress pseudo-fullscreen,
  // document.body.style.overflow stays "hidden" forever on the host page.
  const res = await page.evaluate(async () => {
    await customElements.whenDefined("skyfire-player");
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = undefined;
    document.body.style.overflow = "scroll";
    await el.enterFullscreen();
    const overflowDuring = document.body.style.overflow;
    el.remove(); // disconnected without a prior exitFullscreen()
    const overflowAfter = document.body.style.overflow;
    return { overflowDuring, overflowAfter };
  });
  expect(res.overflowDuring).toBe("hidden");
  expect(res.overflowAfter).toBe("scroll");
});
