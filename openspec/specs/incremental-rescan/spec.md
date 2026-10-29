# incremental-rescan Specification

## Purpose
Makes rescanning a known drive nearly free: only new and changed files are read, nothing is ever forgotten,
and files moved within a drive keep everything learned about them.

## Requirements

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

### Requirement: Previews filled in once a decoder is available
A file analysed without the decoder its format needs SHALL record which decoder was missing. When that decoder is available, the next rescan SHALL read only those files, once each, and give them previews, perceptual hashes and look-matching, updating groups and keepers; while it is missing they SHALL be counted and not read. Catalogs from earlier versions SHALL have such files marked.

#### Scenario: Rescan decodes only files that waited for a decoder
- **WHEN** a source of 20 JPEGs and 5 HEIC photos scanned without a HEIC decoder is rescanned unchanged with one, through a byte-counting file layer
- **THEN** each HEIC is read once in full, no JPEG byte is read, the 5 get image thumbnails and perceptual hashes, and the summary counts 5 previews filled in

#### Scenario: HEIC grouped with its resized JPEG after the decoder arrives
- **WHEN** a HEIC photo and a resized JPEG copy of the same image, separate after a scan without a HEIC decoder, are rescanned with one
- **THEN** they form one duplicate group whose keeper is the HEIC, with a look-based join reason

#### Scenario: Files keep waiting while the decoder is missing
- **WHEN** the same source is rescanned unchanged and still without a HEIC decoder
- **THEN** zero bytes of file content are read and the summary reports 5 HEIC photos awaiting a decoder

#### Scenario: Old catalog marks undecoded HEIC and AVIF
- **WHEN** a catalog written before this change, holding HEIC and AVIF rows with placeholders and a HEIC row with hashes, is opened
- **THEN** the placeholder rows are marked as waiting for their decoder, the row with hashes is not, and the next rescan fills in the AVIF previews with the bundled decoder

### Requirement: A re-decode never fails an analysed file
A file that cannot be read to fill in its preview SHALL keep its catalogued analysis, stage, group and waiting mark, and the error SHALL be reported. A file whose data cannot be decoded SHALL keep its exact-copy analysis and placeholder, SHALL no longer be waiting, and the error SHALL be reported.

#### Scenario: Unreadable file keeps its analysis during a re-decode
- **WHEN** a waiting HEIC that is the keeper of an exported exact-copy group cannot be opened during the rescan that fills in previews
- **THEN** it keeps its stage, analysis and group, it is still waiting, the error is reported, and the export plan copies nothing new

#### Scenario: Corrupt HEIC keeps its exact-copy analysis
- **WHEN** a waiting HEIC whose image data is corrupt is re-decoded with a decoder that rejects it
- **THEN** it keeps its stage, identity, group and placeholder, it is no longer waiting, the error names the decode failure, and a further rescan reads none of its bytes

#### Scenario: Content changed under an unchanged fingerprint is analysed again
- **WHEN** a HEIC waiting for a decoder is rewritten with different bytes of the same length and its modification time restored, and the next rescan has a HEIC decoder
- **THEN** the re-decode's full read finds another identity, and the file is analysed again as a changed file: the identity of the new bytes, hashes, no waiting mark, and a thumbnail under the new identity

### Requirement: Failed decodes are retried when transient or after a decoder change
A HEIC or AVIF whose decode failed with its decoder present SHALL record the failure, the decoder backend and an attempt count (ADR-0098). An unchanged rescan SHALL decode it again while the failure is transient (a timeout) with fewer than 3 attempts, or when the backend changed, and SHALL NOT read it otherwise. A decode refused before it started (AV1 decoding paused, too many decodes, no thread) SHALL use up no attempt (ADR-0100).

#### Scenario: Timed-out AVIF is retried on rescan
- **WHEN** an AVIF's decode is abandoned by the watchdog during a scan, and the folder is rescanned unchanged, first while decoding still hangs and then after it completes
- **THEN** the AVIF keeps a placeholder with a transient failure, the first rescan reads it again (attempt 2), and the second fills in its preview and hashes and clears the failure

#### Scenario: Refused AVIF decodes use up no attempt
- **WHEN** an AVIF is scanned and rescanned unchanged five times while AV1 decoding is paused by abandoned decodes, and once more after they end
- **THEN** every refused run keeps a transient failure with no attempt used and reads the file again, and the last run fills in its preview and hashes and clears the failure

#### Scenario: Corrupt HEIC is not re-read on an unchanged rescan
- **WHEN** a HEIC the decoder rejects is scanned and the folder is rescanned unchanged twice, then once with a newer decoder backend
- **THEN** the unchanged rescans read none of its bytes, and the rescan with the newer backend reads it once and records the new backend

### Requirement: Metadata re-read for frame identity
On the first rescan after frame identity was added, unchanged analysed RAW-format files and same-second Canon camera files whose metadata predates it SHALL have it re-read from the header and bounded reads of frame values past it, read-only, without decoding or hashing, also in a quarantine folder, and SHALL be regrouped; an incomplete re-read SHALL leave the file for later. Other files SHALL NOT be read for it. The scan summary SHALL report the files re-read and the groups split.

#### Scenario: Old catalog splits merged burst frames on rescan
- **WHEN** a catalog written before frame identity holds a merged group of two converted burst frames and the unchanged source is rescanned with a byte-counting file layer
- **THEN** only the RAW, DNG and same-second Canon JPEG files are read, each at most its header bytes, the two frames are in separate groups, and the summary reports 1 group split

#### Scenario: The re-read happens once
- **WHEN** the same source is rescanned a second time
- **THEN** no file is read for frame identity

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

### Requirement: RAW placeholders re-classified once after the update
The RAW decoder generation SHALL be 1, so the first rescan of a source or Fill in previews after the update SHALL re-read each RAW placeholder of generation 0 once and store its corrected reason at generation 1, opening no other file. A re-read RAW that is still a variant not supported SHALL NOT be reported as a run error.

#### Scenario: Rescan re-classifies an old damaged RAW
- **WHEN** a source holding a cut CR2 with an "unsupported variant" placeholder of generation 0, 20 thumbed JPEGs and 5 thumbed CR2s is rescanned unchanged
- **THEN** only the cut CR2 is opened, its placeholder reason becomes "could not be read" at generation 1, and a second rescan opens no file

#### Scenario: Re-read unknown camera is not an error
- **WHEN** a source holding an unknown-camera CR2 placeholder of generation 0 is rescanned unchanged
- **THEN** the file is read once, its reason stays a variant not supported at generation 1, and the run reports 0 errors

### Requirement: RAW files with previews checked once for a cut
The first rescan of a source after the update SHALL check each unchanged RAW row that an earlier version stored with preview pixels for a cut, once, reading only its first 64 KB and its directories. A file found cut SHALL lose its hashes, get the "could not be read" placeholder, be regrouped and counted as damaged. Missing sources SHALL wait until they are scanned.

#### Scenario: Rescan finds a RAW cut after its intact preview
- **WHEN** a 0.3.0 catalog holds a CR2 cut after its intact preview, with a thumbnail and hashes, grouped with its camera JPEG, and the source is rescanned
- **THEN** the CR2 is damaged, out of the JPEG's group, the JPEG is a keeper, damaged is 1, only directory bytes of the CR2s are read, and a second rescan reads nothing
