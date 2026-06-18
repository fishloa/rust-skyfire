# Skyfire test fixtures

Real captured MPEG-TS slices (from the zenith DVB receiver) for demux/decode tests.

| file | content |
|---|---|
| `h264-25fps.ts` | small H.264 25fps sample — quick framing/demux tests |
| `m6-clean.ts`   | French TNT M6 HD — H.264 1080i/PsF + AC-3/E-AC-3 audio |
| `gulli-15s.ts`  | Gulli HD — H.264 progressive-in-interlaced (PsF), E-AC-3 audio |

These exercise the codecs Skyfire targets: H.264 video + AC-3/E-AC-3 audio in MPEG-TS.
