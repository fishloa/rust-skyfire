# Design: readable track labels, track-existence events, fullscreen API

Date: 2026-07-26
Status: approved (brainstorming session), pending implementation plan

## Problem

Three gaps in the browser-facing player, all surfaced while working the backlog:

1. **Language codes are shown raw.** `skyfire-element.js:291` renders
   ``​`${a.language || `Track ${i + 1}`} · ${a.codec}`​`` and line 299 the
   subtitle equivalent, so the picker reads `fra · EAC3`, `qaa · EAC3`,
   `oth · MP2`. Broadcast PMTs carry ISO 639-2 three-letter codes
   (ETSI EN 300 468 §6.2.4, `ISO_639_language_descriptor`); users need names.

2. **Track changes are only noticed when the count changes.** The refresh
   signature is `sig = ${tl.audio.length}/${tl.subtitles.length}`
   (`skyfire-player.js:1011`). A PMT update that swaps a PID, corrects a
   language, or replaces one track with another while the count holds is
   invisible to the UI. Nothing notices when the *currently selected* audio PID
   disappears, so audio can go permanently silent after a PMT reshuffle.

3. **Fullscreen has no programmatic API.** A `⛶` button exists
   (`skyfire-element.js:256`, `:260`) wired to `_toggleFullscreen()` (`:313`),
   but there is no method a host can call, no `fullscreenchange` listener (so
   the icon never reflects real state), and `this.requestFullscreen?.().catch(() => {})`
   discards every rejection.

## Scope

Four units, delivered as four issues under one epic (ADR 0002). Units A and D
are pure JS and independent. Unit B is a Rust/WASM change that gates unit C's
final label format.

Out of scope, deliberately: server-side channel-existence events, richer
`/api/streams` metadata, and any change to `SkyfirePlayer`'s volume/mute
typings gap.

---

## Unit A — language display names

New published module `packages/player/lang.js`, exported so hosts building
their own picker (the `SkyfirePlayer`-direct path, not just
`<skyfire-player>`) get the same behaviour rather than reimplementing it.

```js
languageName(code, locale?)        // "fre" -> "French"
resolveLocale(element)             // element lang -> ancestor -> document -> navigator
```

### Resolution pipeline

