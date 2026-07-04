# Skyfire quality & completeness audit — 2026-07-04

Run of [docs/AUDIT.md](../../AUDIT.md) (§A–§H) on `main` @ `d53faa4`. Method:
baseline gate + dep tools run directly; §B/C/E/F/G/H by parallel read-only auditors.

## Scorecard

| Dim | Area | Score | One-line |
|---|---|---|---|
| §A | Gate & build | **GREEN** | fmt/clippy(-D)/build/nextest 99/99 all green; wasm32 ok; e2e 5/5. 176 pedantic/nursery (advisory). |
| §B | Test quality & coverage | **YELLOW** | No gameable tests; gaps = no-panic tests on untrusted parsers, 1 assert-nothing test, no AC-3 Engine oracle, 7 dead fixtures. |
| §C | Rust code quality | **YELLOW** | 0 unsafe. mpa double-init panic path; ac3 `expect`; 3 swallowed bridge errors (no JS signal); codec-string wildcards; 2 oversized files; u16 truncation. |
| §D | Dependency hygiene | **GREEN** | Dual-licence ok (workspace-inherited) + LICENSE files; no dup versions; no dvb-pes/mpeg-ts stragglers. One unused dep (cli→skyfire-ac3). |
| §E | Docs accuracy | **YELLOW** | ADR index contiguous + rustdoc good; stale `EsDemux`/`TsResync` in *deprecated* plan/specs (need SUPERSEDED banners); COSTS timestamp; npm README type. |
| §F | Public API / contract | **YELLOW** | Codec-string casing inconsistency (`Ac3` vs `AC3`) across two APIs; `.d.ts` lacks codec union; `WasmEngine` undocumented. |
| §G | ADR/constraint conformance | **GREEN** | All Accepted ADRs (0001/0008/0009) honoured; 0 unsafe, dual licence, no Co-Authored-By, scoped commits. |
| §H | Duplication & dead code | **YELLOW** | avcC config-build dup ×3; codec-string map dup; dead `decode_eac3_packet`, `parse_subtitle_pes`, `bridge-guard-test.js`; Engine AC-3 correctness. |

**Overall: healthy.** 0 Critical. The build, ADR conformance, and constraints are
solid; the yellows are consolidation, robustness (no-panic + error visibility),
one real API defect (codec casing), and one latent legacy-path bug (Engine AC-3).

## Cross-cutting themes (deduped across dimensions)

1. **Codec-string casing defect** (§F, §H-C3, §C). `WasmEngine::probe` emits
   `Ac3/EAc3/Mp2` (lib.rs:137-142) but `SkyfireBridge::track_list` emits
   `AC3/EAC3/MP2` (lib.rs:614-617); tests + player rely on the uppercase bridge
   form; `.d.ts` has no codec union. Live player is self-consistent (bridge only),
   so not Critical — but any JS mixing both APIs breaks.
2. **Legacy Engine / WasmEngine batch path** (§B, §H, §F, §C). `skyfire-core::
   Engine::decode_audio` (lib.rs:355) uses `decode_all_eac3` — **E-AC-3 only** —
   so base AC-3 through the Engine/CLI path yields garbage/empty PCM (the live
   player uses `IncrementalDecoder` and is fine). The batch `WasmEngine`/`Engine`
   path is superseded by `SkyfireBridge`, undocumented in `.d.ts`, and carries the
   dead `decode_eac3_packet`. **Decide: fix Engine to use `IncrementalDecoder`, or
   deprecate/remove the legacy batch path.**
3. **Silent failure in the bridge** (§C, §H). `let _ =` swallows
   `decode_au`/`seg.push`/`seg.flush` errors (lib.rs:740,865,888,908) — bad
   audio/segments fail invisibly with no JS-observable signal.
4. **Untrusted-input robustness** (§B, §C). Parsers that eat live TS bytes
   (`mpa::decode_au`, `subtitle_compositor::feed_pes`, subtitle_compositor u16
   truncation) lack no-panic tests + have a latent panic/truncation path — a panic
   in WASM breaks the player.
5. **Duplication** (§H). avcC config-building ×3 (extract a helper); codec-string
   map (single fn); S16LE→f32, JS `ticksToMicros`/`PTS_HZ`, `load_fixture` test
   helper.

## Prioritised fix plan

**P0 — correctness / real defects**
- Unify audio codec strings: one `audio_codec_str(AudioCodec)->&'static str` in
  skyfire-ts (canonical **uppercase**), used by both probe + track_list; drop the
  `_ =>` wildcards (make matches exhaustive); add the codec literal-union to
  `packages/core/index.d.ts`. (§F/§H/§C)
