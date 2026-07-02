# Open-source skyfire player component (#41)

- **Date:** 2026-07-03
- **Status:** Approved (design)
- **Issue:** [#41](https://github.com/fishloa/rust-skyfire/issues/41) (under epic [#27](https://github.com/fishloa/rust-skyfire/issues/27))

## Goal

Publish skyfire as an **open-source, distributable** in-browser player component
that any web app (zenith's `/watch` being the first consumer) can pull from npm
and drop in — WebCodecs HW video + WASM AC-3/E-AC-3 audio + audio-master A/V sync
+ DVB-subtitle overlay, over a progressive-H.264 MPEG-TS-over-HTTP source.

This is **packaging + a clean public API around already-verified code**, not new
playback features. The engine (bridge + player) is built and browser-verified
(epic #27, #43/#39/#40 done); #41 makes it a consumable, versioned library.

## Layered shape (two packages)

The codebase is already split this way (`skyfire-wasm` = bridge, `web/player.js`
= app), so layering is low marginal cost.

### `@skyfire/core` — the assist/bridge
The low-level component: demux + WASM audio decode + sync clock + subtitle
composite + container helpers. The host wires WebCodecs / WebAudio / canvas.
Ships the wasm-pack bundle (`skyfire-wasm`) + a typed JS facade over
`SkyfireBridge`. Public API (camelCase JS facade over the wasm-bindgen methods):

```
new SkyfireBridge()
feed(bytes: Uint8Array): void
flush(): void
tracks(): { videoPid, videoCodec, audio: Track[], subtitles: Track[] } | null
selectAudio(pid: number): void
selectSubtitle(pid: number | null): void
setAudioDownmix(enabled: boolean): void
audioNativeChannels(): number
takeVideoAUs(): { bytes, ptsTicks, dtsTicks, isKeyframe }[]
videoCodec(): string | null            // RFC 6381
videoConfigDescription(): Uint8Array    // avcC
videoInitSegment(): Uint8Array          // fMP4 init (MSE path)
takeVideoMediaSegment(): { bytes, baseMediaDecodeTime, sampleCount } | null
takeAudioPCM(): { samples: Float32Array, sampleRate, channels, ptsTicks }[]
takeSubtitleCues(): { startPts, endPts, regions: Region[] }[]
pcrPts(): number | null                 // audio-master clock source
```

### `@skyfire/player` — turnkey (depends on `@skyfire/core`)
Owns presentation: WebCodecs `VideoDecoder` (or MSE fMP4 fallback), WebAudio
`AudioWorklet`, subtitle overlay, HTTP fetch + hold-open/reconnect, audio-master
sync, multichannel passthrough. Public API:

```
new SkyfirePlayer(canvas: HTMLCanvasElement, opts: {
  streamUrl: string,
  audioPid?: number, subtitlePid?: number, muted?: boolean,
})
init(): Promise<void>   play(): void   pause(): void
selectAudio(pid: number): void
selectSubtitle(pid: number | null): void
tracks(): TrackList
on(event: 'tracks'|'stats'|'error'|'ended', cb): void
destroy(): void
```

**Fallback:** if the `@skyfire` npm org is unavailable, ship a single package
`skyfire-tv` with subpath exports (`.` = player, `./core` = bridge). Config-only
change; the source layout and APIs are identical.

## Source layout

```
packages/
  core/     package.json (@skyfire/core), index.js (facade + wasm glue),
            index.d.ts, README.md; wasm copied in by CI from wasm-pack.
  player/   package.json (@skyfire/player, deps @skyfire/core),
            skyfire-player.js (extracted from web/player.js), index.d.ts, README.md.
web/        the EXAMPLE consumer — index.html + a thin bootstrap that imports
            @skyfire/player (kept working as the local dev/demo + e2e target).
```

`SkyfirePlayer` is **extracted** from the current `web/player.js` (the proven
WebCodecs/MSE/audio/subtitle/sync logic) into `packages/player/skyfire-player.js`
with a class boundary — DOM-app glue (element lookups, status text) moves to the
example bootstrap. No playback-logic rewrite.

## Build + CI

- **Build:** `wasm-pack build crates/skyfire-wasm --target bundler` → assembled
  into `packages/core` (npm consumers use a bundler; the existing `--target web`
  build under `web/pkg` stays for the local example/e2e).
- **Release workflow** (`.github/workflows/release-npm.yml`): on push tag `v*` →
  build wasm → assemble both packages → `npm publish` **core then player**
  (`--access public`, `secrets.NPM_TOKEN`). Publishing happens **only in CI**;
  never from a CLI.
- **PR check:** `npm pack` dry-run of both packages (catch packaging/exports
  errors) + `tsc --noEmit` against the `.d.ts` + a tiny consumer snippet.
- Existing Rust CI (fmt / clippy `-D` / build / nextest / wasm32) unchanged.

## Types & docs

- Hand-written `index.d.ts` for both public APIs (OSS consumers need types).
- Per-package `README.md` (install, usage, API) + a top-level README section; the
  **zenith↔skyfire stream contract** (progressive H.264 `frame_mbs_only=1`,
  untouched AC-3/E-AC-3, separate-PID DVB subs, PCR/PTS preserved — from #27) is
  documented as the input contract. Dual MIT/Apache (already in the repo).

## Testing

- **Rust gate unchanged** — the bridge is already unit/fixture-tested.
- **Packaged-API e2e:** the existing Playwright specs (progressive WebCodecs
  decode, MSE fMP4 fallback, DVB-sub cue) are **re-pointed at the example page
  that imports `@skyfire/player`** — proving the *packaged* public API works, not
  just the old inline app. This is the behavioural gate for the extraction.
- **Packaging gate:** `npm pack` dry-run + `tsc` type-check in CI.
- **External-blocked (unchanged):** iOS-17 real-device playback verify.

## Versioning

Both packages track the workspace version (currently `0.1.x`); a `v0.1.0` tag
triggers the first publish. semver thereafter; core and player versioned in
lockstep initially (player pins the exact core version).

## Out of scope (YAGNI)

- Framework wrappers (React/Vue/Svelte) — the plain DOM API wraps trivially; not
  shipped until a consumer needs one.
- New playback features — #41 is extraction + packaging of verified behaviour.
- HLS ingest, DRM, casting — not in the current engine; not added here.
