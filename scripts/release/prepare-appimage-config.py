#!/usr/bin/env python3
"""Seed a coherent build-host GTK/WebKit/GStreamer runtime before linuxdeploy.

The packager's own AppImage can leak older or differently configured libraries
into dependency discovery. Seed the real UI libraries and their dependency
closure, while leaving host-owned graphics, audio and base libraries alone.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

# Host-owned SONAMEs follow https://github.com/AppImage/pkg2appimage/blob/master/excludelist.
HOST_LIBRARIES = set('''libanl.so.1 libBrokenLocale.so.1 libcidn.so.1 libc.so.6
libdl.so.2 libm.so.6 libmvec.so.1 libnss_compat.so.2 libnss_dns.so.2
libnss_files.so.2 libnss_hesiod.so.2 libnss_nisplus.so.2 libnss_nis.so.2
libpthread.so.0 libresolv.so.2 librt.so.1 libthread_db.so.1 libutil.so.1
libstdc++.so.6 libGL.so.1 libEGL.so.1 libGLdispatch.so.0 libGLX.so.0
libOpenGL.so.0 libdrm.so.2 libglapi.so.0 libgbm.so.1 libxcb.so.1
libX11.so.6 libX11-xcb.so.1 libwayland-client.so.0 libasound.so.2
libfontconfig.so.1 libfreetype.so.6 libharfbuzz.so.0 libcom_err.so.2
libexpat.so.1 libgcc_s.so.1 libgpg-error.so.0 libICE.so.6 libSM.so.6
libusb-1.0.so.0 libuuid.so.1 libz.so.1 libjack.so.0 libpipewire-0.3.so.0
libxcb-dri3.so.0 libxcb-dri2.so.0 libfribidi.so.0 libgmp.so.10'''.split())
ROOT_LIBRARIES = {
    'glib-2.0': [f'lib{name}-2.0.so.0' for name in ('glib', 'gobject', 'gio', 'gmodule', 'gthread')],
    'gtk+-3.0': ['libgtk-3.so.0', 'libgdk-3.so.0'],
    'webkit2gtk-4.1': ['libwebkit2gtk-4.1.so.0', 'libjavascriptcoregtk-4.1.so.0'],
    'libsoup-3.0': ['libsoup-3.0.so.0'],
    'gstreamer-1.0': ['libgstreamer-1.0.so.0'],
}


def pkg_variable(package: str, name: str) -> str:
    return subprocess.check_output(['pkg-config', f'--variable={name}', package], text=True).strip()


def dependencies(output: str) -> dict[str, Path]:
    libraries = {}
    for line in output.splitlines():
        if '=> not found' in line:
            raise ValueError(f'Build-host runtime dependency is missing: {line.strip()}')
        match = re.match(r'^\s*(\S+)\s+=>\s+(.+?)\s+\(0x[0-9a-fA-F]+\)', line)
        if not match:
            continue
        name, path = match.groups()
        if name in HOST_LIBRARIES or name.startswith('ld-linux'):
            continue
        source = Path(path).resolve()
        if name in libraries and libraries[name] != source:
            raise ValueError(f'Conflicting build-host libraries for {name}')
        libraries[name] = source
    return libraries


def configuration() -> dict:
    libraries = {}
    roots = []
    for package, names in ROOT_LIBRARIES.items():
        libdir = Path(pkg_variable(package, 'libdir'))
        for name in names:
            source = libdir / name
            source.resolve(strict=True)
            libraries[name] = source
            roots.append(source)
    # These plugins are bundled by Tauri too. Their dependencies must come from
    # the same build environment as the UI, including Cairo's X11 backend.
    plugin_dirs = [Path(pkg_variable('gstreamer-1.0', 'pluginsdir')),
                   Path(pkg_variable('gdk-pixbuf-2.0', 'gdk_pixbuf_moduledir'))]
    for directory in plugin_dirs:
        if not directory.is_dir():
            raise ValueError(f'Build-host plugin directory is missing: {directory}')
        roots.extend(sorted(directory.glob('*.so')))
    clean_env = {key: value for key, value in os.environ.items()
                 if key not in ('LD_LIBRARY_PATH', 'LD_PRELOAD')}
    output = subprocess.check_output(['ldd', *map(str, roots)], text=True, env=clean_env)
    libraries.update(dependencies(output))
    files = {}
    for name, source in sorted(libraries.items()):
        resolved = source.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(f'Build-host runtime library is missing: {name}')
        files[f'/usr/lib/{name}'] = str(resolved)
    return {'bundle': {'linux': {'appimage': {'files': files}}}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    version = subprocess.check_output(['pkg-config', '--modversion', 'glib-2.0'], text=True).strip()
    if tuple(int(n) for n in version.split('.')[:2]) < (2, 68):
        raise ValueError('AppImage WebKit/Soup requires build-host GLib >= 2.68')
    config = configuration()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(config, indent=2) + '\n')
    print(f"AppImage runtime: GLib {version}, {len(config['bundle']['linux']['appimage']['files'])} coherent libraries")


if __name__ == '__main__':
    main()
