#!/usr/bin/env python3
"""Package the target yt-dlp updater asset and matching BgUtils plugin ZIP."""
import hashlib
import json
import os
import pathlib
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
TARGET = os.environ.get("TARGET", "")
version = "2.0.2"
lock = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
plugin = ROOT / "src-tauri/resources/youtube-support/bgutil/plugin/yt_dlp_plugins"
if not plugin.is_dir() or not any(plugin.rglob("*.py")):
    raise SystemExit(f"Required pinned BgUtils plugin sources are missing: {plugin}")
output = ROOT / ".cache/release-assets"
output.mkdir(parents=True, exist_ok=True)
if TARGET not in lock["ytDlp"]["assets"]:
    raise SystemExit(f"No locked yt-dlp update binary for target {TARGET!r}")
# The verified upstream archive name is retained in lock metadata, but the
# staged resource is normalized by prepare-tools.py to the executable name.
binary_name = "yt-dlp.exe" if TARGET == "x86_64-pc-windows-msvc" else "yt-dlp"
binary_source = ROOT / "src-tauri/resources/tools" / TARGET / binary_name
if not binary_source.is_file():
    raise SystemExit(f"Required yt-dlp updater binary is missing: {binary_source}")
asset = f"burrata-yt-dlp-{TARGET}{'.exe' if TARGET == 'x86_64-pc-windows-msvc' else ''}"
binary_archive = output / asset
binary_archive.write_bytes(binary_source.read_bytes())
binary_data = binary_archive.read_bytes()
plugin_asset = f"bgutil-plugin-{version}-{TARGET}.zip"
archive = output / plugin_asset
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    for path in sorted(plugin.rglob("*")):
        if path.is_file():
            zf.write(path, pathlib.PurePosixPath("yt_dlp_plugins") / path.relative_to(plugin))
data = archive.read_bytes()
record = {
    "version": lock["ytDlp"]["version"],
    "target": TARGET,
    "asset": asset,
    "sha256": hashlib.sha256(binary_data).hexdigest(),
    "sha512": hashlib.sha512(binary_data).hexdigest(),
    "pluginAsset": plugin_asset,
    "pluginSha256": hashlib.sha256(data).hexdigest(),
    "pluginSha512": hashlib.sha512(data).hexdigest(),
}
(output / "youtube-support-target.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
print(f"Packaged {asset} and {plugin_asset}")
