# Delegation costs

Every delegate run is logged here: tokens in/out and USD cost, rolled up per epic.
Required by [ADR 0002](decisions/0002-delegation-working-practice.md).

_Last updated: 2026-06-18 (issue #20)._

## How cost is recorded

After each `crush` run, read its usage (`crush session last --json` in the repo
root → token counts) and compute cost from the model's rate below. Append a row to
the ledger and update the epic rollup in the same change.

## Model rates (USD per 1M tokens)

| Model (`-m` flag) | $/M in | $/M out |
|---|---|---|
| `deepseek/deepseek-v4-flash` | 0.14 | 0.28 |
| `deepseek/deepseek-v4-pro` | 0.44 | 0.87 |
| `fireworks/.../minimax-m3` | 0.30 | 0.20 |
| `fireworks/.../glm-5p2` | 0.40 | 0.40 |

> Source of truth for prices: `config/crush.json` in the agentic repo. Update both
> if rates change.

## Ledger

| Date | Issue | Epic | Model | Tokens in | Tokens out | Cost (USD) |
|---|---|---|---|---|---|---|
| 2026-06-18 | #20 channel map (dvb-si) | #1 | deepseek-v4-pro (flash research aborted) | ~141k | n/a¹ | 0.20 |

¹ crush reports a cumulative session `cost` ($0.20) but only last-turn token counts; cost is the reliable figure.

## Per-epic rollup

| Epic | Issues delegated | Total cost (USD) |
|---|---|---|
| #1 demux | 1 | 0.20 |
| #2 ac3 | 0 | 0.00 |
| #3 video | 0 | 0.00 |
| #4 sync | 0 | 0.00 |
| #5 wasm+shell | 0 | 0.00 |
| #6 deinterlace | 0 | 0.00 |
| #7 live/fallback | 0 | 0.00 |
| #8 fixtures/CI/harness | 0 | 0.00 |
| **Total** | **1** | **0.20** |
