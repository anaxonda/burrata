#!/usr/bin/env bash
set -e

# linuxdeploy patches each bundled ELF's RUNPATH. A global LD_LIBRARY_PATH
# would also override libraries used by host shells, browsers and openers.
if [[ -z "${APPDIR:-}" || ! -x "$APPDIR/usr/bin/burrata" ]]; then
  APPDIR="$(dirname -- "$(readlink -f -- "$0")")"
fi
export APPDIR

source "$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
source "$APPDIR/apprun-hooks/linuxdeploy-plugin-gstreamer.sh"
cd -- "$APPDIR/usr"
exec "$APPDIR/usr/bin/burrata" "$@"
