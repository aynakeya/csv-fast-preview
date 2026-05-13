#!/usr/bin/env bash
set -euo pipefail

out="${1:-wide_long_headers_5gb.csv}"
size_gb="${2:-5}"
cols="${3:-64}"

target=$((size_gb * 1000 * 1000 * 1000))

header=""
for i in $(seq 1 "$cols"); do
  name="very_long_column_name_${i}_this_header_is_intentionally_verbose_for_wide_csv_preview_testing_${i}_suffix"
  if [ "$i" -eq 1 ]; then
    header="$name"
  else
    header="$header,$name"
  fi
done

: > "$out"
printf '%s\n' "$header" >> "$out"

row=""
for i in $(seq 1 "$cols"); do
  value="value_${i}_payload_for_column_${i}_abcdef1234567890"
  if [ "$i" -eq 1 ]; then
    row="$value"
  else
    row="$row,$value"
  fi
done
row_len=$((${#row} + 1))
current=$(wc -c < "$out")
need=$((target - current))
rows=$((need / row_len + 1))

printf 'target_bytes=%s\n' "$target"
printf 'columns=%s\n' "$cols"
printf 'row_bytes=%s\n' "$row_len"
printf 'rows_to_write=%s\n' "$rows"
printf 'output=%s\n' "$out"

awk -v rows="$rows" -v cols="$cols" 'BEGIN {
  for (r = 1; r <= rows; r++) {
    for (c = 1; c <= cols; c++) {
      if (c > 1) printf ",";
      printf "row_%d_col_%d_value_payload_abcdef1234567890", r, c;
    }
    printf "\n";
  }
}' >> "$out"

# Trim down to approximately target bytes while keeping the file usable enough for preview/bench.
actual=$(wc -c < "$out")
if [ "$actual" -gt "$target" ]; then
  truncate -s "$target" "$out"
  printf '\n' >> "$out"
fi

ls -lh "$out"
wc -l "$out"
