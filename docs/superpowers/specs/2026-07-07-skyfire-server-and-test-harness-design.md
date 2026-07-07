# Skyfire server + automated stream test harness — design (Phase 1)

**Date:** 2026-07-07
**Status:** Draft (awaiting review)

## Why

The current player **does not work reliably**: video stutters and stops after a
few seconds, and audio almost never plays. We have no automated way to observe
these failures, so fixes are guesswork. This phase builds the **diagnostic
loop**: a standalone HLS server plus a browser harness that serves every stream
to the WASM client and asserts — per stream — that video decodes *continuously*,
audio flows *continuously*, every audio track is selectable, and DVB subtitles
list and render. The harness is expected to go **RED** on the current player,
reproducing the real bugs across all streams. Fixing the player to green (and
evolving it into a sophisticated player) is **Phase 2**, built against this gate.

This is system-level TDD: build the failing gate first, then fix.

## Scope

**In scope (Phase 1):**
- `skyfire-hls` crate — source-agnostic HLS-of-TS orchestration wrapping
  transmux's `StreamingTsHlsSegmenter`.
- `skyfire-server` bin — thin axum server that serves fixtures over the same
  routes zenith uses.
- `skyfire-cli` probe extension — subtitle-activity + track-metadata probe that
  emits the ground-truth test registry.
- `scripts/preencode-fixtures.sh` — offline ffmpeg pre-encode of the 43 captures
  to clean progressive TS (deinterlace + libx264, audio/subs copied).
- Committed curated fixture subset (~6–8 streams) + `fixtures/streams.json`
  registry; full progressive set kept local (gitignored).
- Minimal player `__sfStats` enrichment (no UI) so the harness can assert the
  4 dimensions.
- `web/tests/streams.spec.mjs` — per-stream Playwright gate + one live-sim test.
- CI e2e job (the first browser gate in CI).

**Out of scope (Phase 2, separate spec):**
- Fixing the stutter/stop + no-audio bugs.
- The sophisticated player UI (controls, track menus, subtitle styling,
  diagnostics overlay).
