# Spec Delta

## Purpose

Provides a reproducible synthetic photo collection with known duplicate groups, keepers and junk, so that
grouping, ranking, scanning and export quality are measured automatically instead of eyeballed.

## ADDED Requirements

### Requirement: Deterministic generation from a seed
Corpus generation SHALL be fully determined by its seed and preset: the same seed and preset MUST produce byte-identical files, paths, modification times and truth file on every platform.

#### Scenario: Same seed same corpus
- **WHEN** the small preset is generated twice with seed 42 into two empty folders
- **THEN** both trees and both truth files are byte-identical, including file modification times

#### Scenario: Different seed differs
- **WHEN** the small preset is generated with seeds 42 and 43
- **THEN** the two truth files differ

### Requirement: Originals look like camera photos
Each original SHALL be a textured, photo-like image with camera EXIF (make, model, serial, DateTimeOriginal, exposure) and, for a share of originals, GPS; originals SHALL span at least 3 camera models and 5 years of capture dates.

#### Scenario: Originals classify as photos
- **WHEN** every original of the small preset is classified with default settings
- **THEN** every original is classified `photo`

### Requirement: Derivatives with known truth
For each original the generator SHALL produce a configurable mix of exact copies, renamed copies, resized copies (1024, 384 and 160 px long edge), recompressions (quality 60-85) and EXIF-stripped copies, each recorded in the truth file as members of the original's group.

#### Scenario: Every derivative kind present
- **WHEN** the small preset is generated
- **THEN** the truth file lists at least 20 files of each derivative kind

### Requirement: Must-not-merge sets
The generator SHALL produce sets of distinct shots that must stay in separate groups: crops, burst-like near shots, same-scene shots minutes apart and visually similar shots from different cameras, each recorded as a must-not-merge set in the truth file.

#### Scenario: Must-not-merge sets recorded
- **WHEN** the small preset is generated
- **THEN** the truth file contains at least 10 sets of each must-not-merge kind, each with at least 2 originals

### Requirement: Junk and cache files
The generator SHALL produce junk with known expected classes: icons, palette UI graphics, tiny web images, cache thumbnails inside cache folders and short or tiny videos.

#### Scenario: Junk has expected classes
- **WHEN** the small preset is generated
- **THEN** the truth file records an expected class for every file and includes at least 20 files each of `graphic`, `junk` and `thumbnail_cache`

### Requirement: Backup-of-backup trees across drives
The generator SHALL distribute files over at least 3 simulated drive roots with nested backup folders (for example `Backup 2015/Old Laptop/Pictures/...`) so the same photo appears on several drives at different depths.

#### Scenario: Copies spread across drives
- **WHEN** the small preset is generated
- **THEN** at least 30% of groups have members under two or more drive roots and the deepest path has at least 6 folders

### Requirement: Truth file
A `truth.json` SHALL list every generated file exactly once with its relative path, drive root, category, expected class, true group id and whether it is the expected keeper; every group with at least one non-junk member SHALL have exactly one expected keeper (or one RAW+JPEG keeper set).

#### Scenario: Truth lists every file
- **WHEN** the small preset is generated
- **THEN** every file on disk appears exactly once in the truth file and no truth entry is missing on disk

#### Scenario: Each group has one keeper
- **WHEN** the truth file of the small preset is read
- **THEN** every group has exactly one expected keeper or exactly one RAW+JPEG keeper set

### Requirement: Grouping evaluator
An evaluator SHALL score a grouping result against the truth file, reporting pairwise precision, pairwise recall, keeper accuracy and the number of must-not-merge violations.

#### Scenario: Perfect grouping scores one
- **WHEN** the truth's own groups and keepers are evaluated
- **THEN** precision, recall and keeper accuracy are 1.0 and violations are 0

#### Scenario: False merge lowers precision
- **WHEN** two true groups of 2 members each, whose originals form a must-not-merge set, are merged into one result group
- **THEN** precision drops below 1.0, recall stays 1.0 and exactly one must-not-merge violation is counted

### Requirement: Presets and CLI generation
The headless CLI SHALL generate a corpus with a seed, a preset (`small` about 500 files, `medium` about 10,000, `large` about 100,000) and an output folder, writing only inside that folder and refusing a non-empty folder.

#### Scenario: CLI generates corpus
- **WHEN** the user runs the fixtures generate command with seed 42, preset small and an empty output folder
- **THEN** it writes the drive trees and `truth.json`, prints the file count and exits with status 0

#### Scenario: Refuses non-empty output
- **WHEN** the output folder already contains a file
- **THEN** the command exits non-zero, names the folder and writes nothing

#### Scenario: Small preset is quick
- **WHEN** the small preset is generated on the reference machine
- **THEN** generation completes in less than 30 s
