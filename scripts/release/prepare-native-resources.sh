#!/usr/bin/env bash
set -euo pipefail
: "${TARGET:?Rust target is required}"
ext=""
if [[ "$TARGET" == *windows* ]]; then ext='.exe'; fi
tool_dir="src-tauri/resources/tools/$TARGET"
target_dir="src-tauri/target/$TARGET/release"
recognizer_target_dir="src-tauri/recognizer/target/$TARGET/release"
mkdir -p "$tool_dir" src-tauri/resources/mcp
for bin in burrata-browser-host burrata-mcp; do
  src="$target_dir/${bin}${ext}"
  test -f "$src" || { echo "Required native helper was not built: $src" >&2; exit 1; }
  cp "$src" "$tool_dir/"
done
recognizer="$recognizer_target_dir/burrata-recognize${ext}"
test -f "$recognizer" || { echo "Required recognizer helper was not built: $recognizer" >&2; exit 1; }
cp "$recognizer" "$tool_dir/"
test -s src-tauri/resources/mcp/server.cjs || { echo 'Bundled MCP bridge missing' >&2; exit 1; }
