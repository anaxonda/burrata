#!/usr/bin/env python3
"""Fetch pinned platform tools and verify each against its upstream SHA-256 manifest."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
TARGET = os.environ.get("TARGET", "")
if TARGET not in LOCK["node"]["archives"] or TARGET not in LOCK["ytDlp"]["assets"]:
    raise SystemExit(f"No locked Node/yt-dlp artifacts for target {TARGET!r}")

DEST = ROOT / "src-tauri/resources/tools" / TARGET
shutil.rmtree(DEST, ignore_errors=True)
DEST.mkdir(parents=True)


def fetch(url: str, path: pathlib.Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "Burrata-release-tool-preparer"})
    with urllib.request.urlopen(request, timeout=120) as response, path.open("wb") as output:
        shutil.copyfileobj(response, output)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verified_download(base: str, name: str, checksum_file: str, tmp: pathlib.Path, pinned_sha256: str | None = None) -> pathlib.Path:
    archive = tmp / name
    sums = tmp / checksum_file
    fetch(base + name, archive)
    fetch(base + checksum_file, sums)
    expected = None
    for line in sums.read_text(encoding="utf-8", errors="replace").splitlines():
        fields = line.strip().split()
        if len(fields) >= 2 and fields[-1].lstrip("*") == name:
            expected = fields[0].lower()
            break
    if expected is None or len(expected) != 64 or any(ch not in "0123456789abcdef" for ch in expected):
        raise RuntimeError(f"No valid published SHA-256 for {name} in {checksum_file}")
    if pinned_sha256 and expected != pinned_sha256:
        raise RuntimeError(f"Published checksum for {name} differs from the locked SHA-256")
    actual = sha256(archive)
    if actual != expected:
        raise RuntimeError(f"SHA-256 mismatch for {name}: expected {expected}, got {actual}")
    return archive


def copy_executable(path: pathlib.Path, name: str) -> None:
    dest = DEST / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, dest)
    if os.name != "nt":
        dest.chmod(0o755)


def extract_archive(archive: pathlib.Path, tmp: pathlib.Path, wanted: tuple[str, ...]) -> None:
    root = tmp / "unpacked"
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir()
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as zf:
            for member in zf.infolist():
                if pathlib.PurePosixPath(member.filename).name in wanted:
                    target = root / pathlib.PurePosixPath(member.filename).name
                    target.write_bytes(zf.read(member))
    else:
        with tarfile.open(archive) as tf:
            for member in tf.getmembers():
                if pathlib.PurePosixPath(member.name).name in wanted and member.isfile():
                    source = tf.extractfile(member)
                    if source:
                        target = root / pathlib.PurePosixPath(member.name).name
                        target.write_bytes(source.read())
    for name in wanted:
        matches = list(root.rglob(name))
        if not matches:
            raise RuntimeError(f"Archive {archive.name} did not contain required {name}")
        copy_executable(matches[0], name)


with tempfile.TemporaryDirectory(prefix="burrata-tools-") as temp_dir:
    tmp = pathlib.Path(temp_dir)
    node_archive_name = LOCK["node"]["archives"][TARGET]
    node_archive = verified_download(LOCK["node"]["upstream"], node_archive_name,
                                     LOCK["node"]["checksumFile"], tmp)
    extract_archive(node_archive, tmp, ("node.exe" if os.name == "nt" else "node", "LICENSE"))
    node_bin = DEST / ("node.exe" if os.name == "nt" else "node")
    actual_node = subprocess.check_output([str(node_bin), "--version"], text=True).strip().lstrip("v")
    if actual_node != LOCK["node"]["version"]:
        raise RuntimeError(f"Bundled Node version mismatch: expected {LOCK['node']['version']}, got {actual_node}")

    ytdlp_name = LOCK["ytDlp"]["assets"][TARGET]
    ytdlp = verified_download(LOCK["ytDlp"]["upstream"], ytdlp_name,
                              LOCK["ytDlp"]["checksumFile"], tmp, LOCK["ytDlp"]["sha256"][TARGET])
    copy_executable(ytdlp, "yt-dlp.exe" if os.name == "nt" else "yt-dlp")
    if ytdlp_name == "yt-dlp.exe":
        (DEST / "yt-dlp.exe").write_bytes(ytdlp.read_bytes())
    ytdlp_bin = DEST / ("yt-dlp.exe" if os.name == "nt" else "yt-dlp")
    actual_ytdlp = subprocess.check_output([str(ytdlp_bin), "--version"], text=True).strip()
    if actual_ytdlp != LOCK["ytDlp"]["version"]:
        raise RuntimeError(f"Bundled yt-dlp version mismatch: expected {LOCK['ytDlp']['version']}, got {actual_ytdlp}")
    fetch(LOCK["ytDlp"]["noticesUrl"], DEST / "YT_DLP_THIRD_PARTY_LICENSES.txt")
    fetch(LOCK["ytDlp"]["licenseUrl"], DEST / "YT_DLP_LICENSE.txt")

    if TARGET in LOCK["ffmpeg"]["assets"]:
        ffmpeg_name = LOCK["ffmpeg"]["assets"][TARGET]
        ffmpeg = verified_download(LOCK["ffmpeg"]["upstream"], ffmpeg_name,
                                   LOCK["ffmpeg"]["checksumFile"], tmp)
        wanted = ("ffmpeg.exe", "ffprobe.exe", "LICENSE.txt") if os.name == "nt" else ("ffmpeg", "ffprobe", "LICENSE.txt")
        extract_archive(ffmpeg, tmp, wanted)
        shutil.move(DEST / "LICENSE.txt", DEST / "FFMPEG_UPSTREAM_LICENSE.txt")
        ffmpeg_sha = sha256(ffmpeg)
        source_commit = LOCK["ffmpeg"]["sourceCommits"][TARGET]
        sources = DEST / "sources"
        sources.mkdir(exist_ok=True)
        source_archive = sources / f"ffmpeg-{source_commit}.tar.gz"
        source_url = f"https://github.com/FFmpeg/FFmpeg/archive/{source_commit}.tar.gz"
        fetch(source_url, source_archive)
        (sources / "FFMPEG_SOURCE_COMMIT.txt").write_text(f"Repository: https://github.com/FFmpeg/FFmpeg\nPinned commit prefix: {source_commit}\nArchive URL: {source_url}\n", encoding="utf-8")
        ffmpeg_source_record = {"commit": source_commit, "archive": source_archive.name, "sha256": sha256(source_archive), "url": source_url}
        recipe = LOCK["ffmpeg"]["btbnBuildRecipe"]
        recipe_archive = sources / f"BtbN-FFmpeg-Builds-{recipe['commit']}.tar.gz"
        fetch(recipe["sourceArchive"], recipe_archive)
        recipe_digest = sha256(recipe_archive)
        if recipe_digest != recipe["sha256"]:
            raise RuntimeError(f"Pinned BtbN build-recipe source SHA-256 mismatch: expected {recipe['sha256']}, got {recipe_digest}")
        recipe_record = {
            "repository": recipe["repository"], "releaseTag": recipe["releaseTag"],
            "commit": recipe["commit"], "archive": recipe_archive.name,
            "sha256": recipe_digest, "url": recipe["sourceArchive"],
            "license": recipe["license"], "variants": recipe["variants"],
        }
    elif TARGET == "aarch64-apple-darwin":
        ffmpeg_name = "ffmpeg-8.1.3.tar.xz (source-built)"
        subprocess.run([sys.executable, str(ROOT / "scripts/release/build-macos-media-tools.py"), str(DEST)], check=True)
        ffmpeg_sha = LOCK["ffmpeg"]["macOSSourceBuild"]["sha256"]
    else:
        raise SystemExit(f"No pinned FFmpeg build is defined for target {TARGET!r}")
    ffmpeg_bin = DEST / ("ffmpeg.exe" if os.name == "nt" else "ffmpeg")
    actual_ffmpeg = subprocess.check_output([str(ffmpeg_bin), "-version"], text=True).splitlines()[0]
    if LOCK["ffmpeg"]["version"] not in actual_ffmpeg:
        raise RuntimeError(f"Bundled FFmpeg version mismatch: expected {LOCK['ffmpeg']['version']}, got {actual_ffmpeg}")
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(ffmpeg_bin), "-L"]))
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(ffmpeg_bin), "-version"]))

    readme = DEST / "LOCKED_TOOLS.json"
    tool_record = {
        "target": TARGET,
        "versions": {"node": LOCK["node"]["version"], "yt-dlp": LOCK["ytDlp"]["version"], "ffmpeg-build": LOCK["ffmpeg"]["version"]},
        "sha256": {node_archive_name: sha256(node_archive), ytdlp_name: sha256(ytdlp), ffmpeg_name: ffmpeg_sha},
        "sources": {"yt-dlp": {"commit": LOCK["ytDlp"]["sourceCommit"], "url": LOCK["ytDlp"]["licenseUrl"]}},
        "license": {"node": LOCK["node"]["license"], "yt-dlp": LOCK["ytDlp"]["license"], "ffmpeg": LOCK["ffmpeg"]["license"]}
    }
    if TARGET in LOCK["ffmpeg"]["assets"]:
        tool_record["ffmpegSource"] = ffmpeg_source_record
        tool_record["ffmpegBuildRecipe"] = recipe_record
    if TARGET == "aarch64-apple-darwin":
        tool_record["fpcalc"] = {"version": LOCK["fpcalc"]["version"], "sha256": LOCK["fpcalc"]["sha256"], "license": LOCK["fpcalc"]["license"]}
    readme.write_text(json.dumps(tool_record, indent=2) + "\n", encoding="utf-8")
print(f"Prepared verified tool resources at {DEST}")
