# Delegation costs

Every delegate run is logged here: tokens in/out and USD cost, rolled up per epic.
Required by [ADR 0002](decisions/0002-delegation-working-practice.md).

_Last updated: 2026-06-19 (issue #12)._

## How cost is recorded

After each `crush` run, read its usage (`crush session last --json` in the repo
root → token counts) and compute cost from the model's rate below. Append a row to
the ledger and update the epic rollup in the same change.

## Model rates (USD per 1M tokens)

| Model (`-m` flag) | $/M in | $/M out |
|---|---|---|
| `deepseek/deepseek-v4-flash` | 0.14 | 0.28 |
| `deepseek/deepseek-v4-pro` | 0.435 | 0.87 |
| `fireworks/.../minimax-m3` | 0.30 | 1.20 |
| `fireworks/.../glm-5p2` | 1.40 | 4.40 |

> Source of truth for prices: `~/.config/crush/crush.json` (verified 2026-06-18).
> Note: coding is output-heavy, and DS Pro's output ($0.87/M) is cheaper than
> M3's ($1.20/M) — prefer DS Pro over M3 for code unless vision is needed.

## Ledger

| Date | Issue | Epic | Model | Tokens in | Tokens out | Cost (USD) |
|---|---|---|---|---|---|---|
| 2026-06-18 | #20 channel map (dvb-si) | #1 | deepseek-v4-pro (flash research aborted) | ~141k | n/a¹ | 0.20 |
| 2026-06-18 | #22 robust AudioClock | #4 | deepseek-v4-pro (M3 attempt aborted) | ~100k | n/a¹ | 0.08 |
| 2026-06-18 | #23 present queue | #4 | deepseek-v4-pro | ~71k | n/a¹ | 0.04 |
| 2026-06-18 | #24 E-AC-3 decode (oxideav-ac3) | #2 | deepseek-v4-pro | ~64k | n/a¹ | 0.06 |
| 2026-06-18 | #25 skyfire-cli channel-map inspector | #8 | deepseek-v4-pro | ~30k | n/a¹ | 0.01 |
| 2026-06-18 | #21 ES+PTS extraction (dvb-pes) | #1 | deepseek-v4-pro | ~50k | n/a¹ | 0.04 |
| 2026-06-18 | #9 H.264 decoder config (avcC) | #3 | deepseek-v4-pro | ~60k | n/a¹ | 0.05 |
| 2026-06-19 | #17 catch-up/stall/latency (2 hangs, fixed) | #4 | deepseek-v4-pro | n/a | n/a¹ | 0.07 |
| 2026-06-19 | #26 skyfire-core engine wiring | #5 | deepseek-v4-pro | ~60k | n/a¹ | 0.05 |
| 2026-06-19 | #15 wasm32 CI build lane | #8 | deepseek-v4-pro | ~30k | n/a¹ | 0.01 |
| 2026-06-19 | #12 wasm-bindgen expose engine | #5 | deepseek-v4-pro | ~40k | n/a¹ | 0.03 |
| 2026-06-22 | #28 skyfire-ts demux + PSI track enum | #27 | **claude-sonnet** (Anthropic subagent) | ~100k total² | — | ~0.30² |

¹ crush reports a cumulative session `cost` but only last-turn token counts; cost is the reliable figure.

² **2026-06-22 — delegation switched from crush to Anthropic subagents**, model-tiered
(Haiku/Sonnet for simple-moderate, Opus for hard) to spend tokens wisely; the crush
rate table above no longer applies to new rows. Subagents report a single total token
figure (not an in/out split), so USD is an estimate at Sonnet rates pending a confirmed
rate source.

## Per-epic rollup

| Epic | Issues delegated | Total cost (USD) |
|---|---|---|
| #1 demux | 2 | 0.24 |
| #2 ac3 | 1 | 0.06 |
| #3 video | 1 | 0.05 |
| #4 sync | 3 | 0.19 |
| #5 wasm+shell | 2 | 0.08 |
| #6 deinterlace | 0 | 0.00 |
| #7 live/fallback | 0 | 0.00 |
| #8 fixtures/CI/harness | 2 | 0.02 |
| **Total** | **11** | **0.64** |
