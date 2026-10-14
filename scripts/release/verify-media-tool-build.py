#!/usr/bin/env python3
"""Verify staged source-built FFmpeg binaries, sources, lock profile and notices."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
TARGET = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("TARGET", "")
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
DEST = ROOT / "src-tauri/resources/tools" / TARGET


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


if TARGET not in LOCK["ffmpeg"]["tlsBackends"]:
    raise SystemExit(f"No media profile for target {TARGET!r}")
record = json.loads((DEST / "LOCKED_TOOLS.json").read_text(encoding="utf-8"))
for name, executable in (("node", "node.exe" if TARGET == "x86_64-pc-windows-msvc" else "node"),
                         ("ytDlp", "yt-dlp.exe" if TARGET == "x86_64-pc-windows-msvc" else "yt-dlp")):
    binary = DEST / executable
    if not binary.is_file():
        raise SystemExit(f"Missing locked {name} executable: {binary}")
    expected = record.get("sha256", {}).get("nodeExecutable" if name == "node" else "ytDlpExecutable")
    if not expected or sha256(binary) != expected:
        raise SystemExit(f"{name} executable checksum differs from LOCKED_TOOLS.json")
    expected_version = LOCK["node"]["version"] if name == "node" else LOCK["ytDlp"]["version"]
    actual_version = subprocess.check_output([str(binary), "--version"], text=True).strip().lstrip("v")
    if actual_version != expected_version:
        raise SystemExit(f"Unexpected {name} version: {actual_version}")
media = json.loads((DEST / "MEDIA_SOURCE_INVENTORY.json").read_text(encoding="utf-8"))
configuration = json.loads((DEST / "FFMPEG_BUILD_CONFIGURATION.json").read_text(encoding="utf-8"))
if media.get("target", TARGET) != TARGET or configuration.get("target") != TARGET:
    raise SystemExit("Media build target does not match the release target")
if configuration.get("version") != LOCK["ffmpeg"]["version"]:
    raise SystemExit("FFmpeg build version differs from the lock")
if configuration.get("tls") != LOCK["ffmpeg"]["tlsBackends"][TARGET]:
    raise SystemExit("FFmpeg TLS backend differs from the locked platform policy")
configured = configuration.get("configure", [])
for flag in LOCK["ffmpeg"]["configureCommon"] + [LOCK["ffmpeg"]["tlsBackends"][TARGET]["flag"]]:
    if flag not in configured:
        raise SystemExit(f"Required FFmpeg configuration flag is missing: {flag}")

source_records = {item["name"]: item for item in media.get("sources", [])}
required = {
    "ffmpeg": LOCK["ffmpeg"]["source"],
    "lame": {"url": LOCK["lame"]["url"], "sha256": LOCK["lame"]["sha256"]},
    "opus": {"url": LOCK["opus"]["url"], "sha256": LOCK["opus"]["sha256"]},
}
if TARGET != "aarch64-apple-darwin":
    required["zlib"] = {"url": LOCK["zlib"]["url"], "sha256": LOCK["zlib"]["sha256"]}
if TARGET == "x86_64-unknown-linux-gnu":
    required["openssl"] = {"url": LOCK["openssl"]["url"], "sha256": LOCK["openssl"]["sha256"]}
if TARGET == "aarch64-apple-darwin":
    fpcalc = LOCK["fpcalc"]
    required["chromaprint"] = {"url": fpcalc["sourceArchive"]["url"], "sha256": fpcalc["sourceArchive"]["sha256"]}
for name, lock in required.items():
    item = source_records.get(name)
    if not item or item.get("url") != lock["url"] or item.get("sha256") != lock["sha256"]:
        raise SystemExit(f"Missing or mismatched locked source record: {name}")
    source_path = DEST / "sources" / item["file"]
    if not source_path.is_file() or sha256(source_path) != lock["sha256"]:
        raise SystemExit(f"Missing or mismatched corresponding source archive: {source_path}")

for binary_name in ("ffmpeg", "ffprobe"):
    suffix = ".exe" if TARGET == "x86_64-pc-windows-msvc" else ""
    binary = DEST / f"{binary_name}{suffix}"
    if not binary.is_file():
        raise SystemExit(f"Missing source-built media binary: {binary}")
    expected = record.get("sha256", {}).get(f"{binary_name}Executable")
    if not expected or sha256(binary) != expected:
        raise SystemExit(f"Media binary checksum differs from LOCKED_TOOLS.json: {binary_name}")
    version = subprocess.check_output([str(binary), "-version"], text=True).splitlines()[0]
    if LOCK["ffmpeg"]["version"] not in version:
        raise SystemExit(f"Unexpected {binary_name} version: {version}")
required_notices = ["FFMPEG_LICENSE.txt", "FFMPEG_BUILD_INFO.txt", "LAME_LICENSE.txt", "OPUS_LICENSE.txt"]
if TARGET != "aarch64-apple-darwin":
    required_notices.append("ZLIB_LICENSE.txt")
for filename in required_notices:
    if not (DEST / filename).is_file():
        raise SystemExit(f"Missing bundled source/build notice: {filename}")
if TARGET == "x86_64-unknown-linux-gnu" and not (DEST / "OPENSSL_LICENSE.txt").is_file():
    raise SystemExit("Missing OpenSSL license")
if sha256(DEST / "MEDIA_SOURCE_INVENTORY.json") != record["sha256"]["mediaSourceInventory"]:
    raise SystemExit("Media source inventory checksum mismatch")
if sha256(DEST / "FFMPEG_BUILD_CONFIGURATION.json") != record["sha256"]["ffmpegBuildConfiguration"]:
    raise SystemExit("FFmpeg build configuration checksum mismatch")
recipe_files = record["sha256"].get("buildRecipe", {})
if not recipe_files:
    raise SystemExit("Missing hashes for the corresponding-source build recipe")
for filename, expected in recipe_files.items():
    recipe_file = DEST / "build-recipe" / filename
    if not recipe_file.is_file() or sha256(recipe_file) != expected:
        raise SystemExit(f"Missing or mismatched build recipe file: {filename}")
print(f"Verified source-built FFmpeg/ffprobe, locked sources, notices, and TLS policy for {TARGET}")
