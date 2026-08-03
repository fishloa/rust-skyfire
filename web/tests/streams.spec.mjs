import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";

const WEB = "http://localhost:8080";
const SF = "http://localhost:8090";
const registry = JSON.parse(
  readFileSync(new URL("../../fixtures/streams.json", import.meta.url)));

// Load a stream in the player and sample __sfStats every 250ms for `durMs`.
// Returns the series of samples + filtered console errors.
async function sampleSeries(page, src, { durMs = 12_000, subs = null } = {}) {
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  // Preselect the subtitle PID via the URL rather than switching it on later.
  // The bridge DISCARDS subtitle PES for a PID that is not selected, and it
  // consumes a clip far faster than realtime — a 20 s fixture is gone in a few
  // hundred ms. Selecting after the track list resolved therefore raced the end
  // of the stream, which is what made this assertion look load-dependent and
  // flaky (#90). Preselected, all four subtitle streams produce cues in ~1 s.
  const q = subs != null ? `&subs=${subs}` : "";
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}${q}`);
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
  const series = await page.evaluate((dur) => new Promise((res) => {
    const out = []; const t0 = Date.now();
    const tick = () => {
      const s = window.__sfStats;
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
    const { series, real } = await sampleSeries(page, src, {
      subs: stream.expect_sub_cues ? stream.subtitle[0].pid : null,
    });
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
      // Switch as soon as ANY audio has decoded, not after 5000 samples.
      // The bridge decodes as fast as segments arrive, not at playback rate,
      // so a 20 s clip is fully consumed in a few hundred ms — wait too long
      // and the stream ends before the switch can be observed, which looks
      // identical to a broken switch. Flipping at the first decoded chunk
      // leaves the rest of the clip as headroom.
      await page.waitForFunction(() => (window.__sfStats?.audioChunks ?? 0) > 0, { timeout: 15_000 });
      const before = await page.evaluate(() => window.__sfStats.audioChunks);
      const droppedBefore = await page.evaluate(() => window.__sfStats.audioDropped ?? 0);
      await page.evaluate((pid) => window.__sfPlayer.selectAudio(pid), stream.alt_audio_pid);
      // `decodedAudioPid` must report the PID whose audio genuinely DECODED,
      // not the PID that was requested — reporting the request would make this
      // self-fulfilling. The chunk-growth check below is what proves audio
      // actually moved to the new track.
      await page.waitForFunction(
        (pid) => window.__sfStats?.decodedAudioPid === pid,
        stream.alt_audio_pid, { timeout: 15_000 });
      // Audio must keep decoding on the NEW track. No end-of-stream escape
      // hatch: a clip that ends without ever decoding the new track has not
      // switched, it has stopped. Chunks (decoded frames) rather than samples,
      // so the threshold does not depend on channel count.
      await page.waitForFunction(
        (b) => (window.__sfStats?.audioChunks ?? 0) > b + 5,
        before, { timeout: 15_000 });
      const pid = await page.evaluate(() => window.__sfStats.decodedAudioPid);
      expect(pid, "decoded pid follows selection").toBe(stream.alt_audio_pid);
      // Chunks silently discarded by the output-channel guard are the failure
      // mode this issue is about, so the count must not climb across a switch.
      const droppedAfter = await page.evaluate(() => window.__sfStats.audioDropped ?? 0);
      expect(droppedAfter, "no PCM chunks dropped across the switch")
        .toBe(droppedBefore);
    });
  }
}

// ── cross-layout audio switch regression (issue #89) ─────────────────────
//
// orf1 pid 257 is AC-3 5.1 (6 channels), pid 258 is MPEG-audio stereo (2), so
// a real switch CHANGES the decoded channel layout and the audio graph must
// reconfigure rather than discarding the mismatched PCM.
//
// The oracle is `nativeChannels`, which the player must publish from the
// bridge's decoder-derived count (`audio_native_channels()`, i.e. the bridge's
// `last_audio_channels`, written only inside `decode_audio` on a successful
// decode, taken from the decoded frame itself). That number cannot be produced
// by echoing a requested PID — which is precisely why it is the assertion.
// 6 → 2 is reachable only if the new track is genuinely being decoded.
test(`stream orf1: cross-layout audio switch really decodes the new layout`, async ({ page }) => {
  test.setTimeout(30_000);
  const src = `${SF}/stream/hls/skyfire/orf1/index.m3u8`;
  await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
  await page.evaluate(() => { document.body.click(); window.sfStartAudio?.(); });
  // Wait until 5.1 audio is genuinely decoding on pid 257.
  await page.waitForFunction(
    () => window.__sfStats?.nativeChannels === 6, { timeout: 15_000 });
  const beforeSamples = await page.evaluate(() => window.__sfStats.audioSamples);
  await page.evaluate(() => window.__sfPlayer.selectAudio(258));
  // Decoder-derived proof the switch took effect: it is now producing STEREO.
  await page.waitForFunction(
    () => window.__sfStats?.nativeChannels === 2, { timeout: 15_000 });
  // And audio must keep flowing on the new track.
  await page.waitForFunction(
    (b) => (window.__sfStats?.audioSamples ?? 0) > b + 5000,
    beforeSamples, { timeout: 15_000 });
  const pid = await page.evaluate(() => window.__sfStats.decodedAudioPid);
  expect(pid, "decoded pid after switch to MP2 stereo").toBe(258);
});

// ── #91: a finite VOD playlist must play ALL of it, then report done/ended ──
//
// Two independent oracles:
//   1. Audio that actually PLAYS OUT reaches within one second of the total
//      advertised by the playlist (sum of EXTINF). Regression: the player
//      went `done` a full segment early (~4 s short) because it reported the
//      stream as ended while a segment of audio was still queued-but-unplayed.
//   2. Reaching #EXT-X-ENDLIST drains to `stats.done === true` AND emits the
//      `ended` event exactly once — so a host can tell a finished stream from
//      a mid-stream freeze.
async function advertisedExtinfSeconds(slug) {
  const resp = await fetch(`${SF}/stream/hls/skyfire/${slug}/index.m3u8`);
  const text = await resp.text();
  let sum = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("#EXTINF:")) {
      sum += parseFloat(line.slice("#EXTINF:".length).split(",")[0]);
    }
  }
  return sum;
}

for (const stream of registry) {
  test(`stream ${stream.slug}: plays out to the advertised total, then done+ended`, async ({ page }) => {
    test.setTimeout(90_000);
    const advertised = await advertisedExtinfSeconds(stream.slug);
    const src = `${SF}/stream/hls/skyfire/${stream.slug}/index.m3u8`;

    await page.goto(`${WEB}/index.html?src=${encodeURIComponent(src)}`);
    // Bind the ended listener BEFORE the player initialises so a fast clip can
    // never finish before we start counting.
    await page.evaluate(() => {
      window.__doneSeen = false;
      window.__endedCount = 0;
      document.addEventListener("sf-ended", () => { window.__endedCount++; });
      document.body.click(); window.sfStartAudio?.();
    });

    // Wait for `done` to be visible on __sfStats, sampling the audioSec ceiling.
    await page.waitForFunction(
      () => window.__sfStats?.done === true, { timeout: 60_000 });

    const final = await page.evaluate(() => {
      const s = window.__sfStats;
      // `audioSec` advances as long as frames are played out; the final sample
      // after `done` is the played-out total.
      return {
        audioSec: s.audioSec ?? 0,
        done: s.done === true,
        endedCount: window.__endedCount,
      };
    });

    // Oracle 1: within one second of the advertised total (short-by-a-segment
    // was ~4 s short — this is far looser and still catches the regression).
    expect(
      final.audioSec,
      `stream ${stream.slug}: audio played out (${final.audioSec.toFixed(2)}s) `
        + `within 1s of advertised ${advertised.toFixed(2)}s`
    ).toBeGreaterThan(advertised - 1.0);

    // Oracle 2: done set, ended fired exactly once.
    expect(final.done, `stream ${stream.slug}: stats.done true`).toBe(true);
    expect(
      final.endedCount,
      `stream ${stream.slug}: ended emitted exactly once`
    ).toBe(1);
  });
}
