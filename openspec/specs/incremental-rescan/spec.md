# incremental-rescan Specification

## Purpose
Makes rescanning a known drive nearly free: only new and changed files are read, nothing is ever forgotten,
and files moved within a drive keep everything learned about them.

## Requirements

### Requirement: Stat-fingerprint diff
A rescan SHALL list and stat the source tree without reading file contents, compare each entry's size, modification time (nanoseconds) and file index where available with the catalog, and classify it as new, changed, unchanged or missing.

#### Scenario: Diff counts before analysis
- **WHEN** after a full scan 3 files are added, 2 are modified and 1 is deleted, and the source is rescanned
- **THEN** the rescan reports new 3, changed 2, missing 1 and the rest unchanged before any file content is read

#### Scenario: Unchanged files not read
- **WHEN** an unchanged source is rescanned with a byte-counting file layer
- **THEN** zero bytes of file content are read

### Requirement: Only new and changed files analyzed
A rescan SHALL analyze new and changed files and SHALL leave unchanged files' results untouched.

#### Scenario: Edited file re-analyzed
- **WHEN** a photo is overwritten with different content and the source is rescanned
- **THEN** its identity, metadata and thumbnail are updated and other files' results are unchanged

### Requirement: Missing files kept in history
A file no longer present SHALL be marked missing with the time it was first found missing and MUST NOT be deleted from the catalog; if it reappears with the same fingerprint it SHALL return to its previous state.

#### Scenario: Missing file kept in history
- **WHEN** a scanned file is deleted from the source and the source is rescanned
- **THEN** the file is marked missing and its metadata and thumbnail are still in the catalog

#### Scenario: Returning file restored
- **WHEN** a file marked missing is restored with its original size and modification time and the source is rescanned
- **THEN** the file is unchanged, no longer missing and was not read

### Requirement: Moved files keep their history
A file moved or renamed within a source SHALL be re-identified by content identity after analysis and SHALL keep its catalog identity, so everything attached to it (keeper overrides, AI results, export history) follows it.

#### Scenario: Moved file keeps history
- **WHEN** a scanned photo is moved to another folder of the same source and the source is rescanned
- **THEN** the catalog shows one live file at the new path with the original file's id and first-seen time, and the old path is not reported missing

### Requirement: No-change rescan budget
A rescan of an unchanged 100,000-file source from a warm cache SHALL complete within 4 s.

#### Scenario: No-change rescan under 4 s
- **WHEN** an unchanged 100,000-file source is rescanned on the reference machine
- **THEN** the duration is reported as a budget measurement and is at most 4 s

### Requirement: Hash upgrades without the sources
When the perceptual hash algorithm version changes, stored hashes SHALL be recomputed from the thumbnail store without reading source files, so offline sources are upgraded too.

#### Scenario: Hash version bump uses thumbnails
- **WHEN** the hash version is bumped and the hash upgrade runs while the source is offline
- **THEN** every thumbed image gets hashes of the new version and no source file is opened
