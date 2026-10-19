# cli Specification

## Purpose
Makes every engine feature usable and verifiable from a terminal or script, with stable machine-readable output
and the same catalog as the desktop application.

## Requirements

### Requirement: Engine features as subcommands
The CLI SHALL provide subcommands for sources (add, list, relink, remove), scans (start, resume), status, duplicate groups (list, show, set keeper, separate, suggestions), doctor and fixtures (generate, evaluate), each with help text.

#### Scenario: Every subcommand has help
- **WHEN** each subcommand is run with `--help`
- **THEN** each prints a usage description and exits with status 0

### Requirement: JSON output
Every subcommand SHALL accept `--json` and then print only a single JSON document with a `schema_version` field to standard output, sending logs to standard error.

#### Scenario: JSON output parses
- **WHEN** sources list, status, groups list, groups show and doctor are run with `--json` on a scanned small corpus
- **THEN** each standard output parses as one JSON document containing `schema_version`

### Requirement: Exit codes
The CLI SHALL exit with 0 on success, 1 on operational failure, 2 on usage errors and 130 after an interrupt.

#### Scenario: Usage error exits 2
- **WHEN** the CLI is run with an unknown flag
- **THEN** it prints the usage error to standard error and exits with status 2

#### Scenario: Failure exits 1
- **WHEN** a scan is started for a source id that does not exist
- **THEN** the CLI prints the reason and exits with status 1

### Requirement: Shared data directory
The CLI SHALL use the same catalog as the application by default and SHALL accept a data directory override by flag or environment variable.

#### Scenario: Data dir override
- **WHEN** a source is added with `PHOTOSQUIRREL_DATA_DIR` set to an empty temp folder
- **THEN** the catalog is created in that folder and the default data directory is untouched

### Requirement: Scan with live progress
The scan command SHALL render the progress snapshot at most 10 times per second; with `--json` it SHALL print one JSON snapshot per line and a final summary line.

#### Scenario: Scan streams JSON snapshots
- **WHEN** the small corpus is scanned with `--json`
- **THEN** every output line parses as JSON, lines are at least 100 ms apart, and the last line is a summary with status completed

### Requirement: Interrupt cancels cooperatively
An interrupt signal during a scan SHALL request cancellation, wait for in-flight results to be flushed and exit with status 130, leaving a resumable run.

#### Scenario: Ctrl-C cancels scan
- **WHEN** an interrupt is sent to a running scan of the medium corpus
- **THEN** the CLI exits with status 130 within 1 s and the run is cancelled

#### Scenario: Resume an interrupted run
- **WHEN** the scan command is run with `--resume` after a cancelled run
- **THEN** it continues the same run id and completes without re-reading finished files

### Requirement: Group inspection and overrides
The groups command SHALL list groups with member counts and reclaimable bytes, show a group's members with their join reasons and the keeper reasons, and set a keeper or separate a member.

#### Scenario: Groups explain keeper
- **WHEN** a group of the scanned small corpus is shown
- **THEN** the output lists each member's path, edge kind and distances, and the keeper's reasons

#### Scenario: Set keeper from CLI
- **WHEN** the set keeper command chooses another member of a group
- **THEN** showing the group lists that member as keeper with reason "chosen by you"

### Requirement: Doctor report
The doctor command SHALL report the application version, data directory, catalog schema version, optional decoder availability (HEIC, poster frames), keychain availability and configured LLM provider, without sending any network request.

#### Scenario: Doctor lists decoders
- **WHEN** doctor is run with `--json` in a build without HEIC support
- **THEN** the output reports HEIC unavailable, the data directory and the catalog schema version, and no network connection is opened

### Requirement: Corpus evaluation
The fixtures evaluate command SHALL score the current catalog's groups against a truth file and exit non-zero when any ADR-0025 gate is not met.

#### Scenario: Evaluate fails below gate
- **WHEN** evaluate is run against a truth file whose groups were deliberately altered to force precision below 0.995
- **THEN** it prints precision, recall and keeper accuracy and exits with status 1

### Requirement: Non-interactive confirmations
Commands that need confirmation (for example removing a source) SHALL prompt only on an interactive terminal, and otherwise MUST refuse unless `--yes` is given.

#### Scenario: Non-interactive needs yes
- **WHEN** source removal is run with standard input not a terminal and without `--yes`
- **THEN** nothing is removed and the CLI exits with status 1 explaining that `--yes` is required
