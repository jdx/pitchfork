# Structured Log Performance

Pitchfork stores structured logs in SQLite and parses them once at ingestion time. This document compares pitchfork's retrieval performance against [hl](https://github.com/pamburus/hl), a terminal log viewer that parses logs on every read and uses multiple cores for parallel processing.

Benchmarks were run on 100,000 JSON log lines, each approximately 300 bytes with 8 fields.

## Single-Core Comparison

| Scenario | pitchfork (SQLite) | hl (file+parse) |
|---|---|---|
| Full retrieval + render | 135ms | 117ms |
| Level=error filter | 40ms | 75ms |

## Multi-Core Comparison (16 cores)

| Scenario | pitchfork (SQLite) | hl (multi-core) |
|---|---|---|
| Full retrieval + render | 135ms | 20ms |
| Level=error filter | 40ms | 15ms |

## Column Selectivity (include_structured)

Pitchfork can avoid reading structured columns when only the raw message is needed:

| Scenario | pitchfork (message only) | pitchfork (all structured cols) |
|---|---|---|
| Full retrieval | 61ms | 101ms |
| Level=error filter | 36ms | — |

## Design Differences

| | pitchfork | hl |
|---|---|---|
| Parsing | Once, at ingestion | On every read |
| Storage | SQLite with column-oriented structured fields | Plain text files |
| Query execution | SQLite indexes and column reads | Multi-core striped pipeline |
| Best for | Persistent storage and repeated queries | One-off streaming inspection |

## Optimization Notes

Several changes contributed to pitchfork's structured log performance:

1. `include_structured` — Select only the columns that are needed. Skipping the four structured columns when they are not used reduced retrieval time by about 40%.
2. `BufWriter` — Batch small writes into fewer syscalls when outputting results.
3. `PRAGMA mmap_size` — Memory-map the database file to reduce read system calls.
4. `write_log_line` — Write directly to a `BufWriter` instead of building a temporary `String` with `format!`.
5. Streaming output — Stream results directly from `LogEntry` to the output without collecting an intermediate `Vec`.
6. Parallel sharding — For large queries (200,000 rows or more), split the query by `id` range and run each shard on a separate thread.
