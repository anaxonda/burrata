#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source_dir="$repo_root/scripts/performance"
work="$(mktemp -d "${TMPDIR:-/tmp}/burrata-ipc-counter.XXXXXX")"
trap 'rm -rf "$work"' EXIT
cc_bin="${CC:-cc}"

"$cc_bin" -shared -fPIC -o "$work/libfixture.so" "$source_dir/ipc_counter_fixture_library.c" \
  $(pkg-config --cflags --libs gobject-2.0)
CC="$cc_bin" "$source_dir/ipc_counter_build.sh" "$work/libipc_counter.so"
"$cc_bin" -o "$work/fixture" "$source_dir/ipc_counter_fixture.c" -L"$work" -lfixture \
  -Wl,-rpath,"$work" $(pkg-config --cflags --libs gobject-2.0)

run_fixture() {
  local mode="$1" expected_registered="$2" expected_count="$3"
  local counter="$work/$mode.json"
  if [[ "$mode" == failed-ipc ]]; then
    IPC_COUNTER_FIXTURE_FAIL_IPC=1
    export IPC_COUNTER_FIXTURE_FAIL_IPC
  fi
  BURRATA_IPC_COUNTER_ENABLE=1 \
  BURRATA_IPC_COUNTER_PATH="$counter" \
  LD_PRELOAD="$work/libipc_counter.so" \
    bash -c 'export BURRATA_IPC_COUNTER_PID=$$; exec "$@"' _ "$work/fixture" "$mode"
  unset IPC_COUNTER_FIXTURE_FAIL_IPC || true
  python3 - "$counter" "$expected_registered" "$expected_count" <<'PY'
import json, pathlib, sys
path, registered, count = pathlib.Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
if not path.exists():
    assert registered == count == 0, (path, registered, count)
else:
    value = json.loads(path.read_text())
    assert value == {"registered": registered, "count": count}, value
    assert path.stat().st_mode & 0o777 == 0o600
PY
  printf '%s: registered=%s count=%s\n' "$mode" "$expected_registered" "$expected_count"
}

run_disabled_fixture() {
  local mode="$1"
  local counter="$work/$mode.json"
  if [[ "$mode" == disabled ]]; then
    BURRATA_IPC_COUNTER_PATH="$counter" \
    BURRATA_IPC_COUNTER_PID="$$" \
    LD_PRELOAD="$work/libipc_counter.so" "$work/fixture" non-ipc
  else
    BURRATA_IPC_COUNTER_ENABLE=1 \
    BURRATA_IPC_COUNTER_PATH="$counter" \
    BURRATA_IPC_COUNTER_PID="$$" \
    LD_PRELOAD="$work/libipc_counter.so" "$work/fixture" non-ipc
  fi
  test ! -e "$counter"
  printf '%s: no counter file\n' "$mode"
}

run_disabled_fixture disabled
run_disabled_fixture wrong-pid
run_fixture non-ipc 0 0
run_fixture failed-ipc 0 0
run_fixture ipc 1 2

webkit_pkg=""
if pkg-config --exists webkit2gtk-4.1; then
  webkit_pkg=webkit2gtk-4.1
elif pkg-config --exists webkit2gtk-4.0; then
  webkit_pkg=webkit2gtk-4.0
fi
if [[ -n "$webkit_pkg" ]]; then
  "$cc_bin" -o "$work/native-fixture" "$source_dir/ipc_counter_native_fixture.c" \
    $(pkg-config --cflags --libs "$webkit_pkg")
  counter="$work/native.json"
  BURRATA_IPC_COUNTER_ENABLE=1 \
  BURRATA_IPC_COUNTER_PATH="$counter" \
  LD_PRELOAD="$work/libipc_counter.so" \
    bash -c 'export BURRATA_IPC_COUNTER_PID=$$; exec "$@"' _ "$work/native-fixture"
  python3 - "$counter" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text())
assert value == {"registered": 1, "count": 2}, value
assert path.stat().st_mode & 0o777 == 0o600
PY
  printf 'real %s manager: registered=1 count=2\n' "$webkit_pkg"
else
  printf 'real WebKitGTK fixture skipped (neither webkit2gtk-4.1 nor -4.0 is installed)\n'
fi
