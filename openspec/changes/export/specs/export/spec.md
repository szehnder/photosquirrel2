# Spec Delta

## Purpose

Copies the best copy of every photo into a separate, dated library folder that photo applications can import,
verifying every byte, never touching the sources and never overwriting anything.

## ADDED Requirements

### Requirement: Plan before copying
An export SHALL first produce a plan listing each keeper not yet exported to the chosen library with its target path, plus totals of files and bytes, collisions and free space; nothing SHALL be written to the library until the user confirms the plan.

#### Scenario: Dry run writes nothing
- **WHEN** an export plan is made for the scanned small corpus into an empty library folder
- **THEN** the plan lists every expected keeper with a target path and the library folder is still empty

#### Scenario: Insufficient space blocks export
- **WHEN** the plan's total bytes exceed the library volume's free space
- **THEN** the plan reports the shortfall and the export cannot be started

### Requirement: Dated library layout
Files SHALL be placed at `<library>/<YYYY>/<YYYY-MM-DD>/<original file name>` using the capture date; files whose date comes only from the file modification time SHALL be placed under `<library>/Undated/<YYYY>/<YYYY-MM-DD>/` using that time.

#### Scenario: Dated file placed by capture date
- **WHEN** a keeper named `IMG_0042.JPG` captured on 2011-08-14 is exported
- **THEN** it is copied to `<library>/2011/2011-08-14/IMG_0042.JPG`

#### Scenario: Undated file placed in Undated
- **WHEN** a keeper without any capture date and modified on 2009-02-03 is exported
- **THEN** it is copied to `<library>/Undated/2009/2009-02-03/`

### Requirement: Filing date from the duplicate group
A keeper without an embedded capture date of its own SHALL be filed by the best capture date its duplicate group knows: the keeper's own DateTimeOriginal, CreateDate or container date first, then another copy's embedded date, then a date in a copy's file name or XMP sidecar, and the file modification time (under `Undated/`) only when none of these exists. The plan, the CLI dry run and the manifest SHALL say where each file's date came from.

#### Scenario: Stripped keeper filed by group capture date
- **WHEN** the keeper of a duplicate group has no EXIF and was modified in 2024, and another copy in the group has DateTimeOriginal 2023-06-26
- **THEN** the keeper is placed in `<library>/2023/2023-06-26/` and its plan entry and manifest record say the date is the DateTimeOriginal of another copy

### Requirement: Never overwrite
An export MUST NOT overwrite or modify any existing file in the library; a different file at the target name SHALL get a ` (2)`, ` (3)`... suffix, and an identical file (same full hash) already there SHALL be recorded as exported without copying.

#### Scenario: Name collision gets suffix
- **WHEN** two different keepers named `IMG_0001.JPG` from the same day are exported
- **THEN** they are copied as `IMG_0001.JPG` and `IMG_0001 (2).JPG`

#### Scenario: Existing file never overwritten
- **WHEN** an unrelated file already exists at a keeper's target path
- **THEN** the existing file's bytes and modification time are unchanged and the keeper is copied with a suffix

#### Scenario: Identical existing file adopted
- **WHEN** the library already contains a byte-identical copy of a keeper at its target path
- **THEN** no copy is made and the keeper is recorded as exported to that path

### Requirement: Verified atomic copy
Each file SHALL be written to a temporary `.ps-partial` name while its full hash is computed, flushed to stable storage, compared with the catalog identity, given the source's modification time and only then renamed into place; on mismatch the partial file SHALL be removed, the file reported and the export continue.

#### Scenario: Verified copy renamed
- **WHEN** a keeper is exported
- **THEN** the final file's full hash equals the source's, its modification time equals the source's and no `.ps-partial` file remains

#### Scenario: Hash mismatch aborts file
- **WHEN** a source file is altered after scanning and then exported
- **THEN** no file appears at its target path, the mismatch is reported and the other files are exported

#### Scenario: Large video verified fully
- **WHEN** a 300 MB video with a sampled identity is exported
- **THEN** its sampled identity is re-checked, its full hash is computed during the copy and stored in the ledger, and the written file's full hash matches it

### Requirement: Sources untouched
An export MUST open source files read-only and MUST NOT change any source file's bytes, name or modification time.

#### Scenario: Export leaves sources untouched
- **WHEN** the small corpus is exported
- **THEN** a snapshot of every source tree (paths, sizes, modification times, hashes) is identical before and after

### Requirement: Library outside sources
The library folder MUST NOT be inside any source and no source may be inside the library.

#### Scenario: Library inside source refused
- **WHEN** the user picks a folder inside a registered source as the library
- **THEN** the export is refused with a message naming the source

### Requirement: Paired files kept together
RAW+JPEG keeper sets and Live Photo still/video pairs SHALL be exported into the same folder with matching file stems, applying any collision suffix to both.

