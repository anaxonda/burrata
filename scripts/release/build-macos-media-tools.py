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
    configure = [
        "./configure", f"--prefix={prefix}", "--disable-autodetect", "--disable-shared", "--enable-static",
        "--enable-gpl", "--enable-libmp3lame", "--enable-libopus", "--enable-securetransport",
        "--pkg-config-flags=--static", f"--extra-cflags=-I{prefix}/include -mmacosx-version-min=14.0",
        f"--extra-ldflags=-L{prefix}/lib -mmacosx-version-min=14.0",
    ]
    run(configure, ffmpeg, env)
    run(["make", "-j" + jobs], ffmpeg, env)
    run(["make", "install"], ffmpeg, env)

    chromaprint = extracted["chromaprint"]
    cp_build = tmp / "chromaprint-build"
    cp_env = env.copy()
    run(["cmake", "-S", str(chromaprint), "-B", str(cp_build), "-DCMAKE_BUILD_TYPE=Release",
         "-DBUILD_TOOLS=ON", "-DBUILD_SHARED_LIBS=OFF", f"-DCMAKE_INSTALL_PREFIX={prefix}",
         f"-DCMAKE_PREFIX_PATH={prefix}", "-DPKG_CONFIG_ARGN=--static",
         f"-DCMAKE_EXE_LINKER_FLAGS=-L{prefix}/lib"], tmp, cp_env)
    run(["cmake", "--build", str(cp_build), "--parallel", jobs], tmp, cp_env)
    run(["cmake", "--install", str(cp_build)], tmp, cp_env)

    for name in ("ffmpeg", "ffprobe", "fpcalc"):
        built = prefix / "bin" / name
        if not built.is_file():
            raise RuntimeError(f"Pinned native build did not produce {name}")
        target = DEST / name
        shutil.copy2(built, target)
        target.chmod(0o755)
    bundled_sources = DEST / "sources"
    bundled_sources.mkdir(exist_ok=True)
    for _, _, _, filename in source_specs:
        shutil.copy2(source_dir / filename, bundled_sources / filename)
    (DEST / "MACOS_MEDIA_SOURCE_INVENTORY.json").write_text(json.dumps({
        "target": TARGET,
        "ffmpeg": {"version": "8.1.3", "license": "GPL build", "configure": configure[1:]},
        "lame": {"version": LOCK["lame"]["version"], "license": LOCK["lame"]["license"]},
        "opus": {"version": LOCK["opus"]["version"], "license": LOCK["opus"]["license"]},
        "fpcalc": {"version": LOCK["fpcalc"]["version"], "license": LOCK["fpcalc"]["license"]},
        "sources": source_inventory,
    }, indent=2) + "\n", encoding="utf-8")
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-L"]))
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-version"]))
    for source, license_name, output_name in (
        (extracted["lame"], "COPYING", "LAME_LICENSE.txt"),
        (extracted["opus"], "COPYING", "OPUS_LICENSE.txt"),
        (chromaprint, "LICENSE.md", "CHROMAPRINT_LICENSE.txt"),
    ):
        license_path = source / license_name
        if not license_path.is_file():
            raise RuntimeError(f"Missing upstream license file: {license_path}")
        (DEST / output_name).write_bytes(license_path.read_bytes())
