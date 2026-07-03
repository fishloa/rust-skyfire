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

## Use skyfire in the browser

### Turn-key player: `@skyfire/player`

The simplest way to embed a skyfire player in your page is to install
[`@skyfire/player`](packages/player/) from npm:

```bash
npm i @skyfire/player
```

Then create a canvas and initialize the player:

```html
<canvas id="video" width="1280" height="720"></canvas>

<script type="module">
  import { SkyfirePlayer } from "@skyfire/player";

  const canvas = document.getElementById("video");
  const player = new SkyfirePlayer(canvas, {
    streamUrl: "https://your-server/path/to/progressive-h264.ts",
  });

  await player.init();
  player.play();

  // Listen for events
  player.on("tracks", (trackList) => {
    console.log("Audio tracks:", trackList.audio);
    console.log("Subtitles:", trackList.subtitles);
  });

  player.on("error", (err) => console.error(err));
  player.on("ended", () => console.log("Stream ended"));

  // Runtime controls
  player.selectAudio(101); // select audio by PID
  player.selectSubtitle(102); // select subtitle by PID
  player.selectSubtitle(null); // disable subtitles
  player.pause();
  player.play();
  player.destroy(); // cleanup
</script>
```

**Events:** `tracks`, `stats`, `error`, `ended`.

**Methods:** `play()`, `pause()`, `selectAudio(pid)`, `selectSubtitle(pid | null)`,
`tracks()`, `on(event, callback)`, `destroy()`.

### Low-level API: `@skyfire/core`

If you need finer control over video/audio rendering or want to wire your own
WebCodecs/WebAudio, use [`@skyfire/core`](packages/core/) directly:

```js
import { initSkyfire, SkyfireBridge } from "@skyfire/core";

// Initialize the WASM runtime
await initSkyfire();

// Create a bridge to demux TS + decode AC-3/E-AC-3
const bridge = new SkyfireBridge();

// Feed TS data (e.g., from fetch)
const response = await fetch("https://your-server/stream.ts");
const reader = response.body.getReader();
let chunk;
while (!(chunk = await reader.read()).done) {
  bridge.feed(chunk.value);

  // Drain video AUs, hand to WebCodecs VideoDecoder
  for (const au of bridge.take_video_aus()) {
    videoDecoder.decode(new EncodedVideoChunk({
      type: au.isKeyframe ? "key" : "delta",
      timestamp: au.ptsTicks ? Number(au.ptsTicks) : 0,
      data: au.bytes,
    }));
  }

  // Drain PCM audio, feed to WebAudio AudioWorklet
  for (const pcm of bridge.take_audio_pcm()) {
    audioWorklet.port.postMessage({ samples: pcm.samples });
  }

  // Drain subtitle cues for canvas blitting
  for (const cue of bridge.take_subtitle_cues()) {
    renderSubtitleCue(cue);
  }
}

bridge.flush();
```

### Input stream contract

Skyfire expects a **video-only-transcoded MPEG-TS** stream with the following
constraints:

- **Video:** progressive H.264 with `frame_mbs_only_flag=1` (no interlaced
  frames). The server must deinterlace any incoming interlaced content.
- **Audio:** untouched AC-3 or E-AC-3 (ETSI TS 102 366), stream-selectable by
  PID.
- **Subtitles:** DVB subtitles (EN 300 743) or teletext, stream-selectable by
  PID, as separate PIDs.
- **Timing:** PAT/PMT present; PCR and PTS intact for A/V synchronisation.

See [epic #27](https://github.com/fishloa/rust-skyfire/issues/27) and
[ADR 0008](docs/decisions/0008-video-only-transcode-wasm-bridge.md) for the full
stream contract and rationale.

### Publishing `@skyfire` packages (maintainers only)

**Never publish from the CLI.** Publishing is automated via CI:

1. Create the `@skyfire` npm organisation (if not already done).
2. Add an `NPM_TOKEN` secret to the GitHub repo (Settings → Secrets and
   variables → Actions → New repository secret).
3. Tag a release and push:
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
4. The `release-npm` GitHub Actions workflow (`.github/workflows/release-npm.yml`)
   will:
   - Build the WASM bridge (`skyfire-wasm` with `wasm-pack --target bundler`)
   - Publish `@skyfire/core` to npm
   - Publish `@skyfire/player` to npm

**Fallback:** if the `@skyfire` npm organisation is unavailable, rename both
package names to `skyfire-tv` (config-only change in the `package.json` files)
and adjust the import paths in your code. The WASM and implementation remain
unchanged.

## Licence

MIT OR Apache-2.0.
