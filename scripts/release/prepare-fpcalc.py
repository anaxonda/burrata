#!/usr/bin/env python3
"""Stage a pinned official Chromaprint fpcalc binary, source, and LGPL notices."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch(url: str, path: pathlib.Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "Burrata-release-tool-preparer"})
    with urllib.request.urlopen(request, timeout=180) as response, path.open("wb") as output:
        shutil.copyfileobj(response, output)


def verified_download(url: str, path: pathlib.Path, expected: str) -> None:
    fetch(url, path)
    actual = sha256(path)
    if actual != expected:
        raise RuntimeError(f"SHA-256 mismatch for {path.name}: expected {expected}, got {actual}")


def member_bytes(archive: pathlib.Path, filename: str) -> bytes:
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as zf:
            matches = [name for name in zf.namelist() if pathlib.PurePosixPath(name).name == filename]
            if len(matches) != 1:
                raise RuntimeError(f"Expected one {filename} in {archive.name}, found {len(matches)}")
            return zf.read(matches[0])
    with tarfile.open(archive, "r:*") as tf:
        matches = [member for member in tf.getmembers() if member.isfile() and pathlib.PurePosixPath(member.name).name == filename]
        if len(matches) != 1:
            raise RuntimeError(f"Expected one {filename} in {archive.name}, found {len(matches)}")
        stream = tf.extractfile(matches[0])
        if stream is None:
            raise RuntimeError(f"Could not read {filename} from {archive.name}")
        return stream.read()


def prepare(target: str, destination: pathlib.Path) -> None:
    fpcalc_lock = LOCK["fpcalc"]
    assets = fpcalc_lock["binaryAssets"]
    if target not in assets:
        raise SystemExit(f"No official pinned fpcalc binary for target {target!r}")
    if target == "x86_64-unknown-linux-gnu" and (os.name == "nt" or sys.platform != "linux"):
        raise SystemExit("Linux fpcalc must be staged on a Linux x86_64 runner")
    if target == "x86_64-pc-windows-msvc" and os.name != "nt":
        raise SystemExit("Windows fpcalc must be staged on a Windows x64 runner")

    asset = assets[target]
    expected_binary_name = asset["binary"]
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="burrata-fpcalc-") as tmp_dir:
        tmp = pathlib.Path(tmp_dir)
        binary_archive = tmp / asset["asset"]
        source_record = fpcalc_lock["sourceArchive"]
        source_archive = tmp / "chromaprint-1.5.1.tar.gz"
        license_record = fpcalc_lock["licenseText"]
        license_text = tmp / license_record["file"]
        verified_download(asset["url"], binary_archive, asset["sha256"])
        verified_download(source_record["url"], source_archive, source_record["sha256"])
        verified_download(license_record["url"], license_text, license_record["sha256"])

        binary = tmp / expected_binary_name
        binary.write_bytes(member_bytes(binary_archive, expected_binary_name))
        if os.name != "nt":
            binary.chmod(0o755)
        binary_sha = sha256(binary)
        if binary_sha != asset["binarySha256"]:
            raise RuntimeError(f"Extracted fpcalc checksum mismatch: expected {asset['binarySha256']}, got {binary_sha}")
        version_output = subprocess.check_output([str(binary), "-version"], text=True, stderr=subprocess.STDOUT).strip()
        version_match = re.search(r"fpcalc version\s+([0-9]+(?:\.[0-9]+)+)", version_output)
        if not version_match or version_match.group(1) != fpcalc_lock["version"]:
            raise RuntimeError(f"Expected fpcalc {fpcalc_lock['version']}, got {version_output!r}")

        # Retain the upstream source archive as corresponding source and keep both
        # the project's MIT/LGPL notice and the complete LGPL text beside it.
        chromaprint_license = member_bytes(source_archive, "LICENSE.md")
        staged_binary = destination / expected_binary_name
        staged_binary.write_bytes(binary.read_bytes())
        if os.name != "nt":
            staged_binary.chmod(0o755)
        sources = destination / "sources"
        sources.mkdir(exist_ok=True)
        shutil.copy2(source_archive, sources / "chromaprint-1.5.1.tar.gz")
        (destination / "CHROMAPRINT_LICENSE.md").write_bytes(chromaprint_license)
        shutil.copy2(license_text, destination / license_record["file"])
        record = {
            "target": target,
            "version": fpcalc_lock["version"],
            "source": {
                "url": source_record["url"],
                "commit": fpcalc_lock["sourceCommit"],
                "archive": "sources/chromaprint-1.5.1.tar.gz",
                "sha256": source_record["sha256"],
            },
            "binary": {
                "asset": asset["asset"],
                "url": asset["url"],
                "archiveSha256": asset["sha256"],
                "file": expected_binary_name,
                "sha256": binary_sha,
                "license": fpcalc_lock["license"],
            },
            "licenseTexts": ["CHROMAPRINT_LICENSE.md", license_record["file"]],
        }
        (destination / "LOCKED_FPCALC.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default=os.environ.get("TARGET"), help="Rust target triple (defaults to TARGET)")
    parser.add_argument("--destination", type=pathlib.Path, help="staging directory (defaults to Burrata's tools resource directory)")
    args = parser.parse_args()
    if not args.target:
        parser.error("set TARGET or pass --target")
    destination = args.destination or ROOT / "src-tauri/resources/tools" / args.target
    prepare(args.target, destination.resolve())
    print(f"Prepared verified fpcalc {LOCK['fpcalc']['version']} at {destination.resolve()}")


if __name__ == "__main__":
    main()
