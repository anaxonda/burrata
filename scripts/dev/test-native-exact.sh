#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
if (($# != 1)) || [[ "$1" != *::* || "$1" == -* ]]; then
  echo "Usage: npm run test:native:exact -- <fully-qualified-test-name>" >&2
  echo "Example: npm run test:native:exact -- sidebar_layout::tests::section_changes_preserve_playlists_and_persist" >&2
  exit 2
fi
cd "$repo_root"
native_test_tmp="${TMPDIR:-$repo_root/.cache/native-test-tmp}"
mkdir -p "$native_test_tmp"
log_file="$(mktemp "$native_test_tmp/burrata-native-exact.XXXXXX")"
trap 'rm -f "$log_file"' EXIT
set +e
env \
  TMPDIR="$native_test_tmp" \
  CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}" \
  CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-0}" \
  CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/.cache/verification-target}" \
  cargo test --offline -j2 --manifest-path src-tauri/Cargo.toml --lib "$1" -- --exact \
  2>&1 | tee "$log_file"
pipeline_status=("${PIPESTATUS[@]}")
cargo_status="${pipeline_status[0]}"
tee_status="${pipeline_status[1]}"
set -e
if ((cargo_status != 0)); then
  exit "$cargo_status"
fi
if ((tee_status != 0)); then
  echo "Failed to retain Cargo output in $log_file." >&2
  exit "$tee_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed;' "$log_file"; then
  echo "Native exact test matched no passing tests; check the fully qualified test name." >&2
  exit 1
fi