- NVENC / in-server transcode (stays zenith's job; see "Transcode" below).

## Background (verified 2026-07-07)

- **The HLS chop already lives in transmux**, not in bespoke zenith code.
  zenith's `backend/crates/zenith-stream/src/skyfire_hls.rs` wraps
  `transmux::ts_hls::StreamingTsHlsSegmenter` and trusts `Sample.is_sync`
  (transmux 0.14+ auto-detects IDR / recovery-point SEI / SPS-in-AU as a RAP).
  skyfire is already on transmux 0.15.1 — the same version — so no bump needed.
- What zenith owns *on top* of transmux (the "segmenting logic to move") is
  orchestration: RAP-trim, build-segmenter-on-first-video-RAP, sample-buffer
  replay, ring buffer, playlist gen, discontinuity, plus DVB-scatterer ingest
  (zenith-only) and NVENC deinterlace (zenith-only, GPU/Linux).
- rust-skyfire has **no Rust HTTP server** and **no HLS-TS segmenting** today;
  only a CMAF/fMP4 `Segmenter` path in skyfire-wasm (MSE fallback) and a static
  Bun `web/serve.ts`.
- The existing Playwright e2e (`web/tests/e2e.spec.mjs`) asserts burst
  thresholds (`decoded > 50`) — a stall "after a few seconds" passes it. Headless
  Chromium software-decodes and chokes on raw broadcast bitstreams
  (`PIPELINE_ERROR_DECODE` on france2/gulli) — a codec-environment artifact,
  **not** the player bug. Pre-encoding to clean progressive removes that
  confounder.

### transmux `StreamingTsHlsSegmenter` API (0.15.1, `src/ts_hls.rs`)

```rust
pub fn new(tracks: Vec<TrackSpec>, target_secs: u32, window: usize) -> Result<Self>;
pub fn push(&mut self, track_id: u32, sample: Sample) -> Result<Option<TsSegment>>;
pub fn finish(&mut self) -> Result<Option<TsSegment>>;
pub fn mark_discontinuity(&mut self);
pub fn add_track(&mut self, spec: TrackSpec) -> Result<()>;
pub fn playlist(&self) -> String;

pub struct TsSegment { pub bytes: Vec<u8>, pub duration: f64,
                       pub discontinuous: bool, pub uri: String /* "{prefix}{seq}.ts" */ }
```

Notes that shape the design:
- Anchor = first video (AVC) track. `tracks` must be passed in source order so the
  muxed PID/PMT layout matches — segments carry **all** tracks (multi-audio +
  subtitle Data), which is exactly what we must preserve.
- `window` is the rolling media-playlist length and must be `> 0`. There is no
  VOD mode; `push` returns a segment when a cut happens, `finish` flushes the
  tail. VOD is achieved by retaining all emitted segments and appending our own
  `#EXT-X-PLAYLIST-TYPE:VOD` + `#EXT-X-ENDLIST`.
- Tracks can arrive late (`add_track`) after construction — mirrors zenith's
  build-on-first-RAP-then-replay pattern.

## Architecture

Two new workspace crates, following the transmux playbook (prove here, adopt
back into zenith later). Nothing in zenith is touched in this phase.

```
raw capture .ts ──(offline, one-time)──► progressive .ts fixtures
                    scripts/preencode-fixtures.sh
                    (ffprobe field_order → bwdif? → libx264;  -c:a copy -c:s copy)

fixtures/streams/<slug>.ts ──► skyfire-server (axum) ──HTTP──► browser
                                 └ skyfire-hls::HlsSession        └ SkyfirePlayer (WASM bridge)
                                    └ skyfire-ts::TsDemux              └ WebCodecs video
                                    └ transmux::StreamingTsHlsSegmenter └ WASM audio → WebAudio
                                                                        └ DVB-sub → cues
                                 GET /stream/hls/skyfire/{slug}/index.m3u8
                                 GET /stream/hls/skyfire/{slug}/{seg}.ts
                                 GET /api/streams

Playwright streams.spec.mjs ── drives player, samples window.__sfStats over time,
                               asserts 4 dims per stream from fixtures/streams.json
```

### Component 1 — `skyfire-hls` crate (library)

Source-agnostic. Fed raw TS bytes; owns no HTTP and no async.

```rust
pub struct HlsConfig {
    pub target_secs: u32,      // segment target (match zenith: 4)
    pub window: Option<usize>, // None = VOD (retain all + ENDLIST); Some(n) = rolling
    pub uri_prefix: String,    // "seg"
}

pub struct HlsSession { /* demux + segmenter + buffered late-tracks + segment store */ }

impl HlsSession {
    pub fn new(cfg: HlsConfig) -> Self;
    pub fn feed(&mut self, data: &[u8]);          // demux, push samples, cut segments
    pub fn finish(&mut self);                     // flush tail; mark VOD complete
    pub fn playlist(&self) -> String;             // rolling window, or VOD + ENDLIST
    pub fn segment(&self, name: &str) -> Option<Arc<Vec<u8>>>;
    pub fn is_ready(&self) -> bool;               // ≥1 segment emitted (for 503 long-poll)
}
```

Ported orchestration (from `skyfire_hls.rs`, minus DVB/NVENC):
- Feed TS to `skyfire_ts::TsDemux`; poll `DemuxEvent`.
- Buffer samples until the **first video RAP** (`Sample.is_sync` on the AVC
  track); then construct `StreamingTsHlsSegmenter` with all known tracks in
  source order, replay the buffer, and drop orphan P-frames before the first
  IDR (RAP-trim — segment 0 must start at a RAP).
- Late audio/subtitle tracks arriving after build → `add_track`.
- `push` → on `Some(TsSegment)`, store `uri → Arc<bytes>` and record duration +
  discontinuity for the playlist.
- `DemuxEvent::Discontinuity` → `mark_discontinuity`.
- Bound the pre-RAP sample buffer (cap, e.g. 2048 samples) to avoid unbounded
  growth if no RAP appears.
- VOD (`window: None`): construct the segmenter with a large window so nothing
  is evicted; retain every segment; `playlist()` emits `#EXT-X-PLAYLIST-TYPE:VOD`
  and, once `finish()` has run, `#EXT-X-ENDLIST`.

### Component 2 — `skyfire-server` bin (axum)

```
GET /stream/hls/skyfire/{slug}/index.m3u8   → playlist (503 until first segment, long-poll)
GET /stream/hls/skyfire/{slug}/{segment}    → video/mp2t bytes, 404 if unknown
GET /api/streams                            → JSON list of available slugs (from fixture dir)
```

- A `Manager` maps `slug → HlsSession`, lazily started from `<fixtures>/<slug>.ts`.
- **VOD mode (default, test gate):** on first request, feed the whole file into
  the session synchronously, then serve. Deterministic.
- **Live-sim mode (one flag / one slug):** feed the file incrementally on a timer
  (paced ~1× realtime) with `window: Some(6)`, exercising rolling window +
  503-long-poll + RAP-trim + discontinuity.
- Playlist URIs rewritten to absolute paths (as zenith does) so the browser
  fetches segments back from the server.
- Permissive CORS (`Access-Control-Allow-Origin: *`) for dev — the web app is
  served by Bun `serve.ts` on another port.
- Path-traversal guard on `{segment}` (reject `/`, `..`).
- CLI: `skyfire-server --fixtures <dir> --port <n> [--live <slug>]`.

### Component 3 — `skyfire-cli` probe + registry emitter

Extend the existing native CLI (uses `TsDemux` already) with a probe mode that,
for a given `.ts`, reports:
- audio tracks: `{pid, lang, codec}` and which is default;
- subtitle tracks: `{pid, lang}`;
- **subtitle activity**: timestamps (PTS) where subtitle Data samples carry
  page-composition segments (i.e. an actual on-screen cue) — used to cut
  sub-bearing clips deterministically;
- video: codec, dimensions.

Output JSON. This feeds two consumers: the pre-encode script (to pick a
sub-bearing window) and `fixtures/streams.json` (ground-truth expectations, not
hand-guessed).

### Component 4 — `scripts/preencode-fixtures.sh` (offline)

For each capture in `.ts-captures/`:
1. `ffprobe` the video `field_order`. If interlaced (`tt/bb/tb/bt`), insert a
   `bwdif` deinterlace filter; else none.
2. Re-encode **video only**: `-c:v libx264 -profile:v high -pix_fmt yuv420p
   -g 50 -keyint_min 50 -sc_threshold 0` → clean, closed-GOP (~2s), RAP-aligned,
   headless-decodable H.264.
3. **Preserve everything else**: `-map 0 -c:a copy -c:s copy -copyts` — every
   audio PID, DVB-subtitle PID, and SI passes through byte-exact (skyfire never
   re-encodes audio; subs pass through). This is what lets the gate prove
   multi-audio + subtitles survive the chop.
4. Write full-length progressive TS to `.ts-captures/progressive/<slug>.ts`
   (local, gitignored).
5. For the committed subset (~6–8 streams, chosen one-per codec/scan-type):
   cut a ~20–30s clip (long enough to catch "stops after a few seconds"). For
   sub-bearing streams, use the skyfire-cli subtitle-activity probe to pick a
   window containing a cue, so cue-render assertions are deterministic. Write to
   `fixtures/streams/<slug>.ts`.
6. Emit/update `fixtures/streams.json` from the skyfire-cli probe over each
   committed clip.

Fails loud if ffmpeg/ffprobe missing or a stream is unreadable. Idempotent.

### Component 5 — Player `__sfStats` enrichment (minimal, no UI)

`SkyfirePlayer` already feeds the bridge and tracks basic stats and already has
`selectAudio`/`selectSubtitle` and DVB-sub→cue handling. Add to `window.__sfStats`:
- `tracks: { audio: [{pid, lang, codec}], subtitle: [{pid, lang}] }`
- `selectedAudio` (pid), `decodedAudioPid` (pid the current PCM came from)
- `subtitleCues` (count of cues rendered so far)
- keep `decoded`, `drawn`, `audioSamples`, `audioFrames`, `avSkewMs`, `w`, `h`.

No menus, no styling — those are Phase 2. Just observable state + the two
selection calls reachable from the page (`window.__sfPlayer.selectAudio(pid)`).

### Component 6 — `web/tests/streams.spec.mjs` (Playwright)

- `globalSetup` builds `skyfire-server` (or uses a prebuilt binary) and spawns it
  against `fixtures/streams/`; also ensures Bun `serve.ts` is up for the web app.
- Reads `fixtures/streams.json`; one parameterized test per stream. Player src =
  `http://localhost:<sfport>/stream/hls/skyfire/<slug>/index.m3u8`.
- **Continuity-based assertions** (the core change — catch stall, not burst):
  - Sample `__sfStats` every ~250ms for the clip duration.
  - **Video:** `decoded`/`drawn` strictly increase across the run; no gap
    > ~500ms with zero progress; final `decoded ≈ fps × clip_seconds`
    (within tolerance); `w`/`h` match registry.
  - **Audio:** `audioSamples` strictly increases across the run; total audio
    duration ≥ `clip_seconds − tolerance`; no silence gap > ~500ms.
  - **A/V:** `avSkewMs` bounded (< ~120ms) at every sample, not just once.
  - **Audio selection:** track-list matches registry; for streams with ≥2 audio,
    call `selectAudio(altPid)` and assert `decodedAudioPid` changes and audio
    keeps flowing.
  - **Subtitles:** subtitle tracks listed per registry; where `expectSubCues`,
    assert `subtitleCues ≥ 1`.
  - **Errors:** zero real console errors.
- One **live-sim** test: server started with `--live <slug>`; assert the playlist
  grows over time, caps at the window, and decode continues across a
  discontinuity.
- Runs headless in CI; a `--headed` local mode uses real GPU WebCodecs for bugs
  that only reproduce with hardware decode.

### Component 7 — CI e2e job

New job in `.github/workflows/` (there is no browser gate today):
- Build WASM `--target web` (absolute out-dir), build `skyfire-server`.
- Install Playwright chromium.
- Start `serve.ts` + `skyfire-server`; run `streams.spec.mjs` over the committed
  subset headless.
- Fails the build on any stream regressing. (Initially expected to fail —
  reproducing the current bugs — until Phase 2 fixes land. The job may start as
  non-blocking / allowed-to-fail and flip to required once green.)

## Data flow

1. Offline: captures → progressive fixtures + registry (one-time / on capture
   refresh).
2. Test run: Playwright starts servers → for each stream, browser loads player →
   player fetches `index.m3u8` → server lazily builds `HlsSession`, feeds the
   fixture, returns playlist → player fetches segments → bridge demuxes, decodes
   video (WebCodecs) + audio (WASM→WebAudio) + subtitles → `__sfStats` updated →
   Playwright samples and asserts continuity → pass/fail per stream.

## Error handling

- Server: 503 while awaiting first segment (bounded long-poll), 404 unknown
  slug/segment, path-traversal guard, permissive CORS.
- `HlsSession`: bounded pre-RAP buffer; if no video RAP within the cap, surface a
  clear error rather than growing unbounded.
- Pre-encode: fail loud on missing tooling / unreadable input; never emit a
  silently-truncated fixture.
- Harness: distinguish real console errors from known headless-codec noise; a
  stall (no progress) is a failure with the last-sampled stats attached for
  diagnosis.

## Testing

- **skyfire-hls**: Rust unit tests over the committed fixtures — segment count,
  each segment starts at a RAP, all source PIDs present in every segment,
  playlist well-formed (VOD ENDLIST; rolling window length + discontinuity tags),
  byte-stable output. These are the ungameable gates (real fixtures, not inline
  bytes).
- **skyfire-server**: integration test — GET playlist + segments, 503→200
  transition, 404, CORS header.
- **Browser gate**: `streams.spec.mjs` as above — the system-level TDD gate.
- CI gate unchanged for Rust: `fmt`, `clippy -D warnings`, `build`, `nextest`.

## Constraints

- No `unsafe`. Dual MIT OR Apache-2.0. No `Co-Authored-By` in commits.
- No rsmpeg/CUDA/ffmpeg *dependency* in the crates — ffmpeg is only invoked by
  the offline pre-encode script, never at serve time.
- Touch only new crates + skyfire-cli + web/ + player stats; keep everything that
  passes green. Do not modify zenith.
- Committed fixtures kept small (curated subset, ~20–30s clips); full set stays
  gitignored under `.ts-captures/progressive/`.

## Open decisions carried into Phase 2

- Fixing the stutter/stop + no-audio root causes (the whole point of the gate).
- The sophisticated player UI + its own visual design pass.
- Whether zenith adopts `skyfire-hls` (dedupe its inline worker).
