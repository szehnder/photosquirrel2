# Spec Delta

## ADDED Requirements

### Requirement: Group composition fits its panel
Every line of the "Why this copy" panel, both the keeper reasons and the "In this group" composition lines, SHALL stay within the panel's content area at every window width and UI font size. A line too long for one row SHALL wrap under its own text, and the count column SHALL stay aligned.

#### Scenario: Pair keeper composition wraps inside the panel
- **WHEN** a group whose keeper set is a RAW+JPEG pair is opened at the narrowest window width and at the largest UI font size
- **THEN** the line "keepers (RAW+JPEG pair): export takes both together" lies inside the panel's content area, on more than one row, with its count still in the count column

#### Scenario: Composition lines fit at the default size
- **WHEN** a group with an exact copy and two resized copies is opened at the default window width and UI font size
- **THEN** every composition line lies inside the panel's content area

### Requirement: Review progress card
The Duplicates list header SHALL show review progress as a card on a tinted surface. Its main figure SHALL be the number of groups left to review, with "N of M reviewed" and a progress bar below it. When no group is left, the card SHALL show a success state naming the total. The figures SHALL cover every group, whatever filter is set.

#### Scenario: Card shows groups left
- **WHEN** the user marks 3 of the small corpus's groups reviewed
- **THEN** the card's main figure is the group total minus 3 followed by "left", and it reads "3 of" the total "reviewed"

#### Scenario: Card shows all reviewed
- **WHEN** the user marks every group reviewed
- **THEN** the card shows its success state, reading "All" with the group total and "groups reviewed", and its progress bar is full

#### Scenario: Card ignores the list filter
- **WHEN** the user marks 2 groups reviewed and then filters by unreviewed
- **THEN** the card still counts every group and shows 2 reviewed
