import { test, expect } from "bun:test";
import { parsePlaylist } from "./hls-source.js";

// ── Test 1: Real playlist (golden) ─────────────────────────────────────────
const GOLDEN_PLAYLIST = `#EXTM3U
#EXT-X-VERSION:3
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-ALLOW-CACHE:YES
#EXT-X-TARGETDURATION:4
#EXTINF:3.477000,
seg0.ts
#EXTINF:1.640000,
seg1.ts
#EXTINF:0.840000,
seg2.ts
#EXT-X-ENDLIST`;

test("real playlist (golden)", () => {
  const result = parsePlaylist(GOLDEN_PLAYLIST, "http://x/index.m3u8");
  expect(result.kind).toBe("media");
  expect(result.segments.length).toBe(3);
  expect(result.endList).toBe(true);
  expect(result.mediaSequence).toBe(0);
  expect(result.targetDuration).toBe(4);
  expect(result.segments[0].uri).toBe("http://x/seg0.ts");
  expect(result.segments[2].uri).toBe("http://x/seg2.ts");
  expect(result.segments[0].seq).toBe(0);
  expect(result.segments[2].seq).toBe(2);
  expect(result.segments[0].duration).toBeGreaterThan(0);
});

// ── Test 2: No ENDLIST → live ───────────────────────────────────────────────
test("no ENDLIST → live", () => {
  const text = `#EXTM3U
#EXT-X-TARGETDURATION:6
#EXTINF:6.0,
seg0.ts
#EXTINF:6.0,
seg1.ts`;
  const result = parsePlaylist(text, "http://live.example.com/stream.m3u8");
  expect(result.kind).toBe("media");
  expect(result.endList).toBe(false);
});

// ── Test 3: MEDIA-SEQUENCE offset ──────────────────────────────────────────
test("MEDIA-SEQUENCE offset", () => {
  const text = `#EXTM3U
#EXT-X-MEDIA-SEQUENCE:5
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg5.ts
#EXTINF:4.0,
seg6.ts
#EXT-X-ENDLIST`;
  const result = parsePlaylist(text, "http://x/playlist.m3u8");
  expect(result.kind).toBe("media");
  expect(result.mediaSequence).toBe(5);
  expect(result.segments[0].seq).toBe(5);
});

// ── Test 4: Discontinuity ──────────────────────────────────────────────────
test("discontinuity flag on second segment", () => {
  const text = `#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg0.ts
#EXT-X-DISCONTINUITY
#EXTINF:4.0,
seg1.ts
#EXT-X-ENDLIST`;
  const result = parsePlaylist(text, "http://x/playlist.m3u8");
  expect(result.kind).toBe("media");
  expect(result.segments[0].discontinuity).toBe(false);
  expect(result.segments[1].discontinuity).toBe(true);
});

// ── Test 5: Master playlist ─────────────────────────────────────────────────
test("master playlist", () => {
  const text = `#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360
low/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720
high/index.m3u8`;
  const result = parsePlaylist(text, "http://cdn.example.com/master.m3u8");
  expect(result.kind).toBe("master");
  expect(result.variants.length).toBe(2);
  expect(result.variants[0].uri).toBe("http://cdn.example.com/low/index.m3u8");
  expect(result.variants[0].bandwidth).toBe(800000);
});

// ── Test 6: Bad input → throws ─────────────────────────────────────────────
test("bad input throws", () => {
  expect(() => parsePlaylist("not a playlist at all", "http://x/")).toThrow();
  expect(() => parsePlaylist("", "http://x/")).toThrow();
  expect(() => parsePlaylist("EXTM3U\n#EXTINF:4,\nseg.ts", "http://x/")).toThrow();
});

// ── Test 7: Bite — distinct inputs, length differs ─────────────────────────
test("bite: distinct inputs yield different segment counts", () => {
  const twoSegs = `#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
segA.ts
#EXTINF:4.0,
segB.ts
#EXT-X-ENDLIST`;

  const threeSegs = `#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
segA.ts
#EXTINF:4.0,
segB.ts
#EXTINF:4.0,
segC.ts
#EXT-X-ENDLIST`;

  const r2 = parsePlaylist(twoSegs, "http://x/pl.m3u8");
  const r3 = parsePlaylist(threeSegs, "http://x/pl.m3u8");

  expect(r2.segments.length).toBe(2);
  expect(r3.segments.length).toBe(3);
  expect(r2.segments.length).not.toBe(r3.segments.length);
});
