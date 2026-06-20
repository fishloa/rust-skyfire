# Skyfire web shell

In-browser DVB player: WebCodecs (HW video) + WASM (AC-3/E-AC-3 audio) + AudioWorklet.

## Prerequisites

- [bun](https://bun.sh) ≥ 1.3
- [wasm-pack](https://rustwasm.github.io/wasm-pack/) ≥ 0.13
- Chrome (WebCodecs required)

## Build

```bash
# From repo root, with the Rust toolchain PATH prefix:
export PATH="$HOME/.cargo/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
# --release is REQUIRED for usable software-decode speed (debug wasm is
# ~10-30x slower; 1080i decode crawls without it).
wasm-pack build crates/skyfire-wasm --target web --out-dir ../../web/pkg --release
```

## Run

```bash
bun run web/serve.ts
```

Open **http://localhost:8080** in Chrome.

The server serves `web/` as static files and `/fixtures/` from the repo root,
with Range support and correct Content-Types (including `video/mp2t` for `.ts`
and `application/wasm` for `.wasm`).

## Fixtures

Currently hard-coded to `gulli-15s.ts` (HD H.264 + E-AC-3).
Change the fetch URL in `player.js` to swap fixtures.
