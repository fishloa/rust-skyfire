#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:-$ROOT/fixtures/france2-8s.ts}"
OUT="$ROOT/web/fixtures-hls"
rm -rf "$OUT"; mkdir -p "$OUT"
# `segment` muxer (NOT `-f hls`, which splits DVB-sub into WebVTT and errors).
# -map 0 -c copy keeps every PID (video + AC-3/E-AC-3 + DVB-subtitle) in each .ts.
ffmpeg -hide_banner -loglevel error -i "$SRC" -map 0 -c copy \
  -f segment -segment_time 2 -segment_format mpegts \
  -segment_list "$OUT/index.m3u8" -segment_list_type m3u8 \
  "$OUT/seg%d.ts"
echo "wrote $OUT:"; ls -1 "$OUT"
