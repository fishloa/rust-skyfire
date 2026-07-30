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
