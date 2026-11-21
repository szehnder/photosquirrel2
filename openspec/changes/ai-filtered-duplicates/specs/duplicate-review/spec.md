# Spec Delta

## ADDED Requirements

### Requirement: AI insight filters on groups
The Duplicates list SHALL filter groups by category, contains people, contains children, contains pets, pet kind, people count range and "Not described yet", combined with its other filters. A group SHALL match when any of its files' effective insights matches. "Not described yet" SHALL match groups in which no file has an insight. The filter SHALL be kept across restarts.

#### Scenario: Filter groups with children
- **WHEN** insights are stored for the scanned small corpus's keepers and the user filters the Duplicates list by "contains children"
- **THEN** exactly the groups having a file whose effective insight has at least one child are listed

#### Scenario: A user correction decides the match
- **WHEN** the user corrects a keeper's category from "nature" to "pets" and filters the Duplicates list by category "pets"
- **THEN** that keeper's group is listed

#### Scenario: Groups not described yet
- **WHEN** insights are stored for half of the corpus's keepers and the user filters by "Not described yet"
- **THEN** exactly the groups with no described file are listed

#### Scenario: AI filter combines with unreviewed
- **WHEN** the user reviews 2 of the groups that contain pets and then filters by "contains pets" and unreviewed
- **THEN** the groups that contain pets are listed except those 2

#### Scenario: Filter kept across restarts
- **WHEN** the user filters the Duplicates list by "contains people" and restarts the application
- **THEN** the Duplicates list opens filtered by "contains people"

#### Scenario: Filtering never changes groups
- **WHEN** the user applies and clears every AI filter in turn
- **THEN** every group's members and keeper are identical to before

### Requirement: Filtered review progress
With an AI filter set, the review progress card SHALL also show how many matching groups are reviewed ("Matching: N of M reviewed"), and marking a group reviewed SHALL open the next unreviewed matching group. J and K SHALL move only between matching groups.

#### Scenario: Matching progress shown
- **WHEN** 5 groups contain pets, the user filters by "contains pets" and marks 2 of them reviewed
- **THEN** the card reads "Matching: 2 of 5 reviewed" and its main figure still counts every group

#### Scenario: Review advances within the filter
- **WHEN** the list is filtered by "contains pets" and the user presses R on a group
- **THEN** the next unreviewed group that contains pets opens, skipping groups that do not match

### Requirement: Filtered list follows new insights
While an AI filter is set, a group SHALL join or leave the list within 1 s of an insight or correction being stored that changes its match, keeping the selected group and the scroll position when the selected group still matches.

#### Scenario: Newly described group joins the list
- **WHEN** the list is filtered by "contains children" and a recording provider stores an insight with 1 child for an undescribed keeper
- **THEN** that keeper's group appears in the list within 1 s and the selected group stays selected
