#!/usr/bin/env python3
"""Seed one coherent build-host GLib family before linuxdeploy scans dependencies.

linuxdeploy's own AppImage can leak its older GLib into dependency discovery.
Preseeding the SONAMEs prevents that runtime from replacing the libraries used
to build WebKit, Soup and GStreamer. This config is only used for Linux bundles.
"""
import argparse
import json
from pathlib import Path
import subprocess


LIBRARIES = ("glib", "gobject", "gio", "gmodule", "gthread")


def configuration(libdir: Path) -> dict:
    files = {}
    for name in LIBRARIES:
        soname = f"lib{name}-2.0.so.0"
        source = (libdir / soname).resolve(strict=True)
        if not source.is_file():
            raise ValueError(f"Missing build-host GLib library: {soname}")
        files[f"/usr/lib/{soname}"] = str(source)
    return {"bundle": {"linux": {"appimage": {"files": files}}}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = subprocess.check_output(["pkg-config", "--modversion", "glib-2.0"], text=True).strip()
    if tuple(int(n) for n in version.split(".")[:2]) < (2, 68):
        raise ValueError("AppImage WebKit/Soup requires build-host GLib >= 2.68")
    libdir = Path(subprocess.check_output(["pkg-config", "--variable=libdir", "glib-2.0"], text=True).strip())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(configuration(libdir), indent=2) + "\n")
    print(f"AppImage GLib family: {version}")


if __name__ == "__main__":
    main()
