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
      if (s) out.push({ t: Date.now() - t0, decoded: s.decoded, drawn: s.drawn,
                        audioSamples: s.audioSamples, avSkewMs: s.avSkewMs,
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

// The longest run of consecutive samples where a counter did not advance.
function maxStallMs(series, key) {
  let worst = 0, lastAdvanceT = series[0]?.t ?? 0, prev = series[0]?.[key] ?? 0;
  for (const s of series) {
    if (s[key] > prev) { worst = Math.max(worst, s.t - lastAdvanceT); lastAdvanceT = s.t; prev = s[key]; }
  }
  // Also account for the tail (no advance until the end).
  const endT = series[series.length - 1]?.t ?? 0;
  return Math.max(worst, endT - lastAdvanceT);
}

for (const stream of registry) {
  test(`stream ${stream.slug}: continuous video + audio`, async ({ page }) => {
    const src = `${SF}/stream/hls/skyfire/${stream.slug}/index.m3u8`;
    const { series, real } = await sampleSeries(page, src);
    expect(series.length, "must collect stats samples").toBeGreaterThan(3);
    const last = series[series.length - 1];

    // ── Video: dimensions + continuous decode, no long stall. ──
    if (stream.video) {
      expect(last.w, "video width").toBe(stream.video.width);
      expect(last.h, "video height").toBe(stream.video.height);
    }
    expect(last.decoded, "final decoded frames")
      .toBeGreaterThan(stream.min_video_frames);
    expect(maxStallMs(series, "decoded"), "no video stall > 800ms")
      .toBeLessThan(800);

    // ── Audio: continuous PCM, no long silence. ──
    expect(last.audioSamples, "audio PCM samples").toBeGreaterThan(10_000);
    expect(maxStallMs(series, "audioSamples"), "no audio dropout > 800ms")
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
      await page.waitForFunction(
        (pid) => window.__sfStats?.decodedAudioPid === pid,
        stream.alt_audio_pid, { timeout: 15_000 });
      // Audio must keep flowing after the switch.
      await page.waitForFunction((b) => window.__sfStats.audioSamples > b + 5000, before, { timeout: 15_000 });
      const pid = await page.evaluate(() => window.__sfStats.decodedAudioPid);
      expect(pid, "decoded pid follows selection").toBe(stream.alt_audio_pid);
    });
  }
}
