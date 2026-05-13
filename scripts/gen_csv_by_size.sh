#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/gen_csv_by_size.sh <output.csv> <size_gb>
out="${1:-/tmp/csvfastview-5gb.csv}"
size_gb="${2:-5}"

# Generate a long payload to reduce row count while keeping valid CSV.
payload_len=900
payload=$(printf 'x%.0s' $(seq 1 "$payload_len"))
line="1,user_1,g_1,${payload}"

: > "$out"
echo "id,name,group,payload" >> "$out"

# target bytes in decimal GB
_target=$((size_gb * 1000 * 1000 * 1000))
cur=$(wc -c < "$out")
need=$((_target - cur))
if [ "$need" -le 0 ]; then
  ls -lh "$out"
  exit 0
fi

# Fill by streaming repeated valid rows
{ yes "$line" 2>/dev/null | head -c "$need" >> "$out"; } || true

# Ensure file ends with newline (optional)
echo >> "$out"
ls -lh "$out"
wc -l "$out"
