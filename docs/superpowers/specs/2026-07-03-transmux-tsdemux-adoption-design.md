> **SUPERSEDED** — historical; the shipped design is
> [docs/superpowers/specs/2026-07-04-adopt-streaming-tsdemux-design.md](2026-07-04-adopt-streaming-tsdemux-design.md).
> Types named below (EsDemux/TsResync/h264_config/AccessUnit/ChannelMap) were
> removed in Part 2.

# transmux 0.10 TsDemux / Media-IR adoption — evaluation

**Date:** 2026-07-03
**Outcome:** PARKED — blocked upstream. No implementation. Revisit when
rust-broadcast #555/#556/#557 land.

## Question

Modernization "part 2": replace skyfire's bespoke MPEG-TS demux and segment
building (`skyfire-ts` / `skyfire-wasm`) with transmux 0.10's `TsDemux` +
Media-IR (`Media`/`Track`/`Sample`) hub.

## What skyfire actually does today

The demux is **not** hand-rolled — it is thin glue over the right deps:

| Concern | Owner |
|---|---|
| PAT/PMT | `dvb_si::SiDemux` |
| PES reassembly + PTS/DTS | `dvb_pes::PesAssembler` / `PesPacket` |
| TS packet alignment / resync | `mpeg_ts::resync::TsResync` |
| DVB-subtitle segments | `dvb_subtitle` |
| H.264 avcC + Annex-B↔fMP4 + fMP4 init/media segments | **`transmux`** (already) |

Genuinely bespoke: ~30 lines of TS-header bit-twiddling (`packet_pid`,
`payload_unit_start`, `packet_payload`), the per-PID `EsDemux` routing loop,
`ChannelMap`/StreamType→codec mapping, GOP-boundary + DTS-delta duration calc,
and the EN 300 743 subtitle **compositor** (rendering, which transmux does not
address at all). Seam type between crates: `skyfire_ts::AccessUnit { pid,
pts_ticks, dts_ticks, es_bytes }`.

## Why TsDemux can't replace it (transmux 0.10)

1. **Whole-buffer, not streaming.** `TsDemux::demux(&[u8]) -> Media` consumes one
   complete slice. Skyfire is a live player feeding 4096-byte chunks with no EOF.
   Fundamental mismatch. → rust-broadcast **#555**.
2. **Per-sample audio PTS lost.** AC-3/E-AC-3 samples are built with
   `Sample::from_raw(data, 0)` (duration 0); only the first sample's DTS is kept
   as `Track::start_decode_time`. Skyfire's audio-master A/V sync clock needs the
   PTS of every audio AU. → rust-broadcast **#556**.
3. **DVB-subtitle + PCR dropped.** No stream_type mapping for subtitle/teletext
   private PES (silently skipped), no IR variant for them, and adaptation-field
   PCR is never parsed. Skyfire needs raw subtitle PES + PTS and PCR. →
   rust-broadcast **#557**.

The one bespoke area transmux could own (segment building) it already owns via
`TrackSpec`/`Sample::from_annexb`/`build_init_segment`/`build_media_segment`.
Wrapping those calls in the `Media`/`Track` IR would be cosmetic.

## Decision

Net of adoption today = trade working, browser-verified capability (streaming +
audio sync + subtitles) for regressions, with ~zero gain on the already-delegated
segment layer. **Do not adopt now.** File the three gaps upstream (#555/#556/#557)
and revisit adoption only after all three land. Tracked in
[OBJECTIVES.md](../../OBJECTIVES.md).
