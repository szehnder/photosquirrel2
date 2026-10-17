# Design

## Context

Builds the I/O engine on top of `media-analysis`, `content-hashing` and `catalog`. Architecture is fixed by
ADR-0004 (read-only sources), ADR-0006 (sources/volume identity), ADR-0007 (stat-fingerprint rescans), ADR-0008
(staged, resumable pipeline), ADR-0009 (threads, lanes, back-pressure), ADR-0010 (probe then single read),
ADR-0011/0012 (identities and hashes), ADR-0016 (thumbnail store), ADR-0019 (snapshots) and ADR-0027 (facade).
The ground-truth corpus (ADR-0025) is the test bed.

## Goals / Non-Goals

**Goals:**
- Correctness under cancel/kill at any instant; read-only by construction with a test that proves it.
- Budgets: 150 files/s analysis, 4 s no-change rescan of 100k, 250 ms control latency (ADR-0026).

**Non-Goals:**
- Grouping (runs after the scan in `duplicate-groups`), AI queue, UI.

## Decisions

- **Crates.** `ps-scan` (I/O crate): `source` (add, fingerprint, relink, fold), `walk` (listing + stat with
  `std::fs::read_dir`, per-directory sorted entries, symlinks not followed, the quarantine folder name reserved by
  ADR-0024 ignored), `lane` (reader pools), `pipeline` (stage graph), `progress`, `control`. `ps-service::scans`
  and `ps-service::sources` wrap them (ADR-0027). `ps-model` gains `SourceId`, `SourceStatus`, `RunId`,
  `RunStatus`, `Stage`, `ProgressSnapshot`.
- **Read-only file layer.** All source access goes through `ps-scan::fs::SourceFs`, whose only open path is
  `OpenOptions::new().read(true)`; a test-only `InstrumentedFs` records every open and byte count (used by the
  "never opens for writing", "junk costs only a probe" and "one full read" scenarios). Writing helpers do not
  exist in this module; `cargo xtask layering` is extended with a grep check that `ps-scan` never calls
  `OpenOptions::write`, `File::create`, `fs::remove_*`, `fs::rename` or `set_permissions` (ADR-0004).
- **Volume identity (ADR-0006).** Linux: `/dev/disk/by-uuid` lookup from `st_dev` via `/proc/self/mountinfo`;
  macOS: `statfs` + DiskArbitration UUID via `diskutil info -plist` fallback; Windows: `GetVolumeInformationW`
  serial. Device class: Linux `/sys/block/<dev>/queue/rotational`, macOS IOKit "Solid State" via `diskutil`,
  Windows `IOCTL_STORAGE_QUERY_PROPERTY` seek penalty; network filesystems -> unknown. Fingerprint: 64 files
  chosen by a hash of their relative paths (stable selection), each hashed with the content identity.
- **Pipeline (ADR-0009/0010).** Walk thread per source -> bounded channel -> lane readers (probe 256 KB; classify;
  skip junk; else full read into a pooled buffer or stream > 64 MB) -> bounded channel -> `rayon` pool of
  `available_parallelism - 1` doing identity, metadata, analysis decode, hashes, thumbnail -> writer
  (`ps-catalog::writer`) and `ps-catalog::thumbs`. The probe prefix is reused as the head of the full read so
  the probe bytes are not read twice where the OS allows a contiguous read.
- **Lane controller.** Every 2 s window: MB/s and p90 read latency; grow by 1 if MB/s improved > 10% and
  latency did not rise > 25%; shrink by 1 if latency rose > 50%; clamp 1..16. A simulated device trait drives
  the lane tests.
- **Memory.** A global semaphore of bytes (512 MB) plus per-reader slot count 2 (ADR-0009).
- **Stages and checkpoints (ADR-0008).** `discovered` rows are written by the walk diff, `probed` with probe
  results and class, `analyzed` with identity/metadata/hashes, `thumbed` with the thumb write acknowledged. The
  thumbnail is written to `thumbs.db` before the `thumbed` stage is committed in the catalog.
- **Rescan (ADR-0007).** The walker produces `(rel_path, size, mtime_ns, file_index)`; the diff is a merge-join
  against `file` rows ordered by `rel_path`. Moves: after analysis a new file whose full identity matches a
  missing file in the same source takes over that row (rel_path updated, id kept). Hash upgrade: a job reading
  `thumbs.db` rows whose `hash_version` is stale.
- **Control (ADR-0008).** `CancelToken` (`AtomicU8` state: run/pause/cancel) checked between files and between
  1 MB read chunks; pause parks threads on a `parking_lot::Condvar`. Stop = drain channels, `Flush` the writer,
  set status. Start-up marks `running` runs `interrupted`. Retry: `attempts` column; skip files with
  `attempts >= 3` and stage failed.
- **Source lost mid-scan.** I/O errors of kind NotFound on the root -> lane stops, source status offline, run
  summary lists it as incomplete. Run statuses are unchanged (ADR-0008); "incomplete sources" is a field of the
  run summary, not a new status.
- **Progress (ADR-0019).** `AtomicU64` counters per stage and per source, a `parking_lot::Mutex<Recent>` ring
  (64 photos, 32 errors), per-reader current folder, 1 s throughput samples; `snapshot()` copies them.
- **Benches.** `ps-scan/tests/budgets.rs`: `scan-analysis-12mp` (files/s), `rescan-nochange-100k` (s),
  `control-latency-p95` (ms); registered in `perf-budgets.toml` with `source.kind = "test"`.

## Risks / Trade-offs

- [Run status for "source lost mid-scan" is not covered by ADR-0008's status list] -> modelled as a completed run
  with incomplete sources listed; no new status. Flagged in the change report.
- [Network shares report no file index and coarse mtimes] -> fingerprint falls back to (size, mtime); documented.
- [Kill-test determinism] -> compare final catalogs after normalising timestamps and run ids.
- [Device-class detection is heuristic] -> wrong guesses are corrected by the adaptive controller.

## Verification

- `cargo test -p ps-scan -p ps-service` (all `scenario_sources_*`, `scenario_scanning_*`,
  `scenario_incremental_rescan_*`, `scenario_scan_control_*`) on generated corpora in temp dirs.
- `cargo test --release -p ps-scan --test budgets -- --nocapture` and `cargo xtask bench --check`.
- `cargo xtask layering` (read-only grep rule), `cargo xtask ci`, `openspec validate scanning --strict`.
