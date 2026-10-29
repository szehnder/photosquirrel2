# Spec Delta

## MODIFIED Requirements

### Requirement: Stat-fingerprint diff
A rescan SHALL list and stat the source tree without reading file contents, compare each entry's size, modification time (nanoseconds) and file index with the catalog, and classify it as new, changed, unchanged or missing. The file index SHALL be used only where the filesystem keeps it across mounts (not FAT or exFAT). On FAT, an exact whole-hour shift (at most 26 h) of an equal-size file SHALL count as unchanged.

#### Scenario: Diff counts before analysis
- **WHEN** after a full scan 3 files are added, 2 are modified and 1 is deleted, and the source is rescanned
- **THEN** the rescan reports new 3, changed 2, missing 1 and the rest unchanged before any file content is read

#### Scenario: Unchanged files not read
- **WHEN** an unchanged source is rescanned with a byte-counting file layer
- **THEN** zero bytes of file content are read

#### Scenario: Remounted FAT card reads nothing
- **WHEN** a scanned source on a filesystem reported as without stable file ids is rescanned after a simulated remount that gives every file a new inode number
- **THEN** zero bytes of file content are read and the rescan reports 0 changed

#### Scenario: New inode numbers still mean changed where ids are stable
- **WHEN** the same simulated remount happens on a filesystem reported as keeping file ids
- **THEN** every file with a new inode number is reported changed

#### Scenario: First rescan after upgrading reads nothing
- **WHEN** a catalog whose rows store inode numbers is rescanned unchanged on a filesystem without stable file ids, after a simulated remount
- **THEN** zero bytes are read and the rows no longer store a file index

#### Scenario: Renamed file on FAT is detected
- **WHEN** a scanned photo on a filesystem without stable file ids is renamed and the source is rescanned
- **THEN** the old path is not reported missing, the file at the new path keeps the original file's id, and only that file is read

#### Scenario: Replaced file on FAT is detected
- **WHEN** a scanned photo on a filesystem without stable file ids is overwritten with different content and a new modification time
- **THEN** the rescan reports it changed and its identity is updated

#### Scenario: FAT time-zone shift is not a change
- **WHEN** every modification time on a FAT source reads one hour later after a remount, sizes unchanged
- **THEN** zero bytes are read and the new modification times are stored

#### Scenario: Other shifts are changes
- **WHEN** a file's modification time moves by 30 minutes on FAT, or by one hour on exFAT, its size unchanged
- **THEN** the rescan reports it changed
