# Spec Delta

## Purpose

Chooses, for every duplicate group, the single copy closest to the original, explains the choice in plain
language and lets the user override it.

## ADDED Requirements

### Requirement: Lexicographic ranking
Group members SHALL be ranked by, in order: format tier (camera RAW, then camera-original JPEG/HEIC, then lossless, then other lossy), pixel count, metadata completeness, estimated JPEG quality, file size, oldest modification time, shortest path depth. The top-ranked member is the keeper.

#### Scenario: Camera original beats PNG export
- **WHEN** a group holds a camera-original 12 MP JPEG and a 12 MP PNG export of it
- **THEN** the JPEG is the keeper with reason "camera original"

#### Scenario: Larger pixel count wins
- **WHEN** a group holds the same camera JPEG at 4000x3000 and at 1024x768, both camera originals
- **THEN** the 4000x3000 copy is the keeper

#### Scenario: Metadata breaks a tie
- **WHEN** two copies have equal tier and pixels but one lacks DateTimeOriginal and GPS
- **THEN** the copy with complete metadata is the keeper

#### Scenario: Shallower path breaks final tie
- **WHEN** two byte-identical copies have equal modification times at `Album/a.jpg` and `Backup/Old/Backup/Album/a.jpg`
- **THEN** `Album/a.jpg` is the keeper

### Requirement: Upscales never win
A larger copy that is an upscale of a smaller member (same pHash, lower estimated quality) MUST NOT outrank the smaller original on pixel count.

#### Scenario: Upscale does not win
- **WHEN** a group holds a 1024x768 original at quality 92 and a 2048x1536 upscale of it at quality 75
- **THEN** the 1024x768 original is the keeper

### Requirement: RAW and JPEG keeper sets
A camera RAW file and a JPEG from the same capture (same camera, same capture time, same file stem) SHALL be kept together as one keeper set.

#### Scenario: RAW and JPEG kept together
- **WHEN** a group holds a DNG, its camera JPEG and a 384 px thumbnail
- **THEN** the DNG and the camera JPEG form the keeper set and the thumbnail is not kept

### Requirement: Cache copies never keepers
A `thumbnail_cache` member SHALL NOT be the keeper when the group contains any member of another class.

#### Scenario: Cache copy never keeper
- **WHEN** a group holds a 2048 px Lightroom preview and a 1024 px album copy
- **THEN** the album copy is the keeper

### Requirement: Keeper reasons
The ranking SHALL return human-readable reasons for the keeper, naming the deciding criterion against the runner-up.

#### Scenario: Keeper reasons listed
- **WHEN** the keeper is chosen by pixel count
- **THEN** the reasons include the keeper's and the runner-up's dimensions

### Requirement: Deterministic ranking
The ranking SHALL be a total order: the keeper MUST NOT depend on the order in which members are presented.

#### Scenario: Ranking ignores input order
- **WHEN** the members of each corpus group are ranked in 20 random orders
- **THEN** every order yields the same keeper

### Requirement: Keeper overrides
The user SHALL be able to choose another member as keeper; the override SHALL be persisted, shown as a reason, and kept across restarts, rescans and regrouping as long as the chosen file remains in the group.

#### Scenario: Keeper override persists
- **WHEN** the user overrides a group's keeper and the catalog is closed and reopened
- **THEN** the chosen member is the keeper with reason "chosen by you"

#### Scenario: Override survives rescan
- **WHEN** a source is rescanned and new copies join the overridden group
- **THEN** the user's chosen member remains the keeper

### Requirement: Corpus keeper gate
On the ground-truth corpus the chosen keeper SHALL match the expected keeper for at least 99% of groups.

#### Scenario: Corpus keeper gate
- **WHEN** the medium corpus with seed 42 is scanned, grouped and evaluated
- **THEN** keeper accuracy is at least 0.99
