# 0116. A `fuseblk` mount is classified by its device's filesystem; FAT shifts reach 26 hours

- Status: accepted
- Date: 2026-10-28
- Deciders: maintainers
- Amends: [0112](0112-file-ids-are-used-only-on-filesystems-that-keep-them.md) (the FUSE rule and the FAT shift limit)

## Context
ADR-0112 classed every `fuseblk` mount as having unstable file ids. `fuseblk` is the generic type of any FUSE
driver on a block device, and udisks mounts NTFS through ntfs-3g as `fuseblk`. NTFS keeps its MFT record numbers,
which ntfs-3g reports as inode numbers, so on such a drive ADR-0112 dropped a replaced-in-place check that works.
The mount table does not name the driver (the super options are `user_id=..,group_id=..,allow_other`), but udev
records the filesystem it found on the block device (`/run/udev/data/b<major>:<minor>`, `E:ID_FS_TYPE=`).

ADR-0112 also capped FAT's whole-hour shift at 15 h. Time zones span UTC-12 to UTC+14, so a card written on one
side of the world and read on the other can read 26 h off.

## Decision
- **A generic FUSE mount (`fuseblk`, or `fuse` without a subtype) is classified by its block device:** the mount
  table's source (`/dev/sdb1`) gives the device number (`st_rdev`), and udev's `ID_FS_TYPE` for it is classified
  with the same table as a kernel mount: `exfat` is unstable, `vfat`/`msdos` are FAT (unstable, FAT time); `ntfs`
  and everything else, and a device udev does not know, are **stable** (the behaviour before ADR-0112). A named FUSE
  type (`fuse.exfat`, `fuse.sshfs`) is classified by its name, as in ADR-0112.
- The lookup is one pure function, `ps_model::fs_ids::linux_fs_ids(magic, dev, &dyn LinuxFsReader)`, whose reader
  (file text and `st_rdev`) is the real system in `ps-scan` and `ps-export` and a fake in tests. It runs only for a
  FUSE mount, once per device per walk.
- **FAT's whole-hour shift is unchanged up to 26 h** (`FAT_SHIFT_MAX_HOURS`). The rest of ADR-0112's rule stays:
  vfat/msdos only, equal size, an exact non-zero multiple of 3,600 s.

## Alternatives considered
- **Read the device's boot sector** - needs read access to the block device, which a desktop user does not have.
- **Keep `fuseblk` unstable** - loses the inode check on every NTFS drive mounted by udisks, the common case for a
  Windows backup drive on Linux.
- **No upper bound on the shift** - a file edited a whole number of days later with an equal size (rare, but a
  stronger coincidence than a zone change) would be missed.

## Consequences
- An NTFS drive through ntfs-3g keeps the replaced-in-place check; an exFAT drive through the FUSE driver still
  rescans without reads after a remount. Where udev's database is missing (some containers), a `fuseblk` exFAT
  drive is stable and re-reads after a remount, as before ADR-0112.
- A same-size FAT replacement whose time differs by 1-26 whole hours is not seen by a rescan; export and cleanup
  verify content, as ADR-0112 records for the 15 h limit.

## Revisit when
A FUSE driver that invents inode numbers is common on a filesystem udev reports as something other than exFAT or
FAT, or udisks names the driver in the mount table.
