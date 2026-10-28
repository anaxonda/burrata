#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
wrapper="$repo_root/scripts/dev/test-native-exact.sh"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/burrata-native-exact-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
cat >"$tmp/bin/cargo" <<'MOCK_CARGO'
#!/usr/bin/env bash
set -euo pipefail
case "${MOCK_CARGO_MODE:-}" in
  pass)
    printf 'test sidebar_layout::tests::section_changes_preserve_playlists_and_persist ... ok\n'
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.01s\n'
    ;;
  no_match)
    printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n'
    ;;
  compile_fail)
    printf 'error: could not compile `burrata` due to previous error\n'
    exit 101
    ;;
  test_fail)
    printf 'test sidebar_layout::tests::section_changes_preserve_playlists_and_persist ... FAILED\n'
    printf 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.01s\n'
    exit 42
    ;;
  *)
    printf 'unexpected mock mode\n' >&2
    exit 99
    ;;
esac
MOCK_CARGO
chmod +x "$tmp/bin/cargo"
test_name='sidebar_layout::tests::section_changes_preserve_playlists_and_persist'

if MOCK_CARGO_MODE=pass PATH="$tmp/bin:$PATH" bash "$wrapper" "$test_name" >"$tmp/pass.log" 2>&1; then
  :
else
  cat "$tmp/pass.log" >&2
  echo "expected a passing exact test to succeed" >&2
  exit 1
fi
if MOCK_CARGO_MODE=no_match PATH="$tmp/bin:$PATH" bash "$wrapper" "$test_name" >"$tmp/no-match.log" 2>&1; then
  echo "expected a zero-test Cargo result to fail" >&2
  exit 1
fi
grep -q 'matched no passing tests' "$tmp/no-match.log"
set +e
MOCK_CARGO_MODE=compile_fail PATH="$tmp/bin:$PATH" bash "$wrapper" "$test_name" >"$tmp/compile-fail.log" 2>&1
compile_status=$?
set -e
if ((compile_status != 101)); then
  cat "$tmp/compile-fail.log" >&2
  echo "expected compile failure status 101, got $compile_status" >&2
  exit 1
fi
grep -q 'could not compile' "$tmp/compile-fail.log"
set +e
MOCK_CARGO_MODE=test_fail PATH="$tmp/bin:$PATH" bash "$wrapper" "$test_name" >"$tmp/test-fail.log" 2>&1
test_status=$?
set -e
if ((test_status != 42)); then
  cat "$tmp/test-fail.log" >&2
  echo "expected test failure status 42, got $test_status" >&2
  exit 1
fi
grep -q 'test result: FAILED' "$tmp/test-fail.log"
echo "native exact preset mock checks passed"
