#!/usr/bin/env python3
"""Synthetic comparison of the former hydrate-all library search with SQL paging.

This isolates the SQLite/JSON filtering and per-row hydration-query work in a
repeatable synthetic database. It is a microbenchmark, not an end-to-end Rust
or user-library measurement.
"""
import json
import sqlite3
import statistics
import time

TRACKS = 20_000
PAGE_SIZE = 50
TERMS = ("kerri", "remix")


def seed(db):
    db.executescript("""
      CREATE TABLE recordings(id TEXT PRIMARY KEY, favorite INTEGER NOT NULL DEFAULT 0,
        disposition TEXT, manual_metadata TEXT NOT NULL DEFAULT '{}');
      CREATE TABLE catalog_listings(id TEXT PRIMARY KEY, recording_id TEXT NOT NULL,
        metadata TEXT NOT NULL);
      CREATE TABLE local_assets(id TEXT PRIMARY KEY, recording_id TEXT NOT NULL,
        path TEXT NOT NULL, hash TEXT NOT NULL, payload TEXT NOT NULL);
    """)
    records = []
    for i in range(TRACKS):
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


def measure(fn, repetitions=5):
    samples = []
    result = None
    for _ in range(repetitions):
        start = time.perf_counter()
        result = fn()
        samples.append(time.perf_counter() - start)
    return statistics.median(samples), result


def main():
    db = sqlite3.connect(":memory:")
    db.execute("PRAGMA temp_store=MEMORY")
    seed(db)
    old_seconds, old_result = measure(lambda: old_search(db))
    new_seconds, new_result = measure(lambda: sql_search(db))
    assert old_result[:2] == new_result[:2], "search implementations returned different pages"
    print(f"database: in-memory SQLite; rows: {TRACKS:,}; matches: {old_result[0]:,}; page: {PAGE_SIZE}")
    print(f"median of 5 runs, seconds: old hydrate-all={old_seconds:.4f}; SQL page={new_seconds:.4f}")
    print(f"speedup: {old_seconds / new_seconds:.1f}x; per-track hydration reads: {old_result[2]:,} -> {new_result[2]:,}")


if __name__ == "__main__":
    main()
