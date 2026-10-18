#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$project_root/scripts/dev/isolated-music-root.sh"
profile="$(mktemp -d /tmp/burrata-launch.XXXXXXXX)"
trap 'rm -rf -- "$profile"' EXIT
printf 'burrata isolated memory run\n' > "$profile/.burrata-isolated-profile"
export BURRATA_ISOLATED_MUSIC_ROOT=/invalid-inherited-root
configure_isolated_music_root "$profile"
[[ "$BURRATA_ISOLATED_MUSIC_ROOT" == "$profile/fixture/Music" ]]
[[ -d "$BURRATA_ISOLATED_MUSIC_ROOT" && ! -L "$BURRATA_ISOLATED_MUSIC_ROOT" ]]
mv "$profile" "${profile}_-case"
profile="${profile}_-case"
configure_isolated_music_root "$profile"
[[ "$BURRATA_ISOLATED_MUSIC_ROOT" == "$profile/fixture/Music" ]]
touch "$profile/.burrata-isolation-unqualified"
if (configure_isolated_music_root "$profile" 2>/dev/null); then exit 1; fi
rm "$profile/.burrata-isolation-unqualified"

# Exact marker contents, not merely the presence of a familiar filename.
printf '\n' >> "$profile/.burrata-isolated-profile"
if (configure_isolated_music_root "$profile" 2>/dev/null); then exit 1; fi
printf 'burrata isolated memory run\n' > "$profile/.burrata-isolated-profile"

# Reject both a Music symlink and a symlink at its fixture ancestor.
mkdir "$profile/outside"
rmdir "$profile/fixture/Music"
ln -s "$profile/outside" "$profile/fixture/Music"
if (configure_isolated_music_root "$profile" 2>/dev/null); then exit 1; fi
rm "$profile/fixture/Music"
rmdir "$profile/fixture"
ln -s "$profile/outside" "$profile/fixture"
if (configure_isolated_music_root "$profile" 2>/dev/null); then exit 1; fi
rm "$profile/fixture"

# A marker symlink cannot authorize a profile.
mv "$profile/.burrata-isolated-profile" "$profile/marker-backup"
ln -s "$profile/marker-backup" "$profile/.burrata-isolated-profile"
if (configure_isolated_music_root "$profile" 2>/dev/null); then exit 1; fi
printf 'isolated Music root tests passed\n'
