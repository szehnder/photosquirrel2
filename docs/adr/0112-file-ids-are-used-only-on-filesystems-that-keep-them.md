# 0112. File ids are used only on filesystems that keep them

- Status: accepted
- Date: 2026-10-27
- Deciders: maintainers
- Amends: [0007](0007-incremental-rescan-by-stat-fingerprint.md) (the stat fingerprint's file index)

## Context
ADR-0007's stat fingerprint is size + modification time + inode/file index. FAT and exFAT store no inode: Linux's
`vfat` and `exfat` drivers (and the FUSE exFAT drivers) invent one when they first load a file, so the numbers depend
on access order and change across mounts. The owner rescanned the Canon SD card (exFAT) after a remount with
nothing changed, and 1,355 of its 2,632 files were "changed": 31 GB read again. FAT (not exFAT) also stores local
time without a zone, so a card moved between machines or remounted after a daylight-saving change can read every
modification time shifted by whole hours.

## Decision
- **The walk asks the OS once per device what holds the files** (cached by `st_dev` for one walk; one `statfs` per
  distinct device):
  - Linux: `statfs` `f_type`. `MSDOS_SUPER_MAGIC` (0x4d44: vfat, msdos) and `EXFAT_SUPER_MAGIC` (0x2011BAB0) are
    *unstable*. `FUSE_SUPER_MAGIC` (0x65735546) is looked up in `/proc/self/mountinfo` by device number: `fuseblk`
    and `fuse.exfat` are unstable; other FUSE types (sshfs, rclone) are stable, as before.
  - macOS: `statfs` `f_fstypename` `msdos` or `exfat` is unstable.
  - Windows: no file index is collected (as before), recorded as `none`.
  - Everything else, and a failed probe, is *stable* (the behaviour before this ADR). A wrong "unstable" costs only
    the inode check; a wrong "stable" is the re-read of before.
  The classification is one pure table in `ps_model::fs_ids` (unit tested), used by `ps-scan` and `ps-export`; the
  syscalls (`rustix::fs::statfs`, already a workspace dependency) stay in those I/O crates.
- **On an unstable filesystem the fingerprint has no file index** (size + mtime only; the relative path is the
  row's key). The diff already ignores a missing index on either side, so an old catalog's first rescan reads none
  of the unchanged files.
- **FAT's whole-hour shift** (vfat/msdos only, not exFAT, which stores a UTC offset): an equal size and a
  modification time an exact non-zero multiple of 3,600 s away, at most 15 h, is unchanged
  (`StatFingerprint::unchanged_with(.., FatTime)`). The owner chose this rule on 2026-10-27.
- **Stored stats follow the walk without a read.** When the diff finds a row unchanged under these rules but its
  stored fingerprint differs (a stale inode, a shifted mtime), the scanner writes the walk's mtime and file index to
  the row (`Command::RefreshStats`, before the diff's other writes, so the upsert's own `changed` test never sees
  the old values). The rule lives once, in Rust; `UPSERT_SQL` is unchanged.
- **Cleanup's pre-move check** compares the catalog's file index with the file's only on a stable filesystem
  (probed per file through `QuarantineHooks::file_ids`). Size, modification time and the content re-hash are
  checked as before.
- **The source records it**: `ids=stable|unstable|none` in its device hint (a text field: no migration), written at
  add, re-link and at the start of each scan of an online source when it differs. `sources list --json` shows
  `stable_file_ids: true|false|null` (null: not recorded, or Windows).

## Alternatives considered
- **Hash the first bytes of every file on FAT** - a read per file, which is what this avoids.
- **Derive a FAT file id from the directory entry's cluster** - not exposed by the Linux drivers or macOS.
- **Drop the file index everywhere** - loses the replaced-in-place check on ext4, APFS and NTFS, which keep ids.
- **Put the hour rule in `UPSERT_SQL` as a parameter** - two copies of one rule (Rust and SQL) to keep equal; the
  stat refresh before the upserts makes the SQL copy unnecessary.

## Consequences
- A remounted card, and the first rescan after upgrading, read nothing; the stale inodes are cleared by one batched
  catalog write per file, once.
- A replacement with the same size and the same (or an exactly whole-hour shifted) 2 s FAT timestamp is not seen
  by the diff, as on Windows. Export's copy-verify against the catalog identity and cleanup's re-hash refuse such a
  file, so it is never copied or moved under the old identity; the next real change is detected as usual.
- `ps-scan` gains `rustix` as a Unix dependency (already used by `ps-export`).

## Revisit when
Windows collects NTFS file indexes, or a network filesystem (SMB `noserverino`) with invented ids shows the same
re-reads.
