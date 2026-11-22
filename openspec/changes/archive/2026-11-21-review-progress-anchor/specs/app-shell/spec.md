# Spec Delta

## ADDED Requirements

### Requirement: Duplicates count shows groups left to review
The navigator's Duplicates item SHALL show the number of duplicate groups not yet reviewed, and its tooltip SHALL give both that number and the group total. When groups exist and none is left, the count SHALL be replaced by a check mark. The count SHALL change when a group is marked reviewed or unreviewed, and after a regroup, without the user reopening a view.

#### Scenario: Count goes down as groups are reviewed
- **WHEN** the user marks 3 groups of the scanned small corpus reviewed
- **THEN** the navigator's Duplicates count is the group total minus 3, and its tooltip reads that number "left to review of" the group total "groups"

#### Scenario: Check mark when every group is reviewed
- **WHEN** the user marks every group reviewed
- **THEN** the Duplicates item shows a check mark instead of a count and stays in the navigator

#### Scenario: Count rises again when a reviewed group changes
- **WHEN** every group is reviewed and a rescan adds another copy of a reviewed group's keeper
- **THEN** the Duplicates count shows 1 again, for the changed group
