#!/usr/bin/env bash
# Build fixtures/streams.json from the committed clips using the skyfire probe.
# Ground-truth expectations for the browser harness come from real probe output,
# never hand-guessed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLIP_DIR="$ROOT/fixtures/streams"
SKYFIRE="${SKYFIRE:-$ROOT/target/debug/skyfire}"
CLIP_SECS="${CLIP_SECS:-20}"
[ -x "$SKYFIRE" ] || { echo "build skyfire first" >&2; exit 1; }

entries=()
for clip in "$CLIP_DIR"/*.ts; do
  [ -e "$clip" ] || continue
  slug="$(basename "$clip" .ts)"
  probe="$("$SKYFIRE" "$clip" --probe)"
  act="$("$SKYFIRE" "$clip" --sub-activity)"
  # grep -c prints 0 + exits 1 on no match; `|| true` keeps pipefail from aborting.
  sub_count="$(printf '%s' "$act" | grep -c '"pts_ticks"' || true)"
  entries+=("$(SLUG="$slug" CLIP_SECS="$CLIP_SECS" SUB="$sub_count" \
    python3 - "$probe" <<'PY'
import json,os,sys
p=json.loads(sys.argv[1])
audio=p.get("audio",[])
default=p.get("default_audio_pid")
alt=next((a["pid"] for a in audio if a["pid"]!=default), None)
fps=25; secs=int(os.environ["CLIP_SECS"])
print(json.dumps({
  "slug":os.environ["SLUG"], "file":f'streams/{os.environ["SLUG"]}.ts',
  "video":p.get("video"), "audio":audio, "default_audio_pid":default,
  "alt_audio_pid":alt, "subtitle":p.get("subtitle",[]),
  "expect_sub_cues": int(os.environ["SUB"])>0,
  "min_video_frames": int(fps*secs*0.6), "clip_secs":secs,
}))
PY
)")
done
if ((${#entries[@]})); then
  # Build joined body explicitly — avoids bash-3.2 empty-array set -u abort and
  # multi-char-IFS-first-char-only join bugs.
  {
    printf '[\n'
    for i in "${!entries[@]}"; do
      if (( i == ${#entries[@]} - 1 )); then
        printf '  %s\n' "${entries[$i]}"
      else
        printf '  %s,\n' "${entries[$i]}"
      fi
    done
    printf ']\n'
  } > "$ROOT/fixtures/streams.json"
else
  printf '[]\n' > "$ROOT/fixtures/streams.json"
fi
echo "wrote fixtures/streams.json with ${#entries[@]} streams"
