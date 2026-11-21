# Tasks

Patch release 0.5.1. No catalog migration. Tests use recorded mount-table text and fakes; none needs root or a real
btrfs filesystem, except the existing workspace test, which runs where CI runs.

## 1. Mount-point fallback

- [ ] 1.1 Add `fs_ids::mountinfo_by_path` with octal-escape decoding and longest component-wise prefix (design D1).
  Verify: `ps-model` unit tests on a recorded btrfs mount table (`/@`, `/@home`, `/@cache` on `0:28`), on nested
  mounts with an escaped space, on a mount over another, and on a path under no listed mount.
- [ ] 1.2 `volume_info` falls back to it, and stats the mount source for the UUID and class (design D2). Verify:
  `scenario_sources_volume_on_a_btrfs_subvolume_identified` and `scenario_sources_longest_mount_point_wins` pass,
  using an injectable mount-table text and device stat, and ext4/FAT fixtures still resolve by device number.
- [ ] 1.3 Rename the existing workspace test to `scenario_sources_workspace_volume_identified`. Verify: it passes on
  the dev machine (btrfs), where it failed before.

## 2. Existing sources

- [ ] 2.1 Add the catalog command `SetVolumeIdentity`, which fills only NULL columns, and have a scan submit it for
  an online source missing either value (design D3). Verify:
  `scenario_sources_source_added_before_the_fix_gains_its_identity` passes (a source row with NULL identity gains
  it, and a recorded UUID survives a scan reporting another one), and a no-change rescan of 100k still meets
  `rescan-nochange-100k`.

## 3. Verification

- [ ] 3.1 `photosquirrel-cli --json sources list` on the dev machine shows the btrfs source's filesystem type,
  UUID and solid-state hint. Verify: the output pasted in this task's progress note.
- [ ] 3.2 `cargo xtask ci` passes with no failing test. Verify: the command is green.
- [ ] 3.3 `openspec validate btrfs-volume-identity --strict` passes. Verify: the command reports it valid.

## Workflow follow-up

- Archive with `/opsx:archive btrfs-volume-identity` once CI is green; ship in 0.5.1.
