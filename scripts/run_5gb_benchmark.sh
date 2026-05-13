#!/usr/bin/env bash
set -euo pipefail

file="${1:-/tmp/csvfastview-5gb.csv}"
report="${2:-/tmp/csvfastview-5gb-report.txt}"

echo "[1/3] generating 5GB csv at $file"
./scripts/gen_csv_by_size.sh "$file" 5

echo "[2/3] running benchmark"
{
  echo "date=$(date -Iseconds)"
  echo "file=$file"
  ls -lh "$file"
  /usr/bin/time -f 'wall=%E maxrss_kb=%M' cargo run --quiet --bin bench -- "$file" , utf8 1 user_1
} | tee "$report"

echo "[3/3] report: $report"
