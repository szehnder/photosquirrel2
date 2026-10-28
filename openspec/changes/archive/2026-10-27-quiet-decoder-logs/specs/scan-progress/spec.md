# Spec Delta

## ADDED Requirements

### Requirement: Decoder failures reported whatever the log filter
A file that a decoder could not read SHALL be reported by the run (its errors list or its placeholder reason, and the summary's counts) from PhotoSquirrel's own result for that file, never only through a log line, so hiding decoder warnings MUST NOT hide a failure.

#### Scenario: Damaged RAW file still reported under the default filter
- **WHEN** a folder with a truncated CR2 fixture is scanned with the default log filter
- **THEN** the run reports the file with its reason and the summary counts it, although no rawler warning is printed
