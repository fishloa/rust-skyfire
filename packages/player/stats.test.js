import { test, expect, mock } from "bun:test";

// ── Pre-mock the WASM-loading modules before importing SkyfirePlayer ──
// skyfire-core/pkg/skyfire_wasm.js auto-runs wasm on import, which bun
// cannot execute.  Mock both the wasm package and the core facade so
// SkyfirePlayer's constructor can run sans WASM.

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

test("stats object exposes enriched fields with defaults", () => {
  const canvas = { getContext: () => ({}), parentElement: null };
  const p = new SkyfirePlayer(canvas, { streamUrl: "about:blank" });
  const s = p._stats;
  expect(s.tracks).toBeDefined();
  expect(Array.isArray(s.tracks.audio)).toBe(true);
  expect(Array.isArray(s.tracks.subtitle)).toBe(true);
  expect(s.selectedAudio).toBeNull();
  expect(s.decodedAudioPid).toBeNull();
  expect(typeof s.subCues === "number").toBe(true);
});
