# Spec Delta

## ADDED Requirements

### Requirement: Fill in previews job
A Fill in previews job SHALL re-decode only files whose placeholder can become a preview now, for all of them or one format or reason. It SHALL read each such file once, walk no folder, skip files on offline sources and count them, update thumbnails and perceptual hashes, re-group the changed files, and support pause and cancel like a scan.

#### Scenario: Fill in reads only waiting files
- **WHEN** Fill in previews runs on a catalog of 500 files where 20 HEIC placeholders wait for a decoder that is now available
- **THEN** exactly those 20 files are opened and read once, no directory is listed, and the 20 get thumbnails

#### Scenario: Filled-in HEIC joins its JPEG group
- **WHEN** Fill in previews decodes a HEIC whose resized JPEG copy is in the catalog
- **THEN** the HEIC and the JPEG are in one group afterwards

#### Scenario: Offline files are skipped and counted
- **WHEN** 5 of the waiting files are on an offline source
- **THEN** the job leaves those 5 waiting, reports 5 skipped because offline, and fills in the rest

#### Scenario: Fill in can be cancelled
- **WHEN** the user cancels Fill in previews after 10 of 20 files
- **THEN** the 10 keep their new previews, the other 10 still wait, and the next run fills in only those 10

### Requirement: Placeholders re-queued after an update
A placeholder whose decoder generation is older than the running version's generation for its format SHALL count as waiting. The Fill in previews job and the next rescan of its source SHALL re-decode it. A placeholder at the current generation whose reason is an unsupported variant or an unreadable file SHALL NOT be retried until the file or the generation changes.

#### Scenario: Rescan after an update fills in old TIFF placeholders
- **WHEN** a source with 3 unchanged TIFF placeholders from generation 0 is rescanned by a version with TIFF generation 1
- **THEN** the 3 files are re-decoded once and get thumbnails, and the rest of the source reads no file bytes

#### Scenario: Current-generation failures are not retried
- **WHEN** a source with an unchanged BigTIFF placeholder at the current TIFF generation is rescanned
- **THEN** the file is not opened
