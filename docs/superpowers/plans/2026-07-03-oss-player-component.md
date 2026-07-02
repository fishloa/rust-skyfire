# OSS skyfire player component Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Package skyfire as two distributable npm libraries — `@skyfire/core` (the WASM demux/decode/sync bridge) and `@skyfire/player` (turnkey WebCodecs+WebAudio player built on core) — with types, an example page, and a CI-publish workflow, wrapping already-verified code.

**Architecture:** `@skyfire/core` ships the `wasm-pack --target bundler` output of `skyfire-wasm` plus a small camelCase JS facade + `.d.ts`. `@skyfire/player` extracts the proven engine from `web/player.js` into a `SkyfirePlayer` class (canvas + options in, events out) that depends on core. `web/` becomes the example consumer and the Playwright e2e target — the e2e re-pointed at the packaged player is the behavioural gate for the extraction. Publishing is CI-only, on a `v*` tag.

**Tech Stack:** Rust→WASM (`wasm-pack`), JavaScript ES modules, TypeScript `.d.ts` (types only, no TS compile of source), npm packages, GitHub Actions, Playwright, bun (local dev/serve).

## Global Constraints

- Dual licence **MIT OR Apache-2.0** (already in repo root: `LICENSE-MIT`, `LICENSE-APACHE`).
- **Never `npm publish` from a CLI** — publishing happens only in CI on a `v*` tag.
- No new playback features — this is **extraction + packaging of verified behaviour**; behaviour must not change (the re-pointed Playwright e2e proves it).
- Public API is camelCase JS (the wasm-bindgen methods are snake_case; the facade renames).
- Packages track the workspace version (currently `0.1.1`); player pins the exact core version.
- Scoped names `@skyfire/core` + `@skyfire/player`; **fallback** to a single `skyfire-tv` package with subpath exports if the `@skyfire` npm org is unavailable (config-only — same source, same APIs).
- Existing Rust CI gate (fmt / clippy `-D warnings` / build / `nextest` / wasm32) stays green and unchanged.
- Reference: `docs/superpowers/specs/2026-07-03-oss-player-component-design.md`.

## Reference: current `SkyfireBridge` wasm-bindgen API (snake_case → facade camelCase)

`new()`, `feed(&[u8])`, `flush()`, `track_list() -> WasmTrackList?`,
`select_audio(u16)`, `select_subtitle(Option<u16>)`, `set_audio_downmix(bool)`,
`audio_native_channels() -> u16`, `take_video_aus() -> WasmVideoAu[]`,
`video_codec() -> String?`, `video_config_description() -> Uint8Array`,
`video_init_segment() -> Uint8Array`, `take_video_media_segment() -> WasmMediaSegment?`,
`take_audio_pcm() -> WasmPcmChunk[]`, `take_subtitle_cues() -> WasmSubtitleCue[]`,
`pcr_pts() -> i64?`.

## File Structure

- `packages/core/package.json` — `@skyfire/core` manifest (files: wasm + facade + types).
- `packages/core/skyfire-core.js` — camelCase facade re-exporting `SkyfireBridge` + `init`.
- `packages/core/index.d.ts` — types for the facade + wasm structs.
- `packages/core/README.md`.
- `packages/core/pkg/` — wasm-pack `--target bundler` output (git-ignored; produced by build).
- `packages/player/package.json` — `@skyfire/player` (deps `@skyfire/core`).
- `packages/player/skyfire-player.js` — `SkyfirePlayer` class (extracted from `web/player.js`).
- `packages/player/index.d.ts` — `SkyfirePlayer` types.
- `packages/player/README.md`.
- `web/index.html` + `web/example.js` — example consumer importing `@skyfire/player`.
- `web/player.js` — becomes a thin re-export/bootstrap OR is replaced by `example.js` (keep e2e stable).
- `.github/workflows/release-npm.yml` — build + publish on tag.
- `.github/workflows/ci.yml` — add a `npm pack` dry-run + `tsc` type-check job (PR gate).
- `scripts/build-packages.sh` — assemble packages locally (wasm-pack → packages/core/pkg, copy files).
- `README.md` (root) — add a "Using skyfire in the browser" section + the stream contract.
- `.gitignore` — add `packages/*/pkg/` and `packages/*/*.tgz`.

---

### Task 1: `@skyfire/core` package — facade + types + manifest

