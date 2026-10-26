# Spec Delta

## ADDED Requirements

### Requirement: Metadata re-read for frame identity
On the first rescan after frame identity was added, unchanged analysed RAW-format files, and Canon camera files of the same capture second, whose metadata predates it SHALL have it re-read from the probe's header bytes only, read-only, without decoding or hashing, also in a quarantine folder, and SHALL be regrouped. Other files SHALL NOT be read for it. The scan summary SHALL report the files re-read and the groups split.

#### Scenario: Old catalog splits merged burst frames on rescan
- **WHEN** a catalog written before frame identity holds a merged group of two converted burst frames and the unchanged source is rescanned with a byte-counting file layer
- **THEN** only the RAW, DNG and same-second Canon JPEG files are read, each at most its header bytes, the two frames are in separate groups, and the summary reports 1 group split

#### Scenario: The re-read happens once
- **WHEN** the same source is rescanned a second time
- **THEN** no file is read for frame identity
