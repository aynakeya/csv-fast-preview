#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/gen_csv.sh <output.csv> <rows>
out="${1:-/tmp/csvfastview-large.csv}"
rows="${2:-1000000}"

echo "id,name,group,payload" > "$out"
awk -v rows="$rows" 'BEGIN {
  for (i = 1; i <= rows; i++) {
    g = i % 1000;
    printf "%d,user_%d,g_%d,data_%d\n", i, i, g, i;
  }
}' >> "$out"

ls -lh "$out"
