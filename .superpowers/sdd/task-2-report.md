# Task 2 Report — Extract `SkyfirePlayer` class into `@skyfire/player`

## Files created

- `packages/player/skyfire-player.js` — `SkyfirePlayer` class (32.7 kB unpacked)
- `packages/player/package.json` — `@skyfire/player` manifest
- `packages/player/index.d.ts` — TypeScript declarations
- `packages/player/README.md` — install/usage docs

## Mapping notes

### Overlay canvas creation

`web/player.js` used `document.getElementById("subs")` to locate a host-provided
container, then created a `<canvas>` inside it via `subsEl.replaceChildren(c)`.
The class cannot reference page-global IDs. Instead, `_createSubsCanvas()` is called
from `init()`: it creates a `<canvas>` and inserts it as a sibling immediately after
`this.canvas` in the DOM, styled `position:absolute; top:0; left:0; width:100%; height:100%;
pointer-events:none`. The host must give the parent element `position:relative` (documented
in the README). `_ensureSubsCanvas()` maps to the old `ensureSubsCanvas()` — checks/resizes
the overlay canvas to match the video canvas.

### Event emission

`status(msg)` wrote to `overlay.textContent` + `console.log`. Mapped to `_status(msg)`
which `console.log`s and emits `"stats"` with `{ ...this._stats, status: msg }`.

`fatal(msg, err)` wrote to `errorEl.textContent`. Mapped to `_fatal(msg, err)` which
`console.error`s and emits `"error"` with `{ message, cause }`.

`window.__sfStats` assignment in `drawFrame` is replaced by `this._emit("stats", {...s})`.
The example consumer (Task 3) re-sets `window.__sfStats` from the `"stats"` event for e2e
compatibility — the library never touches `window`.

### destroy()

New in the class. Aborts the in-flight `fetch` (via `AbortController`), cancels the MSE
drift `rAF`, closes `VideoDecoder`, ends/releases `MediaSource`, closes `AudioContext`,
closes open `VideoFrame` objects in `_presentQueue`, removes the subtitle overlay canvas,
and removes the user-gesture event listeners. Sets `this._destroyed = true` — all public
methods check this flag and no-op after destroy.

### URL opts

`location.search` reads are completely removed from the class:
- `?video=mse` → `opts.forceMse` (boolean, default `false`)
- `?sub=<pid>` → `opts.subtitlePid` (number, applied before stream start via `bridge.select_subtitle`)
- `?src=` → `opts.streamUrl` (required)
- `?live=1` → not yet exposed (hardcoded `false`; Task 3 example.js can add it as an option)

### `main()` → `init()`

`main()` referenced `bridge` as a module-level variable and called `init()` (WASM init).
`init()` calls `initSkyfire()` from `@skyfire/core` (idempotent), constructs
`this.bridge = new SkyfireBridge()`, applies `opts.subtitlePid` if set, then runs the
stream loop. `populateTracks` / `wireControls` are removed; when the track list arrives
the class calls `this._emit("tracks", tl)` instead.

### Re-entrancy guard

`callBridge(method, ...args)` → `this._callBridge(method, ...args)`. Identical logic,
module-level `bridge`, `bridgeLocked`, `pendingBridgeQueue` → instance fields.

### `bridge.set_audio_downmix` / `audio_native_channels`

Unchanged. Called via `this.bridge` in `_ensureAudio`.

### `window.__sfStats` (for e2e)

Not set by the library. The plan's mapping table says: "keep emitting via `on('stats')`;
the **example** sets `window.__sfStats` for e2e". Task 3's `example.js` will do:
`player.on("stats", (s) => { window.__sfStats = s; })`.

## Exit check results

```
node --check packages/player/skyfire-player.js  → PASS

npm pack --dry-run (packages/player):
  4.7kB  README.md
  615B   index.d.ts
  660B   package.json
  32.7kB skyfire-player.js
  total files: 4  ✓

npx -p typescript@5 tsc --noEmit index.d.ts:
  TS2307: Cannot find module '@skyfire/core' — cross-package resolution failure only
  (standalone tsc cannot see @skyfire/core without a workspace node_modules install)
  packages/core/index.d.ts passes clean with the same tsc invocation ✓
  All d.ts syntax is valid — the single error is purely module resolution.
```

## Commit hash

(filled in after commit)
