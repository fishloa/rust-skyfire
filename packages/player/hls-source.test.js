import { test, expect } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parsePlaylist, DirectSource, HlsSource, isHlsUrl, makeSource } from "./hls-source.js";

// ── Fixture helpers ─────────────────────────────────────────────────────────
const HLS = join(import.meta.dir, "../../web/fixtures-hls");
const haveFixture = existsSync(join(HLS, "index.m3u8"));

function diskFetch(u, _opts) {
  const name = new URL(u).pathname.slice(1);
  const data = readFileSync(join(HLS, name));
  return Promise.resolve({
    ok: true,
    status: 200,
    text: async () => data.toString("utf8"),
    arrayBuffer: async () => data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength),
  });
}

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

// ── A3 Tests: DirectSource, HlsSource, isHlsUrl, makeSource ────────────────

// ── A3-1: HlsSource over the real fixture ──────────────────────────────────
test.skipIf(!haveFixture)("A3-1: HlsSource drains real fixture — 3 segments, correct bytes", async () => {
  const src = new HlsSource("http://x/index.m3u8", { fetchImpl: diskFetch });

  const seg0Expected = readFileSync(join(HLS, "seg0.ts"));
  const seg1Expected = readFileSync(join(HLS, "seg1.ts"));
  const seg2Expected = readFileSync(join(HLS, "seg2.ts"));
  const expectedTotal = seg0Expected.length + seg1Expected.length + seg2Expected.length;

  const values = [];
  for (let i = 0; i < 4; i++) {
    const result = await src.read();
    values.push(result);
    if (result.done) break;
  }

  // Exactly 3 non-done reads
  expect(values.length).toBe(4);
  expect(values[0].done).toBe(false);
  expect(values[1].done).toBe(false);
  expect(values[2].done).toBe(false);
  expect(values[3].done).toBe(true);
  expect(values[3].value).toBeUndefined();

  // Each value is a non-empty Uint8Array
  expect(values[0].value).toBeInstanceOf(Uint8Array);
  expect(values[1].value).toBeInstanceOf(Uint8Array);
  expect(values[2].value).toBeInstanceOf(Uint8Array);
  expect(values[0].value.length).toBeGreaterThan(0);
  expect(values[1].value.length).toBeGreaterThan(0);
  expect(values[2].value.length).toBeGreaterThan(0);

  // Total bytes matches sum of real file sizes
  const totalBytes = values[0].value.length + values[1].value.length + values[2].value.length;
  expect(totalBytes).toBe(expectedTotal);

  // isLive is false (ENDLIST present)
  expect(src.isLive).toBe(false);
});

// ── A3-2: Segment order — first value is byte-identical to seg0.ts ─────────
test.skipIf(!haveFixture)("A3-2: first segment is byte-identical to seg0.ts", async () => {
  const src = new HlsSource("http://x/index.m3u8", { fetchImpl: diskFetch });
  const result = await src.read();
  expect(result.done).toBe(false);
  expect(result.value).toBeInstanceOf(Uint8Array);

  const seg0Expected = readFileSync(join(HLS, "seg0.ts"));
  expect(result.value.length).toBe(seg0Expected.length);
  // Byte-identical check: compare as buffers
  expect(Buffer.from(result.value)).toEqual(seg0Expected);
});

// ── A3-3: DirectSource — fake getReader with two chunks then done ───────────
test("A3-3: DirectSource delegates to body.getReader()", async () => {
  const chunk1 = new Uint8Array([1, 2, 3]);
  const chunk2 = new Uint8Array([4, 5, 6]);
  const reads = [
    { done: false, value: chunk1 },
    { done: false, value: chunk2 },
    { done: true, value: undefined },
  ];
  let readIdx = 0;
  const fakeReader = { read: () => Promise.resolve(reads[readIdx++]), cancel: () => {} };
  const fakeBody = { getReader: () => fakeReader };
  const fakeFetch = () =>
    Promise.resolve({ ok: true, status: 200, body: fakeBody });

  const src = new DirectSource("http://x/stream.ts", { fetchImpl: fakeFetch });
  const r1 = await src.read();
  expect(r1).toEqual({ done: false, value: chunk1 });
  const r2 = await src.read();
  expect(r2).toEqual({ done: false, value: chunk2 });
  const r3 = await src.read();
  expect(r3).toEqual({ done: true, value: undefined });
});

test("A3-3: DirectSource throws on !ok response", async () => {
  const fakeFetch = () =>
    Promise.resolve({ ok: false, status: 404, body: null });
  const src = new DirectSource("http://x/stream.ts", { fetchImpl: fakeFetch });
  await expect(src.read()).rejects.toThrow("HTTP 404");
});

// ── A3-4: isHlsUrl ──────────────────────────────────────────────────────────
test("A3-4: isHlsUrl", () => {
  expect(isHlsUrl("/x/index.m3u8")).toBe(true);
  expect(isHlsUrl("/x/stream.ts")).toBe(false);
  expect(isHlsUrl("/x/stream.ts", { hls: true })).toBe(true);
  expect(isHlsUrl("/x/a.m3u8", { hls: false })).toBe(false);
});

// ── A3-5: makeSource ────────────────────────────────────────────────────────
test("A3-5: makeSource returns correct type", () => {
  expect(makeSource("http://x/index.m3u8")).toBeInstanceOf(HlsSource);
  expect(makeSource("http://x/stream.ts")).toBeInstanceOf(DirectSource);
  expect(makeSource("http://x/stream.ts", { hls: true })).toBeInstanceOf(HlsSource);
  expect(makeSource("http://x/index.m3u8", { hls: false })).toBeInstanceOf(DirectSource);
});
