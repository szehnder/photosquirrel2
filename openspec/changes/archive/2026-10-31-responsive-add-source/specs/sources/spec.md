# Spec Delta

## MODIFIED Requirements

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
