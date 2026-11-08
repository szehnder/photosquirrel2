# Spec Delta

## MODIFIED Requirements

### Requirement: Engine features as subcommands
The CLI SHALL provide subcommands for sources (add, list, relink, remove), scans (start, resume), status, duplicate groups (list, show, set keeper, separate, suggestions), doctor, fixtures (generate, evaluate) and updates (status, check, install, rollback), each with help text.

#### Scenario: Every subcommand has help
- **WHEN** each subcommand is run with `--help`
- **THEN** each prints a usage description and exits with status 0

### Requirement: Doctor report
The doctor command SHALL report the application version, data directory, catalog schema version, per-format decoder availability (HEIC, AVIF, poster frames) with the decoder used or the reason and fix for the detected system, any decoder override in force, keychain availability, the configured LLM provider, and the update mode, installation kind and last update check, without sending any network request.

#### Scenario: Doctor lists decoders
- **WHEN** doctor is run with `--json` with the HEIC decoder forced unavailable
- **THEN** the output reports HEIC unavailable with a reason, fix steps for the detected system and the override in force, AVIF available, the data directory and the catalog schema version, and no network connection is opened

#### Scenario: Doctor reports update state offline
- **WHEN** doctor is run with `--json` on a fake per-user installation with mode notify and a recorded check
- **THEN** the output has the update mode, the installation kind, whether it can update itself and the last check time and result, and no network connection is opened

## ADDED Requirements

### Requirement: Update commands
The CLI SHALL provide:
- `update status`: mode, installation kind, the last check, and ready, kept and pending-trial versions, with no network.
- `update check`: one check, whatever the mode.
- `update install`: download, verify and install when no PhotoSquirrel program is running.
- `update rollback`: the guarded rollback.

A scan, export or other command SHALL NEVER check for updates.

#### Scenario: Update status works offline
- **WHEN** `update status --json` is run with the network unavailable
- **THEN** it exits 0 with the mode, installation kind, installed version and update history, and opens no connection

#### Scenario: Update check reports an available version
- **WHEN** `update check --json` is run with mode off against a test server serving a validly signed manifest for a newer version
- **THEN** it exits 0 reporting the available version, its notes address and whether this installation can install it, and the mode is still off

#### Scenario: Install refused while the app runs
- **WHEN** `update install --yes` is run while the application of the same installation is running
- **THEN** it exits 1 with "quit PhotoSquirrel first" and no installed file has changed

#### Scenario: Install refused for a package-managed installation
- **WHEN** `update install --yes` is run on a Debian-package installation with an update available
- **THEN** it exits 1, names the verified package file it saved and prints the `sudo apt install` command

#### Scenario: Scans never check for updates
- **WHEN** a scan, groups list and export plan are run with mode automatic and the update transport recorded
- **THEN** no update request was made
