#!/usr/bin/env python3
"""Build the locked FFmpeg runtime with the MSYS2 MinGW x64 toolchain."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

from importlib.machinery import SourceFileLoader

COMMON = SourceFileLoader("media_common", str(pathlib.Path(__file__).with_name("build-media-tools-common.py"))).load_module()
ROOT = COMMON.ROOT
LOCK = COMMON.LOCK
TARGET = "x86_64-pc-windows-msvc"
DEST = pathlib.Path(sys.argv[1]).resolve()
if os.name != "nt" or not os.environ.get("MSYSTEM", "").startswith("MINGW64"):
    raise SystemExit("Pinned Windows FFmpeg must be built in the MSYS2 MINGW64 environment")
if os.environ.get("PROCESSOR_ARCHITECTURE", "").upper() not in ("AMD64", "ARM64"):
    raise SystemExit("Pinned Windows media tools require an x64 Windows host")
DEST.mkdir(parents=True, exist_ok=True)
jobs = str(max(1, int(os.environ.get("BURRATA_MEDIA_BUILD_JOBS", os.cpu_count() or 2))))
with tempfile.TemporaryDirectory(prefix="burrata-windows-media-") as temp:
    os.environ["BURRATA_MEDIA_BUILD_DIR"] = temp
    prefix, source, inventory = COMMON.prepare_sources(DEST, TARGET, ("ffmpeg", "lame", "opus", "zlib"))
    env = os.environ.copy()
    prefix_posix = COMMON.posix_path(prefix)
    env["PKG_CONFIG_PATH"] = f"{prefix_posix}/lib/pkgconfig"
    env["PKG_CONFIG_LIBDIR"] = env["PKG_CONFIG_PATH"]
    env["PKG_CONFIG_ALLOW_SYSTEM_CFLAGS"] = "1"
    env["PKG_CONFIG_ALLOW_SYSTEM_LIBS"] = "1"
    env["CC"] = "gcc"
    env["AR"] = "ar"
    env["RANLIB"] = "ranlib"
    env["WINDRES"] = "windres"
    compiler_target = subprocess.check_output([env["CC"], "-dumpmachine"], text=True).strip()
    if compiler_target != "x86_64-w64-mingw32":
        raise RuntimeError(f"MSYS2 MINGW64 gcc targets {compiler_target}, expected x86_64-w64-mingw32")
    COMMON.build_libs(prefix, source, jobs, env, windows=True)

    ffmpeg = source["ffmpeg"]
    profile = LOCK["ffmpeg"]
    configure = [
        "sh", "./configure", "--target-os=mingw32", "--arch=x86_64",
        "--cc=gcc", "--ar=ar", "--ranlib=ranlib", "--windres=windres", f"--prefix={prefix_posix}",
        *profile["configureCommon"], profile["tlsBackends"][TARGET]["flag"],
        "--pkg-config-flags=--static",
        f"--extra-cflags=-I{prefix_posix}/include",
        f"--extra-ldflags=-L{prefix_posix}/lib -static",
    ]
    COMMON.run(configure, ffmpeg, env, windows=True)
    COMMON.run(["make", "-j" + jobs], ffmpeg, env, windows=True)
    COMMON.run(["make", "install"], ffmpeg, env, windows=True)

    for name in ("ffmpeg", "ffprobe"):
        binary = prefix / "bin" / f"{name}.exe"
        if not binary.is_file():
            raise RuntimeError(f"Source build did not produce {name}.exe")
        shutil.copy2(binary, DEST / f"{name}.exe")
    COMMON.stage_notices(DEST, source, inventory)
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg.exe"), "-version"]))
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg.exe"), "-L"]))
    (DEST / "FFMPEG_BUILD_CONFIGURATION.json").write_text(json.dumps({
        "target": TARGET,
        "version": profile["version"],
        "configure": configure[1:],
        "tls": profile["tlsBackends"][TARGET],
        "buildJobs": jobs,
        "toolchain": {"system": "MSYS2 MINGW64", "cc": env["CC"], "make": env.get("MAKE", "make")},
    }, indent=2) + "\n", encoding="utf-8")
    subprocess.run([sys.executable, str(ROOT / "scripts/release/test-media-tools.py"), str(DEST / "ffmpeg.exe"), str(DEST / "ffprobe.exe")], check=True)
