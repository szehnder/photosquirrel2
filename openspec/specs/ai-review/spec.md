# ai-review Specification

## Purpose
Lets the user watch AI descriptions arrive, find photos by what they show (people, children, pets, categories)
and correct the AI, while the AI's opinions stay advisory and never change what is kept or exported by themselves.

## Requirements

### Requirement: Live results
The AI Review view SHALL show results as they are stored, newest first, within 1 s of storage, while the queue runs.

#### Scenario: Results appear live
- **WHEN** a recording provider returns an insight while the AI Review view is open
- **THEN** the photo appears at the top of the view within 1 s

### Requirement: Insight details
For each photo the view and the inspector SHALL show subject, description, category, adults, children, pets, tags, the is-photo verdict and the provider and model that produced them.

#### Scenario: Insight details shown
- **WHEN** a photo with an insight of category "people", 3 adults and 2 children is selected
- **THEN** the inspector's AI section shows those values with the provider and model

### Requirement: Insight filters
The gallery and AI Review view SHALL filter by category, contains children, contains pets, pet kind and people count range, combined with other gallery filters.

#### Scenario: Filter contains children
- **WHEN** the user filters by "contains children"
- **THEN** exactly the photos whose effective insight has at least one child are shown

#### Scenario: Filter stays current after a scan
- **WHEN** a scan adds another copy of a photo that has an insight and the user filters by that insight again
- **THEN** the new copy is shown too, and a photo described again replaces its old description in the filter

### Requirement: Advisory only
AI insights MUST NOT change duplicate groups, keepers, exclusions, export selection or cleanup eligibility by themselves.

#### Scenario: AI never changes keeper
- **WHEN** insights are stored for every member of every corpus group
- **THEN** every group's members and keeper are identical to before the AI run

#### Scenario: Non-photo verdict is a suggestion
- **WHEN** the AI judges a likely-photo file to be a screenshot
- **THEN** the file stays in the Library and an "exclude as screenshot?" suggestion is shown

### Requirement: Accept or reject suggestions
The user SHALL be able to accept or reject AI suggestions one by one or in bulk; accepting an exclusion suggestion SHALL record a user exclusion with the AI's reason.

#### Scenario: Accept exclusion suggestion
- **WHEN** the user accepts 5 screenshot suggestions in bulk
- **THEN** the 5 files are excluded with reason "excluded by you: screenshot (AI suggestion)"

### Requirement: User corrections win
The user SHALL be able to edit category, people counts, pets and tags; corrected values SHALL be stored separately from the AI result and used wherever insights are shown, filtered or exported.

#### Scenario: User correction wins
- **WHEN** the user changes a photo's category from "nature" to "travel"
- **THEN** filters and the inspector use "travel" and the original AI value remains stored
