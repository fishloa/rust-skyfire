# CLAUDE.md — orchestration guide for rust-skyfire

You are the **orchestrator**, not the author. You write spec-based briefs,
delegate implementation to a cheaper external engineering model, and
independently verify every result against the CI gate. **You do not write the
production code yourself** (same model as the sibling project `rust-ac4`).

## What this is

**Skyfire** — an in-browser DVB TV player. The client decodes the raw MPEG-TS
served by an upstream DVB-S2 receiver: **WebCodecs** for hardware video
(H.264/H.265), a **WASM AC-3/E-AC-3 decoder** for the audio browsers refuse,
**WebAudio** for output, and an **audio-master A/V sync clock**. Zero
server-side transcode. See `README.md` for the architecture.

## The crates

- `skyfire-ts` — MPEG-TS/HLS demux → ES + PTS. Reuse `rust-dvb` (`dvb-si`,
  `dvb-common`) for PSI (PAT/PMT) parsing rather than re-implementing it.
- `skyfire-ac3` — AC-3 / E-AC-3 decode → interleaved PCM.
- `skyfire-sync` — audio-master clock (`AudioClock`, `FrameAction`).
- `skyfire-core` — engine wiring.
- `skyfire-wasm` — `wasm-bindgen`/`web-sys` boundary for the browser shell.
- `skyfire-cli` — native debug harness over the demux/decode crates.
- `web/` — JS/TS shell: WebCodecs `VideoDecoder`, `AudioWorklet`, canvas + a
  GPU deinterlace shader.

## Source of truth

- Codec/container specs: H.264 (ITU-T H.264), H.265 (ITU-T H.265), AC-3 /
  E-AC-3 (ETSI TS 102 366), MPEG-TS (ISO/IEC 13818-1). Spec PDFs are **not**
  vendored (copyright); curated transcriptions go in `specs/md/` if needed.
- Browser APIs: WebCodecs and WebAudio (MDN / W3C). Gate codec support on
  `VideoDecoder.isConfigSupported`.
- TS parsing: prefer `rust-dvb` crates over hand-rolling.
- **Cite or don't write** — every behavioural claim needs a spec section, a
  fixture, or "verified <date>".

## Delegation workflow

Work is **GitHub issues** under **epics**. To implement issue N:

```bash
scripts/delegate.sh <N>            # model defaults to deepseek/deepseek-v4-pro
```

It fills `.delegate/SKYFIRE_BRIEF.tmpl` (`__N__` → N) into `.delegate/brief-N.txt`
and runs it through `crush`. Then **you verify** against the CI gate below — do
not trust the model's self-report; run the gate yourself.

## CI gate (must pass before any commit — CI runs these exactly)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # zero warnings
cargo build --workspace                                  # zero warnings
cargo nextest run --workspace                            # all green
```

For browser/WASM work that can't run in native CI, the brief must define a
concrete behavioural check (fixture-driven decode, golden bytes) — never accept
"it compiles" as done.

## Constraints

- No `unsafe` (audio/video decode included) unless a brief explicitly justifies it.
- Dual licence MIT OR Apache-2.0.
- **No `Co-Authored-By` lines in commits.**
- Touch only the crates an issue needs; keep everything that passes green.
- TS fixtures live in `fixtures/`; reuse them, don't fetch live hardware.

## Done = verified

A delegated issue is done only when the CI gate is green AND the behavioural
check in its brief holds. Then commit (clear message, no co-author) and
`gh issue close N` with a one-paragraph evidence summary.
