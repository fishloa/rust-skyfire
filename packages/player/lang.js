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
  // `.of()` throws RangeError for any structurally invalid language tag.
  // `raw` comes straight from a PMT ISO_639_language_descriptor (3 raw bytes,
  // String::from_utf8_lossy) — an unset/padded/non-UTF-8 descriptor yields
  // strings like "1ta", "e g", "e-g", "en_" or "   " that throw here. Treat a
  // throw the same as "unresolved" so it falls through to the uppercase
  // passthrough below instead of killing the caller (and, upstream, the
  // stream).
  let name;
  try {
    name = displayNames(locale)?.of(tag);
  } catch {
    name = undefined;
  }
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
