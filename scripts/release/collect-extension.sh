#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?}"
: "${RELEASE_COMMIT:?}"
mkdir -p release-artifacts
test -n "$(find release-artifacts -type f -name '*.xpi' -print -quit)" || { echo 'AMO did not produce a signed Firefox XPI' >&2; exit 1; }
mapfile -t chromium_zips < <(find release-artifacts -maxdepth 1 -type f -name 'burrata-chromium.zip' -print)
test "${#chromium_zips[@]}" -gt 0 || { echo 'Chromium extension ZIP is missing' >&2; exit 1; }
python3 - <<'PY'
import pathlib, zipfile
for path in pathlib.Path('release-artifacts').glob('*.xpi'):
    with zipfile.ZipFile(path) as archive:
        names = {name.lower() for name in archive.namelist()}
        required = {'meta-inf/mozilla.rsa', 'meta-inf/mozilla.sf', 'meta-inf/manifest.mf'}
        if not required.issubset(names):
            raise SystemExit(f'Firefox package lacks AMO signature files: {path.name}')
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
