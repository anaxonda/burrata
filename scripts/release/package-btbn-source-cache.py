#!/usr/bin/env python3
"""Filter BtbN's exact successful-run download cache into source-only assets."""
import argparse
import gzip
import hashlib
import json
import lzma
import pathlib
import re
import shutil
import tarfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
RECIPE = LOCK["ffmpeg"]["btbnBuildRecipe"]
CACHE = RECIPE["sourceCache"]
TARGETS = json.loads((ROOT / CACHE["inputList"]).read_text(encoding="utf-8"))["variants"]
LICENSE_NAME = re.compile(r"(?i)(?:^|/)(?:licen[cs]e(?:s)?|copying|copyright|notice|patents?)(?:[._-].*)?$")
CHUNK = 1024 * 1024


def digest_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def scan_license_files(archive: zipfile.ZipFile, filename: str) -> list[str]:
    licenses = []
    with archive.open(filename) as raw, lzma.LZMAFile(raw) as xz, tarfile.open(fileobj=xz, mode="r|") as source:
        for member in source:
            if member.isfile() and LICENSE_NAME.search(member.name):
                licenses.append(member.name)
    return sorted(set(licenses))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cache_zip", type=pathlib.Path)
    parser.add_argument("output_dir", type=pathlib.Path)
    args = parser.parse_args()
    cache_zip = args.cache_zip.resolve()
    output = args.output_dir.resolve()
    if cache_zip.stat().st_size != CACHE["artifactBytes"]:
        raise SystemExit(f"Source cache byte-size mismatch: expected {CACHE['artifactBytes']}, got {cache_zip.stat().st_size}")
    cache_digest = digest_file(cache_zip)
    if cache_digest != CACHE["artifactSha256"]:
        raise SystemExit(f"Source cache SHA-256 mismatch: expected {CACHE['artifactSha256']}, got {cache_digest}")

    targets_by_entry: dict[str, list[str]] = {}
    for rust_target, item in TARGETS.items():
        for entry in item["cacheEntries"]:
            targets_by_entry.setdefault(entry, []).append(rust_target)
    expected = set(targets_by_entry)
    output.mkdir(parents=True, exist_ok=True)
    part_names = [
        f"btbn-ffmpeg-build-sources-{RECIPE['releaseTag']}-part1.zip",
        f"btbn-ffmpeg-build-sources-{RECIPE['releaseTag']}-part2.zip",
    ]
    parts = [zipfile.ZipFile(output / name, "w", compression=zipfile.ZIP_STORED, allowZip64=True) for name in part_names]
    part_sizes = [0, 0]
    source_records: dict[str, dict] = {}
    found: set[str] = set()
    try:
        with zipfile.ZipFile(cache_zip) as zipped:
            if CACHE["cacheMember"] not in zipped.namelist():
                raise SystemExit(f"Missing expected cache member {CACHE['cacheMember']}")
            with zipped.open(CACHE["cacheMember"]) as raw, gzip.GzipFile(fileobj=raw) as cache_stream:
                with tarfile.open(fileobj=cache_stream, mode="r|") as cache:
                    for member in cache:
                        name = pathlib.PurePosixPath(member.name).name
                        if name not in expected:
                            continue
                        if name in found or not member.isfile():
                            raise SystemExit(f"Duplicate or non-file source-cache member: {member.name}")
                        if part_sizes[0] + member.size > 1_150_000_000 and part_sizes[0]:
                            part_index = 1
                        else:
                            part_index = 0
                        target_file = cache.extractfile(member)
                        if target_file is None:
                            raise SystemExit(f"Could not read source-cache member {member.name}")
                        digest = hashlib.sha256()
                        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                        info.compress_type = zipfile.ZIP_STORED
                        with target_file, parts[part_index].open(info, "w", force_zip64=True) as zipped_out:
                            for chunk in iter(lambda: target_file.read(CHUNK), b""):
                                digest.update(chunk)
                                zipped_out.write(chunk)
                        part_sizes[part_index] += member.size
                        found.add(name)
                        source_records[name] = {
                            "bytes": member.size,
                            "sha256": digest.hexdigest(),
                            "targets": sorted(targets_by_entry[name]),
                            "licenseFiles": [],
                        }
    finally:
        for part in parts:
            part.close()

    if found != expected:
        raise SystemExit(f"Source-cache members missing: {sorted(expected - found)}")
    for part_name in part_names:
        part_path = output / part_name
        if part_path.stat().st_size >= 1_500_000_000:
            raise SystemExit(f"Source asset exceeds the 1.5 GB release-part budget: {part_path}")

    # These source archives contain the pristine upstream source trees. Index
    # their shipped license/notice paths without expanding them to disk.
    for part_name in part_names:
        with zipfile.ZipFile(output / part_name) as source_part:
            for name in source_part.namelist():
                source_records[name]["licenseFiles"] = scan_license_files(source_part, name)

    manifest = {
        "repository": RECIPE["repository"],
        "releaseTag": RECIPE["releaseTag"],
        "commit": RECIPE["commit"],
        "recipeSourceArchive": {
            "url": RECIPE["sourceArchive"],
            "sha256": RECIPE["sha256"],
            "license": RECIPE["license"],
        },
        "sourceCache": {
            "workflowRunId": CACHE["workflowRunId"],
            "artifactId": CACHE["artifactId"],
            "artifactSha256": cache_digest,
            "artifactBytes": cache_zip.stat().st_size,
            "variants": {
                target: {
                    "name": f"{item['target']} {item['variant']} {item['addin']}",
                    "sourceEntries": len(item["cacheEntries"]),
                }
                for target, item in TARGETS.items()
            },
            "uniqueSourceEntries": len(source_records),
            "parts": [
                {"file": name, "bytes": (output / name).stat().st_size, "sha256": digest_file(output / name)}
                for name in part_names
            ],
            "entries": source_records,
        },
    }
    (output / "btbn-ffmpeg-source-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    readme = f"""# BtbN FFmpeg-Builds corresponding source bundle

This source bundle contains the BtbN download-cache source archives used by the exact successful upstream build run for the `win64 gpl 8.1` and `linux64 gpl 8.1` variants.

- BtbN build recipe commit: `{RECIPE['commit']}` ({RECIPE['releaseTag']})
- Recipe source archive SHA-256: `{RECIPE['sha256']}` (MIT licensed; archive includes its LICENSE)
- Upstream build workflow run: `{CACHE['workflowRunId']}`
- Original source-cache artifact SHA-256: `{cache_digest}`
- Source archive parts and per-file SHA-256/license-path inventory: `btbn-ffmpeg-source-manifest.json`

The two ZIP parts contain all unique source-cache archives referenced by the generated upstream target Dockerfiles. Verify each part against the manifest, then extract both ZIPs into one directory. Restore the entries to `.cache/downloads/` in a checkout of the recipe source, then use its `generate.sh`/`build.sh` workflow for the matching targets and variants. `FFMPEG_BUILD_INFO.txt` in each Burrata tool bundle records the final FFmpeg configure flags and build toolchain.

Each source cache archive contains the corresponding upstream source tree. The manifest lists paths matching LICENSE, COPYING, COPYRIGHT, NOTICE, or PATENTS inside every source archive so notices can be located without ambiguity.
"""
    (output / "README-BtbN-FFmpeg-Sources.md").write_text(readme, encoding="utf-8")
    print(json.dumps({"parts": manifest["sourceCache"]["parts"], "sourceEntries": len(found)}, indent=2))


if __name__ == "__main__":
    main()
