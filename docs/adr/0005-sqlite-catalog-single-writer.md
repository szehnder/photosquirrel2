# 0005. SQLite catalog with a single batching writer

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The catalog holds 100k-1M+ files with metadata, hashes, groups, AI results and job state. It must survive crashes (resume), support fast paged queries for the gallery, and be inspectable.

## Decision
The catalog is one SQLite database (`rusqlite`, bundled SQLite) in the app data directory, in WAL mode with `synchronous=NORMAL`. Exactly one **writer thread** owns the write connection and applies batched commands in transactions (flush at 2,000 rows or 100 ms, whichever first). Readers use a small pool of read-only connections. Schema changes are numbered migrations applied at open (`PRAGMA user_version`). Gallery queries use keyset pagination and covering indexes; the UI never runs unbounded queries on its thread.

## Alternatives considered
- **redb / sled / RocksDB** - fast KV but we need ad-hoc relational queries (groups, filters, reports).
- **DuckDB** - great analytics, weaker for many small transactional writes.
- **Postgres + Redis (attempt 1)** - operational burden for a desktop tool.

## Consequences
- Crash safety and resume come from SQLite transactions.
- Write throughput target: >= 50k file rows/s (ADR-0026).

## Revisit when
Write throughput or DB size becomes the scan bottleneck at 1M+ files.
