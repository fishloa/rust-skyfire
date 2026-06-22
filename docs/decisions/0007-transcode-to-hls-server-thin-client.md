# 0007 — Skyfire is a GPU transcode-to-HLS server with a thin client

- **Status:** Superseded by [0008](0008-video-only-transcode-wasm-bridge.md) — the
  AAC transcode + thin-client decision is reversed: audio stays bit-exact
  (untouched AC-3/E-AC-3, WASM-decoded) and the skyfire WASM bridge client returns.
  The H.264 video deinterlace+re-encode decision still holds.
- **Date:** 2026-06-21

## Context

ADR 0006 accepted a server-side **video** deinterlace to make 1080i playable in
any browser. Two follow-on decisions (2026-06-21, owner-directed) complete the
architecture and collapse the client:

1. **Output video codec = H.264** (progressive), not HEVC. The deinterlace
   re-encode happens regardless, so the output codec is a free choice — chosen for
   **universal** hardware decode (iOS native HLS + desktop WebCodecs/MSE, no
   Chrome/Firefox HEVC gating). HEVC as a bandwidth-saving ABR rendition is
   deferred (NVENC makes dual-encode cheap later).
2. **Audio = transcode E-AC-3/AC-3 → AAC** on the server. Browsers don't decode
   AC-3 natively except Safari; AAC is universal. Since video is already
   re-encoded, the old "audio never re-encoded" constraint no longer buys
   anything — drop it.

Result: the server emits **standard HLS — H.264 progressive + AAC** — the most
universally playable combination. That removes every hard client component: no
WASM decode, no WebCodecs, no custom A/V sync (the media element does it).

**Execution constraint (owner):** no subprocess, no CLI. The transcode runs
**in-process via linked libav\*** (FFmpeg libraries) through Rust bindings
(`ffmpeg-next` / `ffmpeg-sys-next`) — not by spawning `ffmpeg`. The graph is the
one verified on zelkova (RTX A2000, NVENC/NVDEC):
`h264_cuvid` (NVDEC) → `yadif_cuda` (→50p) → `h264_nvenc` (High@4.2) for video;
`eac3` decode → `aac` encode for audio; libavformat HLS muxer. Per-source decoder
chosen by PMT `stream_type` (`h264_cuvid` / `hevc_cuvid` / `mpeg2_cuvid`).

## Decision

Skyfire's product is **a GPU transcode-to-HLS server + a thin client.**

- **Server** (new crate, working name `skyfire-transcode`): in-process libav\*
  hardware pipeline on zelkova. Output: standard HLS, H.264 High@4.2 progressive
  50p (NVENC) + AAC-LC, E-AC-3 source. No subprocess.
- **Client (thin):** iOS/Safari → native `<video>` HLS. Desktop Chrome/Firefox →
  `hls.js` (MSE). No WASM, no custom decoders, no custom sync.
- **WASM stack disposition:** `oxideav-h264` retained as (a) an optional
  **zero-transcode** desktop mode behind a switch, (b) a conformance asset.
  `oxideav-ac3` / `skyfire-ac3` leave the mainline (AAC replaces). `skyfire-ts` /
  `skyfire-sync` / `skyfire-wasm` are off the mainline playback path.

## Consequences

- **Roadmap churn:** client epics #2 (AC-3 WASM), #3 (WebCodecs), #4 (sync), #5
  (WASM shell), #6 (GPU deinterlace) drop from the mainline or repurpose; #1 demux
  logic is subsumed by libavformat (kept for PSI/source-codec routing reference).
  OBJECTIVES roadmap updated in the same change.
- **New dependency + build env:** FFmpeg libav\* via `ffmpeg-next`. zelkova needs
  `libav*-dev`, `rustup`, `clang` installed (probe: only runtime libs present).
- **Licensing:** Ubuntu's ffmpeg is built `--enable-gpl` (ships libx264) → linking
  it makes the server binary effectively **GPL**. Fine for an internal,
  non-distributed server; if ever distributed, either comply with GPL or link an
  LGPL-only ffmpeg built without GPL components (we use `nvenc` + native `aac` +
  `yadif_cuda`, all LGPL-clean — no libx264 needed). Documented, not yet enforced.
- **`no-unsafe` carve-out:** `ffmpeg-sys` FFI + hardware-frame-context wiring
  requires `unsafe`, confined to the transcode crate's FFI layer and justified
  per call site. This ADR is that justification.
- **CI:** the transcode crate cannot build/run in native macOS CI (needs CUDA +
  NVIDIA + libav). Feature/target-gate it; its behavioural check runs on zelkova —
  golden HLS: `ffprobe` confirms progressive H.264 + AAC, IDR-aligned segments,
  and it plays on iOS native + `hls.js`.

## Implementation note (2026-06-21)

The transcode does **not** live in a new skyfire crate — it lands **in zenith**,
which already owns DVB ingest, descramble, per-channel TS fan-out, and a
(shelved) CMAF browser-player subsystem. The work is to **revive zenith's parked
`archive/in-browser-player` branch** (pure-Rust `dvb-pes` demux, `rsmpeg`
AC-3/E-AC-3/MP2→AAC audio transcode, pure-Rust fMP4/CMAF muxer, `/hls/cmaf/*`
serving) and add the one stage it lacked: **video deinterlace + H.264 re-encode**
(`h264_cuvid -deint 2` → `h264_nvenc`, in-process via `rsmpeg`). Output is CMAF/
fMP4 (not TS-HLS) — plays on iOS native fMP4-HLS + desktop MSE. This reverses
zenith's "Don't add in-browser playback" rule. Tracked as **zenith#986**
(`backend-builder`). Skyfire keeps the thin-client intent + the `oxideav-h264`
zero-transcode opt-in / conformance asset.
