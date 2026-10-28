#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
if (($# == 0)); then
  echo "Usage: npm run test:watch -- <test-file-or-pattern> [Vitest options]" >&2
  echo "Example: npm run test:watch -- src/App.integration.test.ts -t 'radio playback'" >&2
  exit 2
fi
cd "$repo_root"
exec ./node_modules/.bin/vitest --configLoader runner "$@"
