#!/usr/bin/env python3
"""Keep verified desktop installers in the main release and split support files."""
import hashlib
import json
from pathlib import Path
import shutil
import sys


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            checksum.update(chunk)
    return checksum.hexdigest()


def checksums(directory):
    entries = [f"{digest(p)}  {p.name}\n" for p in sorted(directory.iterdir())
               if p.name != "SHA256SUMS.txt"]
    (directory / "SHA256SUMS.txt").write_text("".join(entries), encoding="utf-8")


def split(main, support):
    if support.exists():
        raise ValueError("Supporting output already exists")
    files = list(main.iterdir())
    if any(not p.is_file() or p.is_symlink() for p in files):
        raise ValueError("Expected a flat directory of regular release assets")
    installers = {}
    labels = set()
    for inventory in main.glob("*-artifact-inventory.json"):
        record = json.loads(inventory.read_text(encoding="utf-8"))
        label = record["label"]
        if label in labels:
            raise ValueError("Duplicate platform inventory")
        labels.add(label)
        prefix = inventory.name.removesuffix("artifact-inventory.json")
        for item in record["artifacts"]:
            name = item["file"]
            if Path(name).name != name or name in installers.values():
                raise ValueError("Invalid or duplicate installer name")
            source = main / (prefix + "installers-" + name)
            if source.stat().st_size != item["bytes"] or digest(source) != item["sha256"]:
                raise ValueError("Installer does not match its inventory")
            installers[source] = name
    if labels != {"linux-x64", "macos-arm64", "windows-x64"} or len(installers) != 5:
        raise ValueError("Expected all three platforms and five desktop installers")
    for suffix in (".deb", ".rpm", ".AppImage", ".dmg", "-setup.exe"):
        if sum(name.endswith(suffix) for name in installers.values()) != 1:
            raise ValueError("Missing or duplicate installer format: " + suffix)
    if {p for p in files if "-installers-" in p.name} != set(installers):
        raise ValueError("Uninventoried installer asset")
    if any((main / name).exists() for name in installers.values()):
        raise ValueError("Clean installer name collides with an existing asset")

    downloads = dict(installers)
    companion_inventories = list(main.glob("*-extension-inventory.json"))
    if len(companion_inventories) != 1:
        raise ValueError("Expected one verified browser companion inventory")
    inventory = companion_inventories[0]
    companions = json.loads(inventory.read_text(encoding="utf-8"))["artifacts"]
    if len(companions) != 2 or sorted(Path(i["file"]).suffix for i in companions) != [".xpi", ".zip"]:
        raise ValueError("Expected Firefox and Chromium companion packages")
    prefix = inventory.name.removesuffix("extension-inventory.json")
    for item in companions:
        name = item["file"]
        if Path(name).name != name or name in downloads.values() or (main / name).exists():
            raise ValueError("Invalid or duplicate companion name")
        source = main / (prefix + name)
        if source.stat().st_size != item["bytes"] or digest(source) != item["sha256"]:
            raise ValueError("Companion does not match its inventory")
        downloads[source] = name

    # Validate everything before moving anything. Helper executables and ZIPs
    # are support assets; only verified installers and companions stay in main.
    support.mkdir()
    for path in files:
        if path in downloads:
            path.rename(main / downloads[path])
        elif path.name == "SHA256SUMS.txt":
            path.unlink()
        else:
            shutil.move(str(path), support / path.name)
    checksums(main)
    checksums(support)


if __name__ == "__main__":
    split(Path(sys.argv[1]), Path(sys.argv[2]))