- Engine AC-3: switch `skyfire-core::Engine::decode_audio` to `IncrementalDecoder`
  (both codecs) **or** deprecate the legacy `Engine`/`WasmEngine` batch path;
  add an AC-3 (base) oracle if kept. (§B/§H)
- `subtitle_compositor` u16 truncation on `Vec::len()` (subtitle_compositor.rs:729)
  → `u16::try_from(..).unwrap_or(..)` + warn. (§C)
- mpa double-init panic path (mpa/lib.rs:86) → early `Ok(None)` when decoder `None`;
  ac3 decoder-build `expect` (ac3/lib.rs:86) → propagate `Result`. (§C)

**P1 — robustness & observability**
- Replace `let _ =` on bridge decode/segmenter with logging + a JS-readable
  `decode_error_count`/`segmenter_error` (lib.rs:740,865,888,908). (§C)
- Add no-panic/malformed-input tests: `mpa::decode_au`, `subtitle_compositor::
  feed_pes`; make `ac3 truncated_frame_no_panic` actually assert (it `let _`s the
  result today). (§B)
- Delete dead code: `decode_eac3_packet` (zero callers), `parse_subtitle_pes` +
  `SubtitleCue` if test-only (rewrite the one test to the compositor path),
  `web/bridge-guard-test.js` (stale release path, unreachable),
  `video_is_interlaced` stub (deprecate). (§H)

**P2 — maintainability**
- Extract `build_avcc_config(record)->(String,Vec<u8>)` shared by core + wasm;
  collapse wasm `on_track_added`/`on_track_updated` dup. (§H-C1)
- Split oversized files: `skyfire-sync/lib.rs` (1899L) → pts/clock/queue/controller;
  `skyfire-wasm/lib.rs` (1996L) → engine/types/bridge/helpers. (§C)
- JS/de-dup: export `ticksToMicros`+`PTS_HZ` from `@skyfire/core`; expose
  `s16le_slice_to_f32` from `skyfire_ac3::downmix`; shared `load_fixture` test
  helper. (§H-C4/C5/C2)
- Batch the high-value pedantic lints (`map_or`, `let_else`, redundant clone/closure).

**P3 — docs & hygiene**
- SUPERSEDED banners on `docs/superpowers/plans/2026-07-01-adopt-transmux.md` +
  parked specs; fix their stale `EsDemux`/`TsResync` code snippets. (§E)
- COSTS.md `Last updated` timestamp; verify + fix npm README `PcmChunk.samples`
  type (`Float32Array`); OBJECTIVES Part-2 PR cite. (§E)
- Remove unused `skyfire-ac3` dep from `skyfire-cli/Cargo.toml`. (§D)

## Execution status (updated 2026-07-04)

**P0 — DONE** (delegated to deepseek-v4-flash, each verified against the full gate + browser e2e):
- Engine base-AC-3 via IncrementalDecoder — PR #72.
- Codec-string unification (§F) + robustness (§C: mpa/ac3 panics, subtitle u16
  truncation, swallowed bridge errors → logged + counters) + dead-code removal
  (§H: decode_eac3_packet, parse_subtitle_pes/SubtitleCue, bridge-guard-test.js,
  video_is_interlaced, unused cli dep) — PR #73.
- 5.1 multichannel (the AC-3++ ask): investigated — **already built + channel-order
  correct** (oxideav-ac3 reorders AC-3→WAVE via `wave_order`; player passes through
  discretely; downmix fallback). Nothing to build; live 5.1 verification is
  hardware-gated. Not a defect.

**Dimension re-score after P0:** §F GREEN (was YELLOW); §C/§H improved to near-GREEN
(remaining items are P2 maintainability, below).

**Remaining (P1/P2/P3), not yet executed:**
- P2 maintainability: avcC config-build dedup (×3), module splits (skyfire-sync,
  skyfire-wasm), JS de-dup (ticksToMicros/PTS_HZ, s16le helper, load_fixture),
  high-value pedantic lints.
- P1 tests: remaining no-panic coverage already largely added in WP2; a base-AC-3
  Engine oracle now exists (PR #72). 7 dead fixtures (g0–g6, m6-clean) still
  unreferenced — remove or add smoke tests.
- P3 docs: SUPERSEDED banners on the old plan/parked specs, COSTS timestamp, npm
  README PcmChunk type, OBJECTIVES PR cites.

## Feed back into the gate (§A)

Promote to automated checks so these never regress:
- `cargo machete` (unused deps) in CI.
- A test asserting probe + track_list emit **identical** codec strings.
- Keep the no-panic parser tests as permanent fuzz-regression anchors.