**Files:**
- Create: `packages/core/package.json`, `packages/core/skyfire-core.js`, `packages/core/index.d.ts`, `packages/core/README.md`
- Create: `scripts/build-packages.sh`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: `wasm-pack build crates/skyfire-wasm --target bundler --out-dir packages/core/pkg` output (`pkg/skyfire_wasm.js` exporting `default` init + `SkyfireBridge`).
- Produces: `@skyfire/core` exporting `initSkyfire(): Promise<void>` and `SkyfireBridge` (the wasm class, used directly — its methods are already the API; the facade only adds `initSkyfire` + types). Named export `SkyfireBridge`.

- [ ] **Step 1: Write the build script**

Create `scripts/build-packages.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLCHAIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
export PATH="$TOOLCHAIN:$PATH"
# Bundler target for npm consumers (webpack/vite/rollup resolve the wasm).
wasm-pack build "$ROOT/crates/skyfire-wasm" --target bundler --release \
  --out-dir "$ROOT/packages/core/pkg"
echo "built packages/core/pkg"
```

`chmod +x scripts/build-packages.sh`.

- [ ] **Step 2: Run the build script; verify pkg output**

Run: `./scripts/build-packages.sh && ls packages/core/pkg/`
Expected: `skyfire_wasm.js`, `skyfire_wasm_bg.wasm`, `skyfire_wasm.d.ts`, `package.json` present.

- [ ] **Step 3: Write the facade**

Create `packages/core/skyfire-core.js`:

```js
// @skyfire/core — WASM demux + AC-3/E-AC-3 decode + A/V-sync + DVB-sub composite.
// The host wires WebCodecs / WebAudio / canvas. See README for the API.
import initWasm, { SkyfireBridge } from "./pkg/skyfire_wasm.js";

let _ready = null;

/** Initialize the WASM module. Idempotent; await before constructing a bridge. */
export function initSkyfire() {
  if (!_ready) _ready = initWasm();
  return _ready;
}

export { SkyfireBridge };
```

- [ ] **Step 4: Write the manifest**

Create `packages/core/package.json`:

```json
{
  "name": "@skyfire/core",
  "version": "0.1.1",
  "description": "In-browser DVB player bridge: MPEG-TS demux + WASM AC-3/E-AC-3 decode + audio-master A/V sync + DVB-subtitle compositing. Host wires WebCodecs/WebAudio.",
  "type": "module",
  "main": "skyfire-core.js",
  "module": "skyfire-core.js",
  "types": "index.d.ts",
  "files": ["skyfire-core.js", "index.d.ts", "pkg/", "README.md"],
  "sideEffects": ["./pkg/skyfire_wasm.js"],
  "license": "MIT OR Apache-2.0",
  "repository": { "type": "git", "url": "https://github.com/fishloa/rust-skyfire" },
  "keywords": ["dvb", "mpeg-ts", "webcodecs", "ac-3", "wasm", "player"]
}
```

- [ ] **Step 5: Write the types**

Create `packages/core/index.d.ts`:

```ts
export function initSkyfire(): Promise<void>;

export interface Track { pid: number; kind: string; language?: string; codec?: string; }
export interface TrackList { videoPid: number; videoCodec: string; audio: Track[]; subtitles: Track[]; }
export interface VideoAu { bytes: Uint8Array; ptsTicks?: bigint; dtsTicks?: bigint; isKeyframe: boolean; }
export interface MediaSegment { bytes: Uint8Array; baseMediaDecodeTime: bigint; sampleCount: number; }
export interface PcmChunk { samples: Float32Array; sampleRate: number; channels: number; ptsTicks?: bigint; }
export interface SubtitleRegion { x: number; y: number; width: number; height: number; rgba: Uint8Array; }
export interface SubtitleCue { startPts: bigint; endPts: bigint; regions: SubtitleRegion[]; }

export class SkyfireBridge {
  constructor();
  feed(bytes: Uint8Array): void;
  flush(): void;
  track_list(): TrackList | undefined;
  select_audio(pid: number): void;
  select_subtitle(pid?: number): void;
  set_audio_downmix(enabled: boolean): void;
  audio_native_channels(): number;
  take_video_aus(): VideoAu[];
  video_codec(): string | undefined;
  video_config_description(): Uint8Array;
  video_init_segment(): Uint8Array;
  take_video_media_segment(): MediaSegment | undefined;
  take_audio_pcm(): PcmChunk[];
  take_subtitle_cues(): SubtitleCue[];
  pcr_pts(): bigint | undefined;
}
```

