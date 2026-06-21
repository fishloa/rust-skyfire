# 0006 — Server-side deinterlace is the only universal in-browser path for 1080i

- **Status:** Accepted (supersedes the playback strategy of [0005](0005-interlaced-h264-webcodecs-wall.md))
- **Date:** 2026-06-21

## Context

ADR 0005 established that **WebCodecs cannot decode interlaced H.264** and chose a
WASM software decoder (`oxideav-h264`) as the zero-transcode answer. That decoder
now works (1463/1463 conformance, ~53 dB; 1080i PAFF decodes correctly). But two
new facts, both probed on real hardware, change the platform calculus:

**1. Software 1080i decode is desktop-only.** Native release profiling: ~4 fps
single-thread; stage split parse/CABAC 11% serial, reconstruct 71%, deblock 18% →
~89% parallel, Amdahl ceiling ~9×. Realtime 25 fps needs ~6–8 cores. Desktop
(24-core) reaches it; an iPhone (`hardwareConcurrency = 4`) cannot.

**2. There is NO in-browser hardware path for interlaced H.264 — on any browser.**
Full probe matrix (deployed to `tv.icomb.place/skyfire-probe/`, run on real
devices):

| Stream | Desktop Chrome WebCodecs | iOS Safari WebCodecs | iOS Safari **native HLS** |
|---|---|---|---|
| Interlaced 1080i (PAFF) | ❌ decoder failure | ❌ decoder failure | video **blank**, audio ✓ |
| Deinterlaced → progressive | ✅ HW | (n/a) | **video ✓ + audio ✓ (HW)** |

iOS native HLS *parses* interlaced (reports 1920×1080, plays the timeline,
decodes E-AC-3 audio) but renders **blank video** — VideoToolbox via the HLS path
will not display field-coded PAFF. The same content **deinterlaced to progressive**
plays perfectly: HW video **and** HW E-AC-3 audio, in-browser, low power.

**Conclusion:** "zero server-side transcode" (the OBJECTIVES premise) is
*fundamentally incompatible* with in-browser **hardware** playback of interlaced
1080i, because no browser hardware-decodes interlaced at all. One of the two must
give. For mobile — where software decode is not an option — the only viable path
is to remove the interlacing **before** the browser sees it.

## Decision

Accept **one minimal, video-only deinterlace transcode on the server** as the
universal delivery path. Audio (E-AC-3 / AC-3) is **copied**, never transcoded.

- **Server:** DVB-S2 TS → demux → **deinterlace + re-encode video to progressive
  H.264** (`yadif` + `libx264`, or HW deinterlace) → HLS segments, **E-AC-3
  passed through** (`-c:a copy`).
- **iOS / Safari:** native `<video>` HLS — HW progressive H.264 + HW E-AC-3. Done.
- **Desktop Chrome/Firefox** (no native HLS, no native AC-3): `hls.js`/MSE for the
  progressive H.264 (HW) + the **WASM AC-3 decoder** (`oxideav-ac3`, already
  shipped) for audio. No software *video* decode needed — the stream is now
  HW-decodable everywhere.

This is a **video-only** transcode: the one thing browsers can't do (de-field the
picture) is done once on the server; everything browsers *can* do natively (HW
H.264, HW/WASM audio) stays at the edge.

## Consequences

- **The WASM software H.264 decoder leaves the critical playback path.**
  `oxideav-h264` is no longer required to play 1080i in a browser. It is retained
  as: (a) the only true **zero-transcode** option for desktop, kept behind a
  capability/preference switch; (b) a verified conformance asset. Epic #6's GPU
  *weave* deinterlace is also off the mobile path (server does it instead).
- **OBJECTIVES primary goal is amended:** from "zero server-side transcode" to
  "**minimal transcode — video deinterlace only, audio never re-encoded**." The
  spirit (no full transcode, no audio re-encode, codecs preserved) holds; the
  letter ("zero") does not survive contact with interlaced hardware reality.
- New server component: a continuous deinterlace+HLS-segmenting pipeline fed by
  the DVB receiver (today proven one-shot against fixtures via `ffmpeg yadif ->
  libx264 -c:a copy -f hls`; productionising the live pipeline is follow-up work).
- Desktop audio keeps the WASM AC-3 decoder rather than a second (audio) transcode
  — preserves "audio never re-encoded" and reuses shipped code. (Alternative —
  server E-AC-3→AAC for a pure-native desktop client — rejected to avoid a second
  transcode.)
- Bandwidth/quality: `libx264` progressive at broadcast bitrate is comparable;
  deinterlace halves temporal artefacts at the cost of one re-encode generation.
