# Device/inode identity changes when Burrata atomically replaces automation.json.
automation_file_identity() {
  local path="$1"
  [[ -e "$path" || -L "$path" ]] || return 1
  stat -c '%d:%i' -- "$path" 2>/dev/null
}

automation_registration_changed() {
  local path="$1" previous_identity="$2" current_identity
  current_identity="$(automation_file_identity "$path")" || return 1
  [[ -n "$current_identity" && "$current_identity" != "$previous_identity" ]]
}
