# catalog Specification

## Purpose
Stores everything PhotoSquirrel knows about sources and files durably and crash-safely, and answers the
queries the gallery, scanner, reports and export need within their performance budgets.

## Requirements

### Requirement: Single local catalog
The application SHALL keep its catalog in one database file in the per-user application data directory (overridable for tests and the CLI), opened in write-ahead-log mode, and MUST NOT write catalog data anywhere inside a source tree.

#### Scenario: Catalog created in data directory
- **WHEN** the catalog is opened with an empty data directory
- **THEN** a catalog file is created there in write-ahead-log mode and nothing is written elsewhere

### Requirement: Versioned migrations
The catalog SHALL carry a schema version and apply numbered migrations in order when opened; a catalog with a newer version than the application supports MUST be refused without modification.

#### Scenario: Fresh catalog migrated to latest
- **WHEN** a new catalog is opened
- **THEN** its schema version equals the latest migration number

#### Scenario: Old catalog upgraded
- **WHEN** a version-1 catalog fixture containing 1,000 file rows is opened by a build with later migrations
- **THEN** it is upgraded to the latest version and all 1,000 rows are still present with unchanged values

#### Scenario: Newer catalog refused
- **WHEN** a catalog whose schema version is one higher than supported is opened
- **THEN** opening fails with a message naming both versions and the file's bytes are unchanged

### Requirement: Batched single writer
All catalog writes SHALL go through one writer that commits batches of at most 2,000 rows or after 100 ms, whichever comes first, and SHALL report write errors to the submitter.

#### Scenario: Batch flushes at 2000 rows
- **WHEN** 2,000 file rows are submitted at once
- **THEN** they are committed in one transaction without waiting for the 100 ms timer

#### Scenario: Batch flushes after 100 ms
- **WHEN** 10 file rows are submitted and no more follow
- **THEN** they are visible to readers within 150 ms

### Requirement: Atomic checkpoints
A file's pipeline stage SHALL be committed in the same transaction as the results of that stage, so that after any crash every file's stage matches the results stored for it.

#### Scenario: Killed writer leaves consistent state
- **WHEN** a child process writing 100,000 staged results is killed at a random moment and the catalog is reopened
- **THEN** every file marked analyzed has its identity and metadata, and no file has results of a stage it is not marked as having reached

### Requirement: Files keyed by source and relative path
Each file row SHALL be identified by its source and its path relative to the source root, and SHALL store its stat fingerprint (size, modification time in nanoseconds, file index where available), stage, class, reasons, identity and hashes.

#### Scenario: Same path upsert is idempotent
- **WHEN** the same source and relative path are written twice with identical values
- **THEN** exactly one file row exists and its values are unchanged

### Requirement: Insert throughput
The catalog SHALL insert at least 50,000 file rows per second through the batched writer on the reference machine.

#### Scenario: Insert 50k rows per second
- **WHEN** 1,000,000 synthetic file rows are submitted through the writer
- **THEN** the measured rate is reported as a budget measurement and is at least 50,000 rows per second

### Requirement: Complete ordered ID list
For any supported filter and sort, the catalog SHALL return the complete ordered list of matching item IDs with a 16-byte sort key each, and SHALL do so for 1,000,000 items within 300 ms.

#### Scenario: Full ID list for filter
- **WHEN** the ID list for "class photo, sorted by capture date" is requested on a 10,000-file catalog
- **THEN** it contains exactly the photo-class files in capture-date order with ties broken by ID

#### Scenario: ID list for 1M within 300 ms
- **WHEN** the ID list for all items sorted by capture date is requested on a 1,000,000-file catalog
- **THEN** it returns 1,000,000 IDs and the duration, reported as a budget measurement, is at most 300 ms

### Requirement: Keyset detail pages
Item details SHALL be fetched in keyset pages of up to 512 items by ID, never by offset, and SHALL be consistent with the ID list order.

#### Scenario: Keyset page of 512
- **WHEN** details for 512 consecutive IDs from the middle of a 1,000,000-item ID list are requested
- **THEN** exactly those 512 items are returned in ID-list order

### Requirement: Readers never block the writer
Queries SHALL run on read-only connections concurrently with writes and MUST NOT delay commits by more than one batch interval.

#### Scenario: Reads during writes
- **WHEN** ID-list queries run continuously while 200,000 rows are written
- **THEN** every query succeeds and the write rate stays at least 50,000 rows per second
