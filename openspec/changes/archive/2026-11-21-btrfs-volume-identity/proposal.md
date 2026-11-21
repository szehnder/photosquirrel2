# Proposal

## Why

On btrfs, PhotoSquirrel cannot tell which volume a source is on. btrfs is the default filesystem of Fedora, openSUSE
and Manjaro, and the owner's own machine uses it. The `ps-scan` test `volume_of_the_manifest_dir_is_identified`
fails there for that reason. It is a real bug, not a test artefact:

- The volume is found by matching the device number from `stat` against `/proc/self/mountinfo`. On btrfs, each
  subvolume reports its own anonymous device number (here `0:48`). The mount table lists every subvolume mount
  under the filesystem's device (`0:28`). Nothing matches, so the filesystem type, the device, the volume UUID and
  the solid-state/rotational class all come back empty.
- **Scans start slower than they should.** A source on an SSD gets the lane start for unknown devices (2 readers
  instead of 8, `scanning` "Adaptive per-source I/O lanes"). The lane controller then has to grow it.
- **The unplugged-drive check does nothing.** No volume UUID is recorded, so an empty mount point left behind by an
  unplugged btrfs drive passes as the drive (`online()` accepts any folder when no UUID is stored).
- **Sources added before the fix stay that way.** The identity is recorded only when a source is added or
  re-linked.

Bind mounts and other filesystems whose `stat` device differs from the mount table's have the same problem.

## What Changes

- **Find the volume by mount point when the device number does not match.** If no mount-table entry has the path's
  device number, use the entry whose mount point is the longest prefix of the path's canonical form, decoding the
  mount table's octal escapes. Take the filesystem type and the device from it. Read the UUID and the
  solid-state/rotational class from that device's own number (`/dev/sda2` is `8:2`), not from the subvolume's.
- **Fill in what an existing source is missing.** At its next scan, a source with no recorded volume UUID or device
  hint records the ones found now. A recorded UUID is never replaced: a mismatch still means another drive.
- The same identification serves export libraries, which record a volume UUID the same way.

Release: 0.5.1 (patch).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `sources`: adds the requirements "Volume identified on any local filesystem" and "Missing volume identity
  recorded at the next scan".

## Impact

- **Code:** `ps-scan` `source::volume_info` (Linux) and the mount-table parsing in `ps-model` `fs_ids`. The scanner
  records a missing identity at scan time. No catalog migration: the columns exist.
- **Not affected:** file-id stability (`statfs` magic, ADR-0007 and ADR-0116), grouping, the corpus gate, macOS and
  Windows (their volume identity is not implemented yet).
- **ADRs relied on:** ADR-0006 (sources and offline volumes), ADR-0007 (rescan identity), ADR-0121 (adding a source
  opens no file).

### Out of scope

- Volume identity on macOS and Windows.
- Recognising btrfs subvolumes as separate volumes. A subvolume is identified as its filesystem, which is what the
  UUID and the device class describe.