#### Scenario: RAW JPEG pair keeps stem
- **WHEN** a keeper set `DSC_0100.DNG` and `DSC_0100.JPG` is exported where `DSC_0100.JPG` already exists from another photo
- **THEN** the pair is written as `DSC_0100 (2).DNG` and `DSC_0100 (2).JPG`

#### Scenario: Live Photo pair kept together
- **WHEN** an Apple Photos export includes `IMG_2000.HEIC` and its paired `IMG_2000.MOV`
- **THEN** both are written to the same folder with the same stem

### Requirement: XMP sidecars
When enabled, each exported file SHALL get a `<name>.xmp` sidecar containing the effective AI and user keywords as subjects, the description, people and pet counts as keywords and PhotoSquirrel provenance (content hash, source paths); sidecars SHALL be valid XMP. Sidecars SHALL be recorded in the export ledger like copies: an interrupted sidecar write SHALL be cleaned up and written again, an exported file without its sidecar SHALL get one when sidecars are on, and a sidecar the export wrote SHALL keep its name when its file is copied again.

#### Scenario: Sidecar contains AI keywords
- **WHEN** a keeper with insight category "pets", 1 dog and a user tag "Rex" is exported with sidecars on
- **THEN** its sidecar is valid XMP whose subjects include "pets", "dog" and "Rex" and whose provenance names its content hash

#### Scenario: Interrupted sidecar is finished on resume
- **WHEN** an export is killed while writing a sidecar and run again
- **THEN** the stale sidecar partial file is removed, every exported file has its sidecar and the export reports nothing left to do

#### Scenario: Deleted copy restored to its name
- **WHEN** a library copy whose sidecar the export wrote is deleted and the export runs again
- **THEN** the photo is copied to its original name, without a suffix, and the sidecar is left unchanged

### Requirement: Manifest
Each export SHALL append one JSON line per exported file to `<library>/.photosquirrel/manifest.jsonl` with its library path, full hash, all duplicate source paths, keeper reasons, profile and time.

#### Scenario: Manifest record per file
- **WHEN** 10 keepers are exported
- **THEN** the manifest gains exactly 10 lines, each parseable and listing every copy's source path

### Requirement: Idempotent and resumable
The export ledger SHALL record each completed copy with its size and modification time; re-running an export to the same library SHALL copy only keepers not yet exported, and an interrupted export SHALL resume, removing stale partial files of its own. A ledger-recorded copy still at its path whose bytes another application changed SHALL be reported as changed in the library and SHALL NOT be copied again unless the user asks. Planning SHALL trust a ledger-recorded copy that still has its recorded size and modification time without reading it, re-hashing only copies whose time changed, or every copy when the user asks to verify.

#### Scenario: Re-export copies only new
- **WHEN** the small corpus is exported, a new drive is scanned and the export is run again
- **THEN** only keepers from the new drive that were not already exported are copied

#### Scenario: Interrupted export resumes
- **WHEN** an export is killed mid-copy and run again
- **THEN** the stale `.ps-partial` file is removed, every keeper ends up exported exactly once and no file is duplicated

#### Scenario: Library copy changed by another app is not copied again
- **WHEN** another application rewrites an exported library copy, changing its size, and the export runs again
- **THEN** the plan lists that copy as changed in the library, nothing is copied and no suffixed duplicate appears; only when the user asks to copy it again is a fresh copy made beside it

#### Scenario: Re-plan reads no file contents
- **WHEN** every keeper has been exported and the export is planned again
- **THEN** the plan lists them all as already exported without reading any library file, and with verify it re-hashes each of them

#### Scenario: Disk full stops cleanly
- **WHEN** the library volume runs out of space during a copy
- **THEN** the partial file is removed, the export stops with a disk-full message and completed files remain recorded

### Requirement: Profiles
Export SHALL offer profiles: Apple Photos (sidecars on, Live Photo pairs together), Google Photos (sidecars off) and Lightroom (sidecars on, RAW+JPEG kept), whose defaults the user can adjust per export.

#### Scenario: Google profile omits sidecars
- **WHEN** the small corpus is exported with the Google Photos profile
- **THEN** no `.xmp` file is written

#### Scenario: Lightroom profile keeps RAW pairs
- **WHEN** the small corpus is exported with the Lightroom profile
- **THEN** every RAW+JPEG keeper set is exported as both files with sidecars

### Requirement: Offline sources deferred
Keepers on offline sources SHALL be shown in the plan as unavailable, skipped by the export and left pending.

#### Scenario: Offline source files deferred
- **WHEN** an export is planned while one corpus drive is offline
- **THEN** that drive's keepers are listed as unavailable and remain pending after the export

### Requirement: Export from the CLI
The CLI SHALL make plans with `export --dry-run` and run exports with a library, a profile and `--yes`, with `--json` output.

#### Scenario: CLI dry run prints plan
- **WHEN** the CLI export dry run runs with `--json` for the scanned small corpus
- **THEN** it prints the plan's files, target paths and totals and writes nothing to the library
