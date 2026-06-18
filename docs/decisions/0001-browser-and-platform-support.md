# 0001 — Browser & platform support, codec-gating policy

- **Status:** Accepted
- **Date:** 2026-06-18

## Context

Skyfire decodes raw MPEG-TS client-side: WebCodecs `VideoDecoder` for HW video,
a WASM AC-3/E-AC-3 decoder for audio. Capability varies sharply by engine, so the
support target must be fixed before the video pipeline (epic #3), shell (#5),
render (#6) and capability fallback (#7) are built.

Relevant forces:

- **WebCodecs `VideoDecoder`**: Chromium full; WebKit (Safari) from 17; Gecko
  (Firefox) shipped recently.
- **H.265 / HEVC decode**: Safari strong on Apple silicon; Chromium
  platform/hardware-dependent; **Firefox has none**.
- **AudioWorklet + WASM**: universal across modern engines.
- **iOS**: Apple requires every iOS browser (Chrome, Firefox, Edge) to use
  WebKit/WKWebView. "Chrome on iOS" is **not** Chromium — it has Safari's
  capabilities, not Chrome's.

## Decision

Support **Chrome/Edge, Safari, and Firefox**, on **desktop and mobile (iOS +
Android)**, explicitly including Chrome-on-iOS.

1. **Treat all iOS browsers as one WebKit target.** No separate iOS-Chrome path;
   it inherits Safari's WebCodecs/H.265 behaviour.
2. **Minimum iOS 17** (the WebCodecs floor). Pre-17 iOS is unsupported.
3. **H.265 policy = gate + H.264 fallback.** At stream start, probe
   `VideoDecoder.isConfigSupported(hevcConfig)`; use H.265 when supported,
   otherwise fall back to the H.264 channel.
4. **No UA sniffing.** All codec selection is driven by the `isConfigSupported`
   capability probe, never by parsing the user-agent string.

## Consequences

- The engine must always have a working H.264 path; H.265 is opportunistic.
  Firefox and weak hardware always land on H.264.
- A startup capability-probe step is mandatory (epic #7). Channels must be
  available in, or transcodable-free to, H.264 for universal reach.
- The `web/` shell (epics #5, #6) must handle touch controls and tighter
  mobile decode/thermal budgets, not just desktop.
- The iOS-17 floor and WebKit-unification let us skip a second iOS code path.

## Support matrix

| Target | Engine | WebCodecs | H.265 | Notes |
|---|---|---|---|---|
| Chrome/Edge desktop | Chromium | full | HW-dependent → gate | primary |
| Safari desktop | WebKit 17+ | yes | strong on Apple silicon | |
| Firefox desktop | Gecko | recent | none → H.264 fallback | |
| iOS (Safari/Chrome/FF) | WebKit 17+ | yes | on capable HW | single target, iOS 17+ |
| Android Chrome | Chromium | full | HW-dependent → gate | thermal/battery budget |
