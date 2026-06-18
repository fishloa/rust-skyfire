# rust-skyfire — an in-browser DVB TV player, built by delegated open models

**Skyfire** plays live satellite TV **in the browser** by decoding the raw
MPEG-TS on the client — no server-side transcoding. It is the sibling client to
[zenith](https://github.com/fishloa/zenith) (the DVB-S2 receiver): zenith serves
clean per-channel TS/HLS, Skyfire decodes and renders it.

## What this project is

Two things at once:

1. **A real in-browser player.** The goal is to play the full satellite lineup
   (H.264 + H.265 video, AC-3 / E-AC-3 audio, MPEG-TS) in any modern browser,
   **with zero server transcode** — the client does the work. This is the way
   around the "browser can't play this" problem (no MSE codec support for AC-3,
   no clean path for 1080i/PsF), without re-encoding on the server.

2. **An experiment in AI-orchestrated engineering** (same model as `rust-ac4`):
   [Claude Code](https://claude.com/claude-code) orchestrates — it writes
   spec-based briefs, launches cheaper open/external models (DeepSeek V4, etc.)
   to write the implementation, and independently verifies every result against
   a hard CI gate. Claude does **not** write the production code itself.

## Architecture — split the decode path

The browser has a HW video decoder but refuses AC-3 audio. So:

- **Video → WebCodecs `VideoDecoder`** — hardware-accelerated H.264 (universal)
  and H.265 (capable browsers). WASM cannot touch the GPU decoder, so video must
  go through WebCodecs, not WASM.
- **Audio → WASM AC-3 / E-AC-3 decoder → PCM → WebAudio `AudioWorklet`** — the
  one codec browsers won't decode. Audio is light, so software/WASM is fine.
- **Receiver → MPEG-TS/HLS demux** → elementary streams + PTS (reusing
  [`rust-dvb`](https://github.com/fishloa/rust-dvb) for PSI parsing).
- **A/V sync → audio is the master clock**: media time comes from PCM samples
  actually played (drift-free); video frames are presented / dropped / held
  against it.
- **Interlace** is mostly moot — most channels are progressive/PsF and decode
  clean; the few true-1080i ones get a GPU weave-deinterlace shader, no re-encode.

## Workspace

| crate | role |
|---|---|
| `skyfire-ts`   | MPEG-TS / HLS demux → ES + PTS (planned: `rust-dvb` `dvb-si` for PSI) |
| `skyfire-ac3`  | AC-3 / E-AC-3 decode → PCM |
| `skyfire-sync` | audio-master A/V sync clock |
| `skyfire-core` | engine — wires demux + audio decode + sync |
| `skyfire-wasm` | `wasm-bindgen` bindings for the browser shell |
| `skyfire-cli`  | native debug CLI (inspect a captured `.ts`) |
| `web/`         | thin JS/TS shell: WebCodecs video, `AudioWorklet`, canvas render |

## Status

Scaffold + contract tests. Real implementation is delegated per GitHub **epic**
and issue — see the open epics and `CLAUDE.md` for the orchestration model.

## Licence

MIT OR Apache-2.0.
