#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?missing release tag}"
: "${RELEASE_COMMIT:?missing source commit}"
: "${TARGET:?missing Rust target}"
: "${LABEL:?missing artifact label}"

bundle_dir="src-tauri/target/$TARGET/release/bundle"
out="release-artifacts"
rm -rf "$out"
mkdir -p "$out/installers"
find "$bundle_dir" -type f \( -name '*.dmg' -o -name '*.exe' -o -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -exec cp -v {} "$out/installers/" \;
test -n "$(find "$out/installers" -type f -print -quit)" || { echo "No installer produced for $TARGET" >&2; exit 1; }
python3 scripts/release/write-inventory.py "$out/artifact-inventory.json"
cp src-tauri/vendor/SOURCES.md "$out/THIRD_PARTY_SOURCES.md"
cp scripts/release/THIRD_PARTY_NOTICES.md "$out/THIRD_PARTY_NOTICES.md"
cp scripts/release/locked-tools.json "$out/locked-tools.json"
cp "src-tauri/resources/tools/$TARGET/LOCKED_TOOLS.json" "$out/LOCKED_TOOLS-$LABEL.json"
python3 - "$TARGET" "$LABEL" "$out" <<'PY'
import json, pathlib, sys
target, label, out = sys.argv[1:]
out = pathlib.Path(out)
tools = json.loads((out / f"LOCKED_TOOLS-{label}.json").read_text())
lines = ["", "## Release-bundled tools", "", "| Tool | Version / source | SHA-256 | License |", "| --- | --- | --- | --- |"]
for name, record in tools.get("sha256", {}).items():
    lines.append(f"| {name} | Pinned archive | {record} | See `THIRD_PARTY_NOTICES.md` and bundled upstream notices |")
source = tools.get("ffmpegSource")
if source:
    lines.append(f"| FFmpeg source archive | {source['commit']} ({source['url']}) | {source['sha256']} | GPL-2.0-or-later source; GPL binary build |")
media_file = pathlib.Path("src-tauri/resources/tools") / target / "MACOS_MEDIA_SOURCE_INVENTORY.json"
if media_file.is_file():
    (out / "MACOS_MEDIA_SOURCE_INVENTORY.json").write_bytes(media_file.read_bytes())
    media = json.loads(media_file.read_text())
    for item in media["sources"]:
        lines.append(f"| {item['name']} {item['version']} source archive | {item['url']} | {item['sha256']} | Refer to bundled upstream license |")
lines.extend(["", "BgUtils POT Provider 2.0.2 source commit: `26475e9d3665b972c4aeb83b4dbaafcf64b88438`; its source and lock files are included in the application resources.", ""])
with (out / "THIRD_PARTY_SOURCES.md").open("a") as stream:
    stream.write("\n".join(lines))
PY
cp ".cache/release-assets/burrata-yt-dlp-$TARGET"* "$out/"
cp ".cache/release-assets/bgutil-plugin-2.0.2-$TARGET.zip" "$out/"
cp ".cache/release-assets/youtube-support-target.json" "$out/"
(cd "$out" && find installers -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS.txt)
