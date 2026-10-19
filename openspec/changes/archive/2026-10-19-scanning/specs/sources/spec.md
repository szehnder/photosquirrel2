# Spec Delta

## Purpose

Lets the user register the folders and drives that hold their backups and lets PhotoSquirrel recognise a drive
it has seen before, even at a different mount point, without ever writing to it.

## ADDED Requirements

### Requirement: Add a source
The user SHALL be able to add a folder as a source. The application SHALL record its root path, the volume identity where the operating system exposes one, its total size and a content fingerprint of up to 64 sampled files.

#### Scenario: Add a source
- **WHEN** the user adds the `drive-a` root of the small corpus as a source
- **THEN** the source is listed with its root path, total size and a fingerprint of at most 64 files

#### Scenario: Missing folder rejected
- **WHEN** the user adds a path that does not exist
- **THEN** no source is created and the error names the path

### Requirement: Adding writes nothing to the source
Adding, fingerprinting, re-linking or removing a source MUST NOT create, modify or delete anything inside the source tree.

#### Scenario: Adding writes nothing to source
- **WHEN** a source is added and fingerprinted
- **THEN** a snapshot of the tree (paths, sizes, modification times) is identical before and after

### Requirement: Paths stored relative to the root
Files SHALL be recorded by source and path relative to the source root, so a new mount point does not change their identity.

#### Scenario: Paths are relative
- **WHEN** a source at `/mnt/usb1/Photos` containing `2010/a.jpg` is scanned
- **THEN** the file is recorded as `2010/a.jpg` in that source

### Requirement: Re-link a moved source
When a source's root is missing, the source SHALL be shown as offline, and the user SHALL be able to re-link it to a new root; re-linking MUST be confirmed only when the sampled content fingerprint matches.

#### Scenario: Remounted drive relinked
- **WHEN** a scanned corpus drive is moved to a new path and re-linked to it
- **THEN** the source keeps its history and a rescan reports 0 new and 0 changed files

#### Scenario: Wrong drive rejected
- **WHEN** a source is re-linked to a folder whose sampled files do not match its fingerprint
- **THEN** re-linking is refused with a mismatch count and the source stays offline

### Requirement: Overlapping sources folded
A source whose root lies inside another source's root SHALL be folded into the outer source, keeping the inner source's file history under the outer source's relative paths.

#### Scenario: Inner source folded into outer
- **WHEN** `drive-a/Backup 2015` is added and scanned, then `drive-a` is added
- **THEN** one source remains and each previously scanned file exists once with its prior stage and results

#### Scenario: Inner of existing reported
- **WHEN** `drive-a` is a source and the user adds `drive-a/Backup 2015`
- **THEN** no new source is created and the user is told it is already covered by `drive-a`

### Requirement: Offline sources stay browsable
An offline source's files, metadata and thumbnails SHALL remain in the catalog and visible, marked as offline.

#### Scenario: Unplugged source is offline
- **WHEN** a scanned source's root is removed and sources are refreshed
- **THEN** the source status is offline and its files are still listed with thumbnails

### Requirement: Remove a source from the catalog
Removing a source SHALL require confirmation, SHALL remove only catalog records for that source and MUST NOT touch files on disk.

#### Scenario: Remove keeps files on disk
- **WHEN** the user removes a scanned source and confirms
- **THEN** its catalog records are gone and the source tree on disk is unchanged
