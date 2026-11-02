#!/usr/bin/env bash
set -euo pipefail
if [[ $# -ne 1 ]]; then
  echo "usage: $0 /absolute/or/relative/output/libipc_counter.so" >&2
  exit 2
fi
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source="$repo_root/scripts/performance/ipc_counter_shim.c"
output="$1"
cc_bin="${CC:-cc}"
mkdir -p "$(dirname "$output")"
"$cc_bin" -shared -fPIC -o "$output" "$source" \
  $(pkg-config --cflags --libs gobject-2.0) -ldl -pthread
