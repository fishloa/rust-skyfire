// bridge-guard-test.js — verifies the re-entrancy guard works.
//
// This test simulates what happens when an external caller (e.g. an audio track
// picker event handler or a setInterval poll) calls a bridge method while the
// main _consumeStream loop already holds the bridge lock.
//
// The guard MUST NOT crash (no "recursive use" panic).  Instead it either:
//   a) queues the call for deferred execution (production wrapper), OR
//   b) returns undefined / logs a warning if queuing isn't practical.

import { SkyfirePlayer } from "../releases/v0.1.4/skyfire.js";

function assert(cond, msg) {
  if (!cond) throw new Error(`FAIL: ${msg}`);
  console.log(`  ✓ ${msg}`);
}

// Create a player instance.  We won't init() with a real stream — we just need
// the guard wiring on the instance.  The constructor sets _bridgeLocked = false
// and _pendingBridgeQueue = [].
const player = new SkyfirePlayer();

// Simulate having a bridge object (mock).
let mockCalls = [];
player._bridge = new Proxy({}, {
  get(_, prop) {
    return (...args) => { mockCalls.push({ prop, args }); };
  }
});

// ── Test 1: Normal single call ────────────────────────────────────────────

mockCalls = [];
player._callBridge("feed", new Uint8Array([0x47, 0x01, 0x00]));
assert(mockCalls.length === 1, "single call executes");
assert(mockCalls[0].prop === "feed", "correct method");

// ── Test 2: Re-entrant call — should queue, not crash ─────────────────────

mockCalls = [];
let outerComplete = false;
const result1 = player._callBridge(() => {
  // Inside first call — bridge is locked.
  const innerResult = player._callBridge("track_list");
  assert(innerResult === undefined, "re-entrant call returns undefined");
  assert(mockCalls.length === 0, "re-entrant call didn't execute yet");
  assert(player._pendingBridgeQueue.length === 1, "re-entrant call was queued");
  assert(player.stats._bridgeReentries === 1, "re-entrant count incremented");
  outerComplete = true;
  return "outer-result";
});

assert(result1 === "outer-result", "outer call returns its value");
assert(outerComplete === true, "outer call completed");
assert(mockCalls.length === 1, "queued call executed after outer returned");
assert(mockCalls[0].prop === "track_list", "queued call was correct method");
assert(player._pendingBridgeQueue.length === 0, "queue drained");

// ── Test 3: Re-entrant with multiple queued calls ─────────────────────────

mockCalls = [];
player._callBridge(() => {
  player._callBridge("method_a");
  player._callBridge("method_b");
  player._callBridge(() => { player._callBridge("method_c"); });
});

assert(mockCalls.length === 3, "all queued calls executed");
assert(mockCalls.map(c => c.prop).join() === "method_a,method_b,method_c", "correct order");

// ── Test 4: Structural guarantee ──────────────────────────────────────────
//
// Verify that NO code path calls the bridge directly from outside the
// _consumeStream loop.  selectAudio/selectSubtitle store flags.
// trackList getter returns cached data.

mockCalls = [];
player._trackList = {
  video_pid: 0x101,
  video_codec: "avc1.640028",
  audio: [{ pid: 0x201, codec: "ac-3", language: "eng" }],
  subtitles: [{ pid: 0x301, kind: "dvb", language: "eng" }],
};

// selectAudio with valid idx sets _pendingAudioPid, does NOT call bridge.
player.selectAudio(0);
assert(mockCalls.length === 0, "selectAudio does NOT call bridge directly");
assert(player._pendingAudioPid === 0x201, "selectAudio stores flag");

// selectSubtitle(-1) sets _pendingSubtitlePid = -1 (disable), no bridge call.
player.selectSubtitle(-1);
assert(mockCalls.length === 0, "selectSubtitle(-1) does NOT call bridge directly");
assert(player._pendingSubtitlePid === -1, "selectSubtitle stores disable flag");

// trackList getter returns cached data, no bridge call.
const tl = player.trackList;
assert(mockCalls.length === 0, "trackList getter does NOT call bridge");
assert(tl.audio[0].pid === 0x201, "trackList returns correct cached data");
assert(tl.subtitles[0].language === "eng", "trackList returns subs");

// cleanup
player._bridge = null;

console.log("\n🎉 All bridge guard tests passed.");
