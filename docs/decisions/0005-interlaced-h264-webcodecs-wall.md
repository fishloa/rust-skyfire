# 0005 — Interlaced H.264 is the real WebCodecs wall; software-decode it in WASM

- **Status:** Accepted
- **Date:** 2026-06-19

## Context

The v1 player loaded but decoded **zero** video frames from the DVB HD fixtures.
Empirically isolated in Chrome (Playwright, bare `VideoDecoder`, demux via the
real engine):

| fixture | SPS `frame_mbs_only_flag` | IDR present | frames decoded |
|---|---|---|---|
| h264-25fps (progressive) | 1 (progressive) | yes | **60/60** ✅ |
| m6-clean (1080i) | 0 (**interlaced**) | **yes, AU0** | **0** ❌ "Decoding error." |
| gulli-15s (1080 PsF) | 0 (**interlaced**) | no | 0 ❌ |

Two independent WebCodecs constraints, both proven:

1. **Requires an IDR to start.** Feeding h264-25fps from a non-IDR frame throws
   *"A key frame is required after configure()."* gulli has zero IDR (open-GOP /
   recovery-point broadcast) → can't start.
2. **Cannot decode interlaced H.264 at all.** m6 has a valid IDR at AU0, fed as
   key, and still yields 0 frames — the only differentiator from the working
   progressive fixture is `frame_mbs_only_flag = 0`. This matches Chrome's known
   WebCodecs limitation (no field/MBAFF/PAFF support).

Virtually all DVB HD is 1080i interlaced H.264, so constraint #2 is fatal for the
core use case — and **re-encoding is off the table** (project premise + owner
directive).

## Decision

Split the video path by stream type:

- **Progressive H.264** → WebCodecs `VideoDecoder` (HW), as today. For open-GOP
  progressive channels lacking an IDR, synthesize/inject a keyframe to satisfy
  constraint #1 (no re-encode — bitstream-level).
- **Interlaced H.264 (1080i/PsF)** → **software-decode in WASM via `oxideav-h264`**
  (the owner's pure-Rust decoder; supports MBAFF, PAFF in progress). Decode is not
  re-encode. Output YUV → GPU **deinterlace** (epic #6) → canvas.

`skyfire-ts` already detects `frame_mbs_only_flag` (added during this
investigation) to route streams.

## Consequences

- The "browser can't play this" problem is bigger than AC-3 audio: it includes
  interlaced video. The WASM software path (heavier CPU, no HW) is the price of
  zero-transcode 1080i in-browser.
- Hard dependency on `oxideav-h264` decode correctness for real broadcast streams
  (tracked upstream). Same model as `oxideav-ac3` for audio.
- Capability routing replaces the "WebCodecs for everything" assumption in
  ADR 0001 for video (ADR 0001's gating policy still governs codec support
  probing). Deinterlace (epic #6) becomes load-bearing, not optional.
