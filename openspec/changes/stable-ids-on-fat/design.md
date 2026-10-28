# Design

## Context

- `ps-scan/src/fs.rs`: `SourceFs::read_dir` stats every entry (`DirEntry::metadata`, no symlink follow) and
  `fingerprint(&Metadata)` builds `StatFingerprint { size, mtime_ns, file_index }`. On Unix `file_index` is
  `Some(m.ino())`; on other platforms (Windows) it is `None`.
- `ps-model/src/ids.rs` `StatFingerprint::unchanged`: size and mtime equal, and the file indexes equal when both
  are `Some`. A missing index on either side is ignored. Its doc comment already names exFAT.
- `ps-scan/src/diff.rs` `diff`: `Some(r) if !r.stat.unchanged(&w.stat)` makes a row changed. The row goes back to
  `discovered` and is read in full.
- `ps-catalog/src/writer.rs` `UPSERT_SQL`: the same test in SQL (`changed`). The `WHERE` clause also rewrites a row
  whose `file_index IS NOT excluded.file_index`, which keeps the stored index current.
- `ps-scan/src/source.rs` `volume_info` (Linux): `st_dev` -> `/proc/self/mountinfo` gives `fs_type` (`exfat`,
  `vfat`, `fuseblk`) and the mount source; `/dev/disk/by-uuid` gives the volume UUID. macOS and Windows return
  `VolumeInfo::default()`. The device hint stores `class=;fs=;dev=`.
- The device number: udisks mounts the card's partition, so `st_dev` is the block device (179:1 for
  `mmcblk0p1`). That is stable for one reader and differs on another reader (`/dev/sdX`). `st_dev` is not part of
  the file fingerprint, and source identity is the volume UUID plus sampled content (ADR-0006). So a changed device
  number changes nothing today; this change only uses it as the probe's cache key within one walk.
- `ps-export/src/quarantine/run.rs`: the pre-move check compares `fsops::file_index_of(md)` with the catalog's
  `file_index` and refuses on a difference.
- Existing seams: `SourceFs::instrumented()` returns an `Instrumentation` that counts opens, bytes read and folder
  listings, and denies opens (`deny_open`). Scan tests run on `ps-fixtures` corpora in temp dirs.

## Decisions

### D1. The probe (new ADR-0112)

`ids_stable(dir, dev) -> IdStability { Stable, Unstable, None }`, cached by `st_dev` for the duration of one walk.
It costs one syscall per distinct device.
- **Linux:** `statfs(dir)` `f_type`:
  - `MSDOS_SUPER_MAGIC` (0x4d44, vfat and msdos) and `EXFAT_SUPER_MAGIC` (0x2011BAB0) -> `Unstable`.
  - `FUSE_SUPER_MAGIC` (0x65735546) -> the mount table's type for that device (`mount_of`): `fuseblk` and
    `fuse.exfat` -> `Unstable`. Other FUSE types (sshfs, rclone) -> `Stable`, as today.
  - Everything else -> `Stable`.
