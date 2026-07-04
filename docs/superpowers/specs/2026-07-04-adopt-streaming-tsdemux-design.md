# Adopt transmux StreamingTsDemux (Part 2) — design

**Date:** 2026-07-04
**Goal:** Replace skyfire's bespoke MPEG-TS demux with transmux 0.12's
`StreamingTsDemux`, DemuxEvent-native. One demux dependency; per-sample audio
PTS, PCR, and discontinuity come from the IR. Decoders / compositor / sync are
unchanged consumers.

## Why now

transmux 0.12.0 ships everything skyfire needs (all confirmed in source):
- `StreamingTsDemux::{new, feed(&[u8]), poll_event() -> Option<DemuxEvent>, finish()}` — incremental, any chunk size, resync internal (#555).
- `DemuxEvent::{TrackAdded(Track), TrackUpdated(Track), Sample{track_id,sample}, Pcr(PcrSample), Discontinuity{pid}}`.
- `Sample.source_timing: Option<SourceTiming{pts,dts}>` — per audio AU, 90 kHz unwrapped (#556). Audio split per syncframe, non-zero `duration`.
- `CodecConfig::Data{stream_type, descriptors, carriage}` — DVB-sub `0x06` → PES Data track with PTS + ES_info descriptors (#576).
- `TrackSpec{track_id, timescale, config, source_pid: Option<u16>, es_info_descriptors: Vec<u8>}` — PID + raw ES_info on every TS track (#582); built via `TrackSpec::new(...).with_source(pid, descriptors)`.
- AVC samples are **length-prefixed** (`annexb_to_length_prefixed`); `CodecConfig::Avc{config: avcC, width, height}` carries a ready avcC with high-profile chroma/bit-depth populated (#563 fix).
- `TrackSpec`/`Sample`/`CodecConfig` are `#[non_exhaustive]` (#580) — construct via constructors, match with a wildcard arm.

## Maximize transmux adoption ("swallow as much as you can")

Push transmux to own **every** container/codec-config/segmenting concern; skyfire
keeps only what transmux is architecturally not (a decoder / a DVB-sub renderer /
the sync + browser layer).

**Adopt from transmux:**
- `StreamingTsDemux` — all TS demux (PSI, PES, PID, PCR, discontinuity, resync).
- `CodecConfig::Avc` (avcC + dims) + length-prefixed samples — video config +
  chunk bytes. Delete skyfire's `h264_config.rs`.
- `sps::rfc6381_avc1` / avcC — WebCodecs `VideoDecoder.configure` description +
  `isConfigSupported` codec string.
- **`Segmenter`** (`new(tracks, timescale, target_secs)` / `push(track_id, sample)`
  / `take_ready()` / `flush()` / `mark_discontinuity()`) — the **MSE-fallback CMAF
  segment building**. Replaces skyfire's hand-rolled GOP-boundary detection +
  DTS-delta duration + `composition_offset` math + per-GOP `build_media_segment`
  in `take_video_media_segment`. It cuts on anchor keyframes and omits Data tracks
  itself; `mark_discontinuity()` is driven by `DemuxEvent::Discontinuity`.
- `CodecConfig::Ac3/Eac3` (channel_count/sample_rate/acmod/lfeon) — pre-configure
  WebAudio + inform the downmix layout, instead of skyfire re-deriving it.

**Stays skyfire (transmux is out of scope here — NOT missing features):**
- AC-3/E-AC-3 → PCM decode (`skyfire-ac3`/oxideav-ac3); MP2 (`skyfire-mpa`).
- DVB-subtitle EN 300 743 → RGBA render (`dvb_subtitle` parse + skyfire compositor);
  transmux carries it opaque only.
- AudioClock A/V sync; WebCodecs/WebAudio/canvas (browser).

**File gaps as issues:** if implementation hits anything skyfire needs that transmux
lacks (e.g. a Segmenter control, a track-list-complete signal), file a
rust-broadcast issue with an ungameable acceptance (like #582) rather than
working around it — then adopt once it ships.

## Architecture (DemuxEvent-native)

skyfire stops parsing TS. `StreamingTsDemux` is the sole demux; the bridge/engine
route on `DemuxEvent` + `track_id`. Flow:

```
bytes → StreamingTsDemux.feed → poll_event loop:
  TrackAdded(Track)  → register track (track_id → {pid, kind, codec, language}), emit track-list
  Sample{track_id}   → dispatch by registered kind:
                         video    → WebCodecs (length-prefixed data + avcC) / MSE build_media_segment
                         audio    → skyfire-ac3 decode(data) @ source_timing.pts → WebAudio
                         subtitle → dvb_subtitle parse(data) @ source_timing.pts → compositor → cues
  Pcr(PcrSample)     → AudioClock PCR input
  Discontinuity{pid} → reset decode state at the boundary (HLS segment splice)
```

Track selection stays **PID-addressable** (`select_audio(pid)`, `select_subtitle(pid)`
unchanged in the JS API): the bridge keeps a `track_id → source_pid` map from
`TrackAdded`, so the existing PID-based selection and routing are preserved on top
of an event core.

## Track metadata (language / kind)

transmux does not parse descriptors. skyfire parses `spec.es_info_descriptors`
(kept `dvb_si` dependency, descriptor-parsing only) per track:
- audio ISO-639 language ← descriptor tag `0x0A`; AC-3 vs E-AC-3 ← `CodecConfig` variant.
- subtitle kind ← DVB-subtitling `0x59` vs teletext `0x56`; language ← `0x0A`.

A small `track_meta(spec) -> {pid, kind, codec, language}` helper in skyfire-ts
owns this. No PAT/PMT reassembly in skyfire anymore — transmux did it.

## Module changes

- **`crates/skyfire-ts/src/lib.rs`** — gut the bespoke demux: delete `packet_pid`/
  `payload_unit_start`/`packet_payload`, `EsDemux`, the `SiDemux`/PAT/PMT loop,
  `build_channel_map*`, `probe`'s TS-walking, and the `AccessUnit`/`ChannelMap`/
  `TimedAccessUnit` types. Add a thin demux wrapper exposing `feed`/`poll_event`
  (re-exporting `transmux::DemuxEvent`) + the `track_meta` descriptor helper +
  the track-list/kind enums the bridge needs.
- **`crates/skyfire-ts/src/h264_config.rs`** — **delete**. transmux `CodecConfig::Avc`
  (avcC + dims) and length-prefixed samples replace it. Golden avcC tests move to
  asserting transmux's recovered avcC on the fixtures.
- **`crates/skyfire-ts/src/subtitle_compositor.rs`** — **keep** (EN 300 743 render).
  Its `feed_pes(pid, pts, &field)` input is unchanged; still fed from Sample data.
- **`crates/skyfire-core/src/lib.rs`** — `Engine` reworked onto `StreamingTsDemux`
  + event routing; drop `TsResync`/`EsDemux`/`ChannelMap`/`probe`. Keep the public
  `video_units`/`channel`-style accessors' behaviour where tests depend on it, but
  re-expressed over events.
- **`crates/skyfire-wasm/src/lib.rs`** — `SkyfireBridge.feed` drives StreamingTsDemux;
  `drain`/`route_access_units` replaced by an event-poll router; track list from
  `TrackAdded`; wire `Pcr`→clock, `Discontinuity`→reset. AC-3/MP2/subtitle dispatch
  unchanged downstream of the router.
  - **WebCodecs path**: hand video `sample.data` (already length-prefixed) as
    `EncodedVideoChunk`s; configure from `spec.config` avcC + `rfc6381_avc1`.
  - **MSE path**: replace the hand-rolled `take_video_media_segment` (GOP detection
    + duration + `composition_offset` + per-GOP `build_media_segment`) with a
    `transmux::Segmenter` — `init_segment()` once, `push(track_id, sample)` per
    video Sample, `take_ready()` for finished CMAF media segments, `flush()` at EOS,
    `mark_discontinuity()` on `DemuxEvent::Discontinuity`. Delete the bespoke
    `video_init_segment`/`take_video_media_segment` GOP/duration code and the
    `TrackSpec{...}` literal.
- **Cargo.toml (ts, wasm, core)** — bump `transmux 0.10 → 0.12`; **drop `dvb-pes`
  and `mpeg-ts`** (subsumed by StreamingTsDemux); keep `dvb-si` (descriptor parse)
  + `dvb-subtitle` (payload).

## Discontinuity → HLS boundary reset

`DemuxEvent::Discontinuity{pid}` (and `PcrSample.discontinuity`) fire at TS
timeline breaks — exactly the HLS segment splices Build A deferred. On a video-PID
discontinuity the bridge flushes the decoder and re-arms keyframe wait; on the
audio PID it re-anchors the clock. This closes the Build A gap (see the HLS spec).

## Error handling

`poll_event` never blocks; malformed/partial input yields no events until more is
fed (`finish()` flushes trailing AUs at EOS). Unknown/opaque streams surface as
`Data` tracks (never dropped). A `CodecConfig` match always carries a wildcard arm
(`#[non_exhaustive]`).

## Testing / exit criteria

The existing behavioural oracles are the equivalence gate — they assert on
**output**, so they hold across the internal rewrite and prove no regression:
- `skyfire-ts`/`core`/`wasm` nextest: gulli-15s track enumeration (video pid+codec,
  audio langs, sub kinds), audio PTS monotonic, **audio PCM byte-match vs
  `gulli.eac3`**, video AU count + IDR flags + PTS, france2 DVB-sub composite RGBA,
  orf2/ac3-51/eac3-51 decode + downmix, mp2 tone. All must stay green.
- Golden avcC tests re-based on transmux's recovered avcC for gulli/h264-25fps.
- New: a track built from `TrackAdded` has the correct `source_pid` + parsed
  language on france2 (fre/fra/qaa) and correct sub kind — the #582 payoff.
- New: a synthesized/real discontinuity triggers the decoder reset path.
- **CI gate**: `cargo fmt --all --check`, `clippy --workspace --all-targets -D warnings`,
  `build`, `nextest` all green.
- **Browser e2e** (authoritative): existing WebCodecs + HLS-of-TS + MSE tests green;
  the HLS discontinuity path exercised.

## Out of scope

No JS API change (`select_audio`/`select_subtitle` stay PID-based). No zenith
change. No fMP4/CMAF delivery (that's Build B). `StreamingTsHlsSegmenter` (#571)
not adopted here.
