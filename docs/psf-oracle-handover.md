# PsF re-signal decoder oracle — sample handover (#38)

The **PsF oracle** (`web/psf-oracle.html` + `web/psf-oracle.js`) is skyfire's
cross-project **gate** for zenith's no-GPU PsF re-signal path (zenith#999 /
zenith#986). It answers one question about a zenith-produced TS: *does WebCodecs
decode it cleanly, or does a still-field-coded slice header trip
`PIPELINE_ERROR_DECODE`?* — before `/skyfire/<slug>` ships that channel.

## How zenith hands over a sample

A zenith re-signaled TS reaches skyfire one of two ways:

1. **Live** (preferred): zenith serves the transcoded stream at
   `https://tv.icomb.place/stream/<serviceSlug>` (`video/MP2T`). Capture a short
   slice:
   ```bash
   curl -s -m 12 "https://tv.icomb.place/stream/<slug>" -o sample.ts
   # optional trim: ffmpeg -copy_unknown -i sample.ts -t 3 -map 0:v -map 0:a:0 -c copy psf.ts
   ```
   PsF-origin channels (e.g. `m6`, French TNT 1080i/PsF) are the ones to gate.

2. **Submitted file**: zenith drops a `<slug>.ts` sample; place it under
   `web/fixtures/` (or any served path) and point the oracle at it.

## How skyfire runs the gate

```bash
# build wasm + serve (see web/README / e2e.spec.mjs header)
(cd web && PORT=8080 bun run serve.ts &)
# open the oracle against the sample:
#   http://localhost:8080/psf-oracle.html?src=/fixtures/<sample>.ts
```

The page sets `window.__sfOracle = { verdict, frames, error }` for headless
harnesses (Playwright reads it):

| Verdict | Meaning |
|---|---|
| `pass` | frames decoded, no `VideoDecoder` error → WebCodecs-safe, channel may ship |
| `fail` | a decoder error (`PIPELINE_ERROR_DECODE`, "closed codec") or zero frames → **do NOT ship**; the re-signal is still field-coded |

A known-good progressive stream must PASS (regression: the
`PsF oracle PASS on a clean progressive stream` case in `web/tests/e2e.spec.mjs`,
using `h264-25fps.ts`). **iOS-17 Safari** must be checked on a real device — the
interlaced wall (ADR 0005) manifests differently across WebKit versions and
cannot be verified headless.

## Findings

- **2026-07-02 — `m6` → FAIL.** A live `/stream/m6` capture is H.264 1080i with
  `field_order = tt` (still interlaced-signaled). WebCodecs decodes 1 frame then
  closes the codec (`Cannot call 'decode' on a closed codec`). zenith's m6 PsF
  re-signal is **not yet producing WebCodecs-decodable progressive** — this gate
  blocks it (zenith-side fix tracked in zenith#999). The oracle itself is
  confirmed working: it PASSes clean progressive and FAILs this real sample.
