# Skyfire — objectives & roadmap

> Single source of truth for **what** we're building and **where it stands**.
> Decisions (**why**) live in [`decisions/`](decisions/). Keep this file current:
> when an epic's state changes, update its row in the same change.

_Last updated: 2026-06-21._

## Primary objective

Play the full DVB satellite lineup **in any supported browser**, via a **GPU
transcode-to-HLS server + a thin client**
([ADR 0007](decisions/0007-transcode-to-hls-server-thin-client.md), building on
[0006](decisions/0006-server-side-deinterlace-for-mobile.md)). On-device probing
proved no browser hardware-decodes interlaced 1080i and software decode is
desktop-only — so the server normalises every channel to **standard HLS
(progressive H.264 + AAC)**, the universally playable combination, and the client
is a thin native/`hls.js` player.

- **Server** (in-process libav\*, **no subprocess**, on zelkova's RTX A2000):
  `h264_cuvid`/`hevc_cuvid`/`mpeg2_cuvid` (NVDEC) → `yadif_cuda` deinterlace
  → 50p → `h264_nvenc` (High@4.2); E-AC-3/AC-3 → **AAC-LC**; libavformat HLS mux.
- **Client (thin):** iOS/Safari → native `<video>` HLS; desktop Chrome/Firefox →
  `hls.js` (MSE). No WASM, no custom decoders, no custom sync — the media element
  handles A/V.
- **Zero-transcode WASM path** (`oxideav-h264` + WebCodecs + audio-master sync) is
  retained as an **optional desktop mode** and conformance asset, off the mainline.
- **Container** → libavformat demux; `rust-dvb` PSI logic kept for source-codec
  routing reference.

Secondary objective: serve as an experiment in AI-orchestrated engineering —
Claude writes spec briefs and verifies; delegated open models write the code.

## Success criteria

- The transcode server turns a real DVB 1080i channel into standard HLS
  (progressive H.264 + AAC), in-process (no subprocess), on zelkova's GPU.
- That HLS plays end-to-end with correct in-sync A/V on **both** iOS Safari
  (native `<video>`) and desktop Chrome/Firefox (`hls.js`).
- The CI gate (fmt + clippy `-D warnings` + build + `nextest`) is green on every
  commit; the GPU transcode crate is target-gated and verified on zelkova
  (golden HLS: `ffprobe` shows progressive H.264 + AAC, IDR-aligned segments).

## First release (v1)

**A DVB 1080i fixture transcoded by the server (in-process libav, NVDEC →
yadif_cuda → NVENC + AAC) plays as standard HLS on iOS Safari native and desktop
`hls.js`, full A/V, no crash / underrun / drift.** Superseded the original
WebCodecs+WASM v1 (see ADR 0007). Costs tracked in
[COSTS.md](COSTS.md) ([ADR 0002](decisions/0002-delegation-working-practice.md)).

## Support scope

Chrome/Edge, Safari, Firefox — desktop and mobile (iOS 17+ as one WebKit target,
Android). H.265 gated per-stream with H.264 fallback. Full detail and rationale:
[ADR 0001](decisions/0001-browser-and-platform-support.md).

## Roadmap (epics)

Tracked as GitHub EPIC issues. Status here mirrors reality; sub-issues are the
delegable work units.

> **ADR 0007 reframe (2026-06-21):** the mainline is now a transcode-to-HLS
> **server** + thin client. Client epics #2–#6 are **superseded** for the mainline
> (kept only for the optional zero-transcode WASM desktop mode / as conformance
> assets). New mainline work: **#E1 transcode server** (in-process libav GPU
> pipeline → H.264+AAC HLS, on zelkova) and **#E2 thin client** (native + hls.js).
> Rows below are pre-pivot status, retained for history until re-issued.

| Epic | Crate(s) | Objective | Status |
|---|---|---|---|
| [#1](https://github.com/fishloa/rust-skyfire/issues/1) | skyfire-ts | MPEG-TS/HLS demux → ES + PTS (reuse `rust-dvb` PSI) | In progress — #20 done ✅, #21 blocked on rust-dvb#249 |
| [#2](https://github.com/fishloa/rust-skyfire/issues/2) | skyfire-ac3 | WASM AC-3 / E-AC-3 decode → PCM | E-AC-3→PCM done ✅ (#24, wraps oxideav-ac3) |
| [#3](https://github.com/fishloa/rust-skyfire/issues/3) | web/ | WebCodecs video pipeline, HW H.264/H.265 | Open — sub-issues #9–#11 |
| [#4](https://github.com/fishloa/rust-skyfire/issues/4) | skyfire-sync, core | Audio-master A/V sync engine | sync logic done ✅ (#22,#23); engine wiring in core/#5 |
| [#5](https://github.com/fishloa/rust-skyfire/issues/5) | skyfire-wasm, web/ | WASM bindings + browser shell | Open — sub-issues #12–#15 |
| [#6](https://github.com/fishloa/rust-skyfire/issues/6) | web/ | Deinterlace + render (GPU weave shader) | Open |
| [#7](https://github.com/fishloa/rust-skyfire/issues/7) | core, web/ | Live-edge, buffering, capability fallback | Open — sub-issues #16–#18 |
| [#8](https://github.com/fishloa/rust-skyfire/issues/8) | fixtures, CI | Fixtures, conformance harness, CI/WASM build | Open — harness #19 (v1 gate) |

## Current state (2026-06-18)

Scaffold + contract-test stage. Six crate stubs (~229 lines total), `web/` empty.
Epics #3/#5/#7 decomposed into sub-issues #9–#18 (baking in ADR 0001 gating +
iOS-17 floor), ready for delegation. Epics #1/#2/#4/#6/#8 still need decomposition.
