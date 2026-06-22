# CLAUDE.md — orchestration guide for rust-skyfire

You are the **orchestrator**, not the author. You write spec-based briefs,
delegate implementation to a cheaper external engineering model, and
independently verify every result against the CI gate. **You do not write the
production code yourself** (same model as the sibling project `rust-ac4`).

## What this is

**Skyfire** — an in-browser DVB TV player. The server (zenith) re-encodes
**video only** — deinterlaces true-1080i → progressive H.264 (NVENC) and re-muxes
TS; **audio (AC-3/E-AC-3), DVB-subtitle/teletext, SI PIDs and PCR/PTS pass through
untouched**. The client is the **skyfire WASM bridge**: it demuxes the TS, hands
progressive H.264 to **WebCodecs** (HW video), decodes the selected AC-3/E-AC-3
PID in **WASM** → **WebAudio**, parses **DVB subtitles** → cues, and runs an
**audio-master A/V sync clock** off PCR/PTS. The browser owns the canvas, WebAudio,
subtitle overlay and controls; WASM is the bridge. **Audio is never re-encoded;
`oxideav-h264` is not used in the browser.** Authoritative architecture:
[ADR 0008](docs/decisions/0008-video-only-transcode-wasm-bridge.md) + the client
epic [#27](https://github.com/fishloa/rust-skyfire/issues/27) (carries the
zenith↔skyfire stream contract). Server transcode lives in zenith (zenith#986).

## Docs (keep current)

- `docs/decisions/` — ADRs (why). Numbered, immutable once Accepted; supersede, don't edit. Update the index in same change.
- `docs/OBJECTIVES.md` — objectives + epic status. Update the row when state changes.
- ADR 0001 fixes support scope (browsers, iOS-17, H.265 gate + H.264 fallback). Honour it.

## The crates

- `skyfire-ts` — MPEG-TS/HLS demux → ES + PTS. Reuse `rust-dvb` (`dvb-si`,
  `dvb-common`) for PSI (PAT/PMT) parsing rather than re-implementing it.
- `skyfire-ac3` — AC-3 / E-AC-3 decode → interleaved PCM.
- `skyfire-sync` — audio-master clock (`AudioClock`, `FrameAction`).
- `skyfire-core` — engine wiring.
- `skyfire-wasm` — the **bridge** API: `wasm-bindgen`/`web-sys` boundary; commands
  in (`selectAudio`/`selectSubtitle`/play), events out (video AUs, PCM, subtitle
  cues, track-list, clock). Reuses `rust-dvb` `dvb-subtitle` for DVB-SUB.
- `skyfire-cli` — native debug harness over the demux/decode crates.
- `web/` — JS/TS shell: WebCodecs `VideoDecoder`, `AudioWorklet`, canvas, subtitle
  overlay, controls. (No deinterlace shader — the server delivers progressive.)

## Source of truth

- Codec/container specs: H.264 (ITU-T H.264), H.265 (ITU-T H.265), AC-3 /
  E-AC-3 (ETSI TS 102 366), MPEG-TS (ISO/IEC 13818-1). Spec PDFs are **not**
  vendored (copyright); curated transcriptions go in `specs/md/` if needed.
- Browser APIs: WebCodecs and WebAudio (MDN / W3C). Gate codec support on
  `VideoDecoder.isConfigSupported`.
- TS parsing: prefer `rust-dvb` crates over hand-rolling.
- **Cite or don't write** — every behavioural claim needs a spec section, a
  fixture, or "verified <date>".

## Delegation workflow (ADR 0002)

Work is **GitHub issues with exit gates**, under **epics**. Per issue N:

1. Write/confirm the issue as a *feature + verifiable exit criteria* (not steps).
2. Delegate via the `delegate` skill (crush) — **not** `scripts/delegate.sh`.
   Cheapest capable model, ramp on no-progress:
   `deepseek-v4-flash` → `deepseek-v4-pro` → `minimax-m3` → `glm-5p2`.
3. **You verify** against the CI gate below — never trust self-report.
4. Record tokens + cost in `docs/COSTS.md` (per issue, rolled up per epic).
5. Green → commit (no co-author) + `gh issue close N` with evidence.

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
