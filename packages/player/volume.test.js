import { test, expect, mock } from "bun:test";

// ── Pre-mock the WASM-loading modules before importing SkyfirePlayer ──

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

const { SkyfirePlayer } = await import("./skyfire-player.js");

function fakePlayer() {
  const canvas = { getContext: () => ({}), parentElement: null };
  return new SkyfirePlayer(canvas, { streamUrl: "about:blank" });
}

test("setVolume/getVolume round-trips and clamps to 0..1", () => {
  const p = fakePlayer();
  p.setVolume(0.5);
  expect(p.getVolume()).toBeCloseTo(0.5);
  p.setVolume(2);   expect(p.getVolume()).toBe(1);
  p.setVolume(-1);  expect(p.getVolume()).toBe(0);
});

test("setMuted toggles muted state without throwing when no audio yet", () => {
  const p = fakePlayer();
  p.setMuted(true);
  expect(p._muted).toBe(true);
  p.setMuted(false);
  expect(p._muted).toBe(false);
});
