#!/usr/bin/env python3
"""Build verified FFmpeg, LAME, Opus, and fpcalc for native Apple Silicon."""
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

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))
TARGET = "aarch64-apple-darwin"
DEST = pathlib.Path(sys.argv[1]).resolve()
if os.uname().machine != "arm64":
    raise SystemExit("Pinned macOS media tools must be built natively on Apple Silicon")
DEST.mkdir(parents=True, exist_ok=True)


def download(url: str, path: pathlib.Path, expected: str) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "Burrata-release-tool-preparer"})
    with urllib.request.urlopen(request, timeout=180) as response, path.open("wb") as output:
        shutil.copyfileobj(response, output)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != expected:
        raise RuntimeError(f"SHA-256 mismatch for {path.name}: expected {expected}, got {digest}")


def extract(archive: pathlib.Path, directory: pathlib.Path) -> pathlib.Path:
    directory.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:*" ) as tf:
        tf.extractall(directory, filter="data")
    roots = list(directory.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise RuntimeError(f"Unexpected source archive layout: {archive.name}")
    return roots[0]


def run(args: list[str], cwd: pathlib.Path, env: dict[str, str] | None = None) -> None:
    subprocess.run(args, cwd=cwd, env=env, check=True)


with tempfile.TemporaryDirectory(prefix="burrata-macos-media-") as temp_dir:
    tmp = pathlib.Path(temp_dir)
    source_dir = tmp / "sources"
    source_dir.mkdir()
    prefix = tmp / "prefix"
    prefix.mkdir()
    source_specs = [
        ("ffmpeg", "https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz", LOCK["ffmpeg"]["macOSSourceBuild"]["sha256"], "ffmpeg-8.1.3.tar.xz"),
        ("lame", LOCK["lame"]["url"], LOCK["lame"]["sha256"], "lame-3.100.tar.gz"),
        ("opus", LOCK["opus"]["url"], LOCK["opus"]["sha256"], "opus-1.6.1.tar.gz"),
        ("chromaprint", LOCK["fpcalc"]["upstream"], LOCK["fpcalc"]["sha256"], "chromaprint-1.5.1.tar.gz"),
    ]
    extracted = {}
    source_inventory = []
    for key, url, digest, filename in source_specs:
        archive = source_dir / filename
        download(url, archive, digest)
        source_inventory.append({"name": key, "version": LOCK["ffmpeg"]["macOSSourceBuild"]["version"] if key == "ffmpeg" else LOCK[key if key != "chromaprint" else "fpcalc"]["version"], "url": url, "sha256": digest, "file": filename})
        extracted[key] = extract(archive, tmp / (key + "-src"))

    jobs = str(os.cpu_count() or 4)
    lame = extracted["lame"]
    chromaprint = extracted["chromaprint"]
    run(["./configure", f"--prefix={prefix}", "--enable-static", "--disable-shared", "--disable-frontend"], lame)
    run(["make", "-j" + jobs], lame)
    run(["make", "install"], lame)

    opus = extracted["opus"]
    run(["./configure", f"--prefix={prefix}", "--enable-static", "--disable-shared"], opus)
    run(["make", "-j" + jobs], opus)
    run(["make", "install"], opus)

    env = os.environ.copy()
    env["PKG_CONFIG_PATH"] = str(prefix / "lib/pkgconfig")
    env["PKG_CONFIG_ALLOW_SYSTEM_CFLAGS"] = "1"
    env["PKG_CONFIG_ALLOW_SYSTEM_LIBS"] = "1"
    ffmpeg = extracted["ffmpeg"]
    macos_configure = LOCK["ffmpeg"]["macOSSourceBuild"]["configure"]
    configure = [
        "./configure", f"--prefix={prefix}", *macos_configure,
        "--pkg-config-flags=--static", f"--extra-cflags=-I{prefix}/include -mmacosx-version-min=14.0",
        f"--extra-ldflags=-L{prefix}/lib -mmacosx-version-min=14.0",
    ]
    run(configure, ffmpeg, env)
    run(["make", "-j" + jobs], ffmpeg, env)
    run(["make", "install"], ffmpeg, env)

    # Chromaprint 1.5.1's fpcalc source uses FFmpeg APIs removed in FFmpeg 7+
    # (avcodec_decode_audio4, AVCodecContext.channels/channel_layout). Use the
    # official pinned arm64 release binary instead of patching those interfaces.
    # The binary only imports system libraries and Apple's Accelerate framework.
    fpcalc_asset = LOCK["fpcalc"]["binaryAssets"][TARGET]
    fpcalc_archive = source_dir / fpcalc_asset["asset"]
    download(fpcalc_asset["url"], fpcalc_archive, fpcalc_asset["sha256"])
    fpcalc_source = extract(fpcalc_archive, tmp / "fpcalc-binary-src")
    fpcalc_built = fpcalc_source / fpcalc_asset["binary"]
    if not fpcalc_built.is_file():
        raise RuntimeError(f"Official fpcalc asset did not contain {fpcalc_asset['binary']}")
    actual_fpcalc_sha = hashlib.sha256(fpcalc_built.read_bytes()).hexdigest()
    if actual_fpcalc_sha != fpcalc_asset["binarySha256"]:
        raise RuntimeError(f"fpcalc binary SHA-256 mismatch: expected {fpcalc_asset['binarySha256']}, got {actual_fpcalc_sha}")
    fpcalc_version = subprocess.check_output([str(fpcalc_built), "-version"], text=True).strip()
    if not fpcalc_version.startswith(f"fpcalc version {LOCK['fpcalc']['version']} "):
        raise RuntimeError(f"Unexpected fpcalc version: {fpcalc_version}")

    for name in ("ffmpeg", "ffprobe"):
        built = prefix / "bin" / name
        if not built.is_file():
            raise RuntimeError(f"Pinned native build did not produce {name}")
        target = DEST / name
        shutil.copy2(built, target)
        target.chmod(0o755)
    shutil.copy2(fpcalc_built, DEST / "fpcalc")
    (DEST / "fpcalc").chmod(0o755)
    bundled_sources = DEST / "sources"
    bundled_sources.mkdir(exist_ok=True)
    for _, _, _, filename in source_specs:
        shutil.copy2(source_dir / filename, bundled_sources / filename)
    (DEST / "MACOS_MEDIA_SOURCE_INVENTORY.json").write_text(json.dumps({
        "target": TARGET,
        "ffmpeg": {
            "version": LOCK["ffmpeg"]["macOSSourceBuild"]["version"],
            "license": "GPL build",
            "configure": configure[1:],
            "systemDependencies": LOCK["ffmpeg"]["macOSSourceBuild"]["systemDependencies"],
        },
        "lame": {"version": LOCK["lame"]["version"], "license": LOCK["lame"]["license"]},
        "opus": {"version": LOCK["opus"]["version"], "license": LOCK["opus"]["license"]},
        "fpcalc": {
            "version": LOCK["fpcalc"]["version"],
            "license": LOCK["fpcalc"]["license"],
            "asset": fpcalc_asset["asset"],
            "assetSha256": fpcalc_asset["sha256"],
            "binarySha256": actual_fpcalc_sha,
            "sourceCommit": LOCK["fpcalc"]["sourceCommit"],
        },
        "sources": source_inventory,
    }, indent=2) + "\n", encoding="utf-8")
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-L"]))
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-version"]))
    for source, license_name, output_name in (
        (extracted["lame"], "COPYING", "LAME_LICENSE.txt"),
        (extracted["opus"], "COPYING", "OPUS_LICENSE.txt"),
        (chromaprint, "LICENSE.md", "CHROMAPRINT_LICENSE.md"),
    ):
        license_path = source / license_name
        if not license_path.is_file():
            raise RuntimeError(f"Missing upstream license file: {license_path}")
        (DEST / output_name).write_bytes(license_path.read_bytes())
    lgpl = LOCK["fpcalc"]["licenseText"]
    lgpl_path = ROOT / lgpl["localPath"]
    lgpl_sha = hashlib.sha256(lgpl_path.read_bytes()).hexdigest()
    if lgpl_sha != lgpl["sha256"]:
        raise RuntimeError(f"Checked-in LGPL text SHA-256 mismatch: expected {lgpl['sha256']}, got {lgpl_sha}")
    shutil.copy2(lgpl_path, DEST / lgpl["file"])
