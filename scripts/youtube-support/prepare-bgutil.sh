#!/usr/bin/env bash
set -euo pipefail

# Build the matching anonymous BgUtils HTTP provider and yt-dlp plugin from
# the upstream immutable 2.0.2 release commit. Requires git, Node >=22, npm.
readonly VERSION="2.0.2"
readonly COMMIT="26475e9d3665b972c4aeb83b4dbaafcf64b88438"
readonly REPO="https://github.com/Brainicism/bgutil-ytdlp-pot-provider.git"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly RESOURCES="${1:-$ROOT/src-tauri/resources}"
readonly TMP="$(mktemp -d)"
readonly NODE_BIN="${NODE_BIN:-node}"
trap 'rm -rf "$TMP"' EXIT

command -v git >/dev/null || { echo "git is required to prepare BgUtils" >&2; exit 1; }
command -v "$NODE_BIN" >/dev/null || { echo "Node.js 22 or newer is required to build BgUtils" >&2; exit 1; }
command -v npm >/dev/null || { echo "npm is required to build BgUtils" >&2; exit 1; }
NODE_PATH="$(command -v "$NODE_BIN")"
export PATH="$(dirname "$NODE_PATH"):$PATH"
NODE_MAJOR="$("$NODE_BIN" -p 'Number(process.versions.node.split(".")[0])')"
if (( NODE_MAJOR < 22 )); then echo "BgUtils 2.0.2 requires Node.js >=22 (found $("$NODE_BIN" --version))" >&2; exit 1; fi

git clone --quiet --depth 1 --single-branch --branch "$VERSION" "$REPO" "$TMP/source"
ACTUAL="$(git -C "$TMP/source" rev-parse HEAD)"
if [[ "$ACTUAL" != "$COMMIT" ]]; then
  echo "BgUtils tag $VERSION resolved to unexpected commit $ACTUAL (expected $COMMIT)" >&2
  exit 1
fi

SERVER="$TMP/source/server"
[[ -f "$SERVER/package-lock.json" && -f "$SERVER/src/main.ts" ]] || { echo "Pinned BgUtils server sources are incomplete" >&2; exit 1; }
[[ "$("$NODE_BIN" -p 'require(process.argv[1]).version' "$SERVER/package.json")" == "$VERSION" ]] || { echo "Pinned BgUtils server version mismatch" >&2; exit 1; }
(cd "$SERVER" && npm ci --no-audit --no-fund && npx --no-install tsc && npm prune --omit=dev --no-audit --no-fund)
[[ -f "$SERVER/build/main.js" ]] || { echo "BgUtils build did not produce server/build/main.js" >&2; exit 1; }
[[ -d "$SERVER/node_modules" ]] || { echo "BgUtils runtime dependencies were not installed" >&2; exit 1; }

# This plugin source ships in the same immutable upstream commit as the
# server. Keep the yt_dlp_plugins namespace intact for yt-dlp's plugin loader.
PLUGIN_SOURCE="$TMP/source/yt_dlp_plugins"
[[ -d "$PLUGIN_SOURCE" ]] || { echo "Pinned BgUtils commit has no yt_dlp_plugins directory" >&2; exit 1; }
PLUGIN_PY="$(find "$PLUGIN_SOURCE" -type f -name '*.py' -print -quit)"
if [[ -z "$PLUGIN_PY" ]]; then
  echo "Pinned BgUtils commit contains no Python yt-dlp plugin source" >&2
  exit 1
fi

STAGE="$TMP/resources/youtube-support/bgutil"
mkdir -p "$STAGE/server" "$STAGE/plugin"
cp -a "$SERVER/build" "$STAGE/server/build"
cp -a "$SERVER/node_modules" "$STAGE/server/node_modules"
cp "$SERVER/package.json" "$SERVER/package-lock.json" "$STAGE/server/"
cp -a "$PLUGIN_SOURCE" "$STAGE/plugin/yt_dlp_plugins"
[[ -f "$TMP/source/LICENSE" ]] || { echo "Pinned BgUtils source has no top-level LICENSE" >&2; exit 1; }
cp "$TMP/source/LICENSE" "$STAGE/LICENSE"
while IFS= read -r -d '' license; do
  relative="${license#"$PLUGIN_SOURCE"/}"
  mkdir -p "$STAGE/plugin/yt_dlp_plugins/$(dirname "$relative")"
  cp "$license" "$STAGE/plugin/yt_dlp_plugins/$relative"
done < <(find "$PLUGIN_SOURCE" -type f \( -iname 'LICENSE' -o -iname 'LICENSE.txt' -o -iname 'COPYING' \) -print0)
cat > "$STAGE/UPSTREAM.txt" <<EOF
BgUtils POT Provider $VERSION
Source: $REPO
Tag: $VERSION
Commit: $COMMIT
Server: server/build/main.js
Plugin: plugin/yt_dlp_plugins
License: LICENSE
EOF

# Replace only this script's managed resource subtree after all checks pass.
DEST="$RESOURCES/youtube-support/bgutil"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
mv "$STAGE" "$DEST"
echo "Prepared verified BgUtils provider $VERSION at $DEST"
