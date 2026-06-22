# Architecture Decision Records (ADRs)

This directory records **why** Skyfire is built the way it is. One file per
decision, numbered, immutable once `Accepted` — to change a decision, write a new
ADR that supersedes the old one (and mark the old one `Superseded by NNNN`).

## Index

| # | Title | Status | Date |
|---|---|---|---|
| [0001](0001-browser-and-platform-support.md) | Browser & platform support, codec-gating policy | Accepted | 2026-06-18 |
| [0002](0002-delegation-working-practice.md) | Delegation working practice (exit-gated issues, crush, cost ledger) | Accepted | 2026-06-18 |
| [0003](0003-first-release-and-test-harness.md) | First release (v1) scope & test harness | v1 scope superseded by 0008 | 2026-06-18 |
| [0004](0004-mpeg-ts-demux-strategy.md) | MPEG-TS demux: reuse dvb-si + new dvb-pes crate | Accepted | 2026-06-18 |
| [0005](0005-interlaced-h264-webcodecs-wall.md) | Interlaced H.264 needs WASM software decode (oxideav-h264) | Superseded by 0006 | 2026-06-19 |
| [0006](0006-server-side-deinterlace-for-mobile.md) | Server-side deinterlace — only universal in-browser path for 1080i | Accepted | 2026-06-21 |
| [0007](0007-transcode-to-hls-server-thin-client.md) | GPU transcode-to-HLS server (H.264+AAC, in-process libav) + thin client | Superseded by 0008 | 2026-06-21 |
| [0008](0008-video-only-transcode-wasm-bridge.md) | Video-only server transcode (TS) + skyfire WASM bridge client | Accepted | 2026-06-22 |

## Format

Each ADR has: **Status** (Proposed / Accepted / Superseded), **Context** (forces
at play), **Decision** (what we chose), **Consequences** (what this commits us to).
Keep them short. Cite specs/APIs where a claim is technical.

## Rules

- Never edit an `Accepted` ADR's decision — supersede it with a new one.
- Every new decision lands here **and** updates this index table in the same change.
- Objectives & roadmap live in [`../OBJECTIVES.md`](../OBJECTIVES.md), not here.
