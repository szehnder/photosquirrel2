# Spec Delta

## ADDED Requirements

### Requirement: Small write-ahead log
The catalog SHALL keep its write-ahead log small: a log that is reset SHALL shrink to at most 64 MiB, and the log SHALL be folded into the database and truncated after migrations, at the end of each scan run and when the catalog is closed. When the catalog or thumbnail store is opened with a log over 8 MiB, it SHALL be folded and truncated on a background thread, without delaying opening or blocking readers.

#### Scenario: Migrated catalog leaves an empty log
- **WHEN** a catalog with 30,000 analysed files at the schema before the colour migration is opened and migrated
- **THEN** once the background checkpoint has ended the log file is empty and all 30,000 rows read back with their colour state

#### Scenario: Large leftover log emptied after open
- **WHEN** a catalog whose log holds over 32 MiB of committed pages, left by a process that never checkpointed, is opened
- **THEN** opening returns, the catalog's rows read back unchanged, and once the background checkpoint has ended the log file is empty

#### Scenario: Large leftover thumbnail log emptied after open
- **WHEN** a thumbnail store whose log holds over 32 MiB of committed pages is opened
- **THEN** once the background checkpoint has ended its log file is empty and every stored thumbnail reads back

#### Scenario: Small leftover log left to normal checkpoints
- **WHEN** a catalog whose log is under 8 MiB is opened
- **THEN** no background checkpoint is started

#### Scenario: Scan run end empties the log
- **WHEN** 30,000 file rows are written during a scan run while readers are open and the run is marked completed
- **THEN** the log file is empty after the writer has committed the completion

#### Scenario: Closed catalog leaves an empty log
- **WHEN** 30,000 file rows are written and the catalog is closed while its readers are still open
- **THEN** the log file is empty
