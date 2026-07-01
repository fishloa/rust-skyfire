// e2e.spec.mjs — skyfire client end-to-end (#37). Codifies the checks run by
// hand in #30–#32 so CI / a local runner can re-verify the full bridge pipeline.
//
// Run:
//   1. build wasm:  PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH" \
//                   wasm-pack build crates/skyfire-wasm --target web --release --out-dir web/pkg
//   2. serve:       (cd web && PORT=8080 bun run serve.ts &)
//   3. test:        cd web && bunx playwright test tests/e2e.spec.mjs --browser=chromium
//
// NOTE: requires @playwright/test + a Chromium. Not wired into the cargo CI gate
// (that's native Rust only); this is the browser behavioural gate.
//
// iOS 17 verification is MANUAL (no headless iOS): open the same player URLs on an
// iPhone (iOS 17+ Safari) over the LAN / tv.icomb.place and confirm the same
// __sfStats (video frames decoded, audio samples played). WebCodecs progressive
// H.264 + the WASM AC-3 path must work; interlaced is excluded by ADR 0008.

import { test, expect } from "@playwright/test";

const BASE = process.env.SKYFIRE_BASE || "http://localhost:8080";

// Read window.__sfStats once the stream reports done (or time out).
async function runToDone(page, src, { click = false, capMs = 20000 } = {}) {
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  await page.goto(`${BASE}/index.html?src=${encodeURIComponent(src)}`);
  if (click) await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
  const stats = await page.evaluate((cap) => new Promise((res) => {
    const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
      if ((s && s.done) || Date.now() - t0 > cap) return res(s || { note: "no stats" });
      setTimeout(tick, 300);
    };
    tick();
  }), capMs);
  // favicon 404 is the only tolerated console error.
  const realErrors = errors.filter((e) => !/favicon/.test(e));
  return { stats, realErrors };
}

test("progressive H.264 decodes via WebCodecs (committed fixture)", async ({ page }) => {
  const { stats, realErrors } = await runToDone(page, "/fixtures/h264-25fps.ts");
  expect(stats.decoded, "frames decoded").toBeGreaterThan(50);
  expect(stats.drawn, "frames drawn").toBeGreaterThan(50);
  expect(realErrors, "no decoder errors").toEqual([]);
});

test("real 1080p deinterlaced content: video + audio + sync", async ({ page }) => {
  // gulli-prog.ts is a local (gitignored) deinterlaced sample; skip if absent.
  const head = await page.request.get(`${BASE}/fixtures/gulli-prog.ts`).catch(() => null);
  test.skip(!head || !head.ok(), "gulli-prog.ts not present");

  const { stats, realErrors } = await runToDone(page, "/fixtures/gulli-prog.ts", { click: true });
  expect(stats.w).toBe(1920);
  expect(stats.h).toBe(1080);
  expect(stats.decoded).toBeGreaterThan(100);
  expect(stats.audioSamples, "AC-3 → PCM samples").toBeGreaterThan(100000);
  expect(stats.audioFrames, "audio actually played").toBeGreaterThan(0);
  expect(Math.abs(stats.avSkewMs), "A/V skew bounded").toBeLessThan(120);
  expect(realErrors).toEqual([]);
});

test("PsF oracle PASS on a clean progressive stream", async ({ page }) => {
  // Sanity: a known-good progressive TS must PASS the oracle. A zenith
  // re-signaled PsF sample is dropped in via SKYFIRE_PSF_SRC when available.
  const src = process.env.SKYFIRE_PSF_SRC || "/fixtures/h264-25fps.ts";
  await page.goto(`${BASE}/psf-oracle.html?src=${encodeURIComponent(src)}`);
  const v = await page.evaluate(() => new Promise((res) => {
    const t0 = Date.now();
    const tick = () => {
      if (window.__sfOracle || Date.now() - t0 > 20000) return res(window.__sfOracle || {});
      setTimeout(tick, 300);
    };
    tick();
  }));
  expect(v.verdict, `oracle verdict (frames=${v.frames}, err=${v.error})`).toBe("pass");
});

// ── MSE / fMP4 video fallback ──────────────────────────────────────────────

async function waitForStats(page, timeoutMs = 15000) {
  return page.evaluate((cap) => new Promise((res) => {
    const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
      if (s && s.videoPath && !s.done && s.mseSegments > 0) return res(s);
      if (Date.now() - t0 > cap) return res(s || { note: "timed out" });
      setTimeout(tick, 200);
    };
    tick();
  }), timeoutMs);
}

test("MSE fallback: segments appended and video time advances", async ({ page }) => {
  // Skip if MSE is not supported for this codec in headless Chromium.
  const codecCheck = await page.evaluate(() => {
    try {
      return MediaSource.isTypeSupported('video/mp4; codecs="avc1.640028"');
    } catch (_) { return false; }
  });
  test.skip(!codecCheck, "MSE / video/mp4 not supported in this browser");

  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });

  await page.goto(`${BASE}/index.html?src=/fixtures/h264-25fps.ts&video=mse`);
  const stats = await waitForStats(page);

  expect(stats.videoPath, "video path is mse").toBe("mse");
  expect(stats.mseSegments, "at least one MSE segment appended").toBeGreaterThan(0);

  // Read currentTime, wait ~1 s, read again — must increase.
  const t1 = stats.videoCurrentTime;
  await page.waitForTimeout(1000);
  const t2 = await page.evaluate(() => window.__sfStats?.videoCurrentTime ?? 0);
  expect(t2, "video currentTime increases over time").toBeGreaterThan(t1);

  const realErrors = errors.filter((e) => !/favicon/.test(e));
  expect(realErrors, "no MSE errors").toEqual([]);
});
