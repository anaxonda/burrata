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
PREPARED_CACHE = ROOT / ".cache/prepared-tool-resources" / TARGET
shutil.rmtree(DEST, ignore_errors=True)
DEST.mkdir(parents=True)

# Reuse a prepared runtime only after checking the locked media profile, source
# archive hashes, embedded build recipe, tool versions, and executable hashes.
if PREPARED_CACHE.is_dir():
    shutil.copytree(PREPARED_CACHE, DEST, dirs_exist_ok=True)
    verified = subprocess.run(
        [sys.executable, str(ROOT / "scripts/release/verify-media-tool-build.py"), TARGET],
        check=False,
    )
    if verified.returncode == 0:
        print(f"Restored verified prepared tool resources from {PREPARED_CACHE}")
        raise SystemExit(0)
    print("Prepared tool cache failed verification; rebuilding from locked sources", file=sys.stderr)
    shutil.rmtree(DEST)
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

    builders = {
        "aarch64-apple-darwin": "build-macos-media-tools.py",
        "x86_64-pc-windows-msvc": "build-windows-media-tools.py",
        "x86_64-unknown-linux-gnu": "build-linux-media-tools.py",
    }
    builder = builders.get(TARGET)
    if builder is None:
        raise SystemExit(f"No pinned FFmpeg build is defined for target {TARGET!r}")
    subprocess.run([sys.executable, str(ROOT / "scripts/release" / builder), str(DEST)], check=True)
    recipe_dir = DEST / "build-recipe"
    recipe_dir.mkdir()
    recipe_files = ["build-media-tools-common.py", builder, "test-media-tools.py", "locked-tools.json"]
    for filename in recipe_files:
        shutil.copy2(ROOT / "scripts/release" / filename, recipe_dir / filename)
    ffmpeg_bin = DEST / ("ffmpeg.exe" if os.name == "nt" else "ffmpeg")
    ffprobe_bin = DEST / ("ffprobe.exe" if os.name == "nt" else "ffprobe")
    actual_ffmpeg = subprocess.check_output([str(ffmpeg_bin), "-version"], text=True).splitlines()[0]
    if LOCK["ffmpeg"]["version"] not in actual_ffmpeg:
        raise RuntimeError(f"Bundled FFmpeg version mismatch: expected {LOCK['ffmpeg']['version']}, got {actual_ffmpeg}")
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(ffmpeg_bin), "-L"]))
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(ffmpeg_bin), "-version"]))
    ffmpeg_name = LOCK["ffmpeg"]["source"]["archive"]
    media_inventory = DEST / "MEDIA_SOURCE_INVENTORY.json"
    build_configuration = DEST / "FFMPEG_BUILD_CONFIGURATION.json"

    readme = DEST / "LOCKED_TOOLS.json"
    tool_record = {
        "target": TARGET,
        "versions": {"node": LOCK["node"]["version"], "yt-dlp": LOCK["ytDlp"]["version"], "ffmpeg-build": LOCK["ffmpeg"]["version"]},
        "sha256": {
            node_archive_name: sha256(node_archive),
            ytdlp_name: sha256(ytdlp),
            "nodeExecutable": sha256(node_bin),
            "ytDlpExecutable": sha256(ytdlp_bin),
            ffmpeg_name: sha256(DEST / "sources" / ffmpeg_name),
            "ffmpegExecutable": sha256(ffmpeg_bin),
            "ffprobeExecutable": sha256(ffprobe_bin),
            "mediaSourceInventory": sha256(media_inventory),
            "ffmpegBuildConfiguration": sha256(build_configuration),
            "buildRecipe": {filename: sha256(recipe_dir / filename) for filename in recipe_files},
        },
        "sources": {"yt-dlp": {"commit": LOCK["ytDlp"]["sourceCommit"], "url": LOCK["ytDlp"]["licenseUrl"]}},
        "license": {"node": LOCK["node"]["license"], "yt-dlp": LOCK["ytDlp"]["license"], "ffmpeg": LOCK["ffmpeg"]["license"]}
    }
    tool_record["mediaSourceInventory"] = json.loads(media_inventory.read_text(encoding="utf-8"))
    tool_record["ffmpegBuildConfiguration"] = json.loads(build_configuration.read_text(encoding="utf-8"))
    if TARGET == "aarch64-apple-darwin":
        tool_record["fpcalc"] = {"version": LOCK["fpcalc"]["version"], "sha256": LOCK["fpcalc"]["sha256"], "license": LOCK["fpcalc"]["license"]}
    readme.write_text(json.dumps(tool_record, indent=2) + "\n", encoding="utf-8")
subprocess.run([sys.executable, str(ROOT / "scripts/release/verify-media-tool-build.py"), TARGET], check=True)
shutil.rmtree(PREPARED_CACHE, ignore_errors=True)
PREPARED_CACHE.parent.mkdir(parents=True, exist_ok=True)
shutil.copytree(DEST, PREPARED_CACHE)
print(f"Prepared verified tool resources at {DEST}")
