#!/usr/bin/env python3
"""Fixture test for target feed checksums and release-asset naming."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSEMBLER = ROOT / "scripts/release/assemble-draft.sh"
TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
}


def check_patched_stream_download_collection():
    """Exercise the real collector with synthetic installers and media-tool records."""
    with tempfile.TemporaryDirectory(prefix="burrata-source-collection-") as temp:
        workspace = pathlib.Path(temp)
        release = workspace / "scripts/release"
        release.mkdir(parents=True)
        # Media binaries are outside this fixture's scope; their verification has
        # its own contract suite. The actual collector and inventory writer run.
        (release / "verify-media-tool-build.py").write_text("pass\n")
        for name in ["write-inventory.py", "THIRD_PARTY_NOTICES.md", "locked-tools.json"]:
            shutil.copy2(ROOT / "scripts/release" / name, release / name)
        vendor = workspace / "src-tauri/vendor"
        vendor.mkdir(parents=True)
        shutil.copy2(ROOT / "src-tauri/vendor/SOURCES.md", vendor / "SOURCES.md")
        shutil.copytree(ROOT / "src-tauri/vendor/stream-download", vendor / "stream-download",
                        ignore=shutil.ignore_patterns("target", "Cargo.lock"))
        target = TARGETS["linux-x64"]
        tools = workspace / "src-tauri/resources/tools" / target
        (tools / "build-recipe").mkdir(parents=True)
        (tools / "LOCKED_TOOLS.json").write_text('{"sha256": {}}')
        (tools / "FFMPEG_BUILD_CONFIGURATION.json").write_text('{}')
        (tools / "build-recipe/fixture.txt").write_text("synthetic media-tool fixture")
        cache = workspace / ".cache"
        assets = cache / "release-assets"
        assets.mkdir(parents=True)
        (cache / "release-smoke-evidence.json").write_text('{"status": "passed"}')
        (cache / "appimage-smoke-evidence.json").write_text('{"status": "passed"}')
        for name in [f"burrata-yt-dlp-{target}", f"bgutil-plugin-2.0.2-{target}.zip", "youtube-support-target.json"]:
            (assets / name).write_bytes(b"synthetic tool asset")
        bundles = workspace / "src-tauri/target" / target / "release/bundle"
        bundles.mkdir(parents=True)
        (bundles / "fixture.deb").write_bytes(b"synthetic installer")
        environment = dict(os.environ, RELEASE_TAG="v1.2.3", RELEASE_COMMIT="a" * 40,
                           TARGET=target, LABEL="linux-x64")
        command = ["bash", str(ROOT / "scripts/release/collect-artifacts.sh")]
        subprocess.run(command, cwd=workspace, env=environment, check=True, capture_output=True)
        output = workspace / "release-artifacts"
        prefix = "stream-download-0.24.4-burrata-patched"
        archive = output / f"{prefix}.tar.gz"
        archive_before = archive.read_bytes()
        with tarfile.open(archive, "r:gz") as bundle:
            expected = ["Cargo.toml", "Cargo.toml.orig", "LICENSE-MIT", "LICENSE-APACHE", "PATCHES.md", "README.md"]
            expected += [path.relative_to(vendor / "stream-download").as_posix()
                         for path in (vendor / "stream-download/src").rglob("*.rs")]
            assert set(bundle.getnames()) == {f"{prefix}/{name}" for name in expected}
            for name in expected:
                assert bundle.extractfile(f"{prefix}/{name}").read() == (vendor / "stream-download" / name).read_bytes()
        for original, packaged in [("LICENSE-MIT", "STREAM_DOWNLOAD_LICENSE_MIT.txt"),
                                   ("LICENSE-APACHE", "STREAM_DOWNLOAD_LICENSE_APACHE.txt"),
                                   ("PATCHES.md", "STREAM_DOWNLOAD_PATCHES.md")]:
            assert (output / packaged).read_bytes() == (vendor / "stream-download" / original).read_bytes()
        inventory = (output / "THIRD_PARTY_SOURCES.md").read_text()
        assert archive.name in inventory and hashlib.sha256(archive_before).hexdigest() in inventory
        assert "stream-download 0.24.4" in (output / "THIRD_PARTY_NOTICES.md").read_text()
        subprocess.run(command, cwd=workspace, env=environment, check=True, capture_output=True)
        assert archive.read_bytes() == archive_before, "source archive must not contain collection timestamps"
        assert (output / "appimage-smoke-evidence.json").read_text() == '{"status": "passed"}'
        (cache / "appimage-smoke-evidence.json").unlink()
        rejected = subprocess.run(command, cwd=workspace, env=environment, capture_output=True)
        assert rejected.returncode != 0, "Linux collection requires AppImage launch evidence"
        (cache / "appimage-smoke-evidence.json").write_text('{"status": "passed"}')
        (vendor / "stream-download/LICENSE-MIT").unlink()
        rejected = subprocess.run(command, cwd=workspace, env=environment, capture_output=True)
        assert rejected.returncode != 0, "source collection must reject missing upstream license text"


check_patched_stream_download_collection()

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
        # Supporting source/license records must remain checksummed supporting
        # downloads rather than changing the public installer/companion set.
        (directory / "STREAM_DOWNLOAD_LICENSE_MIT.txt").write_bytes(b"MIT license fixture")
        (directory / "stream-download-0.24.4-burrata-patched.tar.gz").write_bytes(b"patched source fixture")
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
    assert len(list(output.glob("*-STREAM_DOWNLOAD_LICENSE_MIT.txt"))) == 3
    assert len(list(output.glob("*-stream-download-0.24.4-burrata-patched.tar.gz"))) == 3
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
