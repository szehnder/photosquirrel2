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
Every subcommand SHALL accept `--json` and then print only a single JSON document with a `schema_version` field to standard output, sending logs to standard error. Every path in it SHALL be a string (UTF-8, lossy), with the exact bytes in a `<field>_raw` field only when they are not valid UTF-8.

#### Scenario: JSON output parses
- **WHEN** sources list, status, groups list, groups show and doctor are run with `--json` on a scanned small corpus
- **THEN** each standard output parses as one JSON document containing `schema_version`

#### Scenario: JSON prints paths as strings
- **WHEN** a folder with an unreadable file is scanned with `--json`, then planned and run for cleanup with `--json` after a planned file became unsafe
- **THEN** the scan's `recent_errors[].path`, the plan's folders, the run's root and the run's `skipped[].rel_path` are strings

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
The scan command SHALL render the progress snapshot at most 10 times per second; with `--json` it SHALL print one JSON snapshot per line and a final summary line. With `--json`, lines of type `power` MAY appear between snapshots when the run pauses or resumes for power; no other line types SHALL appear.

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
The doctor command SHALL report the application version, data directory, catalog schema version, per-format decoder availability (HEIC, AVIF, poster frames) with the decoder used or the reason and fix for the detected system, any decoder override in force, keychain availability and configured LLM provider, without sending any network request.

#### Scenario: Doctor lists decoders
- **WHEN** doctor is run with `--json` with the HEIC decoder forced unavailable
- **THEN** the output reports HEIC unavailable with a reason, fix steps for the detected system and the override in force, AVIF available, the data directory and the catalog schema version, and no network connection is opened

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

### Requirement: Power pause is reported
When a foreground scan pauses or resumes for power, the CLI SHALL print one line on standard error saying so and
how to override it, SHALL mark its progress line as paused on battery and, with `--json`, SHALL print one NDJSON
line of type `power` per transition with the event, the run id and the time. While a run is paused, JSON
snapshots SHALL be printed at most once every 5 seconds.

#### Scenario: Power pause JSON lines
- **WHEN** a `--json` scan pauses for battery and later resumes on mains power
- **THEN** every line parses as JSON, a `power` line with event `paused_on_battery` and the run id precedes the paused snapshots, a `power` line with event `resumed_on_mains` follows them, and the last line is a summary with status completed

#### Scenario: Paused snapshots throttled
- **WHEN** a `--json` scan stays paused for battery for 12 seconds
- **THEN** at most four snapshot lines are printed during the pause and each has status paused

#### Scenario: Power pause on stderr
- **WHEN** a scan without `--json` and without a terminal pauses for battery
- **THEN** standard error has a line saying the scan is paused because the computer is on battery power and naming `--ignore-battery`, printed within a second of the pause and not repeated while the pause lasts

#### Scenario: Status shows power pause
- **WHEN** `scan status` is run in another terminal while a CLI scan is paused for battery
- **THEN** its live entry reports the run as paused on battery power

### Requirement: Interrupt while paused
An interrupt signal during a scan that is paused, for power or by request, SHALL cancel it exactly as for a running scan: exit status 130 and a resumable run.

#### Scenario: Ctrl-C while paused on battery
- **WHEN** an interrupt is sent to a CLI scan paused for battery
- **THEN** the CLI exits with status 130 within 1 s, the run is cancelled, and `scan --resume` completes it

### Requirement: Scan summary names undecodable photos
When a scan ends with photos that have no preview because their decoder is missing, the scan command SHALL print, after the counts, one line per format with the count and reason, then how to enable previews; with `--json` the summary SHALL carry the same per-format counts, reasons and fixes. When the decoder is available but files still wait, it SHALL say to rescan.

#### Scenario: Summary explains HEIC without a decoder
- **WHEN** a folder with 3 HEIC photos is scanned with the HEIC decoder forced unavailable
- **THEN** the text summary says 3 HEIC photos have no preview, gives the reason and the fix steps for the detected system, and the `--json` summary has the same count, reason and steps

#### Scenario: No notice when every photo is decoded
- **WHEN** the small corpus, which holds no HEIC, is scanned
- **THEN** neither the text nor the `--json` summary mentions undecodable photos

### Requirement: Preview status and fill in
`photosquirrel-cli previews status` SHALL report the placeholder count by format and reason, and how many can be filled in now. `previews fill [--format F]` SHALL run the Fill in previews job with progress on stderr, Ctrl-C handling like `scan`, and a summary. Both SHALL support `--json`.

#### Scenario: Status counts by reason
- **WHEN** `previews status --json` runs on a catalog with 4 HEIC files waiting for a missing decoder and 1 damaged JPEG
- **THEN** the JSON lists heic/decoder_missing 4 with fillable 0, and jpeg/unreadable 1 with fillable 0

#### Scenario: Fill one format
- **WHEN** `previews fill --format heic --json` runs after a HEIC decoder became available
- **THEN** only HEIC placeholders are re-decoded, and the summary reports filled, skipped offline and failed counts
