# Spec Delta

## ADDED Requirements

### Requirement: Groups filtered by insight
The groups list command SHALL accept the same insight filters as the Duplicates list (category, people, children, pets, pet kind, people count range, not described yet) and an unreviewed filter, and its JSON output SHALL report the matching group count and how many of them are reviewed, alongside the totals.

#### Scenario: CLI lists groups with pets
- **WHEN** insights are stored for the scanned small corpus and the groups list command runs with the pets filter and JSON output
- **THEN** it lists exactly the groups that contain pets and reports their count as matching, with the total group count unchanged

#### Scenario: CLI lists undescribed groups
- **WHEN** no AI queue has run and the groups list command runs with the not-described-yet filter
- **THEN** every group is listed and matching equals the total
