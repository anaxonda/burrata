#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?missing release tag}"
: "${RELEASE_COMMIT:?missing source commit}"
: "${TARGET:?missing Rust target}"
: "${LABEL:?missing artifact label}"

python3 scripts/release/verify-source-release.py

bundle_dir="src-tauri/target/$TARGET/release/bundle"
out="release-artifacts"
rm -rf "$out"
mkdir -p "$out/installers"
test -s .cache/release-smoke-evidence.json || { echo "Installed-artifact smoke evidence is missing" >&2; exit 1; }
cp .cache/release-smoke-evidence.json "$out/smoke-evidence.json"
find "$bundle_dir" -type f \( -name '*.dmg' -o -name '*.exe' -o -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -exec cp -v {} "$out/installers/" \;
test -n "$(find "$out/installers" -type f -print -quit)" || { echo "No installer produced for $TARGET" >&2; exit 1; }
python3 scripts/release/write-inventory.py "$out/artifact-inventory.json"
cp src-tauri/vendor/SOURCES.md "$out/THIRD_PARTY_SOURCES.md"
cp scripts/release/THIRD_PARTY_NOTICES.md "$out/THIRD_PARTY_NOTICES.md"
cp scripts/release/locked-tools.json "$out/locked-tools.json"
cp "src-tauri/resources/tools/$TARGET/LOCKED_TOOLS.json" "$out/LOCKED_TOOLS-$LABEL.json"
if [[ -f "src-tauri/resources/tools/$TARGET/LOCKED_FPCALC.json" ]]; then
  cp "src-tauri/resources/tools/$TARGET/LOCKED_FPCALC.json" "$out/LOCKED_FPCALC-$LABEL.json"
fi
python3 - "$TARGET" "$LABEL" "$out" <<'PY'
import json, pathlib, sys
target, label, out = sys.argv[1:]
out = pathlib.Path(out)
tools = json.loads((out / f"LOCKED_TOOLS-{label}.json").read_text(encoding="utf-8"))
lines = ["", "## Release-bundled tools", "", "| Tool | Version / source | SHA-256 | License |", "| --- | --- | --- | --- |"]
for name, record in tools.get("sha256", {}).items():
    lines.append(f"| {name} | Pinned archive | {record} | See `THIRD_PARTY_NOTICES.md` and bundled upstream notices |")
source = tools.get("ffmpegSource")
if source:
    lines.append(f"| FFmpeg source archive | {source['commit']} ({source['url']}) | {source['sha256']} | GPL-2.0-or-later source; GPL binary build |")
recipe = tools.get("ffmpegBuildRecipe")
if recipe:
    lines.append(f"| BtbN FFmpeg-Builds recipe source archive | {recipe['commit']} ({recipe['url']}) | {recipe['sha256']} | MIT; contains the exact release build scripts and dependency source pins |")
    bundle = recipe.get("sourceBundle")
    if bundle:
        lines.append(f"| BtbN FFmpeg corresponding-source manifest | {bundle['releaseTag']} ({bundle['manifest']}) | {bundle['manifestSha256']} | Full source archives are in the separate [runtime source release](https://github.com/anaxonda/burrata/releases/tag/{bundle['releaseTag']}); the installer carries this compact manifest, not the large archives |")
fpcalc_record = out / f"LOCKED_FPCALC-{label}.json"
if fpcalc_record.is_file():
    fpcalc = json.loads(fpcalc_record.read_text(encoding="utf-8"))
    source = fpcalc["source"]
    lines.append(f"| Chromaprint {fpcalc['version']} source archive | {source['url']} ({source['commit']}) | {source['sha256']} | LGPL-2.1-or-later |")
    for license_name in fpcalc.get("licenseTexts", []):
        license_file = pathlib.Path("src-tauri/resources/tools") / target / license_name
        if not license_file.is_file():
            raise SystemExit(f"Missing fpcalc license text: {license_file}")
media_file = pathlib.Path("src-tauri/resources/tools") / target / "MACOS_MEDIA_SOURCE_INVENTORY.json"
if media_file.is_file():
    (out / "MACOS_MEDIA_SOURCE_INVENTORY.json").write_bytes(media_file.read_bytes())
    media = json.loads(media_file.read_text(encoding="utf-8"))
    for item in media["sources"]:
        lines.append(f"| {item['name']} {item['version']} source archive | {item['url']} | {item['sha256']} | Refer to bundled upstream license |")
lines.extend(["", "BgUtils POT Provider 2.0.2 source commit: `26475e9d3665b972c4aeb83b4dbaafcf64b88438`; its source and lock files are included in the application resources.", ""])
with (out / "THIRD_PARTY_SOURCES.md").open("a", encoding="utf-8", newline="\n") as stream:
    stream.write("\n".join(lines))
PY
if [[ -d "src-tauri/resources/tools/$TARGET/sources" ]]; then
  cp -v "src-tauri/resources/tools/$TARGET/sources/"* "$out/"
fi
cp ".cache/release-assets/burrata-yt-dlp-$TARGET"* "$out/"
cp ".cache/release-assets/bgutil-plugin-2.0.2-$TARGET.zip" "$out/"
cp ".cache/release-assets/youtube-support-target.json" "$out/"
(cd "$out" && {
  find installers -type f -print0 | sort -z | xargs -0 sha256sum
  sha256sum smoke-evidence.json
} > SHA256SUMS.txt)
