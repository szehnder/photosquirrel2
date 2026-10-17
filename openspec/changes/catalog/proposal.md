# Proposal

## Why

PhotoSquirrel must remember everything it has seen across hours-long scans, crashes and months between runs:
sources, files, stages, metadata, hashes, groups, AI results, exports. Scans must resume from the last
consistent checkpoint, and the gallery needs the full ordered list of a million IDs in a fraction of a second.
The catalog is the durable heart of priorities 1 and 2.

## What Changes

- One SQLite catalog in the app data directory, WAL mode, `synchronous=NORMAL`, numbered migrations
  (`PRAGMA user_version`) applied at open; newer-than-supported catalogs refused (ADR-0005).
- A single batching writer thread (flush at 2,000 rows or 100 ms) with typed commands; a small read-only
  connection pool for queries.
- Initial schema: sources, files (keyed by source + relative path, stat fingerprint, stage, class, reasons,
  identity, perceptual hashes, metadata), scan runs, file errors, and placeholder tables for groups, AI and the
  export ledger added by later changes through migrations.
- Atomic checkpoints: a file's stage is written in the same transaction as its results (ADR-0008).
- Query API: complete ordered ID list plus 16-byte sort key for a filter + sort (ADR-0018), keyset pages of 512.
- Benchmarks: >= 50k file rows/s insert; ID-list query for 1M items <= 300 ms (ADR-0026).

## Capabilities

### New Capabilities
- `catalog`: durable, crash-safe storage and fast querying of everything PhotoSquirrel knows about sources and files.

### Modified Capabilities
- None.

## Impact

- ADRs: 0005, 0006, 0007, 0008, 0018, 0025, 0026, 0031.
- Crates: `ps-model` (row/command types, filter and sort model), `ps-catalog` (schema, migrations, writer,
  readers, queries, benches).

## Out of scope

- The scan pipeline that produces rows (`scanning`); group, AI, ledger schemas (migrations in their changes).
- The thumbnail store (`thumbs.db`, delivered in `media-analysis`).
- Catalog export/import, multi-catalog support, remote/shared catalogs.