(Property names mirror the wasm-bindgen getters — verify against `packages/core/pkg/skyfire_wasm.d.ts` at build time and align any casing the generated bindings differ on.)

- [ ] **Step 6: Write the README**

Create `packages/core/README.md` — install (`npm i @skyfire/core`), the "samples-in / host-renders" model, a minimal snippet feeding TS + draining `takeAudioPCM`/`takeVideoAUs`, and the stream contract link. (Full prose; no placeholders.)

- [ ] **Step 7: gitignore the built wasm + tarballs**

Append to `.gitignore`:

```
packages/*/pkg/
packages/*/*.tgz
```

- [ ] **Step 8: Verify packaging (dry-run) + types**

Run: `cd packages/core && npm pack --dry-run`
Expected: lists `skyfire-core.js`, `index.d.ts`, `pkg/*`, `README.md`; no errors.
Run: `npx -y typescript@5 tsc --noEmit --strict index.d.ts`
Expected: no type errors.

- [ ] **Step 9: Commit**

```bash
git add packages/core scripts/build-packages.sh .gitignore
git commit -m "feat(pkg): @skyfire/core — wasm bridge facade + types + build script"
```

---

### Task 2: Extract `SkyfirePlayer` class into `@skyfire/player`

**Files:**
- Create: `packages/player/skyfire-player.js`, `packages/player/package.json`, `packages/player/index.d.ts`, `packages/player/README.md`
- Source: `web/player.js` (extract from; do not delete yet — Task 3 rewires it)

**Interfaces:**
- Consumes: `@skyfire/core` (`initSkyfire`, `SkyfireBridge`).
- Produces:
  ```ts
  class SkyfirePlayer {
    constructor(canvas: HTMLCanvasElement, opts: {
      streamUrl: string; audioPid?: number; subtitlePid?: number; muted?: boolean;
    });
    init(): Promise<void>;
    play(): void; pause(): void;
    selectAudio(pid: number): void;
    selectSubtitle(pid: number | null): void;
    tracks(): TrackList | null;
    on(event: "tracks" | "stats" | "error" | "ended", cb: (data: unknown) => void): void;
    destroy(): void;
  }
  ```

- [ ] **Step 1: Create the class scaffold**

Create `packages/player/skyfire-player.js` importing from `@skyfire/core`:

```js
import { initSkyfire, SkyfireBridge } from "@skyfire/core";

const PTS_HZ = 90_000;
const ticksToMicros = (t) => Number(t) * 1_000_000 / PTS_HZ;

export class SkyfirePlayer {
  constructor(canvas, opts = {}) {
    if (!canvas) throw new Error("SkyfirePlayer: canvas required");
    this.canvas = canvas;
    this.opts = opts;
    this.streamUrl = opts.streamUrl;
    this._listeners = { tracks: [], stats: [], error: [], ended: [] };
    this.bridge = null;
    // …instance state (was module-scope in web/player.js) moved here.
  }
  on(event, cb) { (this._listeners[event] ||= []).push(cb); }
  _emit(event, data) { (this._listeners[event] || []).forEach((cb) => cb(data)); }
  // init/play/pause/selectAudio/selectSubtitle/tracks/destroy — filled in below.
}
```

- [ ] **Step 2: Port the engine from `web/player.js` (mechanical extraction)**

Move the proven logic from `web/player.js` into the class using this mapping — **behaviour must not change**:

