# Spec Delta

## ADDED Requirements

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
