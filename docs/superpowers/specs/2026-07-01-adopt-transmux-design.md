# Adopt `transmux` — drop bespoke H.264 container code + add fMP4/MSE video fallback

- **Date:** 2026-07-01
- **Status:** Approved (design)
- **Scope:** Video container layer only. Audio (AC-3/E-AC-3 → WASM → WebAudio),
  DVB-subtitle, SI/PCR/PTS pass-through are untouched. Aligns with
  [ADR 0008](../../decisions/0008-video-only-transcode-wasm-bridge.md).

## Motivation

`transmux 0.1.0` (from `github.com/fishloa/rust-broadcast`) is a spec-built
(ISO/IEC 14496-12:2015, RFC 8216), `no_std` + `alloc`, `forbid(unsafe)`
container-layer crate: samples-in TS→CMAF/fMP4 remux, Annex B ↔ length-prefixed
NAL conversion, `avcC`/`hvcC` decoder-config records, and HLS playlist
generation. Its only dependency is `broadcast-common ^8`.

Skyfire currently hand-rolls the H.264 container bits in
`skyfire-ts/src/h264_config.rs` (`h264_decoder_config`, `build_avcc_description`,
`annexb_to_avcc`) on top of `h264_reader`. Adopting `transmux`:

1. Replaces that bespoke code with a spec-cited, round-trip-tested library.
2. Rides the forced `broadcast-common 8` migration (see Part 1) that
   `dvb-subtitle 0.1.2` already requires.
3. Unlocks an fMP4/CMAF **MSE fallback** video path for browsers where
   WebCodecs H.264 is unavailable — notably iOS Safari/WebKit — satisfying
   [ADR 0001](../../decisions/0001-support-scope.md)'s iOS-17 requirement.

## Current video path (baseline)

- **AU extraction:** `skyfire-ts/src/lib.rs` `EsDemux` → `AccessUnit { pid,
  pts_ticks, dts_ticks, es_bytes }` (Annex B). `skyfire-core` `Engine.drain_units()`
  routes video AUs by PID.
- **avcC + Annex B→AVCC:** `skyfire-ts/src/h264_config.rs`
  (`h264_decoder_config`, `build_avcc_description`, `annexb_to_avcc`), using
  `h264_reader::annexb::AnnexBReader` for SPS/PPS.
- **Bridge:** `skyfire-wasm/src/lib.rs` `SkyfireBridge.take_video_aus()` converts
  Annex B→AVCC on drain → `WasmVideoAu { bytes, pts_ticks, dts_ticks,
  is_keyframe }`; `video_config_description()` → avcC bytes.
- **Browser:** `web/player.js` `VideoDecoder.configure({ codec, description: avcc })`
  + `EncodedVideoChunk` → `decode()`. **No MSE/fMP4 code exists in the repo.**
- **CLI:** `skyfire-cli` inspects only; no video output. Unaffected.

## Design

### Part 1 — Dependency migration (forced prerequisite)

`cargo update` pulled the renamed `broadcast-common 8` ecosystem.
`dvb-subtitle 0.1.2` and `dvb-pes 0.1.2` moved `parse`/`to_bytes` out of inherent
methods into `broadcast_common::traits::{Parse, Serialize}`, breaking 10 call
sites in `skyfire-ts` (`lib.rs`, `subtitle_compositor.rs`).

- `skyfire-ts/Cargo.toml`: add `broadcast-common = "8"`; deps `dvb-pes = "0.1.2"`,
  `dvb-subtitle = "0.1"` (0.1.2 locked).
- Add `use broadcast_common::{Parse, Serialize}` (or trait-scoped imports) at the
  10 call sites.
- `dvb-si` / `dvb-common 7.9` stay as-is — the PSI path is unaffected; the two
  ecosystems coexist in the tree without conflict.

Exit: CI gate green again with no behavioural change.

### Part 2 — Scope A: primitive swap (WebCodecs path unchanged)

Add `transmux = "0.1"` to `skyfire-ts`. In `h264_config.rs`:

