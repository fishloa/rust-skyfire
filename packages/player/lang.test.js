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

// ── CRITICAL: Intl.DisplayNames#of() throws RangeError for any structurally
// invalid language id. The language string comes from a raw 3-byte PMT
// ISO_639_language_descriptor decoded with String::from_utf8_lossy, so an
// unset/padded/non-UTF-8 descriptor can hand languageName() exactly these
// shapes. Confirmed to throw when called unguarded (bun/ICU, 2026-07-26):
// `new Intl.DisplayNames(["en"], {type:"language", fallback:"none"}).of("1ta")`
// throws RangeError "argument is not a language id" — same for the other three.
// languageName() must swallow the throw and fall through to the uppercase
// passthrough rather than propagating (a propagated throw here kills a live
// stream: _buildMenus -> _applyTracks -> the `tracks` listener -> `_emit`,
// uncaught, out of `_consumeStream`, treated as a stream failure).
test("swallows Intl RangeError for structurally invalid language ids and does not throw", () => {
  expect(() => languageName("1ta", "en")).not.toThrow();
  expect(() => languageName("e g", "en")).not.toThrow();
  expect(() => languageName("e-g", "en")).not.toThrow();
  expect(() => languageName("en_", "en")).not.toThrow();
  expect(() => languageName("   ", "en")).not.toThrow();

  expect(languageName("1ta", "en")).toBe("1TA");
  expect(languageName("e g", "en")).toBe("E G");
  expect(languageName("e-g", "en")).toBe("E-G");
  expect(languageName("en_", "en")).toBe("EN_");
  // Whitespace-only trims to the empty string before it ever reaches Intl —
  // it hits the existing "no code at all" contract (see the null-return test
  // above) rather than the uppercase-passthrough path. Still must not throw.
  expect(languageName("   ", "en")).toBeNull();
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
