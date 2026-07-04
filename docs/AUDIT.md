# Skyfire quality & completeness audit — reproducible plan

> Purpose: a **repeatable, improvable** methodology for auditing skyfire's code and
> docs. Run it on demand (before a release, after a big change, or periodically).
> Each run produces a dated findings report in `docs/superpowers/audits/`.
> Improve the plan by editing this file — add dimensions, tighten pass criteria,
> add tooling. Findings that recur become new gate checks.

## How to run

1. Ensure a clean tree on `main` (or the branch under audit).
2. Run the **baseline gate** (§A) yourself; record the raw output.
3. Fan out one read-only auditor per dimension (§B–§H) — they may run in parallel;
   each returns findings as `severity | path:line | problem | fix` (severity ∈
   Critical / Important / Minor). Auditors do NOT change code.
4. Synthesize into `docs/superpowers/audits/YYYY-MM-DD-audit-report.md`: per-dimension
   findings, a prioritized fix list, and a health scorecard.
5. Triage: Critical/Important → issues or immediate fixes; Minor → backlog. Feed
   anything that should never regress back into §A as an automated gate.

## Scoring

Each dimension gets GREEN (no Important+), YELLOW (Important findings, no Critical),
or RED (any Critical). The report's scorecard lists all eight.

---

## §A — Gate & build health (run directly)

**What:** the exact CI gate plus deeper lints and the browser oracle.
**Method:**
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings          # CI gate (must be zero)
cargo clippy --workspace --all-targets -- -W clippy::pedantic -W clippy::nursery 2>&1 | rg '^warning' | wc -l   # advisory delta
cargo build --workspace
cargo nextest run --workspace
cargo build -p skyfire-wasm --target wasm32-unknown-unknown
# browser (not in cargo CI): wasm-pack build --target web → serve → playwright
PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH" \
  wasm-pack build crates/skyfire-wasm --target web --release --out-dir web/pkg
bash scripts/make-hls-fixture.sh
(cd web && PORT=8080 bun run serve.ts &) && (cd web && bunx playwright test tests/e2e.spec.mjs --browser=chromium)
```
**Pass:** fmt/clippy/build/nextest all green; wasm32 builds; e2e all pass (the
headless no-audio-device path is gated, not a failure). Record the pedantic/nursery
count as a trend line, not a gate.

## §B — Test quality & coverage (auditor)

**What:** are the tests real oracles, and what's untested?
**Check:** every crate's public behaviour has a test asserting on OUTPUT (decoded
PCM bytes, frame counts/flags, parsed track metadata, rendered RGBA), not on
internals; no gameable/trivially-true tests (assert-nothing, tautologies); real
fixtures (not only hand-built bytes) exercise parsers; list source paths/modules
with no test coverage; flaky/env-dependent assertions are gated (see the
audio-device precedent). Note where a fuzz/malformed-input test is missing on a
parser that eats untrusted bytes.
**Pass:** no gameable tests; each crate's core behaviour has a fixture-driven
oracle; coverage gaps enumerated with risk.

## §C — Rust code quality (auditor)

**What:** correctness & maintainability of the Rust.
**Check:** `unwrap`/`expect`/`panic!`/`unreachable!`/indexing in **non-test library
paths** (a panic in the WASM bridge breaks the player — these must be justified or
removed); error handling (thiserror usage, no swallowed errors); dead/unused code;
`#[non_exhaustive]` match arms that silently swallow a variant needing handling;
module cohesion & file size (skyfire-sync 1899 / skyfire-wasm 1996 lines — is each
one responsibility?); the top clippy::pedantic/nursery findings worth fixing;
confirm **zero `unsafe`** (constraint).
**Pass:** no unjustified panic paths in library code; no dead code; no silent
event/variant drops; oversized files flagged with a split proposal.

## §D — Dependency hygiene (run tools + auditor interprets)

**What:** lean, current, licence-clean deps.
**Method:**
```bash
cargo +nightly udeps --workspace 2>&1 | rg -A2 'unused'   # unused deps
cargo machete 2>&1                                         # unused deps (stable)
cargo tree -d                                              # duplicate versions
cargo tree -e features | rg -i 'transmux|dvb|mpeg|broadcast'  # version currency
```
Cross-check pins against crates.io latest. Confirm no `dvb-pes`/`mpeg-ts` remain as
direct skyfire deps (dropped in Part 2). Confirm dual MIT-OR-Apache-2.0 on every
crate + LICENSE files present.
**Pass:** zero unused deps; no unexpected duplicate versions; pins current or a
noted reason; licence fields + files correct.

## §E — Docs accuracy & completeness (auditor)

