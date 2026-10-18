#!/usr/bin/env python3
"""Fixture test for target feed checksums and release-asset naming."""
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSEMBLER = ROOT / "scripts/release/assemble-draft.sh"
TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
}

with tempfile.TemporaryDirectory(prefix="burrata-feed-fixture-") as temp:
    workspace = pathlib.Path(temp)
    incoming = workspace / "incoming"
    incoming.mkdir()
    version = "2026.10.10.233141"
    commit = "a" * 40
    installer_names = set()
    for label, target in TARGETS.items():
        directory = incoming / f"burrata-{label}-{commit}"
        directory.mkdir()
        binary = f"burrata-yt-dlp-{target}{'.exe' if target == 'x86_64-pc-windows-msvc' else ''}"
        payload = f"fixture executable for {target}".encode()
        (directory / binary).write_bytes(payload)
        plugin = f"bgutil-plugin-2.0.2-{target}.zip"
        plugin_data = f"fixture plugin for {target}".encode()
        (directory / plugin).write_bytes(plugin_data)
        (directory / "youtube-support-target.json").write_text(json.dumps({
            "version": version,
            "target": target,
            "asset": binary,
            "sha256": hashlib.sha256(payload).hexdigest(),
            "sha512": hashlib.sha512(payload).hexdigest(),
            "pluginAsset": plugin,
            "pluginSha256": hashlib.sha256(plugin_data).hexdigest(),
            "pluginSha512": hashlib.sha512(plugin_data).hexdigest(),
        }), encoding="utf-8")
        names = {
            "macos-arm64": ["Burrata_1.2.3_aarch64.dmg"],
            "windows-x64": ["Burrata_1.2.3_x64-setup.exe"],
            "linux-x64": ["Burrata_1.2.3_amd64.deb", "Burrata-1.2.3-1.x86_64.rpm", "Burrata_1.2.3_amd64.AppImage"],
        }[label]
        (directory / "installers").mkdir()
        artifacts = []
        for name in names:
            data = ("installer fixture " + name).encode()
            (directory / "installers" / name).write_bytes(data)
            artifacts.append({"file": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
            installer_names.add(name)
        (directory / "artifact-inventory.json").write_text(json.dumps({
            "tag": "v1.2.3", "commit": commit, "label": label,
            "artifacts": artifacts,
        }), encoding="utf-8")
    extension_dir = incoming / "burrata-extensions-fixture"
    extension_dir.mkdir()
    (extension_dir / "signed.xpi").write_bytes(b"signed xpi fixture")
    (extension_dir / "chromium.zip").write_bytes(b"chromium fixture")
    (extension_dir / "extension-inventory.json").write_text(json.dumps({
        "artifacts": [{"file": p.name, "bytes": p.stat().st_size,
                       "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
                      for p in [extension_dir / "signed.xpi", extension_dir / "chromium.zip"]]
    }))
    env = dict(os.environ, RELEASE_TAG="v1.2.3", RELEASE_COMMIT=commit)
    damaged = next(incoming.glob("*/installers/*.deb"))
    original = damaged.read_bytes()
    damaged.write_bytes(b"incorrect installer")
    failed = subprocess.run(["bash", str(ASSEMBLER)], cwd=workspace, env=env, capture_output=True)
    assert failed.returncode != 0
    assert not (workspace / "support-artifacts").exists()
    assert len(list((workspace / "release-artifacts").glob("*-installers-*"))) == 5
    damaged.write_bytes(original)
    subprocess.run(["bash", str(ASSEMBLER)], cwd=workspace, env=env, check=True)
    main_output = workspace / "release-artifacts"
    assert {p.name for p in main_output.iterdir()} == installer_names | {"signed.xpi", "chromium.zip", "SHA256SUMS.txt"}
    output = workspace / "support-artifacts"
    assert not any(output.glob("*.xpi")) and not any(output.glob("*chromium.zip"))
    assert (output / "RELEASE_ACCEPTANCE.md").is_file()
    for directory in (main_output, output):
        checked = set()
        for line in (directory / "SHA256SUMS.txt").read_text().splitlines():
            checksum, name = line.split(None, 1)
            assert hashlib.sha256((directory / name).read_bytes()).hexdigest() == checksum
            checked.add(name)
        assert checked == {p.name for p in directory.iterdir()} - {"SHA256SUMS.txt"}
    feed = json.loads((output / "youtube-support.json").read_text(encoding="utf-8"))
    assert feed["version"] == version
    assert set(feed["targets"]) == set(TARGETS.values())
    for target in TARGETS.values():
        entry = feed["targets"][target]
        asset = output / entry["asset"]
        assert asset.is_file() and asset.name == entry["asset"]
        data = asset.read_bytes()
        assert hashlib.sha256(data).hexdigest() == entry["sha256"]
        assert hashlib.sha512(data).hexdigest() == entry["sha512"]
        assert (output / f"bgutil-plugin-2.0.2-{target}.zip").is_file()
print("Installer/support separation and YouTube support feed fixture passed")
