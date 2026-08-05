import { test, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

// ── #114: channel-join poster → audio-start transition must not double-close ─
//
// On join the player shows the first decoded frame early as a "poster" while
// audio is still spinning up. Regression: `_drawFrame` closes any frame it
// draws (its finally block), but the poster branch left the first frame at the
// head of `_presentQueue` — so when audio started and the present loop reached
// that same entry it DREW a CLOSED VideoFrame:
//
//   InvalidStateError: Failed to execute 'drawImage' on
//   CanvasRenderingContext2D: The VideoFrame has been closed
//
// The throw aborted the presentation pass, video stalled, and the decoder
// backed up. Every VideoFrame must be closed exactly once and never drawn after
// close.
//
// The m6 fixture reproduces this deterministically: its A/V PTS are aligned so
// that when audio first goes live the poster frame is still inside the
// presentable window, so the present loop re-draws the closed head frame.
//
// Oracle: drive a clip through the join (poster drawn, then audio starts) and
// assert (a) ZERO frame-closed/InvalidStateError console messages and (b) the
// presentable-frame count keeps advancing rather than stalling on that throw.

const WEB = "http://localhost:8080";
const POSTER = "http://localhost:8092";
const SLUG = "m6";
let proc;

test.beforeAll(async () => {
  const root = new URL("../../", import.meta.url).pathname;
  proc = spawn(`${root}target/debug/skyfire-server`,
    ["--fixtures", `${root}fixtures/streams`, "--port", "8092"],
    { stdio: "inherit" });
  for (let i = 0; i < 50; i++) {
    try { if ((await fetch(`${POSTER}/api/streams`)).ok) break; } catch {}
    await sleep(200);
  }
});

test.afterAll(() => proc?.kill());

test("#114 join: poster-drawn first frame is not drawn/closed again after audio starts", async ({ page }) => {
  test.setTimeout(60_000);
  const fatal = [];
  const isClosedSig = (text) =>
    /VideoFrame has been closed|InvalidStateError|drawImage on CanvasRenderingContext2D/.test(text);
  // The regression abort is an uncaught exception thrown from the rAF-back
  // presentation pass, so it surfaces as a page error (uncaught) rather than a
  // console message in the harness. Catch both surfaces so the oracle is solid.
  page.on("pageerror", (e) => { if (isClosedSig(String(e))) fatal.push(String(e)); });
  page.on("console", (m) => {
    const text = m.text();
    if (isClosedSig(text)) fatal.push(text);
  });

  const src = `${POSTER}/stream/hls/skyfire/${SLUG}/index.m3u8`;
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);

  // Do NOT start audio yet — leave it fed-but-not-started so the player enters
  // the poster-hold branch and draws the first frame early (the exact join
  // transition #114 is about).
  await page.waitForFunction(() => {
    const s = window.__sfStats;
    return s && s.drawn >= 1 && (s.audioFrames ?? 0) === 0;
  }, { timeout: 20_000 });

  const drawnBefore = await page.evaluate(() => window.__sfStats.drawn);

  // Now audio begins (gesture / play). The present loop takes over pacing from
  // the poster hold; it must NOT re-touch the already-closed poster frame.
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });

  // Sample presentation progress + audio over a window after audio joins.
  const series = await page.evaluate(() => new Promise((res) => {
    const out = []; const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
      if (s) out.push({ t: Date.now() - t0, drawn: s.drawn, audioFrames: s.audioFrames });
      if (Date.now() - t0 > 6000) return res(out);
      setTimeout(tick, 250);
    };
    tick();
  }));

  // 0. THE oracle: the join transition must not draw an already-closed frame.
  expect(fatal, `no VideoFrame-closed/InvalidStateError on join: ${fatal.join(" | ")}`).toEqual([]);

  // 1. Presentation must advance past the poster frame after audio starts.
  const last = series[series.length - 1] ?? {};
  expect(last.drawn, "drawn advances after audio starts").toBeGreaterThan(drawnBefore);
  expect(last.audioFrames, "audio plays out after start").toBeGreaterThan(100_000);

  // 2. No drawn-count stall longer than 1200ms across the audio join. A stall
  //    that then catches up in bursts is the #114 signature.
  let worst = 0, lastAdvanceT = null, prev = null;
  for (const s of series) {
    if (lastAdvanceT === null) { prev = s.drawn; lastAdvanceT = s.t; continue; }
    if (s.drawn > prev) { worst = Math.max(worst, s.t - lastAdvanceT); lastAdvanceT = s.t; prev = s.drawn; }
    else { worst = Math.max(worst, s.t - lastAdvanceT); }
  }
  if (lastAdvanceT !== null) {
    worst = Math.max(worst, (series[series.length - 1]?.t ?? 0) - lastAdvanceT);
  }
  expect(worst, "no presentation stall across audio start > 1200ms").toBeLessThan(1200);

  // 3. The stream really presented a healthy number of frames in total.
  expect(last.drawn, "frames presented in total").toBeGreaterThan(50);
});