- **macOS:** `statfs` `f_fstypename` `msdos` or `exfat` -> `Unstable`.
- **Windows:** `None` (no index is collected, as today).
- Unknown or failed probe -> `Stable` (today's behaviour). A wrong `Unstable` costs only the inode check. A wrong
  `Stable` is today's re-read.

The classification table is a pure function in `ps-model` (`fs_ids::stable_for_magic`, `stable_for_name`), unit
tested and shared with ps-export. The syscalls stay in the I/O crates: `libc` in ps-scan, which adds it as a unix
target dependency (already in the workspace), and `rustix::fs::statfs` in ps-export, which already depends on it.

### D2. Fingerprint without the index

On `Unstable`, `fingerprint` writes `file_index: None`. Nothing else changes:
- `unchanged` ignores the stored inode.
- The upsert clears it once: one row write per file on the first rescan, batched by the writer (ADR-0005). No file
  is read.
- New rows never store one.

The first rescan after the upgrade is the remount case too, and it reads nothing.

### D3. FAT's whole-hour shift

The owner chose this on 2026-10-27: an exact whole-hour shift with the same size is unchanged.

`vfat`/`msdos` store local time with 2 s resolution and no zone. Linux converts with the kernel time zone (or
`tz=UTC`); Windows and macOS use their local zone. Moving the card between machines, or a daylight-saving change
before a remount on a machine with a local-time RTC, shifts every mtime by whole hours. exFAT stores a UTC offset
and is excluded.

Rule: with `IdStability::Unstable` and fs type vfat/msdos, the size is equal and `|Δmtime|` is an exact multiple of
3600 s, at most 15 h -> unchanged, and the row takes the new mtime (a catalog write, no read). This needs the
filesystem kind at diff time:
- `WalkEntry` carries a `fat_time: bool` from the probe.
- `StatFingerprint::unchanged_with(other, FatTime)` holds the rule, so the Rust diff and the SQL `changed`
  expression stay one rule. The SQL gets the flag as a parameter: `?8 AND size = excluded.size AND
  abs(mtime_ns - excluded.mtime_ns) % 3600000000000 = 0 AND abs(...) <= 54000000000000`.

A replaced file that has the same size and a mtime exactly whole hours off is not detected. Export's copy-verify
(`ps-export/src/copy.rs`, the content check against the catalog's identity) and cleanup's re-hash refuse it, so no
wrong copy or move results. The next real change of the file is detected as usual.

### D4. Test seams (no real devices)

`Instrumentation` gains:
- `force_ids(IdStability)` and `force_fat_time(bool)`: the probe's answer for every device, as the walk sees it.
- `remap_file_index(f: fn(u64) -> u64)`: applied to every `ino` that `read_dir` reports. It simulates a remount
  that hands out new inode numbers.
- `shift_mtime(ns)`: applied to every reported mtime. It simulates FAT's zone shift.

Tests scan a `ps-fixtures` corpus in a temp dir through `SourceFs::instrumented()`, remount it with the seams, and
rescan:
- remount with `Unstable`: 0 opens, 0 bytes, `changed` 0
- the same with `Stable`: every remapped file is changed (proves the seam bites and ext4 behaviour is kept)
- a renamed file: one open, re-identified with its history kept
- a replaced file (different size, and the same size with a new mtime set through `File::set_times`): changed
- an old catalog with stored inodes, walked as `Unstable` with a remap: 0 reads, and the rows' `file_index` NULL
  afterwards
- a FAT shift of +1 h: unchanged, with the new mtime stored; +1 h with exFAT, or +30 min: changed

The pure classification gets a table test. The quarantine pre-move check gets a test through ps-export's existing
fixture moves with a stored index that differs and the probe forced `Unstable`. A real vfat or exFAT mount needs
root, so there is no such test. The owner checks the card by hand (tasks 6.3).

### D5. The source records it

`VolumeInfo.ids: IdStability` is filled from the probe at add, at relink and at each scan's volume check, and stored
in the device hint (`;ids=unstable`). `VolumeInfo::ids_from_hint` reads it back, and `SourceInfo` and
`sources list --json` show `stable_file_ids: true|false|null`. No migration: hints without `ids=` read as unknown.
The Sources panel shows nothing new. The CLI field is for support and tests.

## Risks / Trade-offs

- [A same-size, same-timestamp replacement on FAT is missed] -> as on Windows today; export and cleanup verify
  content, so it is never mishandled, only analysed late.
- [The hour rule hides a real edit] -> only with an identical size and an exact whole-hour shift, on FAT only.
- [A probe misclassifies a filesystem] -> the fallback is today's behaviour; the table is a unit test and the ADR
  lists it.

## Deviations (as built)

- **D3, no SQL copy of the hour rule.** The rule lives once, in `StatFingerprint::unchanged_with` (Rust). An
  unchanged row whose stored fingerprint differs from the walk's (a stale inode, a whole-hour FAT shift) goes into
  `DiffOutput::stat_refresh`, and the scanner writes the walk's mtime and file index with a new writer command,
  `Command::RefreshStats` (`UPDATE file SET mtime_ns, file_index ... AND size = ?`), submitted before the diff's
  upserts. The upsert's own `changed` test then never sees the old values (a returning row with a shifted time is
  not reset), so `UPSERT_SQL` needs no `?8` flag and is unchanged. The same write clears stale inodes (D2) for
  unchanged rows, which the diff did not upsert before. Recorded in ADR-0112, written with this change; not
  architectural beyond it.
- **`rustix` in ps-scan, not `libc`.** `rustix::fs::statfs` (already a workspace dependency, used by ps-export) is
  the probe in both crates, so neither needs `unsafe`. A Unix target dependency of ps-scan; noted in ADR-0112.
- **`IdStability::Unrecorded`** is the name of D1's `None` (it would shadow `Option::None`); its hint value is
  `none` as designed.
- **The FUSE mount-table lookup** is a pure parser in `ps_model::fs_ids` (`mountinfo_entry`, `dev_major_minor`),
  used by `source::imp::mount_of` and by both probes; ps-export does not depend on ps-scan.
- **The scan's volume check** records a changed `ids=` with a new writer command, `Command::SetDeviceHint`; only
  the `ids=` part of the stored hint changes (`VolumeInfo::hint_with_ids`), so the device class recorded at add is
  kept. A hint that is not ours (the synthetic library's) is left alone.
- **The quarantine probe** runs per file (`QuarantineHooks::file_ids`, one `statfs` next to a full re-hash), not
  per run, so a source spanning mounts is checked per file.
- **The seams also apply to the pipeline's post-open `fstat`** (`SourceFs::fingerprint`), so a simulated remount
  does not look like a file changed during the scan.
- **`scenario_sources_exfat_source_shows_unstable_file_ids`** is a ps-scan test (the probe can only be forced
  in-process); the JSON field is covered by the CLI test `sources_list_shows_stable_file_ids`, which sets an exFAT
  hint in the catalog and runs `sources list --json`.
- **Tests set times with `ps_fixtures::scratch::set_mtime`**, not `File::set_times` (ps-scan's tests are under the
  read-only layering check).
