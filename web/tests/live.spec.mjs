import { test, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const WEB = "http://localhost:8080";
const LIVE = "http://localhost:8091";
const SLUG = "france-2"; // must be a committed clip slug
let proc;

test.beforeAll(async () => {
  const root = new URL("../../", import.meta.url).pathname;
  proc = spawn(`${root}target/debug/skyfire-server`,
    ["--fixtures", `${root}fixtures/streams`, "--port", "8091", "--live", SLUG],
    { stdio: "inherit" });
  for (let i = 0; i < 50; i++) {
    try { if ((await fetch(`${LIVE}/api/streams`)).ok) break; } catch {}
    await sleep(200);
  }
});
test.afterAll(() => proc?.kill());

test("live-sim: playlist grows and decode continues", async ({ page }) => {
  // Poll the playlist directly: it must gain segments over time.
  const counts = [];
  for (let i = 0; i < 10; i++) {
    const r = await page.request.get(`${LIVE}/stream/hls/skyfire/${SLUG}/index.m3u8`);
    if (r.ok()) {
      const pl = await r.text();
      counts.push((pl.match(/\.ts/g) || []).length);
    } else {
      counts.push(0);
    }
    await page.waitForTimeout(700);
  }
  // Segments appeared (503→ready) and the count moved.
  expect(Math.max(...counts), "segments eventually served").toBeGreaterThan(0);
  // And the player decodes from the live playlist.
  const src = `${LIVE}/stream/hls/skyfire/${SLUG}/index.m3u8`;
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
  await page.waitForFunction(() => (window.__sfStats?.decoded ?? 0) > 5, { timeout: 20_000 });
  const decoded = await page.evaluate(() => window.__sfStats.decoded);
  expect(decoded, "decoded frames from live playlist").toBeGreaterThan(5);
});

// ── #84: a live stream must actually PLAY, not merely decode ────────────────
//
// `decoded > 5` above is not enough, and that gap is exactly how #84 survived
// unreproduced for so long: the player decoded frames while presenting none and
// playing no audio, so this file stayed green. The cause turned out to be the
// playlist fetch — a live origin answers 503 while a channel spins up, and
// _refreshPlaylist threw, killing the stream before `isLive` was even known so
// the reconnect path could not engage.
//
// This asserts what a viewer experiences: frames presented, audio played out,
// and no long presentation stall, over a window several segments wide.
test("live-sim: growing playlist presents frames and plays audio", async ({ page }) => {
  test.setTimeout(90_000);
  const src = `${LIVE}/stream/hls/skyfire/${SLUG}/index.m3u8`;
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });

  const series = await page.evaluate(() => new Promise((res) => {
    const out = []; const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
      if (s) out.push({ t: Date.now() - t0, drawn: s.drawn, audioFrames: s.audioFrames, done: !!s.done });
      if (Date.now() - t0 > 22_000) return res(out);
      setTimeout(tick, 250);
    };
    tick();
  }));

  const live = series.filter((s) => !s.done);
  const last = live[live.length - 1] ?? {};
  expect(last.drawn ?? 0, "frames actually presented from a live playlist").toBeGreaterThan(50);
  expect(last.audioFrames ?? 0, "audio actually played out").toBeGreaterThan(100_000);

  // Longest gap in presentation after it first moves.
  let worst = 0, lastAdvance = null, prev = null;
  for (const s of live) {
    if (prev === null) { if (s.drawn > 0) { prev = s.drawn; lastAdvance = s.t; } continue; }
    if (s.drawn > prev) { worst = Math.max(worst, s.t - lastAdvance); lastAdvance = s.t; prev = s.drawn; }
  }
  expect(worst, "no live presentation stall > 1500ms").toBeLessThan(1500);
});
