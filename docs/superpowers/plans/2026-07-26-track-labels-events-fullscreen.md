# Track Labels, Track-Existence Events, Fullscreen API — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the player's track pickers human-readable, make track-set changes observable to the UI, and give hosts a real fullscreen API.

**Architecture:** Four independent units. A new published JS module maps ISO 639-2 codes to display names via `Intl`. A Rust header-only bitstream probe adds a per-PID channel count to the track list without decoding. The player replaces count-based change detection with deep identity plus a diff, and auto-recovers when the selected audio PID vanishes. The custom element gains fullscreen methods, a state event, and an iOS fallback.

**Tech Stack:** Vanilla ES modules (`packages/player`), `bun test`, Rust 1.94 (`skyfire-ac3`, `skyfire-ts`, `skyfire-wasm`), `wasm-bindgen`, Playwright/Chromium, TypeScript `.d.ts` only (no TS build).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-26-track-labels-events-fullscreen-design.md`
- No `unsafe` anywhere, including bitstream parsing (project CLAUDE.md).
- CI gate must pass before every commit: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings), `cargo build --workspace`, `cargo nextest run --workspace`, `npx -y -p typescript@5 tsc --noEmit -p packages/player`.
- No `Co-Authored-By` lines in commits.
- Dual licence MIT OR Apache-2.0; new files need no licence header (none of the existing ones carry one).
- Every behavioural claim needs a spec section, a fixture, or "verified <date>".
- Published tarball must stay minimal: anything added to `packages/player` that is not runtime code must be excluded from `package.json` `files`.
- Never present a guessed channel count. Absent data means the label degrades, not that a plausible number is invented.
- Reuse `rust-dvb` crates rather than hand-rolling PSI parsing.

**Prerequisite:** #89 (audio flip broken on all 6 alt-PID streams) should land before Task 3 — its reselect logic touches the same code path. Tasks 1, 2 and 4 have no such dependency and can proceed regardless.

---

### Task 1: Language display names (`lang.js`)

**Files:**
- Create: `packages/player/lang.js`
- Create: `packages/player/lang.test.js`
- Modify: `packages/player/package.json` (add `lang.js` to `files`)
- Modify: `packages/player/index.d.ts` (declare the new exports)
- Modify: `packages/player/skyfire-element.js:289-301` (use it in the pickers)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `languageName(code: string | null | undefined, locale?: string, overrides?: Record<string,string>): string | null`
  - `resolveLocale(el: Element | null): string`
  - `ISO_639_2B_TO_1: Record<string,string>` (exported for testing only)

- [ ] **Step 1: Write the failing test**

Create `packages/player/lang.test.js`:

```js
import { test, expect } from "bun:test";
import { languageName, resolveLocale, ISO_639_2B_TO_1 } from "./lang.js";

// ── ISO 639-2/B (bibliographic) codes must resolve. Real PMTs carry these:
// fixtures/streams.json has france-2 emitting BOTH "fre" (pid 257) and
// "fra" (pid 258) for French.
test("maps bibliographic 639-2/B codes to display names", () => {
  expect(languageName("fre", "en")).toBe("French");
  expect(languageName("ger", "en")).toBe("German");
  expect(languageName("dut", "en")).toBe("Dutch");
  expect(languageName("gre", "en")).toBe("Greek");
});

test("maps terminological 639-2/T codes to display names", () => {
  expect(languageName("fra", "en")).toBe("French");
  expect(languageName("deu", "en")).toBe("German");
  expect(languageName("ita", "en")).toBe("Italian");
});

test("covers all 20 B/T divergences", () => {
  expect(Object.keys(ISO_639_2B_TO_1)).toHaveLength(20);
  for (const [b, one] of Object.entries(ISO_639_2B_TO_1)) {
    expect(b).toMatch(/^[a-z]{3}$/);
    expect(one).toMatch(/^[a-z]{2}$/);
  }
});

test("localises to the requested locale", () => {
  expect(languageName("fra", "de")).toBe("Französisch");
  expect(languageName("eng", "fr")).toBe("anglais");
});

// ── qaa-qtz is the ISO 639-2 reserved-for-local-use range; DVB broadcasters
// use it for original/undefined audio. CLDR has no name for it.
test("names the qaa-qtz reserved range as original audio", () => {
  expect(languageName("qaa", "en")).toBe("Original audio");
  expect(languageName("qad", "en")).toBe("Original audio");
  expect(languageName("qtz", "en")).toBe("Original audio");
});

test("honours an overrides map for the reserved range", () => {
  expect(languageName("qaa", "de", { qaa: "Originalton" })).toBe("Originalton");
});

// ── `mis` ("uncoded languages") is a real ISO 639-2 code that CLDR does NOT
// name — `Intl.DisplayNames(…, {fallback:"none"}).of("mis")` is undefined
// (verified 2026-07-26, bun/ICU). orf1 pid 258 ships it, so without an entry
// the picker would read "MIS · MP2".
test("names ISO 639-2 codes that CLDR has no name for", () => {
  expect(languageName("mis", "en")).toBe("Uncoded language");
});

// ── These three DO resolve through CLDR, so they must not be table entries.
test("leaves CLDR-known special codes to Intl", () => {
  expect(languageName("und", "en")).toBe("Unknown language");
  expect(languageName("zxx", "en")).toBe("No linguistic content");
  expect(languageName("mul", "en")).toBe("Multiple languages");
});

// ── rai-1 pid 258 carries "oth", which is not a valid ISO 639-2 code.
test("passes unresolvable codes through uppercased", () => {
  expect(languageName("oth", "en")).toBe("OTH");
  expect(languageName("zzz", "en")).toBe("ZZZ");
});

test("returns null for absent codes so callers can fall back", () => {
  expect(languageName(null)).toBeNull();
  expect(languageName(undefined)).toBeNull();
  expect(languageName("")).toBeNull();
  expect(languageName("   ")).toBeNull();
});

test("normalises case and whitespace", () => {
  expect(languageName(" FRE ", "en")).toBe("French");
});

test("falls back to uppercase when Intl.DisplayNames is unavailable", () => {
  const saved = globalThis.Intl.DisplayNames;
  try {
    globalThis.Intl.DisplayNames = undefined;
    expect(languageName("fre", "en")).toBe("FRE");
  } finally {
    globalThis.Intl.DisplayNames = saved;
  }
});

// ── locale resolution walks the DOM the way lang inheritance does.
test("resolveLocale prefers the element's own lang", () => {
  const el = { getAttribute: (n) => (n === "lang" ? "de" : null), closest: () => null };
  expect(resolveLocale(el)).toBe("de");
});

test("resolveLocale falls back to the nearest lang ancestor", () => {
  const ancestor = { getAttribute: () => "fr" };
  const el = { getAttribute: () => null, closest: (sel) => (sel === "[lang]" ? ancestor : null) };
  expect(resolveLocale(el)).toBe("fr");
});

