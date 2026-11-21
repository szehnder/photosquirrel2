# Spec Delta

## ADDED Requirements

### Requirement: Volume identified on any local filesystem
On Linux, a source's volume SHALL be identified, giving its filesystem type, device, UUID and device class, also when the device number the folder reports is not the one the mount table lists, as on btrfs subvolumes and bind mounts. The mount whose mount point is the longest prefix of the folder's path SHALL then be used, and the UUID and device class SHALL come from that mount's device.

#### Scenario: Volume on a btrfs subvolume identified
- **WHEN** a folder on a btrfs subvolume whose reported device number is not in the mount table is identified
- **THEN** its filesystem type is btrfs, its device is the mounted partition, and its UUID and device class are those of that partition

#### Scenario: Longest mount point wins
- **WHEN** a folder lies under mount points `/`, `/home` and `/home/me/photos` and its device number matches none of them
- **THEN** the `/home/me/photos` mount is used, including when its mount point contains an escaped space

#### Scenario: Workspace volume identified
- **WHEN** the volume of the folder holding the test suite is identified on Linux
- **THEN** it has a filesystem type and a device class hint

### Requirement: Missing volume identity recorded at the next scan
A source with no recorded volume UUID or device hint SHALL record the ones found at its next scan, and a recorded volume UUID MUST NOT be replaced by a scan.

#### Scenario: Source added before the fix gains its identity
- **WHEN** a source recorded without a volume UUID or device hint is scanned on a volume that now identifies
- **THEN** the source records that volume's UUID and device hint, and a later scan from a different volume UUID does not change the recorded one
