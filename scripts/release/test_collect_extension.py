"""Check public companion names, signature preservation and inventory consistency."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]
version = json.loads((ROOT / 'extension/manifest.json').read_text())['version']
for original in ['amo-generated-id.xpi', 'burrata-firefox-reused.xpi']:
    with tempfile.TemporaryDirectory() as directory:
        workspace = Path(directory)
        artifacts = workspace / 'release-artifacts'
        artifacts.mkdir()
        firefox = artifacts / original
        with zipfile.ZipFile(firefox, 'w') as archive:
            for name in ['META-INF/mozilla.rsa', 'META-INF/mozilla.sf', 'META-INF/manifest.mf']:
                archive.writestr(name, 'signature fixture')
        chrome = artifacts / 'burrata-chromium.zip'
        chrome.write_bytes(b'chrome fixture')
        before = [hashlib.sha256(p.read_bytes()).hexdigest() for p in [firefox, chrome]]
        env = dict(os.environ, RELEASE_TAG='v9.8.7', RELEASE_COMMIT='a' * 40)
        subprocess.run(['bash', str(ROOT / 'scripts/release/collect-extension.sh')], cwd=workspace, env=env, check=True)
        expected = [f'firefox-extension-{version}.xpi', f'chrome-extension-{version}.zip']
        inventory = json.loads((artifacts / 'extension-inventory.json').read_text())
        assert {item['file'] for item in inventory['artifacts']} == set(expected)
        assert [hashlib.sha256((artifacts / name).read_bytes()).hexdigest() for name in expected] == before
        for item in inventory['artifacts']:
            assert item['sha256'] == hashlib.sha256((artifacts / item['file']).read_bytes()).hexdigest()
            assert item['bytes'] == (artifacts / item['file']).stat().st_size
        sums = {line.split(None, 1)[1].strip().removeprefix("./"): line.split(None, 1)[0] for line in (artifacts / 'SHA256SUMS.txt').read_text().splitlines()}
        assert set(sums) == set(expected) | {'extension-inventory.json'}
        assert all(hashlib.sha256((artifacts / name).read_bytes()).hexdigest() == digest for name, digest in sums.items())
print('Companion release naming and preserved package bytes pass.')
