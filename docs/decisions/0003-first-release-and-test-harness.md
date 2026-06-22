# 0003 — First release (v1) scope & test harness

- **Status:** Superseded by [0008](0008-video-only-transcode-wasm-bridge.md) for the
  *v1 scope* — v1 is now "play zenith's `/skyfire/<slug>` progressive-TS in Chrome +
  iOS 17 via the WASM bridge" (see ADR 0008 / epic #27), not "Chrome WebCodecs+WASM
  on the three local fixtures". The localhost-`bun`-harness test approach here still
  applies.
- **Date:** 2026-06-18

## Context

ADR 0001 fixes the eventual support matrix (all engines, desktop + mobile). To
ship something real and verifiable first, v1 deliberately narrows to one target:
the maintainer's local browser, **Chrome on macOS**.

Chrome on macOS is Chromium with full WebCodecs, and `http://localhost` **is a
secure context** — so WebCodecs runs without TLS. No certs, no tunnel, no device
provisioning. The harness is just a static file server.

## Decision

**v1 = Chrome on macOS plays all three fixtures in-browser, full A/V, no issues:**

- Fixtures: `h264-25fps.ts`, `m6-clean.ts`, `gulli-15s.ts`.
- H.264 video decoded via WebCodecs → canvas; AC-3 / E-AC-3 audio decoded in WASM
  → WebAudio; audio-master A/V sync. No crash, no underrun, bounded drift.
- Requires epics #1 (demux), #2 (AC-3), #3 (video), #4 (sync), #5 (wasm + shell).

**Test harness:** a `bun` static server serving `web/` + `fixtures/` on
`http://localhost:<port>`. Plain HTTP is fine — localhost is a secure context for
WebCodecs/WASM. No HTTPS, cert, or tunnel for v1. (HTTPS/LAN/tunnel is only needed
later for real iOS devices, per ADR 0001; out of scope here.)

**Release gate (v1 done):** all three fixtures play clean in Chrome/macOS via the
harness — demux PIDs/PTS correct, decoded PCM finite (no NaN/Inf), video frames in
PTS order at correct dimensions, A/V drift bounded over the clip.

## Consequences

- ADR 0001's capability-probe design still governs the code (it's how we reach the
  wider matrix later) — v1 just only *needs* the Chromium/H.264+H.265 path proven.
- The harness is a tracked deliverable under epic #8 (fixtures/conformance/harness).
- v1 spans five epics; it is the first integration milestone, not a single issue.
