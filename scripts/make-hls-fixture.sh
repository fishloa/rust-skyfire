#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Default fixture = h264-25fps.ts: a clean 15s progressive-H.264 TS that decodes
# without error under headless Chromium WebCodecs (france2-8s.ts and gulli-15s.ts
# hit a headless-only decoding-error quirk *as direct files too* — unrelated to
# HLS — so they are unsuitable for the strict no-console-error e2e gate). Pass a
# path to segment a different source, e.g.:
#   scripts/make-hls-fixture.sh fixtures/france2-8s.ts
# france2 is what proves DVB-subtitle PIDs survive segmentation (ffprobe the
# segments) — the `segment` muxer keeps every PID; verified in issue #60.
SRC="${1:-$ROOT/fixtures/h264-25fps.ts}"
OUT="$ROOT/web/fixtures-hls"
rm -rf "$OUT"; mkdir -p "$OUT"
# `segment` muxer (NOT `-f hls`, which splits DVB-sub into WebVTT and errors).
# -map 0 -c copy keeps every PID (video + AC-3/E-AC-3 + DVB-subtitle) in each .ts.
ffmpeg -hide_banner -loglevel error -i "$SRC" -map 0 -c copy \
  -f segment -segment_time 2 -segment_format mpegts \
  -segment_list "$OUT/index.m3u8" -segment_list_type m3u8 \
  "$OUT/seg%d.ts"
echo "wrote $OUT:"; ls -1 "$OUT"
