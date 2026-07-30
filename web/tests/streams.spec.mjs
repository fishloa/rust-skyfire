import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";

const WEB = "http://localhost:8080";
const SF = "http://localhost:8090";
const registry = JSON.parse(
  readFileSync(new URL("../../fixtures/streams.json", import.meta.url)));

// Load a stream in the player and sample __sfStats every 250ms for `durMs`.
// Returns the series of samples + filtered console errors.
async function sampleSeries(page, src, { durMs = 12_000 } = {}) {
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
  const series = await page.evaluate((dur) => new Promise((res) => {
    const out = []; const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
      // Enable the first subtitle track as soon as it resolves (subs default off,
      // like any player) so DVB-sub cue decoding can be verified.
      if (s && s.tracks?.subtitle?.length && !window.__sfSubSel) {
        window.__sfSubSel = true;
        try { window.__sfPlayer?.selectSubtitle(s.tracks.subtitle[0].pid); } catch (_) {}
      }
      if (s) out.push({ t: Date.now() - t0, decoded: s.decoded, drawn: s.drawn,
                        audioSamples: s.audioSamples, audioFrames: s.audioFrames,
                        avSkewMs: s.avSkewMs,
                        w: s.w, h: s.h, subCues: s.subCues,
                        selectedAudio: s.selectedAudio, decodedAudioPid: s.decodedAudioPid,
                        tracks: s.tracks, done: !!s.done });
      if (Date.now() - t0 > dur) return res(out);
      setTimeout(tick, 250);
    };
    tick();
  }), durMs);
  const real = errors.filter((e) =>
    !/favicon/.test(e) &&
    !/AudioContext encountered an error from the audio device/.test(e));
  return { series, real };
}

// Longest run (ms) where a counter did not advance, measured only AFTER it first
// moves — startup latency (decode + first-buffer) is not a stutter. Captures the
// tail gap (frozen until the end) so a mid-stream freeze is caught.
function maxStallMs(series, key) {
  let worst = 0, lastAdvanceT = null, prev = null;
  for (const s of series) {
    if (prev === null) { if (s[key] > 0) { prev = s[key]; lastAdvanceT = s.t; } continue; }
    if (s[key] > prev) { worst = Math.max(worst, s.t - lastAdvanceT); lastAdvanceT = s.t; prev = s[key]; }
  }
  if (lastAdvanceT === null) return Infinity; // never advanced at all → total stall
  const endT = series[series.length - 1]?.t ?? 0;
  return Math.max(worst, endT - lastAdvanceT);
}

