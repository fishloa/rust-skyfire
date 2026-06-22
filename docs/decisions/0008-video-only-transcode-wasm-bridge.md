# 0008 — Video-only server transcode + skyfire WASM bridge client

- **Status:** Accepted (supersedes [0007](0007-transcode-to-hls-server-thin-client.md); restores the audio stance of [0006](0006-server-side-deinterlace-for-mobile.md))
- **Date:** 2026-06-22

## Context

ADR 0007 chose standard H.264+AAC HLS + a thin `hls.js`/native client. Revised
direction (2026-06-22): keep **audio bit-exact**, reuse skyfire's existing
WebCodecs + WASM + sync client, and transport **raw MPEG-TS** — abandoning the
in-browser software H.264 decoder (the server now delivers *progressive* H.264,
which WebCodecs decodes natively on every target).

The server re-encodes **only video**; everything else is carried untouched:

- **Video** → deinterlace + H.264 re-encode → **progressive** (NVENC, ADR 0006/7).
- **Audio** (AC-3 / E-AC-3) → **passthrough, untouched** — decoded client-side in WASM.
- **Subtitles** → separate-PID **DVB subtitling / teletext**, passthrough (confirmed:
  no embedded CEA-608/708-in-SEI on the target lineup, so no re-injection needed).
- **SI / data PIDs** → passthrough.
- **PCR / PTS** → preserved (original broadcaster timing; already A/V-synced).

## Decision

**Server (zenith):** a per-channel **video-transcoding adaptor** in the existing
`Sink<T>`/`Source<T>` chain, inserted before the per-channel `Scatterer`. It
replaces the video ES with progressive H.264 (`h264_cuvid -deint` → `h264_nvenc`,
in-process `rsmpeg`) and **passes through every other PID** (AC-3/E-AC-3,
DVB-subtitle/teletext, SI, PCR/PTS preserved). Output is the existing per-client
**TS** (zenith `HttpTsWorker`) — **not** CMAF/AAC.

**Client (skyfire WASM = the bridge):** browser owns presentation + control;
WASM parses/routes and decodes audio only.

- Browser owns: `<canvas>` + WebCodecs `VideoDecoder`, WebAudio `AudioWorklet`,
  subtitle/CC overlay, controls. **→ WASM commands:** `selectAudio(pid)`,
  `selectSubtitle(track|off)`, play/pause. **← WASM:** video AUs → `VideoDecoder`,
  PCM → WebAudio, subtitle cues → overlay, track-list + PTS clock.
- WASM: demux TS (`skyfire-ts` + `dvb-si`/`dvb-pes`); hand progressive H.264 AUs
  to JS (WASM does **not** decode video); decode the *selected* audio PID
  (`oxideav-ac3`) → PCM; parse DVB subtitles (`dvb-subtitle`) → cues; **audio
  flip** = re-point demux/decoder to a new PID (PTS continuity + decoder reset);
  audio-master sync from PCR/PTS.

## Consequences

- **"Audio never re-encoded" restored.** Only video is re-encoded.
- **oxideav-h264 fully dropped from the browser** — progressive H.264 is
  WebCodecs-decodable on Chrome/Firefox/Safari/iOS 17+ (interlaced was the *only*
  blocker; verify once on a real progressive feed on iOS). Kept upstream as a
  conformance asset, not a skyfire runtime dependency.
- **skyfire crates back on the mainline:** `skyfire-ts` (demux), `skyfire-ac3`
  (WASM AC-3, wraps `oxideav-ac3`), `skyfire-sync` (PCR/PTS master clock),
  `skyfire-wasm` (the bridge API), `web/` (WebCodecs + WebAudio + overlay UI).
  New reuse: `rust-dvb` `dvb-subtitle` (DVB-SUB bitmap regions) and possibly
  `dvb-vbi` (teletext).
- **Transport = MPEG-TS to the browser** (chunked HTTP-TS), skyfire demuxes. No
  HLS dependency for the WASM client; native HLS remains available as a fallback.
- **zenith#986 rescoped** to video-only→TS (drop the AAC transcode + CMAF mux).
- Embedded-SEI CEA-608/708 captions are **out of scope** (would require server
  extract+re-inject across the NVENC re-encode); only separate-PID DVB
  subtitling/teletext is supported.
