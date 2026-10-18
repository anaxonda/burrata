#!/usr/bin/env python3
"""Read-only Linux process-tree CPU/IO sampling; never reads args or environment."""
import argparse
import csv
import json
import os
import time
from pathlib import Path


def read_stat(path):
    text = path.read_text()
    fields = text[text.rfind(")") + 2:].split()
    return {
        "parent": int(fields[1]),
        "ticks": int(fields[11]) + int(fields[12]),
        "start": int(fields[19]),
        "rss": int(fields[21]) * os.sysconf("SC_PAGE_SIZE"),
    }


def snapshot(roots, proc=Path("/proc")):
    processes = {}
    for directory in proc.iterdir():
        if directory.name.isdigit():
            try:
                processes[int(directory.name)] = read_stat(directory / "stat")
            except (OSError, ValueError, IndexError):
                pass
    selected = set(roots) & processes.keys()
    while True:
        children = {pid for pid, item in processes.items() if item["parent"] in selected}
        expanded = selected | children
        if expanded == selected:
            break
        selected = expanded
    result = {}
    for pid in selected:
        directory = proc / str(pid)
        try:
            item = processes[pid]
            item["name"] = (directory / "comm").read_text().strip()
            try:
                counters = dict(line.split(": ") for line in (directory / "io").read_text().splitlines())
                item["readBytes"] = int(counters["read_bytes"])
                item["writeBytes"] = int(counters["write_bytes"])
            except (OSError, KeyError, ValueError):
                pass
            result[(pid, 0)] = item
            for thread in (directory / "task").iterdir():
                try:
                    stat = read_stat(thread / "stat")
                    stat["name"] = (thread / "comm").read_text().strip()
                    result[(pid, int(thread.name))] = stat
                except (OSError, ValueError, IndexError):
                    pass
        except OSError:
            pass
    return result


def interval_rows(previous, current, elapsed, at_seconds, hz):
    rows = []
    for key, item in current.items():
        old = previous.get(key)
        # PID/TID reuse must never become an artificial CPU spike.
        if old is None or item["start"] != old["start"]:
            continue
        row = {"atSeconds": round(at_seconds, 3), "pid": key[0], "tid": key[1],
               "name": item["name"], "kind": "process" if key[1] == 0 else "thread",
               "cpuPercent": round(100 * max(0, item["ticks"] - old["ticks"]) / hz / elapsed, 3)}
        if key[1] == 0:
            row["rssBytes"] = item["rss"]
            for field in ("readBytes", "writeBytes"):
                if field in item and field in old:
                    row[field] = max(0, item[field] - old[field])
        rows.append(row)
    return rows


def group_row(rows, elapsed, at_seconds, name, operation_id=None):
    """Aggregate process rows only; process CPU already includes its own threads."""
    processes = [row for row in rows if row['kind'] == 'process']
    row = {"atSeconds": round(at_seconds, 3), "kind": "group", "group": name,
           "cpuPercent": round(sum(item['cpuPercent'] for item in processes), 3),
           "cpuSeconds": round(sum(item['cpuPercent'] for item in processes) * elapsed / 100, 6),
           "rssBytes": sum(item.get('rssBytes', 0) for item in processes),
           "readBytes": sum(item.get('readBytes', 0) for item in processes),
           "writeBytes": sum(item.get('writeBytes', 0) for item in processes)}
    if operation_id:
        row['operationId'] = operation_id
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, action="append", default=[])
    parser.add_argument("--group", action="append", default=[], metavar="NAME=PID",
                        help="name a separately sampled process tree (repeatable; avoid overlapping roots)")
    parser.add_argument("--operation-id", help="opaque attempt ID to label samples; no app correlation is inferred")
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--interval", type=float, default=0.25)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    groups = {}
    try:
        for item in args.group:
            name, pid = item.rsplit('=', 1)
            groups[name] = [int(pid)]
    except ValueError:
        parser.error("--group must be NAME=PID")
    if args.pid:
        groups['process-group'] = args.pid
    if not groups or any(not name or any(pid <= 0 for pid in pids) for name, pids in groups.items()):
        parser.error("supply positive --pid values or --group NAME=PID")
    if args.seconds <= 0 or not 0.05 <= args.interval <= 5:
        parser.error("positive PIDs/duration and interval between 0.05 and 5 seconds required")
    if not Path("/proc/self/stat").exists():
        parser.error("Linux /proc is required")
    args.output.mkdir(parents=True, exist_ok=True)
    previous = {name: snapshot(pids) for name, pids in groups.items()}
    if any(not all((pid, 0) in previous[name] for pid in pids) for name, pids in groups.items()):
        parser.error("a root process is missing or inaccessible")
    start = last = time.monotonic()
    rows = []
    while last - start < args.seconds:
        time.sleep(min(args.interval, max(0, args.seconds - (last - start))))
        current = {name: snapshot(pids) for name, pids in groups.items()}
        now = time.monotonic()
        elapsed = now - last
        for name in groups:
            group_samples = interval_rows(previous[name], current[name], elapsed, now - start,
                                          os.sysconf("SC_CLK_TCK"))
            if args.operation_id:
                for row in group_samples:
                    row['operationId'] = args.operation_id
            for row in group_samples:
                row['group'] = name
            rows.extend(group_samples)
            rows.append(group_row(group_samples, elapsed, now - start, name, args.operation_id))
        previous, last = current, now
    (args.output / "samples.json").write_text(json.dumps(rows, indent=2) + "\n")
    columns = ["atSeconds", "pid", "tid", "kind", "group", "name", "operationId",
               "cpuPercent", "cpuSeconds", "rssBytes", "readBytes", "writeBytes"]
    with (args.output / "samples.csv").open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns)
        writer.writeheader()
        writer.writerows(rows)
    metadata = {"durationSeconds": last - start, "intervalSeconds": args.interval,
                "groups": groups, "operationId": args.operation_id,
                "cpuPercentBasis": "100 percent is one logical core",
                "scope": "each named root and observed descendants; group rows sum process rows only; process rows include their threads, so do not add thread rows",
                "limitations": "Avoid overlapping named root trees. Processes born and exited between polls and their CPU are omitted. Newly observed tasks need a second sample. Group RSS sums resident sizes and can double-count shared pages. IO bytes are Linux storage counters, not network transfer. Operation ID is a caller label, not inferred app/IPC correlation."}
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Saved {len(rows)} samples to {args.output}")


if __name__ == "__main__":
    main()