| `web/player.js` (module scope) | `SkyfirePlayer` |
|---|---|
| module `let` state (`videoDecoder`, `videoPath`, `mse*`, `audioCtx`, `audioNode`, `streamChannels`, `subQueue`, `firstAudioPtsUs`, `audioFramesPlayed`, `sawKeyframe`, `presentScheduled`, …) | `this.*` instance fields (init in constructor) |
| `document.getElementById("canvas")` | `this.canvas` (constructor arg) |
| `getElementById("overlay"/"subs")` overlay canvas | a child canvas the player creates over `this.canvas` (create in `init`), not page-global |
| `getElementById("audio-select"/"sub-select"/"playpause"/"mute")` + `wireControls`/`populateTracks` | **removed** — controls are the host's job; replaced by `selectAudio/selectSubtitle` methods + the `tracks` event |
| `status()` / `fatal()` | `this._emit("stats"|"error", …)` (no DOM writes) |
| `main()` | `init()` (loads wasm via `initSkyfire()`, constructs `this.bridge`, applies `opts.audioPid/subtitlePid`, starts the consume loop) |
| `consumeStream`, `ensureDecoder`, `pumpVideoInner`, `pumpVideoMseInner`, `decideVideoPath`, `setupMse`, `ensureAudio`, `pumpAudioInner`, `pumpSubtitlesInner`, `present`, `drawSubCue`, `renderSubs`, drift corrector | private methods `this._consume`, `this._ensureDecoder`, … (same bodies; module vars → `this.`) |
| `?video=mse` / `?sub=` URL parsing | `opts` (`opts.forceMse`, `opts.subtitlePid`) — no `location.search` reads in the library |
| `bridge.set_audio_downmix` / `audio_native_channels` (#39) | unchanged (called via `this.bridge`) |
| `window.__sfStats` | keep emitting via `on("stats")`; the **example** sets `window.__sfStats` for e2e |
| `destroy()` (new) | close `VideoDecoder`, `MediaSource`, `AudioContext`; cancel rAF/loops; abort fetch |

Keep the WebCodecs↔MSE gate, audio-master sync, multichannel passthrough, subtitle overlay exactly as they are.

- [ ] **Step 3: Write the manifest**

Create `packages/player/package.json`:

```json
{
  "name": "@skyfire/player",
  "version": "0.1.1",
  "description": "Turnkey in-browser DVB TV player: WebCodecs HW video + WASM AC-3/E-AC-3 audio + audio-master sync + DVB-subtitle overlay, over progressive-H.264 MPEG-TS. Built on @skyfire/core.",
  "type": "module",
  "main": "skyfire-player.js",
  "module": "skyfire-player.js",
  "types": "index.d.ts",
  "files": ["skyfire-player.js", "index.d.ts", "README.md"],
  "dependencies": { "@skyfire/core": "0.1.1" },
  "license": "MIT OR Apache-2.0",
  "repository": { "type": "git", "url": "https://github.com/fishloa/rust-skyfire" },
  "keywords": ["dvb", "tv", "player", "webcodecs", "mpeg-ts", "ac-3"]
}
```

- [ ] **Step 4: Write the types**

Create `packages/player/index.d.ts`:

```ts
import type { TrackList } from "@skyfire/core";
export interface SkyfirePlayerOptions {
  streamUrl: string; audioPid?: number; subtitlePid?: number;
  muted?: boolean; forceMse?: boolean;
}
export type SkyfireEvent = "tracks" | "stats" | "error" | "ended";
export class SkyfirePlayer {
  constructor(canvas: HTMLCanvasElement, opts: SkyfirePlayerOptions);
  init(): Promise<void>;
  play(): void; pause(): void;
  selectAudio(pid: number): void;
  selectSubtitle(pid: number | null): void;
  tracks(): TrackList | null;
  on(event: SkyfireEvent, cb: (data: unknown) => void): void;
  destroy(): void;
}
```

- [ ] **Step 5: README**

Create `packages/player/README.md` — install, a full `<canvas>` + `new SkyfirePlayer(canvas, {streamUrl}).init()` example, the events, and the input stream contract link.

- [ ] **Step 6: Type-check + pack dry-run**

Run: `cd packages/player && npm pack --dry-run` → lists the 4 files.
Run: `node --check skyfire-player.js` → OK.
Run: `npx -y typescript@5 tsc --noEmit index.d.ts` (with core's types resolvable — use a local `tsconfig` paths or skip cross-package resolution and check syntax only).

- [ ] **Step 7: Commit**

```bash
git add packages/player
git commit -m "feat(pkg): @skyfire/player — SkyfirePlayer class extracted from web/player.js"
```

---

### Task 3: Rewire `web/` as the example consumer

**Files:**
- Create: `web/example.js`
- Modify: `web/index.html:67` (script src), `web/package.json` (workspace link to packages)
- Replace/trim: `web/player.js` → the DOM glue that Task 2 removed (controls wiring, `window.__sfStats`) lives here, driving a `SkyfirePlayer`.

**Interfaces:**
- Consumes: `@skyfire/player` `SkyfirePlayer`.
- Produces: a working local example that behaves identically to the pre-extraction app (so the existing e2e passes).

- [ ] **Step 1: Write the example bootstrap**

Create `web/example.js`:

```js
import { SkyfirePlayer } from "@skyfire/player";

const canvas = document.getElementById("canvas");
const params = new URLSearchParams(location.search);
const player = new SkyfirePlayer(canvas, {
  streamUrl: params.get("src") || "/fixtures/h264-25fps.ts",
  subtitlePid: params.has("sub") ? parseInt(params.get("sub"), 10) : undefined,
  forceMse: params.get("video") === "mse",
});

// Re-expose the stats the e2e harness reads.
player.on("stats", (s) => { window.__sfStats = s; });
player.on("tracks", (tl) => { /* populate #audio-select / #sub-select pickers */ });
document.getElementById("audio-select")?.addEventListener("change", (e) =>
  player.selectAudio(parseInt(e.target.value, 10)));
document.getElementById("sub-select")?.addEventListener("change", (e) =>
  player.selectSubtitle(e.target.value === "" ? null : parseInt(e.target.value, 10)));

player.init().catch((err) => { console.error(err); });
```

- [ ] **Step 2: Point index.html at the example + resolve the packages**

`web/index.html:67`: `<script type="module" src="./example.js"></script>`.
Resolve bare `@skyfire/*` specifiers for the browser: add an import map to `index.html` head mapping `@skyfire/player`→`../packages/player/skyfire-player.js`, `@skyfire/core`→`../packages/core/skyfire-core.js`, and `./pkg/skyfire_wasm.js` as used. (Serve `packages/` alongside `web/` — extend `web/serve.ts` to also serve `/packages/`. For the `--target bundler` wasm in packages/core/pkg, the example may instead keep using the existing `web/pkg` `--target web` build; document which build the example uses.)

- [ ] **Step 3: Serve + smoke the example locally**

Run: build wasm for web (`wasm-pack build crates/skyfire-wasm --target web --out-dir web/pkg`), `(cd web && PORT=8080 bun run serve.ts &)`, open `http://localhost:8080/index.html?src=/fixtures/h264-25fps.ts`.
Expected: video decodes (same as before). No console errors beyond the known favicon/no-audio-device ones.

- [ ] **Step 4: Commit**

```bash
git add web/example.js web/index.html web/player.js web/serve.ts web/package.json
git commit -m "refactor(web): web/ becomes the @skyfire/player example consumer"
```

---

### Task 4: Re-point the Playwright e2e at the packaged player

**Files:**
- Modify: `web/tests/e2e.spec.mjs` (only if selectors/URLs changed; the specs should pass unchanged if the example preserves behaviour + `window.__sfStats`)

**Interfaces:**
- Consumes: the example page (Task 3) driving `@skyfire/player`.
- Produces: the behavioural gate proving the extraction preserved WebCodecs / MSE / DVB-sub behaviour.

- [ ] **Step 1: Run the full e2e against the example**

Run (per `e2e.spec.mjs` header): build wasm, serve, `cd web && bunx playwright test tests/e2e.spec.mjs --browser=chromium`.
Expected: the WebCodecs, PsF-oracle, MSE-fallback, and DVB-sub specs pass exactly as before the extraction. (The `real 1080p` audio-device spec stays environment-limited.)

- [ ] **Step 2: Fix any extraction regressions**

If a spec fails, the extraction changed behaviour — diff the ported method against `web/player.js` git history and restore parity. Do NOT weaken the test.

- [ ] **Step 3: Commit**

```bash
git add web/tests
git commit -m "test(e2e): verify @skyfire/player example passes the existing behavioural gate"
```

---

### Task 5: CI — publish workflow + PR packaging gate

**Files:**
- Create: `.github/workflows/release-npm.yml`
- Modify: `.github/workflows/ci.yml` (add a packaging job)

**Interfaces:**
- Consumes: `scripts/build-packages.sh`, `packages/*`.
- Produces: tag-triggered npm publish (core then player); PR-time `npm pack` + `tsc` gate.

- [ ] **Step 1: Write the release workflow**

Create `.github/workflows/release-npm.yml`:

```yaml
name: release-npm
on:
  push:
    tags: ["v*"]
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-unknown-unknown }
      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
      - uses: actions/setup-node@v4
        with: { node-version: "20", registry-url: "https://registry.npmjs.org" }
      - name: Build core wasm (bundler)
        run: wasm-pack build crates/skyfire-wasm --target bundler --release --out-dir packages/core/pkg
      - name: Publish @skyfire/core
        run: npm publish --access public
        working-directory: packages/core
        env: { NODE_AUTH_TOKEN: "${{ secrets.NPM_TOKEN }}" }
      - name: Publish @skyfire/player
        run: npm publish --access public
        working-directory: packages/player
        env: { NODE_AUTH_TOKEN: "${{ secrets.NPM_TOKEN }}" }
```

- [ ] **Step 2: Add the PR packaging gate**

Add a job to `.github/workflows/ci.yml`:

```yaml
  packages:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-unknown-unknown }
      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
      - uses: actions/setup-node@v4
        with: { node-version: "20" }
      - run: wasm-pack build crates/skyfire-wasm --target bundler --release --out-dir packages/core/pkg
      - run: cd packages/core && npm pack --dry-run
      - run: cd packages/player && npm pack --dry-run
      - run: npx -y typescript@5 tsc --noEmit packages/core/index.d.ts
```

- [ ] **Step 3: Validate the workflow YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-npm.yml')); yaml.safe_load(open('.github/workflows/ci.yml')); print('yaml ok')"`
Expected: `yaml ok`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows
git commit -m "ci: npm publish on tag (NPM_TOKEN) + PR packaging/type gate"
```

---

### Task 6: Docs — root README + handoff

**Files:**
- Modify: `README.md` (root)
- Modify: `docs/OBJECTIVES.md` (mark #41 status)

**Interfaces:**
- Consumes: nothing.
- Produces: consumer-facing docs + the publish runbook.

- [ ] **Step 1: Add a "Use skyfire in the browser" section to `README.md`**

Cover: `npm i @skyfire/player`, the `SkyfirePlayer` snippet, the `@skyfire/core` low-level option, the input **stream contract** (progressive H.264 `frame_mbs_only=1`, untouched AC-3/E-AC-3, separate-PID DVB subs, PCR/PTS preserved — link #27), and the **release runbook**: "maintainer adds `NPM_TOKEN` secret + `@skyfire` org; `git tag v0.1.0 && git push --tags` → CI publishes. Never publish from a CLI."

- [ ] **Step 2: Update OBJECTIVES**

In `docs/OBJECTIVES.md`, note #41 delivered (packaged `@skyfire/core` + `@skyfire/player`, CI-publish on tag; iOS real-device still external).

- [ ] **Step 3: Commit**

```bash
git add README.md docs/OBJECTIVES.md
git commit -m "docs: browser-consumer usage + release runbook for @skyfire packages (#41)"
```

---

## Self-Review

**Spec coverage:**
- Layered `@skyfire/core` + `@skyfire/player` → Tasks 1, 2. ✓
- Facade/API + types → Tasks 1, 2 (`.d.ts` both). ✓
- Extraction from `web/player.js`, `web/` as example → Tasks 2, 3. ✓
- CI publish on tag (NPM_TOKEN, never CLI) + PR gate → Task 5. ✓
- Packaged-API e2e as the behavioural gate → Task 4. ✓
- Docs + stream contract + runbook → Task 6. ✓
- `@skyfire`-org fallback to `skyfire-tv` → noted in Global Constraints (config flip; if the org is unavailable at publish time, rename the two `package.json` `name`s to `skyfire-tv` + `skyfire-tv/*` subpath exports and adjust the import map — no source change).
- YAGNI (no framework wrappers, no new features) → honoured; extraction only.

**Placeholder scan:** The README/doc steps say "full prose" but specify exact required content (install, snippet, contract, runbook) — not deferred TODOs. Task 2 Step 2 is a mapping table for a mechanical move, not new code; the e2e (Task 4) is its correctness gate. No `TBD`/`handle edge cases`.

**Type consistency:** `SkyfirePlayer(canvas, opts)` + `init/play/pause/selectAudio/selectSubtitle/tracks/on/destroy` identical across Task 2 (class), Task 2 Step 4 (`.d.ts`), Task 3 (example usage). `initSkyfire` + `SkyfireBridge` consistent across Task 1 facade + types + Task 2 import. Event names `tracks|stats|error|ended` consistent.

**Residual risk (flagged):** the Task 2 extraction is the large/risky step (an ~800-line module → class). Mitigation: it is a behaviour-preserving move gated by the re-pointed Playwright e2e (Task 4) — any regression fails a spec. The `--target bundler` (npm) vs `--target web` (local example) split is called out in Task 3 Step 2; the example may keep using the `web` build while packages ship the `bundler` build.
