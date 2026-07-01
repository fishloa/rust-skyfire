# 0009 — transmux container layer + fMP4/MSE video fallback

- **Status:** Accepted (complements [0008](0008-video-only-transcode-wasm-bridge.md); honours the iOS-17 target of [0001](0001-browser-and-platform-support.md))
- **Date:** 2026-07-01

## Context

ADR 0008 gives WebCodecs the progressive H.264 AUs directly. That leaves two
gaps:

1. The H.264 **container plumbing** (avcC decoder-config, Annex B ↔ length-prefixed
   NAL) was hand-rolled in `skyfire-ts` on top of `h264_reader`.
2. **WebCodecs `VideoDecoder` is not universally available** — notably older/embedded
   WebKit. ADR 0001 commits us to iOS-17 Safari, where WebCodecs H.264 support is
   not guaranteed. ADR 0008 has no video path when `VideoDecoder` is absent.

The `transmux` crate (`github.com/fishloa/rust-broadcast`, on `broadcast-common 8`)
is a spec-built (`ISO/IEC 14496-12`, `RFC 8216`), `no_std`+alloc, `forbid(unsafe)`
**container layer**: samples-in TS→CMAF/fMP4 remux, Annex B↔length-prefixed
conversion, `avcC` builders, and (from 0.3) native SPS decode + RFC 6381 codec
strings. It does no bitstream decoding — demux/transcode stay in the caller.

## Decision

**Adopt `transmux` as skyfire's H.264 container layer, and add an fMP4/CMAF MSE
video path as a fallback to WebCodecs.**

- **skyfire-ts** builds the `avcC` via `transmux::AVCDecoderConfigurationRecord`
  and converts Annex B via `transmux::annexb_to_length_prefixed`. On transmux
  **0.4**, SPS decode + RFC 6381 come from `transmux::sps` — **`h264_reader` is
  dropped** (transmux derives profile/level/constraint/chroma/bit-depth/
  frame_mbs_only/cropped dimensions itself).
- **skyfire-wasm** bridge exposes `video_init_segment()` (ftyp+moov) and
  `take_video_media_segment()` (one CMAF segment per closed GOP: styp+moof+mdat)
  via `transmux::{build_init_segment, build_media_segment}`.
- **web/player.js** picks the path once the codec is known:
  `VideoDecoder.isConfigSupported` succeeds → **WebCodecs (primary, ADR 0008)**;
  else → **MSE**: a video-only `MediaSource`/`SourceBuffer` fed the init + media
  segments. `?video=mse` forces the fallback for testing.
- **Audio is unchanged.** AC-3/E-AC-3 stays WASM→WebAudio; the muted MSE `<video>`
  is slaved to the audio-master clock (seek on gross drift, `playbackRate` nudge
  otherwise). transmux 0.4's AC-3-in-fMP4 support is **not** used — browsers can't
  decode AC-3, which is the whole reason audio is decoded in WASM.

## Consequences

- **`h264_reader` removed** from the workspace; transmux is the sole H.264
  parameter-set path. `skyfire-ts` `h264_config.rs` shrank ~48 LOC.
- **iOS-17 fallback exists** (ADR 0001) without adding a WASM H.264 decoder or
  re-muxing on the server — the client repackages progressive H.264 into fMP4.
- **Verified:** Rust CI gate green; wasm32 builds; `ffmpeg` decodes the produced
  fMP4 byte-faithfully to source; Chromium decodes both WebCodecs and MSE paths
  (`fixtures/h264-mse.ts`, a conformant Main/L3.1 fixture). **iOS-17 Safari real
  device is external-blocked** (no headless iOS) — the MSE path is the reason it
  should now work, but that is unverified until run on hardware.
- **Out of scope:** HEVC/`hvcC` (H.265 gated by ADR 0001), AAC/AC-3 audio muxing
  (audio never muxed), HLS playlist generation (WASM feeds MSE directly).
- transmux's `avcC`/`hvcC` **box-wrapper** layout is verified upstream only against
  an OCR'd `14496-15` (provenance gap). skyfire's live `avc1` path is independently
  validated by the `ffmpeg` + Chromium decode oracles; `hvcC` is unused.
