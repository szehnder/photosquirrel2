# Spec Delta

## ADDED Requirements

### Requirement: Excluded view budget
The Excluded view SHALL have a budget, `excluded-view-open`: the time from the request for the first page and every chip count to both being available, on a catalog of 1M files with 200,000 excluded.

#### Scenario: Excluded view open within budget
- **WHEN** the Excluded view benchmark opens a generated 1M-file catalog with 200,000 excluded files
- **THEN** the time to the first page and all chip counts is reported as a budget measurement and is within its budget
