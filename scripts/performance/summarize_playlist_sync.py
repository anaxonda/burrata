#!/usr/bin/env python3
"""Summarize a profile_playlist_sync capture without reading private app logs."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics


def summarize(directory: Path) -> dict:
    metadata = json.loads((directory / 'profile/metadata.json').read_text())
    samples = json.loads((directory / 'profile/samples.json').read_text())
    phases = json.loads((directory / 'phase-observations.json').read_text())
    groups = {}
    for name in metadata['groups']:
        rows = [row for row in samples if row.get('kind') == 'group' and row.get('group') == name]
        if not rows:
            raise ValueError(f'No group samples for {name}')
        values = sorted(row['cpuPercent'] for row in rows)
        groups[name] = {
            'cpuSeconds': round(sum(row['cpuSeconds'] for row in rows), 6),
            'cpuPercentOneCore': {'median': statistics.median(values),
                                  'p95': values[math.ceil(len(values) * .95) - 1],
                                  'worst': values[-1]},
            'peakRssBytes': max(row['rssBytes'] for row in rows),
        }
    mirrors = []
    healthy = 0
    for item in phases['phases']:
        line = item['line']
        if line.startswith('[perf] playlist_sync.mirror_collections_elapsed '):
            match = re.search(r'elapsed_ms=(\d+)', line)
            if match:
                mirrors.append(int(match[1]))
        healthy += line == '[perf] playlist_sync.watcher_health running=true'
    return {'binarySha256': phases['binarySha256'], 'build': 'development',
            'durationSeconds': metadata['durationSeconds'], 'groups': groups,
            'mirrorCollectionsElapsedMs': mirrors, 'observedHealthyFrontendChecks': healthy,
            'limitations': metadata['limitations']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    report = summarize(args.directory)
    (args.directory / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
