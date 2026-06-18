# Skyfire test fixtures

Real captured MPEG-TS slices (from a DVB-S2 receiver) for demux/decode tests.

| file | content |
|---|---|
| `h264-25fps.ts` | small H.264 25fps sample — video PID 0x0100 only (no audio in this slice) |
| `m6-clean.ts`   | French TNT M6 HD — H.264 1080i/PsF. **Note:** this captured slice carries only the video PID (0x0100); no audio PID present, despite M6 broadcasting AC-3/E-AC-3. |
| `gulli-15s.ts`  | Gulli HD — H.264 PsF (video 0x0100) + **E-AC-3** audio (0x0101, 48 kHz stereo) |
| `gulli.eac3`    | raw E-AC-3 elementary stream extracted from `gulli-15s.ts` (`ffmpeg -map 0:a:0 -c:a copy`) — for decoder tests decoupled from demux. Starts with `0x0B77`. |

These exercise the codecs Skyfire targets: H.264 video + AC-3/E-AC-3 audio. `gulli.eac3`
is the audio-decode fixture; verified PIDs/codecs were confirmed via demux (issue #20).