test("resolveLocale falls back to navigator when nothing is declared", () => {
  const el = { getAttribute: () => null, closest: () => null };
  expect(typeof resolveLocale(el)).toBe("string");
  expect(resolveLocale(el).length).toBeGreaterThan(0);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/player && bun test lang.test.js`
Expected: FAIL — `Cannot find module './lang.js'`

- [ ] **Step 3: Write minimal implementation**

Create `packages/player/lang.js`:

```js
// Display names for the ISO 639-2 three-letter language codes that arrive in
// broadcast PMTs (ETSI EN 300 468 §6.2.4, ISO_639_language_descriptor).
//
// The heavy lifting is Intl.DisplayNames, which is already localised for every
// locale the browser supports. Two things it cannot do on its own:
//
//   1. Bibliographic (639-2/B) codes. Some ICU builds alias "fre" to French
//      already, others do not; real streams carry both forms (france-2 emits
//      "fre" on pid 257 and "fra" on pid 258 for the same language). Mapping
//      B -> 639-1 first makes the result deterministic across runtimes rather
//      than dependent on the host's ICU version.
//   2. Codes CLDR has no name for. The qaa-qtz reserved-for-local-use range
//      (which DVB broadcasters use for original/undefined audio) and "mis"
//      (uncoded languages, shipped by orf1 pid 258) both come back undefined.
//      Those strings are ours and therefore NOT localised — pass `overrides`
//      to supply your own wording.
//
// "und", "mul" and "zxx" DO resolve through CLDR and must not be tabulated.

/** The 20 ISO 639-2 codes whose bibliographic form differs from 639-1. */
export const ISO_639_2B_TO_1 = {
  alb: "sq", arm: "hy", baq: "eu", bur: "my", chi: "zh",
  cze: "cs", dut: "nl", fre: "fr", geo: "ka", ger: "de",
  gre: "el", ice: "is", mac: "mk", mao: "mi", may: "ms",
  per: "fa", rum: "ro", slo: "sk", tib: "bo", wel: "cy",
};

/** Reserved-for-local-use range: qaa through qtz. */
const RESERVED_RANGE = /^q[a-t][a-z]$/;

/**
 * Codes with no CLDR name. Verified 2026-07-26 against bun/ICU:
 * `Intl.DisplayNames(…, {fallback:"none"}).of(code)` returns undefined for
 * these. NOT localised — override per host if that matters.
 */
const NO_CLDR_NAME = {
  mis: "Uncoded language",
};
const RESERVED_DEFAULT = "Original audio";

// Intl.DisplayNames construction is not free and the picker rebuilds on every
// `tracks` event, so instances are cached per locale.
const cache = new Map();

function displayNames(locale) {
  if (typeof Intl?.DisplayNames !== "function") return null;
  if (cache.has(locale)) return cache.get(locale);
  let inst = null;
  try {
    inst = new Intl.DisplayNames([locale], { type: "language", fallback: "none" });
  } catch {
    inst = null;
  }
  cache.set(locale, inst);
  return inst;
}

/**
 * Human-readable name for an ISO 639-2 language code.
 *
 * Returns `null` when there is no code at all, so callers can fall back to a
 * positional label ("Track 2") rather than printing something meaningless.
 * Returns the code uppercased when it cannot be resolved — `oth` (which
 * rai-1 pid 258 carries, and which is not valid ISO 639-2) becomes "OTH".
 */
export function languageName(code, locale = "en", overrides = {}) {
  if (typeof code !== "string") return null;
  const raw = code.trim().toLowerCase();
  if (!raw) return null;

  if (Object.hasOwn(overrides, raw)) return overrides[raw];
  if (RESERVED_RANGE.test(raw)) return RESERVED_DEFAULT;

  const tag = ISO_639_2B_TO_1[raw] ?? raw;
  const name = displayNames(locale)?.of(tag);
  if (name) return name;
  return NO_CLDR_NAME[raw] ?? raw.toUpperCase();
}

/**
 * Display locale for an element, following how `lang` inheritance works:
 * the element's own attribute, then the nearest ancestor that declares one,
 * then the document, then the browser.
 */
export function resolveLocale(el) {
  const own = el?.getAttribute?.("lang");
  if (own) return own;
  const near = el?.closest?.("[lang]")?.getAttribute?.("lang");
  if (near) return near;
  const doc = globalThis.document?.documentElement?.lang;
  if (doc) return doc;
  return globalThis.navigator?.language || "en";
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/player && bun test lang.test.js`
Expected: PASS — 14 tests.

All CLDR-dependent expectations in this test were verified against bun/ICU on 2026-07-26: `of("fr")` in `de` → `Französisch`, `of("en")` in `fr` → `anglais`, `und`/`zxx`/`mul` resolve, `mis`/`oth` do not. If a future ICU build disagrees, fix the table or the code — do **not** relax an expectation to match whatever came out.

- [ ] **Step 5: Commit**

```bash
git add packages/player/lang.js packages/player/lang.test.js
git commit -m "feat(player): ISO 639-2 language display names via Intl"
```

- [ ] **Step 6: Wire it into the pickers**

Modify `packages/player/skyfire-element.js`. Add to the imports at the top of the file:

```js
import { languageName, resolveLocale } from "./lang.js";
```

Replace lines 289-301 (the two `forEach` blocks that build the menus):

```js
    const locale = resolveLocale(this);

    // Broadcasters routinely ship two tracks in the same language (arte pid
    // 257 and 258 are both fra). Where a name repeats, number the repeats so
    // the rows stay distinguishable; unique names stay bare.
    const nameFor = (t, i, fallback) => languageName(t.language, locale) || `${fallback} ${i + 1}`;
    const audioNames = (tl.audio || []).map((a, i) => nameFor(a, i, "Track"));
    const seen = new Map();
    const audioLabels = audioNames.map((n, i) => {
      const a = tl.audio[i];
      const dup = audioNames.filter((x) => x === n).length > 1;
      const chan = a.channels === 6 ? " 5.1" : a.channels === 1 ? " mono" : "";
      let label = `${n} · ${a.codec}${chan}`;
      if (dup) {
        const nth = (seen.get(label) ?? 0) + 1;
        seen.set(label, nth);
        if (nth > 1) label = `${label} (${nth})`;
      }
      return label;
    });

    const am = menu("audio");
    (tl.audio || []).forEach((a, i) => {
      row(am, audioLabels[i], this._selAudio === a.pid || (this._selAudio == null && i === 0),
        () => { this._selAudio = a.pid; this.selectAudio(a.pid); this._buildMenus(); am.classList.add("open"); });
    });

    const sm = menu("subtitle");
    row(sm, "Off", this._selSub == null, () => { this._selSub = null; this.selectSubtitle(null); this._buildMenus(); sm.classList.add("open"); });
    (tl.subtitles || []).forEach((s, i) => {
      const label = nameFor(s, i, "Subtitle");
      row(sm, label, this._selSub === s.pid, () => { this._selSub = s.pid; this.selectSubtitle(s.pid); this._buildMenus(); sm.classList.add("open"); });
    });
```

Note `a.channels` is `undefined` until Task 2 lands; `chan` is then `""` and the label degrades to `French · EAC3`, which is the intended behaviour.

- [ ] **Step 7: Declare the exports and ship the file**

In `packages/player/index.d.ts`, append:

```ts
export function languageName(
  code: string | null | undefined,
  locale?: string,
  overrides?: Record<string, string>,
): string | null;

export function resolveLocale(el: Element | null): string;
```

In `packages/player/package.json`, add `"lang.js"` to the `files` array (after `"hls-source.js"`). `lang.test.js` must NOT be added — the array is a whitelist, so it is excluded automatically.

- [ ] **Step 8: Verify the tarball and typings**

```bash
cd packages/player && npm pack --dry-run 2>&1 | grep -E "lang|total files"
npx -y -p typescript@5 tsc --noEmit -p packages/player
```

Expected: `lang.js` listed, `lang.test.js` absent, total files 8 (was 7). `tsc` exits 0.

- [ ] **Step 9: Commit**

```bash
git add packages/player/skyfire-element.js packages/player/index.d.ts packages/player/package.json
git commit -m "feat(player): render language names in the track pickers"
```

---

### Task 2: Per-track channel count (header-only probe)

**Files:**
- Create: `crates/skyfire-ac3/src/header.rs`
- Create: `crates/skyfire-ts/src/mp2_header.rs`
- Modify: `crates/skyfire-ac3/src/lib.rs` (add `pub mod header;`)
- Modify: `crates/skyfire-ts/src/lib.rs` (add `pub mod mp2_header;`)
- Modify: `crates/skyfire-wasm/src/bridge_dto.rs:24-33` (add `channels`)
- Modify: `crates/skyfire-wasm/src/bridge.rs:179-192` (populate it), `:419-433` (probe hook)
- Modify: `packages/core/index.d.ts:9-13` (declare `channels`)
- Test: `crates/skyfire-ac3/src/header.rs` (unit tests inline), `crates/skyfire-wasm/tests/audio_channels.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `skyfire_ac3::header::channels_from_syncframe(buf: &[u8]) -> Option<u8>`
  - `skyfire_ts::mp2_header::channels_from_header(buf: &[u8]) -> Option<u8>`
  - `WasmAudioTrack.channels: Option<u8>` → `TrackList.audio[].channels?: number` in JS

**Spec basis.** `bsid` sits at bit offset 40 in **both** AC-3 and E-AC-3 — deliberately, so a decoder can dispatch on it (which is exactly what `oxideav_ac3::decoder::make_decoder` does, per the comment in `crates/skyfire-ac3/Cargo.toml`). So one entry point suffices rather than the two the spec sketched.

- AC-3: `syncinfo` = syncword(16) crc1(16) fscod(2) frmsizecod(6) → 40 bits. Then `bsi`: bsid(5) bsmod(3) acmod(3), then `cmixlev`(2) if `acmod & 1 != 0 && acmod != 1`, then `surmixlev`(2) if `acmod & 4 != 0`, then `dsurmod`(2) if `acmod == 2`, then `lfeon`(1). ETSI TS 102 366 §5.4.1–5.4.2.
- E-AC-3: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2), then fscod2(2) if fscod == 3 else numblkscod(2), then acmod(3) lfeon(1), then bsid(5). ETSI TS 102 366 §E.1.2.2.
- `acmod` → channels: `0→2, 1→1, 2→2, 3→3, 4→3, 5→4, 6→4, 7→5`; `+1` if `lfeon`. Table 5.8. So 5.1 is `acmod=7, lfeon=1` → 6.
- MP2: 32-bit header; `mode` is bits 24–25 (the top 2 bits of byte 3). `0b11` = single_channel → 1, else 2. ISO/IEC 11172-3 §2.4.1.3.

- [ ] **Step 1: Write the failing test**

Create `crates/skyfire-ac3/src/header.rs` containing ONLY the tests for now:

```rust
//! Header-only channel-count probe for AC-3 / E-AC-3 sync frames.

#[cfg(test)]
mod tests {
    use super::channels_from_syncframe;

    /// Build a base AC-3 syncframe header with the given acmod/lfeon.
    /// Layout: syncword(16) crc1(16) fscod(2) frmsizecod(6) | bsid(5) bsmod(3) acmod(3) ... lfeon(1)
    fn ac3_header(acmod: u8, lfeon: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits.push_str("0000101101110111"); // syncword 0x0B77
        bits.push_str("0000000000000000"); // crc1
        bits.push_str("00"); // fscod = 48 kHz
        bits.push_str("000000"); // frmsizecod
        bits.push_str("01000"); // bsid = 8 -> base AC-3
        bits.push_str("000"); // bsmod
        bits.push_str(&format!("{acmod:03b}"));
        if acmod & 1 != 0 && acmod != 1 {
            bits.push_str("00"); // cmixlev
        }
        if acmod & 4 != 0 {
            bits.push_str("00"); // surmixlev
        }
        if acmod == 2 {
            bits.push_str("00"); // dsurmod
        }
        bits.push(if lfeon { '1' } else { '0' });
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |acc, b| (acc << 1) | u8::from(*b == b'1')))
            .collect()
    }

    /// E-AC-3: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2)
    ///         numblkscod(2) acmod(3) lfeon(1) bsid(5)
    fn eac3_header(acmod: u8, lfeon: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits.push_str("0000101101110111"); // syncword
        bits.push_str("00"); // strmtyp = 0
        bits.push_str("000"); // substreamid
        bits.push_str("00000000000"); // frmsiz
        bits.push_str("00"); // fscod = 48 kHz (!= 3)
        bits.push_str("11"); // numblkscod = 6 blocks
        bits.push_str(&format!("{acmod:03b}"));
        bits.push(if lfeon { '1' } else { '0' });
        bits.push_str("10000"); // bsid = 16 -> E-AC-3
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |acc, b| (acc << 1) | u8::from(*b == b'1')))
            .collect()
    }

    #[test]
    fn ac3_acmod_table_matches_spec_table_5_8() {
        // ETSI TS 102 366 Table 5.8: acmod -> channel count, before LFE.
        for (acmod, want) in [(0u8, 2u8), (1, 1), (2, 2), (3, 3), (4, 3), (5, 4), (6, 4), (7, 5)] {
            assert_eq!(
                channels_from_syncframe(&ac3_header(acmod, false)),
                Some(want),
                "acmod {acmod}"
            );
        }
    }

    #[test]
    fn ac3_lfe_adds_one_channel() {
        // acmod 7 + lfeon = 3/2.1 = 5.1 = 6 channels.
        assert_eq!(channels_from_syncframe(&ac3_header(7, true)), Some(6));
        assert_eq!(channels_from_syncframe(&ac3_header(2, true)), Some(3));
    }

    #[test]
    fn eac3_acmod_and_lfe_parse_at_annex_e_offsets() {
        assert_eq!(channels_from_syncframe(&eac3_header(2, false)), Some(2));
        assert_eq!(channels_from_syncframe(&eac3_header(7, true)), Some(6));
    }

    #[test]
    fn rejects_non_syncframe_and_short_buffers() {
        assert_eq!(channels_from_syncframe(&[]), None);
        assert_eq!(channels_from_syncframe(&[0x0B, 0x77]), None);
        assert_eq!(channels_from_syncframe(&[0xFF; 16]), None);
    }

    #[test]
    fn real_eac3_fixture_reports_the_ffprobe_channel_count() {
        // fixtures/france2-3s.eac3 is a raw E-AC-3 elementary stream.
        // `ffprobe -show_entries stream=codec_name,channels` -> eac3,2
        // (verified 2026-07-26).
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/france2-3s.eac3"
        ))
        .expect("fixture");
        assert_eq!(channels_from_syncframe(&data), Some(2));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Add `pub mod header;` to `crates/skyfire-ac3/src/lib.rs`, then:

Run: `cargo test -p skyfire-ac3 header 2>&1 | head -20`
Expected: FAIL — `cannot find function channels_from_syncframe in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/skyfire-ac3/src/header.rs`, above the `mod tests`:

```rust
use crate::AC3_SYNCWORD;

/// Channels contributed by each `acmod` value, before LFE.
/// ETSI TS 102 366 Table 5.8.
const ACMOD_CHANNELS: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

/// Minimal big-endian bit reader. No `unsafe`, no allocation.
struct BitReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    /// Reads `n` bits (n <= 16), or `None` past the end of the buffer.
    fn bits(&mut self, n: usize) -> Option<u16> {
        if self.pos + n > self.buf.len() * 8 {
            return None;
        }
        let mut out = 0u16;
        for _ in 0..n {
            let byte = self.buf[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            out = (out << 1) | u16::from(bit);
            self.pos += 1;
        }
        Some(out)
    }
}

/// Channel count for the sync frame at the start of `buf`, read from the
/// header alone — no decode, no decoder state.
///
/// Dispatches base AC-3 vs E-AC-3 on `bsid`, which sits at bit offset 40 in
/// both syntaxes precisely so that a reader can do this. Base AC-3 is
/// `bsid <= 8` (ETSI TS 102 366 §5.4.2.1); E-AC-3 is `bsid` 11–16 (§E.1.3.1.5).
///
/// Returns `None` when the buffer does not start with a sync frame, is too
/// short, or carries an unrecognised `bsid`. Callers MUST treat `None` as
/// "unknown" and never substitute a guess.
#[must_use]
pub fn channels_from_syncframe(buf: &[u8]) -> Option<u8> {
    if !crate::is_ac3_syncframe(buf) {
        return None;
    }
    debug_assert_eq!(
        u16::from(buf[0]) << 8 | u16::from(buf[1]),
        AC3_SYNCWORD,
        "is_ac3_syncframe guarantees the syncword"
    );

    // bsid is at bit 40 in both syntaxes.
    let bsid = BitReader { buf, pos: 40 }.bits(5)?;

    let (acmod, lfeon) = if bsid <= 8 {
        // Base AC-3: syncinfo(40) | bsid(5) bsmod(3) acmod(3) [cmixlev(2)]
        // [surmixlev(2)] [dsurmod(2)] lfeon(1)
        let mut r = BitReader::new(buf);
        r.skip(40 + 5 + 3);
        let acmod = r.bits(3)?;
        if acmod & 1 != 0 && acmod != 1 {
            r.skip(2); // cmixlev
        }
        if acmod & 4 != 0 {
            r.skip(2); // surmixlev
        }
        if acmod == 2 {
            r.skip(2); // dsurmod
        }
        (acmod, r.bits(1)?)
    } else if (11..=16).contains(&bsid) {
        // E-AC-3: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2)
        // [fscod2|numblkscod](2) acmod(3) lfeon(1)
        let mut r = BitReader::new(buf);
        r.skip(16 + 2 + 3 + 11);
        let _fscod = r.bits(2)?;
        r.skip(2); // fscod2 or numblkscod — same width either way
        let acmod = r.bits(3)?;
        (acmod, r.bits(1)?)
    } else {
        return None;
    };

    let base = *ACMOD_CHANNELS.get(usize::from(acmod))?;
    Some(base + u8::from(lfeon == 1))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p skyfire-ac3 header`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/skyfire-ac3/src/header.rs crates/skyfire-ac3/src/lib.rs
git commit -m "feat(ac3): header-only channel-count probe (AC-3 + E-AC-3)"
```

- [ ] **Step 6: Write the failing MP2 test**

Create `crates/skyfire-ts/src/mp2_header.rs`:

```rust
//! Header-only channel-count probe for MPEG-1/2 Layer II frames.

/// Channel count from an MPEG audio frame header.
///
/// `mode` occupies bits 24–25 — the top two bits of byte 3. `0b11` is
/// single_channel (mono); every other value carries two channels
/// (stereo, joint_stereo, dual_channel). ISO/IEC 11172-3 §2.4.1.3.
///
/// Returns `None` unless the buffer starts with a frame sync (11 set bits).
#[must_use]
pub fn channels_from_header(buf: &[u8]) -> Option<u8> {
    if buf.len() < 4 {
        return None;
    }
    // Frame sync: 11 bits all set.
    if buf[0] != 0xFF || (buf[1] & 0xE0) != 0xE0 {
        return None;
    }
    let mode = (buf[3] >> 6) & 0x3;
    Some(if mode == 0b11 { 1 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::channels_from_header;

    fn header(mode: u8) -> [u8; 4] {
        // sync(11)=all ones, version(2)=11 MPEG1, layer(2)=10 LayerII,
        // protection(1)=1, bitrate(4), sampling(2), padding(1), private(1),
        // mode(2) in the top bits of byte 3.
        [0xFF, 0xFD, 0x50, mode << 6]
    }

    #[test]
    fn single_channel_mode_is_mono() {
        assert_eq!(channels_from_header(&header(0b11)), Some(1));
    }

    #[test]
    fn stereo_joint_and_dual_are_two_channels() {
        assert_eq!(channels_from_header(&header(0b00)), Some(2)); // stereo
        assert_eq!(channels_from_header(&header(0b01)), Some(2)); // joint stereo
        assert_eq!(channels_from_header(&header(0b10)), Some(2)); // dual channel
    }

    #[test]
    fn rejects_short_buffers_and_missing_sync() {
        assert_eq!(channels_from_header(&[]), None);
        assert_eq!(channels_from_header(&[0xFF, 0xFD, 0x50]), None);
        assert_eq!(channels_from_header(&[0x00, 0x00, 0x00, 0x00]), None);
    }
}
```

- [ ] **Step 7: Run to verify, then commit**

Add `pub mod mp2_header;` to `crates/skyfire-ts/src/lib.rs`.

Run: `cargo test -p skyfire-ts mp2_header`
Expected: PASS — 3 tests.

```bash
git add crates/skyfire-ts/src/mp2_header.rs crates/skyfire-ts/src/lib.rs
git commit -m "feat(ts): header-only channel-count probe (MPEG Layer II)"
```

- [ ] **Step 8: Write the failing bridge test**

Create `crates/skyfire-wasm/tests/audio_channels.rs`. Model the harness on the existing `crates/skyfire-wasm/tests/audio_channel_consistency.rs` — read it first and reuse its fixture-feeding helper rather than writing a new one.

```rust
//! Per-PID channel counts must appear in the track list without decoding
//! anything but the selected PID (spec unit B, 2026-07-26).

// NOTE TO IMPLEMENTER: copy the bridge-construction + TS-feeding helper from
// tests/audio_channel_consistency.rs. It already handles wasm-bindgen types
// under a native test target. Do not invent a second harness.

/// Expected channels per PID, from
/// `ffprobe -select_streams a -show_entries stream=channels:stream_tags=language`
/// (verified 2026-07-26).
fn assert_channels(file: &str, expected: &[(u16, u8)]) {
    let tl = track_list_for(file);
    let got: Vec<(u16, Option<u8>)> = tl.audio.iter().map(|t| (t.pid, t.channels)).collect();
    let want: Vec<(u16, Option<u8>)> = expected.iter().map(|(p, c)| (*p, Some(*c))).collect();
    assert_eq!(got, want, "{file}");
}

#[test]
fn mixed_codec_stream_reports_channels_for_both_pids() {
    // orf1: AC3 5.1 (deu, pid 257) + MP2 stereo (mis, pid 258). Two codecs,
    // two probe paths, and only ONE of them is the selected/decoded PID —
    // so this fails if the probe only runs on the selected track.
    assert_channels("streams/orf1.ts", &[(257, 6), (258, 2)]);
}

#[test]
fn real_broadcast_mono_mp2_is_detected_as_one_channel() {
    // rai-1 pid 259 is genuinely mono MP2 (mode == 0b11) on real broadcast
    // data, beside three stereo tracks. This is the case a hardcoded
    // "MP2 is always stereo" shortcut would get wrong.
    assert_channels(
        "streams/rai-1.ts",
        &[(257, 2), (258, 2), (259, 1), (260, 2)],
    );
}

#[test]
fn five_one_fixtures_report_six_channels() {
    // ac3-51.ts and eac3-51.ts are 5.1(side) per ffprobe: acmod 7 + lfeon.
    for f in ["ac3-51.ts", "eac3-51.ts"] {
        let tl = track_list_for(f);
        assert_eq!(tl.audio[0].channels, Some(6), "{f} should be 5.1");
    }
}

#[test]
fn mp2_fixture_reports_stereo() {
    let tl = track_list_for("mp2-tone.ts");
    assert_eq!(tl.audio[0].channels, Some(2));
}
```

- [ ] **Step 9: Run to verify it fails**

Run: `cargo test -p skyfire-wasm --test audio_channels 2>&1 | head -20`
Expected: FAIL — no field `channels` on `WasmAudioTrack`.

- [ ] **Step 10: Add the field and populate it**

In `crates/skyfire-wasm/src/bridge_dto.rs`, extend `WasmAudioTrack` (currently lines 24-33):

```rust
    /// ISO 639-2 language (3 chars), or `None`.
    #[wasm_bindgen(getter_with_clone)]
    pub language: Option<String>,
    /// Channel count, read from the first frame header seen on this PID, or
    /// `None` when no frame has been observed yet. Never a guess — the UI
    /// must degrade rather than invent a value.
    pub channels: Option<u8>,
```

In `crates/skyfire-wasm/src/bridge.rs`, add a memo field to the bridge struct alongside `selected_audio_pid` (near line 34):

```rust
    /// Channel count per audio PID, from a header probe of the first frame.
    audio_channels: std::collections::BTreeMap<u16, u8>,
```

Initialise it in the constructor beside `selected_audio_pid: None` (near line 82):

```rust
            audio_channels: std::collections::BTreeMap::new(),
```

In `track_list()` (line 183-190), populate the new field:

```rust
            .map(|m| {
                let pid = m.pid.unwrap_or(0);
                WasmAudioTrack {
                    pid,
                    codec: match m.kind {
                        TrackKind::Audio(c) => audio_codec_str(c).to_string(),
                        _ => "EAC3".to_string(),
                    },
                    language: m.language.map(|l| lang_bytes_to_string(&l)),
                    channels: self.audio_channels.get(&pid).copied(),
                }
            })
```

Add the probe hook. Replace the audio arm at line 419 and add a second arm before `_ => {}` at line 432:

```rust
            TrackKind::Audio(codec) if meta.pid == self.selected_audio_pid => {
                self.probe_channels(meta.pid, codec, &sample.data);
                let pts_ticks = sample.source_timing.as_ref().map(|t| t.pts);
                self.decode_audio(codec, pts_ticks, &sample.data);
            }
            // Unselected audio PIDs are never decoded, but their frame headers
            // still tell us the channel layout — which the picker needs in
            // order to label them.
            TrackKind::Audio(codec) => {
                self.probe_channels(meta.pid, codec, &sample.data);
            }
```

Add the method next to `decode_audio` (before line 436):

```rust
    /// Records the channel count for `pid` from a frame header, once.
    fn probe_channels(&mut self, pid: Option<u16>, codec: AudioCodec, data: &[u8]) {
        let Some(pid) = pid else { return };
        if self.audio_channels.contains_key(&pid) {
            return;
        }
        let ch = match codec {
            AudioCodec::Mp2 => skyfire_ts::mp2_header::channels_from_header(data),
            _ => skyfire_ac3::header::channels_from_syncframe(data),
        };
        if let Some(ch) = ch {
            self.audio_channels.insert(pid, ch);
        }
    }
```

- [ ] **Step 11: Run to verify it passes**

Run: `cargo test -p skyfire-wasm --test audio_channels`
Expected: PASS — 4 tests.

If a fixture's first PES payload is not frame-aligned the probe returns `None`; in that case scan forward for the syncword within the payload rather than loosening the assertion.

- [ ] **Step 12: Declare it in the JS typings**

In `packages/core/index.d.ts`, extend `WasmAudioTrack` (lines 9-13):

```ts
export interface WasmAudioTrack {
  pid: number;
  codec: "AC3" | "EAC3" | "MP2";
  language?: string;
  /** Channel count from a frame-header probe; absent until a frame is seen. */
  channels?: number;
}
```

- [ ] **Step 13: Run the full gate and commit**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo nextest run --workspace
npx -y -p typescript@5 tsc --noEmit packages/core/index.d.ts
git add crates/ packages/core/index.d.ts
git commit -m "feat(bridge): per-PID channel count from frame-header probe"
```

Expected: clippy zero warnings; nextest all green (127 existing + 12 new: 5 in skyfire-ac3, 3 in skyfire-ts, 4 in skyfire-wasm).

---

### Task 3: Track-existence events

**Prerequisite:** #89 should be merged first.

**Files:**
- Create: `packages/player/tracks.js`
- Create: `packages/player/tracks.test.js`
- Modify: `packages/player/skyfire-player.js:1009-1017` (change detection), `:146-152` (emit signature)
- Modify: `packages/player/skyfire-element.js:148` (dispatch the diff)
- Modify: `packages/player/index.d.ts` (diff types + `tracks` overload)
- Modify: `packages/player/package.json` (`files`)

**Interfaces:**
- Consumes: `WasmAudioTrack.channels` from Task 2.
- Produces:
  - `trackSignature(tl): string`
  - `diffTracks(prev, next): {added, removed, changed}`
  - `pickFallbackAudio(audio, lostPid): number | null`
  - `SkyfireTrackDiff` type; `on("tracks", (tl, diff) => …)` second argument

- [ ] **Step 1: Write the failing test**

Create `packages/player/tracks.test.js`:

```js
import { test, expect } from "bun:test";
import { trackSignature, diffTracks, pickFallbackAudio } from "./tracks.js";

const tl = (audio, subtitles = []) => ({ video_pid: 100, video_codec: "H264", audio, subtitles });
const a = (pid, codec = "EAC3", language = "fra", channels = 2) => ({ pid, codec, language, channels });

// ── The bug this unit exists to fix: the old signature was
// `${audio.length}/${subtitles.length}`, so a same-count swap was invisible.
test("signature changes when a PID is swapped at the same count", () => {
  expect(trackSignature(tl([a(257)]))).not.toBe(trackSignature(tl([a(258)])));
});

test("signature changes when a language is corrected at the same count", () => {
  expect(trackSignature(tl([a(257, "EAC3", "fre")])))
    .not.toBe(trackSignature(tl([a(257, "EAC3", "fra")])));
});

test("signature changes when the channel count changes", () => {
  expect(trackSignature(tl([a(257, "EAC3", "fra", 2)])))
    .not.toBe(trackSignature(tl([a(257, "EAC3", "fra", 6)])));
});

test("signature is stable for an identical track set", () => {
  expect(trackSignature(tl([a(257), a(258)]))).toBe(trackSignature(tl([a(257), a(258)])));
});

test("signature is order-independent", () => {
  expect(trackSignature(tl([a(257), a(258)]))).toBe(trackSignature(tl([a(258), a(257)])));
});

test("diff reports an added track", () => {
  const d = diffTracks(tl([a(257)]), tl([a(257), a(258)]));
  expect(d.added.map((t) => t.pid)).toEqual([258]);
  expect(d.removed).toEqual([]);
  expect(d.changed).toEqual([]);
});

test("diff reports a removed track", () => {
  const d = diffTracks(tl([a(257), a(258)]), tl([a(257)]));
  expect(d.removed.map((t) => t.pid)).toEqual([258]);
  expect(d.added).toEqual([]);
});

test("diff reports a changed track as changed, not add+remove", () => {
  const d = diffTracks(tl([a(257, "EAC3", "fre")]), tl([a(257, "EAC3", "fra")]));
  expect(d.changed.map((t) => t.pid)).toEqual([257]);
  expect(d.added).toEqual([]);
  expect(d.removed).toEqual([]);
});

test("diff covers subtitle tracks too", () => {
  const prev = tl([a(257)], [{ pid: 260, kind: "DvbSubtitles", language: "fra" }]);
  const next = tl([a(257)], []);
  expect(diffTracks(prev, next).removed.map((t) => t.pid)).toEqual([260]);
});

test("diff of an identical set is empty", () => {
  const d = diffTracks(tl([a(257)]), tl([a(257)]));
  expect(d.added).toEqual([]);
  expect(d.removed).toEqual([]);
  expect(d.changed).toEqual([]);
});

test("diff treats a null previous list as all-added", () => {
  const d = diffTracks(null, tl([a(257), a(258)]));
  expect(d.added.map((t) => t.pid)).toEqual([257, 258]);
});

// ── Selected-PID loss must not leave audio permanently silent.
test("fallback picks the lowest surviving audio pid", () => {
  expect(pickFallbackAudio([a(258), a(259)], 257)).toBe(258);
});

test("fallback returns null when nothing survives", () => {
  expect(pickFallbackAudio([], 257)).toBeNull();
});

test("fallback is not needed when the selection survives", () => {
  expect(pickFallbackAudio([a(257), a(258)], 257)).toBe(257);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/player && bun test tracks.test.js`
Expected: FAIL — `Cannot find module './tracks.js'`

- [ ] **Step 3: Write minimal implementation**

Create `packages/player/tracks.js`:

```js
// Track-set identity and diffing.
//
// Extracted as pure functions on purpose: no committed fixture changes its PMT
// mid-stream, so this logic cannot be exercised end-to-end (see the spec's
// recorded coverage gap). Keeping it pure means it is fully unit-testable.

const key = (t) => `${t.pid}:${t.codec ?? t.kind ?? ""}:${t.language ?? ""}:${t.channels ?? ""}`;

const all = (tl) => [...(tl?.audio ?? []), ...(tl?.subtitles ?? [])];

/**
 * Identity of a track set. Changes whenever ANY track's pid, codec/kind,
 * language or channel count changes — not merely when the count changes,
 * which is what the player used to key on and why same-count PMT swaps went
 * unnoticed.
 */
export function trackSignature(tl) {
  return all(tl).map(key).sort().join("|");
}

/**
 * What changed between two track lists. A track present in both under the
 * same PID but with different attributes is `changed`, not removed+added.
 */
export function diffTracks(prev, next) {
  const before = new Map(all(prev).map((t) => [t.pid, t]));
  const after = new Map(all(next).map((t) => [t.pid, t]));

  const added = [];
  const changed = [];
  for (const [pid, t] of after) {
    const was = before.get(pid);
    if (!was) added.push(t);
    else if (key(was) !== key(t)) changed.push(t);
  }
  const removed = [...before.entries()]
    .filter(([pid]) => !after.has(pid))
    .map(([, t]) => t);

  return { added, removed, changed };
}

/**
 * Audio PID to use given the current set and the previously selected PID.
 * Returns `lostPid` unchanged when it survives, the lowest surviving PID when
 * it does not, and `null` when no audio remains.
 */
export function pickFallbackAudio(audio, lostPid) {
  const pids = (audio ?? []).map((t) => t.pid).sort((x, y) => x - y);
  if (pids.includes(lostPid)) return lostPid;
  return pids.length ? pids[0] : null;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd packages/player && bun test tracks.test.js`
Expected: PASS — 14 tests.

- [ ] **Step 5: Commit**

```bash
git add packages/player/tracks.js packages/player/tracks.test.js
git commit -m "feat(player): pure track-set identity, diffing and audio fallback"
```

- [ ] **Step 6: Wire into the player**

Add to the imports at the top of `packages/player/skyfire-player.js`:

```js
import { trackSignature, diffTracks, pickFallbackAudio } from "./tracks.js";
```

Change `_emit` (line 150-152) to forward a second argument:

```js
  _emit(event, data, extra) {
    (this._listeners[event] || []).forEach((cb) => cb(data, extra));
  }
```

Replace the change-detection block at lines 1009-1017:

```js
        const tl = this.bridge.track_list();
        if (tl) {
          const sig = trackSignature(tl);
          if (sig !== this._trackSig) {
            const prev = this._trackList;
            this._trackSig = sig;
            this._trackList = tl;
            this._stats.tracks = { audio: tl.audio ?? [], subtitle: tl.subtitles ?? [] };

            const diff = diffTracks(prev, tl);

            // A PMT reshuffle must never leave audio permanently silent: if the
            // selected PID is gone, fall back to the lowest surviving track.
            const sel = this._stats.selectedAudio;
            if (sel != null && !(tl.audio ?? []).some((t) => t.pid === sel)) {
              const next = pickFallbackAudio(tl.audio, sel);
              if (next != null) {
                this.bridge.select_audio(next);
                this._stats.selectedAudio = next;
                diff.reselected = { from: sel, to: next };
                this._status(`audio pid ${sel} vanished → pid ${next}`);
              }
            }

            this._emit("tracks", tl, diff);
```

The remaining lines of the original block (`if (!trackLogged) { … }`) stay as they are.

- [ ] **Step 7: Dispatch the diff from the element**

In `packages/player/skyfire-element.js`, replace line 148:

```js
    engine.on("tracks", (tl, diff) => {
      if (seq !== this._switchSeq) return;
      this._applyTracks(tl);
      if (diff) {
        this.dispatchEvent(new CustomEvent("sf-tracks-changed", {
          detail: diff, bubbles: true, composed: true,
        }));
      }
    });
```

`_applyTracks` already dispatches `sf-tracks` and rebuilds the menus, so its contract is unchanged.

- [ ] **Step 8: Declare the types**

In `packages/player/index.d.ts`, add before the `SkyfirePlayer` class:

```ts
export interface SkyfireTrackDiff {
  added: Array<WasmAudioTrack | WasmSubtitleTrack>;
  removed: Array<WasmAudioTrack | WasmSubtitleTrack>;
  changed: Array<WasmAudioTrack | WasmSubtitleTrack>;
  /** Present when the selected audio PID vanished and a fallback was chosen. */
  reselected?: { from: number; to: number };
}

export function trackSignature(tl: TrackList | null): string;
export function diffTracks(prev: TrackList | null, next: TrackList | null): SkyfireTrackDiff;
export function pickFallbackAudio(audio: WasmAudioTrack[], lostPid: number): number | null;
```

Extend the import at the top of that file to bring in `WasmAudioTrack` and `WasmSubtitleTrack` from `@firemedia/skyfire-core`, and change the `tracks` overload:

```ts
  on(event: "tracks", cb: (tracks: TrackList, diff: SkyfireTrackDiff) => void): void;
```

Add a case to `packages/player/types.test-d.ts` proving the diff is typed:

```ts
player.on("tracks", (t, d) => t.audio.length + d.added.length + (d.reselected?.to ?? 0));
```

Add `"tracks.js"` to `files` in `package.json`.

- [ ] **Step 9: Run the gate and commit**

```bash
cd packages/player && bun test
npx -y -p typescript@5 tsc --noEmit -p packages/player
cd ../.. && cargo nextest run --workspace
```

Expected: all bun tests pass; `tsc` exits 0; nextest green.

```bash
git add packages/player .github/workflows/ci.yml
git commit -m "feat(player): emit track diffs and recover from selected-PID loss"
```

- [ ] **Step 10: Verify no event storm in the browser**

```bash
wasm-pack build crates/skyfire-wasm --target web --release --out-dir "$PWD/web/pkg"
cargo build -p skyfire-server -p skyfire-cli
cd web && bun run test:streams
```

Expected: no worse than the pre-change baseline of 6 passed / 7 failed (#89 and #90 are the known failures). A `tracks` event firing repeatedly for an unchanged set would show up as menu-rebuild churn and track-count assertion failures — if track-count assertions regress, the signature is unstable, most likely because `channels` arrives late and flips the signature once per PID. That is expected exactly once per PID and must settle.

---

### Task 4: Fullscreen API

**Files:**
- Modify: `packages/player/skyfire-element.js:19-30` (CSS), `:256`/`:260` (button), `:312-316` (implementation)
- Modify: `packages/player/index.d.ts` (element interface)
- Test: `web/tests/element.spec.mjs` (append)

**Interfaces:**
- Consumes: nothing.
- Produces: `enterFullscreen()`, `exitFullscreen()`, `toggleFullscreen()`, `isFullscreen` getter, `sf-fullscreenchange` event with `{fullscreen: boolean, mode: "native" | "pseudo"}`.

- [ ] **Step 1: Write the failing test**

Append to `web/tests/element.spec.mjs` (read its existing harness first and reuse the element-mounting helper):

```js
test("fullscreen: exposes a programmatic API and reports state changes", async ({ page }) => {
  await page.goto(`${WEB}/index.html`);
  const api = await page.evaluate(() => {
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    return {
      enter: typeof el.enterFullscreen,
      exit: typeof el.exitFullscreen,
      toggle: typeof el.toggleFullscreen,
      state: typeof el.isFullscreen,
    };
  });
  expect(api).toEqual({ enter: "function", exit: "function", toggle: "function", state: "boolean" });
});

test("fullscreen: rejection surfaces instead of being swallowed", async ({ page }) => {
  await page.goto(`${WEB}/index.html`);
  // Headless Chromium refuses fullscreen without a user gesture. The contract
  // is that the caller learns about it — either a resolve or a reject, never a
  // silent no-op returning undefined.
  const outcome = await page.evaluate(async () => {
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    const p = el.enterFullscreen();
    if (typeof p?.then !== "function") return "not-a-promise";
    try { await p; return "resolved"; } catch { return "rejected"; }
  });
  expect(["resolved", "rejected"]).toContain(outcome);
});

test("fullscreen: falls back to pseudo-fullscreen when the API is absent", async ({ page }) => {
  await page.goto(`${WEB}/index.html`);
  // iPhone Safari has no Element.requestFullscreen and skyfire paints to a
  // canvas, so there is no video element to promote. Simulate that.
  const res = await page.evaluate(async () => {
    const el = document.createElement("skyfire-player");
    document.body.appendChild(el);
    el.requestFullscreen = undefined;
    const seen = [];
    el.addEventListener("sf-fullscreenchange", (e) => seen.push(e.detail));
    await el.enterFullscreen();
    const cls = el.classList.contains("sf-pseudo-fullscreen");
    const state = el.isFullscreen;
    await el.exitFullscreen();
    return { seen, cls, state, after: el.isFullscreen };
  });
  expect(res.cls).toBe(true);
  expect(res.state).toBe(true);
  expect(res.after).toBe(false);
  expect(res.seen[0]).toEqual({ fullscreen: true, mode: "pseudo" });
  expect(res.seen[1]).toEqual({ fullscreen: false, mode: "pseudo" });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd web && bunx playwright test tests/element.spec.mjs --config playwright.config.mjs -g fullscreen`
Expected: FAIL — `enterFullscreen` is `"undefined"`, not `"function"`.

- [ ] **Step 3: Write the implementation**

In `packages/player/skyfire-element.js`, add to the `<style>` block (after line 25):

```css
:host(:fullscreen) { width: 100vw; height: 100vh; background: #000; }
:host(.sf-pseudo-fullscreen) {
  position: fixed; inset: 0; width: 100vw; height: 100vh;
  z-index: 2147483647; background: #000;
}
```

Replace `_toggleFullscreen()` (lines 313-316) with:

```js
  get isFullscreen() {
    return this.ownerDocument.fullscreenElement === this ||
      this.classList.contains("sf-pseudo-fullscreen");
  }

  /**
   * Enter fullscreen. Resolves once the transition is requested; rejects with
   * the underlying reason if the browser refuses — the caller is told, rather
   * than the failure being discarded.
   *
   * Where Element.requestFullscreen does not exist (iPhone Safari, which only
   * promotes <video> elements, and skyfire paints to a <canvas>) this falls
   * back to a fixed-position overlay and reports mode "pseudo".
   */
  async enterFullscreen() {
    if (this.isFullscreen) return;
    if (typeof this.requestFullscreen === "function") {
      await this.requestFullscreen();
      return; // fullscreenchange fires the event
    }
    this.classList.add("sf-pseudo-fullscreen");
    this._pseudoFs = true;
    this._prevOverflow = this.ownerDocument.body.style.overflow;
    this.ownerDocument.body.style.overflow = "hidden";
    this._emitFullscreen(true, "pseudo");
  }

  async exitFullscreen() {
    if (this._pseudoFs) {
      this.classList.remove("sf-pseudo-fullscreen");
      this._pseudoFs = false;
      this.ownerDocument.body.style.overflow = this._prevOverflow ?? "";
      this._emitFullscreen(false, "pseudo");
      return;
    }
    if (this.ownerDocument.fullscreenElement === this) {
      await this.ownerDocument.exitFullscreen();
    }
  }

  toggleFullscreen() {
    return this.isFullscreen ? this.exitFullscreen() : this.enterFullscreen();
  }

  _emitFullscreen(fullscreen, mode) {
    const fsBtn = this._controlsEl?.querySelector(".fs-btn");
    if (fsBtn) fsBtn.setAttribute("aria-pressed", fullscreen ? "true" : "false");
    this.dispatchEvent(new CustomEvent("sf-fullscreenchange", {
      detail: { fullscreen, mode }, bubbles: true, composed: true,
    }));
  }
```

Register a native listener so state changed by any route — including the Escape key — is reflected. Add to `connectedCallback` (find it near the top of the class):

```js
    this._onFsChange = () => this._emitFullscreen(this.isFullscreen, "native");
    this.ownerDocument.addEventListener("fullscreenchange", this._onFsChange);
```

And in `disconnectedCallback`:

```js
    this.ownerDocument.removeEventListener("fullscreenchange", this._onFsChange);
```

Update both button wirings (lines 256 and 260) from `() => this._toggleFullscreen()` to:

```js
      btn("fs-btn", "⛶", () => this.toggleFullscreen());
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd web && bunx playwright test tests/element.spec.mjs --config playwright.config.mjs -g fullscreen`
Expected: PASS — 3 tests.

- [ ] **Step 5: Declare the element typings**

In `packages/player/index.d.ts`, append:

```ts
export interface SkyfireFullscreenChangeDetail {
  fullscreen: boolean;
  mode: "native" | "pseudo";
}

/** The `<skyfire-player>` custom element. */
export interface SkyfirePlayerElement extends HTMLElement {
  readonly isFullscreen: boolean;
  enterFullscreen(): Promise<void>;
  exitFullscreen(): Promise<void>;
  toggleFullscreen(): Promise<void>;
}

declare global {
  interface HTMLElementTagNameMap {
    "skyfire-player": SkyfirePlayerElement;
  }
  interface HTMLElementEventMap {
    "sf-fullscreenchange": CustomEvent<SkyfireFullscreenChangeDetail>;
  }
}
```

- [ ] **Step 6: Run the full gate and commit**

```bash
cd packages/player && npx -y -p typescript@5 tsc --noEmit -p .
cd ../.. && cargo nextest run --workspace
cd web && bun run test:streams
```

Expected: `tsc` 0; nextest green; stream gate no worse than 6/7.

```bash
git add packages/player web/tests/element.spec.mjs
git commit -m "feat(player): fullscreen API with state event and iOS fallback"
```

- [ ] **Step 7: Document the iOS caveat**

Add to `packages/player/README.md`, under the controls section:

```markdown
### Fullscreen

`enterFullscreen()` / `exitFullscreen()` / `toggleFullscreen()` return promises
and reject if the browser refuses (most browsers require a user gesture). The
`sf-fullscreenchange` event carries `{ fullscreen, mode }`.

On iPhone Safari there is no `Element.requestFullscreen` — WebKit only promotes
`<video>` elements, and skyfire renders to a `<canvas>` — so the player falls
back to a fixed-position overlay and reports `mode: "pseudo"`. It fills the
viewport but does not hide Safari's own chrome.
```

```bash
git add packages/player/README.md
git commit -m "docs(player): document the fullscreen API and iOS pseudo-fullscreen"
```

---

## Self-review

**Spec coverage.** Unit A → Task 1. Unit B → Task 2. Unit C → Task 3. Unit D → Task 4, including the iOS fallback the spec put in scope and the `:host(:fullscreen)` rule it noted was missing. The spec's accepted degradation (absent `channels`) is implemented in Task 1 Step 6 (`chan` empty) and Task 2 Step 10 (`Option<u8>`), and asserted nowhere as a guess. The spec's two recorded coverage gaps are carried forward: Task 3 Step 1 states why the logic is pure-function tested, and Task 4 Step 1 states why headless fullscreen is asserted on the contract.

**Deviation from the spec, deliberate.** The spec sketched `channels_ac3()` and `channels_eac3()` as separate entry points. `bsid` sits at bit offset 40 in both syntaxes by design, so Task 2 exposes a single `channels_from_syncframe()` that dispatches internally. Fewer public functions, one place for the dispatch rule.

**Placeholders.** None. Every code step carries the code; every command carries its expected output. Two steps deliberately say "read the existing harness first and reuse it" (Task 2 Step 8, Task 4 Step 1) rather than duplicating a harness this plan cannot see in full — that is a reuse instruction, not a gap.

**Type consistency.** `channels` is `Option<u8>` in Rust, `channels?: number` in both `.d.ts` files, and read as `a.channels` in the element. `trackSignature`/`diffTracks`/`pickFallbackAudio` keep the same names in `tracks.js`, its test, `index.d.ts`, and the player call sites. The diff shape `{added, removed, changed, reselected?}` is identical in `tracks.js`, the test, `SkyfireTrackDiff`, and the `sf-tracks-changed` detail. `sf-fullscreenchange`'s detail matches `SkyfireFullscreenChangeDetail`.

**Ordering.** Tasks 1, 2 and 4 are independent. Task 3 consumes Task 2's `channels` in its signature and should follow #89.
