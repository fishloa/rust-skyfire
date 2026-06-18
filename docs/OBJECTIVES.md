# Skyfire — objectives & roadmap

> Single source of truth for **what** we're building and **where it stands**.
> Decisions (**why**) live in [`decisions/`](decisions/). Keep this file current:
> when an epic's state changes, update its row in the same change.

_Last updated: 2026-06-18._

## Primary objective

Play the full DVB satellite lineup **in any supported browser with zero
server-side transcode** — the client decodes raw MPEG-TS end to end:

- **Video** → WebCodecs `VideoDecoder` (HW H.264 universal, H.265 where capable).
- **Audio** → WASM AC-3 / E-AC-3 decoder → PCM → WebAudio `AudioWorklet`.
- **A/V sync** → audio is the master clock; video chases it (drift-free).
- **Container** → MPEG-TS / HLS demux, reusing `rust-dvb` for PSI parsing.

Secondary objective: serve as an experiment in AI-orchestrated engineering —
Claude writes spec briefs and verifies; delegated open models write the code.

## Success criteria

- A supported browser plays a live channel: correct video, in-sync AC-3 audio,
  no server transcode.
- Codec selection is capability-probed, with an H.264 fallback that always works
  (see [ADR 0001](decisions/0001-browser-and-platform-support.md)).
- The CI gate (fmt + clippy `-D warnings` + build + `nextest`) is green on every
  commit; behavioural tests decode real fixtures, not just compile.

## First release (v1)

**Chrome on macOS plays all three fixtures in-browser, full A/V, no issues** —
H.264 video (WebCodecs) + AC-3/E-AC-3 audio (WASM) + audio-master sync, no crash /
underrun / drift. localhost is a secure context, so the harness is a plain `bun`
static server — no HTTPS/cert/tunnel. Spans epics #1–#5. Full detail:
[ADR 0003](decisions/0003-first-release-and-test-harness.md). Delegation costs
tracked in [COSTS.md](COSTS.md) ([ADR 0002](decisions/0002-delegation-working-practice.md)).

## Support scope

Chrome/Edge, Safari, Firefox — desktop and mobile (iOS 17+ as one WebKit target,
Android). H.265 gated per-stream with H.264 fallback. Full detail and rationale:
[ADR 0001](decisions/0001-browser-and-platform-support.md).

## Roadmap (epics)

Tracked as GitHub EPIC issues. Status here mirrors reality; sub-issues are the
delegable work units.

| Epic | Crate(s) | Objective | Status |
|---|---|---|---|
| [#1](https://github.com/fishloa/rust-skyfire/issues/1) | skyfire-ts | MPEG-TS/HLS demux → ES + PTS (reuse `rust-dvb` PSI) | Open — sub-issues #20–#21 (ADR 0004) |
| [#2](https://github.com/fishloa/rust-skyfire/issues/2) | skyfire-ac3 | WASM AC-3 / E-AC-3 decode → PCM | Open — stub, no sub-issues |
| [#3](https://github.com/fishloa/rust-skyfire/issues/3) | web/ | WebCodecs video pipeline, HW H.264/H.265 | Open — sub-issues #9–#11 |
| [#4](https://github.com/fishloa/rust-skyfire/issues/4) | skyfire-sync, core | Audio-master A/V sync engine | Open — stub, no sub-issues |
| [#5](https://github.com/fishloa/rust-skyfire/issues/5) | skyfire-wasm, web/ | WASM bindings + browser shell | Open — sub-issues #12–#15 |
| [#6](https://github.com/fishloa/rust-skyfire/issues/6) | web/ | Deinterlace + render (GPU weave shader) | Open |
| [#7](https://github.com/fishloa/rust-skyfire/issues/7) | core, web/ | Live-edge, buffering, capability fallback | Open — sub-issues #16–#18 |
| [#8](https://github.com/fishloa/rust-skyfire/issues/8) | fixtures, CI | Fixtures, conformance harness, CI/WASM build | Open — harness #19 (v1 gate) |

## Current state (2026-06-18)

Scaffold + contract-test stage. Six crate stubs (~229 lines total), `web/` empty.
Epics #3/#5/#7 decomposed into sub-issues #9–#18 (baking in ADR 0001 gating +
iOS-17 floor), ready for delegation. Epics #1/#2/#4/#6/#8 still need decomposition.
