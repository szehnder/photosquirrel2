# Spec Delta

## MODIFIED Requirements

### Requirement: Dated library layout
Files SHALL be placed at `<library>/<YYYY>/<YYYY-MM-DD>/<original file name>` using the capture date (a keeper named like a copy, such as `Copy of IMG_5437.JPG`, takes the cleaner name another copy in its group has); files whose filing date has month precision SHALL be placed at `<library>/<YYYY>/<YYYY-MM>/`; files whose date comes only from the file modification time SHALL be placed under `<library>/Undated/<YYYY>/<YYYY-MM-DD>/` using the local day of that time.

#### Scenario: Dated file placed by capture date
- **WHEN** a keeper named `IMG_0042.JPG` captured on 2011-08-14 is exported
- **THEN** it is copied to `<library>/2011/2011-08-14/IMG_0042.JPG`

#### Scenario: Copy-named keeper gets the clean name
- **WHEN** the keeper of a group is named `Copy of IMG_5437.JPG` and another copy in the group is named `IMG_5437.JPG`
- **THEN** it is copied to `<library>/<YYYY>/<YYYY-MM-DD>/IMG_5437.JPG`

#### Scenario: Undated file placed in Undated
- **WHEN** a keeper without any capture date and modified on 2009-02-03 is exported
- **THEN** it is copied to `<library>/Undated/2009/2009-02-03/`

#### Scenario: Undated day is the local day
- **WHEN** a keeper without any capture date, modified at 2019-07-05 03:30 UTC, is scanned and exported with the local zone America/Los_Angeles
- **THEN** it is copied to `<library>/Undated/2019/2019-07-04/`

#### Scenario: Month-precision file placed in a month folder
- **WHEN** a keeper without embedded dates whose only date is the folder `2019-07 Italy` is exported
- **THEN** it is copied to `<library>/2019/2019-07/` and its plan entry and manifest record give the date source as the folder name with precision month

### Requirement: Filing date from the duplicate group
A keeper without an embedded capture date of its own SHALL be filed by the best capture date its duplicate group knows: the keeper's own DateTimeOriginal, CreateDate or container date first, then another copy's embedded date, then a date in a copy's file name or XMP sidecar, then a date in the name of a folder holding the keeper or another copy, and the file modification time (under `Undated/`) only when none of these exists. The plan, the CLI dry run and the manifest SHALL say where each file's date came from.

#### Scenario: Stripped keeper filed by group capture date
- **WHEN** the keeper of a duplicate group has no EXIF and was modified in 2024, and another copy in the group has DateTimeOriginal 2023-06-26
- **THEN** the keeper is placed in `<library>/2023/2023-06-26/` and its plan entry and manifest record say the date is the DateTimeOriginal of another copy

#### Scenario: Embedded date of a copy beats the keeper's folder date
- **WHEN** a stripped keeper is in folder `2019-07-04` and another copy in its group has DateTimeOriginal 2019-07-03 21:00
- **THEN** the keeper is placed in `<library>/2019/2019-07-03/` from the other copy's DateTimeOriginal

#### Scenario: Folder date of a copy files a stripped keeper
- **WHEN** a stripped keeper `IMG_0001.JPG` in folder `Downloads` has no sidecar, and another copy in its group is in folder `2019_07_04 Beach`
- **THEN** the keeper is placed in `<library>/2019/2019-07-04/` and the plan says the date is the folder name of another copy

## ADDED Requirements

### Requirement: Existing libraries keep their layout
A change to the date rules MUST NOT move, rename or copy again a file the export ledger records as exported; such files SHALL stay exported at their recorded paths, only keepers not yet exported SHALL use the current rules, and the plan SHALL report how many exported copies are filed in a folder other than the one the current rules choose.

#### Scenario: Re-export after date rules change copies nothing
- **WHEN** a library is exported, the catalog is re-dated so that 20 exported undated keepers get folder dates, and the export is planned and run again
- **THEN** nothing is copied, no file in the library is moved or added, and the plan reports 20 copies filed under earlier date rules

#### Scenario: New keeper uses the current rules beside old copies
- **WHEN** after that re-dating a new EXIF-less keeper in folder `2019-07-04` is scanned and exported
- **THEN** only it is copied, to `<library>/2019/2019-07-04/`

### Requirement: Library of another catalog flagged
When the chosen library holds `.photosquirrel/manifest.jsonl` but this catalog has no ledger rows for it, the plan SHALL warn that the library was written by another catalog and that files already in it may be copied again into different folders; the export SHALL still never overwrite a file.

#### Scenario: Unknown library warned in the plan
- **WHEN** a library exported by one catalog is chosen as the target by a second catalog of the same sources and the export is planned with `--json`
- **THEN** the plan carries the other-catalog warning and the library's existing files are unchanged after the run
