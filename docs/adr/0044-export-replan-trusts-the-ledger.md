# 0044. An export re-plan trusts the ledger: size and modification time, no re-hash

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
To find the keepers already in the library, the planner read every byte of every library copy the ledger
records (`FsLibrary::identical`), and the Export view plans on open and on every profile change. With a library
of 100k photos (hundreds of GB, often on a USB disk or a NAS) that is a full re-read of the library each time the
view opens, while the completion report (`check_library`) already trusts a ledger row whose copy has its recorded
size. The ledger knew the copy's size and hash, but not its modification time, so a same-size rewrite in place
could only be noticed by hashing.

## Decision
- Ledger rows record the library copy's modification time (`export_ledger.library_mtime_ns`, catalog migration 9),
  read back from the file after the verified rename (or the adoption), so file systems with coarse timestamps
  store what they will report.
- One check, `ps_export::library::check_copy`, serves the planner and the completion report. A `done` row's copy
  that is absent is missing; one with another size is changed, without reading it; one with its recorded size and
  modification time (or a row from before migration 9, which has no time) is trusted without reading it. Only a
  copy whose time changed while its size did not is re-hashed, to tell a touched file from a rewritten one.
- A full re-hash of every recorded copy happens only on request: `export dry-run|run --verify`, `report --verify`
  and the Reports view's Verify, or for rows that lack the hash to compare (the catalog identity is used then).
- Budget `export-replan-50k` (perf-budgets.toml): re-planning a library that holds 50,000 exported files, all in
  the ledger, takes at most 800 ms (median of 3, release, reference machine) and reads no library file. Measured
  187 ms for the files alone and 398 ms once each bundle's sidecar is in the ledger too (ADR-0045), on NVMe. Measured in
  `ps-export/tests/budgets.rs`; `scenario_export_re_plan_reads_no_file_contents` counts the reads.

## Alternatives considered
- **Keep hashing on every plan** - correct but makes opening the Export view cost a full library read.
- **Trust the size alone** (what reports did) - misses a same-size rewrite; the time check costs nothing extra
  (the planner stats the copy anyway) and narrows re-hashing to the files that were touched.
- **Cache hashes by inode** - another store to keep consistent; the ledger already is that store.

## Consequences
- Re-planning costs one `stat` per exported file. Bytes that change while size and time are restored by hand are
  only found by `--verify`, as with any backup tool's quick check.
- Rows recorded before migration 9 are trusted on their size until the content is exported again.

## Revisit when
A library file system reports unstable modification times (some network mounts), so that copies are re-hashed
on most plans.
