# @firemedia/skyfire-player

Turnkey in-browser DVB TV player. Wraps WebCodecs hardware video decode, WASM
AC-3/E-AC-3 audio decode, audio-master A/V sync, and DVB-subtitle overlay. Ships
two layers: a polished **`<skyfire-player>` Web Component** (controls, track/subtitle
menus, fullscreen, picture-in-picture, diagnostics) and the lower-level
**`SkyfirePlayer`** engine class it wraps. Built on
[`@firemedia/skyfire-core`](../core/README.md).

## Install

```bash
npm install @firemedia/skyfire-player
```

`@firemedia/skyfire-core` is a peer dependency — it will be installed automatically as a
declared dependency.

## Web Component (recommended)

Import once to register the element, then drop the tag anywhere — framework-agnostic
(plain HTML, React, Svelte). All UI lives in a Shadow DOM (encapsulated styles).

```js
import "@firemedia/skyfire-player/skyfire-element.js";
```

```html
<skyfire-player src="/live/channel1.ts" controls="full" muted></skyfire-player>
```

**Attributes:** `src` (stream URL; changing it hot-swaps the channel), `controls`
(`full` | `minimal` | `none`), `muted`, `autoplay`, `audio-lead` (seconds of buffer
ahead of the play clock, default 10).

**Controls (`full`):** play/pause, volume + mute, audio-track menu (language + codec),
subtitle menu (Off + per-language), picture-in-picture, fullscreen, diagnostics overlay.
Auto-hides on idle.

**Properties/methods:** `play()`, `pause()`, `selectAudio(pid)`, `selectSubtitle(pid|null)`,
`tracks`. **DOM events:** `sf-tracks`, `sf-stats`, `sf-error`, `sf-ended` (detail carries
the engine payload). Also mirrors stats to `window.__sfStats`.

```js
const el = document.querySelector("skyfire-player");
el.addEventListener("sf-tracks", (e) => console.log("tracks", e.detail));
el.addEventListener("sf-error", (e) => console.error(e.detail));
el.setAttribute("src", "/live/channel2.ts"); // switch channel (clean teardown + reload)
```

### Fullscreen

`enterFullscreen()` / `exitFullscreen()` / `toggleFullscreen()` return promises
and reject if the browser refuses (most browsers require a user gesture). The
`sf-fullscreenchange` event carries `{ fullscreen, mode }`.

On iPhone Safari there is no `Element.requestFullscreen` — WebKit only promotes
`<video>` elements, and skyfire renders to a `<canvas>` — so the player falls
back to a fixed-position overlay and reports `mode: "pseudo"`. It fills the
viewport but does not hide Safari's own chrome.

Prefer the element for embedding. Use the `SkyfirePlayer` class below when you need to
drive decode/sync yourself and build your own UI.

## Quick start (low-level `SkyfirePlayer` class)

```html
<!-- The player draws into this canvas; the subtitle overlay is inserted after it.
     The parent element must have position:relative (or any non-static) so the
     overlay tracks the canvas. -->
<div style="position:relative; display:inline-block;">
  <canvas id="player-canvas"></canvas>
</div>
```

```js
import { SkyfirePlayer } from "@firemedia/skyfire-player";

const canvas = document.getElementById("player-canvas");
const player = new SkyfirePlayer(canvas, {
  streamUrl: "/live/channel1.ts",
  // subtitlePid: 512,   // optional: auto-start DVB-sub compositing from PES 0
  // forceMse: false,    // optional: skip WebCodecs, use MSE fallback
  // muted: false,       // optional: start muted
});

// Listen for events before calling init().
player.on("tracks", (trackList) => {
  // trackList: { videoPid, videoCodec, audio: [{pid, kind, language, codec}], subtitles: [...] }
  console.log("tracks available", trackList);
  // Populate your own UI pickers here, then call player.selectAudio() / selectSubtitle().
});

player.on("stats", (s) => {
  // s: { decoded, drawn, dropped, w, h, aus, path, audioChunks, ..., status? }
  // window.__sfStats is NOT set by the library; set it in your app if your e2e needs it.
});

player.on("error", ({ message, cause }) => {
  console.error("playback error:", message, cause);
});

player.on("ended", (s) => {
  console.log("stream ended", s);
});

// Start: loads WASM, opens the stream, begins decoding.
await player.init();
```

## API

### `new SkyfirePlayer(canvas, opts)`

| Option | Type | Description |
|---|---|---|
| `streamUrl` | `string` | **Required.** URL of the progressive-H.264 MPEG-TS stream. |
| `audioPid` | `number?` | Pre-select an audio PID (applied when the track list arrives). |
| `subtitlePid` | `number?` | Pre-select a subtitle PID (compositing starts from the first PES). |
| `muted` | `boolean?` | Start with audio gain = 0. Default `false`. |
| `forceMse` | `boolean?` | Force the MSE video path, skipping the WebCodecs capability gate. |

### Methods

| Method | Description |
|---|---|
| `init()` | Load WASM, open the stream, start decoding. Returns a Promise that resolves at EOS. |
| `play()` | Resume playback after `pause()`. |
| `pause()` | Pause the audio worklet and signal the bridge. |
| `selectAudio(pid)` | Switch the active audio PID. |
| `selectSubtitle(pid \| null)` | Switch subtitle PID; `null` turns subtitles off. |
| `tracks()` | Returns the current `TrackList` or `null` if not yet received. |
| `on(event, cb)` | Subscribe to an event (see below). |
| `destroy()` | Tear down all resources: VideoDecoder, MediaSource, AudioContext, rAF, fetch. |

### Events

| Event | Payload | Description |
|---|---|---|
| `"tracks"` | `TrackList` | Fired once when the PAT/PMT is parsed and the track list is available. |
| `"stats"` | `object` | Fired on each decoded frame and on status changes. |
| `"error"` | `{ message, cause }` | Non-recoverable playback error. |
| `"ended"` | `object` | Stream finished (EOS). |

## Input stream contract

The server must deliver:

- **Video:** progressive H.264 (`frame_mbs_only_flag = 1`, per ADR 0008). Interlaced
  streams must be deinterlaced server-side — the browser WebCodecs implementation cannot
  decode 1080i. The NVENC transcoder in zenith produces this format.
- **Audio:** AC-3 or E-AC-3 (ETSI TS 102 366) on a separate PID, passed through
  untouched (no re-encoding). The WASM AC-3 decoder handles both stereo and 5.1 (with
  ITU-R BS.775 downmix to stereo when the output device cannot handle 5.1).
- **Subtitles:** DVB subtitles (EN 300 743) on a separate PID, passed through untouched.
- **PCR/PTS:** preserved by the server. The audio-master A/V sync clock depends on these.

See [issue #27](https://github.com/fishloa/rust-skyfire/issues/27) for the full
zenith↔skyfire stream contract.

## Subtitle overlay canvas

The player inserts a `<canvas>` sibling immediately after `this.canvas` in the DOM, styled
`position:absolute; top:0; left:0; width:100%; height:100%; pointer-events:none`. The
canvas's **parent element must have `position:relative`** (or any non-static positioning)
for the overlay to track the video. A wrapping `<div style="position:relative">` is the
simplest approach.

## Licence

MIT OR Apache-2.0
