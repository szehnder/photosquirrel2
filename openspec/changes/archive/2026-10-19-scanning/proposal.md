# Proposal

## Why

The user's photos are spread over many USB drives, SSDs and NAS shares, 100k-1M+ files, re-plugged at different
mount points over the years. Scanning must be read-only, fast on every device class, survive sleep, crashes and
unplugged drives without repeating work, and make rescans of a known drive nearly free. This is priority 2.

## What Changes

- **Sources**: add a root folder; record root path, volume identity, size and a 64-file content fingerprint;
  re-link remounted drives by fingerprint; fold overlapping sources; offline sources stay browsable (ADR-0006).
- **Scan pipeline**: walk -> probe -> classify -> analyze (single read: identity, metadata, decode, perceptual
  hashes, thumbnail) -> persist, with persisted stages and skipped/failed records (ADR-0008, ADR-0010).
- **I/O lanes**: plain threads, bounded queues, per-source adaptive reader pools (1-16), directory-order reads,
  a shared CPU pool of cores minus one, 512 MB global buffer cap (ADR-0009).
- **Incremental rescan**: stat-fingerprint diff (new/changed/unchanged/missing) reported before analysis; moved
  files re-identified by content; hash-version bumps re-hashed from the thumbnail store (ADR-0007, ADR-0012).
- **Scan control**: runs with statuses, cooperative pause/cancel within 250 ms, resume, interrupted-run detection,
  bounded retries for failed files (ADR-0008).
- **Progress snapshots**: atomic counters plus recent ring, sampled at most every 100 ms (ADR-0019).
- `ps-service` facade methods for all of the above (ADR-0027).
- Benchmarks: no-change rescan of 100k files <= 4 s; analysis >= 150 files/s of 12 MP JPEG on 16 cores;
  pause/cancel latency <= 250 ms (ADR-0026).

## Capabilities

### New Capabilities
- `sources`: registering, identifying, re-linking and folding the folders and drives that are scanned.
- `scanning`: the read-only, staged, multi-lane pipeline that turns files on disk into catalogued, analyzed items.
- `incremental-rescan`: rescanning a known source by reading only what is new or changed.
- `scan-control`: starting, pausing, cancelling, resuming and recovering scan runs.

### Modified Capabilities
- None.

## Impact

- ADRs: 0004, 0005, 0006, 0007, 0008, 0009, 0010, 0011, 0012, 0015, 0016, 0019, 0025, 0026, 0027.
- Crates: `ps-model` (source, run, stage, progress snapshot types), `ps-scan` (walker, lanes, pipeline, progress,
  control), `ps-catalog` (source/run queries, diff support), `ps-service` (sources and scans API).

## Out of scope

- Duplicate grouping (`duplicate-groups`), AI stage (`ai-insights`), UI dashboard (`scan-dashboard`).
- Directory-mtime quick mode and OS change journals (ADR-0007 alternatives; later `quick-rescan-dir-mtime`).
- io_uring/IOCP (ADR-0009 revisit).
