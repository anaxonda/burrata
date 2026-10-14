#!/usr/bin/env python3
"""Build the locked FFmpeg media runtime natively for Linux x86_64."""
import json
import os
import pathlib
import shutil
import subprocess
import sys

from importlib.machinery import SourceFileLoader

COMMON = SourceFileLoader("media_common", str(pathlib.Path(__file__).with_name("build-media-tools-common.py"))).load_module()
ROOT = COMMON.ROOT
LOCK = COMMON.LOCK
TARGET = "x86_64-unknown-linux-gnu"
DEST = pathlib.Path(sys.argv[1]).resolve()
if sys.platform != "linux" or os.uname().machine not in ("x86_64", "amd64"):
    raise SystemExit("Pinned Linux media tools must be built natively on Linux x86_64")
DEST.mkdir(parents=True, exist_ok=True)
jobs = str(max(1, int(os.environ.get("BURRATA_MEDIA_BUILD_JOBS", os.cpu_count() or 2))))
with __import__("tempfile").TemporaryDirectory(prefix="burrata-linux-media-") as temp:
    os.environ["BURRATA_MEDIA_BUILD_DIR"] = temp
    prefix, source, inventory = COMMON.prepare_sources(DEST, TARGET, ("ffmpeg", "lame", "opus", "zlib", "openssl"))
    env = os.environ.copy()
    pkgconfig_dirs = [prefix / "lib/pkgconfig", prefix / "lib64/pkgconfig"]
    env["PKG_CONFIG_PATH"] = os.pathsep.join(map(str, pkgconfig_dirs))
    env["PKG_CONFIG_LIBDIR"] = env["PKG_CONFIG_PATH"]
    env["PKG_CONFIG_ALLOW_SYSTEM_CFLAGS"] = "1"
    env["PKG_CONFIG_ALLOW_SYSTEM_LIBS"] = "1"
    COMMON.build_libs(prefix, source, jobs, env, windows=False)

    openssl = source["openssl"]
    COMMON.run(["./Configure", "linux-x86_64", "no-shared", "no-tests", "no-apps", "no-zlib", "no-module", f"--prefix={prefix}", f"--openssldir={prefix / 'ssl'}"], openssl, env)
    COMMON.run(["make", "-j" + jobs], openssl, env)
    COMMON.run(["make", "install_sw"], openssl, env)

    ffmpeg = source["ffmpeg"]
    profile = LOCK["ffmpeg"]
    configure = [
        "./configure", f"--prefix={prefix}", *profile["configureCommon"],
        profile["tlsBackends"][TARGET]["flag"], "--pkg-config-flags=--static",
        f"--extra-cflags=-I{prefix / 'include'}",
        f"--extra-ldflags=-L{prefix / 'lib'}",
    ]
    COMMON.run(configure, ffmpeg, env)
    COMMON.run(["make", "-j" + jobs], ffmpeg, env)
    COMMON.run(["make", "install"], ffmpeg, env)

    for name in ("ffmpeg", "ffprobe"):
        binary = prefix / "bin" / name
        if not binary.is_file():
            raise RuntimeError(f"Source build did not produce {name}")
        shutil.copy2(binary, DEST / name)
        (DEST / name).chmod(0o755)
    COMMON.stage_notices(DEST, source, inventory)
    (DEST / "FFMPEG_BUILD_INFO.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-version"]))
    (DEST / "FFMPEG_LICENSE.txt").write_bytes(subprocess.check_output([str(DEST / "ffmpeg"), "-L"]))
    (DEST / "FFMPEG_BUILD_CONFIGURATION.json").write_text(json.dumps({
        "target": TARGET,
        "version": profile["version"],
        "configure": configure[1:],
        "tls": profile["tlsBackends"][TARGET],
        "buildJobs": jobs,
        "toolchain": {"cc": env.get("CC", "cc"), "make": env.get("MAKE", "make")},
    }, indent=2) + "\n", encoding="utf-8")
    subprocess.run([sys.executable, str(ROOT / "scripts/release/test-media-tools.py"), str(DEST / "ffmpeg"), str(DEST / "ffprobe")], check=True)
