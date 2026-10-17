# Spec Delta

## Purpose

Keeps PhotoSquirrel fast by measuring every performance budget, recording a baseline and failing builds that
exceed a budget or regress noticeably, and by making sure every specified scenario has a test.

## ADDED Requirements

### Requirement: Every budget defined
The budgets file SHALL contain one entry with a name, metric, upper bound, unit, owning change and measurement source for each ADR-0026 budget: gallery scroll p95 (8.3 ms), first gallery frame (600 ms), cold start (400 ms), idle memory (250 MB), catalog insert (50k rows/s), 1M ID list (300 ms), no-change rescan (4 s), analysis (150 files/s), hashing (0.3 ms), near-duplicate lookup (1 ms) and control latency (250 ms).

#### Scenario: All ADR budgets defined
- **WHEN** the budgets file is parsed
- **THEN** it has an entry for each of the eleven ADR-0026 budgets with a positive bound and a source

#### Scenario: List shows every budget
- **WHEN** the bench list command runs
- **THEN** it prints one line per budget with its bound, unit and owning change

### Requirement: Absolute budgets checked locally
The local check SHALL measure every selected budget and fail, naming the budget, the measured value and the bound, when a measurement exceeds its bound or no measurement was produced.

#### Scenario: Absolute budget exceeded fails
- **WHEN** a measurement of 450 ms is evaluated locally against a 400 ms budget
- **THEN** the check fails with a message naming the budget, 450 and 400

#### Scenario: Missing measurement fails
- **WHEN** a selected budget's source prints no measurement
- **THEN** the check fails and names that budget

### Requirement: Regressions caught
A measurement more than 15% worse than the recorded baseline SHALL fail the check in both local and CI modes.

#### Scenario: Regression detected
- **WHEN** a measurement of 48 ms is evaluated against a baseline of 40 ms in CI mode
- **THEN** the check fails with a message naming the benchmark, both values and 20%

### Requirement: CI mode
CI mode SHALL check only relative regressions and SHALL skip measurements that need a window or display.

#### Scenario: CI mode checks regressions only
- **WHEN** a measurement above its absolute bound but within 15% of its baseline is evaluated in CI mode
- **THEN** the check passes, and application window measurements are not run

### Requirement: Baseline recording
The update-baseline option SHALL write the current measurements to the baseline file, keeping entries for budgets that were not measured.

#### Scenario: Baseline update writes values
- **WHEN** the baseline is updated with measurements for two of eleven budgets
- **THEN** the baseline file holds the two new values and the other nine unchanged

### Requirement: All budgets met before release
On the reference machine, the full local check SHALL pass for every budget before a release is tagged, and the resulting baseline SHALL be committed.

#### Scenario: All budgets within limits
- **WHEN** the full local check runs on the reference machine at the release candidate
- **THEN** it reports all eleven ADR budgets met and exits with status 0

### Requirement: Scenario coverage enforced
Every scenario of an archived specification SHALL have a test named after its capability and title; the coverage check SHALL fail and list the missing test names otherwise.

#### Scenario: Missing scenario test
- **WHEN** an archived spec has one scenario with a matching test and one without
- **THEN** the coverage check fails and lists only the missing test name
