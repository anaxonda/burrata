#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?missing release tag}"
: "${RELEASE_COMMIT:?missing source commit}"
: "${TARGET:?missing Rust target}"
: "${LABEL:?missing artifact label}"

python3 scripts/release/verify-media-tool-build.py "$TARGET"

bundle_dir="src-tauri/target/$TARGET/release/bundle"
out="release-artifacts"
rm -rf "$out"
mkdir -p "$out/installers"
test -s .cache/release-smoke-evidence.json || { echo "Installed-artifact smoke evidence is missing" >&2; exit 1; }
cp .cache/release-smoke-evidence.json "$out/smoke-evidence.json"
if [[ "$TARGET" = x86_64-unknown-linux-gnu ]]; then
  test -s .cache/appimage-smoke-evidence.json || { echo "AppImage smoke evidence is missing" >&2; exit 1; }
  cp .cache/appimage-smoke-evidence.json "$out/appimage-smoke-evidence.json"
fi
find "$bundle_dir" -type f \( -name '*.dmg' -o -name '*.exe' -o -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -exec cp -v {} "$out/installers/" \;
test -n "$(find "$out/installers" -type f -print -quit)" || { echo "No installer produced for $TARGET" >&2; exit 1; }
python3 scripts/release/write-inventory.py "$out/artifact-inventory.json"
cp src-tauri/vendor/SOURCES.md "$out/THIRD_PARTY_SOURCES.md"
cp scripts/release/THIRD_PARTY_NOTICES.md "$out/THIRD_PARTY_NOTICES.md"
python3 - "$out" <<'PY'
import gzip, hashlib, io, pathlib, tarfile, sys

out = pathlib.Path(sys.argv[1])
source = pathlib.Path("src-tauri/vendor/stream-download")
prefix = "stream-download-0.24.4-burrata-patched"
required = ["Cargo.toml", "Cargo.toml.orig", "README.md", "LICENSE-MIT", "LICENSE-APACHE", "PATCHES.md"]
files = [source / name for name in required] + sorted((source / "src").rglob("*.rs"))
if not (source / "src/source/mod.rs").is_file() or any(not path.is_file() or path.is_symlink() for path in files):
    raise SystemExit("Patched stream-download sources, licenses or patch record are missing")
for original, destination in [
    ("LICENSE-MIT", "STREAM_DOWNLOAD_LICENSE_MIT.txt"),
    ("LICENSE-APACHE", "STREAM_DOWNLOAD_LICENSE_APACHE.txt"),
    ("PATCHES.md", "STREAM_DOWNLOAD_PATCHES.md"),
]:
    (out / destination).write_bytes((source / original).read_bytes())
archive = out / f"{prefix}.tar.gz"
# Fixed metadata excludes timestamps and host ownership from the source archive.
with archive.open("wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
            for path in sorted(files):
                payload = path.read_bytes()
                member = tarfile.TarInfo(f"{prefix}/{path.relative_to(source).as_posix()}")
                member.size = len(payload)
                member.mode = 0o644
                bundle.addfile(member, io.BytesIO(payload))
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
with (out / "THIRD_PARTY_SOURCES.md").open("a", encoding="utf-8", newline="\n") as record:
    record.write("\n## Patched native streaming dependency\n\n")
    record.write("| Package | Matching source archive | SHA-256 | License |\n| --- | --- | --- | --- |\n")
    record.write(f"| stream-download 0.24.4, Burrata patch | `{archive.name}` | `{digest}` | MIT OR Apache-2.0; both license texts accompany this archive |\n")
PY
cp scripts/release/locked-tools.json "$out/locked-tools.json"
cp "src-tauri/resources/tools/$TARGET/LOCKED_TOOLS.json" "$out/LOCKED_TOOLS-$LABEL.json"
cp "src-tauri/resources/tools/$TARGET/FFMPEG_BUILD_CONFIGURATION.json" "$out/FFMPEG_BUILD_CONFIGURATION.json"
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
    if isinstance(record, dict):
        for filename, digest in record.items():
            lines.append(f"| build recipe `{filename}` | Source for this target | {digest} | See repository license |")
    else:
        lines.append(f"| {name} | Pinned archive | {record} | See `THIRD_PARTY_NOTICES.md` and bundled upstream notices |")
fpcalc_record = out / f"LOCKED_FPCALC-{label}.json"
if fpcalc_record.is_file():
    fpcalc = json.loads(fpcalc_record.read_text(encoding="utf-8"))
    source = fpcalc["source"]
    lines.append(f"| Chromaprint {fpcalc['version']} source archive | {source['url']} ({source['commit']}) | {source['sha256']} | LGPL-2.1-or-later |")
    for license_name in fpcalc.get("licenseTexts", []):
        license_file = pathlib.Path("src-tauri/resources/tools") / target / license_name
        if not license_file.is_file():
            raise SystemExit(f"Missing fpcalc license text: {license_file}")
media_file = pathlib.Path("src-tauri/resources/tools") / target / "MEDIA_SOURCE_INVENTORY.json"
if media_file.is_file():
    (out / "MEDIA_SOURCE_INVENTORY.json").write_bytes(media_file.read_bytes())
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
cp -R "src-tauri/resources/tools/$TARGET/build-recipe" "$out/build-recipe"
for notice in FFMPEG_LICENSE.txt FFMPEG_BUILD_INFO.txt LAME_LICENSE.txt OPUS_LICENSE.txt ZLIB_LICENSE.txt OPENSSL_LICENSE.txt CHROMAPRINT_LICENSE.md LGPL-2.1.txt; do
  if [[ -f "src-tauri/resources/tools/$TARGET/$notice" ]]; then
    cp "src-tauri/resources/tools/$TARGET/$notice" "$out/"
  fi
done
cp ".cache/release-assets/burrata-yt-dlp-$TARGET"* "$out/"
cp ".cache/release-assets/bgutil-plugin-2.0.2-$TARGET.zip" "$out/"
cp ".cache/release-assets/youtube-support-target.json" "$out/"
(cd "$out" && {
  find installers -type f -print0 | sort -z | xargs -0 sha256sum
  sha256sum smoke-evidence.json
} > SHA256SUMS.txt)