1. Normalise: trim, lowercase. Nullish/empty → `null` (caller falls back to
   `Track ${i+1}`, preserving today's behaviour).
2. **`qaa`–`qtz`** (ISO 639-2 reserved-for-local-use range, which DVB
   broadcasters use for original/undefined audio) → `"Original audio"`.
3. **ISO 639-2/B → 639-1 fixup.** `Intl.DisplayNames` resolves terminological
   codes (`fra`) reliably; bibliographic ones (`fre`) it does not, and real
   PMTs carry both — `fixtures/streams.json` has `france-2` emitting *both*
   `fre` (pid 257) and `fra` (pid 258) for French. Mapping to 639-1 before
   calling `Intl` sidesteps the question entirely. The 20 divergent codes:

   ```
   alb→sq  arm→hy  baq→eu  bur→my  chi→zh
   cze→cs  dut→nl  fre→fr  geo→ka  ger→de
   gre→el  ice→is  mac→mk  mao→mi  may→ms
   per→fa  rum→ro  slo→sk  tib→bo  wel→cy
   ```

4. `Intl.DisplayNames([locale], { type: "language", fallback: "none" }).of(tag)`.
   CLDR already localises `und`, `mul`, `zxx`, `mis`, so they need no table.
5. Unresolved (e.g. `oth`, which is not a valid ISO 639-2 code but appears on
   `rai-1` pid 258) → uppercase passthrough, `"OTH"`.
6. `Intl.DisplayNames` absent → uppercase passthrough.

`Intl.DisplayNames` instances are cached per locale; construction is not free
and the picker rebuilds on every `tracks` event.

### Known limitation, accepted

`"Original audio"` is our own string, not CLDR's, so it is not localised. The
function takes an optional `overrides` map so a host can supply its own
wording. Documented rather than hidden.

### Locale source

`resolveLocale(el)`: element's own `lang` attribute → nearest `[lang]`
ancestor (`el.closest("[lang]")`) → `document.documentElement.lang` →
`navigator.language`. Standard DOM inheritance, overridable per element.

### Exit criteria

- `bun test` covers: B-code fixup (`fre`→French, `ger`→German), T-code
  passthrough (`fra`→French), locale switching (`fra`,`de` → `Französisch`),
  `qaa`→Original audio, junk passthrough (`oth`→`OTH`), nullish → `null`,
  and `Intl.DisplayNames`-missing fallback.
- Picker renders `French · EAC3` for `france-2` in the browser gate.
- Module is in the published tarball; `npm pack --dry-run` confirms.

---

## Unit B — per-track channel count

Required by the chosen label format (`French · EAC3 5.1`).

`WasmAudioTrack` is `{pid, codec, language}` (`bridge_dto.rs:24`). Channel
count today comes out of a *decode*, and the bridge decodes only the
**selected** PID — so unselected tracks have no channel information at all.

Fix: a **header-only probe** of the first audio frame seen per audio PID. No
decode, no decoder state, no per-PID decoder instances.

| codec | fields | spec |
|---|---|---|
| AC-3 | `acmod` (3b) + `lfeon` (1b), after `bsid`/`bsmod` and the acmod-conditional `cmixlev`/`surmixlev`/`dsurmod` | ETSI TS 102 366 §5.4.2, Table 5.8 |
| E-AC-3 | `acmod` (3b) + `lfeon` (1b), adjacent, after `strmtyp`/`substreamid`/`frmsiz`/`fscod`/`numblkscod` | ETSI TS 102 366 §E.1.2.2 |
| MP2 | `mode` (2b): `11` single-channel → 1, else 2 | ISO/IEC 11172-3 §2.4.1.3 |

`acmod` → channel count: `0→2 (1+1), 1→1, 2→2, 3→3, 4→3, 5→4, 6→4, 7→5`,
plus 1 when `lfeon` is set. So 5.1 is `acmod=7, lfeon=1` → 6.

### Placement

- `crates/skyfire-ac3/src/header.rs` — `channels_ac3()`, `channels_eac3()`.
  That crate already owns AC-3 framing (`is_ac3_syncframe`).
- `crates/skyfire-ts/src/mp2_header.rs` — `channels_mp2()`. There is no MP2
  crate in the workspace (the bridge uses `self.mpa_decoder`,
  `bridge.rs:438`), and skyfire-ts already identifies elementary streams.
- Bridge dispatches on the track's codec at first frame, memoises per PID.

Both are pure bit-reading with no decoder dependency, so both are unit
testable without audio output.

### Surfaced as

`WasmAudioTrack.channels: Option<u8>` → `TrackList` → `stats.tracks` →
`index.d.ts`.

### Accepted degradation

A PID that carries no frame within the observed window keeps
`channels === undefined`. The label then degrades to `French · EAC3`, and unit
C's numbering disambiguates any remaining collision. The UI must never present
a guessed channel count.

### Exit criteria

- `cargo nextest` asserts channel counts per PID against committed fixtures:
  `orf1` (AC3 `deu` + MP2 `mis`), `arte` (4× EAC3), `rai-1` (MP2 + AC3),
  `orf-3` (MP2 stereo — the #82 regression stream).
- No `unsafe`; clippy clean at `-D warnings`.
- Values agree with `ffprobe` on the same fixtures.

---

## Unit C — track-existence events

### Change detection

Replace the count signature with deep identity over
`pid:codec:language:channels` across audio and subtitles. Any genuine change
re-emits, including same-count PID or language swaps.

### Diff

```js
player.on("tracks", (trackList, diff) => { … });
// diff = { added: [...], removed: [...], changed: [...], reselected?: {from, to} }
```

`changed` = same PID, different codec/language/channels. The second argument
extends the overloads added in PR #88; existing single-argument handlers keep
working unchanged.

`<skyfire-player>` continues to dispatch `sf-tracks` (full list, unchanged
contract) and adds `sf-tracks-changed` carrying the diff, then rebuilds its
menus.

### Selected-PID loss

If `selectedAudio` is absent from the new audio set: re-select the first
surviving audio track, call `select_audio`, update `stats.selectedAudio`, and
report it as `diff.reselected = {from, to}` plus a status line. Audio never
goes permanently silent because of a PMT reshuffle.

### Testing constraint, and the honest workaround

**No committed fixture changes its PMT mid-stream**, so there is no data for a
true end-to-end test. Therefore:

- The diff and reselect logic are extracted as **pure functions**
  (`trackSignature`, `diffTracks`, `pickFallbackAudio`) and unit tested under
  `bun test` with synthetic track lists — including the case this unit exists
  to fix: same count, different PID.
- A Rust test asserts the bridge re-emits a changed `track_list` when the PMT
  changes.
- A crafted PMT-switch fixture is a **follow-up issue**, not a blocker. Noting
  this explicitly so the coverage gap is recorded rather than implied away.

### Exit criteria

- Unit tests cover: same-count PID swap, language correction, track added,
  track removed, selected PID removed → fallback chosen, no-change → no emit.
- Browser gate still green on track-count assertions (no spurious re-emits;
  a `tracks` event storm would show as menu rebuild churn).
- Duplicate names disambiguated: `arte`'s two French tracks render distinctly.

---

## Unit D — fullscreen API

### Surface

```js
element.enterFullscreen()          // -> Promise, rejection propagated
element.exitFullscreen()           // -> Promise
element.toggleFullscreen()         // -> Promise
element.isFullscreen               // getter
// event: sf-fullscreenchange, detail { fullscreen: boolean, mode: "native" | "pseudo" }
```

Rejections are no longer swallowed — `_toggleFullscreen`'s
`.catch(() => {})` at line 315 goes away and the promise is returned.

A `fullscreenchange` listener on `ownerDocument` drives the `⛶` icon and its
`aria-pressed`, so the control reflects state changed by any route including
the Escape key.

### iOS fallback (in scope)

iPhone Safari has no `Element.requestFullscreen` — only
`HTMLVideoElement.webkitEnterFullscreen`, and skyfire paints to a `<canvas>`,
so there is no video element to promote. Without a fallback the new API is
permanently dead on a platform ADR 0001 commits to supporting.

Fallback: a `sf-pseudo-fullscreen` class on the host —
`position: fixed; inset: 0; z-index: 2147483647` — plus a document scroll
lock, reported as `mode: "pseudo"`. Chosen when
`typeof this.requestFullscreen !== "function"`.

Also adds the `:host(:fullscreen)` CSS rule that does not exist today, so
sizing is explicit rather than inherited from the UA stylesheet.

### Typings

`index.d.ts` currently types only `SkyfirePlayer`; the custom element has no
typings at all. Adds a minimal `SkyfirePlayerElement` interface covering the
new methods, the getter, and the event detail — not the element's whole
surface. Broader element typings are a separate concern.

### Exit criteria

- Playwright asserts `enterFullscreen()` resolves or rejects observably (never
  silently no-ops) and that `sf-fullscreenchange` fires with the right `mode`.
  Headless Chromium may refuse real fullscreen without a user gesture, so the
  assertion is on the event/rejection path, not on a real fullscreen box.
- Pseudo-fullscreen path is exercised by stubbing `requestFullscreen` away.
- Icon `aria-pressed` tracks state.

---

## Testing summary

| layer | what |
|---|---|
| `bun test` | `lang.js` mapping; `diffTracks`/`trackSignature`/`pickFallbackAudio` |
| `cargo nextest` | header channel probes vs fixtures; bridge re-emit on PMT change |
| Playwright | rendered labels; fullscreen API + event; no track-event storm |
| CI gate | `fmt`, `clippy -D warnings`, `build`, `nextest`, `tsc -p packages/player` |

## Risks

1. **`channels` may be absent** for quiet PIDs → labels degrade, numbering
   covers it. Never guess a count.
2. **No PMT-change fixture** → unit C's end-to-end path is unit-tested only,
   with a crafted fixture as follow-up.
3. **Headless fullscreen** cannot be fully exercised; assertions target the
   observable contract rather than the OS-level result.
4. `dvb-si` moved 8.4.0 → 8.6.0 in 1dbcb45, two minors of PSI parsing beneath
   the track list unit C touches. Browser gate showed no regression, but unit
   B/C work should re-run it.

## Pre-existing failures, not caused by this work

#89 (audio flip broken on all 6 alt-PID streams) and #90 (DVB-sub cues:
flaky on `france-2`, hard-fail on `arte`) are open and unrelated. Unit C's
reselect logic touches adjacent code to #89 — worth landing #89 first to avoid
two changes in one area.
