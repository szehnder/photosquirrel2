# Design

## Context

- `ps-scan` `source::imp::volume_info` (Linux) stats the folder, splits `st_dev` into major and minor, and asks
  `ps_model::fs_ids::mountinfo_entry(text, major, minor)` for the mount-table line with that `major:minor` (field
  3). From that line it takes the filesystem type and the mount source. Then it calls `block_class(major, minor)`
  (sysfs `queue/rotational`) and `uuid_of(st_dev)` (the `/dev/disk/by-uuid` link whose `rdev` is `st_dev`).
- On the dev machine, the workspace folder reports `0:48`, while every btrfs line of the mount table says `0:28`
  (`/@`, `/@home` and `/@cache` alike). Neither number is the partition's block device (`8:2`). So all three
  lookups miss.
- The add and re-link paths record `volume_uuid` and `device_hint` (`NewSource`, `Command::RelinkSource`). A scan
  never does. `Scanner::online` accepts a folder when no UUID is recorded.

## Goals / Non-Goals

**Goals:**
- One rule for every Linux filesystem: the device number first (unchanged for ext4, XFS, FAT, NTFS, network
  shares), the mount point second.
- Pure parsing, testable on recorded mount-table text, with no root and no real btrfs needed.

**Non-Goals:**
- Telling btrfs subvolumes apart (see proposal).
- macOS and Windows.

## Decisions

### D1. Fall back to the longest mount-point prefix

Add `ps_model::fs_ids::mountinfo_by_path(text, path) -> Option<MountEntry { mount_point, fs_type, source }>`. It
decodes the mount table's octal escapes (`\040` for a space, `\011`, `\012`, `\134`) in the mount point, then picks
the entry whose mount point is the longest component-wise prefix of the canonical path. When two entries share a
mount point (one mounted over another), the later line wins, matching the kernel's order. `volume_info` tries the
device number first and uses this fallback only when that finds nothing.

*Alternative:* `statfs` plus `/sys/fs/btrfs/<fsid>`. Rejected because it only works for btrfs, while bind mounts and
other stacked filesystems have the same mismatch.

### D2. The UUID and class come from the mount source's own device

When the mount source is a block device path (it starts with `/dev/`), stat it and use its `rdev` for
`block_class` and `uuid_of`. Otherwise (`tmpfs`, `overlay`, network shares) keep today's behaviour: network shares
are classed by filesystem type, everything else is unknown. With the device-number match (D1's first step), the
source is stat'ed the same way, so ext4 on a partition is unchanged.

### D3. A scan records a missing identity; it never replaces one

When a scan starts on an online source whose record has no `volume_uuid` or no `device_hint`, it submits the
values found now (a new catalog command, `SetVolumeIdentity { source, volume_uuid, device_hint }`, which writes
only columns that are NULL, and treats a hint recorded with `fs=?` as missing, since that is what a failed
identification stored). A recorded UUID is never overwritten, because a different UUID means another drive
(`Scanner::online`). No migration is needed, as the columns exist.

## Risks / Trade-offs

- [A path under a mount point that is not a prefix after canonicalisation, such as a symlinked root] → Canonicalise
  the folder before matching, as the scanner already does for roots.
- [A previously Unknown device class becomes SolidState and lanes start with 8 readers on a busy disk] → That is
  the specified behaviour for SSDs. The lane controller still shrinks on rising latency.

## Migration Plan

None. Existing btrfs sources record their identity at their next scan (D3).

## Verification

```
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-model -- --test-threads=4 fs_ids
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-scan -- --test-threads=4 \
  scenario_sources_volume_on_a_btrfs_subvolume_identified scenario_sources_longest_mount_point_wins \
  scenario_sources_workspace_volume_identified scenario_sources_source_added_before_the_fix_gains_its_identity
cargo run --release -p photosquirrel-cli -- sources add <a folder on btrfs> && cargo run --release -p photosquirrel-cli -- --json sources list
cargo xtask ci
openspec validate btrfs-volume-identity --strict
```
