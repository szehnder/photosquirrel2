# Spec Delta

## MODIFIED Requirements

### Requirement: Every budget defined
The budgets file SHALL contain one entry with a name, metric, upper bound, unit, owning change and measurement source for each ADR-0026 budget, with idle memory as amended by ADR-0123: gallery scroll p95 (8.3 ms), first gallery frame (600 ms), cold start (400 ms), idle memory (320 MB), catalog insert (50k rows/s), 1M ID list (300 ms), no-change rescan (4 s), analysis (150 files/s), hashing (0.3 ms), near-duplicate lookup (1 ms) and control latency (250 ms).

#### Scenario: All ADR budgets defined
- **WHEN** the budgets file is parsed
- **THEN** it has an entry for each of the eleven ADR-0026 budgets with a positive bound and a source

#### Scenario: List shows every budget
- **WHEN** the bench list command runs
- **THEN** it prints one line per budget with its bound, unit and owning change
