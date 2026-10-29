#!/usr/bin/env python3
"""Relocate bundled components without a process-wide library override."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
TOOLS = {
    'patchelf-0.19.2-x86_64.tar.gz': (
        'https://github.com/NixOS/patchelf/releases/download/0.19.2/patchelf-0.19.2-x86_64.tar.gz',
        '2abdc34fbe949c995a1baa4ec310896a085588a45a99cd0d726851bdfac2a4bb'),
    'linuxdeploy-plugin-appimage.AppImage': (
        'https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage',
        '49d6a17160675a6bd1781699aae6bdf7692d98552e02a3671d2183d10547842e'),
}


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def prepare_tools(directory):
    directory.mkdir(parents=True, exist_ok=True)
    for name, (url, expected) in TOOLS.items():
        path = directory / name
        if not path.is_file() or digest(path) != expected:
            download = path.with_name(name + '.download')
            with urllib.request.urlopen(url, timeout=30) as response, download.open('wb') as output:
                shutil.copyfileobj(response, output)
            if digest(download) != expected:
                download.unlink()
                raise ValueError(f'Bundling tool checksum mismatch: {name}')
            download.replace(path)
    archive = directory / 'patchelf-0.19.2-x86_64.tar.gz'
    with tarfile.open(archive, 'r:gz') as bundle:
        members = [item for item in bundle.getmembers()
                   if item.isfile() and Path(item.name).name == 'patchelf']
        if len(members) != 1:
            raise ValueError('Pinned patchelf archive has an unexpected layout')
        payload = bundle.extractfile(members[0]).read()
    patchelf = directory / 'patchelf'
    patchelf.write_bytes(payload)
    patchelf.chmod(0o755)
    plugin = directory / 'linuxdeploy-plugin-appimage.AppImage'
    plugin.chmod(0o755)
    return patchelf, plugin


def relocate_runtime(appdir, patchelf):
    library_dir = appdir / 'usr/lib'
    pinned_resources = library_dir / 'Burrata/resources'
    changed = 0
    for path in sorted(appdir.rglob('*')):
        # Preserve pinned helper bytes and never follow a link outside AppDir.
        if path.is_symlink() or not path.is_file() or path.is_relative_to(pinned_resources):
            continue
        with path.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                continue
        result = subprocess.run([str(patchelf), '--print-rpath', str(path)],
                                capture_output=True, text=True)
        if result.returncode:
            if 'cannot find section' in result.stderr or 'statically linked' in result.stderr:
                continue
            raise ValueError(f'Could not inspect bundled ELF: {path.relative_to(appdir)}')
        relative = os.path.relpath(library_dir, path.parent)
        prefix = '$ORIGIN' if relative == '.' else '$ORIGIN/' + relative
        paths = list(dict.fromkeys([prefix, *filter(None, result.stdout.strip().split(':'))]))
        rpath = ':'.join(paths)
        if rpath != result.stdout.strip():
            subprocess.run([str(patchelf), '--set-rpath', rpath, str(path)], check=True)
            changed += 1
    return changed


def install_launcher(appdir):
    shutil.copyfile(Path(__file__).with_name('appimage-apprun.sh'), appdir / 'AppRun')
    (appdir / 'AppRun').chmod(0o755)
    (appdir / 'AppRun.wrapped').unlink(missing_ok=True)


def preserve_runtime(installer, destination):
    # Reuse Tauri's already selected runtime; repacking must not fetch a new
    # mutable runtime from the network after the installer has been built.
    offset = int(subprocess.check_output([str(installer), '--appimage-offset'], text=True).strip())
    if not 4096 <= offset <= 16 * 1024 * 1024 or offset >= installer.stat().st_size:
        raise ValueError('Generated AppImage has an invalid runtime boundary')
    with installer.open('rb') as stream:
        runtime = stream.read(offset)
        if runtime[:4] != b'\x7fELF' or stream.read(4) != b'hsqs':
            raise ValueError('Generated AppImage runtime or SquashFS signature is invalid')
    destination.write_bytes(runtime)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tools-dir', type=Path, default=ROOT / '.cache/bundling-tools')
    parser.add_argument('--prepare-only', action='store_true')
    parser.add_argument('--bundle-dir', type=Path)
    args = parser.parse_args()
    patchelf, plugin = prepare_tools(args.tools_dir.resolve())
    if args.prepare_only:
        print('Verified pinned AppImage output tool and patchelf 0.19.2')
        return
    if not args.bundle_dir:
        parser.error('--bundle-dir is required unless preparing tools only')
    directory = args.bundle_dir.resolve(strict=True)
    appdirs = list(directory.glob('*.AppDir'))
    installers = list(directory.glob('*.AppImage'))
    if len(appdirs) != 1 or len(installers) != 1:
        raise ValueError('Expected one generated AppDir and AppImage')
    install_launcher(appdirs[0])
    changed = relocate_runtime(appdirs[0], patchelf)
    env = {key: value for key, value in os.environ.items()
           if key not in ('LD_LIBRARY_PATH', 'LD_PRELOAD', 'APPDIR', 'APPIMAGE')}
    with tempfile.TemporaryDirectory(prefix='burrata-repack-', dir=directory) as temp:
        output = Path(temp) / 'qualified.AppImage'
        runtime = Path(temp) / 'runtime-x86_64'
        preserve_runtime(installers[0], runtime)
        env.update(APPIMAGE_EXTRACT_AND_RUN='1', ARCH='x86_64', OUTPUT=str(output), LDAI_OUTPUT=str(output), LDAI_RUNTIME_FILE=str(runtime))
        subprocess.run([str(plugin), '--appimage-extract-and-run', '--appdir', str(appdirs[0])],
                       env=env, cwd=directory, check=True)
        if not output.is_file() or output.stat().st_size == 0:
            raise ValueError('AppImage output tool did not produce the qualified installer')
        output.chmod(0o755)
        output.replace(installers[0])
    print(f'Finalized AppImage with isolated library paths ({changed} components relocated)')


if __name__ == '__main__':
    main()
