# Skyfire — objectives & roadmap

> Single source of truth for **what** we're building and **where it stands**.
> Decisions (**why**) live in [`decisions/`](decisions/). Keep this file current:
> when an epic's state changes, update its row in the same change.

_Last updated: 2026-07-01._

## Primary objective

Play the full DVB satellite lineup **in any supported browser** via a **skyfire
WASM bridge client** fed a **video-only-transcoded MPEG-TS** from the server
([ADR 0008](decisions/0008-video-only-transcode-wasm-bridge.md), superseding the
HLS/thin-client direction of 0007; keeps the deinterlace decision of
[0006](decisions/0006-server-side-deinterlace-for-mobile.md)). On-device probing
proved no browser hardware-decodes interlaced 1080i — so the server re-encodes
**video only** (deinterlace → progressive H.264) and re-muxes TS; everything else
is carried untouched. The client reuses skyfire's WebCodecs + WASM + sync.

- **Server** (zenith — in-process `rsmpeg`, **no subprocess**, on zelkova's RTX
  A2000): per-channel video-transcode adaptor `h264_cuvid -deint` → `h264_nvenc`
  (High@4.2, 50p) → re-mux TS. **Audio (AC-3/E-AC-3), DVB-subtitle/teletext, SI
  PIDs, PCR/PTS all passthrough untouched.** On-demand per viewer (GPU-scarce).
  Endpoint `GET /skyfire/<serviceSlug>` — chunked single-program TS.
- **Client (skyfire WASM bridge):** demux TS; hand progressive H.264 AUs to
  **WebCodecs** (HW video); decode the selected AC-3/E-AC-3 PID in **WASM** →
  PCM → WebAudio; parse **DVB subtitles** → cues; **audio-master sync** off
  PCR/PTS. Browser owns canvas/WebAudio/overlay/controls; WASM is the bridge.
  H.264 container layer (avcC, Annex B↔fMP4) uses the **`transmux`** crate; where
  WebCodecs is unavailable (e.g. some iOS/WebKit) the client repackages the
  progressive H.264 into **fMP4 and plays via MSE** as a fallback
  ([ADR 0009](decisions/0009-transmux-fmp4-mse-fallback.md)).
- **Audio is never re-encoded.** Only video is touched, and only when interlaced.
- **`oxideav-h264` is dropped from the browser** (server delivers progressive);
  kept upstream as a conformance asset only.

Secondary objective: an experiment in AI-orchestrated engineering — cross-project
coordination between the skyfire and zenith Claude sessions over a shared GitHub
"bus" ([epic #27](https://github.com/fishloa/rust-skyfire/issues/27)).

## Success criteria

- A **zenith video-only-transcoded TS** (progressive H.264 + untouched AC-3/E-AC-3
  + DVB subs + preserved PCR/PTS) plays in-browser via the skyfire WASM bridge:
  **HW video (WebCodecs) + WASM AC-3 audio, A/V in sync**, on desktop Chrome and
  iOS 17+ Safari.
- **Audio-track flip** works at runtime from a browser command; **DVB subtitles**
  render as a toggleable overlay.
- The CI gate (fmt + clippy `-D warnings` + build + `nextest`) is green on every
  commit; behavioural tests decode real fixtures, not just compile.

## First release (v1)

**A zenith `/skyfire/<slug>` stream (progressive H.264 + untouched AC-3 + DVB subs)
plays in desktop Chrome and iOS 17+ Safari via the skyfire WASM bridge — HW video,
WASM AC-3 audio, in sync, with working audio-flip + subtitle overlay.** Tracked as
[epic #27](https://github.com/fishloa/rust-skyfire/issues/27); server side is
zenith#986. Supersedes both the original WebCodecs+WASM v1 and the ADR 0007
HLS/thin-client v1. Costs in [COSTS.md](COSTS.md)
([ADR 0002](decisions/0002-delegation-working-practice.md)).

## Support scope

Chrome/Edge, Safari, Firefox — desktop and mobile (iOS 17+ as one WebKit target,
Android). Server normalises every channel to progressive H.264, so the WebCodecs
video path is universal. Full detail:
[ADR 0001](decisions/0001-browser-and-platform-support.md).

## Roadmap (epics)

Tracked as GitHub EPIC issues. Status here mirrors reality; sub-issues are the
work units.

> **ADR 0008 reframe (2026-06-22):** mainline is now a **video-only server
> transcode (zenith) + skyfire WASM bridge client**. The client epic is
> **[#27](https://github.com/fishloa/rust-skyfire/issues/27)** (WebCodecs video +
> WASM AC-3 + DVB subs over zenith's video-only TS), which carries the
> zenith↔skyfire stream contract and a 10-item sub-issue backlog. It **supersedes
> client epics #2–#6** below. Server transcode lives in zenith (zenith#986), not a
> skyfire crate. Rows below are pre-pivot, retained for history.

### Client build status — epic [#27](https://github.com/fishloa/rust-skyfire/issues/27) (2026-06-22)

| # | Item | State |
|---|---|---|
| #28 | skyfire-ts demux + PSI track enumeration | ✅ done (nextest) |
| #29 | skyfire-wasm streaming bridge API | ✅ done (nextest) |
| #30 | video AUs → WebCodecs → canvas | ✅ browser-verified (Chromium e2e) |
| #31 | AC-3/E-AC-3 → WASM → WebAudio | ✅ browser-verified (PCM played) |
| #32 | audio-master A/V sync | ✅ browser-verified (skew < 120 ms) |
| — | remove dead SW H.264 path | ✅ done |
| #33 | runtime audio-track flip | code done (select_audio+reset); A/B verify needs a multi-audio fixture |
| #34 | DVB subtitle: parse → composite (EN 300 743 → RGBA) → overlay blit | Rust composite + JS overlay done (nextest + e2e no-regression); end-to-end render verify needs a dvb-sub fixture → #40 |
| #35 | UI shell: pickers + controls + overlay | ✅ done (exercised by e2e); bitmap-sub render pending |
| #36 | hold-open + reconnect stream loop | ✅ done (e2e); live zenith endpoint verify pending |
| #37 | E2E Playwright spec | ✅ done — 3/3 green in Chromium (`bunx playwright test`) |
| #38 | PsF decoder oracle (zenith gate) | harness done + PASS-proven; **open** for a real zenith PsF sample |

**v1 (core A/V) is browser-verified:** TS → WASM-bridge demux → WebCodecs HW video
+ WASM AC-3 audio → audio-master sync, on real 1080p deinterlaced content, 3/3 e2e
green. Remaining gaps are external-resource-gated: a DVB-subtitle / multi-audio
capture, a live zenith `/skyfire` endpoint, an iOS 17 device, and the zenith PsF
sample for the #38 gate.

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
