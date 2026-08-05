import { test, expect } from "@playwright/test";
const WEB = "http://localhost:8080";

// Deterministic unit test of the engine's _present() pacing branches, run in a real
// browser (WASM loads there) but with NO audio device dependency — we drive
// framesPlayed / queue directly and stub _drawFrame. Verifies the fix for
// "video runs ahead without audio, then freezes when late audio catches up".

test("HOLD: audio buffered but not playing → poster once, queue not advanced", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    const { SkyfirePlayer } = await import("@firemedia/skyfire-player");
    const p = new SkyfirePlayer({ getContext: () => ({}), parentElement: null }, { streamUrl: "about:blank" });
    p._draws = []; p._drawFrame = (f) => p._draws.push(f.timestamp);
    p._renderSubs = () => {}; p._schedulePresent = () => {}; p._subQueue = [];
    const frame = (ts) => ({ ts, frame: { timestamp: ts, close() {} } });
    p._audioSamplesFed = 96000; p._firstAudioPtsUs = null;
    p._presentQueue = [frame(0), frame(40000), frame(80000)];
    p._present(); p._present();
    return { draws: p._draws, qlen: p._presentQueue.length };
  });
  expect(r.draws).toEqual([0]);   // poster drawn exactly once across two calls
  // #114: the poster frame is removed from the queue once drawn (it must not be
  // left at the head for the present loop to re-draw after it was closed), and
  // the hold does NOT advance pacing into the remaining frames.
  expect(r.qlen).toBe(2);
});

test("SYNC: audio started → video drains from start gated by audio clock", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    const { SkyfirePlayer } = await import("@firemedia/skyfire-player");
    const p = new SkyfirePlayer({ getContext: () => ({}), parentElement: null }, { streamUrl: "about:blank" });
    p._draws = []; p._drawFrame = (f) => p._draws.push(f.timestamp);
    p._renderSubs = () => {}; p._schedulePresent = () => {}; p._subQueue = [];
    const frame = (ts) => ({ ts, frame: { timestamp: ts, close() {} } });
    p._audioSamplesFed = 96000; p._firstAudioPtsUs = 0; p._audioSampleRate = 48000;
    p._audioFramesPlayed = 1920;    // 0.04s played → audioClock = 40_000us (current)
    p._presentQueue = [frame(0), frame(40000), frame(2000000)];
    p._present();
    return { draws: p._draws, qlen: p._presentQueue.length, qfirst: p._presentQueue[0]?.ts ?? null };
  });
  expect(r.draws).toContain(0);
  expect(r.draws).toContain(40000);
  expect(r.qlen).toBe(1);
  expect(r.qfirst).toBe(2000000);
});

test("VIDEO-ONLY: no audio → wall-clock pacing draws", async ({ page }) => {
  await page.goto(`${WEB}/element-test.html`);
  const r = await page.evaluate(async () => {
    const { SkyfirePlayer } = await import("@firemedia/skyfire-player");
    const p = new SkyfirePlayer({ getContext: () => ({}), parentElement: null }, { streamUrl: "about:blank" });
    p._draws = []; p._drawFrame = (f) => p._draws.push(f.timestamp);
    p._renderSubs = () => {}; p._schedulePresent = () => {}; p._subQueue = [];
    const frame = (ts) => ({ ts, frame: { timestamp: ts, close() {} } });
    p._audioSamplesFed = 0; p._firstAudioPtsUs = null;
    p._presentQueue = [frame(0)];
    p._present();
    return { draws: p._draws };
  });
  expect(r.draws).toContain(0);
});
