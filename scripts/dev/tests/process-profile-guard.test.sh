#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd "$(dirname "$0")/.." && pwd)"
source "$script_dir/process-profile-guard.sh"
root="$(mktemp -d "${TMPDIR:-/tmp}/burrata-profile-guard-test.XXXXXX")"
trap 'rm -rf -- "$root"' EXIT
uid="$(id -u)"
mkdir -p "$root/bin" "$root/proc"
binary="$root/bin/burrata"
printf '#!/bin/sh\n' > "$binary"
chmod +x "$binary"
write_process() {
  local pid="$1" executable="$2" comm="$3" home="$4" data="$5" config="$6" env_mode="${7:-full}" cmd_mode="${8:-gui}"
  local dir="$root/proc/$pid"
  mkdir -p "$dir"
  printf '%s\n' "$comm" > "$dir/comm"
  printf 'Name:\t%s\nUid:\t%s\t%s\t%s\t%s\n' "$comm" "$uid" "$uid" "$uid" "$uid" > "$dir/status"
  case "$cmd_mode" in
    gui) printf 'burrata\0--launch-gui\0' > "$dir/cmdline" ;;
    missing-cmdline) ;;
    *) printf 'burrata\0--other-option\0%s\0' "$cmd_mode" > "$dir/cmdline" ;;
  esac
  [[ "$env_mode" != "full" ]] || printf 'HOME=%s\0XDG_DATA_HOME=%s\0XDG_CONFIG_HOME=%s\0' "$home" "$data" "$config" > "$dir/environ"
  if [[ "$executable" != "missing" ]]; then ln -s -- "$executable" "$dir/exe"; fi
}
expect_status() {
  local expected="$1"; shift
  local status=0
  process_profile_conflict "$@" "$root/proc" || status=$?
  [[ "$status" == "$expected" ]] || { echo "expected status $expected, got $status" >&2; exit 1; }
}
requested_data="$root/real/data"
requested_config="$root/real/config"
mkdir -p "$requested_data" "$requested_config"
write_process 700001 "$binary" burrata "$root/home" "$requested_data" "$requested_config"
expect_status 0 "$binary" "$requested_data" "$requested_config"
[[ "$PROFILE_GUARD_PID" == 700001 ]]
expect_status 1 "$binary" "$root/isolated/data" "$root/isolated/config"
rm -rf "$root/proc/700001"

native_pid=700010
for native_flag in --native-messaging --browser-native-host --mcp; do
  write_process "$native_pid" "$binary" burrata "$root/home" "$requested_data" "$requested_config" full "$native_flag"
  expect_status 1 "$binary" "$requested_data" "$requested_config"
  rm -rf "$root/proc/$native_pid"
  ((native_pid += 1))
done

write_process 700020 "$binary" burrata "$root/home" "$requested_data" "$requested_config" full missing-cmdline
expect_status 2 "$binary" "$requested_data" "$requested_config"
[[ "$PROFILE_GUARD_REASON" == *command* ]]
rm -rf "$root/proc/700020"

other_binary="$root/bin/burrata-old"
printf '#!/bin/sh\n' > "$other_binary"
write_process 700002 "$other_binary" burrata "$root/home" "$requested_data" "$requested_config"
expect_status 0 "$binary" "$requested_data" "$requested_config"
rm -rf "$root/proc/700002"

write_process 700003 "$binary (deleted)" burrata "$root/home" "$requested_data" "$requested_config"
expect_status 0 "$binary" "$requested_data" "$requested_config"
rm -rf "$root/proc/700003"

long_binary="$root/bin/burrata-truncated-process-name"
printf '#!/bin/sh\n' > "$long_binary"
truncated_comm="${long_binary##*/}"
truncated_comm="${truncated_comm:0:15}"
write_process 700004 "$long_binary" "$truncated_comm" "$root/home" "$requested_data" "$requested_config"
expect_status 0 "$long_binary" "$requested_data" "$requested_config"
rm -rf "$root/proc/700004"

write_process 700005 "$binary" burrata "$root/home" "$requested_data" "$requested_config" missing-env
expect_status 2 "$binary" "$requested_data" "$requested_config"
[[ "$PROFILE_GUARD_REASON" == *environment* ]]
rm -rf "$root/proc/700005"

write_process 700006 missing burrata "$root/home" "$requested_data" "$requested_config"
expect_status 2 "$binary" "$requested_data" "$requested_config"
[[ "$PROFILE_GUARD_REASON" == *executable* ]]
rm -rf "$root/proc/700006"

write_process 700007 "$other_binary" burrata "$root/home" "$root/isolated/data" "$root/isolated/config"
rm "$root/proc/700007/comm"
expect_status 1 "$binary" "$requested_data" "$requested_config"
printf 'process profile guard fixtures passed\n'
