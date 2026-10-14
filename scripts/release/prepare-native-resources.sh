#!/usr/bin/env bash
set -euo pipefail
: "${TARGET:?Rust target is required}"
ext=""
if [[ "$TARGET" == *windows* ]]; then ext='.exe'; fi
tool_dir="src-tauri/resources/tools/$TARGET"
mkdir -p "$tool_dir" src-tauri/resources/mcp
for bin in burrata-browser-host burrata-mcp; do
  src="src-tauri/target/release/${bin}${ext}"
  test -f "$src" || { echo "Required native helper was not built: $src" >&2; exit 1; }
  cp "$src" "$tool_dir/"
done
recognizer="src-tauri/recognizer/target/release/burrata-recognize${ext}"
test -f "$recognizer" || { echo "Required recognizer helper was not built: $recognizer" >&2; exit 1; }
cp "$recognizer" "$tool_dir/"
test -s src-tauri/resources/mcp/server.cjs || { echo 'Bundled MCP bridge missing' >&2; exit 1; }
