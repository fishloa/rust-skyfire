#!/usr/bin/env bash
# delegate.sh <issue#> [model]
#
# Fill the brief template for a GitHub issue and run it through an external
# engineering model via `crush`. Claude orchestrates and verifies; the model
# writes the code. Default model: deepseek/deepseek-v4-pro.
set -euo pipefail

N="${1:?usage: delegate.sh <issue#> [model]}"
MODEL="${2:-deepseek/deepseek-v4-pro}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "$ROOT/.delegate"
BRIEF="$ROOT/.delegate/brief-${N}.txt"
LOG="$ROOT/.delegate/issue-${N}.log"

sed "s/__N__/${N}/g" "$ROOT/.delegate/SKYFIRE_BRIEF.tmpl" > "$BRIEF"

echo "issue:  #${N}"
echo "model:  ${MODEL}"
echo "brief:  ${BRIEF}"
echo "log:    ${LOG}"
echo "--- launching crush (verify the result against the CI gate yourself) ---"

crush run -m "$MODEL" -c "$ROOT" --quiet "$(cat "$BRIEF")" >> "$LOG" 2>&1