**What:** docs match reality and cover the system.
**Check:** ADR index (`docs/decisions/README.md`) lists every ADR, numbering
contiguous, none edited-after-Accepted (superseded not rewritten); `OBJECTIVES.md`
status rows match shipped reality; every spec in `docs/superpowers/specs/` has a
matching shipped or explicitly-parked state; per-crate top-level rustdoc (`//!`)
exists and is accurate; **stale references** — grep docs for file/function/flag
names and verify they still exist (e.g. anything naming `h264_config`, `EsDemux`,
`dvb-pes` is now wrong); "cite or don't write" — behavioural claims carry a spec
section / fixture / date; npm package READMEs (`packages/core`, `packages/player`)
match the published API; `COSTS.md` present/consistent.
**Pass:** ADR index complete + contiguous; OBJECTIVES current; no stale
file/symbol references in docs; each crate has accurate module docs.

## §F — Public API / contract (auditor)

**What:** the external surface is consistent and matches its types.
**Check:** the `skyfire-wasm` `SkyfireBridge` exported API vs `@skyfire/core`
`index.d.ts` and `@skyfire/player` `index.d.ts` — every exported method/event typed
and present; the **codec-string casing inconsistency** (`WasmEngine::probe` emits
`Ac3`/`EAc3`/`Mp2` vs `WasmTrackList` `AC3`/`EAC3`/`MP2` — a known real defect);
naming consistency across the JS↔WASM boundary; JS API stability (PID-addressable
selection); no undocumented public items.
**Pass:** .d.ts matches implementation; codec strings consistent across all APIs;
public items documented.

## §G — Spec/ADR conformance & constraints (auditor)

**What:** the code honours the decisions and hard constraints.
**Check:** ADR 0001 (browser/platform support; H.265 gate via
`VideoDecoder.isConfigSupported`; H.264 fallback) — is codec support actually
gated? ADR 0008 (video-only transcode; audio/subs/PCR passthrough — client never
re-encodes audio; `oxideav-h264` absent from the browser); ADR 0009 (fMP4/MSE
fallback present + selected when WebCodecs H.264 unavailable). Constraints: **no
`unsafe`**, **dual MIT-OR-Apache-2.0**, **no `Co-Authored-By` in commits**, touch
only needed crates.
**Pass:** each Accepted ADR's mandate is reflected in code; all constraints hold.

---

## §H — Duplication, consolidation & dead code (auditor)

**What:** DRY violations, mergeable near-duplicate functions, and truly-unreachable
code — across both Rust and the JS (`web/`, `packages/`).
**Check:**
- **Copy-paste / near-duplicate blocks** — the same logic repeated across crates or
  files (e.g. TS-packet field extraction, PTS/tick math, descriptor walking, NAL
  iteration, `source_timing`→pts mapping, segment/box parsing). Flag each cluster
  with the locations and a single-home proposal.
- **Similar functions that should merge** — functions with the same shape differing
  only in a constant/type (candidates for one generic/parameterised fn); parallel
  audio paths (AC-3 / E-AC-3 / MP2 decode dispatch) that could share a trait/helper;
  duplicated JS between `web/` example glue and `packages/player`.
- **Dead code** — `pub`/private items with no caller (cross-check with §D tooling
  and `cargo build` `dead_code` warnings under a `#[warn(dead_code)]` sweep);
  unreferenced test helpers; JS functions never imported; leftover shims from the
  demux rewrite; `#[allow(dead_code)]` that can now be removed.
- **Method:**
```bash
# dead-code surfacing (Rust)
RUSTFLAGS="-W dead_code -W unused" cargo build --workspace 2>&1 | rg 'never used|never read|dead_code'
# textual duplication (structural review by the auditor; optional tool if present)
command -v similarity-rs >/dev/null && similarity-rs crates/ || true
rg -n 'fn ' crates/*/src | wc -l   # inventory for the auditor to cluster
```
**Pass:** no verbatim-duplicated logic block > ~15 lines without a shared home; no
dead code (or each `#[allow(dead_code)]` justified); merge candidates listed with a
concrete consolidation and the risk. Every proposed merge must preserve the §A gate
+ §B oracles — propose, don't auto-apply during the audit.

## Improving this plan

- When a finding recurs across runs, promote its check into §A as an automated gate
  (a test, a clippy lint, a CI grep).
- Add a dimension when a new subsystem lands (e.g. §H for a future fMP4-ingest path).
- Tighten pass criteria as the code matures (e.g. raise coverage floors, add a
  malformed-input fuzz corpus, add `cargo-deny` once installed for licence/advisory
  gating).
- Keep each dimension independent and read-only so runs are parallelisable and comparable.
