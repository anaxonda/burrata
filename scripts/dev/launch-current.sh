#!/usr/bin/env bash
# Launch the current debug executable against a verified Vite module endpoint.
# --isolated uses temporary XDG app data and removes it when the app exits.
set -euo pipefail
registration_timeout_seconds="${BURRATA_AUTOMATION_REGISTRATION_TIMEOUT_SECONDS:-20}"
if [[ ! "$registration_timeout_seconds" =~ ^([1-9][0-9]?|1[0-7][0-9]|180)$ ]]; then
  echo "BURRATA_AUTOMATION_REGISTRATION_TIMEOUT_SECONDS must be an integer from 1 to 180." >&2
  exit 2
fi
# Protected mode must own its Vite server; an existing listener may still send HMR.
if [[ "${BURRATA_PROTECTED_FRONTEND:-0}" != "0" && "${BURRATA_PROTECTED_FRONTEND:-0}" != "1" ]]; then
  echo "BURRATA_PROTECTED_FRONTEND must be 0 or 1." >&2
  exit 2
fi

usage() { echo "Usage: $0 [--isolated | --isolated-profile TEMP_DIR [--keep-profile]]" >&2; exit 2; }
mode="current"
profile_root=""
keep_profile=false
if (($#)); then
  if [[ "$#" == 1 && "$1" == "--isolated" ]]; then
  mode="isolated"
  elif [[ "$#" == 2 && "$1" == "--isolated-profile" ]]; then
    mode="isolated-profile"
    profile_root="$2"
  elif [[ "$#" == 3 && "$1" == "--isolated-profile" && "$3" == "--keep-profile" ]]; then
    mode="isolated-profile"
    profile_root="$2"
    keep_profile=true
  else
    usage
  fi
fi

project_root="$(cd "$(dirname "$0")/../.." && pwd)"
app_binary="${BURRATA_APP_BINARY:-$project_root/src-tauri/target/debug/burrata}"
frontend_index="$project_root/dist/index.html"

if ! command -v node >/dev/null || ! "$project_root/node_modules/.bin/vite" --version >/dev/null 2>&1; then
  echo "Vite/Node modules are not installed; install dependencies before launching." >&2
  exit 1
fi
if ! node -e 'const [major, minor] = process.versions.node.split(".").map(Number); process.exit(major > 22 || (major === 22 && minor >= 12) ? 0 : 1)' ; then
  echo "Burrata requires Node.js 22.12 or newer for the verified frontend toolchain." >&2
  exit 1
fi
if [[ ! -x "$app_binary" ]]; then
  echo "Compiled Burrata executable not found: $app_binary" >&2
  exit 1
fi
frontend_mode="dev"
if [[ "$app_binary" == */release/* ]]; then frontend_mode="release"; fi
if [[ "$frontend_mode" == "release" ]]; then
  if [[ ! -f "$frontend_index" ]]; then
    echo "Compiled frontend entry is missing; run the frontend build before launching." >&2
    exit 1
  fi
  entry_js="$(rg -o 'assets/index-[^" ]+\.js' "$frontend_index" | head -n 1 || true)"
  if [[ -z "$entry_js" || ! -s "$project_root/dist/$entry_js" ]]; then
    echo "dist/index.html does not reference an existing compiled JavaScript entry." >&2
    exit 1
  fi
  stale_frontend="$(find "$project_root/src" -type f -newer "$frontend_index" -print -quit)"
  for input in "$project_root/index.html" "$project_root/vite.config.ts" "$project_root/package.json" "$project_root/package-lock.json"; do
    if [[ "$input" -nt "$frontend_index" ]]; then stale_frontend="$input"; break; fi
  done
  if [[ -n "$stale_frontend" ]]; then
    echo "The compiled frontend is older than $stale_frontend; rebuild it before launching." >&2
    exit 1
  fi
  if [[ "$frontend_index" -nt "$app_binary" ]]; then
    echo "The native executable predates the compiled frontend; rebuild Burrata before launching." >&2
    exit 1
  fi
fi
stale_source="$(find "$project_root/src-tauri/src" "$project_root/src-tauri/capabilities" "$project_root/src-tauri/permissions" -type f -newer "$app_binary" -print -quit)"
for input in "$project_root/src-tauri/Cargo.toml" "$project_root/src-tauri/Cargo.lock" "$project_root/src-tauri/build.rs" "$project_root/src-tauri/tauri.conf.json"; do
  if [[ "$input" -nt "$app_binary" ]]; then stale_source="$input"; break; fi
done
# These provider assets are embedded by managed_provider_session.rs at compile time.
# Updating them also requires a new executable, even in Vite development mode.
for embedded_input in scripts/provider-session-worker.mjs scripts/youtube-session-worker.mjs extension/spotify-auth.js extension/provider-operations.js extension/youtube-session-provider.js extension/background.js extension/content.js extension/parsers.js extension/popup.html extension/popup.js; do
  if [[ "$project_root/$embedded_input" -nt "$app_binary" ]]; then stale_source="$project_root/$embedded_input"; break; fi
done
if [[ -n "$stale_source" ]]; then
  echo "The native executable is older than $stale_source; compile Burrata before launching." >&2
  exit 1
fi

if [[ "$mode" == "isolated-profile" ]]; then
  temp_root="$(realpath -e "${TMPDIR:-/tmp}")"
  [[ -d "$profile_root" && ! -L "$profile_root" ]] || { echo "Isolated profile must be an existing real directory." >&2; exit 1; }
  profile_root="$(realpath -e "$profile_root")"
  [[ "$(dirname "$profile_root")" == "$temp_root" && "$(basename "$profile_root")" == burrata-launch.* ]] || { echo "Isolated profile must be directly under the temporary directory with a burrata-launch.* name." >&2; exit 1; }
  [[ -f "$profile_root/.burrata-isolated-profile" && ! -L "$profile_root/.burrata-isolated-profile" ]] || { echo "Isolated profile marker is missing." >&2; exit 1; }
  scratch_dir="$profile_root"
else
  scratch_dir="$(mktemp -d "${TMPDIR:-/tmp}/burrata-launch.XXXXXX")"
fi
launch_log_dir="${BURRATA_LAUNCH_LOG_DIR:-}"
if [[ -n "$launch_log_dir" ]]; then
  mkdir -p "$launch_log_dir"
  launch_log_dir="$(realpath -e "$launch_log_dir")"
  [[ "$launch_log_dir" != "$scratch_dir" && "$launch_log_dir" != "$scratch_dir"/* ]] || { echo "Launch log directory cannot be inside the disposable profile." >&2; exit 1; }
fi
mkdir -p "$scratch_dir"/{cache,config,data,state,tmp}
export BURRATA_VITE_CACHE_DIR="$scratch_dir/cache/vite"
if [[ "$mode" == "isolated" || "$mode" == "isolated-profile" ]]; then
  if [[ "$mode" == "isolated" ]]; then
    printf 'burrata isolated memory run\n' > "$scratch_dir/.burrata-isolated-profile"
  fi
  source "$project_root/scripts/dev/isolated-music-root.sh"
  configure_isolated_music_root "$scratch_dir" || exit 1
  export XDG_CONFIG_HOME="$scratch_dir/config"
  export XDG_DATA_HOME="$scratch_dir/data"
  export XDG_CACHE_HOME="$scratch_dir/cache"
  export XDG_STATE_HOME="$scratch_dir/state"
  export TMPDIR="$scratch_dir/tmp"
fi

data_home="${XDG_DATA_HOME:-${HOME:?HOME is required to locate Burrata app data}/.local/share}"
config_home="${XDG_CONFIG_HOME:-${HOME:?HOME is required to locate Burrata app config}/.config}"
if [[ "$data_home" != /* || "$config_home" != /* ]]; then
  echo "XDG_DATA_HOME and XDG_CONFIG_HOME must be absolute paths for profile-safe launch." >&2
  [[ "$keep_profile" == true ]] || rm -rf -- "$scratch_dir"
  exit 1
fi
automation_file="$data_home/org.burrata.discovery/automation.json"
app_binary_real="$(realpath -e "$app_binary")"
target_data_profile="$(realpath -m -- "$data_home")"
target_config_profile="$(realpath -m -- "$config_home")"
source "$project_root/scripts/dev/process-profile-guard.sh"
guard_status=0
if process_profile_conflict "$app_binary_real" "$target_data_profile" "$target_config_profile"; then
  echo "Burrata already has a process using this or an overlapping profile (PID $PROFILE_GUARD_PID); refusing a duplicate launch." >&2
  [[ "$keep_profile" == true ]] || rm -rf -- "$scratch_dir"
  exit 1
else
  guard_status=$?
fi
if [[ "$guard_status" == 2 ]]; then
  echo "Cannot safely verify a running Burrata process (PID $PROFILE_GUARD_PID): $PROFILE_GUARD_REASON; refusing launch." >&2
  [[ "$keep_profile" == true ]] || rm -rf -- "$scratch_dir"
  exit 1
fi

launch_log="$scratch_dir/app.log"
source "$project_root/scripts/dev/automation-ready.sh"
automation_identity_before=""
if [[ -e "$automation_file" || -L "$automation_file" ]]; then
  automation_identity_before="$(automation_file_identity "$automation_file")" || {
    echo "Cannot inspect the existing automation endpoint identity before launch." >&2
    exit 1
  }
fi
vite_pid=""
vite_owner=""
app_pid=""
process_identity() {
  local pid="$1" stat_tail uid start_ticks
  [[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 && -r "/proc/$pid/stat" && -r "/proc/$pid/status" ]] || return 1
  stat_tail="$(sed 's/^.*) //' "/proc/$pid/stat")"
  start_ticks="$(awk '{ print $20 }' <<< "$stat_tail")"
  uid="$(awk '$1 == "Uid:" { print $2; exit }' "/proc/$pid/status")"
  [[ -n "$start_ticks" && "$uid" == "$(id -u)" ]] || return 1
  printf '%s %s' "$uid" "$start_ticks"
}
is_descendant_of() {
  local pid="$1" ancestor="$2" stat_tail parent
  while [[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 ]]; do
    [[ "$pid" == "$ancestor" ]] && return 0
    [[ -r "/proc/$pid/stat" ]] || return 1
    stat_tail="$(sed 's/^.*) //' "/proc/$pid/stat")"
    parent="$(awk '{ print $2 }' <<< "$stat_tail")"
    [[ -n "$parent" && "$parent" != "$pid" ]] || return 1
    pid="$parent"
  done
  return 1
}
cleanup() {
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
    kill -TERM "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [[ "$vite_owner" == "owned" && -n "$vite_pid" ]] && kill -0 "$vite_pid" 2>/dev/null; then
    kill -TERM "$vite_pid" 2>/dev/null || true
    wait "$vite_pid" 2>/dev/null || true
  fi
  if [[ -n "$launch_log_dir" ]]; then
    [[ ! -f "$scratch_dir/app.log" ]] || cp -- "$scratch_dir/app.log" "$launch_log_dir/app.log"
    [[ ! -f "$scratch_dir/vite.log" ]] || cp -- "$scratch_dir/vite.log" "$launch_log_dir/vite.log"
  fi
  [[ "$keep_profile" == true ]] || rm -rf -- "$scratch_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ "$frontend_mode" == "dev" ]]; then
  node_modules/.bin/vite --configLoader runner --host 127.0.0.1 --port 1420 --strictPort >"$scratch_dir/vite.log" 2>&1 &
  launched_vite_pid=$!
  # Preflight can fail or be interrupted before listener discovery. Cleanup
  # still owns this child, and must not strand its compiler/server process.
  vite_pid="$launched_vite_pid"
  vite_owner="owned"
  if ! node "$project_root/scripts/dev/verify-frontend-modules.mjs"; then
    echo "The compiled Vite module check failed; Burrata was not launched." >&2
    exit 1
  fi
  listener_pid=""
  if command -v ss >/dev/null 2>&1; then
    listener_pids="$(ss -H -lptn 'sport = :1420' 2>/dev/null | sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p' | sort -u)"
    if [[ "$(wc -l <<< "$listener_pids" | tr -d ' ')" == 1 ]]; then listener_pid="$listener_pids"; fi
  fi
  if [[ -z "$listener_pid" ]]; then
    echo "Could not identify the live listener serving port 1420; Burrata was not launched." >&2
    exit 1
  fi
  listener_identity="$(process_identity "$listener_pid")" || { echo "Port 1420 listener identity is inaccessible or belongs to another user." >&2; exit 1; }
  if is_descendant_of "$listener_pid" "$launched_vite_pid"; then
    vite_pid="$listener_pid"
    vite_owner="owned"
  else
    if [[ "${BURRATA_PROTECTED_FRONTEND:-0}" == "1" ]]; then
      echo "Protected frontend requires its own Vite listener; refusing to reuse the existing port 1420 server." >&2
      exit 1
    fi
    vite_pid="$listener_pid"
    vite_owner="shared"
    echo "Using already-running verified Vite listener (PID $vite_pid) as a shared server; cleanup will leave it running."
  fi
  if [[ -n "${BURRATA_LAUNCH_VITE_PID_FILE:-}" ]]; then
    printf '%s %s %s %s\n' "$vite_pid" "$listener_identity" "$vite_owner" > "$BURRATA_LAUNCH_VITE_PID_FILE"
  fi
fi

"$app_binary" >"$launch_log" 2>&1 &
app_pid=$!
printf 'Started compiled Burrata (%s profile); waiting for the authenticated MCP app-status handshake.\n' "$mode"
ready=false
for ((attempt=0; attempt<registration_timeout_seconds*4; attempt++)); do
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "Burrata exited before registering a fresh automation endpoint." >&2
    exit 1
  fi
  if automation_registration_changed "$automation_file" "$automation_identity_before"; then ready=true; break; fi
  sleep 0.25
done
if [[ "$ready" != true ]]; then
  echo "Burrata did not register a fresh automation endpoint within ${registration_timeout_seconds} seconds." >&2
  exit 1
fi
if ! node "$project_root/scripts/release/check-installed-mcp.mjs" "$project_root/scripts/dev/mcp-stdio.sh" "$automation_file"; then
  echo "The authenticated MCP get_app_status handshake did not confirm a mounted frontend." >&2
  exit 1
fi
printf 'MCP confirms the app is running with its frontend mounted. Close Burrata to finish.\n'
wait "$app_pid"
