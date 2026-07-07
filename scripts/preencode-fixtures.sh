#!/usr/bin/env bash
# Offline pre-encode: raw DVB captures → clean progressive H.264 TS the harness
# can decode headless. Deinterlaces true-1080i (bwdif) when the source is
# interlaced; re-encodes video with libx264 (closed GOP, RAP-aligned); copies
# ALL audio + subtitle + SI PIDs untouched (skyfire never re-encodes audio).
#
# Requires: ffmpeg, ffprobe, and a built `skyfire` CLI (cargo build -p skyfire-cli).
#
# Env:
#   CLIP_SECS   committed-clip length in seconds (default 25)
#   ONLY        space-separated slug list to restrict processing (default: all)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$ROOT/.ts-captures"
FULL_OUT="$SRC_DIR/progressive"
CLIP_OUT="$ROOT/fixtures/streams"
CLIP_SECS="${CLIP_SECS:-25}"
SKYFIRE="${SKYFIRE:-$ROOT/target/debug/skyfire}"

# Curated committed subset — one per codec / scan-type. Edit as captures change.
SUBSET=(rai-1 france-2 arte orf1 tf-1 m6)

mkdir -p "$FULL_OUT" "$CLIP_OUT"
command -v ffmpeg  >/dev/null || { echo "ffmpeg not found"  >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "ffprobe not found" >&2; exit 1; }
[ -x "$SKYFIRE" ] || { echo "build skyfire first: cargo build -p skyfire-cli" >&2; exit 1; }

encode_full() {
  local slug="$1" src="$2" out="$3"
  local fo; fo="$(ffprobe -v error -select_streams v:0 -show_entries stream=field_order \
                  -of default=nw=1:nk=1 "$src" 2>/dev/null || echo progressive)"
  local vf=""
  case "$fo" in tt|bb|tb|bt) vf="-vf bwdif=mode=send_frame:parity=auto" ;; esac
  echo "[$slug] field_order=$fo ${vf:+(deinterlacing)}"
  # shellcheck disable=SC2086
  # -copy_unknown + -map 0: preserve EVERY PID (video re-encoded; audio, DVB-sub,
  # teletext and other private/data PIDs copied verbatim, incl. types ffmpeg has
  # no decoder for). Without it, an unknown ES PID aborts the mux (#0:x unsupported).
  ffmpeg -y -hide_banner -loglevel error -copy_unknown -i "$src" -map 0 \
    -c:v libx264 -profile:v high -pix_fmt yuv420p -preset veryfast \
    -g 50 -keyint_min 50 -sc_threshold 0 $vf \
    -b:v 2500k -maxrate 3000k -bufsize 5000k \
    -c:a copy -c:s copy -copyts \
    -f mpegts "$out"
}

want() {  # true if slug is in ONLY (or ONLY is unset)
  [ -z "${ONLY:-}" ] && return 0
  for s in $ONLY; do [ "$s" = "$1" ] && return 0; done
  return 1
}

for src in "$SRC_DIR"/*.ts; do
  [ -e "$src" ] || continue
  slug="$(basename "$src" .ts)"
  want "$slug" || continue
  encode_full "$slug" "$src" "$FULL_OUT/$slug.ts"
done

# Committed clips from the SUBSET, cut around subtitle activity when present.
for slug in "${SUBSET[@]}"; do
  want "$slug" || continue
  full="$FULL_OUT/$slug.ts"
  [ -e "$full" ] || { echo "[$slug] no progressive source; skipping clip" >&2; continue; }
  # Find a subtitle-activity PTS (90kHz) if any; convert to seconds for -ss.
  ss=0
  act="$("$SKYFIRE" "$full" --sub-activity 2>/dev/null || echo '{}')"
  first_pts="$(printf '%s' "$act" | grep -o '"pts_ticks":[0-9]*' | head -1 | grep -o '[0-9]*' || true)"
  if [ -n "${first_pts:-}" ]; then
    # Start a few seconds before the cue so the clip contains its onset.
    ss="$(awk -v p="$first_pts" 'BEGIN{ s=p/90000-3; if(s<0)s=0; printf "%.2f", s }')"
    echo "[$slug] subtitle activity at ${first_pts}tk → clip -ss $ss"
  fi
  ffmpeg -y -hide_banner -loglevel error -copy_unknown -ss "$ss" -i "$full" -map 0 -c copy \
    -t "$CLIP_SECS" -f mpegts "$CLIP_OUT/$slug.ts"
  echo "[$slug] wrote fixtures/streams/$slug.ts"
done

echo "done. full set → $FULL_OUT (gitignored); committed clips → $CLIP_OUT"
