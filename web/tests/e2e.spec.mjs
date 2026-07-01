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

test("MSE fallback: transmux fMP4 decodes + plays via MediaSource", async ({ page }) => {
  // Conformant progressive Main-profile fixture (avc1.4d401f) — MSE-supported,
  // unlike the High 4:4:4 h264-25fps.ts (profile MSE rejects) or gulli-15s.ts
  // (source ref-frame non-conformance Chromium's MSE decoder rejects).
  const codecCheck = await page.evaluate(() =>
    MediaSource.isTypeSupported('video/mp4; codecs="avc1.4d401f"'));
  test.skip(!codecCheck, "MSE avc1.4d401f not supported in this browser");

  await page.goto(`${BASE}/index.html?src=/fixtures/h264-mse.ts&video=mse`);
  // Short fixture — wait for the stream to finish processing.
  await page.evaluate(() => new Promise((res) => {
    const t0 = Date.now();
    const tick = () => (window.__sfStats?.done || Date.now() - t0 > 15000)
      ? res() : setTimeout(tick, 200);
    tick();
  }));

  // The bridge chose MSE and produced CMAF media segments.
  const stats = await page.evaluate(() => window.__sfStats);
  expect(stats.videoPath, "video path is mse").toBe("mse");
  expect(stats.mseSegments, "CMAF media segments appended").toBeGreaterThan(0);

  // The <video> actually decoded the transmux fMP4: correct dimensions and no
  // MediaError. (Feed completes ~instantly but playback is realtime, so read
  // dimensions/error at `done`, then confirm the playhead advances over a real
  // wait — proving decoded frames present forward, not merely buffer.)
  const v1 = await page.evaluate(() => {
    const el = document.querySelector("video");
    return { w: el?.videoWidth, h: el?.videoHeight, err: el?.error?.code ?? null,
             t: el?.currentTime ?? 0 };
  });
  expect(v1.err, "no MediaError (fMP4 decodes)").toBeNull();
  expect(v1.w, "decoded video width").toBe(640);
  expect(v1.h, "decoded video height").toBe(360);

  await page.waitForTimeout(1500);
  const v2 = await page.evaluate(() => {
    const el = document.querySelector("video");
    return { err: el?.error?.code ?? null, t: el?.currentTime ?? 0 };
  });
  expect(v2.err, "still no MediaError after playback").toBeNull();
  expect(v2.t, "playhead advances in realtime (frames decode + present)")
    .toBeGreaterThan(v1.t);
});
