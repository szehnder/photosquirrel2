# Spec Delta

## ADDED Requirements

### Requirement: Duplicates list reload budget
The Duplicates list SHALL have a budget, `duplicates-list-reload`: the time to list the groups again after a change that leaves the groups themselves unchanged (a group marked reviewed, a filter changed, an AI insight stored), on a catalog of 1M files with 100,000 groups of 3 and an AI insight for every keeper, both without and with an AI filter.

#### Scenario: Duplicates list reload within budget
- **WHEN** the Duplicates list benchmark marks a group reviewed and lists again, and lists with "contains people", on a generated 1M-file catalog with 100,000 groups
- **THEN** both list times are reported as budget measurements and are within their budget
