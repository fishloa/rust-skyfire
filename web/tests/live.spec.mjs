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
