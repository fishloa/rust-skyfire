// Compile-time test for the public typings. Not published (see package.json
// `files`); CI runs it via `tsc --noEmit -p packages/player`.
//
// rust-skyfire#87: `on()` used to type every payload as `unknown`, so a
// correctly-typed handler could not be passed without a cast. Each positive
// case below fails to compile if that regresses; each `@ts-expect-error` case
// fails to compile if the overload stops discriminating.
import type { TrackList } from "@firemedia/skyfire-core";
import {
  SkyfirePlayer,
  type SkyfireEndedStats,
  type SkyfireErrorEvent,
  type SkyfireEvent,
  type SkyfireEventMap,
  type SkyfireStats,
} from "./index.js";

declare const player: SkyfirePlayer;

// ── payload types flow into the callback without a cast ─────────────────────
const onTracks = (t: TrackList) => t.audio.map((a) => a.pid);
const onStats = (s: SkyfireStats) => s.avSkewMs + s.tracks.audio.length;
const onError = (e: SkyfireErrorEvent) => e.message;
const onEnded = (s: SkyfireEndedStats) => s.done;

player.on("tracks", onTracks);
player.on("stats", onStats);
player.on("error", onError);
player.on("ended", onEnded);

// ── inline handlers infer their parameter ───────────────────────────────────
player.on("tracks", (t) => t.video_pid.toFixed(0));
player.on("stats", (s) => s.decoded - s.dropped);
player.on("error", (e) => e.message.toUpperCase());
player.on("ended", (s) => s.audioSec.toFixed(1));

// ── wrong payload for the event is rejected ─────────────────────────────────
// @ts-expect-error `stats` does not deliver a TrackList
player.on("stats", onTracks);
// @ts-expect-error `tracks` does not deliver stats
player.on("tracks", onStats);
// @ts-expect-error TrackList has no `message`
player.on("tracks", (t) => t.message);
// @ts-expect-error the error payload is not an `Error` instance
player.on("error", (e: Error) => e.stack);

// ── the map keeps generic/shared handlers typed ─────────────────────────────
function subscribe<E extends SkyfireEvent>(
  event: E,
  cb: (data: SkyfireEventMap[E]) => void,
): void {
  player.on(event, cb);
}
subscribe("tracks", onTracks);
subscribe("error", onError);
