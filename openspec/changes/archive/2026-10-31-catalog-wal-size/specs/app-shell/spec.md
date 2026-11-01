# Spec Delta

## REMOVED Requirements

### Requirement: Fast start
**Reason**: Its idle-memory bound of 250 MB (ADR-0026) is raised to 320 MB by ADR-0123, which renames its scenario; a MODIFIED block cannot drop the old scenario name.
**Migration**: Replaced by "Start-up time and idle memory" below: the same cold-start scenario and the idle-memory scenario "Idle memory within 320 MB".

## ADDED Requirements

### Requirement: Start-up time and idle memory
The application SHALL draw its first frame within 400 ms of a cold start and SHALL stay at or below 320 MB resident memory when idle with a 1,000,000-item catalog.

#### Scenario: Cold start within 400 ms
- **WHEN** the release application is started with its startup measurement flag
- **THEN** the reported time to first frame, recorded as a budget measurement, is at most 400 ms

#### Scenario: Idle memory within 320 MB
- **WHEN** the release application is started with its idle-memory measurement flag on a 1,000,000-item catalog
- **THEN** the reported resident memory, recorded as a budget measurement, is at most 320 MB
