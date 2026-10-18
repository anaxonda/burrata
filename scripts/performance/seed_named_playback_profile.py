#!/usr/bin/env python3
"""Create a fresh disposable native playback profile from public checked-in metadata.
Does not launch an app, contact a provider or read any existing profile.
"""
import argparse
import hashlib
import json
import sqlite3
import tempfile
from datetime import datetime, timezone
from pathlib import Path
import run_isolated_memory as seed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    profile = Path(tempfile.mkdtemp(prefix='burrata-launch.', dir='/tmp'))
    seed.ROWS = 0
    local_id, _ = seed.seed_profile(profile)
    shapes = json.loads((seed.ROOT / 'tests/fixtures/playback/sasa-metadata-shapes.json').read_text())['shapes']
    remote = json.loads((seed.ROOT / 'tests/fixtures/playback/poso-main-catalog.json').read_text())
    records = []
    cases = []
    for shape, suffix, kind in zip(shapes, ['clean', 'merged'], ['separate_fields', 'merged_title']):
        record = seed.track_record(f'fixture:sasa-{suffix}', 'fixture', f'sasa-{suffix}', shape['title'], shape['artist'], shape['durationSeconds'])
        record['version'] = shape['version']
        records.append(record)
        cases.append(dict(trackId=record['id'], metadataShape=kind, expectedDurationSeconds=281))
    records.append(remote)
    db = profile / 'data/org.burrata.discovery/burrata.sqlite'
    with sqlite3.connect(db) as connection:
        for record in records:
            work_id = f"work:{record['id']}"
            connection.execute('INSERT INTO works(id,title,artist) VALUES(?,?,?)', (work_id, record['title'], record['artist']))
            connection.execute('INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)', (record['id'], work_id, record.get('version')))
            connection.execute('INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)',
                (record['id'], record['provider'], record['providerId'], record['id'], record.get('sourceUrl', ''), json.dumps(record), datetime.now(timezone.utc).isoformat()))
        if connection.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise RuntimeError('isolated_fixture_integrity_failed')
    fixture_file = profile / 'fixture/named-cases.json'
    fixture_file.write_text(json.dumps(dict(trackId=local_id, durationSeconds=seed.FIXTURE_SECONDS, sasaCases=cases,
        remoteCase=dict(trackId=remote['id'], expectedDurationSeconds=245.48)), indent=2) + '\n')
    manifest = profile / 'fixture/sasa-manifest.json'
    manifest.write_text(json.dumps(dict(schemaVersion=1, automationFile=str(profile / 'data/org.burrata.discovery/automation.json'),
        binary=str(binary), binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest(), fixtureFile=str(fixture_file)), indent=2) + '\n')
    print(json.dumps(dict(profile=str(profile), controlTrackId=local_id, manifest=str(manifest))))


if __name__ == '__main__':
    main()
