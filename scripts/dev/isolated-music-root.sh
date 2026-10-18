#!/usr/bin/env bash
# Shared by the launcher and its filesystem-only regression tests.
configure_isolated_music_root() {
  local profile="$1" canonical suffix component marker
  canonical="$(realpath -e -- "$profile")" || return 1
  suffix="${profile##*/burrata-launch.}"
  if [[ "$canonical" != "$profile" || "$(dirname -- "$profile")" != /tmp ||
        "$(realpath -e /tmp)" != /tmp || ! "$suffix" =~ ^[A-Za-z0-9_-]+$ ||
        "$(basename -- "$profile")" != "burrata-launch.$suffix" ]]; then
    echo "Isolated Music requires a real /tmp/burrata-launch.<suffix> profile." >&2
    return 1
  fi
  marker="$profile/.burrata-isolated-profile"
  if [[ -e "$profile/.burrata-isolation-unqualified" ]]; then
    echo "This test profile has unqualified isolation; prepare a fresh profile." >&2
    return 1
  fi
  if [[ ! -f "$marker" || -L "$marker" ]] ||
      ! cmp -s -- "$marker" <(printf 'burrata isolated memory run\n'); then
    echo "Isolated profile marker has invalid contents." >&2
    return 1
  fi
  for component in "$profile" "$profile/fixture" "$profile/fixture/Music"; do
    if [[ -L "$component" || ( -e "$component" && ! -d "$component" ) ]]; then
      echo "Isolated Music components must be real directories." >&2
      return 1
    fi
  done
  mkdir -p -- "$profile/fixture/Music" || return 1
  [[ "$(realpath -e -- "$profile/fixture/Music")" == "$profile/fixture/Music" ]] || return 1
  export BURRATA_ISOLATED_MUSIC_ROOT="$profile/fixture/Music"
}
