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
        (directory / "artifact-inventory.json").write_text(json.dumps({
            "tag": "v1.2.3", "commit": commit, "label": label,
        }), encoding="utf-8")
    extension_dir = incoming / "burrata-extensions-fixture"
    extension_dir.mkdir()
    (extension_dir / "signed.xpi").write_bytes(b"signed xpi fixture")
    (extension_dir / "chromium.zip").write_bytes(b"chromium fixture")
    env = dict(os.environ, RELEASE_TAG="v1.2.3", RELEASE_COMMIT=commit)
    subprocess.run(["bash", str(ASSEMBLER)], cwd=workspace, env=env, check=True)
    output = workspace / "release-artifacts"
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
print("YouTube support feed fixture passed")
