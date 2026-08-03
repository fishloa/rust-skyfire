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
    url: u,
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
test.skipIf(!haveFixture)("A3-1: HlsSource drains the real fixture — every segment, in order, correct total bytes", async () => {
  const src = new HlsSource("http://x/index.m3u8", { fetchImpl: diskFetch });

  // Derive the expected layout from the actual fixture on disk — fixture-agnostic
  // (make-hls-fixture.sh may segment any source into any number of segments),
  // still ungameable: asserts against the real files' count + byte sizes.
  const playlist = readFileSync(join(HLS, "index.m3u8"), "utf8");
  const segNames = playlist
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"));
  expect(segNames.length).toBeGreaterThan(1); // multi-segment → exercises the fetch loop
  const expectedTotal = segNames.reduce((n, s) => n + readFileSync(join(HLS, s)).length, 0);

  const values = [];
  for (let i = 0; i < segNames.length + 1; i++) {
    const result = await src.read();
    values.push(result);
    if (result.done) break;
  }

  // One non-done read per segment, then a final done.
  expect(values.length).toBe(segNames.length + 1);
  for (let i = 0; i < segNames.length; i++) {
    expect(values[i].done).toBe(false);
    expect(values[i].value).toBeInstanceOf(Uint8Array);
    expect(values[i].value.length).toBeGreaterThan(0);
  }
  expect(values[segNames.length].done).toBe(true);
  expect(values[segNames.length].value).toBeUndefined();

  // Total bytes matches the sum of the real segment file sizes.
  const totalBytes = values
    .slice(0, segNames.length)
    .reduce((n, v) => n + v.value.length, 0);
  expect(totalBytes).toBe(expectedTotal);

  // isLive is false (ENDLIST present).
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

// ── Live segment availability: a 404 must not kill the stream (zenith#1205) ──
//
// A live packager can publish a segment URI a moment before the bytes are
// readable. Observed on tv.icomb.place 2026-08-01: three of six advertised
// segments returned `404 segment 'segNN.ts' not found` while an earlier one
// served 3.3 MB. RFC 8216 §6.2.2 says a server must not do that, but a live
// client has to survive it — a not-yet-written segment is a wait, not a failure.

const LIVE_PLAYLIST = `#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:50
#EXTINF:4.000,
seg50.ts
#EXTINF:4.000,
seg51.ts`;

/** Fetch stub: playlist always OK; per-segment status driven by `plan`. */
function planFetch(plan, log = []) {
  return (u) => {
    const name = new URL(u, "http://x/").pathname.split("/").pop();
    if (name.endsWith(".m3u8")) {
      return Promise.resolve({
        ok: true, status: 200, url: u,
        text: async () => LIVE_PLAYLIST,
        arrayBuffer: async () => new ArrayBuffer(0),
      });
    }
    log.push(name);
    const status = typeof plan[name] === "function" ? plan[name]() : (plan[name] ?? 200);
    const ok = status >= 200 && status < 300;
    return Promise.resolve({
      ok, status, url: u,
      text: async () => (ok ? "" : `segment '${name}' not found`),
      arrayBuffer: async () => new TextEncoder().encode(`bytes:${name}`).buffer,
    });
  };
}

test("live: a segment that 404s then appears is retried, not fatal", async () => {
  let calls = 0;
  const log = [];
  // seg50 is missing on the first attempt and available on the next.
  const src = new HlsSource("http://x/index.m3u8", {
    fetchImpl: planFetch({ "seg50.ts": () => (++calls === 1 ? 404 : 200) }, log),
    segmentRetryDelayMs: 1,
  });
  const r = await src.read();
  expect(r.done).toBe(false);
  expect(new TextDecoder().decode(r.value)).toBe("bytes:seg50.ts");
  // It re-requested the same segment rather than abandoning the stream.
  expect(log.filter((n) => n === "seg50.ts").length).toBeGreaterThan(1);
});

test("live: a persistently missing segment is skipped, the stream continues", async () => {
  const log = [];
  const src = new HlsSource("http://x/index.m3u8", {
    fetchImpl: planFetch({ "seg50.ts": 404 }, log),
    segmentRetryDelayMs: 1, segmentRetryBudgetMs: 5,
  });
  const r = await src.read();
  // Must not throw, and must move on to the next advertised segment.
  expect(r.done).toBe(false);
  expect(new TextDecoder().decode(r.value)).toBe("bytes:seg51.ts");
});

test("live: a non-availability error still surfaces", async () => {
  // 403 is not "not written yet" — retrying cannot help, so it must not be
  // swallowed into an indefinite retry loop.
  const src = new HlsSource("http://x/index.m3u8", {
    fetchImpl: planFetch({ "seg50.ts": 403 }),
    segmentRetryDelayMs: 1,
  });
  await expect(src.read()).rejects.toThrow(/403/);
});

// ── Playlist restart: MEDIA-SEQUENCE going backwards must resync ────────────
//
// Observed on tv.icomb.place 2026-08-01: MEDIA-SEQUENCE ran 195, then reset to
// 0 and climbed again — the origin restarted the session. The old refresh kept
// a monotonic `_lastSeq`, so after such a reset every incoming segment failed
// `seq > _lastSeq` forever: nothing queued, nothing played, no error raised.
// A silent permanent stall is the worst possible failure mode, so this is
// asserted explicitly.

function seqPlaylist(mediaSeq, names) {
  return [
    "#EXTM3U",
    "#EXT-X-VERSION:3",
    "#EXT-X-TARGETDURATION:4",
    `#EXT-X-MEDIA-SEQUENCE:${mediaSeq}`,
    ...names.flatMap((n) => ["#EXTINF:4.000,", n]),
  ].join("\n");
}

test("live: a MEDIA-SEQUENCE reset resyncs instead of wedging forever", async () => {
  let phase = 0;
  const fetchImpl = (u) => {
    const name = new URL(u, "http://x/").pathname.split("/").pop();
    if (name.endsWith(".m3u8")) {
      // First the session is at 195; then it restarts at 0.
      const body = phase === 0 ? seqPlaylist(195, ["seg195.ts"]) : seqPlaylist(0, ["seg0.ts"]);
      return Promise.resolve({
        ok: true, status: 200, url: u,
        text: async () => body,
        arrayBuffer: async () => new ArrayBuffer(0),
      });
    }
    return Promise.resolve({
      ok: true, status: 200, url: u,
      text: async () => "",
      arrayBuffer: async () => new TextEncoder().encode(`bytes:${name}`).buffer,
    });
  };

  const src = new HlsSource("http://x/index.m3u8", { fetchImpl, segmentRetryDelayMs: 1 });
  const first = await src.read();
  expect(new TextDecoder().decode(first.value)).toBe("bytes:seg195.ts");

  // The origin restarts: sequence numbers now go backwards.
  phase = 1;
  const after = await src.read();
  expect(after.done).toBe(false);
  expect(new TextDecoder().decode(after.value)).toBe("bytes:seg0.ts");
  expect(src.playlistResets).toBe(1);
});
