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

### Requirement: Dates refreshed without reading files
When the capture date rules change, the next rescan of a source SHALL re-date its unchanged, analyzed files from the facts already stored in the catalog, without reading any file content, and SHALL report how many capture dates changed; files with an embedded date SHALL keep it.

#### Scenario: Old catalog re-dated on rescan
- **WHEN** a catalog written by the previous date rules holds EXIF-less files in dated folders and the unchanged source is rescanned with a byte-counting file layer
- **THEN** those files get their folder dates, the rescan reports them as re-dated, files with EXIF dates are unchanged and zero bytes of file content are read

#### Scenario: Re-dating happens once
- **WHEN** the same source is rescanned a second time
- **THEN** no file is re-dated

### Requirement: Opt-in analysis refresh
A rescan SHALL, only when the user asks for an analysis refresh, re-read read-only the probe data of unchanged files whose stored classification facts are missing, and then re-classify them as for a settings change; without that request such files SHALL keep their class and the scan summary SHALL report how many there are.

#### Scenario: Files without facts counted, not read
- **WHEN** a catalog with 50 files probed before classification facts were stored is rescanned without the refresh
- **THEN** the summary reports 50 files that need an analysis refresh and zero bytes of file content are read

#### Scenario: Refresh re-classifies old rows
- **WHEN** the same catalog is rescanned from the CLI with `--refresh-analysis` after lowering the minimum side to 100 px
- **THEN** each of the 50 files is read only up to its probe limit, gets stored facts, and files between 100 and 200 px that were junk are re-classified

### Requirement: An analysis refresh never fails an analysed file
A file that cannot be opened or read for an analysis refresh SHALL keep its catalogued analysis and stage, and the error SHALL be reported. The summary of a refresh SHALL count the files it refreshed and SHALL report as needing a refresh only the files still waiting.

#### Scenario: Unreadable file keeps its analysis
- **WHEN** an analysis refresh cannot open an unchanged file that is the keeper of an exported duplicate group
- **THEN** the file keeps its stage and analysis, the group survives, the summary counts the other files as refreshed and this one as still needing a refresh, and the export plan copies nothing new
