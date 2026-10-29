# Proposal

## Why

The owner rescanned the Canon SD card (exFAT, `/dev/mmcblk0p1`, mounted by udisks) after unplugging it and putting
it back. Nothing on the card had changed, yet the rescan called 1,355 of its 2,632 files "changed" and read 31 GB
again.

The stat fingerprint of ADR-0007 is size + modification time + inode. On FAT and exFAT the inode is not stored on
disk: Linux's `vfat` and `exfat` drivers invent one when they first load each file, so the numbers depend on the
order of access and change across mounts. About half happened to come out the same. ROADMAP row 30 and the README's
"Known limitations" describe the problem. Priority 2 (efficient rescans) says it must not cost a full read.

## What Changes

- **File ids only where the filesystem keeps them.** The walk asks the operating system, once for each device it
  meets, what filesystem holds the files:
  - Linux: `statfs` `f_type`, with the mount table for FUSE mounts.
  - macOS: `statfs` `f_fstypename`.

  On FAT, exFAT and their FUSE drivers the fingerprint records no file index, only size and modification time
  (relative path is the row's key). Windows already records no file index on any filesystem, so nothing changes
  there.
- **Old catalogs need nothing.** A missing file index on either side already counts as "ignore". So the first
  rescan after upgrading compares size and mtime only, reads none of the unchanged files, and clears the stale
  inode numbers from those rows (a catalog write, no file reads).
- **Renamed and replaced files are still found.**
  - A rename is a missing old path plus a new path. The new path is read, and the file is re-identified by content,
    keeping its history (unchanged rule).
  - A file replaced in place differs in size or modification time.
  - A replacement with the same size and the same 2-second FAT timestamp is the one case left. Export and cleanup
    already re-check the content hash, so such a file is never copied or moved under the old identity.
- **FAT's local-time shift.** FAT (not exFAT) stores local time without a zone, so the same file can read an exact
  whole number of hours off after a time-zone or daylight-saving change between mounts. On `vfat`/`msdos` only, a
  difference of a whole number of hours (up to 15) with the same size counts as unchanged, and the row takes the new
  time.
- **The quarantine check follows.** Before moving a file, cleanup compares the catalog's file index with the file's
  current one. That comparison is skipped on the same filesystems, so a remounted card is not refused with "another
  file now has its name". The content re-hash before the move is unchanged.
- **The source says so.** A source records whether its filesystem keeps file ids (`ids=stable|unstable|none` in its
  device hint). `sources list` shows it (`stable_file_ids` in `--json`).

## Capabilities

### Modified Capabilities
- `incremental-rescan`: the fingerprint leaves out the file index where the filesystem does not keep one, and
  tolerates FAT's whole-hour shifts.
- `sources`: a source records and shows whether its filesystem keeps file ids.

## Impact

- `crates/ps-scan/src/fs.rs`:
  - `fingerprint`, `file_index` and `SourceFs::read_dir`, with a per-device cache of the probe
  - a new `ids_stable(path, dev)` probe, Linux and macOS
  - `Instrumentation` test seams (D4)
- `crates/ps-scan/src/source.rs`: `VolumeInfo` (`ids`), `VolumeInfo::hint` and `class_from_hint`'s neighbour
  `ids_from_hint`, Linux `imp::mount_of` (reused for FUSE).
- `crates/ps-scan/src/diff.rs`: the `unchanged` comparison with the FAT hour rule. The comparison itself lives in
  `ps-model/src/ids.rs`: `StatFingerprint::unchanged` gains a `FatTime` variant or flag (D3).
- `crates/ps-catalog/src/writer.rs`: `UPSERT_SQL`'s `changed` condition, the same hour rule in SQL. Its existing
  `file_index IS NOT excluded.file_index` clause already clears stale inodes.
- `crates/ps-export/src/quarantine/run.rs`: the pre-move `fingerprint` closure. `crates/ps-export/src/quarantine/fsops.rs`:
  `file_index_of` and the same probe, through `rustix::fs::statfs`, already a dependency. The post-rename check
  compares two live stats and is unchanged.
- `ps-model`: `fs_ids::stable_for_type` (a pure classification of filesystem magic numbers and names), shared by
  ps-scan and ps-export.
- `photosquirrel-cli` `sources list`: `stable_file_ids`. README: remove the FAT entry from "Known limitations".
- **New ADR-0112** "File ids are used only on filesystems that keep them" (amends 0007). **No migration**: the
  stored file index is ignored when the walk has none, and the device hint is a text field.

## Out of scope

- File ids on Windows (NTFS file index through `GetFileInformationByHandle`): Windows keeps today's
  size + mtime fingerprint.
- SMB mounts with `noserverino` and other network filesystems with invented ids.
- A card mounted at a new mount point (`/run/media/u/CARD1`): the existing re-link flow (ADR-0006).
