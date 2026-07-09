# Sophisticated player — `<skyfire-player>` Web Component — design (Phase 2b)

**Date:** 2026-07-07
**Status:** Draft (approved to proceed)

## Why

The headless `SkyfirePlayer` engine works (Phase 2a fixed the stall). But consumers
(zenith's frontend, any embedder) have to hand-build all the UI — the current
`web/index.html` is a bare demo with a raw `<select>` control bar. This phase ships a
**polished, embeddable player UI** as a Web Component in `@firemedia/skyfire-player`,
so a consumer drops `<skyfire-player src="…">` and gets a production player:
controls, track/subtitle menus, subtitle overlay, buffering/error states, fullscreen,
picture-in-picture, and src-reactive channel switching.

## Scope

**In scope (v1):**
- A `<skyfire-player>` custom element (Shadow DOM) wrapping the **unchanged** headless
  `SkyfirePlayer` engine.
- Controls: play/pause, volume slider + mute, fullscreen, picture-in-picture, audio-track
  menu (language + codec labels), subtitle menu (Off + per-language), diagnostics toggle.
- States: loading spinner, buffering indicator, error surface + Retry, ended / "no signal".
- `src`-reactive switching (host swaps the attribute → clean engine teardown + reload).
- Diagnostics overlay (from `__sfStats`).
- Separate example HTML pages under `web/examples/` (full, minimal, chromeless,
  diagnostics) + an index linking them. `web/index.html` becomes a full-controls
  `<skyfire-player>` page that preserves `window.__sfStats` for the Phase-1 gate.
- Unit + browser tests for the element.

**Out of scope (deferred):**
- Keyboard shortcuts + full ARIA/a11y beyond what semantic elements + existing
  `aria-label`s give for free.
- Channel-list UI / EPG / now-next (the "full TV app" scope).
- Changes to the headless engine's decode/sync internals (kept as-is; it has its own
  tests and the Phase-1 harness contract).

## Architecture

```
<skyfire-player src="…" controls="full" muted audio-lead="10">
  └─ Shadow DOM (scoped <style>, no global leakage, no inline styles)
       ├─ .stage
       │    ├─ <canvas> video        ← headless SkyfirePlayer draws here
       │    ├─ .subs <canvas>         ← DVB-sub overlay (engine renders)
       │    └─ <video hidden>         ← PiP sink (canvas.captureStream) for the WebCodecs path
       ├─ .controls  (auto-hide)      ← play/pause, volume+mute, spacer, audio▾, subs▾, PiP, ⛶, ⓘ
       ├─ menu popovers               ← audio (radio list), subtitle (Off + per-lang)
       ├─ state overlays              ← loading / buffering / error+Retry / ended
       └─ diagnostics overlay         ← decoded/drawn/audioFrames/skew/buffer-ahead/videoPath
  internally: new SkyfirePlayer(shadowCanvas, { streamUrl, muted, audioLeadSeconds })
```

**New file `packages/player/skyfire-element.js`** — `class SkyfirePlayerElement extends
HTMLElement` + `customElements.define("skyfire-player", …)`. The **engine
(`skyfire-player.js`) is unchanged**; the element instantiates it against the
shadow-root canvas and owns all DOM/UI/state.

**Attributes** (reflected):
- `src` — stream URL. Change → teardown + reload (see src-switching).
- `controls` — `"full"` (default) | `"minimal"` (play + fullscreen) | `"none"` (chromeless).
- `muted` — start muted (autoplay-friendly).
- `autoplay` — begin playback without a gesture where the browser allows.
- `audio-lead` — seconds → engine `audioLeadSeconds` (default 10).

**Properties / methods** delegate to the engine: `play()`, `pause()`, `selectAudio(pid)`,
`selectSubtitle(pid)`, `get tracks()`, `get stats()`.

**Events** re-emitted as DOM `CustomEvent`s on the element (bubbling, composed):
`sf-tracks`, `sf-stats`, `sf-error`, `sf-ended` — `detail` carries the engine payload.
The element also mirrors the engine's stats to `window.__sfStats` (keeps the Phase-1
browser gate working unchanged).

## Components & behaviour

### Control bar (`controls="full"`)
Auto-hides after ~2.5s idle; reappears on pointer move / focus-within. Buttons:
- **Play/Pause** → engine `play()`/`pause()`.
- **Volume** slider (0–1) + **Mute** toggle → engine gain / `_muted`.
- **Audio menu** — opens a popover radio list of audio tracks, labelled
  `"<language> · <codec>"` (e.g. `"French · EAC3"`), built from `sf-tracks`, updated
  live as tracks resolve; selecting one calls `selectAudio(pid)` and checks the row.
- **Subtitle menu** — `Off` (default) + one row per subtitle track (`"<language>"` or
  `"Subtitle N"`); selecting calls `selectSubtitle(pid|null)`.
