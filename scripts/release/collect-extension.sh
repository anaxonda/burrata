#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?}"
: "${RELEASE_COMMIT:?}"
mkdir -p release-artifacts
test -n "$(find release-artifacts -type f -name '*.xpi' -print -quit)" || { echo 'AMO did not produce a signed Firefox XPI' >&2; exit 1; }
mapfile -t chromium_zips < <(find release-artifacts -maxdepth 1 -type f -name 'burrata-chromium.zip' -print)
test "${#chromium_zips[@]}" -gt 0 || { echo 'Chromium extension ZIP is missing' >&2; exit 1; }
python3 - "$(dirname "${BASH_SOURCE[0]}")/../../extension" <<'PY'
import json, pathlib, re, sys, zipfile
source = pathlib.Path(sys.argv[1])
version = json.loads((source / 'manifest.json').read_text())['version']
assert re.fullmatch(r'[0-9]+(?:\.[0-9]+){1,3}', version), 'Invalid extension version'
assert json.loads((source / 'manifest.chromium.json').read_text())['version'] == version
root = pathlib.Path('release-artifacts')
firefox = list(root.glob('*.xpi'))
chrome = list(root.glob('burrata-chromium.zip'))
assert len(firefox) == len(chrome) == 1, 'Expected one package for each browser'
with zipfile.ZipFile(firefox[0]) as archive:
    names = {name.lower() for name in archive.namelist()}
    required = {'meta-inf/mozilla.rsa', 'meta-inf/mozilla.sf', 'meta-inf/manifest.mf'}
    assert required.issubset(names), 'Firefox package lacks AMO signature files'
for path, name in [(firefox[0], f'firefox-extension-{version}.xpi'),
                   (chrome[0], f'chrome-extension-{version}.zip')]:
    destination = root / name
    assert not destination.exists(), f'Refusing to overwrite {name}'
    path.rename(destination)
PY
python3 - "$RELEASE_TAG" "$RELEASE_COMMIT" <<'PY'
import hashlib, json, pathlib, sys
files = sorted(p for p in pathlib.Path('release-artifacts').iterdir() if p.is_file())
pathlib.Path('release-artifacts/extension-inventory.json').write_text(json.dumps({
    'tag': sys.argv[1], 'commit': sys.argv[2],
    'artifacts': [{'file': p.name, 'sha256': hashlib.sha256(p.read_bytes()).hexdigest(), 'bytes': p.stat().st_size} for p in files]
}, indent=2) + '\n', encoding='utf-8')
PY
(cd release-artifacts && find . -maxdepth 1 -type f ! -name SHA256SUMS.txt -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS.txt)
