# Return 0 when a Burrata executable/profile overlap is found, 1 when safe,
# and 2 when a matching process cannot be inspected safely. Never print env.
process_profile_conflict() {
  local target_binary="$1" target_data="$2" target_config="$3" proc_root="${4:-/proc}"
  local expected_comm app_comm target_uid pid proc_dir comm uid key first rest exe_link arg native_host
  local process_home data_home config_home entry candidate_data candidate_config
  local -a process_environment=() process_cmdline=()
  local -A candidate_pids=()
  PROFILE_GUARD_PID=""
  PROFILE_GUARD_REASON=""
  expected_comm="$(basename -- "$target_binary")"
  expected_comm="${expected_comm:0:15}"
  app_comm="burrata"
  target_uid="$(id -u)"

  # find compares executable inode links in one process, catching a renamed
  # executable without spawning readlink/realpath once per PID.
  while IFS= read -r proc_dir; do
    pid="${proc_dir%/exe}"
    pid="${pid##*/}"
    [[ "$pid" =~ ^[0-9]+$ ]] && candidate_pids["$pid"]=1
  done < <(find -L "$proc_root" -mindepth 2 -maxdepth 2 -name exe -samefile "$target_binary" -print 2>/dev/null)

  # comm is read with Bash builtins. Unrelated unreadable processes are ignored;
  # a matching target-inode candidate is checked and fails closed below.
  for proc_dir in "$proc_root"/[0-9]*; do
    [[ -d "$proc_dir" ]] || continue
    pid="${proc_dir##*/}"
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    if IFS= read -r comm 2>/dev/null < "$proc_dir/comm"; then
      if [[ "$comm" == "$app_comm" || "$comm" == "$expected_comm" ]]; then
        candidate_pids["$pid"]=1
      fi
    fi
  done

  for pid in "${!candidate_pids[@]}"; do
    proc_dir="$proc_root/$pid"
    [[ -d "$proc_dir" ]] || continue
    if ! exe_link="$(readlink -- "$proc_dir/exe" 2>/dev/null)"; then
      [[ -e "$proc_dir" ]] || continue
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="could not inspect matching process executable"
      return 2
    fi
    comm=""
    if ! IFS= read -r comm 2>/dev/null < "$proc_dir/comm"; then
      [[ -e "$proc_dir" ]] || continue
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="could not read matching process name"
      return 2
    fi
    process_cmdline=()
    if ! mapfile -d '' -t process_cmdline 2>/dev/null < "$proc_dir/cmdline"; then
      [[ -e "$proc_dir" ]] || continue
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="could not inspect matching process command mode"
      return 2
    fi
    if ((${#process_cmdline[@]} == 0)); then
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="matching process command mode is unavailable"
      return 2
    fi
    native_host=false
    for arg in "${process_cmdline[@]}"; do
      if [[ "$arg" == "--native-messaging" || "$arg" == "--browser-native-host" || "$arg" == "--mcp" ]]; then
        native_host=true
        break
      fi
    done
    [[ "$native_host" != true ]] || continue

    uid=""
    if [[ -r "$proc_dir/status" ]]; then
      while IFS=$' \t' read -r key first rest; do
        if [[ "$key" == "Uid:" ]]; then uid="$first"; break; fi
      done < "$proc_dir/status"
    fi
    if [[ -z "$uid" ]]; then
      [[ -e "$proc_dir" ]] || continue
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="could not verify matching process owner"
      return 2
    fi
    [[ "$uid" == "$target_uid" ]] || continue

    process_environment=()
    if ! mapfile -d '' -t process_environment 2>/dev/null < "$proc_dir/environ"; then
      [[ -e "$proc_dir" ]] || continue
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="could not inspect matching process environment"
      return 2
    fi
    process_home=""; data_home=""; config_home=""
    for entry in "${process_environment[@]}"; do
      case "$entry" in
        HOME=*) process_home="${entry#*=}" ;;
        XDG_DATA_HOME=*) data_home="${entry#*=}" ;;
        XDG_CONFIG_HOME=*) config_home="${entry#*=}" ;;
      esac
    done
    if [[ -z "$process_home" ]]; then
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="matching process has no readable HOME value"
      return 2
    fi
    data_home="${data_home:-$process_home/.local/share}"
    config_home="${config_home:-$process_home/.config}"
    if [[ "$data_home" != /* || "$config_home" != /* ]]; then
      PROFILE_GUARD_PID="$pid"
      PROFILE_GUARD_REASON="matching process has invalid XDG profile paths"
      return 2
    fi
    candidate_data="$(realpath -m -- "$data_home" 2>/dev/null)" || {
      PROFILE_GUARD_PID="$pid"; PROFILE_GUARD_REASON="could not canonicalize matching process data profile"; return 2;
    }
    candidate_config="$(realpath -m -- "$config_home" 2>/dev/null)" || {
      PROFILE_GUARD_PID="$pid"; PROFILE_GUARD_REASON="could not canonicalize matching process config profile"; return 2;
    }
    if [[ "$candidate_data" == "$target_data" || "$candidate_config" == "$target_config" ]]; then
      PROFILE_GUARD_PID="$pid"
      return 0
    fi
  done
  return 1
}
