import { test, expect } from "bun:test";
import { trackSignature, diffTracks, pickFallbackAudio } from "./tracks.js";

const tl = (audio, subtitles = []) => ({ video_pid: 100, video_codec: "H264", audio, subtitles });
const a = (pid, codec = "EAC3", language = "fra", channels = 2) => ({ pid, codec, language, channels });

// ── The bug this unit exists to fix: the old signature was
// `${audio.length}/${subtitles.length}`, so a same-count swap was invisible.
test("signature changes when a PID is swapped at the same count", () => {
  expect(trackSignature(tl([a(257)]))).not.toBe(trackSignature(tl([a(258)])));
});

test("signature changes when a language is corrected at the same count", () => {
  expect(trackSignature(tl([a(257, "EAC3", "fre")])))
    .not.toBe(trackSignature(tl([a(257, "EAC3", "fra")])));
});

test("signature changes when the channel count changes", () => {
  expect(trackSignature(tl([a(257, "EAC3", "fra", 2)])))
    .not.toBe(trackSignature(tl([a(257, "EAC3", "fra", 6)])));
});

test("signature is stable for an identical track set", () => {
  expect(trackSignature(tl([a(257), a(258)]))).toBe(trackSignature(tl([a(257), a(258)])));
});

test("signature is order-independent", () => {
  expect(trackSignature(tl([a(257), a(258)]))).toBe(trackSignature(tl([a(258), a(257)])));
});

test("diff reports an added track", () => {
  const d = diffTracks(tl([a(257)]), tl([a(257), a(258)]));
  expect(d.added.map((t) => t.pid)).toEqual([258]);
  expect(d.removed).toEqual([]);
  expect(d.changed).toEqual([]);
});

test("diff reports a removed track", () => {
  const d = diffTracks(tl([a(257), a(258)]), tl([a(257)]));
  expect(d.removed.map((t) => t.pid)).toEqual([258]);
  expect(d.added).toEqual([]);
});

test("diff reports a changed track as changed, not add+remove", () => {
  const d = diffTracks(tl([a(257, "EAC3", "fre")]), tl([a(257, "EAC3", "fra")]));
  expect(d.changed.map((t) => t.pid)).toEqual([257]);
  expect(d.added).toEqual([]);
  expect(d.removed).toEqual([]);
});

test("diff covers subtitle tracks too", () => {
  const prev = tl([a(257)], [{ pid: 260, kind: "DvbSubtitles", language: "fra" }]);
  const next = tl([a(257)], []);
  expect(diffTracks(prev, next).removed.map((t) => t.pid)).toEqual([260]);
});

test("diff of an identical set is empty", () => {
  const d = diffTracks(tl([a(257)]), tl([a(257)]));
  expect(d.added).toEqual([]);
  expect(d.removed).toEqual([]);
  expect(d.changed).toEqual([]);
});

test("diff treats a null previous list as all-added", () => {
  const d = diffTracks(null, tl([a(257), a(258)]));
  expect(d.added.map((t) => t.pid)).toEqual([257, 258]);
});

// ── Selected-PID loss must not leave audio permanently silent.
test("fallback picks the lowest surviving audio pid", () => {
  expect(pickFallbackAudio([a(258), a(259)], 257)).toBe(258);
});

test("fallback returns null when nothing survives", () => {
  expect(pickFallbackAudio([], 257)).toBeNull();
});

test("fallback is not needed when the selection survives", () => {
  expect(pickFallbackAudio([a(257), a(258)], 257)).toBe(257);
});
