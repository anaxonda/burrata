#!/usr/bin/env python3
"""Stage a temporary Firefox companion research build; optionally register its sink."""
import argparse
import json
from pathlib import Path
import shutil
import shlex
import sys

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--register-host', action='store_true')
args = parser.parse_args()
target = root / '.cache/monochrome-research-extension'
shutil.copytree(root / 'extension', target, dirs_exist_ok=True,
                ignore=shutil.ignore_patterns('tests'))
(target / 'research').mkdir(exist_ok=True)
for name in ['capture.js', 'background.js', 'status.html', 'status.js']:
    shutil.copyfile(Path(__file__).parent / name, target / 'research' / name)
manifest = json.loads((target / 'manifest.json').read_text())
manifest['name'] = 'Burrata Browser Companion — Monochrome research'
manifest.pop('host_permissions', None)
optional_hosts = manifest.setdefault('optional_host_permissions', [])
if 'https://monochrome.samidy.com/*' not in optional_hosts:
    optional_hosts.append('https://monochrome.samidy.com/*')
background_scripts = manifest.setdefault('background', {}).setdefault('scripts', [])
background_scripts[:] = [script for script in background_scripts
                         if script != 'research/background.js']
background_scripts.append('research/background.js')
manifest['action']['default_popup'] = 'research/status.html'
(target / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
if args.register_host:
    directory = Path.home() / '.mozilla/native-messaging-hosts'
    directory.mkdir(parents=True, exist_ok=True)
    wrapper = target / 'native-host'
    wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' ' + shlex.quote(str(Path(__file__).parent / 'native_host.py')) + '\n')
    wrapper.chmod(0o700)
    native = {'name': 'org.burrata.monochrome_research', 'description': 'Local sanitized Monochrome research capture', 'path': str(wrapper), 'type': 'stdio', 'allowed_extensions': ['burrata-browser@burrata.local']}
    destination = directory / 'org.burrata.monochrome_research.json'
    destination.write_text(json.dumps(native, indent=2) + '\n')
    destination.chmod(0o600)
print(target / 'manifest.json')
