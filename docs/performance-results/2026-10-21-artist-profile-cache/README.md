# MusicBrainz artist-profile cache lookup profile

Date: 2026-10-21

This is a read-only profile of `Store::cached_musicbrainz_artist_profiles_for_name`
and an in-memory SQLite microbenchmark for the stage 4.6 alias-lookup work. It
does not measure match accuracy or claim timings for another installation.

## Current local cache

The normal Burrata profile database was opened with SQLite URI `mode=ro`. The
inspection executed only `SELECT` and `EXPLAIN QUERY PLAN` statements. It did not
print or persist cached profile contents.

The `artists.profile.musicbrainz` namespace contained **25 rows**, of which **8
were unexpired** and **0 used the retained `9999-12-31T23:59:59Z` expiry** at
inspection time. Three known public-artist exact-name lookups each returned one
cached row; seven repeated reads had median wall times from **0.725 to 0.943 ms**
(first-run samples ranged from 0.5 to 2.1 ms). With 25 total rows, the current
JSON scan is not a storage-index target for this profile.

SQLite 3.53.4 reports this plan:

```text
SEARCH provider_cache USING INDEX sqlite_autoindex_provider_cache_1 (namespace=?)
CORRELATED SCALAR SUBQUERY
SCAN a VIRTUAL TABLE INDEX 3:
```

The primary key `(namespace,key)` limits the scan to the MusicBrainz namespace.
SQLite still checks expiry, JSON validity, canonical-name JSON, and each alias
array value for every candidate row. There is no name or expiry index.

## Controlled in-memory benchmark

Run `python3 scripts/performance/benchmark_artist_profile_cache.py` to compare
the current query with a hypothetical exact-name projection. It creates an
in-memory database with generated profiles and unrelated provider-cache rows;
it never opens the application database. The projection case is illustrative,
not an implementation proposal by itself. It uses ASCII lower/trim and must not
be adopted until its normalization semantics match the production comparison.

On this host, SQLite 3.53.4, with ten warm query iterations and two aliases per
profile, the observed median times were:

| Cached MB profiles | Current JSON query | Exact-name projection |
| ---: | ---: | ---: |
| 1,000 | 4.7–5.8 ms | 0.007–0.012 ms |
| 10,000 | 57.6–63.3 ms | 0.007–0.012 ms |
| 50,000 | 232.8–304.5 ms | 0.003–0.012 ms |

The current query remained proportional to the number of cached MB profiles for
both hits and misses. The projection plan used a covering exact-name index and a
primary-key lookup into `provider_cache`. Synthetic profile JSON and Python's
SQLite interface differ from production payload sizes, SQLite build/runtime,
device storage, cache warmness, and result consumers. Use these measurements only
to explain the scan's scaling shape, not as a production latency prediction.

## Memoization and index implications

An alias candidate can request cached profiles for the requested artist and the
candidate's primary artist; the same names recur across a candidate batch. A
request-scoped memo of name-query results (including empty and ambiguous results)
can remove repeated SQLite calls while preserving current point-in-time expiry
and ambiguity behavior. It must not become a process-wide inferred-alias cache.

The live local profile has only 25 rows, so a persistent name index is not
justified by current cardinality. Reconsider only if measurements on a larger
representative library show substantial profile scans or batch stalls. Such an
index would need transactional updates for both expiring and retained cache
writes, expiry filtering/cleanup, uniqueness and ambiguity preservation, and
normalization consistent with matching. Current storage SQL uses SQLite
`lower(trim(...))`, while matching uses NFC, Unicode lowercasing, punctuation
normalization, and `&` handling. That difference must be addressed explicitly.
