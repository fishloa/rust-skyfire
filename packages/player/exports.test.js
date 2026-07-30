// Consumer smoke test: index.d.ts declares languageName, resolveLocale,
// trackSignature, diffTracks and pickFallbackAudio as package exports, but the
// entry point (main/module = skyfire-player.js) used to export only
// `SkyfirePlayer` — it merely imported from ./lang.js and ./tracks.js without
// re-exporting them. A TS consumer following the typings then got a hard ESM
// link error ("Export named 'languageName' not found in module") that breaks
// their bundle, with no `tsc` run in this repo's CI to catch it. This test
// imports the entry point exactly as a consumer would and asserts each named
// export actually exists there, so the defect is caught without needing tsc.
import { test, expect } from "bun:test";
import {
  SkyfirePlayer,
  languageName,
  resolveLocale,
  trackSignature,
  diffTracks,
  pickFallbackAudio,
} from "./skyfire-player.js";

test("skyfire-player.js re-exports every named export index.d.ts declares", () => {
  expect(typeof SkyfirePlayer).toBe("function");
  expect(typeof languageName).toBe("function");
  expect(typeof resolveLocale).toBe("function");
  expect(typeof trackSignature).toBe("function");
  expect(typeof diffTracks).toBe("function");
  expect(typeof pickFallbackAudio).toBe("function");
});