- `annexb_to_avcc()` → `transmux::annexb_to_length_prefixed` /
  `transmux::Sample::from_annexb`.
- `build_avcc_description()` → `transmux::{AVCDecoderConfigurationRecord,
  AVCConfigurationBox}` serialized to avcC bytes.
- SPS/PPS geometry (width/height/profile/level): prefer
  `transmux::nalu_types::{AvcSps, AvcPps}`. **Drop `h264_reader`** if that covers
  geometry; otherwise keep `h264_reader` for SPS parse only. (Plan-time check.)

Output to the browser is byte-identical (avcC + length-prefixed AUs); the
WebCodecs path is untouched.

### Part 3 — Scope B: fMP4/CMAF MSE fallback

`skyfire-wasm`: add `transmux` dep. New bridge API across the wasm-bindgen
boundary:

- `video_init_segment() -> Vec<u8>` — `ftyp` + fragmented-init `moov`, built once
  SPS/PPS are seen (`TrackSpec` with `CodecConfig::Avc`, timescale 90000 to match
  TS PTS).
- `take_video_media_segment() -> Option<WasmMediaSegment>` — drains pending AUs
  into **one CMAF media segment per closed GOP** (`styp` + `moof` + `mdat`);
  `base_media_decode_time` from DTS, per-sample `duration` / `composition_offset`
  (pts − dts) / `is_sync` derived from AU PTS/DTS/keyframe flag.

`web/player.js`: capability gate. When `VideoDecoder.isConfigSupported(cfg)`
resolves unsupported (or `VideoDecoder` is absent), take the MSE path:
`MediaSource` + a **video-only** `SourceBuffer('video/mp4; codecs=...')`; append
the init segment, then media segments as they drain.

**A/V sync (MSE mode):** audio remains master (WASM → WebAudio). The `<video>`
element is **muted, video-only**. A thin JS drift-corrector slaves
`video.currentTime` (mapped through PTS) to the `AudioClock`:

- drift > ~50 ms → seek `video.currentTime`;
- small drift → nudge `playbackRate` within 0.98–1.02.

`skyfire-sync` (`AudioClock`, `FrameAction`) is unchanged and stays audio-master;
the corrector lives in JS and only runs in MSE mode. WebCodecs mode keeps its
existing frame-timed sync.

### Part 4 — Verification

CLAUDE.md CI gate (must pass before commit):

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo nextest run --workspace
```

Behavioural checks:

- **Native fixture test** (CI-able, deterministic): from a `fixtures/` france-2
  TS, build the init + media segments and assert they re-parse via
  `transmux::parse_box` (round-trip) and that sample count / durations match the
  demuxed AU stream. Golden-byte where stable.
- **Playwright (desktop)**: force the MSE path via a query flag, load france-2,
  assert `<video>` advances, frames render, and A/V drift stays within tolerance.
- **iOS Safari (real device)**: manual, out-of-CI — recorded as external-blocked,
  consistent with the existing e2e story.

## Crate placement

- `transmux` dep in `skyfire-ts` (Part 2 primitives) and `skyfire-wasm`
  (Part 3 pipeline). `no_std` + alloc → WASM-clean.
- `broadcast-common = "8"` dep in `skyfire-ts` for the trait imports.
- `h264_reader` removed if `transmux::nalu_types` covers SPS geometry.

## Out of scope (YAGNI)

- **HEVC / `hvcC`** — `transmux` supports it, but H.265 is gated separately by
  ADR 0001.
- **AAC / `esds`** — skyfire audio is AC-3/E-AC-3, decoded in WASM, never muxed.
- **transmux HLS playlist generation** — WASM feeds MSE directly; no HLS delivery.
- **`media-doctor` / `ts-fix` adoption** — separate future work for the debug
  harness.

## Open plan-time questions

1. Does `transmux::nalu_types::AvcSps` expose width/height/profile/level
   sufficient to drop `h264_reader`? If not, keep `h264_reader` for SPS parse.
2. Exact `codecs=` string for the MSE `SourceBuffer` (RFC 6381, from SPS
   profile/level) — reuse the existing `video_config_codec()` value.
