#!/usr/bin/env python3
"""Synthetic comparison of the former hydrate-all library search with SQL paging.

This isolates the SQLite/JSON filtering and per-row hydration-query work in a
repeatable synthetic database. It is a microbenchmark, not an end-to-end Rust
or user-library measurement.
"""
import json
import argparse
import math
import sqlite3
import statistics
import time

TRACKS = 20_000
PAGE_SIZE = 50
TERMS = ("kerri", "remix")


def seed(db, tracks):
    db.executescript("""
      CREATE TABLE recordings(id TEXT PRIMARY KEY, favorite INTEGER NOT NULL DEFAULT 0,
        disposition TEXT, manual_metadata TEXT NOT NULL DEFAULT '{}');
      CREATE TABLE catalog_listings(id TEXT PRIMARY KEY, recording_id TEXT NOT NULL,
        metadata TEXT NOT NULL);
      CREATE TABLE local_assets(id TEXT PRIMARY KEY, recording_id TEXT NOT NULL,
        path TEXT NOT NULL, hash TEXT NOT NULL, payload TEXT NOT NULL);
    """)
    records = []
    for i in range(tracks):
        match = i % 1000 == 0
        track_id = f"track:{i}"
        track = {
            "id": track_id, "artist": f"Artist {i}", "title": f"Track {i}",
            "album": "Synthetic Album", "version": "Extended Remix" if match else "Original Mix",
            "remixers": ["Kerri Chandler"] if match else [],
        }
        records.append((track_id, track_id, json.dumps(track, separators=(",", ":"))))
    db.executemany("INSERT INTO recordings(id) VALUES(?)", ((row[0],) for row in records))
    db.executemany("INSERT INTO catalog_listings(id,recording_id,metadata) VALUES(?,?,?)", records)
    db.commit()


def hydrate_work(db, track_id):
    # Mirrors the per-track database reads performed by Store::hydrate.
    db.execute("SELECT r.favorite,r.disposition,r.manual_metadata FROM recordings r "
               "JOIN catalog_listings l ON l.recording_id=r.id WHERE l.id=?", (track_id,)).fetchone()
    db.execute("SELECT id,path,hash,payload FROM local_assets WHERE recording_id=? ORDER BY rowid ASC",
               (track_id,)).fetchall()


def old_search(db):
    rows = db.execute("SELECT l.id,l.metadata FROM catalog_listings l WHERE l.id=l.recording_id "
                      "ORDER BY l.rowid DESC").fetchall()
    matches = []
    hydration_count = 0
    for track_id, raw in rows:
        track = json.loads(raw)
        hydrate_work(db, track_id)
        hydration_count += 1
        haystack = " ".join((track["artist"], track["title"], track.get("album") or "",
                             track.get("version") or "", " ".join(track.get("remixers") or []))).lower()
        if all(term in haystack for term in TERMS):
            matches.append(track_id)
    return len(matches), matches[:PAGE_SIZE], hydration_count


def sql_search(db):
    fields = ("lower(COALESCE(json_extract(l.metadata,'$.artist'),'' ) || ' ' || "
              "COALESCE(json_extract(l.metadata,'$.title'),'' ) || ' ' || "
              "COALESCE(json_extract(l.metadata,'$.album'),'' ) || ' ' || "
              "COALESCE(json_extract(l.metadata,'$.version'),'' ) || ' ' || "
              "COALESCE(json_extract(l.metadata,'$.remixers'),''))")
    predicate = " AND ".join(f"instr({fields},?)>0" for _ in TERMS)
    count = db.execute(f"SELECT COUNT(*) FROM catalog_listings l WHERE l.id=l.recording_id AND {predicate}",
                       TERMS).fetchone()[0]
    rows = db.execute(f"SELECT l.id,l.metadata FROM catalog_listings l WHERE l.id=l.recording_id AND {predicate} "
                      "ORDER BY l.rowid DESC LIMIT ? OFFSET ?", (*TERMS, PAGE_SIZE, 0)).fetchall()
    for track_id, raw in rows:
        json.loads(raw)
        hydrate_work(db, track_id)
    return count, [row[0] for row in rows], len(rows)


