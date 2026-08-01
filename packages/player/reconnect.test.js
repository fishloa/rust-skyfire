import { test, expect, mock } from "bun:test";

// ── Pre-mock the WASM-loading modules before importing from skyfire-player ──
// Same reason as stats.test.js: skyfire-core/pkg/skyfire_wasm.js auto-runs wasm
// on import, which bun cannot execute. `reconnectDecision` is a pure function,
// but it lives in skyfire-player.js, so importing it pulls the core facade in.

const MockBridge = class SkyfireBridgeMock {
  constructor() { this._props = {}; }
  get selected_audio_pid() { return this._props.selected_audio_pid ?? null; }
  select_audio() {}
  select_subtitle() {}
  video_codec() { return null; }
  audio_native_channels() { return null; }
  set_playing() {}
};

mock.module("@firemedia/skyfire-core/pkg/skyfire_wasm.js", () => ({
  SkyfireBridge: MockBridge,
  ProbeResult: class {},
  WasmAudioTrack: class {},
  WasmEngine: class {},
  WasmMediaSegment: class {},
  WasmPcmChunk: class {},
  WasmSubtitleCue: class {},
  WasmSubtitleRegion: class {},
  WasmSubtitleTrack: class {},
  WasmTrackList: class {},
  WasmVideoAu: class {},
  WasmVideoUnit: class {},
}));

mock.module("@firemedia/skyfire-core", () => ({
  initSkyfire: () => Promise.resolve(),
  SkyfireBridge: MockBridge,
  PTS_HZ: 90000,
  ticksToMicros: (t) => Math.floor((t * 1_000_000) / 90000),
}));

const { reconnectDecision } = await import("./skyfire-player.js");

// The live reconnect budget must count CONSECUTIVE failures, not failures per
// session. The original loop declared `let attempt = 0` outside the retry loop
// and never reset it, so five transient drops spread across an entire viewing
// session permanently killed playback with "stream failed" — even with hours of
// healthy playback between them. On a live stream that races its packager
// (zenith#1205) the budget was spent within minutes.

const MAX = 5;

test("a failure with no progress since the last one consumes budget", () => {
  const d = reconnectDecision({ attempt: 0, max: MAX, progressed: false });
  expect(d.reconnect).toBe(true);
  expect(d.attempt).toBe(1);
  expect(d.delayMs).toBe(1500);
});

test("backoff grows with consecutive failures and is capped", () => {
  expect(reconnectDecision({ attempt: 1, max: MAX, progressed: false }).delayMs).toBe(3000);
  expect(reconnectDecision({ attempt: 4, max: MAX, progressed: false }).delayMs).toBe(7500);
  // 1500 * 6 would be 9000; the cap holds.
  expect(reconnectDecision({ attempt: 9, max: 20, progressed: false }).delayMs).toBe(8000);
});

test("the budget is exhausted after MAX consecutive failures", () => {
  const d = reconnectDecision({ attempt: MAX, max: MAX, progressed: false });
  expect(d.reconnect).toBe(false);
});

test("progress resets the budget — this is the actual fix", () => {
  // Four failures deep, then the stream played again. The next failure must be
  // treated as the first, not the fifth.
  const d = reconnectDecision({ attempt: 4, max: MAX, progressed: true });
  expect(d.reconnect).toBe(true);
  expect(d.attempt).toBe(1);
  expect(d.delayMs).toBe(1500);
});

test("progress at the exhausted boundary still recovers", () => {
  // Without the reset this returns {reconnect:false} and playback dies.
  const d = reconnectDecision({ attempt: MAX, max: MAX, progressed: true });
  expect(d.reconnect).toBe(true);
  expect(d.attempt).toBe(1);
});

test("non-consecutive failures never exhaust the budget", () => {
  // Simulate a long session: a drop, then healthy playback, repeatedly.
  let attempt = 0;
  for (let i = 0; i < 50; i++) {
    const d = reconnectDecision({ attempt, max: MAX, progressed: true });
    expect(d.reconnect).toBe(true);
    attempt = d.attempt;
  }
  expect(attempt).toBe(1);
});

test("a genuinely dead stream still gives up", () => {
  // No progress between failures — the budget must run out rather than
  // reconnecting forever against a stream that is gone.
  let attempt = 0;
  let reconnects = 0;
  for (;;) {
    const d = reconnectDecision({ attempt, max: MAX, progressed: false });
    if (!d.reconnect) break;
    attempt = d.attempt;
    reconnects++;
    if (reconnects > 20) throw new Error("budget never exhausted");
  }
  expect(reconnects).toBe(MAX);
});
