# sources Specification

## Purpose
Lets the user register the folders and drives that hold their backups and lets PhotoSquirrel recognise a drive
it has seen before, even at a different mount point, without ever writing to it.

## Requirements

### Requirement: Add a source
The user SHALL be able to add a folder as a source. Adding SHALL record its root path and the volume identity where the operating system exposes one at once, at a cost independent of the number of files, opening no file. Its first completed scan SHALL record its total size and a content fingerprint of up to 64 sampled files from what the scan identified, reading no file for it.

#### Scenario: Add a source
- **WHEN** the user adds the `drive-a` root of the small corpus as a source and then scans it to completion
- **THEN** the source is listed with its root path at once, without a size or fingerprint, and after the scan with its total size and a fingerprint of at most 64 files, the scan having opened each file at most once

#### Scenario: Missing folder rejected
- **WHEN** the user adds a path that does not exist
- **THEN** no source is created and the error names the path

#### Scenario: Adding reads no file whatever the tree size
- **WHEN** a folder of 6 files and a folder of 3,000 files with the same subfolders are added through a counting file layer
- **THEN** neither add opens a file or stats a file entry, and both make the same number of metadata calls and folder listings

#### Scenario: Interrupted first scan records the fingerprint later
- **WHEN** a source's first scan is cancelled and then resumed to completion
- **THEN** the cancelled run records no fingerprint and the resumed run records the total size and fingerprint

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
When a source's root is missing, the source SHALL be shown as offline, and the user SHALL be able to re-link it to a new root; re-linking MUST be confirmed only when the sampled content fingerprint matches. A source without a recorded fingerprint SHALL be checked by the files its scans identified so far, and one with no identified file SHALL be re-linked without a check. A completed scan with no file to sample SHALL leave the fingerprint unset, so a later completed scan records it.

#### Scenario: Remounted drive relinked
- **WHEN** a scanned corpus drive is moved to a new path and re-linked to it
- **THEN** the source keeps its history and a rescan reports 0 new and 0 changed files

#### Scenario: Wrong drive rejected
- **WHEN** a source is re-linked to a folder whose sampled files do not match its fingerprint
- **THEN** re-linking is refused with a mismatch count and the source stays offline

#### Scenario: Never scanned source relinked
- **WHEN** a source that was added but never scanned is moved and re-linked to its new path
- **THEN** it is re-linked without a fingerprint check and its first scan records the fingerprint

#### Scenario: Source empty at its first scan records the fingerprint later
- **WHEN** an empty folder is added and scanned to completion, photos are copied into it and it is scanned again, then it is moved and re-linked to another drive and to its new path
- **THEN** the first scan records a size of 0 and no fingerprint, the second records a fingerprint, the other drive is refused with a mismatch count and the new path is accepted

#### Scenario: Relink after an interrupted first scan
- **WHEN** a source whose first scan was cancelled after identifying some files is moved, then re-linked to its new path and to another drive
- **THEN** the new path is accepted and the other drive is refused with a mismatch count

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

### Requirement: Sources record whether file ids are stable
When a source is added, re-linked or scanned, the application SHALL record from the operating system whether its filesystem keeps file ids across mounts (not on FAT or exFAT; unknown where it cannot tell), and `sources list` SHALL show it. A quarantine move SHALL NOT compare file ids on a filesystem that does not keep them.

#### Scenario: exFAT source shows unstable file ids
- **WHEN** a source is added on a filesystem the probe reports as exFAT and `sources list --json` is run
- **THEN** the source has `stable_file_ids` false

#### Scenario: Quarantine after a remount is not refused for a new inode
- **WHEN** a file on a filesystem without stable file ids is planned for quarantine and its inode number differs from the catalog's while its size, time and content are unchanged
- **THEN** the move goes ahead