- **PiP** — see below; hidden if unsupported.
- **Fullscreen** — host element `requestFullscreen()` / `exitFullscreen()`.
- **Diagnostics** toggle — shows/hides the diagnostics overlay.

`controls="minimal"` shows only Play/Pause + Fullscreen. `controls="none"` renders the
stage only (video + subs), no bar.

### Picture-in-Picture
The WebCodecs path draws to a `<canvas>`, which the PiP API can't use directly. The
element keeps a hidden `<video>` fed by `canvas.captureStream(30)`; the PiP button calls
`video.requestPictureInPicture()`. On the **MSE fallback path** the engine already uses a
real `<video>` — PiP targets it directly. Feature-detect `document.pictureInPictureEnabled`
and `HTMLVideoElement.prototype.requestPictureInPicture`; hide the button when absent.

### State machine
`loading` → `playing` ⇄ `buffering` → `ended` | `error`.
- **loading**: from init until the first frame is drawn (`stats.drawn > 0`).
- **buffering**: `drawn`/`audioFrames` not advancing while `!done` and buffer-ahead low
  (derived in the element from consecutive `sf-stats` samples). Shows a spinner.
- **error**: on `sf-error` → panel with the message + **Retry** (re-inits the engine on
  the same `src`).
- **ended**: on `sf-ended`.

### src-reactive switching
`attributeChangedCallback("src", old, new)`: if playing, `engine.destroy()` (aborts fetch,
tears down decoders/audio/worklet), clear canvas + menus + state, then construct a fresh
`SkyfirePlayer` on the new `src` and start. Guards against overlapping switches (ignore a
change already in flight; latest wins).

### Styling
One `<style>` block inside the shadow root — dark theme matching the current demo. **No
inline `style=""` anywhere; no external design system** (skyfire owns this UI). Relative
units; the stage uses `object-fit: contain`; responsive down to small embeds.

## Example pages (`web/examples/`)
Separate standalone pages, each a real `<skyfire-player>` (served by `web/serve.ts`, with
streams from `skyfire-server`):
- `index.html` — a 4-row list linking the four example pages, each with a one-line note.
- `full.html` — `controls="full"`, fullscreen + PiP + diagnostics available.
- `minimal.html` — `controls="minimal"`.
- `chromeless.html` — `controls="none"`; a small JS snippet drives play/track-select to
  show the programmatic API.
- `diagnostics.html` — `controls="full"` with the diagnostics overlay open on load.

`web/index.html` is rewritten to host one `<skyfire-player controls="full">` and keep the
`window.__sfStats` mirror so `web/tests/streams.spec.mjs` (Phase-1 gate) passes unchanged.

## Data flow
Host sets `src` → element constructs `SkyfirePlayer(shadowCanvas, …)` → engine fetches +
demuxes + decodes (video→canvas, audio→WebAudio, subs→overlay) and emits `stats`/`tracks`/
`error`/`ended` → element updates controls/menus/state overlays + re-emits DOM events +
mirrors `__sfStats`. UI actions (buttons/menus) call engine methods.

## Error handling
- Unsupported PiP / fullscreen → hide the control (feature-detect), never throw.
- `src` missing/empty → idle "no source" state, no engine.
- Engine error → error overlay + Retry; Retry rebuilds the engine.
- src-switch mid-play → clean teardown before reload; no leaked decoders/audio nodes.

## Testing
- **Engine unchanged** — its Rust/JS tests and the Phase-1 streams gate stay green.
- **Unit (bun, jsdom-ish / mocked engine):** element registers; attributes reflect;
  `controls` presets render the right buttons; menus build from a mock `sf-tracks` event
  and re-render when it changes; selecting a menu row calls the right engine method;
  `src` change tears down + reconstructs the engine (spy). Canvas/WASM are mocked — this
  tests DOM/UI/state logic only.
- **Browser (Playwright), reusing the streams-gate infra:** load a real `<skyfire-player>`
  against a served stream and assert: controls render for `controls="full"`; audio menu
  lists tracks and selecting switches `decodedAudioPid`; subtitle menu selects a track and
  cues render; fullscreen + PiP buttons invoke their APIs (or are hidden when unsupported);
  buffering/error overlays appear appropriately; `src`-switch reloads cleanly. The existing
  per-stream continuity gate keeps passing (element wraps the engine; `__sfStats` preserved).

## Constraints
- Zero inline `style=""`; all CSS in the shadow-root `<style>`.
- Dual licence MIT OR Apache-2.0. No `Co-Authored-By` in commits.
- Ship `skyfire-element.js` in `packages/player/package.json` `files`; the element is an
  additional entry, the headless engine export stays.
- No changes to the engine's decode/sync internals.
- Keep `window.__sfStats` intact (Phase-1 gate contract).
