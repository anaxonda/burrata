#!/usr/bin/env python3
"""Prepare a loadable Chrome companion without changing Firefox's manifest."""
import json
import base64
import hashlib
from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[1]
source = root / 'extension'
destination = root / '.cache' / 'browser-companion-chromium'
destination.mkdir(parents=True, exist_ok=True)
for name in ('background.js', 'content.js', 'parsers.js', 'popup.js', 'popup.html', 'spotify-auth.js', 'provider-operations.js'):
    shutil.copyfile(source / name, destination / name)
shutil.copytree(source / 'icons', destination / 'icons', dirs_exist_ok=True)
manifest = json.loads((source / 'manifest.chromium.json').read_text())
key = manifest.get('key')
if not isinstance(key, str):
    raise SystemExit('Chromium manifest is missing its stable public key')
try:
    extension_id = ''.join(chr(ord('a') + int(digit, 16)) for digit in hashlib.sha256(base64.b64decode(key, validate=True)).hexdigest()[:32])
except (ValueError, TypeError) as error:
    raise SystemExit(f'Chromium manifest has an invalid public key: {error}')
(destination / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(f'Chrome/Edge Load unpacked directory: {destination} (extension ID: {extension_id})')
