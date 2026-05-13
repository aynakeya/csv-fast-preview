#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/bench_large.sh <csv_path> [delimiter] [encoding] [col] [keyword]
file="${1:?csv path required}"
delim="${2:-,}"
encoding="${3:-utf8}"
col="${4:-1}"
keyword="${5:-user_999999}"

/usr/bin/time -f 'wall=%E maxrss_kb=%M' \
  cargo run --quiet --bin bench -- "$file" "$delim" "$encoding" "$col" "$keyword"
