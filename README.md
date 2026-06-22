# rust-skyfire — an in-browser DVB TV player, built by delegated open models

**Skyfire** plays live satellite TV **in the browser**. The upstream server
(zenith) re-encodes **video only** — deinterlacing true-1080i to progressive
H.264 — and re-muxes MPEG-TS with **audio, subtitles, SI and timing untouched**.
Skyfire's WASM bridge then demuxes that TS and decodes it in the browser:
WebCodecs for the (now progressive) video, WASM for the AC-3/E-AC-3 audio.

## What this project is

Two things at once:

1. **A real in-browser player.** The goal is to play the full satellite lineup
   (H.264 video, AC-3 / E-AC-3 audio, DVB subtitles, MPEG-TS) in any modern
   browser. The server re-encodes **only** the one thing the browser truly can't
   handle — interlaced 1080i video (deinterlace → progressive H.264) — and leaves
   audio, subtitles and timing bit-exact. The browser then does the rest: WebCodecs
   decodes the progressive video, a WASM decoder handles AC-3 (which no browser
   plays natively except Safari). Audio is never re-encoded.

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
- **Interlace** is handled **server-side**: true-1080i is deinterlaced to
  progressive H.264 (NVENC) before the browser ever sees it — no browser
  hardware-decodes interlaced H.264, so this is the only universal path. The
  client only ever receives progressive video. (See
  [ADR 0008](docs/decisions/0008-video-only-transcode-wasm-bridge.md).)

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
