# 0004 — MPEG-TS demux strategy (reuse dvb-si + new dvb-pes)

- **Status:** Accepted
- **Date:** 2026-06-18

## Context

`skyfire-ts` must turn raw MPEG-TS into elementary streams + PTS. `rust-dvb`'s
`dvb-si` (our own crate) already covers the TS packet and PSI layers, and is
`no_std + alloc` → WASM-clean:

- `TsResync` — byte-align a raw capture to 188-byte packets (188/192/204 stride).
- `TsPacket` / `TsHeader` / `AdaptationField` / `Pcr` — packet parse + PCR clock.
- `SectionReassembler`, `SiDemux` — PSI sections → PAT/PMT, stream types, ES PIDs
  (incl. AVC/HEVC/AC-3/E-AC-3 descriptors).

The gap: `dvb-si` has **no PES depacketization or PTS/DTS extraction**. Existing
crates that do (`mpeg2ts-reader`, `mpeg2ts`) bundle their *own* TS packet layer —
using one means dropping `dvb-si` reuse, and neither targets `no_std`/WASM.

## Decision

1. **Reuse `dvb-si`** for resync, packet parse, PCR, and PSI/PAT/PMT → channel
   map. Do **not** use `dvb-stream` (tokio/async — wrong for WASM).
2. **New crate `dvb-pes`** in the `rust-dvb` workspace: standalone, spec-based
   (ISO/IEC 13818-1 §2.4.3.6/§2.4.3.7), `no_std + alloc`, depends on `dvb-common`.
   Input: per-PID TS payload + PUSI. Output: assembled PES packets with PTS/DTS
   (33-bit, 90 kHz) and ES access-unit bytes. Reusable by zenith and others.
3. **`skyfire-ts` wires** `dvb-si` + `dvb-pes` → ES + PTS per video/audio PID.
4. **No HLS for v1** — fixtures are raw `.ts`. HLS playlist parsing belongs to
   epic #7.

`mpeg2ts-reader` / `mpeg2ts` are kept only as **reference implementations** to
validate `dvb-pes` golden bytes/PTS against.

## Consequences

- `skyfire-ts` gains an upstream dependency on `dvb-pes` (path-dep during dev,
  published before release). The `dvb-pes` crate itself is **rust-dvb work**, not
  a Skyfire issue.
- Demux tests are golden: PTS values + first ES bytes from each fixture,
  cross-checked against a reference crate.
- Epic #1 splits into channel-mapping (PSI) and ES-extraction (PES) work units.
