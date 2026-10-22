#!/usr/bin/env python3
"""Controlled in-memory profile lookup benchmark; never opens app/library databases.

Compares the current provider_cache JSON scan with a hypothetical exact-name
projection table. The projection uses ASCII lower/trim for this microbenchmark;
production normalization must be defined consistently with Rust matching.
"""

import argparse
import json
import statistics
import sqlite3
import time

CURRENT_QUERY = """SELECT payload FROM provider_cache
WHERE namespace='artists.profile.musicbrainz'
  AND julianday(expires_at)>julianday('now')
  AND json_valid(payload)
  AND (lower(trim(json_extract(payload,'$.name')))=lower(?1)
    OR EXISTS(SELECT 1 FROM json_each(payload,'$.aliases') a
      WHERE lower(trim(CASE WHEN a.type='object' THEN json_extract(a.value,'$.name')
                            WHEN a.type='text' THEN a.value ELSE NULL END))=lower(?1)))
LIMIT 3"""

INDEXED_QUERY = """SELECT p.payload FROM artist_profile_names a
JOIN provider_cache p ON p.namespace='artists.profile.musicbrainz' AND p.key=a.artist_id
WHERE a.norm_name=lower(trim(?1)) AND julianday(a.expires_at)>julianday('now')
LIMIT 3"""


def timed(connection: sqlite3.Connection, query: str, name: str, iterations: int) -> tuple[float, float]:
    connection.execute(query, (name,)).fetchall()  # warm SQLite's page cache
    samples = []
    for _ in range(iterations):
        start = time.perf_counter()
        connection.execute(query, (name,)).fetchall()
        samples.append((time.perf_counter() - start) * 1000)
    ordered = sorted(samples)
    return statistics.median(samples), ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))]


def run(size: int, iterations: int) -> None:
    db = sqlite3.connect(":memory:")
    db.executescript("""
      CREATE TABLE provider_cache(namespace TEXT NOT NULL,key TEXT NOT NULL,
        payload TEXT NOT NULL,expires_at TEXT NOT NULL,PRIMARY KEY(namespace,key));
      CREATE TABLE artist_profile_names(norm_name TEXT NOT NULL,artist_id TEXT NOT NULL,
        expires_at TEXT NOT NULL,PRIMARY KEY(norm_name,artist_id));
      CREATE INDEX artist_profile_names_lookup
        ON artist_profile_names(norm_name,expires_at,artist_id);
    """)
    profiles = []
    names = []
    expiry = "9999-12-31T23:59:59Z"
    for index in range(size):
        artist_id = f"id-{index}"
        name = f"Performer {index:06d}"
        aliases = [{"name": f"Alias {index:06d}"}, f"Also {index:06d}"]
        payload = json.dumps({"id": artist_id, "name": name, "aliases": aliases}, separators=(",", ":"))
        profiles.append(("artists.profile.musicbrainz", f"{index:09d}", payload, expiry))
        names.extend((value.lower(), artist_id, expiry) for value in (name, aliases[0]["name"], aliases[1]))
    unrelated = [("other.provider", f"other-{index:09d}", "{}", expiry) for index in range(size)]
    db.executemany("INSERT INTO provider_cache VALUES(?,?,?,?)", profiles + unrelated)
    db.executemany("INSERT INTO artist_profile_names VALUES(?,?,?)", names)
    db.commit()

    print(f"profiles={size:,} total_provider_cache_rows={2 * size:,} sqlite={sqlite3.sqlite_version}")
    print("  current plan:", db.execute("EXPLAIN QUERY PLAN " + CURRENT_QUERY, ("missing",)).fetchall())
    print("  indexed plan:", db.execute("EXPLAIN QUERY PLAN " + INDEXED_QUERY, ("missing",)).fetchall())
    probes = ("Alias 000000", f"Alias {size - 1:06d}", "No Such Artist")
    for query in probes:
        current = timed(db, CURRENT_QUERY, query, iterations)
        indexed = timed(db, INDEXED_QUERY, query, iterations)
        print(f"  {query!r}: current median/p95={current[0]:.3f}/{current[1]:.3f} ms; "
              f"projection={indexed[0]:.4f}/{indexed[1]:.4f} ms")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sizes", default="1000,10000,50000", help="comma-separated profile counts")
    parser.add_argument("--iterations", type=int, default=15)
    args = parser.parse_args()
    for size in (int(part) for part in args.sizes.split(",")):
        if size < 1:
            parser.error("sizes must be positive")
        run(size, max(1, args.iterations))


if __name__ == "__main__":
    main()