def percentile95(samples):
    """Nearest-rank p95, stable for small sample counts."""
    return sorted(samples)[max(0, math.ceil(0.95 * len(samples)) - 1)]


def measure(fn, warmups, repetitions):
    for _ in range(warmups):
        fn()
    wall_samples = []
    cpu_samples = []
    result = None
    for _ in range(repetitions):
        wall_start = time.perf_counter()
        cpu_start = time.process_time()
        result = fn()
        cpu_samples.append(time.process_time() - cpu_start)
        wall_samples.append(time.perf_counter() - wall_start)
    return {
        "wall_median_seconds": statistics.median(wall_samples),
        "wall_p95_seconds": percentile95(wall_samples),
        "wall_worst_seconds": max(wall_samples),
        "cpu_median_seconds": statistics.median(cpu_samples),
        "cpu_p95_seconds": percentile95(cpu_samples),
        "cpu_worst_seconds": max(cpu_samples),
        "result": result,
    }


def run_scale(tracks, warmups, repetitions):
    db = sqlite3.connect(":memory:")
    db.execute("PRAGMA temp_store=MEMORY")
    seed(db, tracks)
    old = measure(lambda: old_search(db), warmups, repetitions)
    new = measure(lambda: sql_search(db), warmups, repetitions)
    old_result, new_result = old.pop("result"), new.pop("result")
    failures = int(old_result[:2] != new_result[:2])
    if failures:
        raise AssertionError("search implementations returned different pages")
    return {
        "rows": tracks,
        "matches": old_result[0],
        "page_size": PAGE_SIZE,
        "warmups": warmups,
        "repetitions": repetitions,
        "failures": failures,
        "old_hydration_reads": old_result[2],
        "sql_hydration_reads": new_result[2],
        "old_hydrate_all": old,
        "sql_paged": new,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--counts", nargs="+", type=int, default=[TRACKS],
                        help="synthetic row counts (default: 20000)")
    parser.add_argument("--warmups", type=int, default=0,
                        help="untimed runs per implementation and size")
    parser.add_argument("--repetitions", type=int, default=5,
                        help="timed runs per implementation and size")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    args = parser.parse_args()
    if any(count < 1 for count in args.counts) or args.warmups < 0 or args.repetitions < 1:
        parser.error("counts and repetitions must be positive; warmups cannot be negative")
    results = [run_scale(count, args.warmups, args.repetitions) for count in args.counts]
    report = {
        "workload": "synthetic in-memory SQLite and JSON library discovery/hydration",
        "python": __import__("platform").python_version(),
        "measurements": results,
        "limitations": [
            "does not execute Burrata Rust Store code or the desktop application",
            "does not measure renderer, IPC, real user libraries, filesystem, or steady-state app memory",
            "process CPU is this benchmark process only, not application CPU",
        ],
    }
    if args.json:
        print(json.dumps(report, indent=2))
        return
    print(f"workload: {report['workload']}")
    print(f"warmups: {args.warmups}; repetitions: {args.repetitions}; failures: 0")
    for item in results:
        old, new = item["old_hydrate_all"], item["sql_paged"]
        print(f"rows={item['rows']:,}; matches={item['matches']:,}; hydration reads={item['old_hydration_reads']:,}->{item['sql_hydration_reads']:,}")
        print("  old hydrate-all: wall median/p95/worst "
              f"{old['wall_median_seconds']:.4f}/{old['wall_p95_seconds']:.4f}/{old['wall_worst_seconds']:.4f}s; "
              "CPU median/p95/worst "
              f"{old['cpu_median_seconds']:.4f}/{old['cpu_p95_seconds']:.4f}/{old['cpu_worst_seconds']:.4f}s")
        print("  SQL paged:       wall median/p95/worst "
              f"{new['wall_median_seconds']:.4f}/{new['wall_p95_seconds']:.4f}/{new['wall_worst_seconds']:.4f}s; "
              "CPU median/p95/worst "
              f"{new['cpu_median_seconds']:.4f}/{new['cpu_p95_seconds']:.4f}/{new['cpu_worst_seconds']:.4f}s")


if __name__ == "__main__":
    main()
