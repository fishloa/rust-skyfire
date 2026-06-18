# 0002 — Delegation working practice

- **Status:** Accepted
- **Date:** 2026-06-18

## Context

Skyfire is an AI-orchestration experiment: Claude orchestrates and verifies;
cheaper external models write the implementation. We need one repeatable loop
that keeps Claude's expensive tokens for review and spends the delegate's cheap
tokens on the grunt work — with the cost made visible.

## Decision

1. **Every work unit is a GitHub issue with explicit exit gates.** Claude writes
   the issue as a *feature + verifiable exit criteria* (per the `delegate` skill's
   brief rules), never a step-by-step. Exit gates = the full CI gate plus a
   concrete behavioural check (fixture decode / golden bytes), stated as what must
   hold. The strongest gate is a failing test the delegate must turn green.

2. **Delegate via the `delegate` skill (crush), not `scripts/delegate.sh`.** Run
   headless: `crush run -m <model> -c <repo> --quiet "<brief> [run-tag:<slug>]"`,
   logging to gitignored `.delegate/<slug>.log`.

3. **Pick the cheapest capable model; ramp up only on no-progress:**
   `deepseek-v4-flash` → `deepseek-v4-pro` → `minimax-m3` → `glm-5p2`.
   Start low, escalate a tier when the delegate genuinely thrashes (not on round
   count — 20+ rounds is normal), downgrade for cleanup rounds.

4. **Claude verifies, never trusts self-report.** Run the full CI gate locally;
   read the diff. Green → commit (no `Co-Authored-By`) and `gh issue close N` with
   an evidence paragraph. Fail → continue the same crush session (`-s <uuid>`).

5. **Record cost every run.** After each delegate run, log tokens + cost to
   [`../COSTS.md`](../COSTS.md) (rates + method documented there), rolled up per
   epic. See ADR-linked ledger.

## Consequences

- No issue gets delegated until it carries exit gates.
- `.delegate/` is gitignored; logs are append-only for forensics.
- One model tier runs at a time per issue; parallel issues need disjoint files
  (or a worktree each).
- Cost per feature/epic is always queryable from `COSTS.md`.
