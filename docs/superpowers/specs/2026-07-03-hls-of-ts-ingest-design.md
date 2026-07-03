# HLS-of-TS ingest — design (Build A)

**Date:** 2026-07-03
**Goal:** skyfire client plays an HLS stream whose segments are MPEG-TS, reusing
the existing WASM-bridge pipeline unchanged. Delivery-layer only; container and
demux untouched, so DVB subtitles/audio/PCR keep working.

## Context

`SkyfirePlayer._consumeStream(src)` (`packages/player/skyfire-player.js:830`) does
one `fetch(src)` → `body.getReader()` loop → `this.bridge.feed(value)`. The bridge
is byte-level container-agnostic: it eats TS bytes. So HLS-of-TS ingest =
**replace the single-fetch source with a segment-sequence source that yields TS
byte chunks in order.** Everything downstream of `bridge.feed` (WebCodecs/MSE
video, WASM audio, subtitle compositor, sync clock, pumps) is reused verbatim.

Nothing produces HLS yet — zenith serves chunked TS (`GET /skyfire/<slug>`) and is
off-limits (zenith-claude owns it). skyfire ships the **ingest client**; the HLS
origin is a zenith concern. Dev/test origin = a fixture segmented locally with
ffmpeg's **`segment`** muxer (NOT `-f hls`, which splits DVB-sub into a WebVTT
rendition and errors).

## Architecture

**Byte-source abstraction.** A source exposes `read() → Promise<{done, value}>`
(mirrors `ReadableStreamDefaultReader`) and `cancel()`. Two impls:

- `DirectSource(url, signal)` — wraps today's `fetch().body.getReader()`. No
  behaviour change; extracts current logic.
- `HlsSource(m3u8Url, signal)` — media-playlist parse + sequential segment fetch +
  live reload. Yields each segment's bytes as one or more `value` chunks.

`_consumeStream` reads from a source, not a raw fetch. `init()` selects the source.

**Source detection.** `isHlsUrl(url, opts)`: explicit `opts.hls` (bool) wins;
else `.m3u8` extension (`/\.m3u8(\?|$)/i`) → `HlsSource`, else `DirectSource`.
Content-type sniffing is deferred (it needs a pre-fetch; extension + opt covers
the zenith/dev cases) — out of scope v1.

**Playlist parsing (minimal, `packages/player/hls-source.js`).** Media playlist
tags handled: `#EXTM3U` (required first line), `#EXT-X-MEDIA-SEQUENCE:<n>`
(default 0), `#EXTINF:<dur>,` (per-segment duration, precedes each URI),
`#EXT-X-TARGETDURATION:<s>` (reload interval), segment URI (resolved against the
playlist URL via `new URL(uri, playlistUrl)`), `#EXT-X-ENDLIST` (VOD marker),
`#EXT-X-DISCONTINUITY` (sets a per-segment `discontinuity` flag). Master playlist:
if `#EXT-X-STREAM-INF` present, pick the **first** variant URI and treat it as the
media playlist (no bitrate switching — zenith is single-quality; YAGNI).
Parser output: `{ mediaSequence, targetDuration, endList, segments: [{uri,
duration, seq, discontinuity}] }`.

**Fetch loop (`HlsSource`).** State: `lastSeq = -1`. Each round: fetch+parse
playlist; for each segment with `seq > lastSeq`, fetch it (`fetch(uri, {signal})`),
yield its full body bytes, set `lastSeq = seq`. If `endList`, finish after the
last segment (source returns `{done:true}`). Else (live) wait
`max(targetDuration, 1)` s (halved for responsiveness → `targetDuration/2`, min
0.5s) then re-fetch the playlist; repeat. `read()` returns buffered segment chunks
FIFO; when empty it drives the next fetch. `live` becomes **derived** from
`!endList`, replacing the hardcoded `live=false` at `skyfire-player.js:205` — so
the existing reconnect/retry loop applies to live HLS.

**Discontinuity (v1 scope).** The parser records `discontinuity` per segment and
`HlsSource` exposes it, but v1 does **not** reset the bridge at the boundary — the
ffmpeg test origin slices one continuous stream (no discontinuities), so a reset
path would be unverifiable. Deferred to a follow-up / Build B. Documented
limitation. (Segment-start PAT/PMT repetition is harmless — re-probe yields the
same ChannelMap.)

**Errors.** Segment/playlist fetch failure (non-ok / network) throws out of
`read()`; the existing `_consumeStream` try/catch + reconnect loop handles it
(backoff). `destroy()`'s `_fetchAbortController.abort()` must also abort in-flight
segment/playlist fetches → pass its `signal` into `HlsSource`.

## Dev origin + fixture

`scripts/make-hls-fixture.sh`: `ffmpeg -i fixtures/france2-8s.ts -map 0 -c copy
-f segment -segment_time 2 -segment_format mpegts -segment_list <dir>/index.m3u8
-segment_list_type m3u8 <dir>/seg%d.ts`. Output → `web/fixtures-hls/`
(gitignored). Verified layout: `index.m3u8` (VOD, `MEDIA-SEQUENCE:0`,
`TARGETDURATION:4`, 3× `EXTINF`+`segN.ts`, `ENDLIST`); each `seg*.ts` retains
h264 + 3×eac3 + 2×dvb_subtitle PIDs.

`web/serve.ts`: serve `web/fixtures-hls/` under a route (e.g. `/hls/`) with the
correct MIME (`application/vnd.apple.mpegurl` for `.m3u8`, `video/mp2t` for `.ts`).

## Testing / exit criteria

- **Unit (bun test, `packages/player/hls-source.test.js`):** the media-playlist
  parser against (a) synthetic playlists — media, master, endlist present/absent,
  non-zero media-sequence, discontinuity — and (b) the **real** ffmpeg-produced
  `index.m3u8`. Ungameable: assert exact parsed segment count, URIs, seqs,
  durations, endList bool; mutate a playlist line → parsed output changes.
  Master-playlist input → resolves to the first variant.
- **e2e (authoritative, `web/tests/e2e.spec.mjs`):** Playwright loads the example
  with `?src=/hls/index.m3u8`; asserts `window.__sfStats.decoded > 0` (video frames
  through the real bridge) — same oracle the TS e2e uses. Proves HLS-of-TS plays
  end to end. (Audio=0 and subs clock-gated in headless are known env limits.)
- **CI gate unchanged:** the packages job must still pass (`npm pack --dry-run`,
  `tsc`); add the bun unit test to it.

## Out of scope (v1)

Bitrate switching / variant selection beyond first; byte-range (`EXT-X-BYTERANGE`)
segments; encryption (`EXT-X-KEY`); discontinuity bridge-reset; fMP4/CMAF segments
(that is Build B); any zenith change.
