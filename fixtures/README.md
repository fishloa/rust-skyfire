# Skyfire test fixtures

Real captured MPEG-TS slices (from a DVB-S2 receiver) for demux/decode tests.

| file | content |
|---|---|
| `h264-25fps.ts` | small H.264 25fps sample — video PID 0x0100 only (no audio in this slice) |
| `h264-mse.ts`   | Conformant Main/L3.1 H.264 for MSE path testing |
| `gulli-15s.ts`  | Gulli HD — H.264 PsF (video 0x0100) + **E-AC-3** audio (0x0101, 48 kHz stereo) |
| `gulli.eac3`    | raw E-AC-3 elementary stream extracted from `gulli-15s.ts` (`ffmpeg -map 0:a:0 -c:a copy`) — for decoder tests decoupled from demux. Starts with `0x0B77`. |
| `gulli-prog.ts` | Gulli HD, progressive H.264 + E-AC-3, for e2e WebCodecs tests |
| `france2-8s.ts` | France 2 HD — progressive H.264 + AC-3, 8 s slice |
| `gulli.m3u8`    | HLS playlist referencing TS segments of `gulli-prog.ts` |
| `orf2-ac3-51.ts` | ORF 2 HD (zenith `/stream/orf-2`, ~2 s) — H.264 + **base AC-3 5.1** (6ch, 5.1-side) + MP2 stereo + teletext. Real 5.1 AC-3 for the multichannel decode/downmix path (#43/#39). |
| `ac3-51.ts` / `eac3-51.ts` | synthetic ffmpeg-generated 5.1 AC-3 / E-AC-3 (Main-profile video + 6ch tone) — small CI fixtures for the downmix + multichannel decode tests. |
| `mp2-tone.ts`   | synthetic MPEG Layer II (MP2) stereo tone |

These exercise the codecs Skyfire targets: H.264 video + AC-3/E-AC-3 audio. `gulli.eac3`
is the audio-decode fixture; verified PIDs/codecs were confirmed via demux (issue #20).