for (const stream of registry) {
  test(`stream ${stream.slug}: continuous video + audio`, async ({ page }) => {
    const src = `${SF}/stream/hls/skyfire/${stream.slug}/index.m3u8`;
    const { series, real } = await sampleSeries(page, src);
    expect(series.length, "must collect stats samples").toBeGreaterThan(3);
    const last = series[series.length - 1];

    // ── Video: dims + decoding completeness + smooth PRESENTATION. ──
    // decoded/audioSamples are FED counters — intentionally bursty under the
    // player's feed backpressure, so continuity is asserted on what the user
    // actually sees/hears: drawn (frames presented) and audioFrames (frames
    // played by the worklet clock).
    if (stream.video) {
      expect(last.w, "video width").toBe(stream.video.width);
      expect(last.h, "video height").toBe(stream.video.height);
    }
    expect(last.decoded, "frames decoded")
      .toBeGreaterThan(stream.min_video_frames);
    expect(maxStallMs(series, "drawn"), "no video PRESENTATION stall > 800ms")
      .toBeLessThan(800);
    expect(last.drawn, "frames actually presented (realtime progress)")
      .toBeGreaterThan(stream.min_video_frames * 0.4);

    // ── Audio: PCM decoded + continuous PLAYBACK (worklet clock advances). ──
    expect(last.audioSamples, "audio PCM decoded").toBeGreaterThan(10_000);
    expect(maxStallMs(series, "audioFrames"), "no audio PLAYBACK dropout > 800ms")
      .toBeLessThan(800);

    // ── A/V skew bounded whenever it is reported. ──
    for (const s of series) {
      if (s.audioSamples > 0 && s.decoded > 0) {
        expect(Math.abs(s.avSkewMs), `A/V skew bounded @${s.t}ms`).toBeLessThan(200);
      }
    }

    // ── Track list matches the registry. ──
    expect(last.tracks.audio.length, "audio track count")
      .toBe(stream.audio.length);
    expect(last.tracks.subtitle.length, "subtitle track count")
      .toBe(stream.subtitle.length);

    // ── Subtitles: cues rendered where the registry expects them. ──
    if (stream.expect_sub_cues) {
      const anyCues = series.some((s) => (s.subCues ?? 0) >= 1);
      expect(anyCues, "at least one DVB-sub cue rendered").toBe(true);
    }

    expect(real, `no console errors: ${real.join(" | ")}`).toEqual([]);
  });

  if (stream.alt_audio_pid != null) {
    test(`stream ${stream.slug}: selectAudio switches decoded pid`, async ({ page }) => {
      const src = `${SF}/stream/hls/skyfire/${stream.slug}/index.m3u8`;
      await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
      await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
      // Wait for initial audio, then switch.
      await page.waitForFunction(() => (window.__sfStats?.audioSamples ?? 0) > 5000, { timeout: 15_000 });
      const before = await page.evaluate(() => window.__sfStats.audioSamples);
      await page.evaluate((pid) => window.__sfPlayer.selectAudio(pid), stream.alt_audio_pid);
      // Wait a short time for the decodedAudioPid stat to update (set
      // synchronously by selectAudio itself on the stats event). Use a
      // generous 5s timeout — the stats event fires on every _status and
      // _drawFrame call, so the value is observed within one animation
      // frame or ~100ms in practice.
      await page.waitForFunction(
        (pid) => window.__sfStats?.decodedAudioPid === pid,
        stream.alt_audio_pid, { timeout: 15_000 });
      // Audio must keep flowing after the switch, unless the stream has
      // already ended (VOD clip fully consumed). Check both conditions.
      await page.waitForFunction((b) => {
        const s = window.__sfStats;
        return (s?.audioSamples ?? 0) > b + 5000 || s?.done === true;
      }, before, { timeout: 15_000 });
      const pid = await page.evaluate(() => window.__sfStats.decodedAudioPid);
      expect(pid, "decoded pid follows selection").toBe(stream.alt_audio_pid);
    });
  }
}

// ── cross-layout audio switch regression (issue #89) ─────────────────────
//
// orf1 has pid 257 (AC-3, likely 5.1) and pid 258 (MP2, stereo). Switching
// from 257→258 must reconfigure the audio graph for a different channel count.
// Audio must keep flowing AND the reported native/source channels must change.
test(`stream orf1: cross-layout audio switch keeps flowing and reports new channels`, async ({ page }) => {
  test.setTimeout(30_000);
  const src = `${SF}/stream/hls/skyfire/orf1/index.m3u8`;
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
  // Wait for initial audio (pid 257, AC-3 5.1).
  await page.waitForFunction(
    () => (window.__sfStats?.decodedAudioPid === 257), { timeout: 15_000 });
  // Sample before switch.
  const beforeSamples = await page.evaluate(() => window.__sfStats.audioSamples);
  // Switch to pid 258 (MP2, stereo).
  await page.evaluate(() => window.__sfPlayer.selectAudio(258));
  // Wait for decodedAudioPid to reflect the new PID.
  await page.waitForFunction(
    () => window.__sfStats?.decodedAudioPid === 258, { timeout: 15_000 });
  // Audio must keep flowing after the switch.
  await page.waitForFunction(
    (b) => window.__sfStats.audioSamples > b + 5000, beforeSamples, { timeout: 15_000 });
  // The decoded PID is now 258 (MP2 stereo).
  const pid = await page.evaluate(() => window.__sfStats.decodedAudioPid);
  expect(pid, "decoded pid after switch to MP2 stereo").toBe(258);
});
