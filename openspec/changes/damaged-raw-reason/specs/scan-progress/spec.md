# Spec Delta

## MODIFIED Requirements

### Requirement: Run summary
When a run ends the dashboard SHALL show its duration, files processed, photos and videos found, duplicate groups formed, skipped, failed and damaged counts, and any incomplete sources.

#### Scenario: Run summary on completion
- **WHEN** a scan of the small corpus completes
- **THEN** the summary shows the duration and counts equal to the catalog's totals for the run

### Requirement: Decoder failures reported whatever the log filter
A file that a decoder could not read SHALL be reported by the run (its errors list or its placeholder reason, and the summary's counts) from PhotoSquirrel's own result for that file, never only through a log line, so hiding decoder warnings MUST NOT hide a failure.

#### Scenario: Damaged RAW file still reported under the default filter
- **WHEN** a folder with a truncated CR2 fixture is scanned with the default log filter
- **THEN** the run lists the file among its errors, its placeholder reason is "could not be read", and the summary counts 1 error and 1 damaged file, although no rawler warning is printed

## ADDED Requirements

### Requirement: Damaged files noticed in the run summary
The run summary SHALL count the present files of the run's sources catalogued as could not be read, from one catalog aggregate at the end of the run, so an unchanged rescan still shows them. When the count is above 0 the dashboard SHALL show a warning notice to keep the original until they are checked, with a way to show those files.

#### Scenario: Damaged files counted again on an unchanged rescan
- **WHEN** a source holding one cut CR2 is scanned and then rescanned unchanged
- **THEN** the first summary counts 1 error and 1 damaged file, and the second counts 0 errors and still 1 damaged file

#### Scenario: Dashboard warns about damaged files
- **WHEN** the dashboard shows a run whose summary counts 2 damaged files
- **THEN** it shows a damaged count of 2 and a warning notice, and choosing Show opens the Library filtered to those 2 files
