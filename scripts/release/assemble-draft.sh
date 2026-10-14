#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?}"
: "${RELEASE_COMMIT:?}"
test -d incoming || { echo 'Build artifacts were not downloaded' >&2; exit 1; }
for label in macos-arm64 windows-x64 linux-x64; do
  found=0
  while IFS= read -r inventory; do
    if python3 - "$inventory" "$RELEASE_TAG" "$RELEASE_COMMIT" "$label" <<'PY'
import json, pathlib, sys
data=json.loads(pathlib.Path(sys.argv[1]).read_text())
ok=data.get('tag') == sys.argv[2] and data.get('commit') == sys.argv[3] and data.get('label') == sys.argv[4]
raise SystemExit(0 if ok else 1)
PY
    then found=1; fi
  done < <(find incoming -name artifact-inventory.json -type f)
  test "$found" = 1 || { echo "Required platform inventory missing or mismatched: $label" >&2; exit 1; }
done
test -n "$(find incoming -name '*.xpi' -type f -print -quit)" || { echo 'Signed Firefox XPI is missing; unsigned extensions are not releasable' >&2; exit 1; }
test -n "$(find incoming -name '*.zip' -type f -print -quit)" || { echo 'Chromium ZIP is missing' >&2; exit 1; }
python3 - "$RELEASE_TAG" incoming <<'PY'
import hashlib, json, pathlib, sys
tag, root = sys.argv[1:]
root = pathlib.Path(root)
expected = {"aarch64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}
records = {}
feed_version = None
for path in root.rglob("youtube-support-target.json"):
    item = json.loads(path.read_text())
    target = item.get("target")
    asset = item.get("asset")
    binary = path.parent / asset
    plugin_asset = item.get("pluginAsset")
    plugin = path.parent / plugin_asset if plugin_asset else None
    if target in records or target not in expected or not binary.is_file() or not plugin or not plugin.is_file():
        raise SystemExit(f"Invalid or duplicate YouTube support artifact record: {path}")
    data = binary.read_bytes()
    if hashlib.sha256(data).hexdigest() != item.get("sha256") or hashlib.sha512(data).hexdigest() != item.get("sha512"):
        raise SystemExit(f"yt-dlp update asset checksum mismatch: {binary}")
    plugin_data = plugin.read_bytes()
    if hashlib.sha256(plugin_data).hexdigest() != item.get("pluginSha256") or hashlib.sha512(plugin_data).hexdigest() != item.get("pluginSha512"):
        raise SystemExit(f"BgUtils plugin asset checksum mismatch: {plugin}")
    if feed_version is not None and item.get("version") != feed_version:
        raise SystemExit("yt-dlp update artifact versions do not match")
    feed_version = item.get("version")
    records[target] = {"asset": asset, "sha256": item["sha256"], "sha512": item["sha512"]}
if set(records) != expected:
    raise SystemExit(f"YouTube support assets missing for: {sorted(expected-set(records))}")
(root.parent / "youtube-support.json").write_text(json.dumps({"version": feed_version, "targets": records}, indent=2) + "\n")
PY
rm -rf release-artifacts
mkdir release-artifacts
while IFS= read -r -d '' file; do
  relative="${file#incoming/}"
  base="${relative##*/}"
  case "$base" in
    burrata-yt-dlp-*|bgutil-plugin-*) relative="$base" ;;
    *) relative="${relative//\//-}" ;;
  esac
  case "$relative" in SHA256SUMS.txt|*-SHA256SUMS.txt) continue ;; esac
  cp "$file" "release-artifacts/$relative"
done < <(find incoming -type f -print0)
cp youtube-support.json release-artifacts/youtube-support.json
rm youtube-support.json
(cd release-artifacts && find . -maxdepth 1 -type f ! -name SHA256SUMS.txt -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS.txt)
cat > release-artifacts/RELEASE_ACCEPTANCE.md <<EOF
# Burrata $RELEASE_TAG draft prerelease

Source commit: $RELEASE_COMMIT

This is a draft prerelease assembled from native macOS arm64, Windows x64, and Linux x64 jobs. Review signing and notarization, fresh-machine installation, dependency licensing, and acceptance evidence before publishing. No workflow promotes this draft.
EOF
